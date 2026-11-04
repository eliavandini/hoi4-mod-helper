//! "Build mod": build the HOI4 mod from the saved map (see [`crate::modbuild`])
//! on a worker thread, then check it, streaming both into a log.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use eframe::egui;
use parking_lot::Mutex;

use crate::modbuild::{self, Dirs};

pub const RUNNING: u8 = 0;
pub const OK: u8 = 1;
pub const FAILED: u8 = 2;

pub struct BuildJob {
    pub log: Arc<Mutex<String>>,
    pub state: Arc<AtomicU8>,
    /// The mod folder, once written.
    pub mod_dir: Arc<Mutex<Option<PathBuf>>>,
    /// Number of the build in this session.
    pub id: u64,
}

static BUILDS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl BuildJob {
    pub fn state(&self) -> u8 {
        self.state.load(Ordering::Relaxed)
    }
}

pub fn start(map_dir: &Path, dirs: Dirs, ctx: &egui::Context) -> BuildJob {
    let job = BuildJob {
        log: Arc::new(Mutex::new(String::new())),
        state: Arc::new(AtomicU8::new(RUNNING)),
        mod_dir: Arc::new(Mutex::new(None)),
        id: BUILDS.fetch_add(1, Ordering::Relaxed) + 1,
    };
    let (log, state, out, ctx) = (job.log.clone(), job.state.clone(), job.mod_dir.clone(), ctx.clone());
    let map_dir = map_dir.to_path_buf();
    std::thread::spawn(move || {
        let line = |l: &str| {
            let mut g = log.lock();
            g.push_str(l);
            g.push('\n');
            drop(g);
            ctx.request_repaint();
        };
        line(&format!("map {}\ngame {}\nmods {}\n", map_dir.display(), dirs.game.display(), dirs.mods.display()));
        // a bug in the build must not leave the job running forever
        let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> anyhow::Result<bool> {
            let mod_dir = modbuild::build(&map_dir, &dirs, &line)?;
            *out.lock() = Some(mod_dir.clone());
            line("");
            let errors = modbuild::validate::validate(&mod_dir, &dirs.game, &line)?;
            Ok(errors.is_empty())
        }));
        let good = match run {
            Ok(Ok(good)) => good,
            Ok(Err(e)) => {
                line(&format!("error: {e:#}"));
                false
            }
            Err(p) => {
                let msg = p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
                line(&format!("internal error (please report): {msg}"));
                false
            }
        };
        line(if good { "\nBuild finished, all checks passed." } else { "\nBuild FAILED (see above)." });
        state.store(if good { OK } else { FAILED }, Ordering::Relaxed);
        ctx.request_repaint();
    });
    job
}
