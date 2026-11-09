//! The HOI4 mod made from a map project (formerly make_map_mod.py), and the
//! checks of a built mod (formerly validate_mod.py, see [`validate`]).
//!
//! Inputs, in the map folder: provinces.bmp + provinces.json (as the editor
//! saves them), dem_section.tif (heights in metres) and landcover_smooth.bmp
//! (CLC+ land cover classes). Palettes, texture sizes and the game version
//! come from the HOI4 install; the colours of the terrain colour map, the tree
//! type and the city look are sampled from the vanilla map around Innsbruck.
//!
//! Every nation (one per city, town and village) becomes a country: a tag
//! from its name, a procedural flag, a map colour unlike its neighbours',
//! name lists by language, generic history (neutral, the starting techs of a
//! vanilla minor, a militia) and no focus tree or leader of its own, so the
//! game gives it the generic focus tree and generated leaders. Values set in
//! the editor (terrain, manpower, state category, buildings, resources,
//! victory points, tags, colours, ideology, units) win over derived ones.
//!
//! Writes `<mods>/<folder>/` and `<mods>/<folder>.mod`. The folder is built
//! next to the old one and swapped in at the end; a folder of that name is
//! only replaced if mod-helper (or make_map_mod.py) made it.

pub mod ambient;
pub mod bitmaps;
pub mod countries;
pub mod img;
pub mod meta;
pub mod rail;
pub mod terrain;
pub mod validate;
pub mod vanilla;

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use rayon::prelude::*;
use regex::{Captures, Regex};
use rustc_hash::FxHashSet;

use crate::editor::model::{self, Id, Kind, Nation, State};
use crate::editor::{rivers, supply};
use crate::raster::{self, Pixels};
use crate::util::Progress;
use countries::{Rng, ascii_name, place_rank, short_name};
use img::Filter;
pub use meta::ModMeta;
use vanilla::WATER;

pub const MARKER: &str = ".made_by_mod_helper";
const OLD_MARKER: &str = ".made_by_make_map_mod";

use bitmaps::{SEALED, T_FOREST, T_FOREST_B, T_OCEAN};

/// Name lists (first names, surnames) are copied from these vanilla tags.
fn names_from(language: &str) -> &'static str {
    match language {
        "it" => "ITA",
        "rm" => "SWI",
        _ => "AUS",
    }
}
fn militia(language: &str) -> &'static str {
    match language {
        "it" | "rm" => "Milizia",
        _ => "Landsturm",
    }
}
fn divisions(place: &str) -> u32 {
    match place {
        "city" => 3,
        "town" => 2,
        _ => 1,
    }
}

/// Building slots of the vanilla state categories (common/state_category).
pub const CATEGORY_SLOTS: [(&str, u32); 13] = [
    ("wasteland", 0),
    ("enclave", 0),
    ("tiny_island", 0),
    ("small_island", 1),
    ("pastoral", 1),
    ("rural", 2),
    ("town", 4),
    ("large_town", 5),
    ("city", 6),
    ("large_city", 8),
    ("metropolis", 10),
    ("megalopolis", 12),
    ("large_island", 3),
];
/// State buildings that take a building slot (shares_slots = yes).
const SLOT_BUILDINGS: [&str; 7] = ["industrial_complex", "arms_factory", "dockyard", "synthetic_refinery", "fuel_silo", "rocket_site", "nuclear_reactor"];
pub const IDEOLOGIES: [&str; 4] = ["democratic", "fascism", "communism", "neutrality"];
/// State category by the sealed (urban) km² of the state: (below, category, slots).
const STATE_CATEGORIES: [(f64, &str, u32); 7] = [
    (0.15, "pastoral", 1),
    (0.5, "rural", 2),
    (1.5, "town", 4),
    (3.0, "large_town", 5),
    (6.0, "city", 6),
    (10.0, "large_city", 8),
    (f64::INFINITY, "metropolis", 10),
];
/// Building positions per state (vanilla buildings.txt has these per state).
const STATE_BUILDINGS: [(&str, usize); 10] = [
    ("arms_factory", 6),
    ("industrial_complex", 6),
    ("anti_air_building", 3),
    ("air_base", 1),
    ("radar_station", 1),
    ("nuclear_reactor_spawn", 1),
    ("fuel_silo", 1),
    ("rocket_site_spawn", 1),
    ("synthetic_refinery", 1),
    ("stronghold_network", 1),
];
const PROVINCE_BUILDINGS: [&str; 3] = ["supply_node", "bunker", "special_project_facility_spawn"];
const COAST_BUILDINGS: [&str; 5] = ["naval_base_spawn", "coastal_bunker", "floating_harbor", "naval_supply_hub", "naval_headquarters"];

const WEATHER: &str = "\tweather={
\t\tperiod={
\t\t\tbetween={ 0.0 30.11 }
\t\t\ttemperature={ -6.0 12.0 }
\t\t\tno_phenomenon=0.500
\t\t\train_light=1.000
\t\t\train_heavy=0.150
\t\t\tsnow=0.200
\t\t\tblizzard=0.000
\t\t\tarctic_water=0.000
\t\t\tmud=0.300
\t\t\tsandstorm=0.000
\t\t\tmin_snow_level=0.000
\t\t}
\t}";

/// The HOI4 install and the folder mods go to.
#[derive(Clone, Debug)]
pub struct Dirs {
    pub game: PathBuf,
    pub mods: PathBuf,
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

pub fn default_game_dir() -> PathBuf {
    let steam = home().join(".local/share/Steam/steamapps/common/Hearts of Iron IV");
    let alt = home().join(".steam/steam/steamapps/common/Hearts of Iron IV");
    if !steam.exists() && alt.exists() { alt } else { steam }
}

pub fn default_mods_dir() -> PathBuf {
    home().join(".local/share/Paradox Interactive/Hearts of Iron IV/mod")
}

impl Default for Dirs {
    fn default() -> Self {
        Dirs { game: default_game_dir(), mods: default_mods_dir() }
    }
}

// ------------------------------------------------------------------ helpers

fn fmt2(v: f64) -> String {
    format!("{v:.2}")
}

fn write(path: &Path, text: &str) -> Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).with_context(|| format!("cannot create {}", d.display()))?;
    }
    std::fs::write(path, text).with_context(|| format!("cannot write {}", path.display()))
}

fn write_crlf(path: &Path, text: &str) -> Result<()> {
    write(path, &text.replace('\n', "\r\n"))
}

fn write_bom(path: &Path, text: &str) -> Result<()> {
    write(path, &format!("\u{feff}{text}"))
}

/// A vanilla texture size scaled by the same factor as the map.
fn scaled(size: (u32, u32), vanilla_map: (u32, u32), map: (usize, usize)) -> (usize, usize) {
    let f = |s: u32, v: u32, m: usize| ((s as f64 * m as f64 / v as f64).round_ties_even() as usize).max(1);
    (f(size.0, vanilla_map.0, map.0), f(size.1, vanilla_map.1, map.1))
}

/// The first item with the largest key (Python's max()).
/// Slope (degrees) of every pixel of a height map with `px_m` metres per
/// pixel, as the build's terrain rules see it (numpy-style gradient).
pub fn slope_degrees(dem: &[f32], w: usize, h: usize, px_m: f64) -> Vec<f32> {
    let (gx, gy) = gradient(dem, w, h, px_m);
    gx.par_iter().zip(&gy).map(|(a, b)| a.hypot(*b).atan().to_degrees()).collect()
}

/// Infrastructure of a state when none is set: 3 with 1.5 km² or more
/// built-up land, else 2 (0 if impassable).
pub fn default_infrastructure(impassable: bool, urban_km2: f64) -> u32 {
    if impassable {
        0
    } else if urban_km2 >= 1.5 {
        3
    } else {
        2
    }
}

fn first_max<T: Copy, K: PartialOrd>(items: impl IntoIterator<Item = T>, key: impl Fn(T) -> K) -> Option<T> {
    let mut best: Option<(T, K)> = None;
    for x in items {
        let k = key(x);
        if best.as_ref().is_none_or(|(_, b)| k > *b) {
            best = Some((x, k));
        }
    }
    best.map(|b| b.0)
}

fn pair(a: Id, b: Id) -> (Id, Id) {
    (a.min(b), a.max(b))
}

/// The top-level `type={...}` blocks of vanilla's map/ambient_object.txt.
fn vanilla_blocks(text: &str) -> Vec<&str> {
    let (mut out, mut depth, mut start) = (Vec::new(), 0, 0);
    for (i, ch) in text.char_indices() {
        if ch == '{' {
            if depth == 0 {
                start = text[..i].rfind("type").unwrap_or(0);
            }
            depth += 1;
        } else if ch == '}' {
            depth -= 1;
            if depth == 0 {
                out.push(&text[start..=i]);
            }
        }
    }
    out
}

