//! Supply: railways, supply hubs and naval bases (ports) as the game gets
//! them, and an estimate of the supply the game will give each province.
//!
//! Railways, hubs and ports are kept as map points ([`Railway`], [`Port`]) so
//! they survive saving, re-cutting and merging provinces; [`Net::route`] lays
//! a railway between its stops the way the game's builder does (the shortest
//! way over passable land), here and in the mod build alike.
//!
//! The estimate follows the game's defines (NSupply in common/defines): supply
//! starts at the capital (5.0), at supply hubs connected to it by railway or
//! river (2.8, +0.34 per rail level of the weakest link on the way) and at
//! naval bases (3.3) once one of them is connected; on each province it
//! travels it loses a penalty that starts small and grows with the distance
//! (0.5 + 0.7 per province for hubs), more in rough terrain and over rivers,
//! less with infrastructure. Supply from several sources adds up; 1.0 or more
//! fully supplies a division in the province.

use std::collections::{BTreeMap, BTreeSet};

use rayon::prelude::*;
use rustc_hash::FxHashSet;

use super::model::{Id, Kind, Labels, MapData, PStat, Port, Railway, Stats};
use crate::modbuild::terrain::{LandKind, TerrainTypes};
use super::rivers::Raster;
use crate::modbuild::rail;

pub const MAX_RAIL_LEVEL: u8 = 5;
/// Naval base levels the game allows.
pub const MAX_PORT_LEVEL: u8 = 10;
/// Extra cost (px) for a railway over a strait or water link.
const CROSSING_COST: f64 = 50.0;

pub fn pair(a: Id, b: Id) -> (Id, Id) {
    (a.min(b), a.max(b))
}

/// The first item with the largest key.
fn first_max<T: Copy, K: PartialOrd>(items: impl IntoIterator<Item = T>, key: impl Fn(T) -> K) -> Option<T> {
    let mut best: Option<(T, K)> = None;
    for it in items {
        let k = key(it);
        if best.as_ref().is_none_or(|b| k > b.1) {
            best = Some((it, k));
        }
    }
    best.map(|b| b.0)
}

#[derive(PartialEq)]
struct Item(f64, usize);
impl Eq for Item {}
impl PartialOrd for Item {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Item {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        o.0.total_cmp(&self.0).then(o.1.cmp(&self.1)) // min-heap
    }
}

/// The land supply and railways can cross: passable land provinces and the
/// links between them.
pub struct Net {
    /// Mean pixel position of each province (by id).
    pub centre: Vec<[f64; 2]>,
    pub passable: Vec<bool>,
    /// Links of each passable province (by id): (province, cost in px).
    pub adj: Vec<Vec<(usize, f64)>>,
}

impl Net {
    /// `nb`: neighbours of each province by id; `blocked`: impassable borders
    /// (key [`pair`]); `crossings`: straits and links over water.
    pub fn new(centre: Vec<[f64; 2]>, passable: Vec<bool>, nb: &[Vec<Id>], blocked: &FxHashSet<(Id, Id)>, crossings: &[(Id, Id)]) -> Net {
        let n = centre.len();
        let ok = |p: Id| (p as usize) < n && passable[p as usize];
        let mut links: Vec<BTreeMap<Id, f64>> = vec![BTreeMap::new(); n];
        for p in 0..n as Id {
            if !ok(p) {
                continue;
            }
            for &q in nb.get(p as usize).into_iter().flatten() {
                if q != p && ok(q) && !blocked.contains(&pair(p, q)) {
                    links[p as usize].insert(q, 1.0);
                }
            }
        }
        for &(a, b) in crossings {
            if a != b && ok(a) && ok(b) {
                links[a as usize].entry(b).or_insert(CROSSING_COST);
                links[b as usize].entry(a).or_insert(CROSSING_COST);
            }
        }
        let dist = |p: usize, q: usize| (centre[p][0] - centre[q][0]).hypot(centre[p][1] - centre[q][1]);
        let adj = links.iter().enumerate().map(|(p, l)| l.iter().map(|(&q, &extra)| (q as usize, dist(p, q as usize) + extra)).collect()).collect();
        Net { centre, passable, adj }
    }

    pub fn ok(&self, p: Id) -> bool {
        self.passable.get(p as usize).copied().unwrap_or(false)
    }

    /// Shortest ways from `a` to everywhere: (distance, previous province) by id.
    #[cfg(test)]
    fn from(&self, a: Id) -> (Vec<f64>, Vec<usize>) {
        let n = self.adj.len();
        let mut dist = vec![f64::INFINITY; n];
        let mut pred = vec![rail::NONE; n];
        let mut heap = std::collections::BinaryHeap::new();
        dist[a as usize] = 0.0;
        heap.push(Item(0.0, a as usize));
        while let Some(Item(d, u)) = heap.pop() {
            if d > dist[u] {
                continue;
            }
            for &(v, w) in &self.adj[u] {
                let nd = d + w;
                if nd < dist[v] {
                    dist[v] = nd;
                    pred[v] = u;
                    heap.push(Item(nd, v));
                }
            }
        }
        (dist, pred)
    }

