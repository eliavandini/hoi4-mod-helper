//! Coordinate reference systems, projections and georeferencing metadata.
//!
//! Only the handful of projections relevant for the Alps region are implemented
//! (no PROJ dependency): geographic lon/lat, LAEA Europe (EPSG:3035), UTM and
//! Web Mercator. All use the GRS80 ellipsoid; the ETRS89/WGS84 difference
//! (< 1 m) is irrelevant at map-editing scales.

use std::f64::consts::{FRAC_PI_2, FRAC_PI_4};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

const A: f64 = 6_378_137.0;
const F: f64 = 1.0 / 298.257_222_101;

/// A coordinate reference system identified by its EPSG code (0 = unknown).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Crs {
    pub epsg: u32,
}

/// CRSs offered in the UI.
pub const KNOWN_CRS: &[u32] = &[3035, 4326, 4258, 25832, 25833, 32632, 32633, 3857];

enum Proj {
    Geographic,
    Laea,
    Utm { zone: u32, south: bool },
    WebMercator,
    Unsupported,
}

impl Crs {
    pub const LAEA_EUROPE: Crs = Crs { epsg: 3035 };
    pub const UNKNOWN: Crs = Crs { epsg: 0 };

    pub fn new(epsg: u32) -> Self {
        Crs { epsg }
    }

    fn proj(&self) -> Proj {
        match self.epsg {
            4326 | 4258 | 4269 | 4230 => Proj::Geographic,
            3035 => Proj::Laea,
            25801..=25860 => Proj::Utm { zone: self.epsg - 25800, south: false },
            32601..=32660 => Proj::Utm { zone: self.epsg - 32600, south: false },
            32701..=32760 => Proj::Utm { zone: self.epsg - 32700, south: true },
            3857 | 900913 | 3785 => Proj::WebMercator,
            _ => Proj::Unsupported,
        }
    }

    pub fn is_supported(&self) -> bool {
        !matches!(self.proj(), Proj::Unsupported)
    }

    pub fn is_geographic(&self) -> bool {
        matches!(self.proj(), Proj::Geographic)
    }

    pub fn name(&self) -> String {
        let desc = match self.epsg {
            0 => return "unknown".into(),
            4326 => "WGS 84 (lon/lat)",
            4258 => "ETRS89 (lon/lat)",
            3035 => "LAEA Europe",
            3857 => "Web Mercator",
            25801..=25860 => "ETRS89 / UTM",
            32601..=32660 => "WGS 84 / UTM N",
            32701..=32760 => "WGS 84 / UTM S",
            _ => "unsupported",
        };
        format!("EPSG:{} {}", self.epsg, desc)
    }

    /// Projected (or geographic) coordinates → (lon, lat) in degrees.
    pub fn to_lonlat(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        let r = match self.proj() {
            Proj::Geographic => Some((x, y)),
            Proj::Laea => LAEA_3035.inverse(x, y),
            Proj::Utm { zone, south } => Some(TM.inverse(zone, south, x, y)),
            Proj::WebMercator => {
                let lon = (x / A).to_degrees();
                let lat = (2.0 * (y / A).exp().atan() - FRAC_PI_2).to_degrees();
                Some((lon, lat))
            }
            Proj::Unsupported => None,
        }?;
        (r.0.is_finite() && r.1.is_finite()).then_some(r)
    }

    /// (lon, lat) in degrees → projected (or geographic) coordinates.
    pub fn from_lonlat(&self, lon: f64, lat: f64) -> Option<(f64, f64)> {
        let r = match self.proj() {
            Proj::Geographic => Some((lon, lat)),
            Proj::Laea => LAEA_3035.forward(lon, lat),
            Proj::Utm { zone, south } => Some(TM.forward(zone, south, lon, lat)),
            Proj::WebMercator => {
                let lat = lat.clamp(-85.06, 85.06).to_radians();
                Some((A * lon.to_radians(), A * (FRAC_PI_4 + lat / 2.0).tan().ln()))
            }
            Proj::Unsupported => None,
        }?;
        (r.0.is_finite() && r.1.is_finite()).then_some(r)
    }
}

/// Transform a point between two CRSs. If either CRS is unknown/unsupported
/// the point is passed through unchanged (the data is assumed to already be in
/// the target CRS).
pub fn transform(from: Crs, to: Crs, x: f64, y: f64) -> Option<(f64, f64)> {
    if from == to || !from.is_supported() || !to.is_supported() {
        return Some((x, y));
    }
    let (lon, lat) = from.to_lonlat(x, y)?;
    to.from_lonlat(lon, lat)
}