/// map/ambient_object.txt: the wind sound, ocean sounds at `water` (x, z) and,
/// if `frame`, vanilla's metal frame with the logo.
fn ambient_objects(game: &Path, frame: bool, water: &[(f64, f64)]) -> Result<String> {
    let obj = |name: &str, pos: &str| format!("\tobject={{\n\t\tname=\"{name}\"\n\t\tposition={{\n\t\t\t{pos} \n\t\t}}\n\t\trotation={{\n\t\t\t0 0 0 \n\t\t}}\n\t}}\n");
    let mut out = format!("type={{\n\ttype=\"ambient_wind_entity\"\n\tuse_animation=no\n\talways_visible=yes\n{}}}\n", obj("ambient_wind", "0 0 0"));
    if !water.is_empty() {
        out += "type={\n\ttype=\"ambient_water_entity\"\n\tuse_animation=no\n\tscale=10.000000\n";
        for &(x, z) in water {
            out += &obj("ambient_water", &format!("{} 10 {}", fmt2(x), fmt2(z)));
        }
        out += "}\n";
    }
    if frame {
        let text = vanilla::read_text(&game.join("map/ambient_object.txt"))?;
        let re = Regex::new(r#"type\s*=\s*"frame_border"#).unwrap();
        let blocks: Vec<&str> = vanilla_blocks(&text).into_iter().filter(|b| re.is_match(b)).collect();
        if blocks.is_empty() {
            bail!("no frame_border entities in the game's map/ambient_object.txt");
        }
        for b in blocks {
            out += b;
            out.push('\n');
        }
    }
    Ok(out)
}

/// Per province: pixels, pixels by kind of ground, bounding box.
#[derive(Clone, Copy)]
struct PStats {
    size: u64,
    sealed: u64,
    bbox: [usize; 4],
}

impl Default for PStats {
    fn default() -> Self {
        PStats { size: 0, sealed: 0, bbox: [usize::MAX, usize::MAX, 0, 0] }
    }
}

/// For each coastal province: (sea province, land pixel, sea pixel) in the
/// middle of its longest stretch of coast (rows / columns).
#[allow(clippy::type_complexity)]
fn coast_points(p: &[Id], is_sea: &[bool], w: usize, h: usize) -> BTreeMap<Id, (Id, (usize, usize), (usize, usize))> {
    // land id, sea id, land r, c, sea r, c; in the order numpy finds them
    let mut pairs: Vec<(Id, Id, usize, usize, usize, usize)> = Vec::new();
    for (dr, dc) in [(0usize, 1usize), (1, 0)] {
        let (rh, rw) = (h - dr, w - dc);
        let at = |r: usize, c: usize| p[r * w + c];
        for r in 0..rh {
            for c in 0..rw {
                let (a, b) = (at(r, c), at(r + dr, c + dc));
                if !is_sea[a as usize] && is_sea[b as usize] {
                    pairs.push((a, b, r, c, r + dr, c + dc));
                }
            }
        }
        for r in 0..rh {
            for c in 0..rw {
                let (a, b) = (at(r, c), at(r + dr, c + dc));
                if is_sea[a as usize] && !is_sea[b as usize] {
                    pairs.push((b, a, r + dr, c + dc, r, c));
                }
            }
        }
    }
    let mut by_land: BTreeMap<Id, Vec<usize>> = BTreeMap::new();
    for (i, x) in pairs.iter().enumerate() {
        by_land.entry(x.0).or_default().push(i);
    }
    let mut out = BTreeMap::new();
    for (land, idx) in by_land {
        let mut count: BTreeMap<Id, usize> = BTreeMap::new();
        for &i in &idx {
            *count.entry(pairs[i].1).or_default() += 1;
        }
        let sea = first_max(count.iter().map(|(&s, &c)| (s, c)), |(_, c)| c).unwrap().0;
        let sel: Vec<usize> = idx.into_iter().filter(|&i| pairs[i].1 == sea).collect();
        let n = sel.len() as f64;
        let mr = sel.iter().map(|&i| pairs[i].2 as f64).sum::<f64>() / n;
        let mc = sel.iter().map(|&i| pairs[i].3 as f64).sum::<f64>() / n;
        let mut best = (f64::INFINITY, sel[0]);
        for &i in &sel {
            let d = (pairs[i].2 as f64 - mr).powi(2) + (pairs[i].3 as f64 - mc).powi(2);
            if d < best.0 {
                best = (d, i);
            }
        }
        let x = pairs[best.1];
        out.insert(land, (sea, (x.2, x.3), (x.4, x.5)));
    }
    out
}

/// numpy.gradient of a float image with uniform spacing: (d/dx, d/dy).
fn gradient(f: &[f32], w: usize, h: usize, step: f64) -> (Vec<f32>, Vec<f32>) {
    let (s1, s2) = (step as f32, (2.0 * step) as f32);
    let gx: Vec<f32> = (0..w * h)
        .into_par_iter()
        .map(|i| {
            let x = i % w;
            if w < 2 {
                0.0
            } else if x == 0 {
                (f[i + 1] - f[i]) / s1
            } else if x == w - 1 {
                (f[i] - f[i - 1]) / s1
            } else {
                (f[i + 1] - f[i - 1]) / s2
            }
        })
        .collect();
    let gy: Vec<f32> = (0..w * h)
        .into_par_iter()
        .map(|i| {
            let y = i / w;
            if h < 2 {
                0.0
            } else if y == 0 {
                (f[i + w] - f[i]) / s1
            } else if y == h - 1 {
                (f[i] - f[i - w]) / s1
            } else {
                (f[i + w] - f[i - w]) / s2
            }
        })
        .collect();
    (gx, gy)
}

// ------------------------------------------------------------------ build

/// Build the mod from the saved map in `map_dir`; returns the mod folder.
pub fn build(map_dir: &Path, dirs: &Dirs, log: &dyn Fn(&str)) -> Result<PathBuf> {
    let t0 = std::time::Instant::now();
    log("reading the map");
    let progress = Progress::default();
    let model::Loaded { data: d, labels, extra, .. } = model::load(map_dir, &progress)?;
    let meta = d.mod_meta();
    meta.check()?;
    let n = d.provinces.count();
    if d.provinces.ids() != (1..=n as Id).collect::<Vec<_>>() {
        bail!("province ids in provinces.json are not 1..n (save the map in the editor)");
    }
    for (_, s) in d.states.iter() {
        match s.nation {
            None => bail!("state {} ({}) belongs to no nation", s.id, s.name),
            Some(nid) if d.nations.get(nid).is_none() => bail!("state {} ({}) belongs to nation {nid}, which does not exist", s.id, s.name),
            _ => {}
        }
    }
    for (_, r) in d.regions.iter() {
        if r.provinces.is_empty() {
            bail!("strategic region {} ({}) has no provinces", r.id, r.name);
        }
    }

    // the new folder is built next to the old one, which is replaced at the end
    let mod_dir = dirs.mods.join(&meta.folder);
    if mod_dir.exists() && !mod_dir.join(MARKER).exists() && !mod_dir.join(OLD_MARKER).exists() {
        bail!("{} exists and was not made by mod-helper; choose another mod folder name", mod_dir.display());
    }
    let out = dirs.mods.join(format!("{}.building", meta.folder));
    if out.exists() {
        std::fs::remove_dir_all(&out).with_context(|| format!("cannot remove {}", out.display()))?;
    }
    let result = build_into(&out, map_dir, dirs, &meta, &d, &labels, &extra, log);
    let report = match result {
        Ok(r) => r,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&out);
            return Err(e);
        }
    };
    if mod_dir.exists() {
        std::fs::remove_dir_all(&mod_dir).with_context(|| format!("cannot remove the old {}", mod_dir.display()))?;
    }
    std::fs::rename(&out, &mod_dir).with_context(|| format!("cannot rename {} to {}", out.display(), mod_dir.display()))?;
    // descriptor.mod and <folder>.mod name the final folder
    let desc = std::fs::read_to_string(mod_dir.join("descriptor.mod"))?;
    write(&dirs.mods.join(format!("{}.mod", meta.folder)), &format!("{desc}path=\"{}\"\n", mod_dir.display()))?;
    log(&format!("wrote {}", mod_dir.display()));
    for l in report {
        log(&format!("  {l}"));
    }
    log(&format!("  built in {:.1} s", t0.elapsed().as_secs_f32()));
    Ok(mod_dir)
}