    /// The provinces a railway from `a` to `b` runs through: the shortest way
    /// over passable land (straits cost extra).
    pub fn route(&self, a: Id, b: Id) -> Option<Vec<Id>> {
        if !self.ok(a) || !self.ok(b) {
            return None;
        }
        if a == b {
            return Some(vec![a]);
        }
        // Dijkstra (ties: the lower id first), stopping at b
        let n = self.adj.len();
        let mut dist = vec![f64::INFINITY; n];
        let mut pred = vec![rail::NONE; n];
        let mut heap = std::collections::BinaryHeap::new();
        dist[a as usize] = 0.0;
        heap.push(Item(0.0, a as usize));
        while let Some(Item(d, u)) = heap.pop() {
            if d > dist[u] {
                continue;
            }
            if u == b as usize {
                break;
            }
            for &(v, w) in &self.adj[u] {
                let nd = d + w;
                if nd < dist[v] {
                    dist[v] = nd;
                    pred[v] = u;
                    heap.push(Item(nd, v));
                }
            }
        }
        Self::back(&dist, &pred, a, b)
    }

    /// Shortest distances from `a` to each of `to` (infinite where there is
    /// no way), searching no further than needed.
    pub fn dist_to(&self, a: Id, to: &[Id]) -> Vec<f64> {
        let n = self.adj.len();
        if !self.ok(a) {
            return vec![f64::INFINITY; to.len()];
        }
        let mut dist = vec![f64::INFINITY; n];
        let mut left: FxHashSet<usize> = to.iter().map(|&t| t as usize).collect();
        let mut heap = std::collections::BinaryHeap::new();
        dist[a as usize] = 0.0;
        heap.push(Item(0.0, a as usize));
        while let Some(Item(d, u)) = heap.pop() {
            if d > dist[u] {
                continue;
            }
            left.remove(&u);
            if left.is_empty() {
                break;
            }
            for &(v, w) in &self.adj[u] {
                let nd = d + w;
                if nd < dist[v] {
                    dist[v] = nd;
                    heap.push(Item(nd, v));
                }
            }
        }
        to.iter().map(|&t| dist[t as usize]).collect()
    }

    fn back(d: &[f64], pred: &[usize], a: Id, b: Id) -> Option<Vec<Id>> {
        if !d[b as usize].is_finite() {
            return None;
        }
        let mut out = vec![b];
        let mut node = b as usize;
        while node != a as usize {
            node = pred[node];
            out.push(node as Id);
        }
        out.reverse();
        Some(out)
    }
}

/// A point inside province `p`: its pixel nearest to its mean position.
pub fn inside_point(labels: &[Id], w: usize, st: &PStat, p: Id) -> [f32; 2] {
    if st.px == 0 {
        return [0.0, 0.0];
    }
    let [cx, cy] = st.centre();
    let mut best = (f64::INFINITY, [cx as f32, cy as f32]);
    for y in st.y0..=st.y1 {
        for x in st.x0..=st.x1 {
            if labels[y as usize * w + x as usize] == p {
                let d = (x as f64 + 0.5 - cx).powi(2) + (y as f64 + 0.5 - cy).powi(2);
                if d < best.0 {
                    best = (d, [x as f32 + 0.5, y as f32 + 0.5]);
                }
            }
        }
    }
    best.1
}

// ------------------------------------------------------------------ the map's

/// The net of a map: land provinces in states that are not impassable, linked
/// where they touch (not over impassable borders) and over straits.
pub fn net_of(d: &MapData, stats: &Stats) -> Net {
    let cap = d.provinces.cap().max(stats.prov.len());
    let mut centre = vec![[0.0; 2]; cap];
    let mut passable = vec![false; cap];
    for (id, p) in d.provinces.iter() {
        centre[id as usize] = stats.get(id).centre();
        passable[id as usize] = p.kind == Kind::Land && stats.get(id).px > 0 && d.states.get(p.state).is_some_and(|s| !s.impassable);
    }
    let blocked: FxHashSet<(Id, Id)> = d.adjacencies.iter().filter(|a| a.kind == "impassable").map(|a| pair(a.from, a.to)).collect();
    let crossings: Vec<(Id, Id)> = d.adjacencies.iter().filter(|a| a.kind == "sea" || a.kind.is_empty()).map(|a| (a.from, a.to)).collect();
    Net::new(centre, passable, &stats.nb, &blocked, &crossings)
}

/// Land next to the sea (where a naval base can be).
pub fn coastal(d: &MapData, stats: &Stats, p: Id) -> bool {
    d.kind(p) == Kind::Land && stats.neighbours(p).iter().any(|&q| d.kind(q) == Kind::Sea)
}

/// Victory points of each state: as set in the editor, else (passable states)
/// the most built-up province. `urban_km2` by province id.
pub fn state_vps(d: &MapData, stats: &Stats, urban_km2: &[f64]) -> BTreeMap<Id, Vec<(Id, u32)>> {
    let mut provs: BTreeMap<Id, Vec<Id>> = BTreeMap::new();
    for (id, p) in d.provinces.iter() {
        if p.state != 0 {
            provs.entry(p.state).or_default().push(id);
        }
    }
    let size = |p: Id| stats.get(p).px as u64;
    let mut out = BTreeMap::new();
    for (sid, s) in d.states.iter() {
        if !s.victory_points.is_empty() {
            out.insert(sid, s.victory_points.iter().map(|v| (v[0], v[1])).collect());
        } else if !s.impassable
            && let Some(v) = auto_vp(provs.get(&sid).map_or(&[][..], |v| v), |p| urban_km2.get(p as usize).copied().unwrap_or(0.0), size)
        {
            out.insert(sid, vec![v]);
        }
    }
    out
}

/// The main victory point of each passable state.
pub fn main_vps(d: &MapData, stats: &Stats, vps: &BTreeMap<Id, Vec<(Id, u32)>>) -> BTreeMap<Id, Id> {
    vps.iter().filter(|(s, _)| d.states.get(**s).is_some_and(|s| !s.impassable)).filter_map(|(&s, l)| Some((s, main_vp(l, |p| stats.get(p).px as u64)?))).collect()
}

