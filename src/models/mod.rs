//! The game's 3D models: entities and meshes read from the game, its DLCs
//! and the mod ([`catalog`]), their `.mesh` and `.anim` files ([`mesh`],
//! [`anim`]), posed and drawn by a small software renderer ([`render`]) for
//! previews and the icons of ambient objects on the map.

pub mod anim;
pub mod catalog;
pub mod math;
pub mod mesh;
pub mod pdxbin;
pub mod render;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use rustc_hash::FxHashMap;

use anim::Anim;
use catalog::{AttachDef, Catalog, EntityDef, Pick, StateDef};
use math::{Affine, V3, v3};
use mesh::Mesh;
use render::{Camera, Frame, Proj, Texture, Vert};

/// Textures are made at most this wide for drawing.
const TEX_MAX: usize = 512;
/// Attached entities of attached entities... at most this deep.
const ATTACH_DEPTH: usize = 3;

/// What a model is doing.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub enum Play {
    /// Standing: the first frame of the default state's animation (which
    /// also hides what the animations hide, like a rifle in the other hand).
    #[default]
    Still,
    /// A state of the entity (what the game plays).
    State(String),
    /// An animation of the mesh by its id.
    Anim(String),
}

/// An entity or mesh with everything it needs to be drawn.
pub struct Model {
    pub entity: Option<EntityDef>,
    pub mesh: Option<Arc<Mesh>>,
    /// The mesh file.
    pub file: Option<PathBuf>,
    /// Per part of the mesh.
    pub tex: Vec<Arc<Texture>>,
    /// Entity scale × mesh scale.
    pub scale: f32,
    /// The mesh's animations by id (None: the file is missing or bad).
    pub anims: Vec<(String, Option<Arc<Anim>>)>,
    pub attach: Vec<(AttachDef, Arc<Model>)>,
    /// What could not be found or read.
    pub problems: Vec<String>,
}

/// An animation chosen for a model: the animation, its speed and whether it
/// loops.
struct Chosen<'a> {
    anim: &'a Anim,
    speed: f32,
    looping: bool,
}

/// Whether the mesh of `pick`, or of an entity it attaches, is skinned (its
/// vertices follow bones). The game draws such a model right only while it
/// is animated: standing still (`use_animation = no`), its bones are never
/// posed and its triangles fly across the screen.
pub fn is_skinned(cat: &Catalog, pick: &Pick) -> bool {
    fn go(cat: &Catalog, pick: &Pick, depth: usize) -> bool {
        let (mesh, attach): (Option<&str>, Vec<&str>) = match pick {
            Pick::Entity(e) => match cat.entity(e) {
                Some(d) => (d.pdxmesh.as_deref(), d.attach.iter().map(|a| a.entity.as_str()).collect()),
                None => return false,
            },
            Pick::Mesh(m) => (Some(m.as_str()), Vec::new()),
        };
        let own = mesh
            .and_then(|n| cat.mesh(n))
            .and_then(|md| cat.find(&md.file, Some(md.root)))
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| Mesh::parse(&b).ok())
            .is_some_and(|m| m.parts.iter().any(|p| p.skin.is_some()));
        own || (depth < 4 && attach.iter().any(|e| go(cat, &Pick::Entity(e.to_string()), depth + 1)))
    }
    go(cat, pick, 0)
}

/// What a skinned model placed standing still plays instead (see
/// [`is_skinned`]): its default state, else its "idle" state, else its
/// first state with an animation, else its mesh's "idle" or first
/// animation (for an entity, the build adds a state of that name). None:
/// there is nothing to play.
pub fn still_animation(cat: &Catalog, pick: &Pick) -> Option<String> {
    let mesh_anim = |name: &str| {
        let md = cat.mesh(name)?;
        md.anims.iter().find(|a| a.0.eq_ignore_ascii_case("idle")).or(md.anims.first()).map(|a| a.0.clone())
    };
    match pick {
        Pick::Entity(e) => {
            let d = cat.entity(e)?;
            let playable = |s: &&StateDef| s.animation.is_some();
            d.default_state
                .as_deref()
                .and_then(|n| d.state(n))
                .filter(playable)
                .or_else(|| d.state("idle").filter(playable))
                .or_else(|| d.states.iter().find(playable))
                .map(|s| s.name.clone())
                .or_else(|| d.pdxmesh.as_deref().and_then(mesh_anim))
        }
        Pick::Mesh(m) => mesh_anim(m),
    }
}

impl Model {
    /// See [`is_skinned`].
    pub fn skinned(&self) -> bool {
        self.mesh.as_ref().is_some_and(|m| m.parts.iter().any(|p| p.skin.is_some())) || self.attach.iter().any(|(_, m)| m.skinned())
    }

    pub fn state_names(&self) -> Vec<String> {
        self.entity.as_ref().map(EntityDef::state_names).unwrap_or_default()
    }

