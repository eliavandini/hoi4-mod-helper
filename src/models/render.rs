//! A small software renderer for model previews: textured triangles with a
//! depth buffer, perspective or orthographic, lit by a sun. Both faces of a
//! triangle are drawn (the game's leaves and flat parts are one-sided
//! planes seen from both sides).

use super::math::{Affine, V3, v3};

/// A texture with its smaller copies (each half the size of the one before).
#[derive(Clone, Debug)]
pub struct Texture {
    pub mips: Vec<Mip>,
    /// Pixels with alpha under half are holes (leaves, fences).
    pub cutout: bool,
}

#[derive(Clone, Debug)]
pub struct Mip {
    pub w: usize,
    pub h: usize,
    pub px: Vec<[u8; 4]>,
}

impl Texture {
    /// From RGBA bytes, made at most `max` wide or high.
    pub fn from_rgba(w: usize, h: usize, rgba: &[u8], max: usize, cutout: bool) -> Texture {
        let mut cur = Mip { w, h, px: rgba.chunks_exact(4).map(|c| [c[0], c[1], c[2], c[3]]).collect() };
        while cur.w > max.max(1) || cur.h > max.max(1) {
            cur = cur.half();
        }
        let mut mips = vec![cur];
        while let Some(last) = mips.last()
            && (last.w > 1 || last.h > 1)
        {
            let next = last.half();
            mips.push(next);
        }
        Texture { mips, cutout }
    }
    pub fn solid(c: [u8; 4]) -> Texture {
        Texture { mips: vec![Mip { w: 1, h: 1, px: vec![c] }], cutout: false }
    }
    fn sample(&self, level: usize, u: f32, v: f32) -> [u8; 4] {
        let m = &self.mips[level.min(self.mips.len() - 1)];
        let x = ((u - u.floor()) * m.w as f32) as usize % m.w;
        let y = ((v - v.floor()) * m.h as f32) as usize % m.h;
        m.px[y * m.w + x]
    }
}

impl Mip {
    fn half(&self) -> Mip {
        let (w2, h2) = ((self.w / 2).max(1), (self.h / 2).max(1));
        let mut px = Vec::with_capacity(w2 * h2);
        for y in 0..h2 {
            for x in 0..w2 {
                let mut s = [0u32; 4];
                let mut n = 0;
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let (sx, sy) = ((x * 2 + dx).min(self.w - 1), (y * 2 + dy).min(self.h - 1));
                    let p = self.px[sy * self.w + sx];
                    for k in 0..4 {
                        s[k] += p[k] as u32;
                    }
                    n += 1;
                }
                px.push([(s[0] / n) as u8, (s[1] / n) as u8, (s[2] / n) as u8, (s[3] / n) as u8]);
            }
        }
        Mip { w: w2, h: h2, px }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Proj {
    /// Focal length in pixels; nearest depth drawn.
    Persp { focal: f32, near: f32 },
    /// Pixels per unit.
    Ortho { scale: f32 },
}

/// World → camera (x right, y up, z into the screen) and the projection.
#[derive(Clone, Copy, Debug)]
pub struct Camera {
    pub view: Affine,
    pub proj: Proj,
    /// Direction the light travels (world).
    pub sun: V3,
}

impl Camera {
    /// A camera at `eye` looking at `at` (y up).
    pub fn look_at(eye: V3, at: V3, proj: Proj) -> Camera {
        let f = (at - eye).norm();
        let up = if f.y.abs() > 0.999 { v3(0.0, 0.0, 1.0) } else { v3(0.0, 1.0, 0.0) };
        // left-handed: right = up × forward
        let r = up.cross(f).norm();
        let u = f.cross(r);
        let m = [[r.x, r.y, r.z], [u.x, u.y, u.z], [f.x, f.y, f.z]];
        let rot = Affine { m, t: V3::ZERO };
        Camera { view: Affine { m, t: -rot.dir(eye) }, proj, sun: v3(-0.45, -0.8, 0.4).norm() }
    }
    /// Orbit round `at`: `yaw` degrees clockwise from looking north,
    /// `pitch` degrees down, at `dist`.
    pub fn orbit(at: V3, yaw: f32, pitch: f32, dist: f32, proj: Proj) -> Camera {
        let (sy, cy) = yaw.to_radians().sin_cos();
        let (sp, cp) = pitch.to_radians().sin_cos();
        // from the eye towards `at`: north turned by yaw, tilted down by pitch
        let dir = v3(sy * cp, -sp, cy * cp);
        Camera::look_at(at - dir * dist, at, proj)
    }
}

pub struct Frame {
    pub w: usize,
    pub h: usize,
    pub color: Vec<[u8; 4]>,
    /// 1 / depth (bigger is nearer; 0 is empty).
    pub depth: Vec<f32>,
}

/// A vertex in world space with what is drawn of it.
#[derive(Clone, Copy, Debug)]
pub struct Vert {
    pub p: V3,
    pub n: V3,
    pub uv: [f32; 2],
}

/// Camera-space vertex with the values interpolated across a triangle.
#[derive(Clone, Copy)]
struct CVert {
    c: V3,
    uv: [f32; 2],
    light: f32,
}

impl Frame {
    pub fn new(w: usize, h: usize, clear: [u8; 4]) -> Frame {
        Frame { w, h, color: vec![clear; w * h], depth: vec![0.0; w * h] }
    }

