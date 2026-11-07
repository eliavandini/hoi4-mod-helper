//! The Terrain level: terrain.bmp (the ground textures of the 3D map) as
//! the build makes it with painting over it, import and export of
//! terrain.bmp and heightmap.bmp, and the colour modes that show them.

use std::path::Path;
use std::sync::Arc;

use eframe::egui::{self, Color32, RichText};
use rayon::prelude::*;

use super::doc::Doc;
use super::model::{Id, MapData};
use super::render::PixelFill;
use super::{Act, Editor, WARN};
use crate::modbuild::bitmaps::{self, AUTO, TEXTURES, Texture};
use crate::modbuild::img;
use crate::modbuild::vanilla::WATER;

/// What the Terrain panel asks for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BitmapAct {
    /// The selected provinces get the brush's terrain.
    FillSelection,
    /// All painting goes (the build's terrain everywhere).
    ClearPaint,
    ExportTexture,
    ImportTexture,
    ExportHeights,
    ImportHeights,
    /// The build's heightmap again.
    DropHeights,
}

pub struct TextureUi {
    /// What the brush paints ([`AUTO`]: the build's terrain again).
    pub brush: u8,
    /// terrain.bmp as the build makes it on land, for the land cover loaded
    /// (its address).
    auto: Option<(usize, Arc<Vec<u8>>)>,
    built: Option<BuiltHeights>,
    /// Painted pixels, for a bitmap version.
    painted: Option<(u64, usize)>,
}

/// The build's heightmap for (pixel version, sea), with the land's lowest
/// and highest height (m).
type BuiltHeights = ((u64, u64), Arc<Vec<u8>>, f32, f32);

impl Default for TextureUi {
    fn default() -> Self {
        TextureUi { brush: bitmaps::T_HILLS, auto: None, built: None, painted: None }
    }
}

/// Colour of a heightmap value: blue under the water plane, grey above.
fn height_colour(v: u8) -> [u8; 3] {
    if v <= WATER {
        let t = v as f32 / WATER as f32;
        [(10.0 + 50.0 * t) as u8, (30.0 + 80.0 * t) as u8, (80.0 + 90.0 * t) as u8]
    } else {
        let g = (40 + (v as u32 - WATER as u32 - 1) * 215 / (254 - WATER as u32)) as u8;
        [g, g, g]
    }
}

/// "■ text" with the square in `colour`, as one piece of text (so a
/// wrapping row keeps them together).
pub(super) fn swatch_text(ui: &egui::Ui, colour: Color32, text: &str) -> egui::text::LayoutJob {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let mut job = egui::text::LayoutJob::default();
    job.append("■ ", 0.0, egui::TextFormat::simple(font.clone(), colour));
    job.append(text, 0.0, egui::TextFormat::simple(font, ui.visuals().text_color()));
    job
}

/// Water (sea and lakes of water) of every province.
fn water_provinces(d: &MapData) -> Vec<bool> {
    (0..d.provinces.cap() as Id).map(|p| d.is_water(p)).collect()
}

/// Water (sea and lakes of water) of every pixel.
fn water_pixels(d: &MapData, px: &[Id]) -> Vec<bool> {
    let water = water_provinces(d);
    px.par_iter().map(|&p| water.get(p as usize).copied().unwrap_or(false)).collect()
}

fn water_key(d: &MapData) -> u64 {
    d.provinces.iter().filter(|(_, p)| p.is_water()).fold(1469598103934665603u64, |h, (id, _)| (h ^ id as u64).wrapping_mul(1099511628211))
}

