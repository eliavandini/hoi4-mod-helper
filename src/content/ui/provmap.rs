//! A map of the provinces of the open map, to place things in provinces
//! (the Army page's divisions). Each province has its nation's colour (the
//! nation being edited bright, the others faded); state borders are dark,
//! province borders faint. Markers (a box with a short text) stand at the
//! middle of their province. The wheel zooms, dragging moves; it opens on
//! the nation's own states.

use std::collections::HashMap;
use std::sync::Arc;

use eframe::egui::{self, Color32, Pos2, Rect, RichText, Vec2};

use super::Cx;
use super::statemap::{NO_STATE, PICKED, SEA, View, fit, mix, steer};
use crate::editor::{MapFacts, WATER};

/// The longer side of the map in cells.
const MAX_SIDE: usize = 1800;
/// The least the map opens on, in cells.
const MIN_VIEW: f32 = 220.0;

/// The provinces of the map at a lower resolution.
pub struct Raster {
    pub w: usize,
    pub h: usize,
    /// The province of each cell (row-major); 0: none.
    pub cells: Vec<u32>,
    /// The cell's right or lower neighbour is another province (1) or
    /// another state (2).
    edge: Vec<u8>,
    /// By province: the middle of its cells.
    pub mids: HashMap<u32, Pos2>,
    /// By state: the box of its cells (x0, y0, x1, y1).
    state_boxes: HashMap<u32, [f32; 4]>,
    key: (u64, usize),
}

fn key_of(f: &MapFacts) -> (u64, usize) {
    (f.version, f.pixels.labels.as_ref().map_or(0, |l| Arc::as_ptr(l) as usize))
}

fn state_of(f: &MapFacts, p: u32) -> u32 {
    f.pixels.province_state.get(p as usize).copied().unwrap_or(0)
}

/// The provinces of the map at no more than [`MAX_SIDE`] cells a side.
pub fn raster(f: &MapFacts) -> Option<Raster> {
    let labels = f.pixels.labels.as_ref()?;
    let lab = labels.read();
    let (pw, ph) = (lab.w as usize, lab.h as usize);
    if pw == 0 || ph == 0 {
        return None;
    }
    let step = pw.max(ph).div_ceil(MAX_SIDE).max(1);
    let (w, h) = (pw.div_ceil(step), ph.div_ceil(step));
    let mut cells = vec![0u32; w * h];
    for y in 0..h {
        let sy = (y * step + step / 2).min(ph - 1);
        for x in 0..w {
            let sx = (x * step + step / 2).min(pw - 1);
            cells[y * w + x] = lab.px[sy * pw + sx];
        }
    }
    drop(lab);
    let mut edge = vec![0u8; w * h];
    let mut acc: HashMap<u32, (f64, f64, f64)> = HashMap::new();
    let mut state_boxes: HashMap<u32, [f32; 4]> = HashMap::new();
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let p = cells[i];
            let s = state_of(f, p);
            let other = |q: u32| if q == p { 0 } else if state_of(f, q) != s { 2 } else { 1 };
            let mut e = 0;
            if x + 1 < w {
                e = e.max(other(cells[i + 1]));
            }
            if y + 1 < h {
                e = e.max(other(cells[i + w]));
            }
            edge[i] = e;
            if p != 0 && s != WATER {
                let a = acc.entry(p).or_insert((0.0, 0.0, 0.0));
                a.0 += x as f64 + 0.5;
                a.1 += y as f64 + 0.5;
                a.2 += 1.0;
                if s != 0 {
                    let b = state_boxes.entry(s).or_insert([x as f32, y as f32, x as f32 + 1.0, y as f32 + 1.0]);
                    *b = [b[0].min(x as f32), b[1].min(y as f32), b[2].max(x as f32 + 1.0), b[3].max(y as f32 + 1.0)];
                }
            }
        }
    }
    let mids = acc.into_iter().map(|(p, (sx, sy, n))| (p, Pos2::new((sx / n) as f32, (sy / n) as f32))).collect();
    Some(Raster { w, h, cells, edge, mids, state_boxes, key: key_of(f) })
}

