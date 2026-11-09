//! Terrain-aware province density: a wanted province size for every pixel,
//! from the user's height and slope rules, with smaller provinces on
//! built-up land.


use anyhow::Result;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::util::Progress;

/// Land cover class "sealed" (built-up) in landcover_smooth.bmp (CLC+ Backbone).
const SEALED: u8 = 1;

/// Heights, smoothed slope and built-up share per pixel (same grid as provinces.bmp).
pub struct Terrain {
    pub w: u32,
    pub h: u32,
    pub dem: Vec<f32>,
    /// Slope in degrees, mean over 9×9 px.
    pub slope: Vec<u8>,
    /// Share of sealed land over 15×15 px, 0..255.
    pub urban: Vec<u8>,
    /// Sealed (built-up) pixel.
    pub sealed: Vec<bool>,
    /// What the mod build's terrain rules look at, per pixel: the land
    /// cover class and the slope (degrees, not smoothed, as the build has it).
    pub cover: Vec<u8>,
    pub slope_px: Vec<f32>,
}

/// A province size for the land within a height and slope range. A missing
/// limit means none.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TerrainRule {
    /// Shown in the editor only.
    pub name: String,
    /// From this height up (m).
    pub from_m: Option<f32>,
    /// Below this height (m).
    pub below_m: Option<f32>,
    /// From this slope up (degrees, mean over 9×9 px).
    pub from_slope: Option<f32>,
    /// Below this slope.
    pub below_slope: Option<f32>,
    pub km2: f32,
}

impl Default for TerrainRule {
    fn default() -> Self {
        TerrainRule { name: "new rule".into(), from_m: None, below_m: None, from_slope: None, below_slope: None, km2: 8.0 }
    }
}

impl TerrainRule {
    pub fn matches(&self, height: f32, slope: f32) -> bool {
        self.from_m.is_none_or(|v| height >= v)
            && self.below_m.is_none_or(|v| height < v)
            && self.from_slope.is_none_or(|v| slope >= v)
            && self.below_slope.is_none_or(|v| slope < v)
    }
}

/// Wanted province sizes (km²) by terrain; `detail` multiplies the number of
/// provinces (2 = twice as many, half the size).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(from = "StoredDensity")]
pub struct DensityParams {
    /// Built-up land (where it asks for smaller provinces than its rule).
    pub urban_km2: f32,
    /// Land by height and slope: the first rule that matches a pixel.
    pub rules: Vec<TerrainRule>,
    /// Land no rule matches.
    pub other_km2: f32,
    /// Size of sea provinces.
    pub sea_km2: f32,
    pub detail: f32,
}

impl Default for DensityParams {
    fn default() -> Self {
        // make_provinces.py defaults
        DensityParams::from(StoredDensity::default())
    }
}

impl DensityParams {
    /// Province size (km²) of land at this height and slope, before
    /// built-up areas and `detail`.
    pub fn land_km2(&self, height: f32, slope: f32) -> f32 {
        self.rules.iter().find(|r| r.matches(height, slope)).map_or(self.other_km2, |r| r.km2)
    }
}

/// What settings files hold: the rules, or the fixed sizes from before them.
#[derive(Default, Deserialize)]
#[serde(default)]
struct StoredDensity {
    urban_km2: Option<f32>,
    rules: Option<Vec<TerrainRule>>,
    other_km2: Option<f32>,
    sea_km2: Option<f32>,
    detail: Option<f32>,
    // before the rules: valley floors below 1600 m and 12°, mountains from 2000 m
    valley_km2: Option<f32>,
    slope_km2: Option<f32>,
    mountain_km2: Option<f32>,
}

impl From<StoredDensity> for DensityParams {
    fn from(s: StoredDensity) -> Self {
        let rules = s.rules.unwrap_or_else(|| {
            vec![
                TerrainRule { name: "valley floors".into(), below_m: Some(1600.0), below_slope: Some(12.0), km2: s.valley_km2.unwrap_or(1.5), ..Default::default() },
                TerrainRule { name: "mountains".into(), from_m: Some(2000.0), km2: s.mountain_km2.unwrap_or(30.0), ..Default::default() },
            ]
        });
        DensityParams {
            urban_km2: s.urban_km2.unwrap_or(0.5),
            rules,
            other_km2: s.other_km2.or(s.slope_km2).unwrap_or(8.0),
            sea_km2: s.sea_km2.unwrap_or(0.8),
            detail: s.detail.unwrap_or(1.0),
        }
    }
}

