//! The map project the editor works on: `provinces.bmp` (one colour per
//! province) and `provinces.json` (provinces, states, nations, strategic
//! regions, adjacencies), as first written by make_provinces.py and read by
//! the mod build ([`crate::modbuild`]).
//!
//! In memory, provinces / states / nations / regions live in id-indexed
//! tables; deleted entries leave gaps, which are closed when saving (HOI4
//! wants ids 1..n). A province knows its state and strategic region; the
//! member lists of states and regions are derived (see [`Index`]).

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::geo::FileGeoInfo;
use crate::modbuild::bitmaps;
use crate::raster::{self, Pixels};
use crate::util::Progress;

pub type Id = u32;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Land,
    Sea,
    /// HOI4 "lake" province (no state, no units): a lake of water
    /// ([`Province::water`]) or land taken out of play (the inaccessible-zone
    /// tools).
    Lake,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Land => "land",
            Kind::Sea => "sea",
            Kind::Lake => "removed (lake)",
        }
    }
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Province {
    pub id: Id,
    pub rgb: [u8; 3],
    #[serde(rename = "type")]
    pub kind: Kind,
    #[serde(default)]
    pub edge: bool,
    #[serde(default)]
    pub coastal: bool,
    #[serde(default)]
    pub name: String,
    /// HOI4 terrain to use instead of the one the mod build derives.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terrain: Option<String>,
    /// A lake of water (`kind` lake): under the water plane like the sea.
    /// Other lakes are land taken out of play, which keeps its land look.
    #[serde(default, skip_serializing_if = "is_false")]
    pub water: bool,
    #[serde(default)]
    pub km2: f64,
    #[serde(default)]
    pub nation: Option<Id>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
    #[serde(skip)]
    pub state: Id,
    #[serde(skip)]
    pub region: Id,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub id: Id,
    pub name: String,
    #[serde(default)]
    pub nation: Option<Id>,
    #[serde(default)]
    pub impassable: bool,
    /// Written on save; in memory, provinces point at their state.
    #[serde(default)]
    pub provinces: Vec<Id>,
    /// Overrides; when absent the mod build derives the value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manpower: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub buildings: BTreeMap<String, u32>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub resources: BTreeMap<String, u32>,
    /// [province, value]; empty = automatic.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub victory_points: Vec<[u32; 2]>,
    /// The nations with a core on it; None: its owner alone, empty: nobody.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cores: Option<Vec<Id>>,
    /// The nation that holds it when that is not its owner (it starts occupied).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub controller: Option<Id>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl State {
    /// The nations with a core on it: its own list, else its owner.
    pub fn core_nations(&self) -> Vec<Id> {
        self.cores.clone().unwrap_or_else(|| self.nation.into_iter().collect())
    }
    /// The nation that occupies it (holds it without owning it).
    pub fn occupier(&self) -> Option<Id> {
        self.controller.filter(|c| Some(*c) != self.nation)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Nation {
    pub id: Id,
    pub name: String,
    /// city / town / village
    #[serde(default)]
    pub place: String,
    /// Settlement position (pixel row / column).
    #[serde(default)]
    pub row: f64,
    #[serde(default)]
    pub col: f64,
    #[serde(default)]
    pub capital_state: Id,
    #[serde(default)]
    pub states: Vec<Id>,
    #[serde(default)]
    pub language: String,
    #[serde(default)]
    pub km2: f64,
    /// Overrides for the mod build.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<[u8; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ideology: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub no_units: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Region {
    pub id: Id,
    pub name: String,
    #[serde(default)]
    pub naval: bool,
    #[serde(default)]
    pub provinces: Vec<Id>,
    #[serde(default)]
    pub states: Vec<Id>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

pub const ADJ_KINDS: [(&str, &str); 3] = [
    ("sea", "strait: land units cross the water (needs a sea province 'through')"),
    ("impassable", "blocks the border between two touching provinces"),
    ("", "link between provinces that do not touch (land, or sea with a rule as vanilla's Gibraltar)"),
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Adjacency {
    pub from: Id,
    pub to: Id,
    /// "sea", "impassable" or "" (land)
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub through: Option<Id>,
    /// Pixel [column, row] in `from` / `to` where the crossing is drawn.
    #[serde(default)]
    pub start: Option<[i64; 2]>,
    #[serde(default)]
    pub stop: Option<[i64; 2]>,
    /// adjacency_rule_name (e.g. a canal rule in adjacency_rules.txt)
    #[serde(default)]
    pub rule: String,
    #[serde(default)]
    pub name: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Id-indexed table with gaps (index 0 is never used).
#[derive(Clone, Debug)]
pub struct Table<T> {
    items: Vec<Option<T>>,
}

impl<T> Default for Table<T> {
    fn default() -> Self {
        Table { items: vec![None] }
    }
}

impl<T> Table<T> {
    pub fn get(&self, id: Id) -> Option<&T> {
        self.items.get(id as usize).and_then(|x| x.as_ref())
    }
    pub fn get_mut(&mut self, id: Id) -> Option<&mut T> {
        self.items.get_mut(id as usize).and_then(|x| x.as_mut())
    }
    pub fn contains(&self, id: Id) -> bool {
        self.get(id).is_some()
    }
    pub fn put(&mut self, id: Id, v: T) {
        let i = id as usize;
        if self.items.len() <= i {
            self.items.resize_with(i + 1, || None);
        }
        self.items[i] = Some(v);
    }
    pub fn push(&mut self, f: impl FnOnce(Id) -> T) -> Id {
        let id = self.items.len() as Id;
        self.items.push(Some(f(id)));
        id
    }
    pub fn remove(&mut self, id: Id) -> Option<T> {
        self.items.get_mut(id as usize).and_then(|x| x.take())
    }
    pub fn iter(&self) -> impl Iterator<Item = (Id, &T)> {
        self.items.iter().enumerate().filter_map(|(i, x)| x.as_ref().map(|v| (i as Id, v)))
    }
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (Id, &mut T)> {
        self.items.iter_mut().enumerate().filter_map(|(i, x)| x.as_mut().map(|v| (i as Id, v)))
    }
    pub fn ids(&self) -> Vec<Id> {
        self.iter().map(|(i, _)| i).collect()
    }
    pub fn count(&self) -> usize {
        self.items.iter().filter(|x| x.is_some()).count()
    }
    /// One past the largest id.
    pub fn cap(&self) -> usize {
        self.items.len()
    }
}

/// A point of a river: map pixel x, y and the width there (1 narrowest ..=
/// [`RIVER_WIDTHS`] widest).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RiverPoint(pub f32, pub f32, pub u8);

/// River widths (rivers.bmp indexes 3..=11).
pub const RIVER_WIDTHS: u8 = 9;
/// Widths up to this are small rivers for the game (smaller crossing penalty).
pub const SMALL_RIVER_MAX: u8 = 4;

/// A river from its source (first point) to its mouth (last point). Where it
/// ends at another river it flows into it; where it starts at one it branches
/// off it (see [`super::rivers`]).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct River {
    #[serde(default)]
    pub name: String,
    pub points: Vec<RiverPoint>,
}

/// A railway through the provinces under its stops (map pixels), each leg the
/// shortest way over land between two stops (see [`super::supply`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Railway {
    /// 1..=5
    pub level: u8,
    pub stops: Vec<[f32; 2]>,
}

/// A naval base in the coastal province under `at` (map pixel).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Port {
    pub at: [f32; 2],
    /// 1..=10
    pub level: u8,
}

/// A 3D object placed on the map (an ambient object of
/// map/ambient_object.txt): an entity of the game or the mod, or a mesh
/// (`pdxmesh`) the build makes an entity for.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ambient {
    /// The entity (or with `mesh`, the pdxmesh) it shows.
    pub entity: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub mesh: bool,
    /// Map pixel (x right, y down).
    pub at: [f32; 2],
    /// Height above the ground, or above the water at sea (map units: one
    /// is 10 steps of the heightmap).
    #[serde(default)]
    pub lift: f32,
    /// Degrees about x (tilt forward), y (turn, clockwise seen from above)
    /// and z (roll), as the game's `rotation`.
    #[serde(default)]
    pub rotation: [f32; 3],
    #[serde(default = "one")]
    pub scale: f32,
    /// What it plays: a state of the entity, or an animation of the mesh;
    /// None stands still.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub animation: Option<String>,
    /// Seen from any distance (the game hides small objects zoomed out).
    #[serde(default)]
    pub always_visible: bool,
    /// The object's name in the file (else made from the entity).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
}

fn one() -> f32 {
    1.0
}

/// Everything in provinces.json (cloned for undo; the bitmap is not).
#[derive(Clone, Debug, Default)]
pub struct MapData {
    pub provinces: Table<Province>,
    pub states: Table<State>,
    pub nations: Table<Nation>,
    pub regions: Table<Region>,
    pub adjacencies: Vec<Adjacency>,
    pub rivers: Vec<River>,
    /// None: the mod build lays them (a tree between the states' main victory
    /// points, [`super::supply::auto_links`]).
    pub railways: Option<Vec<Railway>>,
    /// Supply hubs (map pixels); None: one at the main victory point of every
    /// passable state.
    pub supply_hubs: Option<Vec<[f32; 2]>>,
    pub ports: Vec<Port>,
    /// 3D objects on the map.
    pub ambient: Vec<Ambient>,
    /// Other top-level keys (size, pixel_m, origin, crs, ...), kept as they are.
    pub header: Map<String, Value>,
    pub pixel_m: f64,
}

impl Province {
    /// Sea, or a lake of water.
    pub fn is_water(&self) -> bool {
        self.kind == Kind::Sea || (self.kind == Kind::Lake && self.water)
    }
    /// "land", "sea", "lake" or "removed (lake)".
    pub fn kind_name(&self) -> &'static str {
        if self.kind == Kind::Lake && self.water { "lake" } else { self.kind.name() }
    }
}

