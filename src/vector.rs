//! Display-ready vector data: boundaries, rivers and lake outlines projected
//! into the project CRS with precomputed level-of-detail simplifications.

use std::collections::BTreeMap;

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::geo::Crs;
use crate::osm::{OsmBoundaries, Water};

/// Simplification tolerances (world units, i.e. metres) of the LOD levels.
pub const LOD_TOL: [f32; 7] = [1.0, 4.0, 16.0, 64.0, 256.0, 1024.0, 4096.0];

pub struct VectorData {
    /// World coordinates of the local origin all f32 coordinates are relative to.
    pub origin: [f64; 2],
    pub ways: Vec<VWay>,
    pub areas: Vec<VArea>,
    /// Rivers, canals and the outlines of rivers and lakes (lakes first).
    pub water: Vec<VWater>,
    /// Number of ways per admin level.
    pub level_counts: BTreeMap<u8, usize>,
    /// Number of water lines per kind.
    pub water_counts: BTreeMap<Water, usize>,
    /// World bounds: min x, min y, max x, max y.
    pub bounds: [f64; 4],
    pub n_points: usize,
}

pub struct VWay {
    pub level: u8,
    /// Relative to `origin`: min x, min y, max x, max y.
    pub bbox: [f32; 4],
    /// `lods[i]` is simplified with tolerance `LOD_TOL[i]`.
    pub lods: Vec<Vec<[f32; 2]>>,
}

pub struct VWater {
    pub kind: Water,
    /// Relative to `origin`: min x, min y, max x, max y.
    pub bbox: [f32; 4],
    /// `lods[i]` is simplified with tolerance `LOD_TOL[i]`.
    pub lods: Vec<Vec<[f32; 2]>>,
}

/// The simplifications of a line, `LOD_TOL[i]` for the i-th.
fn lods_of(pts: &[[f32; 2]]) -> Vec<Vec<[f32; 2]>> {
    let mut lods = Vec::with_capacity(LOD_TOL.len());
    let mut cur = simplify(pts, LOD_TOL[0]);
    for &tol in &LOD_TOL[1..] {
        let next = simplify(&cur, tol);
        lods.push(cur);
        cur = next;
    }
    lods.push(cur);
    lods
}

pub struct VArea {
    pub name: String,
    pub level: u8,
    pub label: [f32; 2],
    pub bbox: [f32; 4],
}

impl VectorData {
    pub fn build(b: &OsmBoundaries, crs: Crs) -> VectorData {
        let project = |c: [i32; 2]| -> Option<[f64; 2]> {
            let (x, y) = crs.from_lonlat(c[0] as f64 * 1e-7, c[1] as f64 * 1e-7)?;
            Some([x, y])
        };
        // Local origin: first projectable point (keeps f32 coordinates precise).
        let origin = b.ways.iter().flat_map(|w| w.coords.first()).chain(b.water.iter().flat_map(|w| w.coords.first())).find_map(|&c| project(c)).unwrap_or([0.0, 0.0]);
        let rel = |p: [f64; 2]| [(p[0] - origin[0]) as f32, (p[1] - origin[1]) as f32];

        let ways: Vec<VWay> = b
            .ways
            .par_iter()
            .filter_map(|w| {
                let pts: Vec<[f32; 2]> = w.coords.iter().filter_map(|&c| project(c)).map(rel).collect();
                if pts.len() < 2 {
                    return None;
                }
                Some(VWay { level: w.level, bbox: bbox_of(&pts), lods: lods_of(&pts) })
            })
            .collect();
        let water: Vec<VWater> = b
            .water
            .par_iter()
            .filter_map(|w| {
                let pts: Vec<[f32; 2]> = w.coords.iter().filter_map(|&c| project(c)).map(rel).collect();
                (pts.len() >= 2).then(|| VWater { kind: w.kind, bbox: bbox_of(&pts), lods: lods_of(&pts) })
            })
            .collect();

        let mut areas: Vec<VArea> = b
            .areas
            .iter()
            .filter_map(|a| {
                let label = rel(project(a.label?)?);
                let bb = a.bbox?;
                let p0 = rel(project([bb[0], bb[1]])?);
                let p1 = rel(project([bb[2], bb[3]])?);
                let p2 = rel(project([bb[0], bb[3]])?);
                let p3 = rel(project([bb[2], bb[1]])?);
                Some(VArea { name: a.name.clone(), level: a.level, label, bbox: bbox_of(&[p0, p1, p2, p3]) })
            })
            .collect();
        areas.sort_by_key(|a| a.level);

        let mut level_counts = BTreeMap::new();
        let mut water_counts = BTreeMap::new();
        let mut bounds = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
        let mut n_points = 0;
        let mut grow = |bb: [f32; 4]| {
            bounds = [
                bounds[0].min(origin[0] + bb[0] as f64),
                bounds[1].min(origin[1] + bb[1] as f64),
                bounds[2].max(origin[0] + bb[2] as f64),
                bounds[3].max(origin[1] + bb[3] as f64),
            ];
        };
        for w in &ways {
            *level_counts.entry(w.level).or_insert(0) += 1;
            n_points += w.lods[0].len();
            grow(w.bbox);
        }
        for w in &water {
            *water_counts.entry(w.kind).or_insert(0) += 1;
            n_points += w.lods[0].len();
            grow(w.bbox);
        }
        VectorData { origin, ways, areas, water, level_counts, water_counts, bounds, n_points }
    }

