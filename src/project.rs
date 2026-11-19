//! The project's settings: the layer stack and view, the editor's and the
//! content screen's settings, as `project.json` inside the project file
//! ([`crate::bundle`]). Paths of files the project refers to (layers, the
//! elevation and land cover) are stored relative to the project file.
//!
//! Before, a project was this JSON alone (`*.project.json`) with the map in
//! a folder of its own; [`ProjectFile::load_legacy`] reads those to convert
//! them.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

use crate::geo::Crs;
use crate::layers::LayerConfig;
use crate::util;
use crate::view::Camera;

/// Old project files: the settings alone, the map in a folder.
pub const LEGACY_EXT: &str = "project.json";
const VERSION: u32 = 2;

#[derive(Clone, Serialize, Deserialize)]
pub struct ProjectFile {
    pub version: u32,
    /// CRS of the world (display) coordinates.
    pub crs: Crs,
    /// Bottom-most layer first (draw order).
    pub layers: Vec<LayerConfig>,
    #[serde(default)]
    pub view: Option<Camera>,
    /// Map editor settings (and whether it was open).
    #[serde(default)]
    pub editor: Option<crate::editor::Settings>,
    /// Nation content screen settings (and whether it was open).
    #[serde(default)]
    pub content: Option<crate::content::ui::Settings>,
}

impl ProjectFile {
    pub fn new(crs: Crs) -> ProjectFile {
        ProjectFile { version: VERSION, crs, layers: Vec::new(), view: None, editor: None, content: None }
    }

    /// The paths of files the project refers to.
    fn paths_mut(&mut self) -> Vec<&mut PathBuf> {
        let mut v: Vec<&mut PathBuf> = self.layers.iter_mut().map(|l| &mut l.path).collect();
        if let Some(e) = &mut self.editor {
            v.extend(e.dem.as_mut());
            v.extend(e.landcover.as_mut());
        }
        v
    }

    /// Read settings whose relative paths are relative to `base`.
    pub fn from_json(text: &str, base: &Path) -> Result<ProjectFile> {
        let mut p: ProjectFile = serde_json::from_str(text)?;
        for path in p.paths_mut() {
            if path.is_relative() {
                *path = util::normalize(&base.join(&*path));
            }
        }
        Ok(p)
    }

    /// As JSON with paths relative to `base`. The map's folder is left out
    /// (the map is in the project file).
    pub fn to_json(&self, base: &Path) -> Result<String> {
        let mut out = self.clone();
        out.version = VERSION;
        if let Some(e) = &mut out.editor {
            e.map_dir = None;
        }
        for path in out.paths_mut() {
            *path = util::relative_to(path, base);
        }
        Ok(serde_json::to_string_pretty(&out)?)
    }

    /// An old project file (`*.project.json`; its map in `editor.map_dir`).
    pub fn load_legacy(path: &Path) -> Result<ProjectFile> {
        let text = std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
        ProjectFile::from_json(&text, &project_dir(path)).with_context(|| format!("invalid project file {}", path.display()))
    }
}

/// The folder of a project file (absolute).
pub fn project_dir(path: &Path) -> PathBuf {
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    abs.parent().map(Path::to_path_buf).unwrap_or_default()
}

/// An old project file (`*.project.json`).
pub fn is_legacy(path: &Path) -> bool {
    path.file_name().is_some_and(|n| n.to_string_lossy().ends_with(&format!(".{LEGACY_EXT}")))
}

pub const RASTER_EXTS: &[&str] = &["bmp", "tif", "tiff", "png", "jpg", "jpeg"];

// ---------------------------------------------------------------------------
// Recent projects (the welcome screen's list)

/// A project opened before.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Recent {
    pub path: PathBuf,
    /// The mod's name, else the file's.
    pub name: String,
    /// Seconds since 1970.
    pub opened: u64,
    /// "4608 × 2816 px · EPSG:3035 LAEA Europe".
    #[serde(default)]
    pub summary: String,
}

/// mod-helper's own files: `$XDG_DATA_HOME/mod-helper` (or
/// `~/.local/share/mod-helper`).
pub fn data_dir() -> PathBuf {
    if cfg!(test) {
        // tests never touch the user's own
        return std::env::temp_dir().join(format!("mh-test-data-{}", std::process::id()));
    }
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| std::env::home_dir().unwrap_or_default().join(".local/share"))
        .join("mod-helper")
}

