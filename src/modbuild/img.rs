//! Image helpers for the mod build: BMP, DDS and TGA files as HOI4 (and
//! Pillow, which the Python version used) write them, resampling as Pillow
//! does it, and the exact Euclidean distance transform.

use std::path::Path;

use anyhow::{Context as _, Result, bail};
use rayon::prelude::*;

fn u16le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn i32le(b: &[u8], o: usize) -> i32 {
    u32le(b, o) as i32
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).with_context(|| format!("cannot create {}", d.display()))?;
    }
    std::fs::write(path, bytes).with_context(|| format!("cannot write {}", path.display()))
}

// ------------------------------------------------------------------ BMP

/// Size and encoding of a BMP file.
pub struct BmpHeader {
    pub w: u32,
    pub h: u32,
    pub bpp: u16,
    pub compression: u32,
}

pub fn bmp_header(path: &Path) -> Result<BmpHeader> {
    use std::io::Read as _;
    let mut b = [0u8; 54];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut b))
        .with_context(|| format!("cannot read {}", path.display()))?;
    if &b[0..2] != b"BM" {
        bail!("{} is not a BMP file", path.display());
    }
    Ok(BmpHeader { w: i32le(&b, 18).unsigned_abs(), h: i32le(&b, 22).unsigned_abs(), bpp: u16le(&b, 28), compression: u32le(&b, 30) })
}

/// An uncompressed 8-bit indexed or 24-bit bitmap, rows top-down.
pub struct Bmp {
    pub w: u32,
    pub h: u32,
    /// 8: palette indices; 24: RGB triples.
    pub bpp: u16,
    pub data: Vec<u8>,
    pub palette: Vec<[u8; 3]>,
}

pub fn read_bmp(path: &Path) -> Result<Bmp> {
    let b = std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    parse_bmp(&b, &path.display().to_string())
}

/// A BMP file's contents, as [`read_bmp`] reads it.
#[cfg(test)]
pub fn read_bmp_bytes(b: &[u8]) -> Result<Bmp> {
    parse_bmp(b, "the bitmap")
}

fn parse_bmp(b: &[u8], name: &str) -> Result<Bmp> {
    if b.len() < 54 || &b[0..2] != b"BM" {
        bail!("{name} is not a BMP file");
    }
    let off = u32le(b, 10) as usize;
    let hdr = u32le(b, 14) as usize;
    let w = i32le(b, 18).unsigned_abs() as usize;
    let h_raw = i32le(b, 22);
    let h = h_raw.unsigned_abs() as usize;
    let bpp = u16le(b, 28);
    if u32le(b, 30) != 0 {
        bail!("{}: compressed bitmaps are not supported", name);
    }
    if bpp != 8 && bpp != 24 {
        bail!("{}: {bpp}-bit bitmaps are not supported", name);
    }
    let palette: Vec<[u8; 3]> = if bpp == 8 {
        let n = match u32le(b, 46) {
            0 => 256,
            k => k as usize,
        };
        let p0 = 14 + hdr;
        if b.len() < p0 + 4 * n {
            bail!("{}: truncated palette", name);
        }
        (0..n).map(|i| [b[p0 + 4 * i + 2], b[p0 + 4 * i + 1], b[p0 + 4 * i]]).collect()
    } else {
        Vec::new()
    };
    let bb = bpp as usize / 8;
    let stride = (w * bpp as usize).div_ceil(32) * 4;
    if b.len() < off + stride * h {
        bail!("{}: truncated pixel data", name);
    }
    let mut data = vec![0u8; w * h * bb];
    data.par_chunks_mut(w * bb).enumerate().for_each(|(y, d)| {
        let src = if h_raw < 0 { y } else { h - 1 - y };
        let s = &b[off + src * stride..off + src * stride + w * bb];
        if bpp == 24 {
            for x in 0..w {
                d[3 * x] = s[3 * x + 2];
                d[3 * x + 1] = s[3 * x + 1];
                d[3 * x + 2] = s[3 * x];
            }
        } else {
            d.copy_from_slice(s);
        }
    });
    Ok(Bmp { w: w as u32, h: h as u32, bpp, data, palette })
}

