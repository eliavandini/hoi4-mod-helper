//! Checks of the map against HOI4's rules, shown in the editor with a
//! location to jump to.

use rustc_hash::{FxHashMap, FxHashSet};

use std::sync::Arc;

use super::density::Terrain;
use super::doc::Doc;
use super::model::{Id, Index, Kind, Labels, MapData, State, Stats};
use crate::modbuild::buildings::{self, Caps};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Clone, Debug)]
pub struct Issue {
    pub severity: Severity,
    pub msg: String,
    /// Pixel to zoom to.
    pub at: Option<[u32; 2]>,
    /// Province to select.
    pub prov: Option<Id>,
}

/// Copy of what the checks need, so they can run in the background while
/// editing goes on.
pub struct Snapshot {
    pub data: MapData,
    pub labels: Labels,
    pub stats: Stats,
    pub index: Index,
    pub w: u32,
    pub h: u32,
    /// For the built-up land of the states (their automatic category).
    pub terrain: Option<Arc<Terrain>>,
    /// What the game allows of buildings.
    pub caps: Arc<Caps>,
}

impl Snapshot {
    pub fn of(doc: &Doc) -> Snapshot {
        Snapshot {
            data: doc.data.clone(),
            labels: doc.labels.read().clone(),
            stats: doc.stats.clone(),
            index: doc.index.clone(),
            w: doc.w,
            h: doc.h,
            terrain: doc.terrain.clone(),
            caps: buildings::current(),
        }
    }
}

/// A state touches the sea (dockyards and naval bases need that).
pub fn on_coast(d: &MapData, stats: &Stats, provs: &[Id]) -> bool {
    provs.iter().any(|&p| stats.neighbours(p).iter().any(|&o| d.kind(o) == Kind::Sea))
}

/// The category a state has: its own, else the build's automatic one by
/// its built-up km² (None: the terrain is not loaded to tell).
pub fn category_of(s: &State, urban_km2: Option<f64>) -> Option<String> {
    match s.category.as_deref().filter(|c| !c.is_empty()) {
        Some(c) => Some(c.to_string()),
        None if s.impassable => Some("wasteland".into()),
        None => urban_km2.map(|u| crate::modbuild::auto_category(false, u).to_string()),
    }
}

/// Number of 4-connected parts per province.
pub fn parts_per_province(doc: &Snapshot) -> Vec<u32> {
    let lab = &doc.labels;
    let (w, h) = (lab.w as usize, lab.h as usize);
    let mut seen = vec![false; w * h];
    let mut parts = vec![0u32; doc.data.provinces.cap().max(1)];
    let mut stack = Vec::new();
    for s in 0..w * h {
        if seen[s] {
            continue;
        }
        let p = lab.px[s];
        if let Some(c) = parts.get_mut(p as usize) {
            *c += 1;
        }
        seen[s] = true;
        stack.push(s);
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            for (ok, j) in [(x > 0, i.wrapping_sub(1)), (x + 1 < w, i + 1), (y > 0, i.wrapping_sub(w)), (y + 1 < h, i + w)] {
                if ok && !seen[j] && lab.px[j] == p {
                    seen[j] = true;
                    stack.push(j);
                }
            }
        }
    }
    parts
}

/// Points where four provinces meet or that touch only diagonally.
pub fn crossings(doc: &Snapshot) -> Vec<[u32; 2]> {
    let lab = &doc.labels;
    let (w, h) = (lab.w as usize, lab.h as usize);
    let mut out = Vec::new();
    for y in 0..h - 1 {
        for x in 0..w - 1 {
            let i = y * w + x;
            let (a, b, c, d) = (lab.px[i], lab.px[i + 1], lab.px[i + w], lab.px[i + w + 1]);
            if (a != b && a != c && a != d && b != c && b != d && c != d) || (a == d && b == c && a != b) {
                out.push([x as u32 + 1, y as u32 + 1]);
            }
        }
    }
    out
}

