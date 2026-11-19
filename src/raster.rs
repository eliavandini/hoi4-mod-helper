//! In-memory rasters: loading (BMP, GeoTIFF, PNG/JPEG), value inspection and
//! colourisation into display tiles.
//!
//! Pixel data is kept in its native compact form (e.g. 8-bit palette indices
//! for terrain/landcover maps) so later editing tools can work on the exact
//! values HOI4 cares about. Conversion to RGBA only happens per display tile.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};

use anyhow::{Context as _, Result, bail};
use eframe::egui::{Color32, ColorImage};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::geo::{Crs, FileGeoInfo, GeoTransform};
use crate::util::Progress;

pub enum Pixels {
    /// Palette indices plus an RGB palette.
    Indexed8 { data: Vec<u8>, palette: Vec<[u8; 3]> },
    Gray8(Vec<u8>),
    Gray16(Vec<u16>),
    GrayF32(Vec<f32>),
    /// Interleaved RGB.
    Rgb8(Vec<u8>),
    /// Interleaved RGBA (not premultiplied).
    Rgba8(Vec<u8>),
}

pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub pixels: Pixels,
    pub nodata: Option<f64>,
    /// Min/max of the valid values (single-band rasters only).
    pub data_range: Option<(f64, f64)>,
    /// Human-readable description of the source encoding.
    pub format: String,
}

#[derive(Clone, Copy, Debug)]
pub enum PixelValue {
    Index(u8, [u8; 3]),
    Gray(f64),
    Rgb([u8; 3]),
    Rgba([u8; 4]),
    NoData,
}

impl std::fmt::Display for PixelValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PixelValue::Index(i, [r, g, b]) => write!(f, "index {i}  (RGB {r} {g} {b})"),
            PixelValue::Gray(v) if v.fract() == 0.0 => write!(f, "{v}"),
            PixelValue::Gray(v) => write!(f, "{v:.3}"),
            PixelValue::Rgb([r, g, b]) => write!(f, "RGB {r} {g} {b}"),
            PixelValue::Rgba([r, g, b, a]) => write!(f, "RGBA {r} {g} {b} {a}"),
            PixelValue::NoData => write!(f, "no data"),
        }
    }
}

impl Raster {
    pub fn is_single_band(&self) -> bool {
        matches!(self.pixels, Pixels::Gray8(_) | Pixels::Gray16(_) | Pixels::GrayF32(_))
    }

    pub fn byte_size(&self) -> usize {
        match &self.pixels {
            Pixels::Indexed8 { data, .. } => data.len(),
            Pixels::Gray8(d) => d.len(),
            Pixels::Gray16(d) => d.len() * 2,
            Pixels::GrayF32(d) => d.len() * 4,
            Pixels::Rgb8(d) | Pixels::Rgba8(d) => d.len(),
        }
    }

    /// Value range used for display when the style does not override it.
    pub fn default_range(&self) -> (f64, f64) {
        match self.pixels {
            Pixels::Gray8(_) => (0.0, 255.0),
            _ => self.data_range.unwrap_or((0.0, 1.0)),
        }
    }

    pub fn value_at(&self, x: u32, y: u32) -> Option<PixelValue> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = y as usize * self.width as usize + x as usize;
        let gray = |v: f64| {
            if v.is_nan() || self.nodata == Some(v) { PixelValue::NoData } else { PixelValue::Gray(v) }
        };
        Some(match &self.pixels {
            Pixels::Indexed8 { data, palette } => {
                let idx = data[i];
                PixelValue::Index(idx, palette.get(idx as usize).copied().unwrap_or([0, 0, 0]))
            }
            Pixels::Gray8(d) => gray(d[i] as f64),
            Pixels::Gray16(d) => gray(d[i] as f64),
            Pixels::GrayF32(d) => gray(d[i] as f64),
            Pixels::Rgb8(d) => PixelValue::Rgb([d[3 * i], d[3 * i + 1], d[3 * i + 2]]),
            Pixels::Rgba8(d) => PixelValue::Rgba([d[4 * i], d[4 * i + 1], d[4 * i + 2], d[4 * i + 3]]),
        })
    }

    fn compute_range(&mut self) {
        let nodata = self.nodata;
        fn minmax<T: Copy + Send + Sync>(d: &[T], f: impl Fn(T) -> f64 + Sync, nodata: Option<f64>) -> Option<(f64, f64)> {
            let (lo, hi) = d
                .par_chunks(1 << 16)
                .map(|c| {
                    let mut lo = f64::INFINITY;
                    let mut hi = f64::NEG_INFINITY;
                    for &v in c {
                        let v = f(v);
                        if v.is_nan() || Some(v) == nodata {
                            continue;
                        }
                        lo = lo.min(v);
                        hi = hi.max(v);
                    }
                    (lo, hi)
                })
                .reduce(|| (f64::INFINITY, f64::NEG_INFINITY), |a, b| (a.0.min(b.0), a.1.max(b.1)));
            (lo <= hi).then_some((lo, hi))
        }
        self.data_range = match &self.pixels {
            Pixels::Gray8(d) => minmax(d, |v| v as f64, nodata),
            Pixels::Gray16(d) => minmax(d, |v| v as f64, nodata),
            Pixels::GrayF32(d) => minmax(d, |v| v as f64, nodata),
            _ => None,
        };
    }
}

// ---------------------------------------------------------------------------
// Display styling

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Colormap {
    #[default]
    Gray,
    Terrain,
    Rainbow,
}

impl Colormap {
    pub const ALL: [Colormap; 3] = [Colormap::Gray, Colormap::Terrain, Colormap::Rainbow];

