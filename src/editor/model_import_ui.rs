//! The Objects level's "Import a model…" window: a 3D file (glTF / GLB or
//! OBJ as Blender exports them, or a `.mesh` from Paradox's Blender
//! exporter) and its pictures made into the game's files in a content
//! package (see `models::import`), shown as they will be before anything
//! is written.

use std::path::PathBuf;
use std::sync::Arc;

use crossbeam_channel::{Receiver, unbounded};
use eframe::egui::{self, RichText, Sense, vec2};

use super::{ERR, Editor, WARN};
use crate::content::store::SHARED;
use crate::models::catalog::Pick;
use crate::models::import::{self, GlowFrom, Kind, Made, NormalFrom, Options, Source, SurfaceFrom, TexSrc, Up};
use crate::models::{self, Model, Play, View};

/// What pressing Import did: (package, entity, files written), or why not.
type Done = Option<Result<(String, String, Vec<String>), String>>;

const PICTURES: [&str; 6] = ["dds", "png", "jpg", "jpeg", "tga", "PNG"];

#[derive(Default)]
pub struct ImportUi {
    pub open: bool,
    reading: Option<(PathBuf, Receiver<Result<Source, String>>)>,
    src: Option<Arc<Source>>,
    err: Option<String>,
    opts: Option<Options>,
    package: String,
    /// What the options shown made (or why not).
    made: Option<(Options, Result<Arc<Made>, String>)>,
    making: Option<(Options, Receiver<Result<Made, String>>)>,
    model: Option<(usize, Arc<Model>)>,
    view: Option<View>,
    tex: Option<egui::TextureHandle>,
    /// What the picture shows: the made model, the view, the size.
    pic_key: Option<(usize, [i32; 4], [usize; 2])>,
    last_dir: Option<PathBuf>,
    /// Read this file when the window shows first (for screenshots:
    /// MOD_HELPER_OPEN=import-model:FILE).
    start_with: Option<PathBuf>,
}

fn pick_picture(dir: &Option<PathBuf>) -> Option<TexSrc> {
    let mut d = rfd::FileDialog::new().add_filter("picture", &PICTURES);
    if let Some(dir) = dir {
        d = d.set_directory(dir);
    }
    d.pick_file().map(TexSrc::File)
}

impl ImportUi {
    pub fn from_env() -> ImportUi {
        let start_with = std::env::var("MOD_HELPER_OPEN").ok().and_then(|v| v.strip_prefix("import-model:").map(PathBuf::from));
        ImportUi { open: start_with.is_some(), start_with, ..Default::default() }
    }

    fn start_read(&mut self, path: PathBuf, ctx: &egui::Context) {
        let (tx, rx) = unbounded();
        let (p, ctx) = (path.clone(), ctx.clone());
        std::thread::spawn(move || {
            let r = import::read(&p).map_err(|e| format!("{e:#}"));
            let _ = tx.send(r);
            ctx.request_repaint();
        });
        self.last_dir = path.parent().map(|d| d.to_path_buf());
        self.reading = Some((path, rx));
        self.err = None;
    }

    /// Take in what the background finished; start making what the options
    /// now say.
    fn poll(&mut self, ctx: &egui::Context) {
        if let Some((_, rx)) = &self.reading
            && let Ok(r) = rx.try_recv()
        {
            self.reading = None;
            match r {
                Ok(src) => {
                    let mut o = Options::for_source(&src);
                    // keep what was set for the model before when it is the same file again
                    if let (Some(old), Some(prev)) = (&self.opts, &self.src)
                        && prev.file == src.file
                        && old.mats.len() == o.mats.len()
                    {
                        o = old.clone();
                    }
                    self.opts = Some(o);
                    self.src = Some(Arc::new(src));
                    self.made = None;
                    self.model = None;
                }
                Err(e) => self.err = Some(e),
            }
        }
        if let Some((o, rx)) = &self.making
            && let Ok(r) = rx.try_recv()
        {
            self.made = Some((o.clone(), r.map(Arc::new)));
            self.making = None;
        }
        let (Some(src), Some(o)) = (&self.src, &self.opts) else { return };
        let stale = self.made.as_ref().is_none_or(|m| &m.0 != o);
        if stale && self.making.is_none() {
            let (tx, rx) = unbounded();
            let (src, o2, ctx) = (src.clone(), o.clone(), ctx.clone());
            std::thread::spawn(move || {
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| import::make(&src, &o2).map_err(|e| format!("{e:#}"))))
                    .unwrap_or_else(|_| Err("making the model failed (a bug)".into()));
                let _ = tx.send(r);
                ctx.request_repaint();
            });
            self.making = Some((o.clone(), rx));
        }
    }

    fn made_ok(&self) -> Option<&Arc<Made>> {
        self.made.as_ref().and_then(|m| m.1.as_ref().ok())
    }
}

