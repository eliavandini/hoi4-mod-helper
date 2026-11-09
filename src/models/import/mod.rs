//! New models for the game from a 3D file and its pictures, in one step:
//! what Paradox's Blender exporter and the modding guides
//! (steamcommunity.com/sharedfiles/filedetails/?id=3004523941) do by hand.
//!
//! - in: an OBJ (with its MTL) or a glTF / GLB as Blender, or any 3D
//!   program, exports them ([`obj`], [`gltf`]), or a `.mesh` the Paradox
//!   exporter wrote; the textures they name (DDS, PNG, JPEG, TGA), or others
//!   picked;
//! - out ([`make`]): `gfx/models/<folder>/<name>.mesh`, its textures laid
//!   out as the game's shader reads them ([`tex`]), `gfx/entities/<name>.gfx`
//!   (the `pdxmesh`) and `gfx/entities/<name>.asset` (the entity
//!   `<name>_entity`), which [`install`] puts in a content package.
//!
//! The game's space has y up and is left-handed. Blender's exporters write y
//! up, right-handed (or z up for an OBJ when asked): z is mirrored and the
//! triangles turned around, so the model looks as it did in Blender.
//! Texture v counts from the top, as in glTF (OBJ's from the bottom).

pub mod gltf;
pub mod obj;
pub mod tex;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, Result, bail};

use super::math::{V3, v3};
use super::mesh::Mesh;
use super::pdxbin::{self, Node, Prop};
use super::render::Texture;
use super::{Model, catalog};
use tex::Img;

/// Vertices one mesh of the game holds at most (16-bit indexes); bigger
/// ones are cut in several.
const MAX_VERTS: usize = 65535;

/// A picture a material uses.
#[derive(Clone, Debug, PartialEq)]
pub enum TexSrc {
    File(PathBuf),
    /// Inside the model file (a GLB's picture, a data URI).
    Embedded { name: String, bytes: Arc<Vec<u8>> },
}

impl TexSrc {
    pub fn label(&self) -> String {
        match self {
            TexSrc::File(p) => p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string()),
            TexSrc::Embedded { name, .. } => format!("{name} (in the model file)"),
        }
    }
    pub fn decode(&self) -> Result<Img> {
        match self {
            TexSrc::File(p) => {
                let b = std::fs::read(p).with_context(|| format!("cannot read {}", p.display()))?;
                tex::decode(&b, &p.to_string_lossy())
            }
            TexSrc::Embedded { name, bytes } => tex::decode(bytes, name),
        }
    }
}

/// A material of the 3D file.
#[derive(Clone, Debug)]
pub struct SrcMat {
    pub name: String,
    pub diffuse: Option<TexSrc>,
    /// Multiplies the diffuse; the colour where there is none.
    pub color: [f32; 4],
    /// A normal map, or a grey bump (height) map.
    pub normal: Option<TexSrc>,
    /// A surface map already in the game's layout (a `.mesh`'s `spec`).
    pub surface: Option<TexSrc>,
    /// glTF's metal (blue) and roughness (green) map.
    pub metal_rough: Option<TexSrc>,
    pub metal: f32,
    pub rough: f32,
    pub glow: Option<TexSrc>,
    /// Multiplies the glow picture; without one, all of it glows this much.
    pub glow_color: [f32; 3],
    /// Pictures the file names that are not there.
    pub missing: Vec<String>,
}

impl Default for SrcMat {
    fn default() -> Self {
        SrcMat {
            name: String::new(),
            diffuse: None,
            color: [0.8, 0.8, 0.8, 1.0],
            normal: None,
            surface: None,
            metal_rough: None,
            metal: 0.0,
            rough: 0.7,
            glow: None,
            glow_color: [0.0; 3],
            missing: Vec::new(),
        }
    }
}