/// Railways, hubs and ports of a map as the build writes them.
pub struct Plan {
    pub net: Net,
    /// Railways set in the editor (else the automatic ones).
    pub custom_rail: bool,
    pub lines: Lines,
    /// Province pairs of the automatic railways (to make them editable).
    pub auto_links: Vec<(Id, Id)>,
    pub custom_hubs: bool,
    pub hubs: Vec<Id>,
    /// Hubs (index in supply_hubs) not on passable land.
    pub bad_hubs: Vec<usize>,
    pub ports: BTreeMap<Id, u8>,
    /// Ports (index in ports) not on a coast.
    pub bad_ports: Vec<usize>,
}

/// The province at a map point (0 off the map).
pub fn at_point(labels: &Labels, pt: [f32; 2]) -> Id {
    let (x, y) = (pt[0].floor(), pt[1].floor());
    if !(x >= 0.0 && y >= 0.0 && (x as u32) < labels.w && (y as u32) < labels.h) { 0 } else { labels.at(x as u32, y as u32) }
}

/// `main`: [`main_vps`] (the automatic hubs, and what the automatic railways join).
pub fn plan(d: &MapData, stats: &Stats, labels: &Labels, main: &BTreeMap<Id, Id>) -> Plan {
    let net = net_of(d, stats);
    let at = |pt: [f32; 2]| at_point(labels, pt);
    let (lines, auto) = match &d.railways {
        Some(r) => (lay(&net, r, at), Vec::new()),
        None => {
            let state_of: Vec<Id> = (0..net.adj.len() as Id).map(|p| d.state_of(p)).collect();
            let links = auto_links(&net, main, &state_of);
            (lay_provs(&net, &links.iter().map(|&(a, b)| (1, vec![a, b])).collect::<Vec<_>>()), links)
        }
    };
    let (hubs, bad_hubs) = match &d.supply_hubs {
        Some(pts) => hubs_at(&net, pts, at),
        None => (main.values().copied().collect(), Vec::new()),
    };
    let (ports, bad_ports) = ports_at(&d.ports, at, |p| coastal(d, stats, p));
    Plan { net, custom_rail: d.railways.is_some(), lines, auto_links: auto, custom_hubs: d.supply_hubs.is_some(), hubs, bad_hubs, ports, bad_ports }
}

// ------------------------------------------------------------------ automatic

/// The victory point the build makes the main one of a state with no victory
/// points set: its most built-up province (value: 1 per 2 km² sealed land,
/// 1 ..= 10).
pub fn auto_vp(provs: &[Id], urban_km2: impl Fn(Id) -> f64, size: impl Fn(Id) -> u64) -> Option<(Id, u32)> {
    let best = first_max(provs.iter().copied(), |p| (urban_km2(p), size(p)))?;
    Some((best, (urban_km2(best) / 2.0).round_ties_even().clamp(1.0, 10.0) as u32))
}

/// The main victory point of a state (capital, unit and supply location):
/// the highest, then the biggest province.
pub fn main_vp(vps: &[(Id, u32)], size: impl Fn(Id) -> u64) -> Option<Id> {
    first_max(vps.iter().copied(), |(p, v)| (v, size(p))).map(|x| x.0)
}

/// The railway links the build lays when none are set: a minimum spanning tree
/// over neighbouring states (`hub`: main province of each passable state),
/// each link the shortest way between their main provinces. Pairs of hub
/// provinces, to be laid with [`Net::route`].
pub fn auto_links(net: &Net, hub: &BTreeMap<Id, Id>, state_of: &[Id]) -> Vec<(Id, Id)> {
    let states: Vec<Id> = hub.keys().copied().collect();
    let sidx: BTreeMap<Id, usize> = states.iter().enumerate().map(|(i, &s)| (s, i)).collect();
    let src: Vec<Id> = states.iter().map(|s| hub[s]).collect();
    let mut state_nb: BTreeSet<(Id, Id)> = BTreeSet::new();
    for (p, l) in net.adj.iter().enumerate() {
        for &(q, _) in l {
            let (a, b) = (state_of.get(p).copied().unwrap_or(0), state_of.get(q).copied().unwrap_or(0));
            if a != b && a != 0 && b != 0 {
                state_nb.insert(pair(a, b));
            }
        }
    }
    // each state's distances to its neighbours' hubs
    let mut nbs: Vec<Vec<usize>> = vec![Vec::new(); states.len()];
    for &(s1, s2) in &state_nb {
        if let (Some(&i), Some(&j)) = (sidx.get(&s1), sidx.get(&s2)) {
            nbs[i].push(j);
        }
    }
    let dists: Vec<Vec<f64>> = (0..states.len()).into_par_iter().map(|i| net.dist_to(src[i], &nbs[i].iter().map(|&j| src[j]).collect::<Vec<_>>())).collect();
    let mut edges: Vec<(usize, usize, f64)> = Vec::new();
    for (i, js) in nbs.iter().enumerate() {
        for (k, &j) in js.iter().enumerate() {
            let d = dists[i][k];
            if d.is_finite() {
                edges.push((i, j, d + 1e-3));
            }
        }
    }
    rail::spanning_tree(states.len(), &edges).into_iter().map(|(i, j)| (src[i], src[j])).collect()
}

// ------------------------------------------------------------------ resolving

