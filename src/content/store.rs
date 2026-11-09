//! The content of the project: `<map folder>/content/<package>/`, one
//! package per nation (its tag) and `shared` for the mod's own, each laid
//! out like a mod (common/, events/, history/, localisation/, gfx/,
//! interface/). The build copies them into the mod.
//!
//! Files are read into memory when the content screen opens; edits change
//! them there (with undo) and Save writes the changed ones. The previous
//! version of a file that is overwritten or deleted goes to
//! `content/.backups/<time>/`, so a save never loses what was on disk.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};

use super::kinds::{self, Def, Kind};
use super::loc::{self, LocFile};
use super::script::Script;

pub const SHARED: &str = "shared";
/// Folders of the content folder that are not packages.
const SKIP: [&str; 3] = [".backups", ".snapshots", ".git"];

#[derive(Clone, Debug, PartialEq)]
pub enum Body {
    Text(Arc<String>),
    /// Bytes added in this session (not yet on disk).
    Bytes(Arc<Vec<u8>>),
    /// A binary file as it is on disk.
    OnDisk,
}

impl Body {
    pub fn text(&self) -> Option<&str> {
        match self {
            Body::Text(t) => Some(t.as_str()),
            _ => None,
        }
    }
}

/// One file of a package.
pub struct SrcFile {
    pub body: Body,
    /// Started with a byte order mark (kept when written back).
    pub bom: bool,
    /// What is on disk (None: not there).
    pub disk: Option<Body>,
    pub script: Option<Script>,
    pub loc: Option<LocFile>,
    pub defs: Vec<Def>,
}

fn is_text(path: &str) -> bool {
    let ext = path.rsplit_once('.').map(|x| x.1.to_ascii_lowercase()).unwrap_or_default();
    matches!(ext.as_str(), "txt" | "yml" | "gfx" | "gui" | "lua" | "asset" | "csv" | "md" | "json" | "mod")
}

impl SrcFile {
    fn new(path: &str, body: Body, bom: bool, disk: Option<Body>) -> SrcFile {
        let mut f = SrcFile { body, bom, disk, script: None, loc: None, defs: Vec::new() };
        f.reparse(path);
        f
    }
    fn reparse(&mut self, path: &str) {
        self.script = None;
        self.loc = None;
        self.defs.clear();
        let Some(text) = self.body.text() else { return };
        let lower = path.to_ascii_lowercase();
        if lower.ends_with(".yml") {
            self.loc = Some(loc::parse(text));
        } else if lower.ends_with(".txt") || lower.ends_with(".gfx") || lower.ends_with(".gui") {
            let s = Script::parse(text);
            let name = path.rsplit('/').next().unwrap_or(path);
            self.defs = kinds::defs_of(&s, &kinds::kinds_of_path(path), name);
            self.script = Some(s);
        }
    }
    pub fn text(&self) -> Option<&str> {
        self.body.text()
    }
    pub fn dirty(&self) -> bool {
        self.disk.as_ref() != Some(&self.body)
    }
}

#[derive(Default)]
pub struct Package {
    pub files: BTreeMap<String, SrcFile>,
    /// Files deleted in this session that are on disk.
    pub removed: BTreeSet<String>,
}

impl Package {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

/// One file before and after a step.
#[derive(Clone)]
struct Change {
    pkg: String,
    path: String,
    before: Option<(Body, bool)>,
    after: Option<(Body, bool)>,
}

struct Step {
    label: String,
    changes: Vec<Change>,
    /// Steps with the same key close together merge (typing in a field).
    merge: Option<String>,
    at: Instant,
}

/// Where a definition is: package, file, kind and id (and which one of
/// that id in the file when ids repeat).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DefKey {
    pub pkg: String,
    pub file: String,
    pub kind: Kind,
    pub id: String,
    pub nth: usize,
}

pub struct Store {
    /// `<map folder>/content`.
    pub root: PathBuf,
    pub packages: BTreeMap<String, Package>,
    undo: Vec<Step>,
    redo: Vec<Step>,
    open: Option<Step>,
    /// Goes up with every change (for caches).
    pub generation: u64,
}

