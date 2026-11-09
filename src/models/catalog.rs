//! What the game can show on the map: the entities, meshes (`pdxmesh`) and
//! animations defined in the `.asset` and `.gfx` files under `gfx/` of the
//! game, its DLCs and the mod's content packages.
//!
//! - `pdxmesh = { name file scale animation = { id type } meshsettings }`
//!   names a `.mesh` file (a path from the game folder);
//! - `animation = { name file }` an `.anim` file (next to its definition, or
//!   a path from the game folder);
//! - `entity = { name pdxmesh clone default_state state attach scale }` is
//!   what the map places: a mesh with named states that play animations,
//!   and other entities attached to its bones.

use std::path::{Path, PathBuf};

use rustc_hash::FxHashMap;

use crate::content::script::{Block, Script};

/// Where definitions and files come from, in the order a file is looked
/// for (the mod first, as the game reads a mod's files over its own).
#[derive(Clone, Debug, PartialEq)]
pub enum Origin {
    /// A content package of the map (`<map>/content/<package>`).
    Content(String),
    /// A DLC the player needs (its name, as `has_dlc` takes it).
    Dlc(String),
    /// A DLC that is part of the game now.
    Integrated(String),
    Game,
}

impl Origin {
    pub fn label(&self) -> String {
        match self {
            Origin::Content(p) => format!("mod: {p}"),
            Origin::Dlc(d) => format!("DLC: {d}"),
            Origin::Integrated(d) => format!("game ({d})"),
            Origin::Game => "game".into(),
        }
    }
    pub fn is_mod(&self) -> bool {
        matches!(self, Origin::Content(_))
    }
}

#[derive(Clone, Debug)]
pub struct Root {
    pub origin: Origin,
    pub dir: PathBuf,
}

#[derive(Clone, Debug, Default)]
pub struct MeshSetting {
    pub shape: String,
    pub index: usize,
    pub diffuse: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct MeshDef {
    pub name: String,
    /// Path from the game folder, with `/`.
    pub file: String,
    pub scale: f32,
    /// (id the states name, animation definition)
    pub anims: Vec<(String, String)>,
    pub settings: Vec<MeshSetting>,
    pub root: usize,
}

#[derive(Clone, Debug)]
pub struct AnimDef {
    pub name: String,
    /// As written: a file name next to `defined_in`, or a path.
    pub file: String,
    pub root: usize,
    pub defined_in: PathBuf,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StateDef {
    pub name: String,
    /// The mesh's animation id it plays.
    pub animation: Option<String>,
    pub speed: f32,
    pub looping: bool,
    pub chance: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AttachDef {
    /// The `name` of the attach block (what states propagate to).
    pub name: String,
    /// Bone or locator of the mesh.
    pub node: String,
    pub entity: String,
}

#[derive(Clone, Debug, Default)]
pub struct EntityDef {
    pub name: String,
    pub pdxmesh: Option<String>,
    pub default_state: Option<String>,
    pub states: Vec<StateDef>,
    pub attach: Vec<AttachDef>,
    pub scale: f32,
    pub clone_of: Option<String>,
    pub root: usize,
    pub defined_in: PathBuf,
}

impl EntityDef {
    /// State names, each once, the default first.
    pub fn state_names(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        if let Some(d) = &self.default_state {
            out.push(d.clone());
        }
        for s in &self.states {
            if !out.iter().any(|o| o.eq_ignore_ascii_case(&s.name)) {
                out.push(s.name.clone());
            }
        }
        out
    }
    pub fn state(&self, name: &str) -> Option<&StateDef> {
        self.states.iter().find(|s| s.name.eq_ignore_ascii_case(name))
    }
}

/// Something the map can place: an entity, or a mesh no entity shows (the
/// build makes an entity for it).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Pick {
    Entity(String),
    Mesh(String),
}

impl Pick {
    pub fn name(&self) -> &str {
        match self {
            Pick::Entity(n) | Pick::Mesh(n) => n,
        }
    }
}

/// One row of the model browser.
#[derive(Clone, Debug)]
pub struct Item {
    pub pick: Pick,
    /// Folder of the mesh under `gfx/models/` ("units/tanks"), or "(no model)".
    pub folder: String,
    pub root: usize,
    pub animated: bool,
    /// Lower-case name, folder and mesh file, for the search.
    pub haystack: String,
}

#[derive(Debug, Default)]
pub struct Catalog {
    pub roots: Vec<Root>,
    pub entities: Vec<EntityDef>,
    pub meshes: Vec<MeshDef>,
    pub anims: Vec<AnimDef>,
    by_entity: FxHashMap<String, usize>,
    by_mesh: FxHashMap<String, usize>,
    by_anim: FxHashMap<String, usize>,
    pub items: Vec<Item>,
    /// Files that could not be read or parsed.
    pub problems: Vec<String>,
}

fn num(b: &Block, k: &str) -> Option<f32> {
    b.atom(k).and_then(|v| v.trim().parse::<f32>().ok())
}

fn norm_path(p: &str) -> String {
    p.trim().replace('\\', "/").trim_start_matches('/').to_string()
}

/// `rel` under `dir`, matching each part regardless of case when the exact
/// path is not there (the game's files name files in other cases).
pub fn resolve_ci(dir: &Path, rel: &str) -> Option<PathBuf> {
    let exact = dir.join(rel);
    if exact.is_file() {
        return Some(exact);
    }
    let mut at = dir.to_path_buf();
    for part in rel.split('/').filter(|p| !p.is_empty()) {
        let direct = at.join(part);
        if direct.exists() {
            at = direct;
            continue;
        }
        let lower = part.to_lowercase();
        let found = std::fs::read_dir(&at).ok()?.flatten().find(|e| e.file_name().to_string_lossy().to_lowercase() == lower)?;
        at = found.path();
    }
    at.is_file().then_some(at)
}

impl Catalog {
    /// The roots for a game folder and the map's content packages.
    pub fn roots(game: &Path, content: Option<&Path>) -> Vec<Root> {
        let mut roots = Vec::new();
        if let Some(c) = content
            && let Ok(rd) = std::fs::read_dir(c)
        {
            let mut pk: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir() && !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'))).collect();
            pk.sort();
            // "shared" is read first by the build, so nations' files win
            pk.sort_by_key(|p| p.file_name().is_some_and(|n| n == "shared"));
            for p in pk {
                roots.push(Root { origin: Origin::Content(p.file_name().unwrap().to_string_lossy().into_owned()), dir: p });
            }
        }
        for (sub, integrated) in [("dlc", false), ("integrated_dlc", true)] {
            let Ok(rd) = std::fs::read_dir(game.join(sub)) else { continue };
            let mut ds: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.join("gfx").is_dir()).collect();
            ds.sort();
            for d in ds {
                let name = dlc_name(&d).unwrap_or_else(|| d.file_name().unwrap().to_string_lossy().into_owned());
                roots.push(Root { origin: if integrated { Origin::Integrated(name) } else { Origin::Dlc(name) }, dir: d });
            }
        }
        roots.push(Root { origin: Origin::Game, dir: game.to_path_buf() });
        roots
    }

