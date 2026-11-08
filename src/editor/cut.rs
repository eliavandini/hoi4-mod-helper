//! The Cut tool (Provinces and States levels): hold the button and draw a
//! line across a province or a state, from border to border. On letting go,
//! what the line divides becomes separate provinces or states, as one undo
//! step. Esc while drawing drops the line.
//!
//! - The line is the map pixels it passes through, side by side (so nothing
//!   joins across it). An end that stops a little short of the border of what
//!   the line mostly runs through is carried on to that border.
//! - Provinces: every province the line divides is cut. Its biggest piece
//!   keeps the province; each other piece becomes a new province like it
//!   (type, state, zone, terrain). The line's own pixels go to the pieces
//!   beside them; bits under HOI4's 8 px are not cut off.
//! - States: every state the line divides is cut. Each province goes to the
//!   side most of it lies on; with "cut the provinces it crosses", those are
//!   cut first, so the new border follows the line. The biggest piece keeps
//!   the state; each other piece becomes a new state of the same nation.

use std::cmp::Reverse;
use std::collections::VecDeque;

use anyhow::bail;
use eframe::egui::{self, Color32, PointerButton, RichText, Stroke};
use rustc_hash::{FxHashMap, FxHashSet};

use super::doc::Doc;
use super::lasso::most;
use super::model::Id;
use super::ops::{MIN_PROVINCE_PX, pixel_parts};
use super::rivers_ui::to_screen;
use super::{Editor, Level};
use crate::layers::Placement;
use crate::view::DrawCtx;

const LINE: Color32 = Color32::from_rgb(255, 120, 90);
/// How far (screen points) an end that stops short of the border is carried on.
const REACH: f64 = 20.0;

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// The map pixels a line through `points` (map pixel coordinates) passes
/// through, in order, each once; consecutive ones share a side. Only those on
/// a `w` × `h` map.
pub fn line_cells(points: &[[f64; 2]], w: usize, h: usize) -> Vec<usize> {
    let mut out = Vec::new();
    let mut seen = FxHashSet::default();
    let mut push = |c: (i64, i64), out: &mut Vec<usize>| {
        if c.0 >= 0 && c.1 >= 0 && (c.0 as usize) < w && (c.1 as usize) < h {
            let i = c.1 as usize * w + c.0 as usize;
            if seen.insert(i) {
                out.push(i);
            }
        }
    };
    let cell = |p: [f64; 2]| (p[0].floor() as i64, p[1].floor() as i64);
    let mut last: Option<([f64; 2], (i64, i64))> = None;
    let mut visit = |q: [f64; 2], out: &mut Vec<usize>| {
        let c = cell(q);
        if let Some((lq, l)) = last {
            if l == c {
                return;
            }
            if l.0 != c.0 && l.1 != c.1 {
                // a corner step: also the cell the line passes through between
                let m = cell([(lq[0] + q[0]) / 2.0, (lq[1] + q[1]) / 2.0]);
                let via = if m == (l.0, c.1) { m } else { (c.0, l.1) };
                push(via, out);
            }
        }
        push(c, out);
        last = Some((q, c));
    };
    for k in 0..points.len() {
        let a = points[k];
        let Some(&b) = points.get(k + 1) else {
            visit(a, &mut out);
            break;
        };
        // steps under a pixel: the cell changes by at most one each way
        let n = ((dist(a, b) / 0.25).ceil() as usize).max(1);
        for j in 0..n {
            let t = j as f64 / n as f64;
            visit([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t], &mut out);
        }
    }
    out
}

/// The line with its ends carried on, along the way they point, out of
/// `main` (what `owner` says a point lies in) when that takes at most
/// `reach` px; ends in something else stay.
pub fn extend_ends(points: &[[f64; 2]], reach: f64, owner: impl Fn([f64; 2]) -> Id, main: Id) -> Vec<[f64; 2]> {
    let mut pts = points.to_vec();
    if pts.len() < 2 || main == 0 {
        return pts;
    }
    let out_of_main = |pts: &[[f64; 2]]| -> Option<[f64; 2]> {
        let e = *pts.last()?;
        if owner(e) != main {
            return None;
        }
        // the way the line points: from a point at least 2 px back
        let b = pts.iter().rev().find(|&&p| dist(p, e) >= 2.0).or(pts.first()).copied()?;
        let d = dist(b, e);
        if d < 1e-9 {
            return None;
        }
        let dir = [(e[0] - b[0]) / d, (e[1] - b[1]) / d];
        let mut s = 0.5;
        while s <= reach {
            let q = [e[0] + dir[0] * s, e[1] + dir[1] * s];
            if owner(q) != main {
                return Some(q);
            }
            s += 0.5;
        }
        None
    };
    if let Some(q) = out_of_main(&pts) {
        pts.push(q);
    }
    pts.reverse();
    if let Some(q) = out_of_main(&pts) {
        pts.push(q);
    }
    pts.reverse();
    pts
}

