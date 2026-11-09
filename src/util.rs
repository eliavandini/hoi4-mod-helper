use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use parking_lot::Mutex;

/// Progress of a background job, shared between the worker and the UI.
#[derive(Default)]
pub struct Progress {
    /// 0..=10_000
    fraction: AtomicU32,
    stage: Mutex<String>,
}

impl Progress {
    pub fn set(&self, f: f32) {
        self.fraction.store((f.clamp(0.0, 1.0) * 10_000.0) as u32, Ordering::Relaxed);
    }

    pub fn get(&self) -> f32 {
        self.fraction.load(Ordering::Relaxed) as f32 / 10_000.0
    }

    pub fn set_stage(&self, s: impl Into<String>) {
        *self.stage.lock() = s.into();
        self.set(0.0);
    }

    pub fn stage(&self) -> String {
        self.stage.lock().clone()
    }
}

pub fn fmt_bytes(b: usize) -> String {
    let b = b as f64;
    if b >= 1e9 {
        format!("{:.2} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.1} MB", b / 1e6)
    } else if b >= 1e3 {
        format!("{:.0} kB", b / 1e3)
    } else {
        format!("{b} B")
    }
}

/// Format a distance in metres compactly.
pub fn fmt_dist(m: f64) -> String {
    if m.abs() >= 1000.0 { format!("{:.3} km", m / 1000.0) } else { format!("{m:.1} m") }
}

/// Express `path` relative to directory `base` if both are absolute and
/// share a folder below the root (e.g. `/a/b/c` relative to `/a/d` →
/// `../b/c`; `/home/c` relative to `/tmp/d` stays as it is).
pub fn relative_to(path: &Path, base: &Path) -> PathBuf {
    if !path.is_absolute() || !base.is_absolute() {
        return path.to_path_buf();
    }
    let p: Vec<Component> = path.components().collect();
    let b: Vec<Component> = base.components().collect();
    let common = p.iter().zip(&b).take_while(|(a, b)| a == b).count();
    if !p[..common].iter().any(|c| matches!(c, Component::Normal(_))) {
        return path.to_path_buf();
    }
    let mut out = PathBuf::new();
    for _ in common..b.len() {
        out.push("..");
    }
    for c in &p[common..] {
        out.push(c.as_os_str());
    }
    out
}

/// Lexically normalise a path (resolve `.` and `..` without touching the disk).
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The mod's `maps` folder for the tests that need the real map files
/// (`MOD_HELPER_TEST_MAPS`); such tests are ignored by default.
#[cfg(test)]
pub fn test_maps_dir() -> std::path::PathBuf {
    match std::env::var_os("MOD_HELPER_TEST_MAPS") {
        Some(p) => std::path::PathBuf::from(p),
        None => panic!("set MOD_HELPER_TEST_MAPS to the mod's maps folder (with processed maps/ and osm regions/)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths() {
        assert_eq!(relative_to(Path::new("/a/b/c.bmp"), Path::new("/a/d")), PathBuf::from("../b/c.bmp"));
        assert_eq!(relative_to(Path::new("/a/d/c.bmp"), Path::new("/a/d")), PathBuf::from("c.bmp"));
        assert_eq!(normalize(Path::new("/a/d/../b/./c")), PathBuf::from("/a/b/c"));
        assert_eq!(relative_to(Path::new("/home/c.bmp"), Path::new("/tmp/d")), PathBuf::from("/home/c.bmp"));
    }
}