fn bmp_bytes(w: u32, h: u32, bpp: u16, palette: &[[u8; 3]], row: impl Fn(usize, &mut [u8]) + Sync) -> Vec<u8> {
    let (wu, hu) = (w as usize, h as usize);
    let stride = (wu * bpp as usize).div_ceil(32) * 4;
    let off = 14 + 40 + 4 * palette.len();
    let size = off + stride * hu;
    let mut b = Vec::with_capacity(size);
    b.extend_from_slice(b"BM");
    b.extend_from_slice(&(size as u32).to_le_bytes());
    b.extend_from_slice(&0u32.to_le_bytes());
    b.extend_from_slice(&(off as u32).to_le_bytes());
    b.extend_from_slice(&40u32.to_le_bytes());
    b.extend_from_slice(&(w as i32).to_le_bytes());
    b.extend_from_slice(&(h as i32).to_le_bytes()); // bottom-up
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&bpp.to_le_bytes());
    b.extend_from_slice(&0u32.to_le_bytes());
    b.extend_from_slice(&((stride * hu) as u32).to_le_bytes());
    b.extend_from_slice(&3780i32.to_le_bytes()); // 96 dpi, as Pillow writes
    b.extend_from_slice(&3780i32.to_le_bytes());
    b.extend_from_slice(&(palette.len() as u32).to_le_bytes());
    b.extend_from_slice(&(palette.len() as u32).to_le_bytes());
    for c in palette {
        b.extend_from_slice(&[c[2], c[1], c[0], 0]);
    }
    b.resize(size, 0);
    b[off..].par_chunks_mut(stride).enumerate().for_each(|(i, out)| row(hu - 1 - i, out));
    b
}

/// 8-bit bitmap: `data` indexes `palette` (rows top-down).
pub fn write_bmp8(path: &Path, w: u32, h: u32, data: &[u8], palette: &[[u8; 3]]) -> Result<()> {
    let wu = w as usize;
    let bytes = bmp_bytes(w, h, 8, palette, |y, out| out[..wu].copy_from_slice(&data[y * wu..(y + 1) * wu]));
    write_file(path, &bytes)
}

/// 8-bit greyscale bitmap (grey palette), as Pillow writes mode "L".
pub fn write_bmp_gray(path: &Path, w: u32, h: u32, data: &[u8]) -> Result<()> {
    let grey: Vec<[u8; 3]> = (0..=255u8).map(|i| [i, i, i]).collect();
    write_bmp8(path, w, h, data, &grey)
}

/// 24-bit bitmap from RGB triples (rows top-down).
pub fn write_bmp24(path: &Path, w: u32, h: u32, rgb: &[u8]) -> Result<()> {
    let wu = w as usize;
    let bytes = bmp_bytes(w, h, 24, &[], |y, out| {
        let s = &rgb[y * wu * 3..(y + 1) * wu * 3];
        for x in 0..wu {
            out[3 * x] = s[3 * x + 2];
            out[3 * x + 1] = s[3 * x + 1];
            out[3 * x + 2] = s[3 * x];
        }
    });
    write_file(path, &bytes)
}

// ------------------------------------------------------------------ DDS

/// Uncompressed 32-bit DDS (B8G8R8A8, no mipmaps) from RGBA pixels.
pub fn write_dds_rgba(path: &Path, w: u32, h: u32, rgba: &[u8]) -> Result<()> {
    write_file(path, &dds_rgba_bytes(w, h, rgba))
}

/// The bytes of an uncompressed 32-bit DDS (B8G8R8A8, no mipmaps) from RGBA pixels.
pub fn dds_rgba_bytes(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    let mut b = Vec::with_capacity(128 + rgba.len());
    b.extend_from_slice(b"DDS ");
    for v in [124u32, 0x100f, h, w, w * 4, 0, 0] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b.resize(b.len() + 44, 0); // reserved
    for v in [32u32, 0x41, 0, 32, 0x00ff_0000, 0x0000_ff00, 0x0000_00ff, 0xff00_0000, 0x1000, 0, 0, 0, 0] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    debug_assert_eq!(b.len(), 128);
    b.resize(128 + rgba.len(), 0);
    b[128..].par_chunks_mut(4).zip(rgba.par_chunks(4)).for_each(|(o, i)| {
        o.copy_from_slice(&[i[2], i[1], i[0], i[3]]);
    });
    b
}

