//! The nation content screen: the nations of the map down the left, the
//! content of the one picked (its focus trees, events, decisions, ideas,
//! characters, ...) and the project's tools: checks, imports, exports, test
//! play with the game's error log, find & replace, renames, galleries.
//!
//! Content is kept in memory ([`Store`]) with its own undo; Ctrl+S writes
//! it (and the map, when it has changes) to `<map>/content/`.

pub mod check;
pub mod code;
pub mod division;
pub mod form;
pub mod import_ui;
pub mod picker;
pub mod statemap;
pub mod tools;
pub mod tree;
pub mod wizard;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;
use eframe::egui::{self, Color32, RichText};
use serde::{Deserialize, Serialize};

use super::gfx::Gfx;
use super::import;
use super::kinds::{self, Kind};
use super::loc;
use super::script::{self, Edit, Script};
use super::source::{self, Index, Origin};
use super::store::{DefKey, SHARED, Store};
use crate::editor::{Editor, MapFacts, NationInfo};

pub const WARN: Color32 = Color32::from_rgb(255, 190, 90);
pub const ERR: Color32 = Color32::from_rgb(255, 110, 110);
pub const OK: Color32 = Color32::from_rgb(120, 220, 120);

/// What the content screen keeps in the project file.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The screen was open.
    pub active: bool,
    /// The nation (tag) or `shared` last shown.
    pub pkg: Option<String>,
    /// The language texts are edited in.
    pub language: String,
    /// The DLCs the focus tree view assumes (None: all of them).
    pub dlcs: Option<BTreeSet<String>>,
    /// Folders and bundles imported from before.
    pub sources: Vec<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { active: false, pkg: None, language: "english".into(), dlcs: None, sources: Vec::new() }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Overview,
    Trees,
    Kind(Kind),
    Localisation,
    Files,
    Check,
    Import,
    Export,
    Play,
    Tool(tools::Tool),
}

/// Draw `f` in the space left in `ui`, clipped to it: what it draws never
/// makes `ui` bigger (a side panel would otherwise grow a little with every
/// frame its contents overflow by a pixel).
pub fn contained<R>(ui: &mut egui::Ui, f: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let rect = ui.available_rect_before_wrap();
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::top_down(egui::Align::Min)));
    child.shrink_clip_rect(rect);
    let r = f(&mut child);
    ui.allocate_rect(rect, egui::Sense::hover());
    r
}

/// Where to go next (asked by a page).
#[derive(Clone, Debug, PartialEq)]
pub enum Nav {
    Page(Page),
    Pkg(String),
    Def(DefKey),
    File { pkg: String, file: String, line: Option<usize> },
    /// Ask to delete all content of a package.
    DeletePkg(String),
}

pub type Msg = Option<(String, bool, Instant)>;

/// The localisation of the project in one language: key → (package, file, text).
#[derive(Default)]
pub struct LocCache {
    key: (u64, String),
    pub map: HashMap<String, (String, String, String)>,
    /// Base game texts in this language (English is in the index).
    base: Option<(String, Arc<HashMap<String, String>>)>,
}

impl LocCache {
    fn refresh(&mut self, st: &Store, lang: &str) {
        if self.key == (st.generation, lang.to_string()) {
            return;
        }
        self.key = (st.generation, lang.to_string());
        self.map.clear();
        let marker = format!("l_{lang}");
        for (pkg, p) in &st.packages {
            for (path, f) in &p.files {
                if let Some(l) = &f.loc
                    && (l.language.as_deref() == Some(lang) || (l.language.is_none() && path.contains(&marker)))
                {
                    for e in &l.entries {
                        self.map.entry(e.key.clone()).or_insert_with(|| (pkg.clone(), path.clone(), e.text.clone()));
                    }
                }
            }
        }
    }
    /// A key's text: the project's, else the base game's.
    pub fn get<'a>(&'a self, key: &str, base: Option<&'a Index>) -> Option<&'a str> {
        if let Some((_, _, t)) = self.map.get(key) {
            return Some(t);
        }
        match &self.base {
            Some((_, m)) => m.get(key).map(String::as_str),
            None => base.and_then(|b| b.loc.get(key)).map(String::as_str),
        }
    }
    /// The project has the key.
    pub fn own(&self, key: &str) -> Option<&(String, String, String)> {
        self.map.get(key)
    }
}

/// What pages get: the content, the sources, the map's nations.
pub struct Cx<'a> {
    pub st: &'a mut Store,
    pub base: Option<&'a Index>,
    pub gfx: &'a mut Gfx,
    pub settings: &'a mut Settings,
    pub facts: &'a MapFacts,
    pub pkg: &'a str,
    pub nation: Option<&'a NationInfo>,
    pub msg: &'a mut Msg,
    pub nav: &'a mut Option<Nav>,
    pub loc: &'a LocCache,
    pub game: &'a std::path::Path,
    pub user_dir: &'a std::path::Path,
    /// The game's documentation of effects, triggers and modifiers.
    pub docs: &'a crate::content::docs::Docs,
}