fn read_body(path: &Path, rel: &str) -> Result<(Body, bool)> {
    if is_text(rel) {
        let b = std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
        let bom = b.starts_with(&[0xef, 0xbb, 0xbf]);
        let s = String::from_utf8_lossy(&b);
        // the mark is the flag, never in the text (a file written with two keeps one)
        Ok((Body::Text(Arc::new(s.trim_start_matches('\u{feff}').to_string())), bom))
    } else {
        Ok((Body::OnDisk, false))
    }
}

fn walk(dir: &Path, base: &Path, out: &mut Vec<String>) -> Result<()> {
    let rd = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).with_context(|| format!("cannot list {}", dir.display())),
    };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        if p.is_dir() {
            walk(&p, base, out)?;
        } else if !name.ends_with(".bak") && !name.ends_with('~') {
            out.push(p.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/"));
        }
    }
    Ok(())
}

/// Files of a folder laid out like a mod: paths relative to it, with `/`.
pub fn list_files(dir: &Path) -> Result<Vec<String>> {
    let mut v = Vec::new();
    walk(dir, dir, &mut v)?;
    v.sort();
    Ok(v)
}

/// A package name is `shared` or a tag.
pub fn valid_package(name: &str) -> bool {
    name == SHARED || kinds::is_tag(name)
}

impl Store {
    pub fn empty(root: PathBuf) -> Store {
        Store { root, packages: BTreeMap::new(), undo: Vec::new(), redo: Vec::new(), open: None, generation: 1 }
    }

    /// Read `<map_dir>/content` (missing: an empty store).
    pub fn load(map_dir: &Path) -> Result<Store> {
        let root = map_dir.join("content");
        let mut st = Store::empty(root.clone());
        let Ok(rd) = std::fs::read_dir(&root) else { return Ok(st) };
        let mut names: Vec<String> = rd.flatten().filter(|e| e.path().is_dir()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        for name in names {
            if SKIP.contains(&name.as_str()) || name.starts_with('.') {
                continue;
            }
            let dir = root.join(&name);
            let mut pkg = Package::default();
            for rel in list_files(&dir)? {
                let (body, bom) = read_body(&dir.join(&rel), &rel)?;
                pkg.files.insert(rel.clone(), SrcFile::new(&rel, body.clone(), bom, Some(body)));
            }
            st.packages.insert(name, pkg);
        }
        Ok(st)
    }

    pub fn package(&self, name: &str) -> Option<&Package> {
        self.packages.get(name)
    }

    pub fn file(&self, pkg: &str, path: &str) -> Option<&SrcFile> {
        self.packages.get(pkg)?.files.get(path)
    }

    pub fn text(&self, pkg: &str, path: &str) -> Option<&str> {
        self.file(pkg, path)?.text()
    }

    /// Unsaved changes.
    pub fn dirty(&self) -> bool {
        self.packages.values().any(|p| !p.removed.is_empty() || p.files.values().any(|f| f.dirty()))
    }

    // ---------------------------------------------------------- definitions

    pub fn defs(&self) -> impl Iterator<Item = DefKey> + '_ {
        self.packages.iter().flat_map(|(n, p)| {
            p.files.iter().flat_map(move |(path, f)| {
                let mut seen: BTreeMap<(Kind, &str), usize> = BTreeMap::new();
                f.defs.iter().map(move |d| {
                    let nth = seen.entry((d.kind, d.id.as_str())).or_default();
                    let k = DefKey { pkg: n.clone(), file: path.clone(), kind: d.kind, id: d.id.clone(), nth: *nth };
                    *nth += 1;
                    k
                })
            })
        })
    }

    /// The definitions of a package (all of them when `pkg` is None) of a kind.
    pub fn defs_of(&self, pkg: Option<&str>, kind: Kind) -> Vec<DefKey> {
        self.defs().filter(|k| k.kind == kind && pkg.is_none_or(|p| p == k.pkg)).collect()
    }

