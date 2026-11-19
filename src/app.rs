//! The application: layer stack management and the user interface.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use eframe::egui::{
    self, Align2, Color32, CursorIcon, FontId, Key, KeyboardShortcut, Modifiers, PointerButton, Rect, RichText, Sense,
};
use rustc_hash::FxHashMap;

use crate::editor::{self, Editor};
use crate::geo::{self, Crs, FileGeoInfo, GeoTransform, KNOWN_CRS};
use crate::layers::{GeorefOverride, Known, Layer, LayerConfig, LayerData, LayerKind, LayerState, RasterLayer, Residency};
use crate::osm;
use crate::bundle;
use crate::project::{self, ProjectFile, RASTER_EXTS};
use crate::raster::{self, Colormap, Raster};
use crate::tiles::TileManager;
use crate::util::{self, Progress};
use crate::vector::{self, VectorData};
use crate::view::{self, Camera, DrawCtx};

enum JobOutput {
    Raster(Raster, FileGeoInfo),
    Vector(VectorData),
}

struct JobDone {
    layer: u64,
    token: u64,
    result: Result<JobOutput, String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tool {
    Pan,
    MoveLayer,
}

enum LayerAction {
    Remove(u64),
    Reload(u64),
    ZoomTo(u64),
    OneToOne(u64),
    Raise(usize),
    Lower(usize),
}

/// Only one OSM extraction at a time: they are memory- and CPU-hungry.
static OSM_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

struct Screenshot {
    path: PathBuf,
    requested: bool,
    done: Arc<AtomicBool>,
    idle_frames: u32,
}

/// What the app opens with.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Start {
    /// As the project was left.
    Project,
    Editor,
    Content,
}

/// The three tools; one is shown at a time.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Viewer,
    Editor,
    Content,
}