    /// Read every definition under the roots' `gfx/` folders.
    pub fn load(roots: Vec<Root>) -> Catalog {
        let mut c = Catalog { roots, ..Default::default() };
        // later roots are read first, so the mod's definitions (first) win
        let mut files: Vec<(usize, PathBuf)> = Vec::new();
        for (ri, r) in c.roots.iter().enumerate().rev() {
            let mut stack = vec![r.dir.join("gfx")];
            let mut found = Vec::new();
            while let Some(d) = stack.pop() {
                let Ok(rd) = std::fs::read_dir(&d) else { continue };
                for e in rd.flatten() {
                    let p = e.path();
                    let n = e.file_name().to_string_lossy().to_lowercase();
                    if p.is_dir() {
                        // pictures only
                        if !matches!(n.as_str(), "interface" | "flags" | "leaders" | "event_pictures" | "loadingscreens" | "achievements" | "army_icons" | "fonts" | "texticons" | "keyicons" | "cursors") {
                            stack.push(p);
                        }
                    } else if n.ends_with(".asset") || n.ends_with(".gfx") {
                        found.push(p);
                    }
                }
            }
            found.sort();
            files.extend(found.into_iter().map(|f| (ri, f)));
        }
        let texts: Vec<(usize, PathBuf, Option<String>)> = {
            use rayon::prelude::*;
            files
                .into_par_iter()
                .map(|(ri, f)| {
                    let t = std::fs::read(&f).ok().map(|b| {
                        let s = String::from_utf8_lossy(&b).into_owned();
                        s.strip_prefix('\u{feff}').map(str::to_string).unwrap_or(s)
                    });
                    (ri, f, t)
                })
                .collect()
        };
        let mut raw_entities: Vec<EntityDef> = Vec::new();
        for (ri, f, t) in texts {
            let Some(t) = t else {
                c.problems.push(format!("cannot read {}", f.display()));
                continue;
            };
            let lower = t.to_lowercase();
            if !(lower.contains("pdxmesh") || lower.contains("entity") || lower.contains("animation")) {
                continue;
            }
            let s = Script::parse(&t);
            let mut blocks: Vec<&Block> = vec![&s.root];
            blocks.extend(s.root.blocks("objectTypes"));
            for b in blocks {
                for it in &b.items {
                    let Some(v) = it.block() else { continue };
                    if it.is("pdxmesh") {
                        if let Some(m) = mesh_def(v, ri) {
                            c.put_mesh(m);
                        }
                    } else if it.is("animation") {
                        if let (Some(name), Some(file)) = (v.atom("name"), v.atom("file")) {
                            let a = AnimDef { name: name.to_string(), file: norm_path(file), root: ri, defined_in: f.clone() };
                            let i = c.anims.len();
                            c.by_anim.insert(a.name.to_lowercase(), i);
                            c.anims.push(a);
                        }
                    } else if it.is("entity")
                        && let Some(e) = entity_def(v, ri, &f)
                    {
                        raw_entities.push(e);
                    }
                }
            }
        }
        // clones: start from what they clone, then their own keys
        let by_raw: FxHashMap<String, usize> = raw_entities.iter().enumerate().map(|(i, e)| (e.name.to_lowercase(), i)).collect();
        fn resolve(i: usize, raw: &[EntityDef], by: &FxHashMap<String, usize>, depth: usize) -> EntityDef {
            let e = &raw[i];
            let Some(base) = e.clone_of.as_ref().and_then(|c| by.get(&c.to_lowercase())).filter(|&&b| b != i && depth < 16) else { return e.clone() };
            let mut out = resolve(*base, raw, by, depth + 1);
            out.name = e.name.clone();
            out.clone_of = e.clone_of.clone();
            out.root = e.root;
            out.defined_in = e.defined_in.clone();
            if e.pdxmesh.is_some() {
                out.pdxmesh = e.pdxmesh.clone();
            }
            if e.default_state.is_some() {
                out.default_state = e.default_state.clone();
            }
            if e.scale != 1.0 {
                out.scale = e.scale;
            }
            for s in &e.states {
                out.states.retain(|o| !o.name.eq_ignore_ascii_case(&s.name));
            }
            out.states.extend(e.states.iter().cloned());
            for a in &e.attach {
                out.attach.retain(|o| o.name != a.name || a.name.is_empty());
            }
            out.attach.extend(e.attach.iter().cloned());
            out
        }
        for i in 0..raw_entities.len() {
            let e = resolve(i, &raw_entities, &by_raw, 0);
            let k = e.name.to_lowercase();
            // a later definition of the same name (an earlier root) wins
            match c.by_entity.get(&k) {
                Some(&j) => c.entities[j] = e,
                None => {
                    c.by_entity.insert(k, c.entities.len());
                    c.entities.push(e);
                }
            }
        }
        c.make_items();
        c
    }