/// Size of a DDS file and whether it is uncompressed 32-bit with alpha.
pub fn dds_info(path: &Path) -> Result<(u32, u32, &'static str)> {
    use std::io::Read as _;
    let mut b = [0u8; 128];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut b))
        .with_context(|| format!("cannot read {}", path.display()))?;
    if &b[0..4] != b"DDS " {
        bail!("{} is not a DDS file", path.display());
    }
    let pf = u32le(&b, 80);
    let mode = if pf & 0x4 != 0 {
        match &b[84..88] {
            b"DXT1" => "DXT1",
            b"DXT3" => "DXT3",
            b"DXT5" => "DXT5",
            _ => "compressed",
        }
    } else if pf & 0x41 == 0x41 && u32le(&b, 88) == 32 {
        "RGBA"
    } else {
        "RGB"
    };
    Ok((u32le(&b, 16), u32le(&b, 12), mode))
}

/// The top level of a DDS file as RGBA: uncompressed 24/32-bit, DXT1, DXT3, DXT5.
pub fn read_dds(path: &Path) -> Result<(u32, u32, Vec<u8>)> {
    let b = std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    read_dds_bytes(&b, &path.display().to_string())
}

/// [`read_dds`] of a file's bytes (`name` for messages). DX10 headers with
/// B8G8R8A8, R8G8B8A8 and BC1-3 are read too.
pub fn read_dds_bytes(b: &[u8], name: &str) -> Result<(u32, u32, Vec<u8>)> {
    if b.len() < 128 || &b[0..4] != b"DDS " {
        bail!("{name} is not a DDS file");
    }
    let (h, w) = (u32le(b, 12) as usize, u32le(b, 16) as usize);
    if w == 0 || h == 0 || w > 16384 || h > 16384 {
        bail!("{name}: DDS of {w}x{h}");
    }
    let pf = u32le(b, 80);
    let mut data = &b[128..];
    let mut out = vec![0u8; w * h * 4];
    let mut four: [u8; 4] = b[84..88].try_into().unwrap();
    let mut dx10_rgba = None;
    if pf & 0x4 != 0 && &four == b"DX10" {
        if b.len() < 148 {
            bail!("{name}: truncated DDS");
        }
        data = &b[148..];
        match u32le(b, 128) {
            71 | 72 => four = *b"DXT1",
            74 | 75 => four = *b"DXT3",
            77 | 78 => four = *b"DXT5",
            87 | 91 => dx10_rgba = Some(true),
            28 | 29 => dx10_rgba = Some(false),
            f => bail!("{name}: unsupported DDS format (DXGI {f})"),
        }
    }
    if let Some(bgra) = dx10_rgba {
        if data.len() < w * h * 4 {
            bail!("{name}: truncated DDS");
        }
        out.par_chunks_mut(4).zip(data.par_chunks(4)).for_each(|(o, i)| {
            if bgra { o.copy_from_slice(&[i[2], i[1], i[0], i[3]]) } else { o.copy_from_slice(i) }
        });
        return Ok((w as u32, h as u32, out));
    }
    if pf & 0x4 != 0 {
        let four = &four[..];
        let block = if four == b"DXT1" { 8 } else { 16 };
        if four != b"DXT1" && four != b"DXT3" && four != b"DXT5" {
            bail!("{name}: unsupported DDS compression {:?}", String::from_utf8_lossy(four));
        }
        let (bw, bh) = (w.div_ceil(4), h.div_ceil(4));
        if data.len() < bw * bh * block {
            bail!("{name}: truncated DDS");
        }
        out.par_chunks_mut(w * 4 * 4).enumerate().for_each(|(by, rows)| {
            for bx in 0..bw {
                let blk = &data[(by * bw + bx) * block..][..block];
                let mut px = [[0u8; 4]; 16];
                match four {
                    b"DXT1" => bc1_colour(&blk[0..8], false, &mut px),
                    b"DXT3" => {
                        bc1_colour(&blk[8..16], true, &mut px);
                        for (i, p) in px.iter_mut().enumerate() {
                            p[3] = ((blk[i / 2] >> (4 * (i % 2))) & 15) * 17;
                        }
                    }
                    _ => {
                        bc1_colour(&blk[8..16], true, &mut px);
                        bc4_alpha(&blk[0..8], &mut px);
                    }
                }
                for (i, p) in px.iter().enumerate() {
                    let (x, y) = (bx * 4 + i % 4, i / 4);
                    if x < w && by * 4 + y < h {
                        rows[(y * w + x) * 4..][..4].copy_from_slice(p);
                    }
                }
            }
        });
    } else {
        let bits = u32le(b, 88) as usize;
        if bits != 32 && bits != 24 {
            bail!("{name}: unsupported {bits}-bit DDS");
        }
        let masks = [u32le(b, 92), u32le(b, 96), u32le(b, 100), if pf & 0x1 != 0 { u32le(b, 104) } else { 0 }];
        let bb = bits / 8;
        if data.len() < w * h * bb {
            bail!("{name}: truncated DDS");
        }
        let chan = |v: u32, m: u32| -> u8 {
            if m == 0 {
                return 255;
            }
            let s = m.trailing_zeros();
            let max = m >> s;
            (((v & m) >> s) * 255 / max) as u8
        };
        out.par_chunks_mut(4).enumerate().for_each(|(i, o)| {
            let s = &data[i * bb..i * bb + bb];
            let v = if bb == 4 { u32le(s, 0) } else { s[0] as u32 | (s[1] as u32) << 8 | (s[2] as u32) << 16 };
            for c in 0..4 {
                o[c] = chan(v, masks[c]);
            }
        });
    }
    Ok((w as u32, h as u32, out))
}

