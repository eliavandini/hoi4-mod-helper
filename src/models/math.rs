//! The little 3D maths the models need: vectors, quaternions, affine
//! transforms. Paradox's space: x right (east on the map), y up, z forward
//! (north), left-handed.

use std::ops::{Add, Mul, Neg, Sub};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct V3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

pub const fn v3(x: f32, y: f32, z: f32) -> V3 {
    V3 { x, y, z }
}

impl V3 {
    pub const ZERO: V3 = v3(0.0, 0.0, 0.0);
    pub fn from_slice(s: &[f32]) -> V3 {
        v3(s[0], s[1], s[2])
    }
    pub fn dot(self, o: V3) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }
    pub fn cross(self, o: V3) -> V3 {
        v3(self.y * o.z - self.z * o.y, self.z * o.x - self.x * o.z, self.x * o.y - self.y * o.x)
    }
    pub fn len(self) -> f32 {
        self.dot(self).sqrt()
    }
    pub fn norm(self) -> V3 {
        let l = self.len();
        if l > 1e-12 { self * (1.0 / l) } else { self }
    }
    pub fn min(self, o: V3) -> V3 {
        v3(self.x.min(o.x), self.y.min(o.y), self.z.min(o.z))
    }
    pub fn max(self, o: V3) -> V3 {
        v3(self.x.max(o.x), self.y.max(o.y), self.z.max(o.z))
    }
    pub fn lerp(self, o: V3, t: f32) -> V3 {
        self + (o - self) * t
    }
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }
}

impl Add for V3 {
    type Output = V3;
    fn add(self, o: V3) -> V3 {
        v3(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}
impl Sub for V3 {
    type Output = V3;
    fn sub(self, o: V3) -> V3 {
        v3(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}
impl Mul<f32> for V3 {
    type Output = V3;
    fn mul(self, k: f32) -> V3 {
        v3(self.x * k, self.y * k, self.z * k)
    }
}
impl Neg for V3 {
    type Output = V3;
    fn neg(self) -> V3 {
        v3(-self.x, -self.y, -self.z)
    }
}

/// Rotation as x, y, z, w (the order of the game's files).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quat {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Default for Quat {
    fn default() -> Self {
        Quat::IDENTITY
    }
}

impl Quat {
    pub const IDENTITY: Quat = Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 };
    pub fn from_slice(s: &[f32]) -> Quat {
        Quat { x: s[0], y: s[1], z: s[2], w: s[3] }.norm()
    }
    pub fn norm(self) -> Quat {
        let l = (self.x * self.x + self.y * self.y + self.z * self.z + self.w * self.w).sqrt();
        if l > 1e-12 { Quat { x: self.x / l, y: self.y / l, z: self.z / l, w: self.w / l } } else { Quat::IDENTITY }
    }
    fn dot(self, o: Quat) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z + self.w * o.w
    }
    /// Shortest-way interpolation (normalised lerp, plenty for 30 fps samples).
    pub fn nlerp(self, o: Quat, t: f32) -> Quat {
        let o = if self.dot(o) < 0.0 { Quat { x: -o.x, y: -o.y, z: -o.z, w: -o.w } } else { o };
        Quat {
            x: self.x + (o.x - self.x) * t,
            y: self.y + (o.y - self.y) * t,
            z: self.z + (o.z - self.z) * t,
            w: self.w + (o.w - self.w) * t,
        }
        .norm()
    }
    /// Rows of the rotation matrix.
    pub fn matrix(self) -> [[f32; 3]; 3] {
        let Quat { x, y, z, w } = self;
        [
            [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - z * w), 2.0 * (x * z + y * w)],
            [2.0 * (x * y + z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z - x * w)],
            [2.0 * (x * z - y * w), 2.0 * (y * z + x * w), 1.0 - 2.0 * (x * x + y * y)],
        ]
    }
}

/// p' = m · p + t (m by rows).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine {
    pub m: [[f32; 3]; 3],
    pub t: V3,
}

impl Default for Affine {
    fn default() -> Self {
        Affine::IDENTITY
    }
}

