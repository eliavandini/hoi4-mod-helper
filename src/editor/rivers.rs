//! Rivers: lines from source to mouth ([`River`]), drawn into HOI4's
//! rivers.bmp by [`rasterize`], checked by [`check`], and traced from the
//! height map by [`generate`].
//!
//! rivers.bmp (HOI4 wiki, "Map modding", and vanilla's file): one pixel wide,
//! pixels connect only side by side. Index 0 (green) is the source of a river
//! system, 1 (red) the last pixel of a tributary, next to the river it flows
//! into, 2 (yellow) the first pixel of a branch, next to the river it leaves;
//! 3..=11 are widths, narrowest first. A system (pixels connected side by
//! side) has exactly one source: tributaries and branches have none.
//! Indexes up to 6 count as small rivers. Other indexes are comments (vanilla:
//! 255 land, 254 sea).

use std::cmp::Reverse;
use std::collections::{BinaryHeap, VecDeque};

use rustc_hash::{FxHashMap, FxHashSet};

use super::model::{RIVER_WIDTHS, River, RiverPoint};

pub const SOURCE: u8 = 0;
pub const FLOW_IN: u8 = 1;
pub const FLOW_OUT: u8 = 2;
/// rivers.bmp comments for pixels without a river (as vanilla).
pub const LAND: u8 = 255;
pub const SEA: u8 = 254;

/// rivers.bmp index of a width (1 ..= RIVER_WIDTHS).
pub fn width_index(w: u8) -> u8 {
    2 + w.clamp(1, RIVER_WIDTHS)
}

pub fn is_river(v: u8) -> bool {
    v <= 2 + RIVER_WIDTHS
}

/// Something about a river the game would not take as drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct Problem {
    /// Index in the river list.
    pub river: usize,
    pub at: [u32; 2],
    pub msg: String,
    pub error: bool,
}

/// River pixels as rivers.bmp has them.
#[derive(Clone, Debug, Default)]
pub struct Raster {
    /// (pixel index, rivers.bmp index, river) sorted by pixel.
    pub px: Vec<(u32, u8, u32)>,
    pub problems: Vec<Problem>,
}

impl Raster {
    /// The whole rivers.bmp: rivers, else LAND / SEA.
    pub fn image(&self, w: usize, h: usize, is_sea: impl Fn(usize) -> bool) -> Vec<u8> {
        let mut out: Vec<u8> = (0..w * h).map(|i| if is_sea(i) { SEA } else { LAND }).collect();
        for &(i, v, _) in &self.px {
            out[i as usize] = v;
        }
        out
    }

    /// Pixels of one row from x0 to x1 (inclusive).
    pub fn row(&self, w: usize, y: usize, x0: usize, x1: usize) -> &[(u32, u8, u32)] {
        let (a, b) = ((y * w + x0) as u32, (y * w + x1) as u32);
        let s = self.px.partition_point(|p| p.0 < a);
        let e = self.px.partition_point(|p| p.0 <= b);
        &self.px[s..e]
    }
}

const N4: [(i64, i64); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];
const DIAG: [(i64, i64); 4] = [(1, 1), (-1, 1), (1, -1), (-1, -1)];

fn step(i: u32, w: usize, h: usize, d: (i64, i64)) -> Option<u32> {
    let (x, y) = ((i as usize % w) as i64 + d.0, (i as usize / w) as i64 + d.1);
    (x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h).then(|| (y as usize * w + x as usize) as u32)
}

fn xy(i: u32, w: usize) -> [u32; 2] {
    [(i as usize % w) as u32, (i as usize / w) as u32]
}

/// The pixels along a river (4-connected, source first) with the width at
/// each.
fn pixel_path(r: &River, w: usize, h: usize) -> Vec<(u32, u8)> {
    let cell = |p: &RiverPoint| ((p.0.floor() as i64).clamp(0, w as i64 - 1), (p.1.floor() as i64).clamp(0, h as i64 - 1));
    let mut out: Vec<(u32, u8)> = Vec::new();
    let Some(first) = r.points.first() else { return out };
    let (mut cx, mut cy) = cell(first);
    out.push(((cy as usize * w + cx as usize) as u32, first.2));
    for seg in r.points.windows(2) {
        let (a, b) = (&seg[0], &seg[1]);
        let (ex, ey) = cell(b);
        let (dx, dy) = ((b.0 - a.0) as f64, (b.1 - a.1) as f64);
        let len = dx.hypot(dy).max(1e-9);
        // distance of a cell centre from the line a-b
        let off = |x: i64, y: i64| ((x as f64 + 0.5 - a.0 as f64) * dy - (y as f64 + 0.5 - a.1 as f64) * dx).abs() / len;
        let total = ((ex - cx).abs() + (ey - cy).abs()).max(1) as f64;
        let mut k = 0.0;
        while (cx, cy) != (ex, ey) {
            let (sx, sy) = ((ex - cx).signum(), (ey - cy).signum());
            if sy == 0 || (sx != 0 && off(cx + sx, cy) <= off(cx, cy + sy)) {
                cx += sx;
            } else {
                cy += sy;
            }
            k += 1.0;
            let width = (a.2 as f64 + (b.2 as f64 - a.2 as f64) * k / total).round() as u8;
            out.push(((cy as usize * w + cx as usize) as u32, width));
        }
    }
    out
}

