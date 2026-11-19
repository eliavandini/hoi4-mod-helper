//! The lasso (Provinces and States levels): draw a loop around an area.
//! While the button is held it draws freehand; once let go, each click adds
//! a corner (straight lines), and holding the button again goes on freehand.
//! A click on the start, a double-click or Enter closes the loop; Backspace
//! takes back the last part, Escape drops the loop.
//!
//! - Provinces: the area becomes a new province, made as "From image…" makes
//!   one (the type most of it lies on, the state and zone of most of it,
//!   crossings fixed), or it goes to the selected province as the paint
//!   brush would paint it. The brush's "only over" limits apply to both.
//! - States: the land provinces more than half inside become a new state, or
//!   join the selected one.
//!
//! One undo step each. Alt+click picks the province or state to draw onto.

use anyhow::{Result, bail};
use eframe::egui::{self, Color32, PointerButton, RichText, Stroke};
use rustc_hash::FxHashMap;

use super::doc::Doc;
use super::from_image::Coverage;
use super::model::{Id, Kind};
use super::ops::{Constraint, GenParams, MIN_PROVINCE_PX, shape_takes};
use super::rivers_ui::to_screen;
use super::{Editor, Level};
use crate::layers::Placement;
use crate::view::DrawCtx;

const LINE: Color32 = Color32::from_rgb(255, 230, 120);
/// How close (screen points) a click must be to the start to close the loop.
const CLOSE: f64 = 8.0;

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// A loop being drawn (map pixel coordinates): freehand strokes while the
/// button is held, a corner per click.
#[derive(Clone, Debug, Default)]
pub struct Loop {
    pub points: Vec<[f64; 2]>,
    /// Where each part (a stroke or a clicked corner) starts in `points`.
    parts: Vec<usize>,
    /// A stroke is being drawn (the button is held).
    pub freehand: bool,
}

impl Loop {
    /// A stroke from `at`: the start of the loop, or a straight line from
    /// where it got to and on from there.
    pub fn press(&mut self, at: [f64; 2]) {
        self.parts.push(self.points.len());
        self.points.push(at);
        self.freehand = true;
    }

    /// The stroke goes on to `p` (a point every `step`).
    pub fn drag(&mut self, p: [f64; 2], step: f64) {
        if self.freehand && self.points.last().is_none_or(|&l| dist(l, p) >= step) {
            self.points.push(p);
        }
    }

    /// The button is let go at `at`: from now on clicks add corners.
    pub fn release(&mut self, at: Option<[f64; 2]>) {
        if let Some(p) = at.filter(|&p| self.freehand && self.points.last() != Some(&p)) {
            self.points.push(p);
        }
        self.freehand = false;
    }

    /// Whether it has enough points to close.
    pub fn can_close(&self) -> bool {
        self.points.len() >= 3
    }

    /// Whether `p` is on the start (within `close`), where a click closes it.
    pub fn at_start(&self, p: [f64; 2], close: f64) -> bool {
        self.can_close() && dist(p, self.points[0]) <= close
    }

    /// A click at `p`: true when it closes the loop (on its start), else it
    /// adds a corner.
    pub fn click(&mut self, p: [f64; 2], close: f64) -> bool {
        if self.at_start(p, close) {
            return true;
        }
        self.parts.push(self.points.len());
        self.points.push(p);
        false
    }

    /// Take back the last part; false when nothing is left.
    pub fn back(&mut self) -> bool {
        if let Some(k) = self.parts.pop() {
            self.points.truncate(k);
        }
        self.freehand = false;
        !self.points.is_empty()
    }
}

/// The map pixels inside a loop (by their centres; where it winds around
/// itself, inside too), within its bounding box.
#[derive(Clone, Debug)]
pub struct Shape {
    map_w: usize,
    /// [x0, y0, x1, y1], inclusive.
    pub bbox: [usize; 4],
    inside: Vec<bool>,
}