impl Editor {
    pub(super) fn model_import_window(&mut self, ctx: &egui::Context) {
        if !self.ob.imp.open {
            return;
        }
        let Some(content) = self.doc.as_ref().map(|d| d.dir.join("content")) else { return };
        let loader = self.ob.loader.clone();
        let mut open = true;
        let mut done: Done = None;
        egui::Window::new("Import a model").open(&mut open).default_size([820.0, 640.0]).resizable(true).show(ctx, |ui| {
            let imp = &mut self.ob.imp;
            if let Some(p) = imp.start_with.take() {
                imp.start_read(p, ctx);
            }
            imp.poll(ctx);
            if imp.package.is_empty() {
                imp.package = SHARED.into();
            }
            ui.horizontal(|ui| {
                if ui.button("Choose a 3D file…").clicked() {
                    let mut d = rfd::FileDialog::new().add_filter("3D model", &["glb", "gltf", "obj", "mesh"]);
                    if let Some(dir) = &imp.last_dir {
                        d = d.set_directory(dir);
                    }
                    if let Some(p) = d.pick_file() {
                        imp.start_read(p, ctx);
                    }
                }
                match (&imp.reading, &imp.src) {
                    (Some((p, _)), _) => {
                        ui.spinner();
                        ui.label(format!("reading {}…", p.file_name().unwrap_or_default().to_string_lossy()));
                    }
                    (None, Some(s)) => {
                        ui.label(RichText::new(s.file.file_name().unwrap_or_default().to_string_lossy()).strong()).on_hover_text(s.file.display().to_string());
                        if ui.small_button("↻").on_hover_text("Read the file again (after changing it in Blender)").clicked() {
                            let p = s.file.clone();
                            imp.start_read(p, ctx);
                        }
                    }
                    _ => {}
                }
            });
            ui.label(
                RichText::new(
                    "A model made in Blender (or any 3D program), exported as glTF (.glb) or OBJ, or as .mesh with Paradox's exporter. \
                     Its pictures can be DDS, PNG, JPEG or TGA: they become the game's textures (diffuse, normal and specular DDS), \
                     and the model an entity the Place tool puts on the map.",
                )
                .small()
                .weak(),
            );
            if let Some(e) = &imp.err {
                ui.label(RichText::new(e).color(ERR));
            }
            let Some(src) = imp.src.clone() else { return };
            let Some(mut o) = imp.opts.clone() else { return };
            let kind = match src.kind {
                Kind::Gltf => "glTF",
                Kind::Obj => "OBJ",
                Kind::Mesh => "Paradox .mesh",
            };
            ui.label(RichText::new(format!("{kind} · {} triangles · {} material(s)", src.triangles(), src.mats.len())).small());
            for n in &src.notes {
                ui.label(RichText::new(n).small().color(WARN));
            }
            ui.separator();
            let avail = ui.available_size();
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width((avail.x * 0.48).clamp(300.0, 460.0));
                    egui::ScrollArea::vertical().id_salt("model_import_opts").max_height((avail.y - 10.0).max(200.0)).show(ui, |ui| {
                        options_ui(ui, imp, &src, &mut o, loader.as_deref(), &content, &mut done);
                    });
                });
                ui.separator();
                ui.vertical(|ui| preview_ui(ui, imp));
            });
            if imp.opts.as_ref() != Some(&o) {
                imp.opts = Some(o);
            }
        });
        if !open {
            self.ob.imp.open = false;
        }
        match done {
            Some(Ok((pkg, entity, files))) => {
                // read the models again, then show the new one
                self.ob.reload(Some(Pick::Entity(entity.clone())));
                let n = files.len();
                self.content_written.push((pkg.clone(), files));
                self.ob.imp.open = false;
                self.info(format!("imported the model {entity}: {n} files in the content ({pkg}); the Place tool (A) puts it on the map"));
            }
            Some(Err(e)) => self.error(format!("cannot import the model: {e}")),
            None => {}
        }
    }
}