/// The raster of the facts, kept between frames.
fn raster_of(ctx: &egui::Context, f: &MapFacts) -> Option<Arc<Raster>> {
    let id = egui::Id::unique("province_map_raster");
    let have: Option<Arc<Raster>> = ctx.data(|d| d.get_temp(id));
    if let Some(r) = have.filter(|r| r.key == key_of(f)) {
        return Some(r);
    }
    let r = Arc::new(raster(f)?);
    ctx.data_mut(|d| d.insert_temp(id, r.clone()));
    Some(r)
}

/// A box with a short text at the middle of a province.
#[derive(Clone, Debug)]
pub struct Marker {
    pub province: u32,
    pub text: String,
    /// Drawn lit up (the selected province's).
    pub lit: bool,
}

/// What was done on the map.
#[derive(Default)]
pub struct Out {
    /// A land province clicked (or its marker).
    pub clicked: Option<u32>,
    /// A land province right-clicked.
    pub secondary: Option<u32>,
}

/// "province 932 Marling (state 555 Marling - Marlengo, MRL)".
pub fn province_label(f: &MapFacts, p: u32) -> String {
    let name = f.pixels.province_names.get(p as usize).map(String::as_str).unwrap_or("");
    let s = state_of(f, p);
    let state = match s {
        0 => "no state".to_string(),
        WATER => "water".to_string(),
        s => {
            let tag = f.pixels.state_nation.get(&s).and_then(|n| f.nations.iter().find(|x| x.id == *n)).map(|n| format!(", {}", n.tag)).unwrap_or_default();
            format!("state {s} {}{tag}", f.state_names.get(&s).map(String::as_str).unwrap_or(""))
        }
    };
    if name.is_empty() { format!("province {p} ({state})") } else { format!("province {p} {name} ({state})") }
}

