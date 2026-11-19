//! Map layers: persistent configuration plus runtime state.

use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::geo::{self, Crs, FileGeoInfo, GeoTransform};
use crate::raster::{Raster, RasterStyle};
use crate::tiles;
use crate::util::Progress;
use crate::vector::{VectorData, VectorStyle};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LayerKind {
    Raster,
    OsmBoundaries,
}

impl LayerKind {
    pub fn for_path(p: &std::path::Path) -> LayerKind {
        if p.to_string_lossy().to_ascii_lowercase().ends_with(".pbf") { LayerKind::OsmBoundaries } else { LayerKind::Raster }
    }
}

/// Manually set (or automatically guessed) georeferencing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GeorefOverride {
    pub transform: GeoTransform,
    pub crs: Crs,
    /// Where it came from, shown in the UI.
    pub note: String,
}

fn yes() -> bool {
    true
}
fn one() -> f32 {
    1.0
}

/// Everything about a layer that is saved in the project file.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LayerConfig {
    pub name: String,
    pub path: PathBuf,
    pub kind: LayerKind,
    #[serde(default = "yes")]
    pub visible: bool,
    #[serde(default = "one")]
    pub opacity: f32,
    /// Translation applied after georeferencing, in world units (metres).
    #[serde(default)]
    pub offset: [f64; 2],
    #[serde(default)]
    pub georef_override: Option<GeorefOverride>,
    #[serde(default)]
    pub raster_style: RasterStyle,
    #[serde(default)]
    pub vector_style: VectorStyle,
}

impl LayerConfig {
    pub fn new(path: PathBuf) -> Self {
        LayerConfig {
            name: geo::file_name(&path),
            kind: LayerKind::for_path(&path),
            path,
            visible: true,
            opacity: 1.0,
            offset: [0.0, 0.0],
            georef_override: None,
            raster_style: RasterStyle::default(),
            vector_style: VectorStyle::default(),
        }
    }
}

pub enum LayerState {
    /// Hidden: not in memory. It is read when shown, and dropped again
    /// when hidden, so only the layers on screen take memory (a decoded
    /// relief GeoTIFF can take a gigabyte).
    Unloaded,
    Loading(Arc<Progress>),
    Ready(LayerData),
    Failed(String),
}

/// What to do with a layer for whether it is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Residency {
    /// Read it.
    Load,
    /// Drop its data (and a read still running).
    Drop,
    /// Forget that it failed: tried again when shown again.
    Retry,
    Keep,
}

impl LayerState {
    /// Only shown layers are in memory.
    pub fn residency(&self, shown: bool) -> Residency {
        match (self, shown) {
            (LayerState::Unloaded, true) => Residency::Load,
            (LayerState::Ready(_) | LayerState::Loading(_), false) => Residency::Drop,
            (LayerState::Failed(_), false) => Residency::Retry,
            _ => Residency::Keep,
        }
    }
}

/// What is known of a layer once it was read: kept when it is dropped
/// (hidden), so it can still be placed, framed and described.
#[derive(Clone, Debug)]
pub enum Known {
    Raster { width: u32, height: u32, file_geo: FileGeoInfo, format: String, bytes: usize },
    Vector { bounds: [f64; 4], summary: String },
}

impl Known {
    pub fn of(data: &LayerData) -> Known {
        match data {
            LayerData::Raster(r) => Known::Raster { width: r.width, height: r.height, file_geo: r.file_geo.clone(), format: r.format.clone(), bytes: r.bytes },
            LayerData::Vector(v) => Known::Vector {
                bounds: v.bounds,
                summary: format!("{} boundary ways · {} water lines · {} points · {} named areas", v.ways.len(), v.water.len(), v.n_points, v.areas.len()),
            },
        }
    }
}

pub enum LayerData {
    Raster(RasterLayer),
    Vector(VectorData),
}

pub struct RasterLayer {
    pub raster: Arc<RwLock<Raster>>,
    pub file_geo: FileGeoInfo,
    pub width: u32,
    pub height: u32,
    pub max_level: u8,
    /// Bumped whenever the display style changes (invalidates tiles).
    pub generation: u64,
    /// Style the current generation's tiles are rendered with.
    pub style: Arc<RasterStyle>,
    pub format: String,
    pub bytes: usize,
}

impl tiles::TileSource for RasterLayer {
    fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }
    fn max_level(&self) -> u8 {
        self.max_level
    }
    fn generation(&self) -> u64 {
        self.generation
    }
    fn job(&self, level: u8, r: [u32; 4]) -> tiles::TileJob {
        let (raster, style) = (self.raster.clone(), self.style.clone());
        Box::new(move || raster.read().render_tile(&style, level as u32, r[0], r[1], r[2], r[3]))
    }
}

