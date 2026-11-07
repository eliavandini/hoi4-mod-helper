//! The Supply level of the editor: railways, supply hubs, ports (naval
//! bases) and a map of the supply the game would give.

use std::sync::Arc;

use eframe::egui::{self, Align2, Color32, FontId, Key, PointerButton, Pos2, RichText, Stroke};

use super::model::{Id, Kind, Port, Railway};
use super::rivers_ui::to_screen;
use super::supply::{self, Estimate, MAX_PORT_LEVEL, MAX_RAIL_LEVEL, Scope, View};
use super::{Editor, Tool, WARN, version};
use crate::layers::Placement;
use crate::view::DrawCtx;

const PICK: f32 = 8.0;
const SELECTED: Color32 = Color32::from_rgb(255, 225, 60);

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pick {
    Railway(usize),
    /// A hub province.
    Hub(Id),
    /// Index in ports.
    Port(usize),
}

pub struct SupplyUi {
    pub sel: Option<Pick>,
    /// Stops of the railway being laid.
    pub draft: Option<Vec<[f32; 2]>>,
    /// Level of new railways.
    pub level: u8,
    pub port_level: u8,
    pub scope: Scope,
    /// Built-up km² per province, for a pixel version.
    urban: Option<UrbanCache>,
    pub view: Option<(u64, Arc<View>)>,
    pub est: Option<((u64, Scope), Arc<Estimate>)>,
    /// Counts estimates (for the map colours).
    pub est_gen: u64,
    /// Route of the railway being laid to the province under the cursor.
    preview: Option<RoutePreview>,
}

/// (pixel version, land cover loaded, built-up km² per province).
type UrbanCache = (u64, bool, Arc<Vec<f64>>);
/// ((stops, province under the cursor), provinces on the way).
type RoutePreview = ((Vec<[f32; 2]>, Id), Vec<Id>);
/// A change made after the panel is drawn.
type Deferred = Box<dyn FnOnce(&mut Editor)>;

impl Default for SupplyUi {
    fn default() -> Self {
        SupplyUi { sel: None, draft: None, level: 1, port_level: 1, scope: Scope::EachNation, urban: None, view: None, est: None, est_gen: 0, preview: None }
    }
}

/// Colour of an amount of supply: grey none, red to yellow below 1, yellow to
/// green up to 3.
pub fn supply_colour(f: f32) -> [u8; 3] {
    let lerp = |a: [f32; 3], b: [f32; 3], t: f32| [0, 1, 2].map(|k| (a[k] + (b[k] - a[k]) * t.clamp(0.0, 1.0)) as u8);
    if f <= 0.0 {
        [85, 85, 85]
    } else if f < 1.0 {
        lerp([170.0, 30.0, 30.0], [235.0, 205.0, 50.0], f)
    } else {
        lerp([235.0, 205.0, 50.0], [30.0, 150.0, 50.0], (f - 1.0) / 2.0)
    }
}

impl Editor {
    /// Railways, hubs, ports and the rest of what supply needs (cached per
    /// map version and terrain rules).
    pub(super) fn supply_view(&mut self) -> Option<Arc<View>> {
        let terrain = self.province_terrains();
        let doc = self.doc.as_ref()?;
        let has_t = doc.terrain.is_some();
        if !self.su.urban.as_ref().is_some_and(|t| t.0 == doc.pixel_version && t.1 == has_t) {
            let urban = doc.sealed_km2().unwrap_or_default();
            self.su.urban = Some((doc.pixel_version, has_t, Arc::new(urban)));
        }
        let rev = terrain.as_ref().map_or(0, |t| t.rev);
        let key = version(doc) ^ ((has_t as u64) << 63) ^ rev.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        if let Some((k, v)) = &self.su.view
            && *k == key
        {
            return Some(v.clone());
        }
        let urban = &self.su.urban.as_ref().unwrap().2;
        let terr = terrain.map(|t| t.terrain.as_ref().clone()).unwrap_or_default();
        let v = Arc::new(supply::view(&doc.data, &doc.stats, &doc.labels.read(), urban, terr));
        self.su.view = Some((key, v.clone()));
        Some(v)
    }