/// Triangles of one material in the file's space (right-handed, y up unless
/// [`Up::Z`]); texture v from the top.
#[derive(Clone, Debug, Default)]
pub struct SrcPart {
    pub name: String,
    pub pos: Vec<V3>,
    /// Zero where the file has none (made flat).
    pub nrm: Vec<V3>,
    pub uv: Vec<[f32; 2]>,
    pub tri: Vec<u32>,
    pub mat: usize,
}

impl SrcPart {
    /// Faces without normals get their own vertices, facing out of the
    /// face (counter-clockwise, as the file has them).
    pub fn fill_normals(&mut self) {
        if !self.nrm.iter().any(|n| n.len() < 1e-6) {
            return;
        }
        let mut out = SrcPart { name: std::mem::take(&mut self.name), mat: self.mat, ..Default::default() };
        for t in self.tri.chunks_exact(3) {
            let (a, b, c) = (t[0] as usize, t[1] as usize, t[2] as usize);
            let face = (self.pos[b] - self.pos[a]).cross(self.pos[c] - self.pos[a]).norm();
            let keep = [a, b, c].iter().all(|&i| self.nrm[i].len() >= 1e-6);
            for i in [a, b, c] {
                out.tri.push(out.pos.len() as u32);
                out.pos.push(self.pos[i]);
                out.nrm.push(if keep { self.nrm[i] } else if face.is_finite() { face } else { v3(0.0, 1.0, 0.0) });
                out.uv.push(self.uv[i]);
            }
        }
        *self = out;
    }
}

/// Which way is up in an OBJ file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Up {
    /// Blender's default ("Y up, -Z forward").
    #[default]
    Y,
    Z,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Obj,
    Gltf,
    Mesh,
}

/// A 3D file read.
pub struct Source {
    pub file: PathBuf,
    pub kind: Kind,
    pub parts: Vec<SrcPart>,
    pub mats: Vec<SrcMat>,
    /// A `.mesh`: its tree, written again with the new textures' names (its
    /// materials are its meshes, in order).
    pub tree: Option<Node>,
    pub notes: Vec<String>,
}

impl Source {
    pub fn triangles(&self) -> usize {
        self.parts.iter().map(|p| p.tri.len() / 3).sum()
    }
}

/// Read a 3D file by its extension.
pub fn read(path: &Path) -> Result<Source> {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let src = match ext.as_str() {
        "obj" => obj::read(path)?,
        "gltf" | "glb" => gltf::read(path)?,
        "mesh" => read_mesh(path)?,
        _ => bail!("{} is not a model file this reads (.glb, .gltf, .obj or .mesh)", path.display()),
    };
    if src.tree.is_none() && src.triangles() == 0 {
        bail!("{} has no triangles", path.display());
    }
    Ok(src)
}

/// A picture a file names, looked for where it says, then by its name next
/// to the file and in a `textures` folder there (case aside).
pub fn find_picture(dir: &Path, written: &str) -> Option<PathBuf> {
    let w = written.trim().trim_matches('"').replace('\\', "/");
    if w.is_empty() {
        return None;
    }
    let p = Path::new(&w);
    if p.is_absolute() && p.is_file() {
        return Some(p.to_path_buf());
    }
    if let Some(f) = catalog::resolve_ci(dir, w.trim_start_matches("./")) {
        return Some(f);
    }
    let base = w.rsplit('/').next().unwrap_or(&w);
    for sub in ["", "textures", "Textures", "../textures"] {
        let d = if sub.is_empty() { dir.to_path_buf() } else { dir.join(sub) };
        if let Some(f) = catalog::resolve_ci(&d, base) {
            return Some(f);
        }
    }
    None
}

fn is_collision(m: &Node) -> bool {
    m.child("material").and_then(|m| m.str("shader")).is_some_and(|s| s.eq_ignore_ascii_case("collision"))
}

/// The drawn meshes of a `.mesh` tree: (shape, index in the shape, node).
fn drawn_meshes(tree: &Node) -> Vec<(String, usize, &Node)> {
    let mut out = Vec::new();
    if let Some(object) = tree.child("object") {
        for shape in &object.children {
            for (i, m) in shape.children.iter().filter(|c| c.name == "mesh").enumerate() {
                if !is_collision(m) {
                    out.push((shape.name.clone(), i, m));
                }
            }
        }
    }
    out
}