impl RasterLayer {
    pub fn new(raster: Raster, file_geo: FileGeoInfo, style: &RasterStyle) -> Self {
        RasterLayer {
            width: raster.width,
            height: raster.height,
            max_level: tiles::max_level(raster.width, raster.height),
            generation: 1,
            style: Arc::new(style.clone()),
            format: raster.format.clone(),
            bytes: raster.byte_size(),
            raster: Arc::new(RwLock::new(raster)),
            file_geo,
        }
    }
}

pub struct Layer {
    pub id: u64,
    pub cfg: LayerConfig,
    pub state: LayerState,
    /// What was learnt when it was last read (None: never read).
    pub known: Option<Known>,
}

/// Effective georeferencing of a raster layer.
pub struct Georef {
    pub transform: GeoTransform,
    pub crs: Crs,
    pub source: String,
}

impl Layer {
    pub fn raster(&self) -> Option<&RasterLayer> {
        match &self.state {
            LayerState::Ready(LayerData::Raster(r)) => Some(r),
            _ => None,
        }
    }

    pub fn is_loading(&self) -> bool {
        matches!(self.state, LayerState::Loading(_))
    }

    /// Not in memory (hidden).
    pub fn is_unloaded(&self) -> bool {
        matches!(self.state, LayerState::Unloaded)
    }

    /// A raster's size and the georeference of its file: from its data, or
    /// from when it was last read.
    pub fn shape(&self) -> Option<(u32, u32, &FileGeoInfo)> {
        match (&self.state, &self.known) {
            (LayerState::Ready(LayerData::Raster(r)), _) => Some((r.width, r.height, &r.file_geo)),
            (_, Some(Known::Raster { width, height, file_geo, .. })) => Some((*width, *height, file_geo)),
            _ => None,
        }
    }

    /// Georeferencing in effect: manual override, else the file's own.
    pub fn georef(&self) -> Option<Georef> {
        if let Some(o) = &self.cfg.georef_override {
            return Some(Georef { transform: o.transform, crs: o.crs, source: o.note.clone() });
        }
        let (_, _, fg) = self.shape()?;
        let (gt, src) = fg.transform.clone()?;
        let crs = fg.crs.as_ref().map(|c| c.0).unwrap_or(Crs::UNKNOWN);
        Some(Georef { transform: gt, crs, source: src })
    }

    /// Mapping between raster pixels and world coordinates, if known.
    pub fn placement(&self, project_crs: Crs) -> Option<Placement> {
        let g = self.georef()?;
        let (w, h, _) = self.shape()?;
        Placement::new(g.transform, g.crs, project_crs, self.cfg.offset, w, h)
    }

    /// World-space bounds (min x, min y, max x, max y) of the layer (also
    /// of a hidden one read before).
    pub fn world_bounds(&self, project_crs: Crs) -> Option<[f64; 4]> {
        if self.shape().is_some() {
            return self.placement(project_crs).map(|p| p.bounds());
        }
        let b = match (&self.state, &self.known) {
            (LayerState::Ready(LayerData::Vector(v)), _) => v.bounds,
            (_, Some(Known::Vector { bounds, .. })) => *bounds,
            _ => return None,
        };
        let o = self.cfg.offset;
        (b[0] <= b[2]).then(|| [b[0] + o[0], b[1] + o[1], b[2] + o[0], b[3] + o[1]])
    }
}

/// Pixel ↔ world mapping for a raster layer (geotransform, optional
/// reprojection into the project CRS, then the layer offset).
pub struct Placement {
    gt: GeoTransform,
    inv: GeoTransform,
    layer_crs: Crs,
    project_crs: Crs,
    offset: [f64; 2],
    /// True if no reprojection is needed (pure affine mapping).
    pub affine: bool,
    pub width: u32,
    pub height: u32,
}

impl Placement {
    pub fn new(gt: GeoTransform, layer_crs: Crs, project_crs: Crs, offset: [f64; 2], width: u32, height: u32) -> Option<Self> {
        Some(Placement {
            inv: gt.inverse()?,
            gt,
            affine: geo::is_identity(layer_crs, project_crs),
            layer_crs,
            project_crs,
            offset,
            width,
            height,
        })
    }

    #[inline]
    pub fn pixel_to_world(&self, col: f64, row: f64) -> Option<[f64; 2]> {
        let (x, y) = self.gt.apply(col, row);
        let (x, y) = if self.affine { (x, y) } else { geo::transform(self.layer_crs, self.project_crs, x, y)? };
        Some([x + self.offset[0], y + self.offset[1]])
    }