pub struct App {
    project_path: PathBuf,
    crs: Crs,
    layers: Vec<Layer>,
    /// Load token per layer id, so results of superseded loads are ignored.
    load_tokens: FxHashMap<u64, u64>,
    selected: Option<u64>,
    cam: Camera,
    /// Fit the view to the data once loading settles.
    pending_fit: bool,
    /// MOD_HELPER_VIEW was given: the editor's zoom to a newly opened map is skipped.
    keep_view: bool,
    user_moved_view: bool,
    zoom_to_on_load: Option<u64>,
    tiles: TileManager,
    jobs_tx: Sender<JobDone>,
    jobs_rx: Receiver<JobDone>,
    next_id: u64,
    tool: Tool,
    hover: Option<[f64; 2]>,
    map_rect: Rect,
    message: Option<(String, bool, Instant)>,
    show_help: bool,
    screenshot: Option<Screenshot>,
    /// Tool #2, the map editor (when `editor.settings.active`).
    editor: Editor,
    /// Tool #3, the content of the nations (when `content.settings.active`).
    content: crate::content::ui::Screen,
    /// The open project (else only the welcome screen is).
    project: Option<bundle::Project>,
    /// [`bundle::writes`] when the project was packed last.
    synced_writes: u64,
    /// A project of the old format to convert (asked first).
    convert: Option<PathBuf>,
    /// The welcome screen, shown instead of the tools.
    welcome: Option<crate::welcome::Welcome>,
    /// A project to open once the unsaved changes are dealt with.
    pending_open: Option<PathBuf>,
    /// The Map projection window.
    proj_win: Option<crate::welcome::ProjWindow>,
    /// What an import brought over (title, lines).
    report: Option<(String, Vec<String>)>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext, project_arg: Option<PathBuf>, start: Start) -> Self {
        let (jobs_tx, jobs_rx) = crossbeam_channel::unbounded();
        let mut app = App {
            project_path: PathBuf::new(),
            crs: Crs::LAEA_EUROPE,
            layers: Vec::new(),
            load_tokens: FxHashMap::default(),
            selected: None,
            cam: Camera::default(),
            pending_fit: true,
            keep_view: false,
            user_moved_view: false,
            zoom_to_on_load: None,
            tiles: TileManager::new(),
            jobs_tx,
            jobs_rx,
            next_id: 1,
            tool: Tool::Pan,
            hover: None,
            map_rect: Rect::NOTHING,
            message: None,
            show_help: false,
            screenshot: std::env::var_os("MOD_HELPER_SCREENSHOT").map(|p| Screenshot {
                path: PathBuf::from(p),
                requested: false,
                done: Arc::new(AtomicBool::new(false)),
                idle_frames: 0,
            }),
            editor: Editor::new(editor::Settings::default()),
            content: crate::content::ui::Screen::new(Default::default()),
            project: None,
            synced_writes: bundle::writes(),
            convert: None,
            welcome: None,
            pending_open: None,
            proj_win: None,
            report: None,
        };
        // started without a project: the welcome screen
        let Some(path) = project_arg else {
            let mut w = crate::welcome::Welcome::new();
            // for screenshots: MOD_HELPER_WELCOME=new|import opens the form
            match std::env::var("MOD_HELPER_WELCOME").as_deref() {
                Ok("new") => w.new_project(false, &app.game_dir()),
                Ok("import") => w.new_project(true, &app.game_dir()),
                _ => {}
            }
            app.welcome = Some(w);
            return app;
        };
        if project::is_legacy(&path) {
            app.welcome = Some(crate::welcome::Welcome::new());
            app.convert = Some(path);
            return app;
        }
        if let Err(e) = app.load_project(&path, &cc.egui_ctx) {
            app.error(format!("{e:#}"));
            app.welcome = Some(crate::welcome::Welcome::new());
            return app;
        }
        if app.screenshot.is_none() {
            let _ = project::remember_project(&app.project_path);
        }
        match start {
            Start::Project => {}
            Start::Editor => app.set_mode(Mode::Editor, &cc.egui_ctx),
            Start::Content => app.set_mode(Mode::Content, &cc.egui_ctx),
        }
        // for screenshots: MOD_HELPER_CONTENT=TAG[/page[/what]] opens the content screen there
        if let Ok(spec) = std::env::var("MOD_HELPER_CONTENT") {
            app.set_mode(Mode::Content, &cc.egui_ctx);
            app.content.start_at(&spec);
        }
        if app.editor.settings.active || app.content.settings.active {
            app.editor.ensure_open(&cc.egui_ctx);
        }
        if let Some(v) = std::env::var("MOD_HELPER_VIEW").ok() {
            let v: Vec<f64> = v.split(',').filter_map(|s| s.trim().parse().ok()).collect();
            if v.len() == 3 {
                app.cam = Camera { center: [v[0], v[1]], mpp: v[2] };
                app.pending_fit = false;
                app.keep_view = true;
            }
        }
        // for screenshots: MOD_HELPER_LAYER=NAME selects that layer (its properties show)
        if let Ok(name) = std::env::var("MOD_HELPER_LAYER") {
            app.selected = app.layers.iter().find(|l| l.cfg.name == name).map(|l| l.id);
        }
        // for screenshots: MOD_HELPER_PROJECTION=1 opens the Map projection window
        if std::env::var_os("MOD_HELPER_PROJECTION").is_some() {
            app.open_projection_window();
        }
        app
    }

    /// The game's folder the editor uses.
    fn game_dir(&self) -> PathBuf {
        self.editor.settings.game_dir.clone().unwrap_or_else(crate::modbuild::default_game_dir)
    }

    /// Unsaved map or content changes.
    fn unsaved(&self) -> bool {
        self.editor.is_dirty() || self.content.is_dirty()
    }

    /// Open a project (after asking about unsaved changes).
    fn open_project(&mut self, path: PathBuf, ctx: &egui::Context) {
        if self.unsaved() {
            self.pending_open = Some(path);
        } else {
            self.switch_project(path, ctx);
        }
    }

    /// Close what is open (its project file is saved, as on exit) and open
    /// the project at `path` (one of the old format is converted first).
    fn switch_project(&mut self, path: PathBuf, ctx: &egui::Context) {
        if project::is_legacy(&path) {
            self.convert = Some(path);
            return;
        }
        self.sync_project(true);
        match self.load_project(&path, ctx) {
            Ok(()) => {
                if let Err(e) = project::remember_project(&self.project_path) {
                    log::warn!("could not update the recent projects: {e:#}");
                }
                self.info(format!("Opened {}", self.project_path.display()));
            }
            Err(e) => self.error(format!("{e:#}")),
        }
    }

    /// Open the project file at `path` in place of what is open (saved
    /// before, or not to be).
    fn load_project(&mut self, path: &Path, ctx: &egui::Context) -> anyhow::Result<()> {
        let (p, pf) = bundle::Project::open(path)?;
        // a fresh editor and content screen: the old map closes
        self.editor = Editor::new(editor::Settings::default());
        self.editor.project = Some(p.path.clone());
        self.content = crate::content::ui::Screen::new(Default::default());
        self.tiles.remove_layer(editor::OVERLAY_LAYER);
        self.proj_win = None;
        self.project_path = p.path.clone();
        let recovered = p.map_changed();
        self.project = Some(p);
        self.synced_writes = bundle::writes();
        self.welcome = None;
        self.cam = Camera::default();
        self.apply_project(pf, ctx);
        if recovered && self.screenshot.is_none() {
            // saved in the working folder, then the app ended before packing
            self.sync_project(false);
            self.info("Changes saved before the app last closed are in the project file now");
        }
        Ok(())
    }

    /// Pack the project's changes into its file: the map's always, the
    /// settings (layers, view…) when `settings`.
    fn sync_project(&mut self, settings: bool) {
        if self.screenshot.is_some() {
            return;
        }
        self.synced_writes = bundle::writes();
        let pf = self.project_file();
        let Some(p) = &mut self.project else { return };
        if let Err(e) = p.save(&pf, settings) {
            self.error(format!("the project file could not be written: {e:#}"));
        }
    }

    /// The old project waiting to be converted: ask where its file goes.
    fn convert_dialog(&mut self, ctx: &egui::Context) {
        let Some(old) = self.convert.clone() else { return };
        let map = ProjectFile::load_legacy(&old).ok().and_then(|pf| pf.editor.and_then(|e| e.map_dir));
        let mut choice = None;
        egui::Modal::new(egui::Id::unique("convert_project")).show(ctx, |ui| {
            ui.set_max_width(560.0);
            ui.heading("A project of the old format");
            ui.label(format!(
                "{} holds the settings of a project whose map is in a folder of its own{}.",
                geo::file_name(&old),
                map.as_ref().map(|m| format!(" ({})", m.display())).unwrap_or_default()
            ));
            ui.label(
                "A project is one file now. Converting puts the map's own files in it (provinces, nation content, painted terrain, adjacency rules); \
                 the layers' rasters and OSM extracts, the elevation and the land cover stay where they are and the project refers to them.",
            );
            ui.label(RichText::new("The old files are only read; the map's backups stay in its folder.").small().weak());
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("Convert…").on_hover_text("Choose where the project file goes").clicked() {
                    choice = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    choice = Some(false);
                }
            });
        });
        match choice {
            Some(true) => {
                let file = old.file_name().map(|n| n.to_string_lossy().trim_end_matches(&format!(".{}", project::LEGACY_EXT)).to_string()).unwrap_or_default();
                let mut dlg = rfd::FileDialog::new().add_filter("mod-helper project", &[bundle::EXT]).set_file_name(format!("{file}.{}", bundle::EXT));
                if let Some(d) = old.parent() {
                    dlg = dlg.set_directory(d);
                }
                let Some(dest) = dlg.save_file() else { return };
                let dest = with_ext(dest);
                match bundle::convert(&old, &dest) {
                    Ok((p, report)) => {
                        self.convert = None;
                        self.report = Some((format!("Converted {}", geo::file_name(&old)), report));
                        let _ = project::forget_project(&old);
                        self.switch_project(p.path, ctx);
                    }
                    Err(e) => self.error(format!("cannot convert {}: {e:#}", old.display())),
                }
            }
            Some(false) => self.convert = None,
            None => {}
        }
    }

    fn show_welcome(&mut self, new: Option<bool>) {
        let game = self.game_dir();
        let w = self.welcome.get_or_insert_with(crate::welcome::Welcome::new);
        if let Some(import) = new {
            w.new_project(import, &game);
        }
    }

    /// Ask about unsaved changes before another project opens.
    fn pending_open_dialog(&mut self, ctx: &egui::Context) {
        let Some(path) = self.pending_open.clone() else { return };
        let mut choice = None;
        egui::Modal::new(egui::Id::unique("pending_open")).show(ctx, |ui| {
            ui.heading("Unsaved changes");
            ui.label("The open map or nation content has changes that are not saved.");
            ui.label(RichText::new(format!("Then open {}", path.display())).small().weak());
            ui.horizontal(|ui| {
                if ui.button("Save and open").clicked() {
                    choice = Some(0);
                }
                if ui.button("Open without saving").clicked() {
                    choice = Some(1);
                }
                if ui.button("Cancel").clicked() {
                    choice = Some(2);
                }
            });
        });
        match choice {
            Some(0) => {
                if self.content.is_dirty() {
                    self.content.save(&mut self.editor);
                }
                if self.editor.is_dirty() {
                    self.editor.save();
                }
                self.pending_open = None;
                if self.unsaved() {
                    self.error("could not save everything; the project stays open");
                } else {
                    self.switch_project(path, ctx);
                }
            }
            Some(1) => {
                self.pending_open = None;
                self.switch_project(path, ctx);
            }
            Some(_) => self.pending_open = None,
            None => {}
        }
    }

    /// The open map's projection, size and unsaved state.
    fn map_projection(&self) -> Option<(crate::mapproj::Projection, u32, u32, bool)> {
        let d = self.editor.doc.as_ref()?;
        Some((crate::mapproj::Projection::of_map(&d.geo, d.w, d.h, d.data.pixel_m), d.w, d.h, self.editor.is_dirty()))
    }

    fn open_projection_window(&mut self) {
        let cur = self.map_projection().map(|m| m.0);
        self.proj_win = Some(crate::welcome::ProjWindow::new(cur, self.crs));
    }

    fn projection_window(&mut self, ctx: &egui::Context) {
        let map = self.map_projection();
        let Some(win) = &mut self.proj_win else { return };
        let mut open = true;
        let act = win.ui(ctx, &mut open, map);
        if !open {
            self.proj_win = None;
        }
        match act {
            Some(crate::welcome::ProjAction::Display(c)) => {
                self.set_crs(c, ctx);
                self.info(format!("The layers are shown in {}", if c == Crs::UNKNOWN { "no projection".into() } else { c.name() }));
            }
            Some(crate::welcome::ProjAction::Map(p)) => {
                if self.content.is_dirty() {
                    self.content.save(&mut self.editor);
                }
                if self.editor.is_dirty() {
                    self.editor.save();
                    if self.editor.is_dirty() {
                        self.error("the map could not be saved; its projection is unchanged");
                        return;
                    }
                }
                let Some(d) = self.editor.doc.as_ref() else { return };
                let (dir, w, h) = (d.dir.clone(), d.w, d.h);
                match crate::mapproj::apply(&dir, w, h, &p) {
                    Ok(backup) => {
                        self.set_crs(p.crs(), ctx);
                        // read again with its new georeference
                        self.editor.open(dir, ctx);
                        self.proj_win = None;
                        self.info(format!("The map is now a {}; the old georeference is in {}", p.describe(w, h), backup.display()));
                    }
                    Err(e) => self.error(format!("{e:#}")),
                }
            }
            None => {}
        }
    }

    /// Show the layers in another CRS: the OSM layers are projected again.
    fn set_crs(&mut self, crs: Crs, ctx: &egui::Context) {
        if crs == self.crs {
            return;
        }
        self.crs = crs;
        let osm: Vec<u64> = self.layers.iter().filter(|l| l.cfg.kind == LayerKind::OsmBoundaries).map(|l| l.id).collect();
        for id in osm {
            self.apply_action(LayerAction::Reload(id), ctx);
        }
        self.pending_fit = true;
        self.user_moved_view = false;
    }

    fn report_window(&mut self, ctx: &egui::Context) {
        let Some((title, lines)) = &self.report else { return };
        let mut open = true;
        egui::Window::new(title.as_str()).open(&mut open).default_width(620.0).collapsible(false).show(ctx, |ui| {
            egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                for l in lines {
                    let c = if l.starts_with("not imported") || l.starts_with("left out") { Color32::from_rgb(255, 190, 90) } else { ui.visuals().text_color() };
                    ui.label(RichText::new(l).color(c));
                }
            });
        });
        if !open {
            self.report = None;
        }
    }

    // ---------------------------------------------------------------- state

    fn info(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        log::info!("{msg}");
        self.message = Some((msg, false, Instant::now()));
    }

    fn error(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        log::error!("{msg}");
        self.message = Some((msg, true, Instant::now()));
    }

    fn apply_project(&mut self, pf: ProjectFile, ctx: &egui::Context) {
        for l in self.layers.drain(..) {
            self.tiles.remove_layer(l.id);
        }
        self.selected = None;
        self.crs = pf.crs;
        self.pending_fit = pf.view.is_none();
        self.user_moved_view = false;
        if let Some(v) = pf.view {
            self.cam = v;
        }
        for cfg in pf.layers {
            self.add_layer(cfg, ctx);
        }
        let mut settings = pf.editor.unwrap_or_default();
        if let Some(p) = &self.project {
            settings.map_dir = Some(p.map_dir());
        }
        if self.editor.doc.is_some() {
            // keep the open map
            settings.map_dir = self.editor.settings.map_dir.clone();
            settings.active |= self.editor.settings.active;
        }
        self.editor.settings = settings;
        let content = pf.content.unwrap_or_default();
        self.content.settings = crate::content::ui::Settings { active: content.active && !self.editor.settings.active, ..content };
        if self.editor.settings.active || self.content.settings.active {
            self.editor.ensure_open(ctx);
        }
    }

    fn mode(&self) -> Mode {
        if self.content.settings.active {
            Mode::Content
        } else if self.editor.settings.active {
            Mode::Editor
        } else {
            Mode::Viewer
        }
    }

    fn set_mode(&mut self, m: Mode, ctx: &egui::Context) {
        self.editor.settings.active = m == Mode::Editor;
        self.content.settings.active = m == Mode::Content;
        if m != Mode::Viewer {
            self.tool = Tool::Pan;
            self.editor.ensure_open(ctx);
        }
    }

    fn project_file(&self) -> ProjectFile {
        ProjectFile {
            version: 1,
            crs: self.crs,
            layers: self.layers.iter().map(|l| l.cfg.clone()).collect(),
            view: Some(self.cam),
            editor: Some(self.editor.settings.clone()),
            content: Some(self.content.settings.clone()),
        }
    }

    /// Save all: the map and the content when they have changes, then the
    /// project file.
    fn save_project(&mut self) {
        if self.project.is_none() {
            return;
        }
        if self.content.is_dirty() {
            self.content.save(&mut self.editor);
        }
        if self.editor.is_dirty() {
            self.editor.save();
        }
        self.sync_project(true);
        if self.message.as_ref().is_none_or(|m| !m.1 || m.2.elapsed() > Duration::from_secs(1)) {
            self.info(format!("Saved {}", self.project_path.display()));
        }
    }

    fn add_layer(&mut self, cfg: LayerConfig, ctx: &egui::Context) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let mut layer = Layer { id, cfg, state: LayerState::Unloaded, known: None };
        // a hidden layer is read only when shown
        if layer.cfg.visible {
            self.start_load(&mut layer, ctx);
        }
        self.layers.push(layer);
        id
    }

    /// Reads the layers that are shown and not in memory, and drops the
    /// hidden ones (a failed one is tried again when shown again).
    fn sync_loaded(&mut self, ctx: &egui::Context) {
        let mut freed = false;
        for i in 0..self.layers.len() {
            let (visible, id) = (self.layers[i].cfg.visible, self.layers[i].id);
            match self.layers[i].state.residency(visible) {
                Residency::Load => {
                    let mut l = self.layers.remove(i);
                    self.start_load(&mut l, ctx);
                    self.layers.insert(i, l);
                }
                Residency::Drop => {
                    // a read still running is dropped when it ends
                    *self.load_tokens.entry(id).or_insert(0) += 1;
                    self.tiles.remove_layer(id);
                    self.layers[i].state = LayerState::Unloaded;
                    freed = true;
                }
                Residency::Retry => self.layers[i].state = LayerState::Unloaded,
                Residency::Keep => {}
            }
        }
        if freed {
            util::release_freed_memory();
        }
    }

    fn start_load(&mut self, layer: &mut Layer, ctx: &egui::Context) {
        let progress = Arc::new(Progress::default());
        layer.state = LayerState::Loading(progress.clone());
        let token = self.load_tokens.get(&layer.id).copied().unwrap_or(0) + 1;
        self.load_tokens.insert(layer.id, token);
        let (tx, ctx, path, kind, crs, id) =
            (self.jobs_tx.clone(), ctx.clone(), layer.cfg.path.clone(), layer.cfg.kind, self.crs, layer.id);
        std::thread::Builder::new()
            .name(format!("load-{id}"))
            .spawn(move || {
                let t0 = Instant::now();
                let result = match kind {
                    LayerKind::Raster => raster::load(&path, &progress).map(|(r, g)| JobOutput::Raster(r, g)),
                    LayerKind::OsmBoundaries => {
                        progress.set_stage("Waiting for another OSM import");
                        let _guard = OSM_LOCK.lock();
                        osm::load_boundaries(&path, &progress).map(|b| {
                            progress.set_stage("Projecting & simplifying");
                            JobOutput::Vector(VectorData::build(&b, crs))
                        })
                    }
                };
                log::info!("loaded {} in {:.2}s", path.display(), t0.elapsed().as_secs_f32());
                let _ = tx.send(JobDone { layer: id, token, result: result.map_err(|e| format!("{e:#}")) });
                ctx.request_repaint();
            })
            .expect("spawn loader thread");
    }

    fn poll_jobs(&mut self) {
        while let Ok(done) = self.jobs_rx.try_recv() {
            if self.load_tokens.get(&done.layer) != Some(&done.token) {
                continue;
            }
            let Some(layer) = self.layers.iter_mut().find(|l| l.id == done.layer) else { continue };
            match done.result {
                Ok(JobOutput::Raster(r, g)) => {
                    let data = LayerData::Raster(RasterLayer::new(r, g, &layer.cfg.raster_style));
                    layer.known = Some(Known::of(&data));
                    layer.state = LayerState::Ready(data);
                }
                Ok(JobOutput::Vector(v)) => {
                    for &lvl in v.level_counts.keys() {
                        layer.cfg.vector_style.level(lvl);
                    }
                    for &w in v.water_counts.keys() {
                        layer.cfg.vector_style.water_mut(w);
                    }
                    let data = LayerData::Vector(v);
                    layer.known = Some(Known::of(&data));
                    layer.state = LayerState::Ready(data);
                }
                Err(e) => {
                    let name = layer.cfg.name.clone();
                    layer.state = LayerState::Failed(e.clone());
                    self.error(format!("Failed to load {name}: {e}"));
                }
            }
        }
    }

    /// Re-render tiles of rasters whose display style was edited.
    fn sync_styles(&mut self) {
        for l in &mut self.layers {
            if let LayerState::Ready(LayerData::Raster(r)) = &mut l.state {
                if *r.style != l.cfg.raster_style {
                    r.style = Arc::new(l.cfg.raster_style.clone());
                    r.generation += 1;
                }
            }
        }
    }

    /// Give rasters without georeferencing a placement: copy it from a
    /// georeferenced raster of identical size (HOI4 maps all share one size),
    /// otherwise put the image in the middle of the view at 1 px = 1 screen point.
    fn resolve_missing_georefs(&mut self) {
        if self.layers.iter().any(|l| l.cfg.kind == LayerKind::Raster && l.is_loading()) {
            return;
        }
        for i in 0..self.layers.len() {
            let l = &self.layers[i];
            let Some(r) = l.raster() else { continue };
            if l.georef().is_some() {
                continue;
            }
            let size = (r.width, r.height);
            // also of a hidden layer read before
            let matched = self.layers.iter().filter(|o| o.id != l.id).find_map(|o| {
                let (w, h, _) = o.shape()?;
                let g = o.georef()?;
                ((w, h) == size && !g.source.starts_with("placed")).then(|| (g, o.cfg.name.clone()))
            });
            let ov = match matched {
                Some((g, name)) => GeorefOverride { transform: g.transform, crs: g.crs, note: format!("copied from {name} (same size)") },
                None => {
                    let ps = self.cam.mpp;
                    let c = self.cam.center;
                    GeorefOverride {
                        transform: GeoTransform::north_up(c[0] - size.0 as f64 * ps / 2.0, c[1] + size.1 as f64 * ps / 2.0, ps, ps),
                        crs: self.crs,
                        note: "placed at view (file has no georeference)".into(),
                    }
                }
            };
            log::info!("{}: {}", self.layers[i].cfg.name, ov.note);
            self.layers[i].cfg.georef_override = Some(ov);
        }
    }

    fn layer_index(&self, id: u64) -> Option<usize> {
        self.layers.iter().position(|l| l.id == id)
    }

    fn data_bounds(&self) -> Option<[f64; 4]> {
        let union = |it: &mut dyn Iterator<Item = [f64; 4]>| {
            it.reduce(|a, b| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])])
        };
        union(&mut self.layers.iter().filter(|l| l.shape().is_some()).filter_map(|l| l.world_bounds(self.crs)))
            .or_else(|| union(&mut self.layers.iter().filter_map(|l| l.world_bounds(self.crs))))
    }

    fn zoom_to_layer(&mut self, id: u64) {
        let Some(i) = self.layer_index(id) else { return };
        if let Some(b) = self.layers[i].world_bounds(self.crs) {
            self.cam.fit(self.map_rect, b);
            self.user_moved_view = true;
        } else if self.layers[i].is_unloaded() {
            // never read: show it, and frame it once it is
            self.layers[i].cfg.visible = true;
            self.zoom_to_on_load = Some(id);
        }
    }

    /// Zoom so one pixel of the layer is one physical screen pixel.
    fn one_to_one(&mut self, id: u64, ctx: &egui::Context) {
        if let Some(pl) = self.layer_index(id).and_then(|i| self.layers[i].placement(self.crs)) {
            self.cam.mpp = pl.pixel_size_world() / ctx.pixels_per_point() as f64;
            self.user_moved_view = true;
        }
    }

    fn import_paths(&mut self, paths: Vec<PathBuf>, ctx: &egui::Context) {
        for p in paths {
            let cfg = LayerConfig::new(p);
            if cfg.kind == LayerKind::OsmBoundaries && !self.crs.is_supported() {
                self.error(format!("{}: OSM data needs a map projection (File → Map projection…)", cfg.name));
                continue;
            }
            self.info(format!("Importing {}", cfg.name));
            let id = self.add_layer(cfg, ctx);
            self.selected = Some(id);
            self.zoom_to_on_load = Some(id);
        }
    }

    fn import_dialog(&mut self, kind: LayerKind, ctx: &egui::Context) {
        let mut dlg = rfd::FileDialog::new();
        if let Some(d) = self.project_path.parent().filter(|d| d.is_dir()) {
            dlg = dlg.set_directory(d);
        }
        dlg = match kind {
            LayerKind::Raster => dlg.add_filter("Rasters (BMP, TIFF, PNG, JPEG)", RASTER_EXTS),
            LayerKind::OsmBoundaries => dlg.add_filter("OpenStreetMap extracts (*.osm.pbf)", &["pbf"]),
        };
        if let Some(paths) = dlg.pick_files() {
            self.import_paths(paths, ctx);
        }
    }

    fn open_project_dialog(&mut self, ctx: &egui::Context) {
        let mut dlg = rfd::FileDialog::new().add_filter("mod-helper projects", &[bundle::EXT, "json"]);
        if let Some(dir) = self.project_path.parent().filter(|d| d.is_dir()) {
            dlg = dlg.set_directory(dir);
        }
        if let Some(p) = dlg.pick_file() {
            self.open_project(p, ctx);
        }
    }

    /// Save the project as another file, which is the project from then on.
    fn save_as_dialog(&mut self, ctx: &egui::Context) {
        if self.project.is_none() {
            return;
        }
        let mut dlg = rfd::FileDialog::new().add_filter("mod-helper project", &[bundle::EXT]).set_file_name(geo::file_name(&self.project_path));
        if let Some(dir) = self.project_path.parent() {
            dlg = dlg.set_directory(dir);
        }
        let Some(dest) = dlg.save_file() else { return };
        let dest = with_ext(dest);
        // what is not saved goes in too
        if self.content.is_dirty() {
            self.content.save(&mut self.editor);
        }
        if self.editor.is_dirty() {
            self.editor.save();
        }
        let pf = self.project_file();
        let Some(p) = self.project.as_mut() else { return };
        match p.save_as(&dest, &pf) {
            Ok(()) => {
                let path = p.path.clone();
                self.synced_writes = bundle::writes();
                // the working folder moved: open it again from there
                match self.load_project(&path, ctx) {
                    Ok(()) => {
                        let _ = project::remember_project(&path);
                        self.info(format!("Saved as {}", path.display()));
                    }
                    Err(e) => self.error(format!("{e:#}")),
                }
            }
            Err(e) => self.error(format!("{e:#}")),
        }
    }

    fn apply_action(&mut self, action: LayerAction, ctx: &egui::Context) {
        match action {
            LayerAction::Remove(id) => {
                if let Some(i) = self.layer_index(id) {
                    let l = self.layers.remove(i);
                    self.tiles.remove_layer(id);
                    self.load_tokens.remove(&id);
                    if self.selected == Some(id) {
                        self.selected = None;
                    }
                    self.info(format!("Removed layer {}", l.cfg.name));
                }
            }
            LayerAction::Reload(id) => {
                if let Some(i) = self.layer_index(id) {
                    self.tiles.remove_layer(id);
                    let mut l = self.layers.remove(i);
                    l.known = None;
                    if l.cfg.visible {
                        self.start_load(&mut l, ctx);
                    } else {
                        // read when shown
                        *self.load_tokens.entry(id).or_insert(0) += 1;
                        l.state = LayerState::Unloaded;
                    }
                    self.layers.insert(i, l);
                }
            }
            LayerAction::ZoomTo(id) => self.zoom_to_layer(id),
            LayerAction::OneToOne(id) => self.one_to_one(id, ctx),
            LayerAction::Raise(i) => {
                if i + 1 < self.layers.len() {
                    self.layers.swap(i, i + 1);
                }
            }
            LayerAction::Lower(i) => {
                if i > 0 {
                    self.layers.swap(i, i - 1);
                }
            }
        }
    }

    // ---------------------------------------------------------------- input

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        if self.welcome.is_some() {
            if ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::O))) {
                self.open_project_dialog(ctx);
            }
            return;
        }
        if self.content.settings.active {
            // the content screen has its own keys (Ctrl+S saves the content and the map)
            let saving = ctx.input(|i| i.modifiers.command && i.key_pressed(Key::S));
            self.content.shortcuts(ctx, &mut self.editor);
            if saving {
                self.save_project();
            }
            return;
        }
        let shortcut = |m, k| ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(m, k)));
        if shortcut(Modifiers::COMMAND, Key::S) {
            if self.editor.settings.active && self.editor.is_dirty() {
                self.editor.save();
            }
            self.save_project();
        }
        if shortcut(Modifiers::COMMAND, Key::O) {
            self.open_project_dialog(ctx);
        }
        if shortcut(Modifiers::COMMAND, Key::I) {
            self.import_dialog(LayerKind::Raster, ctx);
        }
        let editing = self.editor.settings.active;
        if editing {
            self.editor.shortcuts(ctx);
        }
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let (keys, shift) = ctx.input(|i| {
            let k = [Key::F, Key::M, Key::P, Key::Escape, Key::H, Key::Plus, Key::Equals, Key::Minus, Key::F1];
            (k.map(|k| i.key_pressed(k) && !i.modifiers.command), i.modifiers.shift)
        });
        let [fit, mv, pan, esc, hide, plus, eq, minus, help] = keys;
        if fit {
            let b = if editing { self.editor.bounds(self.crs).or_else(|| self.data_bounds()) } else { self.data_bounds() };
            if let Some(b) = b {
                self.cam.fit(self.map_rect, b);
            }
        }
        if mv && !editing {
            self.tool = if self.tool == Tool::MoveLayer { Tool::Pan } else { Tool::MoveLayer };
        }
        if pan || esc {
            self.tool = Tool::Pan;
        }
        if pan && editing {
            self.editor.set_pan_tool();
        }
        if hide {
            if let Some(i) = self.selected.and_then(|id| self.layer_index(id)) {
                self.layers[i].cfg.visible ^= true;
            }
        }
        if plus || eq || minus {
            let c = self.map_rect.center();
            self.cam.zoom_at(self.map_rect, c, if minus { 0.5 } else { 2.0 });
        }
        if help {
            self.show_help ^= true;
        }
        // Arrow keys nudge the selected layer by one of its pixels in move mode.
        if self.tool == Tool::MoveLayer {
            let dir = ctx.input(|i| {
                let mut d = [0.0f64; 2];
                if i.key_pressed(Key::ArrowLeft) {
                    d[0] -= 1.0;
                }
                if i.key_pressed(Key::ArrowRight) {
                    d[0] += 1.0;
                }
                if i.key_pressed(Key::ArrowUp) {
                    d[1] += 1.0;
                }
                if i.key_pressed(Key::ArrowDown) {
                    d[1] -= 1.0;
                }
                d
            });
            if dir != [0.0, 0.0] {
                if let Some(i) = self.selected.and_then(|id| self.layer_index(id)) {
                    let step = self.layers[i].placement(self.crs).map(|p| p.pixel_size_world()).unwrap_or(self.cam.mpp)
                        * if shift { 10.0 } else { 1.0 };
                    self.layers[i].cfg.offset[0] += dir[0] * step;
                    self.layers[i].cfg.offset[1] += dir[1] * step;
                }
            }
        }
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        if !dropped.is_empty() {
            self.import_paths(dropped, ctx);
        }
    }

    // ---------------------------------------------------------------- UI

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("Welcome screen").clicked() {
                    self.show_welcome(None);
                }
                if ui.button("New project…").clicked() {
                    self.show_welcome(Some(false));
                }
                if ui.button("Import a mod or the base game…").clicked() {
                    self.show_welcome(Some(true));
                }
                if ui.button("Open project…  (Ctrl+O)").clicked() {
                    self.open_project_dialog(&ctx);
                }
                let open = self.project.is_some();
                if ui.add_enabled(open, egui::Button::new("Save project  (Ctrl+S)")).on_hover_text("The map, the nation content and the settings, into the project file").clicked() {
                    self.save_project();
                }
                if ui.add_enabled(open, egui::Button::new("Save project as…")).clicked() {
                    self.save_as_dialog(&ctx);
                }
                ui.separator();
                if ui.add_enabled(open, egui::Button::new("Map projection…")).on_hover_text("Where the map lies on the Earth: a plain raster or a map projection").clicked() {
                    self.open_projection_window();
                }
                ui.separator();
                if ui.button("Import raster (BMP/TIFF/PNG)…  (Ctrl+I)").clicked() {
                    self.import_dialog(LayerKind::Raster, &ctx);
                }
                if ui.button("Import OSM boundaries (.osm.pbf)…").clicked() {
                    self.import_dialog(LayerKind::OsmBoundaries, &ctx);
                }
                ui.separator();
                if ui.button("Quit").clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button("View", |ui| {
                if ui.button("Fit all  (F)").clicked() {
                    if let Some(b) = self.data_bounds() {
                        self.cam.fit(self.map_rect, b);
                    }
                }
                let sel = self.selected;
                if ui.add_enabled(sel.is_some(), egui::Button::new("Zoom to selected layer")).clicked() {
                    self.zoom_to_layer(sel.unwrap());
                }
                if ui.add_enabled(sel.is_some(), egui::Button::new("Selected layer at 1:1 pixels")).clicked() {
                    self.one_to_one(sel.unwrap(), &ctx);
                }
            });
            ui.menu_button("Help", |ui| {
                if ui.button("Controls  (F1)").clicked() {
                    self.show_help = true;
                }
            });
            ui.separator();
            if self.welcome.is_some() {
                if self.project.is_some() && ui.button(format!("← Back to {}", geo::file_name(&self.project_path))).clicked() {
                    self.welcome = None;
                }
                return;
            }
            let mode = self.mode();
            let editing = mode == Mode::Editor;
            if ui.selectable_label(mode == Mode::Viewer, "🗺 Viewer").on_hover_text("Tool #1: layers of maps and boundaries").clicked() {
                self.set_mode(Mode::Viewer, &ctx);
            }
            if ui.selectable_label(editing, "✏ Map editor")
                .on_hover_text("Tool #2: provinces, states, nations, air and sea zones, adjacencies of the mod")
                .clicked()
            {
                self.set_mode(Mode::Editor, &ctx);
            }
            if ui.selectable_label(mode == Mode::Content, "📜 Nation content")
                .on_hover_text("Tool #3: focus trees, events, decisions, ideas, characters... of the nations, with checks, imports and a test play")
                .clicked()
            {
                self.set_mode(Mode::Content, &ctx);
            }
            ui.separator();
            if mode == Mode::Content {
                if self.content.is_dirty() {
                    ui.label(RichText::new("● unsaved content (Ctrl+S)").color(Color32::from_rgb(255, 190, 90)));
                }
                if self.editor.is_dirty() {
                    ui.label(RichText::new("● unsaved map changes").color(Color32::from_rgb(255, 190, 90)));
                }
            } else if !editing {
                ui.selectable_value(&mut self.tool, Tool::Pan, "✋ Pan").on_hover_text("Drag to pan the map (P / Esc)");
                ui.selectable_value(&mut self.tool, Tool::MoveLayer, "✥ Move layer")
                    .on_hover_text("Drag to move the selected layer; arrow keys nudge by one pixel, Shift ×10 (M)");
            } else if self.editor.is_dirty() {
                ui.label(RichText::new("● unsaved map changes (Ctrl+S)").color(Color32::from_rgb(255, 190, 90)));
            }
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let plain = self.crs == Crs::UNKNOWN;
            if let Some(w) = self.hover.filter(|_| self.welcome.is_none()) {
                if plain {
                    ui.monospace(format!("x {:>10.1}  y {:>10.1}", w[0], -w[1]));
                } else {
                    ui.monospace(format!("E {:>10.1}  N {:>10.1}", w[0], w[1]));
                }
                if let Some((lon, lat)) = self.crs.to_lonlat(w[0], w[1]) {
                    ui.monospace(geo::fmt_lonlat(lon, lat));
                }
                if let Some(p) = self.editor.placement(self.crs).filter(|_| self.editor.settings.active).and_then(|pl| pl.world_to_pixel(w)) {
                    ui.monospace(format!("px {:.0}, {:.0}", p[0].floor(), p[1].floor()));
                }
            } else if self.project.is_none() {
                ui.label("no project open");
            } else {
                ui.monospace(if plain { "plain raster".to_string() } else { self.crs.name() });
            }
            if self.project.is_none() || self.welcome.is_some() {
                if let Some((msg, err, t)) = self.message.as_ref().filter(|m| m.2.elapsed() < Duration::from_secs(12)) {
                    ui.separator();
                    let _ = t;
                    ui.label(RichText::new(msg).color(if *err { Color32::from_rgb(255, 110, 110) } else { ui.visuals().text_color() }));
                }
                return;
            }
            ui.separator();
            let per_px = self.cam.mpp / ui.ctx().pixels_per_point() as f64;
            ui.label(format!("{} / screen px", crate::welcome::fmt_world(per_px, self.crs, self.cam.center[1])));
            ui.separator();
            let (n, bytes, pending) = self.tiles.stats();
            ui.label(format!("{n} tiles ({})", util::fmt_bytes(bytes)))
                .on_hover_text("Display tiles in GPU memory (LRU-evicted above 1 GB)");
            if pending > 0 {
                ui.spinner();
            }
            let loading = self.layers.iter().filter(|l| l.is_loading()).count();
            if loading > 0 {
                ui.separator();
                ui.spinner();
                ui.label(format!("loading {loading} layer(s)"));
            }
            let editor_msg = if self.content.settings.active {
                self.content.message().or(self.editor.message())
            } else if self.editor.settings.active {
                self.editor.message()
            } else {
                None
            };
            if let Some((msg, err)) = editor_msg {
                ui.separator();
                let color = if err { Color32::from_rgb(255, 110, 110) } else { ui.visuals().text_color() };
                ui.label(RichText::new(msg).color(color));
            } else if let Some((msg, err, t)) = &self.message {
                if t.elapsed() < Duration::from_secs(12) {
                    ui.separator();
                    let color = if *err { Color32::from_rgb(255, 110, 110) } else { ui.visuals().text_color() };
                    ui.label(RichText::new(msg).color(color));
                }
            }
        });
    }

    fn side_panel(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        ui.horizontal(|ui| {
            ui.heading("Layers");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("+ OSM").on_hover_text("Import administrative boundaries from an .osm.pbf extract").clicked() {
                    self.import_dialog(LayerKind::OsmBoundaries, &ctx);
                }
                if ui.button("+ Raster").on_hover_text("Import a BMP / TIFF / PNG / JPEG map").clicked() {
                    self.import_dialog(LayerKind::Raster, &ctx);
                }
            });
        });
        ui.label(RichText::new("Top of the list is drawn on top. Drop files on the map to import.").small().weak());
        ui.separator();

        let mut action = None;
        let n = self.layers.len();
        egui::ScrollArea::vertical().id_salt("layer_list").max_height(ui.available_height() * 0.35).auto_shrink([false, true]).show(
            ui,
            |ui| {
                for idx in (0..n).rev() {
                    let l = &mut self.layers[idx];
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut l.cfg.visible, "").on_hover_text("Show / hide (H)");
                        if ui.add_enabled(idx + 1 < n, egui::Button::new("⏶").small()).on_hover_text("Move up").clicked() {
                            action = Some(LayerAction::Raise(idx));
                        }
                        if ui.add_enabled(idx > 0, egui::Button::new("⏷").small()).on_hover_text("Move down").clicked() {
                            action = Some(LayerAction::Lower(idx));
                        }
                        let icon = match l.cfg.kind {
                            LayerKind::Raster => "🖼",
                            LayerKind::OsmBoundaries => "🗺",
                        };
                        let selected = self.selected == Some(l.id);
                        let mut text = RichText::new(format!("{icon} {}", l.cfg.name));
                        if !l.cfg.visible {
                            text = text.weak();
                        }
                        if ui.selectable_label(selected, text).clicked() {
                            self.selected = if selected { None } else { Some(l.id) };
                        }
                        match &l.state {
                            LayerState::Loading(p) => {
                                ui.spinner();
                                ui.label(RichText::new(format!("{:.0}%", p.get() * 100.0)).small()).on_hover_text(p.stage());
                            }
                            LayerState::Failed(e) => {
                                ui.colored_label(Color32::from_rgb(255, 110, 110), "⚠").on_hover_text(e);
                            }
                            LayerState::Unloaded | LayerState::Ready(_) => {}
                        }
                    });
                }
                if n == 0 {
                    ui.label(RichText::new("No layers yet.").weak());
                }
            },
        );
        if let Some(a) = action.take() {
            self.apply_action(a, &ctx);
        }
        ui.separator();

        egui::ScrollArea::vertical().id_salt("props").auto_shrink([false, false]).show(ui, |ui| {
            if let Some(i) = self.selected.and_then(|id| self.layer_index(id)) {
                // Georeferences of other rasters, for "copy from".
                let others: Vec<(String, GeorefOverride)> = self
                    .layers
                    .iter()
                    .filter(|o| o.id != self.layers[i].id)
                    .filter_map(|o| {
                        let g = o.georef()?;
                        Some((o.cfg.name.clone(), GeorefOverride { transform: g.transform, crs: g.crs, note: format!("copied from {}", o.cfg.name) }))
                    })
                    .collect();
                let crs = self.crs;
                action = layer_properties(ui, &mut self.layers[i], &others, crs);
            } else {
                ui.label(RichText::new("Select a layer to see its properties.").weak());
            }
            ui.add_space(8.0);
            egui::CollapsingHeader::new("Inspector (values under cursor)").default_open(true).show(ui, |ui| self.inspector(ui));
        });
        if let Some(a) = action {
            self.apply_action(a, &ctx);
        }
    }

    fn inspector(&self, ui: &mut egui::Ui) {
        let Some(w) = self.hover else {
            ui.label(RichText::new("Hover over the map.").weak());
            return;
        };
        egui::Grid::new("inspector").num_columns(3).striped(true).show(ui, |ui| {
            for l in self.layers.iter().rev() {
                let Some(r) = l.raster() else { continue };
                if !l.cfg.visible && self.selected != Some(l.id) {
                    continue;
                }
                let Some(pl) = l.placement(self.crs) else { continue };
                let Some(p) = pl.world_to_pixel(w) else { continue };
                if p[0] < 0.0 || p[1] < 0.0 || p[0] >= r.width as f64 || p[1] >= r.height as f64 {
                    continue;
                }
                let (x, y) = (p[0] as u32, p[1] as u32);
                let value = r.raster.read().value_at(x, y);
                ui.label(&l.cfg.name);
                ui.monospace(format!("{x},{y}"));
                ui.horizontal(|ui| {
                    let swatch = match value {
                        Some(raster::PixelValue::Index(_, c) | raster::PixelValue::Rgb(c)) => Some(Color32::from_rgb(c[0], c[1], c[2])),
                        Some(raster::PixelValue::Rgba(c)) => Some(Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3])),
                        _ => None,
                    };
                    if let Some(c) = swatch {
                        let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), Sense::hover());
                        ui.painter().rect_filled(rect, 2.0, c);
                    }
                    ui.monospace(value.map(|v| v.to_string()).unwrap_or_default());
                });
                ui.end_row();
            }
        });
    }

    fn map_view(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let (response, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        let rect = response.rect;
        self.map_rect = rect;
        painter.rect_filled(rect, 0.0, Color32::from_rgb(26, 28, 32));

        // Initial / post-import framing.
        if self.pending_fit && !self.user_moved_view {
            if let Some(b) = self.data_bounds() {
                self.cam.fit(rect, b);
                if !self.layers.iter().any(|l| l.cfg.kind == LayerKind::Raster && l.is_loading()) {
                    self.pending_fit = false;
                }
            }
        }
        if let Some(id) = self.zoom_to_on_load {
            match self.layer_index(id).map(|i| &self.layers[i]) {
                Some(l) if l.is_loading() || (l.is_unloaded() && l.cfg.visible) => {}
                Some(l) => {
                    if let Some(b) = l.world_bounds(self.crs) {
                        let v = self.cam.view_bounds(rect);
                        let overlaps = b[0] < v[2] && b[2] > v[0] && b[1] < v[3] && b[3] > v[1];
                        if !overlaps {
                            self.cam.fit(rect, b);
                        }
                        self.zoom_to_on_load = None;
                    } else if l.raster().is_none() {
                        self.zoom_to_on_load = None;
                    }
                }
                None => self.zoom_to_on_load = None,
            }
        }

        // In screenshot (test) mode the view is fixed; ignore stray input.
        let interactive = self.screenshot.is_none();
        // Zoom with the wheel / pinch, around the cursor.
        if interactive && response.hovered() {
            let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
            let factor = zoom as f64 * (scroll as f64 * 0.0025).exp();
            if (factor - 1.0).abs() > 1e-6 {
                if let Some(p) = response.hover_pos() {
                    self.cam.zoom_at(rect, p, factor);
                    self.user_moved_view = true;
                }
            }
        }
        let editing = self.editor.settings.active;
        let primary_pans = self.tool == Tool::Pan && !(editing && self.editor.wants_primary());
        let panning = interactive
            && (response.dragged_by(PointerButton::Middle)
                || response.dragged_by(PointerButton::Secondary)
                || (primary_pans && response.dragged_by(PointerButton::Primary)));
        if panning {
            self.cam.pan_pixels(response.drag_delta());
            self.user_moved_view = true;
        }
        if interactive && !editing && self.tool == Tool::MoveLayer && response.dragged_by(PointerButton::Primary) {
            let d = response.drag_delta();
            let mpp = self.cam.mpp;
            match self.selected.and_then(|id| self.layer_index(id)) {
                Some(i) => {
                    self.layers[i].cfg.offset[0] += d.x as f64 * mpp;
                    self.layers[i].cfg.offset[1] -= d.y as f64 * mpp;
                }
                None => {
                    if response.drag_started() {
                        self.info("Select a layer in the list to move it");
                    }
                }
            }
        }
        if response.hovered() {
            ctx.set_cursor_icon(match (self.tool, panning) {
                (_, true) => CursorIcon::Grabbing,
                _ if editing => self.editor.cursor(),
                (Tool::MoveLayer, _) => CursorIcon::Move,
                _ => CursorIcon::Default,
            });
        }
        self.hover = response.hover_pos().map(|p| self.cam.to_world(rect, p));

        let dc = DrawCtx { ctx: &ctx, painter: &painter, rect, cam: self.cam, pixels_per_point: ctx.pixels_per_point() };
        for l in &self.layers {
            if !l.cfg.visible {
                continue;
            }
            match &l.state {
                LayerState::Ready(LayerData::Raster(r)) => {
                    if let Some(pl) = l.placement(self.crs) {
                        view::draw_raster(&dc, &mut self.tiles, l.id, r, &pl, l.cfg.opacity);
                    }
                }
                LayerState::Ready(LayerData::Vector(v)) => view::draw_vector(&dc, v, &l.cfg.vector_style, l.cfg.offset, l.cfg.opacity),
                _ => {}
            }
        }
        if editing {
            self.editor.map_view(&dc, &response, &mut self.tiles, self.crs);
            if let Some(b) = self.editor.take_zoom(self.crs) {
                if !std::mem::take(&mut self.keep_view) {
                    self.cam.fit(rect, b);
                }
            }
        }
        if self.tool == Tool::MoveLayer && !editing {
            if let Some(pl) = self.selected.and_then(|id| self.layer_index(id)).and_then(|i| self.layers[i].placement(self.crs)) {
                view::draw_outline(&dc, &pl, Color32::from_rgb(255, 200, 40));
            }
        }
        // Keep every raster's overview tile warm, so toggling layers is instant.
        for l in &self.layers {
            if let Some(r) = l.raster() {
                self.tiles.warm_overview(&ctx, l.id, r);
            }
        }
        view::draw_scale_bar(&dc, crate::welcome::metres_per_unit(self.crs, self.cam.center[1]));

        if self.layers.is_empty() && !editing {
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                "Drop BMP / TIFF / PNG maps or .osm.pbf extracts here, or use File → Import",
                FontId::proportional(16.0),
                Color32::GRAY,
            );
        }
        if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            painter.rect_stroke(rect.shrink(4.0), 6.0, egui::Stroke::new(3.0, Color32::from_rgb(90, 170, 255)), egui::StrokeKind::Inside);
        }
    }

    fn help_window(&mut self, ctx: &egui::Context) {
        egui::Window::new("Controls").open(&mut self.show_help).resizable(false).collapsible(false).show(ctx, |ui| {
            egui::Grid::new("help").num_columns(2).striped(true).show(ui, |ui| {
                for (k, v) in [
                    ("Mouse wheel / pinch", "Zoom around the cursor"),
                    ("Left drag (Pan tool)", "Pan"),
                    ("Middle / right drag", "Pan (any tool)"),
                    ("Left drag (Move tool)", "Move the selected layer"),
                    ("Arrow keys (Move tool)", "Nudge selected layer by 1 px (Shift: 10 px)"),
                    ("M / P, Esc", "Move tool / Pan tool"),
                    ("H", "Show / hide the selected layer"),
                    ("F", "Fit all rasters in view"),
                    ("+ / -", "Zoom in / out"),
                    ("Ctrl+I", "Import raster"),
                    ("Ctrl+S / Ctrl+O", "Save / open project"),
                    ("Drag & drop", "Import BMP/TIFF/PNG/JPEG or .osm.pbf files"),
                ] {
                    ui.label(RichText::new(k).strong());
                    ui.label(v);
                    ui.end_row();
                }
            });
            ui.separator();
            ui.label(RichText::new("Map editor").strong());
            egui::Grid::new("help_editor").num_columns(2).striped(true).show(ui, |ui| {
                for (k, v) in [
                    ("1 … 7", "Edit provinces / states / nations / air zones / sea zones / adjacencies / inaccessible zones"),
                    ("S / B / L / I / P", "Select / paint (brush) / link (adjacencies) / impassable border / pan tool"),
                    ("Click, Shift / Ctrl+click", "Select; add or remove from the selection"),
                    ("Drag (select tool)", "Select everything the brush touches"),
                    ("Alt+click", "Pick what is under the cursor (to paint with)"),
                    ("[ / ]", "Smaller / larger brush"),
                    ("Delete", "Dissolve the selection into its neighbours"),
                    ("Ctrl+Z / Ctrl+Shift+Z", "Undo / redo"),
                    ("Ctrl+S", "Save the map (and the project)"),
                    ("Esc", "Clear the selection"),
                ] {
                    ui.label(RichText::new(k).strong());
                    ui.label(v);
                    ui.end_row();
                }
            });
            ui.separator();
            ui.label("The project (layer stack, styles, offsets, view) is saved automatically on exit to:");
            ui.monospace(self.project_path.display().to_string());
            if let Some(d) = osm::cache_dir() {
                ui.label("Extracted OSM boundaries are cached in:");
                ui.monospace(d.display().to_string());
            }
        });
    }

    fn screenshot_hook(&mut self, ctx: &egui::Context) {
        let Some(s) = &mut self.screenshot else { return };
        let busy = self.layers.iter().any(|l| l.is_loading())
            || self.tiles.stats().2 > 0
            || ((self.editor.settings.active || self.content.settings.active) && self.editor.is_busy())
            || (self.content.settings.active && self.content.is_busy());
        s.idle_frames = if busy { 0 } else { s.idle_frames + 1 };
        if !s.requested && s.idle_frames > 15 {
            s.requested = true;
            let (path, done) = (s.path.clone(), s.done.clone());
            ctx.request_screenshot(move |img| {
                let bytes: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
                if let Err(e) = image::save_buffer(&path, &bytes, img.size[0] as u32, img.size[1] as u32, image::ColorType::Rgba8) {
                    log::error!("screenshot failed: {e}");
                }
                done.store(true, Ordering::Relaxed);
            });
        }
        if s.done.load(Ordering::Relaxed) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ctx.request_repaint_after(Duration::from_millis(50));
    }
}