/// A path without loops and without touching itself except from one pixel
/// to the next: a pixel beside an earlier one (not the one before) or
/// diagonal to one (not the two before) cuts out the loop back to the
/// earliest it touches.
fn clean_path(raw: &[(u32, u8)], w: usize, h: usize) -> Vec<(u32, u8)> {
    let mut out: Vec<(u32, u8)> = Vec::with_capacity(raw.len());
    let mut pos: FxHashMap<u32, usize> = FxHashMap::default();
    let cut = |out: &mut Vec<(u32, u8)>, pos: &mut FxHashMap<u32, usize>, keep: usize| {
        for q in out.drain(keep + 1..) {
            pos.remove(&q.0);
        }
    };
    // earlier pixels p touches: (index, beside)
    let touching = |out: &[(u32, u8)], pos: &FxHashMap<u32, usize>, p: u32| -> Option<(usize, bool)> {
        let n = out.len();
        let side = N4.iter().filter_map(|&d| step(p, w, h, d)).filter_map(|q| pos.get(&q).copied()).filter(|&j| j + 1 < n).map(|j| (j, true));
        let diag = DIAG.iter().filter_map(|&d| step(p, w, h, d)).filter_map(|q| pos.get(&q).copied()).filter(|&j| j + 2 < n).map(|j| (j, false));
        side.chain(diag).min()
    };
    for &(p, wd) in raw {
        if out.last().is_some_and(|l| l.0 == p) {
            continue;
        }
        if let Some(&j) = pos.get(&p) {
            cut(&mut out, &mut pos, j);
            continue;
        }
        match touching(&out, &pos, p) {
            Some((j, true)) => cut(&mut out, &mut pos, j),
            Some((j, false)) => {
                cut(&mut out, &mut pos, j);
                // round the corner: a pixel beside both that touches nothing else
                let q = out[j].0;
                let ([px, py], [qx, qy]) = (xy(p, w), xy(q, w));
                let corners = [(qx, py), (px, qy)].map(|(x, y)| (y as usize * w + x as usize) as u32);
                let free = |c: u32| {
                    let mut o = out.clone();
                    o.push((c, wd));
                    let mut ps = pos.clone();
                    ps.insert(c, o.len() - 1);
                    touching(&out, &pos, c).is_none_or(|t| t.0 + 1 >= out.len()) && touching(&o, &ps, p).is_none()
                };
                let c = corners.into_iter().find(|&c| free(c)).unwrap_or(corners[0]);
                pos.insert(c, out.len());
                out.push((c, wd));
            }
            None => {}
        }
        pos.insert(p, out.len());
        out.push((p, wd));
    }
    out
}

/// What lies under a pixel, for rivers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ground {
    Land,
    /// A river stops at a lake and starts again after it.
    Lake,
    /// A river ends at the sea.
    Sea,
}

/// Draw the rivers as rivers.bmp has them. Wider rivers first (then in list
/// order); a river that reaches one drawn before ends there as its tributary
/// (red), one that starts at one branches off it (yellow); others start
/// with a source (green). A river ends at the sea; at a lake it stops and
/// starts again after it.
pub fn rasterize(rivers: &[River], w: usize, h: usize, ground: impl Fn(usize) -> Ground) -> Raster {
    let mut order: Vec<usize> = (0..rivers.len()).collect();
    order.sort_by_key(|&i| (Reverse(rivers[i].points.iter().map(|p| p.2).max().unwrap_or(0)), i));
    let mut occ: FxHashMap<u32, (u32, u8)> = FxHashMap::default();
    let mut problems = Vec::new();
    let name = |i: usize| if rivers[i].name.is_empty() { format!("river {}", i + 1) } else { rivers[i].name.clone() };
    let land = |i: u32| ground(i as usize) == Ground::Land;
    for &ri in &order {
        let r = &rivers[ri];
        if r.points.len() < 2 {
            continue;
        }
        let path = clean_path(&pixel_path(r, w, h), w, h);
        // from the first land pixel to the coast
        let Some(start) = path.iter().position(|&(i, _)| land(i)) else {
            problems.push(Problem { river: ri, at: xy(path[0].0, w), msg: format!("{} lies in water", name(ri)), error: false });
            continue;
        };
        let end = path[start..].iter().position(|&(i, _)| ground(i as usize) == Ground::Sea).map_or(path.len(), |e| start + e);
        if path[end..].iter().filter(|&&(i, _)| land(i)).count() > 3 {
            problems.push(Problem { river: ri, at: xy(path[end].0, w), msg: format!("{} reaches the sea and ends there (the rest is left out)", name(ri)), error: false });
        }
        // the stretches on land (split by lakes)
        let mut drawn = false;
        for piece in path[start..end].split(|&(i, _)| !land(i)).filter(|p| !p.is_empty()) {
            let res = draw_piece(piece, w, h, |q| occ.get(&q).copied(), land);
            let Some((out, joins, cut_at)) = res else { continue };
            if let (Some(k), Some(j)) = (cut_at, joins)
                && piece.len() - k > 8
            {
                problems.push(Problem {
                    river: ri,
                    at: xy(piece[k.min(piece.len() - 1)].0, w),
                    msg: format!("{} runs into {} and ends there (the rest is left out)", name(ri), name(j as usize)),
                    error: false,
                });
            }
            for (p, v) in out {
                occ.insert(p, (ri as u32, v));
            }
            drawn = true;
            if joins.is_some() {
                break;
            }
        }
        if !drawn {
            problems.push(Problem { river: ri, at: xy(path[start].0, w), msg: format!("{} is too short to draw (under 2 px on land)", name(ri)), error: false });
        }
    }
    let mut px: Vec<(u32, u8, u32)> = occ.into_iter().map(|(p, (r, v))| (p, v, r)).collect();
    px.sort_unstable();
    Raster { px, problems }
}