fn rgb565(c: u16) -> [u8; 3] {
    let (r, g, b) = ((c >> 11) & 31, (c >> 5) & 63, c & 31);
    [((r << 3) | (r >> 2)) as u8, ((g << 2) | (g >> 4)) as u8, ((b << 3) | (b >> 2)) as u8]
}

/// BC1 colour block (4 colours always for DXT3/5).
fn bc1_colour(b: &[u8], four: bool, px: &mut [[u8; 4]; 16]) {
    let (c0, c1) = (u16le(b, 0), u16le(b, 2));
    let (a, z) = (rgb565(c0), rgb565(c1));
    let mut pal = [[a[0], a[1], a[2], 255], [z[0], z[1], z[2], 255], [0; 4], [0; 4]];
    if four || c0 > c1 {
        for c in 0..3 {
            pal[2][c] = ((2 * a[c] as u16 + z[c] as u16) / 3) as u8;
            pal[3][c] = ((a[c] as u16 + 2 * z[c] as u16) / 3) as u8;
        }
        pal[2][3] = 255;
        pal[3][3] = 255;
    } else {
        for c in 0..3 {
            pal[2][c] = ((a[c] as u16 + z[c] as u16) / 2) as u8;
        }
        pal[2][3] = 255;
    }
    let idx = u32le(b, 4);
    for (i, p) in px.iter_mut().enumerate() {
        *p = pal[((idx >> (2 * i)) & 3) as usize];
    }
}

/// BC4 (DXT5) alpha block.
fn bc4_alpha(b: &[u8], px: &mut [[u8; 4]; 16]) {
    let (a0, a1) = (b[0] as u16, b[1] as u16);
    let mut pal = [0u8; 8];
    pal[0] = a0 as u8;
    pal[1] = a1 as u8;
    if a0 > a1 {
        for i in 1..7u16 {
            pal[i as usize + 1] = (((7 - i) * a0 + i * a1) / 7) as u8;
        }
    } else {
        for i in 1..5u16 {
            pal[i as usize + 1] = (((5 - i) * a0 + i * a1) / 5) as u8;
        }
        pal[6] = 0;
        pal[7] = 255;
    }
    let bits = b[2..8].iter().rev().fold(0u64, |acc, &x| (acc << 8) | x as u64);
    for (i, p) in px.iter_mut().enumerate() {
        p[3] = pal[((bits >> (3 * i)) & 7) as usize];
    }
}

