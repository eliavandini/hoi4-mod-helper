//! The focus tree view: the tree drawn as the game draws it (icons on a
//! grid, prerequisite lines, dashed when one of several is enough, red
//! where focuses exclude each other), and edited on the canvas.
//!
//! Modes: select (click, Shift/Ctrl-click, drag a box; drag focuses to
//! move them, snapped to the grid), link (click a focus, then the one that
//! needs it; Shift: one of several), exclusive (click two focuses), add
//! (click an empty cell), pan. Wheel zooms, the middle button pans.
//! Delete removes the selected focuses (and what names them in the tree),
//! Ctrl+D duplicates them, arrows nudge them, F fits the tree.
//!
//! Also: play the tree (what can be taken after what), find, an outline, a
//! minimap, tidy (an automatic layout), copy and shift a branch, a DLC
//! filter, the checks of the tree, a PNG of the whole tree, and a tree
//! from a draw.io diagram.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Stroke, Vec2, pos2, vec2};
use parking_lot::Mutex;

use super::form::{self, FormState};
use super::{Cx, ERR, Nav, OK, WARN};
use crate::content::drawio;
use crate::content::focus::{self, Focus, State, Tree};
use crate::content::import;
use crate::content::kinds::Kind;
use crate::content::loc;
use crate::content::refactor;
use crate::content::script::{self, Edit, Script};
use crate::content::store::DefKey;

/// Pixels per grid step at zoom 1, as in the game.
const CW: f32 = 96.0;
const CH: f32 = 130.0;
const CARD_W: f32 = 90.0;
const ICON_H: f32 = 64.0;
const PLATE_H: f32 = 34.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Select,
    Link,
    Exclusive,
    Add,
    Pan,
}

impl Mode {
    fn hint(self) -> &'static str {
        match self {
            Mode::Select => "click a focus to select it (Shift/Ctrl: more), drag to move; drag on empty space selects a box",
            Mode::Link => "click a focus, then the focus that needs it (Shift: as one of several, either is enough); again on a link removes it",
            Mode::Exclusive => "click two focuses to make them mutually exclusive (again: not any more)",
            Mode::Add => "click an empty cell for a new focus (it needs the selected one, if any)",
            Mode::Pan => "drag to pan; the wheel zooms",
        }
    }
}

/// A focus as drawn.
#[derive(Clone)]
struct Node {
    f: Focus,
    /// Absolute grid position.
    x: i32,
    y: i32,
    key: Option<DefKey>,
    name: String,
    /// Its focus can be edited (it is in the project).
    editable: bool,
    dlc_hidden: bool,
}

struct Model {
    key: (u64, DefKey, Option<BTreeSet<String>>),
    tree: Tree,
    nodes: Vec<Node>,
    idx: HashMap<String, usize>,
    issues: Vec<focus::Issue>,
    /// min x, min y, max x, max y (grid).
    bounds: (i32, i32, i32, i32),
    dlcs: BTreeSet<String>,
}

#[derive(Clone)]
struct Drag {
    start: Pos2,
    ids: Vec<String>,
    moved: bool,
}

struct Export {
    path: PathBuf,
    tiles: Vec<(Vec2, (u32, u32))>,
    at: usize,
    wait: u32,
    requested: bool,
    shot: Arc<Mutex<Option<Arc<egui::ColorImage>>>>,
    img: image::RgbaImage,
    origin: Vec2,
    /// The zoom of the picture (1: as at zoom 1; less for trees too big for one picture).
    zoom: f32,
    /// The view to go back to.
    view: (f32, Vec2),
}

struct DrawioPlan {
    path: PathBuf,
    pages: Vec<(String, String)>,
    page: usize,
    flip: bool,
    tree_id: String,
    focuses: Vec<drawio::Planned>,
}

#[derive(Default)]
pub struct TreeState {
    /// The tree shown (its id) in a package.
    tree: Option<(String, String)>,
    sel: BTreeSet<String>,
    primary: Option<String>,
    mode: Mode,
    link_from: Option<String>,
    off: Vec2,
    zoom: f32,
    fit: bool,
    /// The next fit is the first look at a tree: a big tree is shown from its top, readable.
    gentle: bool,
    center_on: Option<String>,
    drag: Option<Drag>,
    rubber: Option<(Pos2, Pos2)>,
    pan: Option<Pos2>,
    sim: Option<BTreeSet<String>>,
    find: String,
    hits: Vec<String>,
    hit_at: usize,
    outline: bool,
    outline_filter: String,
    minimap: bool,
    show_hidden: bool,
    model: Option<Arc<Model>>,
    base_shared: Option<(usize, Arc<Vec<Focus>>)>,
    new_tree: Option<String>,
    copy: Option<(String, String, String, i32, i32)>,
    shift: Option<(String, i32, i32)>,
    tidy_ask: bool,
    export: Option<Export>,
    drawio: Option<DrawioPlan>,
    context: Option<String>,
    issues_open: bool,
    rect: CanvasRect,
}

/// Where the canvas was drawn last.
#[derive(Clone, Copy)]
struct CanvasRect(Rect);

impl Default for CanvasRect {
    fn default() -> Self {
        CanvasRect(Rect::ZERO)
    }
}

impl TreeState {
    /// Show a tree (and a focus of it).
    pub fn show(&mut self, tree: String, focus: Option<String>) {
        if let Some((_, t)) = &self.tree
            && *t != tree
        {
            self.fit = true;
            self.gentle = true;
            self.sim = None;
        }
        // the package is the one shown when the page draws
        self.tree = Some((String::new(), tree));
        if let Some(f) = focus {
            self.sel = [f.clone()].into();
            self.primary = Some(f.clone());
            self.center_on = Some(f);
        }
    }

    fn world(n: &Node) -> Pos2 {
        pos2(n.x as f32 * CW + CW / 2.0, n.y as f32 * CH)
    }
    fn to_screen(&self, w: Pos2) -> Pos2 {
        self.rect.0.min + self.off + w.to_vec2() * self.zoom
    }
    fn to_world(&self, p: Pos2) -> Pos2 {
        ((p - self.rect.0.min - self.off) / self.zoom).to_pos2()
    }
    /// The card of a node on screen.
    fn card(&self, n: &Node, d: Vec2) -> Rect {
        let w = Self::world(n) + d;
        let top = self.to_screen(w - vec2(CARD_W / 2.0, 0.0));
        Rect::from_min_size(top, vec2(CARD_W, ICON_H + PLATE_H) * self.zoom)
    }
    /// The grid cell under a screen point.
    fn cell_at(&self, p: Pos2) -> (i32, i32) {
        let w = self.to_world(p);
        (((w.x - CW / 2.0) / CW).round() as i32, ((w.y - (ICON_H + PLATE_H) / 2.0) / CH).round() as i32)
    }
    fn fit_to(&mut self, m: &Model, gentle: bool) {
        let (x0, y0, x1, y1) = m.bounds;
        let w = (x1 - x0 + 1) as f32 * CW + 40.0;
        let h = (y1 - y0 + 1) as f32 * CH + 40.0;
        let z = (self.rect.0.width() / w).min(self.rect.0.height() / h).clamp(0.08, 1.2);
        if gentle && z < 0.45 {
            // too small to read: the top of the tree, around its first focuses
            self.zoom = 0.6;
            let top: Vec<&Node> = m.nodes.iter().filter(|n| n.y == y0).collect();
            let cx = top.iter().map(|n| n.x as f32).sum::<f32>() / top.len().max(1) as f32 * CW + CW / 2.0;
            self.off = vec2(self.rect.0.width() / 2.0 - cx * self.zoom, 20.0 - y0 as f32 * CH * self.zoom);
            return;
        }
        self.zoom = z;
        let cx = ((x0 + x1) as f32 / 2.0) * CW + CW / 2.0;
        let cy = ((y0 + y1) as f32 / 2.0) * CH + CH / 2.0;
        self.off = self.rect.0.size() / 2.0 - vec2(cx, cy) * z;
    }
    fn center(&mut self, n: &Node) {
        if self.zoom < 0.5 {
            self.zoom = 0.8;
        }
        let w = Self::world(n) + vec2(0.0, CH / 2.0);
        self.off = self.rect.0.size() / 2.0 - w.to_vec2() * self.zoom;
    }
}

// ---------------------------------------------------------------- the model

/// The base game's shared focuses (read once per base game).
fn base_shared(ts: &mut TreeState, cx: &Cx) -> Arc<Vec<Focus>> {
    let Some(b) = cx.base else { return Arc::new(Vec::new()) };
    let id = b as *const _ as usize;
    if let Some((i, v)) = &ts.base_shared
        && *i == id
    {
        return v.clone();
    }
    let mut out = Vec::new();
    let mut by_file: BTreeMap<u32, Vec<&crate::content::source::SDef>> = BTreeMap::new();
    for d in b.of_kind(Kind::SharedFocus) {
        by_file.entry(d.file).or_default().push(d);
    }
    for (_, ds) in by_file {
        let Some(t) = b.files.text(b.path(ds[0])) else { continue };
        let s = Script::parse(&t);
        out.extend(focus::shared_focuses(&s));
    }
    let v = Arc::new(out);
    ts.base_shared = Some((id, v.clone()));
    v
}

fn build(cx: &Cx, key: &DefKey, base_shared: &[Focus], owned: &Option<BTreeSet<String>>) -> Option<Model> {
    let (d, _, s) = cx.st.def(key)?;
    let b = d.block(s)?;
    let mut tree = focus::tree_of(b);
    // shared focuses: the project's, then the base game's
    let mut shared: Vec<Focus> = Vec::new();
    let mut shared_keys: HashMap<String, DefKey> = HashMap::new();
    for k in cx.st.defs_of(None, Kind::SharedFocus) {
        if let Some((d, _, s)) = cx.st.def(&k)
            && let Some(b) = d.block(s)
            && let Some(f) = focus::focus_of(b, d.span.clone(), true)
        {
            shared_keys.entry(f.id.clone()).or_insert(k.clone());
            shared.push(f);
        }
    }
    let have: BTreeSet<String> = shared.iter().map(|f| f.id.clone()).collect();
    shared.extend(base_shared.iter().filter(|f| !have.contains(&f.id)).cloned());
    focus::include_shared(&mut tree, &shared);
    let own_keys: HashMap<String, DefKey> = {
        let mut m = HashMap::new();
        for k in cx.st.defs_of(Some(&key.pkg), Kind::Focus) {
            if k.file == key.file
                && let Some((d, _, _)) = cx.st.def(&k)
                && d.group.as_deref() == Some(tree.id.as_str())
            {
                m.entry(k.id.clone()).or_insert(k);
            }
        }
        m
    };
    let pos = tree.positions();
    let dlcs = tree.dlcs();
    let hidden = match owned {
        Some(o) => tree.dlc_hidden(o),
        None => BTreeSet::new(),
    };
    let issues = focus::check(&tree);
    let mut nodes = Vec::new();
    for f in &tree.focuses {
        let (x, y) = pos.get(&f.id).copied().unwrap_or((f.x, f.y));
        let k = if f.shared { shared_keys.get(&f.id).cloned() } else { own_keys.get(&f.id).cloned() };
        let name_key = f.text.clone().unwrap_or_else(|| f.id.clone());
        let name = cx.text(&name_key).map(loc::plain).filter(|t| !t.trim().is_empty()).unwrap_or_else(|| loc::prettify(&f.id));
        nodes.push(Node { f: f.clone(), x, y, editable: k.is_some(), key: k, name, dlc_hidden: hidden.contains(&f.id) });
    }
    let idx = nodes.iter().enumerate().map(|(i, n)| (n.f.id.clone(), i)).collect();
    let bounds = if nodes.is_empty() {
        (0, 0, 0, 0)
    } else {
        (nodes.iter().map(|n| n.x).min().unwrap(), nodes.iter().map(|n| n.y).min().unwrap(), nodes.iter().map(|n| n.x).max().unwrap(), nodes.iter().map(|n| n.y).max().unwrap())
    };
    Some(Model { key: (cx.st.generation, key.clone(), owned.clone()), tree, nodes, idx, issues, bounds, dlcs })
}