    pub(super) fn supply_estimate(&mut self) -> Option<Arc<Estimate>> {
        let v = self.supply_view()?;
        let key = (self.su.view.as_ref()?.0, self.su.scope);
        if let Some((k, e)) = &self.su.est
            && *k == key
        {
            return Some(e.clone());
        }
        let e = Arc::new(v.estimate(&self.doc.as_ref()?.data, self.su.scope));
        self.su.est = Some((key, e.clone()));
        self.su.est_gen += 1;
        Some(e)
    }

    /// The province's point to keep for it: its pixel nearest its middle.
    fn inside(doc: &super::doc::Doc, p: Id) -> [f32; 2] {
        supply::inside_point(&doc.labels.read().px, doc.w as usize, &doc.stats.get(p), p)
    }

    /// Railways as a list to edit (the automatic ones if none are set yet).
    fn own_railways(doc: &super::doc::Doc, v: &View) -> Vec<Railway> {
        match &doc.data.railways {
            Some(r) => r.clone(),
            None => v.plan.auto_links.iter().map(|&(a, b)| Railway { level: 1, stops: vec![Self::inside(doc, a), Self::inside(doc, b)] }).collect(),
        }
    }

    fn own_hubs(doc: &super::doc::Doc, v: &View) -> Vec<[f32; 2]> {
        match &doc.data.supply_hubs {
            Some(h) => h.clone(),
            None => v.plan.hubs.iter().map(|&p| Self::inside(doc, p)).collect(),
        }
    }

    /// Change the railways (made editable first) as one undo step.
    fn edit_railways(&mut self, label: &str, f: impl FnOnce(&mut Vec<Railway>) -> anyhow::Result<String>) {
        let Some(v) = self.supply_view() else { return };
        self.run(label, |d, _| {
            let mut r = Self::own_railways(d, &v);
            let m = f(&mut r)?;
            d.data.railways = Some(r);
            Ok(m)
        });
    }

    fn edit_hubs(&mut self, label: &str, f: impl FnOnce(&mut Vec<[f32; 2]>, &super::doc::Doc) -> anyhow::Result<String>) {
        let Some(v) = self.supply_view() else { return };
        self.run(label, |d, _| {
            let mut h = Self::own_hubs(d, &v);
            let m = f(&mut h, d)?;
            d.data.supply_hubs = Some(h);
            Ok(m)
        });
    }

    fn finish_railway(&mut self) {
        let Some(stops) = self.su.draft.take() else { return };
        if stops.len() < 2 {
            self.error("a railway needs two stops or more");
            return;
        }
        let level = self.su.level;
        let mut idx = None;
        self.edit_railways("Lay railway", |r| {
            r.push(Railway { level, stops });
            idx = Some(r.len() - 1);
            Ok(format!("railway of level {level} laid"))
        });
        self.su.sel = idx.map(Pick::Railway);
        self.su.preview = None;
    }

    fn pick_supply(&mut self, dc: &DrawCtx, pl: &Placement, at: Pos2) -> Option<Pick> {
        let v = self.supply_view()?;
        let doc = self.doc.as_ref()?;
        let scr = |p: [f64; 2]| to_screen(dc, pl, p);
        let mut best: Option<(f32, Pick)> = None;
        let consider = |best: &mut Option<(f32, Pick)>, d: f32, p: Pick| {
            if d <= PICK && best.is_none_or(|b| d < b.0) {
                *best = Some((d, p));
            }
        };
        for (i, port) in doc.data.ports.iter().enumerate() {
            if let Some(s) = scr([port.at[0] as f64, port.at[1] as f64]) {
                consider(&mut best, s.distance(at) - 2.0, Pick::Port(i));
            }
        }
        for &h in &v.plan.hubs {
            if let Some(s) = scr(v.plan.net.centre[h as usize]) {
                consider(&mut best, s.distance(at) - 1.0, Pick::Hub(h));
            }
        }
        if best.is_none() {
            for (i, pieces) in v.plan.lines.pieces.iter().enumerate() {
                for piece in pieces {
                    let pts: Vec<Pos2> = piece.iter().filter_map(|&p| scr(v.plan.net.centre[p as usize])).collect();
                    for s in pts.windows(2) {
                        consider(&mut best, super::dist_to_segment(at, s[0], s[1]), Pick::Railway(i));
                    }
                }
            }
        }
        best.map(|b| b.1)
    }