impl Affine {
    pub const IDENTITY: Affine = Affine { m: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], t: V3::ZERO };

    pub fn trs(t: V3, q: Quat, s: f32) -> Affine {
        let mut m = q.matrix();
        for r in &mut m {
            for v in r.iter_mut() {
                *v *= s;
            }
        }
        Affine { m, t }
    }
    pub fn scale(s: f32) -> Affine {
        Affine { m: [[s, 0.0, 0.0], [0.0, s, 0.0], [0.0, 0.0, s]], t: V3::ZERO }
    }
    pub fn translate(t: V3) -> Affine {
        Affine { t, ..Affine::IDENTITY }
    }
    /// Rotation about the vertical axis by `deg` degrees, clockwise seen from
    /// above (north turns to east): the editor's turn. The game's
    /// `rotation = { 0 y 0 }` turns the other way (see
    /// `modbuild::ambient::game_rotation`).
    pub fn yaw(deg: f32) -> Affine {
        let (s, c) = deg.to_radians().sin_cos();
        Affine { m: [[c, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, c]], t: V3::ZERO }
    }
    /// Rotation about x (tilting the front down for positive degrees).
    pub fn pitch(deg: f32) -> Affine {
        let (s, c) = deg.to_radians().sin_cos();
        Affine { m: [[1.0, 0.0, 0.0], [0.0, c, -s], [0.0, s, c]], t: V3::ZERO }
    }
    pub fn roll(deg: f32) -> Affine {
        let (s, c) = deg.to_radians().sin_cos();
        Affine { m: [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]], t: V3::ZERO }
    }
    /// The game's 12 floats of a bone (`tx`): three columns, then the
    /// translation.
    pub fn from_columns(tx: &[f32]) -> Affine {
        let mut m = [[0.0; 3]; 3];
        for (c, col) in tx[..9].chunks_exact(3).enumerate() {
            for r in 0..3 {
                m[r][c] = col[r];
            }
        }
        Affine { m, t: V3::from_slice(&tx[9..12]) }
    }
    pub fn apply(&self, p: V3) -> V3 {
        self.dir(p) + self.t
    }
    /// The linear part only (directions, normals of rigid transforms).
    pub fn dir(&self, p: V3) -> V3 {
        let m = &self.m;
        v3(
            m[0][0] * p.x + m[0][1] * p.y + m[0][2] * p.z,
            m[1][0] * p.x + m[1][1] * p.y + m[1][2] * p.z,
            m[2][0] * p.x + m[2][1] * p.y + m[2][2] * p.z,
        )
    }
    /// self ∘ o: o first, then self.
    pub fn then_after(&self, o: &Affine) -> Affine {
        let mut m = [[0.0; 3]; 3];
        for (r, row) in m.iter_mut().enumerate() {
            for (c, v) in row.iter_mut().enumerate() {
                *v = (0..3).map(|k| self.m[r][k] * o.m[k][c]).sum();
            }
        }
        Affine { m, t: self.apply(o.t) }
    }
    pub fn inverse(&self) -> Affine {
        let m = &self.m;
        let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
        if det.abs() < 1e-20 {
            return Affine::IDENTITY;
        }
        let k = 1.0 / det;
        let inv = [
            [
                (m[1][1] * m[2][2] - m[1][2] * m[2][1]) * k,
                (m[0][2] * m[2][1] - m[0][1] * m[2][2]) * k,
                (m[0][1] * m[1][2] - m[0][2] * m[1][1]) * k,
            ],
            [
                (m[1][2] * m[2][0] - m[1][0] * m[2][2]) * k,
                (m[0][0] * m[2][2] - m[0][2] * m[2][0]) * k,
                (m[0][2] * m[1][0] - m[0][0] * m[1][2]) * k,
            ],
            [
                (m[1][0] * m[2][1] - m[1][1] * m[2][0]) * k,
                (m[0][1] * m[2][0] - m[0][0] * m[2][1]) * k,
                (m[0][0] * m[1][1] - m[0][1] * m[1][0]) * k,
            ],
        ];
        let a = Affine { m: inv, t: V3::ZERO };
        Affine { m: inv, t: -a.dir(self.t) }
    }
    /// A weighted sum (skinning several bones).
    pub fn add_scaled(&mut self, o: &Affine, w: f32) {
        for r in 0..3 {
            for c in 0..3 {
                self.m[r][c] += o.m[r][c] * w;
            }
        }
        self.t = self.t + o.t * w;
    }
    pub const ZERO: Affine = Affine { m: [[0.0; 3]; 3], t: V3::ZERO };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: V3, b: V3) -> bool {
        (a - b).len() < 1e-4
    }

    #[test]
    fn transforms() {
        let a = Affine::trs(v3(1.0, 2.0, 3.0), Quat::from_slice(&[0.0, 0.38268343, 0.0, 0.9238795]), 2.0);
        let p = v3(0.5, -1.0, 4.0);
        assert!(close(a.inverse().apply(a.apply(p)), p));
        let b = Affine::yaw(30.0);
        assert!(close(a.then_after(&b).apply(p), a.apply(b.apply(p))));
        // the editor's turn takes north (z) to east (x)
        assert!(close(Affine::yaw(90.0).apply(v3(0.0, 0.0, 1.0)), v3(1.0, 0.0, 0.0)));
        // a quaternion about y by 90° is the same turn
        let q = Quat::from_slice(&[0.0, std::f32::consts::FRAC_1_SQRT_2, 0.0, std::f32::consts::FRAC_1_SQRT_2]);
        assert!(close(Affine::trs(V3::ZERO, q, 1.0).apply(v3(0.0, 0.0, 1.0)), v3(1.0, 0.0, 0.0)));
        let c = Affine::from_columns(&[1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 5.0, 6.0, 7.0]);
        assert!(close(c.apply(V3::ZERO), v3(5.0, 6.0, 7.0)));
        let h = Quat::IDENTITY.nlerp(q, 0.5);
        assert!(close(Affine::trs(V3::ZERO, h, 1.0).apply(v3(0.0, 0.0, 1.0)), v3(std::f32::consts::FRAC_1_SQRT_2, 0.0, std::f32::consts::FRAC_1_SQRT_2)));
    }
}
