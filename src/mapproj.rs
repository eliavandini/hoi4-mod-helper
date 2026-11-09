//! What the pixels of a map are on the Earth: a plain raster (pixels of some
//! size, on no place in particular) or a map projection that puts them on
//! the Earth, so real geographic data (OSM extracts, georeferenced rasters)
//! lines up with them. It is kept next to provinces.bmp as GDAL keeps it: a
//! world file (`provinces.wld`) and, for a projection, `provinces.bmp.aux.xml`
//! naming the CRS. New projects choose one; the Map projection window
//! changes it (the pixels stay as they are, only where they are changes).

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::geo::{self, Crs, FileGeoInfo, GeoTransform};

/// HOI4's limits for the map: width and height multiples of 256, at most
/// this many pixels.
pub const MAX_PIXELS: u64 = 13_238_272;

/// Vanilla's map size.
pub const VANILLA_SIZE: (u32, u32) = (5632, 2048);

/// Whether HOI4 takes a map of this size; else why not.
pub fn check_size(w: u32, h: u32) -> Result<(), String> {
    if w == 0 || h == 0 || !w.is_multiple_of(256) || !h.is_multiple_of(256) {
        return Err(format!("{w} × {h}: width and height must be multiples of 256"));
    }
    if w as u64 * h as u64 > MAX_PIXELS {
        return Err(format!("{w} × {h} is {:.1} million pixels; HOI4 takes at most {:.1} million", (w as u64 * h as u64) as f64 / 1e6, MAX_PIXELS as f64 / 1e6));
    }
    Ok(())
}