impl Shape {
    /// The loop through `points` (map pixel coordinates; it closes from the
    /// last back to the first) on a `map_w` × `map_h` map. None when it holds
    /// no pixel centre of the map.
    pub fn of(points: &[[f64; 2]], map_w: usize, map_h: usize) -> Option<Shape> {
        if points.len() < 3 || map_w == 0 || map_h == 0 {
            return None;
        }
        let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
        for p in points {
            for k in 0..2 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        // pixels whose centre (x + 0.5) lies within the box
        let x0 = ((lo[0] - 0.5).ceil() as i64).max(0);
        let y0 = ((lo[1] - 0.5).ceil() as i64).max(0);
        let x1 = ((hi[0] - 0.5).floor() as i64).min(map_w as i64 - 1);
        let y1 = ((hi[1] - 0.5).floor() as i64).min(map_h as i64 - 1);
        if x0 > x1 || y0 > y1 {
            return None;
        }
        let bw = (x1 - x0 + 1) as usize;
        let mut inside = vec![false; bw * (y1 - y0 + 1) as usize];
        // each row: where the loop crosses the line through the pixel centres,
        // and whether it goes down (+1) or up (-1) there; inside where the
        // crossings so far do not cancel out (non-zero winding)
        let mut xs: Vec<(f64, i32)> = Vec::new();
        for y in y0..=y1 {
            let yc = y as f64 + 0.5;
            xs.clear();
            for k in 0..points.len() {
                let (a, b) = (points[k], points[(k + 1) % points.len()]);
                if (a[1] <= yc) != (b[1] <= yc) {
                    let x = a[0] + (yc - a[1]) * (b[0] - a[0]) / (b[1] - a[1]);
                    xs.push((x, if b[1] > a[1] { 1 } else { -1 }));
                }
            }
            xs.sort_by(|a, b| a.0.total_cmp(&b.0));
            let row = (y - y0) as usize * bw;
            let mut wind = 0;
            for k in 0..xs.len() {
                wind += xs[k].1;
                if wind == 0 || k + 1 == xs.len() {
                    continue;
                }
                // centres in [xs[k], xs[k + 1])
                let from = ((xs[k].0 - 0.5).ceil() as i64).max(x0);
                let to = ((xs[k + 1].0 - 0.5).ceil() as i64 - 1).min(x1);
                for x in from..=to {
                    inside[row + (x - x0) as usize] = true;
                }
            }
        }
        inside.iter().any(|&b| b).then_some(Shape { map_w, bbox: [x0 as usize, y0 as usize, x1 as usize, y1 as usize], inside })
    }

    /// Whether map pixel `i` is inside.
    pub fn contains(&self, i: usize) -> bool {
        let (x, y) = (i % self.map_w, i / self.map_w);
        let [x0, y0, x1, y1] = self.bbox;
        (x0..=x1).contains(&x) && (y0..=y1).contains(&y) && self.inside[(y - y0) * (x1 - x0 + 1) + x - x0]
    }

    /// The map pixels inside, row by row.
    pub fn pixels(&self) -> impl Iterator<Item = usize> + '_ {
        let [x0, y0, x1, _] = self.bbox;
        let bw = x1 - x0 + 1;
        self.inside.iter().enumerate().filter(|(_, b)| **b).map(move |(k, _)| (y0 + k / bw) * self.map_w + x0 + k % bw)
    }

    /// Only the pixels `keep` keeps.
    fn filtered(&self, keep: impl Fn(usize) -> bool) -> Shape {
        let [x0, y0, x1, _] = self.bbox;
        let bw = x1 - x0 + 1;
        let inside = self.inside.iter().enumerate().map(|(k, &b)| b && keep((y0 + k / bw) * self.map_w + x0 + k % bw)).collect();
        Shape { inside, ..self.clone() }
    }
}

pub(super) fn most<K: Copy + Ord>(counts: impl Iterator<Item = (K, u64)>) -> Option<K> {
    let mut m = std::collections::BTreeMap::new();
    for (k, n) in counts {
        *m.entry(k).or_insert(0) += n;
    }
    // the most, ties to the lower
    m.into_iter().max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0))).map(|(k, _)| k)
}

impl Doc {
    /// Pixels of each province inside a shape.
    pub fn pixels_in(&self, s: &Shape) -> FxHashMap<Id, u64> {
        let lab = self.labels.read();
        let mut m = FxHashMap::default();
        for i in s.pixels() {
            let p = lab.px[i];
            if p != 0 {
                *m.entry(p).or_insert(0) += 1;
            }
        }
        m
    }

    /// Land provinces more than half inside a shape.
    pub fn land_mostly_in(&self, s: &Shape) -> Vec<Id> {
        let mut v: Vec<Id> = self.pixels_in(s).into_iter().filter(|&(p, n)| self.data.kind(p) == Kind::Land && 2 * n > self.stats.get(p).px as u64).map(|(p, _)| p).collect();
        v.sort_unstable();
        v
    }