fn read_mesh(path: &Path) -> Result<Source> {
    let b = std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    let tree = pdxbin::parse(&b).with_context(|| format!("{}", path.display()))?;
    let mesh = Mesh::parse(&b).with_context(|| format!("{}", path.display()))?;
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut notes = Vec::new();
    let mut mats = Vec::new();
    for (shape, i, m) in drawn_meshes(&tree) {
        let mat = m.child("material");
        let mut sm = SrcMat { name: format!("{shape} #{i}"), color: [1.0; 4], ..Default::default() };
        for (key, slot) in [("diff", &mut sm.diffuse), ("n", &mut sm.normal), ("spec", &mut sm.surface)] {
            let Some(name) = mat.and_then(|m| m.str(key)).filter(|n| !n.is_empty()) else { continue };
            match find_picture(dir, name) {
                Some(f) => *slot = Some(TexSrc::File(f)),
                None => sm.missing.push(name.to_string()),
            }
        }
        mats.push(sm);
    }
    if mats.is_empty() {
        bail!("{} has no meshes to draw", path.display());
    }
    if mesh.parts.iter().any(|p| p.skin.is_some()) {
        notes.push(
            "Its vertices follow bones (an armature): the game draws such a model only while it plays an animation, \
             and animations are not imported. Export it without the armature unless you add one."
                .into(),
        );
    }
    if !mesh.locators.is_empty() {
        notes.push(format!("{} locator(s) kept.", mesh.locators.len()));
    }
    // the parts only to count (the tree is what is written)
    let parts = mesh.parts.iter().map(|p| SrcPart { name: p.shape.clone(), tri: p.tri.clone(), ..Default::default() }).collect();
    Ok(Source { file: path.to_path_buf(), kind: Kind::Mesh, parts, mats, tree: Some(tree), notes })
}

// ------------------------------------------------------------------ options

/// Where a normal map comes from.
#[derive(Clone, Debug, PartialEq)]
pub enum NormalFrom {
    /// The model's (a grey one is a bump map).
    Model,
    File(TexSrc),
    Flat,
    /// Bumps from the diffuse's brightness, as the guide's emboss.
    Bumps,
}

/// Where the surface (shine, metal, gloss) comes from.
#[derive(Clone, Debug, PartialEq)]
pub enum SurfaceFrom {
    Model,
    /// A picture already in the game's layout.
    File(TexSrc),
    Values,
}

#[derive(Clone, Debug, PartialEq)]
pub enum GlowFrom {
    None,
    Model,
    /// Its brightness glows at night.
    File(TexSrc),
}

/// How one material is made.
#[derive(Clone, Debug, PartialEq)]
pub struct MatOpts {
    /// A picture instead of the model's diffuse.
    pub diffuse: Option<TexSrc>,
    pub normal: NormalFrom,
    /// Strength of bumps made from brightness.
    pub bumps: f32,
    /// The normal map's green points down (DirectX, Substance's default).
    pub directx: bool,
    pub surface: SurfaceFrom,
    pub shine: f32,
    pub metal: f32,
    pub gloss: f32,
    pub glow: GlowFrom,
}