impl<'a> Cx<'a> {
    pub fn info(&mut self, m: impl Into<String>) {
        let m = m.into();
        log::info!("{m}");
        *self.msg = Some((m, false, Instant::now()));
    }
    pub fn error(&mut self, m: impl Into<String>) {
        let m = m.into();
        log::warn!("{m}");
        *self.msg = Some((m, true, Instant::now()));
    }
    pub fn go(&mut self, n: Nav) {
        *self.nav = Some(n);
    }
    /// The tag of the package (`shared`: the mod's own, "MOD").
    pub fn tag(&self) -> &str {
        if self.pkg == SHARED { "MOD" } else { self.pkg }
    }
    /// The sources pictures are looked up in.
    pub fn sources(&self) -> Vec<&'a Index> {
        self.base.into_iter().collect()
    }
    /// The picture of a sprite.
    pub fn texture(&mut self, ctx: &egui::Context, sprite: &str) -> Option<(egui::TextureId, egui::Vec2)> {
        let srcs: Vec<&Index> = self.base.into_iter().collect();
        self.gfx.texture(ctx, sprite, self.st, &srcs)
    }
    /// A localised text (the project's, else the base game's).
    pub fn text(&self, key: &str) -> Option<&str> {
        self.loc.get(key, self.base)
    }
    /// A localised text as shown: `$KEY$` references to other texts are
    /// replaced by them (as the game does), then made plain.
    pub fn shown_text(&self, key: &str) -> Option<String> {
        let t = self.text(key)?;
        let mut out = String::new();
        let mut rest = t;
        let mut depth = 0;
        while let Some(a) = rest.find('$') {
            let Some(b) = rest[a + 1..].find('$') else { break };
            let name = &rest[a + 1..a + 1 + b];
            out.push_str(&rest[..a]);
            match self.text(name).filter(|_| depth < 8 && !name.is_empty() && !name.contains(' ')) {
                Some(sub) => out.push_str(sub),
                None => out.push_str(&rest[a..a + b + 2]),
            }
            depth += 1;
            rest = &rest[a + b + 2..];
        }
        out.push_str(rest);
        Some(loc::plain(&out))
    }
    /// A readable name for a definition: its localisation, else its id made readable.
    pub fn name_of(&self, kind: Kind, id: &str) -> String {
        let keys = kinds::loc_keys(kind, id, None);
        keys.first().and_then(|(k, _)| self.text(k)).map(loc::plain).filter(|t| !t.trim().is_empty()).unwrap_or_else(|| loc::prettify(id))
    }

    /// Change a definition's file with edits made against its text and
    /// block (one undo step; `merge` joins steps typed in one field).
    pub fn edit_def(&mut self, key: &DefKey, label: &str, merge: Option<String>, f: impl FnOnce(&str, &script::Block, &script::Item) -> Vec<Edit>) -> bool {
        let Some((d, text, s)) = self.st.def(key) else { return false };
        let Some(b) = d.block(s) else { return false };
        let item = d.item(s);
        let edits = match item {
            Some(it) => f(text, b, it),
            None => {
                // a whole-file definition: no item; give the root a stand-in
                let fake = script::Item { key: None, op: None, value: script::Value::None, span: 0..text.len() };
                f(text, b, &fake)
            }
        };
        if edits.is_empty() {
            return false;
        }
        let new = script::apply(text, &edits);
        self.st.begin_merge(label.to_string(), merge);
        self.st.set_text(&key.pkg, &key.file, new);
        self.st.commit();
        true
    }

    /// Change a whole file's text (one undo step).
    pub fn edit_file(&mut self, pkg: &str, file: &str, label: &str, merge: Option<String>, f: impl FnOnce(&str, Option<&Script>) -> Option<String>) -> bool {
        let (text, s) = match self.st.file(pkg, file) {
            Some(x) => (x.text().unwrap_or("").to_string(), x.script.as_ref()),
            None => (String::new(), None),
        };
        let Some(new) = f(&text, s) else { return false };
        self.st.begin_merge(label.to_string(), merge);
        self.st.set_text(pkg, file, new);
        self.st.commit();
        true
    }

    /// A new definition from its template in the package (one undo step).
    pub fn create(&mut self, kind: Kind, group: Option<&str>, id: &str) -> Option<DefKey> {
        let tag = self.tag().to_string();
        let text = kinds::template(kind, id, &tag);
        let pkg = self.pkg.to_string();
        self.st.begin(format!("new {} {id}", kind.spec().one));
        match import::insert_def(self.st, &pkg, kind, group, id, &text) {
            Ok(()) => {
                self.st.commit();
                self.info(format!("new {} {id}", kind.spec().one));
                self.st.find_in(&pkg, &[kind], id)
            }
            Err(e) => {
                self.st.rollback();
                self.error(format!("cannot add {id}: {e:#}"));
                None
            }
        }
    }

    /// An id for something new: `TAG_<what>_<n>` (events: `ns.n`) not taken yet.
    pub fn new_id(&self, kind: Kind, group: Option<&str>) -> String {
        let taken: BTreeSet<String> = self.st.defs().filter(|k| k.kind == kind).map(|k| k.id).collect();
        let base_has = |id: &str| self.base.is_some_and(|b| b.has(kind, id));
        if kind == Kind::Event {
            let ns = group.map(str::to_string).unwrap_or_else(|| self.tag().to_ascii_lowercase());
            return (1..).map(|n| format!("{ns}.{n}")).find(|id| !taken.contains(id) && !base_has(id)).unwrap();
        }
        let what = kind.spec().one.replace([' ', '/'], "_");
        (1..).map(|n| format!("{}_{what}_{n}", self.tag())).find(|id| !taken.contains(id) && !base_has(id)).unwrap()
    }
}

/// The base game read in the background.
struct BaseJob {
    progress: Arc<AtomicU32>,
    rx: Receiver<Result<Index, String>>,
    game: PathBuf,
}

pub struct Screen {
    pub settings: Settings,
    store: Option<Store>,
    /// A store of another map with unsaved changes (the map changed).
    stale: Option<Store>,
    load_err: Option<String>,
    base: Option<Arc<Index>>,
    base_job: Option<BaseJob>,
    base_err: Option<String>,
    facts: (Option<(PathBuf, u64)>, Arc<MapFacts>),
    vanilla_tags: (PathBuf, BTreeSet<String>),
    page: Page,
    msg: Msg,
    pub gfx: Gfx,
    loc: LocCache,
    nation_filter: String,
    more_kinds: bool,
    tree: tree::TreeState,
    form: form::FormState,
    kind_filter: String,
    new_group: String,
    files: FilesState,
    pub import: import_ui::ImportState,
    pub check: check::CheckState,
    tools: tools::ToolsState,
    /// Ask before deleting all of a nation's content.
    ask_delete_pkg: Option<String>,
    pub ask_close: bool,
    pub close_ok: bool,
    /// Counts per package and kind (at a generation of the store).
    counts: (u64, Arc<BTreeMap<String, BTreeMap<Kind, usize>>>),
    /// The game's documentation (read when the game folder is known).
    docs: (PathBuf, Arc<crate::content::docs::Docs>),
    /// From the start spec: a definition to open, and a window to open on it
    /// (`wizard:<field>`, `picker:<field>`, `code`, `about:<field>`, `map:<field>`).
    start_def: Option<(Kind, String)>,
    start_popup: Option<String>,
}

#[derive(Default)]
struct FilesState {
    sel: Option<(String, String)>,
    jump: Option<usize>,
    new_path: String,
    filter: String,
    rename: Option<String>,
}

impl Screen {
    pub fn new(settings: Settings) -> Screen {
        Screen {
            settings,
            store: None,
            stale: None,
            load_err: None,
            base: None,
            base_job: None,
            base_err: None,
            facts: (None, Arc::new(MapFacts::default())),
            vanilla_tags: (PathBuf::new(), BTreeSet::new()),
            page: Page::Overview,
            msg: None,
            gfx: Gfx::default(),
            loc: LocCache::default(),
            nation_filter: String::new(),
            more_kinds: false,
            tree: tree::TreeState::default(),
            form: form::FormState::default(),
            kind_filter: String::new(),
            new_group: String::new(),
            files: FilesState::default(),
            import: import_ui::ImportState::default(),
            check: check::CheckState::default(),
            tools: tools::ToolsState::default(),
            ask_delete_pkg: None,
            ask_close: false,
            close_ok: false,
            counts: (0, Arc::new(BTreeMap::new())),
            docs: (PathBuf::new(), Arc::new(crate::content::docs::Docs::default())),
            start_def: None,
            start_popup: None,
        }
    }