/// Railways laid through provinces.
#[derive(Clone, Debug, Default)]
pub struct Lines {
    /// The pieces of each railway (consecutive provinces linked), as far as
    /// they could be laid.
    pub pieces: Vec<Vec<Vec<Id>>>,
    /// (railway, message) where a railway could not be laid as set.
    pub faults: Vec<(usize, String)>,
    /// Rail level of each link (key [`pair`]): the highest railway on it.
    pub level: BTreeMap<(Id, Id), u8>,
}

impl Lines {
    /// railways.txt: one line per run of links of one level.
    pub fn railways_txt(&self) -> (String, usize) {
        let mut out = String::new();
        let mut count = 0;
        for lv in 1..=MAX_RAIL_LEVEL {
            let edges: BTreeSet<(Id, Id)> = self.level.iter().filter(|e| *e.1 == lv).map(|e| *e.0).collect();
            for c in rail::rail_chains(&edges) {
                out += &format!("{lv} {} {}\n", c.len(), c.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(" "));
                count += 1;
            }
        }
        (out, count)
    }
}

/// Lay railways given as stop provinces: (level, stops).
pub fn lay_provs(net: &Net, rails: &[(u8, Vec<Id>)]) -> Lines {
    let mut out = Lines::default();
    for (ri, (level, stops)) in rails.iter().enumerate() {
        let level = (*level).clamp(1, MAX_RAIL_LEVEL);
        let mut pieces: Vec<Vec<Id>> = Vec::new();
        let mut cur: Vec<Id> = Vec::new();
        let mut last: Option<Id> = None;
        for (k, &p) in stops.iter().enumerate() {
            if !net.ok(p) {
                out.faults.push((ri, format!("stop {} is not on passable land", k + 1)));
                continue;
            }
            match last {
                None => cur.push(p),
                Some(l) if l == p => {}
                Some(l) => match net.route(l, p) {
                    Some(path) => cur.extend_from_slice(&path[1..]),
                    None => {
                        out.faults.push((ri, format!("no way over land from stop {k} to stop {}", k + 1)));
                        if cur.len() > 1 {
                            pieces.push(std::mem::take(&mut cur));
                        }
                        cur = vec![p];
                    }
                },
            }
            last = Some(p);
        }
        if cur.len() > 1 {
            pieces.push(cur);
        }
        if pieces.is_empty() && !stops.is_empty() && out.faults.last().is_none_or(|f| f.0 != ri) {
            out.faults.push((ri, "needs two stops in different provinces".into()));
        }
        for piece in &pieces {
            for s in piece.windows(2) {
                let e = out.level.entry(pair(s[0], s[1])).or_default();
                *e = (*e).max(level);
            }
        }
        out.pieces.push(pieces);
    }
    out
}

/// Lay railways given as map points; `at` gives the province at a point.
pub fn lay(net: &Net, rails: &[Railway], at: impl Fn([f32; 2]) -> Id) -> Lines {
    let provs: Vec<(u8, Vec<Id>)> = rails.iter().map(|r| (r.level, r.stops.iter().map(|&s| at(s)).collect())).collect();
    lay_provs(net, &provs)
}

/// Hub provinces from map points (in order, each once) and the points that
/// are not on passable land.
pub fn hubs_at(net: &Net, pts: &[[f32; 2]], at: impl Fn([f32; 2]) -> Id) -> (Vec<Id>, Vec<usize>) {
    let mut seen = FxHashSet::default();
    let (mut out, mut bad) = (Vec::new(), Vec::new());
    for (i, &pt) in pts.iter().enumerate() {
        let p = at(pt);
        if !net.ok(p) {
            bad.push(i);
        } else if seen.insert(p) {
            out.push(p);
        }
    }
    (out, bad)
}

/// Port provinces with their level (the highest set in a province) and the
/// ports not on a coast (`coastal`: land next to the sea).
pub fn ports_at(ports: &[Port], at: impl Fn([f32; 2]) -> Id, coastal: impl Fn(Id) -> bool) -> (BTreeMap<Id, u8>, Vec<usize>) {
    let mut out: BTreeMap<Id, u8> = BTreeMap::new();
    let mut bad = Vec::new();
    for (i, pt) in ports.iter().enumerate() {
        let p = at(pt.at);
        if !coastal(p) {
            bad.push(i);
            continue;
        }
        let e = out.entry(p).or_default();
        *e = (*e).max(pt.level.clamp(1, MAX_PORT_LEVEL));
    }
    (out, bad)
}

// ------------------------------------------------------------------ rivers

/// How rivers lie between provinces.
#[derive(Clone, Debug, Default)]
pub struct RiverLinks {
    /// A river flows from one province into the other (key [`pair`]):
    /// supply hubs connect along it like a level-1 railway.
    pub along: FxHashSet<(Id, Id)>,
    /// A river runs along their border without flowing from one into the
    /// other: crossing it costs supply.
    pub across: FxHashSet<(Id, Id)>,
}

pub fn river_links(r: &Raster, labels: &[Id], w: usize, h: usize) -> RiverLinks {
    let on: FxHashSet<u32> = r.px.iter().map(|p| p.0).collect();
    let mut out = RiverLinks::default();
    let mut touch: rustc_hash::FxHashMap<(Id, Id), u32> = Default::default();
    for &(i, _, _) in &r.px {
        let (x, y) = (i as usize % w, i as usize / w);
        let p = labels[i as usize];
        let mut look = |j: usize| {
            let q = labels[j];
            if q != p {
                if on.contains(&(j as u32)) {
                    out.along.insert(pair(p, q));
                } else {
                    *touch.entry(pair(p, q)).or_default() += 1;
                }
            }
        };
        if x > 0 {
            look(i as usize - 1);
        }
        if x + 1 < w {
            look(i as usize + 1);
        }
        if y > 0 {
            look(i as usize - w);
        }
        if y + 1 < h {
            look(i as usize + w);
        }
    }
    out.across = touch.into_iter().filter(|(k, c)| *c >= 2 && !out.along.contains(k)).map(|(k, _)| k).collect();
    out
}