    /// Pick the coarsest LOD whose tolerance is below `max_err` world units.
    pub fn lod_for(max_err: f64) -> usize {
        LOD_TOL.iter().rposition(|&t| (t as f64) <= max_err).unwrap_or(0)
    }
}

fn bbox_of(pts: &[[f32; 2]]) -> [f32; 4] {
    pts.iter().fold([f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY], |b, p| {
        [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])]
    })
}

/// Douglas–Peucker polyline simplification (iterative).
pub fn simplify(pts: &[[f32; 2]], tol: f32) -> Vec<[f32; 2]> {
    let n = pts.len();
    if n <= 2 {
        return pts.to_vec();
    }
    let tol2 = tol * tol;
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[n - 1] = true;
    let mut stack = vec![(0usize, n - 1)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let (pa, pb) = (pts[a], pts[b]);
        let (dx, dy) = (pb[0] - pa[0], pb[1] - pa[1]);
        let len2 = dx * dx + dy * dy;
        let mut best = (0.0f32, 0usize);
        for (i, p) in pts.iter().enumerate().take(b).skip(a + 1) {
            let d2 = if len2 <= f32::EPSILON {
                (p[0] - pa[0]).powi(2) + (p[1] - pa[1]).powi(2)
            } else {
                let t = (((p[0] - pa[0]) * dx + (p[1] - pa[1]) * dy) / len2).clamp(0.0, 1.0);
                (p[0] - pa[0] - t * dx).powi(2) + (p[1] - pa[1] - t * dy).powi(2)
            };
            if d2 > best.0 {
                best = (d2, i);
            }
        }
        if best.0 > tol2 {
            keep[best.1] = true;
            stack.push((a, best.1));
            stack.push((best.1, b));
        }
    }
    pts.iter().zip(keep).filter(|(_, k)| *k).map(|(p, _)| *p).collect()
}

/// Display settings for one admin level.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LevelStyle {
    pub visible: bool,
    pub color: [u8; 4],
    pub width: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VectorStyle {
    pub levels: BTreeMap<u8, LevelStyle>,
    /// Rivers and lake outlines, by kind.
    #[serde(default)]
    pub water: BTreeMap<Water, LevelStyle>,
    pub labels: bool,
    pub width_scale: f32,
}

impl Default for VectorStyle {
    fn default() -> Self {
        VectorStyle { levels: BTreeMap::new(), water: BTreeMap::new(), labels: true, width_scale: 1.0 }
    }
}

impl VectorStyle {
    pub fn level(&mut self, level: u8) -> &mut LevelStyle {
        self.levels.entry(level).or_insert_with(|| default_level_style(level))
    }

    pub fn get(&self, level: u8) -> LevelStyle {
        self.levels.get(&level).copied().unwrap_or_else(|| default_level_style(level))
    }

    pub fn water_mut(&mut self, w: Water) -> &mut LevelStyle {
        self.water.entry(w).or_insert_with(|| default_water_style(w))
    }

    pub fn water(&self, w: Water) -> LevelStyle {
        self.water.get(&w).copied().unwrap_or_else(|| default_water_style(w))
    }
}

