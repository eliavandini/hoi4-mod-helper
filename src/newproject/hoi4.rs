//! A map from the base game or a mod: everything the editor keeps of a map,
//! read from the game's files (the mod's first, then the game's, as the
//! game reads them, with the mod's `replace_path`s).
//!
//! What comes over: provinces (colour, type, terrain, name of the victory
//! point), states (owner, provinces, manpower, category, buildings,
//! resources, victory points, impassable), the countries owning them (name,
//! colour, capital, ruling party), strategic regions, adjacencies and their
//! rules, rivers (traced back into lines), railways, supply hubs, naval
//! bases, 3D objects, terrain.bmp (as painted terrain) and heightmap.bmp
//! (as the imported heightmap). What does not is listed in the report
//! (weather, cores, dated history, province buildings other than naval
//! bases, ...).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use rayon::prelude::*;
use rustc_hash::FxHashMap;
use serde_json::{Map, Value};

use crate::content::loc;
use crate::content::script::{Block, Script};
use crate::editor::model::{self, Adjacency, Ambient, Id, Kind, Labels, MapData, Nation, Port, Province, Railway, Region, State};
use crate::editor::rivers;
use crate::mapproj::Projection;
use crate::modbuild::{self, ModMeta, bitmaps, vanilla};
use crate::raster::{self, Pixels};
use crate::util::Progress;

/// Where the map comes from.
#[derive(Clone, Debug, PartialEq)]
pub struct Source {
    /// The game's folder.
    pub game: PathBuf,
    /// A mod over the game (its folder); None: the base game.
    pub mod_dir: Option<PathBuf>,
}

/// The files of the source as the game sees them.
struct Fs {
    /// The mod's folder (if any) first, then the game's.
    roots: Vec<PathBuf>,
    /// Folders only the mod's files count in (its `replace_path`s).
    replaced: Vec<String>,
    has_mod: bool,
}

impl Fs {
    fn new(src: &Source) -> Result<Fs> {
        let mut roots = Vec::new();
        let mut replaced = Vec::new();
        if let Some(m) = &src.mod_dir {
            if !m.is_dir() {
                bail!("{} is not a folder (a mod packed in a .zip has to be unpacked first)", m.display());
            }
            if let Ok(t) = vanilla::read_text(&m.join("descriptor.mod")) {
                let (_, lists) = crate::editor::launcher::parse_descriptor(&t);
                replaced = lists.get("replace_path").cloned().unwrap_or_default();
                // `replace_path="a"` is a value, not a list
                let re = regex::Regex::new(r#"(?m)^\s*replace_path\s*=\s*"([^"]+)""#).unwrap();
                replaced.extend(re.captures_iter(&t).map(|c| c[1].to_string()));
                for r in &mut replaced {
                    *r = r.trim_matches('/').to_string();
                }
            }
            roots.push(m.clone());
        }
        if !src.game.join("map").is_dir() {
            bail!("no HOI4 install at {} (no map folder)", src.game.display());
        }
        roots.push(src.game.clone());
        Ok(Fs { roots, replaced, has_mod: src.mod_dir.is_some() })
    }

    /// The roots a path is read from (a replaced folder: the mod's only).
    fn roots_for(&self, rel: &str) -> &[PathBuf] {
        let dir = rel.rsplit_once('/').map_or("", |d| d.0);
        if self.has_mod && self.replaced.iter().any(|r| dir == r || dir.starts_with(&format!("{r}/"))) { &self.roots[..1] } else { &self.roots }
    }

    fn file(&self, rel: &str) -> Option<PathBuf> {
        self.roots_for(rel).iter().map(|r| r.join(rel)).find(|p| p.is_file())
    }

    fn text(&self, rel: &str) -> Option<String> {
        self.file(rel).and_then(|p| vanilla::read_text(&p).ok())
    }

    /// The files of a folder with this extension, by name (the mod's win),
    /// sorted by name; `deep` goes into subfolders.
    fn list(&self, dir: &str, ext: &str, deep: bool) -> Vec<PathBuf> {
        let mut by_name: BTreeMap<String, PathBuf> = BTreeMap::new();
        for r in self.roots_for(&format!("{dir}/x")).iter().rev() {
            let base = r.join(dir);
            let mut stack = vec![base.clone()];
            while let Some(d) = stack.pop() {
                let Ok(rd) = std::fs::read_dir(&d) else { continue };
                for e in rd.flatten() {
                    let p = e.path();
                    if p.is_dir() {
                        if deep {
                            stack.push(p);
                        }
                    } else if p.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext)) {
                        let rel = p.strip_prefix(&base).unwrap_or(&p).to_string_lossy().replace('\\', "/");
                        by_name.insert(rel, p);
                    }
                }
            }
        }
        by_name.into_values().collect()
    }
}

/// English texts of the source (the game's, then the mod's over them;
/// `replace` folders last).
fn localisation(fs: &Fs) -> FxHashMap<String, String> {
    let mut out = FxHashMap::default();
    let mut files: Vec<(bool, String, PathBuf)> = Vec::new();
    for (i, r) in fs.roots.iter().enumerate().rev() {
        let mod_root = fs.has_mod && i == 0;
        let base = r.join("localisation");
        let mut stack = vec![base.clone()];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.to_string_lossy().to_ascii_lowercase().ends_with("_l_english.yml") {
                    let rel = p.strip_prefix(&base).unwrap_or(&p).to_string_lossy().to_string();
                    files.push((mod_root, rel, p));
                }
            }
        }
    }
    // the game's first, the mod's over them; replace/ over everything
    files.sort_by_key(|(m, rel, _)| (rel.contains("replace"), *m, rel.clone()));
    for (_, _, p) in files {
        if let Ok(t) = vanilla::read_text(&p) {
            for e in loc::parse(&t).entries {
                out.insert(e.key, loc::plain(&e.text));
            }
        }
    }
    out
}

/// The file names `map/default.map` gives (or the usual ones).
fn map_files(fs: &Fs) -> BTreeMap<&'static str, String> {
    let keys = [
        ("definitions", "definition.csv"),
        ("provinces", "provinces.bmp"),
        ("terrain", "terrain.bmp"),
        ("rivers", "rivers.bmp"),
        ("heightmap", "heightmap.bmp"),
        ("adjacency_rules", "adjacency_rules.txt"),
        ("adjacencies", "adjacencies.csv"),
        ("ambient_object", "ambient_object.txt"),
    ];
    let root = fs.text("map/default.map").map(|t| Script::parse(&t).root).unwrap_or_default();
    keys.iter().map(|&(k, d)| (k, format!("map/{}", root.atom(k).unwrap_or(d)))).collect()
}

