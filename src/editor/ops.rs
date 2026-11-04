//! Edit operations on provinces, states, nations, strategic regions (air and
//! sea zones), adjacencies and inaccessible zones. Callers wrap each in a
//! transaction (`Doc::begin` / `commit`, or `rollback` on error).

use std::collections::{BTreeMap, BinaryHeap, VecDeque};

use anyhow::{Result, anyhow, bail};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};

use super::density::DensityParams;
use super::doc::Doc;
use super::kmeans::{self, Limits};
use super::model::{Adjacency, Id, Index, Kind, Labels, Nation, Province, Region, State};

/// Parameters of the generate tools.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GenParams {
    pub density: DensityParams,
    /// Largest land province.
    pub max_km2: f32,
    /// Smaller pieces join a neighbour.
    pub min_km2: f32,
    /// Province bounding box limit as a fraction of 1/8 of the map.
    pub box_frac: f32,
    pub state_provinces: f32,
    pub state_max_km2: f32,
    pub region_km2: f32,
}

impl Default for GenParams {
    fn default() -> Self {
        GenParams {
            density: DensityParams::default(),
            max_km2: 80.0,
            min_km2: 0.15,
            box_frac: 0.9,
            state_provinces: 10.0,
            state_max_km2: 300.0,
            region_km2: 1500.0,
        }
    }
}

/// Which pixels a paint stroke may take over.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Constraint {
    pub same_kind: bool,
    pub same_state: bool,
    pub same_nation: bool,
}