/// Whether points can be transformed `from` → `to` with an affine map
/// (identical CRSs, or one side unknown so no reprojection is done).
pub fn is_identity(from: Crs, to: Crs) -> bool {
    from == to || !from.is_supported() || !to.is_supported()
}

// ---------------------------------------------------------------------------
// Lambert azimuthal equal area, ellipsoidal oblique aspect (Snyder 1987, p.187)

struct Laea {
    e: f64,
    e2: f64,
    qp: f64,
    rq: f64,
    d: f64,
    sin_b1: f64,
    cos_b1: f64,
    lon0: f64,
    fe: f64,
    fn_: f64,
    apa: [f64; 3],
}

static LAEA_3035: LazyLock<Laea> = LazyLock::new(|| Laea::new(52.0, 10.0, 4_321_000.0, 3_210_000.0));

impl Laea {
    fn new(lat0: f64, lon0: f64, fe: f64, fn_: f64) -> Self {
        let e2 = F * (2.0 - F);
        let e = e2.sqrt();
        let q = |phi: f64| {
            let s = phi.sin();
            (1.0 - e2) * (s / (1.0 - e2 * s * s) - (1.0 / (2.0 * e)) * ((1.0 - e * s) / (1.0 + e * s)).ln())
        };
        let qp = q(FRAC_PI_2);
        let phi1 = lat0.to_radians();
        let b1 = (q(phi1) / qp).asin();
        let rq = A * (qp / 2.0).sqrt();
        let d = A * (phi1.cos() / (1.0 - e2 * phi1.sin().powi(2)).sqrt()) / (rq * b1.cos());
        let e4 = e2 * e2;
        let e6 = e4 * e2;
        let apa = [
            e2 / 3.0 + 31.0 * e4 / 180.0 + 517.0 * e6 / 5040.0,
            23.0 * e4 / 360.0 + 251.0 * e6 / 3780.0,
            761.0 * e6 / 45360.0,
        ];
        Laea { e, e2, qp, rq, d, sin_b1: b1.sin(), cos_b1: b1.cos(), lon0: lon0.to_radians(), fe, fn_, apa }
    }

    fn forward(&self, lon: f64, lat: f64) -> Option<(f64, f64)> {
        let phi = lat.to_radians();
        let s = phi.sin();
        let e = self.e;
        let q = (1.0 - self.e2) * (s / (1.0 - self.e2 * s * s) - (1.0 / (2.0 * e)) * ((1.0 - e * s) / (1.0 + e * s)).ln());
        let beta = (q / self.qp).clamp(-1.0, 1.0).asin();
        let dl = lon.to_radians() - self.lon0;
        let denom = 1.0 + self.sin_b1 * beta.sin() + self.cos_b1 * beta.cos() * dl.cos();
        if denom <= 1e-12 {
            return None; // antipode
        }
        let b = self.rq * (2.0 / denom).sqrt();
        let x = self.fe + b * self.d * beta.cos() * dl.sin();
        let y = self.fn_ + (b / self.d) * (self.cos_b1 * beta.sin() - self.sin_b1 * beta.cos() * dl.cos());
        Some((x, y))
    }

    fn inverse(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        let x = x - self.fe;
        let y = y - self.fn_;
        let rho = ((x / self.d).powi(2) + (self.d * y).powi(2)).sqrt();
        let (beta, lon) = if rho < 1e-9 {
            (self.sin_b1.asin(), self.lon0)
        } else {
            let arg = rho / (2.0 * self.rq);
            if arg > 1.0 {
                return None;
            }
            let c = 2.0 * arg.asin();
            let beta = (c.cos() * self.sin_b1 + self.d * y * c.sin() * self.cos_b1 / rho).clamp(-1.0, 1.0).asin();
            let lon = self.lon0
                + (x * c.sin())
                    .atan2(self.d * rho * self.cos_b1 * c.cos() - self.d * self.d * y * self.sin_b1 * c.sin());
            (beta, lon)
        };
        let phi = beta
            + self.apa[0] * (2.0 * beta).sin()
            + self.apa[1] * (4.0 * beta).sin()
            + self.apa[2] * (6.0 * beta).sin();
        Some((lon.to_degrees(), phi.to_degrees()))
    }
}