/// Paint `value` along the stroke a–b with radius `r` (not on the sea and
/// lakes, where terrain.bmp is always ocean). Returns the pixels changed.
pub fn paint_stroke(doc: &mut Doc, a: [f64; 2], b: [f64; 2], r: f64, value: u8) -> usize {
    let (w, h) = (doc.w as i64, doc.h as i64);
    let r = r.max(0.5);
    let x0 = ((a[0].min(b[0]) - r).floor() as i64).max(0);
    let x1 = ((a[0].max(b[0]) + r).ceil() as i64).min(w - 1);
    let y0 = ((a[1].min(b[1]) - r).floor() as i64).max(0);
    let y1 = ((a[1].max(b[1]) + r).ceil() as i64).min(h - 1);
    if x0 > x1 || y0 > y1 {
        return 0;
    }
    let labels = doc.labels.clone();
    let lab = labels.read();
    let sea = water_provinces(&doc.data);
    doc.with_paint(|p, log| {
        let mut n = 0;
        for y in y0..=y1 {
            for x in x0..=x1 {
                if super::ops::seg_dist2([x as f64 + 0.5, y as f64 + 0.5], a, b) > r * r {
                    continue;
                }
                let i = (y * w + x) as usize;
                if sea[lab.px[i] as usize] || p[i] == value {
                    continue;
                }
                log.push((i as u32, p[i], value));
                p[i] = value;
                n += 1;
            }
        }
        n
    })
}

/// Paint `value` over whole provinces (not sea and lakes).
pub fn paint_provinces(doc: &mut Doc, provs: &[Id], value: u8) -> usize {
    let labels = doc.labels.clone();
    let lab = labels.read();
    let mut px = Vec::new();
    for &p in provs {
        if !doc.data.is_water(p) {
            px.extend(doc.pixels_of(&lab, p));
        }
    }
    doc.with_paint(|p, log| {
        let mut n = 0;
        for i in px {
            if p[i] != value {
                log.push((i as u32, p[i], value));
                p[i] = value;
                n += 1;
            }
        }
        n
    })
}

/// Set the painted terrain to `new` everywhere (AUTO where it is the build's).
fn replace_paint(doc: &mut Doc, new: &[u8]) -> usize {
    doc.with_paint(|p, log| {
        let mut n = 0;
        for (i, (cur, &v)) in p.iter_mut().zip(new).enumerate() {
            if *cur != v {
                log.push((i as u32, *cur, v));
                *cur = v;
                n += 1;
            }
        }
        n
    })
}

/// A save dialog starting in the map folder.
fn save_dialog(dir: &Path, name: &str) -> Option<std::path::PathBuf> {
    rfd::FileDialog::new().set_directory(dir).set_file_name(name).add_filter("bitmap", &["bmp"]).save_file()
}

impl Editor {
    /// terrain.bmp as the build makes it on land (None until the land cover
    /// is loaded).
    pub(super) fn auto_texture(&mut self) -> Option<Arc<Vec<u8>>> {
        let t = self.doc.as_ref()?.terrain.clone()?;
        let key = Arc::as_ptr(&t) as usize;
        if let Some((k, a)) = &self.tex.auto
            && *k == key
        {
            return Some(a.clone());
        }
        let a: Arc<Vec<u8>> = Arc::new(t.cover.par_iter().zip(&t.slope_px).map(|(&c, &s)| bitmaps::auto_texture(c, s)).collect());
        self.tex.auto = Some((key, a.clone()));
        Some(a)
    }

    /// The build's heightmap (None until the heights are loaded), with the
    /// land's lowest and highest height (m).
    fn built_heights(&mut self) -> Option<(Arc<Vec<u8>>, f32, f32)> {
        let doc = self.doc.as_ref()?;
        let t = doc.terrain.clone()?;
        let key = (doc.pixel_version, water_key(&doc.data));
        if let Some((k, v, lo, hi)) = &self.tex.built
            && *k == key
        {
            return Some((v.clone(), *lo, *hi));
        }
        let water = water_pixels(&doc.data, &doc.labels.read().px);
        let (v, lo, hi) = bitmaps::heightmap(&t.dem, &water, doc.w as usize, doc.h as usize);
        let v = Arc::new(v);
        self.tex.built = Some((key, v.clone(), lo, hi));
        Some((v, lo, hi))
    }

    /// The heightmap the build uses: the imported one, else its own.
    pub(super) fn heights_now(&mut self) -> Option<Arc<Vec<u8>>> {
        match self.doc.as_ref()?.heights.clone() {
            Some(h) => Some(h),
            None => self.built_heights().map(|b| b.0),
        }
    }