impl MatOpts {
    pub fn for_mat(m: &SrcMat) -> MatOpts {
        MatOpts {
            diffuse: None,
            normal: if m.normal.is_some() { NormalFrom::Model } else { NormalFrom::Flat },
            bumps: 1.0,
            directx: false,
            surface: if m.surface.is_some() || m.metal_rough.is_some() { SurfaceFrom::Model } else { SurfaceFrom::Values },
            shine: 0.35,
            metal: m.metal.clamp(0.0, 1.0),
            gloss: (1.0 - m.rough).clamp(0.0, 1.0),
            glow: if m.glow.is_some() || m.glow_color.iter().any(|&v| v > 0.01) { GlowFrom::Model } else { GlowFrom::None },
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    /// The model's name: the entity is `<name>_entity`, the mesh
    /// `<name>_mesh`, the files `<name>…`.
    pub name: String,
    /// Under `gfx/models/`.
    pub folder: String,
    pub up: Up,
    /// Map pixels per unit of the file (the pdxmesh's scale).
    pub scale: f32,
    /// Move it so the middle of its bottom is its origin (where it stands).
    pub ground: bool,
    /// Snow on it in winter (`PdxMeshAdvancedSnow`).
    pub snow: bool,
    /// Textures are made at most this big (a power of two).
    pub max_tex: usize,
    pub mats: Vec<MatOpts>,
}

impl Options {
    pub fn for_source(src: &Source) -> Options {
        let stem = src.file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        Options {
            name: slug(&stem),
            folder: "custom".into(),
            up: Up::Y,
            scale: 1.0,
            ground: src.kind != Kind::Mesh,
            snow: false,
            max_tex: 1024,
            mats: src.mats.iter().map(MatOpts::for_mat).collect(),
        }
    }
}

/// Lower case letters, digits and `_`, starting with a letter.
pub fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('_') && !out.is_empty() {
            out.push('_');
        }
    }
    let out = out.trim_end_matches('_').to_string();
    match out.chars().next() {
        None => "model".into(),
        Some(c) if c.is_ascii_digit() => format!("m_{out}"),
        _ => out,
    }
}

pub fn check_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("the model needs a name");
    }
    if !name.starts_with(|c: char| c.is_ascii_lowercase()) || !name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
        bail!("a name is lower case letters, digits and _, starting with a letter");
    }
    if name.len() > 60 {
        bail!("the name is too long");
    }
    Ok(())
}

fn check_folder(folder: &str) -> Result<()> {
    if folder.is_empty() || folder.split('/').any(|p| p.is_empty() || !p.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')) {
        bail!("the folder is names of letters, digits, _ and -, separated by /");
    }
    Ok(())
}

pub fn entity_name(name: &str) -> String {
    format!("{name}_entity")
}
pub fn mesh_name(name: &str) -> String {
    format!("{name}_mesh")
}

// ------------------------------------------------------------------ making

/// What an import makes.
pub struct Made {
    /// Paths in the content package (`gfx/…`) and their bytes.
    pub files: Vec<(String, Vec<u8>)>,
    pub entity: String,
    /// The mesh as written (read back) and the diffuse of each of its parts.
    pub mesh: Arc<Mesh>,
    pub diffuse: Vec<Arc<Img>>,
    pub scale: f32,
    pub notes: Vec<String>,
}

impl Made {
    /// The size in map pixels: (min, max) corners.
    pub fn bounds(&self) -> (V3, V3) {
        (self.mesh.min * self.scale, self.mesh.max * self.scale)
    }
    pub fn vertices(&self) -> usize {
        self.mesh.parts.iter().map(|p| p.pos.len()).sum()
    }
    /// A model for the previews.
    pub fn model(&self) -> Model {
        Model {
            entity: None,
            mesh: Some(self.mesh.clone()),
            file: None,
            tex: self.diffuse.iter().map(|i| Arc::new(Texture::from_rgba(i.w, i.h, &i.rgba, 512, false))).collect(),
            scale: self.scale,
            anims: Vec::new(),
            attach: Vec::new(),
            problems: Vec::new(),
        }
    }
}

/// The three textures of a material.
struct Texs {
    diffuse: Img,
    normal: Img,
    surface: Img,
}

fn normal_of(i: &Img, mo: &MatOpts) -> Img {
    if i.is_grey() {
        tex::normal_from_height(i, mo.bumps)
    } else if i.is_game_normal() {
        i.clone()
    } else {
        tex::normal_from_rgb(i, mo.directx)
    }
}