// ---------------------------------------------------------------------------
// Transverse Mercator (Krüger n-series, as used for UTM)

struct Tm {
    a_hat: f64,
    alpha: [f64; 3],
    beta: [f64; 3],
    delta: [f64; 3],
    n: f64,
}

static TM: LazyLock<Tm> = LazyLock::new(|| {
    let n = F / (2.0 - F);
    let (n2, n3) = (n * n, n * n * n);
    Tm {
        a_hat: A / (1.0 + n) * (1.0 + n2 / 4.0 + n2 * n2 / 64.0),
        alpha: [n / 2.0 - 2.0 * n2 / 3.0 + 5.0 * n3 / 16.0, 13.0 * n2 / 48.0 - 3.0 * n3 / 5.0, 61.0 * n3 / 240.0],
        beta: [n / 2.0 - 2.0 * n2 / 3.0 + 37.0 * n3 / 96.0, n2 / 48.0 + n3 / 15.0, 17.0 * n3 / 480.0],
        delta: [2.0 * n - 2.0 * n2 / 3.0 - 2.0 * n3, 7.0 * n2 / 3.0 - 8.0 * n3 / 5.0, 56.0 * n3 / 15.0],
        n,
    }
});

const UTM_K0: f64 = 0.9996;

impl Tm {
    fn forward(&self, zone: u32, south: bool, lon: f64, lat: f64) -> (f64, f64) {
        let lon0 = (zone as f64 * 6.0 - 183.0).to_radians();
        let phi = lat.to_radians();
        let dl = lon.to_radians() - lon0;
        let k = 2.0 * self.n.sqrt() / (1.0 + self.n);
        let t = (phi.sin().atanh() - k * (k * phi.sin()).atanh()).sinh();
        let xi_p = (t / dl.cos()).atan();
        let eta_p = (dl.sin() / (1.0 + t * t).sqrt()).atanh();
        let mut xi = xi_p;
        let mut eta = eta_p;
        for (j, a) in self.alpha.iter().enumerate() {
            let j2 = 2.0 * (j as f64 + 1.0);
            xi += a * (j2 * xi_p).sin() * (j2 * eta_p).cosh();
            eta += a * (j2 * xi_p).cos() * (j2 * eta_p).sinh();
        }
        let e = 500_000.0 + UTM_K0 * self.a_hat * eta;
        let n = if south { 10_000_000.0 } else { 0.0 } + UTM_K0 * self.a_hat * xi;
        (e, n)
    }

    fn inverse(&self, zone: u32, south: bool, e: f64, n: f64) -> (f64, f64) {
        let lon0 = (zone as f64 * 6.0 - 183.0).to_radians();
        let n0 = if south { 10_000_000.0 } else { 0.0 };
        let xi = (n - n0) / (UTM_K0 * self.a_hat);
        let eta = (e - 500_000.0) / (UTM_K0 * self.a_hat);
        let mut xi_p = xi;
        let mut eta_p = eta;
        for (j, b) in self.beta.iter().enumerate() {
            let j2 = 2.0 * (j as f64 + 1.0);
            xi_p -= b * (j2 * xi).sin() * (j2 * eta).cosh();
            eta_p -= b * (j2 * xi).cos() * (j2 * eta).sinh();
        }
        let chi = (xi_p.sin() / eta_p.cosh()).asin();
        let mut phi = chi;
        for (j, d) in self.delta.iter().enumerate() {
            phi += d * (2.0 * (j as f64 + 1.0) * chi).sin();
        }
        let lon = lon0 + (eta_p.sinh() / xi_p.cos()).atan();
        (lon.to_degrees(), phi.to_degrees())
    }
}

// ---------------------------------------------------------------------------
// Affine geotransform (GDAL convention)

/// GDAL-style geotransform: `X = gt[0] + col*gt[1] + row*gt[2]`,
/// `Y = gt[3] + col*gt[4] + row*gt[5]`, where (col,row) = (0,0) is the
/// top-left corner of the top-left pixel.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GeoTransform(pub [f64; 6]);

impl GeoTransform {
    pub fn identity_pixels() -> Self {
        GeoTransform([0.0, 1.0, 0.0, 0.0, 0.0, -1.0])
    }

    pub fn north_up(origin_x: f64, origin_y: f64, px: f64, py: f64) -> Self {
        GeoTransform([origin_x, px, 0.0, origin_y, 0.0, -py])
    }