impl MapData {
    /// Sea or a lake of water (under the water plane).
    pub fn is_water(&self, p: Id) -> bool {
        self.provinces.get(p).is_some_and(Province::is_water)
    }
    pub fn km2_per_px(&self) -> f64 {
        self.pixel_m * self.pixel_m / 1e6
    }
    /// Vanilla's metal frame with the "Hearts of Iron IV" logo around the map
    /// (map/ambient_object.txt). It is placed for vanilla's 5632×2048 map, so
    /// a map of another size can leave it out ("map_frame": false).
    pub fn map_frame(&self) -> bool {
        self.header.get("map_frame").and_then(Value::as_bool).unwrap_or(true)
    }
    pub fn set_map_frame(&mut self, on: bool) {
        self.header.insert("map_frame".into(), Value::Bool(on));
    }
    /// Name, version, tags... and build settings of the mod ("mod" in provinces.json).
    pub fn mod_meta(&self) -> crate::modbuild::ModMeta {
        self.header.get("mod").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default()
    }
    pub fn set_mod_meta(&mut self, m: &crate::modbuild::ModMeta) {
        self.header.insert("mod".into(), serde_json::to_value(m).unwrap_or(Value::Null));
    }
    pub fn kind(&self, p: Id) -> Kind {
        self.provinces.get(p).map(|p| p.kind).unwrap_or_default()
    }
    pub fn state_of(&self, p: Id) -> Id {
        self.provinces.get(p).map(|p| p.state).unwrap_or(0)
    }
    pub fn region_of(&self, p: Id) -> Id {
        self.provinces.get(p).map(|p| p.region).unwrap_or(0)
    }
    pub fn nation_of_state(&self, s: Id) -> Id {
        self.states.get(s).and_then(|s| s.nation).unwrap_or(0)
    }
    pub fn nation_of(&self, p: Id) -> Id {
        self.nation_of_state(self.state_of(p))
    }
    pub fn impassable(&self, p: Id) -> bool {
        self.states.get(self.state_of(p)).is_some_and(|s| s.impassable)
    }
    pub fn prov_name(&self, p: Id) -> String {
        self.provinces.get(p).map(|x| if x.name.is_empty() { format!("province {p}") } else { x.name.clone() }).unwrap_or_default()
    }
    pub fn state_name(&self, s: Id) -> String {
        self.states.get(s).map(|x| x.name.clone()).unwrap_or_else(|| "—".into())
    }
    pub fn nation_name(&self, n: Id) -> String {
        self.nations.get(n).map(|x| x.name.clone()).unwrap_or_else(|| "—".into())
    }
    pub fn region_name(&self, r: Id) -> String {
        self.regions.get(r).map(|x| x.name.clone()).unwrap_or_else(|| "—".into())
    }

