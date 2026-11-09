//! The welcome screen: the projects opened before, a project file, and a
//! new project: blank (its size and what it is filled with) or the map of
//! the base game or a mod, on a plain raster or a map projection. Also the
//! projection editor the Map projection window shares with it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crossbeam_channel::Receiver;
use eframe::egui::{self, Color32, RichText};

use crate::editor::launcher::{self, ModEntry};
use crate::geo::{self, Crs};
use crate::mapproj::{self, CHOICES, Projection};
use crate::newproject::{self, Created, Fill, Request, Start, hoi4};
use crate::project::{self, Recent};
use crate::util::{self, Progress};

const ERR: Color32 = Color32::from_rgb(255, 110, 110);
const WARN: Color32 = Color32::from_rgb(255, 190, 90);

/// What the welcome screen asks the app to do.
pub enum Action {
    Open(PathBuf),
    OpenDialog,
    Created(Created),
}

// ---------------------------------------------------------------- projections

/// A projection being chosen: a plain raster or a map projection.
#[derive(Clone, Debug)]
pub struct ProjEdit {
    pub plain: bool,
    /// Plain raster: km per pixel.
    pub plain_km: f64,
    /// A key of [`CHOICES`].
    pub choice: &'static str,
    /// Longitude, latitude of the map's centre.
    pub center: [f64; 2],
    /// Metres per pixel (degrees for lon/lat).
    pub pixel: f64,
}

impl ProjEdit {
    pub fn new(p: &Projection) -> ProjEdit {
        match *p {
            Projection::Plain { pixel_m } => ProjEdit { plain: true, plain_km: pixel_m / 1000.0, choice: "laea", center: [11.35, 46.5], pixel: 1000.0 },
            Projection::Geo { crs, center, pixel } => {
                ProjEdit { plain: false, plain_km: p.pixel_m() / 1000.0, choice: mapproj::crs_choice(crs), center, pixel }
            }
        }
    }

    fn degrees(&self) -> bool {
        self.choice == "lonlat"
    }

    pub fn projection(&self) -> Projection {
        if self.plain {
            Projection::Plain { pixel_m: self.plain_km * 1000.0 }
        } else {
            Projection::Geo { crs: mapproj::choice_crs(self.choice, self.center[0], self.center[1]), center: self.center, pixel: self.pixel }
        }
    }