// ---------------------------------------------------------------- edits

/// Edits of focus blocks, file by file, as one undo step. `f` gets the
/// file's text, the focus's block and the focus.
fn edit_focuses(cx: &mut Cx, m: &Model, label: &str, ids: &[String], mut f: impl FnMut(&str, &script::Block, &Node) -> Vec<Edit>) -> bool {
    let mut by_file: BTreeMap<(String, String), Vec<&Node>> = BTreeMap::new();
    for id in ids {
        if let Some(&i) = m.idx.get(id)
            && let Some(k) = &m.nodes[i].key
        {
            by_file.entry((k.pkg.clone(), k.file.clone())).or_default().push(&m.nodes[i]);
        }
    }
    let mut changes = Vec::new();
    for ((pkg, file), nodes) in by_file {
        let Some(sf) = cx.st.file(&pkg, &file) else { continue };
        let (Some(text), Some(s)) = (sf.text(), sf.script.as_ref()) else { continue };
        let mut edits = Vec::new();
        for n in nodes {
            if let Some((_, b)) = focus::find(s, &n.f.id) {
                edits.extend(f(text, b, n));
            }
        }
        if !edits.is_empty() {
            changes.push((pkg, file, script::apply(text, &edits)));
        }
    }
    if changes.is_empty() {
        return false;
    }
    cx.st.begin(label.to_string());
    for (p, f, t) in changes {
        cx.st.set_text(&p, &f, t);
    }
    cx.st.commit();
    true
}

/// Focus blocks added at the end of the tree (one undo step, unless `open` is set).
fn add_to_tree(cx: &mut Cx, m: &Model, label: &str, blocks: &[String]) -> bool {
    let k = &m.key.1;
    let Some(sf) = cx.st.file(&k.pkg, &k.file) else { return false };
    let (Some(text), Some(s)) = (sf.text(), sf.script.as_ref()) else { return false };
    let Some((_, tb)) = focus::find_tree(s, &m.tree.id) else { return false };
    let last = tb.items.iter().rposition(|i| i.is("focus"));
    let joined = blocks.join("\n\n");
    let e = script::insert_item(text, tb, &joined, last);
    let new = script::apply(text, &[e]);
    cx.st.begin(label.to_string());
    cx.st.set_text(&k.pkg, &k.file, new);
    cx.st.commit();
    true
}

/// A new focus id like `TAG_focus_n` not used anywhere.
fn fresh_id(cx: &Cx, m: &Model, base: &str) -> String {
    let taken: BTreeSet<String> = cx.st.defs().filter(|k| matches!(k.kind, Kind::Focus | Kind::SharedFocus)).map(|k| k.id).collect();
    let used = |id: &str| taken.contains(id) || m.idx.contains_key(id) || cx.base.is_some_and(|b| b.has(Kind::Focus, id) || b.has(Kind::SharedFocus, id));
    if !used(base) {
        return base.to_string();
    }
    (2..).map(|n| format!("{base}_{n}")).find(|x| !used(x)).unwrap()
}

/// Move focuses by a grid step (relative ones stay relative to their anchor).
fn move_by(cx: &mut Cx, m: &Model, ids: &[String], dx: i32, dy: i32) {
    if dx == 0 && dy == 0 {
        return;
    }
    let set: BTreeSet<&str> = ids.iter().map(String::as_str).collect();
    let label = if ids.len() == 1 { format!("move {}", ids[0]) } else { format!("move {} focuses", ids.len()) };
    edit_focuses(cx, m, &label, ids, |t, b, n| {
        if n.f.rel.as_deref().is_some_and(|r| set.contains(r)) {
            return Vec::new();
        }
        focus::set_xy(t, b, n.f.x + dx, n.f.y + dy)
    });
}

/// Delete focuses: what names them in the tree is taken out (prerequisites,
/// exclusions, relative positions), then they go with their texts.
fn delete(cx: &mut Cx, m: &Model, ids: &[String]) {
    let gone: BTreeSet<&str> = ids.iter().map(String::as_str).collect();
    let keys: Vec<DefKey> = ids.iter().filter_map(|id| m.idx.get(id).and_then(|&i| m.nodes[i].key.clone())).collect();
    if keys.is_empty() {
        cx.error("these focuses are not in the project (the base game's shared focuses cannot be deleted here)");
        return;
    }
    let others: Vec<String> = m.nodes.iter().filter(|n| !gone.contains(n.f.id.as_str()) && n.editable).filter(|n| n.f.prereq.iter().flatten().chain(&n.f.mutex).chain(n.f.rel.iter()).any(|r| gone.contains(r.as_str()))).map(|n| n.f.id.clone()).collect();
    let label = if ids.len() == 1 { format!("delete {}", ids[0]) } else { format!("delete {} focuses", ids.len()) };
    let anchors: HashMap<&str, (i32, i32)> = m.nodes.iter().map(|n| (n.f.id.as_str(), (n.x, n.y))).collect();
    // one undo step for both parts
    cx.st.begin(label.clone());
    let mut changes = Vec::new();
    {
        let mut by_file: BTreeMap<(String, String), Vec<&Node>> = BTreeMap::new();
        for id in &others {
            let n = &m.nodes[m.idx[id]];
            if let Some(k) = &n.key {
                by_file.entry((k.pkg.clone(), k.file.clone())).or_default().push(n);
            }
        }
        for ((pkg, file), nodes) in by_file {
            let Some(sf) = cx.st.file(&pkg, &file) else { continue };
            let (Some(text), Some(s)) = (sf.text(), sf.script.as_ref()) else { continue };
            let mut edits = Vec::new();
            for n in nodes {
                if let Some((_, b)) = focus::find(s, &n.f.id) {
                    for g in &gone {
                        let refs = n.f.prereq.iter().flatten().chain(&n.f.mutex).chain(n.f.rel.iter()).any(|r| r == g);
                        if refs {
                            edits.extend(focus::forget(text, b, g, anchors.get(g).copied().unwrap_or((0, 0))));
                        }
                    }
                }
            }
            // several forgets of one block may touch the same bytes: keep the first of overlapping ones
            edits.sort_by_key(|e| e.range.start);
            let mut kept: Vec<Edit> = Vec::new();
            for e in edits {
                if kept.last().is_none_or(|l| l.range.end <= e.range.start || (l.range == e.range && l.text == e.text && false)) {
                    if kept.last().is_some_and(|l| l.range == e.range) {
                        continue;
                    }
                    kept.push(e);
                }
            }
            changes.push((pkg, file, script::apply(text, &kept)));
        }
    }
    for (p, f, t) in changes {
        cx.st.set_text(&p, &f, t);
    }
    let keys: Vec<DefKey> = keys.into_iter().filter_map(|k| cx.st.find_in(&k.pkg, &[k.kind], &k.id)).collect();
    let plan = refactor::plan_delete(cx.st, &keys);
    refactor::apply(cx.st, &plan);
    cx.st.commit();
    let left = plan.dangling.len();
    cx.info(if left > 0 { format!("{label}; {left} places outside the tree still name them (Check lists them)") } else { label });
}

/// Copies of focuses (renamed by `rename`, moved by dx, dy) added to the
/// tree, with copies of their texts.
fn copy_focuses(cx: &mut Cx, m: &Model, ids: &[String], rename: &BTreeMap<String, String>, dx: i32, dy: i32, label: &str) -> usize {
    let k = &m.key.1;
    let own: Vec<String> = ids.iter().filter(|id| m.idx.get(*id).is_some_and(|&i| !m.nodes[i].f.shared && m.nodes[i].editable)).cloned().collect();
    let Some(sf) = cx.st.file(&k.pkg, &k.file) else { return 0 };
    let (Some(text), Some(s)) = (sf.text(), sf.script.as_ref()) else { return 0 };
    let blocks = focus::copy_blocks(text, s, &own, rename, dx, dy);
    if blocks.is_empty() {
        return 0;
    }
    let n = blocks.len();
    // texts of the copies
    let lang = cx.settings.language.clone();
    let mut texts = Vec::new();
    for (old, new) in rename {
        for (a, b) in [(old.clone(), new.clone()), (format!("{old}_desc"), format!("{new}_desc"))] {
            if let Some(t) = cx.loc.own(&a).map(|x| x.2.clone()).or_else(|| cx.base.and_then(|b| b.loc.get(&a)).cloned()) {
                texts.push((b, t));
            }
        }
    }
    let ok = add_to_tree(cx, m, label, &blocks);
    if ok && !texts.is_empty() {
        let pkg = k.pkg.clone();
        cx.st.begin_merge(label.to_string(), None);
        import::set_locs(cx.st, &pkg, &lang, &texts);
        cx.st.commit();
    }
    n
}

// ---------------------------------------------------------------- the page