    /// A colour no province uses yet.
    pub fn fresh_colour(&self, salt: u64) -> [u8; 3] {
        let used: FxHashSet<[u8; 3]> = self.provinces.iter().map(|(_, p)| p.rgb).collect();
        let mut rng = super::kmeans::Rng::new(salt ^ (self.provinces.cap() as u64) << 20);
        loop {
            let v = rng.next_u64();
            let c = [(v >> 8) as u8, (v >> 24) as u8, (v >> 40) as u8];
            if c != [0, 0, 0] && !used.contains(&c) {
                return c;
            }
        }
    }
}

/// Province ids per pixel (row-major), 0 = no province.
#[derive(Clone)]
pub struct Labels {
    pub w: u32,
    pub h: u32,
    pub px: Vec<Id>,
}

impl Labels {
    #[inline]
    pub fn at(&self, x: u32, y: u32) -> Id {
        self.px[y as usize * self.w as usize + x as usize]
    }
}

/// Pixel statistics of one province.
#[derive(Clone, Copy, Debug)]
pub struct PStat {
    pub px: u32,
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
    pub sx: u64,
    pub sy: u64,
}

impl Default for PStat {
    fn default() -> Self {
        PStat { px: 0, x0: u32::MAX, y0: u32::MAX, x1: 0, y1: 0, sx: 0, sy: 0 }
    }
}

impl PStat {
    fn add(&mut self, x: u32, y: u32) {
        self.px += 1;
        self.x0 = self.x0.min(x);
        self.y0 = self.y0.min(y);
        self.x1 = self.x1.max(x);
        self.y1 = self.y1.max(y);
        self.sx += x as u64;
        self.sy += y as u64;
    }
    fn merge(&mut self, o: &PStat) {
        self.px += o.px;
        self.x0 = self.x0.min(o.x0);
        self.y0 = self.y0.min(o.y0);
        self.x1 = self.x1.max(o.x1);
        self.y1 = self.y1.max(o.y1);
        self.sx += o.sx;
        self.sy += o.sy;
    }
    /// Centroid in pixel coordinates (centre of the pixels).
    pub fn centre(&self) -> [f64; 2] {
        let n = self.px.max(1) as f64;
        [self.sx as f64 / n + 0.5, self.sy as f64 / n + 0.5]
    }
    pub fn box_w(&self) -> u32 {
        if self.px == 0 { 0 } else { self.x1 - self.x0 + 1 }
    }
    pub fn box_h(&self) -> u32 {
        if self.px == 0 { 0 } else { self.y1 - self.y0 + 1 }
    }
}

