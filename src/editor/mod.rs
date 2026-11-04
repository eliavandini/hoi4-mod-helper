//! Tool #2, the map editor: provinces, states, nations, air and sea zones,
//! adjacencies, state properties and inaccessible zones of the mod's map,
//! edited on top of the viewer's layers.
//!
//! It edits the map project in `maps/processed maps/suedtirol`
//! (provinces.bmp + provinces.json, see [`model`]); "Build mod" makes the
//! HOI4 mod from it and checks it (see [`crate::modbuild`]), with the mod's
//! name, version, tags... from "Mod details".

pub mod build;
pub mod density;
pub mod doc;
pub mod kmeans;
pub mod launcher;
pub mod model;
pub mod ops;
pub mod render;
pub mod validate;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;
use eframe::egui::{self, Align2, Color32, FontId, Key, KeyboardShortcut, Modifiers, PointerButton, Pos2, Rect, RichText, Stroke};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};

use crate::geo::Crs;
use crate::modbuild::{self, ModMeta};
use crate::layers::{Layer, Placement};
use crate::tiles::TileManager;
use crate::util::Progress;
use crate::view::{self, DrawCtx};

use density::Terrain;
use doc::Doc;
use model::{Adjacency, Id, Kind, Nation, Province, Region, State};
use ops::{Constraint, GenParams, Retile};
use render::{Overlay, RenderStyle};
use validate::{Issue, Severity};

/// Tile-cache layer id of the overlay (viewer layers count up from 1).
pub const OVERLAY_LAYER: u64 = u64::MAX - 7;

const ERR: Color32 = Color32::from_rgb(255, 110, 110);
const WARN: Color32 = Color32::from_rgb(255, 190, 90);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Level {
    #[default]
    Provinces,
    States,
    Nations,
    AirZones,
    SeaZones,
    Adjacencies,
    Inaccessible,
}

impl Level {
    pub const ALL: [Level; 7] =
        [Level::Provinces, Level::States, Level::Nations, Level::AirZones, Level::SeaZones, Level::Adjacencies, Level::Inaccessible];
    pub fn name(self) -> &'static str {
        match self {
            Level::Provinces => "Provinces",
            Level::States => "States",
            Level::Nations => "Nations",
            Level::AirZones => "Air zones",
            Level::SeaZones => "Sea zones",
            Level::Adjacencies => "Adjacencies",
            Level::Inaccessible => "Inaccessible",
        }
    }
    fn noun(self) -> &'static str {
        match self {
            Level::Provinces | Level::Inaccessible => "province",
            Level::States => "state",
            Level::Nations => "nation",
            Level::AirZones => "air zone",
            Level::SeaZones => "sea zone",
            Level::Adjacencies => "adjacency",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum MapMode {
    #[default]
    Auto,
    ProvinceColours,
    Types,
    States,
    Nations,
    AirZones,
    SeaZones,
    Size,
    SizeVsTarget,
    Issues,
}

impl MapMode {
    const ALL: [MapMode; 10] = [
        MapMode::Auto,
        MapMode::ProvinceColours,
        MapMode::Types,
        MapMode::States,
        MapMode::Nations,
        MapMode::AirZones,
        MapMode::SeaZones,
        MapMode::Size,
        MapMode::SizeVsTarget,
        MapMode::Issues,
    ];
    fn name(self) -> &'static str {
        match self {
            MapMode::Auto => "by what is edited",
            MapMode::ProvinceColours => "province colours",
            MapMode::Types => "land / sea / impassable / removed",
            MapMode::States => "states",
            MapMode::Nations => "nations",
            MapMode::AirZones => "air zones",
            MapMode::SeaZones => "sea zones",
            MapMode::Size => "province size (km²)",
            MapMode::SizeVsTarget => "province size vs. terrain target",
            MapMode::Issues => "validation issues",
        }
    }
    fn effective(self, level: Level) -> MapMode {
        match (self, level) {
            (MapMode::Auto, Level::Provinces) => MapMode::ProvinceColours,
            (MapMode::Auto, Level::States) => MapMode::States,
            (MapMode::Auto, Level::Nations | Level::Adjacencies) => MapMode::Nations,
            (MapMode::Auto, Level::AirZones) => MapMode::AirZones,
            (MapMode::Auto, Level::SeaZones) => MapMode::SeaZones,
            (MapMode::Auto, Level::Inaccessible) => MapMode::Types,
            (m, _) => m,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Tool {
    #[default]
    Select,
    Paint,
    Link,
    /// Click or drag across borders to make them impassable (or passable).
    Border,
    Pan,
}

/// Editor settings, saved in the viewer's project file.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The editor was open (it opens again with the project).
    pub active: bool,
    pub map_dir: Option<PathBuf>,
    pub level: Level,
    pub mode: MapMode,
    pub tool: Tool,
    /// Brush radius in map pixels.
    pub brush: f32,
    pub constraint: Constraint,
    pub params: GenParams,
    pub opacity: f32,
    pub labels: bool,
    pub borders: bool,
    pub adjacencies: bool,
    pub coarsen: bool,
    pub auto_check: bool,
    /// The HOI4 install (None: the Steam default).
    pub game_dir: Option<PathBuf>,
    /// Where mods are built (None: the game's mod folder in the user's documents).
    pub mods_dir: Option<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            active: false,
            map_dir: None,
            level: Level::Provinces,
            mode: MapMode::Auto,
            tool: Tool::Select,
            brush: 6.0,
            constraint: Constraint::default(),
            params: GenParams::default(),
            opacity: 0.7,
            labels: true,
            borders: true,
            adjacencies: true,
            coarsen: true,
            auto_check: true,
            game_dir: None,
            mods_dir: None,
        }
    }
}

/// Copy of the entity being edited in the properties panel; applied as one
/// undo step.
#[derive(Clone, PartialEq)]
enum Draft {
    Province(Id, Province),
    State(Id, State),
    Nation(Id, Nation),
    Region(Id, Region),
    Adjacency(usize, Adjacency),
}

#[derive(Clone, Debug)]
enum Act {
    Save,
    Build,
    Undo,
    Redo,
    Reload,
    Open,
    SetLevel(Level),
    Select(Level, Vec<Id>),
    ZoomProvince(Id),
    ZoomSelection,
    MergeProvinces,
    DivideProvinces,
    GenerateProvinces,
    DissolveProvinces,
    SplitParts(bool),
    SetKind(Kind),
    NewProvince,
    NewStateFromSel,
    NewZoneFromSel(bool),
    GrowTo(Level),
    MergeStates,
    DivideStates,
    DissolveStates,
    GenerateStates,
    NewNationFromSel,
    SetImpassable(bool),
    MergeNations,
    DivideNations,
    DissolveNations,
    GenerateNations,
    MergeRegions,
    DivideRegions,
    DissolveRegions,
    GenerateRegions,
    MakeImpassable,
    RemoveFromPlay,
    RestoreAccess,
    DeleteAdjacency(usize),
    AdjacencyPoints(usize),
    SetMapFrame(bool),
    SetModMeta(Box<ModMeta>),
    ApplyDraft,
    RevertDraft,
    Check,
    FixCrossings,
}

#[derive(Clone, Copy, PartialEq)]
enum SelOp {
    Replace,
    Add,
    Remove,
}

struct Stroke2 {
    last: [f64; 2],
    op: SelOp,
    /// A paint stroke with an open transaction.
    painting: bool,
}

/// A drag of the border tool: the first border it crosses decides whether it
/// blocks (true) or opens borders.
struct BorderStroke {
    last: [f64; 2],
    make: Option<bool>,
    changed: usize,
}

/// Border lines (polylines along pixel edges) of province pairs, for one pixel version.
type BorderLines = FxHashMap<(Id, Id), Arc<Vec<Vec<[f64; 2]>>>>;

struct MapLabel {
    text: String,
    centre: [f64; 2],
    extent: f64,
}

#[derive(Clone, PartialEq)]
struct StyleKey {
    meta: u64,
    pixels: u64,
    level: Level,
    mode: MapMode,
    sel: u64,
    adj: Option<usize>,
    borders: bool,
    issues: u64,
    targets: Option<(u64, density::DensityParams)>,
}

pub struct Editor {
    pub settings: Settings,
    pub doc: Option<Doc>,
    loading: Option<(Arc<Progress>, Receiver<Result<Doc, String>>)>,
    terrain_rx: Option<Receiver<Result<Terrain, String>>>,
    terrain_msg: Option<String>,
    load_err: Option<String>,
    sel: Vec<Id>,
    sel_adj: Option<usize>,
    divide_n: usize,
    adj_kind: String,
    adj_rule: String,
    link_from: Option<Id>,
    new_province: bool,
    stroke: Option<Stroke2>,
    border_stroke: Option<BorderStroke>,
    hover_border: Option<(Id, Id)>,
    border_lines: (u64, BorderLines),
    style: Arc<RenderStyle>,
    style_key: Option<StyleKey>,
    overlay_gen: u64,
    issues: Vec<Issue>,
    issues_rx: Option<Receiver<(u64, Vec<Issue>)>>,
    issues_for: u64,
    issues_gen: u64,
    issue_provs: Vec<u8>,
    last_edit: Instant,
    hover: Option<[f64; 2]>,
    msg: Option<(String, bool, Instant)>,
    build: Option<build::BuildJob>,
    show_build: bool,
    /// The open "Mod details" window.
    meta: Option<MetaEdit>,
    /// Thumbnail preview: picture path, and the texture or why there is none.
    thumb: Option<(PathBuf, Result<egui::TextureHandle, String>)>,
    /// Game folder and its version ("1.19.*").
    game_version: Option<(PathBuf, String)>,
    /// Open "Mod details" with the map ($MOD_HELPER_OPEN=details, for screenshots).
    meta_at_start: bool,
    /// The game launcher in the build window.
    launcher: launcher::Launcher,
    pub ask_close: bool,
    pub close_ok: bool,
    draft: Option<Draft>,
    draft_orig: Option<Draft>,
    draft_meta: u64,
    nation_filter: String,
    vp_new: (Id, u32),
    zoom_to: Option<[f64; 4]>,
    labels_cache: Option<((u64, u64, Level), Vec<MapLabel>)>,
    targets: Option<((u64, density::DensityParams), Vec<f32>)>,
    sealed: Option<(u64, Vec<f64>)>,
}

/// The "Mod details" window: the edited copy, the stored value it started
/// from, and the text of the free-form tags and the dependencies as typed.
struct MetaEdit {
    m: ModMeta,
    orig: ModMeta,
    other_tags: String,
    deps: String,
}

impl MetaEdit {
    fn new(m: ModMeta) -> Self {
        let other_tags = m.tags.iter().filter(|t| !modbuild::meta::KNOWN_TAGS.contains(&t.as_str())).cloned().collect::<Vec<_>>().join(", ");
        let deps = m.dependencies.join("\n");
        MetaEdit { orig: m.clone(), m, other_tags, deps }
    }
}

/// Size of the vanilla map, the one its frame is made for.
const VANILLA_MAP: [u32; 2] = [5632, 2048];

/// How far from a border (in map pixels) a click of the border tool may land:
/// 8 screen points.
fn border_pick_radius(px_scale: f64) -> f64 {
    (8.0 / px_scale.max(1e-9)).clamp(1.5, 64.0)
}

fn version(doc: &Doc) -> u64 {
    doc.meta_version.wrapping_mul(1_000_003) ^ doc.pixel_version
}

pub fn default_map_dir() -> Option<PathBuf> {
    crate::project::find_maps_dir().map(|m| m.join("processed maps").join("suedtirol"))
}


impl Editor {
    pub fn new(settings: Settings) -> Self {
        Editor {
            settings,
            doc: None,
            loading: None,
            terrain_rx: None,
            terrain_msg: None,
            load_err: None,
            sel: Vec::new(),
            sel_adj: None,
            divide_n: 2,
            adj_kind: "sea".into(),
            adj_rule: String::new(),
            link_from: None,
            new_province: false,
            stroke: None,
            border_stroke: None,
            hover_border: None,
            border_lines: (0, FxHashMap::default()),
            style: Arc::new(RenderStyle::empty()),
            style_key: None,
            overlay_gen: 1,
            issues: Vec::new(),
            issues_rx: None,
            issues_for: 0,
            issues_gen: 0,
            issue_provs: Vec::new(),
            last_edit: Instant::now(),
            hover: None,
            msg: None,
            build: None,
            show_build: std::env::var("MOD_HELPER_OPEN").is_ok_and(|v| v == "play"),
            meta: None,
            thumb: None,
            game_version: None,
            meta_at_start: std::env::var("MOD_HELPER_OPEN").is_ok_and(|v| v == "details"),
            launcher: launcher::Launcher::default(),
            ask_close: false,
            close_ok: false,
            draft: None,
            draft_orig: None,
            draft_meta: 0,
            nation_filter: String::new(),
            vp_new: (0, 1),
            zoom_to: None,
            labels_cache: None,
            targets: None,
            sealed: None,
        }
    }

    /// Style generation of the overlay tiles (for the tile cache).
    pub fn overlay_generation(&self) -> Option<u64> {
        self.doc.as_ref().map(|_| self.overlay_gen)
    }

    /// The map's primary button belongs to the editor (not to panning).
    pub fn wants_primary(&self) -> bool {
        self.doc.is_some() && self.settings.tool != Tool::Pan
    }

    pub fn set_pan_tool(&mut self) {
        self.settings.tool = Tool::Pan;
    }

    /// Loading, checking or building (for the screenshot test mode).
    pub fn is_busy(&self) -> bool {
        self.loading.is_some() || self.terrain_rx.is_some() || self.issues_rx.is_some()
    }