pub fn page(ui: &mut egui::Ui, cx: &mut Cx, ts: &mut TreeState, form_state: &mut FormState) {
    let pkg = cx.pkg.to_string();
    let trees = cx.st.defs_of(Some(&pkg), Kind::FocusTree);
    // the tree shown: the one asked for, else the first of the package
    let want = ts.tree.as_ref().filter(|(p, _)| p.is_empty() || *p == pkg).map(|x| x.1.clone());
    let tree_key = want.and_then(|t| trees.iter().find(|k| k.id == t).cloned()).or_else(|| trees.first().cloned());
    let same = match (&tree_key, &ts.tree) {
        (Some(k), Some((p, t))) => k.id == *t && (p.is_empty() || *p == k.pkg),
        (None, None) => true,
        _ => false,
    };
    if !same {
        ts.fit = true;
        ts.gentle = true;
        ts.sel.clear();
        ts.primary = None;
        ts.sim = None;
        ts.model = None;
    }
    ts.tree = tree_key.as_ref().map(|k| (k.pkg.clone(), k.id.clone()));
    if ts.zoom == 0.0 {
        ts.zoom = 0.7;
        ts.fit = true;
        ts.gentle = true;
        ts.minimap = true;
        ts.issues_open = true;
    }

    // toolbar
    ui.horizontal_wrapped(|ui| {
        ui.heading("Focus trees");
        let mut pick = None;
        egui::ComboBox::from_id_salt("tree_pick").selected_text(tree_key.as_ref().map(|k| k.id.clone()).unwrap_or_else(|| "(none)".into())).show_ui(ui, |ui| {
            for k in &trees {
                if ui.selectable_label(tree_key.as_ref() == Some(k), &k.id).clicked() {
                    pick = Some(k.id.clone());
                }
            }
        });
        if let Some(p) = pick {
            ts.tree = Some((pkg.clone(), p));
            ts.fit = true;
            ts.model = None;
        }
        if ui.button("+ New tree").clicked() {
            ts.new_tree = Some(format!("{}_focus_tree", cx.tag().to_ascii_lowercase()));
        }
        if ui.button("From draw.io…").on_hover_text("A tree planned in draw.io / diagrams.net: boxes are focuses, arrows prerequisites").clicked()
            && let Some(p) = rfd::FileDialog::new().add_filter("draw.io", &["drawio", "xml"]).pick_file()
        {
            match std::fs::read_to_string(&p).map_err(anyhow::Error::from).and_then(|t| drawio::pages(&t)) {
                Ok(pages) => {
                    let mut plan = DrawioPlan { path: p, pages, page: 0, flip: false, tree_id: format!("{}_drawn_tree", cx.tag().to_ascii_lowercase()), focuses: Vec::new() };
                    replan_drawio(cx, &mut plan);
                    ts.drawio = Some(plan);
                }
                Err(e) => cx.error(format!("{e:#}")),
            }
        }
    });
    new_tree_ui(ui, cx, ts);
    drawio_ui(ui, cx, ts);
    let Some(tree_key) = tree_key else {
        ui.add_space(20.0);
        ui.label(format!("{} has no focus tree yet: make one (+ New tree), import one (📥 Import: the base game's trees, a mod's), or draw it in draw.io.", if pkg == crate::content::store::SHARED { "shared" } else { &pkg }));
        return;
    };

    // the model
    let bs = base_shared(ts, cx);
    let fresh = ts.model.as_ref().is_some_and(|m| m.key == (cx.st.generation, tree_key.clone(), cx.settings.dlcs.clone()));
    if !fresh {
        ts.model = build(cx, &tree_key, &bs, &cx.settings.dlcs).map(Arc::new);
    }
    let Some(m) = ts.model.clone() else {
        ui.label(RichText::new("The tree cannot be read (see Check).").color(ERR));
        return;
    };
    ts.sel.retain(|s| m.idx.contains_key(s));
    if ts.primary.as_ref().is_some_and(|p| !m.idx.contains_key(p)) {
        ts.primary = ts.sel.iter().next().cloned();
    }

    toolbar(ui, cx, ts, &m);
    copy_shift_ui(ui, cx, ts, &m);

    // panels: outline, inspector, issues
    if ts.outline {
        egui::Panel::left("tree_outline").default_size(220.0).max_size(ui.available_width() * 0.25).show(ui, |ui| super::contained(ui, |ui| outline(ui, ts, &m)));
    }
    let w = ui.available_width();
    egui::Panel::right("tree_inspector").default_size(420.0_f32.min(w * 0.4)).min_size(260.0_f32.min(w * 0.3)).max_size(w * 0.55).show(ui, |ui| {
        super::contained(ui, |ui| {
            egui::ScrollArea::vertical().id_salt("tree_inspector_scroll").auto_shrink([false, false]).show(ui, |ui| inspector(ui, cx, ts, &m, &tree_key, form_state));
        });
    });
    let (ne, nw) = (m.issues.iter().filter(|i| i.error).count(), m.issues.iter().filter(|i| !i.error).count());
    egui::Panel::bottom("tree_issues").resizable(true).default_size(if ts.issues_open && !m.issues.is_empty() { 110.0 } else { 26.0 }).show(ui, |ui| {
        ui.horizontal(|ui| {
            let t = if m.issues.is_empty() { RichText::new("✔ the tree has no problems").color(OK) } else { RichText::new(format!("{ne} errors, {nw} warnings in the tree")).color(if ne > 0 { ERR } else { WARN }) };
            if ui.selectable_label(ts.issues_open, t).clicked() {
                ts.issues_open ^= true;
            }
            ui.label(RichText::new(format!("{} focuses", m.nodes.len())).weak());
            ui.label(RichText::new(ts.mode.hint()).weak().small());
        });
        if ts.issues_open {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                for i in &m.issues {
                    let r = ui.link(RichText::new(&i.message).color(if i.error { ERR } else { WARN }).small());
                    if r.clicked() && m.idx.contains_key(&i.focus) {
                        ts.sel = [i.focus.clone()].into();
                        ts.primary = Some(i.focus.clone());
                        ts.center_on = Some(i.focus.clone());
                    }
                }
            });
        }
    });
    egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| canvas(ui, cx, ts, &m));
}

fn toolbar(ui: &mut egui::Ui, cx: &mut Cx, ts: &mut TreeState, m: &Arc<Model>) {
    ui.horizontal_wrapped(|ui| {
        for (mode, label, key) in [(Mode::Select, "⬚ Select", "S"), (Mode::Link, "🔗 Link", "L"), (Mode::Exclusive, "⊘ Exclusive", "E"), (Mode::Add, "+ Add", "A"), (Mode::Pan, "✋ Pan", "P")] {
            if ui.selectable_label(ts.mode == mode, label).on_hover_text(format!("{} ({key})", mode.hint())).clicked() {
                ts.mode = mode;
                ts.link_from = None;
            }
        }
        ui.separator();
        if ui.button("Fit").on_hover_text("The whole tree (F)").clicked() {
            ts.fit = true;
        }
        if ui.button("Tidy").on_hover_text("Lay the tree out from its prerequisites (rows by depth, close to the parents)").clicked() {
            ts.tidy_ask = true;
        }
        let sim_on = ts.sim.is_some();
        if ui.selectable_label(sim_on, "▶ Play").on_hover_text("Take focuses as the game would let you: gold ones can be taken, click to take them").clicked() {
            ts.sim = if sim_on { None } else { Some(BTreeSet::new()) };
        }
        if let Some(done) = &mut ts.sim {
            let days: f64 = done.iter().filter_map(|id| m.idx.get(id).and_then(|&i| m.nodes[i].f.cost)).sum::<f64>() * 7.0;
            ui.label(RichText::new(format!("{} taken, {days:.0} days", done.len())).color(OK));
            if ui.small_button("start again").clicked() {
                done.clear();
            }
        }
        ui.separator();
        ui.menu_button(if cx.settings.dlcs.is_some() { "DLC ▾ (filtered)" } else { "DLC ▾" }, |ui| {
            if m.dlcs.is_empty() {
                ui.label("No focus of this tree asks for a DLC.");
                return;
            }
            let mut all = cx.settings.dlcs.is_none();
            if ui.checkbox(&mut all, "all DLCs").changed() {
                cx.settings.dlcs = if all { None } else { Some(m.dlcs.clone()) };
            }
            if let Some(owned) = &mut cx.settings.dlcs {
                for d in &m.dlcs {
                    let mut on = owned.contains(d);
                    if ui.checkbox(&mut on, d).changed() {
                        if on {
                            owned.insert(d.clone());
                        } else {
                            owned.remove(d);
                        }
                    }
                }
                ui.checkbox(&mut ts.show_hidden, "show the hidden ones faintly");
            }
        });
        ui.checkbox(&mut ts.outline, "outline");
        ui.checkbox(&mut ts.minimap, "minimap");
        ui.separator();
        ui.label("🔍");
        let r = ui.add(egui::TextEdit::singleline(&mut ts.find).desired_width(140.0).hint_text("find (Ctrl+F)"));
        if ui.input_mut(|i| i.consume_shortcut(&egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::F))) {
            r.request_focus();
        }
        if r.changed() {
            let q = ts.find.to_lowercase();
            ts.hits = if q.is_empty() { Vec::new() } else { m.nodes.iter().filter(|n| n.f.id.to_lowercase().contains(&q) || n.name.to_lowercase().contains(&q)).map(|n| n.f.id.clone()).collect() };
            ts.hit_at = 0;
            if let Some(h) = ts.hits.first() {
                ts.center_on = Some(h.clone());
            }
        }
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if !ts.hits.is_empty() {
            ui.label(format!("{}/{}", ts.hit_at + 1, ts.hits.len()));
            if ui.small_button("next").clicked() || enter {
                ts.hit_at = (ts.hit_at + 1) % ts.hits.len();
                let h = ts.hits[ts.hit_at].clone();
                ts.sel = [h.clone()].into();
                ts.primary = Some(h.clone());
                ts.center_on = Some(h);
                if enter {
                    r.request_focus();
                }
            }
        } else if !ts.find.is_empty() {
            ui.label(RichText::new("no match").weak());
        }
        ui.separator();
        if ui.button("PNG…").on_hover_text("The whole tree as a picture").clicked()
            && let Some(p) = rfd::FileDialog::new().set_file_name(format!("{}.png", m.tree.id)).add_filter("PNG", &["png"]).save_file()
        {
            start_export(ts, m, p, ui.ctx().pixels_per_point());
        }
    });
    if ts.tidy_ask {
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("Tidy writes new positions for the {} focuses of the tree (relative positions become absolute). Ctrl+Z takes it back.", m.nodes.iter().filter(|n| n.editable).count())).color(WARN));
            if ui.button("Tidy").clicked() {
                tidy(cx, m);
                ts.tidy_ask = false;
                ts.fit = true;
            }
            if ui.button("Cancel").clicked() {
                ts.tidy_ask = false;
            }
        });
    }
}

fn tidy(cx: &mut Cx, m: &Model) {
    let fs: Vec<Focus> = m.nodes.iter().map(|n| n.f.clone()).collect();
    let lay = focus::auto_layout(&fs, 2);
    let ids: Vec<String> = m.nodes.iter().filter(|n| n.editable).map(|n| n.f.id.clone()).collect();
    edit_focuses(cx, m, "tidy the tree", &ids, |t, b, n| {
        let Some(&(x, y)) = lay.get(&n.f.id) else { return Vec::new() };
        let mut e = Vec::new();
        if let Some(r) = b.get("relative_position_id") {
            e.push(script::remove_item(t, r));
        }
        e.extend(focus::set_xy(t, b, x, y));
        e
    });
}

fn start_export(ts: &mut TreeState, m: &Model, path: PathBuf, ppp: f32) {
    let (x0, y0, x1, y1) = m.bounds;
    let margin = 30.0;
    let origin = vec2(x0 as f32 * CW - margin, y0 as f32 * CH - margin);
    let size = vec2((x1 - x0 + 1) as f32 * CW + 2.0 * margin, (y1 - y0 + 1) as f32 * CH + 2.0 * margin);
    // at most 16000 pixels a side
    let zoom = (16000.0 / (size.x.max(size.y) * ppp)).min(1.0);
    let k = zoom * ppp;
    // a tile is the canvas, in world units
    let (tw, th) = (ts.rect.0.width().floor().max(64.0) / zoom, ts.rect.0.height().floor().max(64.0) / zoom);
    let mut tiles = Vec::new();
    let mut y = 0.0;
    while y < size.y {
        let mut x = 0.0;
        while x < size.x {
            tiles.push((vec2(x, y), ((x * k).round() as u32, (y * k).round() as u32)));
            x += tw;
        }
        y += th;
    }
    let img = image::RgbaImage::new((size.x * k).ceil() as u32, (size.y * k).ceil() as u32);
    let view = (ts.zoom, ts.off);
    ts.export = Some(Export { path, tiles, at: 0, wait: 0, requested: false, shot: Arc::new(Mutex::new(None)), img, origin, zoom, view });
}