    pub fn anim(&self, id: &str) -> Option<&Arc<Anim>> {
        self.anims.iter().find(|a| a.0.eq_ignore_ascii_case(id)).and_then(|a| a.1.as_ref())
    }

    fn chosen(&self, play: &Play) -> Option<Chosen<'_>> {
        let state = |name: &str| -> Option<Chosen<'_>> {
            let st = self.entity.as_ref()?.state(name)?;
            Some(Chosen { anim: self.anim(st.animation.as_deref()?)?, speed: st.speed, looping: st.looping })
        };
        match play {
            Play::State(s) => state(s),
            Play::Anim(id) => self.anim(id).map(|a| Chosen { anim: a, speed: 1.0, looping: true }),
            Play::Still => {
                let d = self.entity.as_ref().and_then(|e| e.default_state.clone());
                d.and_then(|d| state(&d))
            }
        }
    }

    /// How long the played animation runs (seconds of the game, with the
    /// state's speed); None without one.
    pub fn duration(&self, play: &Play) -> Option<f32> {
        let c = self.chosen(play)?;
        Some(c.anim.duration() / c.speed.max(0.01))
    }

    /// Each bone's transform (bone → mesh space) at `time` seconds.
    pub fn globals(&self, play: &Play, time: f32) -> Vec<Affine> {
        let Some(mesh) = &self.mesh else { return Vec::new() };
        let t = if matches!(play, Play::Still) { 0.0 } else { time };
        let local = match self.chosen(play) {
            Some(c) => c.anim.pose(mesh, t * c.speed, c.looping),
            None => vec![None; mesh.bones.len()],
        };
        mesh.globals(&local)
    }

    /// Box of the bind pose, scaled (attachments left out).
    pub fn bounds(&self) -> (V3, V3) {
        match &self.mesh {
            Some(m) => (m.min * self.scale, m.max * self.scale),
            None => (v3(-0.5, 0.0, -0.5), v3(0.5, 1.0, 0.5)),
        }
    }

    /// Draw at `world` (model space → world), playing `play`.
    pub fn draw(&self, f: &mut Frame, cam: &Camera, world: &Affine, play: &Play, time: f32) {
        self.draw_at(f, cam, world, play, time, 0);
    }

    fn draw_at(&self, f: &mut Frame, cam: &Camera, world: &Affine, play: &Play, time: f32, depth: usize) {
        let Some(mesh) = &self.mesh else { return };
        let w = world.then_after(&Affine::scale(self.scale));
        let globals = self.globals(play, time);
        let skin: Vec<Affine> = mesh.bones.iter().zip(&globals).map(|(b, g)| g.then_after(&b.inv_bind)).collect();
        for (pi, part) in mesh.parts.iter().enumerate() {
            let verts: Vec<Vert> = (0..part.pos.len())
                .map(|i| {
                    let (mut p, mut n) = (part.pos[i], part.nrm.get(i).copied().unwrap_or(V3::ZERO));
                    if let Some(sk) = &part.skin
                        && !skin.is_empty()
                    {
                        let (bs, ws) = sk[i];
                        let mut m = Affine::ZERO;
                        let mut total = 0.0;
                        for k in 0..4 {
                            if bs[k] >= 0 && ws[k] > 0.0 && (bs[k] as usize) < skin.len() {
                                m.add_scaled(&skin[bs[k] as usize], ws[k]);
                                total += ws[k];
                            }
                        }
                        if total > 1e-6 {
                            let mut norm = Affine::ZERO;
                            norm.add_scaled(&m, 1.0 / total);
                            p = norm.apply(p);
                            n = norm.dir(n);
                        }
                    }
                    Vert { p: w.apply(p), n: w.dir(n).norm(), uv: part.uv[i] }
                })
                .collect();
            let tex = self.tex.get(pi).cloned().unwrap_or_else(|| Arc::new(Texture::solid([150, 150, 150, 255])));
            f.draw(cam, &verts, &part.tri, &tex, [1.0, 1.0, 1.0]);
        }
        if depth >= ATTACH_DEPTH {
            return;
        }
        for (a, child) in &self.attach {
            let Some(node) = mesh.node(&a.node, &globals) else { continue };
            // the child plays the parent's state if it has one of that name, else its default
            let child_play = match play {
                Play::Still => Play::Still,
                Play::State(s) if child.entity.as_ref().is_some_and(|e| e.state(s).is_some()) => Play::State(s.clone()),
                _ => child.entity.as_ref().and_then(|e| e.default_state.clone()).map(Play::State).unwrap_or(Play::Still),
            };
            // a node scaled to nothing hides what hangs on it
            let at = w.then_after(&node);
            if at.dir(v3(1.0, 1.0, 1.0)).len() < 1e-4 {
                continue;
            }
            child.draw_at(f, cam, &at, &child_play, time, depth + 1);
        }
    }
}