    #[inline]
    pub fn apply(&self, col: f64, row: f64) -> (f64, f64) {
        let g = &self.0;
        (g[0] + col * g[1] + row * g[2], g[3] + col * g[4] + row * g[5])
    }

    pub fn inverse(&self) -> Option<GeoTransform> {
        let g = &self.0;
        let det = g[1] * g[5] - g[2] * g[4];
        if det.abs() < 1e-300 {
            return None;
        }
        Some(GeoTransform([
            (g[2] * g[3] - g[5] * g[0]) / det,
            g[5] / det,
            -g[2] / det,
            (g[4] * g[0] - g[1] * g[3]) / det,
            -g[4] / det,
            g[1] / det,
        ]))
    }

    /// Size of one pixel along x and y in CRS units.
    pub fn pixel_size(&self) -> (f64, f64) {
        let g = &self.0;
        (g[1].hypot(g[4]), g[2].hypot(g[5]))
    }
}

// ---------------------------------------------------------------------------
// Sidecar metadata discovery (.aux.xml, world files, .prj)

/// Georeferencing found in a file and/or its sidecars.
#[derive(Clone, Debug, Default)]
pub struct FileGeoInfo {
    pub transform: Option<(GeoTransform, String)>,
    pub crs: Option<(Crs, String)>,
    pub nodata: Option<f64>,
}

impl FileGeoInfo {
    /// Fill in anything still missing from sidecar files next to `path`.
    /// Internal metadata (e.g. GeoTIFF tags) should already be set; GDAL's PAM
    /// `.aux.xml` overrides it, world files and `.prj` only fill gaps.
    pub fn add_sidecars(&mut self, path: &Path) {
        if let Some(aux) = read_aux_xml(path) {
            if let Some(gt) = aux.transform {
                self.transform = Some((gt, "aux.xml".into()));
            }
            if let Some(crs) = aux.crs {
                self.crs = Some((crs, "aux.xml".into()));
            }
            if self.nodata.is_none() {
                self.nodata = aux.nodata;
            }
        }
        if self.transform.is_none() {
            if let Some((gt, p)) = read_world_file(path) {
                self.transform = Some((gt, format!("world file ({})", file_name(&p))));
            }
        }
        if self.crs.is_none() {
            let prj = path.with_extension("prj");
            if let Ok(wkt) = std::fs::read_to_string(&prj) {
                if let Some(crs) = crs_from_wkt(&wkt) {
                    self.crs = Some((crs, ".prj".into()));
                }
            }
        }
    }
}

pub fn file_name(p: &Path) -> String {
    p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())
}

struct AuxXml {
    transform: Option<GeoTransform>,
    crs: Option<Crs>,
    nodata: Option<f64>,
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

fn xml_element<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}");
    let start = xml.find(&open)?;
    let after = &xml[start + open.len()..];
    let gt = after.find('>')?;
    let body = &after[gt + 1..];
    let end = body.find(&format!("</{tag}>"))?;
    Some(&body[..end])
}

fn read_aux_xml(path: &Path) -> Option<AuxXml> {
    let xml = std::fs::read_to_string(sidecar(path, ".aux.xml")).ok()?;
    let transform = xml_element(&xml, "GeoTransform").and_then(|s| {
        let v: Vec<f64> = s.split(',').filter_map(|t| t.trim().parse().ok()).collect();
        (v.len() == 6).then(|| GeoTransform([v[0], v[1], v[2], v[3], v[4], v[5]]))
    });
    let crs = xml_element(&xml, "SRS").and_then(|s| crs_from_wkt(&unescape_xml(s)));
    let nodata = xml_element(&xml, "NoDataValue").and_then(|s| s.trim().parse().ok());
    Some(AuxXml { transform, crs, nodata })
}

