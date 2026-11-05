//! "Province from image": one new province with the shape of a colour
//! channel of an image (every map pixel where the channel is at or above a
//! threshold, or below it). Size limits and separate pieces are left to the
//! checks. Removed land ("lake") works like "Remove from play" instead: one
//! province per air zone, cut to the size limits if wanted, sea left alone.
//!
//! The image is laid on the map by georeference when both have one (any
//! resolution or CRS), pixel for pixel when it has the map's size, and
//! stretched over the map otherwise.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::Receiver;
use eframe::egui::{self, Color32, RichText};
use rayon::prelude::*;

use super::doc::Doc;
use super::model::{Id, Kind};
use super::ops::shape_takes;
use crate::geo::{Crs, FileGeoInfo};
use crate::layers::Placement;
use crate::raster::{self, Pixels, Raster};
use crate::util::Progress;

const ERR: Color32 = Color32::from_rgb(255, 110, 110);
const WARN: Color32 = Color32::from_rgb(255, 190, 90);
/// Sampled value of map pixels the image does not cover (or no data).
pub const OUTSIDE: u16 = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Channel {
    Red,
    Green,
    Blue,
    Alpha,
    /// Luma of the colour (the value of a grey image).
    Brightness,
}

impl Channel {
    pub const ALL: [Channel; 5] = [Channel::Red, Channel::Green, Channel::Blue, Channel::Alpha, Channel::Brightness];
    pub fn name(self) -> &'static str {
        match self {
            Channel::Red => "red",
            Channel::Green => "green",
            Channel::Blue => "blue",
            Channel::Alpha => "alpha",
            Channel::Brightness => "brightness",
        }
    }
}

/// A loaded image.
pub struct Source {
    pub path: PathBuf,
    pub raster: Raster,
    pub geo: FileGeoInfo,
}

impl Source {
    pub fn load(path: &Path) -> anyhow::Result<Source> {
        let (raster, geo) = raster::load(path, &Progress::default())?;
        Ok(Source { path: path.to_path_buf(), raster, geo })
    }
}

/// 0–255 of channel `ch` at pixel `i`, None for no data.
pub fn channel_value(r: &Raster, i: usize, ch: Channel) -> Option<u8> {
    let luma = |c: [u8; 3]| (0.299 * c[0] as f32 + 0.587 * c[1] as f32 + 0.114 * c[2] as f32).round() as u8;
    let pick = |c: [u8; 3], a: u8| match ch {
        Channel::Red => c[0],
        Channel::Green => c[1],
        Channel::Blue => c[2],
        Channel::Alpha => a,
        Channel::Brightness => luma(c),
    };
    let nodata = |v: f64| r.nodata.is_some_and(|n| n == v || (n.is_nan() && v.is_nan()));
    // single-band values stretched over their range
    let stretch = |v: f64| -> u8 {
        let (lo, hi) = r.data_range.unwrap_or((0.0, 255.0));
        (((v - lo) / (hi - lo).max(1e-12)) * 255.0).round().clamp(0.0, 255.0) as u8
    };
    Some(match &r.pixels {
        Pixels::Indexed8 { data, palette } => {
            let k = data[i];
            if nodata(k as f64) {
                return None;
            }
            pick(palette.get(k as usize).copied().unwrap_or([0, 0, 0]), 255)
        }
        Pixels::Gray8(d) => {
            if nodata(d[i] as f64) {
                return None;
            }
            pick([d[i]; 3], 255)
        }
        Pixels::Gray16(d) => {
            if nodata(d[i] as f64) {
                return None;
            }
            let v = if r.data_range.is_some() { stretch(d[i] as f64) } else { (d[i] >> 8) as u8 };
            pick([v; 3], 255)
        }
        Pixels::GrayF32(d) => {
            let v = d[i] as f64;
            if nodata(v) || !v.is_finite() {
                return None;
            }
            pick([stretch(v); 3], 255)
        }
        Pixels::Rgb8(d) => pick([d[3 * i], d[3 * i + 1], d[3 * i + 2]], 255),
        Pixels::Rgba8(d) => pick([d[4 * i], d[4 * i + 1], d[4 * i + 2]], d[4 * i + 3]),
    })
}

