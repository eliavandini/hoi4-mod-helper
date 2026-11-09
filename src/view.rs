//! Map camera and layer rendering.

use eframe::egui::{self, Align2, Color32, FontId, Mesh, Painter, Pos2, Rect, Shape, Stroke, pos2};
use serde::{Deserialize, Serialize};

use crate::layers::Placement;
use crate::tiles::{TILE, TileKey, TileManager, TileSource};
use crate::vector::{VectorData, VectorStyle};

/// World (project CRS, y up) ↔ screen (points, y down) mapping.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Camera {
    pub center: [f64; 2],
    /// World units per screen point.
    pub mpp: f64,
}

impl Default for Camera {
    fn default() -> Self {
        Camera { center: [0.0, 0.0], mpp: 1.0 }
    }
}

impl Camera {
    #[inline]
    pub fn to_screen(&self, rect: Rect, w: [f64; 2]) -> Pos2 {
        let c = rect.center();
        pos2(c.x + ((w[0] - self.center[0]) / self.mpp) as f32, c.y - ((w[1] - self.center[1]) / self.mpp) as f32)
    }

    #[inline]
    pub fn to_world(&self, rect: Rect, p: Pos2) -> [f64; 2] {
        let c = rect.center();
        [self.center[0] + (p.x - c.x) as f64 * self.mpp, self.center[1] - (p.y - c.y) as f64 * self.mpp]
    }

    /// min x, min y, max x, max y of the visible world area.
    pub fn view_bounds(&self, rect: Rect) -> [f64; 4] {
        let a = self.to_world(rect, rect.left_bottom());
        let b = self.to_world(rect, rect.right_top());
        [a[0], a[1], b[0], b[1]]
    }

    pub fn fit(&mut self, rect: Rect, b: [f64; 4]) {
        if !(b[0] < b[2] && b[1] < b[3]) {
            return;
        }
        self.center = [(b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0];
        let mx = (b[2] - b[0]) / (rect.width().max(1.0) as f64);
        let my = (b[3] - b[1]) / (rect.height().max(1.0) as f64);
        self.mpp = mx.max(my) * 1.05;
    }

    /// Zoom by `factor` (>1 = zoom in) keeping the world point under `p` fixed.
    pub fn zoom_at(&mut self, rect: Rect, p: Pos2, factor: f64) {
        let before = self.to_world(rect, p);
        self.mpp = (self.mpp / factor).clamp(1e-4, 1e7);
        let after = self.to_world(rect, p);
        self.center[0] += before[0] - after[0];
        self.center[1] += before[1] - after[1];
    }

    pub fn pan_pixels(&mut self, delta: egui::Vec2) {
        self.center[0] -= delta.x as f64 * self.mpp;
        self.center[1] += delta.y as f64 * self.mpp;
    }
}

pub struct DrawCtx<'a> {
    pub ctx: &'a egui::Context,
    pub painter: &'a Painter,
    pub rect: Rect,
    pub cam: Camera,
    pub pixels_per_point: f32,
}