    pub fn name(&self) -> &'static str {
        match self {
            Colormap::Gray => "Grayscale",
            Colormap::Terrain => "Terrain",
            Colormap::Rainbow => "Rainbow",
        }
    }

    fn stops(&self) -> &'static [(f32, [u8; 3])] {
        match self {
            Colormap::Gray => &[(0.0, [0, 0, 0]), (1.0, [255, 255, 255])],
            Colormap::Terrain => &[
                (0.0, [30, 85, 50]),
                (0.18, [95, 150, 75]),
                (0.38, [215, 205, 135]),
                (0.58, [165, 115, 65]),
                (0.8, [135, 125, 120]),
                (1.0, [255, 255, 255]),
            ],
            Colormap::Rainbow => &[
                (0.0, [48, 18, 59]),
                (0.2, [40, 120, 240]),
                (0.4, [30, 220, 160]),
                (0.6, [180, 240, 50]),
                (0.8, [250, 140, 30]),
                (1.0, [122, 4, 3]),
            ],
        }
    }

    pub fn lut(&self) -> [[u8; 4]; 256] {
        let stops = self.stops();
        let mut out = [[0u8; 4]; 256];
        for (i, o) in out.iter_mut().enumerate() {
            let t = i as f32 / 255.0;
            let k = stops.windows(2).position(|w| t <= w[1].0).unwrap_or(stops.len() - 2);
            let (t0, c0) = stops[k];
            let (t1, c1) = stops[k + 1];
            let f = ((t - t0) / (t1 - t0).max(1e-6)).clamp(0.0, 1.0);
            for c in 0..3 {
                o[c] = (c0[c] as f32 + (c1[c] as f32 - c0[c] as f32) * f).round() as u8;
            }
            o[3] = 255;
        }
        out
    }
}

/// Per-layer display settings for raster layers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct RasterStyle {
    /// Value stretch for single-band rasters (None = automatic).
    pub range: Option<[f64; 2]>,
    pub colormap: Colormap,
}

/// Number of samples per axis used when rendering minified tiles.
const SUPERSAMPLE: u32 = 4;

impl Raster {
    /// Render source pixels `[x0,x1) × [y0,y1)` at pyramid `level`
    /// (each output texel covers `2^level` source pixels per axis).
    pub fn render_tile(&self, style: &RasterStyle, level: u32, x0: u32, y0: u32, x1: u32, y1: u32) -> ColorImage {
        let s = 1u32 << level;
        let tw = (x1 - x0).div_ceil(s) as usize;
        let th = (y1 - y0).div_ceil(s) as usize;
        let mut out = vec![Color32::TRANSPARENT; tw * th];
        let region = Region { w: self.width as usize, s, x0, y0, x1, y1, tw, th };
        let (lo, hi) = style.range.map(|r| (r[0], r[1])).unwrap_or_else(|| self.default_range());
        let scale = if hi > lo { 255.0 / (hi - lo) } else { 0.0 };
        let cmap = style.colormap.lut();
        let nodata = self.nodata;
        let map_value = |v: f64| -> [u8; 4] {
            if v.is_nan() || Some(v) == nodata {
                [0, 0, 0, 0]
            } else {
                cmap[((v - lo) * scale).clamp(0.0, 255.0) as usize]
            }
        };
        match &self.pixels {
            Pixels::Indexed8 { data, palette } => {
                let mut lut = [[0u8, 0, 0, 255]; 256];
                for (l, p) in lut.iter_mut().zip(palette) {
                    *l = [p[0], p[1], p[2], 255];
                }
                region.resample(&mut out, |i| lut[data[i] as usize]);
            }
            Pixels::Gray8(d) => {
                let lut: Vec<[u8; 4]> = (0..256).map(|v| map_value(v as f64)).collect();
                region.resample(&mut out, |i| lut[d[i] as usize]);
            }
            Pixels::Gray16(d) => {
                let lut: Vec<[u8; 4]> = (0..65536).map(|v| map_value(v as f64)).collect();
                region.resample(&mut out, |i| lut[d[i] as usize]);
            }
            Pixels::GrayF32(d) => region.resample(&mut out, |i| map_value(d[i] as f64)),
            Pixels::Rgb8(d) => region.resample(&mut out, |i| [d[3 * i], d[3 * i + 1], d[3 * i + 2], 255]),
            Pixels::Rgba8(d) => region.resample(&mut out, |i| [d[4 * i], d[4 * i + 1], d[4 * i + 2], d[4 * i + 3]]),
        }
        ColorImage::new([tw, th], out)
    }
}

struct Region {
    w: usize,
    s: u32,
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
    tw: usize,
    th: usize,
}

impl Region {
    /// Sample positions for each output texel along one axis.
    fn sample_positions(&self, start: u32, end: u32, n_out: usize) -> (Vec<u32>, u32) {
        let k = self.s.min(SUPERSAMPLE);
        let mut pos = Vec::with_capacity(n_out * k as usize);
        for i in 0..n_out as u32 {
            let f0 = start + i * self.s;
            let f1 = (f0 + self.s).min(end);
            let span = f1 - f0;
            for m in 0..k {
                pos.push(f0 + ((2 * m + 1) * span) / (2 * k));
            }
        }
        (pos, k)
    }

    fn resample(&self, out: &mut [Color32], f: impl Fn(usize) -> [u8; 4]) {
        if self.s == 1 {
            for j in 0..self.th {
                let row = (self.y0 as usize + j) * self.w + self.x0 as usize;
                for i in 0..self.tw {
                    out[j * self.tw + i] = premul(f(row + i));
                }
            }
            return;
        }
        let (xs, k) = self.sample_positions(self.x0, self.x1, self.tw);
        let (ys, _) = self.sample_positions(self.y0, self.y1, self.th);
        let k = k as usize;
        let n = (k * k) as u32;
        for j in 0..self.th {
            for i in 0..self.tw {
                let (mut r, mut g, mut b, mut a) = (0u32, 0u32, 0u32, 0u32);
                for &sy in &ys[j * k..(j + 1) * k] {
                    let row = sy as usize * self.w;
                    for &sx in &xs[i * k..(i + 1) * k] {
                        let c = f(row + sx as usize);
                        let ca = c[3] as u32;
                        r += c[0] as u32 * ca;
                        g += c[1] as u32 * ca;
                        b += c[2] as u32 * ca;
                        a += ca;
                    }
                }
                let d = 255 * n;
                out[j * self.tw + i] =
                    Color32::from_rgba_premultiplied((r / d) as u8, (g / d) as u8, (b / d) as u8, (a / n) as u8);
            }
        }
    }
}

