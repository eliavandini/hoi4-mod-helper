//! Pictures of the content screen: a sprite (`GFX_goal_x`) is looked up in
//! the project's `.gfx` files, then in the sources (the base game, the mod
//! being imported from), its texture read from where it is (a package, the
//! source, the base game) and decoded (DDS, TGA, PNG, JPEG) in the
//! background into an egui texture. Multi-frame sprites show their first
//! frame.

use std::collections::HashMap;
use std::sync::Arc;

use crossbeam_channel::{Receiver, Sender};
use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};

use super::kinds::Kind;
use super::source::{self, Index};
use super::store::Store;
use crate::modbuild::img;

pub enum Slot {
    Loading,
    Ready { tex: TextureHandle },
    Missing(String),
}

/// A decoded picture: RGBA pixels.
pub fn decode(bytes: &[u8], name: &str) -> Result<(u32, u32, Vec<u8>), String> {
    let lower = name.to_ascii_lowercase();
    let r = if bytes.starts_with(b"DDS ") {
        img::read_dds_bytes(bytes, name)
    } else if lower.ends_with(".tga") {
        img::read_tga_bytes(bytes, name)
    } else {
        image::load_from_memory(bytes).map(|i| {
            let i = i.to_rgba8();
            (i.width(), i.height(), i.into_raw())
        }).map_err(|e| anyhow::anyhow!("{name}: {e}"))
    };
    r.map_err(|e| format!("{e:#}"))
}

/// The first of `frames` frames side by side, scaled down to at most `max` px.
fn first_frame(w: u32, h: u32, px: &[u8], frames: u32, max: u32) -> ColorImage {
    let fw = (w / frames.max(1)).max(1);
    let step = (fw.max(h) as f32 / max as f32).max(1.0);
    let (ow, oh) = (((fw as f32 / step).round() as usize).max(1), ((h as f32 / step).round() as usize).max(1));
    let mut out = vec![0u8; ow * oh * 4];
    for y in 0..oh {
        let sy = ((y as f32 * step) as u32).min(h - 1);
        for x in 0..ow {
            let sx = ((x as f32 * step) as u32).min(fw - 1);
            let i = ((sy * w + sx) * 4) as usize;
            let o = (y * ow + x) * 4;
            out[o..o + 4].copy_from_slice(&px[i..i + 4]);
        }
    }
    ColorImage::from_rgba_unmultiplied([ow, oh], &out)
}

/// Where a sprite's picture is: the texture path and the bytes.
pub struct Found {
    pub texture: String,
    pub frames: u32,
    pub bytes: Option<Arc<Vec<u8>>>,
}

/// The sprites the project's packages define: name → (package, texture, frames).
pub fn store_sprites(st: &Store) -> HashMap<String, (String, String, u32)> {
    let mut out = HashMap::new();
    for k in st.defs_of(None, Kind::Sprite) {
        if let Some((d, _, s)) = st.def(&k)
            && let Some(b) = d.block(s)
        {
            let tex = b.atom("texturefile").or_else(|| b.atom("textureFile")).or_else(|| b.atom("textureFile1")).unwrap_or("").replace('\\', "/");
            let frames = b.num("noOfFrames").unwrap_or(1.0).max(1.0) as u32;
            out.entry(k.id.clone()).or_insert((k.pkg.clone(), tex, frames));
        }
    }
    out
}

/// A texture path in a package, tolerant about the extension.
fn store_texture(st: &Store, tex: &str) -> Option<(String, String)> {
    let stem = tex.rsplit_once('.').map(|x| x.0).unwrap_or(tex);
    let cands: Vec<String> = std::iter::once(tex.to_string()).chain(["dds", "tga", "png"].iter().map(|e| format!("{stem}.{e}"))).collect();
    for (name, p) in &st.packages {
        for c in &cands {
            if p.files.contains_key(c) {
                return Some((name.clone(), c.clone()));
            }
        }
    }
    None
}

/// Look a sprite up: the project's sprites, then each source in turn; the
/// texture from the project, then the source that names it, then the others.
pub fn find(name: &str, st: &Store, own: &HashMap<String, (String, String, u32)>, sources: &[&Index]) -> Option<Found> {
    let (texture, frames) = if let Some((_, tex, frames)) = own.get(name) {
        (tex.clone(), *frames)
    } else {
        sources.iter().find_map(|s| s.sprites.get(name).cloned())?
    };
    // a texture path straight away (portraits are often given as files)
    let bytes = store_texture(st, &texture)
        .and_then(|(pkg, p)| st.bytes(&pkg, &p))
        .or_else(|| sources.iter().find_map(|s| source::texture_path(&s.files, &texture).and_then(|p| s.files.read(&p))));
    Some(Found { texture, frames, bytes })
}

