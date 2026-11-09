//! Zip archives of content bundles, in memory (see [`crate::zip`]).

use anyhow::Result;

/// A zip of `files` (name, bytes); names use `/`. The same files make the
/// same bytes.
pub fn write(files: &[(String, Vec<u8>)]) -> Result<Vec<u8>> {
    let mut w = crate::zip::Writer::reproducible(Vec::new());
    for (name, data) in files {
        w.add(name, data)?;
    }
    w.finish()
}

/// The files of a zip: (name, bytes). Folders are left out.
pub fn read(b: &[u8]) -> Result<Vec<(String, Vec<u8>)>> {
    let mut r = crate::zip::Reader::new(std::io::Cursor::new(b), "the file")?;
    let entries: Vec<crate::zip::Entry> = r.entries().iter().filter(|e| !e.name.ends_with('/')).cloned().collect();
    entries.iter().map(|e| Ok((e.name.clone(), r.read(e)?))).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn round_trip() {
        let files = vec![("a/b.txt".to_string(), b"hello hello hello hello hello".to_vec()), ("c.bin".to_string(), vec![1, 2, 3]), ("empty".to_string(), vec![])];
        let z = super::write(&files).unwrap();
        assert_eq!(super::read(&z).unwrap(), files);
        assert!(super::read(b"nope").is_err());
        assert_eq!(super::write(&files).unwrap(), z, "the same bytes");
    }
}