    /// Cursor for the map.
    pub fn cursor(&self) -> egui::CursorIcon {
        match self.settings.tool {
            Tool::Paint | Tool::Link | Tool::Border => egui::CursorIcon::Crosshair,
            Tool::Select => egui::CursorIcon::Default,
            Tool::Pan => egui::CursorIcon::Grab,
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.doc.as_ref().is_some_and(|d| d.dirty)
    }

    fn info(&mut self, m: impl Into<String>) {
        let m = m.into();
        log::info!("{m}");
        self.msg = Some((m, false, Instant::now()));
    }

    fn error(&mut self, m: impl Into<String>) {
        let m = m.into();
        log::warn!("{m}");
        self.msg = Some((m, true, Instant::now()));
    }

    pub fn message(&self) -> Option<(&str, bool)> {
        self.msg.as_ref().filter(|m| m.2.elapsed() < Duration::from_secs(15)).map(|m| (m.0.as_str(), m.1))
    }

    // ------------------------------------------------------------ loading

    /// Open the map in `dir` (provinces.bmp + provinces.json) in the background.
    pub fn open(&mut self, dir: PathBuf, ctx: &egui::Context) {
        let progress = Arc::new(Progress::default());
        let (tx, rx) = crossbeam_channel::bounded(1);
        let (p, c, d) = (progress.clone(), ctx.clone(), dir.clone());
        std::thread::spawn(move || {
            let r = model::load(&d, &p).map(|l| Doc::new(d.clone(), l)).map_err(|e| format!("{e:#}"));
            let _ = tx.send(r);
            c.request_repaint();
        });
        self.loading = Some((progress, rx));
        self.load_err = None;
        self.settings.map_dir = Some(dir);
    }

    /// Open the default (or last) map if nothing is open yet.
    pub fn ensure_open(&mut self, ctx: &egui::Context) {
        if self.doc.is_none() && self.loading.is_none() && self.load_err.is_none() {
            match self.settings.map_dir.clone().or_else(default_map_dir) {
                Some(d) => self.open(d, ctx),
                None => self.load_err = Some("no map folder found; use \"Open map…\"".into()),
            }
        }
    }

    pub fn poll(&mut self, ctx: &egui::Context) {
        if let Some((_, rx)) = &self.loading {
            if let Ok(r) = rx.try_recv() {
                self.loading = None;
                match r {
                    Ok(doc) => {
                        self.info(format!(
                            "opened {}: {} provinces, {} states, {} nations, {} zones",
                            doc.dir.display(),
                            doc.data.provinces.count(),
                            doc.data.states.count(),
                            doc.data.nations.count(),
                            doc.data.regions.count()
                        ));
                        let (dir, w, h, pm) = (doc.dir.clone(), doc.w, doc.h, doc.data.pixel_m);
                        self.doc = Some(doc);
                        self.sel.clear();
                        self.sel_adj = None;
                        self.draft = None;
                        self.style_key = None;
                        self.labels_cache = None;
                        self.border_lines = (0, FxHashMap::default());
                        self.border_stroke = None;
                        self.hover_border = None;
                        self.targets = None;
                        self.sealed = None;
                        self.issues.clear();
                        self.issues_for = 0;
                        self.zoom_to = Some([0.0, 0.0, w as f64, h as f64]);
                        let (tx, rx) = crossbeam_channel::bounded(1);
                        let c = ctx.clone();
                        std::thread::spawn(move || {
                            let r = density::load_terrain(&dir, w, h, pm, &Progress::default()).map_err(|e| format!("{e:#}"));
                            let _ = tx.send(r);
                            c.request_repaint();
                        });
                        self.terrain_rx = Some(rx);
                        self.terrain_msg = Some("loading terrain…".into());
                    }
                    Err(e) => {
                        self.error(format!("cannot open map: {e}"));
                        self.load_err = Some(e);
                    }
                }
            } else {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }
        if let Some(rx) = &self.terrain_rx {
            if let Ok(r) = rx.try_recv() {
                self.terrain_rx = None;
                match r {
                    Ok(t) => {
                        if let Some(d) = &mut self.doc {
                            d.terrain = Some(Arc::new(t));
                        }
                        self.terrain_msg = None;
                    }
                    Err(e) => self.terrain_msg = Some(format!("terrain not available ({e}); generating by terrain is disabled")),
                }
            }
        }
        // background validation
        if let Some(rx) = &self.issues_rx {
            if let Ok((v, issues)) = rx.try_recv() {
                self.issues_rx = None;
                let cap = self.doc.as_ref().map(|d| d.data.provinces.cap()).unwrap_or(0);
                let mut marks = vec![0u8; cap];
                for is in &issues {
                    if let Some(x) = is.prov.and_then(|p| marks.get_mut(p as usize)) {
                        *x = (*x).max(if is.severity == Severity::Error { 2 } else { 1 });
                    }
                }
                self.issue_provs = marks;
                self.issues = issues;
                self.issues_for = v;
                self.issues_gen += 1;
            }
        }
        let want_check = self.doc.as_ref().is_some_and(|d| version(d) != self.issues_for)
            && self.settings.auto_check
            && self.issues_rx.is_none()
            && self.stroke.is_none()
            && self.last_edit.elapsed() > Duration::from_millis(400);
        if want_check {
            self.start_check(ctx);
        } else if self.settings.auto_check && self.issues_rx.is_none() && self.doc.as_ref().is_some_and(|d| version(d) != self.issues_for) {
            ctx.request_repaint_after(Duration::from_millis(450));
        }
        if self.build.as_ref().is_some_and(|b| b.state() == build::RUNNING) {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
        self.launcher.poll();
    }

    fn start_check(&mut self, ctx: &egui::Context) {
        let Some(doc) = &self.doc else { return };
        let snap = validate::Snapshot::of(doc);
        let v = version(doc);
        let (tx, rx) = crossbeam_channel::bounded(1);
        let c = ctx.clone();
        std::thread::spawn(move || {
            let issues = validate::validate(&snap);
            let _ = tx.send((v, issues));
            c.request_repaint();
        });
        self.issues_rx = Some(rx);
    }

    // ------------------------------------------------------------ geometry

    pub fn placement(&self, crs: Crs) -> Option<Placement> {
        let doc = self.doc.as_ref()?;
        let (gt, _) = doc.geo.transform.clone()?;
        let lcrs = doc.geo.crs.as_ref().map(|c| c.0).unwrap_or(Crs::UNKNOWN);
        Placement::new(gt, lcrs, crs, [0.0, 0.0], doc.w, doc.h)
    }

    pub fn bounds(&self, crs: Crs) -> Option<[f64; 4]> {
        self.placement(crs).map(|p| p.bounds())
    }

    /// World bounds to zoom to, requested by the editor.
    pub fn take_zoom(&mut self, crs: Crs) -> Option<[f64; 4]> {
        let b = self.zoom_to?;
        let pl = self.placement(crs)?;
        self.zoom_to = None;
        let pad = ((b[2] - b[0]).max(b[3] - b[1]) * 0.15).max(20.0);
        let mut out = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
        for (x, y) in [(b[0] - pad, b[1] - pad), (b[2] + pad, b[3] + pad), (b[0] - pad, b[3] + pad), (b[2] + pad, b[1] - pad)] {
            let w = pl.pixel_to_world(x, y)?;
            out = [out[0].min(w[0]), out[1].min(w[1]), out[2].max(w[0]), out[3].max(w[1])];
        }
        Some(out)
    }

    fn zoom_provinces(&mut self, provs: &[Id]) {
        let Some(doc) = &self.doc else { return };
        let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
        for &p in provs {
            let s = doc.stats.get(p);
            if s.px > 0 {
                b = [b[0].min(s.x0 as f64), b[1].min(s.y0 as f64), b[2].max(s.x1 as f64 + 1.0), b[3].max(s.y1 as f64 + 1.0)];
            }
        }
        if b[0] < b[2] {
            self.zoom_to = Some(b);
        }
    }

    // ------------------------------------------------------------ selection

    /// The entity at `level` a province belongs to.
    fn entity_of(doc: &Doc, level: Level, p: Id) -> Option<Id> {
        let d = &doc.data;
        let r = match level {
            Level::Provinces | Level::Inaccessible | Level::Adjacencies => p,
            Level::States => d.state_of(p),
            Level::Nations => d.nation_of(p),
            Level::AirZones => {
                if d.kind(p) == Kind::Sea {
                    0
                } else {
                    d.region_of(p)
                }
            }
            Level::SeaZones => {
                if d.kind(p) == Kind::Sea {
                    d.region_of(p)
                } else {
                    0
                }
            }
        };
        (r != 0).then_some(r)
    }

    /// Provinces covered by ids at a level.
    fn provinces_of(doc: &Doc, level: Level, ids: &[Id]) -> Vec<Id> {
        let mut out = Vec::new();
        for &id in ids {
            match level {
                Level::Provinces | Level::Inaccessible | Level::Adjacencies => out.push(id),
                Level::States => out.extend_from_slice(doc.index.provs_of_state(id)),
                Level::Nations => {
                    for &s in doc.index.states_of_nation(id) {
                        out.extend_from_slice(doc.index.provs_of_state(s));
                    }
                }
                Level::AirZones | Level::SeaZones => out.extend_from_slice(doc.index.provs_of_region(id)),
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    fn sel_provinces(&self) -> Vec<Id> {
        match &self.doc {
            Some(doc) => Self::provinces_of(doc, self.settings.level, &self.sel),
            None => Vec::new(),
        }
    }

    fn set_level(&mut self, level: Level) {
        if level == self.settings.level {
            return;
        }
        if let Some(doc) = &self.doc {
            let provs = Self::provinces_of(doc, self.settings.level, &self.sel);
            let mut seen = FxHashSet::default();
            self.sel = provs.into_iter().filter_map(|p| Self::entity_of(doc, level, p)).filter(|e| seen.insert(*e)).collect();
            if level == Level::Adjacencies {
                self.sel.clear();
            }
        }
        self.settings.level = level;
        if level != Level::Adjacencies && self.settings.tool == Tool::Link {
            self.settings.tool = Tool::Select;
        }
        if level == Level::Adjacencies && self.settings.tool == Tool::Paint {
            self.settings.tool = Tool::Link;
        }
        self.link_from = None;
        self.new_province = false;
    }

    fn select_op(&mut self, ids: &[Id], op: SelOp) {
        match op {
            SelOp::Replace => {
                self.sel = ids.to_vec();
            }
            SelOp::Add => {
                for &i in ids {
                    if !self.sel.contains(&i) {
                        self.sel.push(i);
                    }
                }
            }
            SelOp::Remove => self.sel.retain(|i| !ids.contains(i)),
        }
    }

    /// Drop selected ids that no longer exist.
    fn prune_selection(&mut self) {
        let Some(doc) = &self.doc else { return };
        let d = &doc.data;
        let level = self.settings.level;
        self.sel.retain(|&i| match level {
            Level::Provinces | Level::Inaccessible | Level::Adjacencies => d.provinces.contains(i),
            Level::States => d.states.contains(i),
            Level::Nations => d.nations.contains(i),
            Level::AirZones | Level::SeaZones => d.regions.contains(i),
        });
        if self.sel_adj.is_some_and(|i| i >= d.adjacencies.len()) {
            self.sel_adj = None;
        }
    }

    // ------------------------------------------------------------ edits

    /// Run an edit as one undo step.
    fn run(&mut self, label: &str, f: impl FnOnce(&mut Doc, &GenParams) -> anyhow::Result<String>) {
        let g = self.settings.params;
        let Some(doc) = self.doc.as_mut() else { return };
        if doc.in_txn() {
            doc.commit();
        }
        doc.begin(label);
        let r = f(doc, &g);
        match r {
            Ok(m) => {
                doc.commit();
                self.info(if m.is_empty() { label.to_string() } else { m });
            }
            Err(e) => {
                doc.rollback();
                self.error(format!("{label}: {e:#}"));
            }
        }
        self.after_edit();
    }

    fn after_edit(&mut self) {
        self.prune_selection();
        self.last_edit = Instant::now();
    }

    fn apply(&mut self, act: Act, ctx: &egui::Context) {
        let sel = self.sel.clone();
        let provs = self.sel_provinces();
        let n = self.divide_n.max(2);
        let need = |s: &Vec<Id>| !s.is_empty();
        match act {
            Act::Save => self.save(),
            Act::Build => {
                // edits in the open "Mod details" window count
                if let Some(m) = self.meta.as_ref().map(|e| e.m.clone())
                    && self.doc.as_ref().is_some_and(|d| d.data.mod_meta() != m)
                {
                    if let Err(e) = m.check() {
                        self.error(format!("Mod details: {e:#}"));
                        return;
                    }
                    self.apply(Act::SetModMeta(Box::new(m)), ctx);
                }
                if self.is_dirty() {
                    self.save();
                }
                if let Some(doc) = &self.doc {
                    self.build = Some(build::start(&doc.dir, self.dirs(), ctx));
                    self.show_build = true;
                }
            }
            Act::Undo => {
                if let Some(l) = self.doc.as_mut().and_then(|d| d.undo()) {
                    self.info(format!("undid: {l}"));
                    self.after_edit();
                }
            }
            Act::Redo => {
                if let Some(l) = self.doc.as_mut().and_then(|d| d.redo()) {
                    self.info(format!("redid: {l}"));
                    self.after_edit();
                }
            }
            Act::Reload => {
                if let Some(dir) = self.doc.as_ref().map(|d| d.dir.clone()).or(self.settings.map_dir.clone()) {
                    self.doc = None;
                    self.open(dir, ctx);
                }
            }
            Act::Open => {
                if let Some(f) = rfd::FileDialog::new().set_title("Open map: provinces.json").add_filter("provinces.json", &["json"]).pick_file() {
                    if let Some(dir) = f.parent() {
                        self.doc = None;
                        self.open(dir.to_path_buf(), ctx);
                    }
                }
            }
            Act::SetLevel(l) => self.set_level(l),
            Act::Select(l, ids) => {
                self.set_level(l);
                self.sel = ids;
            }
            Act::ZoomProvince(p) => self.zoom_provinces(&[p]),
            Act::ZoomSelection => {
                if self.settings.level == Level::Adjacencies {
                    if let (Some(i), Some(doc)) = (self.sel_adj, &self.doc) {
                        if let Some(a) = doc.data.adjacencies.get(i) {
                            let v = [a.from, a.to];
                            self.zoom_provinces(&v);
                        }
                    }
                } else {
                    self.zoom_provinces(&provs);
                }
            }
            Act::MergeProvinces if sel.len() >= 2 => self.run("Merge provinces", |d, _| {
                let p = d.merge_provinces(&sel)?;
                Ok(format!("merged {} provinces into {}", sel.len(), d.data.prov_name(p)))
            }),
            Act::DivideProvinces if need(&sel) => self.run("Divide provinces", |d, g| {
                let (a, b) = d.retile(&sel, Retile::Count(n), g)?;
                Ok(format!("divided {a} provinces into {b}"))
            }),
            Act::GenerateProvinces if need(&provs) => self.run("Generate provinces", |d, g| {
                let (a, b) = d.retile(&provs, Retile::Density, g)?;
                Ok(format!("{a} provinces re-cut by terrain into {b}"))
            }),
            Act::DissolveProvinces if need(&sel) => self.run("Dissolve provinces", |d, _| {
                let left = d.dissolve_provinces(&sel)?;
                Ok(if left > 0 { format!("{left} provinces had no neighbour of their type and were kept in part") } else { format!("dissolved {} provinces", sel.len()) })
            }),
            Act::SplitParts(all) => {
                let ids: Vec<Id> = if all { self.doc.as_ref().map(|d| d.data.provinces.ids()).unwrap_or_default() } else { provs.clone() };
                self.run("Split separate parts", |d, _| Ok(format!("{} new provinces from separate parts", d.split_disconnected(&ids))))
            }
            Act::SetKind(k) if need(&provs) => self.run(&format!("Make {}", k.name()), |d, _| Ok(format!("{} provinces are now {}", d.set_kind(&provs, k)?, k.name()))),
            Act::NewProvince => {
                self.new_province = true;
                self.settings.tool = Tool::Paint;
                self.info("paint the new province (it takes the state of the first pixel painted)");
            }
            Act::NewStateFromSel if need(&provs) => {
                let mut new_id = 0;
                self.run("New state", |d, _| {
                    new_id = d.new_state_from(&provs)?;
                    Ok(format!("new state {}", d.data.state_name(new_id)))
                });
                if new_id != 0 && self.doc.as_ref().is_some_and(|d| d.data.states.contains(new_id)) {
                    self.set_level(Level::States);
                    self.sel = vec![new_id];
                }
            }
            Act::NewZoneFromSel(naval) if need(&provs) => {
                let mut new_id = 0;
                self.run(if naval { "New sea zone" } else { "New air zone" }, |d, _| {
                    new_id = d.new_region_from(&provs, naval)?;
                    Ok(format!("new zone {}", d.data.region_name(new_id)))
                });
                if new_id != 0 {
                    self.set_level(if naval { Level::SeaZones } else { Level::AirZones });
                    self.sel = vec![new_id];
                }
            }
            Act::GrowTo(l) => {
                if let Some(doc) = &self.doc {
                    let mut seen = FxHashSet::default();
                    let ents: Vec<Id> = provs.iter().filter_map(|&p| Self::entity_of(doc, l, p)).filter(|e| seen.insert(*e)).collect();
                    let grown = Self::provinces_of(doc, l, &ents);
                    self.sel = match self.settings.level {
                        Level::Provinces | Level::Inaccessible => grown,
                        lv => {
                            let mut seen = FxHashSet::default();
                            grown.into_iter().filter_map(|p| Self::entity_of(doc, lv, p)).filter(|e| seen.insert(*e)).collect()
                        }
                    };
                }
            }
            Act::MergeStates if sel.len() >= 2 => self.run("Merge states", |d, _| {
                let s = d.merge_states(&sel)?;
                Ok(format!("merged {} states into {}", sel.len(), d.data.state_name(s)))
            }),
            Act::DivideStates if need(&sel) => self.run("Divide states", |d, _| {
                let mut made = 0;
                for &s in &sel {
                    made += d.divide_state(s, n)?.len();
                }
                Ok(format!("{} states became {made}", sel.len()))
            }),
            Act::DissolveStates if need(&sel) => self.run("Dissolve states", |d, _| Ok(format!("{} provinces joined neighbouring states", d.dissolve_states(&sel)?))),
            Act::GenerateStates => {
                let nations: Vec<Id> = match &self.doc {
                    Some(doc) => {
                        let mut seen = FxHashSet::default();
                        provs.iter().map(|&p| doc.data.nation_of(p)).filter(|&n| n != 0 && seen.insert(n)).collect()
                    }
                    None => Vec::new(),
                };
                if nations.is_empty() {
                    self.error("select states or nations first");
                } else {
                    self.run("Generate states", |d, g| Ok(format!("{} states for {} nations", d.generate_states(&nations, g)?, nations.len())));
                }
            }
            Act::NewNationFromSel => {
                let states: Vec<Id> = match &self.doc {
                    Some(doc) => {
                        let mut seen = FxHashSet::default();
                        provs.iter().map(|&p| doc.data.state_of(p)).filter(|&s| s != 0 && seen.insert(s)).collect()
                    }
                    None => Vec::new(),
                };
                let mut new_id = 0;
                self.run("New nation", |d, _| {
                    new_id = d.new_nation_from(&states)?;
                    Ok(format!("new nation {}", d.data.nation_name(new_id)))
                });
                if new_id != 0 {
                    self.set_level(Level::Nations);
                    self.sel = vec![new_id];
                }
            }
            Act::SetImpassable(v) if need(&sel) => self.run(if v { "Make states impassable" } else { "Make states passable" }, |d, _| {
                for &s in &sel {
                    if let Some(st) = d.data.states.get_mut(s) {
                        st.impassable = v;
                    }
                }
                Ok(String::new())
            }),
            Act::MergeNations if sel.len() >= 2 => self.run("Merge nations", |d, _| {
                let t = d.merge_nations(&sel)?;
                Ok(format!("merged {} nations into {}", sel.len(), d.data.nation_name(t)))
            }),
            Act::DivideNations if need(&sel) => self.run("Divide nations", |d, _| {
                let mut made = 0;
                for &x in &sel {
                    made += d.divide_nation(x, n)?.len();
                }
                Ok(format!("{} nations became {made}", sel.len()))
            }),
            Act::DissolveNations if need(&sel) => self.run("Dissolve nations", |d, _| Ok(format!("{} states joined neighbouring nations", d.dissolve_nations(&sel)?))),
            Act::GenerateNations if need(&sel) => self.run("Share out nations' land", |d, _| Ok(format!("{} states changed hands", d.generate_nations(&sel)?))),
            Act::MergeRegions if sel.len() >= 2 => self.run("Merge zones", |d, _| {
                let r = d.merge_regions(&sel)?;
                Ok(format!("merged into {}", d.data.region_name(r)))
            }),
            Act::DivideRegions if need(&sel) => self.run("Divide zones", |d, _| {
                let mut made = 0;
                for &r in &sel {
                    made += d.divide_region(r, n)?.len();
                }
                Ok(format!("{} zones became {made}", sel.len()))
            }),
            Act::DissolveRegions if need(&sel) => self.run("Dissolve zones", |d, _| Ok(format!("{} provinces joined neighbouring zones", d.dissolve_regions(&sel)?))),
            Act::GenerateRegions if need(&sel) => {
                let count = (self.settings.level == Level::SeaZones).then_some(n);
                self.run("Generate zones", |d, g| Ok(format!("{} zones", d.generate_regions(&sel, g, count)?)))
            }
            Act::MakeImpassable if need(&provs) => {
                let c = self.settings.coarsen;
                self.run("Make impassable", |d, g| Ok(format!("{} impassable states", d.make_impassable(&provs, c, g)?)))
            }
            Act::RemoveFromPlay if need(&provs) => {
                let c = self.settings.coarsen;
                self.run("Remove from play", |d, g| Ok(format!("{} provinces taken out of play", d.remove_from_play(&provs, c, g)?)))
            }
            Act::RestoreAccess if need(&provs) => self.run("Restore access", |d, _| Ok(format!("{} provinces / states back in play", d.restore_access(&provs)?))),
            Act::SetModMeta(m) => {
                if let Err(e) = m.check() {
                    self.error(format!("Mod details: {e:#}"));
                    return;
                }
                self.run("Mod details", |d, _| {
                    d.data.set_mod_meta(&m);
                    Ok(format!("mod details of \"{}\" {} set", m.name.trim(), m.version.trim()))
                });
                if let Some(meta) = &mut self.meta {
                    *meta = MetaEdit::new(*m);
                }
            }
            Act::SetMapFrame(on) => self.run(if on { "Map frame on" } else { "Map frame off" }, |d, _| {
                d.data.set_map_frame(on);
                Ok(if on { "the mod keeps vanilla's map frame".into() } else { "the mod leaves out vanilla's map frame".into() })
            }),
            Act::DeleteAdjacency(i) => {
                self.run("Delete adjacency", |d, _| {
                    if i < d.data.adjacencies.len() {
                        d.data.adjacencies.remove(i);
                    }
                    Ok(String::new())
                });
                self.sel_adj = None;
            }
            Act::AdjacencyPoints(i) => self.run("Recompute crossing points", |d, _| {
                let (f, t) = d.data.adjacencies.get(i).map(|a| (a.from, a.to)).ok_or_else(|| anyhow::anyhow!("no such adjacency"))?;
                let (a, b) = d.closest_points(f, t).ok_or_else(|| anyhow::anyhow!("provinces without pixels"))?;
                let adj = &mut d.data.adjacencies[i];
                adj.start = Some([a[0] as i64, a[1] as i64]);
                adj.stop = Some([b[0] as i64, b[1] as i64]);
                Ok(String::new())
            }),
            Act::ApplyDraft => {
                if let Some(draft) = self.draft.clone() {
                    self.run("Edit properties", move |d, _| {
                        match draft {
                            Draft::Province(id, p) => {
                                let cur = d.data.provinces.get_mut(id).ok_or_else(|| anyhow::anyhow!("province is gone"))?;
                                let (kind, state, region) = (cur.kind, cur.state, cur.region);
                                *cur = Province { kind, state, region, ..p };
                            }
                            Draft::State(id, s) => {
                                *d.data.states.get_mut(id).ok_or_else(|| anyhow::anyhow!("state is gone"))? = s;
                            }
                            Draft::Nation(id, n) => {
                                *d.data.nations.get_mut(id).ok_or_else(|| anyhow::anyhow!("nation is gone"))? = n;
                            }
                            Draft::Region(id, r) => {
                                *d.data.regions.get_mut(id).ok_or_else(|| anyhow::anyhow!("zone is gone"))? = r;
                            }
                            Draft::Adjacency(i, a) => {
                                *d.data.adjacencies.get_mut(i).ok_or_else(|| anyhow::anyhow!("adjacency is gone"))? = a;
                            }
                        }
                        Ok("properties applied".into())
                    });
                    self.draft = None;
                }
            }
            Act::RevertDraft => self.draft = None,
            Act::Check => self.start_check(ctx),
            Act::FixCrossings => self.run("Fix crossings", |d, _| Ok(format!("{} pixels changed", d.fix_crossings(None)))),
            other => self.error(format!("{other:?}: nothing (or not enough) selected")),
        }
    }

    pub fn save(&mut self) {
        let Some(doc) = self.doc.as_mut() else { return };
        match doc.save() {
            Ok(r) => {
                let mut m = format!(
                    "saved {} provinces, {} states, {} nations, {} zones, {} adjacencies",
                    r.provinces, r.states, r.nations, r.regions, r.adjacencies
                );
                if r.renumbered {
                    m.push_str(" (ids renumbered 1..n in the file; Reload to see them)");
                }
                if let Some(b) = r.backup {
                    m.push_str(&format!("; backup in {}", b.display()));
                }
                self.info(m);
            }
            Err(e) => self.error(format!("save failed: {e:#}")),
        }
    }

    // ------------------------------------------------------------ keyboard

    pub fn shortcuts(&mut self, ctx: &egui::Context) {
        let sc = |m, k| ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(m, k)));
        if sc(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z) || sc(Modifiers::COMMAND, Key::Y) {
            self.apply(Act::Redo, ctx);
        }
        if sc(Modifiers::COMMAND, Key::Z) {
            self.apply(Act::Undo, ctx);
        }
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let keys = [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7];
        for (i, k) in keys.into_iter().enumerate() {
            if ctx.input(|inp| inp.key_pressed(k) && inp.modifiers.is_none()) {
                self.set_level(Level::ALL[i]);
            }
        }
        let pressed = |k| ctx.input(|i| i.key_pressed(k) && !i.modifiers.command);
        if pressed(Key::S) {
            self.settings.tool = Tool::Select;
        }
        if pressed(Key::B) && self.settings.level != Level::Adjacencies {
            self.settings.tool = Tool::Paint;
        }
        if pressed(Key::L) {
            self.set_level(Level::Adjacencies);
            self.settings.tool = Tool::Link;
        }
        if pressed(Key::I) {
            self.settings.tool = Tool::Border;
        }
        if pressed(Key::OpenBracket) {
            self.settings.brush = (self.settings.brush / 1.25).max(0.5);
        }
        if pressed(Key::CloseBracket) {
            self.settings.brush = (self.settings.brush * 1.25).min(400.0);
        }
        if pressed(Key::Escape) {
            self.sel.clear();
            self.sel_adj = None;
            self.link_from = None;
            self.new_province = false;
        }
        if pressed(Key::Delete) {
            let act = match self.settings.level {
                Level::Provinces => Some(Act::DissolveProvinces),
                Level::States => Some(Act::DissolveStates),
                Level::Nations => Some(Act::DissolveNations),
                Level::AirZones | Level::SeaZones => Some(Act::DissolveRegions),
                Level::Adjacencies => self.sel_adj.map(Act::DeleteAdjacency),
                Level::Inaccessible => None,
            };
            if let Some(a) = act {
                self.apply(a, ctx);
            }
        }
    }

    // ------------------------------------------------------------ map

    fn refresh_style(&mut self) {
        let level = self.settings.level;
        let mode = self.settings.mode.effective(level);
        if mode == MapMode::SizeVsTarget {
            // cached; computed once the terrain is there and after pixel edits
            self.update_targets();
        }
        let Some(doc) = &self.doc else { return };
        let mut h: u64 = 1469598103934665603;
        for &i in &self.sel {
            h = (h ^ i as u64).wrapping_mul(1099511628211);
        }
        let key = StyleKey {
            meta: doc.meta_version,
            pixels: doc.pixel_version,
            level,
            mode,
            sel: h ^ self.sel.len() as u64,
            adj: self.sel_adj,
            borders: self.settings.borders,
            issues: self.issues_gen,
            targets: if mode == MapMode::SizeVsTarget { self.targets.as_ref().map(|t| t.0) } else { None },
        };
        if self.style_key.as_ref() == Some(&key) {
            return;
        }
        let style = self.build_style(doc, mode);
        self.style = Arc::new(style);
        self.style_key = Some(key);
        self.overlay_gen += 1;
    }

    /// Expected number of provinces per province by terrain (1 = right size).
    fn update_targets(&mut self) {
        let p = self.settings.params.density;
        let Some(doc) = self.doc.as_mut() else { return };
        let key = (doc.pixel_version, p);
        if self.targets.as_ref().is_some_and(|t| t.0 == key) {
            return;
        }
        let Some(dens) = doc.density(&p) else { return };
        let k = doc.data.km2_per_px() as f32;
        let sums = doc.sum_per_province(&dens);
        let d = &doc.data;
        let t: Vec<f32> = (0..d.provinces.cap())
            .map(|i| {
                let i = i as Id;
                match d.provinces.get(i) {
                    Some(pr) if pr.kind == Kind::Sea => doc.stats.get(i).px as f32 * k * p.detail / p.sea_km2,
                    Some(_) => sums[i as usize] as f32 * k,
                    None => 0.0,
                }
            })
            .collect();
        self.targets = Some((key, t));
    }

    fn build_style(&self, doc: &Doc, mode: MapMode) -> RenderStyle {
        let d = &doc.data;
        let n = d.provinces.cap();
        let mut fill = vec![[0u8, 0, 0, 0]; n];
        let mut ga = vec![0u32; n];
        let mut gb = vec![0u32; n];
        let mut selected = vec![false; n];
        const SEA: [u8; 3] = [42, 72, 120];
        const LAKE: [u8; 3] = [92, 150, 160];
        let km2 = d.km2_per_px();
        for (p, pr) in d.provinces.iter() {
            let i = p as usize;
            let st = d.states.get(pr.state);
            let imp = st.is_some_and(|s| s.impassable);
            let dim = |c: [u8; 3]| if imp { [c[0] / 2 + 30, c[1] / 2 + 25, c[2] / 2 + 20] } else { c };
            let nation = d.nation_of(p);
            let c: [u8; 3] = match mode {
                MapMode::ProvinceColours | MapMode::Auto => pr.rgb,
                MapMode::Types => match pr.kind {
                    Kind::Sea => [50, 95, 160],
                    Kind::Lake => [150, 120, 190],
                    Kind::Land if pr.edge => [95, 88, 82],
                    Kind::Land if imp => [140, 110, 80],
                    Kind::Land => [120, 160, 95],
                },
                MapMode::States => match pr.kind {
                    Kind::Sea => SEA,
                    Kind::Lake => LAKE,
                    Kind::Land => dim(render::id_colour(pr.state, 1)),
                },
                MapMode::Nations => match pr.kind {
                    Kind::Sea => SEA,
                    Kind::Lake => LAKE,
                    Kind::Land => dim(d.nations.get(nation).and_then(|x| x.color).unwrap_or_else(|| render::id_colour(nation, 2))),
                },
                MapMode::AirZones => match pr.kind {
                    Kind::Sea => [35, 50, 75],
                    _ => render::id_colour(pr.region, 3),
                },
                MapMode::SeaZones => match pr.kind {
                    Kind::Sea => render::id_colour(pr.region, 4),
                    _ => [72, 72, 72],
                },
                MapMode::Size => {
                    let a = doc.stats.get(p).px as f64 * km2;
                    render::ramp(((a.max(0.01).log10() + 1.0) / 3.0) as f32)
                }
                MapMode::SizeVsTarget => match &self.targets {
                    Some((_, t)) if pr.kind == Kind::Lake || pr.edge || imp => [90, 90, 90],
                    Some((_, t)) => {
                        let e = t.get(i).copied().unwrap_or(1.0).max(1e-3);
                        render::ramp((e.log2() + 2.0) / 4.0)
                    }
                    None => [90, 90, 90],
                },
                MapMode::Issues => match self.issue_provs.get(i).copied().unwrap_or(0) {
                    2 => [230, 60, 60],
                    1 => [240, 170, 60],
                    _ => [105, 110, 105],
                },
            };
            fill[i] = [c[0], c[1], c[2], 255];
            let (a, b) = match pr.kind {
                Kind::Sea => (1_000_000 + pr.region, 1_000_000 + pr.region),
                Kind::Lake => (2_000_000 + p, 2_000_000 + pr.region),
                Kind::Land => match mode {
                    MapMode::AirZones => (pr.state, 3_000_000 + pr.region),
                    _ => (pr.state, nation),
                },
            };
            ga[i] = a;
            gb[i] = b;
        }
        let level = self.settings.level;
        if level == Level::Adjacencies {
            if let Some(a) = self.sel_adj.and_then(|i| d.adjacencies.get(i)) {
                for p in [Some(a.from), Some(a.to), a.through].into_iter().flatten() {
                    if let Some(s) = selected.get_mut(p as usize) {
                        *s = true;
                    }
                }
            }
            if let Some(p) = self.link_from {
                if let Some(s) = selected.get_mut(p as usize) {
                    *s = true;
                }
            }
        } else {
            for p in Self::provinces_of(doc, level, &self.sel) {
                if let Some(s) = selected.get_mut(p as usize) {
                    *s = true;
                }
            }
        }
        RenderStyle { fill, group_a: ga, group_b: gb, selected, borders: self.settings.borders }
    }

    fn map_labels(&mut self) {
        let Some(doc) = &self.doc else { return };
        let key = (doc.meta_version, doc.pixel_version, self.settings.level);
        if self.labels_cache.as_ref().is_some_and(|c| c.0 == key) {
            return;
        }
        let d = &doc.data;
        let mut groups: Vec<(String, Vec<Id>)> = Vec::new();
        match self.settings.level {
            Level::States => {
                for (s, st) in d.states.iter() {
                    groups.push((st.name.clone(), doc.index.provs_of_state(s).to_vec()));
                }
            }
            Level::Nations | Level::Adjacencies | Level::Inaccessible => {
                for (n, nat) in d.nations.iter() {
                    let provs: Vec<Id> = doc.index.states_of_nation(n).iter().flat_map(|&s| doc.index.provs_of_state(s).to_vec()).collect();
                    groups.push((nat.name.split(" - ").next().unwrap_or(&nat.name).to_string(), provs));
                }
            }
            Level::AirZones | Level::SeaZones => {
                let naval = self.settings.level == Level::SeaZones;
                for (r, reg) in d.regions.iter() {
                    if reg.naval == naval {
                        groups.push((reg.name.clone(), doc.index.provs_of_region(r).to_vec()));
                    }
                }
            }
            Level::Provinces => {}
        }
        let labels = groups
            .into_iter()
            .filter(|(_, p)| !p.is_empty())
            .map(|(text, provs)| {
                let (mut sx, mut sy, mut n) = (0f64, 0f64, 0f64);
                let mut b = [u32::MAX, u32::MAX, 0, 0];
                for &p in &provs {
                    let s = doc.stats.get(p);
                    sx += s.sx as f64;
                    sy += s.sy as f64;
                    n += s.px as f64;
                    b = [b[0].min(s.x0), b[1].min(s.y0), b[2].max(s.x1), b[3].max(s.y1)];
                }
                let n = n.max(1.0);
                MapLabel { text, centre: [sx / n + 0.5, sy / n + 0.5], extent: ((b[2] - b[0]).min(b[3] - b[1]) as f64).max(1.0) }
            })
            .collect();
        self.labels_cache = Some((key, labels));
    }

    /// Draw the overlay and handle map input. Returns true if the editor uses
    /// the primary button (so the viewer must not pan with it).
    pub fn map_view(&mut self, dc: &DrawCtx, response: &egui::Response, tiles: &mut TileManager, crs: Crs) -> bool {
        if self.doc.is_none() {
            let text = if let Some((p, _)) = &self.loading {
                format!("Loading map… {} {:.0}%", p.stage(), p.get() * 100.0)
            } else {
                self.load_err.clone().unwrap_or_else(|| "No map open".into())
            };
            dc.painter.text(dc.rect.center() + egui::vec2(0.0, 40.0), Align2::CENTER_CENTER, text, FontId::proportional(16.0), Color32::LIGHT_GRAY);
            return false;
        }
        let Some(pl) = self.placement(crs) else {
            dc.painter.text(dc.rect.center(), Align2::CENTER_CENTER, "provinces.bmp has no georeference (provinces.wld)", FontId::proportional(16.0), ERR);
            return false;
        };
        self.refresh_style();
        let doc = self.doc.as_ref().unwrap();
        let overlay = Overlay { labels: doc.labels.clone(), style: self.style.clone(), w: doc.w, h: doc.h, generation: self.overlay_gen };
        view::draw_raster(dc, tiles, OVERLAY_LAYER, &overlay, &pl, self.settings.opacity);
        tiles.warm_overview(dc.ctx, OVERLAY_LAYER, &overlay);

        // hover
        self.hover = response.hover_pos().and_then(|p| pl.world_to_pixel(dc.cam.to_world(dc.rect, p)));
        let px_scale = pl.pixel_size_world() / dc.cam.mpp; // screen points per map pixel
        let uses_left = self.handle_input(dc, response, &pl, px_scale);
        self.hover_border = match (self.settings.tool, self.hover, &self.doc) {
            (Tool::Border, Some(h), Some(doc)) => doc.border_near(h, border_pick_radius(px_scale)),
            _ => None,
        };
        self.update_border_lines();

        // drawings on top
        self.draw_adjacencies(dc, &pl);
        if self.settings.labels {
            self.map_labels();
            self.draw_labels(dc, &pl, px_scale);
        }
        if self.settings.level == Level::Nations {
            self.draw_capitals(dc, &pl);
        }
        if let Some(h) = self.hover {
            if matches!(self.settings.tool, Tool::Paint) || (self.settings.tool == Tool::Select && self.stroke.is_some()) || self.settings.level == Level::Inaccessible {
                if let Some(w) = pl.pixel_to_world(h[0], h[1]) {
                    let c = dc.cam.to_screen(dc.rect, w);
                    let r = (self.settings.brush as f64 * px_scale) as f32;
                    dc.painter.circle_stroke(c, r.max(2.0), Stroke::new(1.5, Color32::from_rgb(255, 230, 120)));
                }
            }
        }
        if let (Some(f), Some(h)) = (self.link_from, self.hover) {
            if let Some(doc) = &self.doc {
                let c = doc.stats.get(f).centre();
                if let (Some(a), Some(b)) = (pl.pixel_to_world(c[0], c[1]), pl.pixel_to_world(h[0], h[1])) {
                    dc.painter.line_segment([dc.cam.to_screen(dc.rect, a), dc.cam.to_screen(dc.rect, b)], Stroke::new(2.0, Color32::from_rgb(255, 230, 120)));
                }
            }
        }
        uses_left
    }

    fn province_at(&self, px: [f64; 2]) -> Option<Id> {
        let doc = self.doc.as_ref()?;
        if px[0] < 0.0 || px[1] < 0.0 || px[0] >= doc.w as f64 || px[1] >= doc.h as f64 {
            return None;
        }
        let p = doc.labels.read().at(px[0] as u32, px[1] as u32);
        (p != 0).then_some(p)
    }

    fn handle_input(&mut self, dc: &DrawCtx, response: &egui::Response, pl: &Placement, px_scale: f64) -> bool {
        let tool = self.settings.tool;
        if tool == Tool::Pan {
            return false;
        }
        let level = self.settings.level;
        let mods = dc.ctx.input(|i| i.modifiers);
        let to_px = |p: Pos2| pl.world_to_pixel(dc.cam.to_world(dc.rect, p));
        let pos = response.interact_pointer_pos().or(response.hover_pos()).and_then(to_px);
        let brush = self.settings.brush as f64;

        if tool == Tool::Border {
            let origin = dc.ctx.input(|i| i.pointer.press_origin()).and_then(to_px).or(pos);
            self.border_input(dc, response, pos, origin, px_scale);
            return true;
        }

        // Adjacency level: pick lines / link provinces
        if level == Level::Adjacencies {
            if response.clicked_by(PointerButton::Primary) {
                let Some(px) = pos else { return true };
                if tool == Tool::Link {
                    let Some(p) = self.province_at(px) else { return true };
                    match self.link_from {
                        None => {
                            self.link_from = Some(p);
                            let name = self.doc.as_ref().map(|d| d.data.prov_name(p)).unwrap_or_default();
                            self.info(format!("from {name}: now click the other end"));
                            self.style_key = None;
                        }
                        Some(f) => {
                            let (kind, rule) = (self.adj_kind.clone(), self.adj_rule.clone());
                            let mut idx = None;
                            self.run("Add adjacency", |d, _| {
                                let i = d.add_adjacency(f, p, &kind, &rule, "")?;
                                idx = Some(i);
                                Ok(format!("added {}", d.data.adjacencies[i].name))
                            });
                            self.link_from = None;
                            self.sel_adj = idx;
                            self.style_key = None;
                        }
                    }
                } else {
                    // nearest adjacency line within 10 points
                    let click = response.interact_pointer_pos().unwrap_or_default();
                    let mut best: Option<(f32, usize)> = None;
                    if let Some(doc) = &self.doc {
                        for (i, a) in doc.data.adjacencies.iter().enumerate() {
                            if let Some((s, e)) = self.adj_screen(doc, a, dc, pl) {
                                let d = dist_to_segment(click, s, e);
                                if d < 10.0 && best.is_none_or(|b| d < b.0) {
                                    best = Some((d, i));
                                }
                            }
                        }
                    }
                    self.sel_adj = best.map(|b| b.1);
                }
            }
            return true;
        }

        // Alt+click: pick the entity under the cursor
        if mods.alt && response.clicked_by(PointerButton::Primary) {
            if let (Some(px), Some(doc)) = (pos, &self.doc) {
                if let Some(e) = self.province_at(px).and_then(|p| Self::entity_of(doc, level, p)) {
                    self.sel = vec![e];
                }
            }
            return true;
        }

        let painting_level = tool == Tool::Paint && level != Level::Inaccessible;
        if painting_level {
            // Paint: a dab on click, a stroke on drag
            let start = response.drag_started_by(PointerButton::Primary) || response.clicked_by(PointerButton::Primary);
            if start && self.stroke.is_none() {
                let origin = dc.ctx.input(|i| i.pointer.press_origin()).and_then(to_px).or(pos);
                if let Some(o) = origin {
                    if self.begin_paint(o) {
                        self.stroke = Some(Stroke2 { last: o, op: SelOp::Add, painting: true });
                        self.paint_to(o, brush);
                    }
                }
            }
            if self.stroke.is_some() && (response.dragged_by(PointerButton::Primary) || start) {
                if let Some(p) = pos {
                    self.paint_to(p, brush);
                }
            }
            let released = response.drag_stopped_by(PointerButton::Primary) || response.clicked_by(PointerButton::Primary) || !dc.ctx.input(|i| i.pointer.primary_down());
            if released && self.stroke.as_ref().is_some_and(|s| s.painting) {
                self.stroke = None;
                if let Some(doc) = self.doc.as_mut() {
                    if let Some(l) = doc.commit() {
                        self.info(l);
                    }
                }
                self.after_edit();
            }
            return true;
        }

        // Select (and the Inaccessible brush): click picks, drag brushes
        if response.clicked_by(PointerButton::Primary) {
            if let (Some(px), Some(doc)) = (pos, &self.doc) {
                let e = self.province_at(px).and_then(|p| Self::entity_of(doc, level, p));
                let op = if mods.command { SelOp::Remove } else if mods.shift || level == Level::Inaccessible && tool == Tool::Paint { SelOp::Add } else { SelOp::Replace };
                match (e, op) {
                    (Some(e), SelOp::Add) if self.sel.contains(&e) && tool != Tool::Paint => self.sel.retain(|&x| x != e),
                    (Some(e), op) => self.select_op(&[e], op),
                    (None, SelOp::Replace) => self.sel.clear(),
                    _ => {}
                }
            }
        }
        if response.drag_started_by(PointerButton::Primary) {
            let op = if mods.command { SelOp::Remove } else if mods.shift || tool == Tool::Paint { SelOp::Add } else { SelOp::Replace };
            if op == SelOp::Replace {
                self.sel.clear();
            }
            let origin = dc.ctx.input(|i| i.pointer.press_origin()).and_then(to_px).or(pos);
            if let Some(o) = origin {
                self.stroke = Some(Stroke2 { last: o, op: if op == SelOp::Replace { SelOp::Add } else { op }, painting: false });
            }
        }
        if let (Some(st), Some(p)) = (&self.stroke, pos) {
            if response.dragged_by(PointerButton::Primary) {
                let (a, op) = (st.last, st.op);
                if let Some(doc) = &self.doc {
                    let provs = doc.provinces_in_stroke(a, p, brush.max(1.0 / px_scale.max(1e-9)));
                    let mut seen = FxHashSet::default();
                    let ents: Vec<Id> = provs.into_iter().filter_map(|q| Self::entity_of(doc, level, q)).filter(|e| seen.insert(*e)).collect();
                    self.select_op(&ents, op);
                }
                if let Some(st) = &mut self.stroke {
                    st.last = p;
                }
            }
        }
        if response.drag_stopped_by(PointerButton::Primary) {
            self.stroke = None;
        }
        true
    }

    /// Border tool: a click toggles the border nearest to it, a drag blocks (or
    /// opens) every border it crosses; one undo step each.
    fn border_input(&mut self, dc: &DrawCtx, response: &egui::Response, pos: Option<[f64; 2]>, origin: Option<[f64; 2]>, px_scale: f64) {
        if response.clicked_by(PointerButton::Primary) {
            let Some(px) = pos else { return };
            let Some((a, b)) = self.doc.as_ref().and_then(|d| d.border_near(px, border_pick_radius(px_scale))) else {
                self.info("no border here: click on (or next to) a line between two provinces");
                return;
            };
            let on = self.doc.as_ref().is_some_and(|d| d.impassable_border(a, b).is_none());
            self.run(if on { "Make border impassable" } else { "Make border passable" }, |d, _| {
                d.set_border_impassable(a, b, on)?;
                let what = if on { "impassable" } else { "passable again" };
                Ok(format!("border {} | {} is {what}", d.data.prov_name(a), d.data.prov_name(b)))
            });
            self.sel_adj = None;
            return;
        }
        if response.drag_started_by(PointerButton::Primary) && self.border_stroke.is_none() {
            if let Some(o) = origin {
                self.border_stroke = Some(BorderStroke { last: o, make: None, changed: 0 });
            }
        }
        if let (Some(p), true) = (pos, response.dragged_by(PointerButton::Primary)) {
            if let (Some(st), Some(doc)) = (self.border_stroke.as_mut(), self.doc.as_mut()) {
                for (a, b) in doc.borders_crossed(st.last, p) {
                    let has = doc.impassable_border(a, b).is_some();
                    let make = *st.make.get_or_insert(!has);
                    if has == make {
                        continue;
                    }
                    if !doc.in_txn() {
                        doc.begin(if make { "Make borders impassable" } else { "Make borders passable" });
                    }
                    if doc.set_border_impassable(a, b, make).unwrap_or(false) {
                        st.changed += 1;
                        doc.meta_version += 1;
                    }
                }
                st.last = p;
            }
        }
        let released = response.drag_stopped_by(PointerButton::Primary) || !dc.ctx.input(|i| i.pointer.primary_down());
        if released {
            if let Some(st) = self.border_stroke.take() {
                if let Some(doc) = self.doc.as_mut() {
                    if doc.commit().is_some() {
                        let what = if st.make == Some(true) { "impassable" } else { "passable" };
                        self.info(format!("{} borders made {what}", st.changed));
                    }
                }
                self.sel_adj = None;
                self.after_edit();
            }
        }
    }

    /// Start a paint stroke; false if there is nothing to paint with.
    fn begin_paint(&mut self, at: [f64; 2]) -> bool {
        let level = self.settings.level;
        let under = self.province_at(at);
        let Some(doc) = self.doc.as_mut() else { return false };
        if level == Level::Provinces && self.new_province {
            let Some(u) = under else { return false };
            let (kind, state, region) = (doc.data.kind(u), doc.data.state_of(u), doc.data.region_of(u));
            doc.begin("Paint new province");
            let name = doc.data.prov_name(u);
            let id = doc.new_province(kind, state, region, name, Some(u));
            self.sel = vec![id];
            self.new_province = false;
            return true;
        }
        let Some(&target) = self.sel.first() else {
            self.error(format!("select (or Alt+click) the {} to paint with", level.noun()));
            return false;
        };
        let ok = match level {
            Level::Provinces => doc.data.provinces.contains(target),
            Level::States => doc.data.states.contains(target),
            Level::Nations => doc.data.nations.contains(target),
            Level::AirZones | Level::SeaZones => doc.data.regions.contains(target),
            _ => false,
        };
        if !ok {
            return false;
        }
        doc.begin(format!("Paint {}", level.noun()));
        true
    }

    fn paint_to(&mut self, p: [f64; 2], brush: f64) {
        let level = self.settings.level;
        let c = self.settings.constraint;
        let Some(st) = &mut self.stroke else { return };
        let a = st.last;
        st.last = p;
        let Some(&target) = self.sel.first() else { return };
        let Some(doc) = self.doc.as_mut() else { return };
        match level {
            Level::Provinces => {
                doc.paint(a, p, brush, target, c);
            }
            Level::States => {
                let provs = doc.provinces_in_stroke(a, p, brush);
                doc.assign_provinces_to_state(&provs, target);
            }
            Level::Nations => {
                let provs = doc.provinces_in_stroke(a, p, brush);
                let mut states: Vec<Id> = provs.iter().map(|&q| doc.data.state_of(q)).filter(|&s| s != 0).collect();
                states.sort_unstable();
                states.dedup();
                doc.assign_states_to_nation(&states, target);
            }
            Level::AirZones | Level::SeaZones => {
                let provs = doc.provinces_in_stroke(a, p, brush);
                let _ = doc.assign_to_region(&provs, target);
            }
            _ => {}
        }
        doc.meta_version += 1;
        self.last_edit = Instant::now();
    }

    fn adj_screen(&self, doc: &Doc, a: &Adjacency, dc: &DrawCtx, pl: &Placement) -> Option<(Pos2, Pos2)> {
        let pt = |o: Option<[i64; 2]>, p: Id| -> Option<[f64; 2]> {
            match o {
                Some(v) if v[0] >= 0 => Some([v[0] as f64 + 0.5, v[1] as f64 + 0.5]),
                _ => Some(doc.stats.get(p).centre()),
            }
        };
        let s = pt(a.start, a.from)?;
        let e = pt(a.stop, a.to)?;
        let (s, e) = (pl.pixel_to_world(s[0], s[1])?, pl.pixel_to_world(e[0], e[1])?);
        Some((dc.cam.to_screen(dc.rect, s), dc.cam.to_screen(dc.rect, e)))
    }

    /// Impassable borders are drawn along the border itself.
    fn shows_borders(&self) -> bool {
        self.settings.adjacencies || self.settings.level == Level::Adjacencies || self.settings.tool == Tool::Border
    }

    /// Border lines for the impassable borders on show and the one under the
    /// border tool.
    fn update_border_lines(&mut self) {
        let Some(doc) = &self.doc else { return };
        if self.border_lines.0 != doc.pixel_version {
            self.border_lines = (doc.pixel_version, FxHashMap::default());
        }
        let mut want: Vec<(Id, Id)> = Vec::new();
        if self.shows_borders() {
            want.extend(doc.data.adjacencies.iter().filter(|a| a.kind == "impassable").map(|a| (a.from.min(a.to), a.from.max(a.to))));
        }
        want.extend(self.hover_border.map(|(a, b)| (a.min(b), a.max(b))));
        for k in want {
            self.border_lines.1.entry(k).or_insert_with(|| Arc::new(doc.border_lines(k.0, k.1)));
        }
    }

    fn draw_border(&self, dc: &DrawCtx, pl: &Placement, a: Id, b: Id, width: f32, col: Color32) {
        let Some(lines) = self.border_lines.1.get(&(a.min(b), a.max(b))) else { return };
        for line in lines.iter() {
            // through the middle of each pixel edge run: staircases become slopes
            let n = line.len();
            let smooth = std::iter::once(line[0])
                .chain(line.windows(2).map(|w| [(w[0][0] + w[1][0]) / 2.0, (w[0][1] + w[1][1]) / 2.0]))
                .chain(std::iter::once(line[n - 1]));
            let mut pts: Vec<Pos2> = Vec::with_capacity(n + 1);
            for (k, p) in smooth.enumerate() {
                let Some(w) = pl.pixel_to_world(p[0], p[1]) else { continue };
                let q = dc.cam.to_screen(dc.rect, w);
                // thin out points closer than a screen pixel (keep the last one)
                if pts.last().is_some_and(|l: &Pos2| l.distance(q) < 1.0) && k < n {
                    continue;
                }
                pts.push(q);
            }
            if pts.len() < 2 || !dc.rect.expand(width).intersects(Rect::from_points(&pts)) {
                continue;
            }
            dc.painter.add(egui::Shape::line(pts.clone(), Stroke::new(width + 2.0, Color32::from_black_alpha(150))));
            dc.painter.add(egui::Shape::line(pts, Stroke::new(width, col)));
        }
    }

    fn draw_adjacencies(&self, dc: &DrawCtx, pl: &Placement) {
        let Some(doc) = &self.doc else { return };
        let on_level = self.settings.level == Level::Adjacencies;
        let border_tool = self.settings.tool == Tool::Border;
        if let Some((a, b)) = self.hover_border.filter(|_| border_tool) {
            self.draw_border(dc, pl, a, b, 5.0, Color32::from_rgb(255, 225, 60));
        }
        if self.shows_borders() {
            for (i, a) in doc.data.adjacencies.iter().enumerate().filter(|(_, a)| a.kind == "impassable") {
                let selected = on_level && self.sel_adj == Some(i);
                let hovered = border_tool && self.hover_border.is_some_and(|(p, q)| (p.min(q), p.max(q)) == (a.from.min(a.to), a.from.max(a.to)));
                let col = if selected || hovered { Color32::from_rgb(255, 225, 60) } else { Color32::from_rgb(235, 45, 45) };
                let width = if selected { 4.5 } else if on_level || border_tool { 3.5 } else { 2.5 };
                self.draw_border(dc, pl, a.from, a.to, width, col);
            }
        }
        if let (Some((a, b)), Some(h)) = (self.hover_border.filter(|_| border_tool), self.hover) {
            if let Some(w) = pl.pixel_to_world(h[0], h[1]) {
                let c = dc.cam.to_screen(dc.rect, w) + egui::vec2(16.0, -16.0);
                let blocked = doc.impassable_border(a, b).is_some();
                let text = format!(
                    "{} | {}\n{}",
                    doc.data.prov_name(a),
                    doc.data.prov_name(b),
                    if blocked { "impassable · click: open" } else { "click: make impassable" }
                );
                let font = FontId::proportional(13.0);
                for d in [egui::vec2(1.0, 1.0), egui::vec2(-1.0, -1.0), egui::vec2(1.0, -1.0), egui::vec2(-1.0, 1.0)] {
                    dc.painter.text(c + d, Align2::LEFT_BOTTOM, &text, font.clone(), Color32::from_black_alpha(220));
                }
                dc.painter.text(c, Align2::LEFT_BOTTOM, &text, font, Color32::WHITE);
            }
        }
        if !self.settings.adjacencies && !on_level {
            return;
        }
        for (i, a) in doc.data.adjacencies.iter().enumerate() {
            if a.kind == "impassable" && !on_level {
                continue; // drawn along the border above; the cross is the handle on the adjacency level
            }
            let Some((s, e)) = self.adj_screen(doc, a, dc, pl) else { continue };
            if !dc.rect.intersects(Rect::from_two_pos(s, e).expand(4.0)) {
                continue;
            }
            let base = match a.kind.as_str() {
                "sea" => Color32::from_rgb(90, 220, 255),
                "impassable" => Color32::from_rgb(255, 80, 80),
                _ => Color32::from_rgb(120, 240, 120),
            };
            let selected = on_level && self.sel_adj == Some(i);
            let (wd, col) = if selected { (4.0, Color32::from_rgb(255, 225, 60)) } else if on_level { (2.5, base) } else { (1.5, base.gamma_multiply(0.8)) };
            dc.painter.line_segment([s, e], Stroke::new(wd + 1.5, Color32::from_black_alpha(160)));
            dc.painter.line_segment([s, e], Stroke::new(wd, col));
            if a.kind == "impassable" {
                let m = s + (e - s) * 0.5;
                let k = 5.0;
                dc.painter.line_segment([m + egui::vec2(-k, -k), m + egui::vec2(k, k)], Stroke::new(2.0, col));
                dc.painter.line_segment([m + egui::vec2(-k, k), m + egui::vec2(k, -k)], Stroke::new(2.0, col));
            }
            for p in [s, e] {
                dc.painter.circle_filled(p, wd + 1.0, col);
            }
        }
    }

    fn draw_labels(&self, dc: &DrawCtx, pl: &Placement, px_scale: f64) {
        let Some((_, labels)) = &self.labels_cache else { return };
        if self.settings.level == Level::Provinces {
            return;
        }
        let mut order: Vec<&MapLabel> = labels.iter().filter(|l| l.extent * px_scale > 50.0).collect();
        order.sort_by(|a, b| b.extent.total_cmp(&a.extent));
        let mut placed: Vec<Rect> = Vec::new();
        let font = FontId::proportional(13.0);
        for l in order.into_iter().take(400) {
            let Some(w) = pl.pixel_to_world(l.centre[0], l.centre[1]) else { continue };
            let p = dc.cam.to_screen(dc.rect, w);
            if !dc.rect.contains(p) {
                continue;
            }
            let galley = dc.painter.layout_no_wrap(l.text.clone(), font.clone(), Color32::WHITE);
            let rect = Align2::CENTER_CENTER.anchor_size(p, galley.size()).expand(2.0);
            if placed.iter().any(|r| r.intersects(rect)) {
                continue;
            }
            placed.push(rect);
            for d in [egui::vec2(1.0, 1.0), egui::vec2(-1.0, -1.0), egui::vec2(1.0, -1.0), egui::vec2(-1.0, 1.0)] {
                dc.painter.text(p + d, Align2::CENTER_CENTER, &l.text, font.clone(), Color32::from_black_alpha(220));
            }
            dc.painter.galley(rect.shrink(2.0).min, galley, Color32::WHITE);
        }
    }

    fn draw_capitals(&self, dc: &DrawCtx, pl: &Placement) {
        let Some(doc) = &self.doc else { return };
        for (_, n) in doc.data.nations.iter() {
            let provs = doc.index.provs_of_state(n.capital_state);
            let Some(&best) = provs.iter().max_by_key(|&&p| doc.stats.get(p).px) else { continue };
            let c = doc.stats.get(best).centre();
            let Some(w) = pl.pixel_to_world(c[0], c[1]) else { continue };
            let p = dc.cam.to_screen(dc.rect, w);
            if dc.rect.contains(p) {
                dc.painter.circle(p, 3.5, Color32::from_rgb(255, 235, 120), Stroke::new(1.0, Color32::BLACK));
            }
        }
    }

    // ------------------------------------------------------------ panel

    pub fn side_panel(&mut self, ui: &mut egui::Ui, layers: &mut [Layer]) {
        let ctx = ui.ctx().clone();
        let mut acts: Vec<Act> = Vec::new();
        ui.heading("Map editor");
        if let Some((p, _)) = &self.loading {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(format!("{} {:.0}%", p.stage(), p.get() * 100.0));
            });
        }
        if let Some(e) = &self.load_err {
            ui.colored_label(ERR, e);
        }
        if self.doc.is_none() {
            ui.horizontal(|ui| {
                if ui.button("Open map…").on_hover_text("Pick the provinces.json of a map folder").clicked() {
                    acts.push(Act::Open);
                }
                if self.load_err.is_some() && ui.button("Retry").clicked() {
                    self.load_err = None;
                }
            });
            for a in acts {
                self.apply(a, &ctx);
            }
            return;
        }
        self.header_ui(ui, &mut acts);
        ui.separator();
        egui::ScrollArea::vertical().id_salt("editor_scroll").auto_shrink([false, false]).show(ui, |ui| {
            self.selection_ui(ui, &mut acts);
            self.actions_ui(ui, &mut acts);
            self.properties_ui(ui, &mut acts);
            self.generation_ui(ui);
            self.hover_ui(ui);
            self.validation_ui(ui, &mut acts);
            self.mod_options_ui(ui, &mut acts);
            egui::CollapsingHeader::new("Background layers").default_open(false).show(ui, |ui| {
                for l in layers.iter_mut().rev() {
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut l.cfg.visible, &l.cfg.name);
                        ui.add(egui::Slider::new(&mut l.cfg.opacity, 0.0..=1.0).show_value(false));
                    });
                }
            });
        });
        for a in acts {
            self.apply(a, &ctx);
        }
    }

    fn header_ui(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        let doc = self.doc.as_ref().unwrap();
        ui.label(RichText::new(doc.dir.display().to_string()).small().weak());
        ui.label(format!(
            "{} provinces · {} states · {} nations · {} zones · {} adjacencies{}",
            doc.data.provinces.count(),
            doc.data.states.count(),
            doc.data.nations.count(),
            doc.data.regions.count(),
            doc.data.adjacencies.len(),
            if doc.dirty { " · unsaved" } else { "" }
        ));
        if let Some(t) = &self.terrain_msg {
            ui.label(RichText::new(t).small().color(WARN));
        }
        let (undo, redo) = (doc.can_undo().map(str::to_string), doc.can_redo().map(str::to_string));
        ui.horizontal_wrapped(|ui| {
            if ui.add_enabled(doc.dirty, egui::Button::new("💾 Save")).on_hover_text("Write provinces.bmp / provinces.json (Ctrl+S); the old files go to backups/").clicked() {
                acts.push(Act::Save);
            }
            let building = self.build.as_ref().is_some_and(|b| b.state() == build::RUNNING);
            if ui.add_enabled(!building, egui::Button::new("🔨 Build mod")).on_hover_text("Save, make the HOI4 mod in the game's mod folder and check it (see Mod details)").clicked() {
                acts.push(Act::Build);
            }
            if ui.button(if building { "▶ Play… (building)" } else { "▶ Play…" }).on_hover_text("Build log, mods to load and starting the game").clicked() {
                self.show_build = true;
            }
            if ui.add_enabled(undo.is_some(), egui::Button::new("Undo")).on_hover_text(format!("Undo {} (Ctrl+Z)", undo.unwrap_or_default())).clicked() {
                acts.push(Act::Undo);
            }
            if ui.add_enabled(redo.is_some(), egui::Button::new("Redo")).on_hover_text(format!("Redo {} (Ctrl+Shift+Z)", redo.unwrap_or_default())).clicked() {
                acts.push(Act::Redo);
            }
            if ui.button("Reload").on_hover_text("Discard unsaved changes and read the files again").clicked() {
                acts.push(Act::Reload);
            }
            if ui.button("Open…").clicked() {
                acts.push(Act::Open);
            }
        });
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.label("Edit:");
            for (i, l) in Level::ALL.into_iter().enumerate() {
                if ui.selectable_label(self.settings.level == l, l.name()).on_hover_text(format!("key {}", i + 1)).clicked() {
                    acts.push(Act::SetLevel(l));
                }
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("Tool:");
            let level = self.settings.level;
            ui.selectable_value(&mut self.settings.tool, Tool::Select, "⬚ Select").on_hover_text(
                "S · click: select, Shift: add/remove, Ctrl: remove, drag: select with the brush, Alt+click: pick",
            );
            if level != Level::Adjacencies {
                let tip = match level {
                    Level::Provinces => "B · paint the selected province over others (redraw borders)",
                    Level::Inaccessible => "B · brush-select provinces to make inaccessible",
                    _ => "B · paint: what the brush touches joins the selected entity",
                };
                ui.selectable_value(&mut self.settings.tool, Tool::Paint, "🖌 Paint").on_hover_text(tip);
            } else {
                ui.selectable_value(&mut self.settings.tool, Tool::Link, "🔗 Link").on_hover_text("L · click two provinces to add an adjacency");
            }
            ui.selectable_value(&mut self.settings.tool, Tool::Border, "⛔ Border").on_hover_text(
                "I · click a border between two provinces to make it impassable (click again: passable); \
                 drag across borders to block (or open) them all",
            );
            ui.selectable_value(&mut self.settings.tool, Tool::Pan, "✋ Pan").on_hover_text("left drag pans (middle/right drag always pans)");
        });
        if matches!(self.settings.tool, Tool::Paint | Tool::Select) {
            ui.horizontal(|ui| {
                ui.add(egui::Slider::new(&mut self.settings.brush, 0.5..=400.0).logarithmic(true).text("brush px ([ ])"));
            });
            if self.settings.tool == Tool::Paint && self.settings.level == Level::Provinces {
                ui.horizontal_wrapped(|ui| {
                    ui.label("only over:");
                    ui.checkbox(&mut self.settings.constraint.same_kind, "same type").on_hover_text("land paints only land, sea only sea (keeps the coastline)");
                    ui.checkbox(&mut self.settings.constraint.same_state, "same state");
                    ui.checkbox(&mut self.settings.constraint.same_nation, "same nation");
                });
                if self.new_province {
                    ui.colored_label(WARN, "next stroke paints a NEW province");
                }
            }
        }
        ui.horizontal(|ui| {
            ui.label("Colour:");
            egui::ComboBox::from_id_salt("mapmode").selected_text(self.settings.mode.name()).show_ui(ui, |ui| {
                for m in MapMode::ALL {
                    ui.selectable_value(&mut self.settings.mode, m, m.name());
                }
            });
        });
        ui.horizontal_wrapped(|ui| {
            ui.add(egui::Slider::new(&mut self.settings.opacity, 0.0..=1.0).text("opacity"));
            ui.checkbox(&mut self.settings.borders, "borders");
            ui.checkbox(&mut self.settings.labels, "names");
            ui.checkbox(&mut self.settings.adjacencies, "adjacencies");
        });
        if self.settings.tool == Tool::Border {
            ui.label(
                RichText::new("Impassable borders are red. Units cannot cross them; railways avoid them. Drag along a ridge to block every border it crosses.")
                    .small()
                    .weak(),
            );
        }
        if self.settings.mode.effective(self.settings.level) == MapMode::SizeVsTarget {
            ui.label(RichText::new("blue: smaller than the terrain wants · green: right · red: bigger (generate splits it)").small().weak());
            if self.doc.as_ref().is_some_and(|d| d.terrain.is_none()) {
                ui.colored_label(WARN, "needs the terrain (still loading?)");
            }
        }
    }

    /// The mod made from the map: its name, where it goes, the map frame.
    fn mod_options_ui(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        let doc = self.doc.as_ref().unwrap();
        let (w, h) = (doc.w, doc.h);
        let meta = self.meta.as_ref().map(|e| e.m.clone()).unwrap_or_else(|| doc.data.mod_meta());
        let mut frame = doc.data.map_frame();
        let target = self.dirs().mods.join(&meta.folder);
        let mut open_meta = std::mem::take(&mut self.meta_at_start);
        egui::CollapsingHeader::new("Mod").id_salt("modopts").default_open(true).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(meta.name.trim()).strong());
                ui.label(format!("v{}", meta.version.trim()));
                if !meta.author.trim().is_empty() {
                    ui.label(RichText::new(format!("by {}", meta.author.trim())).weak());
                }
            });
            ui.label(RichText::new(format!("builds into {}", target.display())).small().weak());
            if ui.button("✏ Mod details…").on_hover_text("Name, version, author, description, tags, thumbnail, Steam Workshop id, build settings and folders").clicked() {
                open_meta = true;
            }
            if ui
                .checkbox(&mut frame, "HOI4 map frame")
                .on_hover_text(
                    "Vanilla's 3D metal border with the \"Hearts of Iron IV\" logo around the map (map/ambient_object.txt). \
                     It is placed for vanilla's 5632×2048 map: on a map of another size it cuts through it or ends beside it.",
                )
                .changed()
            {
                acts.push(Act::SetMapFrame(frame));
            }
            if frame && (w, h) != (VANILLA_MAP[0], VANILLA_MAP[1]) {
                ui.colored_label(WARN, format!("this map is {w}×{h}, the frame fits {}×{}", VANILLA_MAP[0], VANILLA_MAP[1]));
            }
            ui.label(RichText::new("takes effect with the next Build mod").small().weak());
        });
        if open_meta && self.meta.is_none() {
            self.meta = Some(MetaEdit::new(meta));
        }
    }

    /// Where the HOI4 install is and where mods are built.
    fn dirs(&self) -> modbuild::Dirs {
        modbuild::Dirs {
            game: self.settings.game_dir.clone().unwrap_or_else(modbuild::default_game_dir),
            mods: self.settings.mods_dir.clone().unwrap_or_else(modbuild::default_mods_dir),
        }
    }

    fn game_version(&mut self, game: &Path) -> String {
        if self.game_version.as_ref().is_none_or(|(p, _)| p != game) {
            self.game_version = Some((game.to_path_buf(), modbuild::vanilla::game_version(game)));
        }
        self.game_version.as_ref().unwrap().1.clone()
    }

    /// The thumbnail preview of `path` (loaded once per path).
    fn thumbnail(&mut self, ctx: &egui::Context, path: &Path) -> Result<egui::TextureHandle, String> {
        if self.thumb.as_ref().is_none_or(|(p, _)| p != path) {
            let tex = image::open(path).map_err(|e| e.to_string()).map(|img| {
                let size = format!("{}×{}", img.width(), img.height());
                let small = img.thumbnail(320, 320).to_rgba8();
                let ci = egui::ColorImage::from_rgba_unmultiplied([small.width() as usize, small.height() as usize], small.as_raw());
                (ctx.load_texture(format!("mod_thumb {size}"), ci, egui::TextureOptions::LINEAR), size)
            });
            self.thumb = Some((path.to_path_buf(), tex.map(|(t, _)| t)));
        }
        self.thumb.as_ref().unwrap().1.clone()
    }

    /// The "Mod details" window: descriptor fields, build settings, folders.
    fn meta_window(&mut self, ctx: &egui::Context) {
        let Some(doc) = &self.doc else {
            self.meta = None;
            return;
        };
        let Some(mut e) = self.meta.take() else { return };
        // undo, redo or reload under the open window: follow them unless the copy was edited
        let cur = doc.data.mod_meta();
        if cur != e.orig {
            e = if e.m == e.orig { MetaEdit::new(cur) } else { MetaEdit { orig: cur, ..e } };
        }
        let map_dir = doc.dir.clone();
        let dirs = self.dirs();
        let version = self.game_version(&dirs.game);
        let picture = e.m.picture_path(&map_dir);
        let thumb = picture.as_ref().filter(|p| p.is_file()).map(|p| self.thumbnail(ctx, p));
        let mut open = true;
        let mut act: Option<Act> = None;
        let mut close = false;
        let (mut game_dir, mut mods_dir) = (self.settings.game_dir.clone(), self.settings.mods_dir.clone());
        let mut browse_picture = false;
        egui::Window::new("Mod details").open(&mut open).default_width(560.0).default_height(640.0).show(ctx, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, true]).max_height(ctx.content_rect().height() * 0.75).show(ui, |ui| {
                let m = &mut e.m;
                fn wide<'t>(t: &'t mut String, hint: &str) -> egui::TextEdit<'t> {
                    egui::TextEdit::singleline(t).desired_width(360.0).hint_text(hint.to_string())
                }
                ui.heading("Description");
                egui::Grid::new("meta_desc").num_columns(2).min_col_width(118.0).spacing([12.0, 6.0]).show(ui, |ui| {
                    ui.label("Name");
                    ui.add(wide(&mut m.name, "as the launcher lists it"));
                    ui.end_row();
                    ui.label("Version");
                    ui.add(wide(&mut m.version, "e.g. 0.1"));
                    ui.end_row();
                    ui.label("Author");
                    ui.add(wide(&mut m.author, "you"));
                    ui.end_row();
                    ui.label("Description");
                    ui.add(egui::TextEdit::multiline(&mut m.description).desired_width(360.0).desired_rows(4).hint_text("what the mod is about"));
                    ui.end_row();
                    ui.label("Tags");
                    ui.vertical(|ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.set_max_width(380.0);
                            for t in modbuild::meta::KNOWN_TAGS {
                                let mut on = m.tags.iter().any(|x| x == t);
                                if ui.checkbox(&mut on, t).changed() {
                                    if on {
                                        m.tags.push(t.to_string());
                                    } else {
                                        m.tags.retain(|x| x != t);
                                    }
                                }
                            }
                        });
                        if ui.add(wide(&mut e.other_tags, "other tags, comma separated")).changed() {
                            m.tags.retain(|x| modbuild::meta::KNOWN_TAGS.contains(&x.as_str()));
                            m.tags.extend(e.other_tags.split(',').map(str::trim).filter(|t| !t.is_empty()).map(String::from));
                        }
                    });
                    ui.end_row();
                    ui.label("Thumbnail");
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut m.picture).desired_width(230.0).hint_text("PNG or JPEG, in the map folder or anywhere"));
                            if ui.button("Browse…").clicked() {
                                browse_picture = true;
                            }
                            if !m.picture.is_empty() && ui.button("Clear").clicked() {
                                m.picture.clear();
                            }
                        });
                        match (&picture, &thumb) {
                            (None, _) => {
                                ui.label(RichText::new("none: the launcher shows a blank picture").small().weak());
                            }
                            (Some(p), None) => {
                                ui.colored_label(ERR, format!("{} does not exist", p.display()));
                            }
                            (Some(_), Some(Err(err))) => {
                                ui.colored_label(ERR, format!("cannot read it: {err}"));
                            }
                            (Some(p), Some(Ok(tex))) => {
                                let s = tex.size_vec2();
                                ui.image((tex.id(), s * (160.0 / s.x.max(s.y)).min(1.0)));
                                let bytes = std::fs::metadata(p).map(|x| x.len()).unwrap_or(0);
                                let name = tex.name();
                                ui.label(RichText::new(format!("{} px, {:.0} kB; written as thumbnail.png (at most 1024 px)", name.trim_start_matches("mod_thumb "), bytes as f64 / 1e3)).small().weak());
                            }
                        }
                    });
                    ui.end_row();
                    ui.label("Game version");
                    ui.add(wide(&mut m.supported_version, &format!("{version} (the installed game's)")));
                    ui.end_row();
                    ui.label("Workshop id").on_hover_text("remote_file_id: the Steam Workshop item, once uploaded");
                    ui.add(wide(&mut m.remote_file_id, "set after the first upload"));
                    ui.end_row();
                    ui.label("Dependencies").on_hover_text("Mods this one loads after when they are enabled (load order, not a requirement), by the name in their descriptor.mod; one per line");
                    if ui.add(egui::TextEdit::multiline(&mut e.deps).desired_width(360.0).desired_rows(2).hint_text("one mod name per line")).changed() {
                        m.dependencies = e.deps.lines().map(str::trim).filter(|t| !t.is_empty()).map(String::from).collect();
                    }
                    ui.end_row();
                });
                ui.add_space(6.0);
                ui.heading("Build");
                egui::Grid::new("meta_build").num_columns(2).min_col_width(118.0).spacing([12.0, 6.0]).show(ui, |ui| {
                    ui.label("Mod folder").on_hover_text("Folder and .mod file name in the mod folder; letters, digits, _ and -");
                    ui.add(wide(&mut m.folder, "e.g. suedtirol"));
                    ui.end_row();
                    ui.label("Bookmark country").on_hover_text("The country the 1936 start offers first: a tag set in the editor or part of a settlement name");
                    ui.add(wide(&mut m.player, "empty: the biggest city"));
                    ui.end_row();
                    ui.label("Manpower base").on_hover_text("Population of a nation's capital state besides its built-up land; other states get a quarter");
                    ui.add(egui::DragValue::new(&mut m.manpower_base).range(0.0..=10_000_000.0).speed(1000.0));
                    ui.end_row();
                    ui.label("Manpower per km²").on_hover_text("Population per km² of sealed (built-up) land");
                    ui.add(egui::DragValue::new(&mut m.manpower_urban).range(0.0..=10_000_000.0).speed(1000.0));
                    ui.end_row();
                    ui.label("Relief").on_hover_text("Slope exaggeration of the normal map (shading of the 3D map)");
                    ui.add(egui::Slider::new(&mut m.relief, 0.0..=3.0));
                    ui.end_row();
                    ui.label("Seed").on_hover_text("Random choices: country colours, flags, building positions");
                    ui.add(egui::DragValue::new(&mut m.seed));
                    ui.end_row();
                });
                ui.add_space(6.0);
                ui.heading("Folders on this computer");
                ui.label(RichText::new("kept with the editor settings, not in the map").small().weak());
                egui::Grid::new("meta_dirs").num_columns(2).min_col_width(118.0).spacing([12.0, 6.0]).show(ui, |ui| {
                    for (label, dir, default, check) in [
                        ("HOI4 install", &mut game_dir, modbuild::default_game_dir(), "map/provinces.bmp"),
                        ("Mods go to", &mut mods_dir, modbuild::default_mods_dir(), ""),
                    ] {
                        ui.label(label);
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                let shown = dir.clone().unwrap_or(default.clone());
                                ui.label(RichText::new(shown.display().to_string()).monospace().small());
                                if ui.button("Browse…").clicked()
                                    && let Some(p) = rfd::FileDialog::new().set_directory(&shown).pick_folder()
                                {
                                    *dir = Some(p);
                                }
                                if dir.is_some() && ui.button("Default").clicked() {
                                    *dir = None;
                                }
                            });
                            let shown = dir.clone().unwrap_or(default);
                            if !check.is_empty() && !shown.join(check).exists() {
                                ui.colored_label(ERR, format!("no {check} there"));
                            }
                        });
                        ui.end_row();
                    }
                });
                ui.add_space(6.0);
                egui::CollapsingHeader::new("descriptor.mod").id_salt("meta_desc_preview").show(ui, |ui| {
                    let text = m.descriptor(&version, thumb.as_ref().is_some_and(|t| t.is_ok()));
                    ui.add(egui::Label::new(RichText::new(text).monospace().small()).wrap_mode(egui::TextWrapMode::Extend));
                    if let Some(r) = m.readme() {
                        ui.label(RichText::new("README.txt (the descriptor has no author or description):").small().weak());
                        ui.add(egui::Label::new(RichText::new(r).monospace().small()));
                    }
                });
            });
            ui.separator();
            let problem = e.m.check().err();
            if let Some(p) = &problem {
                ui.colored_label(ERR, format!("{p:#}"));
            }
            let changed = e.m != e.orig;
            ui.horizontal(|ui| {
                if ui.add_enabled(changed && problem.is_none(), egui::Button::new("Apply")).on_hover_text("One undo step; saved with the map").clicked() {
                    act = Some(Act::SetModMeta(Box::new(e.m.clone())));
                }
                if ui.add_enabled(changed, egui::Button::new("Revert")).clicked() {
                    e = MetaEdit::new(e.orig.clone());
                }
                if ui.button("Close").on_hover_text(if changed { "Close and drop the changes" } else { "" }).clicked() {
                    close = true;
                }
                if changed {
                    ui.colored_label(WARN, "not applied");
                }
            });
        });
        if browse_picture {
            let start = picture.as_ref().and_then(|p| p.parent().map(Path::to_path_buf)).unwrap_or(map_dir.clone());
            if let Some(p) = rfd::FileDialog::new().set_directory(start).add_filter("picture", &["png", "jpg", "jpeg"]).pick_file() {
                // inside the map folder: relative, so the map folder can move
                e.m.picture = p.strip_prefix(&map_dir).map(|r| r.display().to_string()).unwrap_or_else(|_| p.display().to_string());
            }
        }
        self.settings.game_dir = game_dir;
        self.settings.mods_dir = mods_dir;
        if open && !close {
            self.meta = Some(e);
        }
        if let Some(a) = act {
            self.apply(a, ctx);
        }
    }

    fn selection_ui(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        let doc = self.doc.as_ref().unwrap();
        let level = self.settings.level;
        if level == Level::Adjacencies {
            return;
        }
        let d = &doc.data;
        let name = |id: Id| match level {
            Level::Provinces | Level::Inaccessible | Level::Adjacencies => d.prov_name(id),
            Level::States => d.state_name(id),
            Level::Nations => d.nation_name(id),
            Level::AirZones | Level::SeaZones => d.region_name(id),
        };
        egui::CollapsingHeader::new(format!("Selection: {} {}(s)", self.sel.len(), level.noun())).id_salt("sel").default_open(true).show(ui, |ui| {
            if self.sel.is_empty() {
                ui.label(RichText::new("Click on the map (Shift adds, drag to brush-select).").weak());
                return;
            }
            ui.horizontal(|ui| {
                if ui.button("Zoom to").clicked() {
                    acts.push(Act::ZoomSelection);
                }
                if ui.button("Clear (Esc)").clicked() {
                    acts.push(Act::Select(level, Vec::new()));
                }
            });
            egui::ScrollArea::vertical().id_salt("sel_list").max_height(110.0).show(ui, |ui| {
                for (i, &id) in self.sel.iter().take(60).enumerate() {
                    let t = format!("{}{} ({id})", if i == 0 { "▶ " } else { "" }, name(id));
                    if ui.selectable_label(i == 0, t).on_hover_text("first = the one painted with / edited below; click to make it first").clicked() {
                        let mut v = self.sel.clone();
                        v.remove(i);
                        v.insert(0, id);
                        acts.push(Act::Select(level, v));
                    }
                }
                if self.sel.len() > 60 {
                    ui.label(format!("… and {} more", self.sel.len() - 60));
                }
            });
        });
    }

    fn actions_ui(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        let level = self.settings.level;
        egui::CollapsingHeader::new("Actions").id_salt("actions").default_open(true).show(ui, |ui| {
            let has = !self.sel.is_empty();
            let two = self.sel.len() >= 2;
            let mut btn = |ui: &mut egui::Ui, enabled: bool, text: &str, tip: &str, act: Act| {
                if ui.add_enabled(enabled, egui::Button::new(text)).on_hover_text(tip).clicked() {
                    acts.push(act);
                }
            };
            let divide_row = |ui: &mut egui::Ui, n: &mut usize, label: &str| {
                ui.add(egui::DragValue::new(n).range(2..=50).prefix(format!("{label} into ")));
            };
            match level {
                Level::Provinces => {
                    ui.horizontal_wrapped(|ui| {
                        btn(ui, two, "Merge", "merge the selected provinces into the first one", Act::MergeProvinces);
                        divide_row(ui, &mut self.divide_n, "");
                        btn(ui, has, "Divide", "cut each selected province into that many compact pieces", Act::DivideProvinces);
                        btn(ui, has, "Generate by terrain", "re-cut the selection: small provinces in towns and valleys, large ones in the mountains (parameters below)", Act::GenerateProvinces);
                        btn(ui, has, "Dissolve (Del)", "remove: the pixels go to neighbouring provinces of the same type", Act::DissolveProvinces);
                        btn(ui, true, "New (paint)", "paint a new province", Act::NewProvince);
                    });
                    ui.horizontal_wrapped(|ui| {
                        btn(ui, has, "Make land", "turn into land (joins a neighbouring state)", Act::SetKind(Kind::Land));
                        btn(ui, has, "Make sea", "turn into sea (joins a neighbouring sea zone)", Act::SetKind(Kind::Sea));
                        btn(ui, has, "Split separate parts", "give each separate piece of a province its own province", Act::SplitParts(false));
                    });
                    ui.horizontal_wrapped(|ui| {
                        btn(ui, has, "New state", "make a new state of the selected land provinces", Act::NewStateFromSel);
                        btn(ui, has, "New air zone", "a new air (land) zone of the selected provinces' states", Act::NewZoneFromSel(false));
                        btn(ui, has, "New sea zone", "a new sea zone of the selected sea provinces", Act::NewZoneFromSel(true));
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Select all of their:");
                        btn(ui, has, "states", "", Act::GrowTo(Level::States));
                        btn(ui, has, "nations", "", Act::GrowTo(Level::Nations));
                        btn(ui, has, "air zones", "", Act::GrowTo(Level::AirZones));
                    });
                }
                Level::States => {
                    ui.horizontal_wrapped(|ui| {
                        btn(ui, two, "Merge", "merge into the first selected state", Act::MergeStates);
                        divide_row(ui, &mut self.divide_n, "");
                        btn(ui, has, "Divide", "cut each selected state into that many states", Act::DivideStates);
                        btn(ui, has, "Dissolve (Del)", "hand the provinces to neighbouring states (same nation first)", Act::DissolveStates);
                    });
                    ui.horizontal_wrapped(|ui| {
                        btn(ui, has, "Generate states", "regroup the provinces of the selected states' nations into states (size parameters below)", Act::GenerateStates);
                        btn(ui, has, "New nation", "make the selected states a new nation", Act::NewNationFromSel);
                        btn(ui, has, "Impassable", "units cannot enter (like the Sahara)", Act::SetImpassable(true));
                        btn(ui, has, "Passable", "", Act::SetImpassable(false));
                    });
                    ui.label(RichText::new("Paint tool: provinces the brush touches join the first selected state.").small().weak());
                }
                Level::Nations => {
                    ui.horizontal_wrapped(|ui| {
                        btn(ui, two, "Merge", "merge into the first selected nation", Act::MergeNations);
                        divide_row(ui, &mut self.divide_n, "");
                        btn(ui, has, "Divide", "cut each selected nation into that many nations (by states)", Act::DivideNations);
                        btn(ui, has, "Dissolve (Del)", "hand the states to neighbouring nations", Act::DissolveNations);
                    });
                    ui.horizontal_wrapped(|ui| {
                        btn(ui, two, "Share out land", "give every state of the selected nations to the nearest of their capitals", Act::GenerateNations);
                        btn(ui, has, "Generate states", "regroup the selected nations' provinces into states", Act::GenerateStates);
                    });
                    ui.label(RichText::new("Paint tool: states the brush touches join the first selected nation. To move single provinces, paint them into a state of the other nation.").small().weak());
                }
                Level::AirZones | Level::SeaZones => {
                    ui.horizontal_wrapped(|ui| {
                        btn(ui, two, "Merge", "merge into the first selected zone", Act::MergeRegions);
                        divide_row(ui, &mut self.divide_n, "");
                        btn(ui, has, "Divide", "cut each selected zone into that many", Act::DivideRegions);
                        btn(ui, has, "Dissolve (Del)", "hand the members to neighbouring zones", Act::DissolveRegions);
                        let tip = if level == Level::SeaZones {
                            "regroup the selected sea zones into the number above"
                        } else {
                            "regroup the selected air zones into zones of the size below (states stay whole)"
                        };
                        btn(ui, has, "Generate", tip, Act::GenerateRegions);
                    });
                    ui.label(
                        RichText::new(if level == Level::AirZones {
                            "Air zones are the land strategic regions; they hold whole states. Paint tool: states the brush touches join the first selected zone."
                        } else {
                            "Sea zones are the naval strategic regions. Paint tool: sea provinces the brush touches join the first selected zone."
                        })
                        .small()
                        .weak(),
                    );
                }
                Level::Adjacencies => self.adjacency_list_ui(ui, acts),
                Level::Inaccessible => {
                    ui.label(RichText::new(
                        "Select provinces (click, or drag the brush). Then:\n\
                         • Impassable: they become impassable states of their nation, as vanilla does with the Sahara and the Himalayas. Units cannot enter; the owner's colour stays.\n\
                         • Remove from play: they become HOI4 \"lake\" provinces: no owner, no state, no units, no supply. They keep their land look and air zone. This was not tested in the game yet; check error.log after a build.\n\
                         Merging them into large provinces leaves fewer provinces for the game to handle.",
                    ).small());
                    ui.checkbox(&mut self.settings.coarsen, "merge into as few provinces as possible");
                    ui.horizontal_wrapped(|ui| {
                        btn(ui, has, "Make impassable", "", Act::MakeImpassable);
                        btn(ui, has, "Remove from play", "", Act::RemoveFromPlay);
                        btn(ui, has, "Restore access", "removed provinces join a neighbouring state; impassable states become passable", Act::RestoreAccess);
                    });
                    if let Some(doc) = &self.doc {
                        let imp = doc.data.states.iter().filter(|(_, s)| s.impassable).count();
                        let lakes = doc.data.provinces.iter().filter(|(_, p)| p.kind == Kind::Lake).count();
                        ui.label(format!("{imp} impassable states, {lakes} removed provinces"));
                    }
                }
            }
        });
    }

    fn adjacency_list_ui(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        ui.horizontal(|ui| {
            ui.label("New:");
            egui::ComboBox::from_id_salt("adjkind").selected_text(if self.adj_kind.is_empty() { "land" } else { &self.adj_kind }).show_ui(ui, |ui| {
                for (k, tip) in model::ADJ_KINDS {
                    ui.selectable_value(&mut self.adj_kind, k.to_string(), if k.is_empty() { "land" } else { k }).on_hover_text(tip);
                }
            });
            ui.add(egui::TextEdit::singleline(&mut self.adj_rule).desired_width(90.0).hint_text("rule (opt.)"))
                .on_hover_text("adjacency_rule_name: a rule defined in adjacency_rules.txt next to provinces.json (copied into the mod by Build)");
        });
        ui.label(RichText::new("Link tool (L): click the two provinces. A strait needs one sea province touching both.").small().weak());
        let Some(doc) = &self.doc else { return };
        egui::ScrollArea::vertical().id_salt("adjlist").max_height(220.0).show(ui, |ui| {
            egui::Grid::new("adjgrid").striped(true).num_columns(3).show(ui, |ui| {
                for (i, a) in doc.data.adjacencies.iter().enumerate() {
                    let kind = if a.kind.is_empty() { "land" } else { a.kind.as_str() };
                    let name = if a.name.is_empty() { format!("{} – {}", doc.data.prov_name(a.from), doc.data.prov_name(a.to)) } else { a.name.clone() };
                    if ui.selectable_label(self.sel_adj == Some(i), name).clicked() {
                        self.sel_adj = Some(i);
                        acts.push(Act::ZoomSelection);
                    }
                    ui.label(kind);
                    if ui.small_button("🗑").on_hover_text("delete").clicked() {
                        acts.push(Act::DeleteAdjacency(i));
                    }
                    ui.end_row();
                }
            });
        });
    }

    fn sync_draft(&mut self) {
        let Some(doc) = &self.doc else { return };
        let d = &doc.data;
        let level = self.settings.level;
        let current = match level {
            Level::Adjacencies => self.sel_adj.and_then(|i| d.adjacencies.get(i).map(|a| Draft::Adjacency(i, a.clone()))),
            _ => self.sel.first().and_then(|&id| match level {
                Level::Provinces | Level::Inaccessible => d.provinces.get(id).map(|p| Draft::Province(id, p.clone())),
                Level::States => d.states.get(id).map(|s| Draft::State(id, s.clone())),
                Level::Nations => d.nations.get(id).map(|n| Draft::Nation(id, n.clone())),
                Level::AirZones | Level::SeaZones => d.regions.get(id).map(|r| Draft::Region(id, r.clone())),
                Level::Adjacencies => None,
            }),
        };
        let same_entity = |a: &Draft, b: &Draft| match (a, b) {
            (Draft::Province(x, _), Draft::Province(y, _))
            | (Draft::State(x, _), Draft::State(y, _))
            | (Draft::Nation(x, _), Draft::Nation(y, _))
            | (Draft::Region(x, _), Draft::Region(y, _)) => x == y,
            (Draft::Adjacency(x, _), Draft::Adjacency(y, _)) => x == y,
            _ => false,
        };
        let modified = self.draft.is_some() && self.draft != self.draft_orig;
        let keep = match (&self.draft, &current) {
            (Some(dr), Some(cur)) => same_entity(dr, cur) && (modified || self.draft_meta == doc.meta_version),
            _ => false,
        };
        if !keep {
            self.draft = current.clone();
            self.draft_orig = current;
            self.draft_meta = doc.meta_version;
            self.vp_new = (0, 1);
        }
    }

    fn properties_ui(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        self.sync_draft();
        let Some(mut draft) = self.draft.take() else { return };
        let doc = self.doc.as_ref().unwrap();
        let sealed = self.sealed.as_ref().filter(|s| s.0 == doc.pixel_version).map(|s| s.1.clone());
        let title = match &draft {
            Draft::Province(..) => "Province",
            Draft::State(..) => "State",
            Draft::Nation(..) => "Nation",
            Draft::Region(_, r) if r.naval => "Sea zone",
            Draft::Region(..) => "Air zone",
            Draft::Adjacency(..) => "Adjacency",
        };
        let mut want_sealed = false;
        egui::CollapsingHeader::new(format!("Properties: {title}")).id_salt("props").default_open(true).show(ui, |ui| {
            match &mut draft {
                Draft::Province(id, p) => province_props(ui, *id, p, doc, acts),
                Draft::State(id, s) => {
                    want_sealed = sealed.is_none();
                    state_props(ui, *id, s, doc, &mut self.nation_filter, &mut self.vp_new, sealed.as_deref(), acts)
                }
                Draft::Nation(id, n) => nation_props(ui, *id, n, doc, acts),
                Draft::Region(id, r) => region_props(ui, *id, r, doc),
                Draft::Adjacency(i, a) => adjacency_props(ui, *i, a, doc, acts),
            }
            let modified = Some(&draft) != self.draft_orig.as_ref();
            ui.horizontal(|ui| {
                if ui.add_enabled(modified, egui::Button::new("Apply")).clicked() {
                    acts.push(Act::ApplyDraft);
                }
                if ui.add_enabled(modified, egui::Button::new("Revert")).clicked() {
                    acts.push(Act::RevertDraft);
                }
                if modified {
                    ui.colored_label(WARN, "not applied yet");
                }
            });
        });
        self.draft = Some(draft);
        if want_sealed {
            if let Some(doc) = &self.doc {
                if let Some(s) = doc.sealed_km2() {
                    self.sealed = Some((doc.pixel_version, s));
                }
            }
        }
    }

    fn generation_ui(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Generation parameters").id_salt("gen").default_open(false).show(ui, |ui| {
            let g = &mut self.settings.params;
            ui.label(RichText::new("Province sizes by terrain, as in make_provinces.py. \"detail\" scales the number of provinces of a generate run; use it per area (e.g. 1.5 in a valley, 0.5 in the mountains).").small().weak());
            egui::Grid::new("gengrid").num_columns(2).show(ui, |ui| {
                let row = |ui: &mut egui::Ui, label: &str, v: &mut f32, range: std::ops::RangeInclusive<f32>, suffix: &str| {
                    ui.label(label);
                    ui.add(egui::DragValue::new(v).range(range).speed(0.05).suffix(suffix));
                    ui.end_row();
                };
                row(ui, "detail", &mut g.density.detail, 0.05..=20.0, " ×");
                row(ui, "built-up areas", &mut g.density.urban_km2, 0.05..=100.0, " km²");
                row(ui, "valley floors", &mut g.density.valley_km2, 0.05..=200.0, " km²");
                row(ui, "slopes < 2000 m", &mut g.density.slope_km2, 0.1..=500.0, " km²");
                row(ui, "mountains ≥ 2000 m", &mut g.density.mountain_km2, 0.1..=2000.0, " km²");
                row(ui, "sea", &mut g.density.sea_km2, 0.05..=500.0, " km²");
                row(ui, "largest province", &mut g.max_km2, 1.0..=5000.0, " km²");
                row(ui, "smallest piece", &mut g.min_km2, 0.01..=50.0, " km²");
                row(ui, "box limit (of 1/8 map)", &mut g.box_frac, 0.1..=1.0, "");
                row(ui, "provinces per state", &mut g.state_provinces, 1.0..=200.0, "");
                row(ui, "largest state", &mut g.state_max_km2, 1.0..=50_000.0, " km²");
                row(ui, "air zone size", &mut g.region_km2, 10.0..=100_000.0, " km²");
            });
            if ui.button("Reset to defaults").clicked() {
                *g = GenParams::default();
            }
        });
    }

    fn hover_ui(&self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Under the cursor").id_salt("hover").default_open(true).show(ui, |ui| {
            let (Some(doc), Some(h)) = (&self.doc, self.hover) else {
                ui.label(RichText::new("Hover over the map.").weak());
                return;
            };
            let Some(p) = self.province_at(h) else {
                ui.label(RichText::new("outside the map").weak());
                return;
            };
            let d = &doc.data;
            let pr = d.provinces.get(p);
            let s = doc.stats.get(p);
            egui::Grid::new("hovergrid").num_columns(2).show(ui, |ui| {
                ui.label("pixel");
                ui.monospace(format!("{} , {}", h[0] as u32, h[1] as u32));
                ui.end_row();
                ui.label("province");
                ui.label(format!("{} ({p}) · {} · {:.2} km²", d.prov_name(p), pr.map(|x| x.kind.name()).unwrap_or("?"), s.px as f64 * d.km2_per_px()));
                ui.end_row();
                let st = d.state_of(p);
                if st != 0 {
                    ui.label("state");
                    ui.label(format!("{} ({st}){}", d.state_name(st), if d.impassable(p) { " · impassable" } else { "" }));
                    ui.end_row();
                    ui.label("nation");
                    ui.label(format!("{} ({})", d.nation_name(d.nation_of(p)), d.nation_of(p)));
                    ui.end_row();
                }
                ui.label("zone");
                ui.label(format!("{} ({})", d.region_name(d.region_of(p)), d.region_of(p)));
                ui.end_row();
                if let Some((_, t)) = &self.targets {
                    if let Some(e) = t.get(p as usize) {
                        ui.label("terrain wants");
                        ui.label(format!("{e:.2} provinces here"));
                        ui.end_row();
                    }
                }
            });
        });
    }

    fn validation_ui(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        let errors = self.issues.iter().filter(|i| i.severity == Severity::Error).count();
        let warnings = self.issues.len() - errors;
        let stale = self.doc.as_ref().is_some_and(|d| version(d) != self.issues_for);
        let title = format!("Checks: {errors} errors, {warnings} warnings{}", if stale { " (outdated)" } else { "" });
        let title = if errors > 0 { RichText::new(title).color(ERR) } else { RichText::new(title) };
        egui::CollapsingHeader::new(title).id_salt("checks").default_open(true).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui.button("Check now").clicked() {
                    acts.push(Act::Check);
                }
                if self.issues_rx.is_some() {
                    ui.spinner();
                }
                ui.checkbox(&mut self.settings.auto_check, "after each edit");
                if ui.button("Fix crossings").on_hover_text("remove points where four provinces meet / diagonal contacts").clicked() {
                    acts.push(Act::FixCrossings);
                }
                if ui.button("Split separate parts").on_hover_text("every separate piece of any province becomes its own province").clicked() {
                    acts.push(Act::SplitParts(true));
                }
            });
            egui::ScrollArea::vertical().id_salt("issues").max_height(200.0).show(ui, |ui| {
                for is in self.issues.iter().take(300) {
                    let c = if is.severity == Severity::Error { ERR } else { WARN };
                    let r = ui.add(egui::Label::new(RichText::new(&is.msg).color(c).small()).sense(egui::Sense::click()));
                    if r.clicked() {
                        if let Some(p) = is.prov {
                            acts.push(Act::Select(Level::Provinces, vec![p]));
                            acts.push(Act::ZoomProvince(p));
                        } else if let Some(at) = is.at {
                            self.zoom_to = Some([at[0] as f64 - 20.0, at[1] as f64 - 20.0, at[0] as f64 + 20.0, at[1] as f64 + 20.0]);
                        }
                    }
                }
                if self.issues.len() > 300 {
                    ui.label(format!("… {} more", self.issues.len() - 300));
                }
            });
        });
    }

    /// Build log and the unsaved-changes dialog.
    pub fn windows(&mut self, ctx: &egui::Context) {
        self.meta_window(ctx);
        if self.show_build {
            let mut open = true;
            let dirs = self.dirs();
            let build = self.build.as_ref().map(|b| (b.state(), b.log.lock().clone(), b.mod_dir.lock().clone(), b.id));
            let launcher = &mut self.launcher;
            egui::Window::new("Build mod and play").open(&mut open).default_size([780.0, 720.0]).show(ctx, |ui| {
                let (state, built) = match &build {
                    None => {
                        ui.label(RichText::new("No build yet: 🔨 Build mod makes the mod from the saved map.").weak());
                        (None, None)
                    }
                    Some((state, log, mod_dir, id)) => {
                        let (status, colour) = match *state {
                            build::RUNNING => ("building…", WARN),
                            build::OK => ("built, all checks passed", Color32::from_rgb(120, 220, 120)),
                            _ => ("build failed", ERR),
                        };
                        egui::CollapsingHeader::new(RichText::new(status).color(colour)).id_salt("build_log").default_open(true).show(ui, |ui| {
                            egui::ScrollArea::both().id_salt("build_log_text").stick_to_bottom(true).max_height(200.0).auto_shrink([false, true]).show(ui, |ui| {
                                ui.add(egui::Label::new(RichText::new(log).monospace().small()).wrap_mode(egui::TextWrapMode::Extend));
                            });
                        });
                        (Some(*state), mod_dir.clone().filter(|_| *state == build::OK).map(|d| (*id, d)))
                    }
                };
                ui.separator();
                ui.heading("Mods");
                launcher.ui(ui, &dirs, state == Some(build::RUNNING), state == Some(build::FAILED), built.as_ref().map(|(id, d)| (*id, d.as_path())));
            });
            self.show_build = open;
        }
        if self.ask_close {
            let mut choice = None;
            egui::Window::new("Unsaved map changes").collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
                ui.label("The map has changes that are not saved.");
                ui.horizontal(|ui| {
                    if ui.button("Save and quit").clicked() {
                        choice = Some(0);
                    }
                    if ui.button("Quit without saving").clicked() {
                        choice = Some(1);
                    }
                    if ui.button("Cancel").clicked() {
                        choice = Some(2);
                    }
                });
            });
            match choice {
                Some(0) => {
                    self.save();
                    self.ask_close = false;
                    if !self.is_dirty() {
                        self.close_ok = true;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
                Some(1) => {
                    self.ask_close = false;
                    self.close_ok = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                Some(_) => self.ask_close = false,
                None => {}
            }
        }
    }
}