    fn put_mesh(&mut self, m: MeshDef) {
        let k = m.name.to_lowercase();
        match self.by_mesh.get(&k) {
            Some(&j) => self.meshes[j] = m,
            None => {
                self.by_mesh.insert(k, self.meshes.len());
                self.meshes.push(m);
            }
        }
    }

    fn make_items(&mut self) {
        let mut used = rustc_hash::FxHashSet::default();
        let mut items = Vec::new();
        for e in &self.entities {
            let m = e.pdxmesh.as_deref().and_then(|m| self.mesh(m));
            if let Some(m) = m {
                used.insert(m.name.to_lowercase());
            }
            let folder = m.map(|m| folder_of(&m.file)).unwrap_or_else(|| "(no model)".into());
            let animated = m.is_some_and(|m| !m.anims.is_empty());
            let file = m.map(|m| m.file.as_str()).unwrap_or("");
            items.push(Item { haystack: format!("{} {} {}", e.name, folder, file).to_lowercase(), pick: Pick::Entity(e.name.clone()), folder, root: e.root, animated });
        }
        for m in &self.meshes {
            if used.contains(&m.name.to_lowercase()) {
                continue;
            }
            let folder = folder_of(&m.file);
            items.push(Item {
                haystack: format!("{} {} {}", m.name, folder, m.file).to_lowercase(),
                pick: Pick::Mesh(m.name.clone()),
                folder,
                root: m.root,
                animated: !m.anims.is_empty(),
            });
        }
        items.sort_by(|a, b| a.folder.cmp(&b.folder).then_with(|| a.pick.name().to_lowercase().cmp(&b.pick.name().to_lowercase())));
        self.items = items;
    }

