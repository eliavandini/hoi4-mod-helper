//! The Rivers level of the editor: drawing and editing rivers, generating
//! them from the heights, and how the game will get them (rivers.bmp).

use std::sync::Arc;

use crossbeam_channel::Receiver;
use eframe::egui::{self, Color32, Key, PointerButton, Pos2, Rect, RichText, Stroke};

use super::model::{RIVER_WIDTHS, River, RiverPoint, SMALL_RIVER_MAX};
use super::rivers::{self, FLOW_IN, FLOW_OUT, Raster, SOURCE};
use super::{Editor, Tool, WARN, version};
use crate::layers::Placement;
use crate::view::DrawCtx;

/// How close (screen points) a click must be to pick a river or a point.
const PICK: f32 = 8.0;
const RIVER: Color32 = Color32::from_rgb(70, 150, 255);
const SELECTED: Color32 = Color32::from_rgb(255, 225, 60);

/// An edit of the selected river: undo label and the change.
type RiverEdit = (&'static str, Box<dyn FnOnce(&mut River)>);

pub struct RiverUi {
    pub sel: Option<usize>,
    /// The river being drawn.
    pub draft: Option<Vec<RiverPoint>>,
    /// Width of new points.
    pub width: u8,
    /// A point being dragged: (river, point).
    pub drag: Option<(usize, usize)>,
    /// Widths from source to mouth for "Set widths".
    pub widths: (u8, u8),
    /// Smallest drained area (km²) of generated rivers.
    pub min_km2: f32,
    pub replace: bool,
    pub gen_rx: Option<Receiver<Vec<River>>>,
    /// Show the rivers.bmp pixels when zoomed in.
    pub show_pixels: bool,
    /// rivers.bmp as the build makes it, for a map version.
    pub raster: Option<(u64, Arc<Raster>)>,
    /// The name being typed for a river: (river, text, its name when the
    /// typing started). It goes in when the field is left.
    pub name_edit: Option<(usize, String, String)>,
}

impl Default for RiverUi {
    fn default() -> Self {
        RiverUi {
            sel: None,
            draft: None,
            width: 3,
            drag: None,
            widths: (1, 6),
            min_km2: 40.0,
            replace: true,
            gen_rx: None,
            show_pixels: true,
            raster: None,
            name_edit: None,
        }
    }
}

pub fn to_screen(dc: &DrawCtx, pl: &Placement, p: [f64; 2]) -> Option<Pos2> {
    pl.pixel_to_world(p[0], p[1]).map(|w| dc.cam.to_screen(dc.rect, w))
}

/// Map pixels in view: [x0, y0, x1, y1] (inclusive), clamped to the map.
pub fn visible_px(dc: &DrawCtx, pl: &Placement, w: u32, h: u32) -> Option<[u32; 4]> {
    let r = dc.rect;
    let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for c in [r.left_top(), r.right_top(), r.left_bottom(), r.right_bottom()] {
        let p = pl.world_to_pixel(dc.cam.to_world(r, c))?;
        b = [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])];
    }
    let cl = |v: f64, m: u32| v.floor().clamp(0.0, m as f64 - 1.0) as u32;
    (b[2] >= 0.0 && b[3] >= 0.0 && b[0] < w as f64 && b[1] < h as f64).then(|| [cl(b[0], w), cl(b[1], h), cl(b[2], w), cl(b[3], h)])
}

/// Colour of a rivers.bmp index.
pub fn index_colour(v: u8) -> Color32 {
    match v {
        SOURCE => Color32::from_rgb(40, 220, 60),
        FLOW_IN => Color32::from_rgb(235, 40, 40),
        FLOW_OUT => Color32::from_rgb(250, 230, 40),
        v => {
            let k = (v.saturating_sub(3)) as f32 / (RIVER_WIDTHS - 1) as f32;
            Color32::from_rgb((20.0 + 40.0 * (1.0 - k)) as u8, (150.0 - 110.0 * k) as u8, (255.0 - 90.0 * k) as u8)
        }
    }
}

fn river_len_km(r: &River, m_per_px: f64) -> f64 {
    r.points.windows(2).map(|s| ((s[1].0 - s[0].0) as f64).hypot((s[1].1 - s[0].1) as f64)).sum::<f64>() * m_per_px / 1000.0
}

pub fn river_name(r: &River, i: usize) -> String {
    if r.name.is_empty() { format!("river {}", i + 1) } else { r.name.clone() }
}

