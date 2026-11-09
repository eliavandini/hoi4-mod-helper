//! Just enough of ZIP for project files: stored and deflated entries, UTF-8
//! names, no zip64 (entries and archives under 4 GB), no encryption. Any
//! zip tool opens what [`Writer`] writes, and [`Reader`] reads what such
//! tools write (within those limits).

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;

use anyhow::{Context as _, Result, bail, ensure};
use flate2::Compression;

const LOCAL: u32 = 0x0403_4b50;
const CENTRAL: u32 = 0x0201_4b50;
const END: u32 = 0x0605_4b50;
const STORED: u16 = 0;
const DEFLATED: u16 = 8;
/// Names are UTF-8.
const FLAG_UTF8: u16 = 0x0800;

/// An entry of an archive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    method: u16,
    pub crc: u32,
    /// Bytes in the archive.
    pub packed: u64,
    /// Bytes unpacked.
    pub size: u64,
    /// Where its local header is.
    offset: u64,
}

/// A name safe to unpack under a folder: relative, `/`-separated, no `..`.
pub fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('/')
        && !name.contains('\\')
        && !name.contains('\0')
        && !name.contains(':')
        && name.split('/').all(|c| !c.is_empty() && c != "." && c != "..")
}

fn u16_at(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

pub struct Reader<R = BufReader<File>> {
    file: R,
    entries: Vec<Entry>,
}

impl Reader {
    pub fn open(path: &Path) -> Result<Reader> {
        let file = File::open(path).with_context(|| format!("cannot open {}", path.display()))?;
        Reader::new(BufReader::new(file), &path.display().to_string())
    }
}

impl<R: Read + Seek> Reader<R> {
    /// Read the directory of an archive (`what` names it in errors).
    pub fn new(mut file: R, what: &str) -> Result<Reader<R>> {
        let len = file.seek(SeekFrom::End(0))?;
        // the end record: 22 bytes plus a comment of up to 64 KB
        let tail_len = len.min(22 + 65_535);
        let mut tail = vec![0; tail_len as usize];
        file.seek(SeekFrom::Start(len - tail_len))?;
        file.read_exact(&mut tail)?;
        let Some(e) = (0..tail.len().saturating_sub(21)).rev().find(|&i| u32_at(&tail, i) == END) else {
            bail!("{what} is not a ZIP archive");
        };
        let (count, cd_size, cd_off) = (u16_at(&tail, e + 10) as usize, u32_at(&tail, e + 12) as u64, u32_at(&tail, e + 16) as u64);
        ensure!(count != 0xFFFF && cd_off != 0xFFFF_FFFF, "{what}: ZIP64 archives are not supported");
        ensure!(cd_off + cd_size <= len, "{what}: the archive is cut short");
        let mut cd = vec![0; cd_size as usize];
        file.seek(SeekFrom::Start(cd_off))?;
        file.read_exact(&mut cd)?;
        let mut entries = Vec::with_capacity(count);
        let mut i = 0;
        while entries.len() < count {
            ensure!(i + 46 <= cd.len() && u32_at(&cd, i) == CENTRAL, "{what}: damaged central directory");
            let (nlen, xlen, clen) = (u16_at(&cd, i + 28) as usize, u16_at(&cd, i + 30) as usize, u16_at(&cd, i + 32) as usize);
            ensure!(i + 46 + nlen <= cd.len(), "{what}: damaged central directory");
            let e = Entry {
                name: String::from_utf8_lossy(&cd[i + 46..i + 46 + nlen]).into_owned(),
                method: u16_at(&cd, i + 10),
                crc: u32_at(&cd, i + 16),
                packed: u32_at(&cd, i + 20) as u64,
                size: u32_at(&cd, i + 24) as u64,
                offset: u32_at(&cd, i + 42) as u64,
            };
            ensure!(
                e.packed != 0xFFFF_FFFF && e.size != 0xFFFF_FFFF && e.offset != 0xFFFF_FFFF,
                "{what}: ZIP64 entries are not supported ({})",
                e.name
            );
            ensure!(u16_at(&cd, i + 8) & 1 == 0, "{what}: {} is encrypted", e.name);
            entries.push(e);
            i += 46 + nlen + xlen + clen;
        }
        Ok(Reader { file, entries })
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn find(&self, name: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.name == name)
    }

    /// The entry's bytes as stored in the archive.
    pub fn raw(&mut self, e: &Entry) -> Result<Vec<u8>> {
        self.seek_data(e)?;
        let mut v = vec![0; e.packed as usize];
        self.file.read_exact(&mut v).with_context(|| format!("cannot read {}", e.name))?;
        Ok(v)
    }

    fn seek_data(&mut self, e: &Entry) -> Result<()> {
        self.file.seek(SeekFrom::Start(e.offset))?;
        let mut h = [0u8; 30];
        self.file.read_exact(&mut h)?;
        ensure!(u32_at(&h, 0) == LOCAL, "{}: damaged entry", e.name);
        let skip = u16_at(&h, 26) as i64 + u16_at(&h, 28) as i64;
        self.file.seek(SeekFrom::Current(skip))?;
        Ok(())
    }

    /// A reader of the entry's unpacked bytes (not checked: for a peek at
    /// its start).
    pub fn open_entry(&mut self, e: &Entry) -> Result<Box<dyn Read + '_>> {
        self.seek_data(e)?;
        let r = (&mut self.file).take(e.packed);
        Ok(match e.method {
            STORED => Box::new(r),
            DEFLATED => Box::new(flate2::read::DeflateDecoder::new(r)),
            m => bail!("{}: compression method {m} is not supported", e.name),
        })
    }

    /// The entry unpacked (and checked).
    pub fn read(&mut self, e: &Entry) -> Result<Vec<u8>> {
        let raw = self.raw(e)?;
        unpack(e, &raw)
    }
}