    /// The definition, its file's text and parse.
    pub fn def(&self, key: &DefKey) -> Option<(&Def, &str, &Script)> {
        let f = self.file(&key.pkg, &key.file)?;
        let s = f.script.as_ref()?;
        let d = f.defs.iter().filter(|d| d.kind == key.kind && d.id == key.id).nth(key.nth)?;
        Some((d, f.text()?, s))
    }

    /// Find a definition by kind and id anywhere (the first one).
    pub fn find(&self, kind: Kind, id: &str) -> Option<DefKey> {
        self.defs().find(|k| k.kind == kind && k.id == id)
    }

    /// Find by id in a package, of any of the kinds.
    pub fn find_in(&self, pkg: &str, kinds: &[Kind], id: &str) -> Option<DefKey> {
        self.defs().find(|k| k.pkg == pkg && kinds.contains(&k.kind) && k.id == id)
    }

    // ---------------------------------------------------------- changes

    /// Start an undo step (ended by [`Store::commit`]).
    pub fn begin(&mut self, label: impl Into<String>) {
        self.begin_merge(label, None);
    }

    /// Start a step that merges with the last one when it has the same key
    /// and came within two seconds (one undo for a word typed in a field).
    pub fn begin_merge(&mut self, label: impl Into<String>, merge: Option<String>) {
        if let Some(s) = self.open.take() {
            self.finish(s);
        }
        self.open = Some(Step { label: label.into(), changes: Vec::new(), merge, at: Instant::now() });
    }

    fn record(&mut self, pkg: &str, path: &str, after: Option<(Body, bool)>) {
        let before = self.file(pkg, path).map(|f| (f.body.clone(), f.bom));
        let step = self.open.get_or_insert_with(|| Step { label: "Edit".into(), changes: Vec::new(), merge: None, at: Instant::now() });
        if let Some(c) = step.changes.iter_mut().find(|c| c.pkg == pkg && c.path == path) {
            c.after = after;
        } else {
            step.changes.push(Change { pkg: pkg.to_string(), path: path.to_string(), before, after });
        }
    }

    fn put(&mut self, pkg: &str, path: &str, v: Option<(Body, bool)>) {
        self.generation += 1;
        match v {
            Some((body, bom)) => {
                let p = self.packages.entry(pkg.to_string()).or_default();
                p.removed.remove(path);
                match p.files.get_mut(path) {
                    Some(f) => {
                        f.body = body;
                        f.bom = bom;
                        f.reparse(path);
                    }
                    None => {
                        p.files.insert(path.to_string(), SrcFile::new(path, body, bom, None));
                    }
                }
            }
            None => {
                if let Some(p) = self.packages.get_mut(pkg)
                    && let Some(f) = p.files.remove(path)
                    && f.disk.is_some()
                {
                    p.removed.insert(path.to_string());
                }
            }
        }
    }

    /// Set a text file (made if missing). New `.yml` files get a byte order mark.
    pub fn set_text(&mut self, pkg: &str, path: &str, text: String) {
        // a byte order mark in the text is the file's mark (written once, on save)
        let marked = text.starts_with('\u{feff}');
        let text = if marked { text.trim_start_matches('\u{feff}').to_string() } else { text };
        if self.text(pkg, path) == Some(text.as_str()) {
            return;
        }
        let bom = marked || self.file(pkg, path).map(|f| f.bom).unwrap_or_else(|| path.to_ascii_lowercase().ends_with(".yml"));
        let v = Some((Body::Text(Arc::new(text)), bom));
        self.record(pkg, path, v.clone());
        self.put(pkg, path, v);
    }

    pub fn set_bytes(&mut self, pkg: &str, path: &str, bytes: Vec<u8>) {
        let v = Some((Body::Bytes(Arc::new(bytes)), false));
        self.record(pkg, path, v.clone());
        self.put(pkg, path, v);
    }

    pub fn remove(&mut self, pkg: &str, path: &str) {
        if self.file(pkg, path).is_none() {
            return;
        }
        self.record(pkg, path, None);
        self.put(pkg, path, None);
    }