    /// Open at `TAG[/page[/what]]` (for screenshots and tests): pages are
    /// overview, trees (what: a tree id), check, play, import, export, loc,
    /// files, or a kind's name ("events").
    pub fn start_at(&mut self, spec: &str) {
        let (spec, popup) = spec.split_once('#').map(|(a, b)| (a, Some(b.to_string()))).unwrap_or((spec, None));
        self.start_popup = popup;
        let mut it = spec.split('/');
        if let Some(p) = it.next().filter(|p| !p.is_empty()) {
            self.settings.pkg = Some(p.to_string());
        }
        let what = it.clone().nth(1).map(str::to_string);
        let focus = it.clone().nth(2).map(str::to_string);
        self.page = match it.next().unwrap_or("overview") {
            "trees" => {
                if let Some(t) = &what {
                    self.tree.show(t.clone(), focus);
                }
                Page::Trees
            }
            "check" => Page::Check,
            "play" => Page::Play,
            "import" => {
                if what.as_deref() == Some("base") {
                    let kind = focus.as_deref().and_then(|k| Kind::all().find(|x| x.spec().label.to_lowercase().starts_with(k)));
                    self.import.show_base(kind, "");
                }
                Page::Import
            }
            "export" => Page::Export,
            "loc" => Page::Localisation,
            "files" => Page::Files,
            p => Kind::all().find(|k| k.spec().label.to_lowercase().starts_with(p)).map(Page::Kind).or_else(|| tools::Tool::ALL.into_iter().find(|t| t.label().to_lowercase().contains(p)).map(Page::Tool)).unwrap_or(Page::Overview),
        };
        if let (Page::Kind(k), Some(id)) = (self.page, what) {
            self.start_def = Some((k, id));
        }
    }

    /// Unsaved content.
    pub fn is_dirty(&self) -> bool {
        self.store.as_ref().is_some_and(|s| s.dirty()) || self.stale.as_ref().is_some_and(|s| s.dirty())
    }

    /// Something is still being read or decoded (screenshots wait for it).
    pub fn is_busy(&self) -> bool {
        self.base_job.is_some() || self.gfx.busy() || self.import.busy()
    }