/// An entry's bytes from its raw ones (checked against its CRC and size).
pub fn unpack(e: &Entry, raw: &[u8]) -> Result<Vec<u8>> {
    let data = match e.method {
        STORED => raw.to_vec(),
        DEFLATED => {
            let mut v = Vec::with_capacity(e.size as usize);
            flate2::read::DeflateDecoder::new(raw).read_to_end(&mut v).with_context(|| format!("{}: damaged data", e.name))?;
            v
        }
        m => bail!("{}: compression method {m} is not supported", e.name),
    };
    ensure!(data.len() as u64 == e.size && crc(&data) == e.crc, "{}: damaged data (size or checksum)", e.name);
    Ok(data)
}

pub fn crc(data: &[u8]) -> u32 {
    let mut c = flate2::Crc::new();
    c.update(data);
    c.sum()
}

/// An entry's data ready to be written.
pub struct Packed {
    method: u16,
    crc: u32,
    size: u64,
    data: Vec<u8>,
}

/// Deflate `data` (stored as it is when that is not smaller). Level 1 for
/// big files (bitmaps of flat colours pack about as well, and much faster).
pub fn pack(data: &[u8]) -> Packed {
    let level = if data.len() > 1 << 20 { Compression::fast() } else { Compression::default() };
    let mut enc = flate2::write::DeflateEncoder::new(Vec::with_capacity(data.len() / 4), level);
    let deflated = enc.write_all(data).and_then(|_| enc.finish()).ok();
    let (method, out) = match deflated {
        Some(d) if d.len() < data.len() => (DEFLATED, d),
        _ => (STORED, data.to_vec()),
    };
    Packed { method, crc: crc(data), size: data.len() as u64, data: out }
}

/// MS-DOS time and date of now (UTC).
fn dos_now() -> (u16, u16) {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (days, rem) = ((secs / 86400) as i64, secs % 86400);
    // civil_from_days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = (yoe + era * 400 + i64::from(m <= 2)).clamp(1980, 2107);
    let time = (((rem / 3600) << 11) | ((rem / 60 % 60) << 5) | ((rem % 60) / 2)) as u16;
    let date = (((y - 1980) << 9) | (m << 5) | d) as u16;
    (time, date)
}