fn dist_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = if ab.length_sq() > 0.0 { ((p - a).dot(ab) / ab.length_sq()).clamp(0.0, 1.0) } else { 0.0 };
    (a + ab * t - p).length()
}

// ---------------------------------------------------------------- properties

const LAND_TERRAIN: [&str; 8] = ["plains", "forest", "hills", "mountain", "marsh", "urban", "jungle", "desert"];
const CATEGORIES: [&str; 13] = [
    "wasteland",
    "enclave",
    "tiny_island",
    "small_island",
    "pastoral",
    "rural",
    "town",
    "large_town",
    "city",
    "large_city",
    "metropolis",
    "megalopolis",
    "large_island",
];
const BUILDINGS: [(&str, &str); 11] = [
    ("infrastructure", "infrastructure (0-5)"),
    ("industrial_complex", "civilian factories"),
    ("arms_factory", "military factories"),
    ("dockyard", "dockyards"),
    ("air_base", "air base (0-10)"),
    ("anti_air_building", "anti-air (0-5)"),
    ("synthetic_refinery", "synthetic refinery"),
    ("fuel_silo", "fuel silo"),
    ("radar_station", "radar (0-6)"),
    ("rocket_site", "rocket site"),
    ("nuclear_reactor", "nuclear reactor"),
];
const RESOURCES: [&str; 6] = ["oil", "aluminium", "rubber", "tungsten", "steel", "chromium"];

