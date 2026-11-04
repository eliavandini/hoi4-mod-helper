//! Checks of a built mod against the game's rules (formerly validate_mod.py).
//!
//! Bitmap sizes and formats, provinces.bmp against definition.csv, heightmap
//! water, states and strategic regions, building / unit / weather positions,
//! railways, adjacencies (straits, impassable borders, links and their rules),
//! "lake" provinces taken out of play, the countries (tags, country files,
//! colours, flags, history, order of battle, names, localisation, bookmark),
//! the tutorial's state and province ids, ambient objects (map frame),
//! localisation and the .mod file.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;

use anyhow::{Context as _, Result, bail};
use regex::Regex;

use super::img;
use super::vanilla::{files_with, read_text, txt_files};

fn re(s: &str) -> Regex {
    Regex::new(s).unwrap()
}

fn ints(s: &str) -> Vec<u32> {
    s.split_whitespace().filter_map(|x| x.parse().ok()).collect()
}

/// `*.yml` files below `dir`, sorted.
fn yml_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut v: Vec<_> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
    v.sort();
    for p in v {
        if p.is_dir() {
            yml_files(&p, out);
        } else if p.extension().is_some_and(|e| e == "yml") {
            out.push(p);
        }
    }
}

struct Checker<'a> {
    errors: Vec<String>,
    log: &'a dyn Fn(&str),
}

impl Checker<'_> {
    fn check(&mut self, ok: bool, msg: impl FnOnce() -> String) {
        if !ok {
            self.errors.push(msg());
        }
    }
}

/// Check the mod in `mod_dir`; logs what it finds and ends with "ALL CHECKS
/// PASSED" or the errors. Returns the errors (empty: all passed).
pub fn validate(mod_dir: &Path, game: &Path, log: &dyn Fn(&str)) -> Result<Vec<String>> {
    let mut c = Checker { errors: Vec::new(), log };
    run(&mut c, mod_dir, game).context("the mod is incomplete")?;
    if c.errors.is_empty() {
        log("ALL CHECKS PASSED");
    } else {
        for e in c.errors.iter().take(30) {
            log(&format!("ERROR: {e}"));
        }
        if c.errors.len() > 30 {
            log(&format!("... {} errors in all", c.errors.len()));
        }
    }
    Ok(c.errors)
}