pub struct Writer<W: Write> {
    out: W,
    pos: u64,
    central: Vec<u8>,
    count: usize,
    time: (u16, u16),
    names: rustc_hash::FxHashSet<String>,
}

impl<W: Write> Writer<W> {
    pub fn new(out: W) -> Writer<W> {
        Writer { out, pos: 0, central: Vec::new(), count: 0, time: dos_now(), names: Default::default() }
    }

    /// Entries dated 1980-01-01 00:00, so the same files make the same bytes.
    pub fn reproducible(out: W) -> Writer<W> {
        Writer { time: (0, (1 << 5) | 1), ..Writer::new(out) }
    }

    fn entry(&mut self, name: &str, method: u16, crc: u32, size: u64, data: &[u8]) -> Result<()> {
        ensure!(safe_name(name), "unsafe name in archive: {name}");
        ensure!(self.names.insert(name.to_string()), "{name} is twice in the archive");
        ensure!(size < 0xFFFF_FFFF && (data.len() as u64) < 0xFFFF_FFFF && self.pos < 0xFFFF_FFFF, "{name}: files over 4 GB are not supported");
        ensure!(self.count < 0xFFFF, "too many files for an archive");
        let n = name.as_bytes();
        let (time, date) = self.time;
        let mut h = Vec::with_capacity(30 + n.len());
        h.extend_from_slice(&LOCAL.to_le_bytes());
        h.extend_from_slice(&20u16.to_le_bytes()); // version needed: 2.0
        h.extend_from_slice(&FLAG_UTF8.to_le_bytes());
        h.extend_from_slice(&method.to_le_bytes());
        h.extend_from_slice(&time.to_le_bytes());
        h.extend_from_slice(&date.to_le_bytes());
        h.extend_from_slice(&crc.to_le_bytes());
        h.extend_from_slice(&(data.len() as u32).to_le_bytes());
        h.extend_from_slice(&(size as u32).to_le_bytes());
        h.extend_from_slice(&(n.len() as u16).to_le_bytes());
        h.extend_from_slice(&0u16.to_le_bytes());
        h.extend_from_slice(n);
        self.out.write_all(&h)?;
        self.out.write_all(data)?;
        let c = &mut self.central;
        c.extend_from_slice(&CENTRAL.to_le_bytes());
        c.extend_from_slice(&(3u16 << 8 | 20).to_le_bytes()); // made by: Unix, 2.0
        c.extend_from_slice(&h[4..30]);
        c.extend_from_slice(&0u16.to_le_bytes()); // comment
        c.extend_from_slice(&0u16.to_le_bytes()); // disk
        c.extend_from_slice(&0u16.to_le_bytes()); // internal attributes
        c.extend_from_slice(&(0o100644u32 << 16).to_le_bytes()); // a regular file, rw-r--r--
        c.extend_from_slice(&(self.pos as u32).to_le_bytes());
        c.extend_from_slice(n);
        self.pos += h.len() as u64 + data.len() as u64;
        self.count += 1;
        Ok(())
    }

    /// Add `data` packed with [`pack`].
    pub fn add(&mut self, name: &str, data: &[u8]) -> Result<()> {
        self.add_packed(name, &pack(data))
    }

    pub fn add_packed(&mut self, name: &str, p: &Packed) -> Result<()> {
        self.entry(name, p.method, p.crc, p.size, &p.data)
    }

    /// Copy an entry of another archive as it is (its raw bytes).
    pub fn add_raw(&mut self, e: &Entry, raw: &[u8]) -> Result<()> {
        ensure!(raw.len() as u64 == e.packed, "{}: wrong raw size", e.name);
        self.entry(&e.name, e.method, e.crc, e.size, raw)
    }