    /// The choices for a map of w × h px; true when something changed.
    pub fn ui(&mut self, ui: &mut egui::Ui, w: u32, h: u32) -> bool {
        let before = (self.plain, self.plain_km, self.choice, self.center, self.pixel);
        ui.radio_value(&mut self.plain, true, "Plain raster").on_hover_text("Pixels of a size, on no place of the Earth: a fantasy map, or a map whose geography does not matter");
        ui.indent("plain", |ui| {
            ui.add_enabled_ui(self.plain, |ui| {
                ui.horizontal(|ui| {
                    ui.label("One pixel is");
                    ui.add(egui::DragValue::new(&mut self.plain_km).speed(0.01).range(0.001..=1000.0).max_decimals(3).suffix(" km"));
                    if ui.small_button("as vanilla").on_hover_text("The Earth's circumference over the map's width (vanilla: 7.1 km over 5632 px)").clicked() {
                        self.plain_km = 40_075.0 / w.max(1) as f64;
                    }
                });
                ui.label(RichText::new("Areas, slopes and distances use it.").small().weak());
            });
        });
        ui.radio_value(&mut self.plain, false, "Map projection").on_hover_text("The map lies on the Earth, so real geographic data (OSM extracts, georeferenced rasters) lines up with it");
        ui.indent("geo", |ui| {
            ui.add_enabled_ui(!self.plain, |ui| {
                egui::Grid::new("proj_grid").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
                    ui.label("Projection");
                    let was_deg = self.degrees();
                    let name = CHOICES.iter().find(|c| c.0 == self.choice).map_or("?", |c| c.1);
                    egui::ComboBox::from_id_salt("proj_choice").selected_text(name).width(330.0).show_ui(ui, |ui| {
                        for (k, label) in CHOICES {
                            ui.selectable_value(&mut self.choice, *k, *label);
                        }
                    });
                    // keep the pixel's size on the ground when its unit changes
                    if was_deg != self.degrees() {
                        self.pixel = if self.degrees() { self.pixel / geo::M_PER_DEG } else { self.pixel * geo::M_PER_DEG };
                    }
                    ui.end_row();
                    ui.label("Centre");
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut self.center[1]).speed(0.01).range(-85.0..=85.0).max_decimals(5).prefix("lat ").suffix("°"));
                        ui.add(egui::DragValue::new(&mut self.center[0]).speed(0.01).range(-180.0..=180.0).max_decimals(5).prefix("lon ").suffix("°"));
                    });
                    ui.end_row();
                    ui.label("Pixel size");
                    if self.degrees() {
                        ui.add(egui::DragValue::new(&mut self.pixel).speed(0.0001).range(1e-6..=1.0).max_decimals(6).suffix("°"));
                    } else {
                        ui.add(egui::DragValue::new(&mut self.pixel).speed(1.0).range(1.0..=100_000.0).max_decimals(2).suffix(" m"));
                    }
                    ui.end_row();
                    if self.choice.starts_with("utm") {
                        ui.label("");
                        let crs = mapproj::choice_crs(self.choice, self.center[0], self.center[1]);
                        ui.label(RichText::new(format!("{} (zone {} from the centre)", crs.name(), geo::utm_zone(self.center[0]))).small().weak());
                        ui.end_row();
                    }
                });
            });
        });
        let p = self.projection();
        ui.add_space(4.0);
        match p.transform(w, h) {
            Some(_) => {
                ui.label(RichText::new(p.describe(w, h)).small());
                if let Some(c) = p.corners(w, h) {
                    ui.label(RichText::new(format!("top left {}, bottom right {}", geo::fmt_lonlat(c[0].0, c[0].1), geo::fmt_lonlat(c[2].0, c[2].1))).small().weak());
                }
            }
            None => {
                ui.colored_label(ERR, "The projection cannot show this centre");
            }
        }
        before != (self.plain, self.plain_km, self.choice, self.center, self.pixel)
    }
}

// ---------------------------------------------------------------- the form

#[derive(Clone, Copy, PartialEq, Eq)]
enum Src {
    Blank,
    Base,
    Mod,
    Folder,
}

const PRESETS: [(u32, u32, &str); 7] = [
    (5632, 2048, "vanilla's size"),
    (4608, 2816, ""),
    (4096, 3072, ""),
    (3584, 3584, "square"),
    (4096, 2048, ""),
    (2048, 2048, "small"),
    (1024, 1024, "tiny, for trying things"),
];

struct Job {
    progress: Arc<Progress>,
    rx: Receiver<Result<Created, String>>,
}

pub struct NewForm {
    name: String,
    /// The project file.
    file: String,
    /// The file follows the name until it is edited.
    file_auto: bool,
    src: Src,
    game: String,
    mods: Option<Result<Vec<ModEntry>, String>>,
    mod_sel: Option<usize>,
    mod_folder: String,
    size: [u32; 2],
    fill: Fill,
    proj: ProjEdit,
    /// The pixel size was set by hand (an import sets it from the map's width otherwise).
    proj_touched: bool,
    job: Option<Job>,
    error: Option<String>,
}

impl NewForm {
    pub fn new(import: bool, game_dir: &Path) -> NewForm {
        let name = if import { "Imported map" } else { "New map" }.to_string();
        let mut f = NewForm {
            file: newproject::default_file(&name).display().to_string(),
            name,
            file_auto: true,
            src: if import { Src::Base } else { Src::Blank },
            game: game_dir.display().to_string(),
            mods: None,
            mod_sel: None,
            mod_folder: String::new(),
            size: [5632, 2048],
            fill: Fill::Land,
            proj: ProjEdit::new(&Projection::Plain { pixel_m: 40_075_000.0 / 5632.0 }),
            proj_touched: false,
            job: None,
            error: None,
        };
        if import {
            f.load_mods();
        }
        f
    }

    fn load_mods(&mut self) {
        let game = PathBuf::from(&self.game);
        self.mods = Some(launcher::GameInfo::read(&game).and_then(|g| launcher::scan(&g.user_dir)).map(|v| v.into_iter().filter(|m| m.path.as_ref().is_some_and(|p| p.is_dir())).collect()).map_err(|e| format!("{e:#}")));
    }