fn new_tree_ui(ui: &mut egui::Ui, cx: &mut Cx, ts: &mut TreeState) {
    let Some(id) = &mut ts.new_tree else { return };
    let mut close = false;
    ui.horizontal(|ui| {
        ui.label("id of the new tree");
        ui.add(egui::TextEdit::singleline(id).desired_width(240.0));
        if ui.button("Make it").clicked() {
            let id = id.trim().to_string();
            if cx.st.defs().any(|k| k.kind == Kind::FocusTree && k.id == id) || cx.base.is_some_and(|b| b.has(Kind::FocusTree, &id)) {
                cx.error(format!("there is a tree {id} already"));
            } else if let Some(k) = cx.create(Kind::FocusTree, Some(&id), &id) {
                ts.tree = Some((k.pkg.clone(), k.id.clone()));
                ts.fit = true;
                close = true;
            }
        }
        if ui.button("Cancel").clicked() {
            close = true;
        }
    });
    if close {
        ts.new_tree = None;
    }
}

fn replan_drawio(cx: &Cx, p: &mut DrawioPlan) {
    let taken: BTreeSet<String> = cx.st.defs().map(|k| k.id).collect();
    let (boxes, arrows) = drawio::parse_page(&p.pages[p.page].1);
    p.focuses = drawio::focuses(&boxes, &arrows, cx.tag(), p.flip, &taken);
}

fn drawio_ui(ui: &mut egui::Ui, cx: &mut Cx, ts: &mut TreeState) {
    let Some(p) = &mut ts.drawio else { return };
    let mut close = false;
    let mut replan = false;
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.label(RichText::new(format!("From {}", p.path.display())).strong());
        ui.horizontal(|ui| {
            if p.pages.len() > 1 {
                egui::ComboBox::from_id_salt("drawio_page").selected_text(p.pages[p.page].0.clone()).show_ui(ui, |ui| {
                    for (i, (n, _)) in p.pages.iter().enumerate() {
                        replan |= ui.selectable_value(&mut p.page, i, n).changed();
                    }
                });
            }
            replan |= ui.checkbox(&mut p.flip, "arrows point to the prerequisite").changed();
            ui.label("tree id");
            ui.add(egui::TextEdit::singleline(&mut p.tree_id).desired_width(200.0));
        });
        let links: usize = p.focuses.iter().map(|f| f.prereq.len()).sum();
        ui.label(format!("{} focuses, {links} prerequisites", p.focuses.len()));
        ui.horizontal(|ui| {
            if ui.add_enabled(!p.focuses.is_empty(), egui::Button::new("Make the tree")).clicked() {
                let tag = if cx.pkg == crate::content::store::SHARED { "MOD".to_string() } else { cx.pkg.to_string() };
                let text = drawio::tree_text(p.tree_id.trim(), &tag, &p.focuses);
                let pkg = cx.pkg.to_string();
                let lang = cx.settings.language.clone();
                cx.st.begin(format!("tree {} from draw.io", p.tree_id.trim()));
                let r = import::insert_def(cx.st, &pkg, Kind::FocusTree, Some(p.tree_id.trim()), p.tree_id.trim(), &text);
                match r {
                    Ok(()) => {
                        let texts: Vec<(String, String)> = p.focuses.iter().map(|f| (f.id.clone(), f.label.clone())).collect();
                        import::set_locs(cx.st, &pkg, &lang, &texts);
                        cx.st.commit();
                        ts.tree = Some((pkg, p.tree_id.trim().to_string()));
                        ts.fit = true;
                        close = true;
                    }
                    Err(e) => {
                        cx.st.rollback();
                        cx.error(format!("{e:#}"));
                    }
                }
            }
            if ui.button("Cancel").clicked() {
                close = true;
            }
        });
    });
    if replan && let Some(p) = &mut ts.drawio {
        replan_drawio(cx, p);
    }
    if close {
        ts.drawio = None;
    }
}

fn copy_shift_ui(ui: &mut egui::Ui, cx: &mut Cx, ts: &mut TreeState, m: &Arc<Model>) {
    if let Some((root, from, to, dx, dy)) = &mut ts.copy {
        let mut close = false;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            let branch = m.tree.branch(root);
            ui.label(RichText::new(format!("Copy the branch of {root} ({} focuses)", branch.len())).strong());
            ui.horizontal(|ui| {
                ui.label("ids: replace");
                ui.add(egui::TextEdit::singleline(from).desired_width(120.0));
                ui.label("with");
                ui.add(egui::TextEdit::singleline(to).desired_width(120.0));
                ui.label("move by");
                ui.add(egui::DragValue::new(dx).prefix("x "));
                ui.add(egui::DragValue::new(dy).prefix("y "));
            });
            let rename: BTreeMap<String, String> = branch
                .iter()
                .map(|id| {
                    let n = if !from.is_empty() && id.contains(from.as_str()) { id.replacen(from.as_str(), to, 1) } else { format!("{id}{to}") };
                    (id.clone(), n)
                })
                .collect();
            let clash: Vec<&String> = rename.values().filter(|n| m.idx.contains_key(*n) || cx.st.defs().any(|k| k.id == **n)).collect();
            if let Some((a, b)) = rename.iter().next() {
                ui.label(RichText::new(format!("{a} → {b} …")).monospace().small());
            }
            if !clash.is_empty() {
                ui.label(RichText::new(format!("{} new ids are taken: {}", clash.len(), clash.iter().take(4).map(|s| s.as_str()).collect::<Vec<_>>().join(", "))).color(ERR));
            }
            ui.horizontal(|ui| {
                if ui.add_enabled(clash.is_empty(), egui::Button::new("Copy")).clicked() {
                    let n = copy_focuses(cx, m, &branch, &rename, *dx, *dy, &format!("copy the branch of {root}"));
                    cx.info(format!("copied {n} focuses"));
                    ts.sel = rename.values().cloned().collect();
                    close = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        if close {
            ts.copy = None;
        }
    }
    if let Some((root, dx, dy)) = &mut ts.shift {
        let mut close = false;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            let branch = m.tree.branch(root);
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("Move the branch of {root} ({} focuses) by", branch.len())).strong());
                ui.add(egui::DragValue::new(dx).prefix("x "));
                ui.add(egui::DragValue::new(dy).prefix("y "));
                if ui.button("Move").clicked() {
                    move_by(cx, m, &branch, *dx, *dy);
                    close = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        if close {
            ts.shift = None;
        }
    }
}

fn outline(ui: &mut egui::Ui, ts: &mut TreeState, m: &Model) {
    ui.add(egui::TextEdit::singleline(&mut ts.outline_filter).hint_text("filter").desired_width(f32::INFINITY));
    let f = ts.outline_filter.to_lowercase();
    let mut order: Vec<&Node> = m.nodes.iter().collect();
    order.sort_by_key(|n| (n.y, n.x));
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        let mut row = i32::MIN;
        for n in order {
            if !f.is_empty() && !n.f.id.to_lowercase().contains(&f) && !n.name.to_lowercase().contains(&f) {
                continue;
            }
            if n.y != row {
                ui.label(RichText::new(format!("row {}", n.y)).small().weak());
                row = n.y;
            }
            let r = ui.selectable_label(ts.sel.contains(&n.f.id), &n.name).on_hover_text(&n.f.id);
            if r.clicked() {
                ts.sel = [n.f.id.clone()].into();
                ts.primary = Some(n.f.id.clone());
                ts.center_on = Some(n.f.id.clone());
            }
        }
    });
}

// ---------------------------------------------------------------- the canvas