// ------------------------------------------------------------------ editor view

/// Ground under each pixel for rivers (by the province's kind).
pub fn ground(d: &MapData, labels: &Labels) -> impl Fn(usize) -> super::rivers::Ground {
    let kinds: Vec<Kind> = (0..d.provinces.cap() as Id).map(|p| d.kind(p)).collect();
    let px = labels.px.clone();
    move |i| match kinds.get(px[i] as usize) {
        Some(Kind::Sea) => super::rivers::Ground::Sea,
        Some(Kind::Lake) => super::rivers::Ground::Lake,
        _ => super::rivers::Ground::Land,
    }
}

/// Pixels of each kind of land of terrain rules per province (see
/// [`TerrainTypes::count`]), as the build counts them.
pub fn land_counts(labels: &Labels, t: &super::density::Terrain, kinds: &[LandKind], cap: usize) -> Vec<u64> {
    TerrainTypes::count(kinds, &labels.px, &t.cover, &t.dem, &t.slope_px, cap)
}

/// HOI4 terrain of each province as the build sets it (by id), and the rule
/// that set it (None: set on the province, sea, lake, map edge, or no rule
/// fits). `counts`: [`land_counts`] with the kinds of `types`.
pub fn province_terrain(d: &MapData, stats: &Stats, types: &TerrainTypes, counts: &[u64]) -> (Vec<String>, Vec<Option<usize>>) {
    let cap = d.provinces.cap();
    let k = types.kinds().len();
    let mut terrain = vec![String::new(); cap];
    let mut rule = vec![None; cap];
    for (p, pr) in d.provinces.iter() {
        let i = p as usize;
        terrain[i] = match pr.kind {
            Kind::Sea => "ocean".into(),
            Kind::Lake => "lakes".into(),
            Kind::Land => match pr.terrain.as_ref().filter(|t| !t.is_empty()) {
                Some(t) => t.clone(),
                None if pr.edge => "mountain".into(),
                None => {
                    let c = counts.get(i * k..(i + 1) * k).unwrap_or(&[]);
                    rule[i] = types.rule_of(c, stats.get(p).px as u64);
                    rule[i].map_or(types.other.as_str(), |r| types.rules[r].terrain.as_str()).to_string()
                }
            },
        };
    }
    (terrain, rule)
}

/// What the editor shows about rivers and supply.
pub struct View {
    pub plan: Plan,
    pub links: RiverLinks,
    /// Capital province of each nation (by nation id; 0: none).
    pub capital: Vec<Id>,
    /// HOI4 terrain of each province (empty without the land cover).
    pub terrain: Vec<String>,
    /// Infrastructure of each province (its state's).
    pub infra: Vec<u8>,
}

/// `urban_km2`: built-up km² by province (empty without the land cover);
/// `terrain`: [`province_terrain`].
pub fn view(d: &MapData, stats: &Stats, labels: &Labels, urban_km2: &[f64], terrain: Vec<String>) -> View {
    let (w, h) = (labels.w as usize, labels.h as usize);
    let rivers = super::rivers::rasterize(&d.rivers, w, h, ground(d, labels));
    let links = river_links(&rivers, &labels.px, w, h);
    let vps = state_vps(d, stats, urban_km2);
    let main = main_vps(d, stats, &vps);
    let plan = plan(d, stats, labels, &main);
    let mut capital = vec![0; d.nations.cap()];
    for (n, nat) in d.nations.iter() {
        if d.states.get(nat.capital_state).is_some_and(|s| s.nation == Some(n)) {
            capital[n as usize] = main.get(&nat.capital_state).copied().unwrap_or(0);
        }
    }
    let mut state_urban: BTreeMap<Id, f64> = BTreeMap::new();
    for (p, pr) in d.provinces.iter() {
        *state_urban.entry(pr.state).or_default() += urban_km2.get(p as usize).copied().unwrap_or(0.0);
    }
    let infra = (0..d.provinces.cap() as Id)
        .map(|p| {
            let Some(s) = d.states.get(d.state_of(p)) else { return 0 };
            let v = s.buildings.get("infrastructure").copied().unwrap_or_else(|| crate::modbuild::default_infrastructure(s.impassable, state_urban.get(&d.state_of(p)).copied().unwrap_or(0.0)));
            v.min(255) as u8
        })
        .collect();
    View { plan, links, capital, terrain, infra }
}

impl View {
    pub fn estimate(&self, d: &MapData, scope: Scope) -> Estimate {
        let n = self.plan.net.adj.len();
        let world = World {
            net: &self.plan.net,
            nation: (0..n as Id).map(|p| d.nation_of(p)).collect(),
            capital: self.capital.clone(),
            terrain: (0..n).map(|p| self.terrain.get(p).map_or(0.0, |t| terrain_penalty(t))).collect(),
            infra: (0..n).map(|p| self.infra.get(p).copied().unwrap_or(0)).collect(),
            rivers: &self.links,
            lines: &self.plan.lines,
            hubs: &self.plan.hubs,
            ports: &self.plan.ports,
        };
        estimate(&world, scope)
    }
}

// ------------------------------------------------------------------ estimate