    fn source(&self) -> Option<hoi4::Source> {
        let game = PathBuf::from(&self.game);
        match self.src {
            Src::Blank => None,
            Src::Base => Some(hoi4::Source { game, mod_dir: None }),
            Src::Mod => {
                let m = self.mods.as_ref()?.as_ref().ok()?.get(self.mod_sel?)?;
                Some(hoi4::Source { game, mod_dir: m.path.clone() })
            }
            Src::Folder => (!self.mod_folder.trim().is_empty()).then(|| hoi4::Source { game, mod_dir: Some(PathBuf::from(self.mod_folder.trim())) }),
        }
    }

    /// The size of the map: chosen, or the source's provinces.bmp.
    fn map_size(&self) -> Result<(u32, u32), String> {
        match self.source() {
            None if self.src == Src::Blank => Ok((self.size[0], self.size[1])),
            None => Err("choose what to import".into()),
            Some(s) => {
                let own = s.mod_dir.as_ref().map(|m| m.join("map/provinces.bmp")).filter(|p| p.is_file());
                let p = own.unwrap_or_else(|| s.game.join("map/provinces.bmp"));
                project::bmp_size(&p).ok_or_else(|| format!("no map at {}", p.display()))
            }
        }
    }

    fn mod_name(&self) -> Option<String> {
        match self.src {
            Src::Mod => self.mods.as_ref()?.as_ref().ok()?.get(self.mod_sel?).map(|m| m.name.clone()),
            Src::Folder => Some(PathBuf::from(self.mod_folder.trim()).file_name()?.to_string_lossy().into_owned()),
            _ => None,
        }
    }

    fn request(&self) -> Result<Request, String> {
        let (w, h) = self.map_size()?;
        if self.src == Src::Blank {
            mapproj::check_size(w, h)?;
        }
        let proj = self.proj.projection();
        proj.transform(w, h).ok_or("the projection cannot show this centre")?;
        if self.name.trim().is_empty() {
            return Err("the project needs a name".into());
        }
        let start = match self.source() {
            None => Start::Blank { w, h, fill: self.fill },
            Some(s) => Start::Import(s),
        };
        let game_dir = Some(PathBuf::from(&self.game)).filter(|g| *g != crate::modbuild::default_game_dir());
        let mut file = PathBuf::from(self.file.trim());
        if file.as_os_str().is_empty() {
            return Err("the project needs a file".into());
        }
        if !self.file.trim().ends_with(&format!(".{}", crate::bundle::EXT)) {
            file = PathBuf::from(format!("{}.{}", self.file.trim(), crate::bundle::EXT));
        }
        if file.exists() {
            return Err(format!("{} exists already", file.display()));
        }
        Ok(Request { name: self.name.trim().to_string(), file, start, proj, game_dir })
    }

