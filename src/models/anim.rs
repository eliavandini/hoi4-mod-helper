//! `.anim` files: per bone a translation, rotation and scale, either fixed
//! or sampled at every frame.
//!
//! `info` has the frame rate (`fps`), the number of frames (`sa`) and one
//! object per bone: which channels are sampled (`sa`, some of "tqs") and the
//! values of the others (`t`, `q`, `s`). `samples` then holds the sampled
//! values, frame by frame, bone by bone in the order of `info`.

use anyhow::{Result, bail};

use super::math::{Affine, Quat, V3};
use super::mesh::Mesh;
use super::pdxbin;

#[derive(Clone, Debug)]
pub struct Track {
    pub bone: String,
    /// One value (fixed) or one per frame.
    pub t: Vec<V3>,
    pub q: Vec<Quat>,
    pub s: Vec<f32>,
}

#[derive(Clone, Debug)]
pub struct Anim {
    pub fps: f32,
    pub frames: usize,
    pub tracks: Vec<Track>,
}

impl Anim {
    pub fn parse(b: &[u8]) -> Result<Anim> {
        let root = pdxbin::parse(b)?;
        let Some(info) = root.child("info") else { bail!("no info in the animation") };
        let fps = info.floats("fps").first().copied().filter(|f| *f > 0.0).unwrap_or(15.0);
        let frames = info.ints("sa").first().copied().unwrap_or(1).max(1) as usize;
        let samples = root.child("samples");
        let st = samples.map(|s| s.floats("t")).unwrap_or(&[]);
        let sq = samples.map(|s| s.floats("q")).unwrap_or(&[]);
        let ss = samples.map(|s| s.floats("s")).unwrap_or(&[]);
        let bones: Vec<(&pdxbin::Node, &str)> = info.children.iter().map(|b| (b, b.str("sa").unwrap_or(""))).collect();
        let per = |c: char, k: usize| bones.iter().filter(|b| b.1.contains(c)).count() * k;
        let (nt, nq, ns) = (per('t', 3), per('q', 4), per('s', 1));
        if st.len() < nt * frames || sq.len() < nq * frames || ss.len() < ns * frames {
            bail!(
                "{} frames of {} bones need {}/{}/{} sampled values, the file has {}/{}/{}",
                frames,
                bones.len(),
                nt * frames,
                nq * frames,
                ns * frames,
                st.len(),
                sq.len(),
                ss.len()
            );
        }
        let mut tracks = Vec::with_capacity(bones.len());
        let (mut ot, mut oq, mut os) = (0, 0, 0);
        for (b, sa) in &bones {
            let mut tr = Track { bone: b.name.clone(), t: Vec::new(), q: Vec::new(), s: Vec::new() };
            if sa.contains('t') {
                tr.t = (0..frames).map(|f| V3::from_slice(&st[f * nt + ot..])).collect();
                ot += 3;
            } else {
                let t = b.floats("t");
                tr.t.push(if t.len() >= 3 { V3::from_slice(t) } else { V3::ZERO });
            }
            if sa.contains('q') {
                tr.q = (0..frames).map(|f| Quat::from_slice(&sq[f * nq + oq..])).collect();
                oq += 4;
            } else {
                let q = b.floats("q");
                tr.q.push(if q.len() >= 4 { Quat::from_slice(q) } else { Quat::IDENTITY });
            }
            if sa.contains('s') {
                tr.s = (0..frames).map(|f| ss[f * ns + os]).collect();
                os += 1;
            } else {
                tr.s.push(b.floats("s").first().copied().unwrap_or(1.0));
            }
            tracks.push(tr);
        }
        Ok(Anim { fps, frames, tracks })
    }

    /// Length in seconds.
    pub fn duration(&self) -> f32 {
        (self.frames.max(2) - 1) as f32 / self.fps
    }

