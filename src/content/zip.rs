//! Zip archives, as far as content bundles need: write files (deflated) and
//! read them back (stored or deflated, no zip64, no encryption).

use std::io::{Read as _, Write as _};

use anyhow::{Context as _, Result, bail};

fn u16le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn crc32(data: &[u8]) -> u32 {
    let mut c = flate2::Crc::new();
    c.update(data);
    c.sum()
}

/// A zip of `files` (name, bytes); names use `/`.
pub fn write(files: &[(String, Vec<u8>)]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    // 1980-01-01 00:00 (DOS time), so the same files give the same bytes
    let (time, date) = (0u16, (1u16 << 5) | 1);
    for (name, data) in files {
        let mut enc = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(data)?;
        let packed = enc.finish()?;
        let (method, body): (u16, &[u8]) = if packed.len() < data.len() { (8, &packed) } else { (0, data) };
        let crc = crc32(data);
        let offset = out.len() as u32;
        let nb = name.as_bytes();
        let mut h = Vec::new();
        h.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        h.extend_from_slice(&20u16.to_le_bytes());
        h.extend_from_slice(&0x0800u16.to_le_bytes()); // UTF-8 names
        h.extend_from_slice(&method.to_le_bytes());
        h.extend_from_slice(&time.to_le_bytes());
        h.extend_from_slice(&date.to_le_bytes());
        h.extend_from_slice(&crc.to_le_bytes());
        h.extend_from_slice(&(body.len() as u32).to_le_bytes());
        h.extend_from_slice(&(data.len() as u32).to_le_bytes());
        h.extend_from_slice(&(nb.len() as u16).to_le_bytes());
        h.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&h);
        out.extend_from_slice(nb);
        out.extend_from_slice(body);
        let mut c = Vec::new();
        c.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        c.extend_from_slice(&20u16.to_le_bytes());
        c.extend_from_slice(&h[4..30]);
        c.extend_from_slice(&[0u8; 6]); // comment length, disk, internal attributes
        c.extend_from_slice(&0u32.to_le_bytes()); // external attributes
        c.extend_from_slice(&offset.to_le_bytes());
        c.extend_from_slice(nb);
        central.extend_from_slice(&c);
        if out.len() > u32::MAX as usize / 2 {
            bail!("the bundle is too big");
        }
    }
    let cd_offset = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0u8; 4]);
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    Ok(out)
}

/// The files of a zip: (name, bytes). Folders are left out.
pub fn read(b: &[u8]) -> Result<Vec<(String, Vec<u8>)>> {
    let eocd = (0..b.len().saturating_sub(21)).rev().take(70_000).find(|&i| u32le(b, i) == 0x0605_4b50).context("not a zip file")?;
    let n = u16le(b, eocd + 10) as usize;
    let mut at = u32le(b, eocd + 16) as usize;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        if at + 46 > b.len() || u32le(b, at) != 0x0201_4b50 {
            bail!("broken zip directory");
        }
        let method = u16le(b, at + 10);
        let csize = u32le(b, at + 20) as usize;
        let usize_ = u32le(b, at + 24) as usize;
        let (nl, xl, cl) = (u16le(b, at + 28) as usize, u16le(b, at + 30) as usize, u16le(b, at + 32) as usize);
        let local = u32le(b, at + 42) as usize;
        let name = String::from_utf8_lossy(b.get(at + 46..at + 46 + nl).context("broken zip")?).into_owned();
        at += 46 + nl + xl + cl;
        if name.ends_with('/') {
            continue;
        }
        if local + 30 > b.len() || u32le(b, local) != 0x0403_4b50 {
            bail!("broken zip entry {name}");
        }
        let start = local + 30 + u16le(b, local + 26) as usize + u16le(b, local + 28) as usize;
        let data = b.get(start..start + csize).with_context(|| format!("truncated zip entry {name}"))?;
        let bytes = match method {
            0 => data.to_vec(),
            8 => {
                let mut v = Vec::with_capacity(usize_);
                flate2::read::DeflateDecoder::new(data).read_to_end(&mut v).with_context(|| format!("cannot unpack {name}"))?;
                v
            }
            m => bail!("{name}: zip compression {m} is not supported"),
        };
        out.push((name, bytes));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn round_trip() {
        let files = vec![("a/b.txt".to_string(), b"hello hello hello hello hello".to_vec()), ("c.bin".to_string(), vec![1, 2, 3]), ("empty".to_string(), vec![])];
        let z = super::write(&files).unwrap();
        assert_eq!(super::read(&z).unwrap(), files);
        assert!(super::read(b"nope").is_err());
    }
}