fn run(c: &mut Checker, md: &Path, g: &Path) -> Result<()> {
    let log = c.log;
    let m = md.join("map");
    let vh = img::bmp_header(&g.join("map/provinces.bmp"))?;
    let (vw0, vh0) = (vh.w, vh.h);
    let ph = img::bmp_header(&m.join("provinces.bmp"))?;
    let (w, h) = (ph.w as usize, ph.h as usize);
    log(&format!("provinces.bmp {w}x{h} {}-bit comp {}", ph.bpp, ph.compression));
    c.check(ph.bpp == 24 && ph.compression == 0 && w % 256 == 0 && h % 256 == 0 && w * h <= 13_238_272, || "provinces.bmp format/size".into());
    for (name, want_bpp) in [("heightmap", 8), ("terrain", 8), ("rivers", 8), ("cities", 8), ("trees", 8), ("world_normal", 24)] {
        let b = img::bmp_header(&m.join(format!("{name}.bmp")))?;
        let v = img::bmp_header(&g.join(format!("map/{name}.bmp")))?;
        let exp = (
            (v.w as f64 * w as f64 / vw0 as f64).round_ties_even() as u32,
            (v.h as f64 * h as f64 / vh0 as f64).round_ties_even() as u32,
        );
        log(&format!("  {name}.bmp {}x{} {}-bit (vanilla {}x{} -> expected {}x{})", b.w, b.h, b.bpp, v.w, v.h, exp.0, exp.1));
        c.check(b.bpp == want_bpp && b.compression == 0 && (b.w, b.h) == exp, || format!("{name}.bmp format/size"));
    }
    let tp = img::read_bmp(&m.join("terrain.bmp"))?.palette;
    let vp = img::read_bmp(&g.join("map/terrain.bmp"))?.palette;
    let k = tp.len().min(vp.len());
    c.check(tp[..k] == vp[..k], || "terrain palette differs from vanilla".into());
    for f in files_with(&m.join("terrain"), "dds")? {
        let (dw, dh, mode) = img::dds_info(&f)?;
        log(&format!("  terrain/{} ({dw}, {dh}) {mode}", f.file_name().unwrap_or_default().to_string_lossy()));
    }

    // provinces
    let prov = img::read_bmp(&m.join("provinces.bmp"))?;
    if prov.bpp != 24 {
        bail!("provinces.bmp is not a 24-bit bitmap");
    }
    let def = read_text(&m.join("definition.csv"))?;
    let rows: Vec<Vec<&str>> = def.lines().map(|l| l.split(';').collect()).collect();
    if rows.len() < 2 || rows.iter().any(|r| r.len() < 8) {
        bail!("definition.csv: missing rows or fields");
    }
    c.check(rows[0][..5] == ["0", "0", "0", "0", "land"], || "definition line 0".into());
    let ids: Vec<u32> = rows[1..].iter().map(|r| r[0].parse().unwrap_or(0)).collect();
    let n = ids.len();
    c.check(ids == (1..=n as u32).collect::<Vec<_>>(), || "province ids not 1..n".into());
    let code = |r: &[&str]| -> u32 { r[1..4].iter().fold(0, |a, x| (a << 8) | x.parse::<u32>().unwrap_or(0)) };
    let cols: HashMap<u32, u32> = rows[1..].iter().map(|r| (code(r), r[0].parse().unwrap_or(0))).collect();
    c.check(cols.len() == n, || "duplicate colours in definition.csv".into());
    let mut pmap = vec![0u32; w * h];
    let mut used: HashSet<u32> = HashSet::new();
    let mut unknown = 0usize;
    for (i, px) in prov.data.as_chunks::<3>().0.iter().enumerate() {
        let k = ((px[0] as u32) << 16) | ((px[1] as u32) << 8) | px[2] as u32;
        used.insert(k);
        match cols.get(&k) {
            Some(&p) => pmap[i] = p,
            None => unknown += 1,
        }
    }
    let only_bmp = used.iter().filter(|k| !cols.contains_key(k)).count();
    let only_csv = cols.keys().filter(|k| !used.contains(k)).count();
    c.check(only_bmp == 0 && only_csv == 0, || format!("colour sets differ: {only_bmp} only in bmp, {only_csv} only in csv"));
    if unknown > 0 || ids != (1..=n as u32).collect::<Vec<_>>() {
        bail!("provinces.bmp and definition.csv do not match; cannot check further");
    }
    let mut typ: Vec<&str> = vec![""; n + 1];
    for r in &rows[1..] {
        typ[r[0].parse::<usize>().unwrap()] = r[4];
    }
    let mut terr: BTreeMap<&str, usize> = BTreeMap::new();
    for r in &rows[1..] {
        *terr.entry(r[6]).or_default() += 1;
    }
    let cats: BTreeSet<String> = re(r"(?m)^\t(\w+)\s*=\s*\{").captures_iter(&read_text(&g.join("common/terrain/00_terrain.txt"))?).map(|x| x[1].to_string()).collect();
    let unknown_t: Vec<&&str> = terr.keys().filter(|t| !cats.contains(**t)).collect();
    c.check(unknown_t.is_empty(), || format!("unknown terrain {unknown_t:?}"));
    for r in &rows[1..] {
        if r[4] == "lake" {
            c.check(r[5] == "false" && r[6] == "lakes" && r[7] == "0", || format!("lake province {}: {r:?}", r[0]));
        } else {
            c.check(matches!(r[4], "land" | "sea"), || format!("province {}: unknown type {}", r[0], r[4]));
        }
    }
    let is = |p: u32, t: &str| typ.get(p as usize).is_some_and(|x| *x == t);
    let hm = img::read_bmp(&m.join("heightmap.bmp"))?;
    let water_ok = hm.data.len() == pmap.len() && pmap.iter().zip(&hm.data).all(|(&p, &v)| if is(p, "sea") { v < 95 } else { v > 95 });
    c.check(water_ok, || "heightmap water does not match sea provinces".into());
    log(&format!(
        "definition.csv: {n} provinces, {} sea, {} lakes (out of play), terrain {terr:?}",
        typ.iter().filter(|t| **t == "sea").count(),
        typ.iter().filter(|t| **t == "lake").count()
    ));
    let mut nb: Vec<BTreeSet<u32>> = vec![BTreeSet::new(); n + 1];
    for y in 0..h {
        for x in 0..w {
            let a = pmap[y * w + x];
            for b in [if x + 1 < w { Some(pmap[y * w + x + 1]) } else { None }, if y + 1 < h { Some(pmap[(y + 1) * w + x]) } else { None }].into_iter().flatten() {
                if a != b {
                    nb[a as usize].insert(b);
                    nb[b as usize].insert(a);
                }
            }
        }
    }
    let touch = |a: u32, b: u32| nb.get(a as usize).is_some_and(|s| s.contains(&b));
    let prov_at = |x: f64, z: f64| -> u32 {
        let (r, col) = ((h as f64 - z) as i64, x as i64);
        if r < 0 || col < 0 || r >= h as i64 || col >= w as i64 { 0 } else { pmap[r as usize * w + col as usize] }
    };

    // states
    let re_id = re(r"\bid\s*=\s*(\d+)");
    let re_provs = re(r"provinces\s*=\s*\{([^}]*)\}");
    let re_slot = re(r"\b(industrial_complex|arms_factory|dockyard|synthetic_refinery|fuel_silo|rocket_site|nuclear_reactor)\s*=\s*(\d+)");
    let mut state_prov: HashMap<u32, u32> = HashMap::new();
    let mut owners: BTreeMap<u32, String> = BTreeMap::new();
    let mut cat_of: BTreeMap<u32, String> = BTreeMap::new();
    let mut blds: BTreeMap<u32, BTreeMap<String, u32>> = BTreeMap::new();
    let mut imp_states: HashSet<u32> = HashSet::new();
    for f in txt_files(&md.join("history/states"))? {
        let t = read_text(&f)?;
        let name = f.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let field = |r: &Regex| r.captures(&t).map(|x| x[1].to_string()).with_context(|| format!("{name}: no {}", r.as_str()));
        let sid: u32 = field(&re_id)?.parse()?;
        let provs = ints(&field(&re_provs)?);
        for &p in &provs {
            c.check(!state_prov.contains_key(&p), || format!("province {p} in two states"));
            state_prov.insert(p, sid);
        }
        owners.insert(sid, field(&re(r"owner\s*=\s*(\w+)"))?);
        cat_of.insert(sid, field(&re(r"state_category\s*=\s*(\w+)"))?);
        let mut b = BTreeMap::new();
        for x in re_slot.captures_iter(&t) {
            *b.entry(x[1].to_string()).or_default() += x[2].parse::<u32>()?;
        }
        blds.insert(sid, b);
        if let Some(v) = re(r"victory_points\s*=\s*\{\s*(\d+)").captures(&t) {
            let p: u32 = v[1].parse()?;
            c.check(provs.contains(&p), || format!("VP of state {sid} not in state"));
        }
        if t.contains("impassable = yes") {
            imp_states.insert(sid);
        }
    }
    let land: Vec<u32> = (1..=n as u32).filter(|&p| is(p, "land")).collect();
    let mut sp: Vec<u32> = state_prov.keys().copied().collect();
    sp.sort();
    c.check(sp == land, || "states do not cover exactly the land provinces".into());
    let owner_tags: BTreeSet<&String> = owners.values().collect();
    log(&format!("states: {} of {} owners, impassable {}", owners.len(), owner_tags.len(), imp_states.len()));

    // strategic regions
    let mut reg_prov: HashMap<u32, u32> = HashMap::new();
    for f in txt_files(&m.join("strategicregions"))? {
        let t = read_text(&f)?;
        let name = f.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let rid: u32 = re_id.captures(&t).with_context(|| format!("{name}: no id"))?[1].parse()?;
        let provs = ints(&re_provs.captures(&t).with_context(|| format!("{name}: no provinces"))?[1]);
        let naval = t.contains("naval_terrain");
        for p in provs {
            c.check(!reg_prov.contains_key(&p), || format!("province {p} in two regions"));
            reg_prov.insert(p, rid);
            c.check(is(p, "sea") == naval, || {
                format!("province {p} ({}) in {} region {rid}", typ.get(p as usize).unwrap_or(&"?"), if naval { "naval" } else { "land" })
            });
        }
    }
    let mut rp: Vec<u32> = reg_prov.keys().copied().collect();
    rp.sort();
    c.check(rp == (1..=n as u32).collect::<Vec<_>>(), || "regions do not cover all provinces".into());

    // positions
    let btxt = read_text(&m.join("buildings.txt"))?;
    let mut types: BTreeMap<String, usize> = BTreeMap::new();
    let mut per_state: HashMap<(u32, String), usize> = HashMap::new();
    let mut sn: HashMap<u32, usize> = HashMap::new();
    for l in btxt.lines() {
        let b: Vec<&str> = l.split(';').collect();
        if b.len() < 7 {
            c.check(false, || format!("buildings.txt: bad line {l:?}"));
            continue;
        }
        let (sid, t) = (b[0].parse::<u32>().unwrap_or(0), b[1]);
        let (x, z) = (b[2].parse::<f64>().unwrap_or(-1.0), b[4].parse::<f64>().unwrap_or(-1.0));
        let adj: u32 = b[6].parse().unwrap_or(0);
        *types.entry(t.to_string()).or_default() += 1;
        *per_state.entry((sid, t.to_string())).or_default() += 1;
        let p = prov_at(x, z);
        if t == "floating_harbor" {
            c.check(is(p, "sea") && p == adj, || format!("floating harbour at {x},{z}"));
        } else {
            c.check(is(p, "land") && state_prov.get(&p) == Some(&sid), || format!("{t} of state {sid} at {x},{z} lies in province {p} (state {:?})", state_prov.get(&p)));
        }
        if t == "naval_base_spawn" {
            c.check(is(adj, "sea") && touch(p, adj), || format!("naval base adj {adj}"));
        }
        if t == "supply_node" {
            *sn.entry(p).or_default() += 1;
        }
    }
    log(&format!("buildings: {types:?}"));
    for &sid in owners.keys() {
        for (t, k) in [("arms_factory", 6), ("industrial_complex", 6), ("anti_air_building", 3), ("air_base", 1)] {
            let have = per_state.get(&(sid, t.to_string())).copied().unwrap_or(0);
            c.check(have >= k, || format!("state {sid} has {have} {t}"));
        }
    }
    c.check(land.iter().all(|p| sn.get(p) == Some(&1)), || "supply_node positions missing/duplicated".into());
    let mut us: HashMap<u32, usize> = HashMap::new();
    for l in read_text(&m.join("unitstacks.txt"))?.lines() {
        let f: Vec<&str> = l.split(';').collect();
        if f.len() < 5 {
            c.check(false, || format!("unitstacks.txt: bad line {l:?}"));
            continue;
        }
        let p: u32 = f[0].parse().unwrap_or(0);
        *us.entry(p).or_default() += 1;
        if f[1] == "0" {
            let at = prov_at(f[2].parse().unwrap_or(-1.0), f[4].parse().unwrap_or(-1.0));
            c.check(at == p, || format!("unitstack of {p} outside it"));
        }
    }
    c.check(
        (1..=n as u32).all(|p| us.get(&p).copied().unwrap_or(0) == if is(p, "lake") { 0 } else { 39 }),
        || "unitstacks count (39 per province, none for lakes)".into(),
    );
    let mut wp: HashMap<u32, usize> = HashMap::new();
    for l in read_text(&m.join("weatherpositions.txt"))?.lines() {
        *wp.entry(l.split(';').next().unwrap_or("").parse().unwrap_or(0)).or_default() += 1;
    }
    let regions: HashSet<u32> = reg_prov.values().copied().collect();
    c.check(wp.keys().copied().collect::<HashSet<_>>() == regions && wp.values().all(|&v| v == 2), || "weatherpositions".into());
    for l in read_text(&m.join("supply_nodes.txt"))?.lines() {
        let p: u32 = l.split_whitespace().nth(1).and_then(|x| x.parse().ok()).unwrap_or(0);
        c.check(is(p, "land"), || "supply node on sea".into());
    }

    // ambient objects: vanilla's frame (3 entities) is made for its map size; the
    // Great Wall belongs to vanilla's China; ocean sounds over the sea
    let amb = m.join("ambient_object.txt");
    if amb.exists() {
        let at = read_text(&amb)?;
        let atypes: Vec<String> = re(r#"(?m)^\ttype\s*=\s*"(\w+)""#).captures_iter(&at).map(|x| x[1].to_string()).collect();
        let frame: Vec<&String> = atypes.iter().filter(|t| t.starts_with("frame_border")).collect();
        c.check(frame.is_empty() || frame.len() == 3, || format!("ambient_object.txt: incomplete map frame {frame:?}"));
        c.check(!atypes.iter().any(|t| t == "chinese_wall_entity"), || "ambient_object.txt: the Great Wall belongs to the vanilla map".into());
        c.check(at.matches('{').count() == at.matches('}').count(), || "ambient_object.txt: unbalanced braces".into());
        for x in re(r#"name="ambient_water"\s*position=\{\s*([-\d.]+) [-\d.]+ ([-\d.]+)"#).captures_iter(&at) {
            let (px, pz): (f64, f64) = (x[1].parse().unwrap_or(-1.0), x[2].parse().unwrap_or(-1.0));
            c.check(px >= 0.0 && px < w as f64 && pz > 0.0 && pz <= h as f64 && is(prov_at(px, pz), "sea"), || format!("ocean sound at {px},{pz} is not over sea"));
        }
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for t in &atypes {
            *counts.entry(t).or_default() += 1;
        }
        let size_note = if !frame.is_empty() && (w as u32, h as u32) != (vw0, vh0) { format!(" (made for {vw0}x{vh0}, this map is {w}x{h})") } else { String::new() };
        log(&format!("ambient objects: {counts:?}, map frame {}{size_note}", if frame.is_empty() { "off" } else { "on" }));
    } else {
        log(&format!("ambient objects: vanilla's (frame made for {vw0}x{vh0}, Great Wall included)"));
    }

    // adjacencies.csv: straits (type sea) from land to land through a sea province,
    // impassable borders between touching provinces, links ("" type) between
    // provinces that do not touch
    let araw = std::fs::read(m.join("adjacencies.csv")).context("cannot read adjacencies.csv")?;
    c.check(araw.is_ascii(), || "adjacencies.csv is not ASCII".into());
    let atext = String::from_utf8_lossy(&araw);
    let alines: Vec<&str> = atext.split("\r\n").collect();
    c.check(alines[0].starts_with("From;To;Type;Through;start_x;start_y;stop_x;stop_y;adjacency_rule_name;Comment"), || "adjacencies header".into());
    let k = alines.len();
    c.check(k >= 3 && alines[k - 2] == "-1;-1;;-1;-1;-1;-1;-1;-1" && alines[k - 1].is_empty(), || "adjacencies terminator".into());
    let rules_txt = if m.join("adjacency_rules.txt").exists() { read_text(&m.join("adjacency_rules.txt"))? } else { String::new() };
    let rule_names: HashSet<String> = re(r#"\bname\s*=\s*"?(\w+)"?"#).captures_iter(&rules_txt).map(|x| x[1].to_string()).collect();
    let mut straits: HashSet<(u32, u32)> = HashSet::new();
    let mut links: HashSet<(u32, u32)> = HashSet::new();
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    let mut seen_adj: HashSet<(u32, u32, String)> = HashSet::new();
    let pr = |a: u32, b: u32| (a.min(b), a.max(b));
    for l in alines.iter().take(k.saturating_sub(2)).skip(1) {
        let f: Vec<&str> = l.split(';').collect();
        if f.len() != 10 {
            c.check(false, || format!("adjacency {l}: {} fields", f.len()));
            continue;
        }
        let num = |i: usize| f[i].parse::<i64>().unwrap_or(-9);
        let (a, b, t, th, rule, cm) = (num(0), num(1), f[2], num(3), f[8], f[9]);
        let (sx, sy, ex, ey) = (num(4), num(5), num(6), num(7));
        *kinds.entry(if t.is_empty() { "link".into() } else { t.to_string() }).or_default() += 1;
        let ok_ids = (1..=n as i64).contains(&a) && (1..=n as i64).contains(&b) && a != b;
        c.check(ok_ids, || format!("adjacency {l}: bad provinces"));
        c.check(rule.is_empty() || rule_names.contains(rule), || format!("adjacency {cm}: rule {rule} not in adjacency_rules.txt"));
        if !ok_ids {
            continue;
        }
        let (a, b) = (a as u32, b as u32);
        c.check(seen_adj.insert((a, b, t.to_string())), || format!("adjacency {cm}: duplicate"));
        match t {
            "sea" => {
                let th = if th > 0 { th as u32 } else { 0 };
                c.check(is(a, "land") && is(b, "land") && is(th, "sea"), || format!("strait {l}: types"));
                c.check(!touch(a, b), || format!("strait {cm}: {a} and {b} already touch"));
                c.check(touch(th, a) && touch(th, b), || format!("strait {cm}: through {th} does not touch both"));
                let imp = |p: u32| state_prov.get(&p).is_some_and(|s| imp_states.contains(s));
                c.check(!imp(a) && !imp(b), || format!("strait {cm}: impassable end"));
                let at = |x: i64, y: i64| -> u32 {
                    let r = h as i64 - 1 - y;
                    if x < 0 || r < 0 || x >= w as i64 || r >= h as i64 { 0 } else { pmap[r as usize * w + x as usize] }
                };
                c.check(at(sx, sy) == a && at(ex, ey) == b, || format!("strait {cm}: start/stop not in from/to"));
                c.check(straits.insert(pr(a, b)), || format!("strait {cm}: duplicate pair"));
            }
            "impassable" => c.check(touch(a, b), || format!("impassable border {cm}: {a} and {b} do not touch")),
            "" => {
                c.check(!is(a, "lake") && !is(b, "lake"), || format!("link {cm}: ends in a lake"));
                if is(a, "land") && is(b, "land") {
                    links.insert(pr(a, b));
                }
            }
            _ => c.check(false, || format!("adjacency {cm}: unknown type {t}")),
        }
    }
    log(&format!("adjacencies: {kinds:?}"));
    let rtxt = read_text(&m.join("railways.txt"))?;
    let (mut rail_straits, mut nrails, mut rail_len) = (0usize, 0usize, 0usize);
    for l in rtxt.lines() {
        let r: Vec<u32> = ints(l);
        if r.len() < 4 {
            c.check(false, || format!("railway {l:?}: too short"));
            continue;
        }
        nrails += 1;
        c.check(r[1] as usize == r.len() - 2, || "railway count".into());
        let path = &r[2..];
        rail_len += path.len();
        let head = || format!("{:?}..", &path[..3.min(path.len())]);
        c.check(path.windows(2).all(|x| touch(x[0], x[1]) || straits.contains(&pr(x[0], x[1])) || links.contains(&pr(x[0], x[1]))), || format!("railway {} not adjacent", head()));
        c.check(
            path.iter().all(|p| is(*p, "land") && state_prov.get(p).is_some_and(|s| !imp_states.contains(s))),
            || format!("railway {} through impassable land", head()),
        );
        rail_straits += path.windows(2).filter(|x| straits.contains(&pr(x[0], x[1]))).count();
    }
    log(&format!("railway steps over straits: {rail_straits}"));
    log(&format!("railways: {nrails}, avg {:.1} provinces", rail_len as f64 / nrails.max(1) as f64));
    let mut loc_keys: HashSet<String> = HashSet::new();
    let mut ymls = Vec::new();
    yml_files(&md.join("localisation"), &mut ymls);
    let re_key = re(r"(?m)^ ([\w.]+):");
    for f in ymls {
        let b = std::fs::read(&f)?;
        c.check(b.starts_with(b"\xef\xbb\xbfl_english:"), || format!("localisation BOM/header {}", f.file_name().unwrap_or_default().to_string_lossy()));
        let t = String::from_utf8_lossy(&b);
        loc_keys.extend(re_key.captures_iter(&t).map(|x| x[1].to_string()));
    }
    log(&format!("localisation keys: {}", loc_keys.len()));

    // countries
    let mut slots: HashMap<String, u32> = HashMap::new();
    for f in txt_files(&g.join("common/state_category"))? {
        let t = read_text(&f)?;
        let k = re(r"local_building_slots\s*=\s*(\d+)").captures(&t).and_then(|x| x[1].parse().ok()).unwrap_or(0);
        slots.insert(f.file_stem().unwrap_or_default().to_string_lossy().into_owned(), k);
    }
    for (sid, cat) in &cat_of {
        c.check(slots.contains_key(cat), || format!("state {sid}: unknown state_category {cat}"));
        let used: u32 = blds[sid].values().sum();
        let have = slots.get(cat).copied().unwrap_or(0);
        c.check(used <= have, || format!("state {sid}: {:?} over the {have} slots of {cat}", blds[sid]));
    }
    let tags_in = |dir: &Path| -> Result<BTreeMap<String, (String, String)>> {
        let r = re(r#"(?m)^\s*([A-Z][A-Z0-9]{2})\s*=\s*"([^"]+)""#);
        let mut out = BTreeMap::new();
        for f in txt_files(dir)? {
            let fname = f.file_name().unwrap_or_default().to_string_lossy().into_owned();
            for x in r.captures_iter(&read_text(&f)?) {
                out.insert(x[1].to_string(), (fname.clone(), x[2].to_string()));
            }
        }
        Ok(out)
    };
    let (vtags, mtags) = (tags_in(&g.join("common/country_tags"))?, tags_in(&md.join("common/country_tags"))?);
    let both: Vec<&String> = mtags.keys().filter(|t| vtags.contains_key(*t)).take(10).collect();
    c.check(both.is_empty(), || format!("tags already in vanilla: {both:?}"));
    c.check(
        mtags.values().all(|(f, _)| f.as_str() < "zz_dynamic_countries.txt"),
        || "country_tags file sorts after zz_dynamic_countries.txt (tags would be dynamic)".into(),
    );
    let colours = read_text(&md.join("common/countries/colors.txt"))?;
    let mut names_txt = String::new();
    for f in txt_files(&md.join("common/names"))? {
        names_txt += &read_text(&f)?;
    }
    let mut hist: HashMap<String, std::path::PathBuf> = HashMap::new();
    for f in txt_files(&md.join("history/countries"))? {
        let name = f.file_name().unwrap_or_default().to_string_lossy().into_owned();
        hist.insert(name.chars().take(3).collect(), f);
    }
    let mut owned: HashMap<&str, HashSet<u32>> = HashMap::new();
    for (sid, t) in &owners {
        owned.entry(t).or_default().insert(*sid);
    }
    let re_oob_tmpl = re(r#"division_template\s*=\s*\{\s*name\s*=\s*"([^"]+)""#);
    let re_div = re(r#"location\s*=\s*(\d+)\s*division_template\s*=\s*"([^"]+)""#);
    for t in &owner_tags {
        let t = t.as_str();
        c.check(mtags.contains_key(t) || vtags.contains_key(t), || format!("{t}: owner tag not defined"));
        let Some((_, path)) = mtags.get(t) else { continue };
        c.check(md.join("common").join(path).exists(), || format!("{t}: country file {path} missing"));
        let entry = re(&format!(r"(?m)^{t}\s*=\s*\{{"));
        c.check(entry.is_match(&colours), || format!("{t}: no colors.txt entry"));
        c.check(entry.is_match(&names_txt), || format!("{t}: no name list"));
        for (sub, size) in [("", (82, 52)), ("medium/", (41, 26)), ("small/", (10, 7))] {
            let fl = md.join(format!("gfx/flags/{sub}{t}.tga"));
            let ok = img::tga_info(&fl).is_ok_and(|(fw, fh, bpp)| (fw, fh) == size && bpp == 32);
            c.check(ok, || format!("{t}: flag {sub}{t}.tga"));
        }
        c.check([t.to_string(), format!("{t}_DEF"), format!("{t}_ADJ")].iter().all(|k| loc_keys.contains(k)), || format!("{t}: country name not localised"));
        let Some(hf) = hist.get(t) else {
            c.check(false, || format!("{t}: no history/countries file"));
            continue;
        };
        let ht = read_text(hf)?;
        let cap: u32 = re(r"(?m)^capital\s*=\s*(\d+)").captures(&ht).and_then(|x| x[1].parse().ok()).unwrap_or(0);
        let cap_ok = owned[t].contains(&cap) && !imp_states.contains(&cap);
        c.check(cap_ok, || format!("{t}: capital {cap} not a passable state of its own"));
        if let Some(oob) = re(r#"(?m)^oob\s*=\s*"(\w+)""#).captures(&ht) {
            let ot = read_text(&md.join(format!("history/units/{}.txt", &oob[1])))?;
            let templates: HashSet<String> = re_oob_tmpl.captures_iter(&ot).map(|x| x[1].to_string()).collect();
            for x in re_div.captures_iter(&ot) {
                let p: u32 = x[1].parse().unwrap_or(0);
                c.check(state_prov.get(&p).is_some_and(|s| owned[t].contains(s)), || format!("{t}: division in province {p} it does not own"));
                c.check(templates.contains(&x[2]), || format!("{t}: unknown template {}", &x[2]));
            }
        }
    }
    for f in txt_files(&md.join("common/bookmarks"))? {
        let bt = read_text(&f)?;
        let dc = re(r#"default_country\s*=\s*"(\w+)""#).captures(&bt).map(|x| x[1].to_string()).unwrap_or_default();
        c.check(owner_tags.contains(&dc), || format!("bookmark default country {dc} owns no state"));
        let listed_ok = re(r"(?m)^\t\t([A-Z][A-Z0-9]{2})\s*=\s*\{").captures_iter(&bt).all(|x| owner_tags.contains(&x[1].to_string()));
        c.check(listed_ok, || "bookmark lists a country without states".into());
    }
    let mut cat_counts: BTreeMap<&str, usize> = BTreeMap::new();
    for cat in cat_of.values() {
        *cat_counts.entry(cat).or_default() += 1;
    }
    log(&format!("countries: {} new tags ({} vanilla), {} own states, categories {cat_counts:?}", mtags.len(), vtags.len(), owner_tags.len()));

    // tutorial (the vanilla one names state 550 and vanilla provinces: crash at game start)
    let tut = md.join("tutorial/tutorial.txt");
    c.check(tut.exists(), || "mod has no tutorial/tutorial.txt; the vanilla one crashes the game".into());
    if tut.exists() {
        let tt = read_text(&tut)?;
        let bad_state = |x: &str| x.parse::<u32>().map_or(true, |v| !owners.contains_key(&v));
        let mut bad: Vec<String> = re(r"\bstate\s*=\s*(\d+)").captures_iter(&tt).map(|x| x[1].to_string()).filter(|x| bad_state(x)).collect();
        for x in re(r"highlight_states_trigger\s*=\s*\{([^}]*)\}").captures_iter(&tt) {
            bad.extend(x[1].split_whitespace().filter(|v| bad_state(v)).map(String::from));
        }
        let mut provs_t: Vec<String> = re(r"(?:\btarget|direction_pointer)\s*=\s*(\d+)").captures_iter(&tt).map(|x| x[1].to_string()).collect();
        for x in re(r"highlight_provinces\s*=\s*\{([^}]*)\}").captures_iter(&tt) {
            provs_t.extend(x[1].split_whitespace().map(String::from));
        }
        bad.extend(provs_t.into_iter().filter(|x| x.parse::<u64>().map_or(true, |v| v < 1 || v > n as u64)));
        c.check(bad.is_empty(), || format!("tutorial names ids not on this map: {bad:?}"));
        if bad.is_empty() {
            log(&format!("tutorial: {} steps, ids ok", re(r"\btutorial\s*=").find_iter(&tt).count()));
        } else {
            log("tutorial: BAD");
        }
    }
    let name = md.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let mod_file = md.parent().unwrap_or(md).join(format!("{name}.mod"));
    let txt = read_text(&mod_file)?;
    c.check(txt.contains(&format!("path=\"{}\"", md.display())), || ".mod path".into());
    Ok(())
}