/// Properties panel for one layer. Returns a requested action.
fn layer_properties(ui: &mut egui::Ui, layer: &mut Layer, others: &[(String, GeorefOverride)], project_crs: Crs) -> Option<LayerAction> {
    let mut action = None;
    let id = layer.id;
    ui.horizontal(|ui| {
        ui.label("Name");
        ui.text_edit_singleline(&mut layer.cfg.name);
    });
    ui.label(RichText::new(layer.cfg.path.display().to_string()).small().weak());

    match &layer.state {
        LayerState::Loading(p) => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(p.stage());
            });
            ui.add(egui::ProgressBar::new(p.get()).show_percentage());
        }
        LayerState::Failed(e) => {
            ui.colored_label(Color32::from_rgb(255, 110, 110), format!("Failed to load: {e}"));
        }
        LayerState::Unloaded => {
            match &layer.known {
                Some(Known::Raster { width, height, format, bytes, .. }) => {
                    ui.label(format!("{width} × {height} px · {format} · {} when shown", util::fmt_bytes(*bytes)));
                }
                Some(Known::Vector { summary, .. }) => {
                    ui.label(summary);
                }
                None => {}
            }
            ui.label(RichText::new("Hidden, so not in memory: it is read when shown and dropped again when hidden.").small().weak());
        }
        LayerState::Ready(LayerData::Raster(r)) => {
            ui.label(format!("{} × {} px · {} · {}", r.width, r.height, r.format, util::fmt_bytes(r.bytes)));
        }
        LayerState::Ready(LayerData::Vector(v)) => {
            ui.label(format!("{} boundary ways · {} water lines · {} points · {} named areas", v.ways.len(), v.water.len(), v.n_points, v.areas.len()));
        }
    }

    ui.horizontal(|ui| {
        ui.add_enabled_ui(!layer.is_loading(), |ui| {
            if ui.button("Zoom to").clicked() {
                action = Some(LayerAction::ZoomTo(id));
            }
            if layer.cfg.kind == LayerKind::Raster && ui.button("1:1").on_hover_text("One layer pixel = one screen pixel").clicked() {
                action = Some(LayerAction::OneToOne(id));
            }
            if ui.button("Reload").on_hover_text("Re-read the file from disk").clicked() {
                action = Some(LayerAction::Reload(id));
            }
        });
        if ui.button("Remove").on_hover_text("Remove from the project (the file is not touched)").clicked() {
            action = Some(LayerAction::Remove(id));
        }
    });
    ui.add(egui::Slider::new(&mut layer.cfg.opacity, 0.0..=1.0).text("Opacity"));

    let pixel_size = layer.placement(project_crs).map(|p| p.pixel_size_world());
    ui.horizontal(|ui| {
        ui.label("Offset");
        let speed = pixel_size.unwrap_or(10.0) * 0.25;
        ui.add(egui::DragValue::new(&mut layer.cfg.offset[0]).speed(speed).prefix("x ").suffix(" m"));
        ui.add(egui::DragValue::new(&mut layer.cfg.offset[1]).speed(speed).prefix("y ").suffix(" m"));
        if ui.add_enabled(layer.cfg.offset != [0.0, 0.0], egui::Button::new("Reset")).clicked() {
            layer.cfg.offset = [0.0, 0.0];
        }
    });
    if let (Some(ps), true) = (pixel_size, layer.cfg.offset != [0.0, 0.0]) {
        ui.label(RichText::new(format!("= {:.2} × {:.2} layer pixels", layer.cfg.offset[0] / ps, layer.cfg.offset[1] / ps)).small().weak());
    }

    if layer.cfg.kind == LayerKind::Raster && !layer.is_loading() {
        egui::CollapsingHeader::new("Georeference").default_open(true).show(ui, |ui| {
            if layer.is_unloaded() && layer.known.is_none() && layer.cfg.georef_override.is_none() {
                // what the file says is known once it is read
                ui.label(RichText::new("Not read yet: its georeference (if the file has one) shows once the layer is shown.").weak());
            } else {
                georef_ui(ui, layer, others, project_crs);
            }
        });
    }

    let Layer { cfg, state, .. } = layer;
    match state {
        LayerState::Ready(LayerData::Raster(r)) => {
            let raster = r.raster.read();
            if raster.is_single_band() {
                egui::CollapsingHeader::new("Display").default_open(true).show(ui, |ui| {
                    let def = raster.default_range();
                    let mut range = cfg.raster_style.range.unwrap_or([def.0, def.1]);
                    let speed = ((range[1] - range[0]).abs() / 300.0).max(0.01);
                    ui.horizontal(|ui| {
                        ui.label("Value range");
                        let a = ui.add(egui::DragValue::new(&mut range[0]).speed(speed));
                        ui.label("–");
                        let b = ui.add(egui::DragValue::new(&mut range[1]).speed(speed));
                        if a.changed() || b.changed() {
                            cfg.raster_style.range = Some(range);
                        }
                        if ui.add_enabled(cfg.raster_style.range.is_some(), egui::Button::new("Auto")).clicked() {
                            cfg.raster_style.range = None;
                        }
                    });
                    if let Some((lo, hi)) = raster.data_range {
                        ui.label(RichText::new(format!("data range {lo} – {hi}")).small().weak());
                    }
                    egui::ComboBox::from_label("Colour map").selected_text(cfg.raster_style.colormap.name()).show_ui(ui, |ui| {
                        for c in Colormap::ALL {
                            ui.selectable_value(&mut cfg.raster_style.colormap, c, c.name());
                        }
                    });
                });
            }
        }
        LayerState::Ready(LayerData::Vector(v)) => {
            egui::CollapsingHeader::new("Display").default_open(true).show(ui, |ui| {
                let style = &mut cfg.vector_style;
                ui.checkbox(&mut style.labels, "Area names");
                ui.add(egui::Slider::new(&mut style.width_scale, 0.25..=4.0).text("Line width"));
                ui.label(RichText::new("Admin levels (meaning varies by country):").small());
                egui::Grid::new("levels").num_columns(3).striped(true).show(ui, |ui| {
                    for (&lvl, &count) in &v.level_counts {
                        let ls = style.level(lvl);
                        ui.checkbox(&mut ls.visible, format!("{lvl}"));
                        ui.label(format!("{} ({count})", vector::level_name(lvl)));
                        let mut c = Color32::from_rgba_unmultiplied(ls.color[0], ls.color[1], ls.color[2], ls.color[3]);
                        if ui.color_edit_button_srgba(&mut c).changed() {
                            ls.color = c.to_srgba_unmultiplied();
                        }
                        ui.end_row();
                    }
                });
                if !v.water_counts.is_empty() {
                    ui.label(RichText::new("Water (drawn under the boundaries):").small());
                    egui::Grid::new("water").num_columns(2).striped(true).show(ui, |ui| {
                        for w in osm::Water::ALL {
                            let Some(&count) = v.water_counts.get(&w) else { continue };
                            let ls = style.water_mut(w);
                            ui.checkbox(&mut ls.visible, format!("{} ({count})", w.label()));
                            let mut c = Color32::from_rgba_unmultiplied(ls.color[0], ls.color[1], ls.color[2], ls.color[3]);
                            if ui.color_edit_button_srgba(&mut c).changed() {
                                ls.color = c.to_srgba_unmultiplied();
                            }
                            ui.end_row();
                        }
                    });
                }
            });
        }
        _ => {}
    }
    action
}