fn textures(m: &SrcMat, mo: &MatOpts, max: usize) -> Result<Texs> {
    let pic = |t: &TexSrc, what: &str| t.decode().with_context(|| format!("{}: the {what}", m.name));
    let diffuse = match mo.diffuse.as_ref().or(m.diffuse.as_ref()) {
        Some(t) => {
            let base = tex::fit(&pic(t, "diffuse")?, max);
            // the model's colour tints its own picture, not one picked instead
            if mo.diffuse.is_some() { base } else { tex::tint(&base, m.color) }
        }
        None => {
            let c = m.color.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
            Img::solid(4, 4, c)
        }
    };
    let mut normal = match &mo.normal {
        NormalFrom::Model => match &m.normal {
            Some(t) => normal_of(&tex::fit(&pic(t, "normal map")?, max), mo),
            None => tex::flat_normal(4, 4),
        },
        NormalFrom::File(t) => normal_of(&tex::fit(&pic(t, "normal map")?, max), mo),
        NormalFrom::Flat => tex::flat_normal(4, 4),
        NormalFrom::Bumps => tex::normal_from_height(&diffuse, mo.bumps),
    };
    let glow = match &mo.glow {
        GlowFrom::None => None,
        GlowFrom::Model => match &m.glow {
            Some(t) => Some((pic(t, "glow")?, m.glow_color)),
            // a glowing colour without a picture: all of it glows
            None if m.glow_color.iter().any(|&v| v > 0.0) => Some((Img::solid(4, 4, [255; 4]), m.glow_color)),
            None => None,
        },
        GlowFrom::File(t) => Some((pic(t, "glow")?, [1.0; 3])),
    };
    if let Some((g, c)) = glow {
        normal = tex::with_glow(&normal, &tex::fit(&g, max), c);
    }
    let surface = match &mo.surface {
        SurfaceFrom::Model => match (&m.surface, &m.metal_rough) {
            (Some(t), _) => tex::fit(&pic(t, "surface (specular) map")?, max),
            (None, Some(t)) => tex::surface_from_metal_rough(&tex::fit(&pic(t, "metal and roughness map")?, max), mo.shine, m.metal, m.rough),
            (None, None) => tex::surface(mo.shine, mo.metal, mo.gloss),
        },
        SurfaceFrom::File(t) => tex::fit(&pic(t, "surface (specular) map")?, max),
        SurfaceFrom::Values => tex::surface(mo.shine, mo.metal, mo.gloss),
    };
    Ok(Texs { diffuse, normal, surface })
}

/// A mesh being written: vertices in the game's space.
#[derive(Default)]
struct OutMesh {
    pos: Vec<V3>,
    nrm: Vec<V3>,
    uv: Vec<[f32; 2]>,
    tri: Vec<u32>,
    mat: usize,
}

fn to_game(p: V3, up: Up) -> V3 {
    match up {
        Up::Y => v3(p.x, p.y, -p.z),
        Up::Z => v3(p.x, p.z, p.y),
    }
}

/// The parts in the game's space, one mesh per material (cut in pieces of
/// at most [`MAX_VERTS`] vertices).
fn out_meshes(src: &Source, o: &Options) -> Vec<OutMesh> {
    let (mut lo, mut hi) = (v3(f32::INFINITY, f32::INFINITY, f32::INFINITY), v3(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY));
    for p in src.parts.iter().flat_map(|p| &p.pos) {
        let g = to_game(*p, o.up);
        lo = lo.min(g);
        hi = hi.max(g);
    }
    let shift = if o.ground && lo.x <= hi.x { v3(-(lo.x + hi.x) * 0.5, -lo.y, -(lo.z + hi.z) * 0.5) } else { V3::ZERO };
    let mut out = Vec::new();
    for mat in 0..src.mats.len() {
        let mut m = OutMesh { mat, ..Default::default() };
        for p in src.parts.iter().filter(|p| p.mat == mat) {
            let base = m.pos.len() as u32;
            m.pos.extend(p.pos.iter().map(|&v| to_game(v, o.up) + shift));
            m.nrm.extend(p.nrm.iter().map(|&n| to_game(n, o.up).norm()));
            m.uv.extend(p.uv.iter().copied());
            // mirrored: each triangle turned around to face out again
            for t in p.tri.chunks_exact(3) {
                m.tri.extend_from_slice(&[base + t[0], base + t[2], base + t[1]]);
            }
        }
        if m.tri.is_empty() {
            continue;
        }
        out.extend(split(m));
    }
    out
}

