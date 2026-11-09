//! Extraction of administrative boundaries, rivers and lake outlines from
//! OpenStreetMap `.osm.pbf` extracts, with an on-disk cache so the (slow) PBF
//! scan only happens once.
//!
//! The extraction works in three passes over the file:
//! 1. decode every blob: collect `boundary=administrative` relations and
//!    water multipolygons, and remember which blobs contain ways / nodes,
//! 2. decode only way blobs: collect the node lists of member ways, rivers,
//!    canals and closed water ways,
//! 3. decode only node blobs: collect coordinates of the needed nodes.
//!
//! Water: `waterway=river|canal` lines; the outlines of `natural=water`,
//! `waterway=riverbank` and `landuse=reservoir` areas (closed ways and
//! multipolygons), as river outlines (`water=river|canal|oxbow|...`) or lake
//! outlines (the rest). Ponds, basins and the like are left out, and so are
//! lakes under [`MIN_LAKE_M2`].

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context as _, Result};
use osmpbf::{BlobDecode, BlobReader, ByteOffset, RelMemberType};
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};

use crate::util::Progress;

const CACHE_VERSION: u32 = 2;

/// Lakes smaller than this (m²) are left out (ponds); river outlines under a
/// tenth of it too.
pub const MIN_LAKE_M2: f64 = 10_000.0;

/// Boundary geometry in geographic coordinates (1e-7 degree units, as in OSM).
#[derive(Serialize, Deserialize, Default)]
pub struct OsmBoundaries {
    /// Unique boundary ways, each tagged with the most important (lowest)
    /// admin level of the relations it belongs to.
    pub ways: Vec<BoundaryWay>,
    pub areas: Vec<AdminArea>,
    /// Rivers, canals and the outlines of rivers and lakes.
    pub water: Vec<WaterLine>,
}

/// What a water line is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Water {
    /// The outline of a lake or reservoir.
    Lake,
    /// The outline of a river's water area (wide rivers).
    RiverArea,
    Canal,
    /// A river's centre line.
    River,
}

impl Water {
    pub const ALL: [Water; 4] = [Water::River, Water::RiverArea, Water::Lake, Water::Canal];

    pub fn label(self) -> &'static str {
        match self {
            Water::River => "rivers",
            Water::RiverArea => "river outlines",
            Water::Lake => "lake outlines",
            Water::Canal => "canals",
        }
    }

    pub fn is_area(self) -> bool {
        matches!(self, Water::Lake | Water::RiverArea)
    }
}

#[derive(Serialize, Deserialize)]
pub struct WaterLine {
    pub kind: Water,
    /// (lon, lat) in 1e-7 degrees.
    pub coords: Vec<[i32; 2]>,
}

#[derive(Serialize, Deserialize)]
pub struct BoundaryWay {
    pub level: u8,
    /// (lon, lat) in 1e-7 degrees.
    pub coords: Vec<[i32; 2]>,
}

#[derive(Serialize, Deserialize)]
pub struct AdminArea {
    pub name: String,
    pub level: u8,
    /// Label position (label / admin_centre member, else bbox centre), 1e-7 deg.
    pub label: Option<[i32; 2]>,
    /// lon_min, lat_min, lon_max, lat_max in 1e-7 degrees.
    pub bbox: Option<[i32; 4]>,
}

#[derive(Serialize, Deserialize)]
struct CacheFile {
    version: u32,
    data: OsmBoundaries,
}

/// Caches of version 1 had no water.
#[derive(Deserialize)]
struct CacheFileV1 {
    #[allow(dead_code)]
    version: u32,
    ways: Vec<BoundaryWay>,
    areas: Vec<AdminArea>,
}

/// A cache file's contents: the current version, or version 1 (no water).
fn decode_cache(bytes: &[u8]) -> Result<(u32, OsmBoundaries)> {
    let (version, _) = postcard::take_from_bytes::<u32>(bytes)?;
    Ok(match version {
        1 => {
            let c: CacheFileV1 = postcard::from_bytes(bytes)?;
            (1, OsmBoundaries { ways: c.ways, areas: c.areas, water: Vec::new() })
        }
        _ => {
            let c: CacheFile = postcard::from_bytes(bytes)?;
            (c.version, c.data)
        }
    })
}