/// Texture bytes for a path (a portrait given as a file).
pub fn find_file(path: &str, st: &Store, sources: &[&Index]) -> Option<Found> {
    let bytes = store_texture(st, path)
        .and_then(|(pkg, p)| st.bytes(&pkg, &p))
        .or_else(|| sources.iter().find_map(|s| source::texture_path(&s.files, path).and_then(|p| s.files.read(&p))))?;
    Some(Found { texture: path.to_string(), frames: 1, bytes: Some(bytes) })
}

pub struct Gfx {
    slots: HashMap<String, Slot>,
    tx: Sender<(String, Result<ColorImage, String>)>,
    rx: Receiver<(String, Result<ColorImage, String>)>,
    /// The project's sprites, at a generation of the store.
    own: (u64, Arc<HashMap<String, (String, String, u32)>>),
    pending: usize,
}

impl Default for Gfx {
    fn default() -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        Gfx { slots: HashMap::new(), tx, rx, own: (0, Arc::new(HashMap::new())), pending: 0 }
    }
}

/// Largest side of a decoded picture (they are shown small).
const MAX_SIDE: u32 = 512;

impl Gfx {
    /// Forget everything (the sources or the project's pictures changed).
    pub fn clear(&mut self) {
        self.slots.clear();
        self.own.0 = 0;
    }

    /// Pictures still being decoded.
    pub fn busy(&self) -> bool {
        self.pending > 0
    }

    fn poll(&mut self, ctx: &egui::Context) {
        while let Ok((key, r)) = self.rx.try_recv() {
            self.pending = self.pending.saturating_sub(1);
            let slot = match r {
                Ok(img) => Slot::Ready { tex: ctx.load_texture(format!("content:{key}"), img, TextureOptions::LINEAR) },
                Err(e) => Slot::Missing(e),
            };
            self.slots.insert(key, slot);
        }
    }

    /// The project's sprites (rebuilt when the store changed).
    pub fn own(&mut self, st: &Store) -> Arc<HashMap<String, (String, String, u32)>> {
        if self.own.0 != st.generation {
            let fresh = Arc::new(store_sprites(st));
            // pictures that were missing may be there now
            if *fresh != *self.own.1 {
                self.slots.retain(|_, s| !matches!(s, Slot::Missing(_)));
            }
            self.own = (st.generation, fresh);
        }
        self.own.1.clone()
    }

    /// The picture of a sprite (or of a texture file when `name` has a `/`).
    pub fn get(&mut self, ctx: &egui::Context, name: &str, st: &Store, sources: &[&Index]) -> &Slot {
        self.poll(ctx);
        if !self.slots.contains_key(name) {
            let own = self.own(st);
            let found = if name.contains('/') { find_file(name, st, sources) } else { find(name, st, &own, sources) };
            let slot = match found {
                None => Slot::Missing(format!("no sprite {name}")),
                Some(Found { bytes: None, texture, .. }) => Slot::Missing(format!("{name}: no picture {texture}")),
                Some(Found { bytes: Some(b), texture, frames, .. }) => {
                    let (tx, key, c) = (self.tx.clone(), name.to_string(), ctx.clone());
                    self.pending += 1;
                    rayon::spawn(move || {
                        let r = decode(&b, &texture).map(|(w, h, px)| first_frame(w, h, &px, frames, MAX_SIDE));
                        let _ = tx.send((key, r));
                        c.request_repaint();
                    });
                    Slot::Loading
                }
            };
            self.slots.insert(name.to_string(), slot);
        }
        &self.slots[name]
    }

    /// The texture of a sprite when it is ready.
    pub fn texture(&mut self, ctx: &egui::Context, name: &str, st: &Store, sources: &[&Index]) -> Option<(egui::TextureId, egui::Vec2)> {
        match self.get(ctx, name, st, sources) {
            Slot::Ready { tex, .. } => Some((tex.id(), tex.size_vec2())),
            _ => None,
        }
    }

    /// Why a sprite shows nothing (None: it does, or is loading).
    pub fn problem(&mut self, ctx: &egui::Context, name: &str, st: &Store, sources: &[&Index]) -> Option<String> {
        match self.get(ctx, name, st, sources) {
            Slot::Missing(e) => Some(e.clone()),
            _ => None,
        }
    }
}

/// A picture fitted into `size` (keeping its shape), or a frame with a
/// question mark when there is none.
pub fn show(ui: &mut egui::Ui, tex: Option<(egui::TextureId, egui::Vec2)>, size: egui::Vec2) -> egui::Response {
    match tex {
        Some((id, s)) => {
            let k = (size.x / s.x).min(size.y / s.y);
            ui.add(egui::Image::new((id, s * k)))
        }
        None => {
            let (r, resp) = ui.allocate_exact_size(size, egui::Sense::hover());
            ui.painter().rect_stroke(r, 3.0, ui.visuals().widgets.noninteractive.bg_stroke, egui::StrokeKind::Inside);
            ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, "?", egui::FontId::proportional(size.y * 0.4), ui.visuals().weak_text_color());
            resp
        }
    }
}