fn canvas(ui: &mut egui::Ui, cx: &mut Cx, ts: &mut TreeState, m: &Arc<Model>) {
    let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
    ts.rect.0 = rect;
    let painter = ui.painter_at(rect);
    let dark = ui.visuals().dark_mode;
    painter.rect_filled(rect, 0.0, if dark { Color32::from_rgb(28, 30, 33) } else { Color32::from_rgb(232, 228, 218) });
    let exporting = ts.export.is_some();
    if let Some(e) = &ts.export {
        ts.zoom = e.zoom;
        let (o, _) = e.tiles[e.at];
        ts.off = -(e.origin + o) * e.zoom;
    } else {
        if ts.fit && ts.rect.0.width() > 10.0 {
            ts.fit_to(m, ts.gentle);
            ts.fit = false;
            ts.gentle = false;
        }
        if let Some(c) = ts.center_on.take()
            && let Some(&i) = m.idx.get(&c)
        {
            ts.center(&m.nodes[i]);
        }
    }

    // input
    let ptr = resp.hover_pos();
    let mods = ui.input(|i| i.modifiers);
    if !exporting && resp.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y + i.zoom_delta().ln() * 200.0);
        if scroll != 0.0
            && let Some(p) = ptr
        {
            let old = ts.zoom;
            ts.zoom = (ts.zoom * (scroll / 400.0).exp()).clamp(0.08, 2.5);
            let k = ts.zoom / old;
            ts.off = (p - rect.min) - ((p - rect.min) - ts.off) * k;
        }
    }
    let hit = |ts: &TreeState, p: Pos2| -> Option<String> {
        m.nodes.iter().rev().filter(|n| !n.dlc_hidden || ts.show_hidden).find(|n| ts.card(n, Vec2::ZERO).contains(p)).map(|n| n.f.id.clone())
    };
    let middle = ui.input(|i| i.pointer.middle_down());
    let pan_drag = ts.mode == Mode::Pan || middle || mods.alt;
    if resp.drag_started() {
        let p = ui.input(|i| i.pointer.press_origin()).unwrap_or_default();
        if pan_drag {
            ts.pan = Some(p);
        } else if ts.mode == Mode::Select {
            match hit(ts, p) {
                Some(id) => {
                    if !ts.sel.contains(&id) {
                        if !(mods.shift || mods.command) {
                            ts.sel.clear();
                        }
                        ts.sel.insert(id.clone());
                        ts.primary = Some(id.clone());
                    }
                    let ids: Vec<String> = ts.sel.iter().filter(|s| m.idx.get(*s).is_some_and(|&i| m.nodes[i].editable)).cloned().collect();
                    if ts.sim.is_none() {
                        ts.drag = Some(Drag { start: p, ids, moved: false });
                    }
                }
                None => ts.rubber = Some((p, p)),
            }
        }
    }
    if resp.dragged() {
        let p = ptr.or(ui.input(|i| i.pointer.interact_pos())).unwrap_or_default();
        if ts.pan.is_some() {
            ts.off += resp.drag_delta();
        } else if let Some(d) = &mut ts.drag {
            if (p - d.start).length() > 4.0 {
                d.moved = true;
            }
        } else if let Some((a, _)) = ts.rubber {
            ts.rubber = Some((a, p));
        }
    }
    let drag_cells = |ts: &TreeState, d: &Drag, p: Pos2| -> (i32, i32) {
        let dw = (p - d.start) / ts.zoom;
        ((dw.x / CW).round() as i32, (dw.y / CH).round() as i32)
    };
    if resp.drag_stopped() {
        let p = ptr.or(ui.input(|i| i.pointer.interact_pos())).unwrap_or_default();
        if let Some(d) = ts.drag.take()
            && d.moved
        {
            let (dx, dy) = drag_cells(ts, &d, p);
            move_by(cx, m, &d.ids, dx, dy);
        }
        if let Some((a, b)) = ts.rubber.take() {
            let r = Rect::from_two_pos(a, b);
            if !(mods.shift || mods.command) {
                ts.sel.clear();
            }
            for n in &m.nodes {
                if r.intersects(ts.card(n, Vec2::ZERO)) && (!n.dlc_hidden || ts.show_hidden) {
                    ts.sel.insert(n.f.id.clone());
                }
            }
            ts.primary = ts.sel.iter().next().cloned();
        }
        ts.pan = None;
    }
    if resp.clicked()
        && let Some(p) = ptr
    {
        resp.request_focus();
        let h = hit(ts, p);
        click(cx, ts, m, h, p, mods);
    }
    if resp.double_clicked()
        && let Some(id) = ptr.and_then(|p| hit(ts, p))
        && ts.sim.is_none()
    {
        ts.sel = [id.clone()].into();
        ts.primary = Some(id);
    }
    if resp.secondary_clicked() {
        ts.context = ptr.and_then(|p| hit(ts, p));
        if let Some(c) = &ts.context
            && !ts.sel.contains(c)
        {
            ts.sel = [c.clone()].into();
            ts.primary = Some(c.clone());
        }
    }
    resp.context_menu(|ui| context_menu(ui, cx, ts, m));
    if !exporting {
        keys(ui, cx, ts, m, resp.hovered() || resp.has_focus());
    }

    // drawing
    let z = ts.zoom;
    let states = ts.sim.as_ref().map(|done| focus::states(&m.tree.focuses, done));
    let drag_d = match (&ts.drag, ptr) {
        (Some(d), Some(p)) if d.moved => {
            let (dx, dy) = drag_cells(ts, d, p);
            Some((d.ids.iter().cloned().collect::<BTreeSet<String>>(), vec2(dx as f32 * CW, dy as f32 * CH)))
        }
        _ => None,
    };
    let shown = |n: &Node| !n.dlc_hidden || ts.show_hidden;
    let off_of = |id: &str| -> Vec2 {
        match &drag_d {
            Some((ids, d)) if ids.contains(id) => *d,
            _ => Vec2::ZERO,
        }
    };
    // grid dots when close
    if z > 0.45 && !exporting {
        let c = if dark { Color32::from_gray(48) } else { Color32::from_gray(205) };
        let w0 = ts.to_world(rect.min);
        let w1 = ts.to_world(rect.max);
        let (gx0, gx1) = ((w0.x / CW).floor() as i32, (w1.x / CW).ceil() as i32);
        let (gy0, gy1) = ((w0.y / CH).floor() as i32, (w1.y / CH).ceil() as i32);
        if (gx1 - gx0) * (gy1 - gy0) < 6000 {
            for gx in gx0..=gx1 {
                for gy in gy0..=gy1 {
                    painter.circle_filled(ts.to_screen(pos2(gx as f32 * CW + CW / 2.0, gy as f32 * CH + (ICON_H + PLATE_H) / 2.0)), 1.5, c);
                }
            }
        }
    }
    // where the game shows the continuous focuses (pixels, like the cells)
    if let Some((x, y)) = m.tree.continuous {
        let r = Rect::from_min_size(ts.to_screen(pos2(x as f32, y as f32)), vec2(4.0 * CW, 1.6 * CH) * z);
        let c = if dark { Color32::from_rgb(120, 140, 170) } else { Color32::from_rgb(70, 90, 130) };
        painter.rect_stroke(r, 4.0, Stroke::new(1.0, c), egui::StrokeKind::Inside);
        if z > 0.3 {
            painter.text(r.min + vec2(6.0, 4.0), Align2::LEFT_TOP, "continuous focuses", FontId::proportional(11.0 * z.max(0.7)), c);
        }
    }
    // prerequisite lines
    let line_col = if dark { Color32::from_rgb(200, 190, 160) } else { Color32::from_rgb(90, 80, 60) };
    for n in &m.nodes {
        if !shown(n) {
            continue;
        }
        let child = ts.card(n, off_of(&n.f.id));
        for g in &n.f.prereq {
            let dashed = g.len() > 1;
            for p in g {
                let Some(&pi) = m.idx.get(p) else { continue };
                let pn = &m.nodes[pi];
                if !shown(pn) {
                    continue;
                }
                let par = ts.card(pn, off_of(p));
                let a = pos2(par.center().x, par.max.y);
                let b = pos2(child.center().x, child.min.y);
                let mid = b.y - 14.0 * z;
                let pts = [a, pos2(a.x, mid.max(a.y)), pos2(b.x, mid.max(a.y)), b];
                let st = Stroke::new((2.0 * z).max(1.0), line_col);
                if dashed {
                    painter.add(egui::Shape::dashed_line(&pts, st, 6.0 * z.max(0.3), 4.0 * z.max(0.3)));
                } else {
                    painter.add(egui::Shape::line(pts.to_vec(), st));
                }
            }
        }
    }
    // exclusive pairs
    let mut drawn: BTreeSet<(String, String)> = BTreeSet::new();
    for n in &m.nodes {
        for o in &n.f.mutex {
            let Some(&oi) = m.idx.get(o) else { continue };
            let on = &m.nodes[oi];
            if !shown(n) || !shown(on) {
                continue;
            }
            let k = if n.f.id < *o { (n.f.id.clone(), o.clone()) } else { (o.clone(), n.f.id.clone()) };
            if !drawn.insert(k) {
                continue;
            }
            let (ra, rb) = (ts.card(n, off_of(&n.f.id)), ts.card(on, off_of(o)));
            let ya = ra.min.y + ICON_H * 0.5 * z;
            let (a, b) = if ra.center().x < rb.center().x { (pos2(ra.max.x, ya), pos2(rb.min.x, rb.min.y + ICON_H * 0.5 * z)) } else { (pos2(rb.max.x, rb.min.y + ICON_H * 0.5 * z), pos2(ra.min.x, ya)) };
            painter.line_segment([a, b], Stroke::new((2.0 * z).max(1.0), Color32::from_rgb(210, 60, 50)));
            let c = a + (b - a) / 2.0;
            painter.circle(c, (8.0 * z).max(3.0), Color32::from_rgb(120, 20, 20), Stroke::new(1.5, Color32::from_rgb(240, 90, 80)));
            if z > 0.4 {
                painter.text(c, Align2::CENTER_CENTER, "!", FontId::proportional(11.0 * z), Color32::WHITE);
            }
        }
    }
    // the focuses
    let issue_ids: BTreeMap<&str, bool> = m.issues.iter().fold(BTreeMap::new(), |mut acc, i| {
        let e = acc.entry(i.focus.as_str()).or_insert(false);
        *e |= i.error;
        acc
    });
    let hits: BTreeSet<&String> = ts.hits.iter().collect();
    let hover = if exporting { None } else { ptr.and_then(|p| hit(ts, p)) };
    for n in &m.nodes {
        if !shown(n) {
            continue;
        }
        let r = ts.card(n, off_of(&n.f.id));
        if !r.intersects(rect.expand(40.0)) {
            continue;
        }
        let faint = n.dlc_hidden;
        let st = states.as_ref().and_then(|s| s.get(&n.f.id)).copied();
        let alpha = |c: Color32| if faint || st == Some(State::Hidden) { c.gamma_multiply(0.35) } else if st == Some(State::Locked) { c.gamma_multiply(0.6) } else { c };
        let plate = Rect::from_min_size(pos2(r.min.x, r.min.y + ICON_H * z), vec2(r.width(), PLATE_H * z));
        let plate_col = match st {
            Some(State::Completed) => Color32::from_rgb(60, 120, 50),
            Some(State::Available) => Color32::from_rgb(150, 120, 40),
            _ if n.f.shared => Color32::from_rgb(80, 60, 110),
            _ => {
                if dark {
                    Color32::from_rgb(62, 66, 60)
                } else {
                    Color32::from_rgb(150, 145, 125)
                }
            }
        };
        painter.rect_filled(plate, CornerRadius::same((3.0 * z) as u8), alpha(plate_col));
        // icon
        let icon_rect = Rect::from_center_size(pos2(r.center().x, r.min.y + ICON_H * z / 2.0), vec2(ICON_H, ICON_H) * z);
        let mut drew_icon = false;
        if z > 0.25
            && let Some(icon) = &n.f.icon
            && let Some((tid, size)) = cx.texture(ui.ctx(), icon)
        {
            let k = (icon_rect.width() / size.x).min(icon_rect.height() / size.y);
            let ir = Rect::from_center_size(icon_rect.center(), size * k);
            painter.image(tid, ir, Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), alpha(Color32::WHITE));
            drew_icon = true;
        }
        if !drew_icon {
            painter.rect_filled(icon_rect.shrink(6.0 * z), CornerRadius::same((6.0 * z) as u8), alpha(if dark { Color32::from_rgb(70, 78, 90) } else { Color32::from_rgb(170, 175, 185) }));
            if z > 0.35 {
                painter.text(icon_rect.center(), Align2::CENTER_CENTER, "?", FontId::proportional(20.0 * z), alpha(Color32::from_gray(200)));
            }
        }
        if z > 0.3 {
            let font = FontId::proportional((11.0 * z).clamp(6.0, 16.0));
            let g = painter.layout(n.name.clone(), font, alpha(Color32::from_rgb(240, 236, 220)), plate.width() - 4.0);
            let gp = plate.center() - g.size() / 2.0;
            painter.with_clip_rect(plate.intersect(rect)).galley(gp, g, Color32::WHITE);
        }
        // marks
        let sel = ts.sel.contains(&n.f.id);
        let outline_col = if sel {
            Some(Color32::from_rgb(90, 170, 255))
        } else if ts.link_from.as_deref() == Some(&n.f.id) {
            Some(Color32::from_rgb(255, 210, 80))
        } else if hits.contains(&n.f.id) {
            Some(Color32::from_rgb(250, 120, 220))
        } else if hover.as_deref() == Some(&n.f.id) {
            Some(Color32::from_gray(200))
        } else {
            None
        };
        if let Some(c) = outline_col {
            painter.rect_stroke(r.expand(2.0), CornerRadius::same(4), Stroke::new(2.0, c), egui::StrokeKind::Outside);
        }
        if !n.editable && z > 0.3 {
            painter.text(r.right_top() + vec2(-3.0, 3.0), Align2::RIGHT_TOP, "🔒", FontId::proportional(10.0 * z.max(0.6)), Color32::from_gray(180));
        }
        if let Some(&err) = issue_ids.get(n.f.id.as_str()) {
            painter.circle_filled(r.left_top() + vec2(5.0, 5.0) * z.max(0.5), (5.0 * z).max(2.5), if err { ERR } else { WARN });
        }
        if !n.f.dlc.is_empty() && z > 0.35 {
            painter.text(r.left_bottom() + vec2(3.0, -PLATE_H * z - 2.0), Align2::LEFT_BOTTOM, "DLC", FontId::proportional(9.0 * z.max(0.7)), Color32::from_rgb(120, 200, 255));
        }
    }
    // the box being dragged out
    if let Some((a, b)) = ts.rubber {
        let r = Rect::from_two_pos(a, b);
        painter.rect(r, 0.0, Color32::from_rgba_unmultiplied(90, 170, 255, 30), Stroke::new(1.0, Color32::from_rgb(90, 170, 255)), egui::StrokeKind::Inside);
    }
    // the new focus's cell in add mode
    if ts.mode == Mode::Add
        && let Some(p) = ptr
        && hit(ts, p).is_none()
    {
        let (gx, gy) = ts.cell_at(p);
        let fake = Node { f: Focus::default(), x: gx, y: gy, key: None, name: String::new(), editable: false, dlc_hidden: false };
        let r = ts.card(&fake, Vec2::ZERO);
        painter.rect_stroke(r, 4.0, Stroke::new(1.5, OK), egui::StrokeKind::Inside);
        painter.text(r.center(), Align2::CENTER_CENTER, format!("+ {gx}, {gy}"), FontId::proportional(12.0), OK);
    }
    if let (Mode::Link | Mode::Exclusive, Some(from), Some(p)) = (ts.mode, &ts.link_from, ptr)
        && let Some(&i) = m.idx.get(from)
    {
        let a = ts.card(&m.nodes[i], Vec2::ZERO).center();
        painter.line_segment([a, p], Stroke::new(1.5, Color32::from_rgb(255, 210, 80)));
    }
    if ts.minimap && !exporting {
        minimap(ui, ts, m, rect, &painter);
    }
    if let Some(p) = ptr
        && !exporting
        && let Some(id) = hit(ts, p)
        && let Some(&i) = m.idx.get(&id)
    {
        let n = &m.nodes[i];
        let mut tip = format!("{}\n{}", n.name, n.f.id);
        if let Some(c) = n.f.cost {
            tip += &format!("\n{} days", (c * 7.0).round());
        }
        if !n.f.prereq.is_empty() {
            tip += &format!("\nneeds: {}", n.f.prereq.iter().map(|g| g.join(" or ")).collect::<Vec<_>>().join(", and "));
        }
        if !n.f.mutex.is_empty() {
            tip += &format!("\nexcludes: {}", n.f.mutex.join(", "));
        }
        if !n.f.dlc.is_empty() {
            tip += &format!("\nonly with {}", n.f.dlc.join(", "));
        }
        if let Some(s) = states.as_ref().and_then(|s| s.get(&n.f.id)) {
            tip += &format!("\n{s:?}");
        }
        resp.clone().on_hover_text_at_pointer(tip);
    }
    // the picture of the whole tree, a part per frame
    if exporting {
        export_step(ui.ctx(), ts, cx, rect);
        ui.ctx().request_repaint();
    }
}