impl Default for Constraint {
    fn default() -> Self {
        Constraint { same_kind: true, same_state: false, same_nation: false }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Retile {
    /// Cut every province into this many pieces.
    Count(usize),
    /// Re-cut by the terrain density (mountains coarse, valleys fine).
    Density,
    /// As few provinces as the box limit allows (impassable / removed areas).
    Coarse,
}

const NONE: u32 = u32::MAX;

fn seg_dist2(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 0.0 { (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
    let (qx, qy) = (a[0] + t * dx - p[0], a[1] + t * dy - p[1]);
    qx * qx + qy * qy
}

pub fn roman(n: usize) -> String {
    const R: [(usize, &str); 9] = [(100, "C"), (90, "XC"), (50, "L"), (40, "XL"), (10, "X"), (9, "IX"), (5, "V"), (4, "IV"), (1, "I")];
    let mut n = n;
    let mut s = String::new();
    for (v, t) in R {
        while n >= v {
            s.push_str(t);
            n -= v;
        }
    }
    s
}

/// Compass direction from `a` to `b` (pixel coordinates, y down).
fn direction(a: [f64; 2], b: [f64; 2]) -> &'static str {
    let ang = (-(b[1] - a[1])).atan2(b[0] - a[0]).to_degrees();
    const D: [&str; 8] = ["East", "North-East", "North", "North-West", "West", "South-West", "South", "South-East"];
    D[(((ang + 360.0 + 22.5) % 360.0) / 45.0) as usize % 8]
}

/// Connected parts (4-neighbourhood) of a pixel set; returns part index per pixel.
fn pixel_parts(px: &[usize], w: usize) -> (Vec<u32>, usize) {
    let pos: FxHashMap<usize, usize> = px.iter().enumerate().map(|(k, &i)| (i, k)).collect();
    let mut part = vec![NONE; px.len()];
    let mut n = 0;
    let mut stack = Vec::new();
    for s in 0..px.len() {
        if part[s] != NONE {
            continue;
        }
        part[s] = n as u32;
        stack.push(s);
        while let Some(k) = stack.pop() {
            let i = px[k];
            let (x, _y) = (i % w, i / w);
            let mut nbs = [usize::MAX; 4];
            if x > 0 {
                nbs[0] = i - 1;
            }
            if x + 1 < w {
                nbs[1] = i + 1;
            }
            if i >= w {
                nbs[2] = i - w;
            }
            nbs[3] = i + w;
            for j in nbs {
                if let Some(&kk) = pos.get(&j) {
                    if part[kk] == NONE {
                        part[kk] = n as u32;
                        stack.push(kk);
                    }
                }
            }
        }
        n += 1;
    }
    (part, n)
}

/// Make each cluster of a local raster one connected piece of at least
/// `min_px` pixels: stray parts and small clusters join the neighbouring
/// cluster they share most border with. `lbl` is `lw × lh`, NONE outside.
fn clean_clusters(lbl: &mut [u32], lw: usize, lh: usize, min_px: usize) {
    let n = lw * lh;
    for _ in 0..12 {
        let mut comp = vec![NONE; n];
        let mut comps: Vec<(u32, usize)> = Vec::new(); // (cluster, size)
        let mut stack = Vec::new();
        for s in 0..n {
            if lbl[s] == NONE || comp[s] != NONE {
                continue;
            }
            let id = comps.len() as u32;
            let c = lbl[s];
            comp[s] = id;
            stack.push(s);
            let mut size = 0;
            while let Some(i) = stack.pop() {
                size += 1;
                let (x, y) = (i % lw, i / lw);
                for (ok, j) in [(x > 0, i.wrapping_sub(1)), (x + 1 < lw, i + 1), (y > 0, i.wrapping_sub(lw)), (y + 1 < lh, i + lw)] {
                    if ok && comp[j] == NONE && lbl[j] == c {
                        comp[j] = id;
                        stack.push(j);
                    }
                }
            }
            comps.push((c, size));
        }
        let mut biggest: FxHashMap<u32, (usize, u32)> = FxHashMap::default();
        let mut total: FxHashMap<u32, usize> = FxHashMap::default();
        for (i, &(c, size)) in comps.iter().enumerate() {
            *total.entry(c).or_insert(0) += size;
            let e = biggest.entry(c).or_insert((size, i as u32));
            if size > e.0 {
                *e = (size, i as u32);
            }
        }
        let moving: Vec<bool> = comps
            .iter()
            .enumerate()
            .map(|(i, &(c, _))| biggest[&c].1 != i as u32 || total[&c] < min_px)
            .collect();
        if !moving.iter().any(|&m| m) {
            break;
        }
        // border counts of moving components with other clusters
        let mut links: FxHashMap<(u32, u32), u32> = FxHashMap::default();
        for i in 0..n {
            let ci = comp[i];
            if ci == NONE || !moving[ci as usize] {
                continue;
            }
            let (x, y) = (i % lw, i / lw);
            for (ok, j) in [(x > 0, i.wrapping_sub(1)), (x + 1 < lw, i + 1), (y > 0, i.wrapping_sub(lw)), (y + 1 < lh, i + lw)] {
                if ok && lbl[j] != NONE && lbl[j] != lbl[i] {
                    *links.entry((ci, lbl[j])).or_insert(0) += 1;
                }
            }
        }
        let mut target: FxHashMap<u32, (u32, u32)> = FxHashMap::default();
        for (&(ci, to), &cnt) in &links {
            let e = target.entry(ci).or_insert((to, cnt));
            if cnt > e.1 || (cnt == e.1 && to < e.0) {
                *e = (to, cnt);
            }
        }
        if target.is_empty() {
            break;
        }
        // move only components whose target cluster is not itself being dissolved
        // into them this round (avoids swaps)
        let mut changed = false;
        for i in 0..n {
            let ci = comp[i];
            if ci != NONE {
                if let Some(&(to, _)) = target.get(&ci) {
                    lbl[i] = to;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
}

impl Doc {
    pub fn limits(&self, g: &GenParams) -> Limits {
        let k = self.data.km2_per_px();
        Limits {
            max_w: ((self.w as f32 / 8.0) * g.box_frac).max(8.0) as u32,
            max_h: ((self.h as f32 / 8.0) * g.box_frac).max(8.0) as u32,
            max_px: ((g.max_km2 as f64 / k) as u32).max(16),
            min_px: ((g.min_km2 as f64 / k) as u32).max(9),
        }
    }

    fn reindex(&mut self) {
        self.index = Index::build(&self.data);
    }

    fn ensure_stat_cap(&mut self) {
        let cap = self.data.provinces.cap();
        if self.stats.prov.len() < cap {
            self.stats.prov.resize(cap, Default::default());
            self.stats.nb.resize(cap, Vec::new());
        }
    }

    // ================================================================ provinces

    /// A new province without pixels (paint it, or fill it in an operation).
    pub fn new_province(&mut self, kind: Kind, state: Id, region: Id, name: String, like: Option<Id>) -> Id {
        let rgb = self.data.fresh_colour(self.data.provinces.cap() as u64 * 31 + self.pixel_version);
        let (edge, terrain) = like.and_then(|l| self.data.provinces.get(l)).map(|p| (p.edge, p.terrain.clone())).unwrap_or((false, None));
        let id = self.data.provinces.push(|id| Province {
            id,
            rgb,
            kind,
            edge,
            coastal: false,
            name,
            terrain,
            km2: 0.0,
            nation: None,
            extra: Default::default(),
            state: if kind == Kind::Land { state } else { 0 },
            region,
        });
        self.ensure_stat_cap();
        id
    }

    /// Paint `target` along the segment a–b (pixel coordinates) with radius `r`.
    pub fn paint(&mut self, a: [f64; 2], b: [f64; 2], r: f64, target: Id, c: Constraint) -> usize {
        let Some(tp) = self.data.provinces.get(target) else { return 0 };
        let (tkind, tstate) = (tp.kind, tp.state);
        let tnation = self.data.nation_of(target);
        let (w, h) = (self.w as i64, self.h as i64);
        let r = r.max(0.5);
        let x0 = ((a[0].min(b[0]) - r).floor() as i64).max(0);
        let x1 = ((a[0].max(b[0]) + r).ceil() as i64).min(w - 1);
        let y0 = ((a[1].min(b[1]) - r).floor() as i64).max(0);
        let y1 = ((a[1].max(b[1]) + r).ceil() as i64).min(h - 1);
        if x0 > x1 || y0 > y1 {
            return 0;
        }
        self.with_pixels(|wr, data| {
            let mut n = 0;
            for y in y0..=y1 {
                for x in x0..=x1 {
                    if seg_dist2([x as f64 + 0.5, y as f64 + 0.5], a, b) > r * r {
                        continue;
                    }
                    let i = (y * w + x) as usize;
                    let cur = wr.get(i);
                    if cur == target
                        || (c.same_kind && data.kind(cur) != tkind)
                        || (c.same_state && data.state_of(cur) != tstate)
                        || (c.same_nation && data.nation_of(cur) != tnation)
                    {
                        continue;
                    }
                    wr.set(i, target);
                    n += 1;
                }
            }
            n
        })
    }

    /// Provinces touched by a brush stroke a–b with radius r.
    pub fn provinces_in_stroke(&self, a: [f64; 2], b: [f64; 2], r: f64) -> Vec<Id> {
        let lab = self.labels.read();
        let (w, h) = (self.w as i64, self.h as i64);
        let r = r.max(0.5);
        let x0 = ((a[0].min(b[0]) - r).floor() as i64).max(0);
        let x1 = ((a[0].max(b[0]) + r).ceil() as i64).min(w - 1);
        let y0 = ((a[1].min(b[1]) - r).floor() as i64).max(0);
        let y1 = ((a[1].max(b[1]) + r).ceil() as i64).min(h - 1);
        let mut out = FxHashSet::default();
        let step = ((r / 4.0).floor() as i64).max(1);
        let mut y = y0;
        while y <= y1 {
            let mut x = x0;
            while x <= x1 {
                if seg_dist2([x as f64 + 0.5, y as f64 + 0.5], a, b) <= r * r {
                    out.insert(lab.px[(y * w + x) as usize]);
                }
                x += step;
            }
            y += step;
        }
        let mut v: Vec<Id> = out.into_iter().collect();
        v.sort_unstable();
        v
    }

    fn remap_province(&mut self, from: Id, to: Id) {
        for a in &mut self.data.adjacencies {
            if a.from == from {
                a.from = to;
            }
            if a.to == from {
                a.to = to;
            }
            if a.through == Some(from) {
                a.through = Some(to);
            }
        }
        for (_, s) in self.data.states.iter_mut() {
            for v in &mut s.victory_points {
                if v[0] == from {
                    v[0] = to;
                }
            }
        }
    }

    pub fn merge_provinces(&mut self, ids: &[Id]) -> Result<Id> {
        let Some(&target) = ids.first() else { bail!("nothing selected") };
        let tk = self.data.kind(target);
        let others: Vec<Id> = ids[1..].iter().copied().filter(|&p| p != target && self.data.provinces.contains(p)).collect();
        if others.is_empty() {
            bail!("select at least two provinces");
        }
        if let Some(&p) = others.iter().find(|&&p| self.data.kind(p) != tk) {
            bail!("{} is {} but {} is {}: change the type first", self.data.prov_name(p), self.data.kind(p).name(), self.data.prov_name(target), tk.name());
        }
        let lists: Vec<Vec<usize>> = {
            let lab = self.labels.read();
            others.iter().map(|&p| self.pixels_of(&lab, p)).collect()
        };
        self.with_pixels(|wr, _| {
            for list in &lists {
                for &i in list {
                    wr.set(i, target);
                }
            }
        });
        for &p in &others {
            self.remap_province(p, target);
        }
        Ok(target)
    }

    /// Re-cut provinces (see [`Retile`]). Returns (provinces before, after).
    pub fn retile(&mut self, ids: &[Id], mode: Retile, g: &GenParams) -> Result<(usize, usize)> {
        let km2 = self.data.km2_per_px() as f32;
        let mut lim = self.limits(g);
        if matches!(mode, Retile::Coarse) {
            lim.max_px = u32::MAX;
        }
        let dens = match mode {
            Retile::Density => Some(self.density(&g.density).ok_or_else(|| {
                anyhow!("the terrain (dem_section.tif, landcover_smooth.bmp) is not loaded yet; it is needed to place provinces by terrain")
            })?),
            _ => None,
        };
        // groups of provinces cut together: same type, state and region
        let mut groups: BTreeMap<(u8, Id, Id, Id), Vec<Id>> = BTreeMap::new();
        for &p in ids {
            let Some(pr) = self.data.provinces.get(p) else { continue };
            let key = match mode {
                Retile::Count(_) => (0, 0, 0, p),
                _ => (pr.kind as u8, pr.state, pr.region, 0),
            };
            groups.entry(key).or_default().push(p);
        }
        if groups.is_empty() {
            bail!("nothing selected");
        }
        let w = self.w as usize;
        let before = ids.len();
        let mut after = 0;
        let mut touched = [usize::MAX, usize::MAX, 0, 0];
        for (gi, (_, provs)) in groups.into_iter().enumerate() {
            // pixels and their original province
            let (px, orig): (Vec<usize>, Vec<Id>) = {
                let lab = self.labels.read();
                let mut px = Vec::new();
                let mut orig = Vec::new();
                for &p in &provs {
                    let v = self.pixels_of(&lab, p);
                    orig.extend(std::iter::repeat_n(p, v.len()));
                    px.extend(v);
                }
                (px, orig)
            };
            if px.is_empty() {
                continue;
            }
            let p0 = provs[0];
            let kind = self.data.kind(p0);
            let coarse_unit = matches!(mode, Retile::Coarse) || kind == Kind::Lake || self.data.impassable(p0) || self.data.provinces.get(p0).is_some_and(|p| p.edge);
            let (parts, nparts) = pixel_parts(&px, w);
            let mut part_px: Vec<Vec<usize>> = vec![Vec::new(); nparts];
            for (k, &pt) in parts.iter().enumerate() {
                part_px[pt as usize].push(k);
            }
            let total = px.len() as f32;
            let mut cluster = vec![NONE; px.len()];
            let mut next_cluster = 0u32;
            for (pi, members) in part_px.iter().enumerate() {
                let pts: Vec<[u32; 2]> = members.iter().map(|&k| [(px[k] % w) as u32, (px[k] / w) as u32]).collect();
                let wts: Vec<f32> = match mode {
                    Retile::Count(n) => vec![n as f32 / total; members.len()],
                    Retile::Coarse => vec![0.0; members.len()],
                    Retile::Density if coarse_unit => vec![0.0; members.len()],
                    Retile::Density if kind == Kind::Sea => {
                        vec![km2 * g.density.detail / g.density.sea_km2.max(0.01); members.len()]
                    }
                    Retile::Density => {
                        let d = dens.as_ref().unwrap();
                        members.iter().map(|&k| d[px[k]] * km2).collect()
                    }
                };
                let k = (wts.iter().map(|&x| x as f64).sum::<f64>().round() as usize).max(1);
                let lbl = kmeans::split_pixels(&pts, &wts, k, &lim, (p0 as u64) << 16 ^ (gi as u64) << 8 ^ pi as u64);
                let nk = lbl.iter().max().map(|&m| m + 1).unwrap_or(1);
                for (&k, &l) in members.iter().zip(&lbl) {
                    cluster[k] = next_cluster + l;
                }
                next_cluster += nk;
            }
            // local raster for clean-up
            let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
            for &i in &px {
                x0 = x0.min(i % w);
                x1 = x1.max(i % w);
                y0 = y0.min(i / w);
                y1 = y1.max(i / w);
            }
            touched = [touched[0].min(x0), touched[1].min(y0), touched[2].max(x1), touched[3].max(y1)];
            let (lw, lh) = (x1 - x0 + 1, y1 - y0 + 1);
            let mut local = vec![NONE; lw * lh];
            for (k, &i) in px.iter().enumerate() {
                local[(i / w - y0) * lw + (i % w - x0)] = cluster[k];
            }
            let min_px = if matches!(mode, Retile::Count(_)) { 9 } else { lim.min_px as usize };
            clean_clusters(&mut local, lw, lh, min_px);
            for (k, &i) in px.iter().enumerate() {
                cluster[k] = local[(i / w - y0) * lw + (i % w - x0)];
            }
            // reuse the original ids where they overlap most
            let mut overlap: FxHashMap<(u32, Id), usize> = FxHashMap::default();
            for (k, &c) in cluster.iter().enumerate() {
                *overlap.entry((c, orig[k])).or_insert(0) += 1;
            }
            let mut pairs: Vec<((u32, Id), usize)> = overlap.into_iter().collect();
            pairs.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            let mut id_of: FxHashMap<u32, Id> = FxHashMap::default();
            let mut used: FxHashSet<Id> = FxHashSet::default();
            let mut like: FxHashMap<u32, Id> = FxHashMap::default();
            for &((c, o), _) in &pairs {
                like.entry(c).or_insert(o);
                if !id_of.contains_key(&c) && !used.contains(&o) {
                    id_of.insert(c, o);
                    used.insert(o);
                }
            }
            let mut clusters: Vec<u32> = like.keys().copied().collect();
            clusters.sort_unstable();
            for c in clusters {
                if !id_of.contains_key(&c) {
                    let src = like[&c];
                    let (kind, state, region, name) = {
                        let p = self.data.provinces.get(src).unwrap();
                        (p.kind, p.state, p.region, p.name.clone())
                    };
                    let id = self.new_province(kind, state, region, name, Some(src));
                    id_of.insert(c, id);
                }
            }
            after += id_of.len();
            self.with_pixels(|wr, _| {
                for (k, &i) in px.iter().enumerate() {
                    wr.set(i, id_of[&cluster[k]]);
                }
            });
        }
        if touched[0] != usize::MAX {
            self.fix_crossings(Some([touched[0], touched[1], touched[2], touched[3]]));
        }
        Ok((before, after))
    }

    /// Remove points where four provinces meet and diagonal-only contacts
    /// (HOI4 map errors), within `bbox` [x0, y0, x1, y1] or everywhere.
    /// Pixels only change to a neighbouring province of the same type.
    pub fn fix_crossings(&mut self, bbox: Option<[usize; 4]>) -> usize {
        let (w, h) = (self.w as usize, self.h as usize);
        let [x0, y0, x1, y1] = bbox.map(|b| [b[0].saturating_sub(1), b[1].saturating_sub(1), (b[2] + 1).min(w - 1), (b[3] + 1).min(h - 1)]).unwrap_or([0, 0, w - 1, h - 1]);
        if !self.in_txn() {
            return 0;
        }
        self.with_pixels(|wr, data| {
            let mut total = 0;
            for _ in 0..50 {
                let mut bad = Vec::new();
                for y in y0..y1 {
                    for x in x0..x1 {
                        let i = y * w + x;
                        let (a, b, c, d) = (wr.get(i), wr.get(i + 1), wr.get(i + w), wr.get(i + w + 1));
                        let all4 = a != b && a != c && a != d && b != c && b != d && c != d;
                        let diag = a == d && b == c && a != b;
                        if all4 || diag {
                            bad.push(i);
                        }
                    }
                }
                if bad.is_empty() {
                    break;
                }
                let mut fixed = 0;
                for i in bad {
                    let (a, b, c, d) = (wr.get(i), wr.get(i + 1), wr.get(i + w), wr.get(i + w + 1));
                    let all4 = a != b && a != c && a != d && b != c && b != d && c != d;
                    let diag = a == d && b == c && a != b;
                    if !(all4 || diag) {
                        continue;
                    }
                    // d takes b or c; else a takes b or c
                    let same = |p: Id, q: Id| data.kind(p) == data.kind(q);
                    let pref = |p: Id, q: Id| data.state_of(p) == data.state_of(q) && data.region_of(p) == data.region_of(q);
                    let mut done = false;
                    for (pix, cur) in [(i + w + 1, d), (i, a)] {
                        let mut cands = [b, c];
                        cands.sort_by_key(|&q| !pref(cur, q));
                        if let Some(&q) = cands.iter().find(|&&q| same(cur, q)) {
                            wr.set(pix, q);
                            done = true;
                            break;
                        }
                    }
                    if done {
                        fixed += 1;
                    }
                }
                total += fixed;
                if fixed == 0 {
                    break;
                }
            }
            total
        })
    }

    /// Give each extra connected part of a province its own province.
    pub fn split_disconnected(&mut self, ids: &[Id]) -> usize {
        let w = self.w as usize;
        let mut created = 0;
        for &p in ids {
            let px = {
                let lab = self.labels.read();
                self.pixels_of(&lab, p)
            };
            let (parts, n) = pixel_parts(&px, w);
            if n <= 1 {
                continue;
            }
            let mut sizes = vec![0usize; n];
            for &pt in &parts {
                sizes[pt as usize] += 1;
            }
            let keep = (0..n).max_by_key(|&i| sizes[i]).unwrap() as u32;
            let (kind, state, region, name) = {
                let pr = self.data.provinces.get(p).unwrap();
                (pr.kind, pr.state, pr.region, pr.name.clone())
            };
            let mut new_ids = vec![p; n];
            for i in 0..n as u32 {
                if i != keep {
                    new_ids[i as usize] = self.new_province(kind, state, region, name.clone(), Some(p));
                    created += 1;
                }
            }
            self.with_pixels(|wr, _| {
                for (k, &i) in px.iter().enumerate() {
                    wr.set(i, new_ids[parts[k] as usize]);
                }
            });
        }
        created
    }

    /// Hand the pixels of provinces to their neighbours (same type, the same
    /// state first). Returns how many could not be dissolved.
    pub fn dissolve_provinces(&mut self, ids: &[Id]) -> Result<usize> {
        let set: FxHashSet<Id> = ids.iter().copied().filter(|&p| self.data.provinces.contains(p)).collect();
        if set.is_empty() {
            bail!("nothing selected");
        }
        let (w, h) = (self.w as usize, self.h as usize);
        // hole: pixel -> original province
        let mut hole: FxHashMap<usize, Id> = FxHashMap::default();
        {
            let lab = self.labels.read();
            for &p in &set {
                for i in self.pixels_of(&lab, p) {
                    hole.insert(i, p);
                }
            }
        }
        let mut assigned: FxHashMap<usize, Id> = FxHashMap::default();
        let nbs = |i: usize| {
            let (x, y) = (i % w, i / w);
            [(x > 0, i.wrapping_sub(1)), (x + 1 < w, i + 1), (y > 0, i.wrapping_sub(w)), (y + 1 < h, i + w)]
        };
        let data = &self.data;
        let allowed = |orig: Id, q: Id, strict: bool| {
            data.kind(q) == data.kind(orig) && (!strict || (data.state_of(q) == data.state_of(orig) && data.region_of(q) == data.region_of(orig)))
        };
        {
            let lab = self.labels.read();
            for strict in [true, false] {
                let mut queue: VecDeque<usize> = VecDeque::new();
                for (&i, &orig) in &hole {
                    if assigned.contains_key(&i) {
                        continue;
                    }
                    for (ok, j) in nbs(i) {
                        if !ok || hole.contains_key(&j) {
                            continue;
                        }
                        let q = lab.px[j];
                        if allowed(orig, q, strict) {
                            assigned.insert(i, q);
                            queue.push_back(i);
                            break;
                        }
                    }
                }
                // also grow from pixels assigned in the strict round
                if !strict {
                    queue.extend(assigned.keys().copied());
                }
                while let Some(i) = queue.pop_front() {
                    let q = assigned[&i];
                    for (ok, j) in nbs(i) {
                        if !ok {
                            continue;
                        }
                        if let Some(&orig) = hole.get(&j) {
                            if !assigned.contains_key(&j) && allowed(orig, q, strict) {
                                assigned.insert(j, q);
                                queue.push_back(j);
                            }
                        }
                    }
                }
            }
        }
        let left: FxHashSet<Id> = hole.iter().filter(|(i, _)| !assigned.contains_key(i)).map(|(_, &p)| p).collect();
        self.with_pixels(|wr, _| {
            for (&i, &q) in &assigned {
                wr.set(i, q);
            }
        });
        Ok(left.len())
    }

    /// Change the type of provinces, keeping states and regions consistent.
    pub fn set_kind(&mut self, ids: &[Id], kind: Kind) -> Result<usize> {
        let mut n = 0;
        for &p in ids {
            let Some(old) = self.data.provinces.get(p).map(|x| x.kind) else { continue };
            if old == kind {
                continue;
            }
            // most-bordering neighbour of a type
            let best_nb = |doc: &Doc, want: &dyn Fn(Id) -> bool| {
                doc.stats.neighbours(p).iter().copied().filter(|&q| want(q)).max_by_key(|&q| doc.stats.border_len(p, q))
            };
            match kind {
                Kind::Sea => {
                    let region = best_nb(self, &|q| self.data.kind(q) == Kind::Sea).map(|q| self.data.region_of(q));
                    let region = match region.filter(|&r| r != 0) {
                        Some(r) => r,
                        None => self.data.regions.push(|id| Region {
                            id,
                            name: "New sea zone".into(),
                            naval: true,
                            provinces: Vec::new(),
                            states: Vec::new(),
                            extra: Default::default(),
                        }),
                    };
                    let pr = self.data.provinces.get_mut(p).unwrap();
                    pr.kind = Kind::Sea;
                    pr.state = 0;
                    pr.region = region;
                }
                Kind::Lake => {
                    let region = if old == Kind::Sea {
                        best_nb(self, &|q| self.data.kind(q) != Kind::Sea).map(|q| self.data.region_of(q)).unwrap_or(0)
                    } else {
                        self.data.region_of(p)
                    };
                    let pr = self.data.provinces.get_mut(p).unwrap();
                    pr.kind = Kind::Lake;
                    pr.state = 0;
                    pr.region = region;
                }
                Kind::Land => {
                    let Some(q) = best_nb(self, &|q| self.data.kind(q) == Kind::Land && self.data.state_of(q) != 0) else {
                        bail!("{} has no land neighbour whose state it could join", self.data.prov_name(p));
                    };
                    let (state, region) = (self.data.state_of(q), self.data.region_of(q));
                    let pr = self.data.provinces.get_mut(p).unwrap();
                    pr.kind = Kind::Land;
                    pr.state = state;
                    pr.region = region;
                }
            }
            n += 1;
        }
        Ok(n)
    }

    // ================================================================ graphs

    /// Border length between provinces of different states (land), plus land
    /// links of straits.
    fn state_links(&self) -> FxHashMap<(Id, Id), u32> {
        let mut out: FxHashMap<(Id, Id), u32> = FxHashMap::default();
        for (&(a, b), &len) in &self.stats.border {
            let (sa, sb) = (self.data.state_of(a), self.data.state_of(b));
            if sa != 0 && sb != 0 && sa != sb {
                *out.entry((sa.min(sb), sa.max(sb))).or_insert(0) += len;
            }
        }
        for adj in &self.data.adjacencies {
            if adj.kind != "impassable" {
                let (sa, sb) = (self.data.state_of(adj.from), self.data.state_of(adj.to));
                if sa != 0 && sb != 0 && sa != sb {
                    *out.entry((sa.min(sb), sa.max(sb))).or_insert(0) += 1;
                }
            }
        }
        out
    }

    fn centre_of(&self, provs: &[Id]) -> [f64; 2] {
        let (mut sx, mut sy, mut n) = (0f64, 0f64, 0f64);
        for &p in provs {
            let s = self.stats.get(p);
            sx += s.sx as f64;
            sy += s.sy as f64;
            n += s.px as f64;
        }
        if n == 0.0 { [0.0, 0.0] } else { [sx / n + 0.5, sy / n + 0.5] }
    }

    fn px_of(&self, provs: &[Id]) -> u64 {
        provs.iter().map(|&p| self.stats.get(p).px as u64).sum()
    }

    /// Partition `items` (each a set of provinces) into k contiguous groups.
    fn partition_items(&self, items: &[Vec<Id>], k: usize, seed: u64) -> Vec<u32> {
        let pos: Vec<[f32; 2]> = items.iter().map(|v| {
            let c = self.centre_of(v);
            [c[0] as f32, c[1] as f32]
        }).collect();
        let w: Vec<f32> = items.iter().map(|v| self.px_of(v) as f32).collect();
        let mut owner: FxHashMap<Id, usize> = FxHashMap::default();
        for (i, v) in items.iter().enumerate() {
            for &p in v {
                owner.insert(p, i);
            }
        }
        let mut nb: Vec<FxHashSet<usize>> = vec![FxHashSet::default(); items.len()];
        for (i, v) in items.iter().enumerate() {
            for &p in v {
                for &q in self.stats.neighbours(p) {
                    if let Some(&j) = owner.get(&q) {
                        if j != i {
                            nb[i].insert(j);
                        }
                    }
                }
            }
        }
        for adj in &self.data.adjacencies {
            if adj.kind != "impassable" {
                if let (Some(&i), Some(&j)) = (owner.get(&adj.from), owner.get(&adj.to)) {
                    if i != j {
                        nb[i].insert(j);
                        nb[j].insert(i);
                    }
                }
            }
        }
        let nb: Vec<Vec<usize>> = nb.into_iter().map(|s| s.into_iter().collect()).collect();
        kmeans::partition(&pos, &w, &nb, k, seed)
    }

    // ================================================================ states

    pub fn new_state(&mut self, name: String, nation: Option<Id>, impassable: bool) -> Id {
        self.data.states.push(|id| State {
            id,
            name,
            nation,
            impassable,
            provinces: Vec::new(),
            manpower: None,
            category: None,
            buildings: Default::default(),
            resources: Default::default(),
            victory_points: Vec::new(),
            extra: Default::default(),
        })
    }

    /// Move land provinces into state `s` (they take the state's strategic region).
    pub fn assign_provinces_to_state(&mut self, provs: &[Id], s: Id) -> usize {
        if !self.data.states.contains(s) {
            return 0;
        }
        let moving: FxHashSet<Id> = provs.iter().copied().collect();
        let mut regions: FxHashMap<Id, u64> = FxHashMap::default();
        for (pid, p) in self.data.provinces.iter() {
            if p.state == s && !moving.contains(&pid) {
                *regions.entry(p.region).or_insert(0) += self.stats.get(pid).px as u64;
            }
        }
        let region = regions.into_iter().max_by_key(|&(r, n)| (n, r)).map(|(r, _)| r);
        let mut n = 0;
        for &p in provs {
            if let Some(pr) = self.data.provinces.get_mut(p) {
                if pr.kind == Kind::Land && pr.state != s {
                    pr.state = s;
                    if let Some(r) = region {
                        pr.region = r;
                    }
                    n += 1;
                }
            }
        }
        n
    }

    pub fn new_state_from(&mut self, provs: &[Id]) -> Result<Id> {
        let land: Vec<Id> = provs.iter().copied().filter(|&p| self.data.kind(p) == Kind::Land).collect();
        if land.is_empty() {
            bail!("select land provinces");
        }
        let mut nations: FxHashMap<Id, u64> = FxHashMap::default();
        for &p in &land {
            *nations.entry(self.data.nation_of(p)).or_insert(0) += self.stats.get(p).px as u64;
        }
        let nation = nations.into_iter().max_by_key(|&(n, c)| (c, n)).map(|(n, _)| n).filter(|&n| n != 0);
        let biggest = *land.iter().max_by_key(|&&p| self.stats.get(p).px).unwrap();
        let name = format!("{} (new)", self.data.prov_name(biggest));
        let s = self.new_state(name, nation, false);
        self.assign_provinces_to_state(&land, s);
        Ok(s)
    }

    pub fn merge_states(&mut self, ids: &[Id]) -> Result<Id> {
        let Some(&target) = ids.first() else { bail!("nothing selected") };
        if ids.len() < 2 {
            bail!("select at least two states");
        }
        let provs: Vec<Id> = ids[1..].iter().flat_map(|&s| self.index.provs_of_state(s).to_vec()).collect();
        let vps: Vec<[u32; 2]> = ids[1..].iter().filter_map(|&s| self.data.states.get(s)).flat_map(|s| s.victory_points.clone()).collect();
        for &p in &provs {
            if let Some(pr) = self.data.provinces.get_mut(p) {
                pr.state = target;
            }
        }
        if let Some(t) = self.data.states.get_mut(target) {
            t.victory_points.extend(vps);
        }
        // nations whose capital was merged away get it fixed in the clean-up
        Ok(target)
    }

    /// Cut a state into `k` states.
    pub fn divide_state(&mut self, s: Id, k: usize) -> Result<Vec<Id>> {
        let provs = self.index.provs_of_state(s).to_vec();
        if provs.len() < 2 {
            bail!("{} has only one province (divide the province first)", self.data.state_name(s));
        }
        let items: Vec<Vec<Id>> = provs.iter().map(|&p| vec![p]).collect();
        let lbl = self.partition_items(&items, k.min(provs.len()), s as u64);
        Ok(self.apply_state_groups(s, &provs, &lbl))
    }

    /// Group 0..k of `lbl` (by province): the largest group keeps `s`, the
    /// others become new states of the same nation.
    fn apply_state_groups(&mut self, s: Id, provs: &[Id], lbl: &[u32]) -> Vec<Id> {
        let st = self.data.states.get(s).cloned().unwrap();
        let mut size: BTreeMap<u32, u64> = BTreeMap::new();
        for (i, &l) in lbl.iter().enumerate() {
            *size.entry(l).or_insert(0) += self.stats.get(provs[i]).px as u64;
        }
        let keep = size.iter().max_by_key(|(l, n)| (**n, std::cmp::Reverse(**l))).map(|(l, _)| *l).unwrap_or(0);
        let mut ids = vec![s];
        let mut n = 1;
        let mut map: FxHashMap<u32, Id> = FxHashMap::default();
        map.insert(keep, s);
        for &l in size.keys() {
            if l != keep {
                n += 1;
                let id = self.new_state(format!("{} {}", st.name, roman(n)), st.nation, st.impassable);
                map.insert(l, id);
                ids.push(id);
            }
        }
        for (i, &l) in lbl.iter().enumerate() {
            self.data.provinces.get_mut(provs[i]).unwrap().state = map[&l];
        }
        ids
    }

    /// Hand the provinces of states to neighbouring states (same nation first).
    pub fn dissolve_states(&mut self, ids: &[Id]) -> Result<usize> {
        let gone: FxHashSet<Id> = ids.iter().copied().collect();
        let mut pending: Vec<Id> = ids.iter().flat_map(|&s| self.index.provs_of_state(s).to_vec()).collect();
        let nation_of: FxHashMap<Id, Id> = pending.iter().map(|&p| (p, self.data.nation_of(p))).collect();
        let mut target: FxHashMap<Id, Id> = FxHashMap::default();
        loop {
            let mut progress = false;
            let mut rest = Vec::new();
            for &p in &pending {
                let mut best: Option<(bool, u32, Id)> = None;
                for &q in self.stats.neighbours(p) {
                    let sq = target.get(&q).copied().unwrap_or_else(|| self.data.state_of(q));
                    if sq == 0 || gone.contains(&sq) || self.data.kind(q) != Kind::Land {
                        continue;
                    }
                    let same = self.data.nation_of_state(sq) == nation_of[&p];
                    let cand = (same, self.stats.border_len(p, q), sq);
                    if best.is_none_or(|b| cand > b) {
                        best = Some(cand);
                    }
                }
                match best {
                    Some((_, _, s)) => {
                        target.insert(p, s);
                        progress = true;
                    }
                    None => rest.push(p),
                }
            }
            pending = rest;
            if pending.is_empty() || !progress {
                break;
            }
        }
        for (&p, &s) in &target {
            self.data.provinces.get_mut(p).unwrap().state = s;
        }
        if !pending.is_empty() {
            bail!("{} provinces have no neighbouring state to join", pending.len());
        }
        Ok(target.len())
    }

    /// Regroup the passable provinces of nations into states of about
    /// `state_provinces` provinces (at most `state_max_km2`).
    pub fn generate_states(&mut self, nations: &[Id], g: &GenParams) -> Result<usize> {
        let km2 = self.data.km2_per_px();
        let mut made = 0;
        for &n in nations {
            let Some(nat) = self.data.nations.get(n).cloned() else { continue };
            let states: Vec<Id> = self.index.states_of_nation(n).iter().copied().filter(|&s| !self.data.states.get(s).is_some_and(|x| x.impassable)).collect();
            let provs: Vec<Id> = states.iter().flat_map(|&s| self.index.provs_of_state(s).to_vec()).filter(|&p| !self.data.provinces.get(p).is_some_and(|x| x.edge)).collect();
            if provs.is_empty() {
                continue;
            }
            // connected parts of the nation's land
            let set: FxHashSet<Id> = provs.iter().copied().collect();
            let mut part: FxHashMap<Id, usize> = FxHashMap::default();
            let mut parts: Vec<Vec<Id>> = Vec::new();
            for &p in &provs {
                if part.contains_key(&p) {
                    continue;
                }
                let id = parts.len();
                let mut stack = vec![p];
                part.insert(p, id);
                let mut members = Vec::new();
                while let Some(v) = stack.pop() {
                    members.push(v);
                    for &q in self.stats.neighbours(v) {
                        if set.contains(&q) && !part.contains_key(&q) {
                            part.insert(q, id);
                            stack.push(q);
                        }
                    }
                }
                parts.push(members);
            }
            let home = [nat.col, nat.row];
            let mut groups: Vec<Vec<Id>> = Vec::new();
            for (pi, members) in parts.iter().enumerate() {
                let area = self.px_of(members) as f64 * km2;
                let k = ((members.len() as f32 / g.state_provinces.max(1.0)).round() as usize)
                    .max((area / g.state_max_km2.max(1.0) as f64).ceil() as usize)
                    .max(1)
                    .min(members.len());
                let items: Vec<Vec<Id>> = members.iter().map(|&p| vec![p]).collect();
                let lbl = self.partition_items(&items, k, n as u64 * 1000 + pi as u64);
                let mut by: BTreeMap<u32, Vec<Id>> = BTreeMap::new();
                for (i, &l) in lbl.iter().enumerate() {
                    by.entry(l).or_default().push(members[i]);
                }
                groups.extend(by.into_values());
            }
            // the capital group holds the province nearest the settlement
            let cap_group = (0..groups.len())
                .min_by(|&a, &b| {
                    let d = |g: &Vec<Id>| {
                        g.iter()
                            .map(|&p| {
                                let c = self.stats.get(p).centre();
                                (c[0] - home[0]).hypot(c[1] - home[1])
                            })
                            .fold(f64::MAX, f64::min)
                    };
                    d(&groups[a]).total_cmp(&d(&groups[b]))
                })
                .unwrap();
            let cap_centre = self.centre_of(&groups[cap_group]);
            let mut used_names: FxHashMap<String, usize> = FxHashMap::default();
            let mut new_states = Vec::new();
            for (gi, grp) in groups.iter().enumerate() {
                let mut name = if gi == cap_group { nat.name.clone() } else { format!("{} {}", nat.name, direction(cap_centre, self.centre_of(grp))) };
                let k = used_names.entry(name.clone()).or_insert(0);
                *k += 1;
                if *k > 1 {
                    name = format!("{name} {}", roman(*k));
                }
                let s = self.new_state(name, Some(n), false);
                for &p in grp {
                    self.data.provinces.get_mut(p).unwrap().state = s;
                }
                if gi == cap_group {
                    self.data.nations.get_mut(n).unwrap().capital_state = s;
                }
                new_states.push(s);
                made += 1;
            }
            // provinces left in the old states (edge provinces) stay there
        }
        self.reindex();
        Ok(made)
    }

    // ================================================================ nations

    pub fn assign_states_to_nation(&mut self, states: &[Id], n: Id) -> usize {
        if !self.data.nations.contains(n) {
            return 0;
        }
        let mut c = 0;
        for &s in states {
            if let Some(st) = self.data.states.get_mut(s) {
                if st.nation != Some(n) {
                    st.nation = Some(n);
                    c += 1;
                }
            }
        }
        c
    }

    pub fn new_nation_from(&mut self, states: &[Id]) -> Result<Id> {
        let states: Vec<Id> = states.iter().copied().filter(|&s| self.data.states.contains(s)).collect();
        if states.is_empty() {
            bail!("select states");
        }
        if states.iter().all(|&s| self.data.states.get(s).is_some_and(|x| x.impassable)) {
            bail!("a nation needs a passable state for its capital (these are all impassable)");
        }
        let capital = *states
            .iter()
            .max_by_key(|&&s| (!self.data.states.get(s).unwrap().impassable, self.px_of(self.index.provs_of_state(s))))
            .unwrap();
        let mut langs: FxHashMap<String, usize> = FxHashMap::default();
        for &s in &states {
            if let Some(n) = self.data.states.get(s).and_then(|x| x.nation).and_then(|n| self.data.nations.get(n)) {
                *langs.entry(n.language.clone()).or_insert(0) += 1;
            }
        }
        let language = langs.into_iter().max_by_key(|(l, c)| (*c, l.clone())).map(|(l, _)| l).unwrap_or_else(|| "de".into());
        let c = self.centre_of(self.index.provs_of_state(capital));
        let name = self.data.state_name(capital);
        let n = self.data.nations.push(|id| Nation {
            id,
            name,
            place: "village".into(),
            row: c[1],
            col: c[0],
            capital_state: capital,
            states: Vec::new(),
            language,
            km2: 0.0,
            tag: None,
            color: None,
            ideology: None,
            no_units: false,
            extra: Default::default(),
        });
        self.assign_states_to_nation(&states, n);
        Ok(n)
    }

    pub fn merge_nations(&mut self, ids: &[Id]) -> Result<Id> {
        let Some(&target) = ids.first() else { bail!("nothing selected") };
        if ids.len() < 2 {
            bail!("select at least two nations");
        }
        let states: Vec<Id> = ids[1..].iter().flat_map(|&n| self.index.states_of_nation(n).to_vec()).collect();
        self.assign_states_to_nation(&states, target);
        Ok(target)
    }

    /// Cut a nation into `k` nations (by its states).
    pub fn divide_nation(&mut self, n: Id, k: usize) -> Result<Vec<Id>> {
        let states = self.index.states_of_nation(n).to_vec();
        if states.len() < 2 {
            bail!("{} has only one state (divide the state first)", self.data.nation_name(n));
        }
        let items: Vec<Vec<Id>> = states.iter().map(|&s| self.index.provs_of_state(s).to_vec()).collect();
        let lbl = self.partition_items(&items, k.min(states.len()), n as u64);
        let cap = self.data.nations.get(n).map(|x| x.capital_state).unwrap_or(0);
        let keep = states.iter().position(|&s| s == cap).map(|i| lbl[i]).unwrap_or(0);
        let mut by: BTreeMap<u32, Vec<Id>> = BTreeMap::new();
        for (i, &l) in lbl.iter().enumerate() {
            by.entry(l).or_default().push(states[i]);
        }
        let mut out = vec![n];
        for (l, grp) in by {
            // a group of impassable states only stays with the old nation
            let passable = grp.iter().any(|&s| !self.data.states.get(s).is_some_and(|x| x.impassable));
            if l != keep && passable {
                let id = self.new_nation_from(&grp)?;
                out.push(id);
            }
        }
        Ok(out)
    }

    /// Hand the states of nations to their neighbours (longest border).
    pub fn dissolve_nations(&mut self, ids: &[Id]) -> Result<usize> {
        let gone: FxHashSet<Id> = ids.iter().copied().collect();
        let links = self.state_links();
        let mut pending: Vec<Id> = ids.iter().flat_map(|&n| self.index.states_of_nation(n).to_vec()).collect();
        let mut target: FxHashMap<Id, Id> = FxHashMap::default();
        loop {
            let mut progress = false;
            let mut rest = Vec::new();
            for &s in &pending {
                let mut best: Option<(u32, Id)> = None;
                for (&(a, b), &len) in &links {
                    let o = if a == s { b } else if b == s { a } else { continue };
                    let on = target.get(&o).copied().unwrap_or_else(|| self.data.nation_of_state(o));
                    if on == 0 || gone.contains(&on) {
                        continue;
                    }
                    if best.is_none_or(|bst| (len, on) > bst) {
                        best = Some((len, on));
                    }
                }
                match best {
                    Some((_, on)) => {
                        target.insert(s, on);
                        progress = true;
                    }
                    None => rest.push(s),
                }
            }
            pending = rest;
            if pending.is_empty() || !progress {
                break;
            }
        }
        for (&s, &n) in &target {
            self.data.states.get_mut(s).unwrap().nation = Some(n);
        }
        if !pending.is_empty() {
            bail!("{} states have no neighbouring nation to join", pending.len());
        }
        Ok(target.len())
    }

    /// Share the states of the given nations out again among their capitals:
    /// every state goes to the capital nearest over land (state to state).
    pub fn generate_nations(&mut self, ids: &[Id]) -> Result<usize> {
        let states: Vec<Id> = ids.iter().flat_map(|&n| self.index.states_of_nation(n).to_vec()).collect();
        if ids.len() < 2 {
            bail!("select at least two nations to share their land out again");
        }
        let set: FxHashSet<Id> = states.iter().copied().collect();
        let centre: FxHashMap<Id, [f64; 2]> = states.iter().map(|&s| (s, self.centre_of(self.index.provs_of_state(s)))).collect();
        let mut adj: FxHashMap<Id, Vec<Id>> = FxHashMap::default();
        for &(a, b) in self.state_links().keys() {
            if set.contains(&a) && set.contains(&b) {
                adj.entry(a).or_default().push(b);
                adj.entry(b).or_default().push(a);
            }
        }
        // multi-source Dijkstra from the capital states
        let mut best: FxHashMap<Id, (f64, Id)> = FxHashMap::default();
        let mut heap = BinaryHeap::new();
        for &n in ids {
            if let Some(c) = self.data.nations.get(n).map(|x| x.capital_state).filter(|c| set.contains(c)) {
                best.insert(c, (0.0, n));
                heap.push((std::cmp::Reverse(0u64), c, n));
            }
        }
        while let Some((std::cmp::Reverse(d), s, n)) = heap.pop() {
            let d = d as f64 / 1000.0;
            if best.get(&s).is_some_and(|b| b.0 < d) {
                continue;
            }
            for &o in adj.get(&s).map(|v| v.as_slice()).unwrap_or(&[]) {
                let (a, b) = (centre[&s], centre[&o]);
                let nd = d + (a[0] - b[0]).hypot(a[1] - b[1]);
                if best.get(&o).is_none_or(|x| nd < x.0) {
                    best.insert(o, (nd, n));
                    heap.push((std::cmp::Reverse((nd * 1000.0) as u64), o, n));
                }
            }
        }
        let mut moved = 0;
        for (&s, &(_, n)) in &best {
            let st = self.data.states.get_mut(s).unwrap();
            if st.nation != Some(n) {
                st.nation = Some(n);
                moved += 1;
            }
        }
        Ok(moved)
    }

    // ================================================================ strategic regions

    pub fn new_region(&mut self, name: String, naval: bool) -> Id {
        self.data.regions.push(|id| Region { id, name, naval, provinces: Vec::new(), states: Vec::new(), extra: Default::default() })
    }

    /// Members of a region selection: land regions take whole states.
    fn region_members(&self, provs: &[Id], naval: bool) -> Vec<Id> {
        let mut out: FxHashSet<Id> = FxHashSet::default();
        for &p in provs {
            match (naval, self.data.kind(p)) {
                (true, Kind::Sea) => {
                    out.insert(p);
                }
                (false, Kind::Land) => {
                    let s = self.data.state_of(p);
                    if s == 0 {
                        out.insert(p);
                    } else {
                        out.extend(self.index.provs_of_state(s).iter().copied());
                    }
                }
                (false, Kind::Lake) => {
                    out.insert(p);
                }
                _ => {}
            }
        }
        let mut v: Vec<Id> = out.into_iter().collect();
        v.sort_unstable();
        v
    }

    pub fn assign_to_region(&mut self, provs: &[Id], r: Id) -> Result<usize> {
        let Some(naval) = self.data.regions.get(r).map(|x| x.naval) else { bail!("no such region") };
        let members = self.region_members(provs, naval);
        let mut n = 0;
        for p in members {
            let pr = self.data.provinces.get_mut(p).unwrap();
            if pr.region != r {
                pr.region = r;
                n += 1;
            }
        }
        Ok(n)
    }

    pub fn new_region_from(&mut self, provs: &[Id], naval: bool) -> Result<Id> {
        let members = self.region_members(provs, naval);
        if members.is_empty() {
            bail!("select {} provinces", if naval { "sea" } else { "land" });
        }
        let biggest = *members.iter().max_by_key(|&&p| self.stats.get(p).px).unwrap();
        let name = if naval { format!("{} waters", self.data.prov_name(biggest)) } else { self.data.state_name(self.data.state_of(biggest)) };
        let r = self.new_region(name, naval);
        self.assign_to_region(&members, r)?;
        Ok(r)
    }

    pub fn merge_regions(&mut self, ids: &[Id]) -> Result<Id> {
        let Some(&target) = ids.first() else { bail!("nothing selected") };
        if ids.len() < 2 {
            bail!("select at least two zones");
        }
        let naval = self.data.regions.get(target).map(|r| r.naval).unwrap_or(false);
        if ids.iter().any(|&r| self.data.regions.get(r).is_some_and(|x| x.naval != naval)) {
            bail!("air zones (land) and sea zones cannot be merged");
        }
        for &r in &ids[1..] {
            for p in self.index.provs_of_region(r).to_vec() {
                self.data.provinces.get_mut(p).unwrap().region = target;
            }
        }
        Ok(target)
    }

    /// Items of a region for partitioning: states (land) or provinces (sea).
    fn region_items(&self, provs: &[Id], naval: bool) -> Vec<Vec<Id>> {
        if naval {
            return provs.iter().map(|&p| vec![p]).collect();
        }
        let mut by: BTreeMap<Id, Vec<Id>> = BTreeMap::new();
        let mut loose = Vec::new();
        for &p in provs {
            match self.data.state_of(p) {
                0 => loose.push(vec![p]),
                s => by.entry(s).or_default().push(p),
            }
        }
        by.into_values().chain(loose).collect()
    }

    pub fn divide_region(&mut self, r: Id, k: usize) -> Result<Vec<Id>> {
        let Some(reg) = self.data.regions.get(r).cloned() else { bail!("no such zone") };
        let provs = self.index.provs_of_region(r).to_vec();
        let items = self.region_items(&provs, reg.naval);
        if items.len() < 2 {
            bail!("{} cannot be divided further", reg.name);
        }
        let lbl = self.partition_items(&items, k.min(items.len()), r as u64);
        Ok(self.apply_region_groups(r, &items, &lbl))
    }

    fn apply_region_groups(&mut self, r: Id, items: &[Vec<Id>], lbl: &[u32]) -> Vec<Id> {
        let reg = self.data.regions.get(r).cloned().unwrap();
        let mut size: BTreeMap<u32, u64> = BTreeMap::new();
        for (i, &l) in lbl.iter().enumerate() {
            *size.entry(l).or_insert(0) += self.px_of(&items[i]);
        }
        let keep = size.iter().max_by_key(|(l, n)| (**n, std::cmp::Reverse(**l))).map(|(l, _)| *l).unwrap_or(0);
        let mut map: FxHashMap<u32, Id> = FxHashMap::default();
        map.insert(keep, r);
        let mut out = vec![r];
        let mut n = 1;
        for &l in size.keys() {
            if l != keep {
                n += 1;
                let id = self.new_region(format!("{} {}", reg.name, roman(n)), reg.naval);
                map.insert(l, id);
                out.push(id);
            }
        }
        for (i, &l) in lbl.iter().enumerate() {
            for &p in &items[i] {
                self.data.provinces.get_mut(p).unwrap().region = map[&l];
            }
        }
        out
    }

    pub fn dissolve_regions(&mut self, ids: &[Id]) -> Result<usize> {
        let gone: FxHashSet<Id> = ids.iter().copied().collect();
        let mut pending: Vec<Id> = ids.iter().flat_map(|&r| self.index.provs_of_region(r).to_vec()).collect();
        let mut target: FxHashMap<Id, Id> = FxHashMap::default();
        loop {
            let mut progress = false;
            let mut rest = Vec::new();
            for &p in &pending {
                let sea = self.data.kind(p) == Kind::Sea;
                let mut best: Option<(u32, Id)> = None;
                for &q in self.stats.neighbours(p) {
                    let rq = target.get(&q).copied().unwrap_or_else(|| self.data.region_of(q));
                    if rq == 0 || gone.contains(&rq) || (self.data.kind(q) == Kind::Sea) != sea {
                        continue;
                    }
                    let c = (self.stats.border_len(p, q), rq);
                    if best.is_none_or(|b| c > b) {
                        best = Some(c);
                    }
                }
                match best {
                    Some((_, r)) => {
                        target.insert(p, r);
                        progress = true;
                    }
                    None => rest.push(p),
                }
            }
            pending = rest;
            if pending.is_empty() || !progress {
                break;
            }
        }
        // land: keep states in one region (majority of their provinces)
        let mut by_state: FxHashMap<Id, FxHashMap<Id, u64>> = FxHashMap::default();
        for (&p, &r) in &target {
            let s = self.data.state_of(p);
            if s != 0 {
                *by_state.entry(s).or_default().entry(r).or_insert(0) += self.stats.get(p).px as u64;
            }
        }
        for (&p, &r) in &target {
            let s = self.data.state_of(p);
            let r = by_state.get(&s).and_then(|m| m.iter().max_by_key(|(r, n)| (**n, **r)).map(|(r, _)| *r)).unwrap_or(r);
            self.data.provinces.get_mut(p).unwrap().region = r;
        }
        if !pending.is_empty() {
            bail!("{} provinces have no neighbouring zone to join", pending.len());
        }
        Ok(target.len())
    }

    /// Regroup the members of zones into zones of about `region_km2` (land)
    /// or, for sea, the same number of zones as selected unless `count` is given.
    pub fn generate_regions(&mut self, ids: &[Id], g: &GenParams, count: Option<usize>) -> Result<usize> {
        let regs: Vec<Region> = ids.iter().filter_map(|&r| self.data.regions.get(r).cloned()).collect();
        let Some(first) = regs.first() else { bail!("nothing selected") };
        let naval = first.naval;
        if regs.iter().any(|r| r.naval != naval) {
            bail!("select only air zones or only sea zones");
        }
        let provs: Vec<Id> = regs.iter().flat_map(|r| self.index.provs_of_region(r.id).to_vec()).collect();
        let items = self.region_items(&provs, naval);
        let area = self.px_of(&provs) as f64 * self.data.km2_per_px();
        let k = count.unwrap_or_else(|| if naval { regs.len() } else { (area / g.region_km2.max(1.0) as f64).round().max(1.0) as usize }).clamp(1, items.len());
        let lbl = self.partition_items(&items, k, first.id as u64 * 7);
        // reuse the selected ids for the biggest groups
        let mut size: BTreeMap<u32, u64> = BTreeMap::new();
        for (i, &l) in lbl.iter().enumerate() {
            *size.entry(l).or_insert(0) += self.px_of(&items[i]);
        }
        let mut order: Vec<u32> = size.keys().copied().collect();
        order.sort_by_key(|l| std::cmp::Reverse(size[l]));
        let mut map: FxHashMap<u32, Id> = FxHashMap::default();
        for (i, &l) in order.iter().enumerate() {
            let id = match regs.get(i) {
                Some(r) => r.id,
                None => self.new_region(format!("{} {}", first.name, roman(i + 1)), naval),
            };
            map.insert(l, id);
        }
        for (i, &l) in lbl.iter().enumerate() {
            for &p in &items[i] {
                self.data.provinces.get_mut(p).unwrap().region = map[&l];
            }
        }
        Ok(order.len())
    }

    // ================================================================ inaccessible zones

    /// Connected groups of provinces (by border), split by `key`.
    fn connected_groups(&self, provs: &[Id], key: impl Fn(Id) -> u64) -> Vec<Vec<Id>> {
        let set: FxHashSet<Id> = provs.iter().copied().collect();
        let mut seen: FxHashSet<Id> = FxHashSet::default();
        let mut out = Vec::new();
        for &p in provs {
            if !seen.insert(p) {
                continue;
            }
            let k = key(p);
            let mut stack = vec![p];
            let mut grp = Vec::new();
            while let Some(v) = stack.pop() {
                grp.push(v);
                for &q in self.stats.neighbours(v) {
                    if set.contains(&q) && key(q) == k && seen.insert(q) {
                        stack.push(q);
                    }
                }
            }
            out.push(grp);
        }
        out
    }

    /// Put land provinces into impassable states (vanilla style, like the
    /// Sahara): one per nation and connected area. With `coarsen`, each area
    /// becomes as few provinces as the size limits allow.
    pub fn make_impassable(&mut self, provs: &[Id], coarsen: bool, g: &GenParams) -> Result<usize> {
        let land: Vec<Id> = provs.iter().copied().filter(|&p| self.data.kind(p) == Kind::Land).collect();
        if land.is_empty() {
            bail!("select land provinces");
        }
        let groups = self.connected_groups(&land, |p| self.data.nation_of(p) as u64);
        let mut made = 0;
        for grp in groups {
            let n = self.data.nation_of(grp[0]);
            let name = format!("{} (impassable)", if n != 0 { self.data.nation_name(n) } else { "Wasteland".into() });
            let s = self.new_state(name, (n != 0).then_some(n), true);
            // keep the region of most of the area
            let mut regions: FxHashMap<Id, u64> = FxHashMap::default();
            for &p in &grp {
                *regions.entry(self.data.region_of(p)).or_insert(0) += self.stats.get(p).px as u64;
            }
            let region = regions.into_iter().max_by_key(|&(r, c)| (c, r)).map(|(r, _)| r).unwrap_or(0);
            for &p in &grp {
                let pr = self.data.provinces.get_mut(p).unwrap();
                pr.state = s;
                if region != 0 {
                    pr.region = region;
                }
            }
            if coarsen && grp.len() > 1 {
                self.retile(&grp, Retile::Coarse, g)?;
            }
            made += 1;
        }
        Ok(made)
    }

    /// Take land provinces out of play: HOI4 "lake" provinces with no state,
    /// no owner and no units (they keep their land look and their air zone).
    pub fn remove_from_play(&mut self, provs: &[Id], coarsen: bool, g: &GenParams) -> Result<usize> {
        let land: Vec<Id> = provs.iter().copied().filter(|&p| self.data.kind(p) != Kind::Sea).collect();
        if land.is_empty() {
            bail!("select land provinces");
        }
        for &p in &land {
            let pr = self.data.provinces.get_mut(p).unwrap();
            pr.kind = Kind::Lake;
            pr.state = 0;
        }
        if coarsen {
            let groups = self.connected_groups(&land, |p| self.data.region_of(p) as u64);
            for grp in groups {
                if grp.len() > 1 {
                    self.retile(&grp, Retile::Coarse, g)?;
                }
            }
        }
        Ok(land.len())
    }

    /// Back into play: removed provinces join a neighbouring state; impassable
    /// states become passable.
    pub fn restore_access(&mut self, provs: &[Id]) -> Result<usize> {
        let mut n = 0;
        let lakes: Vec<Id> = provs.iter().copied().filter(|&p| self.data.kind(p) == Kind::Lake).collect();
        if !lakes.is_empty() {
            n += self.set_kind(&lakes, Kind::Land)?;
        }
        let states: FxHashSet<Id> = provs.iter().map(|&p| self.data.state_of(p)).filter(|&s| s != 0).collect();
        for s in states {
            if let Some(st) = self.data.states.get_mut(s) {
                if st.impassable {
                    st.impassable = false;
                    n += 1;
                }
            }
        }
        Ok(n)
    }

    // ================================================================ adjacencies

    /// Pixels of `p` on its border (subsampled to about `max`).
    fn border_pixels(&self, lab: &Labels, p: Id, max: usize) -> Vec<[u32; 2]> {
        let (w, h) = (self.w as usize, self.h as usize);
        let mut out = Vec::new();
        for i in self.pixels_of(lab, p) {
            let (x, y) = (i % w, i / w);
            let edge = (x > 0 && lab.px[i - 1] != p) || (x + 1 < w && lab.px[i + 1] != p) || (y > 0 && lab.px[i - w] != p) || (y + 1 < h && lab.px[i + w] != p);
            if edge {
                out.push([x as u32, y as u32]);
            }
        }
        if out.len() > max {
            let step = out.len().div_ceil(max);
            out = out.into_iter().step_by(step).collect();
        }
        out
    }

    /// Closest pixels of two provinces (start in `a`, stop in `b`).
    pub fn closest_points(&self, a: Id, b: Id) -> Option<([u32; 2], [u32; 2])> {
        let lab = self.labels.read();
        let pa = self.border_pixels(&lab, a, 3000);
        let pb = self.border_pixels(&lab, b, 3000);
        let mut best: Option<(u64, [u32; 2], [u32; 2])> = None;
        for &x in &pa {
            for &y in &pb {
                let d = (x[0] as i64 - y[0] as i64).pow(2) as u64 + (x[1] as i64 - y[1] as i64).pow(2) as u64;
                if best.is_none_or(|b| d < b.0) {
                    best = Some((d, x, y));
                }
            }
        }
        best.map(|b| (b.1, b.2))
    }

    // ------------------------------------------------------------ impassable borders

    /// Index of the impassable adjacency between `a` and `b` (either way round).
    pub fn impassable_border(&self, a: Id, b: Id) -> Option<usize> {
        self.data.adjacencies.iter().position(|x| x.kind == "impassable" && ((x.from, x.to) == (a, b) || (x.from, x.to) == (b, a)))
    }

    /// Make the border of two touching provinces impassable (`on`) or passable
    /// again; false if it already was.
    pub fn set_border_impassable(&mut self, a: Id, b: Id, on: bool) -> Result<bool> {
        match (self.impassable_border(a, b), on) {
            (Some(_), true) | (None, false) => Ok(false),
            (None, true) => {
                self.add_adjacency(a, b, "impassable", "", "")?;
                Ok(true)
            }
            (Some(i), false) => {
                self.data.adjacencies.remove(i);
                Ok(true)
            }
        }
    }

    /// The border nearest to `at` (pixel coordinates) within `r` pixels: the
    /// province under `at` and the touching province closest to it.
    pub fn border_near(&self, at: [f64; 2], r: f64) -> Option<(Id, Id)> {
        let lab = self.labels.read();
        let (w, h) = (lab.w as i64, lab.h as i64);
        let (cx, cy) = (at[0].floor() as i64, at[1].floor() as i64);
        if cx < 0 || cy < 0 || cx >= w || cy >= h {
            return None;
        }
        let p = lab.at(cx as u32, cy as u32);
        if p == 0 {
            return None;
        }
        let ri = r.ceil().clamp(1.0, 64.0) as i64;
        let mut best: Option<(f64, Id)> = None;
        for y in (cy - ri).max(0)..=(cy + ri).min(h - 1) {
            for x in (cx - ri).max(0)..=(cx + ri).min(w - 1) {
                let q = lab.at(x as u32, y as u32);
                if q == 0 || q == p {
                    continue;
                }
                let d = (x as f64 + 0.5 - at[0]).hypot(y as f64 + 0.5 - at[1]);
                if d <= r + 0.75 && best.is_none_or(|b| d < b.0) && self.stats.border_len(p, q) > 0 {
                    best = Some((d, q));
                }
            }
        }
        best.map(|(_, q)| (p, q))
    }

    /// The borders (pairs of touching provinces) the segment a–b crosses, in order.
    pub fn borders_crossed(&self, a: [f64; 2], b: [f64; 2]) -> Vec<(Id, Id)> {
        let lab = self.labels.read();
        let at = |p: [f64; 2]| -> Id {
            if p[0] < 0.0 || p[1] < 0.0 || p[0] >= lab.w as f64 || p[1] >= lab.h as f64 { 0 } else { lab.at(p[0] as u32, p[1] as u32) }
        };
        let n = ((b[0] - a[0]).hypot(b[1] - a[1]) / 0.4).ceil().max(1.0) as usize;
        let mut prev = at(a);
        let mut out = Vec::new();
        for i in 1..=n {
            let t = i as f64 / n as f64;
            let q = at([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]);
            if q != prev {
                if prev != 0 && q != 0 && self.stats.border_len(prev, q) > 0 {
                    out.push((prev, q));
                }
                prev = q;
            }
        }
        out
    }

    /// The shared border of two provinces as polylines along pixel edges
    /// (pixel coordinates, a pixel spans x..x+1).
    pub fn border_lines(&self, a: Id, b: Id) -> Vec<Vec<[f64; 2]>> {
        let segs = self.border_runs(a, b);
        // chain the runs at shared end points
        let key = |p: [f64; 2]| (p[0] as u32, p[1] as u32);
        let mut at: FxHashMap<(u32, u32), Vec<usize>> = FxHashMap::default();
        for (i, s) in segs.iter().enumerate() {
            at.entry(key(s[0])).or_default().push(i);
            at.entry(key(s[1])).or_default().push(i);
        }
        let mut used = vec![false; segs.len()];
        let extend = |pts: &mut Vec<[f64; 2]>, used: &mut Vec<bool>| {
            while let Some(&j) = at.get(&key(*pts.last().unwrap())).and_then(|v| v.iter().find(|&&j| !used[j])) {
                used[j] = true;
                let s = segs[j];
                pts.push(if key(s[0]) == key(*pts.last().unwrap()) { s[1] } else { s[0] });
            }
        };
        let mut out = Vec::new();
        for i in 0..segs.len() {
            if used[i] {
                continue;
            }
            used[i] = true;
            let mut fwd = vec![segs[i][0], segs[i][1]];
            extend(&mut fwd, &mut used);
            let mut back = vec![segs[i][0]];
            extend(&mut back, &mut used);
            back.reverse();
            back.pop();
            back.extend(fwd);
            out.push(back);
        }
        out
    }

    /// The shared border of two provinces as straight runs of pixel edges.
    fn border_runs(&self, a: Id, b: Id) -> Vec<[[f64; 2]; 2]> {
        let lab = self.labels.read();
        let (sa, sb) = (self.stats.get(a), self.stats.get(b));
        if sa.px == 0 || sb.px == 0 {
            return Vec::new();
        }
        // scan the smaller box
        let (p, q, s) = if sa.box_w() as u64 * sa.box_h() as u64 <= sb.box_w() as u64 * sb.box_h() as u64 { (a, b, sa) } else { (b, a, sb) };
        let (w, h) = (lab.w, lab.h);
        let mut vert: Vec<(u32, u32)> = Vec::new(); // (x of the line, row)
        let mut horz: Vec<(u32, u32)> = Vec::new(); // (y of the line, column)
        for y in s.y0..=s.y1 {
            for x in s.x0..=s.x1 {
                if lab.at(x, y) != p {
                    continue;
                }
                if x + 1 < w && lab.at(x + 1, y) == q {
                    vert.push((x + 1, y));
                }
                if x > 0 && lab.at(x - 1, y) == q {
                    vert.push((x, y));
                }
                if y + 1 < h && lab.at(x, y + 1) == q {
                    horz.push((y + 1, x));
                }
                if y > 0 && lab.at(x, y - 1) == q {
                    horz.push((y, x));
                }
            }
        }
        vert.sort_unstable();
        horz.sort_unstable();
        let mut out = Vec::new();
        for (edges, vertical) in [(&vert, true), (&horz, false)] {
            let mut i = 0;
            while i < edges.len() {
                let (line, start) = edges[i];
                let mut end = start + 1;
                i += 1;
                while i < edges.len() && edges[i] == (line, end) {
                    end += 1;
                    i += 1;
                }
                let (l, s0, s1) = (line as f64, start as f64, end as f64);
                out.push(if vertical { [[l, s0], [l, s1]] } else { [[s0, l], [s1, l]] });
            }
        }
        out
    }

    /// Add an adjacency between `from` and `to` (see model::ADJ_KINDS).
    pub fn add_adjacency(&mut self, from: Id, to: Id, kind: &str, rule: &str, name: &str) -> Result<usize> {
        if from == to {
            bail!("pick two different provinces");
        }
        let (fk, tk) = (self.data.kind(from), self.data.kind(to));
        let touching = self.stats.border_len(from, to) > 0;
        let mut through = None;
        match kind {
            "sea" => {
                if fk != Kind::Land || tk != Kind::Land {
                    bail!("a strait links two land provinces");
                }
                if touching {
                    bail!("{} and {} already touch", self.data.prov_name(from), self.data.prov_name(to));
                }
                let both: Vec<Id> = self
                    .stats
                    .neighbours(from)
                    .iter()
                    .copied()
                    .filter(|&s| self.data.kind(s) == Kind::Sea && self.stats.border_len(to, s) > 0)
                    .collect();
                let Some(t) = both.into_iter().max_by_key(|&s| self.stats.border_len(from, s) + self.stats.border_len(to, s)) else {
                    bail!("no sea province touches both {} and {}: pick provinces on opposite shores of one sea province", self.data.prov_name(from), self.data.prov_name(to));
                };
                through = Some(t);
            }
            "impassable" => {
                if !touching {
                    bail!("an impassable border needs two provinces that touch");
                }
            }
            _ => {
                if touching {
                    bail!("{} and {} already touch", self.data.prov_name(from), self.data.prov_name(to));
                }
            }
        }
        if self.data.adjacencies.iter().any(|a| a.kind == kind && ((a.from, a.to) == (from, to) || (a.from, a.to) == (to, from))) {
            bail!("this adjacency exists already");
        }
        let (start, stop) = self.closest_points(from, to).ok_or_else(|| anyhow!("provinces without pixels"))?;
        let name = if name.is_empty() { format!("{} - {}", self.data.prov_name(from), self.data.prov_name(to)) } else { name.to_string() };
        self.data.adjacencies.push(Adjacency {
            from,
            to,
            kind: kind.to_string(),
            through,
            start: Some([start[0] as i64, start[1] as i64]),
            stop: Some([stop[0] as i64, stop[1] as i64]),
            rule: rule.to_string(),
            name,
            extra: Default::default(),
        });
        Ok(self.data.adjacencies.len() - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roman_numerals() {
        assert_eq!(roman(2), "II");
        assert_eq!(roman(4), "IV");
        assert_eq!(roman(14), "XIV");
    }

    #[test]
    fn directions() {
        assert_eq!(direction([0.0, 0.0], [10.0, 0.0]), "East");
        assert_eq!(direction([0.0, 0.0], [0.0, -10.0]), "North");
        assert_eq!(direction([0.0, 0.0], [-10.0, 10.0]), "South-West");
    }

    #[test]
    fn clean_clusters_joins_stray_parts() {
        // 6×1 strip: clusters 0 0 1 0 1 1 -> the lone 0 at index 3 joins 1
        let mut l = vec![0, 0, 1, 0, 1, 1];
        clean_clusters(&mut l, 6, 1, 1);
        let changes = l.windows(2).filter(|w| w[0] != w[1]).count();
        assert_eq!(changes, 1, "{l:?}");
    }

    #[test]
    fn pixel_parts_counts() {
        // pixels 0,1 and 5 of a 4-wide raster: 5 is (1,1), below 1 -> one part
        let (_, n) = pixel_parts(&[0, 1, 5], 4);
        assert_eq!(n, 1);
        let (_, n) = pixel_parts(&[0, 2], 4);
        assert_eq!(n, 2);
    }
}