/// Draw a raster layer (or any other tile source) through the tile pyramid.
pub fn draw_raster(dc: &DrawCtx, tm: &mut TileManager, layer_id: u64, rl: &dyn TileSource, pl: &Placement, opacity: f32) {
    let (w, h) = rl.size();
    let max_level = rl.max_level();
    let generation = rl.generation();

    // Always keep the coarsest overview tile around as a fallback.
    let top = TileKey { layer: layer_id, level: max_level, tx: 0, ty: 0 };
    if tm.get(&top).is_none_or(|t| t.generation != generation) {
        tm.request(dc.ctx, top, true, rl, [0, 0, w, h]);
    }

    // Pyramid level: source pixels per physical screen pixel.
    let src_per_px = dc.cam.mpp / pl.pixel_size_world() / dc.pixels_per_point as f64;
    let level = if src_per_px > 1.0 { (src_per_px.log2().floor() as i64).clamp(0, max_level as i64) as u8 } else { 0 };

    // Visible pixel range (sample the view border for reprojected layers).
    let vb = dc.cam.view_bounds(dc.rect);
    let n = if pl.affine { 1 } else { 8 };
    let mut pmin = [f64::INFINITY; 2];
    let mut pmax = [f64::NEG_INFINITY; 2];
    for i in 0..=n {
        let t = i as f64 / n as f64;
        let x = vb[0] + (vb[2] - vb[0]) * t;
        let y = vb[1] + (vb[3] - vb[1]) * t;
        for p in [[x, vb[1]], [x, vb[3]], [vb[0], y], [vb[2], y]] {
            if let Some(px) = pl.world_to_pixel(p) {
                pmin = [pmin[0].min(px[0]), pmin[1].min(px[1])];
                pmax = [pmax[0].max(px[0]), pmax[1].max(px[1])];
            }
        }
    }
    if !pl.affine {
        let m = [(pmax[0] - pmin[0]) * 0.05 + 2.0, (pmax[1] - pmin[1]) * 0.05 + 2.0];
        pmin = [pmin[0] - m[0], pmin[1] - m[1]];
        pmax = [pmax[0] + m[0], pmax[1] + m[1]];
    }
    let x0 = pmin[0].max(0.0);
    let y0 = pmin[1].max(0.0);
    let x1 = pmax[0].min(w as f64);
    let y1 = pmax[1].min(h as f64);
    if !(x0 < x1 && y0 < y1) {
        return;
    }

    let span = (TILE as u64) << level;
    let ntx = (w as u64).div_ceil(span);
    let nty = (h as u64).div_ceil(span);
    let tx0 = (x0 as u64 / span).min(ntx - 1);
    let ty0 = (y0 as u64 / span).min(nty - 1);
    let tx1 = ((x1.ceil() as u64).div_ceil(span)).min(ntx);
    let ty1 = ((y1.ceil() as u64).div_ceil(span)).min(nty);

    let tint = Color32::WHITE.gamma_multiply(opacity.clamp(0.0, 1.0));
    let grid = if pl.affine { 1 } else { 8 };
    let mut wanted: Vec<(f64, TileKey, [u32; 4])> = Vec::new();
    let center_px = [(x0 + x1) / 2.0, (y0 + y1) / 2.0];

    for ty in ty0..ty1 {
        for tx in tx0..tx1 {
            let key = TileKey { layer: layer_id, level, tx: tx as u32, ty: ty as u32 };
            let region = [
                (tx * span) as u32,
                (ty * span) as u32,
                ((tx + 1) * span).min(w as u64) as u32,
                ((ty + 1) * span).min(h as u64) as u32,
            ];
            let mut drawn = false;
            let mut fresh = false;
            if let Some(t) = tm.get(&key) {
                fresh = t.generation == generation;
                draw_tile_part(dc, pl, &t.tex, key, region, grid, tint);
                drawn = true;
            }
            if !drawn {
                // Fall back to the closest coarser tile that is available.
                for l in level + 1..=max_level {
                    let shift = l - level;
                    let parent = TileKey { layer: layer_id, level: l, tx: (tx >> shift) as u32, ty: (ty >> shift) as u32 };
                    if let Some(t) = tm.get(&parent) {
                        draw_tile_part(dc, pl, &t.tex, parent, region, grid, tint);
                        break;
                    }
                }
            }
            if !fresh {
                let c = [(region[0] + region[2]) as f64 / 2.0, (region[1] + region[3]) as f64 / 2.0];
                let d = (c[0] - center_px[0]).hypot(c[1] - center_px[1]);
                wanted.push((d, key, region));
            }
        }
    }
    wanted.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (_, key, region) in wanted {
        if tm.is_pending(&key) {
            continue;
        }
        if !tm.request(dc.ctx, key, false, rl, region) {
            break;
        }
    }
}

