//! Where content can come from: the base game, the mods of the game's mod
//! folder (Steam Workshop, Paradox Mods, local), any folder laid out like a
//! mod, another nation of this project, or a bundle exported from one.
//!
//! [`index`] reads a source's definitions (kind, id, file and bytes), its
//! English localisation, its sprites and its country tags, so the content
//! screen can list them, show them and import them. Only what a definition
//! is and where it is is kept; its text is read again when needed.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, Result};
use rayon::prelude::*;

use super::kinds::{self, Kind, SPECS};
use super::loc;
use super::script::Script;
use super::store;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    Base,
    /// A mod of the game's mod folder (its name).
    Mod(String),
    Folder,
    /// A bundle file (.zip) exported from a nation.
    Bundle,
    /// A nation (package) of this project.
    Nation(String),
}

/// The files of a source.
#[derive(Clone)]
pub enum Files {
    Folder(PathBuf),
    /// Folders read as one, the first that has a file wins (the game and its DLCs).
    Layers(Arc<Vec<PathBuf>>),
    Memory(Arc<BTreeMap<String, Arc<Vec<u8>>>>),
}

/// The base game: its folder, then those of its DLCs (dlc/*, integrated_dlc/*).
pub fn base_files(game: &Path) -> Files {
    let mut roots = vec![game.to_path_buf()];
    for d in ["integrated_dlc", "dlc"] {
        if let Ok(rd) = std::fs::read_dir(game.join(d)) {
            let mut v: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
            v.sort();
            roots.extend(v);
        }
    }
    Files::Layers(Arc::new(roots))
}

impl Files {
    pub fn read(&self, rel: &str) -> Option<Arc<Vec<u8>>> {
        match self {
            Files::Folder(root) => std::fs::read(root.join(rel)).ok().map(Arc::new),
            Files::Layers(roots) => roots.iter().find_map(|r| std::fs::read(r.join(rel)).ok()).map(Arc::new),
            Files::Memory(m) => m.get(rel).cloned(),
        }
    }
    pub fn text(&self, rel: &str) -> Option<String> {
        let b = self.read(rel)?;
        let s = String::from_utf8_lossy(&b).into_owned();
        Some(s.strip_prefix('\u{feff}').map(str::to_string).unwrap_or(s))
    }
    pub fn exists(&self, rel: &str) -> bool {
        match self {
            Files::Folder(root) => root.join(rel).is_file(),
            Files::Layers(roots) => roots.iter().any(|r| r.join(rel).is_file()),
            Files::Memory(m) => m.contains_key(rel),
        }
    }
    /// Files in a folder (relative paths with `/`), with this extension
    /// (empty: any); `deep` goes into subfolders.
    pub fn list(&self, dir: &str, ext: &str, deep: bool) -> Vec<String> {
        let ok = |name: &str| ext.is_empty() || name.to_ascii_lowercase().ends_with(&format!(".{ext}"));
        match self {
            Files::Layers(roots) => {
                let mut v: BTreeSet<String> = BTreeSet::new();
                for r in roots.iter() {
                    v.extend(Files::Folder(r.clone()).list(dir, ext, deep));
                }
                v.into_iter().collect()
            }
            Files::Folder(root) => {
                let mut out = Vec::new();
                let base = root.join(dir);
                if deep {
                    if let Ok(v) = store::list_files(&base) {
                        out.extend(v.into_iter().filter(|p| ok(p)).map(|p| format!("{dir}/{p}")));
                    }
                } else if let Ok(rd) = std::fs::read_dir(&base) {
                    for e in rd.flatten() {
                        let n = e.file_name().to_string_lossy().into_owned();
                        if e.path().is_file() && ok(&n) {
                            out.push(format!("{dir}/{n}"));
                        }
                    }
                }
                out.sort();
                out
            }
            Files::Memory(m) => {
                let pre = format!("{dir}/");
                m.keys().filter(|k| k.starts_with(&pre) && ok(k) && (deep || !k[pre.len()..].contains('/'))).cloned().collect()
            }
        }
    }
    pub fn all(&self) -> Vec<String> {
        match self {
            Files::Folder(root) => store::list_files(root).unwrap_or_default(),
            Files::Layers(roots) => roots.iter().flat_map(|r| store::list_files(r).unwrap_or_default()).collect::<BTreeSet<_>>().into_iter().collect(),
            Files::Memory(m) => m.keys().cloned().collect(),
        }
    }
}