/// The channel of the image at every map pixel (row by row; OUTSIDE where
/// the image has nothing), and how the image was laid on the map.
pub fn sample(src: &Source, ch: Channel, w: u32, h: u32, map_geo: &FileGeoInfo) -> (Vec<u16>, String) {
    let r = &src.raster;
    let (iw, ih) = (r.width as i64, r.height as i64);
    let at = |c: i64, row: i64| -> u16 {
        if c < 0 || row < 0 || c >= iw || row >= ih {
            return OUTSIDE;
        }
        channel_value(r, (row * iw + c) as usize, ch).map_or(OUTSIDE, u16::from)
    };
    let map_crs = map_geo.crs.as_ref().map(|c| c.0).unwrap_or(Crs::UNKNOWN);
    let placed = match (&map_geo.transform, &src.geo.transform) {
        (Some((mgt, _)), Some((igt, _))) => {
            let icrs = src.geo.crs.as_ref().map(|c| c.0).unwrap_or(Crs::UNKNOWN);
            Placement::new(*mgt, map_crs, map_crs, [0.0, 0.0], w, h).zip(Placement::new(*igt, icrs, map_crs, [0.0, 0.0], r.width, r.height)).map(|p| (p, icrs))
        }
        _ => None,
    };
    let (wu, hu) = (w as usize, h as usize);
    let mut out = vec![OUTSIDE; wu * hu];
    let how = if let Some(((pm, pi), icrs)) = placed {
        out.par_chunks_mut(wu).enumerate().for_each(|(row, line)| {
            for (c, v) in line.iter_mut().enumerate() {
                if let Some(px) = pm.pixel_to_world(c as f64 + 0.5, row as f64 + 0.5).and_then(|wp| pi.world_to_pixel(wp)) {
                    *v = at(px[0].floor() as i64, px[1].floor() as i64);
                }
            }
        });
        let crs = if icrs == map_crs || icrs == Crs::UNKNOWN { String::new() } else { format!(", reprojected from {}", icrs.name()) };
        format!("laid on the map by georeference ({:.1} m pixels{crs})", src.geo.transform.as_ref().map(|t| t.0.pixel_size().0).unwrap_or(0.0))
    } else if (iw, ih) == (w as i64, h as i64) {
        out.par_chunks_mut(wu).enumerate().for_each(|(row, line)| {
            for (c, v) in line.iter_mut().enumerate() {
                *v = at(c as i64, row as i64);
            }
        });
        "pixel for pixel (same size as the map)".to_string()
    } else {
        out.par_chunks_mut(wu).enumerate().for_each(|(row, line)| {
            let ir = ((row as f64 + 0.5) * ih as f64 / h as f64) as i64;
            for (c, v) in line.iter_mut().enumerate() {
                *v = at(((c as f64 + 0.5) * iw as f64 / w as f64) as i64, ir);
            }
        });
        format!("stretched over the map (no georeference, {iw}×{ih} instead of {w}×{h})")
    };
    (out, how)
}

/// Whether a sampled value is in the shape.
#[inline]
pub fn in_shape(v: u16, threshold: u8, below: bool) -> bool {
    v != OUTSIDE && if below { v < threshold as u16 } else { v >= threshold as u16 }
}

/// What the shape covers on the map now.
#[derive(Clone, Debug, Default)]
pub struct Coverage {
    /// Pixels in the shape (inside the map), by kind of the province under them.
    pub land: u64,
    pub sea: u64,
    pub lake: u64,
    /// Map pixels the image does not cover.
    pub outside: u64,
    /// Provinces with pixels in the shape, and those entirely in it.
    pub provinces: usize,
    pub whole: usize,
    /// Air zones of the land and removed provinces in the shape.
    pub zones: usize,
}