#[inline]
fn premul(c: [u8; 4]) -> Color32 {
    if c[3] == 255 {
        Color32::from_rgb(c[0], c[1], c[2])
    } else {
        let a = c[3] as u32;
        Color32::from_rgba_premultiplied(
            (c[0] as u32 * a / 255) as u8,
            (c[1] as u32 * a / 255) as u8,
            (c[2] as u32 * a / 255) as u8,
            c[3],
        )
    }
}

// ---------------------------------------------------------------------------
// Loading

/// Load a raster and its georeferencing from disk.
pub fn load(path: &Path, progress: &Progress) -> Result<(Raster, FileGeoInfo)> {
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let mut magic = [0u8; 4];
    File::open(path)
        .with_context(|| format!("cannot open {}", path.display()))?
        .read_exact(&mut magic)
        .context("file too short")?;
    let mut geo = FileGeoInfo::default();
    progress.set_stage("Reading");
    let mut raster = if &magic[..2] == b"BM" {
        load_bmp(path, progress)?
    } else if &magic == b"II*\0" || &magic == b"MM\0*" || &magic == b"II+\0" || &magic == b"MM\0+" {
        load_tiff(path, progress, &mut geo)?
    } else if matches!(ext.as_str(), "png" | "jpg" | "jpeg") {
        load_with_image_crate(path)?
    } else {
        bail!("unsupported file format (expected BMP, TIFF, PNG or JPEG)");
    };
    geo.add_sidecars(path);
    if raster.nodata.is_none() {
        raster.nodata = geo.nodata;
    }
    progress.set_stage("Computing statistics");
    raster.compute_range();
    Ok((raster, geo))
}

/// Width and height of a raster from its header (BMP, TIFF, PNG, JPEG),
/// without reading its pixels.
pub fn probe_size(path: &Path) -> Option<(u32, u32)> {
    let mut magic = [0u8; 4];
    File::open(path).ok()?.read_exact(&mut magic).ok()?;
    if &magic[..2] == b"BM" {
        let mut b = [0u8; 26];
        File::open(path).ok()?.read_exact(&mut b).ok()?;
        return Some((le_u32(&b, 18) as i32).unsigned_abs()).zip(Some((le_u32(&b, 22) as i32).unsigned_abs()));
    }
    if &magic == b"II*\0" || &magic == b"MM\0*" || &magic == b"II+\0" || &magic == b"MM\0+" {
        return open_tiff(path).ok()?.dimensions().ok();
    }
    image::image_dimensions(path).ok()
}

// --- BMP -------------------------------------------------------------------

fn le_u16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn le_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn load_bmp(path: &Path, progress: &Progress) -> Result<Raster> {
    let mut f = BufReader::new(File::open(path)?);
    let mut fh = [0u8; 14];
    f.read_exact(&mut fh)?;
    let data_offset = le_u32(&fh, 10) as u64;
    let mut sz = [0u8; 4];
    f.read_exact(&mut sz)?;
    let dib_size = u32::from_le_bytes(sz) as usize;
    if !(12..=1024).contains(&dib_size) {
        bail!("invalid BMP header size {dib_size}");
    }
    // shorter headers (OS/2 2.x: 16 to 64 bytes) leave the rest 0, which is
    // what the missing fields mean
    let mut dib = vec![0u8; dib_size.max(40)];
    dib[..4].copy_from_slice(&sz);
    f.read_exact(&mut dib[4..dib_size])?;

    let (width, height_signed, bpp, compression, clr_used, pal_entry) = if dib_size == 12 {
        (le_u16(&dib, 4) as i64, le_u16(&dib, 6) as i16 as i64, le_u16(&dib, 10), 0, 0, 3)
    } else {
        (
            le_u32(&dib, 4) as i32 as i64,
            le_u32(&dib, 8) as i32 as i64,
            le_u16(&dib, 14),
            le_u32(&dib, 16),
            le_u32(&dib, 32),
            4,
        )
    };
    let top_down = height_signed < 0;
    let (w, h) = (width.unsigned_abs() as usize, height_signed.unsigned_abs() as usize);
    if w == 0 || h == 0 {
        bail!("empty BMP");
    }
    // Channel masks: in the header for V2+ headers, or right after a 40-byte header.
    let mut masks = [0u32; 4];
    if compression == 3 || compression == 6 {
        if dib_size >= 52 {
            for (k, m) in masks.iter_mut().enumerate().take(if dib_size >= 56 { 4 } else { 3 }) {
                *m = le_u32(&dib, 40 + 4 * k);
            }
        } else {
            let n = if compression == 6 { 4 } else { 3 };
            let mut mb = vec![0u8; 4 * n];
            f.read_exact(&mut mb)?;
            for k in 0..n {
                masks[k] = le_u32(&mb, 4 * k);
            }
        }
    } else if bpp == 16 {
        masks = [0x7C00, 0x03E0, 0x001F, 0];
    } else if bpp == 32 {
        masks = [0x00FF_0000, 0x0000_FF00, 0x0000_00FF, 0];
        // V3+ headers may declare an alpha mask even for BI_RGB.
        if dib_size >= 56 {
            masks[3] = le_u32(&dib, 52);
        }
    }

    let mut palette = Vec::new();
    if bpp <= 8 {
        let n = if clr_used > 0 { (clr_used as usize).min(256) } else { 1 << bpp };
        let mut pb = vec![0u8; n * pal_entry];
        f.read_exact(&mut pb)?;
        palette = pb.chunks(pal_entry).map(|c| [c[2], c[1], c[0]]).collect();
    }

    f.seek(SeekFrom::Start(data_offset))?;
    let row_of = |r: usize| if top_down { r } else { h - 1 - r };
    progress.set(0.1);

    let format;
    let pixels = match (bpp, compression) {
        (1 | 4 | 8, 0) => {
            let stride = (bpp as usize * w).div_ceil(32) * 4;
            let mut raw = vec![0u8; stride * h];
            f.read_exact(&mut raw).context("BMP pixel data truncated")?;
            progress.set(0.6);
            let mut data = vec![0u8; w * h];
            data.par_chunks_mut(w).enumerate().for_each(|(y, out)| {
                let src = &raw[row_of(y) * stride..][..stride];
                match bpp {
                    8 => out.copy_from_slice(&src[..w]),
                    4 => out.iter_mut().enumerate().for_each(|(x, o)| *o = (src[x / 2] >> (4 * (1 - x % 2))) & 0xF),
                    _ => out.iter_mut().enumerate().for_each(|(x, o)| *o = (src[x / 8] >> (7 - x % 8)) & 1),
                }
            });
            let px = indexed_or_gray(data, palette);
            format = bmp_palette_format(&px, bpp, "");
            px
        }
        (8, 1) => {
            let mut raw = Vec::new();
            f.read_to_end(&mut raw)?;
            let data = decode_rle8(&raw, w, h, top_down);
            let px = indexed_or_gray(data, palette);
            format = bmp_palette_format(&px, 8, ", RLE");
            px
        }
        (24, 0) => {
            let stride = (3 * w).div_ceil(4) * 4;
            let mut raw = vec![0u8; stride * h];
            f.read_exact(&mut raw).context("BMP pixel data truncated")?;
            progress.set(0.6);
            let mut data = vec![0u8; w * h * 3];
            data.par_chunks_mut(w * 3).enumerate().for_each(|(y, out)| {
                let src = &raw[row_of(y) * stride..];
                for x in 0..w {
                    out[3 * x] = src[3 * x + 2];
                    out[3 * x + 1] = src[3 * x + 1];
                    out[3 * x + 2] = src[3 * x];
                }
            });
            format = "24-bit RGB BMP".into();
            Pixels::Rgb8(data)
        }
        (16 | 32, 0 | 3 | 6) => {
            let bytes = bpp as usize / 8;
            let stride = (bytes * w).div_ceil(4) * 4;
            let mut raw = vec![0u8; stride * h];
            f.read_exact(&mut raw).context("BMP pixel data truncated")?;
            progress.set(0.6);
            let has_alpha = masks[3] != 0;
            let ch = if has_alpha { 4 } else { 3 };
            let mut data = vec![0u8; w * h * ch];
            data.par_chunks_mut(w * ch).enumerate().for_each(|(y, out)| {
                let src = &raw[row_of(y) * stride..];
                for x in 0..w {
                    let v = if bytes == 4 { le_u32(src, 4 * x) } else { le_u16(src, 2 * x) as u32 };
                    for c in 0..ch {
                        out[ch * x + c] = extract_mask(v, masks[c]);
                    }
                }
            });
            format = format!("{bpp}-bit {} BMP", if has_alpha { "RGBA" } else { "RGB" });
            if has_alpha { Pixels::Rgba8(data) } else { Pixels::Rgb8(data) }
        }
        _ => bail!("unsupported BMP variant: {bpp} bits per pixel, compression {compression}"),
    };
    progress.set(1.0);
    Ok(Raster { width: w as u32, height: h as u32, pixels, nodata: None, data_range: None, format })
}