fn unescape_xml(s: &str) -> String {
    s.replace("&quot;", "\"").replace("&apos;", "'").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

/// Best-effort EPSG detection from WKT1/WKT2 text.
pub fn crs_from_wkt(wkt: &str) -> Option<Crs> {
    // The outermost authority is the last one in the string.
    let candidates = [("AUTHORITY[\"EPSG\",\"", true), ("ID[\"EPSG\",", false)];
    let mut best: Option<(usize, u32)> = None;
    for (pat, _) in candidates {
        if let Some(pos) = wkt.rfind(pat) {
            let digits: String = wkt[pos + pat.len()..].chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(code) = digits.parse::<u32>() {
                if best.is_none_or(|(p, _)| pos > p) {
                    best = Some((pos, code));
                }
            }
        }
    }
    if let Some((_, code)) = best {
        return Some(Crs::new(code));
    }
    // ESRI-style WKT without authority codes.
    let lower = wkt.to_ascii_lowercase();
    if lower.contains("lambert_azimuthal_equal_area") && lower.contains("52") && lower.contains("10") {
        return Some(Crs::LAEA_EUROPE);
    }
    if let Some(pos) = lower.find("utm_zone_").or_else(|| lower.find("utm zone ")) {
        let rest = &lower[pos + 9..];
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        let zone: u32 = digits.parse().ok()?;
        let south = rest[digits.len()..].starts_with('s');
        return Some(Crs::new(if south { 32700 } else { 32600 } + zone));
    }
    if lower.starts_with("geogcs") && (lower.contains("wgs_1984") || lower.contains("wgs 84")) {
        return Some(Crs::new(4326));
    }
    None
}

/// Look for a world file (`.wld`, `.tfw`, `.bpw`, …) next to `path`.
fn read_world_file(path: &Path) -> Option<(GeoTransform, PathBuf)> {
    let ext = path.extension()?.to_string_lossy().to_ascii_lowercase();
    let mut exts = Vec::new();
    if ext.len() >= 2 {
        let b = ext.as_bytes();
        exts.push(format!("{}{}w", b[0] as char, b[ext.len() - 1] as char));
    }
    exts.push(format!("{ext}w"));
    exts.push("wld".into());
    let mut candidates: Vec<PathBuf> = Vec::new();
    for e in &exts {
        candidates.push(path.with_extension(e));
        candidates.push(path.with_extension(e.to_ascii_uppercase()));
    }
    candidates.push(sidecar(path, ".wld"));
    for c in candidates {
        let Ok(text) = std::fs::read_to_string(&c) else { continue };
        let v: Vec<f64> = text.split_whitespace().filter_map(|t| t.parse().ok()).collect();
        if v.len() < 6 {
            continue;
        }
        // A D B E C F, where (C, F) is the *centre* of the upper-left pixel.
        let (a, d, b, e, c0, f0) = (v[0], v[1], v[2], v[3], v[4], v[5]);
        let gt = GeoTransform([c0 - 0.5 * a - 0.5 * b, a, b, f0 - 0.5 * d - 0.5 * e, d, e]);
        return Some((gt, c));
    }
    None
}

// ---------------------------------------------------------------------------
// Writing georeferences (world file + .aux.xml), as GDAL does for a BMP

const GRS80: &str = r#"SPHEROID["GRS 1980",6378137,298.257222101,AUTHORITY["EPSG","7019"]]"#;
const WGS84_SPHEROID: &str = r#"SPHEROID["WGS 84",6378137,298.257223563,AUTHORITY["EPSG","7030"]]"#;
const GREENWICH: &str = r#"PRIMEM["Greenwich",0,AUTHORITY["EPSG","8901"]]"#;
const DEGREE: &str = r#"UNIT["degree",0.0174532925199433,AUTHORITY["EPSG","9122"]]"#;
const METRE: &str = r#"UNIT["metre",1,AUTHORITY["EPSG","9001"]]"#;

fn geogcs_wgs84() -> String {
    format!(r#"GEOGCS["WGS 84",DATUM["WGS_1984",{WGS84_SPHEROID},AUTHORITY["EPSG","6326"]],{GREENWICH},{DEGREE},AUTHORITY["EPSG","4326"]]"#)
}

fn geogcs_etrs89() -> String {
    format!(r#"GEOGCS["ETRS89",DATUM["European_Terrestrial_Reference_System_1989",{GRS80},AUTHORITY["EPSG","6258"]],{GREENWICH},{DEGREE},AUTHORITY["EPSG","4258"]]"#)
}

/// WKT1 (as GDAL writes it) of the CRSs this program can project; None for
/// the others.
pub fn wkt(crs: Crs) -> Option<String> {
    let s = match crs.proj() {
        Proj::Geographic => match crs.epsg {
            4326 => geogcs_wgs84(),
            4258 => geogcs_etrs89(),
            _ => return None,
        },
        Proj::Laea => format!(
            r#"PROJCS["ETRS89-extended / LAEA Europe",{},PROJECTION["Lambert_Azimuthal_Equal_Area"],PARAMETER["latitude_of_center",52],PARAMETER["longitude_of_center",10],PARAMETER["false_easting",4321000],PARAMETER["false_northing",3210000],{METRE},AXIS["Northing",NORTH],AXIS["Easting",EAST],AUTHORITY["EPSG","3035"]]"#,
            geogcs_etrs89()
        ),
        Proj::Utm { zone, south } => {
            let etrs = (25801..=25860).contains(&crs.epsg);
            let (name, geog) = if etrs {
                (format!("ETRS89 / UTM zone {zone}N"), geogcs_etrs89())
            } else {
                (format!("WGS 84 / UTM zone {zone}{}", if south { 'S' } else { 'N' }), geogcs_wgs84())
            };
            format!(
                r#"PROJCS["{name}",{geog},PROJECTION["Transverse_Mercator"],PARAMETER["latitude_of_origin",0],PARAMETER["central_meridian",{}],PARAMETER["scale_factor",0.9996],PARAMETER["false_easting",500000],PARAMETER["false_northing",{}],{METRE},AXIS["Easting",EAST],AXIS["Northing",NORTH],AUTHORITY["EPSG","{}"]]"#,
                zone as i32 * 6 - 183,
                if south { 10_000_000 } else { 0 },
                crs.epsg
            )
        }
        Proj::WebMercator => format!(
            r#"PROJCS["WGS 84 / Pseudo-Mercator",{},PROJECTION["Mercator_1SP"],PARAMETER["central_meridian",0],PARAMETER["scale_factor",1],PARAMETER["false_easting",0],PARAMETER["false_northing",0],{METRE},AXIS["Easting",EAST],AXIS["Northing",NORTH],EXTENSION["PROJ4","+proj=merc +a=6378137 +b=6378137 +lat_ts=0 +lon_0=0 +x_0=0 +y_0=0 +k=1 +units=m +nadgrids=@null +wktext +no_defs"],AUTHORITY["EPSG","3857"]]"#,
            geogcs_wgs84()
        ),
        Proj::Unsupported => return None,
    };
    Some(s)
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// The world file a raster gets: `<stem>.wld`.
pub fn world_file_path(raster: &Path) -> PathBuf {
    raster.with_extension("wld")
}

/// The sidecars of `raster` that hold a georeference and exist: what
/// [`FileGeoInfo::add_sidecars`] reads.
pub fn georef_sidecars(raster: &Path) -> Vec<PathBuf> {
    let mut v = vec![sidecar(raster, ".aux.xml"), raster.with_extension("prj")];
    let ext = raster.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let mut exts = vec!["wld".to_string(), format!("{ext}w")];
    if ext.len() >= 2 {
        let b = ext.as_bytes();
        exts.push(format!("{}{}w", b[0] as char, b[ext.len() - 1] as char));
    }
    for e in exts {
        v.push(raster.with_extension(&e));
        v.push(raster.with_extension(e.to_ascii_uppercase()));
    }
    v.push(sidecar(raster, ".wld"));
    v.sort();
    v.dedup();
    v.retain(|p| p.is_file());
    v
}

/// Give `raster` this georeference: the old sidecars go, then a world file
/// (`<stem>.wld`) and, with a CRS, an `.aux.xml` naming it. Without a
/// transform the raster is left without any.
pub fn write_georef(raster: &Path, georef: Option<(GeoTransform, Option<Crs>)>) -> anyhow::Result<()> {
    use anyhow::Context as _;
    for p in georef_sidecars(raster) {
        std::fs::remove_file(&p).with_context(|| format!("cannot remove {}", p.display()))?;
    }
    let Some((gt, crs)) = georef else { return Ok(()) };
    let g = gt.0;
    // the world file names the centre of the top-left pixel
    let (cx, cy) = (g[0] + 0.5 * g[1] + 0.5 * g[2], g[3] + 0.5 * g[4] + 0.5 * g[5]);
    // (the shortest text that reads back as the same number)
    let wld = format!("{}\n{}\n{}\n{}\n{}\n{}\n", g[1], g[4], g[2], g[5], cx, cy);
    let path = world_file_path(raster);
    std::fs::write(&path, wld).with_context(|| format!("cannot write {}", path.display()))?;
    if let Some(w) = crs.and_then(wkt) {
        let axis = if crs == Some(Crs::LAEA_EUROPE) || crs.is_some_and(|c| c.is_geographic()) { " dataAxisToSRSAxisMapping=\"2,1\"" } else { " dataAxisToSRSAxisMapping=\"1,2\"" };
        let xml = format!("<PAMDataset>\n  <SRS{axis}>{}</SRS>\n  <Metadata>\n    <MDI key=\"AREA_OR_POINT\">Area</MDI>\n  </Metadata>\n</PAMDataset>\n", escape_xml(&w));
        let path = sidecar(raster, ".aux.xml");
        std::fs::write(&path, xml).with_context(|| format!("cannot write {}", path.display()))?;
    }
    Ok(())
}

/// UTM zone (1..=60) of a longitude.
pub fn utm_zone(lon: f64) -> u32 {
    (((lon + 180.0) / 6.0).floor() as i64).rem_euclid(60) as u32 + 1
}

/// The UTM CRS for a place: ETRS89 (EPSG:258zz) or WGS 84 (EPSG:326zz /
/// 327zz south of the equator).
pub fn utm_crs(lon: f64, lat: f64, etrs89: bool) -> Crs {
    let z = utm_zone(lon);
    match (etrs89 && lat >= 0.0, lat < 0.0) {
        (true, _) => Crs::new(25800 + z),
        (false, true) => Crs::new(32700 + z),
        (false, false) => Crs::new(32600 + z),
    }
}

/// Metres per degree of latitude (mean).
pub const M_PER_DEG: f64 = 111_320.0;

/// Format a lon/lat pair for display.
pub fn fmt_lonlat(lon: f64, lat: f64) -> String {
    format!(
        "{:.5}°{} {:.5}°{}",
        lat.abs(),
        if lat >= 0.0 { 'N' } else { 'S' },
        lon.abs(),
        if lon >= 0.0 { 'E' } else { 'W' }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // Reference values from GDAL 3.13 `gdaltransform` (lon 11.35°, lat 46.5°).
    #[test]
    fn laea_matches_gdal() {
        let c = Crs::LAEA_EUROPE;
        let (x, y) = c.from_lonlat(11.35, 46.5).unwrap();
        assert!((x - 4_424_731.603).abs() < 0.01 && (y - 2_599_465.792).abs() < 0.01, "{x} {y}");
        let (lon, lat) = c.to_lonlat(x, y).unwrap();
        assert!((lon - 11.35).abs() < 1e-7 && (lat - 46.5).abs() < 1e-7, "{lon} {lat}"); // ~1 cm (series truncation)
        // Top-left corner of the processed maps.
        let (lon, lat) = c.to_lonlat(4_351_320.0, 2_692_980.0).unwrap();
        assert!((lon - 10.400_986_5).abs() < 1e-6 && (lat - 47.349_679_6).abs() < 1e-6, "{lon} {lat}");
    }

    #[test]
    fn utm_matches_gdal() {
        let c = Crs::new(25832);
        let (x, y) = c.from_lonlat(11.35, 46.5).unwrap();
        assert!((x - 680_317.354).abs() < 0.01 && (y - 5_152_286.445).abs() < 0.01, "{x} {y}");
        let (lon, lat) = c.to_lonlat(x, y).unwrap();
        assert!((lon - 11.35).abs() < 1e-7 && (lat - 46.5).abs() < 1e-7, "{lon} {lat}"); // ~1 cm (series truncation)
    }

    #[test]
    fn geotransform_inverse() {
        let gt = GeoTransform([4351320.0, 45.0, 0.0, 2692980.0, 0.0, -45.0]);
        let inv = gt.inverse().unwrap();
        let (x, y) = gt.apply(123.5, 456.25);
        let (c, r) = inv.apply(x, y);
        assert!((c - 123.5).abs() < 1e-9 && (r - 456.25).abs() < 1e-9);
    }

    #[test]
    fn wkt_epsg() {
        let wkt = r#"PROJCS["ETRS89-extended / LAEA Europe",GEOGCS["ETRS89",AUTHORITY["EPSG","4258"]],AUTHORITY["EPSG","3035"]]"#;
        assert_eq!(crs_from_wkt(wkt), Some(Crs::new(3035)));
        let wkt2 = r#"PROJCRS["x",BASEGEOGCRS["y",ID["EPSG",4258]],ID["EPSG",25832]]"#;
        assert_eq!(crs_from_wkt(wkt2), Some(Crs::new(25832)));
    }
}
