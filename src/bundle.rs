//! Project files (`*.hoi4proj`): one file with all a project is, a ZIP
//! archive of its settings (`project.json`, [`ProjectFile`]) and its map
//! (`map/…`: provinces.bmp and provinces.json, the georeference, the painted
//! terrain and imported heightmap, adjacency rules, the nations' content).
//! What the project only uses stays where it is and the project refers to
//! it: rasters and OSM extracts shown as layers, the elevation and land
//! cover the map was made from, the game.
//!
//! The editor, the content screen and the build work on files, so the map is
//! unpacked into a working folder, one per project file in mod-helper's data
//! folder ([`work_root`]); the editor's backups stay there too, beside the
//! map. Saving packs what changed back into the project file (a new file,
//! then renamed over the old one). A working folder is used again while the
//! project file is the one it was unpacked from; else it is unpacked again.

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context as _, Result, bail, ensure};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::geo::Crs;
use crate::project::{self, ProjectFile};
use crate::util;
use crate::zip;

pub const EXT: &str = "hoi4proj";
const SETTINGS: &str = "project.json";
const MAP: &str = "map/";
/// In the map folder but not part of the project: backups (the editor's and
/// the content screen's) and version control.
const SKIP: [&str; 4] = ["backups", "content/.backups", "content/.snapshots", ".git"];

static WRITES: AtomicU64 = AtomicU64::new(0);

/// Something wrote to a map folder (a save): the open project packs it.
pub fn touched() {
    WRITES.fetch_add(1, Ordering::Relaxed);
}

/// Counts [`touched`].
pub fn writes() -> u64 {
    WRITES.load(Ordering::Relaxed)
}

fn skipped(rel: &str) -> bool {
    SKIP.iter().any(|s| rel == *s || rel.starts_with(&format!("{s}/"))) || rel.ends_with(".tmp") || rel.ends_with(".mhtmp") || rel.split('/').any(|c| c == ".git")
}

/// Size and modification time of a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Meta {
    len: u64,
    /// Nanoseconds since 1970.
    modified: u128,
}

fn meta(path: &Path) -> Option<Meta> {
    let m = std::fs::metadata(path).ok()?;
    let modified = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    Some(Meta { len: m.len(), modified })
}

/// The map folder's files (relative, `/`-separated) but those [`SKIP`]ped.
fn scan(dir: &Path) -> Result<BTreeMap<String, Meta>> {
    fn walk(dir: &Path, rel: &str, out: &mut BTreeMap<String, Meta>) -> Result<()> {
        let rd = match std::fs::read_dir(dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e).with_context(|| format!("cannot read {}", dir.display())),
        };
        for e in rd {
            let e = e?;
            let name = e.file_name().to_string_lossy().into_owned();
            let r = if rel.is_empty() { name } else { format!("{rel}/{name}") };
            if skipped(&r) {
                continue;
            }
            let p = e.path();
            if p.is_dir() {
                walk(&p, &r, out)?;
            } else if let Some(m) = meta(&p).filter(|_| p.is_file()) {
                out.insert(r, m);
            }
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    walk(dir, "", &mut out)?;
    Ok(out)
}

/// What a working folder holds, as of the last unpack or pack.
#[derive(Default, Serialize, Deserialize)]
struct Stamp {
    /// The project file it belongs to, and that file's size and time then.
    file: PathBuf,
    file_meta: Option<Meta>,
    /// The map's files.
    files: BTreeMap<String, Meta>,
    /// CRC of the settings packed.
    settings_crc: u32,
}

fn absolute(path: &Path) -> PathBuf {
    util::normalize(&std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()))
}

/// The working folder of a project file: `<data dir>/projects/<name>-<hash
/// of its path>`.
pub fn work_root(path: &Path) -> PathBuf {
    let path = absolute(path);
    let h = util::fnv1a(path.as_os_str().as_encoded_bytes());
    let stem = path.file_name().map(|n| n.to_string_lossy().trim_end_matches(&format!(".{EXT}")).to_string()).unwrap_or_default();
    project::data_dir().join("projects").join(format!("{}-{h:016x}", crate::newproject::slug(&stem)))
}

/// An open project: its file and its working folder.
pub struct Project {
    pub path: PathBuf,
    root: PathBuf,
    stamp: Stamp,
}

impl Project {
    /// The unpacked map (what the editor opens).
    pub fn map_dir(&self) -> PathBuf {
        self.root.join("map")
    }