    /// Change a text file with a function of its text ("" when missing).
    pub fn edit_text(&mut self, pkg: &str, path: &str, f: impl FnOnce(&str) -> String) {
        let old = self.text(pkg, path).unwrap_or("").to_string();
        let new = f(&old);
        if new != old || self.file(pkg, path).is_none() {
            self.set_text(pkg, path, new);
        }
    }

    /// The bytes of a file: in memory, or read from disk.
    pub fn bytes(&self, pkg: &str, path: &str) -> Option<Arc<Vec<u8>>> {
        let f = self.file(pkg, path)?;
        match &f.body {
            Body::Text(t) => {
                let mut v = if f.bom { vec![0xef, 0xbb, 0xbf] } else { Vec::new() };
                v.extend_from_slice(t.as_bytes());
                Some(Arc::new(v))
            }
            Body::Bytes(b) => Some(b.clone()),
            Body::OnDisk => std::fs::read(self.root.join(pkg).join(path)).ok().map(Arc::new),
        }
    }

    /// End the step; nothing changed: no step.
    pub fn commit(&mut self) {
        if let Some(s) = self.open.take() {
            self.finish(s);
        }
    }

    fn finish(&mut self, s: Step) {
        let changes: Vec<Change> = s.changes.into_iter().filter(|c| c.before != c.after).collect();
        if changes.is_empty() {
            return;
        }
        self.redo.clear();
        if let (Some(key), Some(last)) = (&s.merge, self.undo.last_mut())
            && last.merge.as_ref() == Some(key)
            && last.at.elapsed() < Duration::from_secs(2)
        {
            for c in changes {
                if let Some(o) = last.changes.iter_mut().find(|o| o.pkg == c.pkg && o.path == c.path) {
                    o.after = c.after;
                } else {
                    last.changes.push(c);
                }
            }
            last.at = Instant::now();
            return;
        }
        self.undo.push(Step { label: s.label, changes, merge: s.merge, at: Instant::now() });
        if self.undo.len() > 300 {
            self.undo.remove(0);
        }
    }

    /// Drop the open step, putting its files back.
    pub fn rollback(&mut self) {
        if let Some(s) = self.open.take() {
            for c in s.changes.into_iter().rev() {
                self.put(&c.pkg, &c.path, c.before);
            }
        }
    }

    pub fn can_undo(&self) -> Option<&str> {
        self.undo.last().map(|s| s.label.as_str())
    }
    pub fn can_redo(&self) -> Option<&str> {
        self.redo.last().map(|s| s.label.as_str())
    }

    pub fn undo(&mut self) -> Option<String> {
        self.commit();
        let s = self.undo.pop()?;
        for c in s.changes.iter().rev() {
            self.put(&c.pkg, &c.path, c.before.clone());
        }
        let l = s.label.clone();
        self.redo.push(s);
        Some(l)
    }

    pub fn redo(&mut self) -> Option<String> {
        self.commit();
        let s = self.redo.pop()?;
        for c in &s.changes {
            self.put(&c.pkg, &c.path, c.after.clone());
        }
        let l = s.label.clone();
        self.undo.push(s);
        Some(l)
    }

    // ---------------------------------------------------------- disk