/// The map. `own`: the states of the nation being edited (bright, the rest
/// faded; the map opens on them), `lit`: provinces lit up, `hover`: lines
/// about a province under the mouse, `hint`: what a click does.
#[allow(clippy::too_many_arguments)]
pub fn map(ui: &mut egui::Ui, cx: &Cx, id: egui::Id, own: &[u32], lit: &[u32], markers: &[Marker], hover: &dyn Fn(u32) -> Vec<String>, hint: &str, size: Vec2) -> Out {
    let mut out = Out::default();
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, Color32::from_rgb(SEA[0] / 2, SEA[1] / 2, SEA[2] / 2));
    let Some(r) = raster_of(ui.ctx(), cx.facts) else {
        painter.text(rect.center(), egui::Align2::CENTER_CENTER, "no map is open: open it in the map editor", egui::FontId::proportional(14.0), Color32::WHITE);
        return out;
    };
    let vid = id.with("view");
    let whole = [0.0, 0.0, r.w as f32, r.h as f32];
    let own_box = own.iter().filter_map(|s| r.state_boxes.get(s)).copied().reduce(|a, b| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]);
    // a small nation is shown with its neighbours
    let at_least = |b: [f32; 4]| {
        let (cx_, cy_) = ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
        let half = ((b[2] - b[0]).max(b[3] - b[1]) / 2.0).max(MIN_VIEW / 2.0);
        [cx_ - half, cy_ - half, cx_ + half, cy_ + half]
    };
    let mut v: View = ui.data(|d| d.get_temp(vid)).unwrap_or_else(|| fit(own_box.map(at_least).unwrap_or(whole), size));
    steer(ui, &resp, rect, &mut v, r.w, r.h);
    let to_screen = |c: Pos2| rect.center() + (c - v.centre) * v.scale;
    let to_cell = |p: Pos2| v.centre + (p - rect.center()) / v.scale;
    let land = |p: u32| p != 0 && !matches!(state_of(cx.facts, p), WATER);
    let province_at = |p: Pos2| {
        let c = to_cell(p);
        (c.x >= 0.0 && c.y >= 0.0 && (c.x as usize) < r.w && (c.y as usize) < r.h).then(|| r.cells[c.y as usize * r.w + c.x as usize]).filter(|&p| land(p))
    };
    // the markers on the screen (a marker under the mouse wins over the province)
    let font = egui::FontId::proportional(11.0);
    let boxes: Vec<(Rect, &Marker)> = markers
        .iter()
        .filter_map(|m| {
            let at = to_screen(*r.mids.get(&m.province)?);
            let wide = ui.fonts_mut(|f| f.layout_no_wrap(m.text.clone(), font.clone(), Color32::WHITE).size().x) + 10.0;
            let b = Rect::from_center_size(at, Vec2::new(wide.max(18.0), 16.0));
            rect.intersects(b).then_some((b, m))
        })
        .collect();
    let hovered = resp.hover_pos().and_then(|m| boxes.iter().rev().find(|(b, _)| b.contains(m)).map(|(_, mk)| mk.province).or_else(|| province_at(m)));

    // the picture, again when what is lit up changes
    let tid = id.with("texture");
    let mut lit: Vec<u32> = lit.to_vec();
    lit.sort_unstable();
    let drawn = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (r.key, &lit, own, hovered).hash(&mut h);
        h.finish()
    };
    let have: Option<(u64, egui::TextureHandle)> = ui.data(|d| d.get_temp(tid));
    let tex = match have {
        Some((k, t)) if k == drawn => t,
        have => {
            let img = picture(cx.facts, &r, own, &lit, hovered);
            let t = match have {
                Some((_, mut t)) => {
                    t.set(img, egui::TextureOptions::NEAREST);
                    t
                }
                None => ui.ctx().load_texture("province map", img, egui::TextureOptions::NEAREST),
            };
            ui.data_mut(|d| d.insert_temp(tid, (drawn, t.clone())));
            t
        }
    };
    let full = Rect::from_min_max(to_screen(Pos2::ZERO), to_screen(Pos2::new(r.w as f32, r.h as f32)));
    painter.image(tex.id(), full, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
    for (b, m) in &boxes {
        let (fill, text) = if m.lit { (Color32::from_rgb(PICKED[0], PICKED[1], PICKED[2]), Color32::BLACK) } else { (Color32::from_rgb(52, 64, 40), Color32::WHITE) };
        painter.rect_filled(*b, 3.0, fill);
        painter.rect_stroke(*b, 3.0, egui::Stroke::new(1.0, Color32::from_black_alpha(220)), egui::StrokeKind::Inside);
        painter.text(b.center(), egui::Align2::CENTER_CENTER, &m.text, font.clone(), text);
    }
    ui.data_mut(|d| d.insert_temp(vid, v));
    if let Some(p) = hovered {
        let resp = resp.on_hover_ui_at_pointer(|ui| {
            ui.label(RichText::new(province_label(cx.facts, p)).strong());
            for l in hover(p) {
                ui.label(l);
            }
            ui.label(RichText::new(hint).weak().small());
        });
        if resp.clicked() {
            out.clicked = Some(p);
        }
        if resp.secondary_clicked() {
            out.secondary = Some(p);
        }
    }
    out
}

/// The view of a map widget set to the whole map (or, `None`, back to the
/// nation's states).
pub fn reset_view(ui: &egui::Ui, id: egui::Id, whole: Option<Vec2>) {
    ui.data_mut(|d| d.remove::<View>(id.with("view")));
    if let (Some(size), Some(r)) = (whole, ui.data(|d| d.get_temp::<Arc<Raster>>(egui::Id::unique("province_map_raster")))) {
        ui.data_mut(|d| d.insert_temp(id.with("view"), fit([0.0, 0.0, r.w as f32, r.h as f32], size)));
    }
}