impl Editor {
    /// rivers.bmp as the build would make it (cached per map version).
    pub(super) fn river_raster(&mut self) -> Option<Arc<Raster>> {
        let doc = self.doc.as_ref()?;
        let v = version(doc);
        if let Some((k, r)) = &self.rv.raster
            && *k == v
        {
            return Some(r.clone());
        }
        let lab = doc.labels.read();
        let r = Arc::new(rivers::rasterize(&doc.data.rivers, doc.w as usize, doc.h as usize, super::supply::ground(&doc.data, &lab)));
        drop(lab);
        self.rv.raster = Some((v, r.clone()));
        Some(r)
    }

    fn river_screen(&self, dc: &DrawCtx, pl: &Placement, r: &River) -> Vec<Pos2> {
        r.points.iter().filter_map(|p| to_screen(dc, pl, [p.0 as f64, p.1 as f64])).collect()
    }

    /// The point of the selected river near a screen position.
    fn river_point_at(&self, dc: &DrawCtx, pl: &Placement, at: Pos2) -> Option<(usize, usize)> {
        let doc = self.doc.as_ref()?;
        let ri = self.rv.sel?;
        let r = doc.data.rivers.get(ri)?;
        let pts = self.river_screen(dc, pl, r);
        let (k, d) = pts.iter().enumerate().map(|(k, p)| (k, p.distance(at))).min_by(|a, b| a.1.total_cmp(&b.1))?;
        (d <= PICK).then_some((ri, k))
    }

    /// The river (and segment) nearest a screen position.
    fn river_at(&self, dc: &DrawCtx, pl: &Placement, at: Pos2) -> Option<(usize, usize)> {
        let doc = self.doc.as_ref()?;
        let mut best: Option<(f32, usize, usize)> = None;
        for (i, r) in doc.data.rivers.iter().enumerate() {
            let pts = self.river_screen(dc, pl, r);
            for (k, s) in pts.windows(2).enumerate() {
                let d = super::dist_to_segment(at, s[0], s[1]);
                if d <= PICK && best.is_none_or(|b| d < b.0) {
                    best = Some((d, i, k));
                }
            }
        }
        best.map(|b| (b.1, b.2))
    }

    fn finish_river(&mut self) {
        let Some(pts) = self.rv.draft.take() else { return };
        if pts.len() < 2 {
            self.error("a river needs two points or more");
            return;
        }
        let mut idx = None;
        self.run("Draw river", |d, _| {
            d.data.rivers.push(River { name: String::new(), points: pts });
            idx = Some(d.data.rivers.len() - 1);
            Ok(format!("new river with {} points", d.data.rivers.last().unwrap().points.len()))
        });
        self.rv.sel = idx;
    }