fn bmp_palette_format(px: &Pixels, bpp: u16, extra: &str) -> String {
    match px {
        Pixels::Indexed8 { palette, .. } => format!("{bpp}-bit indexed BMP{extra} ({} colours)", palette.len()),
        _ => format!("{bpp}-bit grayscale BMP{extra}"),
    }
}

fn extract_mask(v: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 255;
    }
    let shift = mask.trailing_zeros();
    let bits = (mask >> shift).count_ones();
    let x = (v & mask) >> shift;
    if bits >= 8 { (x >> (bits - 8)) as u8 } else { ((x * 255) / ((1 << bits) - 1)) as u8 }
}

/// An 8-bit image whose palette is the identity gray ramp is really grayscale
/// (e.g. HOI4 heightmaps); store it as such so value stretching works.
fn indexed_or_gray(data: Vec<u8>, palette: Vec<[u8; 3]>) -> Pixels {
    let used_max = data.par_iter().copied().max().unwrap_or(0) as usize;
    let is_ramp = palette.len() > used_max && palette.iter().enumerate().all(|(i, p)| p[0] as usize == i && p[1] == p[0] && p[2] == p[0]);
    if is_ramp { Pixels::Gray8(data) } else { Pixels::Indexed8 { data, palette } }
}

fn decode_rle8(raw: &[u8], w: usize, h: usize, top_down: bool) -> Vec<u8> {
    let mut out = vec![0u8; w * h];
    let (mut x, mut y, mut i) = (0usize, 0usize, 0usize);
    let mut put = |x: usize, y: usize, v: u8| {
        if x < w && y < h {
            let row = if top_down { y } else { h - 1 - y };
            out[row * w + x] = v;
        }
    };
    while i + 1 < raw.len() {
        let (n, v) = (raw[i] as usize, raw[i + 1]);
        i += 2;
        if n > 0 {
            for k in 0..n {
                put(x + k, y, v);
            }
            x += n;
        } else {
            match v {
                0 => {
                    x = 0;
                    y += 1;
                }
                1 => break,
                2 => {
                    if i + 1 >= raw.len() {
                        break;
                    }
                    x += raw[i] as usize;
                    y += raw[i + 1] as usize;
                    i += 2;
                }
                n => {
                    let n = n as usize;
                    for k in 0..n {
                        if let Some(&b) = raw.get(i + k) {
                            put(x + k, y, b);
                        }
                    }
                    x += n;
                    i += n + (n & 1);
                }
            }
        }
    }
    out
}

// --- TIFF ------------------------------------------------------------------

use tiff::decoder::{ChunkType, Decoder, DecodingResult, Limits};
use tiff::tags::Tag;
use tiff::ColorType;

type TiffDecoder = Decoder<BufReader<File>>;

fn open_tiff(path: &Path) -> Result<TiffDecoder> {
    Ok(Decoder::new(BufReader::with_capacity(1 << 20, File::open(path)?))?.with_limits(Limits::unlimited()))
}