/// Draw the part `region` (source pixels) of the texture belonging to tile `key`.
fn draw_tile_part(
    dc: &DrawCtx,
    pl: &Placement,
    tex: &egui::TextureHandle,
    key: TileKey,
    region: [u32; 4],
    grid: usize,
    tint: Color32,
) {
    let scale = (1u64 << key.level) as f64;
    let ox = key.tx as f64 * TILE as f64 * scale;
    let oy = key.ty as f64 * TILE as f64 * scale;
    let [tw, th] = tex.size();
    let (uw, uh) = (tw as f64 * scale, th as f64 * scale);
    let mut mesh = Mesh::with_texture(tex.id());
    let (rx0, ry0, rx1, ry1) = (region[0] as f64, region[1] as f64, region[2] as f64, region[3] as f64);
    for j in 0..=grid {
        let py = ry0 + (ry1 - ry0) * j as f64 / grid as f64;
        for i in 0..=grid {
            let px = rx0 + (rx1 - rx0) * i as f64 / grid as f64;
            let Some(wp) = pl.pixel_to_world(px, py) else { return };
            mesh.vertices.push(egui::epaint::Vertex {
                pos: dc.cam.to_screen(dc.rect, wp),
                uv: pos2(((px - ox) / uw) as f32, ((py - oy) / uh) as f32),
                color: tint,
            });
        }
    }
    let row = (grid + 1) as u32;
    for j in 0..grid as u32 {
        for i in 0..grid as u32 {
            let a = j * row + i;
            mesh.indices.extend_from_slice(&[a, a + 1, a + row, a + 1, a + row + 1, a + row]);
        }
    }
    dc.painter.add(Shape::mesh(mesh));
}

/// Draw the water lines, then the boundary lines (and labels) of a vector layer.
pub fn draw_vector(dc: &DrawCtx, vd: &VectorData, style: &VectorStyle, offset: [f64; 2], opacity: f32) {
    let vb = dc.cam.view_bounds(dc.rect);
    let o = [vd.origin[0] + offset[0], vd.origin[1] + offset[1]];
    let lb = [(vb[0] - o[0]) as f32, (vb[1] - o[1]) as f32, (vb[2] - o[0]) as f32, (vb[3] - o[1]) as f32];
    let lod = VectorData::lod_for(dc.cam.mpp * 0.6);
    let inv = 1.0 / dc.cam.mpp;
    let c = dc.rect.center();
    let cl = [dc.cam.center[0] - o[0], dc.cam.center[1] - o[1]];
    let to_screen = |p: [f32; 2]| pos2(c.x + ((p[0] as f64 - cl[0]) * inv) as f32, c.y - ((p[1] as f64 - cl[1]) * inv) as f32);
    let alpha = opacity.clamp(0.0, 1.0);
    let in_view = |b: [f32; 4]| !(b[2] < lb[0] || b[0] > lb[2] || b[3] < lb[1] || b[1] > lb[3]);

    // rivers and lakes under the boundaries (sorted by kind)
    let mut last_kind = None;
    let (mut stroke, mut visible) = (Stroke::NONE, false);
    for w in &vd.water {
        if last_kind != Some(w.kind) {
            last_kind = Some(w.kind);
            let ls = style.water(w.kind);
            visible = ls.visible;
            let col = Color32::from_rgba_unmultiplied(ls.color[0], ls.color[1], ls.color[2], ls.color[3]);
            stroke = Stroke::new(ls.width * style.width_scale, col.gamma_multiply(alpha));
        }
        if visible && in_view(w.bbox) {
            dc.painter.add(Shape::line(w.lods[lod].iter().map(|&p| to_screen(p)).collect(), stroke));
        }
    }

    let mut last_level = u8::MAX;
    for way in &vd.ways {
        if way.level != last_level {
            last_level = way.level;
            let ls = style.get(way.level);
            visible = ls.visible;
            let col = Color32::from_rgba_unmultiplied(ls.color[0], ls.color[1], ls.color[2], ls.color[3]);
            stroke = Stroke::new(ls.width * style.width_scale, col.gamma_multiply(alpha));
        }
        if !visible {
            continue;
        }
        let b = way.bbox;
        if b[2] < lb[0] || b[0] > lb[2] || b[3] < lb[1] || b[1] > lb[3] {
            continue;
        }
        let pts: Vec<Pos2> = way.lods[lod].iter().map(|&p| to_screen(p)).collect();
        dc.painter.add(Shape::line(pts, stroke));
    }

    if !style.labels {
        return;
    }
    // Areas are sorted by admin level, so more important names win collisions.
    let mut placed: Vec<Rect> = Vec::new();
    for a in &vd.areas {
        if placed.len() >= 400 || a.name.is_empty() || !style.get(a.level).visible {
            continue;
        }
        let b = a.bbox;
        if b[2] < lb[0] || b[0] > lb[2] || b[3] < lb[1] || b[1] > lb[3] {
            continue;
        }
        let screen_w = (b[2] - b[0]) as f64 * inv;
        if !(90.0..3500.0).contains(&screen_w) {
            continue;
        }
        let p = to_screen(a.label);
        if !dc.rect.contains(p) {
            continue;
        }
        let ls = style.get(a.level);
        let size = match a.level {
            0..=2 => 18.0,
            3..=4 => 16.0,
            5..=6 => 14.0,
            7..=8 => 12.5,
            _ => 11.0,
        };
        let font = FontId::proportional(size);
        let col = Color32::from_rgb(ls.color[0], ls.color[1], ls.color[2]).gamma_multiply(alpha);
        let galley = dc.painter.layout_no_wrap(a.name.clone(), font.clone(), col);
        let rect = Align2::CENTER_CENTER.anchor_size(p, galley.size()).expand(3.0);
        if placed.iter().any(|r| r.intersects(rect)) {
            continue;
        }
        placed.push(rect);
        let shadow = Color32::from_black_alpha((200.0 * alpha) as u8);
        for d in [egui::vec2(1.0, 1.0), egui::vec2(-1.0, -1.0), egui::vec2(1.0, -1.0), egui::vec2(-1.0, 1.0)] {
            dc.painter.text(p + d, Align2::CENTER_CENTER, &a.name, font.clone(), shadow);
        }
        dc.painter.galley(rect.shrink(3.0).min, galley, col);
    }
}