/// Loads models (and caches their files) for any thread.
pub struct Loader {
    pub cat: Arc<Catalog>,
    meshes: Mutex<FxHashMap<PathBuf, Result<Arc<Mesh>, String>>>,
    textures: Mutex<FxHashMap<PathBuf, Option<Arc<Texture>>>>,
    anims: Mutex<FxHashMap<PathBuf, Result<Arc<Anim>, String>>>,
    models: Mutex<FxHashMap<Pick, Arc<Model>>>,
}

/// Whether a texture's alpha cuts holes: leaves and fences have it either
/// fully on or fully off over a good part of the picture, while other
/// textures keep a mask of something else there (or none).
pub fn is_cutout(rgba: &[u8]) -> bool {
    let n = rgba.len() / 4;
    if n == 0 {
        return false;
    }
    let (mut clear, mut solid) = (0usize, 0usize);
    for a in rgba.chunks_exact(4).map(|c| c[3]) {
        if a < 24 {
            clear += 1;
        } else if a > 232 {
            solid += 1;
        }
    }
    let (c, s) = (clear as f64 / n as f64, solid as f64 / n as f64);
    c > 0.08 && s > 0.15 && c + s > 0.85
}

impl Loader {
    pub fn new(cat: Arc<Catalog>) -> Loader {
        Loader { cat, meshes: Default::default(), textures: Default::default(), anims: Default::default(), models: Default::default() }
    }

    fn mesh_file(&self, path: &Path) -> Result<Arc<Mesh>, String> {
        if let Some(m) = self.meshes.lock().get(path) {
            return m.clone();
        }
        let m = std::fs::read(path).map_err(|e| e.to_string()).and_then(|b| Mesh::parse(&b).map_err(|e| format!("{e:#}"))).map(Arc::new);
        self.meshes.lock().insert(path.to_path_buf(), m.clone());
        m
    }

    fn anim_file(&self, path: &Path) -> Result<Arc<Anim>, String> {
        if let Some(a) = self.anims.lock().get(path) {
            return a.clone();
        }
        let a = std::fs::read(path).map_err(|e| e.to_string()).and_then(|b| Anim::parse(&b).map_err(|e| format!("{e:#}"))).map(Arc::new);
        self.anims.lock().insert(path.to_path_buf(), a.clone());
        a
    }

    pub fn texture_file(&self, path: &Path) -> Option<Arc<Texture>> {
        if let Some(t) = self.textures.lock().get(path) {
            return t.clone();
        }
        let t = std::fs::read(path).ok().and_then(|b| {
            let name = path.to_string_lossy();
            let lower = name.to_lowercase();
            let (w, h, rgba) = if lower.ends_with(".tga") {
                crate::modbuild::img::read_tga_bytes(&b, &name).ok()?
            } else {
                crate::modbuild::img::read_dds_bytes(&b, &name).ok()?
            };
            Some(Arc::new(Texture::from_rgba(w as usize, h as usize, &rgba, TEX_MAX, is_cutout(&rgba))))
        });
        self.textures.lock().insert(path.to_path_buf(), t.clone());
        t
    }

    /// The model of an entity or mesh (cached; what is missing is listed in
    /// its problems).
    pub fn model(&self, pick: &Pick) -> Arc<Model> {
        self.model_at(pick, 0)
    }

    fn model_at(&self, pick: &Pick, depth: usize) -> Arc<Model> {
        if let Some(m) = self.models.lock().get(pick) {
            return m.clone();
        }
        let m = Arc::new(self.build(pick, depth));
        self.models.lock().insert(pick.clone(), m.clone());
        m
    }