    pub fn entity(&self, name: &str) -> Option<&EntityDef> {
        self.by_entity.get(&name.to_lowercase()).map(|&i| &self.entities[i])
    }
    pub fn mesh(&self, name: &str) -> Option<&MeshDef> {
        self.by_mesh.get(&name.to_lowercase()).map(|&i| &self.meshes[i])
    }
    pub fn anim(&self, name: &str) -> Option<&AnimDef> {
        self.by_anim.get(&name.to_lowercase()).map(|&i| &self.anims[i])
    }
    #[cfg(test)]
    pub fn item(&self, p: &Pick) -> Option<&Item> {
        self.items.iter().find(|i| &i.pick == p)
    }
    /// The root a pick is defined in.
    pub fn origin_of(&self, p: &Pick) -> Option<&Origin> {
        let r = match p {
            Pick::Entity(e) => self.entity(e)?.root,
            Pick::Mesh(m) => self.mesh(m)?.root,
        };
        self.roots.get(r).map(|r| &r.origin)
    }

    /// A file by its path from the game folder: in `first`'s root, then in
    /// every root in order.
    pub fn find(&self, rel: &str, first: Option<usize>) -> Option<PathBuf> {
        let rel = norm_path(rel);
        first
            .into_iter()
            .chain(0..self.roots.len())
            .filter_map(|ri| self.roots.get(ri))
            .find_map(|r| resolve_ci(&r.dir, &rel))
    }

    /// The `.anim` file of an animation definition.
    pub fn anim_file(&self, a: &AnimDef) -> Option<PathBuf> {
        if !a.file.contains('/')
            && let Some(dir) = a.defined_in.parent()
            && let Some(p) = resolve_ci(dir, &a.file)
        {
            return Some(p);
        }
        // a path from the game folder, or one next to the definition's folder there
        if let Some(p) = self.find(&a.file, Some(a.root)) {
            return Some(p);
        }
        let root = &self.roots.get(a.root)?.dir;
        let rel_dir = a.defined_in.parent()?.strip_prefix(root).ok()?;
        self.find(&format!("{}/{}", rel_dir.to_string_lossy().replace('\\', "/"), a.file), Some(a.root))
    }