/// Derived from the bitmap: per-province statistics and borders.
#[derive(Default, Clone)]
pub struct Stats {
    pub prov: Vec<PStat>,
    /// Border length (pixel edges) between two provinces, key (min, max).
    pub border: FxHashMap<(Id, Id), u32>,
    pub nb: Vec<Vec<Id>>,
}

impl Stats {
    pub fn get(&self, p: Id) -> PStat {
        self.prov.get(p as usize).copied().unwrap_or_default()
    }
    pub fn border_len(&self, a: Id, b: Id) -> u32 {
        self.border.get(&(a.min(b), a.max(b))).copied().unwrap_or(0)
    }
    pub fn neighbours(&self, p: Id) -> &[Id] {
        self.nb.get(p as usize).map(|v| v.as_slice()).unwrap_or(&[])
    }
}

pub fn compute_stats(labels: &Labels, cap: usize) -> Stats {
    let (w, h) = (labels.w as usize, labels.h as usize);
    let max_label = labels.px.par_iter().copied().max().unwrap_or(0) as usize;
    let cap = cap.max(max_label + 1);
    const BAND: usize = 64;
    let parts: Vec<(Vec<PStat>, FxHashMap<(Id, Id), u32>)> = (0..h.div_ceil(BAND))
        .into_par_iter()
        .map(|b| {
            let mut st = vec![PStat::default(); cap];
            let mut border: FxHashMap<(Id, Id), u32> = FxHashMap::default();
            let mut add_border = |a: Id, c: Id| {
                *border.entry((a.min(c), a.max(c))).or_insert(0) += 1;
            };
            for y in b * BAND..((b + 1) * BAND).min(h) {
                let row = &labels.px[y * w..(y + 1) * w];
                let below = (y + 1 < h).then(|| &labels.px[(y + 1) * w..(y + 2) * w]);
                for x in 0..w {
                    let a = row[x];
                    st[a as usize].add(x as u32, y as u32);
                    if x + 1 < w && row[x + 1] != a {
                        add_border(a, row[x + 1]);
                    }
                    if let Some(bl) = below {
                        if bl[x] != a {
                            add_border(a, bl[x]);
                        }
                    }
                }
            }
            (st, border)
        })
        .collect();
    let mut prov = vec![PStat::default(); cap];
    let mut border: FxHashMap<(Id, Id), u32> = FxHashMap::default();
    for (st, b) in parts {
        for (p, s) in prov.iter_mut().zip(&st) {
            if s.px > 0 {
                p.merge(s);
            }
        }
        for (k, v) in b {
            *border.entry(k).or_insert(0) += v;
        }
    }
    let mut nb = vec![Vec::new(); cap];
    for &(a, b) in border.keys() {
        if a != 0 && b != 0 {
            nb[a as usize].push(b);
            nb[b as usize].push(a);
        }
    }
    for v in &mut nb {
        v.sort_unstable();
    }
    Stats { prov, border, nb }
}

/// Member lists derived from the per-province state / region.
#[derive(Default, Clone)]
pub struct Index {
    pub state_provs: Vec<Vec<Id>>,
    pub region_provs: Vec<Vec<Id>>,
    pub nation_states: Vec<Vec<Id>>,
}

impl Index {
    pub fn build(d: &MapData) -> Index {
        let mut ix = Index {
            state_provs: vec![Vec::new(); d.states.cap()],
            region_provs: vec![Vec::new(); d.regions.cap()],
            nation_states: vec![Vec::new(); d.nations.cap()],
        };
        for (id, p) in d.provinces.iter() {
            if let Some(v) = ix.state_provs.get_mut(p.state as usize) {
                if p.state != 0 {
                    v.push(id);
                }
            }
            if let Some(v) = ix.region_provs.get_mut(p.region as usize) {
                if p.region != 0 {
                    v.push(id);
                }
            }
        }
        for (id, s) in d.states.iter() {
            if let Some(n) = s.nation {
                if let Some(v) = ix.nation_states.get_mut(n as usize) {
                    v.push(id);
                }
            }
        }
        ix
    }
    pub fn provs_of_state(&self, s: Id) -> &[Id] {
        self.state_provs.get(s as usize).map(|v| v.as_slice()).unwrap_or(&[])
    }
    pub fn provs_of_region(&self, r: Id) -> &[Id] {
        self.region_provs.get(r as usize).map(|v| v.as_slice()).unwrap_or(&[])
    }
    pub fn states_of_nation(&self, n: Id) -> &[Id] {
        self.nation_states.get(n as usize).map(|v| v.as_slice()).unwrap_or(&[])
    }
}