/// Load boundaries from the cache if it is up to date, otherwise extract them.
pub fn load_boundaries(path: &Path, progress: &Progress) -> Result<OsmBoundaries> {
    let cache = cache_path(path);
    if let Some(cache) = &cache {
        if let Ok(bytes) = std::fs::read(cache) {
            progress.set_stage("Reading cache");
            match decode_cache(&bytes) {
                Ok((CACHE_VERSION, data)) => {
                    log::info!("loaded OSM boundaries for {} from cache {}", path.display(), cache.display());
                    return Ok(data);
                }
                _ => log::warn!("ignoring stale/corrupt cache {}", cache.display()),
            }
        }
    }
    let data = extract(path, progress)?;
    if let Some(cache) = cache {
        let file = CacheFile { version: CACHE_VERSION, data };
        match postcard::to_stdvec(&file) {
            Ok(bytes) => {
                if let Some(dir) = cache.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                if let Err(e) = std::fs::write(&cache, bytes) {
                    log::warn!("could not write cache {}: {e}", cache.display());
                }
            }
            Err(e) => log::warn!("could not serialise cache: {e}"),
        }
        return Ok(file.data);
    }
    Ok(data)
}

/// Read a cache file (`osm-boundaries-*.bin`) directly, e.g. when the PBF it
/// was made from is gone. Caches of version 1 have no water.
pub fn load_cache_file(path: &Path) -> Result<OsmBoundaries> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let (version, data) = decode_cache(&bytes).with_context(|| format!("decoding {}", path.display()))?;
    anyhow::ensure!(version == CACHE_VERSION || version == 1, "{} has cache version {version}, expected {CACHE_VERSION}", path.display());
    Ok(data)
}

pub fn cache_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .map(|d| d.join("mod-helper"))
}

/// Cache file keyed by path, size and modification time of the PBF.
fn cache_path(path: &Path) -> Option<PathBuf> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    let canon = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let key = format!("{}|{}|{}|{}", canon.display(), meta.len(), mtime, CACHE_VERSION);
    // FNV-1a: stable across Rust versions, unlike DefaultHasher.
    let hash = key.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3));
    let stem = path.file_name()?.to_string_lossy().replace(".osm.pbf", "");
    Some(cache_dir()?.join(format!("osm-boundaries-{stem}-{hash:016x}.bin")))
}

struct Relation {
    level: u8,
    name: String,
    ways: Vec<i64>,
    label_node: Option<i64>,
}

#[derive(Default)]
struct Pass1 {
    relations: Vec<Relation>,
    /// Water multipolygons: what they are and their member ways.
    water_rels: Vec<(Water, Vec<i64>)>,
    node_blobs: Vec<u64>,
    way_blobs: Vec<u64>,
}

impl Pass1 {
    fn merge(mut self, mut o: Pass1) -> Pass1 {
        self.relations.append(&mut o.relations);
        self.water_rels.append(&mut o.water_rels);
        self.node_blobs.append(&mut o.node_blobs);
        self.way_blobs.append(&mut o.way_blobs);
        self
    }
}

/// What a way or relation with `tags` is as water. `closed`: a way whose
/// ends meet, or a multipolygon (only those can be lakes and river areas).
fn water_of<'a>(tags: impl IntoIterator<Item = (&'a str, &'a str)>, closed: bool) -> Option<Water> {
    let (mut waterway, mut natural, mut water, mut landuse) = ("", "", "", "");
    for (k, v) in tags {
        match k {
            "waterway" => waterway = v,
            "natural" => natural = v,
            "water" => water = v,
            "landuse" => landuse = v,
            _ => {}
        }
    }
    match waterway {
        "river" => return Some(Water::River),
        "canal" => return Some(Water::Canal),
        "riverbank" if closed => return Some(Water::RiverArea),
        _ => {}
    }
    if !closed || (natural != "water" && landuse != "reservoir") {
        return None;
    }
    match water {
        "river" | "stream" | "canal" | "oxbow" | "rapids" | "stream_pool" => Some(Water::RiverArea),
        "pond" | "basin" | "wastewater" | "reflecting_pool" | "swimming_pool" | "fish_pass" | "moat" | "ditch" | "drain" | "fountain" | "lock" | "pool" => None,
        _ => Some(Water::Lake),
    }
}

