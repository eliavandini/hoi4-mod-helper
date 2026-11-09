//! `.mesh` files: shapes of triangle meshes with their material and skin,
//! the skeleton they are skinned to, and locators.

use anyhow::{Result, bail};
use rustc_hash::FxHashMap;

use super::math::{Affine, Quat, V3, v3};
use super::pdxbin::{self, Node};

/// One mesh of a shape: its triangles, material and skin.
#[derive(Clone, Debug, Default)]
pub struct Part {
    /// The shape's name (what `meshsettings` name).
    pub shape: String,
    /// Index of the mesh in its shape.
    pub index: usize,
    pub pos: Vec<V3>,
    pub nrm: Vec<V3>,
    pub uv: Vec<[f32; 2]>,
    pub tri: Vec<u32>,
    /// Diffuse texture, a file name next to the mesh.
    pub diffuse: String,
    /// Per vertex up to four bones (indexes into [`Mesh::bones`], -1 none)
    /// and their weights.
    pub skin: Option<Vec<([i32; 4], [f32; 4])>>,
}

#[derive(Clone, Debug)]
pub struct Bone {
    pub name: String,
    pub parent: Option<usize>,
    /// Mesh space → bone space in the bind pose.
    pub inv_bind: Affine,
    /// Bone space → parent's space in the bind pose.
    pub local_bind: Affine,
}

#[derive(Clone, Debug)]
pub struct Locator {
    pub name: String,
    pub pos: V3,
    pub rot: Quat,
    /// The bone it moves with.
    pub parent: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub parts: Vec<Part>,
    pub bones: Vec<Bone>,
    pub locators: Vec<Locator>,
    /// Box of the vertices (bind pose).
    pub min: V3,
    pub max: V3,
}