    /// Write the changed files (and delete the removed ones). What they
    /// replace on disk goes to `.backups/<time>/` first. Returns the number of
    /// files written or removed.
    pub fn save(&mut self) -> Result<usize> {
        self.commit();
        let stamp = crate::editor::model::timestamp();
        let mut backup = self.root.join(".backups").join(&stamp);
        let mut k = 2;
        while backup.exists() {
            backup = self.root.join(".backups").join(format!("{stamp}-{k}"));
            k += 1;
        }
        let mut n = 0;
        let names: Vec<String> = self.packages.keys().cloned().collect();
        for name in names {
            let dir = self.root.join(&name);
            let (dirty, removed): (Vec<String>, Vec<String>) = {
                let p = &self.packages[&name];
                (p.files.iter().filter(|(_, f)| f.dirty()).map(|(k, _)| k.clone()).collect(), p.removed.iter().cloned().collect())
            };
            for rel in removed.iter().chain(&dirty) {
                let disk = dir.join(rel);
                if disk.exists() {
                    let b = backup.join(&name).join(rel);
                    std::fs::create_dir_all(b.parent().unwrap()).with_context(|| format!("cannot create {}", b.display()))?;
                    std::fs::copy(&disk, &b).with_context(|| format!("cannot back up {}", disk.display()))?;
                }
            }
            for rel in &removed {
                let disk = dir.join(rel);
                if disk.exists() {
                    std::fs::remove_file(&disk).with_context(|| format!("cannot delete {}", disk.display()))?;
                }
                n += 1;
            }
            for rel in &dirty {
                let disk = dir.join(rel);
                std::fs::create_dir_all(disk.parent().unwrap()).with_context(|| format!("cannot create {}", disk.display()))?;
                let bytes = self.bytes(&name, rel).with_context(|| format!("{name}/{rel} has no data"))?;
                let tmp = disk.with_extension("mhtmp");
                std::fs::write(&tmp, bytes.as_slice()).with_context(|| format!("cannot write {}", tmp.display()))?;
                std::fs::rename(&tmp, &disk).with_context(|| format!("cannot write {}", disk.display()))?;
                n += 1;
            }
            let p = self.packages.get_mut(&name).unwrap();
            p.removed.clear();
            for rel in dirty {
                if let Some(f) = p.files.get_mut(&rel) {
                    if matches!(f.body, Body::Bytes(_)) {
                        f.body = Body::OnDisk;
                    }
                    f.disk = Some(f.body.clone());
                }
            }
            // a package left with no files: its folder goes (if empty)
            if p.files.is_empty() {
                remove_empty_dirs(&dir);
                self.packages.remove(&name);
            } else {
                remove_empty_dirs(&dir);
            }
        }
        // undo steps hold bodies; binary ones written to disk stay valid as bytes
        if n > 0 {
            crate::bundle::touched();
        }
        Ok(n)
    }

    /// Paths of a package whose name contains the text, for pickers.
    pub fn files_of(&self, pkg: &str) -> Vec<String> {
        self.packages.get(pkg).map(|p| p.files.keys().cloned().collect()).unwrap_or_default()
    }

    /// Rename a package (a nation's tag changed). The files keep their names
    /// (see [`super::refactor::rename_tag`] for the text).
    pub fn rename_package(&mut self, from: &str, to: &str) -> Result<()> {
        if !valid_package(to) {
            bail!("{to:?} is not a tag");
        }
        if self.packages.get(to).is_some_and(|p| !p.is_empty()) {
            bail!("{to} already has content");
        }
        let files: Vec<(String, Option<(Body, bool)>)> = self
            .packages
            .get(from)
            .map(|p| p.files.iter().map(|(k, f)| (k.clone(), Some((self.materialize(from, k, f), f.bom)))).collect())
            .unwrap_or_default();
        for (path, v) in files {
            self.record(to, &path, v.clone());
            self.put(to, &path, v);
            self.record(from, &path, None);
            self.put(from, &path, None);
        }
        Ok(())
    }

    /// A body that does not depend on the package's folder on disk.
    fn materialize(&self, pkg: &str, path: &str, f: &SrcFile) -> Body {
        match &f.body {
            Body::OnDisk => Body::Bytes(self.bytes(pkg, path).unwrap_or_default()),
            b => b.clone(),
        }
    }

    /// A package's files as bytes (for export and copies).
    pub fn package_bytes(&self, pkg: &str) -> Vec<(String, Arc<Vec<u8>>)> {
        self.files_of(pkg).into_iter().filter_map(|p| self.bytes(pkg, &p).map(|b| (p, b))).collect()
    }
}

