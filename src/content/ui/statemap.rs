//! A map of the states of the open map, to pick states by clicking them.
//! Each state has its nation's colour (the nation being edited bright, the
//! others faded), the picked ones are lit up and the one under the mouse is
//! named. The wheel zooms, dragging moves the map; it opens on the nation's
//! own states.

use std::collections::HashMap;
use std::sync::Arc;

use eframe::egui::{self, Color32, Pos2, Rect, RichText, Vec2};

use super::Cx;
use crate::editor::{MapFacts, WATER};

/// The longer side of the map in cells.
const MAX_SIDE: usize = 1600;

const SEA: [u8; 3] = [42, 72, 120];
const NO_STATE: [u8; 3] = [86, 86, 82];
const PICKED: [u8; 3] = [255, 196, 40];

/// The states of the map at a lower resolution.
pub struct Raster {
    pub w: usize,
    pub h: usize,
    /// The state of each cell (row-major); 0: none, [`WATER`]: water.
    pub cells: Vec<u32>,
    /// The cell has a neighbour of another state to its right or below.
    border: Vec<bool>,
    /// By state: the box of its cells (x0, y0, x1, y1) and the middle of its cells.
    pub boxes: HashMap<u32, ([f32; 4], Pos2)>,
    /// What it was made from: the map's version and pixels.
    key: (u64, usize),
}

fn key_of(f: &MapFacts) -> (u64, usize) {
    (f.version, f.pixels.labels.as_ref().map_or(0, |l| Arc::as_ptr(l) as usize))
}

/// The states of the map at no more than [`MAX_SIDE`] cells a side (each
/// cell takes the pixel in its middle).
pub fn raster(f: &MapFacts) -> Option<Raster> {
    let labels = f.pixels.labels.as_ref()?;
    let lab = labels.read();
    let (pw, ph) = (lab.w as usize, lab.h as usize);
    if pw == 0 || ph == 0 {
        return None;
    }
    let step = pw.max(ph).div_ceil(MAX_SIDE).max(1);
    let (w, h) = (pw.div_ceil(step), ph.div_ceil(step));
    let ps = &f.pixels.province_state;
    let mut cells = vec![0u32; w * h];
    for y in 0..h {
        let sy = (y * step + step / 2).min(ph - 1);
        for x in 0..w {
            let sx = (x * step + step / 2).min(pw - 1);
            let p = lab.px[sy * pw + sx] as usize;
            cells[y * w + x] = ps.get(p).copied().unwrap_or(0);
        }
    }
    drop(lab);
    let mut border = vec![false; w * h];
    // (x0, y0, x1, y1, sum x, sum y, n)
    let mut acc: HashMap<u32, ([f32; 4], f64, f64, f64)> = HashMap::new();
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let s = cells[i];
            border[i] = (x + 1 < w && cells[i + 1] != s) || (y + 1 < h && cells[i + w] != s);
            if s != 0 && s != WATER {
                let e = acc.entry(s).or_insert(([x as f32, y as f32, x as f32, y as f32], 0.0, 0.0, 0.0));
                e.0 = [e.0[0].min(x as f32), e.0[1].min(y as f32), e.0[2].max(x as f32 + 1.0), e.0[3].max(y as f32 + 1.0)];
                e.1 += x as f64 + 0.5;
                e.2 += y as f64 + 0.5;
                e.3 += 1.0;
            }
        }
    }
    let boxes = acc.into_iter().map(|(s, (b, sx, sy, n))| (s, (b, Pos2::new((sx / n) as f32, (sy / n) as f32)))).collect();
    Some(Raster { w, h, cells, border, boxes, key: key_of(f) })
}

/// The raster of the facts, kept between frames.
fn raster_of(ctx: &egui::Context, f: &MapFacts) -> Option<Arc<Raster>> {
    let id = egui::Id::unique("state_map_raster");
    let have: Option<Arc<Raster>> = ctx.data(|d| d.get_temp(id));
    if let Some(r) = have.filter(|r| r.key == key_of(f)) {
        return Some(r);
    }
    let r = Arc::new(raster(f)?);
    ctx.data_mut(|d| d.insert_temp(id, r.clone()));
    Some(r)
}