fn options_ui(ui: &mut egui::Ui, imp: &mut ImportUi, src: &Source, o: &mut Options, loader: Option<&models::Loader>, content: &std::path::Path, done: &mut Done) {
    let entity = import::entity_name(&o.name);
    let mut blocked: Option<String> = import::check_name(&o.name).err().map(|e| e.to_string());
    egui::Grid::new("model_import_grid").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
        ui.label("name");
        ui.vertical(|ui| {
            ui.add(egui::TextEdit::singleline(&mut o.name).desired_width(220.0));
            ui.label(RichText::new(format!("entity {entity}")).small().weak());
            // a name the game or the mod has already
            if let Some(cat) = loader.map(|l| &l.cat)
                && let Some(origin) = cat.origin_of(&Pick::Entity(entity.clone())).or(cat.origin_of(&Pick::Mesh(import::mesh_name(&o.name))))
            {
                if origin.is_mod() {
                    ui.label(RichText::new("The mod has a model of that name: importing replaces it (its files go to the backups).").small().color(WARN));
                } else {
                    let msg = format!("{} has a model of that name: pick another", origin.label());
                    ui.label(RichText::new(&msg).small().color(ERR));
                    blocked = Some(msg);
                }
            }
        });
        ui.end_row();
        ui.label("package");
        let mut pkgs: Vec<String> = std::fs::read_dir(content)
            .map(|rd| rd.flatten().filter(|e| e.path().is_dir()).map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| !n.starts_with('.')).collect())
            .unwrap_or_default();
        if !pkgs.iter().any(|p| p == SHARED) {
            pkgs.push(SHARED.into());
        }
        pkgs.sort();
        egui::ComboBox::from_id_salt("model_import_pkg").selected_text(&imp.package).show_ui(ui, |ui| {
            for p in pkgs {
                ui.selectable_value(&mut imp.package, p.clone(), p);
            }
        })
        .response
        .on_hover_text("The content package the files go in: shared (the mod's own), or a nation's");
        ui.end_row();
        ui.label("folder");
        ui.horizontal(|ui| {
            ui.label(RichText::new("gfx/models/").weak());
            ui.add(egui::TextEdit::singleline(&mut o.folder).desired_width(140.0));
        });
        ui.end_row();
        if src.kind == Kind::Obj {
            ui.label("up");
            ui.horizontal(|ui| {
                ui.radio_value(&mut o.up, Up::Y, "Y").on_hover_text("Blender's OBJ export as it starts (Y up, -Z forward)");
                ui.radio_value(&mut o.up, Up::Z, "Z").on_hover_text("Exported with Z up (Blender's own axes)");
            });
            ui.end_row();
        }
        ui.label("stands");
        ui.add_enabled(src.kind != Kind::Mesh, egui::Checkbox::new(&mut o.ground, "on the middle of its bottom"))
            .on_hover_text("Moves the model so the middle of its bottom is where it stands on the map (the build sets that on the ground)");
        ui.end_row();
        ui.label("size");
        ui.vertical(|ui| {
            let raw = imp.made_ok().map(|m| m.mesh.max - m.mesh.min);
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut o.scale).range(0.0001..=10000.0).speed(0.01).max_decimals(4)).on_hover_text("Map pixels per unit of the file (the mesh's scale)");
                ui.label("map px per unit");
            });
            if let Some(r) = raw
                && r.y > 1e-6
            {
                let mut h = r.y * o.scale;
                ui.horizontal(|ui| {
                    if ui.add(egui::DragValue::new(&mut h).range(0.001..=10000.0).speed(0.01).max_decimals(3)).changed() && h > 0.0 {
                        o.scale = h / r.y;
                    }
                    ui.label(format!("map px high, {:.2} × {:.2} wide", r.x * o.scale, r.z * o.scale));
                });
            }
            ui.label(RichText::new("On the game's map a tree is about 0.5 map px high, Big Ben about 6.").small().weak());
        });
        ui.end_row();
        ui.label("snow");
        ui.checkbox(&mut o.snow, "white in winter").on_hover_text("PdxMeshAdvancedSnow, as the game's buildings and trees");
        ui.end_row();
        ui.label("textures");
        egui::ComboBox::from_id_salt("model_import_max").selected_text(format!("at most {} px", o.max_tex)).show_ui(ui, |ui| {
            for s in [256, 512, 1024, 2048, 4096] {
                ui.selectable_value(&mut o.max_tex, s, format!("{s} px"));
            }
        })
        .response
        .on_hover_text("Pictures are made a power of two on each side (what the game wants), at most this big; they are written uncompressed with mipmaps");
        ui.end_row();
    });
    for (i, (m, mo)) in src.mats.iter().zip(o.mats.iter_mut()).enumerate() {
        egui::CollapsingHeader::new(RichText::new(format!("Material: {}", m.name)).strong()).id_salt(("model_mat", i)).default_open(src.mats.len() <= 3).show(ui, |ui| {
            egui::Grid::new(("model_mat_grid", i)).num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
                ui.label("colour");
                ui.horizontal(|ui| {
                    let from = match (&mo.diffuse, &m.diffuse) {
                        (Some(t), _) => format!("{} (picked)", t.label()),
                        (None, Some(t)) => t.label(),
                        (None, None) => "its colour (no picture)".into(),
                    };
                    ui.label(from);
                    if ui.small_button("Picture…").on_hover_text("Another picture instead of the model's").clicked()
                        && let Some(t) = pick_picture(&imp.last_dir)
                    {
                        mo.diffuse = Some(t);
                    }
                    if mo.diffuse.is_some() && ui.small_button("×").on_hover_text("Back to the model's").clicked() {
                        mo.diffuse = None;
                    }
                });
                ui.end_row();
                ui.label("bumps");
                ui.vertical(|ui| {
                    let label = |n: &NormalFrom| match n {
                        NormalFrom::Model => format!("the model's: {}", m.normal.as_ref().map(TexSrc::label).unwrap_or_default()),
                        NormalFrom::File(t) => format!("picture: {}", t.label()),
                        NormalFrom::Flat => "none (smooth)".into(),
                        NormalFrom::Bumps => "from the colour's brightness".into(),
                    };
                    egui::ComboBox::from_id_salt(("model_normal", i)).selected_text(label(&mo.normal)).width(230.0).show_ui(ui, |ui| {
                        if m.normal.is_some() {
                            ui.selectable_value(&mut mo.normal, NormalFrom::Model, label(&NormalFrom::Model));
                        }
                        ui.selectable_value(&mut mo.normal, NormalFrom::Flat, label(&NormalFrom::Flat));
                        ui.selectable_value(&mut mo.normal, NormalFrom::Bumps, label(&NormalFrom::Bumps))
                            .on_hover_text("Bright parts stand out, as the guide's emboss: for stone, wood, roofs");
                        if ui.selectable_label(matches!(mo.normal, NormalFrom::File(_)), "a normal or bump map…").clicked()
                            && let Some(t) = pick_picture(&imp.last_dir)
                        {
                            mo.normal = NormalFrom::File(t);
                        }
                    });
                    if mo.normal != NormalFrom::Flat {
                        ui.horizontal(|ui| {
                            ui.add(egui::Slider::new(&mut mo.bumps, 0.1..=4.0).text("strength")).on_hover_text("For bumps made from brightness (a grey bump map too)");
                        });
                    }
                    if matches!(mo.normal, NormalFrom::Model | NormalFrom::File(_)) {
                        ui.checkbox(&mut mo.directx, "green points down (DirectX)").on_hover_text("Normal maps from Substance or DirectX tools; Blender's point up");
                    }
                });
                ui.end_row();
                ui.label("surface");
                ui.vertical(|ui| {
                    let has_model = m.surface.is_some() || m.metal_rough.is_some();
                    let label = |s: &SurfaceFrom| match s {
                        SurfaceFrom::Model => format!("the model's: {}", m.surface.as_ref().or(m.metal_rough.as_ref()).map(TexSrc::label).unwrap_or_default()),
                        SurfaceFrom::File(t) => format!("picture: {}", t.label()),
                        SurfaceFrom::Values => "the same all over".into(),
                    };
                    egui::ComboBox::from_id_salt(("model_surface", i)).selected_text(label(&mo.surface)).width(230.0).show_ui(ui, |ui| {
                        if has_model {
                            ui.selectable_value(&mut mo.surface, SurfaceFrom::Model, label(&SurfaceFrom::Model));
                        }
                        ui.selectable_value(&mut mo.surface, SurfaceFrom::Values, label(&SurfaceFrom::Values));
                        if ui
                            .selectable_label(matches!(mo.surface, SurfaceFrom::File(_)), "a specular map in the game's layout…")
                            .on_hover_text("Green: shine, blue: metal, alpha: gloss (as the game's own _specular.dds)")
                            .clicked()
                            && let Some(t) = pick_picture(&imp.last_dir)
                        {
                            mo.surface = SurfaceFrom::File(t);
                        }
                    });
                    let values = mo.surface == SurfaceFrom::Values || (mo.surface == SurfaceFrom::Model && m.surface.is_none());
                    if values {
                        ui.add(egui::Slider::new(&mut mo.shine, 0.0..=1.0).text("shine"));
                        if m.metal_rough.is_none() || mo.surface == SurfaceFrom::Values {
                            ui.add(egui::Slider::new(&mut mo.metal, 0.0..=1.0).text("metal"));
                            ui.add(egui::Slider::new(&mut mo.gloss, 0.0..=1.0).text("gloss"));
                        }
                    }
                });
                ui.end_row();
                ui.label("glow");
                ui.vertical(|ui| {
                    let has_model = m.glow.is_some() || m.glow_color.iter().any(|&v| v > 0.01);
                    let label = |g: &GlowFrom| match g {
                        GlowFrom::None => "none".to_string(),
                        GlowFrom::Model => format!("the model's: {}", m.glow.as_ref().map(TexSrc::label).unwrap_or_else(|| "its emission colour".into())),
                        GlowFrom::File(t) => format!("picture: {}", t.label()),
                    };
                    egui::ComboBox::from_id_salt(("model_glow", i)).selected_text(label(&mo.glow)).width(230.0).show_ui(ui, |ui| {
                        ui.selectable_value(&mut mo.glow, GlowFrom::None, label(&GlowFrom::None));
                        if has_model {
                            ui.selectable_value(&mut mo.glow, GlowFrom::Model, label(&GlowFrom::Model));
                        }
                        if ui.selectable_label(matches!(mo.glow, GlowFrom::File(_)), "a picture…").on_hover_text("Its bright parts light up at night (windows, lamps)").clicked()
                            && let Some(t) = pick_picture(&imp.last_dir)
                        {
                            mo.glow = GlowFrom::File(t);
                        }
                    });
                });
                ui.end_row();
            });
            if !m.missing.is_empty() {
                ui.label(RichText::new(format!("Not found: {} (pick them above)", m.missing.join(", "))).small().color(WARN));
            }
        });
    }
    ui.separator();
    match &imp.made {
        _ if imp.making.is_some() && imp.made.as_ref().is_none_or(|m| &m.0 != o) => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("making it…");
            });
        }
        Some((_, Err(e))) => {
            ui.label(RichText::new(e).color(ERR));
        }
        _ => {}
    }
    let Some(made) = imp.made_ok().cloned() else { return };
    for n in &made.notes {
        ui.label(RichText::new(n).small().color(WARN));
    }
    egui::CollapsingHeader::new(format!("{} files into content/{}", made.files.len(), imp.package)).id_salt("model_import_files").show(ui, |ui| {
        for (f, b) in &made.files {
            ui.label(RichText::new(format!("{f}  ({})", size_label(b.len()))).small().monospace());
        }
    });
    let current = imp.made.as_ref().is_some_and(|m| &m.0 == o);
    if let Some(b) = &blocked {
        ui.label(RichText::new(b).color(ERR));
    }
    let resp = ui.add_enabled(current && blocked.is_none(), egui::Button::new(RichText::new("Import").strong()));
    if resp.clicked() {
        *done = Some(match import::install(&made, content, &imp.package) {
            Ok(w) => Ok((imp.package.clone(), made.entity.clone(), w)),
            Err(e) => Err(format!("{e:#}")),
        });
    }
}