pub fn validate(doc: &Snapshot) -> Vec<Issue> {
    let d = &doc.data;
    let mut out = Vec::new();
    let mut push = |severity, msg: String, prov: Option<Id>| {
        let at = prov.map(|p| {
            let c = doc.stats.get(p).centre();
            [c[0] as u32, c[1] as u32]
        });
        out.push(Issue { severity, msg, at, prov });
    };
    let (max_w, max_h) = (doc.w / 8, doc.h / 8);
    let parts = parts_per_province(doc);
    for (p, pr) in d.provinces.iter() {
        let s = doc.stats.get(p);
        let name = d.prov_name(p);
        if s.px < 8 {
            push(Severity::Error, format!("{name} ({p}) has {} px; HOI4 needs at least 8", s.px), Some(p));
        }
        if s.box_w() >= max_w || s.box_h() >= max_h {
            push(Severity::Error, format!("{name} ({p}) spans {} × {} px; HOI4 allows less than {max_w} × {max_h} (1/8 of the map)", s.box_w(), s.box_h()), Some(p));
        }
        if parts.get(p as usize).copied().unwrap_or(0) > 1 {
            push(Severity::Warning, format!("{name} ({p}) is in {} separate pieces", parts[p as usize]), Some(p));
        }
        match pr.kind {
            Kind::Land if pr.state == 0 => push(Severity::Error, format!("land province {name} ({p}) is in no state"), Some(p)),
            _ => {}
        }
        if pr.region == 0 {
            push(Severity::Error, format!("{name} ({p}) is in no strategic region"), Some(p));
        } else if let Some(r) = d.regions.get(pr.region) {
            if r.naval != (pr.kind == Kind::Sea) {
                push(Severity::Error, format!("{name} ({p}) is {} but in the {} zone {}", pr.kind_name(), if r.naval { "sea" } else { "air (land)" }, r.name), Some(p));
            }
        }
    }
    // land and sea touching only at a corner: "Fix crossings" never changes
    // the sea, and vanilla's map has 155 of them; the rest are map errors
    let (coast, x): (Vec<[u32; 2]>, Vec<[u32; 2]>) = crossings(doc).into_iter().partition(|&[x, y]| {
        let w = doc.labels.w as usize;
        let i = (y as usize - 1) * w + x as usize - 1;
        let (a, b) = (doc.labels.px[i], doc.labels.px[i + 1]);
        a == doc.labels.px[i + w + 1] && b == doc.labels.px[i + w] && ((d.kind(a) == Kind::Sea) != (d.kind(b) == Kind::Sea))
    });
    if !x.is_empty() {
        out.push(Issue {
            severity: Severity::Error,
            msg: format!("{} points where four provinces meet or provinces touch only diagonally (use \"Fix crossings\")", x.len()),
            at: Some(x[0]),
            prov: None,
        });
    }
    if !coast.is_empty() {
        out.push(Issue { severity: Severity::Warning, msg: format!("{} points where land and sea touch only diagonally", coast.len()), at: Some(coast[0]), prov: None });
    }
    // states
    let mut st_regions: FxHashMap<Id, FxHashSet<Id>> = FxHashMap::default();
    for (_, pr) in d.provinces.iter() {
        if pr.state != 0 {
            st_regions.entry(pr.state).or_default().insert(pr.region);
        }
    }
    let sealed = doc.terrain.as_ref().map(|t| super::doc::sealed_per_province(t, &doc.labels, d));
    for (s, st) in d.states.iter() {
        let provs = doc.index.provs_of_state(s);
        let first = provs.first().copied();
        if st.nation.is_none() {
            out.push(Issue { severity: Severity::Error, msg: format!("state {} ({s}) has no nation", st.name), at: None, prov: first });
        }
        if st_regions.get(&s).is_some_and(|r| r.len() > 1) {
            out.push(Issue { severity: Severity::Warning, msg: format!("state {} ({s}) lies in several strategic regions", st.name), at: None, prov: first });
        }
        // contiguity over province borders and straits
        if provs.len() > 1 {
            let set: FxHashSet<Id> = provs.iter().copied().collect();
            let mut links: FxHashMap<Id, Vec<Id>> = FxHashMap::default();
            for a in &d.adjacencies {
                if a.kind != "impassable" && set.contains(&a.from) && set.contains(&a.to) {
                    links.entry(a.from).or_default().push(a.to);
                    links.entry(a.to).or_default().push(a.from);
                }
            }
            let mut seen: FxHashSet<Id> = FxHashSet::default();
            let mut stack = vec![provs[0]];
            seen.insert(provs[0]);
            while let Some(v) = stack.pop() {
                for &q in doc.stats.neighbours(v).iter().chain(links.get(&v).into_iter().flatten()) {
                    if set.contains(&q) && seen.insert(q) {
                        stack.push(q);
                    }
                }
            }
            if seen.len() < provs.len() {
                let stray = provs.iter().copied().find(|p| !seen.contains(p));
                out.push(Issue { severity: Severity::Warning, msg: format!("state {} ({s}) is not contiguous", st.name), at: None, prov: stray });
            }
        }
        for v in &st.victory_points {
            if d.state_of(v[0]) != s {
                out.push(Issue { severity: Severity::Error, msg: format!("victory point of state {} is in a province outside it", st.name), at: None, prov: first });
            }
        }
        // what the game allows of its buildings
        let caps = &doc.caps;
        if let Some(c) = st.category.as_deref().filter(|c| !c.is_empty())
            && caps.slots(c).is_none()
        {
            out.push(Issue { severity: Severity::Error, msg: format!("state {} ({s}): the game has no state category {c}", st.name), at: None, prov: first });
        }
        let urban = sealed.as_ref().map(|u| provs.iter().map(|&p| u.get(p as usize).copied().unwrap_or(0.0)).sum::<f64>());
        let cat = category_of(st, urban).filter(|c| caps.slots(c).is_some());
        for p in caps.problems(&st.buildings, cat.as_deref(), on_coast(d, &doc.stats, provs)) {
            out.push(Issue { severity: Severity::Error, msg: format!("state {} ({s}): {p}", st.name), at: None, prov: first });
        }
        if let Some(p) = cat.as_deref().and_then(|c| caps.slot_problem(&st.buildings, c)) {
            out.push(Issue { severity: Severity::Warning, msg: format!("state {} ({s}): {p}", st.name), at: None, prov: first });
        }
    }
    // nations
    let mut tags: FxHashMap<String, Id> = FxHashMap::default();
    for (n, nat) in d.nations.iter() {
        let cap = d.states.get(nat.capital_state);
        if cap.is_none_or(|c| c.nation != Some(n) || c.impassable) {
            out.push(Issue {
                severity: Severity::Error,
                msg: format!("nation {} has no passable capital state", nat.name),
                at: None,
                prov: doc.index.states_of_nation(n).first().and_then(|&s| doc.index.provs_of_state(s).first().copied()),
            });
        }
        if let Some(o) = cap.and_then(|c| c.occupier()) {
            out.push(Issue {
                severity: Severity::Warning,
                msg: format!("the capital of {} is occupied by {}: it starts without its capital in its hands; pick another capital", nat.name, d.nation_name(o)),
                at: None,
                prov: doc.index.provs_of_state(nat.capital_state).first().copied(),
            });
        }
        if let Some(t) = &nat.tag {
            if t.len() != 3 || !t.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) || !t.starts_with(|c: char| c.is_ascii_uppercase()) {
                out.push(Issue { severity: Severity::Error, msg: format!("tag {t} of {} is not three capitals/digits starting with a letter", nat.name), at: None, prov: None });
            }
            if let Some(o) = tags.insert(t.clone(), n) {
                out.push(Issue { severity: Severity::Error, msg: format!("tag {t} is used by {} and {}", d.nation_name(o), nat.name), at: None, prov: None });
            }
        }
    }
    // regions
    for (r, reg) in d.regions.iter() {
        let provs = doc.index.provs_of_region(r);
        let set: FxHashSet<Id> = provs.iter().copied().collect();
        if let Some(&p0) = provs.first() {
            let mut seen: FxHashSet<Id> = FxHashSet::default();
            let mut stack = vec![p0];
            seen.insert(p0);
            while let Some(v) = stack.pop() {
                for &q in doc.stats.neighbours(v) {
                    if set.contains(&q) && seen.insert(q) {
                        stack.push(q);
                    }
                }
            }
            if seen.len() < provs.len() {
                let stray = provs.iter().copied().find(|p| !seen.contains(p));
                out.push(Issue { severity: Severity::Warning, msg: format!("{} zone {} is not contiguous", if reg.naval { "sea" } else { "air" }, reg.name), at: None, prov: stray });
            }
        }
    }
    // adjacencies
    let mut pairs: FxHashSet<(Id, Id, String)> = FxHashSet::default();
    for a in &d.adjacencies {
        let label = if a.name.is_empty() { format!("{} - {}", d.prov_name(a.from), d.prov_name(a.to)) } else { a.name.clone() };
        let touching = doc.stats.border_len(a.from, a.to) > 0;
        let key = (a.from.min(a.to), a.from.max(a.to), a.kind.clone());
        if !pairs.insert(key) {
            out.push(Issue { severity: Severity::Error, msg: format!("adjacency {label} is listed twice"), at: None, prov: Some(a.from) });
        }
        match a.kind.as_str() {
            "sea" if d.kind(a.from) == Kind::Sea && d.kind(a.to) == Kind::Sea => {
                // (vanilla's Kiel and Suez cross land that touches one of the seas)
                if !a.through.is_some_and(|t| d.kind(t) == Kind::Land) {
                    out.push(Issue { severity: Severity::Error, msg: format!("canal {label}: it must cross a land province"), at: None, prov: Some(a.from) });
                }
            }
            "sea" => {
                let ok = a.through.is_some_and(|t| d.kind(t) == Kind::Sea && doc.stats.border_len(a.from, t) > 0 && doc.stats.border_len(a.to, t) > 0);
                if !ok {
                    out.push(Issue { severity: Severity::Error, msg: format!("strait {label}: its sea province does not touch both ends"), at: None, prov: Some(a.from) });
                }
                if d.kind(a.from) != Kind::Land || d.kind(a.to) != Kind::Land {
                    out.push(Issue { severity: Severity::Error, msg: format!("strait {label}: both ends must be land"), at: None, prov: Some(a.from) });
                }
                if d.impassable(a.from) || d.impassable(a.to) {
                    out.push(Issue { severity: Severity::Warning, msg: format!("strait {label} ends in an impassable state"), at: None, prov: Some(a.from) });
                }
            }
            "impassable" if !touching => {
                out.push(Issue { severity: Severity::Error, msg: format!("impassable border {label}: the provinces do not touch"), at: None, prov: Some(a.from) });
            }
            _ => {}
        }
        if a.kind != "impassable" && touching {
            out.push(Issue { severity: Severity::Warning, msg: format!("adjacency {label}: the provinces touch already"), at: None, prov: Some(a.from) });
        }
    }
    // rivers and supply (what the build would leave out)
    let (w, h) = (doc.w as usize, doc.h as usize);
    let ras = super::rivers::rasterize(&d.rivers, w, h, super::supply::ground(d, &doc.labels));
    for p in &ras.problems {
        out.push(Issue { severity: Severity::Warning, msg: p.msg.clone(), at: Some(p.at), prov: None });
    }
    let plan = super::supply::plan(d, &doc.stats, &doc.labels, &Default::default());
    let px = |pt: [f32; 2]| Some([pt[0].max(0.0) as u32, pt[1].max(0.0) as u32]);
    for (ri, msg) in &plan.lines.faults {
        let at = d.railways.as_ref().and_then(|r| r.get(*ri)).and_then(|r| r.stops.first().copied()).and_then(px);
        out.push(Issue { severity: Severity::Warning, msg: format!("railway {}: {msg}", ri + 1), at, prov: None });
    }
    for &i in &plan.bad_hubs {
        let at = d.supply_hubs.as_ref().and_then(|h| h.get(i).copied()).and_then(px);
        out.push(Issue { severity: Severity::Warning, msg: format!("supply hub {} is not on passable land (left out)", i + 1), at, prov: None });
    }
    for &i in &plan.bad_ports {
        out.push(Issue { severity: Severity::Warning, msg: format!("port {} is not on a coast (left out)", i + 1), at: px(d.ports[i].at), prov: None });
    }
    out.sort_by(|a, b| a.severity.cmp(&b.severity));
    out
}