/// Cut a mesh with too many vertices in pieces.
fn split(m: OutMesh) -> Vec<OutMesh> {
    if m.pos.len() <= MAX_VERTS {
        return vec![m];
    }
    let mut out: Vec<OutMesh> = Vec::new();
    let mut cur = OutMesh { mat: m.mat, ..Default::default() };
    let mut remap: rustc_hash::FxHashMap<u32, u32> = Default::default();
    for t in m.tri.chunks_exact(3) {
        let new = t.iter().filter(|i| !remap.contains_key(i)).count();
        if cur.pos.len() + new > MAX_VERTS {
            out.push(std::mem::replace(&mut cur, OutMesh { mat: m.mat, ..Default::default() }));
            remap.clear();
        }
        for &i in t {
            let k = *remap.entry(i).or_insert_with(|| {
                cur.pos.push(m.pos[i as usize]);
                cur.nrm.push(m.nrm[i as usize]);
                cur.uv.push(m.uv[i as usize]);
                cur.pos.len() as u32 - 1
            });
            cur.tri.push(k);
        }
    }
    if !cur.tri.is_empty() {
        out.push(cur);
    }
    out
}

/// Per vertex the direction of growing u and, in w, which way v grows: the
/// game's shader takes `cross(normal, tangent) × w` as the way v grows
/// (down the picture).
fn tangents(pos: &[V3], nrm: &[V3], uv: &[[f32; 2]], tri: &[u32]) -> Vec<[f32; 4]> {
    let n = pos.len();
    let (mut tu, mut tv) = (vec![V3::ZERO; n], vec![V3::ZERO; n]);
    for t in tri.chunks_exact(3) {
        let (a, b, c) = (t[0] as usize, t[1] as usize, t[2] as usize);
        let (e1, e2) = (pos[b] - pos[a], pos[c] - pos[a]);
        let (du1, dv1, du2, dv2) = (uv[b][0] - uv[a][0], uv[b][1] - uv[a][1], uv[c][0] - uv[a][0], uv[c][1] - uv[a][1]);
        let r = du1 * dv2 - du2 * dv1;
        if r.abs() < 1e-12 {
            continue;
        }
        let su = (e1 * dv2 - e2 * dv1) * (1.0 / r);
        let sv = (e2 * du1 - e1 * du2) * (1.0 / r);
        if !su.is_finite() || !sv.is_finite() {
            continue;
        }
        for i in [a, b, c] {
            tu[i] = tu[i] + su;
            tv[i] = tv[i] + sv;
        }
    }
    (0..n)
        .map(|i| {
            let nn = nrm[i];
            let mut t = tu[i] - nn * nn.dot(tu[i]);
            if t.len() < 1e-8 || !t.is_finite() {
                let any = if nn.x.abs() < 0.9 { v3(1.0, 0.0, 0.0) } else { v3(0.0, 1.0, 0.0) };
                t = any - nn * nn.dot(any);
            }
            let t = t.norm();
            let w = if nn.cross(t).dot(tv[i]) < 0.0 { -1.0 } else { 1.0 };
            [t.x, t.y, t.z, w]
        })
        .collect()
}

fn floats(v: impl IntoIterator<Item = f32>) -> Prop {
    Prop::Floats(v.into_iter().collect())
}
fn strs(s: &str) -> Prop {
    Prop::Strs(vec![s.to_string()])
}
fn node(name: &str, props: Vec<(&str, Prop)>, children: Vec<Node>) -> Node {
    Node { name: name.into(), props: props.into_iter().map(|(k, v)| (k.to_string(), v)).collect(), children }
}