    pub fn rgba(&self) -> Vec<u8> {
        self.color.iter().flatten().copied().collect()
    }

    /// Draw triangles (indices into `verts`) with a texture and a tint.
    pub fn draw(&mut self, cam: &Camera, verts: &[Vert], tri: &[u32], tex: &Texture, tint: [f32; 3]) {
        let cv: Vec<CVert> = verts
            .iter()
            .map(|v| {
                let n = v.n;
                let lit = if n.len() > 0.0 { (-cam.sun).dot(n.norm()).max(0.0) } else { 0.6 };
                CVert { c: cam.view.apply(v.p), uv: v.uv, light: 0.42 + 0.68 * lit }
            })
            .collect();
        for t in tri.chunks_exact(3) {
            let (a, b, c) = (t[0] as usize, t[1] as usize, t[2] as usize);
            if a >= cv.len() || b >= cv.len() || c >= cv.len() {
                continue;
            }
            // the orthographic camera too keeps what is behind it out
            let near = match cam.proj {
                Proj::Persp { near, .. } => near,
                Proj::Ortho { .. } => 1e-3,
            };
            if cv[a].c.z >= near && cv[b].c.z >= near && cv[c].c.z >= near {
                self.raster(cam, [cv[a], cv[b], cv[c]], tex, tint);
                continue;
            }
            let poly = clip_near(&[cv[a], cv[b], cv[c]], near);
            for k in 1..poly.len().saturating_sub(1) {
                self.raster(cam, [poly[0], poly[k], poly[k + 1]], tex, tint);
            }
        }
    }

    fn project(&self, cam: &Camera, c: V3) -> (f32, f32, f32) {
        let (cx, cy) = (self.w as f32 / 2.0, self.h as f32 / 2.0);
        match cam.proj {
            Proj::Persp { focal, .. } => {
                let iz = 1.0 / c.z;
                (cx + c.x * focal * iz, cy - c.y * focal * iz, iz)
            }
            // 1 / depth too, for the depth test only (it is in front of the camera)
            Proj::Ortho { scale } => (cx + c.x * scale, cy - c.y * scale, 1.0 / c.z),
        }
    }