/// `{ r g b }`, `rgb { r g b }` or `hsv { h s v }` (0..1).
fn colour(b: &Block, item: &crate::content::script::Item) -> Option<[u8; 3]> {
    let v: Vec<f64> = b.values().filter_map(|x| x.parse().ok()).collect();
    if v.len() < 3 {
        return None;
    }
    let hsv = matches!(&item.value, crate::content::script::Value::Tagged(t, _) if t.text.eq_ignore_ascii_case("hsv"));
    if hsv {
        let (h, s, val) = (v[0].rem_euclid(1.0), v[1].clamp(0.0, 1.0), v[2].clamp(0.0, 1.0));
        let i = (h * 6.0).floor();
        let f = h * 6.0 - i;
        let (p, q, t) = (val * (1.0 - s), val * (1.0 - s * f), val * (1.0 - s * (1.0 - f)));
        let (r, g, bb) = match i as i64 % 6 {
            0 => (val, t, p),
            1 => (q, val, p),
            2 => (p, val, t),
            3 => (p, q, val),
            4 => (t, p, val),
            _ => (val, p, q),
        };
        return Some([r, g, bb].map(|c| (c * 255.0).round().clamp(0.0, 255.0) as u8));
    }
    // a few mods give 0..1 floats for rgb
    let scale = if v.iter().take(3).all(|&c| c <= 1.0) && v.iter().take(3).any(|&c| c > 0.0 && c < 1.0) { 255.0 } else { 1.0 };
    Some([v[0], v[1], v[2]].map(|c| (c * scale).round().clamp(0.0, 255.0) as u8))
}

/// One row of definition.csv.
struct Def {
    id: Id,
    rgb: [u8; 3],
    kind: Kind,
    lake: bool,
    coastal: bool,
    terrain: String,
    continent: i64,
}

fn read_definitions(text: &str) -> Vec<Def> {
    let mut out = Vec::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split(';').map(str::trim).collect();
        if f.len() < 5 {
            continue;
        }
        let Ok(id) = f[0].parse::<Id>() else { continue };
        if id == 0 {
            continue;
        }
        let c = |i: usize| f[i].parse::<u8>().unwrap_or(0);
        let typ = f[4].to_ascii_lowercase();
        out.push(Def {
            id,
            rgb: [c(1), c(2), c(3)],
            kind: match typ.as_str() {
                "sea" => Kind::Sea,
                "lake" => Kind::Lake,
                _ => Kind::Land,
            },
            lake: typ == "lake",
            coastal: f.get(5).is_some_and(|v| v.eq_ignore_ascii_case("true")),
            terrain: f.get(6).map(|s| s.to_string()).unwrap_or_default(),
            continent: f.get(7).and_then(|v| v.parse().ok()).unwrap_or(0),
        });
    }
    out
}

/// "1939.1.1" and the like: a dated block of a history.
fn is_date(k: &str) -> bool {
    let mut parts = k.split('.');
    parts.next().is_some_and(|y| !y.is_empty() && y.chars().all(|c| c.is_ascii_digit())) && parts.next().is_some()
}

fn num(s: &str) -> Option<f64> {
    s.parse().ok()
}

/// Adjacencies as the editor and the build take them: straits whose sea
/// does not touch both ends go to a sea that does (or go), straits between
/// provinces that touch and impassable borders between provinces that do
/// not go, twice-listed ones go, and straits get crossing points in their
/// provinces (vanilla leaves most to the game). Canals stay as they are.
fn fix_adjacencies(doc: &mut crate::editor::doc::Doc) -> Vec<String> {
    let mut keep: Vec<Adjacency> = Vec::new();
    let mut seen = BTreeSet::new();
    let (mut repaired, mut failed, mut touching, mut not_touching, mut twice, mut placed) = (0, 0, 0, 0, 0, 0);
    let (w, h) = (doc.w as i64, doc.h as i64);
    let inside = |doc: &crate::editor::doc::Doc, pt: Option<[i64; 2]>, p: Id| {
        pt.is_some_and(|q| q[0] >= 0 && q[1] >= 0 && q[0] < w && q[1] < h && doc.labels.read().at(q[0] as u32, q[1] as u32) == p)
    };
    for a in doc.data.adjacencies.clone() {
        let b = match a.kind.as_str() {
            "impassable" if doc.stats.border_len(a.from, a.to) == 0 => {
                not_touching += 1;
                continue;
            }
            "sea" if !doc.is_canal(&a) => {
                let mut b = if doc.strait_ok(&a) {
                    a
                } else {
                    match doc.repair_strait(&a, None) {
                        Ok(b) => {
                            repaired += 1;
                            b
                        }
                        Err(_) => {
                            failed += 1;
                            continue;
                        }
                    }
                };
                if doc.stats.border_len(b.from, b.to) > 0 {
                    touching += 1;
                    continue;
                }
                if !inside(doc, b.start, b.from) || !inside(doc, b.stop, b.to) {
                    if let Some((s, e)) = doc.closest_points(b.from, b.to) {
                        b.start = Some([s[0] as i64, s[1] as i64]);
                        b.stop = Some([e[0] as i64, e[1] as i64]);
                        placed += 1;
                    }
                }
                b
            }
            _ => a,
        };
        if !seen.insert((b.from.min(b.to), b.from.max(b.to), b.kind.clone())) {
            twice += 1;
            continue;
        }
        keep.push(b);
    }
    doc.data.adjacencies = keep;
    let mut notes = Vec::new();
    if placed > 0 {
        notes.push(format!("{placed} straits got crossing points in their provinces"));
    }
    if repaired > 0 {
        notes.push(format!("{repaired} straits whose sea does not touch both shores cross a sea that does"));
    }
    for (n, what) in [
        (failed, "straits without a sea touching both shores"),
        (touching, "straits between provinces that touch"),
        (not_touching, "impassable borders between provinces that do not touch"),
        (twice, "adjacencies listed twice"),
    ] {
        if n > 0 {
            notes.push(format!("left out: {n} {what}"));
        }
    }
    notes
}