// ---------------------------------------------------------------------------
// Loading

pub struct Loaded {
    pub data: MapData,
    pub labels: Labels,
    pub geo: FileGeoInfo,
    pub extra: Bitmaps,
}

/// What the map keeps besides its provinces, as bitmaps of its size:
/// painted terrain.bmp ([`bitmaps::AUTO`] where the build decides) and an
/// imported heightmap.
#[derive(Clone, Default)]
pub struct Bitmaps {
    pub paint: Option<Vec<u8>>,
    pub heights: Option<Vec<u8>>,
}

/// The bitmaps of the map in `dir`, if it has them.
fn load_bitmaps(dir: &Path, w: u32, h: u32) -> Result<Bitmaps> {
    let size = |f: &str, (bw, bh): (u32, u32)| -> Result<()> {
        if (bw, bh) != (w, h) {
            bail!("{f} is {bw}×{bh} px, provinces.bmp {w}×{h}");
        }
        Ok(())
    };
    let paint = match dir.join(bitmaps::PAINT_FILE) {
        f if f.is_file() => {
            let b = crate::modbuild::img::read_bmp(&f)?;
            if b.bpp != 8 {
                bail!("{} is not an 8-bit bitmap", bitmaps::PAINT_FILE);
            }
            size(bitmaps::PAINT_FILE, (b.w, b.h))?;
            Some(b.data)
        }
        _ => None,
    };
    let heights = match dir.join(bitmaps::HEIGHTS_FILE) {
        f if f.is_file() => {
            let (bw, bh, v) = bitmaps::read_grey(&f)?;
            size(bitmaps::HEIGHTS_FILE, (bw, bh))?;
            Some(v)
        }
        _ => None,
    };
    Ok(Bitmaps { paint, heights })
}

fn rgb_code(c: [u8; 3]) -> u32 {
    (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32
}

pub fn load(dir: &Path, progress: &Progress) -> Result<Loaded> {
    progress.set_stage("reading provinces.json");
    let json_path = dir.join("provinces.json");
    let text = std::fs::read_to_string(&json_path).with_context(|| format!("cannot read {}", json_path.display()))?;
    let mut root: Map<String, Value> =
        serde_json::from_str(&text).with_context(|| format!("invalid JSON in {}", json_path.display()))?;
    let mut take = |k: &str| root.remove(k).unwrap_or(Value::Array(Vec::new()));
    let provinces: Vec<Province> = serde_json::from_value(take("provinces")).context("provinces")?;
    let states: Vec<State> = serde_json::from_value(take("states")).context("states")?;
    let nations: Vec<Nation> = serde_json::from_value(take("nations")).context("nations")?;
    let regions: Vec<Region> = serde_json::from_value(take("regions")).context("regions")?;
    let adj_v = take("adjacencies");
    let straits_v = take("straits");
    let mut adjacencies: Vec<Adjacency> = serde_json::from_value(adj_v).context("adjacencies")?;
    if adjacencies.is_empty() {
        // make_provinces.py writes strait crossings as "straits" (always type sea)
        adjacencies = serde_json::from_value(straits_v).context("straits")?;
        for a in &mut adjacencies {
            a.kind = "sea".into();
        }
    }
    let rivers: Vec<River> = serde_json::from_value(take("rivers")).context("rivers")?;
    let ports: Vec<Port> = serde_json::from_value(take("ports")).context("ports")?;
    let ambient: Vec<Ambient> = serde_json::from_value(take("ambient_objects")).context("ambient_objects")?;
    let railways: Option<Vec<Railway>> = root.remove("railways").map(serde_json::from_value).transpose().context("railways")?;
    let supply_hubs: Option<Vec<[f32; 2]>> = root.remove("supply_hubs").map(serde_json::from_value).transpose().context("supply_hubs")?;
    let pixel_m = root.get("pixel_m").and_then(Value::as_f64).unwrap_or(1.0);

    let mut data = MapData { header: root, pixel_m, adjacencies, rivers, railways, supply_hubs, ports, ambient, ..Default::default() };
    for p in provinces {
        if p.id == 0 || data.provinces.contains(p.id) {
            bail!("province id {} is 0 or used twice", p.id);
        }
        data.provinces.put(p.id, p);
    }
    for s in states {
        for &p in &s.provinces {
            if let Some(pr) = data.provinces.get_mut(p) {
                pr.state = s.id;
            }
        }
        data.states.put(s.id, s);
    }
    for n in nations {
        data.nations.put(n.id, n);
    }
    for r in regions {
        for &p in &r.provinces {
            if let Some(pr) = data.provinces.get_mut(p) {
                pr.region = r.id;
            }
        }
        data.regions.put(r.id, r);
    }

    let (r, geo) = raster::load(&dir.join("provinces.bmp"), progress)?;
    let Pixels::Rgb8(rgb) = &r.pixels else { bail!("provinces.bmp is not a 24-bit RGB bitmap ({})", r.format) };
    progress.set_stage("matching colours");
    let by_rgb: FxHashMap<u32, Id> = data.provinces.iter().map(|(id, p)| (rgb_code(p.rgb), id)).collect();
    let px: Vec<Id> = rgb
        .par_chunks_exact(3)
        .map(|c| by_rgb.get(&((c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32)).copied().unwrap_or(0))
        .collect();
    if let Some(i) = px.iter().position(|&v| v == 0) {
        let n = px.iter().filter(|&&v| v == 0).count();
        bail!(
            "provinces.bmp has {n} pixels with colours not in provinces.json (first at x {}, y {})",
            i % r.width as usize,
            i / r.width as usize
        );
    }
    let extra = load_bitmaps(dir, r.width, r.height)?;
    Ok(Loaded { data, labels: Labels { w: r.width, h: r.height, px }, geo, extra })
}

// ---------------------------------------------------------------------------
// Saving

pub struct SaveReport {
    pub provinces: usize,
    pub states: usize,
    pub nations: usize,
    pub regions: usize,
    pub adjacencies: usize,
    pub renumbered: bool,
    pub backup: Option<PathBuf>,
}

/// Old id -> new id (1..n in id order), 0 for gaps.
fn compaction<T>(t: &Table<T>) -> (Vec<Id>, bool) {
    let mut map = vec![0; t.cap()];
    let mut next = 1;
    for (id, _) in t.iter() {
        map[id as usize] = next;
        next += 1;
    }
    let gaps = t.count() + 1 != t.cap();
    (map, gaps)
}

/// "YYYYMMDD-HHMMSS" (UTC) for backup folder names.
pub fn timestamp() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (days, rem) = (secs / 86400, secs % 86400);
    // civil_from_days (Howard Hinnant)
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}{m:02}{d:02}-{:02}{:02}{:02}", rem / 3600, rem / 60 % 60, rem % 60)
}