/// A definition of a source.
#[derive(Clone, Debug)]
pub struct SDef {
    pub kind: Kind,
    pub id: String,
    pub group: Option<String>,
    pub file: u32,
    pub span: std::ops::Range<usize>,
    pub tags: Vec<String>,
}

pub struct Index {
    pub label: String,
    pub origin: Origin,
    pub files: Files,
    /// Script files that were read (SDef::file indexes this).
    pub paths: Vec<String>,
    pub defs: Vec<SDef>,
    /// English localisation: key → text.
    pub loc: HashMap<String, String>,
    /// Sprite name → (texture file, frames).
    pub sprites: HashMap<String, (String, u32)>,
    /// Tags of common/country_tags.
    pub tags: BTreeSet<String>,
    /// `idea_token`s of advisors (they count as ideas: has_idea = token).
    pub idea_tokens: std::collections::HashSet<String>,
    /// Ids by kind (for lookups).
    by_id: HashMap<(Kind, String), Vec<usize>>,
    pub problems: Vec<String>,
}

impl Index {
    pub fn find(&self, kind: Kind, id: &str) -> Option<&SDef> {
        self.by_id.get(&(kind, id.to_string())).and_then(|v| v.first()).map(|&i| &self.defs[i])
    }
    /// Index of the (first) definition of this kind and id.
    pub fn find_index(&self, kind: Kind, id: &str) -> Option<usize> {
        self.by_id.get(&(kind, id.to_string())).and_then(|v| v.first()).copied()
    }
    pub fn has(&self, kind: Kind, id: &str) -> bool {
        self.by_id.contains_key(&(kind, id.to_string()))
    }
    /// Kinds an id is defined as.
    pub fn kinds_of(&self, id: &str) -> Vec<Kind> {
        Kind::all().filter(|k| self.has(*k, id)).collect()
    }
    pub fn of_kind(&self, kind: Kind) -> impl Iterator<Item = &SDef> {
        self.defs.iter().filter(move |d| d.kind == kind)
    }
    pub fn path(&self, d: &SDef) -> &str {
        &self.paths[d.file as usize]
    }
    /// The source text of a definition.
    pub fn text_of(&self, d: &SDef) -> Option<String> {
        let t = self.files.text(self.path(d))?;
        t.get(d.span.clone()).map(str::to_string)
    }
    /// Country tags a definition is for: those it names, its tree's
    /// countries, or the tag its id or file starts with.
    pub fn tags_of(&self, d: &SDef) -> Vec<String> {
        let mut t = d.tags.clone();
        if let Some(p) = kinds::tag_prefix(&d.id) {
            t.push(p.to_string());
        }
        let name = self.path(d).rsplit('/').next().unwrap_or("");
        if let Some(p) = name.get(..3).filter(|p| kinds::is_tag(p) && name[3..].starts_with([' ', '_', '.', '-'])) {
            t.push(p.to_string());
        }
        t.sort();
        t.dedup();
        t
    }
    /// The localisation of a key in a language (English is in memory; the
    /// others are read from the source's files).
    pub fn loc_in(&self, language: &str) -> HashMap<String, String> {
        if language == "english" {
            return self.loc.clone();
        }
        let mut out = HashMap::new();
        for f in loc_paths(&self.files, language) {
            if let Some(t) = self.files.text(&f) {
                for e in loc::parse(&t).entries {
                    out.insert(e.key, e.text);
                }
            }
        }
        out
    }
}

/// The localisation files of a language: localisation/<language>/** and
/// localisation/*_l_<language>.yml.
pub fn loc_paths(files: &Files, language: &str) -> Vec<String> {
    let suffix = format!("_l_{language}.yml");
    let mut v: Vec<String> = files.list(&format!("localisation/{language}"), "yml", true);
    v.extend(files.list("localisation", "yml", false).into_iter().filter(|p| p.to_ascii_lowercase().ends_with(&suffix)));
    // the game reads them in name order; replace/ last
    v.sort_by_key(|p| (p.contains("/replace/"), p.clone()));
    v
}