/// supply_flow_penalty_factor of a terrain (common/terrain/00_terrain.txt).
pub fn terrain_penalty(t: &str) -> f32 {
    match t {
        "forest" => 0.08,
        "hills" => 0.1,
        "mountain" => 0.12,
        "urban" => -0.2,
        "jungle" | "marsh" => 0.16,
        "desert" => 0.14,
        _ => 0.0,
    }
}

/// A supply source: (initial flow, penalty at the first province, added per
/// province after).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Source(pub f32, pub f32, pub f32);
pub const CAPITAL: Source = Source(5.0, 0.5, 1.2);
pub const NODE: Source = Source(2.8, 0.5, 0.7);
pub const NAVAL: Source = Source(3.3, 0.84, 1.1);
pub const NODE_FLOW_BONUS_PER_RAIL_LEVEL: f32 = 0.34;
const DROP_REDUCTION_AT_MAX_INFRA: f32 = 0.3;
const RIVER_PENALTY: f32 = 0.2;
const THRESHOLD: f32 = 0.1;
const MAX_INFRA: f32 = 5.0;

/// How much of the terrain penalty applies `d` provinces from the source
/// (the game's logistics curve).
fn terrain_scale(d: f32) -> f32 {
    (0.9 / (1.0 + (-1.3 * (d - 2.3)).exp())).max(0.25)
}

/// What the estimate runs on (all by province id).
pub struct World<'a> {
    pub net: &'a Net,
    /// Nation owning each province (0: none).
    pub nation: Vec<Id>,
    /// Capital province of each nation (by nation id; 0: none).
    pub capital: Vec<Id>,
    /// [`terrain_penalty`] of each province.
    pub terrain: Vec<f32>,
    pub infra: Vec<u8>,
    pub rivers: &'a RiverLinks,
    pub lines: &'a Lines,
    pub hubs: &'a [Id],
    pub ports: &'a BTreeMap<Id, u8>,
}

/// Who supplies whom.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Scope {
    /// Every nation from its own capital, in its own land.
    EachNation,
    /// All the land as one country with the capital of this nation.
    OneCountry(Id),
}

#[derive(Clone, Debug, Default)]
pub struct Estimate {
    /// Supply of each province.
    pub flow: Vec<f32>,
    /// Each hub (as in World::hubs): its starting flow, None if it is not
    /// connected to its capital.
    pub hubs: Vec<Option<f32>>,
    /// Ports that get supply (connected to their capital by rail, or by sea
    /// through another one that is).
    pub ports: BTreeMap<Id, bool>,
}

pub fn estimate(wd: &World, scope: Scope) -> Estimate {
    let n = wd.net.adj.len();
    let land = |p: usize| wd.net.passable[p];
    let owner = |p: usize| -> Id {
        match scope {
            Scope::EachNation => wd.nation.get(p).copied().unwrap_or(0),
            Scope::OneCountry(_) => 1,
        }
    };
    // rail and river links
    let mut links: Vec<Vec<(usize, u8)>> = vec![Vec::new(); n];
    let mut add = |a: Id, b: Id, l: u8| {
        let (a, b) = (a as usize, b as usize);
        if a < n && b < n {
            links[a].push((b, l));
            links[b].push((a, l));
        }
    };
    for (&(a, b), &l) in &wd.lines.level {
        add(a, b, l);
    }
    for &(a, b) in &wd.rivers.along {
        add(a, b, 1);
    }
    let countries: Vec<(Id, Id)> = match scope {
        Scope::EachNation => wd.capital.iter().enumerate().filter(|(_, c)| **c != 0).map(|(n, &c)| (n as Id, c)).collect(),
        Scope::OneCountry(nat) => wd.capital.get(nat as usize).filter(|c| **c != 0).map(|&c| vec![(1, c)]).unwrap_or_default(),
    };
    let mut est = Estimate { flow: vec![0.0; n], hubs: vec![None; wd.hubs.len()], ports: wd.ports.keys().map(|&p| (p, false)).collect() };
    for (country, cap) in countries {
        let cap = cap as usize;
        if cap >= n || !land(cap) || owner(cap) != country {
            continue;
        }
        let mine = |p: usize| land(p) && owner(p) == country;
        // weakest rail level on the best way from the origins (widest path)
        let widest = |origins: &[usize]| -> Vec<u8> {
            let mut best = vec![0u8; n];
            let mut heap = std::collections::BinaryHeap::new();
            for &o in origins {
                best[o] = MAX_RAIL_LEVEL;
                heap.push((MAX_RAIL_LEVEL, std::cmp::Reverse(o)));
            }
            while let Some((l, std::cmp::Reverse(p))) = heap.pop() {
                if l < best[p] {
                    continue;
                }
                for &(q, lq) in &links[p] {
                    let v = l.min(lq);
                    if mine(q) && v > best[q] {
                        best[q] = v;
                        heap.push((v, std::cmp::Reverse(q)));
                    }
                }
            }
            best
        };
        let mut origins = vec![cap];
        let my_ports: Vec<usize> = wd.ports.keys().map(|&p| p as usize).filter(|&p| p < n && mine(p)).collect();
        let by_rail = widest(&origins);
        let naval = my_ports.iter().any(|&p| by_rail[p] > 0);
        if naval {
            origins.extend(my_ports.iter().copied());
            for &p in &my_ports {
                est.ports.insert(p as Id, true);
            }
        }
        let reach = if naval { widest(&origins) } else { by_rail };
        let mut sources: Vec<(usize, Source)> = vec![(cap, CAPITAL)];
        if naval {
            sources.extend(my_ports.iter().filter(|&&p| p != cap).map(|&p| (p, NAVAL)));
        }
        for (k, &h) in wd.hubs.iter().enumerate() {
            let h = h as usize;
            if h >= n || !mine(h) || reach[h] == 0 {
                continue;
            }
            let f0 = NODE.0 + NODE_FLOW_BONUS_PER_RAIL_LEVEL * reach[h] as f32;
            est.hubs[k] = Some(f0);
            if h != cap {
                sources.push((h, Source(f0, NODE.1, NODE.2)));
            }
        }
        let parts: Vec<Vec<(usize, f32)>> = sources.par_iter().map(|&(s, src)| spread(wd, s, src, &mine)).collect();
        for part in parts {
            for (p, f) in part {
                est.flow[p] += f;
            }
        }
    }
    est
}