/// Where the map looks: its middle in cells and screen points per cell.
#[derive(Clone, Copy, Debug)]
struct View {
    centre: Pos2,
    scale: f32,
}

/// The view that shows `b` (x0, y0, x1, y1 in cells) in `size`, with a margin.
fn fit(b: [f32; 4], size: Vec2) -> View {
    let (bw, bh) = ((b[2] - b[0]).max(4.0), (b[3] - b[1]).max(4.0));
    let scale = (size.x / (bw * 1.15)).min(size.y / (bh * 1.15));
    View { centre: Pos2::new((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0), scale }
}

/// The box of these states (None: none of them is on the map).
fn box_of(r: &Raster, states: impl IntoIterator<Item = u32>) -> Option<[f32; 4]> {
    states.into_iter().filter_map(|s| r.boxes.get(&s)).map(|x| x.0).reduce(|a, b| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])])
}

fn mix(c: [u8; 3], to: [u8; 3], t: f32) -> [u8; 3] {
    [0, 1, 2].map(|i| (c[i] as f32 + (to[i] as f32 - c[i] as f32) * t).round() as u8)
}

/// The map; returns the state clicked. `own`: the states of the nation
/// being edited (bright; the rest faded), `picked`: lit up.
pub fn map(ui: &mut egui::Ui, cx: &Cx, id: egui::Id, own: &[u32], picked: &[u32], size: Vec2) -> Option<u32> {
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, Color32::from_rgb(SEA[0] / 2, SEA[1] / 2, SEA[2] / 2));
    let Some(r) = raster_of(ui.ctx(), cx.facts) else {
        painter.text(rect.center(), egui::Align2::CENTER_CENTER, "no map is open", egui::FontId::proportional(14.0), Color32::WHITE);
        return None;
    };
    let vid = id.with("view");
    let whole = [0.0, 0.0, r.w as f32, r.h as f32];
    let mut v: View = ui.data(|d| d.get_temp(vid)).unwrap_or_else(|| fit(box_of(&r, picked.iter().chain(own).copied()).unwrap_or(whole), size));
    // the wheel zooms about the mouse, dragging moves
    if resp.hovered() {
        let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
        let k = zoom * (scroll * 0.003).exp();
        if (k - 1.0).abs() > 1e-4
            && let Some(m) = resp.hover_pos()
        {
            let at = v.centre + (m - rect.center()) / v.scale;
            let min = (size.x / r.w as f32).min(size.y / r.h as f32) * 0.8;
            v.scale = (v.scale * k).clamp(min, 40.0);
            v.centre = at - (m - rect.center()) / v.scale;
            ui.input_mut(|i| i.smooth_scroll_delta = Vec2::ZERO);
        }
    }
    if resp.dragged() {
        v.centre -= resp.drag_delta() / v.scale;
    }
    v.centre = v.centre.clamp(Pos2::ZERO, Pos2::new(r.w as f32, r.h as f32));
    let to_screen = |c: Pos2| rect.center() + (c - v.centre) * v.scale;
    let to_cell = |p: Pos2| v.centre + (p - rect.center()) / v.scale;
    let state_at = |p: Pos2| {
        let c = to_cell(p);
        (c.x >= 0.0 && c.y >= 0.0 && (c.x as usize) < r.w && (c.y as usize) < r.h).then(|| r.cells[c.y as usize * r.w + c.x as usize]).filter(|&s| s != 0 && s != WATER)
    };
    let hover = resp.hover_pos().and_then(state_at);

    // the picture, again when what is lit up changes
    let tid = id.with("texture");
    let mut lit: Vec<u32> = picked.to_vec();
    lit.sort_unstable();
    let drawn = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (r.key, &lit, own, hover).hash(&mut h);
        h.finish()
    };
    let have: Option<(u64, egui::TextureHandle)> = ui.data(|d| d.get_temp(tid));
    let tex = match have {
        Some((k, t)) if k == drawn => t,
        have => {
            let img = picture(cx.facts, &r, own, &lit, hover);
            let t = match have {
                Some((_, mut t)) => {
                    t.set(img, egui::TextureOptions::NEAREST);
                    t
                }
                None => ui.ctx().load_texture("state map", img, egui::TextureOptions::NEAREST),
            };
            ui.data_mut(|d| d.insert_temp(tid, (drawn, t.clone())));
            t
        }
    };
    let full = Rect::from_min_max(to_screen(Pos2::ZERO), to_screen(Pos2::new(r.w as f32, r.h as f32)));
    painter.image(tex.id(), full, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);

    // names of the states that are big enough on the screen
    let font = egui::FontId::proportional(11.0);
    for (s, (b, mid)) in &r.boxes {
        let wide = (b[2] - b[0]) * v.scale;
        let p = to_screen(*mid);
        if wide < 70.0 || !rect.contains(p) {
            continue;
        }
        // the whole name when it fits, else its first part
        let name = cx.facts.state_names.get(s).map(String::as_str).unwrap_or("");
        let fits = |t: &str| ui.fonts_mut(|f| f.layout_no_wrap(t.to_string(), font.clone(), Color32::WHITE).size().x) < wide - 6.0;
        let short = if fits(name) { name.to_string() } else { name.split(" - ").next().unwrap_or(name).chars().take(18).collect() };
        painter.text(p + Vec2::new(1.0, 1.0), egui::Align2::CENTER_CENTER, &short, font.clone(), Color32::from_black_alpha(200));
        painter.text(p, egui::Align2::CENTER_CENTER, &short, font.clone(), Color32::WHITE);
    }
    ui.data_mut(|d| d.insert_temp(vid, v));
    let mut picked_now = None;
    if let Some(s) = hover {
        let resp = resp.on_hover_ui_at_pointer(|ui| {
            ui.label(RichText::new(state_label(cx.facts, s)).strong());
            ui.label(RichText::new("click: pick it").weak().small());
        });
        if resp.clicked() {
            picked_now = Some(s);
        }
    }
    picked_now
}