    pub fn finish(mut self) -> Result<W> {
        let mut end = Vec::with_capacity(22);
        end.extend_from_slice(&END.to_le_bytes());
        end.extend_from_slice(&0u16.to_le_bytes());
        end.extend_from_slice(&0u16.to_le_bytes());
        end.extend_from_slice(&(self.count as u16).to_le_bytes());
        end.extend_from_slice(&(self.count as u16).to_le_bytes());
        end.extend_from_slice(&(self.central.len() as u32).to_le_bytes());
        end.extend_from_slice(&(self.pos as u32).to_le_bytes());
        end.extend_from_slice(&0u16.to_le_bytes());
        self.out.write_all(&self.central)?;
        self.out.write_all(&end)?;
        self.out.flush()?;
        Ok(self.out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let tmp = std::env::temp_dir().join(format!("mh-zip-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let (a, b) = (tmp.join("a.zip"), tmp.join("b.zip"));
        let big: Vec<u8> = (0..3_000_000u32).map(|i| (i / 5000) as u8).collect();
        let noise: Vec<u8> = (0..1000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
        let mut w = Writer::new(File::create(&a).unwrap());
        w.add("project.json", b"{\"version\": 2}").unwrap();
        w.add("map/provinces.bmp", &big).unwrap();
        w.add("map/content/TYR/ä.txt", &noise).unwrap();
        w.add("map/empty", b"").unwrap();
        assert!(w.add("map/empty", b"x").is_err(), "twice");
        assert!(w.add("../evil", b"x").is_err());
        w.finish().unwrap();
        let mut r = Reader::open(&a).unwrap();
        let names: Vec<_> = r.entries().iter().map(|e| e.name.clone()).collect();
        assert_eq!(names, ["project.json", "map/provinces.bmp", "map/content/TYR/ä.txt", "map/empty"]);
        let e = r.find("map/provinces.bmp").unwrap().clone();
        assert!(e.packed < e.size / 50, "flat data packs small: {} of {}", e.packed, e.size);
        assert_eq!(r.read(&e).unwrap(), big);
        let mut head = [0u8; 4];
        r.open_entry(&e).unwrap().read_exact(&mut head).unwrap();
        assert_eq!(head, big[..4]);
        assert_eq!(r.read(&r.find("map/content/TYR/ä.txt").unwrap().clone()).unwrap(), noise);
        assert_eq!(r.read(&r.find("map/empty").unwrap().clone()).unwrap(), b"");
        // copied raw into another archive: the same
        let mut w = Writer::new(File::create(&b).unwrap());
        for e in r.entries().to_vec() {
            let raw = r.raw(&e).unwrap();
            w.add_raw(&e, &raw).unwrap();
        }
        w.finish().unwrap();
        assert_eq!(std::fs::read(&a).unwrap().len(), std::fs::read(&b).unwrap().len());
        let mut r2 = Reader::open(&b).unwrap();
        assert_eq!(r2.read(&r2.find("map/provinces.bmp").unwrap().clone()).unwrap(), big);
        // damage is noticed
        let mut bytes = std::fs::read(&a).unwrap();
        let at = (e.offset + 30 + e.name.len() as u64 + e.packed / 2) as usize;
        bytes[at] ^= 0xFF;
        std::fs::write(&b, &bytes).unwrap();
        let mut r3 = Reader::open(&b).unwrap();
        let e = r3.find("map/provinces.bmp").unwrap().clone();
        assert!(r3.read(&e).is_err());
        assert!(Reader::open(&tmp.join("missing.zip")).is_err());
        std::fs::write(&b, b"not a zip at all, just some text").unwrap();
        assert!(Reader::open(&b).is_err());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn names() {
        for ok in ["a", "map/provinces.bmp", "map/content/TYR/ä.txt", ".hidden"] {
            assert!(safe_name(ok), "{ok}");
        }
        for bad in ["", "/etc/passwd", "../x", "a/../../x", "a//b", "a\\b", "C:x", "a/./b", "a/"] {
            assert!(!safe_name(bad), "{bad}");
        }
    }
}