pub const BACKUPS_KEPT: usize = 10;

/// Seconds since 1970 of a [`timestamp`] ("YYYYMMDD-HHMMSS", UTC).
pub fn parse_timestamp(s: &str) -> Option<u64> {
    let (date, time) = s.split_once('-')?;
    if date.len() != 8 || time.len() != 6 || !date.bytes().chain(time.bytes()).all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n = |t: &str| t.parse::<i64>().ok();
    let (y, m, d) = (n(&date[..4])?, n(&date[4..6])?, n(&date[6..])?);
    let (hh, mm, ss) = (n(&time[..2])?, n(&time[2..4])?, n(&time[4..])?);
    // days_from_civil (Howard Hinnant)
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days * 86400 + hh * 3600 + mm * 60 + ss).ok()
}

/// A copy of the map's files that "Save" made before writing over them.
#[derive(Clone, Debug)]
pub struct Backup {
    pub dir: PathBuf,
    /// When it was made (UTC, as the folder is named).
    pub name: String,
    pub provinces: Option<usize>,
}

impl Backup {
    /// "3 h 10 min ago" and the like.
    pub fn age(&self) -> String {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        match parse_timestamp(&self.name).map(|t| now.saturating_sub(t) / 60) {
            None => String::new(),
            Some(m) if m < 60 => format!("{m} min ago"),
            Some(m) if m < 48 * 60 => format!("{} h {} min ago", m / 60, m % 60),
            Some(m) => format!("{} days ago", m / (24 * 60)),
        }
    }
}

/// The backups of the map in `dir`, newest first.
pub fn backups(dir: &Path) -> Vec<Backup> {
    #[derive(Deserialize)]
    struct Count {
        provinces: Vec<serde::de::IgnoredAny>,
    }
    let Ok(entries) = std::fs::read_dir(dir.join("backups")) else { return Vec::new() };
    let mut out: Vec<Backup> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.join("provinces.bmp").is_file() && p.join("provinces.json").is_file())
        .map(|p| {
            let provinces = std::fs::read_to_string(p.join("provinces.json")).ok().and_then(|t| serde_json::from_str::<Count>(&t).ok()).map(|c| c.provinces.len());
            Backup { name: p.file_name().unwrap_or_default().to_string_lossy().into_owned(), dir: p, provinces }
        })
        .collect();
    out.sort_by(|a, b| b.name.cmp(&a.name));
    out
}