impl Coverage {
    pub fn of(doc: &Doc, values: &[u16], threshold: u8, below: bool) -> Coverage {
        let lab = doc.labels.read();
        let cap = doc.data.provinces.cap();
        let (counts, outside) = lab
            .px
            .par_chunks(1 << 16)
            .zip(values.par_chunks(1 << 16))
            .fold(
                || (vec![0u64; cap], 0u64),
                |(mut acc, mut out), (p, v)| {
                    for (&p, &v) in p.iter().zip(v) {
                        if v == OUTSIDE {
                            out += 1;
                        } else if in_shape(v, threshold, below) && (p as usize) < cap {
                            acc[p as usize] += 1;
                        }
                    }
                    (acc, out)
                },
            )
            .reduce(
                || (vec![0u64; cap], 0),
                |(mut a, o1), (b, o2)| {
                    for (x, y) in a.iter_mut().zip(b) {
                        *x += y;
                    }
                    (a, o1 + o2)
                },
            );
        let mut c = Coverage { outside, ..Default::default() };
        let mut zones = std::collections::BTreeSet::new();
        for (p, &n) in counts.iter().enumerate() {
            if n == 0 || p == 0 {
                continue;
            }
            let kind = doc.data.kind(p as Id);
            match kind {
                Kind::Land => c.land += n,
                Kind::Sea => c.sea += n,
                Kind::Lake => c.lake += n,
            }
            let r = doc.data.region_of(p as Id);
            if kind != Kind::Sea && doc.data.regions.get(r).is_some_and(|z| !z.naval) {
                zones.insert(r);
            }
            c.provinces += 1;
            if n == doc.stats.get(p as Id).px as u64 {
                c.whole += 1;
            }
        }
        c.zones = zones.len();
        c
    }

    /// Pixels a new province of kind `k` takes ("only over …": see
    /// [`shape_takes`]).
    pub fn taken(&self, k: Kind, only_kind: bool) -> u64 {
        [(Kind::Land, self.land), (Kind::Sea, self.sea), (Kind::Lake, self.lake)]
            .into_iter()
            .filter(|&(under, _)| !only_kind || shape_takes(k, under))
            .map(|(_, n)| n)
            .sum()
    }

    /// The kind most of the shape lies on.
    pub fn main_kind(&self) -> Kind {
        if self.sea > self.land && self.sea >= self.lake {
            Kind::Sea
        } else if self.lake > self.land {
            Kind::Lake
        } else {
            Kind::Land
        }
    }
}

/// What the window asks the editor to do.
pub struct Create {
    pub values: Arc<Vec<u16>>,
    pub threshold: u8,
    pub below: bool,
    pub kind: Kind,
    pub only_kind: bool,
    pub name: String,
}

type Key = (PathBuf, Channel);
/// Image, channel, threshold, below, map version.
type CovKey = (Key, u8, bool, u64);
/// The same, plus the kind and "only over".
type TexKey = (Key, u8, bool, u64, Kind, bool);

/// State of the "Province from image" window.
pub struct FromImage {
    pub open: bool,
    path: String,
    loading: Option<Receiver<Result<Source, String>>>,
    src: Option<Arc<Source>>,
    err: Option<String>,
    channel: Channel,
    threshold: u8,
    below: bool,
    /// None: the kind most of the shape lies on.
    kind: Option<Kind>,
    only_kind: bool,
    name: String,
    sampled: Option<(Key, Arc<Vec<u16>>, String)>,
    /// Coverage of the shape.
    cov: Option<(CovKey, Coverage)>,
    /// Preview.
    tex: Option<(TexKey, egui::TextureHandle)>,
    /// Load `path` when the window is next drawn.
    pending: bool,
}

impl Default for FromImage {
    fn default() -> Self {
        FromImage {
            open: false,
            path: String::new(),
            loading: None,
            src: None,
            err: None,
            channel: Channel::Red,
            threshold: 128,
            below: false,
            kind: None,
            only_kind: true,
            name: String::new(),
            sampled: None,
            cov: None,
            tex: None,
            pending: false,
        }
    }
}

fn kind_name(k: Kind) -> &'static str {
    match k {
        Kind::Land => "land",
        Kind::Sea => "sea",
        Kind::Lake => "lake (out of play)",
    }
}

impl FromImage {
    /// Open with an image ($MOD_HELPER_OPEN=from-image:PATH, for screenshots).
    pub fn at_start() -> Self {
        let mut f = FromImage::default();
        if let Some(p) = std::env::var("MOD_HELPER_OPEN").ok().and_then(|v| v.strip_prefix("from-image:").map(String::from)) {
            f.open = true;
            f.path = p;
            f.pending = true;
        }
        f
    }

    pub fn start_load(&mut self, path: PathBuf, ctx: &egui::Context) {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let c = ctx.clone();
        self.path = path.display().to_string();
        self.err = None;
        std::thread::spawn(move || {
            let _ = tx.send(Source::load(&path).map_err(|e| format!("{e:#}")));
            c.request_repaint();
        });
        self.loading = Some(rx);
    }