    /// Map input on the Rivers level. Select: click picks a river, drag moves
    /// a point of the selected one, Shift+click adds a point, Ctrl+click
    /// removes one. Draw: clicks add points, double-click (or Enter) ends.
    pub(super) fn rivers_input(&mut self, dc: &DrawCtx, response: &egui::Response, pl: &Placement) -> bool {
        let mods = dc.ctx.input(|i| i.modifiers);
        let to_px = |p: Pos2| pl.world_to_pixel(dc.cam.to_world(dc.rect, p));
        let click = response.interact_pointer_pos();
        if self.rv.sel.is_some_and(|i| self.doc.as_ref().is_none_or(|d| i >= d.data.rivers.len())) {
            self.rv.sel = None;
        }
        match self.settings.tool {
            Tool::Draw => {
                if response.double_clicked_by(PointerButton::Primary) || response.clicked_by(PointerButton::Secondary) {
                    self.finish_river();
                    return true;
                }
                if response.clicked_by(PointerButton::Primary)
                    && let Some(px) = click.and_then(to_px)
                {
                    let w = self.rv.width;
                    self.rv.draft.get_or_insert_with(Vec::new).push(RiverPoint(px[0] as f32, px[1] as f32, w));
                }
                true
            }
            _ => {
                // dragging a point of the selected river
                if response.drag_started_by(PointerButton::Primary) {
                    let origin = dc.ctx.input(|i| i.pointer.press_origin());
                    if let Some(hit) = origin.and_then(|o| self.river_point_at(dc, pl, o)) {
                        self.rv.drag = Some(hit);
                        if let Some(doc) = self.doc.as_mut() {
                            doc.begin("Move river point");
                        }
                    }
                }
                if let Some((ri, k)) = self.rv.drag {
                    if response.dragged_by(PointerButton::Primary)
                        && let (Some(px), Some(doc)) = (click.and_then(to_px), self.doc.as_mut())
                        && let Some(p) = doc.data.rivers.get_mut(ri).and_then(|r| r.points.get_mut(k))
                    {
                        (p.0, p.1) = (px[0] as f32, px[1] as f32);
                        doc.meta_version += 1;
                    }
                    if response.drag_stopped_by(PointerButton::Primary) || !dc.ctx.input(|i| i.pointer.primary_down()) {
                        self.rv.drag = None;
                        if let Some(doc) = self.doc.as_mut() {
                            doc.commit();
                        }
                        self.after_edit();
                    }
                    return true;
                }
                if !response.clicked_by(PointerButton::Primary) {
                    return true;
                }
                let Some(at) = click else { return true };
                if mods.command {
                    if let Some((ri, k)) = self.river_point_at(dc, pl, at) {
                        self.run("Remove river point", |d, _| {
                            let r = &mut d.data.rivers[ri];
                            if r.points.len() <= 2 {
                                anyhow::bail!("a river keeps two points at least (Delete removes it)");
                            }
                            r.points.remove(k);
                            Ok(String::new())
                        });
                    }
                    return true;
                }
                if mods.shift
                    && let (Some(sel), Some((ri, k)), Some(px)) = (self.rv.sel, self.river_at(dc, pl, at), to_px(at))
                    && sel == ri
                {
                    self.run("Add river point", |d, _| {
                        let r = &mut d.data.rivers[ri];
                        let wd = r.points[k].2.max(r.points[k + 1].2);
                        r.points.insert(k + 1, RiverPoint(px[0] as f32, px[1] as f32, wd));
                        Ok(String::new())
                    });
                    return true;
                }
                self.rv.sel = self.river_at(dc, pl, at).map(|h| h.0);
                true
            }
        }
    }

    pub(super) fn rivers_keys(&mut self, ctx: &egui::Context) {
        let pressed = |k| ctx.input(|i| i.key_pressed(k) && !i.modifiers.command);
        if self.rv.draft.is_some() {
            if pressed(Key::Enter) {
                self.finish_river();
            }
            if pressed(Key::Backspace)
                && let Some(d) = &mut self.rv.draft
            {
                d.pop();
            }
            if pressed(Key::Escape) {
                self.rv.draft = None;
            }
        } else if pressed(Key::Delete) {
            self.delete_river();
        }
    }

    /// A river name typed but not in the map yet.
    pub(super) fn river_name_pending(&self) -> bool {
        self.rv.name_edit.as_ref().is_some_and(|e| e.1 != e.2)
    }

    /// Put the typed river name into the map (one undo step).
    pub(super) fn flush_river_name(&mut self) {
        let Some((ri, new, old)) = self.rv.name_edit.clone() else { return };
        if new == old {
            return;
        }
        self.rv.name_edit = Some((ri, new.clone(), new.clone()));
        self.run("Rename river", move |d, _| {
            let r = d.data.rivers.get_mut(ri).ok_or_else(|| anyhow::anyhow!("the river is gone"))?;
            r.name = new;
            Ok(format!("river {} is now {}", ri + 1, river_name(r, ri)))
        });
    }

    fn delete_river(&mut self) {
        self.flush_river_name();
        let Some(ri) = self.rv.sel.filter(|&i| self.doc.as_ref().is_some_and(|d| i < d.data.rivers.len())) else { return };
        self.run("Delete river", |d, _| {
            let r = d.data.rivers.remove(ri);
            Ok(format!("deleted {}", river_name(&r, ri)))
        });
        self.rv.sel = None;
    }