/// A material node.
fn material(shader: &str, t: &[String; 3]) -> Node {
    node("material", vec![("shader", strs(shader)), ("diff", strs(&t[0])), ("n", strs(&t[1])), ("spec", strs(&t[2]))], vec![])
}

fn mesh_node(m: &OutMesh, shader: &str, t: &[String; 3]) -> Node {
    let ta = tangents(&m.pos, &m.nrm, &m.uv, &m.tri);
    let (mut lo, mut hi) = (m.pos[0], m.pos[0]);
    for p in &m.pos {
        lo = lo.min(*p);
        hi = hi.max(*p);
    }
    node(
        "mesh",
        vec![
            ("p", floats(m.pos.iter().flat_map(|p| [p.x, p.y, p.z]))),
            ("n", floats(m.nrm.iter().flat_map(|p| [p.x, p.y, p.z]))),
            ("ta", floats(ta.into_iter().flatten())),
            ("u0", floats(m.uv.iter().flatten().copied())),
            ("tri", Prop::Ints(m.tri.iter().map(|&i| i as i32).collect())),
        ],
        vec![node("aabb", vec![("min", floats([lo.x, lo.y, lo.z])), ("max", floats([hi.x, hi.y, hi.z]))], vec![]), material(shader, t)],
    )
}