fn size_label(n: usize) -> String {
    if n >= 1 << 20 { format!("{:.1} MB", n as f64 / (1 << 20) as f64) } else { format!("{} KB", n.div_ceil(1024)) }
}

/// The model as made, turned with the mouse.
fn preview_ui(ui: &mut egui::Ui, imp: &mut ImportUi) {
    let Some(made) = imp.made_ok().cloned() else {
        ui.label(RichText::new("The preview shows here.").weak());
        return;
    };
    let key = Arc::as_ptr(&made) as usize;
    if imp.model.as_ref().is_none_or(|m| m.0 != key) {
        imp.model = Some((key, Arc::new(made.model())));
    }
    let model = imp.model.as_ref().unwrap().1.clone();
    let view = imp.view.get_or_insert_with(View::default);
    let w = ui.available_width().max(200.0);
    let size = vec2(w, (w * 0.8).clamp(180.0, 520.0));
    let ppp = ui.ctx().pixels_per_point();
    let (pw, ph) = (((size.x * ppp) as usize).clamp(32, 1400), ((size.y * ppp) as usize).clamp(32, 1000));
    let vk = [(view.yaw * 10.0) as i32, (view.pitch * 10.0) as i32, (view.zoom * 1000.0) as i32, view.ground as i32];
    let pk = (key, vk, [pw, ph]);
    if imp.pic_key != Some(pk) || imp.tex.is_none() {
        let bg = if ui.visuals().dark_mode { [34, 37, 44, 255] } else { [200, 205, 212, 255] };
        let f = models::picture(&model, pw, ph, *view, &Play::Still, 0.0, bg);
        let img = egui::ColorImage::from_rgba_unmultiplied([f.w, f.h], &f.rgba());
        match &mut imp.tex {
            Some(t) => t.set(img, egui::TextureOptions::LINEAR),
            None => imp.tex = Some(ui.ctx().load_texture("model import preview", img, egui::TextureOptions::LINEAR)),
        }
        imp.pic_key = Some(pk);
    }
    let tex = imp.tex.as_ref().unwrap();
    let resp = ui.add(egui::Image::new((tex.id(), size)).sense(Sense::click_and_drag()));
    let view = imp.view.as_mut().unwrap();
    if resp.dragged() {
        let d = resp.drag_delta();
        view.yaw = (view.yaw - d.x * 0.6).rem_euclid(360.0);
        view.pitch = (view.pitch + d.y * 0.4).clamp(-15.0, 89.0);
    }
    if resp.hovered() {
        let s = ui.input_mut(|i| std::mem::take(&mut i.smooth_scroll_delta)).y;
        if s != 0.0 {
            view.zoom = (view.zoom * (s * 0.003).exp()).clamp(0.2, 20.0);
        }
    }
    if resp.double_clicked() {
        *view = View { ground: view.ground, ..View::default() };
    }
    resp.on_hover_text("Drag to turn, wheel to zoom, double-click to reset. Shows the colours; the game adds the bumps, shine and snow.");
    ui.checkbox(&mut view.ground, "ground");
    let (lo, hi) = made.bounds();
    let s = hi - lo;
    ui.label(RichText::new(format!("{:.2} × {:.2} map px, {:.2} high · {} triangles · {} vertices", s.x, s.z, s.y, made.mesh.triangles(), made.vertices())).small());
}