// ------------------------------------------------------------------ TGA

/// 32-bit uncompressed TGA, rows bottom-up, with a TGA 2.0 footer (as Pillow writes it).
pub fn write_tga_rgba(path: &Path, w: u32, h: u32, rgba: &[u8]) -> Result<()> {
    let (wu, hu) = (w as usize, h as usize);
    let mut b = Vec::with_capacity(18 + rgba.len() + 26);
    b.extend_from_slice(&[0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    b.extend_from_slice(&(w as u16).to_le_bytes());
    b.extend_from_slice(&(h as u16).to_le_bytes());
    b.extend_from_slice(&[32, 8]);
    for y in (0..hu).rev() {
        for x in 0..wu {
            let p = &rgba[(y * wu + x) * 4..][..4];
            b.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
        }
    }
    b.extend_from_slice(&[0; 8]);
    b.extend_from_slice(b"TRUEVISION-XFILE.\0");
    write_file(path, &b)
}

/// A TGA file as RGBA: true colour (24/32-bit) or grey, plain or RLE, rows
/// either way up.
pub fn read_tga_bytes(b: &[u8], name: &str) -> Result<(u32, u32, Vec<u8>)> {
    if b.len() < 18 {
        bail!("{name}: not a TGA file");
    }
    let (idlen, cmap, kind) = (b[0] as usize, b[1], b[2]);
    let (w, h, bpp, desc) = (u16le(b, 12) as usize, u16le(b, 14) as usize, b[16] as usize, b[17]);
    if cmap != 0 || !matches!(kind, 2 | 3 | 10 | 11) || !matches!(bpp, 8 | 24 | 32) || w == 0 || h == 0 {
        bail!("{name}: unsupported TGA (type {kind}, {bpp} bits)");
    }
    let bb = bpp / 8;
    let mut px = Vec::with_capacity(w * h * bb);
    let mut i = 18 + idlen;
    if kind >= 9 {
        while px.len() < w * h * bb && i < b.len() {
            let c = b[i] as usize;
            i += 1;
            let n = (c & 127) + 1;
            if c & 128 != 0 {
                let p = b.get(i..i + bb).with_context(|| format!("{name}: truncated TGA"))?;
                for _ in 0..n {
                    px.extend_from_slice(p);
                }
                i += bb;
            } else {
                px.extend_from_slice(b.get(i..i + n * bb).with_context(|| format!("{name}: truncated TGA"))?);
                i += n * bb;
            }
        }
    } else {
        px.extend_from_slice(b.get(i..i + w * h * bb).with_context(|| format!("{name}: truncated TGA"))?);
    }
    px.truncate(w * h * bb);
    if px.len() < w * h * bb {
        bail!("{name}: truncated TGA");
    }
    let top_down = desc & 0x20 != 0;
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        let sy = if top_down { y } else { h - 1 - y };
        for x in 0..w {
            let s = &px[(sy * w + x) * bb..][..bb];
            let o = &mut out[(y * w + x) * 4..][..4];
            match bb {
                1 => o.copy_from_slice(&[s[0], s[0], s[0], 255]),
                3 => o.copy_from_slice(&[s[2], s[1], s[0], 255]),
                _ => o.copy_from_slice(&[s[2], s[1], s[0], s[3]]),
            }
        }
    }
    Ok((w as u32, h as u32, out))
}

/// Width, height and bits per pixel of a TGA file.
pub fn tga_info(path: &Path) -> Result<(u32, u32, u8)> {
    use std::io::Read as _;
    let mut b = [0u8; 18];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut b))
        .with_context(|| format!("cannot read {}", path.display()))?;
    Ok((u16le(&b, 12) as u32, u16le(&b, 14) as u32, b[16]))
}

// ------------------------------------------------------------------ resampling

/// Resampling filters as Pillow defines them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Filter {
    Box,
    Bilinear,
    Lanczos,
}