fn remove_empty_dirs(dir: &Path) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            if e.path().is_dir() {
                remove_empty_dirs(&e.path());
            }
        }
    }
    let _ = std::fs::remove_dir(dir);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mh-content-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn one_byte_order_mark() {
        let d = tmp("bom");
        // a file once written with two marks reads with one
        std::fs::create_dir_all(d.join("content/TYR/localisation/english")).unwrap();
        std::fs::write(d.join("content/TYR/localisation/english/a_l_english.yml"), "\u{feff}\u{feff}l_english:\n a:0 \"A\"\n").unwrap();
        let mut st = Store::load(&d).unwrap();
        assert_eq!(st.bytes("TYR", "localisation/english/a_l_english.yml").unwrap()[..6], *b"\xef\xbb\xbfl_e");
        // text given with its mark is saved with one
        st.begin("x");
        st.set_text("TYR", "localisation/english/b_l_english.yml", "\u{feff}l_english:\n".into());
        st.commit();
        st.save().unwrap();
        let b = std::fs::read(d.join("content/TYR/localisation/english/b_l_english.yml")).unwrap();
        assert!(b.starts_with(b"\xef\xbb\xbfl_english:"), "{b:?}");
    }

    #[test]
    fn edits_undo_and_save() {
        let map = tmp("store");
        std::fs::create_dir_all(map.join("content/TYR/events")).unwrap();
        std::fs::write(map.join("content/TYR/events/a.txt"), "add_namespace = tyr\ncountry_event = { id = tyr.1 }\n").unwrap();
        std::fs::write(map.join("content/TYR/flag.tga"), [1u8, 2, 3]).unwrap();
        let mut st = Store::load(&map).unwrap();
        assert!(!st.dirty());
        assert_eq!(st.defs_of(Some("TYR"), Kind::Event).len(), 1);
        assert!(matches!(st.file("TYR", "flag.tga").unwrap().body, Body::OnDisk));
        assert_eq!(st.bytes("TYR", "flag.tga").unwrap().as_slice(), &[1, 2, 3]);

        st.begin("add event");
        st.edit_text("TYR", "events/a.txt", |t| format!("{t}country_event = {{ id = tyr.2 }}\n"));
        st.set_text("TYR", "localisation/english/tyr_l_english.yml", loc::render("english", &[("tyr.2.t".into(), "Two".into())]));
        st.commit();
        assert!(st.dirty());
        assert_eq!(st.defs_of(Some("TYR"), Kind::Event).len(), 2);
        assert_eq!(st.can_undo(), Some("add event"));
        st.undo();
        assert!(!st.dirty());
        assert_eq!(st.defs_of(Some("TYR"), Kind::Event).len(), 1);
        assert!(st.file("TYR", "localisation/english/tyr_l_english.yml").is_none());
        st.redo();
        assert_eq!(st.defs_of(Some("TYR"), Kind::Event).len(), 2);

        // typing merges
        for c in ["a", "ab", "abc"] {
            st.begin_merge("type", Some("field".into()));
            st.set_text("TYR", "notes.txt", c.into());
            st.commit();
        }
        assert_eq!(st.can_undo(), Some("type"));
        st.undo();
        assert!(st.file("TYR", "notes.txt").is_none());
        assert_eq!(st.can_undo(), Some("add event"));

        st.begin("remove");
        st.remove("TYR", "flag.tga");
        st.commit();
        let n = st.save().unwrap();
        assert_eq!(n, 3);
        assert!(!st.dirty());
        assert!(!map.join("content/TYR/flag.tga").exists());
        let yml = std::fs::read(map.join("content/TYR/localisation/english/tyr_l_english.yml")).unwrap();
        assert!(yml.starts_with(&[0xef, 0xbb, 0xbf]));
        let backups = list_files(&map.join("content/.backups")).unwrap();
        assert_eq!(backups.len(), 2, "{backups:?}");
        // a fresh load sees the same
        let st2 = Store::load(&map).unwrap();
        assert_eq!(st2.defs_of(Some("TYR"), Kind::Event).len(), 2);
        assert!(st2.package(".backups").is_none());

        // rename a package
        st.begin("rename");
        st.rename_package("TYR", "TRL").unwrap();
        st.commit();
        assert!(st.package("TYR").is_none_or(|p| p.is_empty()));
        assert_eq!(st.defs_of(Some("TRL"), Kind::Event).len(), 2);
        st.save().unwrap();
        assert!(map.join("content/TRL/events/a.txt").exists());
        assert!(!map.join("content/TYR").exists());
        let _ = std::fs::remove_dir_all(&map);
    }
}