    /// The per-pixel colours of the "terrain.bmp" and "heightmap" colour
    /// modes (from the caches [`Self::auto_texture`] and
    /// [`Self::heights_now`] filled).
    pub(super) fn pixel_fill(&self, doc: &Doc, heightmap: bool) -> Option<PixelFill> {
        let d = &doc.data;
        let cap = d.provinces.cap();
        if heightmap {
            let base = doc.heights.clone().or_else(|| self.tex.built.as_ref().map(|b| b.1.clone()))?;
            let colours = (0..=255u8).map(height_colour).collect();
            Some(PixelFill { base, over: None, colours, on: vec![true; cap] })
        } else {
            let base = self.tex.auto.as_ref()?.1.clone();
            let mut colours = bitmaps::vanilla_palette();
            colours.push([0, 0, 0]);
            let on = (0..cap as Id).map(|p| d.provinces.contains(p) && !d.is_water(p)).collect();
            Some(PixelFill { base, over: Some(doc.paint.clone()), colours, on })
        }
    }

    pub(super) fn painted_count(&mut self) -> usize {
        let Some(doc) = &self.doc else { return 0 };
        match self.tex.painted {
            Some((v, n)) if v == doc.bitmap_version => n,
            _ => {
                let n = doc.paint.read().par_iter().filter(|&&v| v != AUTO).count();
                self.tex.painted = Some((doc.bitmap_version, n));
                n
            }
        }
    }

    /// terrain.bmp under a pixel: (index, painted); None off the map or
    /// without the land cover.
    fn texture_at(&self, px: [f64; 2]) -> Option<(u8, bool)> {
        let doc = self.doc.as_ref()?;
        if px[0] < 0.0 || px[1] < 0.0 || px[0] >= doc.w as f64 || px[1] >= doc.h as f64 {
            return None;
        }
        let i = px[1] as usize * doc.w as usize + px[0] as usize;
        if doc.data.is_water(doc.labels.read().px[i]) {
            return Some((bitmaps::T_OCEAN, false));
        }
        match doc.paint.read()[i] {
            AUTO => self.tex.auto.as_ref().map(|a| (a.1[i], false)),
            v => Some((v, true)),
        }
    }

    /// Alt+click on the Terrain level: the brush takes the terrain there.
    pub(super) fn pick_texture(&mut self, px: [f64; 2]) {
        match self.texture_at(px) {
            Some((bitmaps::T_OCEAN, false)) => self.info("the sea and lakes are always ocean in terrain.bmp"),
            Some((v, _)) => {
                self.tex.brush = v;
                self.info(format!("brush: {}", bitmaps::texture_name(v)));
            }
            None => {}
        }
    }

    pub(super) fn texture_hover_rows(&self, ui: &mut egui::Ui, h: [f64; 2], heights: bool) {
        if heights {
            let Some(doc) = &self.doc else { return };
            let i = h[1] as usize * doc.w as usize + h[0] as usize;
            let (v, how) = match (&doc.heights, &self.tex.built) {
                (Some(m), _) => (m.get(i).copied(), "imported".to_string()),
                (None, Some((_, b, lo, hi))) => {
                    let v = b.get(i).copied();
                    let m = v.filter(|&v| v > WATER).map(|v| lo + (v - WATER - 1) as f32 * bitmaps::metres_per_step(*lo, *hi));
                    (v, m.map_or("the build's".into(), |m| format!("the build's, ≈ {m:.0} m")))
                }
                _ => (None, String::new()),
            };
            if let Some(v) = v {
                ui.label("heightmap");
                ui.label(format!("{v}{} ({how})", if v <= WATER { " · under water" } else { "" }));
                ui.end_row();
            }
            return;
        }
        if let Some((v, painted)) = self.texture_at(h) {
            ui.label("terrain.bmp");
            ui.label(format!("{} ({})", bitmaps::texture_name(v), if painted { "painted" } else { "as built" }));
            ui.end_row();
        }
    }