/// One stretch of a river on land: its pixels with their rivers.bmp index,
/// the river it flows into (if it does) and where it stops on the stretch.
/// `other`: the river already drawn at a pixel and its index there.
#[allow(clippy::type_complexity)]
fn draw_piece(path: &[(u32, u8)], w: usize, h: usize, other: impl Fn(u32) -> Option<(u32, u8)>, land: impl Fn(u32) -> bool) -> Option<(Vec<(u32, u8)>, Option<u32>, Option<usize>)> {
    let side_of = |p: u32| -> Vec<u32> { N4.iter().filter_map(|&d| step(p, w, h, d)).filter(|&q| other(q).is_some()).collect() };
    let diag_of = |p: u32| -> Vec<u32> { DIAG.iter().filter_map(|&d| step(p, w, h, d)).filter(|&q| other(q).is_some()).collect() };
    // every pixel diagonal to p on another river shares a side neighbour with
    // it that is a river pixel (or `own`): no touching at a corner only
    let corners_ok = |p: u32, own: &[u32]| {
        diag_of(p).into_iter().all(|d| {
            let ([px, py], [dx, dy]) = (xy(p, w), xy(d, w));
            [(dx, py), (px, dy)].into_iter().map(|(x, y)| (y as usize * w + x as usize) as u32).any(|c| other(c).is_some() || own.contains(&c))
        })
    };
    // a pixel can end a tributary (or start a branch) beside another river:
    // one river pixel beside it, a plain one with room for one more neighbour
    let joins_at = |e: u32, own: &[u32]| -> Option<u32> {
        if other(e).is_some() || !land(e) {
            return None;
        }
        let side = side_of(e);
        let [m] = side[..] else { return None };
        let (river, v) = other(m)?;
        let busy = side_of(m).len();
        (v > FLOW_OUT && busy <= 2 && corners_ok(e, own)).then_some(river)
    };
    let mut out: Vec<(u32, u8)> = Vec::new();
    let (mut branch, mut joins): (Option<u32>, Option<u32>) = (None, None);
    let mut cut_at = None;
    for (k, &(p, wd)) in path.iter().enumerate() {
        if other(p).is_some() {
            if out.is_empty() {
                continue; // starts on another river: begins beside it
            }
            cut_at = Some(k);
            break;
        }
        let v = width_index(wd);
        let contact = !side_of(p).is_empty() || !diag_of(p).is_empty();
        if !contact {
            out.push((p, v));
            continue;
        }
        let at_start = out.is_empty() || (out.len() == 1 && branch.is_some());
        if at_start {
            // a branch: its first pixel beside the river it leaves (sliding
            // along while the line still runs beside it)
            if let Some(r) = joins_at(p, &[]) {
                out.clear();
                out.push((p, FLOW_OUT));
                branch = Some(r);
            } else if !out.is_empty() && side_of(p).is_empty() && corners_ok(p, &[out[0].0]) {
                out.push((p, v)); // leaving the junction diagonally
            } else {
                // too close to start here: start further on
                out.clear();
                branch = None;
            }
            continue;
        }
        // a branch ends short of the next river (joining it would make one
        // river system with two sources)
        if branch.is_some() {
            cut_at = Some(k);
            break;
        }
        // reaches another river: flows into it here, or round the corner, or
        // beside it from the pixel before; else it ends short of it
        let q = out.last().unwrap().0;
        let mine: FxHashSet<u32> = out.iter().map(|o| o.0).collect();
        let before_q = out.len().checked_sub(2).map(|i| out[i].0);
        // a pixel added after `prev` (whose own predecessor is `pp`) touches
        // nothing else of this river
        let alone = |e: u32, prev: u32, pp: Option<u32>| {
            !mine.contains(&e)
                && N4.iter().filter_map(|&d| step(e, w, h, d)).all(|o| o == prev || !mine.contains(&o))
                && DIAG.iter().filter_map(|&d| step(e, w, h, d)).all(|o| Some(o) == pp || !mine.contains(&o))
        };
        if let Some(r) = joins_at(p, &[q]) {
            out.push((p, FLOW_IN));
            joins = Some(r);
        } else if let Some((c, r)) = side_of(p).is_empty().then(|| {
            let [px, py] = xy(p, w);
            diag_of(p).into_iter().find_map(|d| {
                let [dx, dy] = xy(d, w);
                [(dx, py), (px, dy)]
                    .into_iter()
                    .map(|(x, y)| (y as usize * w + x as usize) as u32)
                    .filter(|&c| alone(c, p, Some(q)))
                    .find_map(|c| if corners_ok(p, &[q, c]) { Some((c, joins_at(c, &[p])?)) } else { None })
            })
        }).flatten()
        {
            out.push((p, v));
            out.push((c, FLOW_IN));
            joins = Some(r);
        } else if let Some((e, r)) = N4.iter().filter_map(|&d| step(q, w, h, d)).filter(|&e| e != p && alone(e, q, before_q)).find_map(|e| Some((e, joins_at(e, &[q])?))) {
            out.push((e, FLOW_IN));
            joins = Some(r);
        }
        cut_at = Some(k + 1);
        break;
    }
    if out.len() < 2 && !(out.len() == 1 && joins.is_some()) {
        return None;
    }
    if branch.is_none() && joins.is_none() {
        out[0].1 = SOURCE;
    }
    Some((out, joins, cut_at))
}