fn georef_ui(ui: &mut egui::Ui, layer: &mut Layer, others: &[(String, GeorefOverride)], project_crs: Crs) {
    let has_file_georef = layer.shape().is_some_and(|(_, _, g)| g.transform.is_some());
    match layer.georef() {
        Some(g) => {
            egui::Grid::new("georef").num_columns(2).show(ui, |ui| {
                ui.label("Source");
                ui.label(&g.source);
                ui.end_row();
                ui.label("CRS");
                if g.crs == Crs::UNKNOWN {
                    ui.label(format!("unknown – assuming {}", project_crs.name()));
                } else if !g.crs.is_supported() {
                    ui.colored_label(Color32::from_rgb(255, 170, 60), format!("{} – not supported, shown unprojected", g.crs.name()));
                } else if g.crs != project_crs {
                    ui.label(format!("{} (reprojected on the fly)", g.crs.name()));
                } else {
                    ui.label(g.crs.name());
                }
                ui.end_row();
                let (px, py) = g.transform.pixel_size();
                ui.label("Pixel size");
                ui.label(format!("{px} × {py}{}", if g.crs.is_geographic() { "°" } else { " m" }));
                ui.end_row();
                ui.label("Top-left");
                ui.label(format!("{:.3}, {:.3}", g.transform.0[0], g.transform.0[3]));
                ui.end_row();
            });
        }
        None => {
            ui.label("No georeference.");
        }
    }

    if let Some(ov) = &mut layer.cfg.georef_override {
        ui.add_space(4.0);
        let mut changed = false;
        let g = &mut ov.transform.0;
        let speed = g[1].abs().max(1e-6) * 0.25;
        egui::Grid::new("georef_edit").num_columns(2).show(ui, |ui| {
            ui.label("Top-left x");
            changed |= ui.add(egui::DragValue::new(&mut g[0]).speed(speed)).changed();
            ui.end_row();
            ui.label("Top-left y");
            changed |= ui.add(egui::DragValue::new(&mut g[3]).speed(speed)).changed();
            ui.end_row();
            ui.label("Pixel width");
            changed |= ui.add(egui::DragValue::new(&mut g[1]).speed(speed * 0.01).range(1e-9..=f64::MAX)).changed();
            ui.end_row();
            ui.label("Pixel height");
            let mut ph = -g[5];
            if ui.add(egui::DragValue::new(&mut ph).speed(speed * 0.01).range(1e-9..=f64::MAX)).changed() {
                g[5] = -ph;
                changed = true;
            }
            ui.end_row();
            ui.label("CRS");
            let before = ov.crs;
            egui::ComboBox::from_id_salt("georef_crs").selected_text(ov.crs.name()).show_ui(ui, |ui| {
                ui.selectable_value(&mut ov.crs, Crs::UNKNOWN, "unknown (= project CRS)");
                for &code in KNOWN_CRS {
                    ui.selectable_value(&mut ov.crs, Crs::new(code), Crs::new(code).name());
                }
            });
            changed |= before != ov.crs;
            ui.end_row();
        });
        if changed {
            ov.note = "manual".into();
        }
        if has_file_georef && ui.button("Revert to the file's georeference").clicked() {
            layer.cfg.georef_override = None;
        }
    } else if ui.button("Edit georeference manually").clicked() {
        let g = layer.georef();
        layer.cfg.georef_override = Some(GeorefOverride {
            transform: g.as_ref().map(|g| g.transform).unwrap_or(GeoTransform::identity_pixels()),
            crs: g.map(|g| g.crs).unwrap_or(project_crs),
            note: "manual".into(),
        });
    }
    if !others.is_empty() {
        egui::ComboBox::from_id_salt("georef_copy").selected_text("Copy georeference from…").show_ui(ui, |ui| {
            for (name, ov) in others {
                if ui.selectable_label(false, name).clicked() {
                    layer.cfg.georef_override = Some(ov.clone());
                }
            }
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_jobs();
        self.sync_loaded(&ctx);
        self.sync_styles();
        self.resolve_missing_georefs();
        if self.editor.settings.active || self.content.settings.active {
            self.editor.poll(&ctx);
        }
        let mut live: FxHashMap<u64, u64> = self.layers.iter().filter_map(|l| l.raster().map(|r| (l.id, r.generation))).collect();
        if let Some(g) = self.editor.overlay_generation().filter(|_| self.editor.settings.active) {
            live.insert(editor::OVERLAY_LAYER, g);
        }
        // unsaved map edits: ask before quitting
        if ctx.input(|i| i.viewport().close_requested()) && self.editor.is_dirty() && !self.editor.close_ok && self.screenshot.is_none() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.editor.ask_close = true;
            self.set_mode(Mode::Editor, &ctx);
        } else if ctx.input(|i| i.viewport().close_requested()) && self.content.is_dirty() && !self.content.close_ok && self.screenshot.is_none() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.content.ask_close = true;
            self.set_mode(Mode::Content, &ctx);
        }
        self.tiles.begin_frame(&ctx, &live);
        if self.screenshot.is_none() {
            self.handle_shortcuts(&ctx);
        }
        self.handle_dropped_files(&ctx);

        egui::Panel::top("menu").show(ui, |ui| self.menu_bar(ui));
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        let (game, unsaved) = (self.game_dir(), self.unsaved());
        if let Some(w) = &mut self.welcome {
            let act = egui::CentralPanel::default().show(ui, |ui| w.ui(ui, &game, unsaved)).inner;
            match act {
                Some(crate::welcome::Action::Open(p)) => self.open_project(p, &ctx),
                Some(crate::welcome::Action::OpenDialog) => self.open_project_dialog(&ctx),
                Some(crate::welcome::Action::Created(c)) => {
                    let title = format!("New project {}", geo::file_name(&c.project));
                    let mut lines = c.report;
                    lines.push(format!("project file {}", c.project.display()));
                    self.report = Some((title, lines));
                    self.open_project(c.project, &ctx);
                }
                None => {}
            }
            self.pending_open_dialog(&ctx);
            self.convert_dialog(&ctx);
            self.report_window(&ctx);
            self.screenshot_hook(&ctx);
            return;
        }
        match self.mode() {
            Mode::Content => {
                let (content, editor) = (&mut self.content, &mut self.editor);
                egui::CentralPanel::default().show(ui, |ui| content.ui(ui, editor));
            }
            Mode::Editor => {
                let (editor, layers) = (&mut self.editor, &mut self.layers);
                egui::Panel::right("editor_side").default_size(440.0).show(ui, |ui| editor.side_panel(ui, layers));
                egui::CentralPanel::no_frame().show(ui, |ui| self.map_view(ui));
            }
            Mode::Viewer => {
                egui::Panel::right("side").default_size(380.0).show(ui, |ui| self.side_panel(ui));
                egui::CentralPanel::no_frame().show(ui, |ui| self.map_view(ui));
            }
        }
        self.help_window(&ctx);
        self.projection_window(&ctx);
        self.pending_open_dialog(&ctx);
        self.convert_dialog(&ctx);
        self.report_window(&ctx);
        self.editor.windows(&ctx);
        // the build copies the nation content from disk: what is not saved goes in first
        if self.editor.take_build_wanted() {
            if self.content.is_dirty() {
                self.content.save(&mut self.editor);
            }
            self.editor.start_build(&ctx);
        }

        // a save wrote the working folder: into the project file with it
        if self.project.is_some() && bundle::writes() != self.synced_writes {
            self.sync_project(false);
        }
        self.tiles.end_frame();
        if self.layers.iter().any(|l| l.is_loading()) {
            ctx.request_repaint_after(Duration::from_millis(150));
        }
        self.screenshot_hook(&ctx);
    }

    fn on_exit(&mut self) {
        if self.screenshot.is_some() || self.project.is_none() {
            return;
        }
        self.sync_project(true);
        log::info!("saved the project {}", self.project_path.display());
    }
}

/// `path` ending in `.hoi4proj`.
fn with_ext(path: PathBuf) -> PathBuf {
    if path.to_string_lossy().ends_with(&format!(".{}", bundle::EXT)) { path } else { PathBuf::from(format!("{}.{}", path.display(), bundle::EXT)) }
}