fn province_props(ui: &mut egui::Ui, id: Id, p: &mut Province, doc: &Doc, acts: &mut Vec<Act>) {
    let d = &doc.data;
    let s = doc.stats.get(id);
    egui::Grid::new("provprops").num_columns(2).show(ui, |ui| {
        ui.label("name");
        ui.text_edit_singleline(&mut p.name);
        ui.end_row();
        ui.label("id / colour");
        ui.horizontal(|ui| {
            ui.monospace(format!("{id}"));
            let (r, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
            ui.painter().rect_filled(r, 2.0, Color32::from_rgb(p.rgb[0], p.rgb[1], p.rgb[2]));
        });
        ui.end_row();
        ui.label("type");
        ui.label(format!("{}{}", p.kind.name(), if p.edge { " · map edge" } else { "" }));
        ui.end_row();
        ui.label("size");
        ui.label(format!("{:.2} km² · {} px · box {} × {}", s.px as f64 * d.km2_per_px(), s.px, s.box_w(), s.box_h()));
        ui.end_row();
        ui.label("terrain");
        let opts: Vec<&str> = match p.kind {
            Kind::Land => LAND_TERRAIN.to_vec(),
            Kind::Sea => vec!["ocean"],
            Kind::Lake => vec!["lakes"],
        };
        egui::ComboBox::from_id_salt("terrain").selected_text(p.terrain.clone().unwrap_or_else(|| "auto (from the land cover)".into())).show_ui(ui, |ui| {
            ui.selectable_value(&mut p.terrain, None, "auto (from the land cover)");
            for t in opts {
                ui.selectable_value(&mut p.terrain, Some(t.to_string()), t);
            }
        });
        ui.end_row();
        if p.state != 0 {
            ui.label("state");
            if ui.link(format!("{} ({})", d.state_name(p.state), p.state)).clicked() {
                acts.push(Act::Select(Level::States, vec![p.state]));
            }
            ui.end_row();
            let n = d.nation_of(id);
            ui.label("nation");
            if ui.link(format!("{} ({n})", d.nation_name(n))).clicked() {
                acts.push(Act::Select(Level::Nations, vec![n]));
            }
            ui.end_row();
        }
        ui.label("zone");
        let naval = d.regions.get(p.region).is_some_and(|r| r.naval);
        if ui.link(format!("{} ({})", d.region_name(p.region), p.region)).clicked() {
            acts.push(Act::Select(if naval { Level::SeaZones } else { Level::AirZones }, vec![p.region]));
        }
        ui.end_row();
    });
}

#[allow(clippy::too_many_arguments)]
fn state_props(ui: &mut egui::Ui, id: Id, s: &mut State, doc: &Doc, filter: &mut String, vp_new: &mut (Id, u32), sealed: Option<&[f64]>, acts: &mut Vec<Act>) {
    let d = &doc.data;
    let provs = doc.index.provs_of_state(id);
    let px: u64 = provs.iter().map(|&p| doc.stats.get(p).px as u64).sum();
    egui::Grid::new("stateprops").num_columns(2).show(ui, |ui| {
        ui.label("name");
        ui.text_edit_singleline(&mut s.name);
        ui.end_row();
        ui.label("owner");
        ui.vertical(|ui| {
            let cur = s.nation.map(|n| format!("{} ({n})", d.nation_name(n))).unwrap_or_else(|| "—".into());
            egui::ComboBox::from_id_salt("owner").selected_text(cur).height(320.0).show_ui(ui, |ui| {
                ui.add(egui::TextEdit::singleline(filter).hint_text("filter…"));
                let f = filter.to_lowercase();
                let mut list: Vec<(Id, &str)> = d.nations.iter().map(|(n, x)| (n, x.name.as_str())).filter(|(_, nm)| f.is_empty() || nm.to_lowercase().contains(&f)).collect();
                list.sort_by_key(|x| x.1.to_lowercase());
                for (n, nm) in list.into_iter().take(300) {
                    ui.selectable_value(&mut s.nation, Some(n), nm);
                }
            });
        });
        ui.end_row();
        ui.label("impassable");
        ui.checkbox(&mut s.impassable, "units cannot enter");
        ui.end_row();
        ui.label("size");
        let mut info = format!("{} provinces · {:.1} km²", provs.len(), px as f64 * d.km2_per_px());
        if let Some(sk) = sealed {
            let u: f64 = provs.iter().map(|&p| sk.get(p as usize).copied().unwrap_or(0.0)).sum();
            info.push_str(&format!(" · built-up {u:.2} km²"));
        }
        ui.label(info);
        ui.end_row();
        ui.label("population");
        ui.horizontal(|ui| {
            let mut auto = s.manpower.is_none();
            if ui.checkbox(&mut auto, "auto").on_hover_text("the mod build derives it from the built-up area").changed() {
                s.manpower = if auto { None } else { Some(100_000) };
            }
            if let Some(m) = &mut s.manpower {
                ui.add(egui::DragValue::new(m).range(0..=100_000_000u64).speed(500.0));
            }
        });
        ui.end_row();
        ui.label("category");
        egui::ComboBox::from_id_salt("category").selected_text(s.category.clone().unwrap_or_else(|| "auto".into())).show_ui(ui, |ui| {
            ui.selectable_value(&mut s.category, None, "auto (by built-up area)");
            for c in CATEGORIES {
                ui.selectable_value(&mut s.category, Some(c.to_string()), c);
            }
        });
        ui.end_row();
    });
    ui.label(RichText::new("Buildings (unticked = the mod build decides)").strong());
    egui::Grid::new("buildings").num_columns(2).show(ui, |ui| {
        for (k, label) in BUILDINGS {
            let mut on = s.buildings.contains_key(k);
            if ui.checkbox(&mut on, label).changed() {
                if on {
                    s.buildings.insert(k.to_string(), 0);
                } else {
                    s.buildings.remove(k);
                }
            }
            if let Some(v) = s.buildings.get_mut(k) {
                ui.add(egui::DragValue::new(v).range(0..=50));
            } else {
                ui.label("");
            }
            ui.end_row();
        }
    });
    ui.label(RichText::new("Resources").strong());
    egui::Grid::new("resources").num_columns(2).show(ui, |ui| {
        for k in RESOURCES {
            let mut v = s.resources.get(k).copied().unwrap_or(0);
            ui.label(k);
            if ui.add(egui::DragValue::new(&mut v).range(0..=1000)).changed() {
                if v == 0 {
                    s.resources.remove(k);
                } else {
                    s.resources.insert(k.to_string(), v);
                }
            }
            ui.end_row();
        }
    });
    ui.label(RichText::new("Victory points (none = automatic: the most built-up province)").strong());
    let mut remove = None;
    for (i, vp) in s.victory_points.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            if ui.link(d.prov_name(vp[0])).clicked() {
                acts.push(Act::ZoomProvince(vp[0]));
            }
            ui.add(egui::DragValue::new(&mut vp[1]).range(1..=100));
            if ui.small_button("🗑").clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        s.victory_points.remove(i);
    }
    ui.horizontal(|ui| {
        let name = if vp_new.0 == 0 { "province…".to_string() } else { d.prov_name(vp_new.0) };
        egui::ComboBox::from_id_salt("vpprov").selected_text(name).show_ui(ui, |ui| {
            for &p in provs {
                ui.selectable_value(&mut vp_new.0, p, format!("{} ({p})", d.prov_name(p)));
            }
        });
        ui.add(egui::DragValue::new(&mut vp_new.1).range(1..=100));
        if ui.add_enabled(vp_new.0 != 0, egui::Button::new("Add VP")).clicked() {
            s.victory_points.retain(|v| v[0] != vp_new.0);
            s.victory_points.push([vp_new.0, vp_new.1]);
        }
    });
}