impl Filter {
    fn support(self) -> f64 {
        match self {
            Filter::Box => 0.5,
            Filter::Bilinear => 1.0,
            Filter::Lanczos => 3.0,
        }
    }
    fn weight(self, x: f64) -> f64 {
        match self {
            Filter::Box => {
                if x > -0.5 && x <= 0.5 {
                    1.0
                } else {
                    0.0
                }
            }
            Filter::Bilinear => {
                let x = x.abs();
                if x < 1.0 { 1.0 - x } else { 0.0 }
            }
            Filter::Lanczos => {
                let sinc = |x: f64| if x == 0.0 { 1.0 } else { (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x) };
                if (-3.0..3.0).contains(&x) { sinc(x) * sinc(x / 3.0) } else { 0.0 }
            }
        }
    }
}

/// Per output pixel: first input pixel and normalised weights (Pillow's
/// precompute_coeffs).
fn coeffs(in_size: usize, out_size: usize, f: Filter) -> Vec<(usize, Vec<f64>)> {
    let scale = in_size as f64 / out_size as f64;
    let filterscale = scale.max(1.0);
    let support = f.support() * filterscale;
    (0..out_size)
        .map(|xx| {
            let center = (xx as f64 + 0.5) * scale;
            let ss = 1.0 / filterscale;
            let xmin = ((center - support + 0.5) as i64).max(0) as usize;
            let xmax = ((center + support + 0.5) as i64).min(in_size as i64) as usize;
            let mut k: Vec<f64> = (xmin..xmax.max(xmin)).map(|x| f.weight((x as f64 - center + 0.5) * ss)).collect();
            let ww: f64 = k.iter().sum();
            if ww != 0.0 {
                for v in &mut k {
                    *v /= ww;
                }
            }
            (xmin, k)
        })
        .collect()
}

/// Resize a float image (one channel), horizontal pass first, as Pillow does
/// for mode "F".
pub fn resize_f32(src: &[f32], w: usize, h: usize, ow: usize, oh: usize, f: Filter) -> Vec<f32> {
    let kx = coeffs(w, ow, f);
    let mut tmp = vec![0f32; ow * h];
    tmp.par_chunks_mut(ow).enumerate().for_each(|(y, out)| {
        let row = &src[y * w..(y + 1) * w];
        for (o, (x0, k)) in out.iter_mut().zip(&kx) {
            *o = k.iter().enumerate().map(|(i, &c)| row[x0 + i] as f64 * c).sum::<f64>() as f32;
        }
    });
    let ky = coeffs(h, oh, f);
    let mut out = vec![0f32; ow * oh];
    out.par_chunks_mut(ow).enumerate().for_each(|(yy, o)| {
        let (y0, k) = &ky[yy];
        for (x, v) in o.iter_mut().enumerate() {
            *v = k.iter().enumerate().map(|(i, &c)| tmp[(y0 + i) * ow + x] as f64 * c).sum::<f64>() as f32;
        }
    });
    out
}

const PRECISION_BITS: u32 = 32 - 8 - 2;

fn fixed(k: &[f64]) -> Vec<i32> {
    k.iter()
        .map(|&v| {
            let s = v * (1u32 << PRECISION_BITS) as f64;
            (if v < 0.0 { s - 0.5 } else { s + 0.5 }) as i32
        })
        .collect()
}

fn clip8(v: i64) -> u8 {
    (v >> PRECISION_BITS).clamp(0, 255) as u8
}