    /// Map input on the Supply level: Railway lays a railway through the
    /// provinces clicked, Hub and Port add or remove one in the province
    /// clicked, Select picks.
    pub(super) fn supply_input(&mut self, dc: &DrawCtx, response: &egui::Response, pl: &Placement) -> bool {
        let to_px = |p: Pos2| pl.world_to_pixel(dc.cam.to_world(dc.rect, p));
        let click = response.interact_pointer_pos();
        let px = click.and_then(to_px);
        match self.settings.tool {
            Tool::Rail => {
                if response.double_clicked_by(PointerButton::Primary) || response.clicked_by(PointerButton::Secondary) {
                    self.finish_railway();
                } else if response.clicked_by(PointerButton::Primary) {
                    let Some(px) = px else { return true };
                    let ok = self.supply_view().zip(self.province_at(px)).is_some_and(|(v, p)| v.plan.net.ok(p));
                    if !ok {
                        self.error("railways run over passable land only");
                    } else {
                        self.su.draft.get_or_insert_with(Vec::new).push([px[0] as f32, px[1] as f32]);
                    }
                }
            }
            Tool::Hub => {
                if response.clicked_by(PointerButton::Primary) {
                    let (Some(px), Some(v)) = (px, self.supply_view()) else { return true };
                    let Some(p) = self.province_at(px).filter(|&p| v.plan.net.ok(p)) else {
                        self.error("supply hubs go on passable land");
                        return true;
                    };
                    let had = v.plan.hubs.contains(&p);
                    let name = self.doc.as_ref().map(|d| d.data.prov_name(p)).unwrap_or_default();
                    self.edit_hubs(if had { "Remove supply hub" } else { "Add supply hub" }, |h, d| {
                        let lab = d.labels.read();
                        let at = |q: &[f32; 2]| supply::at_point(&lab, *q);
                        if had {
                            h.retain(|q| at(q) != p);
                            Ok(format!("no supply hub in {name}"))
                        } else {
                            h.push([px[0] as f32, px[1] as f32]);
                            Ok(format!("supply hub in {name}"))
                        }
                    });
                    self.su.sel = (!had).then_some(Pick::Hub(p));
                }
            }
            Tool::Port => {
                if response.clicked_by(PointerButton::Primary) {
                    let Some(px) = px else { return true };
                    let Some(p) = self.province_at(px) else { return true };
                    let Some(doc) = self.doc.as_ref() else { return true };
                    if !supply::coastal(&doc.data, &doc.stats, p) {
                        self.error("ports go in land provinces on the sea");
                        return true;
                    }
                    let name = doc.data.prov_name(p);
                    let lab = doc.labels.read();
                    let inside: Vec<usize> = doc.data.ports.iter().enumerate().filter(|(_, q)| supply::at_point(&lab, q.at) == p).map(|(i, _)| i).collect();
                    drop(lab);
                    let lv = self.su.port_level;
                    if inside.is_empty() {
                        let mut idx = None;
                        self.run("Add port", |d, _| {
                            d.data.ports.push(Port { at: [px[0] as f32, px[1] as f32], level: lv });
                            idx = Some(d.data.ports.len() - 1);
                            Ok(format!("naval base (level {lv}) in {name}"))
                        });
                        self.su.sel = idx.map(Pick::Port);
                    } else {
                        self.run("Remove port", |d, _| {
                            for &i in inside.iter().rev() {
                                d.data.ports.remove(i);
                            }
                            Ok(format!("no naval base in {name}"))
                        });
                        self.su.sel = None;
                    }
                }
            }
            _ => {
                if response.clicked_by(PointerButton::Primary)
                    && let Some(at) = click
                {
                    self.su.sel = self.pick_supply(dc, pl, at);
                }
            }
        }
        true
    }