fn nation_props(ui: &mut egui::Ui, id: Id, n: &mut Nation, doc: &Doc, acts: &mut Vec<Act>) {
    let d = &doc.data;
    let states = doc.index.states_of_nation(id);
    egui::Grid::new("nationprops").num_columns(2).show(ui, |ui| {
        ui.label("name");
        ui.text_edit_singleline(&mut n.name);
        ui.end_row();
        ui.label("tag");
        ui.horizontal(|ui| {
            let mut auto = n.tag.is_none();
            if ui.checkbox(&mut auto, "auto").on_hover_text("the mod build makes one from the name").changed() {
                n.tag = if auto { None } else { Some(String::new()) };
            }
            if let Some(t) = &mut n.tag {
                if ui.add(egui::TextEdit::singleline(t).desired_width(40.0).char_limit(3)).changed() {
                    *t = t.to_ascii_uppercase();
                }
            }
        });
        ui.end_row();
        ui.label("colour");
        ui.horizontal(|ui| {
            let mut auto = n.color.is_none();
            if ui.checkbox(&mut auto, "auto").changed() {
                n.color = if auto { None } else { Some(render::id_colour(id, 2)) };
            }
            if let Some(c) = &mut n.color {
                ui.color_edit_button_srgb(c);
            }
        });
        ui.end_row();
        ui.label("language");
        egui::ComboBox::from_id_salt("lang").selected_text(n.language.clone()).show_ui(ui, |ui| {
            for (l, t) in [("de", "German (Austrian names)"), ("it", "Italian"), ("rm", "Romansh (Swiss names)")] {
                ui.selectable_value(&mut n.language, l.to_string(), t);
            }
        });
        ui.end_row();
        ui.label("ideology");
        egui::ComboBox::from_id_salt("ideology").selected_text(n.ideology.clone().unwrap_or_else(|| "neutrality (default)".into())).show_ui(ui, |ui| {
            ui.selectable_value(&mut n.ideology, None, "neutrality (default)");
            for i in ["democratic", "fascism", "communism", "neutrality"] {
                ui.selectable_value(&mut n.ideology, Some(i.to_string()), i);
            }
        });
        ui.end_row();
        ui.label("settlement");
        egui::ComboBox::from_id_salt("place").selected_text(n.place.clone()).show_ui(ui, |ui| {
            for p in ["city", "town", "village"] {
                ui.selectable_value(&mut n.place, p.to_string(), p);
            }
        });
        ui.end_row();
        ui.label("capital");
        egui::ComboBox::from_id_salt("capital").selected_text(d.state_name(n.capital_state)).show_ui(ui, |ui| {
            for &s in states {
                if !d.states.get(s).is_some_and(|x| x.impassable) {
                    ui.selectable_value(&mut n.capital_state, s, d.state_name(s));
                }
            }
        });
        ui.end_row();
        ui.label("army");
        let mut units = !n.no_units;
        if ui.checkbox(&mut units, "starts with a militia").changed() {
            n.no_units = !units;
        }
        ui.end_row();
        ui.label("states");
        ui.vertical(|ui| {
            for &s in states.iter().take(12) {
                if ui.link(d.state_name(s)).clicked() {
                    acts.push(Act::Select(Level::States, vec![s]));
                }
            }
        });
        ui.end_row();
    });
}