/// "12 Bozen (BOZ Bolzano)": the state, its nation's tag and name.
pub fn state_label(f: &MapFacts, s: u32) -> String {
    let name = f.state_names.get(&s).cloned().unwrap_or_default();
    let nation = f.pixels.state_nation.get(&s).and_then(|n| f.nations.iter().find(|x| x.id == *n));
    match nation {
        Some(n) => format!("{s} {name} ({} {})", n.tag, n.name),
        None => format!("{s} {name}"),
    }
}

/// The map's picture: states in their nation's colour (`own` bright, the
/// others faded), borders darker, `lit` lit up, `hover` lighter.
fn picture(f: &MapFacts, r: &Raster, own: &[u32], lit: &[u32], hover: Option<u32>) -> egui::ColorImage {
    let colour_of: HashMap<u32, [u8; 3]> = f.nations.iter().map(|n| (n.id, n.colour)).collect();
    // by state id (an index: this runs for every cell)
    let top = r.boxes.keys().copied().max().unwrap_or(0) as usize;
    let mut by_state: Vec<[u8; 3]> = vec![NO_STATE; top + 1];
    for &s in r.boxes.keys() {
        let base = f.pixels.state_nation.get(&s).and_then(|n| colour_of.get(n)).copied().unwrap_or(NO_STATE);
        let mut c = if own.is_empty() || own.contains(&s) { base } else { mix(base, [150, 150, 145], 0.6) };
        if lit.binary_search(&s).is_ok() {
            c = PICKED;
        }
        if hover == Some(s) {
            c = mix(c, [255, 255, 255], 0.45);
        }
        by_state[s as usize] = c;
    }
    let px: Vec<Color32> = r
        .cells
        .iter()
        .zip(&r.border)
        .map(|(&s, &edge)| {
            let c = match s {
                WATER => SEA,
                0 => NO_STATE,
                s => by_state.get(s as usize).copied().unwrap_or(NO_STATE),
            };
            let c = if edge { mix(c, [20, 20, 20], 0.55) } else { c };
            Color32::from_rgb(c[0], c[1], c[2])
        })
        .collect();
    egui::ColorImage::new([r.w, r.h], px)
}