/// The area (m²) of a ring of (lon, lat) in 1e-7 degrees, near enough.
pub fn ring_m2(coords: &[[i32; 2]]) -> f64 {
    let Some(first) = coords.first() else { return 0.0 };
    let k = (first[1] as f64 * 1e-7).to_radians().cos();
    let m = |c: [i32; 2]| [(c[0] - first[0]) as f64 * 1e-7 * 111_320.0 * k, (c[1] - first[1]) as f64 * 1e-7 * 110_574.0];
    let mut a = 0.0;
    for w in coords.windows(2) {
        let (p, q) = (m(w[0]), m(w[1]));
        a += p[0] * q[1] - q[0] * p[1];
    }
    (a / 2.0).abs()
}

/// The area (m²) of a box [lon0, lat0, lon1, lat1] in 1e-7 degrees.
fn box_m2(b: [i32; 4]) -> f64 {
    ring_m2(&[[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]], [b[0], b[1]]])
}

/// The smallest area kept of a kind of water (m²).
fn min_m2(w: Water) -> f64 {
    if w == Water::Lake { MIN_LAKE_M2 } else { MIN_LAKE_M2 / 10.0 }
}

/// A way's node list as coordinates, split where nodes are missing from the
/// extract (ways clipped by the extract polygon); pieces of 2 points or more.
fn parts_of(refs: &[i64], coords: &FxHashMap<i64, [i32; 2]>) -> Vec<Vec<[i32; 2]>> {
    let mut parts: Vec<Vec<[i32; 2]>> = vec![Vec::new()];
    for r in refs {
        match coords.get(r) {
            Some(&c) => parts.last_mut().unwrap().push(c),
            None if !parts.last().unwrap().is_empty() => parts.push(Vec::new()),
            None => {}
        }
    }
    parts.retain(|p| p.len() >= 2);
    parts
}

/// What pass 2 keeps of a way.
enum WayOut {
    /// A member of a boundary or water relation.
    Member(i64, Vec<i64>),
    /// A river or canal, or a closed water way.
    Water(i64, Water, Vec<i64>),
}