    fn stamp_file(root: &Path) -> PathBuf {
        root.join("stamp.json")
    }

    fn write_stamp(&self) -> Result<()> {
        let f = Project::stamp_file(&self.root);
        let tmp = f.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(&self.stamp)?).with_context(|| format!("cannot write {}", tmp.display()))?;
        std::fs::rename(&tmp, &f).with_context(|| format!("cannot write {}", f.display()))?;
        Ok(())
    }

    /// Open a project file: its settings (with the editor's map folder set to
    /// the working folder), its map unpacked.
    pub fn open(path: &Path) -> Result<(Project, ProjectFile)> {
        let path = absolute(path);
        if project::is_legacy(&path) {
            bail!("{} is a project of the old format (a map folder and its settings): it has to be converted first", path.display());
        }
        let mut zr = zip::Reader::open(&path)?;
        let Some(e) = zr.find(SETTINGS).cloned() else { bail!("{} is not a mod-helper project (it has no {SETTINGS})", path.display()) };
        let text = String::from_utf8(zr.read(&e)?).with_context(|| format!("{SETTINGS} is not text"))?;
        let mut pf = ProjectFile::from_json(&text, &project::project_dir(&path)).with_context(|| format!("invalid {SETTINGS} in {}", path.display()))?;
        let root = work_root(&path);
        let stamp: Option<Stamp> = std::fs::read(Project::stamp_file(&root)).ok().and_then(|b| serde_json::from_slice(&b).ok());
        let current = stamp.as_ref().is_some_and(|s| s.file == path && s.file_meta == meta(&path) && root.join("map").is_dir());
        let mut p = Project { path: path.clone(), root, stamp: stamp.unwrap_or_default() };
        if current {
            log::info!("{}: the working folder {} is up to date", path.display(), p.root.display());
        } else {
            p.unpack(&mut zr)?;
        }
        p.stamp.settings_crc = zip::crc(text.as_bytes());
        pf.editor.get_or_insert_with(Default::default).map_dir = Some(p.map_dir());
        Ok((p, pf))
    }

    /// Unpack the archive's map into the working folder (the files there
    /// before go, but the [`SKIP`]ped ones).
    fn unpack(&mut self, zr: &mut zip::Reader) -> Result<()> {
        let map = self.map_dir();
        std::fs::create_dir_all(&map).with_context(|| format!("cannot create {}", map.display()))?;
        for (rel, _) in scan(&map)? {
            let f = map.join(&rel);
            std::fs::remove_file(&f).with_context(|| format!("cannot remove {}", f.display()))?;
        }
        // folders, and backups someone packed in, are left out
        let entries: Vec<zip::Entry> = zr.entries().iter().filter(|e| e.name.starts_with(MAP) && !e.name.ends_with('/') && !skipped(&e.name[MAP.len()..])).cloned().collect();
        for e in &entries {
            let rel = &e.name[MAP.len()..];
            ensure!(zip::safe_name(rel), "{}: unsafe name in the project file: {}", self.path.display(), e.name);
            let raw = zr.raw(e)?;
            let data = zip::unpack(e, &raw)?;
            let f = map.join(rel);
            if let Some(d) = f.parent() {
                std::fs::create_dir_all(d).with_context(|| format!("cannot create {}", d.display()))?;
            }
            std::fs::write(&f, data).with_context(|| format!("cannot write {}", f.display()))?;
        }
        self.stamp = Stamp { file: self.path.clone(), file_meta: meta(&self.path), files: scan(&map)?, settings_crc: 0 };
        self.write_stamp()?;
        log::info!("{}: unpacked {} files into {}", self.path.display(), entries.len(), map.display());
        Ok(())
    }

    /// The map's files changed since the last pack (or unpack).
    pub fn map_changed(&self) -> bool {
        scan(&self.map_dir()).map_or(true, |now| now != self.stamp.files)
    }

    /// Pack what changed into the project file, with `pf` as its settings.
    /// With `settings` false, only changes of the map write the file (the
    /// settings then go with them). Returns whether the file was written.
    pub fn save(&mut self, pf: &ProjectFile, settings: bool) -> Result<bool> {
        let json = pf.to_json(&project::project_dir(&self.path))?;
        let crc = zip::crc(json.as_bytes());
        let now = scan(&self.map_dir())?;
        let gone = !self.path.exists();
        if now == self.stamp.files && (!settings || crc == self.stamp.settings_crc) && !gone {
            return Ok(false);
        }
        let src = (!gone).then(|| self.path.clone());
        self.write_to(&self.path.clone(), src.as_deref(), &json, &now)?;
        self.stamp.file = self.path.clone();
        self.stamp.file_meta = meta(&self.path);
        self.stamp.files = now;
        self.stamp.settings_crc = crc;
        self.write_stamp()?;
        Ok(true)
    }

    /// Write the project file `dest`: the settings, the map's files `now`;
    /// those unchanged since the stamp are copied packed from `src`.
    fn write_to(&self, dest: &Path, src: Option<&Path>, json: &str, now: &BTreeMap<String, Meta>) -> Result<()> {
        let t0 = std::time::Instant::now();
        let mut old = src.and_then(|s| zip::Reader::open(s).ok());
        let map = self.map_dir();
        let unchanged = |rel: &str, m: &Meta| -> bool {
            self.stamp.files.get(rel) == Some(m) && old.as_ref().is_some_and(|z| z.find(&format!("{MAP}{rel}")).is_some())
        };
        let fresh: Vec<&String> = now.iter().filter(|(rel, m)| !unchanged(rel, m)).map(|(rel, _)| rel).collect();
        let packed: BTreeMap<&String, zip::Packed> = fresh
            .par_iter()
            .map(|rel| {
                let f = map.join(rel.as_str());
                let data = std::fs::read(&f).with_context(|| format!("cannot read {}", f.display()))?;
                Ok((*rel, zip::pack(&data)))
            })
            .collect::<Result<_>>()?;
        if let Some(d) = dest.parent() {
            std::fs::create_dir_all(d).with_context(|| format!("cannot create {}", d.display()))?;
        }
        let tmp = PathBuf::from(format!("{}.tmp", dest.display()));
        let mut write = || -> Result<()> {
            let f = std::fs::File::create(&tmp).with_context(|| format!("cannot write {}", tmp.display()))?;
            let mut w = zip::Writer::new(std::io::BufWriter::new(f));
            w.add(SETTINGS, json.as_bytes())?;
            for rel in now.keys() {
                let name = format!("{MAP}{rel}");
                match packed.get(rel) {
                    Some(p) => w.add_packed(&name, p)?,
                    None => {
                        let z = old.as_mut().expect("unchanged files come from the old archive");
                        let e = z.find(&name).cloned().expect("checked above");
                        let raw = z.raw(&e)?;
                        w.add_raw(&e, &raw)?;
                    }
                }
            }
            let f = w.finish()?.into_inner().map_err(|e| e.into_error())?;
            f.sync_all()?;
            Ok(())
        };
        if let Err(e) = write() {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.context(format!("cannot write {}", dest.display())));
        }
        std::fs::rename(&tmp, dest).with_context(|| format!("cannot write {}", dest.display()))?;
        log::info!("packed {} ({} files, {} changed) in {:.2} s", dest.display(), now.len(), fresh.len(), t0.elapsed().as_secs_f32());
        Ok(())
    }

    /// Save the project as `dest` (all of it, settings `pf`); the working
    /// folder (and its backups) moves along. The project is `dest` after.
    pub fn save_as(&mut self, dest: &Path, pf: &ProjectFile) -> Result<()> {
        let dest = absolute(dest);
        if dest == self.path {
            self.save(pf, true)?;
            return Ok(());
        }
        ensure!(!project::is_legacy(&dest), "project files end in .{EXT}");
        let json = pf.to_json(&project::project_dir(&dest))?;
        let now = scan(&self.map_dir())?;
        let src = self.path.exists().then(|| self.path.clone());
        self.write_to(&dest, src.as_deref(), &json, &now)?;
        let root = work_root(&dest);
        set_aside(&root)?;
        if let Some(d) = root.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::rename(&self.root, &root).with_context(|| format!("cannot move {} to {}", self.root.display(), root.display()))?;
        self.root = root;
        self.path = dest.clone();
        self.stamp = Stamp { file: dest.clone(), file_meta: meta(&dest), files: now, settings_crc: zip::crc(json.as_bytes()) };
        self.write_stamp()?;
        Ok(())
    }

    /// A new project file `path` with the map `make` writes into the folder
    /// it is given (which does not exist yet) and the settings `pf`.
    pub fn create<R>(path: &Path, pf: &ProjectFile, make: impl FnOnce(&Path) -> Result<R>) -> Result<(Project, R)> {
        let path = absolute(path);
        ensure!(path.file_name().is_some_and(|n| n.to_string_lossy().ends_with(&format!(".{EXT}"))), "project files end in .{EXT}");
        if path.exists() {
            bail!("{} exists already", path.display());
        }
        let root = work_root(&path);
        set_aside(&root)?;
        let mut p = Project { path: path.clone(), root, stamp: Stamp::default() };
        let r = match make(&p.map_dir()) {
            Ok(r) => r,
            Err(e) => {
                // nothing half made stays behind
                let _ = std::fs::remove_dir_all(&p.root);
                return Err(e);
            }
        };
        let mut pf = pf.clone();
        if let Some(e) = &mut pf.editor {
            e.map_dir = None;
        }
        if let Err(e) = p.save(&pf, true) {
            let _ = std::fs::remove_dir_all(&p.root);
            return Err(e);
        }
        Ok((p, r))
    }
}