/// Copy the current files to `dir/backups/<time>/` and drop the oldest backups
/// beyond [`BACKUPS_KEPT`] (only folders this function made).
fn backup(dir: &Path) -> Result<Option<PathBuf>> {
    let files = ["provinces.bmp", "provinces.json"];
    if !files.iter().all(|f| dir.join(f).exists()) {
        return Ok(None);
    }
    let root = dir.join("backups");
    let dest = root.join(timestamp());
    std::fs::create_dir_all(&dest).with_context(|| format!("cannot create {}", dest.display()))?;
    for f in files {
        std::fs::copy(dir.join(f), dest.join(f)).with_context(|| format!("cannot back up {f}"))?;
    }
    for f in [bitmaps::PAINT_FILE, bitmaps::HEIGHTS_FILE] {
        if dir.join(f).is_file() {
            std::fs::copy(dir.join(f), dest.join(f)).with_context(|| format!("cannot back up {f}"))?;
        }
    }
    std::fs::write(dest.join(".made_by_mod_helper"), "backup made by the mod-helper map editor before saving\n")?;
    let mut ours: Vec<PathBuf> = std::fs::read_dir(&root)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.join(".made_by_mod_helper").exists())
        .collect();
    ours.sort();
    while ours.len() > BACKUPS_KEPT {
        let old = ours.remove(0);
        std::fs::remove_dir_all(&old).with_context(|| format!("cannot remove old backup {}", old.display()))?;
    }
    Ok(Some(dest))
}