/// Resize an 8-bit image with `ch` interleaved channels, in Pillow's fixed
/// point arithmetic.
pub fn resize_u8(src: &[u8], w: usize, h: usize, ch: usize, ow: usize, oh: usize, f: Filter) -> Vec<u8> {
    let kx: Vec<(usize, Vec<i32>)> = coeffs(w, ow, f).into_iter().map(|(x0, k)| (x0, fixed(&k))).collect();
    let half = 1i64 << (PRECISION_BITS - 1);
    let mut tmp = vec![0u8; ow * h * ch];
    tmp.par_chunks_mut(ow * ch).enumerate().for_each(|(y, out)| {
        let row = &src[y * w * ch..(y + 1) * w * ch];
        for (xx, (x0, k)) in kx.iter().enumerate() {
            for c in 0..ch {
                let s: i64 = k.iter().enumerate().map(|(i, &kv)| row[(x0 + i) * ch + c] as i64 * kv as i64).sum();
                out[xx * ch + c] = clip8(s + half);
            }
        }
    });
    let ky: Vec<(usize, Vec<i32>)> = coeffs(h, oh, f).into_iter().map(|(y0, k)| (y0, fixed(&k))).collect();
    let mut out = vec![0u8; ow * oh * ch];
    out.par_chunks_mut(ow * ch).enumerate().for_each(|(yy, o)| {
        let (y0, k) = &ky[yy];
        for x in 0..ow * ch {
            let s: i64 = k.iter().enumerate().map(|(i, &kv)| tmp[(y0 + i) * ow * ch + x] as i64 * kv as i64).sum();
            o[x] = clip8(s + half);
        }
    });
    out
}

/// Nearest-neighbour resize (Pillow: the source pixel under each output
/// pixel's centre).
pub fn resize_nearest<T: Copy + Send + Sync>(src: &[T], w: usize, h: usize, ow: usize, oh: usize) -> Vec<T> {
    let (sx, sy) = (w as f64 / ow as f64, h as f64 / oh as f64);
    let xs: Vec<usize> = (0..ow).map(|x| (((x as f64 + 0.5) * sx) as usize).min(w - 1)).collect();
    let mut out = Vec::with_capacity(ow * oh);
    for y in 0..oh {
        let r = (((y as f64 + 0.5) * sy) as usize).min(h - 1);
        out.extend(xs.iter().map(|&x| src[r * w + x]));
    }
    out
}

// ------------------------------------------------------------------ filters

/// 3×3 mean of one float plane, edges mirrored (scipy's uniform_filter,
/// mode "reflect"): rows first, then columns.
pub fn uniform3(src: &[f32], w: usize, h: usize) -> Vec<f32> {
    let mut tmp = vec![0f32; w * h];
    tmp.par_chunks_mut(w).enumerate().for_each(|(y, out)| {
        let (a, c) = (y.saturating_sub(1), (y + 1).min(h - 1));
        for x in 0..w {
            out[x] = ((src[a * w + x] as f64 + src[y * w + x] as f64 + src[c * w + x] as f64) / 3.0) as f32;
        }
    });
    let mut out = vec![0f32; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, o)| {
        let row = &tmp[y * w..(y + 1) * w];
        for x in 0..w {
            let (a, c) = (x.saturating_sub(1), (x + 1).min(w - 1));
            o[x] = ((row[a] as f64 + row[x] as f64 + row[c] as f64) / 3.0) as f32;
        }
    });
    out
}

/// Squared Euclidean distance from every `true` pixel to the nearest `false`
/// pixel (0 for `false` pixels); Felzenszwalb & Huttenlocher's exact
/// algorithm, as scipy's distance_transform_edt.
pub fn edt_sq(fg: &[bool], w: usize, h: usize) -> Vec<f32> {
    const INF: f64 = 1e20;
    // columns, stored column-major
    let mut cols = vec![0f32; w * h];
    cols.par_chunks_mut(h).enumerate().for_each_init(
        || (vec![0f64; h], vec![0f64; h], vec![0usize; h], vec![0f64; h + 1]),
        |(f, d, v, z), (x, out)| {
            for y in 0..h {
                f[y] = if fg[y * w + x] { INF } else { 0.0 };
            }
            dt1(f, d, v, z);
            for y in 0..h {
                out[y] = d[y].min(f32::MAX as f64) as f32;
            }
        },
    );
    let mut out = vec![0f32; w * h];
    out.par_chunks_mut(w).enumerate().for_each_init(
        || (vec![0f64; w], vec![0f64; w], vec![0usize; w], vec![0f64; w + 1]),
        |(f, d, v, z), (y, o)| {
            for x in 0..w {
                f[x] = cols[x * h + y] as f64;
            }
            dt1(f, d, v, z);
            for x in 0..w {
                o[x] = d[x].min(f32::MAX as f64) as f32;
            }
        },
    );
    out
}