    /// A new province of the area, of the type most of it lies on (see
    /// [`Doc::province_from_shape`]). `c`: only over that type, and the state
    /// and nation most of it lies in. Named after the province it takes most
    /// of. Returns the new provinces and the pixels they took.
    pub fn lasso_new_province(&mut self, s: &Shape, c: Constraint, coarsen: Option<&GenParams>) -> Result<(Vec<Id>, usize)> {
        let counts = self.pixels_in(s);
        let mut cov = Coverage::default();
        for (&p, &n) in &counts {
            match self.data.kind(p) {
                Kind::Land => cov.land += n,
                Kind::Sea => cov.sea += n,
                Kind::Lake => cov.lake += n,
            }
        }
        let kind = cov.main_kind();
        let d = &self.data;
        let of_kind = |p: Id| !c.same_kind || shape_takes(kind, d.kind(p));
        let taken = || counts.iter().filter(|(p, _)| of_kind(**p)).map(|(&p, &n)| (p, n));
        let state = if c.same_state { most(taken().map(|(p, n)| (d.state_of(p), n))) } else { None };
        let nation = if c.same_nation { most(taken().map(|(p, n)| (d.nation_of(p), n))) } else { None };
        let ok = |p: Id| state.is_none_or(|s| d.state_of(p) == s) && nation.is_none_or(|x| d.nation_of(p) == x);
        let Some(like) = most(taken().filter(|&(p, _)| ok(p))) else { bail!("the loop holds no province it may take") };
        let shape = {
            let lab = self.labels.read();
            s.filtered(|i| ok(lab.px[i]))
        };
        let name = d.prov_name(like);
        self.province_from_shape(|i| shape.contains(i), kind, c.same_kind, name, coarsen)
    }

    /// The area goes to province `target`, as the paint brush would paint it
    /// with the limits `c`. A province it leaves under 8 px goes entirely;
    /// crossings along its edge are fixed. Returns the pixels taken.
    pub fn lasso_onto_province(&mut self, s: &Shape, target: Id, c: Constraint) -> Result<usize> {
        let Some(t) = self.data.provinces.get(target) else { bail!("no such province") };
        let (kind, state, nation) = (t.kind, t.state, self.data.nation_of(target));
        let d = &self.data;
        let cap = d.provinces.cap();
        let ok: Vec<bool> = (0..cap as Id)
            .map(|p| {
                p != target
                    && d.provinces.contains(p)
                    && (!c.same_kind || d.kind(p) == kind)
                    && (!c.same_state || d.state_of(p) == state)
                    && (!c.same_nation || d.nation_of(p) == nation)
            })
            .collect();
        let take = {
            let lab = self.labels.read();
            let mut take = vec![false; lab.px.len()];
            let mut taken = vec![0u32; cap];
            for i in s.pixels() {
                let p = lab.px[i] as usize;
                if ok.get(p) == Some(&true) {
                    take[i] = true;
                    taken[p] += 1;
                }
            }
            let whole: Vec<bool> = taken.iter().enumerate().map(|(p, &k)| k > 0 && (1..MIN_PROVINCE_PX).contains(&self.stats.get(p as Id).px.saturating_sub(k))).collect();
            if whole.iter().any(|&b| b) {
                for (t, &p) in take.iter_mut().zip(&lab.px) {
                    *t |= whole.get(p as usize) == Some(&true);
                }
            }
            take
        };
        let w = self.w as usize;
        let (n, bbox) = self.with_pixels(|wr, _| {
            let (mut n, mut b) = (0, [usize::MAX, usize::MAX, 0, 0]);
            for (i, _) in take.iter().enumerate().filter(|(_, t)| **t) {
                wr.set(i, target);
                n += 1;
                let (x, y) = (i % w, i / w);
                b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
            }
            (n, b)
        });
        if n == 0 {
            bail!("the loop holds no pixels {} may take (see \"only over\")", self.data.prov_name(target));
        }
        self.fix_crossings(Some(bbox));
        Ok(n)
    }
}

impl Editor {
    /// Lasso tool on the map (see the module comment). `pos`, `origin`: the
    /// pointer and where the button went down, in map pixels; `px_scale`:
    /// screen points per map pixel.
    pub(super) fn lasso_input(&mut self, ctx: &egui::Context, response: &egui::Response, pos: Option<[f64; 2]>, origin: Option<[f64; 2]>, px_scale: f64) {
        let pt = 1.0 / px_scale.max(1e-9); // a screen point in map pixels
        let primary = PointerButton::Primary;
        if response.drag_started_by(primary)
            && let Some(o) = origin
        {
            self.lasso.get_or_insert_with(Loop::default).press(o);
        }
        if let (Some(l), Some(p)) = (self.lasso.as_mut(), pos)
            && response.dragged_by(primary)
        {
            l.drag(p, 2.0 * pt);
        }
        if let Some(l) = self.lasso.as_mut().filter(|l| l.freehand)
            && (response.drag_stopped_by(primary) || !ctx.input(|i| i.pointer.primary_down()))
        {
            l.release(pos);
        }
        if !response.clicked_by(primary) {
            return;
        }
        let Some(p) = pos else { return };
        let close = CLOSE * pt;
        if response.double_clicked_by(primary) || response.triple_clicked_by(primary) {
            // (egui counts clicks on one spot only: the first one added a
            // corner there, or closed the loop already)
            if let Some(l) = self.lasso.take() {
                self.finish_lasso(&l.points);
            }
            return;
        }
        if self.lasso.get_or_insert_with(Loop::default).click(p, close) {
            let l = self.lasso.take().unwrap();
            self.finish_lasso(&l.points);
        }
    }