/// Decode `n` chunks on a few threads (each with its own state from `init`,
/// e.g. a file handle) and hand the results to `sink` on the calling thread.
fn decode_parallel<S, D: Send>(
    n: u32,
    progress: &Progress,
    init: impl Fn() -> Result<S> + Sync,
    decode: impl Fn(&mut S, u32) -> Result<D> + Sync,
    mut sink: impl FnMut(u32, D) -> Result<()>,
) -> Result<()> {
    let next = AtomicU32::new(0);
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(8);
    std::thread::scope(|scope| -> Result<()> {
        let (tx, rx) = crossbeam_channel::bounded::<Result<(u32, D)>>(threads * 4);
        for _ in 0..threads {
            let (tx, next, init, decode) = (tx.clone(), &next, &init, &decode);
            scope.spawn(move || {
                let mut state = match init() {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = tx.send(Err(e));
                        return;
                    }
                };
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= n {
                        break;
                    }
                    let r = decode(&mut state, i).map(|d| (i, d)).with_context(|| format!("chunk {i}"));
                    if tx.send(r).is_err() {
                        break;
                    }
                }
            });
        }
        drop(tx);
        let mut done = 0u32;
        for msg in rx {
            let (i, d) = msg.context("TIFF decoding failed")?;
            sink(i, d)?;
            done += 1;
            if done % 64 == 0 {
                progress.set(done as f32 / n.max(1) as f32);
            }
        }
        Ok(())
    })
}

fn load_tiff(path: &Path, progress: &Progress, geo: &mut FileGeoInfo) -> Result<Raster> {
    let mut dec = open_tiff(path)?;
    let (w, h) = dec.dimensions()?;
    read_geotiff_tags(&mut dec, geo);
    let nodata = dec
        .find_tag(Tag::GdalNodata)
        .ok()
        .flatten()
        .and_then(|v| v.into_string().ok())
        .and_then(|s| s.trim_matches(|c: char| c == '\0' || c.is_whitespace()).parse::<f64>().ok());
    progress.set_stage("Decoding");

    // 8-bit images are decoded by our own chunk reader: the tiff crate
    // refuses palette images and chokes on some valid LZW tiles.
    if let Some(layout) = Raw8Layout::detect(&mut dec, w, h)? {
        drop(dec);
        let (pixels, format) = load_raw8_tiff(path, layout, w, h, progress)?;
        return Ok(Raster { width: w, height: h, pixels, nodata, data_range: None, format });
    }

    let ct = dec.colortype()?;
    let spp: usize = match ct {
        ColorType::Gray(_) => 1,
        ColorType::GrayA(_) => 2,
        ColorType::RGB(_) => 3,
        ColorType::RGBA(_) => 4,
        other => bail!("unsupported TIFF colour type {other:?}"),
    };
    let planar = dec.find_tag_unsigned::<u16>(Tag::PlanarConfiguration)?.unwrap_or(1);
    if planar == 2 && spp > 1 {
        bail!("band-separate (planar) multi-band TIFFs are not supported yet");
    }
    let n_chunks = match dec.get_chunk_type() {
        ChunkType::Strip => dec.strip_count()?,
        ChunkType::Tile => dec.tile_count()?,
    };
    let (cw, ch) = dec.chunk_dimensions();
    let across = w.div_ceil(cw);
    let origin = |i: u32| (((i % across) * cw) as usize, ((i / across) * ch) as usize);

    // The first chunk tells us the sample type.
    let first_dims = dec.chunk_data_dimensions(0);
    let first = dec.read_chunk(0)?;
    let mut asm = Assembler::new(&first, w as usize, h as usize, spp)?;
    asm.put(origin(0), first_dims, first)?;
    drop(dec);

    decode_parallel(
        n_chunks - 1,
        progress,
        || open_tiff(path),
        |dec, i| {
            let i = i + 1;
            Ok((dec.chunk_data_dimensions(i), dec.read_chunk(i)?))
        },
        |i, (dims, res)| asm.put(origin(i + 1), dims, res),
    )?;

    let (pixels, format) = asm.finish()?;
    Ok(Raster { width: w, height: h, pixels, nodata, data_range: None, format })
}

/// Layout of an 8-bit-per-sample, pixel-interleaved TIFF we can decode ourselves.
struct Raw8Layout {
    photometric: u16,
    spp: usize,
    compression: u16,
    predictor: u16,
    colormap: Option<Vec<u16>>,
    tiled: bool,
    offsets: Vec<u64>,
    counts: Vec<u64>,
    cw: u32,
    ch: u32,
}