fn minimap(ui: &egui::Ui, ts: &mut TreeState, m: &Model, rect: Rect, painter: &egui::Painter) {
    let (x0, y0, x1, y1) = m.bounds;
    let ww = (x1 - x0 + 1) as f32 * CW;
    let wh = (y1 - y0 + 1) as f32 * CH;
    let size = vec2(180.0, 180.0 * (wh / ww).clamp(0.3, 1.0));
    let mr = Rect::from_min_size(rect.max - size - vec2(10.0, 10.0), size);
    painter.rect_filled(mr, 4.0, Color32::from_rgba_unmultiplied(0, 0, 0, 150));
    let k = (size.x / ww).min(size.y / wh);
    let o = mr.min + (size - vec2(ww, wh) * k) / 2.0;
    let to_m = |w: Pos2| o + (w.to_vec2() - vec2(x0 as f32 * CW, y0 as f32 * CH)) * k;
    for n in &m.nodes {
        let c = if ts.sel.contains(&n.f.id) { Color32::from_rgb(90, 170, 255) } else { Color32::from_gray(190) };
        painter.circle_filled(to_m(TreeState::world(n) + vec2(0.0, ICON_H / 2.0)), 1.6, c);
    }
    let v = Rect::from_two_pos(to_m(ts.to_world(rect.min)), to_m(ts.to_world(rect.max))).intersect(mr);
    painter.rect_stroke(v, 0.0, Stroke::new(1.0, Color32::from_rgb(255, 210, 80)), egui::StrokeKind::Inside);
    let r = ui.interact(mr, ui.scope_id().with("minimap"), Sense::click_and_drag());
    if (r.clicked() || r.dragged())
        && let Some(p) = r.interact_pointer_pos()
    {
        let w = (p - o) / k + vec2(x0 as f32 * CW, y0 as f32 * CH);
        ts.off = rect.size() / 2.0 - w * ts.zoom;
    }
}

fn export_step(ctx: &egui::Context, ts: &mut TreeState, cx: &mut Cx, rect: Rect) {
    let Some(e) = &mut ts.export else { return };
    let ppp = ctx.pixels_per_point();
    let shot = e.shot.lock().take();
    if let Some(img) = shot {
        let (_, (px, py)) = e.tiles[e.at];
        // the canvas part of the screenshot
        let (sx, sy) = ((rect.min.x * ppp) as u32, (rect.min.y * ppp) as u32);
        let (w, h) = ((rect.width() * ppp) as u32, (rect.height() * ppp) as u32);
        for y in 0..h {
            for x in 0..w {
                let (ix, iy) = (sx + x, sy + y);
                if ix as usize >= img.size[0] || iy as usize >= img.size[1] {
                    continue;
                }
                let (ox, oy) = (px + x, py + y);
                if ox < e.img.width() && oy < e.img.height() {
                    let c = img.pixels[iy as usize * img.size[0] + ix as usize];
                    e.img.put_pixel(ox, oy, image::Rgba(c.to_array()));
                }
            }
        }
        e.at += 1;
        e.wait = 0;
        e.requested = false;
        if e.at >= e.tiles.len() {
            let r = e.img.save(&e.path);
            let path = e.path.clone();
            (ts.zoom, ts.off) = e.view;
            ts.export = None;
            match r {
                Ok(()) => cx.info(format!("saved the tree as {}", path.display())),
                Err(err) => cx.error(format!("cannot write {}: {err}", path.display())),
            }
        }
        return;
    }
    e.wait += 1;
    if !e.requested && e.wait > 3 && !cx.gfx.busy() {
        e.requested = true;
        let shot = e.shot.clone();
        let c = ctx.clone();
        ctx.request_screenshot(move |img| {
            *shot.lock() = Some(img);
            c.request_repaint();
        });
    }
}

/// A click on the canvas (`hit`: the focus under it).
fn click(cx: &mut Cx, ts: &mut TreeState, m: &Arc<Model>, hit: Option<String>, p: Pos2, mods: egui::Modifiers) {
    if let Some(done) = &mut ts.sim {
        if let Some(id) = hit {
            let states = focus::states(&m.tree.focuses, done);
            match states.get(&id) {
                Some(State::Available) => {
                    done.insert(id);
                }
                Some(State::Completed) => {
                    // undo it and what came after it
                    for b in m.tree.branch(&id) {
                        done.remove(&b);
                    }
                }
                Some(State::Locked) => cx.info(format!("{id} needs {} first", m.idx.get(&id).map(|&i| m.nodes[i].f.prereq.iter().map(|g| g.join(" or ")).collect::<Vec<_>>().join(", and ")).unwrap_or_default())),
                _ => cx.info(format!("{id} cannot be taken any more (an exclusive focus was taken)")),
            }
        }
        return;
    }
    match ts.mode {
        Mode::Select | Mode::Pan => match hit {
            Some(id) => {
                if mods.shift || mods.command {
                    if !ts.sel.remove(&id) {
                        ts.sel.insert(id.clone());
                    }
                } else {
                    ts.sel = [id.clone()].into();
                }
                ts.primary = Some(id);
            }
            None => {
                if !(mods.shift || mods.command) {
                    ts.sel.clear();
                    ts.primary = None;
                }
            }
        },
        Mode::Link | Mode::Exclusive => match (hit, ts.link_from.clone()) {
            (None, _) => ts.link_from = None,
            (Some(id), None) => ts.link_from = Some(id),
            (Some(id), Some(from)) if id == from => ts.link_from = None,
            (Some(to), Some(from)) => {
                if ts.mode == Mode::Link {
                    link(cx, m, &from, &to, mods.shift);
                } else {
                    exclusive(cx, m, &from, &to);
                }
                ts.link_from = None;
                ts.sel = [to.clone()].into();
                ts.primary = Some(to);
            }
        },
        Mode::Add => {
            if let Some(id) = hit {
                ts.sel = [id.clone()].into();
                ts.primary = Some(id);
                return;
            }
            let (gx, gy) = ts.cell_at(p);
            let parent = ts.primary.clone().filter(|p| ts.sel.contains(p));
            let id = fresh_id(cx, m, &format!("{}_focus", cx.tag()));
            let pre: Vec<Vec<String>> = parent.iter().map(|p| vec![p.clone()]).collect();
            let text = focus::new_focus(&id, gx, gy, "GFX_goal_unknown", 10.0, &pre, false);
            if add_to_tree(cx, m, &format!("new focus {id}"), &[text]) {
                ts.sel = [id.clone()].into();
                ts.primary = Some(id.clone());
                cx.info(format!("new focus {id} at {gx}, {gy}: name it on the right"));
            }
        }
    }
}

fn link(cx: &mut Cx, m: &Model, from: &str, to: &str, either: bool) {
    let Some(&ti) = m.idx.get(to) else { return };
    let n = &m.nodes[ti];
    if !n.editable {
        cx.error(format!("{to} is not in the project; it cannot be changed here"));
        return;
    }
    let has = n.f.prereq.iter().flatten().any(|p| p == from);
    if has {
        edit_focuses(cx, m, &format!("{to} does not need {from}"), &[to.to_string()], |t, b, _| focus::remove_prereq(t, b, from));
        return;
    }
    // a link the other way round would go round in a circle
    if m.tree.branch(to).iter().any(|x| x == from) {
        cx.error(format!("{from} comes after {to} already: the link would go round"));
        return;
    }
    let group = if either && !n.f.prereq.is_empty() { Some(0) } else { None };
    let label = if group.is_some() { format!("{to} needs {from} (or the others)") } else { format!("{to} needs {from}") };
    edit_focuses(cx, m, &label, &[to.to_string()], |t, b, _| focus::add_prereq(t, b, from, group));
}

fn exclusive(cx: &mut Cx, m: &Model, a: &str, b: &str) {
    let on = !m.idx.get(a).is_some_and(|&i| m.nodes[i].f.mutex.iter().any(|x| x == b));
    let label = if on { format!("{a} and {b} exclude each other") } else { format!("{a} and {b} do not exclude each other") };
    let ids = [a.to_string(), b.to_string()];
    edit_focuses(cx, m, &label, &ids, |t, blk, n| {
        let other = if n.f.id == a { b } else { a };
        focus::set_mutex(t, blk, other, on)
    });
}