    fn start(&mut self, ctx: &egui::Context) {
        match self.request() {
            Ok(req) => {
                let progress = Arc::new(Progress::default());
                let (tx, rx) = crossbeam_channel::bounded(1);
                let (p, c) = (progress.clone(), ctx.clone());
                std::thread::spawn(move || {
                    let r = newproject::create(&req, &p).map(|(_, c)| c).map_err(|e| format!("{e:#}"));
                    let _ = tx.send(r);
                    c.request_repaint();
                });
                self.job = Some(Job { progress, rx });
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
    }

    pub fn busy(&self) -> bool {
        self.job.is_some()
    }

    /// Returns the made project, or None (Some(Err) for Cancel).
    fn ui(&mut self, ui: &mut egui::Ui) -> Option<Result<Created, ()>> {
        if let Some(job) = &self.job {
            match job.rx.try_recv() {
                Ok(Ok(c)) => {
                    self.job = None;
                    return Some(Ok(c));
                }
                Ok(Err(e)) => {
                    self.job = None;
                    self.error = Some(e);
                }
                Err(_) => {
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
                }
            }
        }
        let mut out = None;
        let busy = self.job.is_some();
        ui.add_enabled_ui(!busy, |ui| {
            egui::Grid::new("new_project").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                ui.label(RichText::new("Name").strong());
                if ui.add(egui::TextEdit::singleline(&mut self.name).desired_width(360.0)).changed() && self.file_auto {
                    self.file = newproject::default_file(&self.name).display().to_string();
                }
                ui.end_row();
                ui.label(RichText::new("File").strong());
                ui.horizontal(|ui| {
                    if ui.add(egui::TextEdit::singleline(&mut self.file).desired_width(360.0)).changed() {
                        self.file_auto = false;
                    }
                    if ui.button("Browse…").clicked() {
                        let cur = PathBuf::from(self.file.trim());
                        let mut dlg = rfd::FileDialog::new().add_filter("mod-helper project", &[crate::bundle::EXT]).set_file_name(format!("{}.{}", newproject::slug(&self.name), crate::bundle::EXT));
                        if let Some(d) = cur.parent().filter(|d| d.is_dir()) {
                            dlg = dlg.set_directory(d);
                        }
                        if let Some(p) = dlg.save_file() {
                            self.file = p.display().to_string();
                            self.file_auto = false;
                        }
                    }
                });
                ui.end_row();
                ui.label("");
                ui.label(
                    RichText::new("The project is this one file: the map and its nations' content. Rasters, OSM extracts and the game stay where they are; the project refers to them.")
                        .small()
                        .weak(),
                );
                ui.end_row();
            });
            ui.add_space(6.0);
            ui.separator();
            ui.label(RichText::new("The map").strong());
            ui.horizontal(|ui| {
                ui.radio_value(&mut self.src, Src::Blank, "Blank");
                ui.radio_value(&mut self.src, Src::Base, "The base game's");
                if ui.radio_value(&mut self.src, Src::Mod, "A mod's").clicked() && self.mods.is_none() {
                    self.load_mods();
                }
                ui.radio_value(&mut self.src, Src::Folder, "A mod folder");
            });
            ui.indent("src", |ui| self.source_ui(ui));
            ui.add_space(6.0);
            ui.separator();
            ui.label(RichText::new("Where the map lies").strong());
            let size = self.map_size().ok();
            let (w, h) = size.unwrap_or((self.size[0], self.size[1]));
            // an import's plain pixel follows its width until set by hand
            if self.src != Src::Blank && !self.proj_touched && self.proj.plain {
                self.proj.plain_km = 40_075.0 / w.max(1) as f64;
            }
            if self.proj.ui(ui, w, h) {
                self.proj_touched = true;
            }
            if !self.proj.plain && self.src != Src::Blank {
                ui.label(RichText::new("HOI4's own maps are not drawn in a real projection: their land will not line up exactly.").small().color(WARN));
            }
        });
        ui.add_space(8.0);
        if let Some(e) = &self.error {
            ui.colored_label(ERR, e);
        }
        if let Some(job) = &self.job {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(job.progress.stage());
            });
            ui.add(egui::ProgressBar::new(job.progress.get()).desired_width(420.0));
        }
        ui.horizontal(|ui| {
            let ok = self.request();
            let create = ui.add_enabled(!busy && ok.is_ok(), egui::Button::new(RichText::new(if self.src == Src::Blank { "Create the project" } else { "Import as a new project" }).strong()));
            if let Err(e) = &ok
                && !busy
            {
                ui.colored_label(WARN, e);
            }
            if create.clicked() {
                self.start(ui.ctx());
            }
            if ui.add_enabled(!busy, egui::Button::new("Cancel")).clicked() {
                out = Some(Err(()));
            }
        });
        out
    }

    fn source_ui(&mut self, ui: &mut egui::Ui) {
        if self.src != Src::Blank {
            ui.horizontal(|ui| {
                ui.label("Game folder");
                if ui.add(egui::TextEdit::singleline(&mut self.game).desired_width(420.0)).lost_focus() {
                    self.mods = None;
                }
                if ui.button("Browse…").clicked()
                    && let Some(p) = rfd::FileDialog::new().set_directory(&self.game).pick_folder()
                {
                    self.game = p.display().to_string();
                    self.mods = None;
                }
            });
        }
        match self.src {
            Src::Blank => {
                ui.horizontal(|ui| {
                    ui.label("Size");
                    let label = |w: u32, h: u32| {
                        let note = PRESETS.iter().find(|p| (p.0, p.1) == (w, h)).map(|p| p.2).filter(|n| !n.is_empty());
                        format!("{w} × {h}{}", note.map(|n| format!(" ({n})")).unwrap_or_default())
                    };
                    egui::ComboBox::from_id_salt("size_preset").selected_text(label(self.size[0], self.size[1])).width(240.0).show_ui(ui, |ui| {
                        for (w, h, _) in PRESETS {
                            if ui.selectable_label(self.size == [w, h], label(w, h)).clicked() {
                                self.size = [w, h];
                            }
                        }
                    });
                    ui.label("or");
                    for i in 0..2 {
                        let r = ui.add(egui::DragValue::new(&mut self.size[i]).speed(16.0).range(256..=16384).suffix(" px"));
                        if r.lost_focus() || r.drag_stopped() {
                            self.size[i] = (self.size[i] as f64 / 256.0).round().max(1.0) as u32 * 256;
                        }
                        if i == 0 {
                            ui.label("×");
                        }
                    }
                });
                let (w, h) = (self.size[0], self.size[1]);
                match mapproj::check_size(w, h) {
                    Ok(()) => ui.label(RichText::new(format!("{:.1} of HOI4's {:.1} million pixels", (w as u64 * h as u64) as f64 / 1e6, mapproj::MAX_PIXELS as f64 / 1e6)).small().weak()),
                    Err(e) => ui.colored_label(ERR, e),
                };
                ui.horizontal(|ui| {
                    ui.label("Filled with");
                    ui.radio_value(&mut self.fill, Fill::Land, "land").on_hover_text("One land province (in a state of a nation, in an air zone) to divide and paint");
                    ui.radio_value(&mut self.fill, Fill::Sea, "sea").on_hover_text("One sea province (in a sea zone) to paint land on");
                });
            }
            Src::Base => {
                self.size_note(ui);
            }
            Src::Mod => {
                if self.mods.is_none() {
                    self.load_mods();
                }
                match &self.mods {
                    Some(Ok(mods)) if mods.is_empty() => {
                        ui.label(RichText::new("No mods (as folders) in the game's mod folder.").weak());
                    }
                    Some(Ok(mods)) => {
                        let sel = self.mod_sel.and_then(|i| mods.get(i)).map_or("choose a mod…".to_string(), |m| m.name.clone());
                        let mut pick = None;
                        egui::ComboBox::from_id_salt("mod_pick").selected_text(sel).width(420.0).height(400.0).show_ui(ui, |ui| {
                            for (i, m) in mods.iter().enumerate() {
                                let has_map = m.path.as_ref().is_some_and(|p| p.join("map/provinces.bmp").is_file());
                                let text = format!("{}{}  ·  {}", m.name, if m.version.is_empty() { String::new() } else { format!(" {}", m.version) }, if has_map { "own map" } else { "the base game's map" });
                                if ui.selectable_label(self.mod_sel == Some(i), text).clicked() {
                                    pick = Some(i);
                                }
                            }
                        });
                        if let Some(i) = pick {
                            self.mod_sel = Some(i);
                            if self.name == "Imported map" || self.name.is_empty() {
                                self.name = mods[i].name.clone();
                                if self.file_auto {
                                    self.file = newproject::default_file(&self.name).display().to_string();
                                }
                            }
                        }
                        if let Some(p) = self.mod_sel.and_then(|i| mods.get(i)).and_then(|m| m.path.clone()) {
                            ui.label(RichText::new(p.display().to_string()).small().weak());
                        }
                    }
                    Some(Err(e)) => {
                        ui.colored_label(ERR, e.clone());
                    }
                    None => {}
                }
                self.size_note(ui);
            }
            Src::Folder => {
                ui.horizontal(|ui| {
                    ui.label("Mod folder");
                    ui.add(egui::TextEdit::singleline(&mut self.mod_folder).desired_width(420.0));
                    if ui.button("Browse…").clicked()
                        && let Some(p) = rfd::FileDialog::new().pick_folder()
                    {
                        self.mod_folder = p.display().to_string();
                        if let Some(n) = self.mod_name()
                            && self.file_auto
                        {
                            self.name = n;
                            self.file = newproject::default_file(&self.name).display().to_string();
                        }
                    }
                });
                self.size_note(ui);
            }
        }
    }

    fn size_note(&self, ui: &mut egui::Ui) {
        match self.map_size() {
            Ok((w, h)) => ui.label(RichText::new(format!("The map keeps its size, {w} × {h} px. Provinces, states, countries, strategic regions, adjacencies, rivers, railways, supply hubs, naval bases, 3D objects, terrain and heights come over; a report lists what does not.")).small().weak()),
            Err(e) => ui.label(RichText::new(e).small().color(WARN)),
        };
    }
}