fn recent_file() -> PathBuf {
    data_dir().join("recent_projects.json")
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// The projects opened before, last opened first.
pub fn recent_projects() -> Vec<Recent> {
    let mut v: Vec<Recent> = std::fs::read_to_string(recent_file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    v.sort_by(|a, b| b.opened.cmp(&a.opened));
    for r in v.iter_mut().filter(|r| r.summary.is_empty() && r.path.is_file()) {
        *r = describe(&r.path, r.opened);
    }
    v
}

fn write_recent(v: &[Recent]) -> Result<()> {
    let f = recent_file();
    if let Some(d) = f.parent() {
        std::fs::create_dir_all(d).with_context(|| format!("cannot create {}", d.display()))?;
    }
    let tmp = f.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(v)?).with_context(|| format!("cannot write {}", tmp.display()))?;
    std::fs::rename(&tmp, &f).with_context(|| format!("cannot write {}", f.display()))?;
    Ok(())
}

/// Width and height of a BMP from its first 26 bytes.
pub fn bmp_size_of(b: &[u8]) -> Option<(u32, u32)> {
    (b.len() >= 26 && &b[..2] == b"BM").then(|| (i32::from_le_bytes([b[18], b[19], b[20], b[21]]).unsigned_abs(), i32::from_le_bytes([b[22], b[23], b[24], b[25]]).unsigned_abs()))
}

/// Width and height of a BMP from its header.
pub fn bmp_size(path: &Path) -> Option<(u32, u32)> {
    use std::io::Read as _;
    let mut b = [0u8; 26];
    std::fs::File::open(path).ok()?.read_exact(&mut b).ok()?;
    bmp_size_of(&b)
}

/// Width and height of a raster (from its header where the format allows).
pub fn raster_size(path: &Path) -> Option<(u32, u32)> {
    crate::raster::probe_size(path)
}

/// The mod's name in a provinces.json (its "mod" header), read from `r`.
pub fn mod_name(r: impl std::io::Read) -> Option<String> {
    #[derive(Deserialize)]
    struct Head {
        #[serde(rename = "mod", default)]
        meta: Option<serde_json::Value>,
    }
    let h: Head = serde_json::from_reader(std::io::BufReader::new(r)).ok()?;
    h.meta?.get("name")?.as_str().map(str::to_string).filter(|n| !n.trim().is_empty())
}

/// The list entry of a project file: the name of its map's mod, the map's
/// size and projection.
fn describe(path: &Path, opened: u64) -> Recent {
    let file = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let stem = file.strip_suffix(&format!(".{LEGACY_EXT}")).or(file.strip_suffix(&format!(".{}", crate::bundle::EXT))).unwrap_or(&file).to_string();
    let mut r = Recent { path: path.to_path_buf(), name: stem, opened, summary: String::new() };
    let (size, crs, note) = if is_legacy(path) {
        let Ok(pf) = ProjectFile::load_legacy(path) else { return r };
        let size = match pf.editor.and_then(|e| e.map_dir) {
            Some(m) => {
                if let Some(n) = std::fs::File::open(m.join("provinces.json")).ok().and_then(mod_name) {
                    r.name = n;
                }
                bmp_size(&m.join("provinces.bmp")).map(|(w, h)| format!("{w} × {h} px"))
            }
            None => Some("no map".to_string()),
        };
        (size, pf.crs, Some("old format: opening converts it"))
    } else {
        let Ok(s) = crate::bundle::summary(path) else { return r };
        if let Some(n) = s.name {
            r.name = n;
        }
        (s.size.map(|(w, h)| format!("{w} × {h} px")), s.crs, None)
    };
    let mut parts: Vec<String> = size.into_iter().collect();
    parts.push(if crs == Crs::UNKNOWN { "plain raster".to_string() } else { crs.name() });
    parts.extend(note.map(str::to_string));
    r.summary = parts.join(" · ");
    r
}

/// Put a project at the top of the list.
pub fn remember_project(path: &Path) -> Result<()> {
    let path = util::normalize(&std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()));
    let mut v = recent_projects();
    v.retain(|r| r.path != path);
    v.insert(0, describe(&path, now()));
    v.truncate(30);
    write_recent(&v)
}

/// Take a project off the list (its files stay).
pub fn forget_project(path: &Path) -> Result<()> {
    let mut v = recent_projects();
    v.retain(|r| r.path != path);
    write_recent(&v)
}

/// "3 h ago" and the like.
pub fn ago(t: u64) -> String {
    if t == 0 {
        return String::new();
    }
    let m = now().saturating_sub(t) / 60;
    match m {
        0 => "just now".into(),
        m if m < 60 => format!("{m} min ago"),
        m if m < 48 * 60 => format!("{} h ago", m / 60),
        m => format!("{} days ago", m / (24 * 60)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bmp_header_size() {
        let dir = std::env::temp_dir().join(format!("mh-bmpsize-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("x.bmp");
        crate::modbuild::img::write_bmp24(&p, 512, 256, &vec![0; 512 * 256 * 3]).unwrap();
        assert_eq!(bmp_size(&p), Some((512, 256)));
        assert_eq!(bmp_size(&dir.join("none.bmp")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