/// A map's width or height: "40,075 km", "161.7 km", "850 m".
fn fmt_len(m: f64) -> String {
    let km = m / 1000.0;
    if km >= 1000.0 {
        let s = format!("{:.0}", km);
        let mut out = String::new();
        for (i, c) in s.chars().enumerate() {
            if i > 0 && (s.len() - i) % 3 == 0 {
                out.push(',');
            }
            out.push(c);
        }
        format!("{out} km")
    } else if km >= 10.0 {
        format!("{km:.1} km")
    } else {
        crate::util::fmt_dist(m)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Projection {
    /// Pixels `pixel_m` metres wide, on no place of the Earth.
    Plain { pixel_m: f64 },
    /// The map in `crs`, its centre at `center` (lon, lat), a pixel `pixel`
    /// units of the CRS wide (metres; degrees for lon/lat).
    Geo { crs: Crs, center: [f64; 2], pixel: f64 },
}

/// The projections offered for new maps, by name.
pub const CHOICES: &[(&str, &str)] = &[
    ("laea", "Lambert equal area, Europe (EPSG:3035)"),
    ("utm", "UTM, WGS 84 (zone from the centre)"),
    ("utm_etrs", "UTM, ETRS89 (Europe, zone from the centre)"),
    ("mercator", "Web Mercator (EPSG:3857)"),
    ("lonlat", "Longitude / latitude, plate carrée (EPSG:4326)"),
];

/// The CRS of a choice of [`CHOICES`] for a map centred on (lon, lat).
pub fn choice_crs(choice: &str, lon: f64, lat: f64) -> Crs {
    match choice {
        "utm" => geo::utm_crs(lon, lat, false),
        "utm_etrs" => geo::utm_crs(lon, lat, true),
        "mercator" => Crs::new(3857),
        "lonlat" => Crs::new(4326),
        _ => Crs::LAEA_EUROPE,
    }
}

/// The choice of [`CHOICES`] a CRS is.
pub fn crs_choice(crs: Crs) -> &'static str {
    match crs.epsg {
        3857 => "mercator",
        4326 | 4258 => "lonlat",
        25801..=25860 => "utm_etrs",
        32601..=32760 => "utm",
        _ => "laea",
    }
}

impl Projection {
    /// What the map's georeference says it is (`pixel_m`: the map's pixel
    /// size, for a plain raster).
    pub fn of_map(geo: &FileGeoInfo, w: u32, h: u32, pixel_m: f64) -> Projection {
        let crs = geo.crs.as_ref().map(|c| c.0).unwrap_or(Crs::UNKNOWN);
        if let Some((gt, _)) = &geo.transform
            && crs.is_supported()
        {
            let (cx, cy) = gt.apply(w as f64 / 2.0, h as f64 / 2.0);
            if let Some((lon, lat)) = crs.to_lonlat(cx, cy) {
                return Projection::Geo { crs, center: [lon, lat], pixel: gt.pixel_size().0 };
            }
        }
        Projection::Plain { pixel_m: if pixel_m > 0.0 { pixel_m } else { 1000.0 } }
    }

    /// The CRS of the world coordinates ([`Crs::UNKNOWN`] for a plain raster).
    pub fn crs(&self) -> Crs {
        match self {
            Projection::Plain { .. } => Crs::UNKNOWN,
            Projection::Geo { crs, .. } => *crs,
        }
    }

    pub fn is_plain(&self) -> bool {
        matches!(self, Projection::Plain { .. })
    }

    /// Where the pixels are: GDAL's geotransform of the map. A plain raster
    /// has its top-left corner at 0, 0 and `pixel_m` per pixel.
    pub fn transform(&self, w: u32, h: u32) -> Option<GeoTransform> {
        match *self {
            Projection::Plain { pixel_m } => Some(GeoTransform::north_up(0.0, 0.0, pixel_m, pixel_m)),
            Projection::Geo { crs, center, pixel } => {
                let (cx, cy) = crs.from_lonlat(center[0], center[1])?;
                Some(GeoTransform::north_up(cx - w as f64 / 2.0 * pixel, cy + h as f64 / 2.0 * pixel, pixel, pixel))
            }
        }
    }

    /// About how many metres a pixel is (provinces.json's `pixel_m`: areas,
    /// slopes, distances).
    pub fn pixel_m(&self) -> f64 {
        match *self {
            Projection::Plain { pixel_m } => pixel_m,
            Projection::Geo { crs, center, pixel } => {
                if crs.is_geographic() {
                    pixel * geo::M_PER_DEG
                } else if crs.epsg == 3857 {
                    pixel * center[1].to_radians().cos()
                } else {
                    pixel
                }
            }
        }
    }

    /// Longitude and latitude of the corners (top-left, top-right,
    /// bottom-right, bottom-left), for a projection.
    pub fn corners(&self, w: u32, h: u32) -> Option<[(f64, f64); 4]> {
        let Projection::Geo { crs, .. } = self else { return None };
        let gt = self.transform(w, h)?;
        let c = |x: f64, y: f64| {
            let (a, b) = gt.apply(x, y);
            crs.to_lonlat(a, b)
        };
        Some([c(0.0, 0.0)?, c(w as f64, 0.0)?, c(w as f64, h as f64)?, c(0.0, h as f64)?])
    }

    /// "LAEA Europe, 35.1 m per pixel: 161.7 × 98.8 km around 46.70°N 11.40°E".
    pub fn describe(&self, w: u32, h: u32) -> String {
        let pm = self.pixel_m();
        let size = format!("{} × {}", fmt_len(w as f64 * pm), fmt_len(h as f64 * pm));
        match self {
            Projection::Plain { pixel_m } => format!("plain raster, {} per pixel: {size}", crate::util::fmt_dist(*pixel_m)),
            Projection::Geo { crs, center, pixel } => {
                let px = if crs.is_geographic() { format!("{pixel:.5}° (~{})", crate::util::fmt_dist(pm)) } else { crate::util::fmt_dist(*pixel) };
                format!("{}, {px} per pixel: {size} around {}", crs.name(), geo::fmt_lonlat(center[0], center[1]))
            }
        }
    }

    /// The header keys of provinces.json that follow the projection.
    pub fn set_header(&self, header: &mut Map<String, Value>, w: u32, h: u32) {
        header.insert("pixel_m".into(), serde_json::json!(self.pixel_m()));
        match (self, self.transform(w, h)) {
            (Projection::Geo { crs, .. }, Some(gt)) => {
                header.insert("crs".into(), Value::String(format!("EPSG:{}", crs.epsg)));
                header.insert("origin".into(), serde_json::json!([gt.0[0], gt.0[3]]));
            }
            _ => {
                header.remove("crs");
                header.remove("origin");
            }
        }
    }

    /// Write the georeference next to `raster` (see [`geo::write_georef`]).
    pub fn write_georef(&self, raster: &Path, w: u32, h: u32) -> Result<()> {
        let gt = self.transform(w, h).context("the centre is outside of what the projection can show")?;
        let crs = (!self.is_plain()).then(|| self.crs());
        geo::write_georef(raster, Some((gt, crs)))
    }
}

/// Give the saved map in `dir` (w × h) another projection: its old
/// georeference and provinces.json go to `backups/projection-<time>/`, then
/// the new sidecars are written and provinces.json's pixel size, CRS and
/// origin updated. Returns the backup folder.
pub fn apply(dir: &Path, w: u32, h: u32, p: &Projection) -> Result<PathBuf> {
    let bmp = dir.join("provinces.bmp");
    let json_path = dir.join("provinces.json");
    let text = std::fs::read_to_string(&json_path).with_context(|| format!("cannot read {}", json_path.display()))?;
    let mut root: Map<String, Value> = serde_json::from_str(&text).with_context(|| format!("invalid JSON in {}", json_path.display()))?;
    p.transform(w, h).context("the centre is outside of what the projection can show")?;
    let backup = dir.join("backups").join(format!("projection-{}", crate::editor::model::timestamp()));
    std::fs::create_dir_all(&backup).with_context(|| format!("cannot create {}", backup.display()))?;
    for f in geo::georef_sidecars(&bmp).into_iter().chain([json_path.clone()]) {
        let name = f.file_name().unwrap_or_default();
        std::fs::copy(&f, backup.join(name)).with_context(|| format!("cannot back up {}", f.display()))?;
    }
    p.write_georef(&bmp, w, h)?;
    p.set_header(&mut root, w, h);
    let tmp = json_path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&Value::Object(root))?).with_context(|| format!("cannot write {}", tmp.display()))?;
    std::fs::rename(&tmp, &json_path).with_context(|| format!("cannot write {}", json_path.display()))?;
    crate::bundle::touched();
    Ok(backup)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert!(check_size(5632, 2048).is_ok());
        assert!(check_size(4608, 2816).is_ok());
        assert!(check_size(5000, 2048).is_err());
        assert!(check_size(6144, 2304).is_err()); // 14.2 Mpx
        assert!(check_size(0, 256).is_err());
    }

    #[test]
    fn transforms_and_sizes() {
        let p = Projection::Geo { crs: Crs::LAEA_EUROPE, center: [11.35, 46.5], pixel: 35.1 };
        let gt = p.transform(4608, 2816).unwrap();
        let (cx, cy) = gt.apply(2304.0, 1408.0);
        let (lon, lat) = Crs::LAEA_EUROPE.to_lonlat(cx, cy).unwrap();
        assert!((lon - 11.35).abs() < 1e-7 && (lat - 46.5).abs() < 1e-7, "{lon} {lat}"); // ~1 cm (series truncation)
        assert_eq!(p.pixel_m(), 35.1);
        let ll = Projection::Geo { crs: Crs::new(4326), center: [0.0, 0.0], pixel: 360.0 / 5632.0 };
        assert!((ll.pixel_m() - 7115.9).abs() < 1.0, "{}", ll.pixel_m());
        let c = ll.corners(5632, 2048).unwrap();
        assert!((c[0].0 + 180.0).abs() < 1e-9 && (c[2].0 - 180.0).abs() < 1e-9);
        let plain = Projection::Plain { pixel_m: 7000.0 };
        assert_eq!(plain.crs(), Crs::UNKNOWN);
        assert_eq!(plain.transform(10, 10).unwrap().apply(1.0, 1.0), (7000.0, -7000.0));
        assert!(plain.corners(10, 10).is_none());
        assert_eq!(fmt_len(40_075_000.0), "40,075 km");
        assert_eq!(fmt_len(161_740.0), "161.7 km");
    }

    /// Every projection written next to a raster reads back as the same one.
    #[test]
    fn georef_round_trip() {
        let dir = std::env::temp_dir().join(format!("mh-proj-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bmp = dir.join("provinces.bmp");
        std::fs::write(&bmp, b"x").unwrap();
        // an old .bpw world file and .prj go when the new georeference is written
        std::fs::write(dir.join("provinces.bpw"), "1\n0\n0\n-1\n0.5\n-0.5\n").unwrap();
        std::fs::write(dir.join("provinces.prj"), "GEOGCS[\"WGS 84\"]").unwrap();
        let (w, h) = (4608, 2816);
        let cases = [
            Projection::Geo { crs: Crs::LAEA_EUROPE, center: [11.35, 46.5], pixel: 35.1 },
            Projection::Geo { crs: geo::utm_crs(11.35, 46.5, true), center: [11.35, 46.5], pixel: 50.0 },
            Projection::Geo { crs: geo::utm_crs(-70.0, -33.0, false), center: [-70.0, -33.0], pixel: 100.0 },
            Projection::Geo { crs: Crs::new(3857), center: [10.0, 50.0], pixel: 1000.0 },
            Projection::Geo { crs: Crs::new(4326), center: [10.0, 20.0], pixel: 0.01 },
            Projection::Plain { pixel_m: 7000.0 },
        ];
        for p in cases {
            p.write_georef(&bmp, w, h).unwrap();
            let mut g = FileGeoInfo::default();
            g.add_sidecars(&bmp);
            let back = Projection::of_map(&g, w, h, p.pixel_m());
            match (p, back) {
                (Projection::Geo { crs, center, pixel }, Projection::Geo { crs: c2, center: m2, pixel: p2 }) => {
                    assert_eq!(crs, c2);
                    assert!((center[0] - m2[0]).abs() < 1e-6 && (center[1] - m2[1]).abs() < 1e-6, "{center:?} {m2:?}");
                    assert!((pixel - p2).abs() < 1e-9);
                }
                (a, b) => assert_eq!(a, b),
            }
            let (gt, want) = (g.transform.unwrap().0.0, p.transform(w, h).unwrap().0);
            assert!(gt.iter().zip(want).all(|(a, b)| (a - b).abs() <= 1e-9 * b.abs().max(1.0)), "{gt:?} {want:?}");
            assert!(!dir.join("provinces.bpw").exists() && !dir.join("provinces.prj").exists());
        }
        // the plain raster has a world file and no CRS
        assert!(dir.join("provinces.wld").exists() && !dir.join("provinces.bmp.aux.xml").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
