//! Textures as the game's model shader reads them (`PdxMeshAdvanced` in
//! `gfx/FX/pdxmesh.shader`):
//!
//! - diffuse: the colour;
//! - normal: G = x (towards the picture's right), A = y (towards its top),
//!   B = glow at night (emissive), R a copy of G;
//! - "specular" (the surface): G = shine (the shader squares it, × 0.4),
//!   B = metalness, A = glossiness, R unused.
//!
//! Written as uncompressed B8G8R8A8 DDS with all mipmaps, each side a power
//! of two (what the game wants).

use anyhow::{Result, bail};

use crate::modbuild::img::{self, Filter};

/// An RGBA picture.
#[derive(Clone, Debug, PartialEq)]
pub struct Img {
    pub w: usize,
    pub h: usize,
    pub rgba: Vec<u8>,
}

impl Img {
    pub fn solid(w: usize, h: usize, c: [u8; 4]) -> Img {
        Img { w, h, rgba: c.repeat(w * h) }
    }
    fn px(&self, x: usize, y: usize) -> [u8; 4] {
        let i = (y * self.w + x) * 4;
        [self.rgba[i], self.rgba[i + 1], self.rgba[i + 2], self.rgba[i + 3]]
    }
    pub fn resized(&self, w: usize, h: usize) -> Img {
        if (w, h) == (self.w, self.h) {
            return self.clone();
        }
        let f = if w < self.w || h < self.h { Filter::Lanczos } else { Filter::Bilinear };
        Img { w, h, rgba: img::resize_u8(&self.rgba, self.w, self.h, 4, w, h, f) }
    }
    /// Every pixel grey (red, green and blue alike): a height map, not a
    /// normal map.
    pub fn is_grey(&self) -> bool {
        self.rgba.chunks_exact(4).step_by(7).all(|c| c[0].abs_diff(c[1]) < 6 && c[1].abs_diff(c[2]) < 6)
    }
    /// Laid out as the game's normal maps are already (blue is glow, mostly
    /// off; the y in alpha), not as an ordinary normal map (blue mostly up).
    pub fn is_game_normal(&self) -> bool {
        let n = (self.rgba.len() / 4).max(1) as f64;
        let (mut b, mut a) = (0f64, 0f64);
        for c in self.rgba.chunks_exact(4) {
            b += c[2] as f64;
            a += c[3] as f64;
        }
        b / n < 64.0 && (64.0..192.0).contains(&(a / n))
    }
}

/// Decode DDS, TGA, PNG or JPEG.
pub fn decode(bytes: &[u8], name: &str) -> Result<Img> {
    let lower = name.to_ascii_lowercase();
    let (w, h, rgba) = if bytes.starts_with(b"DDS ") {
        img::read_dds_bytes(bytes, name)?
    } else if lower.ends_with(".tga") {
        img::read_tga_bytes(bytes, name)?
    } else {
        let i = image::load_from_memory(bytes).map_err(|e| anyhow::anyhow!("{name}: {e} (pictures can be DDS, PNG, JPEG or TGA)"))?.to_rgba8();
        (i.width(), i.height(), i.into_raw())
    };
    if w == 0 || h == 0 {
        bail!("{name} is empty");
    }
    Ok(Img { w: w as usize, h: h as usize, rgba })
}

/// The power of two nearest to `n` (in proportion), at most `max`.
pub fn pow2(n: usize, max: usize) -> usize {
    let n = n.max(1);
    let lo = 1usize << (usize::BITS - 1 - n.leading_zeros());
    let p = if lo == n || (n as f64 / lo as f64) < 1.5 { lo } else { lo * 2 };
    let max = max.max(1);
    p.min(1usize << (usize::BITS - 1 - max.leading_zeros()))
}

/// Resized so each side is a power of two of at most `max`.
pub fn fit(i: &Img, max: usize) -> Img {
    i.resized(pow2(i.w, max), pow2(i.h, max))
}

/// Multiplied by a colour (glTF's base colour factor).
pub fn tint(i: &Img, c: [f32; 4]) -> Img {
    if c.iter().all(|&v| (v - 1.0).abs() < 1e-4) {
        return i.clone();
    }
    let mut o = i.clone();
    for p in o.rgba.chunks_exact_mut(4) {
        for k in 0..4 {
            p[k] = (p[k] as f32 * c[k].clamp(0.0, 1.0)).round() as u8;
        }
    }
    o
}

fn unit(v: u8) -> f32 {
    v as f32 / 255.0 * 2.0 - 1.0
}
fn byte(v: f32) -> u8 {
    ((v.clamp(-1.0, 1.0) + 1.0) * 0.5 * 255.0).round() as u8
}

/// A normal (x right, y up in the picture) in the game's layout.
fn game_normal(x: f32, y: f32) -> [u8; 4] {
    let g = byte(x);
    [g, g, 0, byte(y)]
}

/// Facing straight out.
pub fn flat_normal(w: usize, h: usize) -> Img {
    Img::solid(w, h, game_normal(0.0, 0.0))
}