fn extract(path: &Path, progress: &Progress) -> Result<OsmBoundaries> {
    let total = std::fs::metadata(path)?.len().max(1);
    let t0 = std::time::Instant::now();

    // ---- pass 1: relations + blob classification
    progress.set_stage("Scanning relations (1/3)");
    let max_offset = AtomicU64::new(0);
    let reader = BlobReader::seekable_from_path(path).with_context(|| format!("cannot open {}", path.display()))?;
    let pass1 = reader
        .par_bridge()
        .map(|blob| -> Result<Pass1> {
            let blob = blob?;
            let offset = blob.offset().map(|o| o.0).unwrap_or(0);
            let seen = max_offset.fetch_max(offset, Ordering::Relaxed).max(offset);
            progress.set(seen as f32 / total as f32);
            let mut out = Pass1::default();
            let BlobDecode::OsmData(block) = blob.decode()? else { return Ok(out) };
            let (mut has_nodes, mut has_ways) = (false, false);
            for group in block.groups() {
                has_nodes |= group.dense_nodes().next().is_some() || group.nodes().next().is_some();
                has_ways |= group.ways().next().is_some();
                for rel in group.relations() {
                    let tags: Vec<(&str, &str)> = rel.tags().collect();
                    let mut admin = false;
                    let mut multipolygon = false;
                    let mut level = None;
                    let mut name = None;
                    let mut name_de = None;
                    for &(k, v) in &tags {
                        match k {
                            "boundary" => admin = v == "administrative",
                            "admin_level" => level = v.trim().parse::<u8>().ok(),
                            "name" => name = Some(v.to_string()),
                            "name:de" => name_de = Some(v.to_string()),
                            "type" => multipolygon = v == "multipolygon",
                            _ => {}
                        }
                    }
                    if let Some(w) = water_of(tags.iter().copied(), true).filter(|w| multipolygon && w.is_area()) {
                        let ways = rel.members().filter(|m| m.member_type == RelMemberType::Way && matches!(m.role().unwrap_or(""), "outer" | "inner" | "")).map(|m| m.member_id).collect();
                        out.water_rels.push((w, ways));
                    }
                    let (true, Some(level)) = (admin, level) else { continue };
                    let mut ways = Vec::new();
                    let (mut label, mut centre) = (None, None);
                    for m in rel.members() {
                        match m.member_type {
                            RelMemberType::Way => ways.push(m.member_id),
                            RelMemberType::Node => match m.role().unwrap_or("") {
                                "label" => label = Some(m.member_id),
                                "admin_centre" => centre = Some(m.member_id),
                                _ => {}
                            },
                            RelMemberType::Relation => {}
                        }
                    }
                    out.relations.push(Relation {
                        level,
                        name: name.or(name_de).unwrap_or_default(),
                        ways,
                        label_node: label.or(centre),
                    });
                }
            }
            if has_nodes {
                out.node_blobs.push(offset);
            }
            if has_ways {
                out.way_blobs.push(offset);
            }
            Ok(out)
        })
        .try_reduce(Pass1::default, |a, b| Ok(a.merge(b)))?;
    log::info!(
        "{}: {} admin relations, {} water multipolygons, {} way blobs, {} node blobs ({:.1}s)",
        path.display(),
        pass1.relations.len(),
        pass1.water_rels.len(),
        pass1.way_blobs.len(),
        pass1.node_blobs.len(),
        t0.elapsed().as_secs_f32()
    );

    // ---- pass 2: member ways, rivers and water ways
    progress.set_stage("Reading boundary and water ways (2/3)");
    let mut way_level: FxHashMap<i64, u8> = FxHashMap::default();
    for r in &pass1.relations {
        for &w in &r.ways {
            way_level.entry(w).and_modify(|l| *l = (*l).min(r.level)).or_insert(r.level);
        }
    }
    let water_members: FxHashSet<i64> = pass1.water_rels.iter().flat_map(|r| r.1.iter().copied()).collect();
    let found = read_blobs(path, &pass1.way_blobs, progress, |block, out: &mut Vec<WayOut>| {
        for group in block.groups() {
            for way in group.ways() {
                let id = way.id();
                let member = way_level.contains_key(&id) || water_members.contains(&id);
                let candidate = water_of(way.tags(), true);
                if !member && candidate.is_none() {
                    continue;
                }
                let refs: Vec<i64> = way.refs().collect();
                let closed = refs.len() >= 4 && refs.first() == refs.last();
                if let Some(k) = candidate.filter(|k| !k.is_area() || closed) {
                    out.push(WayOut::Water(id, k, refs.clone()));
                }
                if member {
                    out.push(WayOut::Member(id, refs));
                }
            }
        }
    })?;
    let mut way_refs: FxHashMap<i64, Vec<i64>> = FxHashMap::default();
    let mut water_ways: Vec<(i64, Water, Vec<i64>)> = Vec::new();
    for f in found {
        match f {
            WayOut::Member(id, refs) => {
                way_refs.insert(id, refs);
            }
            WayOut::Water(id, k, refs) => water_ways.push((id, k, refs)),
        }
    }

    // ---- pass 3: node coordinates
    progress.set_stage("Reading boundary and water nodes (3/3)");
    let mut needed: FxHashSet<i64> = way_refs.values().flatten().copied().collect();
    needed.extend(water_ways.iter().flat_map(|w| w.2.iter().copied()));
    needed.extend(pass1.relations.iter().filter_map(|r| r.label_node));
    let coords: FxHashMap<i64, [i32; 2]> = read_blobs(path, &pass1.node_blobs, progress, |block, out: &mut Vec<(i64, [i32; 2])>| {
        for group in block.groups() {
            for n in group.dense_nodes() {
                if needed.contains(&n.id()) {
                    out.push((n.id(), [n.decimicro_lon(), n.decimicro_lat()]));
                }
            }
            for n in group.nodes() {
                if needed.contains(&n.id()) {
                    out.push((n.id(), [n.decimicro_lon(), n.decimicro_lat()]));
                }
            }
        }
    })?
    .into_iter()
    .collect();

    // ---- assemble
    progress.set_stage("Assembling");
    let way_coords: FxHashMap<i64, Vec<Vec<[i32; 2]>>> = way_refs.iter().map(|(id, refs)| (*id, parts_of(refs, &coords))).collect();
    let mut ways = Vec::new();
    for (id, parts) in &way_coords {
        if let Some(&level) = way_level.get(id) {
            ways.extend(parts.iter().map(|p| BoundaryWay { level, coords: p.clone() }));
        }
    }
    // Finer levels first so coarser (more important) borders are drawn on top.
    ways.sort_by_key(|w| std::cmp::Reverse(w.level));

    let mut areas: Vec<AdminArea> = pass1
        .relations
        .iter()
        .map(|r| {
            let mut bb = [i32::MAX, i32::MAX, i32::MIN, i32::MIN];
            for w in &r.ways {
                for c in way_coords.get(w).into_iter().flatten().flatten() {
                    bb = [bb[0].min(c[0]), bb[1].min(c[1]), bb[2].max(c[0]), bb[3].max(c[1])];
                }
            }
            let bbox = (bb[0] <= bb[2]).then_some(bb);
            let label = r
                .label_node
                .and_then(|n| coords.get(&n).copied())
                .or_else(|| bbox.map(|b| [((b[0] as i64 + b[2] as i64) / 2) as i32, ((b[1] as i64 + b[3] as i64) / 2) as i32]));
            AdminArea { name: r.name.clone(), level: r.level, label, bbox }
        })
        .collect();
    areas.retain(|a| a.bbox.is_some());

    let water = assemble_water(&water_ways, &pass1.water_rels, &way_coords, &coords);
    let n_points: usize = ways.iter().map(|w| w.coords.len()).sum();
    let n_water: usize = water.iter().map(|w| w.coords.len()).sum();
    log::info!(
        "{}: {} boundary ways ({} points), {} areas, {} water lines ({} points) in {:.1}s",
        path.display(),
        ways.len(),
        n_points,
        areas.len(),
        water.len(),
        n_water,
        t0.elapsed().as_secs_f32()
    );
    Ok(OsmBoundaries { ways, areas, water })
}