    pub(super) fn supply_keys(&mut self, ctx: &egui::Context) {
        let pressed = |k| ctx.input(|i| i.key_pressed(k) && !i.modifiers.command);
        if self.su.draft.is_some() {
            if pressed(Key::Enter) {
                self.finish_railway();
            }
            if pressed(Key::Backspace)
                && let Some(d) = &mut self.su.draft
            {
                d.pop();
            }
            if pressed(Key::Escape) {
                self.su.draft = None;
            }
        } else if pressed(Key::Delete) {
            self.delete_supply_sel();
        }
    }

    fn delete_supply_sel(&mut self) {
        match self.su.sel {
            Some(Pick::Railway(i)) => self.edit_railways("Delete railway", |r| {
                if i < r.len() {
                    r.remove(i);
                }
                Ok(String::new())
            }),
            Some(Pick::Hub(p)) => self.edit_hubs("Remove supply hub", |h, d| {
                let lab = d.labels.read();
                h.retain(|q| supply::at_point(&lab, *q) != p);
                Ok(String::new())
            }),
            Some(Pick::Port(i)) => self.run("Remove port", |d, _| {
                if i < d.data.ports.len() {
                    d.data.ports.remove(i);
                }
                Ok(String::new())
            }),
            None => return,
        }
        self.su.sel = None;
    }

    /// Railways, hubs, ports and capitals on the map.
    pub(super) fn draw_supply(&mut self, dc: &DrawCtx, pl: &Placement, px_scale: f64) {
        let Some(v) = self.supply_view() else { return };
        let est = self.supply_estimate();
        let Some(doc) = self.doc.as_ref() else { return };
        let net = &v.plan.net;
        let scr = |p: Id| to_screen(dc, pl, net.centre[p as usize]);
        let zoom = (px_scale as f32 / 3.0).clamp(0.7, 2.0);
        let auto = !v.plan.custom_rail;
        // the railways, by level
        for (&(a, b), &lv) in &v.plan.lines.level {
            if let (Some(s), Some(e)) = (scr(a), scr(b)) {
                if !dc.rect.expand(10.0).contains(s) && !dc.rect.expand(10.0).contains(e) {
                    continue;
                }
                let w = (1.0 + 0.7 * lv as f32) * zoom;
                dc.painter.line_segment([s, e], Stroke::new(w + 2.0, Color32::from_black_alpha(170)));
                let c = if auto { Color32::from_rgb(190, 190, 190) } else { Color32::from_rgb(240, 240, 240) };
                dc.painter.line_segment([s, e], Stroke::new(w, c));
            }
        }
        if let Some(Pick::Railway(i)) = self.su.sel {
            for piece in v.plan.lines.pieces.get(i).into_iter().flatten() {
                let pts: Vec<Pos2> = piece.iter().filter_map(|&p| scr(p)).collect();
                dc.painter.add(egui::Shape::line(pts, Stroke::new(3.0 * zoom + 1.0, SELECTED)));
            }
        }
        // the railway being laid, and on to the cursor
        if let Some(stops) = self.su.draft.clone() {
            let lab = doc.labels.read();
            let at = |q: [f32; 2]| supply::at_point(&lab, q);
            let provs: Vec<Id> = stops.iter().map(|&s| at(s)).collect();
            let hover = self.hover.map(|h| at([h[0] as f32, h[1] as f32])).unwrap_or(0);
            drop(lab);
            let key = (stops.clone(), hover);
            if self.su.preview.as_ref().is_none_or(|p| p.0 != key) {
                let mut route: Vec<Id> = Vec::new();
                for s in provs.iter().copied().chain(std::iter::once(hover).filter(|&h| h != 0)).collect::<Vec<_>>().windows(2) {
                    if let Some(r) = net.route(s[0], s[1]) {
                        route.extend(if route.last() == r.first() { &r[1..] } else { &r[..] });
                    }
                }
                if route.is_empty() {
                    route = provs.clone();
                }
                self.su.preview = Some((key, route));
            }
            let route = &self.su.preview.as_ref().unwrap().1;
            let pts: Vec<Pos2> = route.iter().filter_map(|&p| scr(p)).collect();
            dc.painter.add(egui::Shape::line(pts, Stroke::new(3.0, Color32::from_rgb(255, 160, 40))));
            for &s in &stops {
                if let Some(p) = to_screen(dc, pl, [s[0] as f64, s[1] as f64]) {
                    dc.painter.circle(p, 3.5, Color32::WHITE, Stroke::new(1.0, Color32::BLACK));
                }
            }
        }
        // capitals: gold rings (under the hubs)
        for &c in v.capital.iter().filter(|&&c| c != 0) {
            if let Some(p) = scr(c).filter(|p| dc.rect.contains(*p)) {
                dc.painter.circle_stroke(p, 5.5 * zoom, Stroke::new(3.0, Color32::BLACK));
                dc.painter.circle_stroke(p, 5.5 * zoom, Stroke::new(1.8, Color32::from_rgb(255, 205, 0)));
            }
        }
        // supply hubs: white when connected to the capital, red when not
        for (k, &h) in v.plan.hubs.iter().enumerate() {
            let Some(p) = scr(h) else { continue };
            if !dc.rect.contains(p) {
                continue;
            }
            let on = est.as_ref().is_none_or(|e| e.hubs.get(k).is_some_and(|x| x.is_some()));
            let r = 3.5 * zoom;
            let fill = if on { Color32::WHITE } else { Color32::from_rgb(230, 60, 60) };
            let sel = self.su.sel == Some(Pick::Hub(h));
            dc.painter.rect(egui::Rect::from_center_size(p, egui::vec2(2.0 * r, 2.0 * r)), 1.0, fill, Stroke::new(if sel { 2.5 } else { 1.0 }, if sel { SELECTED } else { Color32::BLACK }), egui::StrokeKind::Outside);
        }
        // ports
        for (i, port) in doc.data.ports.iter().enumerate() {
            let Some(p) = to_screen(dc, pl, [port.at[0] as f64, port.at[1] as f64]) else { continue };
            if !dc.rect.contains(p) {
                continue;
            }
            let prov = supply::at_point(&doc.labels.read(), port.at);
            let ok = !v.plan.bad_ports.contains(&i);
            let on = ok && est.as_ref().is_none_or(|e| e.ports.get(&prov).copied().unwrap_or(false));
            let fill = if !ok { Color32::from_rgb(230, 60, 60) } else if on { Color32::from_rgb(40, 110, 220) } else { Color32::from_rgb(110, 120, 140) };
            let sel = self.su.sel == Some(Pick::Port(i));
            let r = 6.0 * zoom.max(1.0);
            dc.painter.circle(p, r, fill, Stroke::new(if sel { 2.5 } else { 1.0 }, if sel { SELECTED } else { Color32::BLACK }));
            dc.painter.text(p, Align2::CENTER_CENTER, port.level.to_string(), FontId::proportional(r * 1.4), Color32::WHITE);
        }
    }