/// Read a source. `progress` gets a share done (0..1).
pub fn index(label: &str, origin: Origin, files: Files, progress: &(dyn Fn(f32) + Sync)) -> Result<Index> {
    let mut paths: BTreeSet<String> = BTreeSet::new();
    for s in SPECS {
        for dir in s.folders {
            let deep = s.kind == Kind::Sprite;
            paths.extend(files.list(dir, s.ext, deep));
        }
    }
    // Sprites are also in gfx/*.gfx in some mods
    paths.extend(files.list("gfx", "gfx", false));
    let paths: Vec<String> = paths.into_iter().collect();
    let n = paths.len().max(1);
    let done = std::sync::atomic::AtomicUsize::new(0);
    struct One {
        defs: Vec<kinds::Def>,
        sprites: Vec<(String, String, u32)>,
        tokens: Vec<String>,
        err: Option<String>,
    }
    let parsed: Vec<One> = paths
        .par_iter()
        .map(|p| {
            let r = match files.text(p) {
                Some(t) => {
                    let s = Script::parse(&t);
                    let name = p.rsplit('/').next().unwrap_or(p);
                    let defs = kinds::defs_of(&s, &kinds::kinds_of_path(p), name);
                    let mut sprites = Vec::new();
                    if p.ends_with(".gfx") {
                        for d in &defs {
                            if d.kind == Kind::Sprite
                                && let Some(b) = d.block(&s)
                            {
                                let tex = b.atom("texturefile").or_else(|| b.atom("textureFile")).or_else(|| b.atom("textureFile1")).unwrap_or("");
                                let frames = b.num("noOfFrames").unwrap_or(1.0).max(1.0) as u32;
                                sprites.push((d.id.clone(), tex.replace('\\', "/"), frames));
                            }
                        }
                    }
                    let tokens = if p.starts_with("common/characters/") { idea_tokens(&s.root) } else { Vec::new() };
                    One { defs, sprites, tokens, err: None }
                }
                None => One { defs: Vec::new(), sprites: Vec::new(), tokens: Vec::new(), err: Some(format!("cannot read {p}")) },
            };
            let k = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if k.is_multiple_of(64) {
                progress(0.8 * k as f32 / n as f32);
            }
            r
        })
        .collect();
    let mut defs = Vec::new();
    let mut sprites = HashMap::new();
    let mut problems = Vec::new();
    let mut tokens = std::collections::HashSet::new();
    for (i, one) in parsed.into_iter().enumerate() {
        if let Some(e) = one.err {
            problems.push(e);
        }
        tokens.extend(one.tokens);
        for d in one.defs {
            defs.push(SDef { kind: d.kind, id: d.id, group: d.group, file: i as u32, span: d.span, tags: d.tags });
        }
        for (name, tex, frames) in one.sprites {
            sprites.insert(name, (tex, frames));
        }
    }
    progress(0.85);
    let loc_files = loc_paths(&files, "english");
    let texts: Vec<Vec<(String, String)>> = loc_files.par_iter().map(|f| files.text(f).map(|t| loc::parse(&t).entries.into_iter().map(|e| (e.key, e.text)).collect()).unwrap_or_default()).collect();
    let mut locm = HashMap::new();
    for v in texts {
        locm.extend(v);
    }
    progress(0.95);
    let mut tags = BTreeSet::new();
    for f in files.list("common/country_tags", "txt", false) {
        if let Some(t) = files.text(&f) {
            for it in Script::parse(&t).root.items {
                if let Some(k) = it.key.as_ref().map(|k| k.text.clone())
                    && kinds::is_tag(&k)
                {
                    tags.insert(k);
                }
            }
        }
    }
    let mut by_id: HashMap<(Kind, String), Vec<usize>> = HashMap::new();
    for (i, d) in defs.iter().enumerate() {
        by_id.entry((d.kind, d.id.clone())).or_default().push(i);
    }
    progress(1.0);
    Ok(Index { label: label.to_string(), origin, files, paths, defs, loc: locm, sprites, tags, idea_tokens: tokens, by_id, problems })
}