/// The water lines: rivers and canals, the outlines of closed water ways
/// (ponds under [`min_m2`] left out), and the member ways of water
/// multipolygons whose box is not under it. Each way once; lakes first,
/// rivers last (drawn on top).
fn assemble_water(
    tagged: &[(i64, Water, Vec<i64>)],
    rels: &[(Water, Vec<i64>)],
    member_coords: &FxHashMap<i64, Vec<Vec<[i32; 2]>>>,
    coords: &FxHashMap<i64, [i32; 2]>,
) -> Vec<WaterLine> {
    let mut used: FxHashSet<i64> = FxHashSet::default();
    let mut out = Vec::new();
    for (id, kind, refs) in tagged {
        let parts = parts_of(refs, coords);
        // a whole ring under the size: a pond (a clipped one stays)
        if kind.is_area() && parts.len() == 1 && parts[0].first() == parts[0].last() && ring_m2(&parts[0]) < min_m2(*kind) {
            continue;
        }
        if used.insert(*id) {
            out.extend(parts.into_iter().map(|c| WaterLine { kind: *kind, coords: c }));
        }
    }
    for (kind, members) in rels {
        let mut bb = [i32::MAX, i32::MAX, i32::MIN, i32::MIN];
        for c in members.iter().filter_map(|w| member_coords.get(w)).flatten().flatten() {
            bb = [bb[0].min(c[0]), bb[1].min(c[1]), bb[2].max(c[0]), bb[3].max(c[1])];
        }
        if bb[0] > bb[2] || box_m2(bb) < min_m2(*kind) {
            continue;
        }
        for w in members {
            if let Some(parts) = member_coords.get(w).filter(|_| used.insert(*w)) {
                out.extend(parts.iter().map(|c| WaterLine { kind: *kind, coords: c.clone() }));
            }
        }
    }
    out.sort_by_key(|w| w.kind);
    out
}