    /// Lasso keys: Enter closes the loop, Backspace takes back its last part.
    pub(super) fn lasso_keys(&mut self, ctx: &egui::Context) {
        let Some(l) = self.lasso.as_mut() else { return };
        let pressed = |k| ctx.input(|i| i.key_pressed(k) && i.modifiers.is_none());
        if pressed(egui::Key::Enter) {
            let l = self.lasso.take().unwrap();
            self.finish_lasso(&l.points);
        } else if pressed(egui::Key::Backspace) && !l.freehand && !l.back() {
            self.lasso = None;
        }
    }

    /// Apply a loop (map pixel coordinates) on the Provinces or States level.
    pub(super) fn finish_lasso(&mut self, pts: &[[f64; 2]]) {
        let Some(doc) = &self.doc else { return };
        let Some(shape) = Shape::of(pts, doc.w as usize, doc.h as usize) else {
            self.info("lasso: the loop holds no area");
            return;
        };
        let level = self.settings.level;
        let onto = self.settings.lasso_onto;
        let target = self.sel.first().copied();
        if onto && target.is_none() {
            self.error(format!("lasso: select (or Alt+click) the {} to draw onto", level.noun()));
            return;
        }
        let (c, coarsen) = (self.settings.constraint, self.settings.coarsen);
        let mut made: Vec<Id> = Vec::new();
        const NONE_INSIDE: &str = "no land province is more than half inside the loop";
        match (level, target.filter(|_| onto)) {
            (Level::Provinces, None) => self.run("Lasso: new province", |d, g| {
                let (ids, n) = d.lasso_new_province(&shape, c, coarsen.then_some(g))?;
                made = ids;
                Ok(match made.as_slice() {
                    [id] => format!("new province {} ({id}): {n} px", d.data.prov_name(*id)),
                    _ => format!("{} new provinces: {n} px", made.len()),
                })
            }),
            (Level::Provinces, Some(t)) => self.run("Lasso onto province", |d, _| {
                let n = d.lasso_onto_province(&shape, t, c)?;
                Ok(format!("{n} px drawn onto {}", d.data.prov_name(t)))
            }),
            (Level::States, None) => self.run("Lasso: new state", |d, _| {
                let provs = d.land_mostly_in(&shape);
                if provs.is_empty() {
                    bail!(NONE_INSIDE);
                }
                let s = d.new_state_from(&provs)?;
                made = vec![s];
                Ok(format!("new state {}: {} provinces", d.data.state_name(s), provs.len()))
            }),
            (Level::States, Some(t)) => self.run("Lasso onto state", |d, _| {
                let provs = d.land_mostly_in(&shape);
                let n = d.assign_provinces_to_state(&provs, t);
                if n == 0 {
                    bail!(if provs.is_empty() { NONE_INSIDE } else { "the land provinces inside are in that state already" });
                }
                Ok(format!("{n} provinces joined {}", d.data.state_name(t)))
            }),
            _ => {}
        }
        // (a clean-up may have removed what was made again)
        let Some(doc) = &self.doc else { return };
        made.retain(|&e| if level == Level::States { doc.data.states.contains(e) } else { doc.data.provinces.contains(e) });
        if !made.is_empty() {
            self.sel = made;
        }
    }

    /// The loop being drawn: its lines, the line on to the cursor, a dashed
    /// line back to the start, and the start (filled where a click closes).
    pub(super) fn draw_lasso(&self, dc: &DrawCtx, pl: &Placement) {
        let Some(l) = &self.lasso else { return };
        let mut s: Vec<egui::Pos2> = l.points.iter().filter_map(|&p| to_screen(dc, pl, p)).collect();
        let Some(&start) = s.first() else { return };
        let cursor = self.hover.filter(|_| !l.freehand).and_then(|h| to_screen(dc, pl, h));
        s.extend(cursor);
        if s.len() >= 2 {
            dc.painter.add(egui::Shape::line(s.clone(), Stroke::new(3.5, Color32::from_black_alpha(150))));
            dc.painter.add(egui::Shape::line(s.clone(), Stroke::new(1.5, LINE)));
            dc.painter.extend(egui::Shape::dashed_line(&[s[s.len() - 1], start], Stroke::new(1.0, LINE), 4.0, 4.0));
        }
        let closes = l.can_close() && cursor.is_some_and(|c| c.distance(start) as f64 <= CLOSE);
        let fill = if closes { LINE } else { Color32::from_black_alpha(150) };
        dc.painter.circle(start, if closes { 5.0 } else { 3.5 }, fill, Stroke::new(1.5, LINE));
    }