// ---------------------------------------------------------------- the screen

pub struct Welcome {
    recent: Vec<Recent>,
    pub form: Option<NewForm>,
}

impl Welcome {
    pub fn new() -> Welcome {
        Welcome { recent: project::recent_projects(), form: None }
    }

    /// Show the form for a new project (an import with `import`).
    pub fn new_project(&mut self, import: bool, game_dir: &Path) {
        if !self.form.as_ref().is_some_and(NewForm::busy) {
            self.form = Some(NewForm::new(import, game_dir));
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, game_dir: &Path, unsaved: bool) -> Option<Action> {
        let mut action = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let width = ui.available_width().min(860.0);
            let margin = ((ui.available_width() - width) / 2.0).max(0.0);
            ui.horizontal(|ui| {
                ui.add_space(margin);
                ui.vertical(|ui| {
                    ui.set_width(width);
                    ui.add_space(24.0);
                    ui.heading(RichText::new("mod-helper").size(28.0));
                    ui.label(RichText::new("Maps and nation content for Hearts of Iron IV mods").weak());
                    ui.add_space(16.0);
                    if unsaved {
                        ui.colored_label(WARN, "The open map or content has unsaved changes: they are asked about before another project opens.");
                        ui.add_space(8.0);
                    }
                    if let Some(form) = &mut self.form {
                        ui.heading(if form.src == Src::Blank { "New project" } else { "Import a map as a new project" });
                        ui.add_space(6.0);
                        match form.ui(ui) {
                            Some(Ok(c)) => {
                                self.form = None;
                                action = Some(Action::Created(c));
                            }
                            Some(Err(())) => self.form = None,
                            None => {}
                        }
                        return;
                    }
                    ui.horizontal(|ui| {
                        let big = |t: &str| egui::Button::new(RichText::new(t).size(16.0)).min_size(egui::vec2(0.0, 40.0));
                        if ui.add(big("➕ New project…")).on_hover_text("A blank map of the size you choose").clicked() {
                            self.new_project(false, game_dir);
                        }
                        if ui.add(big("⬇ Import a mod or the base game…")).on_hover_text("A new project with the map of the base game or of a mod").clicked() {
                            self.new_project(true, game_dir);
                        }
                        if ui.add(big("📂 Open a project file…")).on_hover_text("A *.hoi4proj file (an old *.project.json is converted)").clicked() {
                            action = Some(Action::OpenDialog);
                        }
                    });
                    ui.add_space(20.0);
                    ui.label(RichText::new("Recent projects").strong().size(16.0));
                    ui.separator();
                    if self.recent.is_empty() {
                        ui.label(RichText::new("None yet: make a new project, import one, or open a project file.").weak());
                    }
                    let mut forget = None;
                    for r in &self.recent {
                        let exists = r.path.is_file();
                        egui::Frame::group(ui.style()).show(ui, |ui| {
                            ui.set_width(width - 16.0);
                            ui.horizontal(|ui| {
                                ui.vertical(|ui| {
                                    let title = RichText::new(&r.name).size(15.0).strong();
                                    let open = ui.add_enabled(exists, egui::Button::new(title).frame(false)).on_hover_text("Open");
                                    if open.clicked() {
                                        action = Some(Action::Open(r.path.clone()));
                                    }
                                    ui.label(RichText::new(r.path.display().to_string()).small().weak());
                                    let mut line = r.summary.clone();
                                    if !exists {
                                        line = "the file is gone".into();
                                    }
                                    let when = project::ago(r.opened);
                                    if !when.is_empty() {
                                        line = format!("{line} · opened {when}");
                                    }
                                    ui.label(RichText::new(line).small());
                                });
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui.small_button("✖").on_hover_text("Take off the list (the files stay)").clicked() {
                                        forget = Some(r.path.clone());
                                    }
                                    if ui.add_enabled(exists, egui::Button::new("Open")).clicked() {
                                        action = Some(Action::Open(r.path.clone()));
                                    }
                                });
                            });
                        });
                    }
                    if let Some(p) = forget {
                        let _ = project::forget_project(&p);
                        self.recent = project::recent_projects();
                    }
                    ui.add_space(16.0);
                    ui.label(RichText::new(format!("The list is kept in {}", project::data_dir().display())).small().weak());
                });
            });
        });
        action
    }
}