impl Mesh {
    pub fn parse(b: &[u8]) -> Result<Mesh> {
        let root = pdxbin::parse(b)?;
        let Some(object) = root.child("object") else { bail!("no object in the mesh") };
        let mut mesh = Mesh::default();
        let mut by_name: FxHashMap<String, usize> = FxHashMap::default();
        for shape in &object.children {
            // the shape's skeleton: its bone ix → our bone index
            let mut remap: FxHashMap<i32, usize> = FxHashMap::default();
            if let Some(sk) = shape.child("skeleton") {
                let mut raw: Vec<(i32, Option<i32>, &Node)> = sk.children.iter().map(|b| (b.ints("ix").first().copied().unwrap_or(-1), b.ints("pa").first().copied(), b)).collect();
                raw.sort_by_key(|r| r.0);
                for (ix, _, b) in &raw {
                    let i = *by_name.entry(b.name.clone()).or_insert_with(|| {
                        let tx = b.floats("tx");
                        let inv_bind = if tx.len() >= 12 { Affine::from_columns(tx) } else { Affine::IDENTITY };
                        mesh.bones.push(Bone { name: b.name.clone(), parent: None, inv_bind, local_bind: Affine::IDENTITY });
                        mesh.bones.len() - 1
                    });
                    remap.insert(*ix, i);
                }
                for (ix, pa, _) in &raw {
                    if let (Some(&i), Some(&p)) = (remap.get(ix), pa.and_then(|p| remap.get(&p)))
                        && i != p
                    {
                        mesh.bones[i].parent = Some(p);
                    }
                }
            }
            for (index, m) in shape.children.iter().filter(|c| c.name == "mesh").enumerate() {
                let p = m.floats("p");
                let n = m.floats("n");
                let u = m.floats("u0");
                let nv = p.len() / 3;
                let tri: Vec<u32> = m.ints("tri").iter().map(|&i| i as u32).collect();
                if tri.iter().any(|&i| i as usize >= nv) {
                    bail!("shape {}: a triangle names a vertex the mesh does not have", shape.name);
                }
                let mat = m.child("material");
                // the box the game picks units by, never drawn
                if mat.and_then(|m| m.str("shader")).is_some_and(|s| s.eq_ignore_ascii_case("collision")) {
                    continue;
                }
                let skin = m.child("skin").and_then(|s| {
                    let ix = s.ints("ix");
                    let w = s.floats("w");
                    if ix.len() < nv * 4 || w.len() < nv * 4 || remap.is_empty() {
                        return None;
                    }
                    Some(
                        (0..nv)
                            .map(|v| {
                                let mut b = [-1i32; 4];
                                let mut ws = [0f32; 4];
                                for k in 0..4 {
                                    if let Some(&i) = remap.get(&ix[v * 4 + k]) {
                                        b[k] = i as i32;
                                        ws[k] = w[v * 4 + k];
                                    }
                                }
                                (b, ws)
                            })
                            .collect(),
                    )
                });
                mesh.parts.push(Part {
                    shape: shape.name.clone(),
                    index,
                    pos: p.chunks_exact(3).map(V3::from_slice).collect(),
                    nrm: if n.len() == p.len() { n.chunks_exact(3).map(V3::from_slice).collect() } else { Vec::new() },
                    uv: if u.len() / 2 == nv { u.chunks_exact(2).map(|c| [c[0], c[1]]).collect() } else { vec![[0.0; 2]; nv] },
                    tri,
                    diffuse: mat.and_then(|m| m.str("diff")).unwrap_or("").to_string(),
                    skin,
                });
            }
        }
        if let Some(loc) = root.child("locator") {
            for l in &loc.children {
                let p = l.floats("p");
                let q = l.floats("q");
                mesh.locators.push(Locator {
                    name: l.name.clone(),
                    pos: if p.len() >= 3 { V3::from_slice(p) } else { V3::ZERO },
                    rot: if q.len() >= 4 { Quat::from_slice(q) } else { Quat::IDENTITY },
                    parent: l.str("pa").map(str::to_string),
                });
            }
        }
        // bind pose of each bone relative to its parent
        let globals: Vec<Affine> = mesh.bones.iter().map(|b| b.inv_bind.inverse()).collect();
        for i in 0..mesh.bones.len() {
            mesh.bones[i].local_bind = match mesh.bones[i].parent {
                Some(p) => mesh.bones[p].inv_bind.then_after(&globals[i]),
                None => globals[i],
            };
        }
        let (mut lo, mut hi) = (v3(f32::INFINITY, f32::INFINITY, f32::INFINITY), v3(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY));
        for p in mesh.parts.iter().flat_map(|p| &p.pos) {
            if p.is_finite() {
                lo = lo.min(*p);
                hi = hi.max(*p);
            }
        }
        (mesh.min, mesh.max) = if lo.x <= hi.x { (lo, hi) } else { (V3::ZERO, V3::ZERO) };
        Ok(mesh)
    }

    pub fn triangles(&self) -> usize {
        self.parts.iter().map(|p| p.tri.len() / 3).sum()
    }

    pub fn bone(&self, name: &str) -> Option<usize> {
        self.bones.iter().position(|b| b.name.eq_ignore_ascii_case(name))
    }

    /// Each bone's transform (bone space → mesh space) for local transforms
    /// per bone (the bind pose where None).
    pub fn globals(&self, local: &[Option<Affine>]) -> Vec<Affine> {
        let mut g: Vec<Option<Affine>> = vec![None; self.bones.len()];
        fn get(m: &Mesh, local: &[Option<Affine>], g: &mut Vec<Option<Affine>>, i: usize, depth: usize) -> Affine {
            if let Some(a) = g[i] {
                return a;
            }
            let l = local.get(i).copied().flatten().unwrap_or(m.bones[i].local_bind);
            let a = match m.bones[i].parent {
                Some(p) if depth < 256 => get(m, local, g, p, depth + 1).then_after(&l),
                _ => l,
            };
            g[i] = Some(a);
            a
        }
        (0..self.bones.len()).map(|i| get(self, local, &mut g, i, 0)).collect()
    }