/// The `idea_token`s of the advisors in a characters file.
pub fn idea_tokens(root: &super::script::Block) -> Vec<String> {
    let mut out = Vec::new();
    root.walk(&mut |i, _| {
        if i.is("idea_token")
            && let Some(t) = i.text()
        {
            out.push(t.to_string());
        }
    });
    out
}

/// A nation (package) of the store as a source.
pub fn of_package(st: &store::Store, pkg: &str) -> Result<Index> {
    let m: BTreeMap<String, Arc<Vec<u8>>> = st.package_bytes(pkg).into_iter().collect();
    index(&format!("nation {pkg}"), Origin::Nation(pkg.to_string()), Files::Memory(Arc::new(m)), &|_| {})
}

/// A bundle: the files of a zip under `files/`, and its manifest.
pub fn of_bundle(path: &Path) -> Result<(Index, serde_json::Value)> {
    let b = std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    let entries = super::zip::read(&b).with_context(|| format!("{} is not a bundle", path.display()))?;
    let mut m = BTreeMap::new();
    let mut manifest = serde_json::Value::Null;
    for (name, data) in entries {
        if name == "manifest.json" {
            manifest = serde_json::from_slice(&data).unwrap_or(serde_json::Value::Null);
        } else if let Some(rel) = name.strip_prefix("files/") {
            m.insert(rel.to_string(), Arc::new(data));
        }
    }
    let label = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    Ok((index(&label, Origin::Bundle, Files::Memory(Arc::new(m)), &|_| {})?, manifest))
}

/// The texture file of a sprite in a source, tolerant about the extension
/// (vanilla .gfx files name .tga files that are .dds now, and the game finds them).
pub fn texture_path(files: &Files, tex: &str) -> Option<String> {
    if tex.is_empty() {
        return None;
    }
    if files.exists(tex) {
        return Some(tex.to_string());
    }
    let stem = tex.rsplit_once('.').map(|x| x.0).unwrap_or(tex);
    ["dds", "tga", "png"].iter().map(|e| format!("{stem}.{e}")).find(|p| files.exists(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexes_memory_sources() {
        let mut m = BTreeMap::new();
        let put = |m: &mut BTreeMap<String, Arc<Vec<u8>>>, k: &str, v: &str| {
            m.insert(k.to_string(), Arc::new(v.as_bytes().to_vec()));
        };
        put(&mut m, "events/GER.txt", "add_namespace = germany\ncountry_event = { id = germany.1 trigger = { tag = GER } }");
        put(&mut m, "common/ideas/GER.txt", "ideas = { country = { GER_spirit = { picture = x } } }");
        put(&mut m, "interface/x.gfx", "spriteTypes = { spriteType = { name = \"GFX_idea_x\" texturefile = \"gfx/x.tga\" } }");
        put(&mut m, "gfx/x.dds", "DDS ");
        put(&mut m, "localisation/english/ger_l_english.yml", "\u{feff}l_english:\n GER_spirit:0 \"Spirit\"\n");
        put(&mut m, "localisation/german/ger_l_german.yml", "\u{feff}l_german:\n GER_spirit:0 \"Geist\"\n");
        put(&mut m, "common/country_tags/00.txt", "GER = \"countries/Germany.txt\"\n");
        let files = Files::Memory(Arc::new(m));
        let ix = index("test", Origin::Folder, files, &|_| {}).unwrap();
        assert!(ix.has(Kind::Event, "germany.1"));
        let d = ix.find(Kind::Idea, "GER_spirit").unwrap();
        assert_eq!(ix.tags_of(d), ["GER"]);
        assert!(ix.text_of(d).unwrap().starts_with("GER_spirit = {"));
        assert_eq!(ix.loc.get("GER_spirit").map(String::as_str), Some("Spirit"));
        assert_eq!(ix.loc_in("german").get("GER_spirit").map(String::as_str), Some("Geist"));
        assert_eq!(ix.sprites["GFX_idea_x"].0, "gfx/x.tga");
        assert_eq!(texture_path(&ix.files, "gfx/x.tga").as_deref(), Some("gfx/x.dds"));
        assert!(ix.tags.contains("GER"));
    }
}
