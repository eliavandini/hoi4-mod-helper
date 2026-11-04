//! Extraction of administrative boundaries from OpenStreetMap `.osm.pbf`
//! extracts, with an on-disk cache so the (slow) PBF scan only happens once.
//!
//! The extraction works in three passes over the file:
//! 1. decode every blob: collect `boundary=administrative` relations and
//!    remember which blobs contain ways / nodes,
//! 2. decode only way blobs: collect the node lists of member ways,
//! 3. decode only node blobs: collect coordinates of the needed nodes.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context as _, Result};
use osmpbf::{BlobDecode, BlobReader, ByteOffset, RelMemberType};
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};

use crate::util::Progress;

const CACHE_VERSION: u32 = 1;

/// Boundary geometry in geographic coordinates (1e-7 degree units, as in OSM).
#[derive(Serialize, Deserialize, Default)]
pub struct OsmBoundaries {
    /// Unique boundary ways, each tagged with the most important (lowest)
    /// admin level of the relations it belongs to.
    pub ways: Vec<BoundaryWay>,
    pub areas: Vec<AdminArea>,
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

/// Load boundaries from the cache if it is up to date, otherwise extract them.
pub fn load_boundaries(path: &Path, progress: &Progress) -> Result<OsmBoundaries> {
    let cache = cache_path(path);
    if let Some(cache) = &cache {
        if let Ok(bytes) = std::fs::read(cache) {
            progress.set_stage("Reading cache");
            match postcard::from_bytes::<CacheFile>(&bytes) {
                Ok(c) if c.version == CACHE_VERSION => {
                    log::info!("loaded OSM boundaries for {} from cache {}", path.display(), cache.display());
                    return Ok(c.data);
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
/// was made from is gone.
pub fn load_cache_file(path: &Path) -> Result<OsmBoundaries> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let c: CacheFile = postcard::from_bytes(&bytes).with_context(|| format!("decoding {}", path.display()))?;
    anyhow::ensure!(c.version == CACHE_VERSION, "{} has cache version {}, expected {CACHE_VERSION}", path.display(), c.version);
    Ok(c.data)
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
    node_blobs: Vec<u64>,
    way_blobs: Vec<u64>,
}

impl Pass1 {
    fn merge(mut self, mut o: Pass1) -> Pass1 {
        self.relations.append(&mut o.relations);
        self.node_blobs.append(&mut o.node_blobs);
        self.way_blobs.append(&mut o.way_blobs);
        self
    }
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
                    let mut admin = false;
                    let mut level = None;
                    let mut name = None;
                    let mut name_de = None;
                    for (k, v) in rel.tags() {
                        match k {
                            "boundary" => admin = v == "administrative",
                            "admin_level" => level = v.trim().parse::<u8>().ok(),
                            "name" => name = Some(v.to_string()),
                            "name:de" => name_de = Some(v.to_string()),
                            _ => {}
                        }
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
        "{}: {} admin relations, {} way blobs, {} node blobs ({:.1}s)",
        path.display(),
        pass1.relations.len(),
        pass1.way_blobs.len(),
        pass1.node_blobs.len(),
        t0.elapsed().as_secs_f32()
    );

    // ---- pass 2: member ways
    progress.set_stage("Reading boundary ways (2/3)");
    let mut way_level: FxHashMap<i64, u8> = FxHashMap::default();
    for r in &pass1.relations {
        for &w in &r.ways {
            way_level.entry(w).and_modify(|l| *l = (*l).min(r.level)).or_insert(r.level);
        }
    }
    let way_refs: FxHashMap<i64, Vec<i64>> = read_blobs(path, &pass1.way_blobs, progress, |block, out: &mut Vec<(i64, Vec<i64>)>| {
        for group in block.groups() {
            for way in group.ways() {
                if way_level.contains_key(&way.id()) {
                    out.push((way.id(), way.refs().collect()));
                }
            }
        }
    })?
    .into_iter()
    .collect();

    // ---- pass 3: node coordinates
    progress.set_stage("Reading boundary nodes (3/3)");
    let mut needed: FxHashSet<i64> = way_refs.values().flatten().copied().collect();
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
    let mut way_coords: FxHashMap<i64, Vec<Vec<[i32; 2]>>> = FxHashMap::default();
    let mut ways = Vec::new();
    for (id, refs) in &way_refs {
        // Split at nodes missing from the extract (boundaries clipped by the extract polygon).
        let mut parts: Vec<Vec<[i32; 2]>> = vec![Vec::new()];
        for r in refs {
            match coords.get(r) {
                Some(&c) => parts.last_mut().unwrap().push(c),
                None if !parts.last().unwrap().is_empty() => parts.push(Vec::new()),
                None => {}
            }
        }
        parts.retain(|p| p.len() >= 2);
        let level = way_level[id];
        for p in &parts {
            ways.push(BoundaryWay { level, coords: p.clone() });
        }
        way_coords.insert(*id, parts);
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

    let n_points: usize = ways.iter().map(|w| w.coords.len()).sum();
    log::info!(
        "{}: {} boundary ways ({} points), {} areas in {:.1}s",
        path.display(),
        ways.len(),
        n_points,
        areas.len(),
        t0.elapsed().as_secs_f32()
    );
    Ok(OsmBoundaries { ways, areas })
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