    /// The Terrain level's panel.
    pub(super) fn texture_panel(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        let auto = self.auto_texture();
        let painted = self.painted_count();
        let nsel = self.sel.len();
        let Some(doc) = &self.doc else { return };
        let imported = doc.heights.is_some();
        let has_heights = doc.terrain.is_some();
        egui::CollapsingHeader::new("Terrain (terrain.bmp)").id_salt("texture").default_open(true).show(ui, |ui| {
            ui.label(
                RichText::new(
                    "terrain.bmp sets the ground textures and colours of the 3D map; the build makes it from the land cover and the slope, and puts trees where it has forest. \
                     Paint over it with the Paint tool (B); Alt+click picks the terrain under the cursor. The sea is always ocean. \
                     It is only the look: the terrain units fight in is the province's (Mod details → Terrain types).",
                )
                .small()
                .weak(),
            );
            ui.horizontal(|ui| {
                ui.label("Brush:");
                ui.label(RichText::new(if self.tex.brush == AUTO { "as built (removes painting)".to_string() } else { bitmaps::texture_name(self.tex.brush) }).strong());
            });
            let brush = &mut self.tex.brush;
            let pal = bitmaps::vanilla_palette();
            let item = |ui: &mut egui::Ui, brush: &mut u8, t: &Texture| {
                let c = pal[t.index as usize];
                let text = swatch_text(ui, Color32::from_rgb(c[0], c[1], c[2]), &format!("{} {}", t.index, t.name));
                ui.selectable_value(brush, t.index, text).on_hover_text(format!("00_terrain.txt: {}, type {}", t.key, t.kind));
            };
            ui.horizontal_wrapped(|ui| {
                ui.selectable_value(brush, AUTO, "as built").on_hover_text("the brush takes painting away: the build decides there again");
                for t in TEXTURES.iter().take(9) {
                    item(ui, brush, t);
                }
            });
            egui::CollapsingHeader::new("other vanilla terrains").id_salt("texture_more").show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for t in TEXTURES.iter().skip(9).filter(|t| t.land) {
                        item(ui, brush, t);
                    }
                });
            });
            ui.horizontal_wrapped(|ui| {
                let what = if self.tex.brush == AUTO { "Back to as built" } else { "Paint them" };
                if ui.add_enabled(nsel > 0, egui::Button::new(format!("{what} ({nsel} selected)"))).on_hover_text("the selected provinces get the brush's terrain").clicked() {
                    acts.push(Act::Bitmap(BitmapAct::FillSelection));
                }
                ui.label(RichText::new(format!("{painted} px painted")).small());
                if ui.add_enabled(painted > 0, egui::Button::new("Clear all painting")).clicked() {
                    acts.push(Act::Bitmap(BitmapAct::ClearPaint));
                }
            });
            if auto.is_none() {
                ui.colored_label(WARN, "the land cover is not loaded yet: the terrain as built shows once it is");
            }
            ui.add_space(4.0);
            ui.label(RichText::new("Import and export").strong());
            ui.label(
                RichText::new(
                    "Export writes the bitmap the build would make (with your painting or imported heightmap), to edit in an image editor. \
                     Import takes one back (the map's size; terrain.bmp as an 8-bit bitmap with the game's palette): pixels that differ from the build's terrain become painted, \
                     and an imported heightmap is used instead of the one from dem_section.tif. Both are one undo step and are saved with the map.",
                )
                .small()
                .weak(),
            );
            egui::Grid::new("bitmap_io").num_columns(2).show(ui, |ui| {
                ui.label("terrain.bmp");
                ui.horizontal(|ui| {
                    if ui.add_enabled(auto.is_some(), egui::Button::new("Export…")).clicked() {
                        acts.push(Act::Bitmap(BitmapAct::ExportTexture));
                    }
                    if ui.add_enabled(auto.is_some(), egui::Button::new("Import…")).clicked() {
                        acts.push(Act::Bitmap(BitmapAct::ImportTexture));
                    }
                });
                ui.end_row();
                ui.label("heightmap.bmp");
                ui.horizontal(|ui| {
                    if ui.add_enabled(has_heights || imported, egui::Button::new("Export…")).clicked() {
                        acts.push(Act::Bitmap(BitmapAct::ExportHeights));
                    }
                    if ui.button("Import…").clicked() {
                        acts.push(Act::Bitmap(BitmapAct::ImportHeights));
                    }
                    if imported && ui.button("Use the build's").on_hover_text("drop the imported heightmap").clicked() {
                        acts.push(Act::Bitmap(BitmapAct::DropHeights));
                    }
                });
                ui.end_row();
            });
            ui.label(
                RichText::new(if imported {
                    "heightmap: imported (the build fits it to the sea: land above the water plane, sea under it)"
                } else {
                    "heightmap: the build's, from dem_section.tif"
                })
                .small()
                .weak(),
            );
            ui.label(RichText::new("Colour the map by \"terrain.bmp\" or \"heightmap.bmp\" to see them.").small().weak());
        });
    }

    pub(super) fn bitmap_act(&mut self, act: BitmapAct) {
        let Some(dir) = self.doc.as_ref().map(|d| d.dir.clone()) else { return };
        let brush = self.tex.brush;
        match act {
            BitmapAct::FillSelection => {
                let provs = self.sel_provinces();
                let what = if brush == AUTO { "as built".to_string() } else { bitmaps::texture_name(brush) };
                self.run("Paint terrain.bmp", |d, _| Ok(format!("{} px of {} provinces now {what}", paint_provinces(d, &provs, brush), provs.len())));
            }
            BitmapAct::ClearPaint => {
                let Some((w, h)) = self.doc.as_ref().map(|d| (d.w, d.h)) else { return };
                self.run("Clear painted terrain", |d, _| Ok(format!("{} painted px are as built again", replace_paint(d, &vec![AUTO; (w * h) as usize]))));
            }
            BitmapAct::ExportTexture => {
                let Some(path) = save_dialog(&dir, "terrain.bmp") else { return };
                match self.export_texture(&path) {
                    Ok(m) => self.info(m),
                    Err(e) => self.error(format!("export terrain.bmp: {e:#}")),
                }
            }
            BitmapAct::ImportTexture => {
                let Some(path) = rfd::FileDialog::new().set_directory(&dir).add_filter("bitmap", &["bmp"]).pick_file() else { return };
                self.import_texture(&path);
            }
            BitmapAct::ExportHeights => {
                let Some(path) = save_dialog(&dir, "heightmap.bmp") else { return };
                match self.export_heights(&path) {
                    Ok(m) => self.info(m),
                    Err(e) => self.error(format!("export heightmap.bmp: {e:#}")),
                }
            }
            BitmapAct::ImportHeights => {
                let Some(path) = rfd::FileDialog::new().set_directory(&dir).add_filter("image", &["bmp", "png", "tif", "tiff"]).pick_file() else { return };
                self.import_heights(&path);
            }
            BitmapAct::DropHeights => self.run("Use the build's heightmap", |d, _| {
                d.set_heights(None);
                Ok("the build makes the heightmap from dem_section.tif again".into())
            }),
        }
    }

    /// Write terrain.bmp as the build would make it.
    pub(super) fn export_texture(&mut self, path: &Path) -> anyhow::Result<String> {
        anyhow::ensure!(self.auto_texture().is_some(), "the land cover is not loaded");
        let palette = bitmaps::game_palette(&self.dirs().game);
        let doc = self.doc.as_ref().unwrap();
        let t = doc.terrain.clone().unwrap();
        let water = water_pixels(&doc.data, &doc.labels.read().px);
        let out = bitmaps::final_texture(&t.cover, &t.slope_px, &water, Some(&doc.paint.read()));
        img::write_bmp8(path, doc.w, doc.h, &out, &palette)?;
        Ok(format!("wrote {} (terrain.bmp as the build makes it)", path.display()))
    }

    /// Paint what an image has that differs from the build's terrain.
    pub(super) fn import_texture(&mut self, path: &Path) {
        let Some(auto) = self.auto_texture() else {
            self.error("import terrain.bmp: the land cover is not loaded yet");
            return;
        };
        let palette = bitmaps::game_palette(&self.dirs().game);
        let read = bitmaps::read_texture(path, &palette);
        let name = path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        let mut unknown = 0;
        self.run(&format!("Import {name}"), |d, _| {
            let (w, h, v) = read?;
            anyhow::ensure!((w, h) == (d.w, d.h), "it is {w}×{h} px, the map {}×{}", d.w, d.h);
            let sea = water_pixels(&d.data, &d.labels.read().px);
            let cur = d.paint.read().clone();
            let new: Vec<u8> = (0..v.len()).into_par_iter().map(|i| if sea[i] { cur[i] } else if v[i] == auto[i] { AUTO } else { v[i] }).collect();
            unknown = new.par_iter().filter(|&&x| x != AUTO && bitmaps::texture(x).is_none_or(|t| !t.land)).count();
            let changed = replace_paint(d, &new);
            let painted = new.par_iter().zip(&sea).filter(|(x, s)| **x != AUTO && !**s).count();
            Ok(format!("imported {name}: {painted} px differ from the build's terrain and are painted ({changed} px changed)"))
        });
        if unknown > 0 {
            self.error(format!("{unknown} imported px are not a vanilla land terrain (ocean, lakes or an index 00_terrain.txt does not define)"));
        }
    }

    /// Write the heightmap the build would use.
    pub(super) fn export_heights(&mut self, path: &Path) -> anyhow::Result<String> {
        let h = self.heights_now().ok_or_else(|| anyhow::anyhow!("the heights are not loaded"))?;
        let doc = self.doc.as_ref().unwrap();
        // what the build writes: an imported one fitted to the sea
        let v: Vec<u8> = match &doc.heights {
            Some(imp) => {
                let sea = water_pixels(&doc.data, &doc.labels.read().px);
                match self.tex.built.as_ref().filter(|_| doc.terrain.is_some()) {
                    Some((_, built, _, _)) => bitmaps::fit_to_sea(imp, built, &sea).0,
                    None => imp.as_ref().clone(),
                }
            }
            None => h.as_ref().clone(),
        };
        img::write_bmp_gray(path, doc.w, doc.h, &v)?;
        Ok(format!("wrote {} ({})", path.display(), if doc.heights.is_some() { "the imported heightmap" } else { "the build's heightmap" }))
    }

    /// Use an image as the heightmap.
    pub(super) fn import_heights(&mut self, path: &Path) {
        let read = bitmaps::read_grey(path);
        let name = path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        let mut misfit = (0, 0);
        self.run(&format!("Import {name} as the heightmap"), |d, _| {
            let (w, h, v) = read?;
            anyhow::ensure!((w, h) == (d.w, d.h), "it is {w}×{h} px, the map {}×{}", d.w, d.h);
            let sea = water_pixels(&d.data, &d.labels.read().px);
            misfit = (0..v.len()).fold((0, 0), |(l, s), i| (l + (!sea[i] && v[i] <= WATER) as usize, s + (sea[i] && v[i] >= WATER) as usize));
            d.set_heights(Some(Arc::new(v)));
            Ok(format!("imported {name} as the heightmap; it is saved with the map and the build uses it"))
        });
        if misfit.0 + misfit.1 > 0 {
            self.error(format!(
                "{} land px are at or under the water plane ({WATER}) and {} px of sea or lakes above it: the build moves them (land to {}, water to its own depth)",
                misfit.0,
                misfit.1,
                WATER + 1
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::{Value, json};

    use super::super::density::Terrain;
    use super::super::model::{self, Labels, Loaded};
    use super::super::{Level, Settings};
    use super::*;
    use crate::modbuild::bitmaps::{CROP, T_FARM, T_HILLS, T_OCEAN};

    fn from<T: serde::de::DeserializeOwned>(v: Value) -> T {
        serde_json::from_value(v).unwrap()
    }

    fn out_dir(name: &str) -> PathBuf {
        let base = std::env::var_os("MOD_HELPER_TEST_OUT").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let d = base.join(format!("mod-helper-test-{name}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// 4×2 px: land province 1 on the left half, sea province 2 on the
    /// right; flat cropland (the build's terrain: farmland).
    fn small(dir: PathBuf) -> Doc {
        let mut data = MapData { pixel_m: 100.0, ..Default::default() };
        data.provinces.put(1, from(json!({"id": 1, "rgb": [10, 0, 0], "type": "land", "name": "land"})));
        data.provinces.put(2, from(json!({"id": 2, "rgb": [0, 0, 90], "type": "sea", "name": "sea"})));
        let geo = crate::geo::FileGeoInfo { transform: None, crs: None, nodata: None };
        let px = vec![1, 1, 2, 2, 1, 1, 2, 2];
        let mut doc = Doc::new(dir, Loaded { data, labels: Labels { w: 4, h: 2, px }, geo, extra: Default::default() });
        let n = 8;
        let dem = vec![500.0, 900.0, 0.0, 0.0, 700.0, 1300.0, 0.0, 0.0];
        doc.terrain = Some(Arc::new(Terrain { w: 4, h: 2, dem, slope: vec![0; n], urban: vec![0; n], sealed: vec![false; n], cover: vec![CROP; n], slope_px: vec![2.0; n] }));
        doc
    }

    fn paint(ed: &Editor) -> Vec<u8> {
        ed.doc.as_ref().unwrap().paint.read().clone()
    }

    #[test]
    fn paint_export_import_and_save() {
        let dir = out_dir("texture-small");
        let mut ed = Editor::new(Settings::default());
        ed.doc = Some(small(dir.clone()));
        ed.settings.level = Level::Terrain;
        assert_eq!(ed.auto_texture().unwrap()[0], T_FARM);

        // a stroke over the whole map paints the land only
        let doc = ed.doc.as_mut().unwrap();
        doc.begin("Paint terrain.bmp");
        assert_eq!(paint_stroke(doc, [0.0, 0.0], [4.0, 2.0], 5.0, T_HILLS), 4);
        doc.commit();
        let a = AUTO;
        assert_eq!(paint(&ed), [T_HILLS, T_HILLS, a, a, T_HILLS, T_HILLS, a, a]);
        ed.doc.as_mut().unwrap().undo();
        assert_eq!(paint(&ed), [a; 8]);
        ed.doc.as_mut().unwrap().redo();
        assert_eq!(paint(&ed)[0], T_HILLS);
        assert_eq!(ed.texture_at([0.5, 0.5]), Some((T_HILLS, true)));
        assert_eq!(ed.texture_at([2.5, 0.5]), Some((T_OCEAN, false)));

        // export: what the build writes; imported back it is all as painted
        let f = dir.join("export.bmp");
        ed.export_texture(&f).unwrap();
        let b = img::read_bmp(&f).unwrap();
        assert_eq!(b.data, [T_HILLS, T_HILLS, T_OCEAN, T_OCEAN, T_HILLS, T_HILLS, T_OCEAN, T_OCEAN]);
        ed.tex.brush = AUTO;
        ed.sel = vec![1];
        ed.bitmap_act(BitmapAct::FillSelection);
        assert_eq!(paint(&ed), [a; 8], "the selection back to as built");
        ed.import_texture(&f);
        assert_eq!(paint(&ed), [T_HILLS, T_HILLS, a, a, T_HILLS, T_HILLS, a, a]);
        // an image like the build's terrain but for one marsh pixel paints only that one
        let mut v = b.data.clone();
        v[..2].copy_from_slice(&[T_FARM, 9]);
        v[4..6].copy_from_slice(&[T_FARM, T_FARM]);
        img::write_bmp8(&f, 4, 2, &v, &bitmaps::vanilla_palette()).unwrap();
        ed.import_texture(&f);
        assert_eq!(paint(&ed), [a, 9, a, a, a, a, a, a]);

        // heightmap: the build's, exported and imported back
        let hf = dir.join("heights.bmp");
        ed.export_heights(&hf).unwrap();
        let built = img::read_bmp(&hf).unwrap().data;
        assert_eq!(built, [WATER + 1, 176, 91, 90, 136, 255, 91, 90], "land from the lowest (96) to the highest (255)");
        assert!(built[2] < WATER && built[3] < WATER);
        let mut mine = built.clone();
        mine[5] = 200;
        img::write_bmp_gray(&hf, 4, 2, &mine).unwrap();
        ed.import_heights(&hf);
        assert_eq!(ed.doc.as_ref().unwrap().heights.as_deref(), Some(&mine));

        // saved with the map and read back
        ed.doc.as_mut().unwrap().save().unwrap();
        let l = model::load(&dir, &Default::default()).unwrap();
        assert_eq!(l.extra.paint.as_deref(), Some(&paint(&ed)[..]));
        assert_eq!(l.extra.heights.as_deref(), Some(&mine[..]));
        // the build's heightmap again (undo brings the import back); clear the painting
        ed.bitmap_act(BitmapAct::DropHeights);
        assert!(ed.doc.as_ref().unwrap().heights.is_none());
        ed.doc.as_mut().unwrap().undo();
        assert!(ed.doc.as_ref().unwrap().heights.is_some());
        ed.doc.as_mut().unwrap().redo();
        ed.bitmap_act(BitmapAct::ClearPaint);
        assert_eq!(ed.painted_count(), 0);
        ed.doc.as_mut().unwrap().save().unwrap();
        assert!(!dir.join(bitmaps::PAINT_FILE).exists() && !dir.join(bitmaps::HEIGHTS_FILE).exists(), "files left");
        let l = model::load(&dir, &Default::default()).unwrap();
        assert!(l.extra.paint.is_none() && l.extra.heights.is_none());
        // the backup of the save before has them
        let b = model::backups(&dir);
        assert!(b[0].dir.join(bitmaps::PAINT_FILE).exists() && b[0].dir.join(bitmaps::HEIGHTS_FILE).exists());
        // loading that backup brings them back, as one edit
        let old = model::load(&b[0].dir, &Default::default()).unwrap();
        let doc = ed.doc.as_mut().unwrap();
        doc.begin("Load backup");
        doc.replace_map(old).unwrap();
        doc.commit();
        assert_eq!(paint(&ed)[1], 9);
        assert!(ed.doc.as_ref().unwrap().heights.is_some());
        ed.doc.as_mut().unwrap().undo();
        assert_eq!(paint(&ed), [a; 8]);
        assert!(ed.doc.as_ref().unwrap().heights.is_none());
    }

    #[test]
    fn colour_modes_show_the_pixels() {
        let mut ed = Editor::new(Settings::default());
        ed.doc = Some(small(PathBuf::from("/nonexistent")));
        ed.settings.level = Level::Terrain;
        ed.refresh_style();
        let pf = ed.style.pixels.as_ref().expect("no pixels in the terrain.bmp mode");
        assert!(pf.on[1] && !pf.on[2]);
        let img = super::super::render::render(&ed.doc.as_ref().unwrap().labels.read(), &ed.style, 0, [0, 0, 4, 2]);
        let pal = bitmaps::vanilla_palette();
        let c = |k: usize| [img.pixels[k].r(), img.pixels[k].g(), img.pixels[k].b()];
        assert_eq!(c(0), pal[T_FARM as usize]);
        // painted: shown after the stroke
        let doc = ed.doc.as_mut().unwrap();
        doc.begin("Paint terrain.bmp");
        paint_stroke(doc, [0.5, 0.5], [0.5, 0.5], 0.5, 9);
        doc.commit();
        ed.refresh_style();
        let img = super::super::render::render(&ed.doc.as_ref().unwrap().labels.read(), &ed.style, 0, [0, 0, 4, 2]);
        assert_eq!([img.pixels[0].r(), img.pixels[0].g(), img.pixels[0].b()], pal[9]);
        ed.settings.mode = super::super::MapMode::Heightmap;
        ed.refresh_style();
        assert!(ed.style.pixels.as_ref().is_some_and(|p| p.on[2]));
    }
}