    /// Folders of the items with counts, sorted.
    #[cfg(test)]
    pub fn folders(&self) -> Vec<(String, usize)> {
        let mut m: std::collections::BTreeMap<String, usize> = Default::default();
        for it in &self.items {
            *m.entry(it.folder.clone()).or_default() += 1;
        }
        m.into_iter().collect()
    }
}

fn folder_of(file: &str) -> String {
    let f = file.to_lowercase();
    let f = f.strip_prefix("gfx/models/").or_else(|| f.strip_prefix("gfx/")).unwrap_or(&f);
    match f.rfind('/') {
        Some(i) => f[..i].to_string(),
        None => "(top)".into(),
    }
}

fn dlc_name(dir: &Path) -> Option<String> {
    let rd = std::fs::read_dir(dir).ok()?;
    for e in rd.flatten() {
        if e.path().extension().is_some_and(|x| x == "dlc") {
            let t = std::fs::read_to_string(e.path()).ok()?;
            return Script::parse(&t).root.atom("name").map(str::to_string);
        }
    }
    None
}

fn mesh_def(b: &Block, root: usize) -> Option<MeshDef> {
    let name = b.atom("name")?.to_string();
    let file = norm_path(b.atom("file")?);
    let anims = b.blocks("animation").filter_map(|a| Some((a.atom("id")?.to_string(), a.atom("type")?.to_string()))).collect();
    let settings = b
        .blocks("meshsettings")
        .map(|s| MeshSetting {
            shape: s.atom("name").unwrap_or("").to_string(),
            index: s.atom("index").and_then(|v| v.parse().ok()).unwrap_or(0),
            diffuse: s.atom("texture_diffuse").map(str::to_string),
        })
        .collect();
    Some(MeshDef { name, file, scale: num(b, "scale").unwrap_or(1.0), anims, settings, root })
}

fn entity_def(b: &Block, root: usize, f: &Path) -> Option<EntityDef> {
    let name = b.atom("name")?.to_string();
    let states = b
        .blocks("state")
        .filter_map(|s| {
            Some(StateDef {
                name: s.atom("name")?.to_string(),
                animation: s.atom("animation").map(str::to_string),
                speed: num(s, "animation_speed").unwrap_or(1.0),
                looping: !s.atom("looping").is_some_and(|v| v.eq_ignore_ascii_case("no")),
                chance: num(s, "chance").unwrap_or(1.0),
            })
        })
        .collect();
    let attach = b
        .blocks("attach")
        .flat_map(|a| {
            let n = a.atom("name").unwrap_or("").to_string();
            a.items
                .iter()
                .filter(|i| !i.is("name") && !i.is("alpha_blend") && !i.is("scale"))
                .filter_map(|i| Some(AttachDef { name: n.clone(), node: i.key.as_ref()?.text.clone(), entity: i.text()?.to_string() }))
                .collect::<Vec<_>>()
        })
        .collect();
    Some(EntityDef {
        name,
        pdxmesh: b.atom("pdxmesh").map(str::to_string),
        default_state: b.atom("default_state").map(str::to_string),
        states,
        attach,
        scale: num(b, "scale").unwrap_or(1.0),
        clone_of: b.atom("clone").map(str::to_string),
        root,
        defined_in: f.to_path_buf(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, rel: &str, text: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    #[test]
    fn definitions_clones_and_files() {
        let tmp = std::env::temp_dir().join(format!("mh_catalog_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let (game, content) = (tmp.join("game"), tmp.join("content"));
        write(
            &game,
            "gfx/entities/units.gfx",
            "objectTypes = {\n pdxmesh = { name = \"tank_mesh\" file = \"gfx/models/units/Tanks/tank.mesh\" scale = 0.5\n  animation = { id = \"idle\" type = \"tank_idle_anim\" } }\n pdxmesh = { name = \"tree_mesh\" file = \"gfx/models/mapitems/trees/tree.mesh\" }\n}",
        );
        write(
            &game,
            "gfx/entities/units.asset",
            "entity = { name = \"tank_entity\" pdxmesh = \"tank_mesh\" default_state = \"idle\" state = { name = \"idle\" animation = \"idle\" animation_speed = 2 } attach = { name = \"gun\" turret_node = \"gun_entity\" } scale = 2 }\n\
             entity = { name = \"GER_tank_entity\" clone = \"tank_entity\" state = { name = \"move\" animation = \"idle\" looping = no } }",
        );
        write(&game, "gfx/models/units/animation.asset", "animation = { name = \"tank_idle_anim\" file = \"tank_idle.anim\" }");
        write(&game, "gfx/models/units/tank_idle.anim", "x");
        write(&game, "gfx/models/units/tanks/tank.mesh", "x");
        write(&game, "dlc/dlc099_test/dlc099.dlc", "name = \"Test Pack\"\n");
        write(&game, "dlc/dlc099_test/gfx/entities/x.asset", "entity = { name = \"dlc_entity\" pdxmesh = \"tree_mesh\" }\nentity = { name = \"dlc_tree_entity\" pdxmesh = \"tree_mesh\" }");
        write(&content, "shared/gfx/entities/mine.asset", "entity = { name = \"dlc_entity\" pdxmesh = \"tank_mesh\" }");
        let c = Catalog::load(Catalog::roots(&game, Some(&content)));
        assert_eq!(c.roots.iter().map(|r| r.origin.label()).collect::<Vec<_>>(), ["mod: shared", "DLC: Test Pack", "game"]);
        // the mod's definition wins
        assert_eq!(c.entity("dlc_entity").unwrap().pdxmesh.as_deref(), Some("tank_mesh"));
        assert_eq!(c.origin_of(&Pick::Entity("dlc_entity".into())), Some(&Origin::Content("shared".into())));
        // a clone has what it clones, and its own states on top
        let ger = c.entity("GER_tank_entity").unwrap();
        assert_eq!(ger.state_names(), ["idle", "move"]);
        assert_eq!(ger.attach[0].node, "turret_node");
        assert_eq!(ger.scale, 2.0);
        assert!(!ger.state("move").unwrap().looping);
        assert_eq!(ger.state("idle").unwrap().speed, 2.0);
        let m = c.mesh("tank_mesh").unwrap();
        assert_eq!((m.scale, m.anims[0].1.as_str()), (0.5, "tank_idle_anim"));
        // case of the folders differs from the file's
        assert_eq!(c.find(&m.file, None), Some(game.join("gfx/models/units/tanks/tank.mesh")));
        assert_eq!(c.anim_file(c.anim("tank_idle_anim").unwrap()), Some(game.join("gfx/models/units/tank_idle.anim")));
        // tank_mesh is shown by GER_tank_entity, tree_mesh by entities too
        assert!(c.items.iter().all(|i| matches!(i.pick, Pick::Entity(_))));
        assert_eq!(c.origin_of(&Pick::Entity("dlc_tree_entity".into())), Some(&Origin::Dlc("Test Pack".into())));
        assert_eq!(c.item(&Pick::Entity("GER_tank_entity".into())).unwrap().folder, "units/tanks");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