/// Import the map of `src` into the empty folder `out`. `meta` names the
/// mod (the rest of a mod's descriptor fills in what it leaves empty);
/// `proj` places the map. Returns the report: what came over and what not.
pub fn import(src: &Source, out: &Path, meta: &ModMeta, proj: &Projection, progress: &Progress) -> Result<Vec<String>> {
    let fs = Fs::new(src)?;
    let mut report: Vec<String> = Vec::new();
    let mut left_out: BTreeMap<&str, usize> = BTreeMap::new();
    let files = map_files(&fs);
    let need = |k: &str| -> Result<PathBuf> { fs.file(&files[k]).with_context(|| format!("{} not found in the mod or the game", files[k])) };

    progress.set_stage("reading the localisation");
    let texts = localisation(&fs);
    let text = |k: &str| texts.get(k).filter(|t| !t.trim().is_empty()).cloned();

    // ------------------------------------------------------------ provinces
    progress.set_stage("reading provinces.bmp");
    let (r, _) = raster::load(&need("provinces")?, progress)?;
    let Pixels::Rgb8(rgb) = &r.pixels else { bail!("{} is not a 24-bit RGB bitmap ({})", files["provinces"], r.format) };
    let (w, h) = (r.width, r.height);
    if let Err(e) = crate::mapproj::check_size(w, h) {
        report.push(format!("note: {e}"));
    }
    let defs = read_definitions(&vanilla::read_text(&need("definitions")?)?);
    if defs.is_empty() {
        bail!("{} has no provinces", files["definitions"]);
    }
    progress.set_stage("matching colours");
    let code = |c: [u8; 3]| (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32;
    let mut by_rgb: FxHashMap<u32, Id> = defs.iter().map(|d| (code(d.rgb), d.id)).collect();
    let mut data = MapData::default();
    for d in &defs {
        let mut extra = Map::new();
        if d.continent != 0 {
            extra.insert("continent".into(), d.continent.into());
        }
        let land_terrain = d.kind == Kind::Land && !d.terrain.is_empty() && d.terrain != "unknown";
        data.provinces.put(
            d.id,
            Province {
                id: d.id,
                rgb: d.rgb,
                kind: d.kind,
                edge: false,
                coastal: d.coastal,
                name: text(&format!("VICTORY_POINTS_{}", d.id)).unwrap_or_default(),
                terrain: land_terrain.then(|| d.terrain.clone()),
                water: d.lake,
                km2: 0.0,
                nation: None,
                extra,
                state: 0,
                region: 0,
            },
        );
    }
    // colours definition.csv does not have: a land province each
    let mut unknown = 0usize;
    let mut px: Vec<Id> = rgb.par_chunks_exact(3).map(|c| by_rgb.get(&code([c[0], c[1], c[2]])).copied().unwrap_or(0)).collect();
    if px.contains(&0) {
        for (i, c) in rgb.chunks_exact(3).enumerate() {
            if px[i] != 0 {
                continue;
            }
            let k = code([c[0], c[1], c[2]]);
            let id = *by_rgb.entry(k).or_insert_with(|| {
                unknown += 1;
                data.provinces.push(|id| Province {
                    id,
                    rgb: [c[0], c[1], c[2]],
                    kind: Kind::Land,
                    edge: false,
                    coastal: false,
                    name: String::new(),
                    terrain: None,
                    water: false,
                    km2: 0.0,
                    nation: None,
                    extra: Map::new(),
                    state: 0,
                    region: 0,
                })
            });
            px[i] = id;
        }
        report.push(format!("note: {unknown} colours of provinces.bmp are not in definition.csv; each is a land province now"));
    }
    let labels = Labels { w, h, px };
    let stats = model::compute_stats(&labels, data.provinces.cap());
    let empty: Vec<Id> = data.provinces.ids().into_iter().filter(|&p| stats.get(p).px == 0).collect();
    if !empty.is_empty() {
        report.push(format!("note: {} provinces of definition.csv have no pixels and are left out (ids from {} renumbered)", empty.len(), empty[0]));
        for p in &empty {
            data.provinces.remove(*p);
        }
    }
    // a pixel of each province near its middle (railway stops, supply hubs, ports)
    let inside: Vec<[f32; 2]> = {
        let cap = data.provinces.cap();
        let centre: Vec<[f64; 2]> = (0..cap as Id).map(|p| stats.get(p).centre()).collect();
        let w = w as usize;
        let best = labels
            .px
            .par_chunks(w)
            .enumerate()
            .fold(
                || vec![(f64::MAX, [0f32; 2]); cap],
                |mut acc, (y, row)| {
                    for (x, &p) in row.iter().enumerate() {
                        let c = centre[p as usize];
                        let d = (x as f64 + 0.5 - c[0]).powi(2) + (y as f64 + 0.5 - c[1]).powi(2);
                        if d < acc[p as usize].0 {
                            acc[p as usize] = (d, [x as f32 + 0.5, y as f32 + 0.5]);
                        }
                    }
                    acc
                },
            )
            .reduce(|| vec![(f64::MAX, [0f32; 2]); cap], |a, b| a.into_iter().zip(b).map(|(x, y)| if y.0 < x.0 { y } else { x }).collect());
        best.into_iter().map(|b| b.1).collect()
    };
    let at = |p: Id| inside.get(p as usize).copied();
    let exists = |d: &MapData, p: Id| d.provinces.contains(p);

    // ------------------------------------------------------------ states
    progress.set_stage("reading the states");
    let cats: BTreeSet<&str> = modbuild::CATEGORY_SLOTS.iter().map(|c| c.0).collect();
    let mut owners: BTreeMap<String, Vec<Id>> = BTreeMap::new();
    // by state: the tags with a core on it, and the tag that holds it
    let mut core_tags: BTreeMap<Id, Vec<String>> = BTreeMap::new();
    let mut controllers: BTreeMap<Id, String> = BTreeMap::new();
    let state_files = fs.list("history/states", "txt", false);
    for f in &state_files {
        let Ok(t) = vanilla::read_text(f) else { continue };
        let root = Script::parse(&t).root;
        for st in root.blocks("state") {
            let Some(sid) = st.num("id").map(|v| v as Id).filter(|&v| v > 0) else { continue };
            if data.states.contains(sid) {
                report.push(format!("note: state {sid} is defined twice; the first ({}) is kept", f.display()));
                continue;
            }
            let key = st.atom("name").unwrap_or_default().to_string();
            let name = text(&key).unwrap_or_else(|| if key.is_empty() { format!("State {sid}") } else { loc::prettify(&key) });
            let hist = st.block("history");
            let mut s = State {
                id: sid,
                name,
                nation: None,
                impassable: st.yes("impassable"),
                provinces: Vec::new(),
                manpower: st.num("manpower").map(|v| v.max(0.0) as u64),
                category: None,
                buildings: BTreeMap::new(),
                resources: BTreeMap::new(),
                victory_points: Vec::new(),
                cores: None,
                controller: None,
                extra: Map::new(),
            };
            if !key.is_empty() {
                s.extra.insert("source_name".into(), Value::String(key));
            }
            match st.atom("state_category") {
                Some(c) if cats.contains(c) => s.category = Some(c.to_string()),
                Some(c) => {
                    *left_out.entry("state categories the build does not know (it picks one)").or_default() += 1;
                    s.extra.insert("source_category".into(), Value::String(c.to_string()));
                }
                None => {}
            }
            if let Some(res) = st.block("resources") {
                for it in &res.items {
                    if let Some(v) = it.text().and_then(num) {
                        s.resources.insert(it.key().to_string(), v.round().max(0.0) as u32);
                    }
                }
            }
            if let Some(hist) = hist {
                if let Some(o) = hist.atom("owner") {
                    owners.entry(o.to_string()).or_default().push(sid);
                    s.extra.insert("source_owner".into(), Value::String(o.to_string()));
                }
                for vp in hist.blocks("victory_points") {
                    let v: Vec<f64> = vp.values().filter_map(num).collect();
                    if let [p, val, ..] = v[..] {
                        s.victory_points.push([p as u32, val.round().max(1.0) as u32]);
                    }
                }
                if let Some(b) = hist.block("buildings") {
                    for it in &b.items {
                        match (it.text().and_then(num), it.block()) {
                            (Some(v), _) => {
                                s.buildings.insert(it.key().to_string(), v.max(0.0) as u32);
                            }
                            (None, Some(pb)) => {
                                let Some(p) = it.key().parse::<Id>().ok() else { continue };
                                for x in &pb.items {
                                    match (x.key(), x.text().and_then(num)) {
                                        ("naval_base", Some(lv)) if lv > 0.0 => {
                                            if let Some(a) = at(p).filter(|_| exists(&data, p)) {
                                                data.ports.push(Port { at: a, level: (lv as u8).clamp(1, 10) });
                                            }
                                        }
                                        _ => *left_out.entry("province buildings other than naval bases (bunkers, ...)").or_default() += 1,
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
                let dated = hist.items.iter().filter(|i| is_date(i.key())).count();
                if dated > 0 {
                    *left_out.entry("dated history entries of states").or_default() += dated;
                }
                core_tags.insert(sid, hist.all("add_core_of").filter_map(|i| i.text()).map(str::to_string).collect());
                if let Some(c) = hist.atom("controller") {
                    controllers.insert(sid, c.to_string());
                }
                let claims = hist.all("add_claim_by").count();
                if claims > 0 {
                    *left_out.entry("claims on states").or_default() += claims;
                }
            }
            if let Some(pb) = st.block("provinces") {
                for p in pb.values().filter_map(|v| v.parse::<Id>().ok()) {
                    match data.provinces.get_mut(p) {
                        Some(pr) if pr.kind == Kind::Land => {
                            if pr.state != 0 {
                                report.push(format!("note: province {p} is in states {} and {sid}; it stays in {}", pr.state, pr.state));
                            } else {
                                pr.state = sid;
                            }
                        }
                        Some(_) => *left_out.entry("sea or lake provinces listed in states").or_default() += 1,
                        None => {}
                    }
                }
            }
            s.victory_points.retain(|v| data.provinces.get(v[0]).is_some_and(|p| p.state == sid));
            data.states.put(sid, s);
        }
    }
    let unowned_land = data.provinces.iter().filter(|(_, p)| p.kind == Kind::Land && p.state == 0).count();
    if unowned_land > 0 {
        report.push(format!("note: {unowned_land} land provinces are in no state"));
    }

    // ------------------------------------------------------------ countries
    progress.set_stage("reading the countries");
    let tag_files: BTreeMap<String, String> = {
        let re = regex::Regex::new(r#"(?m)^\s*([A-Za-z][A-Za-z0-9]{2})\s*=\s*"([^"]+)""#).unwrap();
        let mut m = BTreeMap::new();
        for f in fs.list("common/country_tags", "txt", false) {
            if let Ok(t) = vanilla::read_text(&f) {
                for c in re.captures_iter(&t) {
                    m.entry(c[1].to_string()).or_insert(c[2].to_string());
                }
            }
        }
        m
    };
    let history: BTreeMap<String, PathBuf> = fs
        .list("history/countries", "txt", false)
        .into_iter()
        .filter_map(|p| {
            let n = p.file_name()?.to_string_lossy().to_string();
            let tag = n.get(..3)?.to_string();
            Some((tag, p))
        })
        .collect();
    let vanilla_tags = vanilla::tags(&src.game).unwrap_or_default();
    let mut kept_tags = 0;
    let mut sizes: Vec<(String, usize)> = owners.iter().map(|(t, s)| (t.clone(), s.len())).collect();
    sizes.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut nation_of_tag: BTreeMap<String, Id> = BTreeMap::new();
    for (tag, _) in &sizes {
        let states = &owners[tag];
        let color = tag_files.get(tag).and_then(|f| fs.text(&format!("common/{f}"))).and_then(|t| {
            let root = Script::parse(&t).root;
            let item = root.get("color")?;
            colour(item.block()?, item)
        });
        let hist = history.get(tag).and_then(|p| vanilla::read_text(p).ok()).map(|t| Script::parse(&t).root);
        let capital = hist.as_ref().and_then(|h| h.num("capital")).map(|v| v as Id).filter(|c| states.contains(c)).unwrap_or(states[0]);
        let ideology = hist
            .as_ref()
            .and_then(|h| h.block("set_politics"))
            .and_then(|p| p.atom("ruling_party"))
            .filter(|i| modbuild::IDEOLOGIES.contains(i))
            .map(str::to_string);
        let own = !vanilla_tags.contains(tag) && modbuild::countries::tag_ok(tag) && !modbuild::countries::RESERVED_TAGS.contains(&tag.as_str());
        kept_tags += own as usize;
        let place = match states.len() {
            n if n >= 8 => "city",
            n if n >= 2 => "town",
            _ => "village",
        };
        // the capital's main victory point, else its middle
        let home = data
            .states
            .get(capital)
            .and_then(|s| s.victory_points.iter().max_by_key(|v| v[1]).map(|v| v[0]))
            .and_then(at)
            .or_else(|| data.provinces.iter().find(|(_, p)| p.state == capital).and_then(|(id, _)| at(id)))
            .unwrap_or([0.0, 0.0]);
        let mut extra = Map::new();
        extra.insert("source_tag".into(), Value::String(tag.clone()));
        let nid = data.nations.push(|id| Nation {
            id,
            name: text(tag).unwrap_or_else(|| tag.clone()),
            place: place.into(),
            row: home[1] as f64,
            col: home[0] as f64,
            capital_state: capital,
            states: Vec::new(),
            language: String::new(),
            km2: 0.0,
            tag: own.then(|| tag.clone()),
            color,
            ideology,
            no_units: false,
            extra,
        });
        for &s in states {
            if let Some(st) = data.states.get_mut(s) {
                st.nation = Some(nid);
            }
        }
        nation_of_tag.insert(tag.clone(), nid);
    }
    // cores (kept when they are not just the owner's) and occupiers, of the countries that own states
    for (sid, st) in data.states.iter_mut() {
        let cores: Vec<Id> = core_tags.get(&sid).into_iter().flatten().filter_map(|t| nation_of_tag.get(t).copied()).collect();
        let lost = core_tags.get(&sid).map_or(0, |c| c.len()) - cores.len();
        if lost > 0 {
            *left_out.entry("cores of countries that own no state").or_default() += lost;
        }
        if cores != st.nation.into_iter().collect::<Vec<_>>() {
            st.cores = Some(cores);
        }
        match controllers.get(&sid).map(|t| nation_of_tag.get(t)) {
            Some(Some(&c)) if Some(c) != st.nation => st.controller = Some(c),
            Some(None) => *left_out.entry("occupiers that own no state").or_default() += 1,
            _ => {}
        }
    }
    let unowned = data.states.iter().filter(|(_, s)| s.nation.is_none()).count();
    if unowned > 0 {
        report.push(format!("note: {unowned} states have no owner; the build needs one for each"));
    }
    if kept_tags < data.nations.count() {
        report.push(format!(
            "{} countries have tags of the base game; the build gives them new tags (their own is kept as \"source_tag\")",
            data.nations.count() - kept_tags
        ));
    }

    // ------------------------------------------------------------ strategic regions
    progress.set_stage("reading the strategic regions");
    for f in fs.list("map/strategicregions", "txt", false) {
        let Ok(t) = vanilla::read_text(&f) else { continue };
        let root = Script::parse(&t).root;
        for sr in root.blocks("strategic_region") {
            let Some(rid) = sr.num("id").map(|v| v as Id).filter(|&v| v > 0) else { continue };
            if data.regions.contains(rid) {
                continue;
            }
            let key = sr.atom("name").unwrap_or_default().to_string();
            let mut provs = 0;
            let mut sea = 0;
            if let Some(pb) = sr.block("provinces") {
                for p in pb.values().filter_map(|v| v.parse::<Id>().ok()) {
                    if let Some(pr) = data.provinces.get_mut(p)
                        && pr.region == 0
                    {
                        pr.region = rid;
                        provs += 1;
                        sea += (pr.kind == Kind::Sea) as usize;
                    }
                }
            }
            if provs == 0 {
                continue;
            }
            if sr.block("weather").is_some() {
                *left_out.entry("weather of strategic regions (the build writes its own)").or_default() += 1;
            }
            data.regions.put(
                rid,
                Region {
                    id: rid,
                    name: text(&key).unwrap_or_else(|| if key.is_empty() { format!("Region {rid}") } else { loc::prettify(&key) }),
                    naval: sea * 2 > provs,
                    provinces: Vec::new(),
                    states: Vec::new(),
                    extra: Map::new(),
                },
            );
        }
    }
    // the editor keeps air (land) zones and sea zones apart; the game's
    // regions mix them here and there (islands in a sea, a bay in a land
    // region): the fewer go into a region of their own
    let mut split = 0;
    let mut moved = 0;
    for rid in data.regions.ids() {
        let members: Vec<(Id, bool)> = data.provinces.iter().filter(|(_, p)| p.region == rid).map(|(id, p)| (id, p.kind == Kind::Sea)).collect();
        let sea = members.iter().filter(|m| m.1).count();
        let naval = sea * 2 > members.len();
        let strays: Vec<Id> = members.iter().filter(|m| m.1 != naval).map(|m| m.0).collect();
        let r = data.regions.get_mut(rid).unwrap();
        r.naval = naval;
        if strays.is_empty() {
            continue;
        }
        let name = format!("{} ({})", r.name, if naval { "land" } else { "sea" });
        let new = data.regions.push(|id| Region { id, name, naval: !naval, provinces: Vec::new(), states: Vec::new(), extra: Map::new() });
        for p in &strays {
            data.provinces.get_mut(*p).unwrap().region = new;
        }
        split += 1;
        moved += strays.len();
    }
    if split > 0 {
        report.push(format!(
            "{split} strategic regions have land and sea; their {moved} provinces of the other kind are in {split} new regions (the editor keeps air and sea zones apart)"
        ));
    }
    let no_region = data.provinces.iter().filter(|(_, p)| p.region == 0 && p.kind != Kind::Lake).count();
    if no_region > 0 {
        report.push(format!("note: {no_region} provinces are in no strategic region"));
    }

    // ------------------------------------------------------------ adjacencies
    progress.set_stage("reading the adjacencies");
    if let Some(t) = fs.text(&files["adjacencies"]) {
        for line in t.lines().skip(1) {
            let f: Vec<&str> = line.split(';').map(str::trim).collect();
            if f.len() < 4 || f[0].starts_with('#') {
                continue;
            }
            let (Ok(from), Ok(to)) = (f[0].parse::<i64>(), f[1].parse::<i64>()) else { continue };
            if from <= 0 || to <= 0 || !exists(&data, from as Id) || !exists(&data, to as Id) {
                continue;
            }
            let n = |i: usize| f.get(i).and_then(|v| v.parse::<i64>().ok()).unwrap_or(-1);
            let pos = |x: i64, y: i64| (x >= 0 && y >= 0).then(|| [x, h as i64 - 1 - y]);
            let kind = f[2].to_ascii_lowercase();
            let kind = match kind.as_str() {
                "sea" | "impassable" | "" => kind,
                _ => {
                    *left_out.entry("adjacencies of other types (kept as land links)").or_default() += 1;
                    String::new()
                }
            };
            data.adjacencies.push(Adjacency {
                from: from as Id,
                to: to as Id,
                kind,
                through: Some(n(3)).filter(|&v| v > 0 && exists(&data, v as Id)).map(|v| v as Id),
                start: pos(n(4), n(5)),
                stop: pos(n(6), n(7)),
                rule: f.get(8).map(|s| s.to_string()).unwrap_or_default(),
                name: f.get(9).map(|s| s.to_string()).unwrap_or_default(),
                extra: Map::new(),
            });
        }
    }
    std::fs::create_dir_all(out).with_context(|| format!("cannot create {}", out.display()))?;
    if let Some(p) = fs.file(&files["adjacency_rules"]) {
        std::fs::copy(&p, out.join("adjacency_rules.txt")).context("cannot copy adjacency_rules.txt")?;
    }

    // ------------------------------------------------------------ rivers
    progress.set_stage("tracing the rivers");
    if let Some(p) = fs.file(&files["rivers"]) {
        let b = modbuild::img::read_bmp(&p)?;
        if b.bpp == 8 && (b.w, b.h) == (w, h) {
            data.rivers = rivers::trace(&b.data, w as usize, h as usize, 0.5);
        } else {
            report.push(format!("note: {} is not an 8-bit bitmap of the map's size; no rivers", files["rivers"]));
        }
    }

    // ------------------------------------------------------------ railways, supply
    progress.set_stage("reading railways and supply hubs");
    if let Some(t) = fs.text("map/railways.txt") {
        let mut rails = Vec::new();
        for line in t.lines() {
            let v: Vec<i64> = line.split('#').next().unwrap_or("").split_whitespace().filter_map(|x| x.parse().ok()).collect();
            if v.len() < 4 {
                continue;
            }
            let stops: Vec<[f32; 2]> = v[2..].iter().filter(|&&p| p > 0 && exists(&data, p as Id)).filter_map(|&p| at(p as Id)).collect();
            if stops.len() >= 2 {
                rails.push(Railway { level: v[0].clamp(1, 5) as u8, stops });
            }
        }
        data.railways = Some(rails);
    }
    if let Some(t) = fs.text("map/supply_nodes.txt") {
        let hubs: Vec<[f32; 2]> = t
            .lines()
            .filter_map(|l| {
                let v: Vec<i64> = l.split_whitespace().filter_map(|x| x.parse().ok()).collect();
                v.get(1).copied()
            })
            .filter(|&p| p > 0 && exists(&data, p as Id))
            .filter_map(|p| at(p as Id))
            .collect();
        data.supply_hubs = Some(hubs);
    }

    // ------------------------------------------------------------ 3D objects
    let mut frame = false;
    if let Some(t) = fs.text(&files["ambient_object"]) {
        let root = Script::parse(&t).root;
        for ty in root.blocks("type") {
            let Some(entity) = ty.atom("type") else { continue };
            let e = entity.to_ascii_lowercase();
            if e.starts_with("frame_border") {
                frame = true;
                continue;
            }
            if e.contains("ambient_wind") || e.contains("ambient_water") {
                continue;
            }
            let scale = ty.num("scale").unwrap_or(1.0) as f32;
            for o in ty.blocks("object") {
                let pos: Vec<f32> = o.block("position").map(|b| b.values().filter_map(|v| v.parse().ok()).collect()).unwrap_or_default();
                let rot: Vec<f32> = o.block("rotation").map(|b| b.values().filter_map(|v| v.parse().ok()).collect()).unwrap_or_default();
                let [x, y, z, ..] = pos[..] else { continue };
                let r = |i: usize| rot.get(i).copied().unwrap_or(0.0);
                let turn = (-r(1)).rem_euclid(360.0);
                data.ambient.push(Ambient {
                    entity: entity.to_string(),
                    mesh: false,
                    at: [x, h as f32 - z],
                    lift: y,
                    rotation: [r(0), if turn > 180.0 { turn - 360.0 } else { turn }, r(2)],
                    scale,
                    animation: None,
                    always_visible: ty.yes("always_visible"),
                    name: o.atom("name").unwrap_or_default().to_string(),
                });
            }
        }
    }

    // ------------------------------------------------------------ bitmaps
    progress.set_stage("reading terrain.bmp and heightmap.bmp");
    let n = (w * h) as usize;
    let paint = match fs.file(&files["terrain"]) {
        Some(p) => match bitmaps::read_texture(&p, &bitmaps::vanilla_palette()) {
            Ok((tw, th, v)) if (tw, th) == (w, h) => v,
            Ok((tw, th, _)) => {
                report.push(format!("note: {} is {tw} × {th}, not {w} × {h}; the build decides the terrain", files["terrain"]));
                vec![bitmaps::AUTO; n]
            }
            Err(e) => {
                report.push(format!("note: {}: {e:#}; the build decides the terrain", files["terrain"]));
                vec![bitmaps::AUTO; n]
            }
        },
        None => vec![bitmaps::AUTO; n],
    };
    let heights = match fs.file(&files["heightmap"]).map(|p| bitmaps::read_grey(&p)) {
        Some(Ok((hw, hh, v))) if (hw, hh) == (w, h) => Some(v),
        Some(Ok((hw, hh, _))) => {
            report.push(format!("note: {} is {hw} × {hh}, not {w} × {h}; no heights", files["heightmap"]));
            None
        }
        Some(Err(e)) => {
            report.push(format!("note: {}: {e:#}; no heights", files["heightmap"]));
            None
        }
        None => None,
    };

    // ------------------------------------------------------------ the mod
    let mut m = meta.clone();
    if let Some(dir) = &src.mod_dir
        && let Ok(t) = vanilla::read_text(&dir.join("descriptor.mod"))
    {
        let (v, l) = crate::editor::launcher::parse_descriptor(&t);
        if m.version.is_empty() {
            m.version = v.get("version").cloned().unwrap_or_default();
        }
        if let Some(sv) = v.get("supported_version") {
            m.supported_version = sv.clone();
        }
        if let Some(tags) = l.get("tags").filter(|t| !t.is_empty()) {
            m.tags = tags.clone();
        }
    }
    data.set_mod_meta(&m);
    data.set_map_frame(frame);
    proj.set_header(&mut data.header, w, h);
    data.pixel_m = proj.pixel_m();
    let source = match &src.mod_dir {
        Some(d) => format!("mod {}", d.display()),
        None => format!("base game {}", src.game.display()),
    };
    data.header.insert("imported_from".into(), Value::String(source.clone()));

    // state categories with too few building slots for the factories there
    let slots = |c: &str| modbuild::CATEGORY_SLOTS.iter().find(|x| x.0 == c).map(|x| x.1);
    let mut bumped = 0;
    for (_, st) in data.states.iter_mut() {
        let used: u32 = st.buildings.iter().filter(|(k, _)| modbuild::SLOT_BUILDINGS.contains(&k.as_str())).map(|(_, v)| *v).sum();
        if let Some(c) = st.category.clone()
            && slots(&c).is_some_and(|k| k < used)
            && let Some(bigger) = modbuild::CATEGORY_SLOTS.iter().filter(|x| x.1 >= used).min_by_key(|x| x.1)
        {
            st.category = Some(bigger.0.to_string());
            bumped += 1;
        }
    }
    if bumped > 0 {
        report.push(format!("{bumped} states have more factories than their category has building slots (the game adds slots by script): they get a bigger category"));
    }

    progress.set_stage("saving the map");
    // as an open map of the editor: memberships made consistent, the
    // adjacencies checked as the editor checks them
    let loaded = model::Loaded { data, labels, geo: Default::default(), extra: model::Bitmaps { paint: Some(paint), heights } };
    let mut doc = crate::editor::doc::Doc::new(out.to_path_buf(), loaded);
    report.extend(fix_adjacencies(&mut doc));
    // points where four provinces meet, provinces touching only diagonally:
    // the game logs them as map errors, the editor's checks too
    doc.begin("import");
    let fixed = doc.fix_crossings(None);
    doc.commit();
    if fixed > 0 {
        report.push(format!("{fixed} px changed where four provinces met or provinces touched only diagonally (map errors for the game)"));
    }
    doc.save()?;
    proj.write_georef(&out.join("provinces.bmp"), w, h)?;
    let data = &doc.data;

    let mut lines = vec![
        format!("imported the map of the {source}: {w} × {h} px"),
        format!(
            "{} provinces, {} states, {} countries, {} strategic regions, {} adjacencies",
            data.provinces.count(),
            data.states.count(),
            data.nations.count(),
            data.regions.count(),
            data.adjacencies.len()
        ),
        format!(
            "{} rivers, {} railways, {} supply hubs, {} naval bases, {} 3D objects",
            data.rivers.len(),
            data.railways.as_ref().map_or(0, Vec::len),
            data.supply_hubs.as_ref().map_or(0, Vec::len),
            data.ports.len(),
            data.ambient.len()
        ),
    ];
    lines.extend(report);
    for (what, n) in left_out {
        lines.push(format!("not imported: {n} {what}"));
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definitions_and_colours() {
        let d = read_definitions("0;0;0;0;land;false;unknown;0\n1;230;81;119;lake;false;lakes;7\n2;0;0;55;land;false;forest;1\n4;0;0;232;sea;true;ocean;0\nbad line\n");
        assert_eq!(d.len(), 3);
        assert!(d[0].lake && d[0].kind == Kind::Lake);
        assert_eq!((d[1].id, d[1].rgb, d[1].terrain.as_str(), d[1].continent), (2, [0, 0, 55], "forest", 1));
        assert!(d[2].coastal && d[2].kind == Kind::Sea);
        let root = Script::parse("color = { 106 119 89 }\ncolor_ui = rgb { 1 2 3 }\nc2 = hsv { 0.0 1.0 1.0 }").root;
        let c = |k: &str| {
            let i = root.get(k).unwrap();
            colour(i.block().unwrap(), i)
        };
        assert_eq!(c("color"), Some([106, 119, 89]));
        assert_eq!(c("color_ui"), Some([1, 2, 3]));
        assert_eq!(c("c2"), Some([255, 0, 0]));
        assert!(is_date("1939.1.1") && !is_date("owner") && !is_date("1234"));
    }

    /// A mod with its own states (replace_path) over a tiny game.
    #[test]
    fn tiny_mod_import() {
        let tmp = std::env::temp_dir().join(format!("mh-hoi4import-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let (game, md, out) = (tmp.join("game"), tmp.join("mod"), tmp.join("out"));
        let put = |p: PathBuf, s: &str| {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, s).unwrap();
        };
        // 512 x 256: a land province left (x < 200), a second one in the
        // middle, sea right (x >= 400)
        let (w, h) = (512u32, 256u32);
        let colours = [[10u8, 20, 30], [40, 50, 60], [0, 0, 200]];
        let prov = |x: u32| if x < 200 { 0 } else if x < 400 { 1 } else { 2 };
        let mut rows = Vec::new();
        for _y in 0..h {
            let mut r = Vec::new();
            for x in 0..w {
                let c = colours[prov(x)];
                r.extend_from_slice(&c);
            }
            rows.push(r);
        }
        std::fs::create_dir_all(game.join("map")).unwrap();
        crate::modbuild::img::write_bmp24(&game.join("map/provinces.bmp"), w, h, &rows.concat()).unwrap();
        put(game.join("map/definition.csv"), "0;0;0;0;land;false;unknown;0\n1;10;20;30;land;false;hills;1\n2;40;50;60;land;false;plains;1\n3;0;0;200;sea;true;ocean;0\n");
        put(game.join("map/adjacencies.csv"), "From;To;Type;Through;start_x;start_y;stop_x;stop_y;adjacency_rule_name;Comment\n1;2;impassable;-1;-1;-1;-1;-1;;Wall\n-1;-1;;-1;-1;-1;-1;-1;-1\n");
        put(game.join("map/railways.txt"), "2 2 1 2\n");
        put(game.join("map/supply_nodes.txt"), "1 1\n");
        put(game.join("map/ambient_object.txt"), "type={\n\ttype=\"frame_border_entity\"\n\tobject={ name=\"f\" position={ 0 0 0 } rotation={ 0 0 0 } }\n}\ntype={\n\ttype=\"lighthouse_entity\"\n\tscale=2.0\n\tobject={ name=\"lh\" position={ 450 1 56 } rotation={ 0 90 0 } }\n}\n");
        put(game.join("map/strategicregions/1-Land.txt"), "strategic_region={ id=1 name=\"STRATEGICREGION_1\" provinces={ 1 2 } }\n");
        put(game.join("map/strategicregions/2-Sea.txt"), "strategic_region={ id=2 name=\"STRATEGICREGION_2\" provinces={ 3 } weather={ } }\n");
        put(game.join("history/states/1-Old.txt"), "state={ id=1 name=\"STATE_1\" provinces={ 1 2 } history={ owner = GER } }\n");
        put(game.join("common/country_tags/00_countries.txt"), "GER = \"countries/Germany.txt\"\n");
        put(game.join("common/countries/Germany.txt"), "color = { 1 2 3 }\n");
        put(game.join("localisation/english/map_l_english.yml"), "l_english:\n STRATEGICREGION_1:0 \"Heartland\"\n STATE_1:0 \"Old state\"\n VICTORY_POINTS_2:0 \"Midtown\"\n");
        // the mod replaces the states: two of them, owned by a new country
        put(md.join("descriptor.mod"), "name=\"Tiny\"\nversion=\"2.0\"\nreplace_path=\"history/states\"\n");
        put(md.join("history/states/1-West.txt"), "state={ id=1 name=\"STATE_1\" manpower=5000 state_category=city provinces={ 1 } history={ owner = TIN add_core_of = TIN victory_points = { 1 5 } buildings = { infrastructure = 3 1 = { naval_base = 2 } } 1939.1.1 = { owner = GER } } }\n");
        // East is another country's, also a core of Tiny (and of Germany, which owns nothing), held by Tiny
        put(md.join("history/states/2-East.txt"), "state={ id=2 name=\"STATE_2\" state_category=made_up provinces={ 2 } history={ owner = SEC add_core_of = SEC add_core_of = TIN add_core_of = GER controller = TIN } }\n");
        put(md.join("common/country_tags/01_tiny.txt"), "TIN = \"countries/Tiny.txt\"\n");
        put(md.join("common/countries/Tiny.txt"), "color = rgb { 200 100 50 }\n");
        put(md.join("history/countries/TIN - Tiny.txt"), "capital = 1\nset_politics = { ruling_party = communism }\n");
        put(md.join("localisation/english/tiny_l_english.yml"), "l_english:\n TIN:0 \"Tinyland\"\n STATE_1:0 \"West\"\n STATE_2:0 \"East\"\n");

        let src = Source { game: game.clone(), mod_dir: Some(md.clone()) };
        let meta = ModMeta { name: "Tiny import".into(), folder: "tiny_import".into(), version: String::new(), ..Default::default() };
        let proj = Projection::Plain { pixel_m: 5000.0 };
        let report = import(&src, &out, &meta, &proj, &Progress::default()).unwrap();
        let joined = report.join("\n");
        assert!(!joined.contains("not in definition.csv"), "{joined}");
        let l = model::load(&out, &Progress::default()).unwrap();
        let d = &l.data;
        assert_eq!((l.labels.w, l.labels.h), (w, h));
        assert_eq!(d.provinces.count(), 3, "{joined}");
        assert_eq!(d.provinces.get(1).unwrap().terrain.as_deref(), Some("hills"));
        assert_eq!(d.provinces.get(2).unwrap().name, "Midtown");
        assert_eq!(d.provinces.get(3).unwrap().kind, Kind::Sea);
        // the mod's states, not the game's
        assert_eq!(d.states.count(), 2);
        let west = d.states.get(1).unwrap();
        assert_eq!((west.name.as_str(), west.manpower, west.category.as_deref()), ("West", Some(5000), Some("city")));
        assert_eq!(west.victory_points, vec![[1, 5]]);
        assert_eq!(west.buildings.get("infrastructure"), Some(&3));
        assert_eq!(d.states.get(2).unwrap().category, None, "unknown categories are left to the build");
        assert_eq!(d.nations.count(), 2);
        let (tin, n) = d.nations.iter().find(|(_, n)| n.tag.as_deref() == Some("TIN")).unwrap();
        assert_eq!((n.name.as_str(), n.color, n.ideology.as_deref(), n.capital_state), ("Tinyland", Some([200, 100, 50]), Some("communism"), 1));
        // cores and occupation: the owner's alone is automatic; Germany owns no state
        let east = d.states.get(2).unwrap();
        let sec = east.nation.unwrap();
        assert_ne!(sec, tin);
        assert_eq!((west.cores.clone(), west.controller), (None, None));
        assert_eq!((east.cores.clone(), east.controller, east.occupier()), (Some(vec![sec, tin]), Some(tin), Some(tin)));
        assert!(joined.contains("cores of countries that own no state"), "{joined}");
        assert_eq!(d.regions.count(), 2);
        let r1 = d.regions.get(1).unwrap();
        assert_eq!((r1.name.as_str(), r1.naval), ("Heartland", false));
        assert!(d.regions.get(2).unwrap().naval);
        assert_eq!(d.adjacencies.len(), 1);
        assert_eq!(d.adjacencies[0].kind, "impassable");
        assert_eq!(d.railways.as_ref().unwrap().len(), 1);
        assert_eq!(d.supply_hubs.as_ref().unwrap().len(), 1);
        assert_eq!(d.ports.len(), 1);
        assert_eq!(d.ports[0].level, 2);
        assert_eq!(d.ambient.len(), 1);
        let a = &d.ambient[0];
        assert_eq!((a.entity.as_str(), a.at, a.lift, a.rotation[1], a.scale), ("lighthouse_entity", [450.0, 200.0], 1.0, -90.0, 2.0));
        assert!(d.map_frame());
        assert_eq!(d.mod_meta().version, "2.0");
        assert!((d.pixel_m - 5000.0).abs() < 1e-9);
        assert!(joined.contains("dated history"), "{joined}");
        assert!(out.join("provinces.wld").exists());
        // a game without the map is refused
        assert!(import(&Source { game: tmp.join("nothing"), mod_dir: None }, &tmp.join("o2"), &meta, &proj, &Progress::default()).is_err());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