    /// Supply of a province for the hover panel.
    pub(super) fn supply_hover_rows(&self, ui: &mut egui::Ui, p: Id) {
        let Some((_, v)) = &self.su.view else { return };
        if let Some((_, e)) = &self.su.est {
            ui.label("supply");
            let f = e.flow.get(p as usize).copied().unwrap_or(0.0);
            ui.label(if f > 0.0 { format!("{f:.2}{}", if f >= 1.0 { " (enough for a division)" } else { "" }) } else { "none".into() });
            ui.end_row();
        }
        if let Some(t) = v.terrain.get(p as usize).filter(|t| !t.is_empty()) {
            ui.label("terrain");
            ui.label(format!("{t} (supply penalty {:+.0}%)", supply::terrain_penalty(t) * 100.0));
            ui.end_row();
        }
        ui.label("infrastructure");
        ui.label(v.infra.get(p as usize).copied().unwrap_or(0).to_string());
        ui.end_row();
        let lv = v.plan.lines.level.iter().filter(|(k, _)| k.0 == p || k.1 == p).map(|(_, l)| *l).max();
        let mut extra = Vec::new();
        if let Some(l) = lv {
            extra.push(format!("railway level {l}"));
        }
        if v.plan.hubs.contains(&p) {
            extra.push("supply hub".into());
        }
        if let Some(l) = v.plan.ports.get(&p) {
            extra.push(format!("naval base {l}"));
        }
        if v.links.along.iter().any(|k| k.0 == p || k.1 == p) {
            extra.push("on a river".into());
        }
        if !extra.is_empty() {
            ui.label("");
            ui.label(extra.join(" · "));
            ui.end_row();
        }
    }