/// How a line divides an area (`px`: its map pixels, `w`: the map's width): a
/// label per pixel, 0 for what stays (the parts the line does not cut, and the
/// biggest piece of each part it does), 1.. for the pieces cut off; and how
/// many pieces were cut off (0: it divides nothing). Pieces under `min` px are
/// not cut off; the line's own pixels go to the pieces beside them.
pub fn divide(px: &[usize], on_line: impl Fn(usize) -> bool, w: usize, min: usize) -> (Vec<u32>, u32) {
    const OPEN: u32 = u32::MAX;
    let (before, nb) = pixel_parts(px, w);
    let rest: Vec<usize> = (0..px.len()).filter(|&k| !on_line(px[k])).collect();
    let rest_px: Vec<usize> = rest.iter().map(|&k| px[k]).collect();
    let (after, na) = pixel_parts(&rest_px, w);
    // each part left: its size and the part it was a piece of
    let mut size = vec![0usize; na];
    let mut part_of = vec![0u32; na];
    for (j, &a) in after.iter().enumerate() {
        size[a as usize] += 1;
        part_of[a as usize] = before[rest[j]];
    }
    let mut pieces: Vec<Vec<usize>> = vec![Vec::new(); nb];
    for a in 0..na {
        if size[a] >= min {
            pieces[part_of[a] as usize].push(a);
        }
    }
    let mut label = vec![OPEN; na];
    let mut cut = vec![false; nb];
    let mut next = 1u32;
    for (b, ps) in pieces.iter_mut().enumerate() {
        if ps.len() < 2 {
            continue;
        }
        cut[b] = true;
        ps.sort_by_key(|&a| (Reverse(size[a]), a));
        label[ps[0]] = 0;
        for &a in &ps[1..] {
            label[a] = next;
            next += 1;
        }
    }
    let mut lbl: Vec<u32> = before.iter().map(|&b| if cut[b as usize] { OPEN } else { 0 }).collect();
    if next == 1 {
        return (lbl, 0);
    }
    for (j, &a) in after.iter().enumerate() {
        if cut[part_of[a as usize] as usize] {
            lbl[rest[j]] = label[a as usize];
        }
    }
    // the line and the bits too small to cut off: to the nearest piece
    let pos: FxHashMap<usize, usize> = px.iter().enumerate().map(|(k, &i)| (i, k)).collect();
    let mut queue: VecDeque<usize> = (0..px.len()).filter(|&k| lbl[k] != OPEN).collect();
    while let Some(k) = queue.pop_front() {
        let i = px[k];
        let x = i % w;
        let nbs = [(x > 0).then(|| i - 1), (x + 1 < w).then(|| i + 1), i.checked_sub(w), Some(i + w)];
        for j in nbs.into_iter().flatten() {
            if let Some(&kk) = pos.get(&j)
                && lbl[kk] == OPEN
            {
                lbl[kk] = lbl[k];
                queue.push_back(kk);
            }
        }
    }
    for l in &mut lbl {
        if *l == OPEN {
            *l = 0;
        }
    }
    (lbl, next - 1)
}

/// What a cut did: each province or state cut, with the new ones cut off it.
pub type Cuts = Vec<(Id, Vec<Id>)>;