    /// Small picture of the map: the shape in orange (dim where it is left
    /// alone), land grey, water blue.
    fn preview(doc: &Doc, values: &[u16], thr: u8, below: bool, kind: Kind, only_kind: bool) -> egui::ColorImage {
        let (w, h) = (doc.w as usize, doc.h as usize);
        let tw = 360usize;
        let th = ((h * tw) / w.max(1)).max(1);
        let lab = doc.labels.read();
        let mut px = Vec::with_capacity(tw * th);
        for ty in 0..th {
            let y = ((ty as f64 + 0.5) * h as f64 / th as f64) as usize;
            for tx in 0..tw {
                let x = ((tx as f64 + 0.5) * w as f64 / tw as f64) as usize;
                let i = y * w + x;
                let k = doc.data.kind(lab.px[i]);
                let c = if in_shape(values[i], thr, below) {
                    if !only_kind || shape_takes(kind, k) { Color32::from_rgb(255, 150, 40) } else { Color32::from_rgb(110, 70, 40) }
                } else {
                    match k {
                        Kind::Land => Color32::from_gray(if values[i] == OUTSIDE { 45 } else { 80 }),
                        Kind::Sea => Color32::from_rgb(35, 55, 95),
                        Kind::Lake => Color32::from_rgb(45, 75, 110),
                    }
                };
                px.push(c);
            }
        }
        egui::ColorImage { size: [tw, th], pixels: px, source_size: egui::vec2(tw as f32, th as f32) }
    }