fn region_props(ui: &mut egui::Ui, id: Id, r: &mut Region, doc: &Doc) {
    let provs = doc.index.provs_of_region(id);
    let px: u64 = provs.iter().map(|&p| doc.stats.get(p).px as u64).sum();
    egui::Grid::new("regionprops").num_columns(2).show(ui, |ui| {
        ui.label("name");
        ui.text_edit_singleline(&mut r.name);
        ui.end_row();
        ui.label("size");
        ui.label(format!("{} provinces · {:.0} km²", provs.len(), px as f64 * doc.data.km2_per_px()));
        ui.end_row();
    });
}

fn adjacency_props(ui: &mut egui::Ui, i: usize, a: &mut Adjacency, doc: &Doc, acts: &mut Vec<Act>) {
    let d = &doc.data;
    egui::Grid::new("adjprops").num_columns(2).show(ui, |ui| {
        ui.label("name");
        ui.text_edit_singleline(&mut a.name);
        ui.end_row();
        ui.label("type");
        egui::ComboBox::from_id_salt("adjkind2").selected_text(if a.kind.is_empty() { "land" } else { &a.kind }).show_ui(ui, |ui| {
            for (k, tip) in model::ADJ_KINDS {
                ui.selectable_value(&mut a.kind, k.to_string(), if k.is_empty() { "land" } else { k }).on_hover_text(tip);
            }
        });
        ui.end_row();
        for (label, p) in [("from", a.from), ("to", a.to)] {
            ui.label(label);
            if ui.link(format!("{} ({p})", d.prov_name(p))).clicked() {
                acts.push(Act::ZoomProvince(p));
            }
            ui.end_row();
        }
        ui.label("through");
        ui.horizontal(|ui| {
            let mut t = a.through.unwrap_or(0);
            if ui.add(egui::DragValue::new(&mut t)).on_hover_text("sea province the strait crosses (0 = none)").changed() {
                a.through = (t != 0).then_some(t);
            }
            if let Some(t) = a.through {
                ui.label(d.prov_name(t));
            }
        });
        ui.end_row();
        ui.label("rule");
        ui.text_edit_singleline(&mut a.rule);
        ui.end_row();
        ui.label("ends");
        ui.horizontal(|ui| {
            if ui.button("swap from / to").clicked() {
                std::mem::swap(&mut a.from, &mut a.to);
                std::mem::swap(&mut a.start, &mut a.stop);
            }
            if ui.button("recompute points").on_hover_text("put the crossing points where the two provinces come closest").clicked() {
                acts.push(Act::AdjacencyPoints(i));
            }
        });
        ui.end_row();
    });
}