    fn build(&self, pick: &Pick, depth: usize) -> Model {
        let cat = &self.cat;
        let mut problems = Vec::new();
        let entity = match pick {
            Pick::Entity(e) => {
                let d = cat.entity(e).cloned();
                if d.is_none() {
                    problems.push(format!("no entity {e} in the game or the mod"));
                }
                d
            }
            Pick::Mesh(_) => None,
        };
        let mesh_name = match pick {
            Pick::Entity(_) => entity.as_ref().and_then(|e| e.pdxmesh.clone()),
            Pick::Mesh(m) => Some(m.clone()),
        };
        let mesh_def = mesh_name.as_deref().and_then(|n| {
            let d = cat.mesh(n).cloned();
            if d.is_none() {
                problems.push(format!("no pdxmesh {n}"));
            }
            d
        });
        let mut model = Model {
            scale: entity.as_ref().map_or(1.0, |e| e.scale) * mesh_def.as_ref().map_or(1.0, |m| m.scale),
            entity,
            mesh: None,
            file: None,
            tex: Vec::new(),
            anims: Vec::new(),
            attach: Vec::new(),
            problems,
        };
        if let Some(md) = &mesh_def {
            match cat.find(&md.file, Some(md.root)) {
                None => model.problems.push(format!("{}: no file {}", md.name, md.file)),
                Some(path) => match self.mesh_file(&path) {
                    Err(e) => model.problems.push(format!("{}: {e}", md.file)),
                    Ok(mesh) => {
                        let dir = md.file.rsplit_once('/').map(|d| d.0).unwrap_or("");
                        for part in &mesh.parts {
                            let over = md.settings.iter().find(|s| s.shape.eq_ignore_ascii_case(&part.shape) && s.index == part.index).and_then(|s| s.diffuse.clone());
                            let name = over.unwrap_or_else(|| part.diffuse.clone());
                            let tex = if name.is_empty() {
                                None
                            } else {
                                // next to the mesh (in the mesh's own folder first), else a path
                                path.parent()
                                    .and_then(|d| catalog::resolve_ci(d, &name))
                                    .or_else(|| cat.find(&format!("{dir}/{name}"), Some(md.root)))
                                    .or_else(|| cat.find(&name, Some(md.root)))
                                    .and_then(|p| self.texture_file(&p))
                            };
                            if tex.is_none() && !name.is_empty() {
                                model.problems.push(format!("texture {name} not found or not readable"));
                            }
                            model.tex.push(tex.unwrap_or_else(|| Arc::new(Texture::solid([150, 150, 150, 255]))));
                        }
                        model.file = Some(path);
                        model.mesh = Some(mesh);
                    }
                },
            }
            for (id, ty) in &md.anims {
                let a = match cat.anim(ty) {
                    None => Err(format!("no animation {ty}")),
                    Some(ad) => match cat.anim_file(ad) {
                        None => Err(format!("animation {ty}: no file {}", ad.file)),
                        Some(p) => self.anim_file(&p),
                    },
                };
                if let Err(e) = &a {
                    model.problems.push(e.clone());
                }
                model.anims.push((id.clone(), a.ok()));
            }
        }
        if depth < ATTACH_DEPTH
            && let Some(e) = &model.entity
        {
            for a in &e.attach {
                if cat.entity(&a.entity).is_some() {
                    let child = self.model_at(&Pick::Entity(a.entity.clone()), depth + 1);
                    model.attach.push((a.clone(), child));
                }
            }
        }
        model
    }
}

/// How a preview looks at a model.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    /// Degrees clockwise from looking north.
    pub yaw: f32,
    /// Degrees down.
    pub pitch: f32,
    /// 1: the whole model fills the picture.
    pub zoom: f32,
    pub ground: bool,
}

impl Default for View {
    fn default() -> Self {
        View { yaw: 35.0, pitch: 22.0, zoom: 1.0, ground: true }
    }
}

/// A picture of a model, `w` × `h` pixels.
pub fn picture(m: &Model, w: usize, h: usize, view: View, play: &Play, time: f32, clear: [u8; 4]) -> Frame {
    let mut f = Frame::new(w, h, clear);
    let (lo, hi) = m.bounds();
    let centre = (lo + hi) * 0.5;
    let radius = ((hi - lo).len() * 0.5).max(0.05);
    let fov_half = 20f32.to_radians();
    let focal = h.min(w) as f32 / 2.0 / fov_half.tan();
    let dist = radius / fov_half.sin() / view.zoom.max(0.05);
    let cam = Camera::orbit(centre, view.yaw, view.pitch, dist, Proj::Persp { focal, near: dist * 0.01 });
    if view.ground {
        let (gv, gi) = render::ground_quad(lo.y.min(0.0), radius * 3.0, 24.0);
        f.draw(&cam, &gv, &gi, &render::ground_grid(), [1.0; 3]);
    }
    m.draw(&mut f, &cam, &Affine::IDENTITY, play, time);
    f
}

/// The model seen from straight above, north up, `px` pixels square with
/// the model's origin in the middle; and how many model units (map pixels)
/// half the picture spans.
pub fn top_icon(m: &Model, px: usize) -> (Frame, f32) {
    let (lo, hi) = m.bounds();
    let half = [lo.x.abs(), hi.x.abs(), lo.z.abs(), hi.z.abs()].into_iter().fold(0.05f32, f32::max) * 1.05;
    let mut f = Frame::new(px, px, [0, 0, 0, 0]);
    let height = (hi.y - lo.y).abs() + 10.0;
    let cam = Camera::orbit(v3(0.0, lo.y, 0.0), 0.0, 90.0, height + hi.y.abs(), Proj::Ortho { scale: px as f32 / 2.0 / half });
    m.draw(&mut f, &cam, &Affine::IDENTITY, &Play::Still, 0.0);
    (f, half)
}

#[cfg(test)]
mod tests;