/// The supply one source gives each province it reaches.
fn spread(wd: &World, s: usize, src: Source, mine: &dyn Fn(usize) -> bool) -> Vec<(usize, f32)> {
    let mut best: rustc_hash::FxHashMap<usize, f32> = Default::default();
    best.insert(s, src.0);
    let mut frontier = vec![(s, src.0)];
    let mut k = 1u32;
    while !frontier.is_empty() {
        let base = src.1 + src.2 * (k - 1) as f32;
        let ts = terrain_scale(k as f32);
        let mut next = Vec::new();
        for (p, f) in frontier {
            for &(q, _) in &wd.net.adj[p] {
                if !mine(q) {
                    continue;
                }
                let infra = wd.infra.get(q).copied().unwrap_or(0) as f32;
                let mut pen = base * (1.0 + wd.terrain.get(q).copied().unwrap_or(0.0) * ts) * (1.0 - DROP_REDUCTION_AT_MAX_INFRA * infra.min(MAX_INFRA) / MAX_INFRA);
                if wd.rivers.across.contains(&pair(p as Id, q as Id)) {
                    pen *= 1.0 + RIVER_PENALTY;
                }
                let fq = f - pen.max(0.0);
                if fq > THRESHOLD && best.get(&q).is_none_or(|&b| fq > b) {
                    best.insert(q, fq);
                    next.push((q, fq));
                }
            }
        }
        frontier = next;
        k += 1;
    }
    best.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A row of `n` provinces 1..=n, 10 px apart.
    fn row(n: usize) -> Net {
        let centre: Vec<[f64; 2]> = (0..=n).map(|i| [i as f64 * 10.0, 0.0]).collect();
        let passable: Vec<bool> = (0..=n).map(|i| i > 0).collect();
        let nb: Vec<Vec<Id>> = (0..=n as Id).map(|i| if i == 0 { vec![] } else { [i - 1, i + 1].into_iter().filter(|&q| q >= 1 && q <= n as Id).collect() }).collect();
        Net::new(centre, passable, &nb, &FxHashSet::default(), &[])
    }

    #[test]
    fn routes_and_levels() {
        let net = row(8);
        assert_eq!(net.route(2, 5), Some(vec![2, 3, 4, 5]));
        let l = lay_provs(&net, &[(2, vec![1, 4]), (5, vec![3, 6]), (1, vec![0, 8])]);
        assert_eq!(l.level[&(1, 2)], 2);
        assert_eq!(l.level[&(3, 4)], 5, "the higher railway counts");
        assert_eq!(l.level[&(5, 6)], 5);
        assert!(l.faults.iter().any(|f| f.0 == 2), "{:?}", l.faults);
        let (txt, count) = l.railways_txt();
        assert_eq!(count, 2, "{txt}");
        assert!(txt.contains("2 3 1 2 3\n") && txt.contains("5 4 3 4 5 6\n"), "{txt}");
    }

    #[test]
    fn route_stops_early_with_the_same_way() {
        // a grid with equal costs: many ways tie
        let (gw, gh) = (12usize, 9usize);
        let id = |x: usize, y: usize| (y * gw + x + 1) as Id;
        let n = gw * gh + 1;
        let mut centre = vec![[0.0; 2]; n];
        let mut nb = vec![Vec::new(); n];
        for y in 0..gh {
            for x in 0..gw {
                centre[id(x, y) as usize] = [x as f64 * 10.0, y as f64 * 10.0];
                for (dx, dy) in [(1i64, 0i64), (-1, 0), (0, 1), (0, -1)] {
                    let (a, b) = (x as i64 + dx, y as i64 + dy);
                    if a >= 0 && b >= 0 && (a as usize) < gw && (b as usize) < gh {
                        nb[id(x, y) as usize].push(id(a as usize, b as usize));
                    }
                }
            }
        }
        let net = Net::new(centre, (0..n).map(|i| i > 0).collect(), &nb, &FxHashSet::default(), &[]);
        for (a, b) in [(1, n as Id - 1), (5, 70), (100, 3), (13, 14)] {
            let (d, pred) = net.from(a);
            assert_eq!(net.route(a, b), Net::back(&d, &pred, a, b), "{a} to {b}");
        }
    }

    #[test]
    fn blocked_borders_cut_the_way() {
        let centre: Vec<[f64; 2]> = (0..=4).map(|i| [i as f64 * 10.0, 0.0]).collect();
        let nb = vec![vec![], vec![2], vec![1, 3], vec![2, 4], vec![3]];
        let blocked: FxHashSet<(Id, Id)> = [(2, 3)].into_iter().collect();
        let net = Net::new(centre.clone(), vec![false, true, true, true, true], &nb, &blocked, &[]);
        assert_eq!(net.route(1, 4), None);
        // a strait gets round it
        let net = Net::new(centre, vec![false, true, true, true, true], &nb, &blocked, &[(1, 4)]);
        assert_eq!(net.route(1, 4), Some(vec![1, 4]));
    }

    #[test]
    fn spanning_tree_links_neighbouring_states() {
        let net = row(9);
        // states of three provinces each, hubs in the middle
        let state_of: Vec<Id> = (0..=9).map(|i| if i == 0 { 0 } else { (i as Id - 1) / 3 + 1 }).collect();
        let hub: BTreeMap<Id, Id> = [(1, 2), (2, 5), (3, 8)].into_iter().collect();
        let links = auto_links(&net, &hub, &state_of);
        assert_eq!(links.len(), 2);
        let l = lay_provs(&net, &links.iter().map(|&(a, b)| (1, vec![a, b])).collect::<Vec<_>>());
        assert_eq!(l.level.len(), 6, "2..=8 joined: {:?}", l.level);
    }

    fn world<'a>(net: &'a Net, rivers: &'a RiverLinks, lines: &'a Lines, hubs: &'a [Id], ports: &'a BTreeMap<Id, u8>, owner: Vec<Id>) -> World<'a> {
        let n = net.adj.len();
        World { net, nation: owner, capital: vec![0, 1], terrain: vec![0.0; n], infra: vec![0; n], rivers, lines, hubs, ports }
    }

    #[test]
    fn supply_falls_off_and_hubs_need_a_railway() {
        let net = row(20);
        let rivers = RiverLinks::default();
        let none = Lines::default();
        let ports = BTreeMap::new();
        let hubs = [15];
        let est = estimate(&world(&net, &rivers, &none, &hubs, &ports, vec![1; 21]), Scope::EachNation);
        assert_eq!(est.flow[1], 5.0);
        assert!((est.flow[2] - 4.5).abs() < 1e-5, "{}", est.flow[2]);
        assert!((est.flow[3] - 2.8).abs() < 1e-5, "minus 0.5 + 1.2: {}", est.flow[3]);
        assert_eq!(est.flow[15], 0.0, "the hub is not connected");
        assert_eq!(est.hubs, vec![None]);
        let lines = lay_provs(&net, &[(3, vec![1, 15])]);
        let est = estimate(&world(&net, &rivers, &lines, &hubs, &ports, vec![1; 21]), Scope::EachNation);
        let f0 = NODE.0 + 3.0 * NODE_FLOW_BONUS_PER_RAIL_LEVEL;
        assert_eq!(est.hubs, vec![Some(f0)]);
        assert!((est.flow[15] - f0).abs() < 1e-5);
        assert!(est.flow[16] > 2.0 && est.flow[16] < f0);
        // terrain and infrastructure
        let mut wd = world(&net, &rivers, &lines, &hubs, &ports, vec![1; 21]);
        wd.terrain[16] = 0.12;
        let rough = estimate(&wd, Scope::EachNation).flow[16];
        wd.terrain[16] = 0.0;
        wd.infra[16] = 5;
        let built = estimate(&wd, Scope::EachNation).flow[16];
        assert!(rough < est.flow[16] && built > est.flow[16], "{rough} {} {built}", est.flow[16]);
    }

    #[test]
    fn rivers_connect_and_cost() {
        let net = row(10);
        let mut rivers = RiverLinks::default();
        for i in 1..6 {
            rivers.along.insert((i, i + 1));
        }
        let none = Lines::default();
        let ports = BTreeMap::new();
        let hubs = [6];
        let est = estimate(&world(&net, &rivers, &none, &hubs, &ports, vec![1; 11]), Scope::EachNation);
        assert_eq!(est.hubs, vec![Some(NODE.0 + NODE_FLOW_BONUS_PER_RAIL_LEVEL)], "a river is a level-1 railway");
        let mut across = RiverLinks::default();
        across.across.insert((1, 2));
        let a = estimate(&world(&net, &across, &none, &[], &ports, vec![1; 11]), Scope::EachNation);
        assert!((a.flow[2] - (5.0 - 0.5 * 1.2)).abs() < 1e-5, "{}", a.flow[2]);
    }

    #[test]
    fn each_nation_stays_home_one_country_does_not() {
        let net = row(10);
        let rivers = RiverLinks::default();
        let lines = lay_provs(&net, &[(1, vec![1, 9])]);
        let ports = BTreeMap::new();
        let hubs = [9];
        // provinces 6.. belong to nation 2 without a capital
        let owner: Vec<Id> = (0..=10).map(|i| if i <= 5 { 1 } else { 2 }).collect();
        let wd = world(&net, &rivers, &lines, &hubs, &ports, owner);
        let each = estimate(&wd, Scope::EachNation);
        assert_eq!(each.flow[6], 0.0);
        assert_eq!(each.hubs, vec![None]);
        let one = estimate(&wd, Scope::OneCountry(1));
        assert!(one.hubs[0].is_some() && one.flow[9] > 2.0);
    }

    #[test]
    fn ports_need_a_connected_port() {
        let net = row(12);
        let rivers = RiverLinks::default();
        let ports: BTreeMap<Id, u8> = [(3, 1), (11, 2)].into_iter().collect();
        let none = Lines::default();
        let est = estimate(&world(&net, &rivers, &none, &[], &ports, vec![1; 13]), Scope::EachNation);
        assert!(est.ports.values().all(|a| !a) && est.flow[11] == 0.0);
        let lines = lay_provs(&net, &[(1, vec![1, 3])]);
        let est = estimate(&world(&net, &rivers, &lines, &[], &ports, vec![1; 13]), Scope::EachNation);
        assert!(est.ports.values().all(|a| *a));
        assert!((est.flow[11] - NAVAL.0).abs() < 1e-5, "{}", est.flow[11]);
    }
}