impl Raw8Layout {
    /// `None` if the image needs the tiff crate (other bit depths, JPEG, YCbCr, planar…).
    fn detect(dec: &mut TiffDecoder, w: u32, h: u32) -> Result<Option<Self>> {
        let photometric = dec.find_tag_unsigned::<u16>(Tag::PhotometricInterpretation)?.unwrap_or(1);
        let spp = dec.find_tag_unsigned::<u16>(Tag::SamplesPerPixel)?.unwrap_or(1) as usize;
        let bps = dec.find_tag_unsigned_vec::<u16>(Tag::BitsPerSample)?.unwrap_or_else(|| vec![1]);
        let sample_format = dec.find_tag_unsigned_vec::<u16>(Tag::SampleFormat)?.unwrap_or_else(|| vec![1]);
        let planar = dec.find_tag_unsigned::<u16>(Tag::PlanarConfiguration)?.unwrap_or(1);
        let compression = dec.find_tag_unsigned::<u16>(Tag::Compression)?.unwrap_or(1);
        let predictor = dec.find_tag_unsigned::<u16>(Tag::Predictor)?.unwrap_or(1);
        let supported = matches!(photometric, 0..=3)
            && (1..=4).contains(&spp)
            && bps.iter().all(|&b| b == 8)
            && sample_format.iter().all(|&f| f == 1)
            && (planar == 1 || spp == 1)
            && matches!(compression, 1 | 5 | 8 | 32946 | 32773)
            && matches!(predictor, 1 | 2);
        if !supported {
            if photometric == 3 {
                bail!("palette TIFFs with {bps:?} bits × {spp} samples per pixel / compression {compression} are not supported");
            }
            return Ok(None);
        }
        let colormap = if photometric == 3 {
            Some(dec.find_tag_unsigned_vec::<u16>(Tag::ColorMap)?.context("palette TIFF without a ColorMap")?)
        } else {
            None
        };
        let tiled = dec.find_tag(Tag::TileOffsets)?.is_some();
        let (offsets, counts, cw, ch) = if tiled {
            (
                dec.find_tag_unsigned_vec::<u64>(Tag::TileOffsets)?.context("TileOffsets")?,
                dec.find_tag_unsigned_vec::<u64>(Tag::TileByteCounts)?.context("TileByteCounts")?,
                dec.find_tag_unsigned::<u32>(Tag::TileWidth)?.context("TileWidth")?,
                dec.find_tag_unsigned::<u32>(Tag::TileLength)?.context("TileLength")?,
            )
        } else {
            (
                dec.find_tag_unsigned_vec::<u64>(Tag::StripOffsets)?.context("StripOffsets")?,
                dec.find_tag_unsigned_vec::<u64>(Tag::StripByteCounts)?.context("StripByteCounts")?,
                w,
                dec.find_tag_unsigned::<u32>(Tag::RowsPerStrip)?.unwrap_or(h).clamp(1, h),
            )
        };
        if offsets.len() != counts.len() || cw == 0 || ch == 0 {
            bail!("inconsistent TIFF chunk tables");
        }
        Ok(Some(Raw8Layout { photometric, spp, compression, predictor, colormap, tiled, offsets, counts, cw, ch }))
    }
}

fn load_raw8_tiff(path: &Path, l: Raw8Layout, w: u32, h: u32, progress: &Progress) -> Result<(Pixels, String)> {
    let (wu, hu, spp) = (w as usize, h as usize, l.spp);
    let across = w.div_ceil(l.cw);
    // Chunk origin and size as stored (tiles are always full size, the last strip is cropped).
    let geom = |i: u32| {
        let (x0, y0) = ((i % across) * l.cw, (i / across) * l.ch);
        let stored = if l.tiled { (l.cw, l.ch) } else { (l.cw, l.ch.min(h.saturating_sub(y0))) };
        ((x0 as usize, y0 as usize), stored)
    };
    let mut out = vec![0u8; wu * hu * spp];
    decode_parallel(
        l.offsets.len() as u32,
        progress,
        || Ok(File::open(path)?),
        |file, i| {
            let (_, (sw, sh)) = geom(i);
            let expected = sw as usize * sh as usize * spp;
            let mut raw = vec![0u8; l.counts[i as usize] as usize];
            file.seek(SeekFrom::Start(l.offsets[i as usize]))?;
            file.read_exact(&mut raw)?;
            let mut data = match l.compression {
                1 => raw,
                8 | 32946 => {
                    let mut d = Vec::with_capacity(expected);
                    flate2::read::ZlibDecoder::new(&raw[..]).read_to_end(&mut d)?;
                    d
                }
                5 => weezl::decode::Decoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8).decode(&raw)?,
                _ => unpack_packbits(&raw, expected),
            };
            data.resize(expected, 0);
            if l.predictor == 2 {
                for row in data.chunks_mut(sw as usize * spp) {
                    for x in spp..row.len() {
                        row[x] = row[x].wrapping_add(row[x - spp]);
                    }
                }
            }
            Ok(data)
        },
        |i, data| {
            let ((x0, y0), (sw, sh)) = geom(i);
            let copy = ((sw as usize).min(wu.saturating_sub(x0)), (sh as usize).min(hu.saturating_sub(y0)));
            copy_rows(&mut out, wu, spp, (x0, y0), copy, &data, sw as usize);
            Ok(())
        },
    )?;
    let comp = match l.compression {
        1 => "",
        5 => ", LZW",
        32773 => ", PackBits",
        _ => ", Deflate",
    };
    Ok(match (spp, l.colormap) {
        (1, Some(cm)) => {
            let n = cm.len() / 3;
            let palette = (0..n.min(256)).map(|i| [(cm[i] >> 8) as u8, (cm[n + i] >> 8) as u8, (cm[2 * n + i] >> 8) as u8]).collect();
            (Pixels::Indexed8 { data: out, palette }, format!("8-bit palette TIFF{comp} ({n} colours)"))
        }
        (1, None) => {
            if l.photometric == 0 {
                out.par_iter_mut().for_each(|v| *v = 255 - *v); // WhiteIsZero
            }
            (Pixels::Gray8(out), format!("8-bit grayscale TIFF{comp}"))
        }
        (2, _) => (Pixels::Rgba8(out.chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect()), format!("8-bit gray+alpha TIFF{comp}")),
        (3, _) => (Pixels::Rgb8(out), format!("24-bit RGB TIFF{comp}")),
        _ => (Pixels::Rgba8(out), format!("32-bit RGBA TIFF{comp}")),
    })
}

fn unpack_packbits(src: &[u8], expected: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(expected);
    let mut i = 0;
    while i < src.len() && out.len() < expected {
        let n = src[i] as i8;
        i += 1;
        if n >= 0 {
            let len = n as usize + 1;
            out.extend_from_slice(&src[i..(i + len).min(src.len())]);
            i += len;
        } else if n != -128 {
            if let Some(&b) = src.get(i) {
                out.extend(std::iter::repeat_n(b, (1 - n as isize) as usize));
            }
            i += 1;
        }
    }
    out
}