/// Breaks of HOI4's rules in a rivers.bmp (pixel, message).
pub fn check(img: &[u8], w: usize, h: usize) -> Vec<([u32; 2], String)> {
    let mut out = Vec::new();
    let riv = |x: i64, y: i64| x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h && is_river(img[y as usize * w + x as usize]);
    let at = |x: i64, y: i64| img[y as usize * w + x as usize];
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            if !riv(x, y) {
                continue;
            }
            let v = at(x, y);
            let n: Vec<u8> = N4.iter().filter(|d| riv(x + d.0, y + d.1)).map(|d| at(x + d.0, y + d.1)).collect();
            let markers = n.iter().filter(|&&q| q <= FLOW_OUT).count();
            let ok = match v {
                SOURCE => n.len() == 1,
                FLOW_IN | FLOW_OUT => n.len() == 2,
                _ => (1..=2).contains(&n.len()) || (n.len() == 3 && markers >= 1),
            };
            if !ok {
                out.push(([x as u32, y as u32], format!("river pixel (index {v}) with {} river pixels beside it", n.len())));
            }
            if riv(x + 1, y) && riv(x, y + 1) && riv(x + 1, y + 1) {
                out.push(([x as u32, y as u32], "2×2 block of river pixels".into()));
            }
            for d in [(1i64, 1i64), (-1, 1)] {
                if riv(x + d.0, y + d.1) && !riv(x + d.0, y) && !riv(x, y + d.1) {
                    out.push(([x as u32, y as u32], "river pixels touch only at a corner".into()));
                }
            }
        }
    }
    // one source per system
    let mut seen = vec![false; w * h];
    for s in 0..w * h {
        if seen[s] || !is_river(img[s]) {
            continue;
        }
        seen[s] = true;
        let mut stack = vec![s];
        let mut sources = 0;
        while let Some(i) = stack.pop() {
            sources += (img[i] == SOURCE) as usize;
            for d in N4 {
                if let Some(j) = step(i as u32, w, h, d) {
                    let j = j as usize;
                    if !seen[j] && is_river(img[j]) {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
        }
        if sources != 1 {
            out.push((xy(s as u32, w), format!("river system with {sources} sources (needs exactly one)")));
        }
    }
    out
}

// ------------------------------------------------------------------ from heights

/// Sortable key of a height (NaN and no-data lowest).
fn key(f: f32) -> u32 {
    let f = if f.is_nan() || f < -1000.0 { -1000.0 } else { f };
    let b = f.to_bits();
    if b >> 31 == 1 { !b } else { b | 0x8000_0000 }
}

/// Opposite directions are pairs (2k, 2k + 1).
const D8: [(i64, i64); 8] = [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (-1, -1), (1, -1), (-1, 1)];
const OUTLET: u8 = 255;

/// Rivers where the water from at least `min_km2` gathers, following the
/// heights down to the sea or the map edge (depressions filled; through
/// lakes). Widths grow with the area drained: 1 at `min_km2`, one more for
/// each doubling. Main rivers come first, tributaries end on them; rivers
/// with under 10 px on land are left out.
pub fn generate(dem: &[f32], sea: &[bool], lake: &[bool], w: usize, h: usize, km2_per_px: f64, min_km2: f64) -> Vec<River> {
    let n = w * h;
    // flow directions by flooding from the outlets (Priority-Flood with a
    // queue for pits and flats): each pixel drains to the one it was reached from
    let mut dir = vec![OUTLET; n];
    let mut closed: Vec<bool> = sea.to_vec();
    let mut heap: BinaryHeap<Reverse<(u32, u64, u32)>> = BinaryHeap::new();
    let mut pit: VecDeque<(u32, u32)> = VecDeque::new();
    let mut counter = 0u64;
    for i in 0..n {
        if sea[i] {
            continue;
        }
        let (x, y) = ((i % w) as i64, (i / w) as i64);
        let edge = x == 0 || y == 0 || x as usize == w - 1 || y as usize == h - 1;
        let coast = D8.iter().any(|d| step(i as u32, w, h, *d).is_some_and(|j| sea[j as usize]));
        if edge || coast {
            closed[i] = true;
            heap.push(Reverse((key(dem[i]), counter, i as u32)));
            counter += 1;
        }
    }
    loop {
        let (c, k) = if let Some((c, k)) = pit.pop_front() {
            (c, k)
        } else if let Some(Reverse((k, _, c))) = heap.pop() {
            (c, k)
        } else {
            break;
        };
        for (di, d) in D8.iter().enumerate() {
            let Some(j) = step(c, w, h, *d) else { continue };
            let ju = j as usize;
            if closed[ju] {
                continue;
            }
            closed[ju] = true;
            // j drains back the way it was reached: the opposite direction
            dir[ju] = (di ^ 1) as u8;
            let kj = key(dem[ju]);
            if kj <= k {
                pit.push_back((j, k));
            } else {
                heap.push(Reverse((kj, counter, j)));
                counter += 1;
            }
        }
    }
    drop(closed);
    let down = |i: usize| -> Option<usize> { (dir[i] != OUTLET).then(|| step(i as u32, w, h, D8[dir[i] as usize]).unwrap() as usize) };
    // drained area (pixels), upstream first
    let mut indeg = vec![0u8; n];
    for (i, &s) in sea.iter().enumerate() {
        if !s && let Some(j) = down(i) {
            indeg[j] += 1;
        }
    }
    let mut acc = vec![1u32; n];
    let mut queue: Vec<usize> = (0..n).filter(|&i| !sea[i] && indeg[i] == 0).collect();
    while let Some(i) = queue.pop() {
        if let Some(j) = down(i) {
            acc[j] += acc[i];
            indeg[j] -= 1;
            if indeg[j] == 0 {
                queue.push(j);
            }
        }
    }
    drop(indeg);
    let min_px = (min_km2 / km2_per_px).ceil().max(2.0) as u32;
    let stream = |i: usize| !sea[i] && acc[i] >= min_px;
    // the main inflow of each stream pixel: the one draining the most
    let mut main_in: FxHashMap<u32, u32> = FxHashMap::default();
    let mut has_in: FxHashSet<u32> = FxHashSet::default();
    for i in 0..n {
        if !stream(i) {
            continue;
        }
        if let Some(j) = down(i)
            && stream(j)
        {
            has_in.insert(j as u32);
            let e = main_in.entry(j as u32).or_insert(i as u32);
            if (acc[i], Reverse(i)) > (acc[*e as usize], Reverse(*e as usize)) {
                *e = i as u32;
            }
        }
    }
    let width = |a: u32| -> u8 { (1.0 + (a as f64 / min_px as f64).log2().floor()).clamp(1.0, RIVER_WIDTHS as f64) as u8 };
    let mut found: Vec<(u32, Vec<u32>)> = Vec::new(); // (drained area at the end, pixels)
    for head in 0..n {
        if !stream(head) || has_in.contains(&(head as u32)) {
            continue;
        }
        let mut path = vec![head as u32];
        let mut cur = head;
        while let Some(j) = down(cur) {
            if sea[j] {
                break;
            }
            path.push(j as u32);
            if main_in.get(&(j as u32)) != Some(&(cur as u32)) {
                break; // a tributary: ends on the main river
            }
            cur = j;
        }
        let end_acc = acc[cur];
        if path.iter().filter(|&&i| !lake[i as usize]).count() >= 10 {
            found.push((end_acc, path));
        }
    }
    found.sort_by_key(|f| (Reverse(f.0), f.1[0]));
    found
        .into_iter()
        .map(|(_, path)| {
            let pts: Vec<RiverPoint> = path.iter().map(|&i| RiverPoint((i as usize % w) as f32 + 0.5, (i as usize / w) as f32 + 0.5, width(acc[i as usize]))).collect();
            River { name: String::new(), points: simplify(&pts, 1.0) }
        })
        .collect()
}

/// The rivers of a rivers.bmp (a mod's or the game's) as lines again: each
/// system from its source (green) along the river, the side rivers at the
/// pixels beside it: a tributary (its red pixel) from its far end into the
/// river, a branch (its yellow pixel) from the river to its far end.
/// Without a source a system starts at an end. Lines are simplified within
/// `tol` px.
pub fn trace(img: &[u8], w: usize, h: usize, tol: f32) -> Vec<River> {
    let river = |i: u32| is_river(img[i as usize]);
    let marker = |i: u32| matches!(img[i as usize], FLOW_IN | FLOW_OUT);
    let mut seen = vec![false; w * h];
    let nbs = |i: u32| N4.iter().filter_map(move |&d| step(i, w, h, d)).filter(move |&q| river(q));
    // walk from `start` (seen) over pixels not seen yet; at a fork the
    // unmarked way goes on and the others are side rivers (pixel, the river
    // pixel beside it)
    let walk = |start: u32, seen: &mut Vec<bool>, sides: &mut Vec<(u32, u32)>| -> Vec<u32> {
        let mut path = vec![start];
        let mut at = start;
        loop {
            let next: Vec<u32> = nbs(at).filter(|&q| !seen[q as usize]).collect();
            for &q in &next {
                seen[q as usize] = true;
            }
            let (marked, plain): (Vec<u32>, Vec<u32>) = next.into_iter().partition(|&q| marker(q));
            sides.extend(marked.into_iter().map(|q| (q, at)));
            let mut plain = plain.into_iter();
            let Some(go) = plain.next() else { break };
            // no marker says which way the river goes: the widest goes on
            let mut rest: Vec<u32> = plain.collect();
            let go = if rest.is_empty() {
                go
            } else {
                rest.push(go);
                rest.sort_by_key(|&q| (Reverse(img[q as usize]), q));
                rest.remove(0)
            };
            sides.extend(rest.into_iter().map(|q| (q, at)));
            path.push(go);
            at = go;
        }
        path
    };
    let width_of = |path: &[u32]| -> Vec<u8> {
        // markers take the width of the pixel next to them on the path
        let raw: Vec<Option<u8>> = path.iter().map(|&p| img[p as usize]).map(|v| (v >= 3).then(|| v - 2)).collect();
        (0..raw.len())
            .map(|k| raw[k].or_else(|| (1..raw.len()).find_map(|d| raw.get(k + d).copied().flatten().or_else(|| k.checked_sub(d).and_then(|j| raw[j])))).unwrap_or(1))
            .collect()
    };
    let line = |path: &[u32]| -> River {
        let wd = width_of(path);
        let pts: Vec<RiverPoint> = path.iter().zip(&wd).map(|(&i, &d)| RiverPoint((i as usize % w) as f32 + 0.5, (i as usize / w) as f32 + 0.5, d)).collect();
        River { name: String::new(), points: simplify(&pts, tol) }
    };
    // sources first, then ends, then anything left (loops)
    let mut starts: Vec<u32> = (0..(w * h) as u32).filter(|&i| img[i as usize] == SOURCE).collect();
    starts.extend((0..(w * h) as u32).filter(|&i| river(i) && img[i as usize] != SOURCE && nbs(i).count() <= 1));
    starts.extend((0..(w * h) as u32).filter(|&i| river(i)));
    let mut out = Vec::new();
    for s in starts {
        if seen[s as usize] {
            continue;
        }
        seen[s as usize] = true;
        let mut sides = Vec::new();
        let main = walk(s, &mut seen, &mut sides);
        if main.len() >= 2 {
            out.push(line(&main));
        }
        while let Some((q, at)) = sides.pop() {
            let mut path = walk(q, &mut seen, &mut sides);
            if img[q as usize] == FLOW_OUT {
                // a branch leaves the river at `at`
                path.insert(0, at);
            } else {
                // a tributary (or a side without a marker) flows into it
                path.reverse();
                path.push(at);
            }
            if path.len() >= 2 {
                out.push(line(&path));
            }
        }
    }
    out
}

/// Fewer points (Douglas-Peucker within `tol` px); where the width changes a
/// point stays.
pub fn simplify(pts: &[RiverPoint], tol: f32) -> Vec<RiverPoint> {
    if pts.len() <= 2 {
        return pts.to_vec();
    }
    let mut keep = vec![false; pts.len()];
    keep[0] = true;
    keep[pts.len() - 1] = true;
    for i in 1..pts.len() {
        if pts[i].2 != pts[i - 1].2 {
            keep[i - 1] = true;
            keep[i] = true;
        }
    }
    let marks: Vec<usize> = (0..pts.len()).filter(|&i| keep[i]).collect();
    for run in marks.windows(2) {
        dp(pts, run[0], run[1], tol, &mut keep);
    }
    pts.iter().zip(&keep).filter(|(_, k)| **k).map(|(p, _)| *p).collect()
}

fn dp(pts: &[RiverPoint], a: usize, b: usize, tol: f32, keep: &mut [bool]) {
    if b <= a + 1 {
        return;
    }
    let (ax, ay, bx, by) = (pts[a].0, pts[a].1, pts[b].0, pts[b].1);
    let (dx, dy) = (bx - ax, by - ay);
    let len = dx.hypot(dy).max(1e-6);
    let (mut best, mut at) = (0f32, a);
    for (i, p) in pts.iter().enumerate().take(b).skip(a + 1) {
        let d = ((p.0 - ax) * dy - (p.1 - ay) * dx).abs() / len;
        if d > best {
            best = d;
            at = i;
        }
    }
    if best > tol {
        keep[at] = true;
        dp(pts, a, at, tol, keep);
        dp(pts, at, b, tol, keep);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn river(pts: &[(f32, f32)], wd: u8) -> River {
        River { name: String::new(), points: pts.iter().map(|&(x, y)| RiverPoint(x, y, wd)).collect() }
    }

    fn image(r: &Raster, w: usize, h: usize) -> Vec<u8> {
        r.image(w, h, |_| false)
    }

    #[test]
    fn one_river_has_one_source_and_is_one_pixel_wide() {
        let (w, h) = (60, 40);
        let r = rasterize(&[river(&[(2.5, 3.5), (30.2, 20.7), (55.5, 35.5)], 3)], w, h, |_| Ground::Land);
        let img = image(&r, w, h);
        assert!(check(&img, w, h).is_empty(), "{:?}", check(&img, w, h));
        assert_eq!(img.iter().filter(|&&v| v == SOURCE).count(), 1);
        assert_eq!(img[3 * w + 2], SOURCE, "the source is the first point");
        assert!(r.problems.is_empty());
    }

    #[test]
    fn tributary_ends_red_beside_the_main_river() {
        let (w, h) = (60, 40);
        let main = river(&[(1.5, 20.5), (58.5, 20.5)], 6);
        // drawn across the main river: it ends where it meets it
        let trib = river(&[(30.5, 2.5), (31.5, 35.5)], 2);
        let r = rasterize(&[trib, main], w, h, |_| Ground::Land);
        let img = image(&r, w, h);
        assert!(check(&img, w, h).is_empty(), "{:?}", check(&img, w, h));
        assert_eq!(img.iter().filter(|&&v| v == SOURCE).count(), 1, "the tributary has no source of its own");
        let red: Vec<usize> = (0..w * h).filter(|&i| img[i] == FLOW_IN).collect();
        assert_eq!(red.len(), 1);
        assert_eq!(red[0] / w, 19, "the red pixel is just above the main river");
        assert!(r.problems.iter().any(|p| p.msg.contains("runs into")), "{:?}", r.problems);
    }

    #[test]
    fn diagonal_approach_gets_a_corner_pixel() {
        let (w, h) = (50, 50);
        let main = river(&[(1.5, 25.5), (48.5, 25.5)], 5);
        let trib = river(&[(5.5, 2.5), (27.5, 24.5)], 2);
        let r = rasterize(&[main, trib], w, h, |_| Ground::Land);
        let img = image(&r, w, h);
        assert!(check(&img, w, h).is_empty(), "{:?}", check(&img, w, h));
        assert_eq!(img.iter().filter(|&&v| v == FLOW_IN).count(), 1);
    }

    #[test]
    fn branch_starts_yellow() {
        let (w, h) = (60, 40);
        let main = river(&[(1.5, 20.5), (58.5, 20.5)], 6);
        let branch = river(&[(20.5, 20.5), (40.5, 37.5)], 2);
        let img = image(&rasterize(&[main, branch], w, h, |_| Ground::Land), w, h);
        assert!(check(&img, w, h).is_empty(), "{:?}", check(&img, w, h));
        assert_eq!(img.iter().filter(|&&v| v == FLOW_OUT).count(), 1);
        assert_eq!(img.iter().filter(|&&v| v == SOURCE).count(), 1);
    }

    #[test]
    fn rivers_stop_at_the_sea_and_tight_bends_are_cut() {
        let (w, h) = (40, 30);
        let sea = |i: usize| i % w >= 30;
        // a hairpin: down, one step right, back up, then to the sea
        let r = river(&[(2.5, 2.5), (2.5, 20.5), (3.5, 20.5), (3.5, 5.5), (20.5, 5.5), (38.5, 5.5)], 4);
        let ras = rasterize(&[r], w, h, |i| if sea(i) { Ground::Sea } else { Ground::Land });
        let img = ras.image(w, h, sea);
        assert!(check(&img, w, h).is_empty(), "{:?}", check(&img, w, h));
        assert!((0..h).all(|y| (30..w).all(|x| !is_river(img[y * w + x]))), "nothing in the sea");
        assert!(is_river(img[5 * w + 29]), "reaches the coast");
    }

    #[test]
    fn a_lake_splits_a_river_into_two_systems() {
        let (w, h) = (60, 20);
        let lake = |i: usize| (20..30).contains(&(i % w)) && (5..15).contains(&(i / w));
        let ground = |i: usize| if lake(i) { Ground::Lake } else { Ground::Land };
        let ras = rasterize(&[river(&[(2.5, 10.5), (57.5, 10.5)], 3)], w, h, ground);
        let img = image(&ras, w, h);
        assert!(check(&img, w, h).is_empty(), "{:?}", check(&img, w, h));
        assert_eq!(img.iter().filter(|&&v| v == SOURCE).count(), 2, "a new source after the lake");
        assert_eq!(img[10 * w + 30], SOURCE);
        assert!((20..30).all(|x| !is_river(img[10 * w + x])), "nothing on the lake");
        assert!(ras.problems.is_empty(), "{:?}", ras.problems);
    }

    #[test]
    fn a_tributary_into_an_inner_bend_stays_valid() {
        let (w, h) = (50, 45);
        let main = river(&[(1.5, 20.5), (30.5, 20.5), (30.5, 43.5)], 6);
        let trib = river(&[(5.5, 40.5), (29.5, 21.5)], 2);
        let img = image(&rasterize(&[main, trib], w, h, |_| Ground::Land), w, h);
        assert!(check(&img, w, h).is_empty(), "{:?}", check(&img, w, h));
        assert_eq!(img.iter().filter(|&&v| v == FLOW_IN).count(), 1);
    }

    /// Drawn rivers traced back from the bitmap draw the same bitmap.
    #[test]
    fn traced_rivers_draw_the_same_bitmap() {
        let (w, h) = (80, 60);
        let main = river(&[(1.5, 30.5), (40.5, 32.5), (78.5, 20.5)], 6);
        let trib = river(&[(20.5, 2.5), (25.5, 20.5), (30.5, 31.5)], 2);
        let trib2 = river(&[(10.5, 55.5), (35.5, 45.5), (45.5, 33.5)], 3);
        let branch = river(&[(60.5, 26.5), (65.5, 45.5), (70.5, 57.5)], 4);
        let other = river(&[(5.5, 5.5), (12.5, 18.5)], 1);
        let rs = [main, trib, trib2, branch, other];
        let img = rasterize(&rs, w, h, |_| Ground::Land).image(w, h, |_| false);
        assert!(check(&img, w, h).is_empty(), "{:?}", check(&img, w, h));
        let traced = trace(&img, w, h, 0.5);
        assert_eq!(traced.len(), 5, "{traced:?}");
        let again = rasterize(&traced, w, h, |_| Ground::Land);
        assert!(again.problems.is_empty(), "{:?}", again.problems);
        let img2 = again.image(w, h, |_| false);
        let differ = img.iter().zip(&img2).filter(|(a, b)| a != b).count();
        assert_eq!(differ, 0, "{differ} px differ");
        // the branch starts at the main river, the tributaries end at it
        let ends = |r: &River| (r.points[0], *r.points.last().unwrap());
        assert!(traced.iter().any(|r| (ends(r).1.1 - 31.0).abs() < 3.0 && ends(r).0.1 < 5.0), "{traced:?}");
    }

    #[test]
    fn random_rivers_always_make_a_valid_bitmap() {
        let (w, h) = (140, 100);
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        let mut rnd = move |n: f32| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 40) as f32 / (1u64 << 24) as f32 * n
        };
        let ground = |i: usize| {
            let (x, y) = ((i % w) as f32, (i / w) as f32);
            if x + y > 200.0 {
                Ground::Sea
            } else if (x - 50.0).hypot(y - 40.0) < 7.0 {
                Ground::Lake
            } else {
                Ground::Land
            }
        };
        for round in 0..40 {
            let rivers: Vec<River> = (0..12)
                .map(|_| {
                    let k = 2 + rnd(6.0) as usize;
                    River { name: String::new(), points: (0..k).map(|_| RiverPoint(rnd(w as f32), rnd(h as f32), 1 + rnd(9.0) as u8)).collect() }
                })
                .collect();
            let ras = rasterize(&rivers, w, h, ground);
            let img = ras.image(w, h, |i| ground(i) == Ground::Sea);
            let breaks = check(&img, w, h);
            assert!(breaks.is_empty(), "round {round}: {:?}", &breaks[..breaks.len().min(6)]);
            assert!((0..w * h).all(|i| ground(i) == Ground::Land || !is_river(img[i])), "round {round}: river on water");
        }
    }

    #[test]
    fn widths_follow_the_points() {
        let (w, h) = (40, 10);
        let r = River { name: String::new(), points: vec![RiverPoint(1.5, 5.5, 1), RiverPoint(38.5, 5.5, 9)] };
        let img = image(&rasterize(&[r], w, h, |_| Ground::Land), w, h);
        assert_eq!(img[5 * w + 2], width_index(1));
        assert_eq!(img[5 * w + 38], width_index(9));
    }

    #[test]
    fn generated_rivers_drain_a_valley_and_pass_the_check() {
        // a valley sloping to the sea on the right, side valleys from top and bottom
        let (w, h) = (120, 80);
        let sea: Vec<bool> = (0..w * h).map(|i| i % w >= 115).collect();
        let dem: Vec<f32> = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32, (i / w) as f32);
                let across = (y - 40.0).abs() * 8.0 + ((x * 0.37).sin() * 3.0);
                let along = (120.0 - x) * 2.0;
                across + along + ((x * 0.11).cos() * (y * 0.07).sin()) * 5.0
            })
            .collect();
        let rivers = generate(&dem, &sea, &vec![false; w * h], w, h, 1.0, 60.0);
        assert!(!rivers.is_empty());
        let main = &rivers[0];
        assert!(main.points.last().unwrap().0 > 110.0, "the main river reaches the sea");
        assert!((main.points.last().unwrap().1 - 40.5).abs() < 6.0, "along the valley floor");
        assert!(main.points.first().unwrap().2 < main.points.last().unwrap().2, "it widens downstream");
        let ras = rasterize(&rivers, w, h, |i| if sea[i] { Ground::Sea } else { Ground::Land });
        let img = ras.image(w, h, |i| sea[i]);
        assert!(check(&img, w, h).is_empty(), "{:?}", check(&img, w, h));
    }

    #[test]
    fn check_finds_breaks() {
        let (w, h) = (6, 6);
        let mut img = vec![LAND; w * h];
        // two sources in one system, and a 2x2 block
        for (x, y, v) in [(1, 1, SOURCE), (2, 1, 4), (3, 1, SOURCE), (1, 3, 4), (2, 3, 4), (1, 4, 4), (2, 4, 4)] {
            img[y * w + x] = v;
        }
        let msgs: Vec<String> = check(&img, w, h).into_iter().map(|m| m.1).collect();
        assert!(msgs.iter().any(|m| m.contains("2 sources")), "{msgs:?}");
        assert!(msgs.iter().any(|m| m.contains("2×2")), "{msgs:?}");
        assert!(msgs.iter().any(|m| m.contains("0 sources")), "{msgs:?}");
    }
}