#[allow(clippy::too_many_arguments)]
fn build_into(
    out: &Path,
    map_dir: &Path,
    dirs: &Dirs,
    meta: &ModMeta,
    d: &model::MapData,
    labels: &model::Labels,
    extra: &model::Bitmaps,
    log: &dyn Fn(&str),
) -> Result<Vec<String>> {
    let (pmap, w, h): (&[Id], usize, usize) = (&labels.px, labels.w as usize, labels.h as usize);
    let n = d.provinces.count();
    let provs: Vec<&model::Province> = (1..=n as Id).map(|i| d.provinces.get(i).unwrap()).collect();
    let states: Vec<&State> = d.states.iter().map(|(_, s)| s).collect();
    let regions: Vec<&model::Region> = d.regions.iter().map(|(_, r)| r).collect();
    let nations: BTreeMap<Id, &Nation> = d.nations.iter().collect();
    let adjs = &d.adjacencies;
    let crossings: Vec<&model::Adjacency> = adjs.iter().filter(|x| x.kind == "sea" || x.kind.is_empty()).collect();
    let px_m = d.pixel_m;
    let mut rng = Rng::new(meta.seed);

    let mapdir = out.join("map");
    std::fs::create_dir_all(mapdir.join("terrain")).with_context(|| format!("cannot create {}", mapdir.display()))?;
    write(&out.join(MARKER), "made by mod-helper (Build mod); this folder is replaced on the next build\n")?;

    log("reading heights and land cover");
    let progress = Progress::default();
    let (dem_r, _) = raster::load(&map_dir.join("dem_section.tif"), &progress)?;
    let dem: Vec<f32> = match dem_r.pixels {
        Pixels::GrayF32(v) => v,
        Pixels::Gray16(v) => v.into_iter().map(|x| x as f32).collect(),
        Pixels::Gray8(v) => v.into_iter().map(|x| x as f32).collect(),
        _ => bail!("dem_section.tif is not a single-band height map"),
    };
    let (lc_r, _) = raster::load(&map_dir.join("landcover_smooth.bmp"), &progress)?;
    let lc: Vec<u8> = match lc_r.pixels {
        Pixels::Indexed8 { data, .. } | Pixels::Gray8(data) => data,
        _ => bail!("landcover_smooth.bmp is not an 8-bit class map"),
    };
    if (dem_r.width as usize, dem_r.height as usize) != (w, h) || (lc_r.width as usize, lc_r.height as usize) != (w, h) {
        bail!("dem_section.tif / landcover_smooth.bmp do not match provinces.bmp ({w} x {h})");
    }

    log("reading the game's files");
    let v = vanilla::load(&dirs.game, log)?;

    // ------------------------------------------------------------- bitmaps
    log("bitmaps");
    let kind = |p: Id| provs[p as usize - 1].kind;
    let by_id = |f: &dyn Fn(Id) -> bool| -> Vec<bool> { (0..=n as Id).map(|i| i > 0 && f(i)).collect() };
    let is_sea = by_id(&|p| kind(p) == Kind::Sea);
    let is_lake = by_id(&|p| kind(p) == Kind::Lake);
    let is_land = by_id(&|p| kind(p) == Kind::Land);
    let is_edge = by_id(&|p| provs[p as usize - 1].edge);
    // what is under the water plane: the sea and lakes of water
    let is_water = by_id(&|p| provs[p as usize - 1].is_water());
    let water_mask: Vec<bool> = pmap.par_iter().map(|&p| is_water[p as usize]).collect();

    // the heights: the build's, or the one imported in the editor made to fit the sea
    let (built, lo, hi) = bitmaps::heightmap(&dem, &water_mask, w, h);
    let mut warnings: Vec<String> = Vec::new();
    // (the heightmap, and the build's own when another is used)
    let (height, built): (Vec<u8>, Option<Vec<u8>>) = match &extra.heights {
        None => (built, None),
        Some(imp) => {
            let (v, land, water) = bitmaps::fit_to_sea(imp, &built, &water_mask);
            if land + water > 0 {
                warnings.push(format!("imported heightmap: {land} land px raised above the water plane, {water} px of sea or lakes lowered under it"));
            }
            (v, Some(built))
        }
    };
    img::write_bmp_gray(&mapdir.join("heightmap.bmp"), w as u32, h as u32, &height)?;

    let sea_f: Vec<f32> = water_mask.par_iter().map(|&s| s as u8 as f32).collect();
    // downsampled sea: water if any sea falls in the pixel, so thin channels survive
    let any_sea = |sw: usize, sh: usize| -> Vec<bool> { img::resize_f32(&sea_f, w, h, sw, sh, Filter::Box).into_iter().map(|v| v > 0.0).collect() };

    let slope = slope_degrees(&dem, w, h, px_m);
    let terr = bitmaps::final_texture(&lc, &slope, &water_mask, extra.paint.as_deref());
    img::write_bmp8(&mapdir.join("terrain.bmp"), w as u32, h as u32, &terr, &v.terrain.palette)?;

    let (nw, nh) = scaled(v.normal_size, v.map_size, (w, h));
    {
        // the shading follows the heights: what an imported heightmap changed
        // goes on top of the measured ones
        let relief_m: std::borrow::Cow<[f32]> = match built {
            None => (&dem[..]).into(),
            Some(built) => {
                let step = bitmaps::metres_per_step(lo, hi);
                dem.par_iter().zip(&height).zip(&built).map(|((&d, &a), &b)| d + (a as f32 - b as f32) * step).collect::<Vec<f32>>().into()
            }
        };
        let small = img::resize_f32(&relief_m, w, h, nw, nh, Filter::Bilinear);
        drop(relief_m);
        let (sx, sy) = gradient(&small, nw, nh, px_m * w as f64 / nw as f64);
        let relief = meta.relief as f32;
        let rgb: Vec<u8> = (0..nw * nh)
            .into_par_iter()
            .flat_map_iter(|i| {
                let (nx, ny, nz) = (-sx[i] * relief, sy[i] * relief, 1.0f32);
                let len = (nx * nx + ny * ny + nz * nz).sqrt();
                [(nx / len * 0.5 + 0.5) * 255.0, (ny / len * 0.5 + 0.5) * 255.0, 128.0 + nz / len * 127.0].map(|c| c.clamp(0.0, 255.0) as u8)
            })
            .collect();
        img::write_bmp24(&mapdir.join("world_normal.bmp"), nw as u32, nh as u32, &rgb)?;
    }
    // rivers as drawn in the editor (one pixel wide, with source and junction markers)
    let ground = |i: usize| {
        let p = pmap[i] as usize;
        if is_sea[p] {
            rivers::Ground::Sea
        } else if is_lake[p] {
            rivers::Ground::Lake
        } else {
            rivers::Ground::Land
        }
    };
    let river_px = rivers::rasterize(&d.rivers, w, h, ground);
    warnings.extend(river_px.problems.iter().map(|p| format!("rivers: {} (at {}, {})", p.msg, p.at[0], p.at[1])));
    img::write_bmp8(&mapdir.join("rivers.bmp"), w as u32, h as u32, &river_px.image(w, h, |i| water_mask[i]), &v.rivers.palette)?;
    // trees where terrain.bmp has forest (the land cover's forest, or painted)
    let (tw, th) = scaled(v.trees.size, v.map_size, (w, h));
    {
        let forest: Vec<f32> = terr.par_iter().map(|&t| matches!(t, T_FOREST | T_FOREST_B) as u8 as f32).collect();
        let share = img::resize_f32(&forest, w, h, tw, th, Filter::Box);
        let sea_t = any_sea(tw, th);
        let trees: Vec<u8> = share.iter().zip(&sea_t).map(|(&s, &sea)| if s > 0.4 && !sea { v.tree_index } else { v.trees.bg }).collect();
        img::write_bmp8(&mapdir.join("trees.bmp"), tw as u32, th as u32, &trees, &v.trees.palette)?;
    }
    let (cw, ch) = scaled(v.cities.size, v.map_size, (w, h));
    img::write_bmp8(&mapdir.join("cities.bmp"), cw as u32, ch as u32, &vec![v.city_index; cw * ch], &v.cities.palette)?;
    std::fs::copy(map_dir.join("provinces.bmp"), mapdir.join("provinces.bmp")).context("cannot copy provinces.bmp")?;

    let sealed_f: Vec<f32> = lc.par_iter().map(|&c| (c == SEALED) as u8 as f32).collect();
    for name in vanilla::DDS_FILES {
        let info = v.dds.get(name);
        let (sw, sh) = info.map(|i| scaled(i.size, v.map_size, (w, h))).unwrap_or((w / 2, h / 2));
        let sea_s = any_sea(sw, sh);
        let rgba: Vec<u8> = if name.starts_with("colormap_rgb") {
            let base = info.map(|i| [i.land[0], i.land[1], i.land[2]]).unwrap_or([90, 100, 60]);
            let mut lut = [[0u8; 3]; 256];
            for (t, l) in lut.iter_mut().enumerate() {
                *l = v.colours[t].unwrap_or(base);
            }
            lut[T_OCEAN as usize] = v.colours[T_OCEAN as usize].unwrap_or(info.map(|i| [i.water[0], i.water[1], i.water[2]]).unwrap_or([40, 70, 110]));
            let small = img::resize_nearest(&terr, w, h, sw, sh);
            let planes: Vec<Vec<f32>> = (0..3).map(|c| img::uniform3(&small.iter().map(|&t| lut[t as usize][c] as f32).collect::<Vec<_>>(), sw, sh)).collect();
            let urban = img::resize_f32(&sealed_f, w, h, sw, sh, Filter::Box);
            (0..sw * sh)
                .flat_map(|i| {
                    let a = (urban[i] * 255.0).clamp(0.0, 255.0) as u8; // city lights
                    if sea_s[i] {
                        let o = lut[T_OCEAN as usize];
                        [o[0], o[1], o[2], a]
                    } else {
                        [planes[0][i] as u8, planes[1][i] as u8, planes[2][i] as u8, a]
                    }
                })
                .collect()
        } else {
            let land_c = info.map(|i| i.land).unwrap_or([128, 128, 128, 0]);
            let water_c = info.map(|i| i.water).unwrap_or([40, 70, 110, 255]);
            sea_s.iter().flat_map(|&s| if s { water_c } else { land_c }).collect()
        };
        img::write_dds_rgba(&mapdir.join("terrain").join(name), sw as u32, sh as u32, &rgba)?;
    }

    // ------------------------------------------------------------- per-province data
    log("positions");
    let stats: Vec<PStats> = pmap
        .par_chunks(w)
        .enumerate()
        .fold(
            || vec![PStats::default(); n + 1],
            |mut acc, (y, row)| {
                for (x, &p) in row.iter().enumerate() {
                    let i = y * w + x;
                    let s = &mut acc[p as usize];
                    s.size += 1;
                    s.sealed += (lc[i] == SEALED) as u64;
                    s.bbox = [s.bbox[0].min(x), s.bbox[1].min(y), s.bbox[2].max(x), s.bbox[3].max(y)];
                }
                acc
            },
        )
        .reduce(
            || vec![PStats::default(); n + 1],
            |mut a, b| {
                for (x, y) in a.iter_mut().zip(&b) {
                    x.size += y.size;
                    x.sealed += y.sealed;
                    x.bbox = [x.bbox[0].min(y.bbox[0]), x.bbox[1].min(y.bbox[1]), x.bbox[2].max(y.bbox[2]), x.bbox[3].max(y.bbox[3])];
                }
                a
            },
        );
    // the kinds of land of the terrain rules, per province
    let kinds = meta.terrain.kinds();
    let nk = kinds.len();
    let land_counts = terrain::TerrainTypes::count(&kinds, pmap, &lc, &dem, &slope, n + 1);
    drop(slope);
    let size = |p: Id| stats[p as usize].size;
    let gameplay_terrain = |i: Id| -> String {
        let s = &stats[i as usize];
        let pr = provs[i as usize - 1];
        if is_sea[i as usize] {
            "ocean".into()
        } else if is_lake[i as usize] {
            "lakes".into()
        } else if let Some(t) = pr.terrain.as_ref().filter(|t| !t.is_empty()) {
            t.clone()
        } else if is_edge[i as usize] {
            "mountain".into()
        } else {
            let c = if nk == 0 { &[][..] } else { &land_counts[i as usize * nk..(i as usize + 1) * nk] };
            meta.terrain.terrain_of(c, s.size).into()
        }
    };
    let terrain_of: Vec<String> = std::iter::once(String::new()).chain((1..=n as Id).map(gameplay_terrain)).collect();
    let mut lines = vec!["0;0;0;0;land;false;unknown;0".to_string()];
    for p in &provs {
        let i = p.id as usize;
        let typ = match p.kind {
            Kind::Land => "land",
            Kind::Sea => "sea",
            Kind::Lake => "lake",
        };
        lines.push(format!(
            "{i};{};{};{};{typ};{};{};{}",
            p.rgb[0],
            p.rgb[1],
            p.rgb[2],
            if p.coastal && !is_lake[i] { "true" } else { "false" },
            terrain_of[i],
            if is_land[i] { 1 } else { 0 }
        ));
    }
    write_crlf(&mapdir.join("definition.csv"), &(lines.join("\n") + "\n"))?;

    // province centres: the pixel farthest from the border (first in row order)
    let not_border: Vec<bool> = (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y, p) = (i % w, i / w, pmap[i]);
            !((x > 0 && pmap[i - 1] != p) || (x + 1 < w && pmap[i + 1] != p) || (y > 0 && pmap[i - w] != p) || (y + 1 < h && pmap[i + w] != p))
        })
        .collect();
    let inner2 = img::edt_sq(&not_border, w, h);
    drop(not_border);
    let mut best = vec![(-1f32, 0usize); n + 1];
    for (i, &p) in pmap.iter().enumerate() {
        if inner2[i] > best[p as usize].0 {
            best[p as usize] = (inner2[i], i);
        }
    }
    let centre: Vec<(usize, usize)> = best.iter().map(|&(_, i)| (i / w, i % w)).collect();
    let xyz = |(r, c): (usize, usize), sea: bool| -> String {
        let y = if sea { WATER as f64 / 10.0 } else { height[r * w + c].max(WATER) as f64 / 10.0 };
        format!("{};{};{}", fmt2(c as f64 + 0.5), fmt2(y), fmt2(h as f64 - r as f64 - 0.5))
    };

    // neighbours
    let nb_pairs: FxHashSet<(Id, Id)> = pmap
        .par_chunks(w)
        .enumerate()
        .fold(FxHashSet::default, |mut acc, (y, row)| {
            for x in 0..w {
                let a = row[x];
                if x + 1 < w && row[x + 1] != a {
                    acc.insert(pair(a, row[x + 1]));
                }
                if y + 1 < h {
                    let b = pmap[(y + 1) * w + x];
                    if b != a {
                        acc.insert(pair(a, b));
                    }
                }
            }
            acc
        })
        .reduce(FxHashSet::default, |mut a, b| {
            a.extend(b);
            a
        });
    let mut nb: Vec<BTreeSet<Id>> = vec![BTreeSet::new(); n + 1];
    for &(a, b) in &nb_pairs {
        nb[a as usize].insert(b);
        nb[b as usize].insert(a);
    }
    let coast = coast_points(pmap, &is_sea, w, h);
    let mut state_of = vec![0 as Id; n + 1];
    for s in &states {
        for &p in &s.provinces {
            if (1..=n as Id).contains(&p) {
                state_of[p as usize] = s.id;
            }
        }
    }
    if let Some(p) = (1..=n as Id).find(|&p| is_land[p as usize] && state_of[p as usize] == 0) {
        let k = (1..=n).filter(|&q| is_land[q] && state_of[q] == 0).count();
        bail!("{k} land provinces are in no state, e.g. {} ({p})", provs[p as usize - 1].name);
    }
    for s in &states {
        if s.provinces.iter().any(|&p| !(1..=n as Id).contains(&p) || !is_land[p as usize]) {
            bail!("state {} ({}) has provinces that are not land", s.id, s.name);
        }
    }
    let urban_km2: Vec<f64> = stats.iter().map(|s| s.sealed as f64 * px_m * px_m / 1e6).collect();

    // victory points: set in the editor, else the most built-up province; vp is
    // the main one (capital province, unit and supply location) of each passable state
    for s in &states {
        if let Some(p) = s.victory_points.iter().map(|v| v[0]).find(|p| !s.provinces.contains(p)) {
            bail!("victory point province {p} of state {} ({}) is not in the state", s.id, s.name);
        }
    }
    let pstats = model::compute_stats(labels, n + 1);
    let vps: BTreeMap<Id, Vec<(Id, u32)>> = supply::state_vps(d, &pstats, &urban_km2);
    let main_vp: BTreeMap<Id, Id> = supply::main_vps(d, &pstats, &vps);
    let vp: BTreeMap<Id, (Id, u32)> = main_vp.iter().map(|(&s, &p)| (s, (p, vps[&s].iter().find(|x| x.0 == p).unwrap().1))).collect();
    let state_urban: BTreeMap<Id, f64> = states.iter().map(|s| (s.id, s.provinces.iter().fold(0.0, |a, &p| a + urban_km2[p as usize]))).collect();

    // ------------------------------------------------------------- nations
    log("nations");
    let nation_of_state: BTreeMap<Id, Id> = states.iter().map(|s| (s.id, s.nation.unwrap())).collect();
    let mut nat_urban: BTreeMap<Id, f64> = BTreeMap::new();
    for s in &states {
        *nat_urban.entry(s.nation.unwrap()).or_default() += state_urban[&s.id];
    }
    let urban_of = |id: Id| nat_urban.get(&id).copied().unwrap_or(0.0);
    // city, then towns, then villages; within each by sealed land (a stand-in for population)
    let mut by_rank: Vec<&Nation> = nations.values().copied().collect();
    by_rank.sort_by(|a, b| {
        place_rank(&a.place)
            .cmp(&place_rank(&b.place))
            .then(urban_of(b.id).total_cmp(&urban_of(a.id)))
            .then(b.km2.total_cmp(&a.km2))
            .then(a.id.cmp(&b.id))
    });
    let tag_of = countries::nation_tags(&nations.values().copied().collect::<Vec<_>>(), &vanilla::tags(&dirs.game)?)?;
    let country_of_state = |sid: Id| tag_of[&nation_of_state[&sid]].clone();
    let mut nat_adj: BTreeMap<Id, BTreeSet<Id>> = nations.keys().map(|&k| (k, BTreeSet::new())).collect();
    let mut land_links: Vec<(Id, Id)> = Vec::new();
    for p in 1..=n as Id {
        if is_land[p as usize] {
            land_links.extend(nb[p as usize].iter().filter(|&&q| is_land[q as usize]).map(|&q| (p, q)));
        }
    }
    land_links.extend(crossings.iter().filter(|x| is_land.get(x.from as usize) == Some(&true) && is_land.get(x.to as usize) == Some(&true)).map(|x| (x.from, x.to)));
    for (p, q) in land_links {
        let (n1, n2) = (nation_of_state[&state_of[p as usize]], nation_of_state[&state_of[q as usize]]);
        if n1 != n2 {
            nat_adj.get_mut(&n1).unwrap().insert(n2);
            nat_adj.get_mut(&n2).unwrap().insert(n1);
        }
    }
    let mut capital: BTreeMap<String, Id> = BTreeMap::new();
    for nat in nations.values() {
        if !vp.contains_key(&nat.capital_state) {
            bail!("capital state {} of {} is impassable, has no victory point or does not exist", nat.capital_state, nat.name);
        }
        capital.insert(tag_of[&nat.id].clone(), nat.capital_state);
    }
    let player_nat: &Nation = if meta.player.trim().is_empty() {
        by_rank[0]
    } else {
        let want = meta.player.trim().to_lowercase();
        by_rank
            .iter()
            .find(|x| tag_of[&x.id].to_lowercase() == want)
            .or_else(|| by_rank.iter().find(|x| x.name.to_lowercase().contains(&want)))
            .copied()
            .with_context(|| format!("bookmark country {:?}: no such tag or settlement", meta.player))?
    };
    let player = tag_of[&player_nat.id].clone();

    // buildings: per state in its victory point province, per province at its
    // centre, coast at the shore
    let mut interior_points = |p: Id, k: usize| -> Vec<(usize, usize)> {
        let b = stats[p as usize].bbox;
        let mut deep = Vec::new();
        let mut all = Vec::new();
        for y in b[1]..=b[3] {
            for x in b[0]..=b[2] {
                let i = y * w + x;
                if pmap[i] == p {
                    all.push((y, x));
                    if inner2[i] >= 9.0 {
                        deep.push((y, x));
                    }
                }
            }
        }
        let pool = if deep.is_empty() { all } else { deep };
        rng.choose(pool.len(), k).into_iter().map(|i| pool[i]).collect()
    };
    let mut blines: Vec<String> = Vec::new();
    let per_state: usize = STATE_BUILDINGS.iter().map(|b| b.1).sum();
    for s in &states {
        let anchor = vp.get(&s.id).map(|v| v.0).unwrap_or_else(|| first_max(s.provinces.iter().copied(), size).unwrap());
        let mut pts = interior_points(anchor, per_state).into_iter();
        for (btype, k) in STATE_BUILDINGS {
            for _ in 0..k {
                blines.push(format!("{};{btype};{};0.00;0", s.id, xyz(pts.next().unwrap(), false)));
            }
        }
        if let Some(p) = first_max(s.provinces.iter().copied().filter(|p| coast.contains_key(p)), size) {
            blines.push(format!("{};dockyard;{};0.00;0", s.id, xyz(coast[&p].1, false)));
        }
    }
    for p in 1..=n as Id {
        if !is_land[p as usize] {
            continue;
        }
        let st = state_of[p as usize];
        for btype in PROVINCE_BUILDINGS {
            blines.push(format!("{st};{btype};{};0.00;0", xyz(centre[p as usize], false)));
        }
        if let Some(&(sea, lrc, src)) = coast.get(&p) {
            for btype in COAST_BUILDINGS {
                if btype == "floating_harbor" {
                    blines.push(format!("{st};{btype};{};0.00;{sea}", xyz(src, true)));
                } else {
                    let adj = if btype == "naval_base_spawn" { sea } else { 0 };
                    blines.push(format!("{st};{btype};{};0.00;{adj}", xyz(lrc, false)));
                }
            }
        }
    }
    write(&mapdir.join("buildings.txt"), &(blines.join("\n") + "\n"))?;

    let mut us = String::new();
    for p in 1..=n as Id {
        if is_lake[p as usize] {
            continue; // vanilla lakes have no unit positions
        }
        let loc = xyz(centre[p as usize], is_sea[p as usize]);
        for t in 0..39 {
            us += &format!("{p};{t};{loc};0.00;0.00\n");
        }
    }
    write(&mapdir.join("unitstacks.txt"), &us)?;
    let biggest = |r: &model::Region| first_max(r.provinces.iter().copied(), size).unwrap();
    let mut wp = String::new();
    for r in &regions {
        for kind in ["small", "large"] {
            wp += &format!("{};{};{kind}\n", r.id, xyz(centre[biggest(r) as usize], r.naval));
        }
    }
    write(&mapdir.join("weatherpositions.txt"), &wp)?;
    // supply: railways, hubs and naval bases as set in the editor, else automatic
    let sup = supply::plan(d, &pstats, labels, &main_vp);
    for &i in &sup.bad_hubs {
        warnings.push(format!("supply hub {} is not on passable land; left out", i + 1));
    }
    for &i in &sup.bad_ports {
        warnings.push(format!("port {} is not on a coast; left out", i + 1));
    }
    for (ri, msg) in &sup.lines.faults {
        warnings.push(format!("railway {}: {msg}", ri + 1));
    }
    write(&mapdir.join("supply_nodes.txt"), &sup.hubs.iter().map(|p| format!("1 {p}\n")).collect::<String>())?;
    // vanilla's frame, logo and Great Wall sit at fixed positions of its 5632x2048 map
    let water: Vec<(f64, f64)> = regions
        .iter()
        .filter(|r| r.naval)
        .map(|r| {
            let (rr, cc) = centre[biggest(r) as usize];
            (cc as f64 + 0.5, h as f64 - rr as f64 - 0.5)
        })
        .collect();
    let mut ambient_txt = ambient_objects(&dirs.game, d.map_frame(), &water)?;
    // the 3D objects placed in the editor, on the ground (or the water) as built
    let mut ambient_line = None;
    if !d.ambient.is_empty() {
        log("ambient objects: reading the game's models");
        let cat = crate::models::catalog::Catalog::load(crate::models::catalog::Catalog::roots(&dirs.game, Some(&map_dir.join("content"))));
        let ground = |x: f32, y: f32| {
            let (c, r) = ((x.max(0.0) as usize).min(w - 1), (y.max(0.0) as usize).min(h - 1));
            let i = r * w + c;
            if water_mask[i] { WATER as f64 / 10.0 } else { height[i].max(WATER) as f64 / 10.0 }
        };
        let b = ambient::objects(&d.ambient, h, &ground, Some(&cat));
        ambient_txt += &b.blocks;
        if let Some(a) = &b.asset {
            write(&out.join("gfx/entities/mod_helper_ambient.asset"), a)?;
        }
        warnings.extend(b.warnings);
        ambient_line = Some(format!(
            "{} ambient objects in {} types{}",
            d.ambient.len(),
            b.types,
            if b.asset.is_some() { " (entities made in gfx/entities/mod_helper_ambient.asset)" } else { "" }
        ));
    }
    write(&mapdir.join("ambient_object.txt"), &ambient_txt)?;

    // railways: as laid in the editor, else a minimum spanning tree over
    // neighbouring passable states (any owner), each link the shortest land
    // way between their victory points; shared stretches laid once
    let (rail_txt, rail_lines) = sup.lines.railways_txt();
    write(&mapdir.join("railways.txt"), &rail_txt)?;

    // adjacency rules: from adjacency_rules.txt next to provinces.json, if there is one
    let rules_file = map_dir.join("adjacency_rules.txt");
    let rules_txt = if rules_file.exists() { vanilla::read_text(&rules_file)? } else { String::new() };
    let rule_names: BTreeSet<String> = Regex::new(r#"\bname\s*=\s*"?(\w+)"?"#).unwrap().captures_iter(&rules_txt).map(|c| c[1].to_string()).collect();
    let mut adj_lines = vec!["From;To;Type;Through;start_x;start_y;stop_x;stop_y;adjacency_rule_name;Comment".to_string()];
    for x in adjs {
        let t = x.kind.as_str();
        if !matches!(t, "sea" | "impassable" | "") {
            bail!("adjacency {}: unknown type {t:?}", x.name);
        }
        if !x.rule.is_empty() && !rule_names.contains(&x.rule) {
            bail!("adjacency {}: rule {} is not defined in {}", x.name, x.rule, rules_file.display());
        }
        let through = x.through.filter(|&t| t != 0);
        if t == "sea" && through.is_none() {
            bail!("strait {}: no sea province to cross", x.name);
        }
        let mut pos = "-1;-1;-1;-1".to_string();
        if let (true, Some(s), Some(e)) = (t == "sea", x.start, x.stop)
            && s.iter().chain(&e).all(|&v| v >= 0)
        {
            pos = format!("{};{};{};{}", s[0], h as i64 - 1 - s[1], e[0], h as i64 - 1 - e[1]);
        }
        adj_lines.push(format!("{};{};{t};{};{pos};{};{}", x.from, x.to, through.map_or(-1, |v| v as i64), x.rule, ascii_name(&x.name)));
    }
    adj_lines.push("-1;-1;;-1;-1;-1;-1;-1;-1".into());
    write_crlf(&mapdir.join("adjacencies.csv"), &(adj_lines.join("\n") + "\n"))?;
    write(&mapdir.join("adjacency_rules.txt"), if rules_txt.trim().is_empty() { "# no adjacency rules\n" } else { &rules_txt })?;

    // ------------------------------------------------------------- states, regions, countries
    log("history");
    let slots_of = |c: &str| CATEGORY_SLOTS.iter().find(|x| x.0 == c).map(|x| x.1);
    let mut cat_of: BTreeMap<Id, (String, u32)> = BTreeMap::new();
    for s in &states {
        let cat = match s.category.as_deref().filter(|c| !c.is_empty()) {
            Some(c) => {
                let Some(k) = slots_of(c) else { bail!("state {} ({}): unknown state category {c}", s.id, s.name) };
                (c.to_string(), k)
            }
            None if s.impassable => ("wasteland".into(), 0),
            None => {
                let c = STATE_CATEGORIES.iter().find(|c| state_urban[&s.id] < c.0).unwrap();
                (c.1.to_string(), c.2)
            }
        };
        cat_of.insert(s.id, cat);
    }
    // a nation's industry: 1 civilian factory and 1 more per 2 km² sealed land, 1
    // military per 4 km²; in its capital first, then its most urban states, as far
    // as building slots go. Factories set in the editor are kept as they are and do
    // not count against this.
    let mut plan: BTreeMap<Id, (u32, u32)> = BTreeMap::new();
    for nat in nations.values() {
        let u = urban_of(nat.id);
        let (mut civ, mut arms) = (1 + (u / 2.0).floor() as u32, (u / 4.0).floor() as u32);
        let mut own_states: Vec<&&State> = states.iter().filter(|s| s.nation == Some(nat.id) && !s.impassable).collect();
        own_states.sort_by(|a, b| (a.id != nat.capital_state).cmp(&(b.id != nat.capital_state)).then(state_urban[&b.id].total_cmp(&state_urban[&a.id])));
        for s in own_states {
            let own = &s.buildings;
            let used: u32 = own.iter().filter(|(k, _)| SLOT_BUILDINGS.contains(&k.as_str())).map(|(_, v)| *v).sum();
            let free = cat_of[&s.id].1.saturating_sub(used);
            let c = if own.contains_key("industrial_complex") { 0 } else { free.min(civ) };
            let m = if own.contains_key("arms_factory") { 0 } else { (free - c).min(arms) };
            civ -= c;
            arms -= m;
            plan.insert(s.id, (c, m));
        }
    }
    let mut categories: BTreeMap<String, usize> = BTreeMap::new();
    let mut factories = [0u32, 0u32];
    for s in &states {
        let u = state_urban[&s.id];
        let is_cap = s.id == nations[&s.nation.unwrap()].capital_state;
        let cat = cat_of[&s.id].0.clone();
        let manpower: u64 = match s.manpower {
            Some(m) => m,
            None if s.impassable => 100,
            None => {
                let base = if is_cap { meta.manpower_base } else { meta.manpower_base / 4.0 };
                ((base + u * meta.manpower_urban) / 100.0).round_ties_even() as u64 * 100
            }
        };
        *categories.entry(cat.clone()).or_default() += 1;
        let country = country_of_state(s.id);
        let mut hist = vec![format!("\t\towner = {country}"), format!("\t\tadd_core_of = {country}")];
        for (p, val) in vps.get(&s.id).into_iter().flatten() {
            hist.push(format!("\t\tvictory_points = {{ {p} {val} }}"));
        }
        let (civ, arms) = plan.get(&s.id).copied().unwrap_or((0, 0));
        let mut blds: Vec<(String, u32)> = vec![
            ("infrastructure".into(), default_infrastructure(s.impassable, u)),
            ("industrial_complex".into(), civ),
            ("arms_factory".into(), arms),
        ];
        for (k, &val) in &s.buildings {
            match blds.iter_mut().find(|b| &b.0 == k) {
                Some(b) => b.1 = val,
                None => blds.push((k.clone(), val)),
            }
        }
        let naval: String = s.provinces.iter().filter_map(|p| sup.ports.get(p).map(|lv| format!("\t\t\t{p} = {{\n\t\t\t\tnaval_base = {lv}\n\t\t\t}}\n"))).collect();
        factories[0] += blds.iter().find(|b| b.0 == "industrial_complex").map_or(0, |b| b.1);
        factories[1] += blds.iter().find(|b| b.0 == "arms_factory").map_or(0, |b| b.1);
        hist.push(format!(
            "\t\tbuildings = {{\n{}{naval}\t\t}}",
            blds.iter().filter(|(k, v)| *v != 0 || k == "infrastructure").map(|(k, v)| format!("\t\t\t{k} = {v}\n")).collect::<String>()
        ));
        let mut extra = if s.impassable { "\timpassable = yes\n".to_string() } else { String::new() };
        if s.resources.values().any(|&v| v != 0) {
            extra += "\tresources = {\n";
            for (k, v) in s.resources.iter().filter(|(_, v)| **v != 0) {
                extra += &format!("\t\t{k} = {v}\n");
            }
            extra += "\t}\n";
        }
        let provinces = s.provinces.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(" ");
        write(
            &out.join("history/states").join(format!("{}-STATE_{}.txt", s.id, s.id)),
            &format!(
                "state = {{\n\tid = {id}\n\tname = \"STATE_{id}\"\n\tmanpower = {manpower}\n\tstate_category = {cat}\n{extra}\n\thistory = {{\n{}\n\t}}\n\n\tprovinces = {{\n\t\t{provinces}\n\t}}\n\tlocal_supplies = 0.0\n}}\n",
                hist.join("\n"),
                id = s.id
            ),
        )?;
    }
    for r in &regions {
        let naval = if r.naval { "\n\tnaval_terrain = water_fjords" } else { "" };
        let provinces = r.provinces.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(" ");
        write(
            &mapdir.join("strategicregions").join(format!("{}-STRATEGICREGION_{}.txt", r.id, r.id)),
            &format!("strategic_region = {{\n\tid = {id}\n\tname = \"STRATEGICREGION_{id}\"\n\tprovinces = {{\n\t\t{provinces}\n\t}}{naval}\n{WEATHER}\n}}\n", id = r.id),
        )?;
    }

    // countries: tags, colours, flags, names, history, militia
    let mut colour = countries::nation_colours(&nat_adj, &mut rng);
    for x in nations.values() {
        if let Some(c) = x.color {
            colour.insert(x.id, c);
        }
    }
    let mut tag_lines = vec!["# generated by mod-helper: one country per settlement (before zz_dynamic_countries.txt)".to_string()];
    let mut colour_lines = Vec::new();
    let names = vanilla::name_blocks(&dirs.game, &["AUS", "ITA", "SWI"])?;
    let mut name_lines = Vec::new();
    let mut used_flags = HashSet::new();
    let fname_re = Regex::new(r"[^A-Za-z0-9 ]").unwrap();
    // tag → the history file written for it (the content's goes after it)
    let mut history_of: BTreeMap<String, String> = BTreeMap::new();
    for nat in &by_rank {
        let t = &tag_of[&nat.id];
        tag_lines.push(format!("{t} = \"countries/Suedtirol_{t}.txt\""));
        let [r, g, b] = colour[&nat.id];
        write(
            &out.join("common/countries").join(format!("Suedtirol_{t}.txt")),
            &format!("graphical_culture = western_european_gfx\ngraphical_culture_2d = western_european_2d\n\ncolor = {{ {r} {g} {b} }}\n"),
        )?;
        let ui = [r, g, b].map(|c| ((c as f64 * 1.25 + 25.0) as u32).min(255));
        colour_lines.push(format!("{t} = {{\n\tcolor = rgb {{ {r} {g} {b} }}\n\tcolor_ui = rgb {{ {} {} {} }}\n}}", ui[0], ui[1], ui[2]));
        countries::save_flags(&countries::flag_rgb(&mut rng, &mut used_flags), &out.join("gfx/flags"), t)?;
        name_lines.push(format!("{t} = {{{}}}", names[names_from(&nat.language)]));
        let ideology = nat.ideology.as_deref().filter(|i| !i.is_empty()).unwrap_or("neutrality");
        if !IDEOLOGIES.contains(&ideology) {
            bail!("{}: unknown ideology {ideology}", nat.name);
        }
        let mut pop: BTreeMap<&str, u32> = [("democratic", 20), ("fascism", 5), ("communism", 5), ("neutrality", 70)].into_iter().collect();
        let (a, b2) = (pop["neutrality"], pop[ideology]);
        pop.insert(ideology, a);
        pop.insert("neutrality", b2);
        let cap = capital[t];
        let fname = fname_re.replace_all(&ascii_name(short_name(&nat.name)), "").trim().to_string();
        let fname = if fname.is_empty() { t.clone() } else { fname };
        let oob = if nat.no_units { String::new() } else { format!("oob = \"{t}_1936\"\n") };
        history_of.insert(t.clone(), format!("history/countries/{t} - {fname}.txt"));
        write(
            &out.join("history/countries").join(format!("{t} - {fname}.txt")),
            &format!(
                "capital = {cap}
{oob}
# Starting tech, as a vanilla minor (Luxembourg)
set_technology = {{
\tinfantry_weapons = 1
\tinfantry_weapons1 = 1
\ttech_trucks = 1
\tgw_artillery = 1
\ttech_support = 1
\ttech_engineers = 1
\tbasic_train = 1
}}
IF = {{
\tlimit = {{
\t\tNOT = {{ has_dlc = \"By Blood Alone\" }}
\t}}
\tset_technology = {{
\t\tearly_fighter = 1
\t}}
}}

set_research_slots = {}
set_stability = 0.6
set_war_support = 0.2

set_politics = {{
\truling_party = {ideology}
\tlast_election = \"1932.1.1\"
\telection_frequency = 48
\telections_allowed = no
}}
set_popularities = {{
\tdemocratic = {}
\tfascism = {}
\tcommunism = {}
\tneutrality = {}
}}
",
                if nat.place == "city" { 3 } else { 2 },
                pop["democratic"],
                pop["fascism"],
                pop["communism"],
                pop["neutrality"]
            ),
        )?;
        if nat.no_units {
            continue;
        }
        let mil = militia(&nat.language);
        let home_p = vp[&cap].0;
        let divs: String = (1..=divisions(&nat.place))
            .map(|k| {
                format!(
                    "\tdivision = {{\n\t\tname = \"{k}. {mil}\"\n\t\tlocation = {home_p}\n\t\tdivision_template = \"{mil}\"\n\t\tstart_experience_factor = 0.1\n\t\tstart_equipment_factor = 0.7\n\t}}\n"
                )
            })
            .collect();
        write(
            &out.join("history/units").join(format!("{t}_1936.txt")),
            &format!(
                "division_template = {{\n\tname = \"{mil}\"\n\tregiments = {{\n\t\tinfantry = {{ x = 0 y = 0 }}\n\t\tinfantry = {{ x = 0 y = 1 }}\n\t}}\n}}\n\nunits = {{\n{divs}}}\n"
            ),
        )?;
    }
    write(&out.join("common/country_tags/01_suedtirol_countries.txt"), &(tag_lines.join("\n") + "\n"))?;
    let vanilla_colours = vanilla::read_text(&dirs.game.join("common/countries/colors.txt"))?;
    write(
        &out.join("common/countries/colors.txt"),
        &format!("{}\n\n# map nations (mod-helper)\n{}\n", vanilla_colours.trim_end(), colour_lines.join("\n")),
    )?;
    write(
        &out.join("common/names/01_suedtirol_names.txt"),
        &format!("# generated by mod-helper: name lists of AUS / ITA / SWI by language\n{}\n", name_lines.join("\n")),
    )?;

    // bookmark: the city and the larger towns as majors, the other towns as
    // minors; any village can be picked on the map
    let mut shown: Vec<&Nation> = by_rank.iter().filter(|x| x.place != "village").copied().collect();
    if !shown.iter().any(|x| x.id == player_nat.id) {
        shown.insert(0, player_nat);
    }
    let entry = |x: &Nation, minor: bool| {
        format!(
            "\t\t{} = {{\n{}\t\t\thistory = \"OTHER_GATHERING_STORM_DESC\"\n\t\t\tideology = {}\n\t\t\tideas = {{ }}\n\t\t\tfocuses = {{ }}\n\t\t}}",
            tag_of[&x.id],
            if minor { "\t\t\tminor = yes\n" } else { "" },
            x.ideology.as_deref().filter(|i| !i.is_empty()).unwrap_or("neutrality")
        )
    };
    let mut entries: Vec<String> = shown.iter().take(7).map(|x| entry(x, false)).collect();
    entries.push("\t\t\"---\" = {\n\t\t\thistory = \"OTHER_GATHERING_STORM_DESC\"\n\t\t}".into());
    entries.extend(shown.iter().skip(7).map(|x| entry(x, true)));
    write(
        &out.join("common/bookmarks/suedtirol.txt"),
        &format!(
            "bookmarks = {{\n\tbookmark = {{\n\t\tname = \"GATHERING_STORM_NAME\"\n\t\tdesc = \"GATHERING_STORM_DESC\"\n\t\tdate = 1936.1.1.12\n\t\tpicture = \"GFX_select_date_1936\"\n\t\tdefault_country = \"{player}\"\n\t\tdefault = yes\n{}\n\t}}\n}}\n",
            entries.join("\n")
        ),
    )?;
    let q = |s: &str| s.replace('"', "'");
    let mut loc = vec!["l_english:".to_string()];
    loc.extend(states.iter().map(|s| format!(" STATE_{}: \"{}\"", s.id, q(&s.name))));
    // vanilla names states per owner (AUS_STATE_4); those win over STATE_n when the
    // tag owns the state, so every such key of our ids gets our name
    let owner_keys = vanilla::owner_state_keys(&dirs.game)?;
    for s in &states {
        for tag in owner_keys.get(&s.id).into_iter().flatten() {
            loc.push(format!(" {tag}_STATE_{}: \"{}\"", s.id, q(&s.name)));
        }
    }
    for list in vps.values() {
        for (p, _) in list {
            loc.push(format!(" VICTORY_POINTS_{p}: \"{}\"", q(&provs[*p as usize - 1].name)));
        }
    }
    loc.extend(regions.iter().map(|r| format!(" STRATEGICREGION_{}: \"{}\"", r.id, q(&r.name))));
    write_bom(&out.join("localisation/english/replace/suedtirol_map_l_english.yml"), &(loc.join("\n") + "\n"))?;
    let mut cloc = vec!["l_english:".to_string()];
    for nat in &by_rank {
        let (t, nm) = (&tag_of[&nat.id], q(short_name(&nat.name)));
        cloc.extend([format!(" {t}: \"{nm}\""), format!(" {t}_DEF: \"{nm}\""), format!(" {t}_ADJ: \"{nm}\"")]);
    }
    write_bom(&out.join("localisation/english/suedtirol_countries_l_english.yml"), &(cloc.join("\n") + "\n"))?;

    // tutorial: the vanilla one (Italy against Ethiopia) names state 550 and
    // provinces of the vanilla map; the game looks them up unchecked when a session
    // starts and crashes. Same steps, pointed at this map.
    let mut tutorial_note = "vanilla tutorial not found".to_string();
    let vt = dirs.game.join("tutorial/tutorial.txt");
    if vt.exists() {
        let home = capital[&player];
        let home_p = vp[&home].0;
        let cands: Vec<Id> = if nat_adj[&player_nat.id].is_empty() {
            nations.keys().copied().filter(|&k| k != player_nat.id).collect()
        } else {
            nat_adj[&player_nat.id].iter().copied().collect()
        };
        let foe_nat = first_max(cands.iter().copied(), |i| (-(place_rank(&nations[&i].place) as f64), nations[&i].km2)).unwrap_or(player_nat.id);
        let foe_tag = tag_of[&foe_nat].clone();
        let foe_p = vp[&capital[&foe_tag]].0;
        let port = coast
            .keys()
            .copied()
            .filter(|&p| is_land[p as usize] && state_of[p as usize] != 0 && country_of_state(state_of[p as usize]) == player)
            .min_by_key(|&p| (state_of[p as usize] != home, p))
            .unwrap_or(home_p);
        // vanilla: 5010 Ethiopian capital, 12856 / 7980 on the way there, 12766 a port; else Eritrea
        let prov_map: BTreeMap<u64, Id> = [(5010, foe_p), (12856, foe_p), (7980, foe_p), (12766, port)].into_iter().collect();
        let mapped = |x: &str| x.parse::<u64>().ok().and_then(|v| prov_map.get(&v).copied()).unwrap_or(home_p);
        let raw = std::fs::read(&vt)?;
        let bom = raw.starts_with(b"\xef\xbb\xbf");
        let mut text = String::from_utf8_lossy(&raw[if bom { 3 } else { 0 }..]).into_owned();
        text = Regex::new(r"(\bstate\s*=\s*)(\d+)").unwrap().replace_all(&text, |c: &Captures| format!("{}{home}", &c[1])).into_owned();
        text = Regex::new(r"(highlight_states_trigger\s*=\s*\{)[^}]*\}").unwrap().replace_all(&text, |c: &Captures| format!("{} {home} }}", &c[1])).into_owned();
        for key in ["target", "direction_pointer"] {
            text = Regex::new(&format!(r"(\b{key}\s*=\s*)(\d+)")).unwrap().replace_all(&text, |c: &Captures| format!("{}{}", &c[1], mapped(&c[2]))).into_owned();
        }
        text = Regex::new(r"(highlight_provinces\s*=\s*\{)([^}]*)\}")
            .unwrap()
            .replace_all(&text, |c: &Captures| {
                let mut seen = Vec::new();
                for x in c[2].split_whitespace() {
                    let m = mapped(x).to_string();
                    if !seen.contains(&m) {
                        seen.push(m);
                    }
                }
                format!("{} {} }}", &c[1], seen.join(" "))
            })
            .into_owned();
        text = Regex::new(r#"(\btarget\s*=\s*")[A-Z0-9]{3}(")"#).unwrap().replace_all(&text, |c: &Captures| format!("{}{foe_tag}{}", &c[1], &c[2])).into_owned();
        let state_ids: BTreeSet<Id> = states.iter().map(|s| s.id).collect();
        let mut bad: Vec<String> = Regex::new(r"\bstate\s*=\s*(\d+)").unwrap().captures_iter(&text).map(|c| c[1].to_string()).filter(|x| x.parse().is_ok_and(|v: Id| !state_ids.contains(&v))).collect();
        bad.extend(
            Regex::new(r"(?:target|direction_pointer)\s*=\s*(\d+)")
                .unwrap()
                .captures_iter(&text)
                .map(|c| c[1].to_string())
                .filter(|x| x.parse::<u64>().is_ok_and(|v| v < 1 || v > n as u64)),
        );
        if !bad.is_empty() {
            bail!("tutorial still names ids this map does not have: {bad:?}");
        }
        write(&out.join("tutorial/tutorial.txt"), &format!("{}{text}", if bom { "\u{feff}" } else { "" }))?;
        let home_name = states.iter().find(|s| s.id == home).map(|s| s.name.clone()).unwrap_or_default();
        tutorial_note = format!("tutorial: {player} state {home} ({home_name}), enemy {foe_tag} province {foe_p}, port province {port}");
    }

    // descriptor, thumbnail, readme
    let mut notes = Vec::new();
    let picture = match meta.picture_path(map_dir) {
        Some(p) => match meta::write_thumbnail(&p, &out.join("thumbnail.png")) {
            Ok(warn) => {
                notes.extend(warn);
                true
            }
            Err(e) => {
                notes.push(format!("no thumbnail: {e:#}"));
                false
            }
        },
        None => false,
    };
    write(&out.join("descriptor.mod"), &meta.descriptor(&v.version, picture))?;
    if let Some(r) = meta.readme() {
        write(&out.join("README.txt"), &r)?;
    }

    // ------------------------------------------------------------- content
    log("nation content");
    let content_lines = crate::content::build::copy_into(out, map_dir, &history_of)?;

    // ------------------------------------------------------------- summary
    let mut tcount: BTreeMap<&str, usize> = BTreeMap::new();
    for t in &terrain_of[1..] {
        *tcount.entry(t.as_str()).or_default() += 1;
    }
    let sorted_desc = |m: &BTreeMap<&str, usize>| {
        let mut v: Vec<(&&str, &usize)> = m.iter().collect();
        v.sort_by(|a, b| b.1.cmp(a.1));
        v.iter().map(|(k, c)| format!("{k} {c}")).collect::<Vec<_>>().join(", ")
    };
    let cat_counts: BTreeMap<&str, usize> = categories.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    let water_px: Vec<u8> = height.iter().zip(&water_mask).filter(|(_, s)| **s).map(|(h, _)| *h).collect();
    let water_lakes = provs.iter().filter(|p| p.kind == Kind::Lake && p.water).count();
    let mut report = vec![
        format!(
            "\"{}\" {}, {w} x {h}, {n} provinces, {} states, {} strategic regions, {} building positions, {} adjacencies ({} straits), {} lakes, {} provinces out of play",
            meta.name,
            meta.version,
            states.len(),
            regions.len(),
            blines.len(),
            adjs.len(),
            adjs.iter().filter(|a| a.kind == "sea").count(),
            water_lakes,
            is_lake.iter().filter(|&&l| l).count() - water_lakes
        ),
        format!("gameplay terrain: {}", sorted_desc(&tcount)),
        format!("{} countries; factories {} civilian, {} military", by_rank.len(), factories[0], factories[1]),
        format!(
            "{} railway lines ({} rail links, {}), {} supply hubs ({}), {} naval bases, {} rivers ({} px)",
            rail_lines,
            sup.lines.level.len(),
            if sup.custom_rail { "as laid" } else { "automatic" },
            sup.hubs.len(),
            if sup.custom_hubs { "as set" } else { "automatic" },
            sup.ports.len(),
            d.rivers.len(),
            river_px.px.len()
        ),
        format!("state categories: {}", sorted_desc(&cat_counts)),
        format!(
            "player {player} ({}); bookmark: {}",
            player_nat.name,
            shown.iter().map(|x| format!("{} {}", tag_of[&x.id], short_name(&x.name))).collect::<Vec<_>>().join(", ")
        ),
        format!(
            "heightmap{}: land {}..255 for {lo:.0}..{hi:.0} m, sea and lakes {}..{}",
            if extra.heights.is_some() { " (imported)" } else { "" },
            WATER + 1,
            water_px.iter().min().copied().unwrap_or(0),
            water_px.iter().max().copied().unwrap_or(0)
        ),
        format!("vanilla samples: tree index {}, city index {}, {} terrain colours, game {}", v.tree_index, v.city_index, v.colours.iter().flatten().count(), v.version),
        format!("map frame {}", if d.map_frame() { "on" } else { "off" }),
        tutorial_note,
    ];
    if let Some(p) = &extra.paint {
        let n = p.par_iter().zip(&water_mask).filter(|(v, s)| **v != bitmaps::AUTO && !**s).count();
        report.push(format!("terrain.bmp: {n} px as painted in the editor"));
    }
    report.extend(ambient_line);
    report.extend(content_lines);
    report.extend(warnings);
    report.extend(notes);
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        // texture sizes scale with the map, rounding half to even as Python does
        assert_eq!(scaled((2816, 1024), (5632, 2048), (4608, 2816)), (2304, 1408));
        assert_eq!(scaled((5, 1), (10, 10), (5, 5)), (2, 1)); // 2.5 -> 2, 0.5 -> 0 -> at least 1
        // the first of equal maxima, as Python's max()
        assert_eq!(first_max([(1, 5), (2, 7), (3, 7)], |x| x.1), Some((2, 7)));
        assert_eq!(first_max(Vec::<(u32, u32)>::new(), |x| x.1), None);
        assert_eq!(fmt2(1.005), "1.00");
        assert_eq!(pair(5, 2), (2, 5));
    }

    #[test]
    fn gradient_like_numpy() {
        // np.gradient([[0, 1, 4], [2, 3, 8]], 2): d/dy [1, 1, 2] in both rows, d/dx [.5, 1, 1.5] and [.5, 1.5, 2.5]
        let f = [0.0, 1.0, 4.0, 2.0, 3.0, 8.0];
        let (gx, gy) = gradient(&f, 3, 2, 2.0);
        assert_eq!(gx, vec![0.5, 1.0, 1.5, 0.5, 1.5, 2.5]);
        assert_eq!(gy, vec![1.0, 1.0, 2.0, 1.0, 1.0, 2.0]);
    }

    #[test]
    fn coast() {
        // land 1 (left), land 2 (top right), sea 3 (bottom right)
        // 1 1 2 2
        // 1 1 3 3
        // 1 1 3 3
        let p = [1, 1, 2, 2, 1, 1, 3, 3, 1, 1, 3, 3];
        let is_sea = [false, false, false, true];
        let c = coast_points(&p, &is_sea, 4, 3);
        assert_eq!(c.keys().copied().collect::<Vec<_>>(), vec![1, 2]);
        let (sea, land_px, sea_px) = c[&1];
        assert_eq!(sea, 3);
        assert_eq!(land_px.1, 1, "the land pixel is next to the sea");
        assert_eq!(sea_px.1, 2);
        assert!(land_px.0 == sea_px.0 && (1..=2).contains(&land_px.0));
        assert_eq!(c[&2].0, 3);
        assert_eq!((c[&2].1.0 + 1, c[&2].1.1), c[&2].2, "province 2 meets the sea below it");
    }

    #[test]
    fn ambient_without_frame() {
        let t = ambient_objects(Path::new("/nonexistent"), false, &[(10.5, 20.5)]).unwrap();
        assert!(t.contains("ambient_wind_entity") && t.contains("10.50 10 20.50"));
        assert!(!t.contains("frame_border"));
        assert_eq!(t.matches('{').count(), t.matches('}').count());
        assert!(ambient_objects(Path::new("/nonexistent"), true, &[]).is_err(), "the frame needs the game's file");
        let blocks = vanilla_blocks("type={\n\ttype=\"a\"\n\tx={ 1 }\n}\ntype={ type=\"b\" }\n");
        assert_eq!(blocks.len(), 2);
        assert!(blocks[1].contains("\"b\""));
    }
}