/// A 🗺 button that opens the map (`open`: opens it now); the state clicked on it.
pub fn button(ui: &mut egui::Ui, cx: &Cx, id: egui::Id, current: &str, open: bool) -> Option<u32> {
    let resp = ui.small_button("🗺").on_hover_text("pick the state on the map");
    if open {
        egui::Popup::open_id(ui.ctx(), egui::Popup::default_response_id(&resp));
    }
    let mut out = None;
    egui::Popup::from_toggle_button_response(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        let own: Vec<u32> = cx.nation.map(|n| n.states.clone()).unwrap_or_default();
        let picked: Vec<u32> = current.trim().parse().ok().into_iter().collect();
        ui.horizontal(|ui| {
            ui.label(RichText::new("Click a state to pick it. Wheel: zoom, drag: move.").weak().small());
            if ui.small_button("whole map").clicked() {
                ui.data_mut(|d| d.remove::<View>(id.with("view")));
                if let Some(r) = raster_of(ui.ctx(), cx.facts) {
                    ui.data_mut(|d| d.insert_temp(id.with("view"), fit([0.0, 0.0, r.w as f32, r.h as f32], Vec2::new(640.0, 420.0))));
                }
            }
            if !own.is_empty() && ui.small_button("the nation's states").clicked() {
                ui.data_mut(|d| d.remove::<View>(id.with("view")));
            }
        });
        if let Some(s) = map(ui, cx, id, &own, &picked, Vec2::new(640.0, 420.0)) {
            out = Some(s);
            ui.close();
        }
        if let Some(s) = picked.first() {
            ui.label(RichText::new(format!("now: {}", state_label(cx.facts, *s))).small());
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::StatePixels;
    use crate::editor::model::Labels;
    use parking_lot::RwLock;

    /// A 4000×1000 map: province 1 (state 5) on the left, province 2 (state
    /// 6) on the right, a strip of sea (province 3) at the bottom.
    fn facts() -> MapFacts {
        let (w, h) = (4000u32, 1000u32);
        let px = (0..w * h).map(|i| if i / w >= 900 { 3 } else if i % w < 1000 { 1 } else { 2 }).collect();
        let labels = Labels { w, h, px };
        MapFacts { pixels: StatePixels { labels: Some(Arc::new(RwLock::new(labels))), province_state: vec![0, 5, 6, WATER], ..Default::default() }, version: 7, ..Default::default() }
    }

    #[test]
    fn the_states_at_a_lower_resolution() {
        let r = raster(&facts()).unwrap();
        assert_eq!((r.w, r.h), (1334, 334));
        assert_eq!(r.cells[0], 5);
        assert_eq!(r.cells[1000], 6);
        assert_eq!(r.cells[r.w * 333], WATER);
        let (b5, mid5) = r.boxes[&5];
        assert_eq!(b5[0], 0.0);
        assert!((b5[2] - 333.0).abs() <= 1.0 && (b5[3] - 300.0).abs() <= 1.0, "{b5:?}");
        assert!((mid5.x - b5[2] / 2.0).abs() < 1.0);
        assert!(!r.boxes.contains_key(&WATER));
        // borders between the states and above the sea
        assert!(r.border[332] && !r.border[100] && r.border[r.w * 299 + 10]);
        // the box of both states, and a view that fits it
        let b = box_of(&r, [5, 6]).unwrap();
        assert_eq!(b[0], 0.0);
        assert!(b[2] > 1330.0);
        let v = fit(b, Vec2::new(640.0, 420.0));
        assert!(v.scale > 0.3 && v.scale < 0.5, "{v:?}");
    }

    #[test]
    fn colours_of_the_picture() {
        let f = facts();
        let r = raster(&f).unwrap();
        let img = picture(&f, &r, &[], &[6], None);
        let at = |x: usize, y: usize| img.pixels[y * r.w + x];
        assert_eq!(at(1000, 100), Color32::from_rgb(PICKED[0], PICKED[1], PICKED[2]));
        assert_eq!(at(100, 100), Color32::from_rgb(NO_STATE[0], NO_STATE[1], NO_STATE[2]));
        assert_eq!(at(100, 333), Color32::from_rgb(SEA[0], SEA[1], SEA[2]));
    }
}