fn num(v: f32) -> String {
    let s = format!("{:.4}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-0" { "0".into() } else { s.to_string() }
}

/// Make the files of a model.
pub fn make(src: &Source, o: &Options) -> Result<Made> {
    check_name(&o.name)?;
    check_folder(&o.folder)?;
    if !(o.scale.is_finite() && o.scale > 0.0) {
        bail!("the scale must be more than 0");
    }
    if o.mats.len() != src.mats.len() {
        bail!("the options are for another model");
    }
    let name = &o.name;
    let shader = if o.snow { "PdxMeshAdvancedSnow" } else { "PdxMeshAdvanced" };
    let model_dir = format!("gfx/models/{}", o.folder);
    let mut notes = Vec::new();

    // textures: one set per material, named after it when there are several
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut tex_names: Vec<[String; 3]> = Vec::new();
    let mut diffuse: Vec<Arc<Img>> = Vec::new();
    let mut used: Vec<String> = Vec::new();
    for (i, (m, mo)) in src.mats.iter().zip(&o.mats).enumerate() {
        let stem = if src.mats.len() == 1 {
            name.clone()
        } else {
            let s = slug(&m.name);
            let s = if s == "model" || used.contains(&s) { format!("{i}") } else { s };
            used.push(s.clone());
            format!("{name}_{s}")
        };
        let t = textures(m, mo, o.max_tex)?;
        let names = [format!("{stem}_diffuse.dds"), format!("{stem}_normal.dds"), format!("{stem}_specular.dds")];
        for (n, img) in names.iter().zip([&t.diffuse, &t.normal, &t.surface]) {
            files.push((format!("{model_dir}/{n}"), tex::dds(img)));
        }
        tex_names.push(names);
        diffuse.push(Arc::new(t.diffuse));
    }

    // the mesh, and which material each drawn mesh has
    let mut settings: Vec<(String, usize, usize)> = Vec::new();
    let tree = match &src.tree {
        Some(t) => {
            let mut t = t.clone();
            if let Some(object) = t.children.iter_mut().find(|c| c.name == "object") {
                let mut k = 0;
                for shape in &mut object.children {
                    for (idx, m) in shape.children.iter_mut().filter(|c| c.name == "mesh").enumerate() {
                        if !is_collision(m) && k < tex_names.len() {
                            let mat = material(shader, &tex_names[k]);
                            m.children.retain(|c| c.name != "material");
                            m.children.push(mat);
                            settings.push((shape.name.clone(), idx, k));
                            k += 1;
                        }
                    }
                }
            }
            if o.ground {
                notes.push("A .mesh keeps its origin as it is.".into());
            }
            t
        }
        None => {
            let meshes = out_meshes(src, o);
            let shape = format!("{name}Shape");
            let nodes: Vec<Node> = meshes.iter().map(|m| mesh_node(m, shader, &tex_names[m.mat])).collect();
            for (i, m) in meshes.iter().enumerate() {
                settings.push((shape.clone(), i, m.mat));
            }
            if meshes.len() > src.mats.iter().enumerate().filter(|(i, _)| src.parts.iter().any(|p| p.mat == *i)).count() {
                notes.push(format!("Cut in {} meshes (one holds at most {MAX_VERTS} vertices).", meshes.len()));
            }
            node("", vec![("pdxasset", Prop::Ints(vec![1, 0]))], vec![node("object", vec![], vec![node(&shape, vec![], nodes)])])
        }
    };
    let bytes = pdxbin::write(&tree);
    let mesh = Mesh::parse(&bytes).context("the mesh made does not read back")?;
    // the drawn parts' diffuse, in the order the mesh lists them
    let part_tex: Vec<Arc<Img>> = settings.iter().map(|s| diffuse[s.2].clone()).collect();
    let mesh_file = format!("{model_dir}/{name}.mesh");
    files.insert(0, (mesh_file.clone(), bytes));

    // the definitions
    let file_name = src.file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut gfx = format!(
        "# Made by mod-helper from {file_name}: the model the entity {} shows.\nobjectTypes = {{\n\tpdxmesh = {{\n\t\tname = \"{}\"\n\t\tfile = \"{mesh_file}\"\n\t\tscale = {}\n",
        entity_name(name),
        mesh_name(name),
        num(o.scale)
    );
    for (shape, idx, k) in &settings {
        let t = &tex_names[*k];
        gfx.push_str(&format!(
            "\n\t\tmeshsettings = {{\n\t\t\tname = \"{shape}\"\n\t\t\tindex = {idx}\n\t\t\ttexture_diffuse = \"{}\"\n\t\t\ttexture_normal = \"{}\"\n\t\t\ttexture_specular = \"{}\"\n\t\t\tshader = \"{shader}\"\n\t\t}}\n",
            t[0], t[1], t[2]
        ));
    }
    gfx.push_str("\t}\n}\n");
    let asset = format!(
        "# Made by mod-helper from {file_name}: what the map's objects (map/ambient_object.txt) name.\nentity = {{\n\tname = \"{}\"\n\tpdxmesh = \"{}\"\n}}\n",
        entity_name(name),
        mesh_name(name)
    );
    files.push((format!("gfx/entities/{name}.gfx"), gfx.into_bytes()));
    files.push((format!("gfx/entities/{name}.asset"), asset.into_bytes()));
    Ok(Made { files, entity: entity_name(name), mesh: Arc::new(mesh), diffuse: part_tex, scale: o.scale, notes })
}

/// Write what an import made into `<content>/<package>/`. Files it replaces
/// go to `<content>/.backups/<time>/<package>/` first. Returns the paths
/// written.
pub fn install(made: &Made, content: &Path, package: &str) -> Result<Vec<String>> {
    let dir = content.join(package);
    let stamp = crate::editor::model::timestamp();
    let backup = content.join(".backups").join(format!("{stamp}-model"));
    let mut written = Vec::new();
    for (rel, bytes) in &made.files {
        let to = dir.join(rel);
        if to.is_file() {
            let b = backup.join(package).join(rel);
            std::fs::create_dir_all(b.parent().unwrap()).with_context(|| format!("cannot create {}", b.display()))?;
            std::fs::copy(&to, &b).with_context(|| format!("cannot back up {}", to.display()))?;
        }
        std::fs::create_dir_all(to.parent().unwrap()).with_context(|| format!("cannot create {}", to.display()))?;
        let tmp = to.with_extension("mhtmp");
        std::fs::write(&tmp, bytes).with_context(|| format!("cannot write {}", tmp.display()))?;
        std::fs::rename(&tmp, &to).with_context(|| format!("cannot write {}", to.display()))?;
        written.push(rel.clone());
    }
    crate::bundle::touched();
    Ok(written)
}

#[cfg(test)]
mod tests;