    pub(super) fn supply_panel(&mut self, ui: &mut egui::Ui) {
        let Some(v) = self.supply_view() else { return };
        let est = self.supply_estimate();
        let Some(doc) = self.doc.as_ref() else { return };
        let d = &doc.data;
        let plan = &v.plan;
        let mut act: Option<Deferred> = None;
        let mut zoom: Option<[f64; 4]> = None;
        egui::CollapsingHeader::new("Supply").id_salt("supply").default_open(true).show(ui, |ui| {
            ui.label(
                RichText::new(match self.settings.tool {
                    Tool::Rail => "Click the provinces the railway stops in (it follows the shortest way over land between them); double-click, right-click or Enter ends it, Backspace takes back a stop, Esc cancels.",
                    Tool::Hub => "Click a province to add a supply hub there, or to remove the one there.",
                    Tool::Port => "Click a province on the sea to add a naval base (with the level below), or to remove the one there.",
                    _ => "Click a railway, hub or port to select it; Del removes it.",
                })
                .small()
                .weak(),
            );
            // railways
            ui.separator();
            let links = plan.lines.level.len();
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Railways").strong());
                if plan.custom_rail {
                    ui.label(format!("{} as laid ({links} links)", d.railways.as_ref().map_or(0, |r| r.len())));
                    if ui.button("Back to automatic").on_hover_text("drop the railways laid here; the build joins neighbouring states again").clicked() {
                        act = Some(Box::new(|e| e.run("Automatic railways", |d, _| {
                            d.data.railways = None;
                            Ok(String::new())
                        })));
                    }
                    if ui.button("Remove all").clicked() {
                        act = Some(Box::new(|e| e.run("Remove all railways", |d, _| {
                            d.data.railways = Some(Vec::new());
                            Ok(String::new())
                        })));
                    }
                } else {
                    ui.label(format!("automatic: {} ({links} links) between neighbouring states", plan.auto_links.len()))
                        .on_hover_text("a minimum spanning tree over neighbouring states, each the shortest way between their main victory points");
                    if ui.button("Make editable").on_hover_text("keep these railways to change them (also happens with the first change)").clicked() {
                        act = Some(Box::new(|e| e.edit_railways("Make railways editable", |_| Ok(String::new()))));
                    }
                }
            });
            ui.add(egui::Slider::new(&mut self.su.level, 1..=MAX_RAIL_LEVEL).text("level of new railways"));
            if let Some(Pick::Railway(i)) = self.su.sel {
                let cur = d.railways.as_ref().and_then(|r| r.get(i)).map_or(1, |r| r.level);
                let stops = d.railways.as_ref().and_then(|r| r.get(i)).map_or(2, |r| r.stops.len());
                let provs: usize = plan.lines.pieces.get(i).map_or(0, |p| p.iter().map(|x| x.len()).sum());
                let mut lv = cur;
                ui.horizontal(|ui| {
                    ui.label(format!("Selected: railway {} · {stops} stops · {provs} provinces", i + 1));
                    egui::ComboBox::from_id_salt("rail_level").selected_text(format!("level {lv}")).show_ui(ui, |ui| {
                        for l in 1..=MAX_RAIL_LEVEL {
                            ui.selectable_value(&mut lv, l, format!("level {l}"));
                        }
                    });
                    if lv != cur {
                        act = Some(Box::new(move |e| e.edit_railways("Railway level", |r| {
                            r[i].level = lv;
                            Ok(format!("railway {} is level {lv}", i + 1))
                        })));
                    }
                    if ui.button("🗑 Delete").clicked() {
                        act = Some(Box::new(|e| e.delete_supply_sel()));
                    }
                });
                for (_, m) in plan.lines.faults.iter().filter(|f| f.0 == i) {
                    ui.colored_label(WARN, m);
                }
            }
            // hubs
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Supply hubs").strong());
                if plan.custom_hubs {
                    ui.label(format!("{} as set", plan.hubs.len()));
                    if ui.button("Back to automatic").on_hover_text("one in the main victory point of each state").clicked() {
                        act = Some(Box::new(|e| e.run("Automatic supply hubs", |d, _| {
                            d.data.supply_hubs = None;
                            Ok(String::new())
                        })));
                    }
                    if ui.button("Remove all").clicked() {
                        act = Some(Box::new(|e| e.run("Remove all supply hubs", |d, _| {
                            d.data.supply_hubs = Some(Vec::new());
                            Ok(String::new())
                        })));
                    }
                } else {
                    ui.label(format!("automatic: {} (the main victory point of each state)", plan.hubs.len()));
                    if ui.button("Make editable").clicked() {
                        act = Some(Box::new(|e| e.edit_hubs("Make supply hubs editable", |_, _| Ok(String::new()))));
                    }
                }
            });
            if let Some(e) = &est {
                let off: Vec<Id> = plan.hubs.iter().zip(&e.hubs).filter(|(_, x)| x.is_none()).map(|(&h, _)| h).collect();
                if !off.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        ui.colored_label(WARN, format!("{} hubs get no supply (no railway or river to their capital):", off.len()));
                        for &h in off.iter().take(8) {
                            if ui.link(d.prov_name(h)).clicked() {
                                let c = plan.net.centre[h as usize];
                                zoom = Some([c[0] - 60.0, c[1] - 60.0, c[0] + 60.0, c[1] + 60.0]);
                                self.su.sel = Some(Pick::Hub(h));
                            }
                        }
                    });
                }
            }
            // ports
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Ports").strong());
                ui.label(format!("{} naval bases", plan.ports.len()));
                if ui.button("One per coastal state").on_hover_text("a naval base (level below) in the most built-up coastal province of each passable state on the sea without one").clicked() {
                    act = Some(Box::new(|e| e.ports_per_state()));
                }
                if ui.add_enabled(!d.ports.is_empty(), egui::Button::new("Remove all")).clicked() {
                    act = Some(Box::new(|e| e.run("Remove all ports", |d, _| {
                        d.data.ports.clear();
                        Ok(String::new())
                    })));
                }
            });
            ui.add(egui::Slider::new(&mut self.su.port_level, 1..=MAX_PORT_LEVEL).text("level of new ports"));
            if let Some(Pick::Port(i)) = self.su.sel
                && let Some(p) = d.ports.get(i)
            {
                let mut lv = p.level;
                ui.horizontal(|ui| {
                    ui.label(format!("Selected: port {}", i + 1));
                    egui::ComboBox::from_id_salt("port_level").selected_text(format!("level {lv}")).show_ui(ui, |ui| {
                        for l in 1..=MAX_PORT_LEVEL {
                            ui.selectable_value(&mut lv, l, format!("level {l}"));
                        }
                    });
                    if lv != p.level {
                        act = Some(Box::new(move |e| e.run("Port level", |d, _| {
                            d.data.ports[i].level = lv;
                            Ok(String::new())
                        })));
                    }
                    if ui.button("🗑 Delete").clicked() {
                        act = Some(Box::new(|e| e.delete_supply_sel()));
                    }
                });
            }
            for &i in &plan.bad_ports {
                ui.colored_label(WARN, format!("port {} is not on a coast: left out", i + 1));
            }

            // the estimate
            ui.separator();
            ui.label(RichText::new("Supply (estimate)").strong()).on_hover_text(
                "From the game's supply rules (common/defines): 5 at the capital, 2.8 at hubs joined to it by railway or river \
                 (+0.34 per level of the weakest railway on the way), 3.3 at naval bases once one is joined; less on each province \
                 further, more so in rough terrain and over rivers, less so with infrastructure. Motorization, damage and \
                 fleets are left out. 1 or more fully supplies a division.",
            );
            let mut one = matches!(self.su.scope, Scope::OneCountry(_));
            ui.horizontal_wrapped(|ui| {
                ui.radio_value(&mut one, false, "each nation from its capital");
                ui.radio_value(&mut one, true, "all as one country, capital of");
            });
            let biggest = d.nations.iter().max_by(|a, b| a.1.km2.total_cmp(&b.1.km2)).map_or(0, |n| n.0);
            let mut nat = match self.su.scope {
                Scope::OneCountry(n) if d.nations.contains(n) => n,
                _ => biggest,
            };
            if one {
                egui::ComboBox::from_id_salt("supply_capital").selected_text(d.nation_name(nat)).height(300.0).show_ui(ui, |ui| {
                    let mut list: Vec<(Id, &str)> = d.nations.iter().map(|(i, n)| (i, n.name.as_str())).collect();
                    list.sort_by_key(|x| x.1.to_lowercase());
                    for (i, name) in list {
                        ui.selectable_value(&mut nat, i, name);
                    }
                });
            }
            self.su.scope = if one { Scope::OneCountry(nat) } else { Scope::EachNation };
            if let Some(e) = &est {
                let land: Vec<usize> = (0..plan.net.passable.len()).filter(|&p| plan.net.passable[p]).collect();
                let full = land.iter().filter(|&&p| e.flow[p] >= 1.0).count();
                let some = land.iter().filter(|&&p| e.flow[p] > 0.0).count();
                ui.label(format!(
                    "{:.0}% of the passable provinces fully supplied, {:.0}% some, {:.0}% none",
                    100.0 * full as f64 / land.len().max(1) as f64,
                    100.0 * (some - full) as f64 / land.len().max(1) as f64,
                    100.0 * (land.len() - some) as f64 / land.len().max(1) as f64
                ));
            }
            ui.horizontal(|ui| {
                for (f, t) in [(0.0, "none"), (0.5, "0.5"), (1.0, "1"), (2.0, "2"), (3.0, "3+")] {
                    let c = supply_colour(f);
                    ui.label(RichText::new("■").color(Color32::from_rgb(c[0], c[1], c[2])));
                    ui.label(RichText::new(t).small());
                }
            });
            if self.settings.mode.effective(self.settings.level) != super::MapMode::Supply {
                ui.label(RichText::new("(colour the map by \"supply\" to see it)").small().weak());
            }
            if doc.terrain.is_none() {
                ui.colored_label(WARN, "the land cover is not loaded: no terrain, built-up land or automatic victory points yet");
            }
        });
        if let Some(b) = zoom {
            self.zoom_to = Some(b);
        }
        if let Some(f) = act {
            f(self);
        }
    }

    /// A naval base in the most built-up coastal province of each passable
    /// state on the sea that has none.
    fn ports_per_state(&mut self) {
        let Some(v) = self.supply_view() else { return };
        let Some(doc) = self.doc.as_ref() else { return };
        let urban = self.su.urban.as_ref().map(|t| t.2.clone()).unwrap_or_default();
        let d = &doc.data;
        let mut best: std::collections::BTreeMap<Id, (Id, f64, u32)> = Default::default();
        for (p, pr) in d.provinces.iter() {
            if pr.kind != Kind::Land || !v.plan.net.ok(p) || !supply::coastal(d, &doc.stats, p) {
                continue;
            }
            let key = (urban.get(p as usize).copied().unwrap_or(0.0), doc.stats.get(p).px);
            let e = best.entry(pr.state).or_insert((p, key.0, key.1));
            if (key.0, key.1) > (e.1, e.2) {
                *e = (p, key.0, key.1);
            }
        }
        let has: std::collections::BTreeSet<Id> = v.plan.ports.keys().map(|&p| d.state_of(p)).collect();
        let new: Vec<[f32; 2]> = best.iter().filter(|(s, _)| !has.contains(s)).map(|(_, &(p, _, _))| Self::inside(doc, p)).collect();
        let lv = self.su.port_level;
        self.run("Ports per coastal state", |d, _| {
            let n = new.len();
            d.data.ports.extend(new.into_iter().map(|at| Port { at, level: lv }));
            Ok(format!("{n} naval bases added"))
        });
    }
}
