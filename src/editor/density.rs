//! Terrain-aware province density, as in make_provinces.py: built-up land gets
//! the smallest provinces, then valley floors, slopes, and high mountains the
//! largest.

use std::path::Path;

use anyhow::{Result, bail};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::raster::{self, Pixels};
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
}

/// Wanted province sizes (km²) by terrain; `detail` multiplies the number of
/// provinces (2 = twice as many, half the size).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct DensityParams {
    pub urban_km2: f32,
    pub valley_km2: f32,
    pub slope_km2: f32,
    pub mountain_km2: f32,
    /// Size of sea provinces.
    pub sea_km2: f32,
    pub detail: f32,
}

impl Default for DensityParams {
    fn default() -> Self {
        // make_provinces.py defaults
        DensityParams { urban_km2: 0.5, valley_km2: 1.5, slope_km2: 8.0, mountain_km2: 30.0, sea_km2: 0.8, detail: 1.0 }
    }
}

/// Load dem_section.tif and landcover_smooth.bmp from the map folder.
pub fn load_terrain(dir: &Path, w: u32, h: u32, pixel_m: f64, progress: &Progress) -> Result<Terrain> {
    let (dem_r, _) = raster::load(&dir.join("dem_section.tif"), progress)?;
    let (lc_r, _) = raster::load(&dir.join("landcover_smooth.bmp"), progress)?;
    if (dem_r.width, dem_r.height) != (w, h) || (lc_r.width, lc_r.height) != (w, h) {
        bail!("dem_section.tif / landcover_smooth.bmp are not {w} x {h} like provinces.bmp");
    }
    let dem: Vec<f32> = match dem_r.pixels {
        Pixels::GrayF32(v) => v,
        Pixels::Gray16(v) => v.into_iter().map(|x| x as f32).collect(),
        Pixels::Gray8(v) => v.into_iter().map(|x| x as f32).collect(),
        _ => bail!("dem_section.tif is not a single-band height map"),
    };
    let lc: Vec<u8> = match lc_r.pixels {
        Pixels::Indexed8 { data, .. } | Pixels::Gray8(data) => data,
        _ => bail!("landcover_smooth.bmp is not an 8-bit class map"),
    };
    progress.set_stage("slopes");
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
    let slope = box_mean(&slope_deg, wu, hu, 4).into_iter().map(|v| v.round().clamp(0.0, 255.0) as u8).collect();
    let sealed: Vec<bool> = lc.iter().map(|&c| c == SEALED).collect();
    let sealed_f: Vec<f32> = sealed.iter().map(|&s| s as u8 as f32).collect();
    let urban = box_mean(&sealed_f, wu, hu, 7).into_iter().map(|v| (v * 255.0).round().clamp(0.0, 255.0) as u8).collect();
    Ok(Terrain { w, h, dem, slope, urban, sealed })
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
            let dem = t.dem[i];
            let mut d = if dem >= 2000.0 { 1.0 / p.mountain_km2 } else { 1.0 / p.slope_km2 };
            if (t.slope[i] as f32) < 12.0 && dem < 1600.0 {
                d = 1.0 / p.valley_km2;
            }
            let urban = t.urban[i] as f32 / 255.0;
            d = d.max((urban / 0.3).clamp(0.0, 1.0) / p.urban_km2);
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
