//! Display of the province map as a tiled overlay: a fill colour per province
//! (by map mode), borders graded by what they separate, selection highlight.

use std::sync::Arc;

use eframe::egui::{Color32, ColorImage};
use parking_lot::RwLock;

use super::model::Labels;
use crate::tiles::{self, TileJob, TileSource};

pub struct RenderStyle {
    /// Fill per province id (alpha 0 = transparent).
    pub fill: Vec<[u8; 4]>,
    /// Borders between different `group_a` (e.g. states) are drawn darker,
    /// between different `group_b` (e.g. nations) darkest.
    pub group_a: Vec<u32>,
    pub group_b: Vec<u32>,
    pub selected: Vec<bool>,
    pub borders: bool,
}

impl RenderStyle {
    pub fn empty() -> Self {
        RenderStyle { fill: Vec::new(), group_a: Vec::new(), group_b: Vec::new(), selected: Vec::new(), borders: true }
    }
}

pub struct Overlay {
    pub labels: Arc<RwLock<Labels>>,
    pub style: Arc<RenderStyle>,
    pub w: u32,
    pub h: u32,
    pub generation: u64,
}

impl TileSource for Overlay {
    fn size(&self) -> (u32, u32) {
        (self.w, self.h)
    }
    fn max_level(&self) -> u8 {
        tiles::max_level(self.w, self.h)
    }
    fn generation(&self) -> u64 {
        self.generation
    }
    fn job(&self, level: u8, region: [u32; 4]) -> TileJob {
        let (labels, style) = (self.labels.clone(), self.style.clone());
        Box::new(move || render(&labels.read(), &style, level, region))
    }
}

#[inline]
fn shade(c: [u8; 4], f: u32) -> [u8; 4] {
    [(c[0] as u32 * f / 256) as u8, (c[1] as u32 * f / 256) as u8, (c[2] as u32 * f / 256) as u8, c[3].max(200)]
}

pub fn render(lab: &Labels, st: &RenderStyle, level: u8, r: [u32; 4]) -> ColorImage {
    let s = 1u32 << level;
    let tw = (r[2] - r[0]).div_ceil(s) as usize;
    let th = (r[3] - r[1]).div_ceil(s) as usize;
    let (w, h) = (lab.w, lab.h);
    // sample columns / rows, with one extra on each side for the borders
    let xs: Vec<u32> = (-1..=tw as i64).map(|i| (r[0] as i64 + i * s as i64 + s as i64 / 2).clamp(0, w as i64 - 1) as u32).collect();
    let ys: Vec<u32> = (-1..=th as i64).map(|j| (r[1] as i64 + j * s as i64 + s as i64 / 2).clamp(0, h as i64 - 1) as u32).collect();
    let row_of = |y: u32| -> Vec<u32> { xs.iter().map(|&x| lab.at(x, y)).collect() };
    let fill = |p: u32| st.fill.get(p as usize).copied().unwrap_or([0, 0, 0, 0]);
    let ga = |p: u32| st.group_a.get(p as usize).copied().unwrap_or(0);
    let gb = |p: u32| st.group_b.get(p as usize).copied().unwrap_or(0);
    let sel = |p: u32| st.selected.get(p as usize).copied().unwrap_or(false);
    let province_borders = level <= 1;
    let mut out = vec![Color32::TRANSPARENT; tw * th];
    let mut up = row_of(ys[0]);
    let mut cur = row_of(ys[1]);
    for j in 0..th {
        let down = row_of(ys[j + 2]);
        for i in 0..tw {
            let a = cur[i + 1];
            let mut c = fill(a);
            let sa = sel(a);
            if sa {
                c = [((c[0] as u32 + 255) / 2) as u8, ((c[1] as u32 + 230) / 2) as u8, ((c[2] as u32 + 120) / 2) as u8, c[3].max(170)];
            }
            if st.borders {
                // strongest border to any of the four neighbours
                let mut rank = 0u8;
                for b in [cur[i], cur[i + 2], up[i + 1], down[i + 1]] {
                    if b == a {
                        continue;
                    }
                    let k = if sel(b) != sa {
                        4
                    } else if gb(b) != gb(a) {
                        3
                    } else if ga(b) != ga(a) {
                        2
                    } else if province_borders {
                        1
                    } else {
                        0
                    };
                    rank = rank.max(k);
                }
                c = match rank {
                    4 => [255, 215, 0, 255],
                    3 => [20, 20, 22, 255],
                    2 => shade(c, 110),
                    1 => shade(c, 190),
                    _ => c,
                };
            }
            out[j * tw + i] = if c[3] == 255 {
                Color32::from_rgb(c[0], c[1], c[2])
            } else {
                Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3])
            };
        }
        up = std::mem::replace(&mut cur, down);
    }
    ColorImage::new([tw, th], out)
}

/// Pleasant distinct colour for an id.
pub fn id_colour(id: u32, salt: u32) -> [u8; 3] {
    let h = ((id.wrapping_mul(2_654_435_761) ^ salt.wrapping_mul(40503)) % 3600) as f32 / 3600.0;
    let v = (id.wrapping_mul(97) % 5) as f32;
    hsv(h, 0.42 + v * 0.07, 0.62 + (4.0 - v) * 0.07)
}

pub fn hsv(h: f32, s: f32, v: f32) -> [u8; 3] {
    let i = (h * 6.0).floor();
    let f = h * 6.0 - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - f * s), v * (1.0 - (1.0 - f) * s));
    let (r, g, b) = match i as i32 % 6 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    [(r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8]
}

/// Blue (low) – green – red (high) for t in 0..1.
pub fn ramp(t: f32) -> [u8; 3] {
    let t = t.clamp(0.0, 1.0);
    hsv((1.0 - t) * 0.66, 0.75, 0.9)
}