    fn raster(&mut self, cam: &Camera, v: [CVert; 3], tex: &Texture, tint: [f32; 3]) {
        let p: Vec<(f32, f32, f32)> = v.iter().map(|v| self.project(cam, v.c)).collect();
        let area = (p[1].0 - p[0].0) * (p[2].1 - p[0].1) - (p[1].1 - p[0].1) * (p[2].0 - p[0].0);
        if area.abs() < 1e-8 || !area.is_finite() {
            return;
        }
        let minx = p.iter().map(|q| q.0).fold(f32::INFINITY, f32::min).floor().max(0.0) as i64;
        let maxx = p.iter().map(|q| q.0).fold(f32::NEG_INFINITY, f32::max).ceil().min(self.w as f32 - 1.0) as i64;
        let miny = p.iter().map(|q| q.1).fold(f32::INFINITY, f32::min).floor().max(0.0) as i64;
        let maxy = p.iter().map(|q| q.1).fold(f32::NEG_INFINITY, f32::max).ceil().min(self.h as f32 - 1.0) as i64;
        if minx > maxx || miny > maxy {
            return;
        }
        // texture level from texels per pixel
        let m0 = &tex.mips[0];
        let duv = (v[1].uv[0] - v[0].uv[0]) * (v[2].uv[1] - v[0].uv[1]) - (v[1].uv[1] - v[0].uv[1]) * (v[2].uv[0] - v[0].uv[0]);
        let texels = duv.abs() * (m0.w * m0.h) as f32;
        let level = if texels > 0.0 { (0.5 * (texels / area.abs()).log2()).max(0.0) as usize } else { 0 };
        let persp = matches!(cam.proj, Proj::Persp { .. });
        // attributes over depth for perspective-correct interpolation
        let w: [f32; 3] = [p[0].2, p[1].2, p[2].2];
        let att = |i: usize| -> [f32; 3] {
            let k = if persp { w[i] } else { 1.0 };
            [v[i].uv[0] * k, v[i].uv[1] * k, v[i].light * k]
        };
        let (a0, a1, a2) = (att(0), att(1), att(2));
        let inv = 1.0 / area;
        for y in miny..=maxy {
            let py = y as f32 + 0.5;
            for x in minx..=maxx {
                let px = x as f32 + 0.5;
                let b0 = ((p[1].0 - px) * (p[2].1 - py) - (p[1].1 - py) * (p[2].0 - px)) * inv;
                let b1 = ((p[2].0 - px) * (p[0].1 - py) - (p[2].1 - py) * (p[0].0 - px)) * inv;
                let b2 = 1.0 - b0 - b1;
                if b0 < 0.0 || b1 < 0.0 || b2 < 0.0 {
                    continue;
                }
                let iz = b0 * w[0] + b1 * w[1] + b2 * w[2];
                let i = y as usize * self.w + x as usize;
                if iz <= self.depth[i] {
                    continue;
                }
                let k = if persp { 1.0 / iz } else { 1.0 };
                let u = (b0 * a0[0] + b1 * a1[0] + b2 * a2[0]) * k;
                let vv = (b0 * a0[1] + b1 * a1[1] + b2 * a2[1]) * k;
                let l = (b0 * a0[2] + b1 * a1[2] + b2 * a2[2]) * k;
                let t = tex.sample(level, u, vv);
                if tex.cutout && t[3] < 128 {
                    continue;
                }
                self.depth[i] = iz;
                let c = |ch: usize| (t[ch] as f32 * l * tint[ch]).min(255.0) as u8;
                self.color[i] = [c(0), c(1), c(2), 255];
            }
        }
    }
}

/// The part of a polygon at depth `near` or more.
fn clip_near(poly: &[CVert], near: f32) -> Vec<CVert> {
    let mut out = Vec::with_capacity(4);
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        let (ia, ib) = (a.c.z >= near, b.c.z >= near);
        if ia {
            out.push(a);
        }
        if ia != ib {
            let t = (near - a.c.z) / (b.c.z - a.c.z);
            out.push(CVert {
                c: a.c.lerp(b.c, t),
                uv: [a.uv[0] + (b.uv[0] - a.uv[0]) * t, a.uv[1] + (b.uv[1] - a.uv[1]) * t],
                light: a.light + (b.light - a.light) * t,
            });
        }
    }
    out
}

/// A square of ground with grid lines every unit (`size` units wide).
pub fn ground_grid() -> Texture {
    let n = 64;
    let mut px = vec![[0u8; 4]; n * n];
    for y in 0..n {
        for x in 0..n {
            let line = x % 16 == 0 || y % 16 == 0;
            px[y * n + x] = if line { [120, 128, 120, 255] } else { [86, 92, 84, 255] };
        }
    }
    Texture { mips: vec![Mip { w: n, h: n, px }], cutout: false }
}

