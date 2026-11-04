//! Tiled, multi-resolution display of rasters.
//!
//! Each raster is displayed through a pyramid of 512×512 tiles: level `L`
//! covers `512·2^L` source pixels per tile side. Tiles are rendered on a
//! background thread pool on demand, uploaded as GPU textures and evicted in
//! LRU order once the texture budget is exceeded. This keeps huge rasters
//! (e.g. a 40000² land-cover map) responsive and bounded in GPU memory.

use crossbeam_channel::{Receiver, Sender};
use eframe::egui::{self, ColorImage, TextureFilter, TextureHandle, TextureOptions, TextureWrapMode};
use rustc_hash::FxHashMap;

pub const TILE: u32 = 512;

/// Renders one tile on a worker thread.
pub type TileJob = Box<dyn FnOnce() -> ColorImage + Send + 'static>;

/// Anything drawn through the tile pyramid: a raster layer, the map editor overlay.
pub trait TileSource {
    fn size(&self) -> (u32, u32);
    fn max_level(&self) -> u8;
    /// Bumped whenever the rendering changes (invalidates tiles).
    fn generation(&self) -> u64;
    /// Job rendering source pixels `[x0,x1)×[y0,y1)` at `level`.
    fn job(&self, level: u8, region: [u32; 4]) -> TileJob;
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct TileKey {
    pub layer: u64,
    pub level: u8,
    pub tx: u32,
    pub ty: u32,
}

pub struct Tile {
    pub tex: TextureHandle,
    /// Style generation the tile was rendered with.
    pub generation: u64,
    last_used: u64,
    bytes: usize,
    pinned: bool,
}

struct Done {
    key: TileKey,
    generation: u64,
    pinned: bool,
    image: ColorImage,
}

pub struct TileManager {
    tiles: FxHashMap<TileKey, Tile>,
    pending: FxHashMap<TileKey, u64>,
    tx: Sender<Done>,
    rx: Receiver<Done>,
    pool: rayon::ThreadPool,
    frame: u64,
    used_bytes: usize,
    pub budget_bytes: usize,
    max_inflight: usize,
}

const OPTIONS: TextureOptions = TextureOptions {
    magnification: TextureFilter::Nearest,
    minification: TextureFilter::Linear,
    wrap_mode: TextureWrapMode::ClampToEdge,
    mipmap_mode: None,
};

/// Smallest pyramid level at which the whole raster fits in one tile.
pub fn max_level(w: u32, h: u32) -> u8 {
    let mut l = 0;
    while (w.max(h) >> l) > TILE {
        l += 1;
    }
    l
}

impl TileManager {
    pub fn new() -> Self {
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(2, 8);
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .thread_name(|i| format!("tile-{i}"))
            .build()
            .expect("tile thread pool");
        let (tx, rx) = crossbeam_channel::unbounded();
        TileManager {
            tiles: FxHashMap::default(),
            pending: FxHashMap::default(),
            tx,
            rx,
            pool,
            frame: 0,
            used_bytes: 0,
            budget_bytes: 1024 << 20,
            max_inflight: threads * 2,
        }
    }

    /// Upload finished tiles. `live` maps layer id → current style generation;
    /// results for removed layers or outdated generations are dropped.
    pub fn begin_frame(&mut self, ctx: &egui::Context, live: &FxHashMap<u64, u64>) {
        self.frame += 1;
        let mut uploaded = 0;
        while uploaded < 48 {
            let Ok(done) = self.rx.try_recv() else { break };
            if self.pending.get(&done.key) == Some(&done.generation) {
                self.pending.remove(&done.key);
            }
            if live.get(&done.key.layer) != Some(&done.generation) {
                continue;
            }
            let bytes = done.image.pixels.len() * 4;
            let k = done.key;
            let name = format!("tile-{}-{}-{}-{}", k.layer, k.level, k.tx, k.ty);
            let tex = ctx.load_texture(name, done.image, OPTIONS);
            let tile = Tile { tex, generation: done.generation, last_used: self.frame, bytes, pinned: done.pinned };
            if let Some(old) = self.tiles.insert(done.key, tile) {
                self.used_bytes -= old.bytes;
            }
            self.used_bytes += bytes;
            uploaded += 1;
        }
        if !self.rx.is_empty() {
            ctx.request_repaint();
        }
    }

    pub fn get(&mut self, key: &TileKey) -> Option<&Tile> {
        let frame = self.frame;
        self.tiles.get_mut(key).map(|t| {
            t.last_used = frame;
            &*t
        })
    }

    pub fn is_pending(&self, key: &TileKey) -> bool {
        self.pending.contains_key(key)
    }

    pub fn can_request(&self) -> bool {
        self.pending.len() < self.max_inflight
    }

    /// Queue rendering of source pixels `region` of `src` at the key's level.
    /// Returns false if the queue is full.
    pub fn request(&mut self, ctx: &egui::Context, key: TileKey, pinned: bool, src: &dyn TileSource, region: [u32; 4]) -> bool {
        let generation = src.generation();
        if self.pending.get(&key) == Some(&generation) {
            return true;
        }
        if !self.can_request() {
            return false;
        }
        self.pending.insert(key, generation);
        let job = src.job(key.level, region);
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        self.pool.spawn(move || {
            let image = job();
            let _ = tx.send(Done { key, generation, pinned, image });
            ctx.request_repaint();
        });
        true
    }

    /// Request the coarsest tile of a source if it is missing or outdated, so it is
    /// ready as a fallback (e.g. when a layer is shown again).
    pub fn warm_overview(&mut self, ctx: &egui::Context, layer: u64, src: &dyn TileSource) {
        let key = TileKey { layer, level: src.max_level(), tx: 0, ty: 0 };
        let (w, h) = src.size();
        if self.get(&key).is_none_or(|t| t.generation != src.generation()) && self.can_request() {
            self.request(ctx, key, true, src, [0, 0, w, h]);
        }
    }

    /// Evict least recently used tiles above the budget.
    pub fn end_frame(&mut self) {
        if self.used_bytes <= self.budget_bytes {
            return;
        }
        let mut candidates: Vec<(u64, TileKey)> = self
            .tiles
            .iter()
            .filter(|(_, t)| !t.pinned && t.last_used + 1 < self.frame)
            .map(|(k, t)| (t.last_used, *k))
            .collect();
        candidates.sort_unstable_by_key(|c| c.0);
        let target = self.budget_bytes * 9 / 10;
        for (_, key) in candidates {
            if self.used_bytes <= target {
                break;
            }
            if let Some(t) = self.tiles.remove(&key) {
                self.used_bytes -= t.bytes;
            }
        }
    }

    pub fn remove_layer(&mut self, layer: u64) {
        let used = &mut self.used_bytes;
        self.tiles.retain(|k, t| {
            if k.layer == layer {
                *used -= t.bytes;
                false
            } else {
                true
            }
        });
        self.pending.retain(|k, _| k.layer != layer);
    }

    pub fn stats(&self) -> (usize, usize, usize) {
        (self.tiles.len(), self.used_bytes, self.pending.len())
    }
}