impl Default for Welcome {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------- the window

/// The Map projection window: where the open map lies (or, without a map,
/// the projection the viewer shows the layers in).
pub struct ProjWindow {
    pub edit: ProjEdit,
    /// The display CRS without a map ([`Crs::UNKNOWN`]: plain).
    pub display: Crs,
    /// `edit` starts from the map's projection (else from a default, until
    /// the map is open).
    from_map: bool,
}

/// What the window asks for.
pub enum ProjAction {
    /// Give the open map this projection.
    Map(Projection),
    /// Show the layers in this CRS.
    Display(Crs),
}

impl ProjWindow {
    pub fn new(current: Option<Projection>, display: Crs) -> ProjWindow {
        let from_map = current.is_some();
        ProjWindow { edit: ProjEdit::new(&current.unwrap_or(Projection::Geo { crs: Crs::LAEA_EUROPE, center: [11.35, 46.5], pixel: 1000.0 })), display, from_map }
    }

    /// `map`: the open map's projection and size, and whether it has unsaved changes.
    pub fn ui(&mut self, ctx: &egui::Context, open: &mut bool, map: Option<(Projection, u32, u32, bool)>) -> Option<ProjAction> {
        let mut out = None;
        if let Some((cur, ..)) = map
            && !self.from_map
        {
            self.edit = ProjEdit::new(&cur);
            self.from_map = true;
        }
        egui::Window::new("Map projection").open(open).resizable(false).collapsible(false).default_width(520.0).show(ctx, |ui| match map {
            Some((cur, w, h, dirty)) => {
                ui.label(RichText::new("Now").strong());
                ui.label(cur.describe(w, h));
                ui.add_space(6.0);
                ui.label(RichText::new("New").strong());
                self.edit.ui(ui, w, h);
                let new = self.edit.projection();
                ui.add_space(6.0);
                ui.label(
                    RichText::new("The map's pixels stay as they are; only where they lie on the Earth changes (and the pixel size areas and slopes use). The old georeference and provinces.json are kept with the map's backups (projection-<time>).")
                        .small()
                        .weak(),
                );
                if new.is_plain() {
                    ui.label(RichText::new("OSM layers need a map projection; they are left empty on a plain raster.").small().color(WARN));
                }
                let label = if dirty { "Save the map and apply" } else { "Apply" };
                let same = match (cur, new) {
                    (Projection::Plain { pixel_m: a }, Projection::Plain { pixel_m: b }) => (a - b).abs() < 1e-9,
                    (a, b) => a == b,
                };
                if ui.add_enabled(!same && new.transform(w, h).is_some(), egui::Button::new(label)).clicked() {
                    out = Some(ProjAction::Map(new));
                }
            }
            None => {
                ui.label("The map is not open (yet). Without it, this is the projection the viewer shows its layers in.");
                egui::ComboBox::from_id_salt("display_crs").selected_text(if self.display == Crs::UNKNOWN { "plain (no reprojection)".to_string() } else { self.display.name() }).show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.display, Crs::UNKNOWN, "plain (no reprojection)");
                    for &c in crate::geo::KNOWN_CRS {
                        ui.selectable_value(&mut self.display, Crs::new(c), Crs::new(c).name());
                    }
                });
                if ui.button("Apply").clicked() {
                    out = Some(ProjAction::Display(self.display));
                }
            }
        });
        out
    }
}

/// Metres per unit of the world coordinates around latitude `lat` (for the
/// scale bar).
pub fn metres_per_unit(crs: Crs, center_y: f64) -> f64 {
    if crs.is_geographic() {
        geo::M_PER_DEG * center_y.to_radians().cos().abs().max(1e-6)
    } else if crs.epsg == 3857 {
        crs.to_lonlat(0.0, center_y).map_or(1.0, |(_, lat)| lat.to_radians().cos())
    } else {
        1.0
    }
}

/// "12.3 km" for a length in world units.
pub fn fmt_world(len: f64, crs: Crs, center_y: f64) -> String {
    util::fmt_dist(len * metres_per_unit(crs, center_y))
}