/// Decode the blobs at `offsets` in parallel and collect what `f` extracts.
fn read_blobs<T: Send>(
    path: &Path,
    offsets: &[u64],
    progress: &Progress,
    f: impl Fn(&osmpbf::PrimitiveBlock, &mut Vec<T>) + Sync,
) -> Result<Vec<T>> {
    let mut sorted = offsets.to_vec();
    sorted.sort_unstable();
    let n = sorted.len().max(1);
    let mut reader = BlobReader::seekable_from_path(path)?;
    let done = AtomicU64::new(0);
    let blobs = sorted.into_iter().map(move |o| reader.blob_from_offset(ByteOffset(o)));
    blobs
        .par_bridge()
        .map(|blob| -> Result<Vec<T>> {
            let mut out = Vec::new();
            if let BlobDecode::OsmData(block) = blob?.decode()? {
                f(&block, &mut out);
            }
            let d = done.fetch_add(1, Ordering::Relaxed) + 1;
            progress.set(d as f32 / n as f32);
            Ok(out)
        })
        .try_reduce(Vec::new, |mut a, mut b| {
            a.append(&mut b);
            Ok(a)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags<'a>(t: &'a [(&'a str, &'a str)]) -> impl Iterator<Item = (&'a str, &'a str)> {
        t.iter().copied()
    }

    #[test]
    fn what_is_water() {
        assert_eq!(water_of(tags(&[("waterway", "river"), ("name", "Etsch")]), false), Some(Water::River));
        assert_eq!(water_of(tags(&[("waterway", "canal")]), false), Some(Water::Canal));
        assert_eq!(water_of(tags(&[("waterway", "stream")]), false), None);
        // areas only when closed
        assert_eq!(water_of(tags(&[("natural", "water"), ("water", "lake")]), true), Some(Water::Lake));
        assert_eq!(water_of(tags(&[("natural", "water"), ("water", "lake")]), false), None);
        assert_eq!(water_of(tags(&[("natural", "water")]), true), Some(Water::Lake));
        assert_eq!(water_of(tags(&[("landuse", "reservoir")]), true), Some(Water::Lake));
        assert_eq!(water_of(tags(&[("natural", "water"), ("water", "river")]), true), Some(Water::RiverArea));
        assert_eq!(water_of(tags(&[("waterway", "riverbank")]), true), Some(Water::RiverArea));
        // ponds and basins are not
        assert_eq!(water_of(tags(&[("natural", "water"), ("water", "pond")]), true), None);
        assert_eq!(water_of(tags(&[("natural", "water"), ("water", "wastewater")]), true), None);
        assert_eq!(water_of(tags(&[("natural", "wood")]), true), None);
    }

    /// A square ring of `side` 1e-7 degrees at about 46° N.
    fn square(x: i32, y: i32, side: i32) -> Vec<[i32; 2]> {
        vec![[x, y], [x + side, y], [x + side, y + side], [x, y + side], [x, y]]
    }

    #[test]
    fn areas_in_square_metres() {
        // 0.001° × 0.001° at 46° N: about 77 m × 111 m
        let a = ring_m2(&square(112_000_000, 460_000_000, 10_000));
        assert!((8_400.0..8_700.0).contains(&a), "{a}");
        assert_eq!(ring_m2(&[]), 0.0);
        assert!((box_m2([112_000_000, 460_000_000, 112_010_000, 460_010_000]) - a).abs() < 1.0);
    }

    #[test]
    fn small_water_is_left_out_and_each_way_drawn_once() {
        // nodes 1.. on squares: a pond of 0.001° (under 1 ha), a lake of 0.002°
        let mut coords: FxHashMap<i64, [i32; 2]> = FxHashMap::default();
        let mut ring = |id0: i64, sq: Vec<[i32; 2]>| -> Vec<i64> {
            let ids: Vec<i64> = (0..4).map(|k| id0 + k).collect();
            for (k, c) in sq[..4].iter().enumerate() {
                coords.insert(id0 + k as i64, *c);
            }
            let mut r = ids.clone();
            r.push(ids[0]);
            r
        };
        let pond = ring(10, square(112_000_000, 460_000_000, 10_000));
        let lake = ring(20, square(112_100_000, 460_000_000, 20_000));
        let river: Vec<i64> = vec![20, 21];
        let tagged = vec![(1, Water::Lake, pond), (2, Water::Lake, lake.clone()), (3, Water::River, river)];
        // a multipolygon of the lake's way (drawn once) and of a way of its own
        let mut members: FxHashMap<i64, Vec<Vec<[i32; 2]>>> = FxHashMap::default();
        members.insert(2, parts_of(&lake, &coords));
        members.insert(4, vec![square(112_200_000, 460_000_000, 20_000)]);
        let rels = vec![(Water::Lake, vec![2, 4])];
        let w = assemble_water(&tagged, &rels, &members, &coords);
        assert_eq!(w.iter().map(|l| l.kind).collect::<Vec<_>>(), [Water::Lake, Water::Lake, Water::River]);
        assert_eq!(w[0].coords.len(), 5);
        // a way clipped by the extract (a node missing): two pieces, kept
        let clipped = vec![(5, Water::Lake, vec![10, 11, 99, 12, 13, 10])];
        assert_eq!(assemble_water(&clipped, &[], &FxHashMap::default(), &coords).len(), 2);
    }

    #[test]
    fn caches_of_version_1_still_read() {
        #[derive(Serialize)]
        struct V1<'a> {
            version: u32,
            ways: &'a [BoundaryWay],
            areas: &'a [AdminArea],
        }
        let ways = [BoundaryWay { level: 8, coords: vec![[1, 2], [3, 4]] }];
        let areas = [AdminArea { name: "Bozen".into(), level: 8, label: Some([2, 3]), bbox: Some([1, 2, 3, 4]) }];
        let bytes = postcard::to_stdvec(&V1 { version: 1, ways: &ways, areas: &areas }).unwrap();
        let (v, d) = decode_cache(&bytes).unwrap();
        assert!(v == 1 && d.ways.len() == 1 && d.areas[0].name == "Bozen" && d.water.is_empty());
        let now = CacheFile { version: CACHE_VERSION, data: OsmBoundaries { ways: Vec::new(), areas: Vec::new(), water: vec![WaterLine { kind: Water::River, coords: vec![[5, 6], [7, 8]] }] } };
        let (v, d) = decode_cache(&postcard::to_stdvec(&now).unwrap()).unwrap();
        assert!(v == CACHE_VERSION && d.water.len() == 1 && d.water[0].kind == Water::River);
    }

    #[test]
    #[ignore = "needs the OSM extracts (MOD_HELPER_TEST_MAPS)"]
    fn rivers_and_lakes_of_the_real_extract() {
        let path = crate::util::test_maps_dir().join("osm regions/nord-est-261101.osm.pbf");
        let b = extract(&path, &Progress::default()).unwrap();
        let count = |k: Water| b.water.iter().filter(|w| w.kind == k).count();
        assert!(count(Water::River) > 500 && count(Water::Lake) > 1000 && count(Water::RiverArea) > 500, "{:?}", Water::ALL.map(count));
        // the Kalterer See (46.38° N, 11.26° E): a lake outline around it
        let at = [112_600_000, 463_800_000];
        let around = |w: &&WaterLine| {
            let (x, y): (Vec<i32>, Vec<i32>) = w.coords.iter().map(|c| (c[0], c[1])).unzip();
            w.kind == Water::Lake && x.iter().min() < Some(&at[0]) && x.iter().max() > Some(&at[0]) && y.iter().min() < Some(&at[1]) && y.iter().max() > Some(&at[1])
        };
        assert!(b.water.iter().any(|w| around(&w)), "no lake around the Kalterer See");
        // the Etsch near Bozen (46.47° N, 11.32° E): a river within 2 km
        let near = |c: &[i32; 2]| (c[0] - 113_200_000).abs() < 300_000 && (c[1] - 464_700_000).abs() < 200_000;
        assert!(b.water.iter().any(|w| w.kind == Water::River && w.coords.iter().any(near)));
    }
}