/// Two triangles of ground at height `y` from -half to half (x and z),
/// with `cells` repeats of the texture.
pub fn ground_quad(y: f32, half: f32, cells: f32) -> (Vec<Vert>, Vec<u32>) {
    let up = v3(0.0, 1.0, 0.0);
    let c = cells / 4.0;
    let verts = vec![
        Vert { p: v3(-half, y, -half), n: up, uv: [0.0, 0.0] },
        Vert { p: v3(half, y, -half), n: up, uv: [c, 0.0] },
        Vert { p: v3(half, y, half), n: up, uv: [c, c] },
        Vert { p: v3(-half, y, half), n: up, uv: [0.0, c] },
    ];
    (verts, vec![0, 1, 2, 0, 2, 3])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_with_depth_and_holes() {
        let cam = Camera::look_at(v3(0.0, 0.0, -5.0), V3::ZERO, Proj::Persp { focal: 50.0, near: 0.1 });
        let mut f = Frame::new(64, 64, [0, 0, 0, 0]);
        let quad = |z: f32| {
            let n = v3(0.0, 0.0, -1.0);
            vec![
                Vert { p: v3(-1.0, -1.0, z), n, uv: [0.0, 1.0] },
                Vert { p: v3(1.0, -1.0, z), n, uv: [1.0, 1.0] },
                Vert { p: v3(1.0, 1.0, z), n, uv: [1.0, 0.0] },
                Vert { p: v3(-1.0, 1.0, z), n, uv: [0.0, 0.0] },
            ]
        };
        let red = Texture::solid([200, 0, 0, 255]);
        let green = Texture::solid([0, 200, 0, 255]);
        // red behind, green in front: green wins whatever the order
        f.draw(&cam, &quad(0.0), &[0, 1, 2, 0, 2, 3], &green, [1.0; 3]);
        f.draw(&cam, &quad(1.0), &[0, 1, 2, 0, 2, 3], &red, [1.0; 3]);
        let mid = f.color[32 * 64 + 32];
        assert!(mid[1] > 100 && mid[0] == 0, "{mid:?}");
        // 1 unit at 5 deep is 10 px: the quad spans 22..42
        assert_eq!(f.color[32 * 64 + 20][3], 0);
        assert_eq!(f.color[32 * 64 + 24][3], 255);
        // a see-through texture leaves its holes
        let mut f2 = Frame::new(64, 64, [0, 0, 0, 0]);
        let holes = Texture::from_rgba(2, 1, &[0, 0, 255, 255, 0, 0, 255, 0], 16, true);
        f2.draw(&cam, &quad(0.0), &[0, 1, 2, 0, 2, 3], &holes, [1.0; 3]);
        assert_eq!(f2.color[32 * 64 + 26][3], 255);
        assert_eq!(f2.color[32 * 64 + 38][3], 0);
        // a triangle half behind the eye is cut, not lost or smeared
        let mut f3 = Frame::new(64, 64, [0, 0, 0, 0]);
        let n = v3(0.0, 1.0, 0.0);
        let floor = [
            Vert { p: v3(-3.0, -1.0, -10.0), n, uv: [0.0, 0.0] },
            Vert { p: v3(3.0, -1.0, -10.0), n, uv: [0.0, 0.0] },
            Vert { p: v3(0.0, -1.0, 10.0), n, uv: [0.0, 0.0] },
        ];
        f3.draw(&cam, &floor, &[0, 1, 2], &red, [1.0; 3]);
        assert!(f3.color[63 * 64 + 32][3] == 255 && f3.color[10 * 64 + 32][3] == 0);
        assert_eq!(Texture::from_rgba(8, 4, &[9; 8 * 4 * 4], 4, false).mips.iter().map(|m| (m.w, m.h)).collect::<Vec<_>>(), [(4, 2), (2, 1), (1, 1)]);
    }

    #[test]
    fn orbit_looks_at_the_target() {
        let cam = Camera::orbit(v3(1.0, 2.0, 3.0), 30.0, 25.0, 10.0, Proj::Ortho { scale: 1.0 });
        let c = cam.view.apply(v3(1.0, 2.0, 3.0));
        assert!(c.x.abs() < 1e-4 && c.y.abs() < 1e-4 && (c.z - 10.0).abs() < 1e-4, "{c:?}");
        // seen from the south (yaw 0), east is to the right
        let cam = Camera::orbit(V3::ZERO, 0.0, 0.0, 10.0, Proj::Ortho { scale: 1.0 });
        assert!(cam.view.apply(v3(1.0, 0.0, 0.0)).x > 0.9);
        // from above (pitch 90), north is up on the screen
        let cam = Camera::orbit(V3::ZERO, 0.0, 90.0, 10.0, Proj::Ortho { scale: 1.0 });
        assert!(cam.view.apply(v3(0.0, 0.0, 1.0)).y > 0.9);
        assert!(cam.view.apply(v3(1.0, 0.0, 0.0)).x > 0.9);
    }
}