fn read_geotiff_tags(dec: &mut TiffDecoder, geo: &mut FileGeoInfo) {
    let mut f64s = |tag: Tag| dec.find_tag(tag).ok().flatten().and_then(|v| v.into_f64_vec().ok());
    let transform = f64s(Tag::ModelTransformationTag);
    let tiepoint = f64s(Tag::ModelTiepointTag);
    let scale = f64s(Tag::ModelPixelScaleTag);
    let keys = dec.find_tag_unsigned_vec::<u16>(Tag::GeoKeyDirectoryTag).ok().flatten();

    let (mut model_type, mut raster_type, mut proj_code, mut geog_code) = (0u16, 1u16, None, None);
    if let Some(k) = keys.filter(|k| k.len() >= 4) {
        for e in 0..k[3] as usize {
            let b = 4 + 4 * e;
            if b + 3 >= k.len() {
                break;
            }
            let (id, loc, val) = (k[b], k[b + 1], k[b + 3]);
            if loc != 0 {
                continue;
            }
            let user_defined = val == 0 || val == 32767;
            match id {
                1024 => model_type = val,
                1025 => raster_type = val,
                2048 if !user_defined => geog_code = Some(val as u32),
                3072 if !user_defined => proj_code = Some(val as u32),
                _ => {}
            }
        }
    }
    let code = if model_type == 2 { geog_code } else { proj_code.or(geog_code) };
    if let Some(code) = code {
        geo.crs = Some((Crs::new(code), "GeoTIFF".into()));
    }

    let mut gt = if let Some(m) = transform.filter(|m| m.len() >= 16) {
        Some([m[3], m[0], m[1], m[7], m[4], m[5]])
    } else if let (Some(tp), Some(sc)) = (tiepoint, scale) {
        (tp.len() >= 6 && sc.len() >= 2).then(|| {
            let (i, j, x, y) = (tp[0], tp[1], tp[3], tp[4]);
            [x - i * sc[0], sc[0], 0.0, y + j * sc[1], 0.0, -sc[1]]
        })
    } else {
        None
    };
    if let Some(g) = gt.as_mut() {
        if raster_type == 2 {
            // PixelIsPoint: tie point refers to the pixel centre (GDAL shifts by half a pixel).
            g[0] -= 0.5 * g[1] + 0.5 * g[2];
            g[3] -= 0.5 * g[4] + 0.5 * g[5];
        }
        geo.transform = Some((GeoTransform(*g), "GeoTIFF".into()));
    }
}

/// Collects decoded TIFF chunks into one contiguous buffer.
enum Assembler {
    U8 { out: Vec<u8>, w: usize, spp: usize },
    U16 { out: Vec<u16>, w: usize, spp: usize },
    F32 { out: Vec<f32>, w: usize, src: &'static str },
}

fn copy_chunk<T: Copy>(out: &mut [T], out_w: usize, spp: usize, origin: (usize, usize), (dw, dh): (u32, u32), src: &[T]) {
    let (dw, dh) = (dw as usize, dh as usize);
    if dh == 0 {
        return;
    }
    // Chunks from the tiff crate are cropped to the image, so the stride is the data width.
    let src_stride = (src.len() / dh / spp).max(dw);
    copy_rows(out, out_w, spp, origin, (dw, dh), src, src_stride);
}

/// Copy a `w×h` block (stride `src_stride` pixels) into `out` at `(x0, y0)`.
fn copy_rows<T: Copy>(out: &mut [T], out_w: usize, spp: usize, (x0, y0): (usize, usize), (w, h): (usize, usize), src: &[T], src_stride: usize) {
    let row_len = w * spp;
    for r in 0..h {
        let dst = ((y0 + r) * out_w + x0) * spp;
        let s = r * src_stride * spp;
        if let (Some(d), Some(s)) = (out.get_mut(dst..dst + row_len), src.get(s..s + row_len)) {
            d.copy_from_slice(s);
        }
    }
}

impl Assembler {
    fn new(first: &DecodingResult, w: usize, h: usize, spp: usize) -> Result<Self> {
        let n = w * h * spp;
        Ok(match first {
            DecodingResult::U8(_) => Assembler::U8 { out: vec![0; n], w, spp },
            DecodingResult::U16(_) => Assembler::U16 { out: vec![0; n], w, spp },
            _ if spp == 1 => {
                let src = match first {
                    DecodingResult::F32(_) => "32-bit float",
                    DecodingResult::F64(_) => "64-bit float",
                    DecodingResult::F16(_) => "16-bit float",
                    DecodingResult::I8(_) => "8-bit signed",
                    DecodingResult::I16(_) => "16-bit signed",
                    DecodingResult::I32(_) => "32-bit signed",
                    DecodingResult::U32(_) => "32-bit unsigned",
                    _ => "64-bit integer",
                };
                Assembler::F32 { out: vec![0.0; n], w, src }
            }
            _ => bail!("unsupported sample type for a {spp}-band TIFF"),
        })
    }

    fn put(&mut self, origin: (usize, usize), dims: (u32, u32), res: DecodingResult) -> Result<()> {
        match (self, res) {
            (Assembler::U8 { out, w, spp }, DecodingResult::U8(v)) => copy_chunk(out, *w, *spp, origin, dims, &v),
            (Assembler::U16 { out, w, spp }, DecodingResult::U16(v)) => copy_chunk(out, *w, *spp, origin, dims, &v),
            (Assembler::F32 { out, w, .. }, r) => {
                let v: Vec<f32> = match r {
                    DecodingResult::F32(v) => v,
                    DecodingResult::F64(v) => v.into_iter().map(|x| x as f32).collect(),
                    DecodingResult::F16(v) => v.into_iter().map(|x| x.to_f32()).collect(),
                    DecodingResult::I8(v) => v.into_iter().map(|x| x as f32).collect(),
                    DecodingResult::I16(v) => v.into_iter().map(|x| x as f32).collect(),
                    DecodingResult::I32(v) => v.into_iter().map(|x| x as f32).collect(),
                    DecodingResult::U32(v) => v.into_iter().map(|x| x as f32).collect(),
                    DecodingResult::U64(v) => v.into_iter().map(|x| x as f32).collect(),
                    DecodingResult::I64(v) => v.into_iter().map(|x| x as f32).collect(),
                    _ => bail!("inconsistent sample types between TIFF chunks"),
                };
                copy_chunk(out, *w, 1, origin, dims, &v)
            }
            _ => bail!("inconsistent sample types between TIFF chunks"),
        }
        Ok(())
    }