    /// Lasso settings in the panel (below the tools).
    pub(super) fn lasso_panel(&mut self, ui: &mut egui::Ui) {
        let level = self.settings.level;
        let noun = level.noun();
        ui.horizontal_wrapped(|ui| {
            ui.label("the loop makes:");
            ui.radio_value(&mut self.settings.lasso_onto, false, format!("a new {noun}"));
            ui.radio_value(&mut self.settings.lasso_onto, true, format!("more of the selected {noun}"));
        });
        let what = match (level, self.settings.lasso_onto) {
            (Level::States, false) => "The land provinces more than half inside become a new state.",
            (Level::States, true) => "The land provinces more than half inside join the selected state (Alt+click picks it).",
            (_, false) => "The area becomes a new province of the type most of it lies on, in the state and zone of most of it.",
            (_, true) => "The area goes to the selected province (Alt+click picks it), as the paint brush would paint it.",
        };
        let how = "Hold the button to draw freehand; let go and click to add corners. Close: click the start, double-click or Enter. Backspace takes back the last part, Esc drops the loop.";
        ui.label(RichText::new(format!("{how} {what}")).small().weak());
        if let Some(l) = &self.lasso {
            let close = if l.can_close() { ": click the start, double-click or Enter closes it" } else { "" };
            ui.colored_label(LINE, format!("loop of {} points{close}", l.points.len()));
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::super::model::{Labels, Loaded, MapData, Region, State};
    use super::super::{Settings, Tool};
    use super::*;

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<[f64; 2]> {
        vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
    }

    fn inside(s: &Shape) -> Vec<usize> {
        s.pixels().collect()
    }

    #[test]
    fn loops_hold_the_pixel_centres_inside() {
        // centres 1.5..4.5 by 1.5..3.5
        let s = Shape::of(&rect(1.0, 1.0, 5.0, 4.0), 10, 10).unwrap();
        assert_eq!(s.bbox, [1, 1, 4, 3]);
        assert_eq!(inside(&s).len(), 12);
        assert!(s.contains(11) && s.contains(34) && !s.contains(0) && !s.contains(15) && !s.contains(41));
        // a triangle: the row through centre y = 0.5 holds x 0..=3 (x + 0.5 < 4)
        let s = Shape::of(&[[0.0, 0.0], [8.0, 0.0], [0.0, 8.0]], 10, 10).unwrap();
        assert_eq!(inside(&s).iter().filter(|&&i| i < 10).count(), 7);
        assert_eq!(inside(&s).len(), 7 + 6 + 5 + 4 + 3 + 2 + 1);
        // clipped to the map; a line or a loop off the map holds nothing
        let s = Shape::of(&rect(-5.0, -5.0, 3.0, 2.0), 10, 10).unwrap();
        assert_eq!(inside(&s), vec![0, 1, 2, 10, 11, 12]);
        assert!(Shape::of(&[[1.0, 1.0], [5.0, 5.0], [3.0, 3.0]], 10, 10).is_none());
        assert!(Shape::of(&rect(20.0, 20.0, 30.0, 30.0), 10, 10).is_none());
        assert!(Shape::of(&[[1.0, 1.0], [5.0, 5.0]], 10, 10).is_none());
        // drawn past its start (the overlap twice around), and a figure of
        // eight (one half each way round): all inside
        let mut twice = rect(1.0, 1.0, 5.0, 4.0);
        twice.extend(rect(1.0, 1.0, 5.0, 4.0));
        assert_eq!(inside(&Shape::of(&twice, 10, 10).unwrap()).len(), 12);
        // (crossing at 4, 2: the row through 2.5 is inside on x 0..3 and 5..8)
        let eight = [[0.0, 0.0], [8.0, 4.0], [8.0, 0.0], [0.0, 4.0]];
        let s = Shape::of(&eight, 10, 10).unwrap();
        let row: Vec<usize> = (0..10).filter(|&x| s.contains(20 + x)).collect();
        assert_eq!(row, [0, 1, 2, 5, 6, 7]);
    }

    fn from<T: serde::de::DeserializeOwned>(v: Value) -> T {
        serde_json::from_value(v).unwrap()
    }

    /// 20×10 px: land 1 (state A) on the left half, land 2 (state B) top
    /// right, sea 3 bottom right; the land in one air zone.
    fn map() -> Doc {
        let mut data = MapData { pixel_m: 100.0, ..Default::default() };
        for (id, kind) in [(1, "land"), (2, "land"), (3, "sea")] {
            data.provinces.put(id, from(json!({"id": id, "rgb": [10 * id, 0, 0], "type": kind, "name": format!("p{id}")})));
        }
        for (id, name) in [(1, "A"), (2, "B")] {
            data.states.put(id, from::<State>(json!({"id": id, "name": name})));
        }
        data.regions.put(1, from::<Region>(json!({"id": 1, "name": "air"})));
        data.regions.put(2, from::<Region>(json!({"id": 2, "name": "sea", "naval": true})));
        for (p, s, r) in [(1, 1, 1), (2, 2, 1), (3, 0, 2)] {
            let pr = data.provinces.get_mut(p).unwrap();
            (pr.state, pr.region) = (s, r);
        }
        let px = (0..200).map(|i| if i % 20 < 10 { 1 } else if i / 20 < 5 { 2 } else { 3 }).collect();
        let geo = crate::geo::FileGeoInfo { transform: None, crs: None, nodata: None };
        Doc::new(Default::default(), Loaded { data, labels: Labels { w: 20, h: 10, px }, geo, extra: Default::default() })
    }

    fn count(doc: &Doc, p: Id) -> usize {
        doc.labels.read().px.iter().filter(|&&q| q == p).count()
    }

    fn step<R>(doc: &mut Doc, f: impl FnOnce(&mut Doc) -> Result<R>) -> Result<R> {
        doc.begin("step");
        let r = f(doc);
        if r.is_ok() {
            doc.commit();
        } else {
            doc.rollback();
        }
        r
    }

    #[test]
    fn new_provinces_and_more_of_one() {
        let mut doc = map();
        // x 4..14, y 2..8: land 36 px of 1 and 12 of 2, sea 12
        let s = Shape::of(&rect(4.0, 2.0, 14.0, 8.0), 20, 10).unwrap();
        assert_eq!(doc.pixels_in(&s).get(&3), Some(&12));
        let only_land = Constraint::default();
        let (ids, n) = step(&mut doc, |d| d.lasso_new_province(&s, only_land, None)).unwrap();
        assert_eq!((ids.len(), n), (1, 48));
        let p = ids[0];
        let pr = doc.data.provinces.get(p).unwrap();
        assert!(pr.kind == Kind::Land && pr.state == 1 && pr.region == 1 && pr.name == "p1");
        assert_eq!(count(&doc, 3), 50, "the sea stays");
        doc.undo();
        // in the state most of it lies in only
        let c = Constraint { same_state: true, ..only_land };
        let (_, n) = step(&mut doc, |d| d.lasso_new_province(&s, c, None)).unwrap();
        assert_eq!(n, 36);
        doc.undo();
        // over everything: the land's province takes the sea too
        let all = Constraint { same_kind: false, ..only_land };
        let (_, n) = step(&mut doc, |d| d.lasso_new_province(&s, all, None)).unwrap();
        assert_eq!(n, 60);
        doc.undo();

        // more of 2: x 6..12, y 0..4 takes 16 px of 1
        let s = Shape::of(&rect(6.0, 0.0, 12.0, 4.0), 20, 10).unwrap();
        assert_eq!(step(&mut doc, |d| d.lasso_onto_province(&s, 2, only_land)).unwrap(), 16);
        assert_eq!((count(&doc, 1), count(&doc, 2)), (84, 66));
        doc.undo();
        // not over another state, nor the sea
        assert!(step(&mut doc, |d| d.lasso_onto_province(&s, 2, c)).is_err());
        let sea = Shape::of(&rect(10.0, 5.0, 20.0, 10.0), 20, 10).unwrap();
        assert!(step(&mut doc, |d| d.lasso_onto_province(&sea, 2, only_land)).is_err());
        // what it leaves of a province under 8 px goes too: all but 5 px of 1
        let most = Shape::of(&[[0.0, 0.0], [10.0, 0.0], [10.0, 9.4], [5.0, 9.4], [5.0, 10.0], [0.0, 10.0]], 20, 10).unwrap();
        assert_eq!(most.pixels().count(), 95);
        assert_eq!(step(&mut doc, |d| d.lasso_onto_province(&most, 2, only_land)).unwrap(), 100);
        assert_eq!(count(&doc, 1), 0);
    }

    #[test]
    fn the_editor_draws_provinces_and_states() {
        let mut ed = Editor::new(Settings { tool: Tool::Lasso, ..Settings::default() });
        ed.doc = Some(map());
        // a new province over most of 1 and some of 2: selected, one undo step
        ed.finish_lasso(&rect(2.0, 1.0, 13.0, 9.0));
        let doc = ed.doc.as_ref().unwrap();
        let p = ed.sel[0];
        assert!(p > 3 && doc.data.kind(p) == Kind::Land && count(doc, p) == 8 * 8 + 3 * 4);
        assert_eq!(doc.can_undo(), Some("Lasso: new province"));
        // without a selection there is nothing to draw onto
        ed.settings.lasso_onto = true;
        ed.sel.clear();
        ed.finish_lasso(&rect(0.0, 0.0, 3.0, 3.0));
        assert!(ed.message().is_some_and(|(m, err)| err && m.contains("select")));

        // states: 1 is more than half inside, 2 not
        ed.settings.level = Level::States;
        ed.doc.as_mut().unwrap().undo();
        ed.settings.lasso_onto = false;
        ed.finish_lasso(&rect(-1.0, -1.0, 12.0, 11.0));
        let doc = ed.doc.as_ref().unwrap();
        let s = ed.sel[0];
        assert_eq!(doc.data.state_of(1), s);
        assert_eq!(doc.data.state_of(2), 2);
        assert_eq!(doc.data.state_name(s), "p1 (new)");
        // then 2 joins it
        ed.settings.lasso_onto = true;
        ed.finish_lasso(&rect(9.0, -1.0, 21.0, 6.0));
        assert_eq!(ed.doc.as_ref().unwrap().data.state_of(2), s);
        // a loop that holds no land province by half
        ed.sel = vec![s];
        ed.finish_lasso(&rect(0.0, 0.0, 2.0, 2.0));
        assert!(ed.message().is_some_and(|(m, err)| err && m.contains("half inside")));
    }

    #[test]
    fn strokes_and_corners() {
        let mut l = Loop::default();
        l.press([0.0, 0.0]);
        l.drag([0.5, 0.0], 1.0); // too close for a point
        l.drag([4.0, 0.0], 1.0);
        l.release(Some([4.0, 3.0]));
        assert_eq!(l.points, [[0.0, 0.0], [4.0, 0.0], [4.0, 3.0]]);
        assert!(!l.freehand && l.can_close());
        // corners; a click on the start closes
        assert!(!l.click([0.0, 3.0], 0.5));
        assert!(!l.click([2.0, 2.0], 0.5));
        assert!(l.at_start([0.2, 0.2], 0.5) && !l.at_start([1.0, 0.0], 0.5));
        assert!(l.click([0.2, 0.2], 0.5));
        // a new stroke goes on from there; Backspace takes back a stroke or corner at a time
        l.press([3.0, 3.0]);
        l.drag([3.0, 5.0], 1.0);
        l.release(None);
        assert_eq!(l.points.len(), 7);
        assert!(l.back());
        assert_eq!(l.points.len(), 5);
        assert!(l.back() && l.back());
        assert_eq!(l.points, [[0.0, 0.0], [4.0, 0.0], [4.0, 3.0]]);
        assert!(!l.back(), "the first stroke was all that was left");
        assert!(l.points.is_empty());
    }

    /// The lasso on `map()` shown at 10 screen points per map pixel, driven by
    /// pointer and key events through egui.
    struct Screen {
        ctx: egui::Context,
        time: f64,
    }

    impl Screen {
        fn new() -> Screen {
            Screen { ctx: egui::Context::default(), time: 0.0 }
        }

        fn frame(&mut self, ed: &mut Editor, events: Vec<egui::Event>) {
            self.time += 1.0 / 60.0;
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0));
            let input = egui::RawInput { events, time: Some(self.time), screen_rect: Some(screen), ..Default::default() };
            let mut out = self.ctx.run_ui(input, |ui| {
                let r = ui.allocate_rect(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(200.0, 100.0)), egui::Sense::click_and_drag());
                let to_px = |p: egui::Pos2| Some([p.x as f64 / 10.0, p.y as f64 / 10.0]);
                let pos = r.interact_pointer_pos().or(r.hover_pos()).and_then(to_px);
                let origin = ui.input(|i| i.pointer.press_origin()).and_then(to_px).or(pos);
                ed.lasso_input(ui.ctx(), &r, pos, origin, 10.0);
                ed.lasso_keys(ui.ctx());
            });
            out.textures_delta.clear(); // no renderer here
        }