    pub fn message(&self) -> Option<(&str, bool)> {
        self.msg.as_ref().filter(|m| m.2.elapsed() < Duration::from_secs(15)).map(|m| (m.0.as_str(), m.1))
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

    // ------------------------------------------------------------ loading

    fn start_base(&mut self, game: PathBuf, ctx: &egui::Context) {
        if !game.join("common").is_dir() {
            self.base_err = Some(format!("no HOI4 install at {} (set it in the map editor's build settings)", game.display()));
            return;
        }
        let progress = Arc::new(AtomicU32::new(0));
        let (tx, rx) = crossbeam_channel::bounded(1);
        let (p, c, g) = (progress.clone(), ctx.clone(), game.clone());
        std::thread::spawn(move || {
            let t = Instant::now();
            let r = source::index("base game", Origin::Base, source::base_files(&g), &|f| {
                p.store(f.to_bits(), Ordering::Relaxed);
                c.request_repaint();
            })
            .map_err(|e| format!("{e:#}"));
            if let Ok(ix) = &r {
                log::info!("base game read in {:.1} s: {} definitions, {} texts, {} sprites", t.elapsed().as_secs_f32(), ix.defs.len(), ix.loc.len(), ix.sprites.len());
            }
            let _ = tx.send(r);
            c.request_repaint();
        });
        self.base_job = Some(BaseJob { progress, rx, game });
        self.base_err = None;
    }

    fn poll(&mut self, editor: &Editor, ctx: &egui::Context) {
        let game = editor.game_dirs().game;
        if let Some(j) = &self.base_job
            && let Ok(r) = j.rx.try_recv()
        {
            let game_now = j.game.clone();
            self.base_job = None;
            match r {
                Ok(ix) => {
                    self.base = Some(Arc::new(ix));
                    self.gfx.clear();
                    self.check.stale();
                    let _ = game_now;
                }
                Err(e) => self.base_err = Some(e),
            }
        }
        let base_for_other_game = self.base.as_ref().is_some_and(|_| self.vanilla_tags.0 != game);
        if (self.base.is_none() || base_for_other_game) && self.base_job.is_none() && self.base_err.is_none() {
            self.start_base(game.clone(), ctx);
        }
        if self.vanilla_tags.0 != game {
            let tags = crate::modbuild::vanilla::tags(&game).unwrap_or_default();
            self.vanilla_tags = (game.clone(), tags);
            self.facts.0 = None;
        }
        // the map's nations, again when the map changed
        let g = editor.map_generation();
        if g != self.facts.0
            && let Some(f) = editor.map_facts(&self.vanilla_tags.1)
        {
            self.facts = (g.clone(), Arc::new(f));
        }
        // the content of the open map
        if let Some(dir) = editor.map_dir() {
            let want = dir.join("content");
            if self.store.as_ref().is_none_or(|s| s.root != want) && self.load_err.is_none() {
                if let Some(old) = self.store.take()
                    && old.dirty()
                {
                    self.stale = Some(old);
                }
                match Store::load(dir) {
                    Ok(st) => {
                        let n: usize = st.packages.values().map(|p| p.files.len()).sum();
                        self.info(format!("the nations' content: {} nations, {n} files", st.packages.len()));
                        self.store = Some(st);
                        self.check.stale();
                        self.gfx.clear();
                    }
                    Err(e) => self.load_err = Some(format!("{e:#}")),
                }
            }
        }
    }

    /// Fix the tags of nations that have content (their packages are named by them).
    fn pin_tags(&mut self, editor: &mut Editor) {
        let Some(st) = &self.store else { return };
        let need: Vec<(u32, String)> = self
            .facts
            .1
            .nations
            .iter()
            .filter(|n| !n.pinned && !n.tag.is_empty() && st.package(&n.tag).is_some_and(|p| !p.is_empty()))
            .map(|n| (n.id, n.tag.clone()))
            .collect();
        for (id, tag) in need {
            match editor.pin_tag(id, &tag) {
                Ok(()) => self.info(format!("{tag} is fixed in the map now, so its content stays with it (Ctrl+S saves the map too)")),
                Err(e) => self.error(format!("cannot fix the tag {tag}: {e}")),
            }
        }
    }

    // ------------------------------------------------------------ actions

    /// Save the content (and the map when it has changes).
    pub fn save(&mut self, editor: &mut Editor) {
        if editor.is_dirty() {
            editor.save();
        }
        let Some(st) = self.store.as_mut() else { return };
        match st.save() {
            Ok(0) => self.info("the content has no changes"),
            Ok(n) => {
                let m = format!("saved the content: {n} files");
                self.info(m);
            }
            Err(e) => self.error(format!("saving the content failed: {e:#}")),
        }
    }

    pub fn undo(&mut self, redo: bool) {
        let Some(st) = self.store.as_mut() else { return };
        let r = if redo { st.redo() } else { st.undo() };
        match r {
            Some(l) => self.info(format!("{}: {l}", if redo { "redid" } else { "undid" })),
            None => self.info(if redo { "nothing to redo" } else { "nothing to undo" }),
        }
    }

    pub fn shortcuts(&mut self, ctx: &egui::Context, editor: &mut Editor) {
        use egui::{Key, KeyboardShortcut, Modifiers};
        let sc = |m, k| ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(m, k)));
        if sc(Modifiers::COMMAND, Key::S) {
            self.save(editor);
        }
        // text fields keep their own undo while they are being typed in
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        if sc(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z) || sc(Modifiers::COMMAND, Key::Y) {
            self.undo(true);
        } else if sc(Modifiers::COMMAND, Key::Z) {
            self.undo(false);
        }
    }

    fn counts(&mut self) -> Arc<BTreeMap<String, BTreeMap<Kind, usize>>> {
        let Some(st) = &self.store else { return Arc::new(BTreeMap::new()) };
        if self.counts.0 != st.generation {
            let mut m: BTreeMap<String, BTreeMap<Kind, usize>> = BTreeMap::new();
            for k in st.defs() {
                *m.entry(k.pkg).or_default().entry(k.kind).or_default() += 1;
            }
            self.counts = (st.generation, Arc::new(m));
        }
        self.counts.1.clone()
    }

    // ------------------------------------------------------------ UI

    pub fn ui(&mut self, ui: &mut egui::Ui, editor: &mut Editor) {
        let ctx = ui.ctx().clone();
        editor.ensure_open(&ctx);
        self.poll(editor, &ctx);
        // files the map editor wrote to disk (an imported model)
        let written = editor.take_content_written();
        let mut errs = Vec::new();
        if let Some(st) = &mut self.store {
            for (pkg, files) in written {
                if let Err(e) = st.adopt(&pkg, &files) {
                    errs.push(format!("{e:#}"));
                }
            }
        }
        for e in errs {
            self.error(e);
        }
        self.pin_tags(editor);
        let (loading, err) = editor.load_state();
        if self.store.is_none() {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| {
                    if let Some(e) = err.or(self.load_err.as_deref()) {
                        ui.label(RichText::new(e).color(ERR));
                        ui.label("The content is kept with the map: open a map in the map editor first.");
                    } else if loading {
                        ui.spinner();
                        ui.label("reading the map…");
                    } else {
                        ui.label("Open a map in the map editor first: the content is kept with it.");
                    }
                });
            });
            return;
        }
        if self.stale.is_some() {
            self.stale_banner(ui);
        }
        let facts = self.facts.1.clone();
        // the package shown
        let pkg = self.settings.pkg.clone().filter(|p| p == SHARED || facts.nations.iter().any(|n| n.tag == *p) || self.store.as_ref().unwrap().package(p).is_some());
        let pkg = pkg.or_else(|| facts.nations.first().map(|n| n.tag.clone())).unwrap_or_else(|| SHARED.to_string());
        self.settings.pkg = Some(pkg.clone());

        let counts = self.counts();
        egui::Panel::left("content_nav").default_size(250.0).min_size(200.0).max_size(ui.available_width() * 0.3).show(ui, |ui| contained(ui, |ui| self.nav(ui, &facts, &counts, &pkg, editor)));
        let user_dir = crate::editor::launcher::GameInfo::read(&editor.game_dirs().game).map(|g| g.user_dir).unwrap_or_else(|_| crate::modbuild::default_mods_dir().parent().map(|p| p.to_path_buf()).unwrap_or_default());
        let game = editor.game_dirs().game;
        if self.docs.0 != game {
            self.docs = (game.clone(), Arc::new(crate::content::docs::Docs::load(&game)));
        }
        let docs = self.docs.1.clone();
        let mut nav = None;
        {
            let Screen { store, base, gfx, settings, msg, loc, page, tree, form, kind_filter, new_group, files, import, check, tools, .. } = self;
            let st = store.as_mut().unwrap();
            loc.refresh(st, &settings.language);
            let nation = facts.nations.iter().find(|n| n.tag == pkg);
            let mut cx = Cx { st, base: base.as_deref(), gfx, settings, facts: &facts, pkg: &pkg, nation, msg, nav: &mut nav, loc, game: &game, user_dir: &user_dir, docs: &docs };
            egui::CentralPanel::default().show(ui, |ui| match *page {
                Page::Overview => overview(ui, &mut cx, &counts),
                Page::Trees => tree::page(ui, &mut cx, tree, form),
                Page::Kind(k) => kind_page(ui, &mut cx, k, form, kind_filter, new_group),
                Page::Localisation => tools::loc_page(ui, &mut cx, tools),
                Page::Files => files_page(ui, &mut cx, files),
                Page::Check => check::check_page(ui, &mut cx, check),
                Page::Import => import_ui::import_page(ui, &mut cx, import),
                Page::Export => import_ui::export_page(ui, &mut cx, import),
                Page::Play => check::play_page(ui, &mut cx, check, editor),
                Page::Tool(t) => tools::page(ui, &mut cx, t, tools),
            });
        }
        if let Some(n) = nav {
            self.go(n);
        }
        self.start_popups(&pkg);
        self.delete_pkg_dialog(&ctx);
        self.close_dialog(&ctx, editor);
    }

    /// The definition and window the start spec asks for, once the content is read.
    fn start_popups(&mut self, pkg: &str) {
        let Some(st) = &self.store else { return };
        if let Some((k, id)) = self.start_def.take() {
            match st.find_in(pkg, &[k], &id) {
                Some(key) => self.go(Nav::Def(key)),
                None => log::warn!("start: no {} {id} in {pkg}", k.spec().one),
            }
            return;
        }
        let Some(key) = self.form.key.clone() else { return };
        let Some(p) = self.start_popup.take() else { return };
        if p == "code" {
            self.form.tab = form::Tab::Code;
            let role = crate::content::wizard::role_of_kind(key.kind);
            self.form.wizard.open(wizard::Target { key, field: String::new(), nth: None, role, cursor: None });
        } else if let Some(f) = p.strip_prefix("wizard:") {
            let (f, page) = f.split_once(':').map(|(a, b)| (a, Some(b))).unwrap_or((f, None));
            let has = st.def(&key).and_then(|(d, _, s)| d.block(s)).is_some_and(|b| b.get(f).is_some());
            if let Some(role) = crate::content::wizard::role_of(key.kind, f) {
                self.form.wizard.open(wizard::Target { key, field: f.to_string(), nth: has.then_some(0), role, cursor: None });
                if let Some(pg) = page {
                    self.form.wizard.show_page(pg);
                }
            }
        } else if p.starts_with("about:") || p.starts_with("map:") {
            self.form.start = Some(p);
        } else if let Some(rest) = p.strip_prefix("pick:") {
            // pick:<regiments|support|regimental_support>:<x>:<y>, the designer's list for a place
            self.form.division.open_at(rest);
        } else if let Some(f) = p.strip_prefix("picker:") {
            let prefix = key.kind.spec().fields.iter().find(|x| x.key == f).and_then(|x| if let kinds::FieldType::Sprite(p) = x.ty { Some(p) } else { None }).unwrap_or("");
            self.form.picker = Some(form::PickTarget { key, field: f.to_string(), prefix: prefix.to_string() });
            self.form.pick.find_current = true;
        }
    }

    fn go(&mut self, n: Nav) {
        match n {
            Nav::Page(p) => self.page = p,
            Nav::Pkg(p) => {
                self.settings.pkg = Some(p);
                self.form.key = None;
            }
            Nav::Def(k) => {
                self.settings.pkg = Some(k.pkg.clone());
                self.page = match k.kind {
                    Kind::Focus => Page::Trees,
                    k => Page::Kind(k),
                };
                if k.kind == Kind::Focus
                    && let Some(st) = &self.store
                    && let Some((d, _, _)) = st.def(&k)
                {
                    self.tree.show(d.group.clone().unwrap_or_default(), Some(k.id.clone()));
                }
                self.form.select(Some(k));
            }
            Nav::File { pkg, file, line } => {
                self.settings.pkg = Some(pkg.clone());
                self.page = Page::Files;
                self.files.sel = Some((pkg, file));
                self.files.jump = line;
            }
            Nav::DeletePkg(p) => self.ask_delete_pkg = Some(p),
        }
    }

    fn stale_banner(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("content_stale").show(ui, |ui| {
            ui.horizontal(|ui| {
                let root = self.stale.as_ref().map(|s| s.root.display().to_string()).unwrap_or_default();
                ui.label(RichText::new(format!("The content of the map opened before ({root}) has changes that are not saved.")).color(WARN));
                if ui.button("Save them").clicked()
                    && let Some(mut s) = self.stale.take()
                {
                    match s.save() {
                        Ok(n) => self.info(format!("saved {n} files of {root}")),
                        Err(e) => {
                            self.error(format!("{e:#}"));
                            self.stale = Some(s);
                        }
                    }
                }
                if ui.button("Discard them").clicked() {
                    self.stale = None;
                }
            });
        });
    }

    fn nav(&mut self, ui: &mut egui::Ui, facts: &MapFacts, counts: &BTreeMap<String, BTreeMap<Kind, usize>>, pkg: &str, editor: &mut Editor) {
        ui.horizontal(|ui| {
            ui.heading("Nation content");
        });
        ui.horizontal(|ui| {
            let dirty = self.store.as_ref().is_some_and(|s| s.dirty());
            if ui.add_enabled(dirty || editor.is_dirty(), egui::Button::new("💾 Save")).on_hover_text("Write the content (and the map) — Ctrl+S").clicked() {
                self.save(editor);
            }
            let st = self.store.as_ref().unwrap();
            let (u, r) = (st.can_undo().map(str::to_string), st.can_redo().map(str::to_string));
            if ui.add_enabled(u.is_some(), egui::Button::new("Undo")).on_hover_text(format!("Undo {} (Ctrl+Z)", u.unwrap_or_default())).clicked() {
                self.undo(false);
            }
            if ui.add_enabled(r.is_some(), egui::Button::new("Redo")).on_hover_text(format!("Redo {} (Ctrl+Y)", r.unwrap_or_default())).clicked() {
                self.undo(true);
            }
            if self.store.as_ref().unwrap().dirty() {
                ui.label(RichText::new("●").color(WARN)).on_hover_text("unsaved content");
            }
        });
        match (&self.base, &self.base_job, &self.base_err) {
            (_, Some(j), _) => {
                let f = f32::from_bits(j.progress.load(Ordering::Relaxed));
                ui.add(egui::ProgressBar::new(f).text("reading the base game…"));
            }
            (None, None, Some(e)) => {
                ui.label(RichText::new(e).color(WARN).small());
                if ui.small_button("try again").clicked() {
                    self.base_err = None;
                }
            }
            _ => {}
        }
        if let Some(e) = &facts.tag_error {
            ui.label(RichText::new(format!("tags: {e}")).color(ERR).small());
        }
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("🔍");
            ui.add(egui::TextEdit::singleline(&mut self.nation_filter).hint_text("nation or tag").desired_width(f32::INFINITY));
        });
        let filter = self.nation_filter.to_lowercase();
        let mut pick = None;
        let st = self.store.as_ref().unwrap();
        egui::ScrollArea::vertical().id_salt("nations").max_height((ui.available_height() * 0.42).max(120.0)).auto_shrink([false, true]).show(ui, |ui| {
            let n_shared: usize = counts.get(SHARED).map(|m| m.values().sum()).unwrap_or(0);
            if ui.selectable_label(pkg == SHARED, format!("🌐 shared (the whole mod){}", if n_shared > 0 { format!("  {n_shared}") } else { String::new() })).on_hover_text("Content that is no one nation's: ideologies, on actions, scripted effects, sprites...").clicked() {
                pick = Some(SHARED.to_string());
            }
            for n in &facts.nations {
                if !filter.is_empty() && !n.name.to_lowercase().contains(&filter) && !n.tag.to_lowercase().contains(&filter) {
                    continue;
                }
                let c: usize = counts.get(&n.tag).map(|m| m.values().sum()).unwrap_or(0);
                let mut t = RichText::new(format!("{}  {}{}", n.tag, n.name, if c > 0 { format!("  · {c}") } else { String::new() }));
                if c > 0 {
                    t = t.strong();
                }
                let r = ui.selectable_label(pkg == n.tag, t);
                let r = r.on_hover_text(format!("{} ({}), {} states{}", n.name, n.place, n.states.len(), if n.pinned { "" } else { "; tag made by the build" }));
                if r.clicked() {
                    pick = Some(n.tag.clone());
                }
            }
            // packages no nation has
            for name in st.packages.keys() {
                if name != SHARED
                    && !facts.nations.iter().any(|n| n.tag == *name)
                    && st.package(name).is_some_and(|p| !p.is_empty())
                    && ui.selectable_label(pkg == name, RichText::new(format!("⚠ {name} (no nation has this tag)")).color(WARN)).on_hover_text("The build stops on content for no nation: give a nation this tag, or move the content").clicked()
                {
                    pick = Some(name.clone());
                }
            }
        });
        if let Some(p) = pick {
            self.go(Nav::Pkg(p));
        }
        ui.separator();
        let c = counts.get(pkg).cloned().unwrap_or_default();
        let n = |k: Kind| c.get(&k).copied().unwrap_or(0);
        egui::ScrollArea::vertical().id_salt("pages").auto_shrink([false, false]).show(ui, |ui| {
            let mut page = self.page;
            let title = if pkg == SHARED { "shared".to_string() } else { facts.nations.iter().find(|x| x.tag == pkg).map(|x| format!("{pkg} {}", x.name)).unwrap_or(pkg.to_string()) };
            ui.label(RichText::new(title).strong());
            let label = |name: &str, n: usize| if n > 0 { format!("{name}  {n}") } else { name.to_string() };
            ui.selectable_value(&mut page, Page::Overview, "🏠 Overview");
            ui.selectable_value(&mut page, Page::Trees, label("🌳 Focus trees", n(Kind::FocusTree)));
            const MAIN: [Kind; 8] = [Kind::Event, Kind::Decision, Kind::DecisionCategory, Kind::Idea, Kind::Character, Kind::CountryHistory, Kind::DivisionTemplate, Kind::SharedFocus];
            for k in MAIN {
                ui.selectable_value(&mut page, Page::Kind(k), label(&format!("   {}", k.label()), n(k)));
            }
            let open = egui::CollapsingHeader::new("More kinds").default_open(self.more_kinds).show(ui, |ui| {
                for k in Kind::listed().filter(|k| !MAIN.contains(k) && *k != Kind::FocusTree) {
                    let t = RichText::new(label(k.label(), n(k)));
                    let hint = if k.spec().national { "usually a nation's own" } else { "usually the whole mod's: the shared package is the place for it" };
                    ui.selectable_value(&mut page, Page::Kind(k), if n(k) > 0 { t.strong() } else { t }).on_hover_text(format!("{}\n{hint}", k.spec().help));
                }
            });
            self.more_kinds = open.body_returned.is_some();
            ui.selectable_value(&mut page, Page::Localisation, "🗣 Localisation");
            ui.selectable_value(&mut page, Page::Files, "📁 Files");
            ui.separator();
            ui.label(RichText::new("The whole project").strong());
            let (e, w) = self.check.counts();
            let check_label = if e + w > 0 { format!("✔ Check  ({e} errors, {w} warnings)") } else { "✔ Check".to_string() };
            ui.selectable_value(&mut page, Page::Check, check_label);
            ui.selectable_value(&mut page, Page::Import, "📥 Import");
            ui.selectable_value(&mut page, Page::Export, "📤 Export and copy");
            ui.selectable_value(&mut page, Page::Play, "▶ Test play and errors");
            for t in tools::Tool::ALL {
                ui.selectable_value(&mut page, Page::Tool(t), t.label());
            }
            if page != self.page {
                self.page = page;
            }
        });
    }

    fn delete_pkg_dialog(&mut self, ctx: &egui::Context) {
        let Some(p) = self.ask_delete_pkg.clone() else { return };
        let mut close = false;
        egui::Modal::new(egui::Id::unique("delete_pkg")).show(ctx, |ui| {
            ui.heading(format!("Delete all content of {p}?"));
            ui.label("Every file of it goes (Ctrl+Z brings them back; saving keeps a backup of each).");
            ui.horizontal(|ui| {
                if ui.button(RichText::new("Delete").color(ERR)).clicked() {
                    if let Some(st) = self.store.as_mut() {
                        st.begin(format!("delete the content of {p}"));
                        for f in st.files_of(&p) {
                            st.remove(&p, &f);
                        }
                        st.commit();
                    }
                    close = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        if close {
            self.ask_delete_pkg = None;
        }
    }

    fn close_dialog(&mut self, ctx: &egui::Context, editor: &mut Editor) {
        if !self.ask_close {
            return;
        }
        let mut choice = None;
        egui::Modal::new(egui::Id::unique("content_close")).show(ctx, |ui| {
            ui.heading("Unsaved content");
            ui.label("The nation content has changes that are not saved.");
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
                self.save(editor);
                self.ask_close = false;
                if !self.is_dirty() && !editor.is_dirty() {
                    self.close_ok = true;
                    editor.close_ok = true;
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

// ---------------------------------------------------------------- overview

fn overview(ui: &mut egui::Ui, cx: &mut Cx, counts: &BTreeMap<String, BTreeMap<Kind, usize>>) {
    let pkg = cx.pkg.to_string();
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        if pkg == SHARED {
            ui.heading("Shared content");
            ui.label("Content of the mod as a whole: ideologies, on actions, scripted effects and triggers, opinion modifiers, sprites, music. The build copies it into the mod with the nations' content.");
        } else if let Some(n) = cx.nation {
            ui.heading(format!("{} — {}", n.tag, n.name));
            egui::Grid::new("nation_facts").num_columns(2).show(ui, |ui| {
                ui.label("tag");
                ui.horizontal(|ui| {
                    ui.monospace(&n.tag);
                    if n.pinned {
                        ui.label(RichText::new("fixed in the map").weak());
                    } else {
                        ui.label(RichText::new("made by the build from the name; it is fixed as soon as the nation has content").color(WARN));
                    }
                });
                ui.end_row();
                ui.label("settlement");
                ui.label(&n.place);
                ui.end_row();
                ui.label("capital state");
                ui.label(format!("{} ({})", cx.facts.state_names.get(&n.capital_state).cloned().unwrap_or_default(), n.capital_state));
                ui.end_row();
                ui.label("states");
                ui.label(format!("{}: {}", n.states.len(), n.states.iter().take(12).map(|s| s.to_string()).collect::<Vec<_>>().join(", ")));
                ui.end_row();
                ui.label("language");
                ui.label(&n.language);
                ui.end_row();
            });
            ui.label(RichText::new("The build makes the country itself (its file, colour, flag, capital, starting army and technology); a country history here adds to what it writes.").weak());
        } else {
            ui.heading(&pkg);
            ui.label(RichText::new("No nation of the map has this tag: the build stops until one does (rename the package on Tools → Rename, or give a nation this tag in the map editor).").color(WARN));
        }
        ui.separator();
        ui.heading("Content");
        let c = counts.get(&pkg).cloned().unwrap_or_default();
        if c.is_empty() {
            ui.label("Nothing yet. Start with one of these, or import a tree, events and the rest from the base game or a mod (📥 Import).");
        }
        egui::Grid::new("content_counts").num_columns(3).striped(true).show(ui, |ui| {
            for (k, n) in &c {
                ui.label(k.label());
                ui.label(n.to_string());
                if ui.small_button("open").clicked() {
                    cx.go(Nav::Page(if *k == Kind::FocusTree || *k == Kind::Focus { Page::Trees } else { Page::Kind(*k) }));
                }
                ui.end_row();
            }
        });
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            let quick: &[Kind] = if pkg == SHARED {
                &[Kind::ScriptedEffect, Kind::ScriptedTrigger, Kind::OnAction, Kind::OpinionModifier, Kind::Ideology, Kind::Sprite]
            } else {
                &[Kind::FocusTree, Kind::Event, Kind::Decision, Kind::Idea, Kind::Character, Kind::CountryHistory]
            };
            for &k in quick {
                if ui.button(format!("+ {}", k.spec().one)).clicked() {
                    let id = if k == Kind::CountryHistory { pkg.clone() } else { cx.new_id(k, None) };
                    if let Some(key) = cx.create(k, None, &id) {
                        cx.go(Nav::Def(key));
                    }
                }
            }
        });
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            if ui.button("📥 Import from the base game or a mod…").clicked() {
                cx.go(Nav::Page(Page::Import));
            }
            if ui.button("📤 Export as a bundle, or copy to another nation…").clicked() {
                cx.go(Nav::Page(Page::Export));
            }
            if ui.button("✔ Check").clicked() {
                cx.go(Nav::Page(Page::Check));
            }
        });
        if !c.is_empty() {
            ui.add_space(16.0);
            if ui.button(RichText::new(format!("🗑 Delete all content of {pkg}…")).color(ERR)).clicked() {
                cx.go(Nav::DeletePkg(pkg.clone()));
            }
        }
    });
}

// ---------------------------------------------------------------- kind lists

/// Definitions of a kind in the package, in file order: (key, group).
pub fn defs_in(st: &Store, pkg: &str, kind: Kind) -> Vec<(DefKey, Option<String>)> {
    st.defs_of(Some(pkg), kind)
        .into_iter()
        .map(|k| {
            let g = st.def(&k).and_then(|(d, _, _)| d.group.clone());
            (k, g)
        })
        .collect()
}

/// Groups definitions of a kind can be in (category, namespace...).
pub fn groups_of(st: &Store, base: Option<&Index>, pkg: &str, kind: Kind) -> Vec<String> {
    let mut g: BTreeSet<String> = defs_in(st, pkg, kind).into_iter().filter_map(|x| x.1).collect();
    match kind {
        Kind::Idea => g.extend(["country", "economy", "trade_laws", "mobilization_laws", "political_advisor", "theorist", "army_chief", "navy_chief", "air_chief", "high_command"].map(String::from)),
        Kind::Decision => {
            g.extend(st.defs_of(None, Kind::DecisionCategory).into_iter().map(|k| k.id));
            if let Some(b) = base {
                g.extend(["political_actions", "economy_decisions", "foreign_politics", "war_measures"].iter().filter(|c| b.has(Kind::DecisionCategory, c)).map(|c| c.to_string()));
            }
        }
        _ => {}
    }
    g.into_iter().collect()
}

fn kind_page(ui: &mut egui::Ui, cx: &mut Cx, kind: Kind, form: &mut form::FormState, filter: &mut String, new_group: &mut String) {
    let spec = kind.spec();
    let pkg = cx.pkg.to_string();
    ui.horizontal(|ui| {
        ui.heading(format!("{} — {}", spec.label, if pkg == SHARED { "shared" } else { &pkg }));
        ui.label(RichText::new(spec.help).weak());
    });
    let list = defs_in(cx.st, &pkg, kind);
    let groups = groups_of(cx.st, cx.base, &pkg, kind);
    let needs_group = matches!(kind, Kind::Idea | Kind::Decision | Kind::Event);
    ui.horizontal(|ui| {
        if needs_group {
            let label = match kind {
                Kind::Idea => "category",
                Kind::Decision => "decision category",
                _ => "namespace",
            };
            if new_group.is_empty() || (kind != Kind::Event && !groups.contains(new_group)) {
                *new_group = match kind {
                    Kind::Event => pkg.to_ascii_lowercase(),
                    Kind::Idea => "country".into(),
                    _ => groups.first().cloned().unwrap_or_else(|| "political_actions".into()),
                };
            }
            egui::ComboBox::from_id_salt("new_group").selected_text(new_group.as_str()).show_ui(ui, |ui| {
                for g in &groups {
                    ui.selectable_value(new_group, g.clone(), g);
                }
            });
            if kind == Kind::Event {
                ui.add(egui::TextEdit::singleline(new_group).desired_width(80.0)).on_hover_text("the namespace of new events");
            }
            ui.label(RichText::new(label).weak());
        }
        let group = needs_group.then_some(new_group.as_str());
        if ui.button(format!("+ New {}", spec.one)).clicked() {
            let id = if kind == Kind::CountryHistory { pkg.clone() } else { cx.new_id(kind, group) };
            if let Some(k) = cx.create(kind, group, &id) {
                form.select(Some(k));
            }
        }
        ui.separator();
        ui.label("🔍");
        ui.add(egui::TextEdit::singleline(filter).desired_width(160.0).hint_text("filter"));
    });
    ui.separator();
    if form.key.as_ref().is_some_and(|k| k.kind != kind || k.pkg != pkg || cx.st.def(k).is_none()) {
        form.select(None);
    }
    if form.key.is_none()
        && let Some((k, _)) = list.first()
    {
        form.select(Some(k.clone()));
    }
    let f = filter.to_lowercase();
    egui::Panel::left("kind_list").default_size(280.0).max_size(ui.available_width() * 0.4).show(ui, |ui| {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            if list.is_empty() {
                ui.label(RichText::new(format!("No {} yet.", spec.label.to_lowercase())).weak());
            }
            let mut last_group: Option<&Option<String>> = None;
            for (k, g) in &list {
                let name = cx.name_of(kind, &k.id);
                if !f.is_empty() && !k.id.to_lowercase().contains(&f) && !name.to_lowercase().contains(&f) {
                    continue;
                }
                if g.is_some() && last_group != Some(g) {
                    ui.label(RichText::new(g.clone().unwrap_or_default()).small().weak());
                    last_group = Some(g);
                }
                let sel = form.key.as_ref() == Some(k);
                ui.horizontal(|ui| {
                    if let Some(sprite) = form::list_sprite(cx, k) {
                        let tex = cx.texture(ui.ctx(), &sprite);
                        super::gfx::show(ui, tex, egui::vec2(22.0, 22.0));
                    }
                    let r = ui.selectable_label(sel, RichText::new(&k.id).monospace()).on_hover_text(format!("{name}\n{}", k.file));
                    if r.clicked() {
                        form.select(Some(k.clone()));
                    }
                    if name != loc::prettify(&k.id) {
                        ui.label(RichText::new(name).weak().small());
                    }
                });
            }
        });
    });
    egui::CentralPanel::default().show(ui, |ui| {
        if let Some(k) = form.key.clone() {
            form::def_editor(ui, cx, &k, form);
        }
    });
}

// ---------------------------------------------------------------- files

fn files_page(ui: &mut egui::Ui, cx: &mut Cx, fs: &mut FilesState) {
    let pkg = cx.pkg.to_string();
    let files = cx.st.files_of(&pkg);
    if fs.sel.as_ref().is_some_and(|(p, f)| *p != pkg || !files.contains(f)) {
        fs.sel = None;
    }
    ui.horizontal(|ui| {
        ui.heading(format!("Files — {}", if pkg == SHARED { "shared" } else { &pkg }));
        ui.label(RichText::new(format!("{}", cx.st.root.join(&pkg).display())).weak().small());
    });
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut fs.new_path).desired_width(320.0).hint_text("events/tyr_more.txt"));
        if ui.button("+ New file").clicked() {
            let p = fs.new_path.trim().trim_start_matches('/').replace('\\', "/");
            if p.is_empty() || p.contains("..") {
                cx.error("give the file a path in the mod, like events/tyr_more.txt");
            } else if files.contains(&p) {
                cx.error(format!("{p} is there already"));
            } else {
                let text = if p.ends_with(".yml") { loc::render(loc::language_of_name(&p).unwrap_or("english"), &[]) } else { String::new() };
                cx.st.begin(format!("new file {p}"));
                cx.st.set_text(&pkg, &p, text);
                cx.st.commit();
                fs.sel = Some((pkg.clone(), p));
                fs.new_path.clear();
            }
        }
        if ui.button("Add files from disk…").on_hover_text("Pictures (gfx/...), music, any file the mod needs; they go to the folder of the path box (or the root)").clicked()
            && let Some(paths) = rfd::FileDialog::new().pick_files()
        {
            let dir = fs.new_path.trim().trim_end_matches('/').to_string();
            cx.st.begin("add files");
            let mut n = 0;
            for p in paths {
                let Ok(b) = std::fs::read(&p) else { continue };
                let name = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
                let rel = if dir.is_empty() { name } else { format!("{dir}/{name}") };
                match String::from_utf8(b.clone()) {
                    Ok(t) if !rel.ends_with(".dds") && !rel.ends_with(".tga") && !rel.ends_with(".png") => cx.st.set_text(&pkg, &rel, t.trim_start_matches('\u{feff}').to_string()),
                    _ => cx.st.set_bytes(&pkg, &rel, b),
                }
                n += 1;
            }
            cx.st.commit();
            cx.info(format!("added {n} files"));
        }
        ui.label("🔍");
        ui.add(egui::TextEdit::singleline(&mut fs.filter).desired_width(140.0));
    });
    ui.separator();
    let f = fs.filter.to_lowercase();
    egui::Panel::left("file_list").default_size(300.0).max_size(ui.available_width() * 0.4).show(ui, |ui| {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let mut last_dir = String::new();
            for p in &files {
                if !f.is_empty() && !p.to_lowercase().contains(&f) {
                    continue;
                }
                let (dir, name) = p.rsplit_once('/').unwrap_or(("", p));
                if dir != last_dir {
                    ui.label(RichText::new(format!("{dir}/")).small().weak());
                    last_dir = dir.to_string();
                }
                let dirty = cx.st.file(&pkg, p).is_some_and(|x| x.dirty());
                let errs = cx.st.file(&pkg, p).and_then(|x| x.script.as_ref()).is_some_and(|s| !s.errors.is_empty());
                let mut t = RichText::new(format!("{}{name}", if dirty { "● " } else { "" }));
                if errs {
                    t = t.color(ERR);
                }
                if ui.selectable_label(fs.sel.as_ref().is_some_and(|s| s.1 == *p), t).clicked() {
                    fs.sel = Some((pkg.clone(), p.clone()));
                    fs.rename = None;
                }
            }
        });
    });
    egui::CentralPanel::default().show(ui, |ui| {
        let Some((_, path)) = fs.sel.clone() else {
            ui.label(RichText::new("Pick a file.").weak());
            return;
        };
        ui.horizontal(|ui| {
            ui.monospace(&path);
            if let Some(r) = &mut fs.rename {
                ui.add(egui::TextEdit::singleline(r).desired_width(300.0));
                if ui.button("Move").clicked() {
                    let to = r.trim().trim_start_matches('/').to_string();
                    if !to.is_empty() && to != path && !files.contains(&to) {
                        cx.st.begin(format!("move {path} to {to}"));
                        match cx.st.text(&pkg, &path).map(str::to_string) {
                            Some(t) => cx.st.set_text(&pkg, &to, t),
                            None => {
                                if let Some(b) = cx.st.bytes(&pkg, &path) {
                                    cx.st.set_bytes(&pkg, &to, (*b).clone());
                                }
                            }
                        }
                        cx.st.remove(&pkg, &path);
                        cx.st.commit();
                        fs.sel = Some((pkg.clone(), to));
                        fs.rename = None;
                    }
                }
                if ui.button("Cancel").clicked() {
                    fs.rename = None;
                }
            } else if ui.small_button("Move / rename").clicked() {
                fs.rename = Some(path.clone());
            }
            if ui.small_button("Format").on_hover_text("Indent by the braces").clicked() {
                cx.edit_file(&pkg, &path, "format", None, |t, _| Some(script::format(t)));
            }
            if ui.small_button(RichText::new("Delete").color(ERR)).clicked() {
                cx.st.begin(format!("delete {path}"));
                cx.st.remove(&pkg, &path);
                cx.st.commit();
                fs.sel = None;
            }
        });
        let Some(file) = cx.st.file(&pkg, &path) else { return };
        if let Some(text) = file.text() {
            let lang = code::Lang::of_path(&path);
            let errors: Vec<(usize, String)> = file.script.as_ref().map(|s| s.errors.iter().map(|e| (script::line_of(text, e.at), e.message.clone())).collect()).unwrap_or_default();
            let lines: Vec<usize> = errors.iter().map(|e| e.0).collect();
            let defs: Vec<(Kind, String, usize)> = file.defs.iter().map(|d| (d.kind, d.id.clone(), script::line_of(text, d.span.start))).collect();
            let mut buf = text.to_string();
            for (l, m) in errors.iter().take(5) {
                if ui.link(RichText::new(format!("line {l}: {m}")).color(ERR)).clicked() {
                    fs.jump = Some(*l);
                }
            }
            if !defs.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("in it:").weak());
                    for (k, id, l) in defs.iter().take(40) {
                        if ui.small_button(id).on_hover_text(format!("{} at line {l}", k.spec().one)).clicked() {
                            fs.jump = Some(*l);
                        }
                    }
                    if defs.len() > 40 {
                        ui.label(format!("… {} more", defs.len() - 40));
                    }
                });
            }
            let rows = ((ui.available_height() - 30.0) / 15.0).max(8.0) as usize;
            let out = code::editor(ui, egui::Id::unique(("file", &pkg, &path)), &mut buf, lang, &lines, rows, &mut fs.jump);
            if out.changed {
                cx.st.begin_merge(format!("edit {path}"), Some(format!("file:{pkg}/{path}")));
                cx.st.set_text(&pkg, &path, buf);
                cx.st.commit();
            }
            if let Some(l) = out.cursor_line {
                ui.label(RichText::new(format!("line {l}")).small().weak());
            }
        } else {
            let tex = cx.texture(ui.ctx(), &path);
            let n = cx.st.bytes(&pkg, &path).map(|b| b.len()).unwrap_or(0);
            ui.label(format!("{n} bytes"));
            if path.ends_with(".dds") || path.ends_with(".tga") || path.ends_with(".png") || path.ends_with(".jpg") {
                super::gfx::show(ui, tex, egui::vec2(256.0, 256.0));
            }
        }
    });
}

/// Pick a nation (or shared) from a combo box: returns the one picked.
pub fn nation_combo(ui: &mut egui::Ui, id: &str, facts: &MapFacts, current: &mut String, with_shared: bool) -> bool {
    let label = if current == SHARED { "shared".to_string() } else { facts.nations.iter().find(|n| n.tag == *current).map(|n| format!("{} {}", n.tag, n.name)).unwrap_or(current.clone()) };
    let mut changed = false;
    egui::ComboBox::from_id_salt(id).selected_text(label).height(400.0).show_ui(ui, |ui| {
        if with_shared {
            changed |= ui.selectable_value(current, SHARED.to_string(), "shared").changed();
        }
        for n in &facts.nations {
            changed |= ui.selectable_value(current, n.tag.clone(), format!("{} {}", n.tag, n.name)).changed();
        }
    });
    changed
}