fn keys(ui: &egui::Ui, cx: &mut Cx, ts: &mut TreeState, m: &Arc<Model>, active: bool) {
    if !active || ui.ctx().egui_wants_keyboard_input() {
        return;
    }
    use egui::{Key, Modifiers};
    let (press, mods) = ui.input(|i| {
        let ks = [Key::Delete, Key::Backspace, Key::ArrowLeft, Key::ArrowRight, Key::ArrowUp, Key::ArrowDown, Key::F, Key::Escape, Key::S, Key::L, Key::E, Key::A, Key::P];
        (ks.map(|k| i.key_pressed(k)), i.modifiers)
    });
    let [del, bksp, left, right, up, down, fit, esc, s, l, e, a, p] = press;
    let sel: Vec<String> = ts.sel.iter().cloned().collect();
    if (del || bksp) && !sel.is_empty() && ts.sim.is_none() {
        delete(cx, m, &sel);
        ts.sel.clear();
    }
    let step = if mods.shift { 5 } else { 1 };
    let (dx, dy) = (if right { step } else if left { -step } else { 0 }, if down { step } else if up { -step } else { 0 });
    if (dx != 0 || dy != 0) && !sel.is_empty() && ts.sim.is_none() {
        move_by(cx, m, &sel, dx, dy);
    }
    if mods.command {
        if ui.input_mut(|i| i.consume_shortcut(&egui::KeyboardShortcut::new(Modifiers::COMMAND, Key::D))) && !sel.is_empty() {
            duplicate(cx, ts, m, &sel);
        }
        if ui.input_mut(|i| i.consume_shortcut(&egui::KeyboardShortcut::new(Modifiers::COMMAND, Key::A))) {
            ts.sel = m.nodes.iter().map(|n| n.f.id.clone()).collect();
        }
        return;
    }
    if fit {
        ts.fit = true;
    }
    if esc {
        ts.link_from = None;
        ts.sel.clear();
        ts.mode = Mode::Select;
    }
    for (k, mode) in [(s, Mode::Select), (l, Mode::Link), (e, Mode::Exclusive), (a, Mode::Add), (p, Mode::Pan)] {
        if k {
            ts.mode = mode;
            ts.link_from = None;
        }
    }
}

fn duplicate(cx: &mut Cx, ts: &mut TreeState, m: &Arc<Model>, ids: &[String]) {
    let taken: BTreeSet<String> = cx.st.defs().map(|k| k.id).collect();
    let mut rename = BTreeMap::new();
    for id in ids {
        let mut n = format!("{id}_copy");
        let mut k = 2;
        while taken.contains(&n) || m.idx.contains_key(&n) || rename.values().any(|v| *v == n) {
            n = format!("{id}_copy{k}");
            k += 1;
        }
        rename.insert(id.clone(), n);
    }
    let maxx = ids.iter().filter_map(|i| m.idx.get(i).map(|&j| m.nodes[j].x)).max().unwrap_or(0);
    let minx = ids.iter().filter_map(|i| m.idx.get(i).map(|&j| m.nodes[j].x)).min().unwrap_or(0);
    let n = copy_focuses(cx, m, ids, &rename, maxx - minx + 1, 0, &format!("duplicate {} focuses", ids.len()));
    if n > 0 {
        ts.sel = rename.into_values().collect();
    }
}

fn context_menu(ui: &mut egui::Ui, cx: &mut Cx, ts: &mut TreeState, m: &Arc<Model>) {
    let Some(id) = ts.context.clone() else {
        if ui.button("Fit the tree").clicked() {
            ts.fit = true;
            ui.close();
        }
        if ui.button("Select all").clicked() {
            ts.sel = m.nodes.iter().map(|n| n.f.id.clone()).collect();
            ui.close();
        }
        return;
    };
    let Some(&i) = m.idx.get(&id) else { return };
    let n = m.nodes[i].clone();
    ui.label(RichText::new(&n.name).strong());
    if ui.button("Select its branch").clicked() {
        ts.sel = m.tree.branch(&id).into_iter().collect();
        ui.close();
    }
    if ui.button("Copy its branch…").clicked() {
        let from = format!("{}_", cx.tag());
        let w = m.tree.branch(&id).iter().filter_map(|b| m.idx.get(b).map(|&j| m.nodes[j].x)).max().unwrap_or(n.x) - n.x + 1;
        ts.copy = Some((id.clone(), from.clone(), format!("{from}copy_"), w.max(1), 0));
        ui.close();
    }
    if ui.button("Move its branch…").clicked() {
        ts.shift = Some((id.clone(), 1, 0));
        ui.close();
    }
    if ui.button("Duplicate the selected (Ctrl+D)").clicked() {
        let sel: Vec<String> = ts.sel.iter().cloned().collect();
        duplicate(cx, ts, m, &sel);
        ui.close();
    }
    ui.separator();
    for g in &n.f.prereq {
        for p in g {
            if ui.button(format!("Does not need {p}")).clicked() {
                link(cx, m, p, &id, false);
                ui.close();
            }
        }
    }
    for x in &n.f.mutex {
        if ui.button(format!("Does not exclude {x}")).clicked() {
            exclusive(cx, m, &id, x);
            ui.close();
        }
    }
    if ui.button("Link from here…").on_hover_text("then click the focus that needs this one").clicked() {
        ts.mode = Mode::Link;
        ts.link_from = Some(id.clone());
        ui.close();
    }
    ui.separator();
    if let Some(k) = &n.key
        && ui.button("Open in the code").clicked()
    {
        if let Some((d, t, _)) = cx.st.def(k) {
            let line = script::line_of(t, d.span.start);
            cx.go(Nav::File { pkg: k.pkg.clone(), file: k.file.clone(), line: Some(line) });
        }
        ui.close();
    }
    if n.editable && ui.button(RichText::new("Delete the selected").color(ERR)).clicked() {
        let sel: Vec<String> = ts.sel.iter().cloned().collect();
        delete(cx, m, &sel);
        ts.sel.clear();
        ui.close();
    }
}

// ---------------------------------------------------------------- inspector

fn inspector(ui: &mut egui::Ui, cx: &mut Cx, ts: &mut TreeState, m: &Arc<Model>, tree_key: &DefKey, fs: &mut FormState) {
    if ts.sel.len() > 1 {
        ui.heading(format!("{} focuses", ts.sel.len()));
        let sel: Vec<String> = ts.sel.iter().cloned().collect();
        let prim = ts.primary.clone().and_then(|p| m.idx.get(&p).map(|&i| (m.nodes[i].x, m.nodes[i].y)));
        ui.horizontal_wrapped(|ui| {
            if let Some((px, py)) = prim {
                if ui.button("Same row").on_hover_text("as the focus clicked first").clicked() {
                    edit_focuses(cx, m, "line up in a row", &sel, |t, b, n| if n.f.rel.is_some() { Vec::new() } else { focus::set_xy(t, b, n.f.x, py) });
                }
                if ui.button("Same column").clicked() {
                    edit_focuses(cx, m, "line up in a column", &sel, |t, b, n| if n.f.rel.is_some() { Vec::new() } else { focus::set_xy(t, b, px, n.f.y) });
                }
            }
            if ui.button("Duplicate").clicked() {
                duplicate(cx, ts, m, &sel);
            }
            if ui.button(RichText::new("Delete").color(ERR)).clicked() {
                delete(cx, m, &sel);
                ts.sel.clear();
            }
        });
        egui::ScrollArea::vertical().show(ui, |ui| {
            for id in &sel {
                ui.label(RichText::new(id).monospace().small());
            }
        });
        return;
    }
    let Some(id) = ts.sel.iter().next().cloned() else {
        // the tree itself
        ui.heading(format!("Tree {}", tree_key.id));
        ui.label(RichText::new("Pick a focus to edit it. The tree's own settings:").weak());
        fs.select(Some(tree_key.clone()));
        form::fields(ui, cx, tree_key, Kind::FocusTree.spec().fields, &[], fs);
        form::popups(ui, cx, fs);
        ui.separator();
        ui.label(RichText::new("Shared focuses it includes come from the project and the base game.").weak().small());
        return;
    };
    let Some(&i) = m.idx.get(&id) else { return };
    let n = m.nodes[i].clone();
    ui.horizontal(|ui| {
        if let Some(icon) = &n.f.icon {
            let tex = cx.texture(ui.ctx(), icon);
            crate::content::gfx::show(ui, tex, vec2(56.0, 52.0)).on_hover_text(icon);
        }
        ui.vertical(|ui| {
            ui.label(RichText::new(&n.name).heading());
            ui.label(RichText::new(&n.f.id).monospace().small());
            ui.label(RichText::new(format!("at {}, {}{}{}", n.x, n.y, n.f.rel.as_ref().map(|r| format!(" (relative to {r})")).unwrap_or_default(), n.f.cost.map(|c| format!(" · {} days", (c * 7.0).round())).unwrap_or_default())).small());
        });
    });
    if !n.editable {
        ui.label(RichText::new("A shared focus of the base game: it cannot be changed here (import it to change a copy).").color(WARN));
        return;
    }
    let key = n.key.clone().unwrap();
    // needs / excludes
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("needs").weak());
        if n.f.prereq.is_empty() {
            ui.label(RichText::new("nothing").weak());
        }
        for (gi, g) in n.f.prereq.iter().enumerate() {
            if gi > 0 {
                ui.label("and");
            }
            for (k, p) in g.iter().enumerate() {
                if k > 0 {
                    ui.label("or");
                }
                if ui.small_button(format!("{p} ✖")).on_hover_text(format!("{} does not need {p} any more", n.f.id)).clicked() {
                    link(cx, m, p, &id, false);
                }
            }
        }
    });
    if !n.f.mutex.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("excludes").weak());
            for x in &n.f.mutex {
                if ui.small_button(format!("{x} ✖")).clicked() {
                    exclusive(cx, m, &id, x);
                }
            }
        });
    }
    let kids = m.tree.children(&id);
    if !kids.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("needed by").weak());
            for k in kids {
                if ui.small_button(k).clicked() {
                    ts.sel = [k.to_string()].into();
                    ts.primary = Some(k.to_string());
                    ts.center_on = Some(k.to_string());
                }
            }
        });
    }
    ui.horizontal_wrapped(|ui| {
        if ui.small_button("branch: copy…").clicked() {
            let from = format!("{}_", cx.tag());
            ts.copy = Some((id.clone(), from.clone(), format!("{from}copy_"), 2, 0));
        }
        if ui.small_button("branch: move…").clicked() {
            ts.shift = Some((id.clone(), 1, 0));
        }
        if ui.small_button("branch: select").clicked() {
            ts.sel = m.tree.branch(&id).into_iter().collect();
        }
    });
    ui.separator();
    fs.select(Some(key.clone()));
    form::def_editor(ui, cx, &key, fs);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::gfx::Gfx;
    use crate::content::store::Store;
    use crate::content::ui::{LocCache, Settings};
    use crate::editor::MapFacts;

    const TREE: &str = "focus_tree = {