/// Scale bar in the bottom-left corner.
pub fn draw_scale_bar(dc: &DrawCtx) {
    let target = 120.0 * dc.cam.mpp;
    let mag = 10f64.powf(target.log10().floor());
    let len = [1.0, 2.0, 5.0, 10.0].iter().map(|m| m * mag).rfind(|&l| l <= target).unwrap_or(mag);
    let px = (len / dc.cam.mpp) as f32;
    let base = dc.rect.left_bottom() + egui::vec2(16.0, -18.0);
    let stroke = Stroke::new(2.0, Color32::WHITE);
    let bg = Rect::from_min_max(base + egui::vec2(-6.0, -22.0), base + egui::vec2(px + 6.0, 6.0));
    dc.painter.rect_filled(bg, 3.0, Color32::from_black_alpha(140));
    dc.painter.line_segment([base, base + egui::vec2(px, 0.0)], stroke);
    dc.painter.line_segment([base, base + egui::vec2(0.0, -5.0)], stroke);
    dc.painter.line_segment([base + egui::vec2(px, 0.0), base + egui::vec2(px, -5.0)], stroke);
    let label = if len >= 1000.0 { format!("{} km", len / 1000.0) } else { format!("{len} m") };
    dc.painter.text(base + egui::vec2(px / 2.0, -8.0), Align2::CENTER_BOTTOM, label, FontId::proportional(12.0), Color32::WHITE);
}

/// Outline of a raster layer (used to highlight the selected layer).
pub fn draw_outline(dc: &DrawCtx, pl: &Placement, color: Color32) {
    let pts: Vec<Pos2> = pl.outline(32).into_iter().map(|w| dc.cam.to_screen(dc.rect, w)).collect();
    dc.painter.add(Shape::dashed_line(&pts, Stroke::new(1.5, color), 8.0, 5.0));
}