/// The map's picture: provinces in their state's nation's colour (`own`
/// bright, the others faded), state borders dark, province borders faint,
/// `lit` lit up, `hover` lighter.
fn picture(f: &MapFacts, r: &Raster, own: &[u32], lit: &[u32], hover: Option<u32>) -> egui::ColorImage {
    let colour_of: HashMap<u32, [u8; 3]> = f.nations.iter().map(|n| (n.id, n.colour)).collect();
    // by state, then by province (indexes: this runs for every cell)
    let ps = &f.pixels.province_state;
    let mut by_province: Vec<[u8; 3]> = vec![NO_STATE; ps.len().max(1)];
    for (p, &s) in ps.iter().enumerate() {
        let mut c = match s {
            WATER => SEA,
            0 => NO_STATE,
            s => {
                let base = f.pixels.state_nation.get(&s).and_then(|n| colour_of.get(n)).copied().unwrap_or(NO_STATE);
                if own.is_empty() || own.contains(&s) { base } else { mix(base, [150, 150, 145], 0.6) }
            }
        };
        if lit.binary_search(&(p as u32)).is_ok() {
            c = PICKED;
        }
        if hover == Some(p as u32) {
            c = mix(c, [255, 255, 255], 0.45);
        }
        by_province[p] = c;
    }
    let px: Vec<Color32> = r
        .cells
        .iter()
        .zip(&r.edge)
        .map(|(&p, &e)| {
            let c = by_province.get(p as usize).copied().unwrap_or(NO_STATE);
            let c = match e {
                2 => mix(c, [20, 20, 20], 0.6),
                1 => mix(c, [20, 20, 20], 0.22),
                _ => c,
            };
            Color32::from_rgb(c[0], c[1], c[2])
        })
        .collect();
    egui::ColorImage::new([r.w, r.h], px)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::StatePixels;
    use crate::editor::model::Labels;
    use parking_lot::RwLock;

    /// A 4000×1000 map: provinces 1 and 2 (state 5) on the left, 3 (state
    /// 6) on the right, a strip of sea (province 4) at the bottom.
    fn facts() -> MapFacts {
        let (w, h) = (4000u32, 1000u32);
        let px = (0..w * h).map(|i| if i / w >= 900 { 4 } else if i % w < 500 { 1 } else if i % w < 1000 { 2 } else { 3 }).collect();
        let labels = Labels { w, h, px };
        MapFacts {
            pixels: StatePixels { labels: Some(Arc::new(RwLock::new(labels))), province_state: vec![0, 5, 5, 6, WATER], province_names: vec![String::new(), "Meran".into(), String::new(), String::new(), String::new()], ..Default::default() },
            state_names: [(5, "Burggrafenamt".to_string()), (6, "Vinschgau".to_string())].into(),
            version: 7,
            ..Default::default()
        }
    }

    #[test]
    fn provinces_at_a_lower_resolution() {
        let f = facts();
        let r = raster(&f).unwrap();
        assert_eq!((r.w, r.h), (1334, 334));
        assert_eq!((r.cells[0], r.cells[200], r.cells[400], r.cells[r.w * 333]), (1, 2, 3, 4));
        // the border of provinces faint, of states and the sea dark
        assert_eq!(r.edge[166], 1);
        assert_eq!(r.edge[332], 2);
        assert_eq!(r.edge[100], 0);
        assert_eq!(r.edge[r.w * 299 + 10], 2);
        // the middles of land provinces, not of the sea
        let m1 = r.mids[&1];
        assert!((m1.x - 83.5).abs() < 1.0 && (m1.y - 150.0).abs() < 1.0, "{m1:?}");
        assert!(!r.mids.contains_key(&4));
        let b = r.state_boxes[&5];
        assert_eq!((b[0], b[2]), (0.0, 333.0));
        assert_eq!(province_label(&f, 1), "province 1 Meran (state 5 Burggrafenamt)");
        assert_eq!(province_label(&f, 4), "province 4 (water)");
    }

    #[test]
    fn colours_of_the_picture() {
        let f = facts();
        let r = raster(&f).unwrap();
        let img = picture(&f, &r, &[], &[2], None);
        let at = |x: usize, y: usize| img.pixels[y * r.w + x];
        assert_eq!(at(200, 100), Color32::from_rgb(PICKED[0], PICKED[1], PICKED[2]));
        assert_eq!(at(100, 100), Color32::from_rgb(NO_STATE[0], NO_STATE[1], NO_STATE[2]));
        assert_eq!(at(100, 333), Color32::from_rgb(SEA[0], SEA[1], SEA[2]));
    }
}