fn write_bmp24(path: &Path, w: u32, h: u32, row: impl Fn(u32, &mut Vec<u8>) + Sync) -> Result<()> {
    let stride = (w as usize * 3).div_ceil(4) * 4;
    let size = 54 + stride * h as usize;
    let mut out = Vec::with_capacity(size);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(size as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(h as i32).to_le_bytes()); // bottom-up
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&((stride * h as usize) as u32).to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    let rows: Vec<Vec<u8>> = (0..h)
        .into_par_iter()
        .map(|y| {
            let mut v = Vec::with_capacity(stride);
            row(h - 1 - y, &mut v);
            v.resize(stride, 0);
            v
        })
        .collect();
    for r in rows {
        out.extend_from_slice(&r);
    }
    let tmp = path.with_extension("bmp.tmp");
    std::fs::File::create(&tmp)
        .and_then(|mut f| f.write_all(&out))
        .with_context(|| format!("cannot write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("cannot write {}", path.display()))?;
    Ok(())
}

/// Write provinces.bmp and provinces.json (ids renumbered to 1..n), and the
/// painted terrain and imported heightmap (removed when there are none).
pub fn save(dir: &Path, data: &MapData, labels: &Labels, stats: &Stats, paint: &[u8], heights: Option<&[u8]>) -> Result<SaveReport> {
    let backup = backup(dir)?;
    let (pf, hf) = (dir.join(bitmaps::PAINT_FILE), dir.join(bitmaps::HEIGHTS_FILE));
    if paint.par_iter().any(|&v| v != bitmaps::AUTO) {
        let mut pal = bitmaps::vanilla_palette();
        pal.push([0, 0, 0]); // AUTO
        crate::modbuild::img::write_bmp8(&pf, labels.w, labels.h, paint, &pal)?;
    } else if pf.exists() {
        std::fs::remove_file(&pf).with_context(|| format!("cannot remove {}", pf.display()))?;
    }
    match heights {
        Some(v) => crate::modbuild::img::write_bmp_gray(&hf, labels.w, labels.h, v)?,
        None if hf.exists() => std::fs::remove_file(&hf).with_context(|| format!("cannot remove {}", hf.display()))?,
        None => {}
    }
    let (pmap, g1) = compaction(&data.provinces);
    let (smap, g2) = compaction(&data.states);
    let (nmap, g3) = compaction(&data.nations);
    let (rmap, g4) = compaction(&data.regions);
    let ix = Index::build(data);
    let m = |map: &Vec<Id>, id: Id| map.get(id as usize).copied().unwrap_or(0);
    let km2 = data.km2_per_px();

    // colours: unique per province
    let mut rgb: Vec<[u8; 3]> = vec![[0, 0, 0]; data.provinces.cap()];
    let mut seen = FxHashSet::default();
    for (id, p) in data.provinces.iter() {
        let mut c = p.rgb;
        let mut salt = id as u64 * 7919;
        while c == [0, 0, 0] || seen.contains(&c) {
            c = data.fresh_colour(salt);
            salt += 1;
        }
        seen.insert(c);
        rgb[id as usize] = c;
    }

    let mut provinces = Vec::new();
    for (id, p) in data.provinces.iter() {
        let mut q = p.clone();
        q.id = m(&pmap, id);
        q.rgb = rgb[id as usize];
        q.km2 = (stats.get(id).px as f64 * km2 * 1000.0).round() / 1000.0;
        q.nation = (p.kind == Kind::Land).then(|| m(&nmap, data.nation_of(id))).filter(|&n| n != 0);
        // as vanilla: land next to the sea, and sea next to land
        q.coastal = match p.kind {
            Kind::Land => stats.neighbours(id).iter().any(|&o| data.kind(o) == Kind::Sea),
            Kind::Sea => stats.neighbours(id).iter().any(|&o| o != 0 && data.kind(o) != Kind::Sea),
            Kind::Lake => false,
        };
        provinces.push(serde_json::to_value(&q)?);
    }
    let mut states = Vec::new();
    for (id, s) in data.states.iter() {
        let mut t = s.clone();
        t.id = m(&smap, id);
        t.nation = s.nation.map(|n| m(&nmap, n)).filter(|&n| n != 0);
        t.provinces = ix.provs_of_state(id).iter().map(|&p| m(&pmap, p)).filter(|&p| p != 0).collect();
        t.victory_points = s.victory_points.iter().filter(|v| m(&pmap, v[0]) != 0).map(|v| [m(&pmap, v[0]), v[1]]).collect();
        states.push(serde_json::to_value(&t)?);
    }
    let mut nations = Vec::new();
    for (id, n) in data.nations.iter() {
        let mut t = n.clone();
        t.id = m(&nmap, id);
        t.capital_state = m(&smap, n.capital_state);
        t.states = ix.states_of_nation(id).iter().map(|&s| m(&smap, s)).collect();
        let px: u64 = ix.states_of_nation(id).iter().flat_map(|&s| ix.provs_of_state(s)).map(|&p| stats.get(p).px as u64).sum();
        t.km2 = (px as f64 * km2 * 100.0).round() / 100.0;
        nations.push(serde_json::to_value(&t)?);
    }
    let mut regions = Vec::new();
    for (id, r) in data.regions.iter() {
        let mut t = r.clone();
        t.id = m(&rmap, id);
        t.provinces = ix.provs_of_region(id).iter().map(|&p| m(&pmap, p)).collect();
        let mut st: Vec<Id> = ix.provs_of_region(id).iter().map(|&p| data.state_of(p)).filter(|&s| s != 0).map(|s| m(&smap, s)).collect();
        st.sort_unstable();
        st.dedup();
        t.states = st;
        regions.push(serde_json::to_value(&t)?);
    }
    let mut adjacencies = Vec::new();
    for a in &data.adjacencies {
        let (f, t) = (m(&pmap, a.from), m(&pmap, a.to));
        if f == 0 || t == 0 {
            continue;
        }
        let mut b = a.clone();
        b.from = f;
        b.to = t;
        b.through = a.through.map(|x| m(&pmap, x)).filter(|&x| x != 0);
        adjacencies.push(serde_json::to_value(&b)?);
    }

    let mut root = Map::new();
    for (k, v) in &data.header {
        root.insert(k.clone(), v.clone());
    }
    root.insert("size".into(), serde_json::json!([labels.w, labels.h]));
    root.insert("edited_with".into(), Value::String("mod-helper map editor".into()));
    let report = SaveReport {
        provinces: provinces.len(),
        states: states.len(),
        nations: nations.len(),
        regions: regions.len(),
        adjacencies: adjacencies.len(),
        renumbered: g1 || g2 || g3 || g4,
        backup,
    };
    root.insert("nations".into(), Value::Array(nations));
    root.insert("provinces".into(), Value::Array(provinces));
    root.insert("states".into(), Value::Array(states));
    root.insert("regions".into(), Value::Array(regions));
    root.insert("adjacencies".into(), Value::Array(adjacencies));
    // map points, not province ids: nothing to renumber
    if !data.rivers.is_empty() {
        root.insert("rivers".into(), serde_json::to_value(&data.rivers)?);
    }
    if let Some(r) = &data.railways {
        root.insert("railways".into(), serde_json::to_value(r)?);
    }
    if let Some(h) = &data.supply_hubs {
        root.insert("supply_hubs".into(), serde_json::to_value(h)?);
    }
    if !data.ports.is_empty() {
        root.insert("ports".into(), serde_json::to_value(&data.ports)?);
    }
    if !data.ambient.is_empty() {
        root.insert("ambient_objects".into(), serde_json::to_value(&data.ambient)?);
    }

    let w = labels.w as usize;
    write_bmp24(&dir.join("provinces.bmp"), labels.w, labels.h, |y, out| {
        for &p in &labels.px[y as usize * w..(y as usize + 1) * w] {
            let c = rgb.get(p as usize).copied().unwrap_or([0, 0, 0]);
            out.extend_from_slice(&[c[2], c[1], c[0]]);
        }
    })?;
    let json = serde_json::to_string_pretty(&Value::Object(root))?;
    let path = dir.join("provinces.json");
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).with_context(|| format!("cannot write {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("cannot write {}", path.display()))?;
    crate::bundle::touched();
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_times() {
        assert_eq!(parse_timestamp("19700101-000000"), Some(0));
        assert_eq!(parse_timestamp("20261104-234427"), Some(1_793_835_867));
        assert_eq!(parse_timestamp("20240229-120000"), Some(1_709_208_000));
        assert_eq!(parse_timestamp(&timestamp()).map(|t| t / 10), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs() / 10));
        assert_eq!(parse_timestamp("backups"), None);
        assert_eq!(parse_timestamp("2026110-2344270"), None);
    }
}