/// Water in blues, rivers widest; canals start hidden.
pub fn default_water_style(w: Water) -> LevelStyle {
    let (color, width, visible) = match w {
        Water::River => ([70, 150, 255, 255], 1.6, true),
        Water::RiverArea => ([110, 180, 255, 255], 1.0, true),
        Water::Lake => ([90, 170, 255, 255], 1.3, true),
        Water::Canal => ([120, 190, 230, 255], 0.9, false),
    };
    LevelStyle { visible, color, width }
}

pub fn default_level_style(level: u8) -> LevelStyle {
    let (color, width) = match level {
        0..=2 => ([255, 255, 255, 255], 3.0),
        3 => ([255, 90, 90, 255], 2.6),
        4 => ([255, 70, 210, 255], 2.4),
        5 => ([255, 120, 40, 255], 2.0),
        6 => ([255, 170, 0, 255], 1.8),
        7 => ([220, 220, 60, 255], 1.5),
        8 => ([255, 255, 90, 255], 1.2),
        9 => ([80, 230, 255, 255], 1.0),
        10 => ([130, 255, 130, 255], 0.9),
        _ => ([190, 190, 255, 255], 0.8),
    };
    LevelStyle { visible: level <= 8, color, width }
}

/// Typical meaning of an admin level in Austria / Italy, for the UI.
pub fn level_name(level: u8) -> &'static str {
    match level {
        2 => "country",
        3 => "macro-region",
        4 => "state / region",
        5 => "-",
        6 => "district / province",
        7 => "comunità di valle / -",
        8 => "municipality",
        9 => "municipal part",
        10 => "fraction / cadastral",
        11 => "locality",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simplify_straight_line() {
        let pts: Vec<[f32; 2]> = (0..100).map(|i| [i as f32, 0.0]).collect();
        assert_eq!(simplify(&pts, 0.5), vec![[0.0, 0.0], [99.0, 0.0]]);
        let zig = vec![[0.0, 0.0], [1.0, 5.0], [2.0, 0.0]];
        assert_eq!(simplify(&zig, 1.0).len(), 3);
    }

    #[test]
    fn water_is_projected_and_counted() {
        use crate::osm::{BoundaryWay, WaterLine};
        let b = OsmBoundaries {
            ways: vec![BoundaryWay { level: 8, coords: vec![[113_000_000, 464_000_000], [113_100_000, 464_000_000]] }],
            areas: Vec::new(),
            water: vec![
                WaterLine { kind: Water::Lake, coords: vec![[112_600_000, 463_800_000], [112_610_000, 463_800_000], [112_610_000, 463_810_000], [112_600_000, 463_800_000]] },
                WaterLine { kind: Water::River, coords: vec![[113_200_000, 464_700_000], [113_200_000, 464_000_000]] },
                WaterLine { kind: Water::River, coords: vec![[113_200_000, 464_700_000]] },
            ],
        };
        let v = VectorData::build(&b, Crs::LAEA_EUROPE);
        assert_eq!(v.water.len(), 2, "a line needs two points");
        assert_eq!(v.water_counts.get(&Water::River), Some(&1));
        assert_eq!(v.n_points, 2 + 4 + 2);
        // the river reaches further north than the boundary: in the bounds
        let top = v.origin[1] + v.water[1].bbox[3] as f64;
        assert!(v.bounds[3] >= top - 1.0 && v.bounds[3] > v.origin[1] + v.ways[0].bbox[3] as f64);
    }

    #[test]
    fn styles_saved_before_water_still_load() {
        let old = r#"{"levels":{"8":{"visible":true,"color":[255,255,90,255],"width":1.2}},"labels":true,"width_scale":1.0}"#;
        let mut s: VectorStyle = serde_json::from_str(old).unwrap();
        assert!(s.water.is_empty() && s.water(Water::River).visible && !s.water(Water::Canal).visible);
        s.water_mut(Water::Lake).visible = false;
        let back: VectorStyle = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert!(!back.water(Water::Lake).visible && back == s);
    }

    #[test]
    fn lod_choice() {
        assert_eq!(VectorData::lod_for(0.1), 0);
        assert_eq!(VectorData::lod_for(20.0), 2);
        assert_eq!(VectorData::lod_for(1e9), LOD_TOL.len() - 1);
    }
}