/// 1-D squared distance transform of sampled function `f`.
fn dt1(f: &[f64], d: &mut [f64], v: &mut [usize], z: &mut [f64]) {
    let n = f.len();
    if n == 0 {
        return;
    }
    let mut k = 0usize;
    v[0] = 0;
    z[0] = f64::NEG_INFINITY;
    z[1] = f64::INFINITY;
    for q in 1..n {
        let qf = q as f64;
        let mut s;
        loop {
            let p = v[k] as f64;
            s = ((f[q] + qf * qf) - (f[v[k]] + p * p)) / (2.0 * (qf - p));
            if s <= z[k] && k > 0 {
                k -= 1;
            } else {
                break;
            }
        }
        if s <= z[k] {
            // k == 0: the new parabola is lower everywhere
            v[0] = q;
            z[0] = f64::NEG_INFINITY;
            z[1] = f64::INFINITY;
            continue;
        }
        k += 1;
        v[k] = q;
        z[k] = s;
        z[k + 1] = f64::INFINITY;
    }
    k = 0;
    for (q, out) in d.iter_mut().enumerate() {
        while z[k + 1] < q as f64 {
            k += 1;
        }
        let dq = q as f64 - v[k] as f64;
        *out = dq * dq + f[v[k]];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edt_matches_brute_force() {
        let (w, h) = (23, 17);
        let fg: Vec<bool> = (0..w * h).map(|i| (i * 7919 + i / 5) % 11 != 0).collect();
        let d = edt_sq(&fg, w, h);
        for y in 0..h {
            for x in 0..w {
                let want = if !fg[y * w + x] {
                    0
                } else {
                    (0..h)
                        .flat_map(|yy| (0..w).map(move |xx| (xx, yy)))
                        .filter(|&(xx, yy)| !fg[yy * w + xx])
                        .map(|(xx, yy)| (xx as i64 - x as i64).pow(2) + (yy as i64 - y as i64).pow(2))
                        .min()
                        .unwrap()
                };
                assert_eq!(d[y * w + x] as i64, want, "at {x},{y}");
            }
        }
    }

    #[test]
    fn box_resize_averages() {
        // 4x2 -> 2x1: each output pixel is the mean of a 2x2 block
        let src = [0.0, 2.0, 4.0, 6.0, 2.0, 4.0, 6.0, 8.0];
        let out = resize_f32(&src, 4, 2, 2, 1, Filter::Box);
        assert_eq!(out, vec![2.0, 6.0]);
        // 8-bit: rounded after each pass (row 2 gives 3, 8; then 2, 6.5 -> 7)
        let src8 = [0u8, 2, 4, 6, 2, 4, 6, 9];
        assert_eq!(resize_u8(&src8, 4, 2, 1, 2, 1, Filter::Box), vec![2, 7]);
        let same = resize_u8(&[7u8; 30], 6, 5, 1, 4, 3, Filter::Lanczos);
        assert!(same.iter().all(|&v| v == 7));
    }

    #[test]
    fn nearest_picks_centres() {
        let src: Vec<u32> = (0..8).collect();
        assert_eq!(resize_nearest(&src, 8, 1, 4, 1), vec![1, 3, 5, 7]);
    }

    #[test]
    fn dxt_blocks() {
        // colour block: c0 = white, c1 = black, indices 0,1,2,3 in the first row
        let mut px = [[0u8; 4]; 16];
        bc1_colour(&[0xff, 0xff, 0, 0, 0b1110_0100, 0, 0, 0], true, &mut px);
        assert_eq!(px[0], [255, 255, 255, 255]);
        assert_eq!(px[1], [0, 0, 0, 255]);
        assert_eq!(px[2], [170, 170, 170, 255]);
        assert_eq!(px[3], [85, 85, 85, 255]);
        bc4_alpha(&[200, 100, 0b0000_1000, 0, 0, 0, 0, 0], &mut px);
        assert_eq!(px[0][3], 200);
        assert_eq!(px[1][3], 100);
    }
}