\tid = tyr_tree
\tcountry = { factor = 0 }
\tfocus = {
\t\tid = TYR_a
\t\ticon = GFX_goal_generic_construct_infrastructure
\t\tx = 0
\t\ty = 0
\t\tcost = 10
\t}
\tfocus = {
\t\tid = TYR_b
\t\tx = 0
\t\ty = 1
\t\tcost = 10
\t\tprerequisite = { focus = TYR_a }
\t}
\tfocus = {
\t\tid = TYR_c
\t\tx = 2
\t\ty = 1
\t\tcost = 10
\t\tprerequisite = { focus = TYR_a }
\t}
\tfocus = {
\t\tid = TYR_d
\t\trelative_position_id = TYR_b
\t\tx = 0
\t\ty = 1
\t\tcost = 10
\t\tprerequisite = { focus = TYR_b }
\t}
}
";

    fn store() -> Store {
        let mut st = Store::empty(std::env::temp_dir().join("mh-tree-none"));
        st.begin("setup");
        st.set_text("TYR", "common/national_focus/tyr.txt", TREE.to_string());
        st.set_text("TYR", "localisation/english/tyr_l_english.yml", "\u{feff}l_english:\n TYR_a:0 \"First\"\n TYR_b:0 \"Second\"\n TYR_b_desc:0 \"About it\"\n".to_string());
        st.commit();
        st
    }

    fn with_cx<R>(st: &mut Store, f: impl FnOnce(&mut Cx) -> R) -> R {
        let (mut gfx, mut settings, facts, mut loc) = (Gfx::default(), Settings::default(), MapFacts::default(), LocCache::default());
        loc.refresh(st, "english");
        let (mut msg, mut nav) = (None, None);
        let mut cx = Cx { st, base: None, gfx: &mut gfx, settings: &mut settings, facts: &facts, pkg: "TYR", nation: None, msg: &mut msg, nav: &mut nav, loc: &loc, game: std::path::Path::new("/none"), user_dir: std::path::Path::new("/none"), docs: &Default::default() };
        f(&mut cx)
    }

    fn model(cx: &Cx) -> Arc<Model> {
        let key = cx.st.defs_of(Some("TYR"), Kind::FocusTree)[0].clone();
        Arc::new(build(cx, &key, &[], &None).expect("the tree"))
    }

    fn focus<'a>(m: &'a Model, id: &str) -> &'a Node {
        &m.nodes[*m.idx.get(id).unwrap_or_else(|| panic!("no {id}"))]
    }

    #[test]
    fn reads_the_tree_as_drawn() {
        let mut st = store();
        with_cx(&mut st, |cx| {
            let m = model(cx);
            assert_eq!(m.nodes.len(), 4);
            // relative positions are resolved
            assert_eq!((focus(&m, "TYR_d").x, focus(&m, "TYR_d").y), (0, 2));
            assert!(m.nodes.iter().all(|n| n.editable));
            assert_eq!(m.bounds, (0, 0, 2, 2));
            assert!(m.issues.is_empty(), "{:?}", m.issues);
        });
    }

    #[test]
    fn links_and_unlinks() {
        let mut st = store();
        with_cx(&mut st, |cx| {
            // both needed (a block each)
            link(cx, &model(cx), "TYR_c", "TYR_d", false);
            let m = model(cx);
            assert_eq!(focus(&m, "TYR_d").f.prereq, vec![vec!["TYR_b".to_string()], vec!["TYR_c".to_string()]]);
            // again: no longer needed
            link(cx, &m, "TYR_c", "TYR_d", false);
            let m = model(cx);
            assert_eq!(focus(&m, "TYR_d").f.prereq, vec![vec!["TYR_b".to_string()]]);
            // one of them is enough
            link(cx, &m, "TYR_c", "TYR_d", true);
            let m = model(cx);
            assert_eq!(focus(&m, "TYR_d").f.prereq, vec![vec!["TYR_b".to_string(), "TYR_c".to_string()]]);
            // a link that would go round is refused
            let before = cx.st.text("TYR", "common/national_focus/tyr.txt").unwrap().to_string();
            link(cx, &m, "TYR_d", "TYR_a", false);
            assert_eq!(cx.st.text("TYR", "common/national_focus/tyr.txt").unwrap(), before);
            assert!(cx.msg.as_ref().is_some_and(|m| m.1), "an error is shown");
        });
        // each change is one undo step
        assert!(st.undo().is_some());
        let s = Script::parse(st.text("TYR", "common/national_focus/tyr.txt").unwrap());
        let (_, b) = focus::find(&s, "TYR_d").unwrap();
        assert_eq!(b.blocks("prerequisite").count(), 1);
    }

    #[test]
    fn exclusive_goes_both_ways() {
        let mut st = store();
        with_cx(&mut st, |cx| {
            exclusive(cx, &model(cx), "TYR_b", "TYR_c");
            let m = model(cx);
            assert_eq!(focus(&m, "TYR_b").f.mutex, vec!["TYR_c".to_string()]);
            assert_eq!(focus(&m, "TYR_c").f.mutex, vec!["TYR_b".to_string()]);
            exclusive(cx, &m, "TYR_c", "TYR_b");
            let m = model(cx);
            assert!(focus(&m, "TYR_b").f.mutex.is_empty() && focus(&m, "TYR_c").f.mutex.is_empty());
        });
    }

    #[test]
    fn moves_keep_relative_positions() {
        let mut st = store();
        with_cx(&mut st, |cx| {
            move_by(cx, &model(cx), &["TYR_b".into(), "TYR_d".into()], 1, 0);
            let m = model(cx);
            assert_eq!((focus(&m, "TYR_b").x, focus(&m, "TYR_b").y), (1, 1));
            // d moved with its anchor, and is still relative to it
            assert_eq!((focus(&m, "TYR_d").x, focus(&m, "TYR_d").y), (1, 2));
            assert_eq!(focus(&m, "TYR_d").f.rel.as_deref(), Some("TYR_b"));
        });
    }

    #[test]
    fn deletes_and_forgets_what_named_it() {
        let mut st = store();
        with_cx(&mut st, |cx| {
            delete(cx, &model(cx), &["TYR_b".into()]);
            let m = model(cx);
            assert!(!m.idx.contains_key("TYR_b"));
            let d = focus(&m, "TYR_d");
            assert!(d.f.prereq.is_empty(), "{:?}", d.f.prereq);
            assert!(d.f.rel.is_none());
            // where it was on screen
            assert_eq!((d.x, d.y), (0, 2));
            assert!(m.issues.is_empty(), "{:?}", m.issues);
            let loc = cx.st.text("TYR", "localisation/english/tyr_l_english.yml").unwrap();
            assert!(!loc.contains("TYR_b") && loc.contains("TYR_a"), "{loc}");
        });
        // one step brings it all back
        st.undo();
        assert!(st.text("TYR", "common/national_focus/tyr.txt").unwrap().contains("id = TYR_b"));
        assert!(st.text("TYR", "localisation/english/tyr_l_english.yml").unwrap().contains("TYR_b_desc"));
    }

    #[test]
    fn duplicates_a_branch() {
        let mut st = store();
        let mut ts = TreeState::default();
        with_cx(&mut st, |cx| {
            let m = model(cx);
            duplicate(cx, &mut ts, &m, &["TYR_b".into(), "TYR_d".into()]);
            let m = model(cx);
            let b = focus(&m, "TYR_b_copy");
            let d = focus(&m, "TYR_d_copy");
            // the copy hangs where the original does, and its own parts are linked to each other
            assert_eq!(b.f.prereq, vec![vec!["TYR_a".to_string()]]);
            assert_eq!(d.f.prereq, vec![vec!["TYR_b_copy".to_string()]]);
            assert_eq!(d.f.rel.as_deref(), Some("TYR_b_copy"));
            assert_eq!((b.x, b.y), (1, 1));
            assert!(m.issues.is_empty(), "{:?}", m.issues);
            assert_eq!(ts.sel, ["TYR_b_copy".to_string(), "TYR_d_copy".to_string()].into());
            // texts came along
            let loc = cx.st.text("TYR", "localisation/english/tyr_l_english.yml").unwrap();
            assert!(loc.contains("TYR_b_copy:") && loc.contains("TYR_b_copy_desc:"), "{loc}");
        });
    }

    #[test]
    fn adds_a_focus_on_a_cell() {
        let mut st = store();
        let mut ts = TreeState { mode: Mode::Add, zoom: 1.0, rect: CanvasRect(Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 800.0))), ..Default::default() };
        ts.sel = ["TYR_c".to_string()].into();
        ts.primary = Some("TYR_c".into());
        with_cx(&mut st, |cx| {
            let m = model(cx);
            let p = ts.to_screen(pos2(3.0 * CW + CW / 2.0, 2.0 * CH + 40.0));
            click(cx, &mut ts, &m, None, p, egui::Modifiers::NONE);
            let m = model(cx);
            let n = focus(&m, "TYR_focus");
            assert_eq!((n.x, n.y), (3, 2));
            assert_eq!(n.f.prereq, vec![vec!["TYR_c".to_string()]]);
        });
        assert_eq!(ts.primary.as_deref(), Some("TYR_focus"));
    }

    #[test]
    fn tidy_leaves_no_overlaps() {
        let mut st = store();
        with_cx(&mut st, |cx| {
            // put two on one spot first
            move_by(cx, &model(cx), &["TYR_c".into()], -2, 0);
            assert!(!model(cx).issues.is_empty());
            tidy(cx, &model(cx));
            let m = model(cx);
            assert!(m.issues.is_empty(), "{:?}", m.issues);
            assert!(m.nodes.iter().all(|n| n.f.rel.is_none()));
            // parents stay above their children
            assert!(focus(&m, "TYR_a").y < focus(&m, "TYR_b").y && focus(&m, "TYR_b").y < focus(&m, "TYR_d").y);
        });
    }

    #[test]
    fn plays_the_tree() {
        let mut st = store();
        let mut ts = TreeState { sim: Some(BTreeSet::new()), ..Default::default() };
        with_cx(&mut st, |cx| {
            exclusive(cx, &model(cx), "TYR_b", "TYR_c");
            let m = model(cx);
            // b needs a first
            click(cx, &mut ts, &m, Some("TYR_b".into()), Pos2::ZERO, egui::Modifiers::NONE);
            assert!(ts.sim.as_ref().unwrap().is_empty());
            for id in ["TYR_a", "TYR_b"] {
                click(cx, &mut ts, &m, Some(id.into()), Pos2::ZERO, egui::Modifiers::NONE);
            }
            let st = focus::states(&m.tree.focuses, ts.sim.as_ref().unwrap());
            assert_eq!(st["TYR_c"], State::Hidden);
            assert_eq!(st["TYR_d"], State::Available);
            // taking a back takes back what came after it
            click(cx, &mut ts, &m, Some("TYR_a".into()), Pos2::ZERO, egui::Modifiers::NONE);
            assert!(ts.sim.as_ref().unwrap().is_empty());
        });
    }
}