/// Move an old working folder out of the way (its backups may matter): to
/// `<root>-old-<time>`.
fn set_aside(root: &Path) -> Result<()> {
    if root.exists() {
        let old = PathBuf::from(format!("{}-old-{}", root.display(), crate::editor::model::timestamp()));
        std::fs::rename(root, &old).with_context(|| format!("cannot move {} aside", root.display()))?;
        log::info!("moved an old working folder aside to {}", old.display());
    }
    Ok(())
}

/// What the welcome screen shows of a project file.
pub struct Summary {
    /// The mod's name.
    pub name: Option<String>,
    pub size: Option<(u32, u32)>,
    pub crs: Crs,
}

/// Read the summary of a project file (without unpacking it).
pub fn summary(path: &Path) -> Result<Summary> {
    let mut zr = zip::Reader::open(path)?;
    let e = zr.find(SETTINGS).cloned().with_context(|| format!("{} has no {SETTINGS}", path.display()))?;
    #[derive(Deserialize)]
    struct Head {
        crs: Crs,
    }
    let crs = serde_json::from_slice::<Head>(&zr.read(&e)?)?.crs;
    let name = match zr.find("map/provinces.json").cloned() {
        Some(e) => project::mod_name(zr.open_entry(&e)?),
        None => None,
    };
    let size = match zr.find("map/provinces.bmp").cloned() {
        Some(e) => {
            let mut b = [0u8; 26];
            zr.open_entry(&e)?.read_exact(&mut b).ok().and_then(|_| project::bmp_size_of(&b))
        }
        None => None,
    };
    Ok(Summary { name, size, crs })
}