/// Load dem_section.tif and landcover_smooth.bmp from the map folder (or
/// what stands in for them, see [`crate::modbuild::bitmaps::inputs`]).
pub fn load_terrain(src: &crate::modbuild::bitmaps::Sources, w: u32, h: u32, pixel_m: f64, heights: Option<&[u8]>, progress: &Progress) -> Result<Terrain> {
    let crate::modbuild::bitmaps::Inputs { dem, cover: lc, .. } = crate::modbuild::bitmaps::inputs(src, w as usize, h as usize, heights, progress)?;
    progress.set_stage("slopes");
    // the build's slope uses the map's pixel size as it is (f64)
    let build_px_m = pixel_m;
    let pixel_m = pixel_m as f32;
    let (wu, hu) = (w as usize, h as usize);
    let dem_c: Vec<f32> = dem.iter().map(|&v| if v < -1000.0 { 0.0 } else { v }).collect();
    let slope_deg: Vec<f32> = (0..hu)
        .into_par_iter()
        .flat_map_iter(|y| {
            let dem_c = &dem_c;
            (0..wu).map(move |x| {
                let at = |xx: usize, yy: usize| dem_c[yy * wu + xx];
                let (xa, xb) = (x.saturating_sub(1), (x + 1).min(wu - 1));
                let (ya, yb) = (y.saturating_sub(1), (y + 1).min(hu - 1));
                let gx = (at(xb, y) - at(xa, y)) / ((xb - xa).max(1) as f32 * pixel_m);
                let gy = (at(x, yb) - at(x, ya)) / ((yb - ya).max(1) as f32 * pixel_m);
                gx.hypot(gy).atan().to_degrees()
            })
        })
        .collect();
    let slope_px = crate::modbuild::slope_degrees(&dem, wu, hu, build_px_m);
    let slope = box_mean(&slope_deg, wu, hu, 4).into_iter().map(|v| v.round().clamp(0.0, 255.0) as u8).collect();
    let sealed: Vec<bool> = lc.iter().map(|&c| c == SEALED).collect();
    let sealed_f: Vec<f32> = sealed.iter().map(|&s| s as u8 as f32).collect();
    let urban = box_mean(&sealed_f, wu, hu, 7).into_iter().map(|v| (v * 255.0).round().clamp(0.0, 255.0) as u8).collect();
    Ok(Terrain { w, h, dem, slope, urban, sealed, cover: lc, slope_px })
}

/// Mean over a (2r+1)² window (shrunk at the borders).
pub fn box_mean(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let rows: Vec<f32> = (0..h)
        .into_par_iter()
        .flat_map_iter(|y| {
            let row = &src[y * w..(y + 1) * w];
            let mut pre = vec![0f64; w + 1];
            for x in 0..w {
                pre[x + 1] = pre[x] + row[x] as f64;
            }
            (0..w).map(move |x| {
                let (a, b) = (x.saturating_sub(r), (x + r + 1).min(w));
                ((pre[b] - pre[a]) / (b - a) as f64) as f32
            })
        })
        .collect();
    let mut out = vec![0f32; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, o)| {
        let (a, b) = (y.saturating_sub(r), (y + r + 1).min(h));
        for yy in a..b {
            let row = &rows[yy * w..(yy + 1) * w];
            for x in 0..w {
                o[x] += row[x];
            }
        }
        let n = (b - a) as f32;
        for v in o.iter_mut() {
            *v /= n;
        }
    });
    out
}

/// Wanted provinces per km² for every pixel (land rules everywhere; callers
/// use their own rule for sea). Smoothed like make_provinces.py (Gaussian
/// σ≈6 px, here three box passes).
pub fn density(t: &Terrain, p: &DensityParams) -> Vec<f32> {
    let raw: Vec<f32> = (0..t.dem.len())
        .into_par_iter()
        .map(|i| {
            let mut d = 1.0 / p.land_km2(t.dem[i], t.slope[i] as f32).max(0.001);
            let urban = t.urban[i] as f32 / 255.0;
            d = d.max((urban / 0.3).clamp(0.0, 1.0) / p.urban_km2.max(0.001));
            d * p.detail
        })
        .collect();
    let (w, h) = (t.w as usize, t.h as usize);
    let a = box_mean(&raw, w, h, 6);
    let b = box_mean(&a, w, h, 6);
    box_mean(&b, w, h, 6)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_first_match_wins() {
        let p = DensityParams::default();
        assert_eq!(p.land_km2(1000.0, 5.0), 1.5); // valley floor
        assert_eq!(p.land_km2(1000.0, 20.0), 8.0); // slope: no rule
        assert_eq!(p.land_km2(1800.0, 5.0), 8.0);
        assert_eq!(p.land_km2(2000.0, 5.0), 30.0);
        let mut q = p.clone();
        q.rules.insert(0, TerrainRule { from_m: Some(2500.0), km2: 60.0, ..Default::default() });
        assert_eq!(q.land_km2(2600.0, 30.0), 60.0);
        assert_eq!(q.land_km2(2400.0, 30.0), 30.0);
    }

    #[test]
    fn old_settings_become_rules() {
        let old = r#"{"urban_km2":0.1,"valley_km2":0.13,"slope_km2":3.0,"mountain_km2":30.0,"sea_km2":0.8,"detail":1.0}"#;
        let p: DensityParams = serde_json::from_str(old).unwrap();
        assert_eq!((p.urban_km2, p.other_km2, p.rules.len()), (0.1, 3.0, 2));
        assert_eq!(p.land_km2(1000.0, 5.0), 0.13);
        assert_eq!(p.land_km2(2100.0, 5.0), 30.0);
        // and the new form reads back as it was written
        let back: DensityParams = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn box_mean_of_constant_is_constant() {
        let v = vec![3.0f32; 50 * 30];
        assert!(box_mean(&v, 50, 30, 4).iter().all(|&x| (x - 3.0).abs() < 1e-5));
    }

    #[test]
    fn box_mean_window() {
        let mut v = vec![0f32; 9];
        v[4] = 9.0; // centre of 3×3
        let m = box_mean(&v, 3, 3, 1);
        assert!((m[4] - 1.0).abs() < 1e-6);
        assert!((m[0] - 9.0 / 4.0).abs() < 1e-6); // corner window is 2×2
    }
}