    /// The bones' local transforms at `time` seconds (wrapped round when
    /// looping, else held at the end), for the bones of `mesh`.
    pub fn pose(&self, mesh: &Mesh, time: f32, looping: bool) -> Vec<Option<Affine>> {
        let d = self.duration();
        let t = if looping && d > 0.0 { time.rem_euclid(d) } else { time.clamp(0.0, d) };
        let f = t * self.fps;
        let f0 = (f.floor() as usize).min(self.frames - 1);
        let f1 = (f0 + 1).min(self.frames - 1);
        let k = (f - f0 as f32).clamp(0.0, 1.0);
        let mut out = vec![None; mesh.bones.len()];
        for tr in &self.tracks {
            let Some(b) = mesh.bone(&tr.bone) else { continue };
            let at = |n: usize, i: usize| if n == 1 { 0 } else { i.min(n - 1) };
            let tv = tr.t[at(tr.t.len(), f0)].lerp(tr.t[at(tr.t.len(), f1)], k);
            let qv = tr.q[at(tr.q.len(), f0)].nlerp(tr.q[at(tr.q.len(), f1)], k);
            let sv = tr.s[at(tr.s.len(), f0)] + (tr.s[at(tr.s.len(), f1)] - tr.s[at(tr.s.len(), f0)]) * k;
            out[b] = Some(Affine::trs(tv, qv, sv));
        }
        out
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::super::mesh::tests::two_bone_mesh;
    use super::super::pdxbin::{Node, Prop};
    use super::*;
    use crate::models::math::v3;

    /// The tip bone of [`two_bone_mesh`] turning 90° about z over 3 frames.
    pub fn bend_anim() -> Vec<u8> {
        let s = std::f32::consts::FRAC_1_SQRT_2;
        let (h0, h1) = ((std::f32::consts::PI / 8.0).sin(), (std::f32::consts::PI / 8.0).cos());
        let bone = |name: &str, sa: &str, t: [f32; 3]| Node {
            name: name.into(),
            props: vec![
                ("sa".into(), Prop::Strs(vec![sa.into()])),
                ("t".into(), Prop::Floats(t.to_vec())),
                ("q".into(), Prop::Floats(vec![0.0, 0.0, 0.0, 1.0])),
                ("s".into(), Prop::Floats(vec![1.0])),
            ],
            children: vec![],
        };
        let root = Node {
            name: String::new(),
            props: vec![("pdxasset".into(), Prop::Ints(vec![1, 0]))],
            children: vec![
                Node {
                    name: "info".into(),
                    props: vec![("fps".into(), Prop::Floats(vec![2.0])), ("sa".into(), Prop::Ints(vec![3])), ("j".into(), Prop::Ints(vec![2]))],
                    children: vec![bone("root", "", [0.0; 3]), bone("tip", "q", [0.0, 2.0, 0.0])],
                },
                Node { name: "samples".into(), props: vec![("q".into(), Prop::Floats(vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, h0, h1, 0.0, 0.0, s, s]))], children: vec![] },
            ],
        };
        pdxbin::write(&root)
    }

    #[test]
    fn samples_and_poses() {
        let a = Anim::parse(&bend_anim()).unwrap();
        assert_eq!((a.frames, a.tracks.len()), (3, 2));
        assert!((a.duration() - 1.0).abs() < 1e-6);
        let m = Mesh::parse(&two_bone_mesh()).unwrap();
        // at the end the tip is turned 90° about z: its +y points to -x
        let g = m.globals(&a.pose(&m, 1.0, false));
        let up = g[1].dir(v3(0.0, 1.0, 0.0));
        assert!((up - v3(-1.0, 0.0, 0.0)).len() < 1e-4, "{up:?}");
        assert!((g[1].apply(V3::ZERO) - v3(0.0, 2.0, 0.0)).len() < 1e-5);
        // half way (frame 1) 45°; looping wraps round
        let g = m.globals(&a.pose(&m, 1.5, true));
        let up = g[1].dir(v3(0.0, 1.0, 0.0));
        assert!((up - v3(-0.70710677, 0.70710677, 0.0)).len() < 1e-4, "{up:?}");
        assert!(Anim::parse(&two_bone_mesh()).is_err(), "a mesh is not an animation");
    }
}