/// An ordinary normal map (red right, green up; `directx`: green down) in
/// the game's layout.
pub fn normal_from_rgb(i: &Img, directx: bool) -> Img {
    let mut o = Img::solid(i.w, i.h, [0; 4]);
    for (d, s) in o.rgba.chunks_exact_mut(4).zip(i.rgba.chunks_exact(4)) {
        let y = if directx { -unit(s[1]) } else { unit(s[1]) };
        d.copy_from_slice(&game_normal(unit(s[0]), y));
    }
    o
}

/// Bumps from brightness (bright is raised), as the guide's emboss does:
/// `strength` 1 tilts a full black-to-white step over one pixel by 45°.
pub fn normal_from_height(i: &Img, strength: f32) -> Img {
    let (w, h) = (i.w, i.h);
    let lum: Vec<f32> = i.rgba.chunks_exact(4).map(|c| (0.299 * c[0] as f32 + 0.587 * c[1] as f32 + 0.114 * c[2] as f32) / 255.0).collect();
    let at = |x: isize, y: isize| lum[(y.rem_euclid(h as isize) as usize) * w + x.rem_euclid(w as isize) as usize];
    // a picture of 512 px: the slopes of a 512 px one, whatever its size
    let s = strength * 2.0 * (w.max(h) as f32 / 512.0).max(0.25);
    let mut o = Img::solid(w, h, [0; 4]);
    for y in 0..h as isize {
        for x in 0..w as isize {
            let dx = (at(x + 1, y) - at(x - 1, y)) * 0.5;
            let dy_down = (at(x, y + 1) - at(x, y - 1)) * 0.5;
            // the slope leans the normal away from the higher side
            let (nx, ny, nz) = (-s * dx, s * dy_down, 1.0f32);
            let l = (nx * nx + ny * ny + nz * nz).sqrt();
            let i = (y as usize * w + x as usize) * 4;
            o.rgba[i..i + 4].copy_from_slice(&game_normal(nx / l, ny / l));
        }
    }
    o
}

/// The glow at night into a normal map's blue: the picture's brightness
/// times `color`. The normal map grows to the glow's size when smaller.
pub fn with_glow(normal: &Img, glow: &Img, color: [f32; 3]) -> Img {
    let (w, h) = (normal.w.max(glow.w), normal.h.max(glow.h));
    let mut n = normal.resized(w, h);
    let g = glow.resized(w, h);
    for (d, s) in n.rgba.chunks_exact_mut(4).zip(g.rgba.chunks_exact(4)) {
        let v = (0.299 * s[0] as f32 * color[0] + 0.587 * s[1] as f32 * color[1] + 0.114 * s[2] as f32 * color[2]).round();
        d[2] = v.clamp(0.0, 255.0) as u8;
    }
    n
}

/// The surface as one value each (0..1).
pub fn surface(shine: f32, metal: f32, gloss: f32) -> Img {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Img::solid(4, 4, [0, b(shine), b(metal), b(gloss)])
}

/// glTF's metal (blue) and roughness (green) map, times its factors.
pub fn surface_from_metal_rough(i: &Img, shine: f32, metal: f32, rough: f32) -> Img {
    let mut o = Img::solid(i.w, i.h, [0; 4]);
    let sh = (shine.clamp(0.0, 1.0) * 255.0).round() as u8;
    for (d, s) in o.rgba.chunks_exact_mut(4).zip(i.rgba.chunks_exact(4)) {
        let m = s[2] as f32 * metal.clamp(0.0, 1.0);
        let r = s[1] as f32 * rough.clamp(0.0, 1.0);
        d.copy_from_slice(&[0, sh, m.round() as u8, (255.0 - r).round() as u8]);
    }
    o
}

/// Each mipmap after the first: half the size (at least 1), 2×2 averaged.
fn half(i: &Img) -> Img {
    let (w, h) = ((i.w / 2).max(1), (i.h / 2).max(1));
    let mut o = Img::solid(w, h, [0; 4]);
    for y in 0..h {
        for x in 0..w {
            let mut s = [0u32; 4];
            let mut n = 0;
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let (sx, sy) = ((x * 2 + dx).min(i.w - 1), (y * 2 + dy).min(i.h - 1));
                let p = i.px(sx, sy);
                for k in 0..4 {
                    s[k] += p[k] as u32;
                }
                n += 1;
            }
            let o4 = &mut o.rgba[(y * w + x) * 4..(y * w + x) * 4 + 4];
            for k in 0..4 {
                o4[k] = ((s[k] + n / 2) / n) as u8;
            }
        }
    }
    o
}