        fn button(&mut self, ed: &mut Editor, (x, y): (f32, f32), pressed: bool) {
            let pos = egui::pos2(x, y);
            self.frame(ed, vec![egui::Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE }]);
        }

        fn moved(&mut self, ed: &mut Editor, (x, y): (f32, f32)) {
            self.frame(ed, vec![egui::Event::PointerMoved(egui::pos2(x, y))]);
        }

        /// A click, a second after the last one.
        fn click(&mut self, ed: &mut Editor, at: (f32, f32)) {
            self.time += 1.0;
            self.quick_click(ed, at);
        }

        fn quick_click(&mut self, ed: &mut Editor, at: (f32, f32)) {
            self.moved(ed, at);
            self.button(ed, at, true);
            self.button(ed, at, false);
        }

        /// Hold the button along a path.
        fn drag(&mut self, ed: &mut Editor, path: &[(f32, f32)]) {
            self.time += 1.0;
            self.moved(ed, path[0]);
            self.button(ed, path[0], true);
            for &p in &path[1..] {
                self.moved(ed, p);
            }
            self.button(ed, path[path.len() - 1], false);
        }

        fn key(&mut self, ed: &mut Editor, key: egui::Key) {
            self.frame(ed, vec![egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }]);
        }
    }

    #[test]
    fn freehand_while_held_then_corners() {
        let mut ed = Editor::new(Settings { tool: Tool::Lasso, ..Settings::default() });
        ed.doc = Some(map());
        let mut sc = Screen::new();
        // the rectangle 2.5..13.5 × 1.5..8.5 (map pixels) holds 11 × 7 pixel
        // centres, 9 of them on the sea: 68 px of land
        let made = |ed: &Editor| ed.doc.as_ref().unwrap().can_undo() == Some("Lasso: new province") && count(ed.doc.as_ref().unwrap(), ed.sel[0]) == 68;
        let (a, b, c, d) = ((25.0, 15.0), (135.0, 15.0), (135.0, 85.0), (25.0, 85.0));

        // held along three sides: letting go leaves the loop open
        sc.drag(&mut ed, &[a, (80.0, 15.0), b, (135.0, 50.0), c]);
        let l = ed.lasso.as_ref().expect("letting go closed the loop");
        assert!(!l.freehand && l.points.len() >= 4 && l.points[0] == [2.5, 1.5] && l.points[l.points.len() - 1] == [13.5, 8.5]);
        // a click adds a corner, a click on (next to) the start closes it
        sc.click(&mut ed, d);
        assert!(ed.lasso.is_some() && ed.doc.as_ref().unwrap().can_undo().is_none());
        sc.click(&mut ed, (27.0, 17.0));
        assert!(ed.lasso.is_none() && made(&ed), "{:?}", ed.message());
        ed.doc.as_mut().unwrap().undo();

        // a double-click closes it (the first click is the last corner)
        sc.drag(&mut ed, &[a, (80.0, 15.0), b, (135.0, 50.0), c]);
        sc.click(&mut ed, d);
        sc.quick_click(&mut ed, d);
        assert!(ed.lasso.is_none() && made(&ed), "{:?}", ed.message());
        ed.doc.as_mut().unwrap().undo();

        // all corners, clicked quickly (no double-clicks: other spots);
        // Backspace takes back the last one, Enter closes
        sc.click(&mut ed, a);
        for p in [b, (150.0, 50.0), c] {
            sc.quick_click(&mut ed, p);
        }
        assert_eq!(ed.lasso.as_ref().unwrap().points.len(), 4);
        sc.key(&mut ed, egui::Key::Backspace);
        sc.key(&mut ed, egui::Key::Backspace);
        assert_eq!(ed.lasso.as_ref().unwrap().points.len(), 2);
        sc.click(&mut ed, c);
        sc.click(&mut ed, d);
        sc.key(&mut ed, egui::Key::Enter);
        assert!(ed.lasso.is_none() && made(&ed), "{:?}", ed.message());
        ed.doc.as_mut().unwrap().undo();

        // two strokes: the second goes on with a straight line from the first
        sc.drag(&mut ed, &[a, (80.0, 15.0), b]);
        sc.drag(&mut ed, &[c, (80.0, 85.0), d]);
        sc.key(&mut ed, egui::Key::Enter);
        assert!(ed.lasso.is_none() && made(&ed), "{:?}", ed.message());
        // a loop too thin to hold a pixel centre makes nothing
        let n = ed.doc.as_ref().unwrap().data.provinces.iter().count();
        sc.drag(&mut ed, &[a, b]);
        sc.key(&mut ed, egui::Key::Enter);
        assert!(ed.lasso.is_none() && ed.doc.as_ref().unwrap().data.provinces.iter().count() == n);
    }
}