    /// Rivers on the map: lines from the points, and when zoomed in the
    /// pixels of rivers.bmp.
    pub(super) fn draw_rivers(&mut self, dc: &DrawCtx, pl: &Placement, px_scale: f64, editing: bool) {
        if self.doc.is_none() {
            return;
        }
        let pixels = editing && self.rv.show_pixels && px_scale >= 3.0;
        if pixels
            && let (Some(r), Some(doc)) = (self.river_raster(), self.doc.as_ref())
            && let Some([x0, y0, x1, y1]) = visible_px(dc, pl, doc.w, doc.h)
        {
            let w = doc.w as usize;
            for y in y0..=y1 {
                for &(i, v, _) in r.row(w, y as usize, x0 as usize, x1 as usize) {
                    let x = i as usize % w;
                    if let (Some(a), Some(b)) = (to_screen(dc, pl, [x as f64, y as f64]), to_screen(dc, pl, [x as f64 + 1.0, y as f64 + 1.0])) {
                        dc.painter.rect_filled(Rect::from_two_pos(a, b), 0.0, index_colour(v));
                    }
                }
            }
        }
        let doc = self.doc.as_ref().unwrap();
        let scale = (px_scale as f32 / 2.0).clamp(1.0, 3.0);
        let alpha = if pixels { 110 } else { 255 };
        for (i, r) in doc.data.rivers.iter().enumerate() {
            let sel = editing && self.rv.sel == Some(i);
            let pts: Vec<(Pos2, u8)> = r.points.iter().filter_map(|p| to_screen(dc, pl, [p.0 as f64, p.1 as f64]).map(|s| (s, p.2))).collect();
            if pts.len() < 2 || !dc.rect.expand(10.0).intersects(Rect::from_points(&pts.iter().map(|p| p.0).collect::<Vec<_>>())) {
                continue;
            }
            for s in pts.windows(2) {
                let wd = (0.8 + 0.3 * s[0].1.max(s[1].1) as f32) * scale;
                let c = if sel { SELECTED } else { RIVER };
                dc.painter.line_segment([s[0].0, s[1].0], Stroke::new(wd, Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), alpha)));
            }
            if sel {
                for (k, (p, _)) in pts.iter().enumerate() {
                    let fill = if k == 0 { index_colour(SOURCE) } else if k + 1 == pts.len() { Color32::from_rgb(40, 90, 230) } else { Color32::WHITE };
                    dc.painter.circle(*p, 4.0, fill, Stroke::new(1.0, Color32::BLACK));
                }
                // which way it flows
                for s in pts.windows(2) {
                    let (a, b) = (s[0].0, s[1].0);
                    if a.distance(b) > 30.0 {
                        let m = a + (b - a) * 0.5;
                        let d = (b - a).normalized() * 7.0;
                        let n = egui::vec2(-d.y, d.x) * 0.6;
                        dc.painter.add(egui::Shape::convex_polygon(vec![m + d, m - d + n, m - d - n], SELECTED, Stroke::new(1.0, Color32::BLACK)));
                    }
                }
            }
        }
        if editing && let Some(draft) = &self.rv.draft {
            let mut pts: Vec<Pos2> = draft.iter().filter_map(|p| to_screen(dc, pl, [p.0 as f64, p.1 as f64])).collect();
            if let Some(h) = self.hover.and_then(|h| to_screen(dc, pl, h)) {
                pts.push(h);
            }
            dc.painter.add(egui::Shape::line(pts.clone(), Stroke::new(2.5, Color32::from_rgb(255, 160, 40))));
            for p in pts.iter().take(draft.len()) {
                dc.painter.circle(*p, 3.5, Color32::WHITE, Stroke::new(1.0, Color32::BLACK));
            }
        }
    }

    /// Generated rivers from the background job, as one undo step.
    fn poll_river_generation(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.rv.gen_rx else { return };
        match rx.try_recv() {
            Ok(found) => {
                self.rv.gen_rx = None;
                self.flush_river_name();
                self.rv.name_edit = None;
                let replace = self.rv.replace;
                self.run("Generate rivers", |d, _| {
                    let n = found.len();
                    if replace {
                        d.data.rivers = found;
                    } else {
                        d.data.rivers.extend(found);
                    }
                    Ok(format!("{n} rivers from the heights"))
                });
                self.rv.sel = None;
            }
            Err(crossbeam_channel::TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
            Err(_) => {
                self.rv.gen_rx = None;
                self.error("generating rivers failed");
            }
        }
    }

    fn start_river_generation(&mut self) {
        let Some(doc) = &self.doc else { return };
        let Some(t) = doc.terrain.clone() else {
            self.error("the heights are not loaded (yet)");
            return;
        };
        let d = &doc.data;
        let (sea, lake): (Vec<bool>, Vec<bool>) = {
            let kinds: Vec<super::model::Kind> = (0..d.provinces.cap() as u32).map(|p| d.kind(p)).collect();
            let lab = doc.labels.read();
            let of = |k: super::model::Kind| lab.px.iter().map(|&p| kinds.get(p as usize) == Some(&k)).collect();
            (of(super::model::Kind::Sea), of(super::model::Kind::Lake))
        };
        let (w, h, k, min) = (doc.w as usize, doc.h as usize, d.km2_per_px(), self.rv.min_km2 as f64);
        let (tx, rx) = crossbeam_channel::bounded(1);
        std::thread::spawn(move || {
            let _ = tx.send(rivers::generate(&t.dem, &sea, &lake, w, h, k, min));
        });
        self.rv.gen_rx = Some(rx);
    }

    pub(super) fn rivers_panel(&mut self, ui: &mut egui::Ui) {
        // another river selected: the name typed for the last one goes to it
        if self.rv.name_edit.as_ref().is_some_and(|e| Some(e.0) != self.rv.sel) {
            self.flush_river_name();
            self.rv.name_edit = None;
        }
        self.poll_river_generation(ui.ctx());
        let raster = self.river_raster();
        let Some(doc) = self.doc.as_ref() else { return };
        let m_per_px = doc.data.pixel_m;
        let n = doc.data.rivers.len();
        let total: f64 = doc.data.rivers.iter().map(|r| river_len_km(r, m_per_px)).sum();
        let mut zoom: Option<[f64; 4]> = None;
        let mut select: Option<usize> = None;
        let mut edit: Option<RiverEdit> = None;
        let mut delete = false;
        let mut clear = false;
        let mut generate = false;
        egui::CollapsingHeader::new(format!("Rivers: {n} ({total:.0} km)")).id_salt("rivers").default_open(true).show(ui, |ui| {
            match self.settings.tool {
                Tool::Draw => {
                    ui.label(RichText::new("Click from the source down to the mouth; double-click, right-click or Enter ends the river, Backspace takes back a point, Esc cancels.").small().weak());
                }
                _ => {
                    ui.label(RichText::new("Click a river to select it; drag its points; Shift+click on it adds a point, Ctrl+click on a point removes it; Del deletes it.").small().weak());
                }
            }
            ui.horizontal(|ui| {
                ui.add(egui::Slider::new(&mut self.rv.width, 1..=RIVER_WIDTHS).text("width of new points"));
            });
            ui.label(RichText::new(format!("1 is the narrowest, {RIVER_WIDTHS} the widest; up to {SMALL_RIVER_MAX} the game counts as a small river")).small().weak());
            ui.checkbox(&mut self.rv.show_pixels, "show the rivers.bmp pixels when zoomed in").on_hover_text(
                "green: source, red: flows into another river, yellow: branches off one; blue: darker is wider",
            );

            if let Some(ri) = self.rv.sel.filter(|&i| i < n) {
                let r = &doc.data.rivers[ri];
                ui.separator();
                // the typed name lives across frames; it follows the map (undo)
                // while nothing is typed
                let fresh = match &self.rv.name_edit {
                    Some((i, typed, was)) => *i != ri || (typed == was && *was != r.name),
                    None => true,
                };
                if fresh {
                    self.rv.name_edit = Some((ri, r.name.clone(), r.name.clone()));
                }
                if let Some((_, typed, _)) = self.rv.name_edit.as_mut() {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Selected:").strong());
                        ui.add(egui::TextEdit::singleline(typed).hint_text(river_name(r, ri)).desired_width(150.0))
                            .on_hover_text("Enter or a click elsewhere renames the river");
                    });
                }
                let (lo, hi) = (r.points.iter().map(|p| p.2).min().unwrap_or(1), r.points.iter().map(|p| p.2).max().unwrap_or(1));
                ui.label(format!("{} points · {:.1} km · width {lo}{}", r.points.len(), river_len_km(r, m_per_px), if hi != lo { format!(" to {hi}") } else { String::new() }));
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut self.rv.widths.0).range(1..=RIVER_WIDTHS).prefix("width from "));
                    ui.add(egui::DragValue::new(&mut self.rv.widths.1).range(1..=RIVER_WIDTHS).prefix("to "));
                    let (a, b) = self.rv.widths;
                    if ui.button("Set widths").on_hover_text("from the source to the mouth, evenly along the river").clicked() {
                        edit = Some((
                            "River widths",
                            Box::new(move |r: &mut River| {
                                let len: Vec<f32> = std::iter::once(0.0)
                                    .chain(r.points.windows(2).scan(0.0, |acc, s| {
                                        *acc += (s[1].0 - s[0].0).hypot(s[1].1 - s[0].1);
                                        Some(*acc)
                                    }))
                                    .collect();
                                let total = len.last().copied().unwrap_or(0.0).max(1e-6);
                                for (p, l) in r.points.iter_mut().zip(len) {
                                    p.2 = (a as f32 + (b as f32 - a as f32) * l / total).round() as u8;
                                }
                            }),
                        ));
                    }
                });
                ui.horizontal(|ui| {
                    if ui.button("⇄ Reverse").on_hover_text("make the other end the source").clicked() {
                        edit = Some(("Reverse river", Box::new(|r: &mut River| r.points.reverse())));
                    }
                    if ui.button("Simplify").on_hover_text("fewer points where they lie on a line (within 1 px)").clicked() {
                        edit = Some(("Simplify river", Box::new(|r: &mut River| r.points = rivers::simplify(&r.points, 1.0))));
                    }
                    if ui.button("Zoom to").clicked() {
                        zoom = bbox(r);
                    }
                    if ui.button("🗑 Delete (Del)").clicked() {
                        delete = true;
                    }
                });
            }

            ui.separator();
            ui.label(RichText::new("From the heights").strong());
            ui.horizontal(|ui| {
                ui.add(egui::Slider::new(&mut self.rv.min_km2, 2.0..=2000.0).logarithmic(true).text("km² drained at least"))
                    .on_hover_text("a river starts where the water of this much land gathers; less makes more and longer rivers");
            });
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.rv.replace, "replace the rivers");
                let busy = self.rv.gen_rx.is_some();
                if busy {
                    ui.spinner();
                }
                let ok = doc.terrain.is_some() && !busy;
                if ui
                    .add_enabled(ok, egui::Button::new("Generate"))
                    .on_hover_text("follow the water down the heights to the sea (or the map's edge); widths grow with the land drained")
                    .on_disabled_hover_text("needs the heights (dem_section.tif)")
                    .clicked()
                {
                    generate = true;
                }
                if ui.add_enabled(n > 0, egui::Button::new("Remove all")).clicked() {
                    clear = true;
                }
            });

            if let Some(r) = &raster {
                ui.separator();
                let small = r.px.iter().filter(|p| p.1 <= 2 + SMALL_RIVER_MAX).count();
                ui.label(format!("rivers.bmp: {} river pixels ({} small)", r.px.len(), small));
                for p in r.problems.iter().take(20) {
                    if ui.link(RichText::new(&p.msg).color(WARN)).on_hover_text("zoom there").clicked() {
                        let (x, y) = (p.at[0] as f64, p.at[1] as f64);
                        zoom = Some([x - 40.0, y - 40.0, x + 40.0, y + 40.0]);
                        select = Some(p.river);
                    }
                }
            }
            if n > 0 {
                ui.separator();
                egui::ScrollArea::vertical().id_salt("river_list").max_height(160.0).show(ui, |ui| {
                    for (i, r) in doc.data.rivers.iter().enumerate() {
                        let hi = r.points.iter().map(|p| p.2).max().unwrap_or(1);
                        let t = format!("{} · {:.1} km · width {hi}", river_name(r, i), river_len_km(r, m_per_px));
                        if ui.selectable_label(self.rv.sel == Some(i), t).clicked() {
                            select = Some(i);
                            zoom = bbox(r);
                        }
                    }
                });
            }
        });
        if let Some(i) = select {
            self.rv.sel = Some(i);
        }
        if let Some(b) = zoom {
            self.zoom_to = Some(b);
        }
        // the name goes in once its field is left (before any other change)
        let busy = ui.ctx().egui_wants_keyboard_input() || ui.ctx().input(|i| i.pointer.any_down());
        if !busy || edit.is_some() || delete || clear || generate {
            self.flush_pending();
        }
        if let (Some((label, f)), Some(ri)) = (edit, self.rv.sel) {
            self.run(label, |d, _| {
                f(d.data.rivers.get_mut(ri).ok_or_else(|| anyhow::anyhow!("the river is gone"))?);
                Ok(String::new())
            });
        }
        if delete {
            self.delete_river();
        }
        if generate {
            self.start_river_generation();
        }
        if clear {
            self.rv.name_edit = None;
            self.run("Remove all rivers", |d, _| {
                let n = d.data.rivers.len();
                d.data.rivers.clear();
                Ok(format!("removed {n} rivers"))
            });
            self.rv.sel = None;
        }
    }
}

fn bbox(r: &River) -> Option<[f64; 4]> {
    let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for p in &r.points {
        b = [b[0].min(p.0 as f64), b[1].min(p.1 as f64), b[2].max(p.0 as f64), b[3].max(p.1 as f64)];
    }
    (b[0] <= b[2]).then(|| [b[0] - 20.0, b[1] - 20.0, b[2] + 20.0, b[3] + 20.0])
}