/// A DDS file: uncompressed B8G8R8A8 with every mipmap down to 1×1.
pub fn dds(i: &Img) -> Vec<u8> {
    let mut levels = vec![i.clone()];
    while levels.last().is_some_and(|l| l.w > 1 || l.h > 1) {
        let next = half(levels.last().unwrap());
        levels.push(next);
    }
    let total: usize = levels.iter().map(|l| l.rgba.len()).sum();
    let mut b = Vec::with_capacity(128 + total);
    b.extend_from_slice(b"DDS ");
    // size, flags (caps, height, width, pitch, pixel format, mipmap count), height, width, pitch, depth, mipmaps
    for v in [124u32, 0x2_100f, i.h as u32, i.w as u32, i.w as u32 * 4, 0, levels.len() as u32] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b.resize(b.len() + 44, 0);
    // pixel format: size, alpha + rgb, -, 32 bits, masks; caps: texture, complex, mipmap
    for v in [32u32, 0x41, 0, 32, 0x00ff_0000, 0x0000_ff00, 0x0000_00ff, 0xff00_0000, 0x40_1008, 0, 0, 0, 0] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    debug_assert_eq!(b.len(), 128);
    for l in &levels {
        for p in l.rgba.chunks_exact(4) {
            b.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
        }
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn powers_of_two() {
        assert_eq!(pow2(1, 1024), 1);
        assert_eq!(pow2(512, 1024), 512);
        assert_eq!(pow2(700, 1024), 512);
        assert_eq!(pow2(800, 1024), 1024);
        assert_eq!(pow2(3000, 1024), 1024);
        assert_eq!(pow2(3, 1024), 4);
        assert_eq!(pow2(100, 48), 32);
    }

    #[test]
    fn dds_has_every_mipmap_and_reads_back() {
        let mut i = Img::solid(8, 4, [10, 20, 30, 255]);
        i.rgba[0] = 250;
        let b = dds(&i);
        // 8×4, 4×2, 2×1, 1×1
        assert_eq!(u32::from_le_bytes(b[28..32].try_into().unwrap()), 4);
        assert_eq!(b.len(), 128 + (32 + 8 + 2 + 1) * 4);
        let (w, h, px) = img::read_dds_bytes(&b, "t.dds").unwrap();
        assert_eq!((w, h), (8, 4));
        assert_eq!(&px[..4], &[250, 20, 30, 255]);
        assert_eq!(&px[4..8], &[10, 20, 30, 255]);
    }

    #[test]
    fn normals_in_the_games_layout() {
        // flat: x 0 in green, y 0 in alpha, no glow (as the game's nonormal.dds)
        assert_eq!(flat_normal(1, 1).rgba, vec![128, 128, 0, 128]);
        // an ordinary map tilted right and up
        let n = normal_from_rgb(&Img::solid(1, 1, [255, 255, 128, 255]), false);
        assert_eq!(n.rgba, vec![255, 255, 0, 255]);
        assert!(n.is_game_normal() || n.rgba[3] == 255);
        let dx = normal_from_rgb(&Img::solid(1, 1, [128, 0, 255, 255]), true);
        assert_eq!(dx.rgba[3], 255, "DirectX green down is up turned over");
        assert!(!Img::solid(2, 2, [128, 128, 255, 255]).is_game_normal());
        assert!(Img::solid(2, 2, [125, 125, 0, 127]).is_game_normal());
    }

    #[test]
    fn bumps_lean_away_from_the_high_side() {
        // bright on the right: the surface rises to the right, its normal leans left (x < 0)
        let mut i = Img::solid(8, 8, [0, 0, 0, 255]);
        for y in 0..8 {
            for x in 4..8 {
                let k = (y * 8 + x) * 4;
                i.rgba[k..k + 3].copy_from_slice(&[255, 255, 255]);
            }
        }
        let n = normal_from_height(&i, 1.0);
        let at = |x: usize, y: usize| n.px(x, y);
        assert!(at(4, 3)[1] < 100, "x leans left at the step: {:?}", at(4, 3));
        assert!((120..=136).contains(&at(4, 3)[3]), "no tilt up or down");
        assert_eq!(at(1, 3), [128, 128, 0, 128], "flat away from it");
        // bright at the bottom: rises downwards in the picture, leans up (y > 0)
        let mut j = Img::solid(8, 8, [0, 0, 0, 255]);
        for k in 32..64 {
            j.rgba[k * 4..k * 4 + 3].copy_from_slice(&[255, 255, 255]);
        }
        let m = normal_from_height(&j, 1.0);
        assert!(m.px(3, 4)[3] > 150, "{:?}", m.px(3, 4));
    }

    #[test]
    fn surfaces_and_glow() {
        assert_eq!(surface(0.5, 0.0, 1.0).rgba[..4], [0, 128, 0, 255]);
        let mr = surface_from_metal_rough(&Img::solid(1, 1, [0, 255, 255, 255]), 0.2, 1.0, 0.5);
        assert_eq!(mr.rgba, vec![0, 51, 255, 128]);
        let g = with_glow(&flat_normal(4, 4), &Img::solid(8, 8, [255, 255, 255, 255]), [1.0; 3]);
        assert_eq!((g.w, g.h), (8, 8));
        assert_eq!(g.rgba[..4], [128, 128, 255, 128]);
    }
}