impl Doc {
    /// Cut the provinces the line (its map pixels, see [`line_cells`])
    /// divides; `only`: just these. Crossings along the cuts are fixed.
    pub fn cut_provinces(&mut self, line: &[usize], only: Option<&FxHashSet<Id>>) -> Cuts {
        let w = self.w as usize;
        let on: FxHashSet<usize> = line.iter().copied().collect();
        let mut hit: Vec<Id> = {
            let lab = self.labels.read();
            line.iter().map(|&i| lab.px[i]).filter(|&p| p != 0 && self.data.provinces.contains(p) && only.is_none_or(|o| o.contains(&p))).collect()
        };
        hit.sort_unstable();
        hit.dedup();
        let mut out = Vec::new();
        let mut bbox = [usize::MAX, usize::MAX, 0, 0];
        for p in hit {
            let px = {
                let lab = self.labels.read();
                self.pixels_of(&lab, p)
            };
            let (lbl, n) = divide(&px, |i| on.contains(&i), w, MIN_PROVINCE_PX as usize);
            if n == 0 {
                continue;
            }
            let pr = self.data.provinces.get(p).unwrap();
            let (kind, state, region, name) = (pr.kind, pr.state, pr.region, pr.name.clone());
            let ids: Vec<Id> = (0..n).map(|_| self.new_province(kind, state, region, name.clone(), Some(p))).collect();
            self.with_pixels(|wr, _| {
                for (k, &i) in px.iter().enumerate() {
                    if lbl[k] > 0 {
                        wr.set(i, ids[lbl[k] as usize - 1]);
                        let (x, y) = (i % w, i / w);
                        bbox = [bbox[0].min(x), bbox[1].min(y), bbox[2].max(x), bbox[3].max(y)];
                    }
                }
            });
            out.push((p, ids));
        }
        if !out.is_empty() {
            self.fix_crossings(Some(bbox));
        }
        out
    }

    /// Cut the states the line divides: each of their provinces goes to the
    /// side most of it lies on; with `cut_provinces`, the provinces of those
    /// states that the line divides are cut first. Returns the states cut and
    /// how many provinces were cut off.
    pub fn cut_states(&mut self, line: &[usize], cut_provinces: bool) -> (Cuts, usize) {
        let w = self.w as usize;
        let on: FxHashSet<usize> = line.iter().copied().collect();
        let mut states: Vec<Id> = {
            let lab = self.labels.read();
            line.iter().map(|&i| self.data.state_of(lab.px[i])).filter(|&s| s != 0).collect()
        };
        states.sort_unstable();
        states.dedup();
        // where each lies (the boxes of its provinces, as they are before any cut)
        let boxes: Vec<[usize; 4]> = states
            .iter()
            .map(|&s| {
                self.index.provs_of_state(s).iter().map(|&p| self.stats.get(p)).filter(|st| st.px > 0).fold([usize::MAX, usize::MAX, 0, 0], |b, st| {
                    [b[0].min(st.x0 as usize), b[1].min(st.y0 as usize), b[2].max(st.x1 as usize), b[3].max(st.y1 as usize)]
                })
            })
            .collect();
        let mut provinces_cut = 0;
        if cut_provinces {
            let only: FxHashSet<Id> = states.iter().flat_map(|&s| self.index.provs_of_state(s).to_vec()).collect();
            provinces_cut = self.cut_provinces(line, Some(&only)).iter().map(|c| c.1.len()).sum();
        }
        let state_of: Vec<Id> = (0..self.data.provinces.cap()).map(|p| self.data.state_of(p as Id)).collect();
        let mut out = Vec::new();
        for (&s, &[x0, y0, x1, y1]) in states.iter().zip(&boxes) {
            if x0 > x1 {
                continue;
            }
            let (px, prov): (Vec<usize>, Vec<Id>) = {
                let lab = self.labels.read();
                (y0..=y1).flat_map(|y| (x0..=x1).map(move |x| y * w + x)).map(|i| (i, lab.px[i])).filter(|&(_, p)| state_of.get(p as usize) == Some(&s)).unzip()
            };
            let (lbl, n) = divide(&px, |i| on.contains(&i), w, MIN_PROVINCE_PX as usize);
            if n == 0 {
                continue;
            }
            // each province: to the piece most of it lies in
            let mut counts: FxHashMap<Id, FxHashMap<u32, u64>> = FxHashMap::default();
            for (k, &p) in prov.iter().enumerate() {
                *counts.entry(p).or_default().entry(lbl[k]).or_insert(0) += 1;
            }
            let mut provs: Vec<Id> = counts.keys().copied().collect();
            provs.sort_unstable();
            let groups: Vec<u32> = provs.iter().map(|p| most(counts[p].iter().map(|(&l, &n)| (l, n))).unwrap_or(0)).collect();
            if groups.iter().all(|&g| g == groups[0]) {
                continue;
            }
            let sizes: Vec<u64> = provs.iter().map(|p| counts[p].values().sum()).collect();
            let ids = self.apply_state_groups(s, &provs, &groups, &sizes);
            out.push((s, ids[1..].to_vec()));
        }
        (out, provinces_cut)
    }
}