    /// Where a node is (a bone, or a locator) for bone transforms `globals`:
    /// node space → mesh space.
    pub fn node(&self, name: &str, globals: &[Affine]) -> Option<Affine> {
        if let Some(b) = self.bone(name) {
            return globals.get(b).copied();
        }
        let l = self.locators.iter().find(|l| l.name.eq_ignore_ascii_case(name))?;
        let own = Affine::trs(l.pos, l.rot, 1.0);
        match l.parent.as_deref().and_then(|p| self.bone(p)) {
            Some(b) => globals.get(b).map(|g| g.then_after(&own)),
            None => Some(own),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::super::pdxbin::{Node, Prop};
    use super::*;

    fn n(name: &str, props: Vec<(&str, Prop)>, children: Vec<Node>) -> Node {
        Node { name: name.into(), props: props.into_iter().map(|(k, v)| (k.to_string(), v)).collect(), children }
    }

    /// A bar of two bones: vertices 0, 1 on the root bone, 2, 3 on the tip
    /// bone, which sits 2 up.
    pub fn two_bone_mesh() -> Vec<u8> {
        let ident = |t: [f32; 3]| Prop::Floats(vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, t[0], t[1], t[2]]);
        let mesh = n(
            "mesh",
            vec![
                ("p", Prop::Floats(vec![-1.0, 0.0, 0.0, 1.0, 0.0, 0.0, -1.0, 2.0, 0.0, 1.0, 2.0, 0.0])),
                ("n", Prop::Floats(vec![0.0, 0.0, -1.0].repeat(4))),
                ("u0", Prop::Floats(vec![0.0, 1.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0])),
                ("tri", Prop::Ints(vec![0, 1, 2, 2, 1, 3])),
            ],
            vec![
                n("material", vec![("shader", Prop::Strs(vec!["PdxMeshStandard".into()])), ("diff", Prop::Strs(vec!["bar_diffuse.dds".into()]))], vec![]),
                n("skin", vec![("bones", Prop::Ints(vec![1])), ("ix", Prop::Ints(vec![0, -1, -1, -1, 0, -1, -1, -1, 1, -1, -1, -1, 1, -1, -1, -1])), ("w", Prop::Floats(vec![1.0, 0.0, 0.0, 0.0].repeat(4)))], vec![]),
            ],
        );
        let skel = n(
            "skeleton",
            vec![],
            vec![n("root", vec![("ix", Prop::Ints(vec![0])), ("tx", ident([0.0; 3]))], vec![]), n("tip", vec![("ix", Prop::Ints(vec![1])), ("pa", Prop::Ints(vec![0])), ("tx", ident([0.0, -2.0, 0.0]))], vec![])],
        );
        let root = n(
            "",
            vec![("pdxasset", Prop::Ints(vec![1, 0]))],
            vec![
                n("object", vec![], vec![n("barShape", vec![], vec![mesh, skel])]),
                n("locator", vec![], vec![n("top_node", vec![("p", Prop::Floats(vec![0.0, 1.0, 0.0])), ("q", Prop::Floats(vec![0.0, 0.0, 0.0, 1.0])), ("pa", Prop::Strs(vec!["tip".into()]))], vec![])]),
            ],
        );
        pdxbin::write(&root)
    }

    #[test]
    fn reads_a_skinned_mesh() {
        let m = Mesh::parse(&two_bone_mesh()).unwrap();
        assert_eq!(m.parts.len(), 1);
        let p = &m.parts[0];
        assert_eq!((p.shape.as_str(), p.diffuse.as_str(), p.tri.len()), ("barShape", "bar_diffuse.dds", 6));
        assert_eq!(m.bones.len(), 2);
        assert_eq!(m.bones[1].parent, Some(0));
        // the tip sits 2 up from the root in the bind pose
        let g = m.globals(&[None, None]);
        assert!((g[1].apply(V3::ZERO) - v3(0.0, 2.0, 0.0)).len() < 1e-5);
        assert_eq!(p.skin.as_ref().unwrap()[2].0, [1, -1, -1, -1]);
        // the locator rides on the tip: 3 up
        assert!((m.node("top_node", &g).unwrap().apply(V3::ZERO) - v3(0.0, 3.0, 0.0)).len() < 1e-5);
        assert_eq!((m.min, m.max), (v3(-1.0, 0.0, 0.0), v3(1.0, 2.0, 0.0)));
    }
}