    fn finish(self) -> Result<(Pixels, String)> {
        Ok(match self {
            Assembler::U8 { out, spp: 1, .. } => (Pixels::Gray8(out), "8-bit grayscale TIFF".into()),
            Assembler::U8 { out, spp: 2, .. } => {
                let rgba = out.chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect();
                (Pixels::Rgba8(rgba), "8-bit gray+alpha TIFF".into())
            }
            Assembler::U8 { out, spp: 3, .. } => (Pixels::Rgb8(out), "24-bit RGB TIFF".into()),
            Assembler::U8 { out, .. } => (Pixels::Rgba8(out), "32-bit RGBA TIFF".into()),
            Assembler::U16 { out, spp: 1, .. } => (Pixels::Gray16(out), "16-bit grayscale TIFF".into()),
            Assembler::U16 { out, spp, .. } => {
                let bytes: Vec<u8> = out.iter().map(|v| (v >> 8) as u8).collect();
                match spp {
                    2 => (Pixels::Rgba8(bytes.chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect()), "16-bit gray+alpha TIFF (shown as 8-bit)".into()),
                    3 => (Pixels::Rgb8(bytes), "48-bit RGB TIFF (shown as 8-bit)".into()),
                    _ => (Pixels::Rgba8(bytes), "64-bit RGBA TIFF (shown as 8-bit)".into()),
                }
            }
            Assembler::F32 { out, src, .. } => (Pixels::GrayF32(out), format!("{src} single-band TIFF")),
        })
    }
}

// --- PNG / JPEG ------------------------------------------------------------

fn load_with_image_crate(path: &Path) -> Result<Raster> {
    let mut reader = image::ImageReader::open(path)?.with_guessed_format()?;
    reader.no_limits();
    let img = reader.decode()?;
    let (w, h) = (img.width(), img.height());
    let (pixels, format) = match img {
        image::DynamicImage::ImageLuma8(b) => (Pixels::Gray8(b.into_raw()), "8-bit grayscale"),
        image::DynamicImage::ImageLuma16(b) => (Pixels::Gray16(b.into_raw()), "16-bit grayscale"),
        image::DynamicImage::ImageRgb8(b) => (Pixels::Rgb8(b.into_raw()), "24-bit RGB"),
        other => (Pixels::Rgba8(other.to_rgba8().into_raw()), "RGBA"),
    };
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_uppercase()).unwrap_or_default();
    Ok(Raster { width: w, height: h, pixels, nodata: None, data_range: None, format: format!("{format} {ext}") })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A BMP with the 16-byte OS/2 2.x header reads (it ran past the header).
    #[test]
    fn short_bmp_header() {
        let mut b = Vec::new();
        b.extend_from_slice(b"BM");
        b.extend_from_slice(&(14u32 + 16 + 4).to_le_bytes());
        b.extend_from_slice(&[0; 4]);
        b.extend_from_slice(&(14u32 + 16).to_le_bytes());
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u32.to_le_bytes()); // width
        b.extend_from_slice(&1u32.to_le_bytes()); // height
        b.extend_from_slice(&1u16.to_le_bytes()); // planes
        b.extend_from_slice(&24u16.to_le_bytes());
        b.extend_from_slice(&[30, 20, 10, 0]); // B G R + row padding
        let p = std::env::temp_dir().join(format!("mh-short-{}.bmp", std::process::id()));
        std::fs::write(&p, &b).unwrap();
        let r = load_bmp(&p, &Progress::default());
        let _ = std::fs::remove_file(&p);
        let r = r.unwrap();
        assert_eq!((r.width, r.height), (1, 1));
        let Pixels::Rgb8(px) = &r.pixels else { panic!("{}", r.format) };
        assert_eq!(px[..], [10, 20, 30]);
    }

    #[test]
    fn colormap_endpoints() {
        let lut = Colormap::Gray.lut();
        assert_eq!(lut[0], [0, 0, 0, 255]);
        assert_eq!(lut[255], [255, 255, 255, 255]);
    }

    #[test]
    fn tile_downsampling_averages() {
        let r = Raster {
            width: 4,
            height: 2,
            pixels: Pixels::Gray8(vec![0, 255, 0, 255, 0, 255, 0, 255]),
            nodata: None,
            data_range: None,
            format: String::new(),
        };
        let img = r.render_tile(&RasterStyle::default(), 1, 0, 0, 4, 2);
        assert_eq!(img.size, [2, 1]);
        assert!((img.pixels[0].r() as i32 - 127).abs() <= 1);
    }

    /// `landcover.bmp` was exported from `lc_section.tif` with gdal_translate,
    /// so the palette-TIFF and BMP decoders must agree on every pixel.
    #[test]
    #[ignore = "needs the mod's map files (MOD_HELPER_TEST_MAPS=<maps folder> cargo test -- --ignored)"]
    fn palette_tiff_matches_bmp_export() {
        let dir = crate::util::test_maps_dir().join("processed maps");
        let p = Progress::default();
        let (a, _) = load(&dir.join("lc_section.tif"), &p).unwrap();
        let (b, _) = load(&dir.join("landcover.bmp"), &p).unwrap();
        let (Pixels::Indexed8 { data: da, palette: pa }, Pixels::Indexed8 { data: db, palette: pb }) = (&a.pixels, &b.pixels) else {
            panic!("expected two indexed rasters");
        };
        assert_eq!((a.width, a.height), (b.width, b.height));
        assert!(da == db, "pixel data differs");
        assert_eq!(pa, pb, "palettes differ");
    }

    #[test]
    fn packbits() {
        // 3 literals, then a run of 4 × 0xAA
        assert_eq!(unpack_packbits(&[2, 1, 2, 3, (-3i8) as u8, 0xAA], 7), vec![1, 2, 3, 0xAA, 0xAA, 0xAA, 0xAA]);
    }

    #[test]
    fn mask_extraction() {
        assert_eq!(extract_mask(0x7C00, 0x7C00), 255);
        assert_eq!(extract_mask(0x00AB_0000, 0x00FF_0000), 0xAB);
    }
}