/// The files of an old map folder that are the project's (the rest of such
/// folders were inputs and outputs of the scripts that made the map).
const LEGACY_FILES: [&str; 9] = [
    "provinces.bmp",
    "provinces.json",
    "provinces.wld",
    "provinces.bmp.aux.xml",
    "provinces.prj",
    "provinces.bpw",
    crate::modbuild::bitmaps::PAINT_FILE,
    crate::modbuild::bitmaps::HEIGHTS_FILE,
    "adjacency_rules.txt",
];

/// Convert an old project (`*.project.json` and the map folder it names)
/// into the project file `dest`. The old files are only read. The elevation
/// and land cover of the map folder become references. Returns what was
/// done.
pub fn convert(legacy: &Path, dest: &Path) -> Result<(Project, Vec<String>)> {
    let mut pf = ProjectFile::load_legacy(legacy)?;
    let ed = pf.editor.get_or_insert_with(Default::default);
    let Some(map) = ed.map_dir.take() else { bail!("{} has no map folder", legacy.display()) };
    ensure!(map.join("provinces.json").is_file() && map.join("provinces.bmp").is_file(), "no map in {} (provinces.bmp and provinces.json)", map.display());
    let mut report = vec![format!("the map of {}", map.display())];
    for (slot, name, what) in [(&mut ed.dem, crate::modbuild::bitmaps::DEM_FILE, "elevation"), (&mut ed.landcover, crate::modbuild::bitmaps::COVER_FILE, "land cover")] {
        if slot.is_none() && map.join(name).is_file() {
            *slot = Some(map.join(name));
            report.push(format!("refers to the {what} {}", map.join(name).display()));
        }
    }
    let (p, n) = Project::create(dest, &pf, |dir| {
        std::fs::create_dir_all(dir)?;
        let mut n = 0;
        for f in LEGACY_FILES {
            if map.join(f).is_file() {
                std::fs::copy(map.join(f), dir.join(f)).with_context(|| format!("cannot copy {f}"))?;
                n += 1;
            }
        }
        let content = map.join("content");
        for (rel, _) in scan(&content)?.into_iter().filter(|(rel, _)| !skipped(&format!("content/{rel}"))) {
            let to = dir.join("content").join(&rel);
            std::fs::create_dir_all(to.parent().unwrap())?;
            std::fs::copy(content.join(&rel), &to).with_context(|| format!("cannot copy content/{rel}"))?;
            n += 1;
        }
        Ok(n)
    })?;
    report.push(format!("{n} files of the map are in {}", p.path.display()));
    report.push(format!("left where they are: the backups and the other files of {}", map.display()));
    Ok((p, report))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapproj::Projection;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mh-bundle-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn blank(dir: &Path) -> Result<()> {
        crate::newproject::create_blank(dir, 512, 256, crate::newproject::Fill::Land, &Projection::Plain { pixel_m: 1000.0 }, &crate::newproject::mod_meta("Bundle test"))
    }

    /// Settings and map go into one file; opened again it is all there;
    /// a save writes only when something changed.
    #[test]
    fn create_open_save() {
        let d = tmp("cos");
        let path = d.join("docs/My map.hoi4proj");
        let mut pf = ProjectFile::new(Crs::UNKNOWN);
        pf.editor = Some(crate::editor::Settings { dem: Some(d.join("rasters/dem.tif")), ..Default::default() });
        let (mut p, ()) = Project::create(&path, &pf, blank).unwrap();
        assert!(path.is_file() && p.map_dir().join("provinces.json").is_file());
        assert!(Project::create(&path, &pf, blank).is_err(), "exists");
        let names: Vec<String> = zip::Reader::open(&path).unwrap().entries().iter().map(|e| e.name.clone()).collect();
        assert!(names.contains(&"project.json".to_string()) && names.contains(&"map/provinces.bmp".to_string()), "{names:?}");
        assert!(!names.iter().any(|n| n.contains("backups")));
        // references are relative to the project file
        let mut zr = zip::Reader::open(&path).unwrap();
        let e = zr.find(SETTINGS).unwrap().clone();
        let text = String::from_utf8(zr.read(&e).unwrap()).unwrap();
        assert!(text.contains("../rasters/dem.tif") && !text.contains("map_dir\": \"/"), "{text}");
        let s = summary(&path).unwrap();
        assert_eq!((s.name.as_deref(), s.size, s.crs), (Some("Bundle test"), Some((512, 256)), Crs::UNKNOWN));

        // nothing changed: nothing written
        let (_, pf2) = Project::open(&path).unwrap();
        assert!(!p.save(&pf2, false).unwrap() && !p.save(&pf2, true).unwrap());
        assert_eq!(pf2.editor.as_ref().unwrap().dem, Some(d.join("rasters/dem.tif")));
        assert_eq!(pf2.editor.as_ref().unwrap().map_dir, Some(p.map_dir()));

        // a change of the map is packed; backups are not
        let map = p.map_dir();
        std::fs::create_dir_all(map.join("content/TYR")).unwrap();
        std::fs::write(map.join("content/TYR/a.txt"), "focus = {}").unwrap();
        std::fs::create_dir_all(map.join("backups/1")).unwrap();
        std::fs::write(map.join("backups/1/provinces.json"), "{}").unwrap();
        std::fs::create_dir_all(map.join("content/.backups/1")).unwrap();
        std::fs::write(map.join("content/.backups/1/x.txt"), "old").unwrap();
        assert!(p.map_changed());
        assert!(p.save(&pf2, false).unwrap());
        assert!(!p.map_changed());
        let names: Vec<String> = zip::Reader::open(&path).unwrap().entries().iter().map(|e| e.name.clone()).collect();
        assert!(names.contains(&"map/content/TYR/a.txt".to_string()));
        assert!(!names.iter().any(|n| n.contains("backups")), "{names:?}");
        // settings alone: only when asked
        let mut pf3 = pf2.clone();
        pf3.crs = Crs::LAEA_EUROPE;
        assert!(!p.save(&pf3, false).unwrap());
        assert!(p.save(&pf3, true).unwrap());
        assert_eq!(summary(&path).unwrap().crs, Crs::LAEA_EUROPE);

        // the file replaced by another (a copy from elsewhere): unpacked again,
        // the backups stay
        let other = d.join("other.hoi4proj");
        let (_, ()) = Project::create(&other, &pf, blank).unwrap();
        std::fs::copy(&other, &path).unwrap();
        let (p2, _) = Project::open(&path).unwrap();
        assert!(!p2.map_dir().join("content/TYR/a.txt").exists(), "unpacked again");
        assert!(p2.map_dir().join("backups/1/provinces.json").exists(), "backups stay");

        // save as: the new file has it all, the working folder moves along
        let mut p2 = p2;
        let moved = d.join("elsewhere/Copy.hoi4proj");
        p2.save_as(&moved, &pf).unwrap();
        assert_eq!(p2.path, moved);
        assert!(p2.map_dir().join("backups/1/provinces.json").exists());
        let (p3, _) = Project::open(&moved).unwrap();
        assert_eq!(p3.map_dir(), p2.map_dir());
        assert!(!p3.map_changed());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// An old project (settings + map folder) becomes one file; the old
    /// files stay as they were.
    #[test]
    fn convert_old_projects() {
        let d = tmp("legacy");
        let map = d.join("maps/my map");
        blank(&map).unwrap();
        std::fs::create_dir_all(map.join("content/TYR/.git")).unwrap();
        std::fs::write(map.join("content/TYR/a.txt"), "focus = {}").unwrap();
        std::fs::write(map.join("content/TYR/.git/HEAD"), "x").unwrap();
        std::fs::create_dir_all(map.join("content/.backups/1/TYR")).unwrap();
        std::fs::write(map.join("content/.backups/1/TYR/a.txt"), "old").unwrap();
        std::fs::create_dir_all(map.join("backups/1")).unwrap();
        std::fs::write(map.join("backups/1/provinces.json"), "{}").unwrap();
        std::fs::write(map.join(crate::modbuild::bitmaps::DEM_FILE), "not read here").unwrap();
        std::fs::write(map.join("sea.bmp"), "an input of the scripts").unwrap();
        let legacy = d.join("tool/mod-helper.project.json");
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        let mut pf = ProjectFile::new(Crs::LAEA_EUROPE);
        pf.layers.push(crate::layers::LayerConfig::new(d.join("maps/sea.bmp")));
        pf.editor = Some(crate::editor::Settings { active: true, map_dir: Some(map.clone()), ..Default::default() });
        std::fs::write(&legacy, pf.to_json(&legacy.parent().unwrap().to_path_buf()).unwrap().replace("\"map_dir\": null", &format!("\"map_dir\": {:?}", map.display().to_string()))).unwrap();
        let before = scan(&d).unwrap();

        let dest = d.join("projects/My map.hoi4proj");
        let (p, report) = convert(&legacy, &dest).unwrap();
        assert!(report.iter().any(|l| l.contains("elevation")), "{report:?}");
        let names: Vec<String> = zip::Reader::open(&dest).unwrap().entries().iter().map(|e| e.name.clone()).collect();
        assert!(names.contains(&"map/provinces.bmp".to_string()) && names.contains(&"map/content/TYR/a.txt".to_string()), "{names:?}");
        assert!(!names.iter().any(|n| n.contains("backups") || n.contains(".git") || n.contains("sea.bmp") || n.contains("dem_section")), "{names:?}");
        let (_, pf2) = Project::open(&dest).unwrap();
        assert_eq!(pf2.crs, Crs::LAEA_EUROPE);
        assert_eq!(pf2.layers[0].path, d.join("maps/sea.bmp"));
        let ed = pf2.editor.unwrap();
        assert_eq!(ed.dem, Some(map.join(crate::modbuild::bitmaps::DEM_FILE)));
        assert_eq!((ed.landcover, ed.map_dir), (None, Some(p.map_dir())));
        assert!(!p.map_changed());
        // the old files: as they were
        let mut after = scan(&d).unwrap();
        after.retain(|k, _| !k.starts_with("projects/"));
        assert_eq!(before, after);
        assert!(convert(&legacy, &dest).is_err(), "exists");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn skipped_files() {
        for s in ["backups", "backups/x/provinces.bmp", "content/.backups/1/a", "content/.snapshots/x", ".git/HEAD", "content/TYR/.git/x", "provinces.bmp.tmp"] {
            assert!(skipped(s), "{s}");
        }
        for s in ["provinces.bmp", "content/TYR/events/a.txt", "content/backups.txt"] {
            assert!(!skipped(s), "{s}");
        }
    }
}
