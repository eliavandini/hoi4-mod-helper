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
    Loading(Arc<Progress>),
    Ready(LayerData),
    Failed(String),
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

    /// Georeferencing in effect: manual override, else the file's own.
    pub fn georef(&self) -> Option<Georef> {
        if let Some(o) = &self.cfg.georef_override {
            return Some(Georef { transform: o.transform, crs: o.crs, source: o.note.clone() });
        }
        let r = self.raster()?;
        let (gt, src) = r.file_geo.transform.clone()?;
        let crs = r.file_geo.crs.as_ref().map(|c| c.0).unwrap_or(Crs::UNKNOWN);
        Some(Georef { transform: gt, crs, source: src })
    }

    /// Mapping between raster pixels and world coordinates, if known.
    pub fn placement(&self, project_crs: Crs) -> Option<Placement> {
        let g = self.georef()?;
        let r = self.raster()?;
        Placement::new(g.transform, g.crs, project_crs, self.cfg.offset, r.width, r.height)
    }

    /// World-space bounds (min x, min y, max x, max y) of the layer.
    pub fn world_bounds(&self, project_crs: Crs) -> Option<[f64; 4]> {
        match &self.state {
            LayerState::Ready(LayerData::Raster(_)) => self.placement(project_crs).map(|p| p.bounds()),
            LayerState::Ready(LayerData::Vector(v)) => {
                let o = self.cfg.offset;
                (v.bounds[0] <= v.bounds[2]).then(|| [v.bounds[0] + o[0], v.bounds[1] + o[1], v.bounds[2] + o[0], v.bounds[3] + o[1]])
            }
            _ => None,
        }
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