impl Editor {
    /// Cut tool on the map (see the module comment). `pos`, `origin`: the
    /// pointer and where the button went down, in map pixels; `px_scale`:
    /// screen points per map pixel.
    pub(super) fn cut_input(&mut self, ctx: &egui::Context, response: &egui::Response, pos: Option<[f64; 2]>, origin: Option<[f64; 2]>, px_scale: f64) {
        let pt = 1.0 / px_scale.max(1e-9); // a screen point in map pixels
        let primary = PointerButton::Primary;
        if response.drag_started_by(primary)
            && let Some(o) = origin
        {
            self.cut = Some(vec![o]);
        }
        if let (Some(c), Some(p)) = (self.cut.as_mut(), pos)
            && response.dragged_by(primary)
            && c.last().is_none_or(|&l| dist(l, p) >= 2.0 * pt)
        {
            c.push(p);
        }
        if self.cut.is_some() && (response.drag_stopped_by(primary) || !ctx.input(|i| i.pointer.primary_down())) {
            let mut c = self.cut.take().unwrap();
            if let Some(p) = pos.filter(|p| c.last() != Some(p)) {
                c.push(p);
            }
            self.finish_cut(&c, REACH * pt);
        } else if response.clicked_by(primary) {
            self.info(format!("hold the button and draw a line across a {}, from border to border", self.settings.level.noun()));
        }
    }

    /// Cut along a line (map pixel coordinates) on the Provinces or States
    /// level; ends up to `reach` px short of the border are carried on to it.
    pub(super) fn finish_cut(&mut self, pts: &[[f64; 2]], reach: f64) {
        let Some(doc) = &self.doc else { return };
        let level = self.settings.level;
        let states = level == Level::States;
        let (w, h) = (doc.w as usize, doc.h as usize);
        let line = {
            let lab = doc.labels.read();
            let of = |p: Id| if states { doc.data.state_of(p) } else { p };
            let owner = |q: [f64; 2]| if q[0] >= 0.0 && q[1] >= 0.0 && q[0] < w as f64 && q[1] < h as f64 { of(lab.at(q[0] as u32, q[1] as u32)) } else { 0 };
            let main = most(line_cells(pts, w, h).into_iter().map(|i| (of(lab.px[i]), 1u64)).filter(|&(x, _)| x != 0)).unwrap_or(0);
            line_cells(&extend_ends(pts, reach, owner, main), w, h)
        };
        let noun = level.noun();
        if line.len() < 2 {
            self.info(format!("cut: draw a line across a {noun}"));
            return;
        }
        let nothing = format!("the line divides no {noun}: draw it across one, from border to border");
        let cut_provinces = self.settings.cut_provinces;
        let mut made: Vec<Id> = Vec::new();
        match level {
            Level::Provinces => self.run("Cut provinces", |d, _| {
                let cuts = d.cut_provinces(&line, None);
                if cuts.is_empty() {
                    bail!(nothing);
                }
                made = cuts.iter().flat_map(|(p, n)| std::iter::once(*p).chain(n.iter().copied())).collect();
                Ok(match cuts.as_slice() {
                    [(p, n)] => format!("{} cut in {}", d.data.prov_name(*p), n.len() + 1),
                    _ => format!("{} provinces cut: {} new", cuts.len(), made.len() - cuts.len()),
                })
            }),
            Level::States => self.run("Cut states", |d, _| {
                let (cuts, provs) = d.cut_states(&line, cut_provinces);
                if cuts.is_empty() {
                    bail!(if cut_provinces { nothing } else { format!("{nothing} (or tick \"cut the provinces it crosses\": now each goes whole to one side)") });
                }
                made = cuts.iter().flat_map(|(s, n)| std::iter::once(*s).chain(n.iter().copied())).collect();
                let also = if provs > 0 { format!(", {provs} provinces cut off along it") } else { String::new() };
                Ok(match cuts.as_slice() {
                    [(s, n)] => format!("{} cut in {}{also}", d.data.state_name(*s), n.len() + 1),
                    _ => format!("{} states cut: {} new{also}", cuts.len(), made.len() - cuts.len()),
                })
            }),
            _ => {}
        }
        // (a clean-up may have removed what was made again)
        let Some(doc) = &self.doc else { return };
        made.retain(|&e| if states { doc.data.states.contains(e) } else { doc.data.provinces.contains(e) });
        if !made.is_empty() {
            self.sel = made;
        }
    }

