//! Project files (`*.project.json`): the layer stack and view, so a session
//! can be resumed. Layer paths are stored relative to the project file.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

use crate::geo::Crs;
use crate::layers::{LayerConfig, LayerKind};
use crate::util;
use crate::view::Camera;

pub const PROJECT_EXT: &str = "project.json";
const VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
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
}

impl ProjectFile {
    pub fn load(path: &Path) -> Result<ProjectFile> {
        let text = std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
        let mut p: ProjectFile = serde_json::from_str(&text).with_context(|| format!("invalid project file {}", path.display()))?;
        let base = project_dir(path);
        for l in &mut p.layers {
            if l.path.is_relative() {
                l.path = util::normalize(&base.join(&l.path));
            }
        }
        Ok(p)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let base = project_dir(path);
        let layers = self
            .layers
            .iter()
            .map(|l| {
                let mut l = l.clone();
                l.path = util::relative_to(&l.path, &base);
                l
            })
            .collect();
        let out = ProjectFile { version: VERSION, crs: self.crs, layers, view: self.view, editor: self.editor.clone() };
        let json = serde_json::to_string_pretty(&out)?;
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, json).with_context(|| format!("cannot write {}", tmp.display()))?;
        std::fs::rename(&tmp, path).with_context(|| format!("cannot write {}", path.display()))?;
        Ok(())
    }
}

fn project_dir(path: &Path) -> PathBuf {
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    abs.parent().map(Path::to_path_buf).unwrap_or_default()
}

/// The mod's `maps` directory (sibling of the `mod-helper` crate).
pub fn find_maps_dir() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("../maps"));
        candidates.push(cwd.join("maps"));
    }
    candidates.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../maps"));
    candidates.into_iter().find(|p| p.is_dir()).map(|p| util::normalize(&std::path::absolute(&p).unwrap_or(p)))
}

/// Default project location: next to the crate when running from a checkout.
pub fn default_project_path() -> PathBuf {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = if crate_dir.is_dir() { crate_dir.to_path_buf() } else { std::env::current_dir().unwrap_or_default() };
    dir.join(format!("mod-helper.{PROJECT_EXT}"))
}

pub const RASTER_EXTS: &[&str] = &["bmp", "tif", "tiff", "png", "jpg", "jpeg"];

fn is_raster(p: &Path) -> bool {
    p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).is_some_and(|e| RASTER_EXTS.contains(&e.as_str()))
}

/// Stacking priority guessed from the file name (lower = further down).
fn stack_priority(name: &str) -> u8 {
    let n = name.to_ascii_lowercase();
    if n.contains("height") || n.contains("dem") {
        0
    } else if n.contains("sea") || n.contains("water") || n.contains("river") {
        2
    } else if n.contains("province") || n.contains("state") {
        3
    } else {
        1
    }
}

/// Initial project: all rasters in `maps/processed maps` plus all OSM
/// extracts in `maps/osm regions`.
pub fn default_project(maps_dir: Option<&Path>) -> ProjectFile {
    let mut layers = Vec::new();
    if let Some(maps) = maps_dir {
        let mut rasters: Vec<PathBuf> = std::fs::read_dir(maps.join("processed maps"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file() && is_raster(p))
            .collect();
        rasters.sort_by_key(|p| (stack_priority(&p.to_string_lossy()), p.clone()));
        let mut shown_base = false;
        for p in rasters {
            let mut cfg = LayerConfig::new(p);
            // Show one base map plus the province map; everything else starts hidden.
            match stack_priority(&cfg.name) {
                0 if !shown_base => shown_base = true,
                3 if cfg.name.to_ascii_lowercase().contains("province") => cfg.opacity = 0.45,
                _ => cfg.visible = false,
            }
            layers.push(cfg);
        }
        let mut pbfs: Vec<PathBuf> = std::fs::read_dir(maps.join("osm regions"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| LayerKind::for_path(p) == LayerKind::OsmBoundaries)
            .collect();
        pbfs.sort();
        layers.extend(pbfs.into_iter().map(LayerConfig::new));
    }
    ProjectFile { version: VERSION, crs: Crs::LAEA_EUROPE, layers, view: None, editor: None }
}