    /// The window; returns a request to make the province.
    /// `coarsen`: cut removed land to the size limits (the setting of the
    /// "Inaccessible" panel).
    pub fn ui(&mut self, ctx: &egui::Context, doc: &Doc, map_version: u64, coarsen: &mut bool) -> Option<Create> {
        if !self.open {
            return None;
        }
        if std::mem::take(&mut self.pending) {
            self.start_load(PathBuf::from(self.path.trim()), ctx);
        }
        if let Some(rx) = &self.loading {
            match rx.try_recv() {
                Ok(Ok(src)) => {
                    if self.name.is_empty() || self.src.as_ref().is_some_and(|s| self.name == stem(&s.path)) {
                        self.name = stem(&src.path);
                    }
                    self.src = Some(Arc::new(src));
                    self.loading = None;
                    self.sampled = None;
                }
                Ok(Err(e)) => {
                    self.err = Some(e);
                    self.loading = None;
                }
                Err(crossbeam_channel::TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(100)),
                Err(_) => self.loading = None,
            }
        }
        // the channel on the map (once per image and channel)
        if let Some(src) = &self.src {
            let key = (src.path.clone(), self.channel);
            if self.sampled.as_ref().is_none_or(|(k, _, _)| *k != key) {
                let (v, how) = sample(src, self.channel, doc.w, doc.h, &doc.geo);
                self.sampled = Some((key, Arc::new(v), how));
            }
        }
        let mut create = None;
        let mut open = true;
        let mut browse = false;
        let mut load = false;
        egui::Window::new("Province from image").open(&mut open).default_width(400.0).show(ctx, |ui| {
            ui.label(RichText::new("One new province with the shape of a colour channel of an image.").weak());
            ui.horizontal(|ui| {
                let r = ui.add(egui::TextEdit::singleline(&mut self.path).hint_text("image (BMP, TIFF, PNG, JPEG)").desired_width(250.0));
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    load = true;
                }
                if ui.button("Browse…").clicked() {
                    browse = true;
                }
                if ui.button("Load").clicked() {
                    load = true;
                }
            });
            if self.loading.is_some() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("loading…");
                });
            }
            if let Some(e) = &self.err {
                ui.colored_label(ERR, e);
            }
            let Some(src) = self.src.clone() else { return };
            let Some((key, values, how)) = self.sampled.clone() else { return };
            ui.label(RichText::new(format!("{}: {}×{} {}, {how}", stem(&src.path), src.raster.width, src.raster.height, src.raster.format)).small().weak());
            ui.horizontal_wrapped(|ui| {
                ui.label("Channel");
                for c in Channel::ALL {
                    ui.selectable_value(&mut self.channel, c, c.name());
                }
            });
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("from_image_cmp").selected_text(if self.below { "below" } else { "at least" }).show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.below, false, "at least");
                    ui.selectable_value(&mut self.below, true, "below");
                });
                ui.add(egui::Slider::new(&mut self.threshold, 0..=255).text("value"));
            });
            // what it covers (recounted when the shape or the map changes)
            let ckey = (key.clone(), self.threshold, self.below, map_version);
            if self.cov.as_ref().is_none_or(|(k, _)| *k != ckey) {
                self.cov = Some((ckey, Coverage::of(doc, &values, self.threshold, self.below)));
            }
            let cov = self.cov.as_ref().unwrap().1.clone();
            let kind = self.kind.unwrap_or_else(|| cov.main_kind());
            let tkey = (key.clone(), self.threshold, self.below, map_version, kind, self.only_kind);
            if self.tex.as_ref().is_none_or(|(k, _)| *k != tkey) {
                let img = Self::preview(doc, &values, self.threshold, self.below, kind, self.only_kind);
                self.tex = Some((tkey, ctx.load_texture("from_image_preview", img, egui::TextureOptions::LINEAR)));
            }
            let tex = self.tex.as_ref().unwrap().1.clone();
            ui.horizontal(|ui| {
                ui.label("New province");
                egui::ComboBox::from_id_salt("from_image_kind").selected_text(kind_name(kind)).show_ui(ui, |ui| {
                    for k in [Kind::Land, Kind::Sea, Kind::Lake] {
                        if ui.selectable_label(kind == k, kind_name(k)).clicked() {
                            self.kind = Some(k);
                        }
                    }
                });
                if self.kind.is_some() && ui.small_button("auto").on_hover_text("the kind most of the shape lies on").clicked() {
                    self.kind = None;
                }
            });
            let over = match kind {
                Kind::Lake => "only over land and removed provinces (sea stays)",
                Kind::Land => "only over land provinces",
                Kind::Sea => "only over sea provinces",
            };
            ui.checkbox(&mut self.only_kind, over).on_hover_text("Leave the parts of the shape over other kinds of province as they are (like the paint tool)");
            ui.horizontal(|ui| {
                ui.label("Name");
                ui.add(egui::TextEdit::singleline(&mut self.name).desired_width(250.0));
            });
            ui.image((tex.id(), tex.size_vec2()));
            let km2 = |px: u64| px as f64 * doc.data.pixel_m * doc.data.pixel_m / 1e6;
            let taken = cov.taken(kind, self.only_kind);
            ui.label(format!(
                "{taken} px ({:.0} km²) become {} · over {} provinces, {} of them entirely (they go)",
                km2(taken),
                if kind == Kind::Lake { "removed land" } else { "the province" },
                cov.provinces,
                cov.whole
            ));
            let left = cov.land + cov.sea + cov.lake - taken;
            if left > 0 {
                ui.label(RichText::new(format!("{left} px of the shape over other kinds stay as they are (dim orange)")).small().weak());
            }
            if !self.only_kind && kind != Kind::Sea && cov.sea > 0 {
                ui.colored_label(WARN, format!("{} sea px become {}", cov.sea, kind_name(kind)));
            }
            if cov.outside == values.len() as u64 {
                ui.colored_label(ERR, "the image does not cover the map");
            }
            if kind == Kind::Lake {
                ui.checkbox(coarsen, "merge into as few provinces as possible").on_hover_text(
                    "Cut each air zone's part into as few provinces as HOI4's size limits allow, as \"Remove from play\" does (the same setting)",
                );
                ui.label(
                    RichText::new(format!(
                        "Like \"Remove from play\": no state or owner, and each part keeps its air zone ({} zone{}: one province each{}). Crossings along the edge are fixed.",
                        cov.zones,
                        if cov.zones == 1 { "" } else { "s" },
                        if *coarsen { ", then cut to the size limits" } else { "; size and separate pieces show up in the checks" },
                    ))
                    .small()
                    .weak(),
                );
            } else {
                ui.label(
                    RichText::new("State and zone: those of most of the area. Crossings along its edge are fixed; size and separate pieces show up in the checks.").small().weak(),
                );
            }
            ui.horizontal(|ui| {
                if ui.add_enabled(taken > 0 && !self.name.trim().is_empty(), egui::Button::new("Create province")).clicked() {
                    create = Some(Create {
                        values: values.clone(),
                        threshold: self.threshold,
                        below: self.below,
                        kind,
                        only_kind: self.only_kind,
                        name: self.name.trim().to_string(),
                    });
                }
            });
        });
        if browse {
            let start = PathBuf::from(&self.path).parent().map(Path::to_path_buf).filter(|p| p.is_dir()).or_else(|| doc.dir.parent().map(Path::to_path_buf));
            let mut d = rfd::FileDialog::new().add_filter("image", &["bmp", "tif", "tiff", "png", "jpg", "jpeg"]);
            if let Some(s) = start {
                d = d.set_directory(s);
            }
            if let Some(p) = d.pick_file() {
                self.start_load(p, ctx);
            }
        } else if load && !self.path.trim().is_empty() {
            self.start_load(PathBuf::from(self.path.trim()), ctx);
        }
        self.open = open && create.is_none();
        create
    }
}