    #[inline]
    pub fn world_to_pixel(&self, w: [f64; 2]) -> Option<[f64; 2]> {
        let (x, y) = (w[0] - self.offset[0], w[1] - self.offset[1]);
        let (x, y) = if self.affine { (x, y) } else { geo::transform(self.project_crs, self.layer_crs, x, y)? };
        let (c, r) = self.inv.apply(x, y);
        Some([c, r])
    }

    /// Points along the raster border (for bounds / outlines).
    pub fn outline(&self, per_edge: usize) -> Vec<[f64; 2]> {
        let (w, h) = (self.width as f64, self.height as f64);
        let n = if self.affine { 1 } else { per_edge.max(1) };
        let mut px = Vec::with_capacity(4 * n + 1);
        for i in 0..n {
            px.push((w * i as f64 / n as f64, 0.0));
        }
        for i in 0..n {
            px.push((w, h * i as f64 / n as f64));
        }
        for i in 0..n {
            px.push((w * (1.0 - i as f64 / n as f64), h));
        }
        for i in 0..n {
            px.push((0.0, h * (1.0 - i as f64 / n as f64)));
        }
        px.push((0.0, 0.0));
        px.into_iter().filter_map(|(c, r)| self.pixel_to_world(c, r)).collect()
    }

    pub fn bounds(&self) -> [f64; 4] {
        self.outline(16).iter().fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |b, p| {
            [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])]
        })
    }

    /// Approximate size of one pixel in world units near the raster centre.
    pub fn pixel_size_world(&self) -> f64 {
        let (cx, cy) = (self.width as f64 / 2.0, self.height as f64 / 2.0);
        let d = |a: Option<[f64; 2]>, b: Option<[f64; 2]>| match (a, b) {
            (Some(a), Some(b)) => Some((a[0] - b[0]).hypot(a[1] - b[1])),
            _ => None,
        };
        let c = self.pixel_to_world(cx, cy);
        match (d(c, self.pixel_to_world(cx + 1.0, cy)), d(c, self.pixel_to_world(cx, cy + 1.0))) {
            (Some(a), Some(b)) if a > 0.0 && b > 0.0 => (a * b).sqrt(),
            _ => {
                let (a, b) = self.gt.pixel_size();
                (a * b).sqrt()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hidden_layer_keeps_its_place() {
        let gt = GeoTransform::north_up(1000.0, 2000.0, 10.0, 10.0);
        let file_geo = FileGeoInfo { transform: Some((gt, "file".into())), crs: Some((Crs::LAEA_EUROPE, "file".into())), nodata: None };
        let mut l = Layer { id: 1, cfg: LayerConfig::new(PathBuf::from("a.tif")), state: LayerState::Unloaded, known: None };
        // never read: nothing known
        assert!(l.world_bounds(Crs::LAEA_EUROPE).is_none() && l.georef().is_none() && l.is_unloaded());
        // read once, then dropped: still placed
        l.known = Some(Known::Raster { width: 30, height: 20, file_geo, format: "x".into(), bytes: 600 });
        assert_eq!(l.world_bounds(Crs::LAEA_EUROPE), Some([1000.0, 1800.0, 1300.0, 2000.0]));
        assert_eq!(l.georef().map(|g| g.source), Some("file".to_string()));
        l.cfg.offset = [5.0, -5.0];
        assert_eq!(l.world_bounds(Crs::LAEA_EUROPE), Some([1005.0, 1795.0, 1305.0, 1995.0]));
        // an OSM layer: its bounds
        let mut v = Layer { id: 2, cfg: LayerConfig::new(PathBuf::from("a.osm.pbf")), state: LayerState::Unloaded, known: Some(Known::Vector { bounds: [1.0, 2.0, 3.0, 4.0], summary: String::new() }) };
        assert_eq!(v.world_bounds(Crs::LAEA_EUROPE), Some([1.0, 2.0, 3.0, 4.0]));
        v.known = Some(Known::Vector { bounds: [f64::MAX, f64::MAX, f64::MIN, f64::MIN], summary: String::new() });
        assert_eq!(v.world_bounds(Crs::LAEA_EUROPE), None, "an empty extract has no bounds");
    }

    #[test]
    fn only_shown_layers_are_in_memory() {
        let loading = LayerState::Loading(Arc::new(Progress::default()));
        let failed = LayerState::Failed("gone".into());
        assert_eq!(LayerState::Unloaded.residency(true), Residency::Load);
        assert_eq!(LayerState::Unloaded.residency(false), Residency::Keep);
        assert_eq!(loading.residency(false), Residency::Drop, "hidden while it is read");
        assert_eq!(loading.residency(true), Residency::Keep);
        assert_eq!(failed.residency(true), Residency::Keep, "no retry while shown");
        assert_eq!(failed.residency(false), Residency::Retry);
    }
}