    /// The line being drawn.
    pub(super) fn draw_cut(&self, dc: &DrawCtx, pl: &Placement) {
        let Some(c) = &self.cut else { return };
        let s: Vec<egui::Pos2> = c.iter().filter_map(|&p| to_screen(dc, pl, p)).collect();
        if s.len() >= 2 {
            dc.painter.add(egui::Shape::line(s.clone(), Stroke::new(3.5, Color32::from_black_alpha(150))));
            dc.painter.add(egui::Shape::line(s, Stroke::new(1.5, LINE)));
        }
    }

    /// Cut settings in the panel (below the tools).
    pub(super) fn cut_panel(&mut self, ui: &mut egui::Ui) {
        let what = if self.settings.level == Level::States {
            ui.checkbox(&mut self.settings.cut_provinces, "cut the provinces it crosses").on_hover_text("so the new border follows the line; else each province goes whole to the side most of it lies on");
            "The biggest side keeps the state; each other side becomes a new state of the same nation (victory points go with their provinces)."
        } else {
            "The biggest piece keeps the province; each other piece becomes a new province like it (same type, state and zone)."
        };
        let how = "Hold the button and draw a line across, from border to border; letting go cuts. Esc while drawing drops the line.";
        ui.label(RichText::new(format!("{how} {what}")).small().weak());
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::super::model::{Kind, Labels, Loaded, MapData, Region, State};
    use super::super::{Settings, Tool};
    use super::*;

    fn from<T: serde::de::DeserializeOwned>(v: Value) -> T {
        serde_json::from_value(v).unwrap()
    }

    /// 20×10 px: land 1–4 in columns of 5 px, all in state 1 (manpower
    /// 1000, a victory point in 4); on rows 8 and 9 land 5 (state 2) below
    /// 1, 2 and the left of 3, sea 6 below the rest (no four provinces meet
    /// at a point); one air zone.
    fn map() -> Doc {
        let mut data = MapData { pixel_m: 100.0, ..Default::default() };
        for id in 1..=6 {
            let kind = if id == 6 { "sea" } else { "land" };
            data.provinces.put(id, from(json!({"id": id, "rgb": [10 * id, 0, 0], "type": kind, "name": format!("p{id}")})));
        }
        data.states.put(1, from::<State>(json!({"id": 1, "name": "A", "manpower": 1000, "victory_points": [[4, 5]]})));
        data.states.put(2, from::<State>(json!({"id": 2, "name": "B"})));
        data.regions.put(1, from::<Region>(json!({"id": 1, "name": "air"})));
        for p in 1..=6 {
            let pr = data.provinces.get_mut(p).unwrap();
            (pr.state, pr.region) = (if p == 5 { 2 } else if p == 6 { 0 } else { 1 }, 1);
        }
        let px = (0..200).map(|i: usize| if i / 20 >= 8 { if i % 20 < 12 { 5 } else { 6 } } else { 1 + (i % 20 / 5) as Id }).collect();
        let geo = crate::geo::FileGeoInfo { transform: None, crs: None, nodata: None };
        Doc::new(Default::default(), Loaded { data, labels: Labels { w: 20, h: 10, px }, geo, extra: Default::default() })
    }

    fn count(doc: &Doc, p: Id) -> usize {
        doc.labels.read().px.iter().filter(|&&q| q == p).count()
    }

    fn parts(doc: &Doc, p: Id) -> usize {
        let px: Vec<usize> = doc.labels.read().px.iter().enumerate().filter(|&(_, &q)| q == p).map(|(i, _)| i).collect();
        pixel_parts(&px, doc.w as usize).1
    }

    fn step<R>(doc: &mut Doc, f: impl FnOnce(&mut Doc) -> R) -> R {
        doc.begin("step");
        let r = f(doc);
        doc.commit();
        r
    }

    #[test]
    fn lines_are_pixels_side_by_side() {
        // across: one row
        assert_eq!(line_cells(&[[0.5, 1.5], [3.5, 1.5]], 10, 10), [10, 11, 12, 13]);
        // a diagonal: every step to the next pixel shares a side with it
        let d = line_cells(&[[0.2, 0.7], [5.8, 5.3]], 10, 10);
        assert!(d.windows(2).all(|p| {
            let (a, b) = (p[0] as i64, p[1] as i64);
            (a - b).abs() == 1 || (a - b).abs() == 10
        }), "{d:?}");
        assert_eq!((d[0], d[d.len() - 1]), (0, 55));
        // off the map: only what is on it; past its start: each pixel once
        assert_eq!(line_cells(&[[-3.5, 0.5], [1.5, 0.5]], 10, 10), [0, 1]);
        assert_eq!(line_cells(&[[0.5, 0.5], [2.5, 0.5], [0.5, 0.5]], 10, 10), [0, 1, 2]);
    }

    #[test]
    fn what_a_line_divides() {
        // a 10×10 square, cut down column 4: the line goes to its sides
        let sq: Vec<usize> = (0..100).collect();
        let (lbl, n) = divide(&sq, |i| i % 10 == 4, 10, 8);
        assert_eq!(n, 1);
        assert!((0..100).all(|i| lbl[i] == if i % 10 < 4 { 1 } else if i % 10 > 4 { 0 } else { lbl[i] }));
        // a line that stops inside divides nothing; nor one that cuts a bit
        // under 8 px off a corner
        assert_eq!(divide(&sq, |i| i % 10 == 4 && i / 10 < 9, 10, 8).1, 0);
        assert_eq!(divide(&sq, |i| i == 2 || i == 12 || i == 21 || i == 20, 10, 8).1, 0);
        // a zigzag cuts three pieces; of two separate parts only the one crossed
        assert_eq!(divide(&sq, |i| i % 10 == 2 || i % 10 == 6, 10, 8).1, 2);
        let two: Vec<usize> = (0..100).filter(|i| i % 10 != 5).collect();
        let (lbl, n) = divide(&two, |i| i / 10 == 4 && i % 10 < 5, 10, 8);
        assert_eq!(n, 1);
        assert!(two.iter().zip(&lbl).all(|(&i, &l)| i % 10 < 5 || l == 0), "the part not crossed stays");
    }

    #[test]
    fn ends_short_of_the_border_reach_it() {
        let owner = |q: [f64; 2]| if q[0] >= 2.0 && q[0] < 8.0 { 1 } else { 2 };
        let line = [[3.0, 5.0], [6.0, 5.0]];
        let ext = extend_ends(&line, 3.0, owner, 1);
        assert_eq!(ext.len(), 4);
        assert!(ext[0][0] < 2.0 && ext[3][0] >= 8.0);
        assert_eq!(extend_ends(&line, 1.0, owner, 1).len(), 2, "too far");
        assert_eq!(extend_ends(&line, 3.0, owner, 2).len(), 2, "not in what it cuts");
    }

    #[test]
    fn provinces_are_cut_in_pieces() {
        let mut doc = map();
        // across 1 on row 2, from the map's edge into 2: rows 0–1 (and the
        // line) are cut off, 1 keeps rows 3–7
        let line = line_cells(&[[-1.0, 2.5], [6.5, 2.5]], 20, 10);
        let cuts = step(&mut doc, |d| d.cut_provinces(&line, None));
        assert_eq!(cuts.len(), 1);
        let (p, ref n) = cuts[0];
        assert_eq!((p, n.len()), (1, 1));
        let q = n[0];
        let pr = doc.data.provinces.get(q).unwrap();
        assert!(pr.kind == Kind::Land && pr.state == 1 && pr.region == 1 && pr.name == "p1");
        assert_eq!(count(&doc, 1) + count(&doc, q), 40);
        assert!(count(&doc, 1) >= 25 && count(&doc, q) >= 10);
        assert_eq!((parts(&doc, 1), parts(&doc, q)), (1, 1));
        assert_eq!(count(&doc, 2), 40, "2 is only touched");
        assert_eq!(doc.can_undo(), Some("step"));
        doc.undo();
        assert_eq!(count(&doc, 1), 40);
        // a line that stops inside cuts nothing
        let short = line_cells(&[[-1.0, 3.5], [3.5, 3.5]], 20, 10);
        assert!(step(&mut doc, |d| d.cut_provinces(&short, None)).is_empty());
    }

    #[test]
    fn states_are_cut_along_provinces_or_the_line() {
        let mut doc = map();
        // down column 10 (in 3): 1 and 2 stay, 3 and 4 make a new state
        let line = line_cells(&[[10.5, -1.0], [10.5, 11.0]], 20, 10);
        let (cuts, provs) = step(&mut doc, |d| d.cut_states(&line, false));
        assert_eq!((cuts.len(), provs), (1, 0));
        let (s, ref n) = cuts[0];
        let t = n[0];
        assert_eq!((s, n.len()), (1, 1));
        let d = &doc.data;
        assert_eq!([1, 2, 3, 4, 5].map(|p| d.state_of(p)), [1, 1, t, t, 2]);
        let (a, b) = (d.states.get(1).unwrap(), d.states.get(t).unwrap());
        assert_eq!((a.manpower, b.manpower), (Some(500), Some(500)));
        assert!(a.victory_points.is_empty() && b.victory_points == [[4, 5]]);
        assert_eq!(b.name, "A II");
        assert_eq!(count(&doc, 3), 40, "no province was cut");
        doc.undo();

        // down column 12 with the provinces cut: 3 is cut, its right goes with 4
        let line = line_cells(&[[12.5, -1.0], [12.5, 11.0]], 20, 10);
        let (cuts, provs) = step(&mut doc, |d| d.cut_states(&line, true));
        assert_eq!((cuts.len(), provs), (1, 1));
        let t = cuts[0].1[0];
        let d = &doc.data;
        let right: Vec<Id> = d.provinces.iter().filter(|(_, p)| p.state == t).map(|(id, _)| id).collect();
        assert!(right.contains(&4) && right.len() == 2 && !right.contains(&3), "{right:?}");
        let px = doc.labels.read();
        assert!((0..160).all(|i| (i % 20 < 12) == (d.state_of(px.px[i]) == 1) || i % 20 == 12), "the new border runs down the line");
    }

    /// The tool driven by pointer events through egui, at 10 screen points
    /// per map pixel.
    fn frame(ctx: &egui::Context, time: &mut f64, ed: &mut Editor, events: Vec<egui::Event>) {
        *time += 1.0 / 60.0;
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0));
        let input = egui::RawInput { events, time: Some(*time), screen_rect: Some(screen), ..Default::default() };
        let _ = ctx.run_ui(input, |ui| {
            let r = ui.allocate_rect(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(200.0, 100.0)), egui::Sense::click_and_drag());
            let to_px = |p: egui::Pos2| Some([p.x as f64 / 10.0, p.y as f64 / 10.0]);
            let pos = r.interact_pointer_pos().or(r.hover_pos()).and_then(to_px);
            let origin = ui.input(|i| i.pointer.press_origin()).and_then(to_px).or(pos);
            ed.cut_input(ui.ctx(), &r, pos, origin, 10.0);
        });
    }

    #[test]
    fn drawing_a_line_cuts() {
        let mut ed = Editor::new(Settings { tool: Tool::Cut, ..Settings::default() });
        ed.doc = Some(map());
        let (ctx, mut time) = (egui::Context::default(), 0.0);
        let button = |pos, pressed| vec![egui::Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE }];
        // across 2 on row 4, from 1.5 px inside its left edge (carried on to
        // it: 20 points reach 2 px here) into 3
        let path = [(65.0, 45.0), (80.0, 45.0), (95.0, 45.0), (115.0, 45.0)];
        let p = |(x, y): (f64, f64)| egui::pos2(x as f32, y as f32);
        frame(&ctx, &mut time, &mut ed, vec![egui::Event::PointerMoved(p(path[0]))]);
        frame(&ctx, &mut time, &mut ed, button(p(path[0]), true));
        for &q in &path[1..] {
            frame(&ctx, &mut time, &mut ed, vec![egui::Event::PointerMoved(p(q))]);
        }
        assert!(ed.cut.as_ref().is_some_and(|c| c.len() >= 3), "drawing");
        frame(&ctx, &mut time, &mut ed, button(p(path[3]), false));
        assert!(ed.cut.is_none());
        let doc = ed.doc.as_ref().unwrap();
        assert_eq!(doc.can_undo(), Some("Cut provinces"), "{:?}", ed.message());
        assert_eq!(ed.sel.len(), 2);
        assert!(ed.sel.contains(&2) && count(doc, 2) + count(doc, ed.sel[1]) == 40);
        assert!(ed.message().is_some_and(|(m, err)| !err && m.contains("p2 cut in 2")));
    }
}