fn stem(p: &Path) -> String {
    p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geo::GeoTransform;

    fn src(w: u32, h: u32, pixels: Pixels, gt: Option<GeoTransform>) -> Source {
        let geo = FileGeoInfo { transform: gt.map(|g| (g, "test".into())), crs: None, nodata: None };
        Source { path: "t.png".into(), raster: Raster { width: w, height: h, pixels, nodata: None, data_range: None, format: String::new() }, geo }
    }

    #[test]
    fn channels() {
        let s = src(2, 1, Pixels::Rgba8(vec![10, 20, 30, 40, 255, 255, 255, 0]), None);
        assert_eq!(channel_value(&s.raster, 0, Channel::Red), Some(10));
        assert_eq!(channel_value(&s.raster, 0, Channel::Blue), Some(30));
        assert_eq!(channel_value(&s.raster, 0, Channel::Alpha), Some(40));
        assert_eq!(channel_value(&s.raster, 1, Channel::Brightness), Some(255));
        let p = src(2, 1, Pixels::Indexed8 { data: vec![0, 1], palette: vec![[240, 240, 240], [0, 128, 255]] }, None);
        assert_eq!(channel_value(&p.raster, 1, Channel::Green), Some(128));
        assert_eq!(channel_value(&p.raster, 0, Channel::Alpha), Some(255));
        let mut f = src(2, 1, Pixels::GrayF32(vec![100.0, 300.0]), None);
        f.raster.data_range = Some((100.0, 300.0));
        assert_eq!(channel_value(&f.raster, 0, Channel::Red), Some(0));
        assert_eq!(channel_value(&f.raster, 1, Channel::Brightness), Some(255));
        f.raster.nodata = Some(300.0);
        assert_eq!(channel_value(&f.raster, 1, Channel::Red), None);
        assert!(in_shape(200, 128, false) && !in_shape(100, 128, false) && in_shape(100, 128, true) && !in_shape(OUTSIDE, 128, true));
    }

    #[test]
    fn laying_the_image_on_the_map() {
        // a 4x2 map of 10 m pixels; a 2x1 image of 20 m pixels over its right half
        let map = FileGeoInfo { transform: Some((GeoTransform::north_up(0.0, 20.0, 10.0, 10.0), "t".into())), crs: None, nodata: None };
        let img = src(2, 1, Pixels::Gray8(vec![50, 200]), Some(GeoTransform::north_up(20.0, 20.0, 20.0, 20.0)));
        let (v, how) = sample(&img, Channel::Red, 4, 2, &map);
        assert!(how.contains("georeference"), "{how}");
        assert_eq!(v, vec![OUTSIDE, OUTSIDE, 50, 50, OUTSIDE, OUTSIDE, 50, 50]);
        // same size: pixel for pixel
        let same = src(4, 2, Pixels::Gray8((0..8).collect()), None);
        let (v, how) = sample(&same, Channel::Red, 4, 2, &map);
        assert!(how.contains("pixel for pixel"));
        assert_eq!(v, (0..8).collect::<Vec<u16>>());
        // other size, no georeference: stretched
        let small = src(2, 1, Pixels::Gray8(vec![1, 2]), None);
        let (v, how) = sample(&small, Channel::Red, 4, 2, &map);
        assert!(how.contains("stretched"));
        assert_eq!(v, vec![1, 1, 2, 2, 1, 1, 2, 2]);
    }
}
