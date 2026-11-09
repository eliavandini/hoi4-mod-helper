//! The wizard window: script built from menus instead of typed, then put
//! into a field of a definition (or at the cursor of the code tab).
//!
//! Effects, conditions and modifiers come from a catalog of the common ones
//! (with pickers for countries, states, ideas, focuses, events, ...) and from
//! the game's documentation (all of them, with their description and
//! example). What is built can be wrapped: conditions in OR / NOT / AND,
//! effects in `if` (with a condition built the same way), `hidden_effect`,
//! another country or a state.
//!
//! Fields with a shape of their own get a page of their own: AI weights and
//! mean time to happen (a base and modifiers with conditions), the countries
//! of a focus tree, prerequisites and exclusive focuses, a country's
//! politics, popularities, technologies and ideas, research bonuses, focus
//! search filters and the battalions of a division template.
//!
//! The other blocks are built whole from a form ([`shapes`]): a character's
//! roles, positions, lists, weights, names, rules, a colour, equipment
//! bonuses.

mod shapes;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use eframe::egui::{self, RichText};

use super::{Cx, ERR, WARN, code};
use crate::content::kinds::Kind;
use crate::content::script::{self, Script};
use crate::content::store::DefKey;
use crate::content::shapes as sh;
use crate::content::wizard::{self as wz, Item, Param, Role};

/// Where what is built goes.
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub key: DefKey,
    /// The field; empty: the whole definition (the code tab, at its cursor).
    pub field: String,
    /// Which one of the field (None: a new one is added).
    pub nth: Option<usize>,
    pub role: Role,
    /// The cursor in the field's editor when the wizard was opened (a char index).
    pub cursor: Option<usize>,
}

/// What the window gives back.
pub enum Out {
    /// Added to the field (at the cursor; keys it has once take the new value).
    Add(String),
    /// The field's new contents.
    Replace(String),
}

#[derive(Clone, Debug, PartialEq)]
enum Pick {
    Item(usize),
    Doc(String),
    Modifier(String),
}

/// What a condition is being built for.
#[derive(Clone, Copy, Debug, PartialEq)]
enum CondFor {
    If,
    Modifier,
    /// A part of a form (`WizardState::part`).
    Part,
}

#[derive(Default)]
pub struct WizardState {
    pub target: Option<Target>,
    /// The page: a catalog, or the field's own.
    page: Option<Role>,
    search: String,
    pick: Option<Pick>,
    values: Vec<String>,
    /// The code of an entry of the documentation, as edited.
    code: String,
    negate: bool,
    /// What is built so far.
    pub built: String,
    /// The scope effects and conditions are listed for ("" : all).
    scope: Option<String>,
    mod_cat: String,
    /// A condition is being built (conditions added go into `cond`).
    cond_for: Option<(CondFor, Role)>,
    cond: String,
    wrap_tag: String,
    wrap_state: String,
    wrap_chance: String,
    // the field's own pages
    base: String,
    mult: String,
    unit: usize,
    tag: String,
    picks: Vec<String>,
    /// What was ticked when the page opened (listed first, so rows do not jump while ticking).
    first: Vec<String>,
    pops: BTreeMap<String, f32>,
    politics: [String; 4],
    category: String,
    bonus: String,
    battalion: String,
    count: String,
    /// The pages were set from this field.
    init_for: Option<(DefKey, String, Option<usize>)>,
    pub(super) lists: Lists,
    /// An entry to pick when the page shows next.
    pending_pick: Option<Pick>,
    /// A field built from a form: what it has.
    form: sh::Form,
    /// The part of the form being built with a catalog: (path, label).
    part: (String, String),
}

impl WizardState {
    pub fn open(&mut self, t: Target) {
        if self.target.as_ref().is_some_and(|o| o.key == t.key && o.field == t.field && o.nth == t.nth) {
            self.target = Some(t);
            return;
        }
        self.page = Some(t.role);
        self.cond_for = None;
        self.cond.clear();
        self.built.clear();
        self.target = Some(t);
    }
}

impl WizardState {
    /// Show a page and pick an entry by name (`trigger`, `trigger=has_war`; for screenshots and tests).
    pub fn show_page(&mut self, spec: &str) {
        let (page, pick) = spec.split_once('=').map(|(a, b)| (a, Some(b))).unwrap_or((spec, None));
        self.page = Some(match page {
            "effect" => Role::Effect,
            "trigger" => Role::Trigger,
            "modifier" => Role::Modifier,
            _ => return,
        });
        if let Some(p) = pick {
            let items = if page == "effect" { wz::EFFECTS } else { wz::TRIGGERS };
            self.pending_pick = match items.iter().position(|i| i.key == p) {
                Some(n) if page != "modifier" => Some(Pick::Item(n)),
                _ if page == "modifier" => Some(Pick::Modifier(p.to_string())),
                _ => Some(Pick::Doc(p.to_string())),
            };
        }
    }
}

// ---------------------------------------------------------------- lists for pickers

/// (value, what it is) lists by kind of value, with the store generation and
/// whether the base game was there when they were made.
pub(super) type Lists = HashMap<Param, (u64, bool, Arc<Vec<(String, String)>>)>;

/// (value, what it is) for a kind of value: the map's nations and states,
/// ids of the content and the base game.
pub(super) fn list(cx: &Cx, lists: &mut Lists, p: Param) -> Arc<Vec<(String, String)>> {
    let key = (cx.st.generation, cx.base.is_some());
    if let Some((g, b, l)) = lists.get(&p)
        && (*g, *b) == key
    {
        return l.clone();
    }
    let mut v: Vec<(String, String)> = Vec::new();
    let statics = |xs: &[&str]| xs.iter().map(|x| (x.to_string(), String::new())).collect::<Vec<_>>();
    match p {
        Param::Tag => {
            v.extend(cx.facts.nations.iter().map(|n| (n.tag.clone(), n.name.clone())));
            v.extend(wz::TARGETS.iter().map(|t| (t.to_string(), "scope".to_string())));
            if let Some(b) = cx.base {
                let mine: BTreeSet<&str> = cx.facts.nations.iter().map(|n| n.tag.as_str()).collect();
                v.extend(b.tags.iter().filter(|t| !mine.contains(t.as_str())).map(|t| (t.clone(), cx.text(t).map(crate::content::loc::plain).unwrap_or_else(|| "base game".into()))));
            }
        }
        Param::State => v.extend(cx.facts.state_names.iter().map(|(id, n)| (id.to_string(), n.clone()))),
        Param::Ref(kind) => {
            let mut seen = BTreeSet::new();
            for k in cx.st.defs().filter(|k| k.kind == kind) {
                if seen.insert(k.id.clone()) {
                    v.push((k.id.clone(), format!("{} · {}", cx.name_of(kind, &k.id), k.pkg)));
                }
            }
            if let Some(b) = cx.base {
                for d in b.defs.iter().filter(|d| d.kind == kind) {
                    if seen.insert(d.id.clone()) {
                        v.push((d.id.clone(), cx.name_of(kind, &d.id)));
                    }
                }
            }
        }
        Param::Building => v = statics(wz::BUILDINGS),
        Param::Resource => v = statics(wz::RESOURCES),
        Param::TechCategory => v = statics(wz::TECH_CATEGORIES),
        Param::Of(xs) => v = statics(xs),
        Param::Sprite => {
            let mut seen = BTreeSet::new();
            for k in cx.st.defs().filter(|k| k.kind == Kind::Sprite) {
                if seen.insert(k.id.clone()) {
                    v.push((k.id.clone(), k.pkg.clone()));
                }
            }
            if let Some(b) = cx.base {
                v.extend(b.sprites.keys().filter(|n| !seen.contains(*n)).map(|n| (n.clone(), String::new())));
            }
            v.sort();
        }
        Param::SubIdeology => {
            // the types of the ideologies: the content's, then the base game's
            let mut seen = BTreeSet::new();
            let mut add = |v: &mut Vec<(String, String)>, ideology: &str, types: &script::Block| {
                for t in types.items.iter().filter_map(|i| i.key.as_ref()) {
                    if seen.insert(t.text.clone()) {
                        v.push((t.text.clone(), ideology.to_string()));
                    }
                }
            };
            for k in cx.st.defs().filter(|k| k.kind == Kind::Ideology) {
                if let Some((d, _, s)) = cx.st.def(&k)
                    && let Some(t) = d.block(s).and_then(|b| b.block("types"))
                {
                    add(&mut v, &k.id, t);
                }
            }
            if let Some(b) = cx.base {
                for d in b.of_kind(Kind::Ideology) {
                    let Some(text) = b.text_of(d) else { continue };
                    let s = Script::parse(&text);
                    for i in &s.root.items {
                        if let Some(t) = i.block().and_then(|bl| bl.block("types")) {
                            add(&mut v, i.key(), t);
                        }
                    }
                }
            }
        }
        Param::File(folder) => {
            let dir = format!("{folder}/");
            let mut seen = BTreeSet::new();
            for (pkg, p) in &cx.st.packages {
                for path in p.files.keys() {
                    if let Some(name) = path.strip_prefix(&dir).filter(|n| !n.contains('/'))
                        && let Some((stem, _)) = name.rsplit_once('.')
                        && seen.insert(stem.to_string())
                    {
                        v.push((stem.to_string(), pkg.clone()));
                    }
                }
            }
            // the starting divisions the build makes from the map
            if folder == "history/units" {
                v.extend(cx.facts.nations.iter().map(|n| format!("{}_1936", n.tag)).filter(|n| !seen.contains(n)).map(|n| (n, "made by the build from the map".to_string())));
            }
        }
        Param::IdIn(folder, block) => {
            let mut seen = BTreeSet::new();
            let dir = format!("{folder}/");
            for (pkg, p) in &cx.st.packages {
                for (path, f) in p.files.iter().filter(|(path, _)| path.starts_with(&dir)) {
                    let _ = path;
                    for id in f.script.iter().flat_map(|s| s.root.blocks(block)).filter_map(|b| b.atom("id")) {
                        if seen.insert(id.to_string()) {
                            v.push((id.to_string(), pkg.clone()));
                        }
                    }
                }
            }
            v.extend(base_ids(cx.game, folder, block).iter().filter(|id| !seen.contains(id.as_str())).map(|id| (id.clone(), "base game".to_string())));
        }
        Param::WarGoal => {
            v = statics(wz::WAR_GOALS);
            if let Some(b) = cx.base {
                v.extend(b.defs.iter().filter(|d| d.kind == Kind::WarGoal && !wz::WAR_GOALS.contains(&d.id.as_str())).map(|d| (d.id.clone(), String::new())));
            }
        }
        _ => {}
    }
    let l = Arc::new(v);
    lists.insert(p, (key.0, key.1, l.clone()));
    l
}

/// The ids of `block = { id = x }` in the base game's files of a folder
/// (read once).
fn base_ids(game: &std::path::Path, folder: &str, block: &str) -> Arc<Vec<String>> {
    /// (game, folder, block) → ids.
    type Ids = BTreeMap<(std::path::PathBuf, String, String), Arc<Vec<String>>>;
    static CACHE: std::sync::Mutex<Ids> = std::sync::Mutex::new(BTreeMap::new());
    let key = (game.to_path_buf(), folder.to_string(), block.to_string());
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    cache
        .entry(key)
        .or_insert_with(|| {
            let mut files: Vec<_> = std::fs::read_dir(game.join(folder)).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "txt")).collect();
            files.sort();
            let mut out = Vec::new();
            for f in files {
                let text = std::fs::read_to_string(&f).unwrap_or_default();
                let s = Script::parse(&text);
                out.extend(s.root.blocks(block).filter_map(|b| b.atom("id")).map(String::from));
            }
            out.dedup();
            Arc::new(out)
        })
        .clone()
}

/// A text field with a searchable list of choices beside it.
fn choose(ui: &mut egui::Ui, id: egui::Id, value: &mut String, items: &dyn Fn() -> Arc<Vec<(String, String)>>, width: f32) -> bool {
    let mut changed = ui.add(egui::TextEdit::singleline(value).id(id).desired_width(width)).changed();
    if let Some(v) = pick_menu(ui, id, value, items) {
        *value = v;
        changed = true;
    }
    changed
}

/// A searchable list of choices behind a ▾: what is picked.
pub(super) fn pick_menu(ui: &mut egui::Ui, id: egui::Id, value: &str, items: &dyn Fn() -> Arc<Vec<(String, String)>>) -> Option<String> {
    let mut picked = None;
    ui.menu_button("▾", |ui| {
        let qid = id.with("q");
        let mut q: String = ui.data(|d| d.get_temp(qid)).unwrap_or_default();
        let r = ui.add(egui::TextEdit::singleline(&mut q).hint_text("search").desired_width(260.0));
        r.request_focus();
        ui.data_mut(|d| d.insert_temp(qid, q.clone()));
        let words: Vec<String> = q.to_lowercase().split_whitespace().map(String::from).collect();
        let all = items();
        let hits: Vec<&(String, String)> = all.iter().filter(|(v, l)| {
            let hay = format!("{} {}", v.to_lowercase(), l.to_lowercase());
            words.iter().all(|w| hay.contains(w.as_str()))
        })
        .take(400)
        .collect();
        ui.label(RichText::new(format!("{} of {}", hits.len(), all.len())).weak().small());
        egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
            for (v, l) in hits {
                let text = if l.is_empty() { v.clone() } else { format!("{v}   {l}") };
                if ui.selectable_label(value == v, text).clicked() {
                    picked = Some(v.clone());
                    ui.close();
                }
            }
        });
    });
    picked
}

/// The editor of one value of an entry.
fn param_ui(ui: &mut egui::Ui, cx: &Cx, lists: &mut Lists, id: egui::Id, p: Param, value: &mut String) {
    match p {
        Param::YesNo => {
            let mut yes = value == "yes";
            let label = if yes { "yes" } else { "no" };
            if ui.checkbox(&mut yes, label).changed() {
                *value = if yes { "yes" } else { "no" }.into();
            }
        }
        Param::Number | Param::Word | Param::Text | Param::Date => {
            ui.add(egui::TextEdit::singleline(value).id(id).desired_width(if p == Param::Number { 90.0 } else { 220.0 }));
        }
        Param::Loc => {
            ui.add(egui::TextEdit::singleline(value).id(id).desired_width(220.0).hint_text("localisation key"));
            if let Some(t) = cx.text(value.trim()) {
                ui.label(RichText::new(crate::content::loc::plain(t)).weak().small());
            } else if !value.trim().is_empty() {
                ui.label(RichText::new("no text yet").color(WARN).small()).on_hover_text("add it on the Localisation page");
            }
        }
        _ => {
            let l = list(cx, lists, p);
            choose(ui, id, value, &|| l.clone(), 200.0);
            if let Param::Ref(k) = p
                && !value.trim().is_empty()
            {
                let v = value.trim();
                let known = cx.st.find(k, v).is_some() || cx.base.is_some_and(|b| b.has(k, v));
                if known {
                    ui.label(RichText::new(cx.name_of(k, v)).weak().small());
                } else {
                    ui.label(RichText::new(format!("no {} {v}", k.spec().one)).color(WARN).small());
                }
            }
            if p == Param::State
                && let Some(s) = super::statemap::button(ui, cx, id.with("map"), value, false)
            {
                *value = s.to_string();
            }
            if p == Param::State
                && let Ok(id) = value.trim().parse::<u32>()
            {
                match cx.facts.state_names.get(&id) {
                    Some(n) => ui.label(RichText::new(n).weak().small()),
                    None => ui.label(RichText::new("not a state of the map").color(WARN).small()),
                };
            }
        }
    }
}

// ---------------------------------------------------------------- the field as it is

/// The inner text of the target field (None: it is not there, or not a block).
fn current(cx: &Cx, t: &Target) -> Option<String> {
    let (d, text, s) = cx.st.def(&t.key)?;
    let b = d.block(s)?;
    let it = b.all(&t.field).nth(t.nth?)?;
    match (it.block(), it.atom()) {
        (Some(bl), _) => Some(script::inner_text(text, bl)),
        (_, Some(a)) => Some(a.text.clone()),
        _ => None,
    }
}

/// The values of the field's own pages, from what the field has.
fn init(cx: &Cx, ws: &mut WizardState, t: &Target) {
    let had = current(cx, t);
    if let Some(shape) = sh::shape_of(t.key.kind, &t.field) {
        ws.form = sh::read(&shape, had.as_deref(), &sh::Fill { id: &t.key.id, tag: cx.tag() });
    }
    let cur = had.unwrap_or_default();
    let s = Script::parse(&cur);
    let atom = |k: &str| s.root.atom(k).map(str::to_string);
    let words = || -> Vec<String> { s.root.items.iter().filter_map(|i| i.key.as_ref().map(|k| k.text.clone())).collect() };
    ws.picks.clear();
    ws.mult = "2".into();
    ws.count = "1".into();
    ws.bonus = "0.1".into();
    ws.tag = cx.tag().to_string();
    ws.base = String::new();
    match t.role {
        Role::AiWeight => ws.base = atom("factor").or_else(|| atom("base")).unwrap_or_else(|| "1".into()),
        Role::Mtth => {
            let (u, v) = [("days", 0), ("months", 1), ("years", 2)].iter().find_map(|(k, u)| atom(k).map(|v| (*u, v))).unwrap_or((0, "30".into()));
            ws.unit = u;
            ws.base = v;
            ws.mult = "0.5".into();
        }
        Role::TreeCountry => {
            ws.base = atom("factor").unwrap_or_else(|| "0".into());
            ws.mult = "10".into();
        }
        Role::Prerequisite | Role::Exclusive => ws.picks = s.root.items.iter().filter(|i| i.is("focus")).filter_map(|i| i.text().map(str::to_string)).collect(),
        Role::Technologies | Role::SearchFilters => ws.picks = words(),
        Role::Ideas => {
            // `{ a b }` (words without values) or one id
            ws.picks = if s.root.items.is_empty() && !cur.trim().is_empty() { vec![cur.trim().to_string()] } else { s.root.items.iter().filter_map(|i| i.key.as_ref().map(|k| k.text.clone()).or_else(|| i.text().map(str::to_string))).collect() };
        }
        Role::Politics => {
            ws.politics = [atom("ruling_party").unwrap_or_else(|| "neutrality".into()), atom("last_election").unwrap_or_else(|| "1932.11.6".into()), atom("election_frequency").unwrap_or_else(|| "48".into()), atom("elections_allowed").unwrap_or_else(|| "yes".into())];
        }
        Role::Popularities => {
            ws.pops = s.root.items.iter().filter_map(|i| Some((i.key.as_ref()?.text.clone(), i.text()?.parse::<f32>().ok()?))).collect();
            for i in ["democratic", "communism", "fascism", "neutrality"] {
                ws.pops.entry(i.to_string()).or_insert(0.0);
            }
        }
        Role::ResearchBonus => ws.category = "infantry_weapons".into(),
        Role::Regiments => ws.battalion = "infantry".into(),
        Role::Support => ws.battalion = "engineer".into(),
        _ => {}
    }
    ws.first = if t.role.is_shape() { ws.form.words.clone() } else { ws.picks.clone() };
    ws.init_for = Some((t.key.clone(), t.field.clone(), t.nth));
}

// ---------------------------------------------------------------- the window

/// The wizard window; Some when something is to go into the target.
pub fn window(ctx: &egui::Context, cx: &mut Cx, ws: &mut WizardState) -> Option<(Target, Out)> {
    let t = ws.target.clone()?;
    if ws.init_for.as_ref() != Some(&(t.key.clone(), t.field.clone(), t.nth)) {
        init(cx, ws, &t);
    }
    let mut open = true;
    let mut out = None;
    let what = if t.field.is_empty() { format!("the code of {}", t.key.id) } else { format!("{} of {}", t.field, t.key.id) };
    egui::Window::new(format!("🪄 Wizard: {what}")).id(egui::Id::unique("content_wizard")).open(&mut open).default_size([1000.0, 660.0]).show(ctx, |ui| {
        // pages
        let page = ws.page.unwrap_or(t.role);
        ui.horizontal(|ui| {
            let mut pages: Vec<Role> = Vec::new();
            if !t.role.is_catalog() {
                pages.push(t.role);
            }
            if t.role.is_catalog() {
                pages.extend([Role::Effect, Role::Trigger, Role::Modifier]);
            } else if matches!(t.role, Role::AiWeight | Role::Mtth | Role::TreeCountry) {
                pages.push(Role::Trigger);
            } else if matches!(ws.cond_for, Some((CondFor::Part, _))) && page.is_catalog() {
                pages.push(page);
            }
            let mut p = page;
            for r in pages {
                ui.selectable_value(&mut p, r, RichText::new(r.label()).strong());
            }
            if p != page {
                // back to the form from building a part of it: what was built goes in
                if matches!(ws.cond_for, Some((CondFor::Part, _))) && !p.is_catalog() {
                    ws.form.vals.insert(ws.part.0.clone(), ws.cond.trim().to_string());
                    ws.cond.clear();
                    ws.cond_for = None;
                }
                ws.page = Some(p);
                ws.pick = None;
            }
            if t.cursor.is_some() && !t.role.replaces() {
                ui.label(RichText::new("goes in after the line of the cursor").weak().small());
            }
        });
        if let Some((what, back)) = ws.cond_for {
            ui.horizontal(|ui| {
                let label = match what {
                    CondFor::If => "Conditions you add now go into the condition of the if.".to_string(),
                    CondFor::Modifier => "Conditions you add now go into the condition of the modifier.".to_string(),
                    CondFor::Part => format!("What you add now goes into “{}”.", ws.part.1),
                };
                ui.label(RichText::new(label).color(WARN));
                if ui.button("✔ Done").clicked() {
                    if what == CondFor::Part {
                        ws.form.vals.insert(ws.part.0.clone(), ws.cond.trim().to_string());
                        ws.cond.clear();
                    }
                    ws.page = Some(back);
                    ws.cond_for = None;
                    ws.pick = None;
                }
            });
        }
        ui.separator();
        // a form: what the field will be, and what is wrong with it
        let shape = sh::shape_of(t.key.kind, &t.field).filter(|_| t.role.is_shape());
        let problems = shape.map(|s| {
            ws.built = sh::write(&s, &ws.form);
            sh::problems(&s, &ws.form)
        });
        // what is built (at the bottom)
        let replace = t.role.replaces();
        egui::Panel::bottom("wizard_built").resizable(true).default_size(210.0).min_size(120.0).show(ui, |ui| {
            if !replace && (page == Role::Effect || page == Role::Trigger || t.role == Role::Trigger) {
                wrappers(ui, cx, ws, page);
            }
            ui.horizontal(|ui| {
                ui.label(RichText::new(if replace { "The field will be:" } else { "Script being built:" }).strong());
                if !replace && ui.small_button("clear").clicked() {
                    ws.built.clear();
                }
                if ui.small_button("format").clicked() {
                    ws.built = script::format(&ws.built).trim_end().to_string();
                }
            });
            let parsed = Script::parse(&ws.built);
            let errs: Vec<usize> = parsed.errors.iter().map(|e| script::line_of(&ws.built, e.at)).collect();
            let mut jump = None;
            let rows = ((ui.available_height() - 40.0) / 16.0).max(3.0) as usize;
            if replace {
                // made from the page above
                code::view(ui, egui::Id::unique("wizard_built_view"), &ws.built, code::Lang::Script, rows);
            } else {
                code::editor(ui, egui::Id::unique("wizard_built_code"), &mut ws.built, code::Lang::Script, &errs, rows, &mut jump);
            }
            ui.horizontal(|ui| {
                let ok = parsed.errors.is_empty() && (replace || !ws.built.trim().is_empty()) && problems.as_ref().is_none_or(|p| p.is_empty());
                let label = match (replace, t.field.is_empty()) {
                    (true, _) => format!("Set {}", t.field),
                    (false, true) => "Insert into the code".to_string(),
                    (false, false) if t.nth.is_none() => format!("Add {} with this", t.field),
                    _ => format!("Insert into {}", t.field),
                };
                if ui.add_enabled(ok, egui::Button::new(RichText::new(label).strong())).clicked() {
                    let text = script::format(&ws.built).trim_end().to_string();
                    out = Some((t.clone(), if replace { Out::Replace(text) } else { Out::Add(text) }));
                }
                if let Some(e) = parsed.errors.first() {
                    ui.label(RichText::new(format!("line {}: {}", script::line_of(&ws.built, e.at), e.message)).color(ERR).small());
                } else if let Some(p) = problems.as_ref().and_then(|p| p.first()) {
                    let more = problems.as_ref().map(|p| p.len() - 1).unwrap_or(0);
                    ui.label(RichText::new(if more > 0 { format!("{p} (and {more} more)") } else { p.clone() }).color(WARN).small());
                }
            });
        });
        match (page, shape) {
            (Role::Effect | Role::Trigger | Role::Modifier, _) => catalog(ui, cx, ws, page),
            (_, Some(s)) => shapes::page(ui, cx, ws, &t, s),
            (r, None) => own_page(ui, cx, ws, &t, r),
        }
    });
    if !open {
        ws.target = None;
        ws.init_for = None;
    }
    if out.is_some() {
        ws.target = None;
        ws.init_for = None;
        ws.built.clear();
        ws.cond.clear();
    }
    out
}

/// Wrap what is built: conditions in OR / NOT / AND; effects in if, hidden,
/// another country, a state, a chance.
fn wrappers(ui: &mut egui::Ui, cx: &Cx, ws: &mut WizardState, page: Role) {
    let empty = ws.built.trim().is_empty();
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Wrap it all:").weak());
        let conditions = (page == Role::Trigger && ws.cond_for.is_none()) || ws.target.as_ref().is_some_and(|t| t.role == Role::Trigger);
        if conditions {
            for (k, help) in [("OR", "one of them is enough"), ("NOT", "none of them"), ("AND", "all of them (inside OR / NOT)")] {
                if ui.add_enabled(!empty, egui::Button::new(k).small()).on_hover_text(help).clicked() {
                    ws.built = wz::wrap(k, &ws.built);
                }
            }
        } else {
            if ui.add_enabled(!empty, egui::Button::new("if…").small()).on_hover_text("only when a condition holds").clicked() {
                let cond = if ws.cond.trim().is_empty() { "always = yes".to_string() } else { ws.cond.trim().to_string() };
                ws.built = wz::wrap("if", &format!("{}\n{}", wz::wrap("limit", &cond), ws.built.trim()));
                ws.cond.clear();
            }
            ui.add(egui::TextEdit::singleline(&mut ws.cond).desired_width(180.0).hint_text("condition of the if"));
            if ui.small_button("🪄").on_hover_text("build the condition").clicked() {
                ws.cond_for = Some((CondFor::If, page));
                ws.page = Some(Role::Trigger);
                ws.pick = None;
            }
            ui.separator();
            if ui.add_enabled(!empty, egui::Button::new("hidden").small()).on_hover_text("hidden_effect: done without showing it").clicked() {
                ws.built = wz::wrap("hidden_effect", &ws.built);
            }
            ui.separator();
            let tags = list(cx, &mut ws.lists, Param::Tag);
            choose(ui, egui::Id::unique("wrap_tag"), &mut ws.wrap_tag, &|| tags.clone(), 60.0);
            if ui.add_enabled(!empty && !ws.wrap_tag.trim().is_empty(), egui::Button::new("for this country").small()).clicked() {
                ws.built = wz::wrap(ws.wrap_tag.trim(), &ws.built);
            }
            let states = list(cx, &mut ws.lists, Param::State);
            choose(ui, egui::Id::unique("wrap_state"), &mut ws.wrap_state, &|| states.clone(), 50.0);
            if let Some(s) = super::statemap::button(ui, cx, egui::Id::unique("wrap_state_map"), &ws.wrap_state, false) {
                ws.wrap_state = s.to_string();
            }
            if ui.add_enabled(!empty && !ws.wrap_state.trim().is_empty(), egui::Button::new("for this state").small()).clicked() {
                ws.built = wz::wrap(ws.wrap_state.trim(), &ws.built);
            }
            if ui.add_enabled(!empty, egui::Button::new("every owned state").small()).clicked() {
                ws.built = wz::wrap("every_owned_state", &ws.built);
            }
            ui.add(egui::TextEdit::singleline(&mut ws.wrap_chance).desired_width(36.0).hint_text("%"));
            let pct = ws.wrap_chance.trim().parse::<f64>().ok().filter(|p| (0.0..=100.0).contains(p));
            if ui.add_enabled(!empty && pct.is_some(), egui::Button::new("with this chance").small()).clicked() {
                ws.built = wz::wrap("random", &format!("chance = {}\n{}", ws.wrap_chance.trim(), ws.built.trim()));
            }
        }
    });
}

/// Add a line to what is built (or to the condition being built).
fn add_line(ws: &mut WizardState, line: &str) {
    if ws.cond_for.is_some() {
        ws.cond = wz::append(&ws.cond, line);
    } else {
        ws.built = wz::append(&ws.built, line);
    }
}

// ---------------------------------------------------------------- catalogs

fn scope_ui(ui: &mut egui::Ui, ws: &mut WizardState) {
    let scope = ws.scope.get_or_insert_with(|| "COUNTRY".into());
    egui::ComboBox::from_id_salt("wizard_scope")
        .selected_text(match scope.as_str() {
            "" => "anywhere",
            "COUNTRY" => "for a country",
            "STATE" => "for a state",
            "CHARACTER" => "for a character",
            s => s,
        })
        .show_ui(ui, |ui| {
            for (v, l) in [("COUNTRY", "for a country"), ("STATE", "for a state"), ("CHARACTER", "for a character"), ("", "anywhere")] {
                ui.selectable_value(scope, v.to_string(), l);
            }
        })
        .response
        .on_hover_text("Where the script runs: a focus, decision, idea or country event is for a country; inside `123 = { }` for a state");
}

fn catalog(ui: &mut egui::Ui, cx: &mut Cx, ws: &mut WizardState, page: Role) {
    let all_docs: &crate::content::docs::Docs = cx.docs;
    let w = ui.available_width();
    let mut picked = None;
    egui::Panel::left("wizard_list").resizable(true).default_size(330.0_f32.min(w * 0.45)).min_size(220.0).max_size(w * 0.6).show(ui, |ui| {
        super::contained(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("🔍");
                ui.add(egui::TextEdit::singleline(&mut ws.search).desired_width(160.0).hint_text("search"));
                if page == Role::Modifier {
                    let cats: BTreeSet<&str> = all_docs.modifiers.iter().flat_map(|m| m.categories.iter().map(String::as_str)).collect();
                    egui::ComboBox::from_id_salt("wizard_mod_cat").selected_text(if ws.mod_cat.is_empty() { "all kinds" } else { ws.mod_cat.as_str() }).show_ui(ui, |ui| {
                        ui.selectable_value(&mut ws.mod_cat, String::new(), "all kinds");
                        for c in cats {
                            ui.selectable_value(&mut ws.mod_cat, c.to_string(), c);
                        }
                    });
                } else {
                    scope_ui(ui, ws);
                }
            });
            let q: Vec<String> = ws.search.to_lowercase().split_whitespace().map(String::from).collect();
            let hit = |s: &str| q.iter().all(|w| s.contains(w.as_str()));
            egui::ScrollArea::vertical().id_salt(("wizard_list_scroll", page as u8)).auto_shrink([false, false]).show(ui, |ui| {
                match page {
                    Role::Modifier => {
                        let loc = |k: &str| cx.text(k).map(crate::content::loc::plain);
                        for (group, names) in wz::MODIFIERS {
                            let rows: Vec<(&str, String)> = names.iter().map(|n| (*n, wz::modifier_label(n, &loc))).filter(|(n, l)| hit(&format!("{n} {}", l.to_lowercase()))).collect();
                            if rows.is_empty() {
                                continue;
                            }
                            egui::CollapsingHeader::new(*group).default_open(true).id_salt(("wm", *group)).show(ui, |ui| {
                                for (n, l) in rows {
                                    if ui.selectable_label(ws.pick == Some(Pick::Modifier(n.to_string())), &l).on_hover_text(n).clicked() {
                                        picked = Some(Pick::Modifier(n.to_string()));
                                    }
                                }
                            });
                        }
                        let all: Vec<&crate::content::docs::Modifier> = all_docs.modifiers.iter().filter(|m| ws.mod_cat.is_empty() || m.categories.contains(&ws.mod_cat)).collect();
                        let rows: Vec<(&str, String)> = all.iter().map(|m| (m.name.as_str(), wz::modifier_label(&m.name, &loc))).filter(|(n, l)| hit(&format!("{n} {}", l.to_lowercase()))).collect();
                        egui::CollapsingHeader::new(format!("All modifiers of the game ({})", rows.len())).default_open(!q.is_empty()).id_salt("wm_all").show(ui, |ui| {
                            for (n, l) in rows.iter().take(800) {
                                if ui.selectable_label(ws.pick == Some(Pick::Modifier(n.to_string())), l.as_str()).on_hover_text(*n).clicked() {
                                    picked = Some(Pick::Modifier(n.to_string()));
                                }
                            }
                            if rows.len() > 800 {
                                ui.label(RichText::new("search to see the others").weak().small());
                            }
                        });
                        if all_docs.modifiers.is_empty() {
                            ui.label(RichText::new("The game's documentation was not found (documentation/ in the game folder).").weak().small());
                        }
                    }
                    _ => {
                        let items: &[Item] = if page == Role::Effect { wz::EFFECTS } else { wz::TRIGGERS };
                        let mut groups: Vec<&str> = items.iter().map(|i| i.group).collect();
                        groups.dedup();
                        for g in groups {
                            let rows: Vec<(usize, &Item)> = items.iter().enumerate().filter(|(_, i)| i.group == g && hit(&format!("{} {}", i.label.to_lowercase(), i.key))).collect();
                            if rows.is_empty() {
                                continue;
                            }
                            egui::CollapsingHeader::new(g).default_open(true).id_salt(("wc", page as u8, g)).show(ui, |ui| {
                                for (n, i) in rows {
                                    if ui.selectable_label(ws.pick == Some(Pick::Item(n)), i.label).on_hover_text(i.key).clicked() {
                                        picked = Some(Pick::Item(n));
                                    }
                                }
                            });
                        }
                        let scope = ws.scope.clone().unwrap_or_default();
                        let docs = if page == Role::Effect { &all_docs.effects } else { &all_docs.triggers };
                        let rows: Vec<&crate::content::docs::Entry> = docs.iter().filter(|e| (scope.is_empty() || e.in_scope(&scope)) && hit(&format!("{} {}", e.name, e.text.to_lowercase()))).collect();
                        let what = if page == Role::Effect { "effects" } else { "conditions" };
                        egui::CollapsingHeader::new(format!("All {what} of the game ({})", rows.len())).default_open(!q.is_empty()).id_salt(("wd", page as u8)).show(ui, |ui| {
                            for e in rows {
                                let first = e.text.lines().next().unwrap_or("");
                                if ui.selectable_label(ws.pick == Some(Pick::Doc(e.name.clone())), RichText::new(&e.name).monospace()).on_hover_text(first).clicked() {
                                    picked = Some(Pick::Doc(e.name.clone()));
                                }
                            }
                        });
                        if docs.is_empty() {
                            ui.label(RichText::new("The game's documentation was not found (documentation/ in the game folder).").weak().small());
                        }
                    }
                }
            });
        });
    });
    if let Some(p) = picked.or_else(|| ws.pending_pick.take()) {
        set_pick(cx, ws, page, p);
    }
    egui::ScrollArea::vertical().id_salt("wizard_detail").auto_shrink([false, false]).show(ui, |ui| detail(ui, cx, ws, page));
}

/// Pick an entry: its values start as the catalog's examples.
fn set_pick(cx: &Cx, ws: &mut WizardState, page: Role, p: Pick) {
    let all_docs: &crate::content::docs::Docs = cx.docs;
    ws.negate = false;
    match &p {
        Pick::Item(n) => {
            let items = if page == Role::Effect { wz::EFFECTS } else { wz::TRIGGERS };
            ws.values = items[*n].params.iter().map(|(_, ty, d)| if d.is_empty() && *ty == Param::Tag { cx.tag().to_string() } else { d.to_string() }).collect();
        }
        Pick::Doc(name) => {
            let e = if page == Role::Effect { all_docs.effect(name) } else { all_docs.trigger(name) };
            ws.code = e.and_then(|e| e.example.clone()).unwrap_or_else(|| format!("{name} = "));
        }
        Pick::Modifier(name) => {
            let boolean = all_docs.modifier(name).is_some_and(|m| m.decimals.is_none());
            ws.values = vec![if boolean { "yes".into() } else { "0.1".into() }];
        }
    }
    ws.pick = Some(p);
}

fn doc_text(ui: &mut egui::Ui, e: &crate::content::docs::Entry) {
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(format!("for: {}", if e.scopes.is_empty() { "anything".into() } else { e.scopes.join(", ").to_lowercase() })).weak().small());
        if !e.targets.is_empty() {
            ui.label(RichText::new(format!("· can point at: {}", e.targets.join(", "))).weak().small());
        }
    });
    if !e.text.is_empty() {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            egui::ScrollArea::vertical().id_salt(("doc_text", &e.name)).max_height(180.0).show(ui, |ui| {
                ui.label(RichText::new(crate::content::docs::plain(&e.text)).monospace().small());
            });
        });
    }
}

fn detail(ui: &mut egui::Ui, cx: &mut Cx, ws: &mut WizardState, page: Role) {
    let all_docs: &crate::content::docs::Docs = cx.docs;
    let Some(p) = ws.pick.clone() else {
        ui.add_space(20.0);
        ui.label(RichText::new(match page {
            Role::Effect => "Pick what is to happen on the left: the common ones by topic, or any effect of the game.",
            Role::Trigger => "Pick a condition on the left: the common ones by topic, or any condition of the game.",
            _ => "Pick a modifier on the left.",
        }).weak());
        return;
    };
    let doc = |name: &str| if page == Role::Effect { all_docs.effect(name) } else { all_docs.trigger(name) };
    match p {
        Pick::Item(n) => {
            let items = if page == Role::Effect { wz::EFFECTS } else { wz::TRIGGERS };
            let item = items[n];
            ui.heading(item.label);
            if let Some(e) = doc(item.key) {
                ui.label(RichText::new(&e.name).monospace().weak());
                doc_text(ui, e);
            }
            ui.add_space(6.0);
            ws.values.resize(item.params.len(), String::new());
            for (i, (label, ty, _)) in item.params.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.add_sized([150.0, 18.0], egui::Label::new(*label).truncate());
                    let id = egui::Id::unique(("wizard_param", n, i));
                    let mut v = std::mem::take(&mut ws.values[i]);
                    param_ui(ui, cx, &mut ws.lists, id, *ty, &mut v);
                    ws.values[i] = v;
                });
            }
            let problem = wz::missing(&item, &ws.values);
            let mut line = wz::render(&item, &ws.values);
            if page == Role::Trigger {
                ui.checkbox(&mut ws.negate, "the opposite (NOT)");
                if ws.negate {
                    line = wz::wrap("NOT", &line);
                }
            }
            ui.add_space(4.0);
            code::view(ui, egui::Id::unique(("wizard_line", n)), &line, code::Lang::Script, line.lines().count().min(10));
            add_button(ui, ws, &line, problem);
        }
        Pick::Doc(name) => {
            ui.heading(RichText::new(&name).monospace());
            if let Some(e) = doc(&name) {
                doc_text(ui, e);
                if e.example.is_none() {
                    ui.label(RichText::new("The documentation has no example: write the value below.").weak().small());
                }
            }
            ui.label(RichText::new("The script (edit it):").weak());
            let parsed = Script::parse(&ws.code);
            let errs: Vec<usize> = parsed.errors.iter().map(|e| script::line_of(&ws.code, e.at)).collect();
            let mut jump = None;
            let rows = ws.code.lines().count().clamp(2, 14);
            code::editor(ui, egui::Id::unique(("wizard_doc_code", &name)), &mut ws.code, code::Lang::Script, &errs, rows, &mut jump);
            let mut line = ws.code.trim().to_string();
            if page == Role::Trigger {
                ui.checkbox(&mut ws.negate, "the opposite (NOT)");
                if ws.negate {
                    line = wz::wrap("NOT", &line);
                }
            }
            let problem = if let Some(e) = parsed.errors.first() {
                Some(format!("line {}: {}", script::line_of(&ws.code, e.at), e.message))
            } else if parsed.root.items.is_empty() || parsed.root.items.iter().any(|i| i.key.is_some() && i.atom().is_none() && i.block().is_none()) {
                Some("give it a value".to_string())
            } else {
                None
            };
            add_button(ui, ws, &line, problem);
        }
        Pick::Modifier(name) => {
            let loc = |k: &str| cx.text(k).map(crate::content::loc::plain);
            ui.heading(wz::modifier_label(&name, &loc));
            ui.label(RichText::new(&name).monospace().weak());
            let m = all_docs.modifier(&name).cloned();
            if let Some(m) = &m {
                ui.label(RichText::new(format!("for: {}{}", m.categories.join(", "), match m.decimals {
                    Some(d) => format!(" · a number, shown with {d} decimals"),
                    None => " · yes / no".into(),
                }))
                .weak()
                .small());
                if !m.text.is_empty() {
                    ui.label(&m.text);
                }
            }
            if name.ends_with("_factor") {
                ui.label(RichText::new("A factor: 0.1 is +10 %, -0.25 is -25 %.").weak().small());
            }
            ws.values.resize(1, String::new());
            ui.horizontal(|ui| {
                ui.label("Value");
                if m.as_ref().is_some_and(|m| m.decimals.is_none()) {
                    param_ui(ui, cx, &mut ws.lists, egui::Id::unique("wizard_mod_value"), Param::YesNo, &mut ws.values[0]);
                } else {
                    ui.add(egui::TextEdit::singleline(&mut ws.values[0]).desired_width(90.0));
                }
            });
            let v = ws.values[0].trim().to_string();
            let problem = (v.is_empty() || (v != "yes" && v != "no" && v.parse::<f64>().is_err())).then(|| "the value is a number (or yes / no)".to_string());
            let line = format!("{name} = {v}");
            code::view(ui, egui::Id::unique(("wizard_line_mod", &name)), &line, code::Lang::Script, 1);
            add_button(ui, ws, &line, problem);
        }
    }
}

fn add_button(ui: &mut egui::Ui, ws: &mut WizardState, line: &str, problem: Option<String>) {
    ui.horizontal(|ui| {
        let into = match ws.cond_for {
            Some((CondFor::Part, _)) => format!("Add to “{}” ↓", ws.part.1),
            Some(_) => "Add to the condition ↓".to_string(),
            None => "Add ↓".to_string(),
        };
        if ui.add_enabled(problem.is_none(), egui::Button::new(RichText::new(into).strong())).clicked() {
            add_line(ws, line);
        }
        if let Some(p) = problem {
            ui.label(RichText::new(p).color(WARN).small());
        }
    });
    if ws.cond_for.is_some() {
        ui.add_space(6.0);
        ui.label(RichText::new(if matches!(ws.cond_for, Some((CondFor::Part, _))) { "So far:" } else { "The condition so far:" }).weak());
        ui.add(egui::TextEdit::multiline(&mut ws.cond).code_editor().desired_rows(3).desired_width(f32::INFINITY));
    }
}

// ---------------------------------------------------------------- the fields' own pages

/// The foci of the tree a focus is in (id, name), the content's.
fn tree_focuses(cx: &Cx, key: &DefKey) -> Vec<(String, String)> {
    let group = cx.st.def(key).and_then(|(d, _, _)| d.group.clone());
    let mut v: Vec<(String, String)> = cx.st.defs().filter(|k| matches!(k.kind, Kind::Focus | Kind::SharedFocus) && k.pkg == key.pkg && k.id != key.id).filter(|k| cx.st.def(k).map(|(d, _, _)| d.group.clone()) == Some(group.clone())).map(|k| (k.id.clone(), cx.name_of(k.kind, &k.id))).collect();
    v.sort();
    v.dedup();
    v
}

/// A list of ids with checkboxes (search, the ticked first).
fn tick_list(ui: &mut egui::Ui, id: &str, items: &[(String, String)], picks: &mut Vec<String>, first: &[String], search: &mut String) {
    ui.horizontal(|ui| {
        ui.label("🔍");
        ui.add(egui::TextEdit::singleline(search).desired_width(220.0));
        ui.label(RichText::new(format!("{} ticked", picks.len())).weak());
        if !picks.is_empty() && ui.small_button("untick all").clicked() {
            picks.clear();
        }
    });
    let q: Vec<String> = search.to_lowercase().split_whitespace().map(String::from).collect();
    // ticked ones not in the list (written by hand) stay listed
    let mut rows: Vec<(String, String)> = picks.iter().filter(|p| !items.iter().any(|(v, _)| v == *p)).map(|p| (p.clone(), String::new())).collect();
    rows.extend(items.iter().cloned());
    rows.sort_by_key(|(v, _)| !first.contains(v));
    let rows: Vec<(String, String)> = rows.into_iter().filter(|(v, l)| q.iter().all(|w| format!("{} {}", v.to_lowercase(), l.to_lowercase()).contains(w.as_str()))).collect();
    egui::ScrollArea::vertical().id_salt(("tick_list", id)).max_height(ui.available_height() - 8.0).auto_shrink([false, false]).show_rows(ui, 20.0, rows.len(), |ui, range| {
        for (v, l) in &rows[range] {
            let mut on = picks.contains(v);
            let text = if l.is_empty() { RichText::new(v).monospace() } else { RichText::new(format!("{v}   {l}")) };
            if ui.checkbox(&mut on, text).changed() {
                if on {
                    picks.push(v.clone());
                } else {
                    picks.retain(|p| p != v);
                }
            }
        }
    });
}

fn own_page(ui: &mut egui::Ui, cx: &mut Cx, ws: &mut WizardState, t: &Target, role: Role) {
    match role {
        Role::AiWeight | Role::Mtth | Role::TreeCountry => weight_page(ui, cx, ws, role),
        Role::Prerequisite | Role::Exclusive => {
            ui.label(RichText::new(if role == Role::Prerequisite {
                "Tick the focuses it needs. One of the ticked is enough; for \"all of them\" add another prerequisite with one focus each."
            } else {
                "Tick the focuses it excludes: taking one locks the others (the tree checks shows when the other side does not say so too)."
            })
            .weak());
            let items = tree_focuses(cx, &t.key);
            let mut search = std::mem::take(&mut ws.search);
            tick_list(ui, "focus", &items, &mut ws.picks, &ws.first, &mut search);
            ws.search = search;
            ws.built = ws.picks.iter().map(|f| format!("focus = {f}")).collect::<Vec<_>>().join("\n");
        }
        Role::Technologies | Role::Ideas => {
            let kind = if role == Role::Technologies { Kind::Technology } else { Kind::Idea };
            ui.label(RichText::new(if role == Role::Technologies { "Tick the technologies the country starts with." } else { "Tick the ideas (laws, national spirits) the country starts with." }).weak());
            let items = list(cx, &mut ws.lists, Param::Ref(kind));
            let mut search = std::mem::take(&mut ws.search);
            tick_list(ui, "refs", &items, &mut ws.picks, &ws.first, &mut search);
            ws.search = search;
            ws.built = ws.picks.iter().map(|p| if role == Role::Technologies { format!("{p} = 1") } else { p.clone() }).collect::<Vec<_>>().join("\n");
        }
        Role::SearchFilters => {
            ui.label(RichText::new("The filters of the focus search in the game the focus shows under.").weak());
            let items: Vec<(String, String)> = wz::SEARCH_FILTERS.iter().map(|f| (f.to_string(), cx.text(f).map(crate::content::loc::plain).unwrap_or_default())).collect();
            let mut search = std::mem::take(&mut ws.search);
            tick_list(ui, "filters", &items, &mut ws.picks, &ws.first, &mut search);
            ws.search = search;
            ws.built = ws.picks.join("\n");
        }
        Role::Politics => {
            ui.label(RichText::new("The country's government at the start.").weak());
            let labels = ["Ruling party", "Last election", "Months between elections", "Elections allowed"];
            let types = [Param::Ref(Kind::Ideology), Param::Date, Param::Number, Param::YesNo];
            for i in 0..4 {
                ui.horizontal(|ui| {
                    ui.add_sized([170.0, 18.0], egui::Label::new(labels[i]));
                    let mut v = std::mem::take(&mut ws.politics[i]);
                    param_ui(ui, cx, &mut ws.lists, egui::Id::unique(("politics", i)), types[i], &mut v);
                    ws.politics[i] = v;
                });
            }
            if !wz::is_date(ws.politics[1].trim()) {
                ui.label(RichText::new("the last election is a date: 1932.11.6").color(WARN).small());
            }
            ws.built = format!("ruling_party = {}\nlast_election = \"{}\"\nelection_frequency = {}\nelections_allowed = {}", ws.politics[0].trim(), ws.politics[1].trim(), ws.politics[2].trim(), ws.politics[3].trim());
        }
        Role::Popularities => {
            ui.label(RichText::new("How popular each ideology is, in %. The game wants them to make 100.").weak());
            let ideologies = list(cx, &mut ws.lists, Param::Ref(Kind::Ideology));
            for (id, _) in ideologies.iter() {
                ws.pops.entry(id.clone()).or_insert(0.0);
            }
            let names: Vec<String> = ws.pops.keys().cloned().collect();
            for n in &names {
                ui.horizontal(|ui| {
                    ui.add_sized([150.0, 18.0], egui::Label::new(cx.name_of(Kind::Ideology, n)).truncate());
                    let v = ws.pops.get_mut(n).unwrap();
                    ui.add(egui::Slider::new(v, 0.0..=100.0).integer().text(n.as_str()));
                });
            }
            let sum: f32 = ws.pops.values().sum();
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("together {sum:.0} %")).color(if (sum - 100.0).abs() < 0.5 { super::OK } else { WARN }));
                if sum > 0.0 && (sum - 100.0).abs() >= 0.5 && ui.button("make them 100").clicked() {
                    let k = 100.0 / sum;
                    for v in ws.pops.values_mut() {
                        *v = (*v * k).round();
                    }
                    // rounding: the largest takes the rest
                    let s: f32 = ws.pops.values().sum();
                    if let Some(v) = ws.pops.values_mut().max_by(|a, b| a.total_cmp(b)) {
                        *v += 100.0 - s;
                    }
                }
            });
            ws.built = ws.pops.iter().filter(|(_, v)| **v > 0.0).map(|(k, v)| format!("{k} = {v:.0}")).collect::<Vec<_>>().join("\n");
        }
        Role::ResearchBonus => {
            ui.label(RichText::new("Research faster in a category of technologies, for as long as the idea is there.").weak());
            ui.horizontal(|ui| {
                ui.add_sized([120.0, 18.0], egui::Label::new("Category"));
                let mut v = std::mem::take(&mut ws.category);
                param_ui(ui, cx, &mut ws.lists, egui::Id::unique("rb_cat"), Param::TechCategory, &mut v);
                ws.category = v;
            });
            ui.horizontal(|ui| {
                ui.add_sized([120.0, 18.0], egui::Label::new("Bonus (0.1 = 10 %)"));
                ui.add(egui::TextEdit::singleline(&mut ws.bonus).desired_width(80.0));
            });
            let ok = !ws.category.trim().is_empty() && ws.bonus.trim().parse::<f64>().is_ok();
            let line = format!("{} = {}", ws.category.trim(), ws.bonus.trim());
            add_button(ui, ws, &line, (!ok).then(|| "a category and a number".to_string()));
        }
        Role::Regiments | Role::Support => battalion_page(ui, cx, ws, t, role == Role::Support),
        _ => {}
    }
}

/// AI weight, mean time to happen, the countries of a tree: a base, and
/// modifiers with a condition.
fn weight_page(ui: &mut egui::Ui, cx: &mut Cx, ws: &mut WizardState, role: Role) {
    let units = ["days", "months", "years"];
    match role {
        Role::AiWeight => ui.label(RichText::new("How much the AI wants it: a base factor (1 is usual, 0 never), multiplied by every modifier whose condition holds.").weak()),
        Role::Mtth => ui.label(RichText::new("How soon it fires on average once its trigger holds, made shorter or longer by modifiers (a factor of 0.5 halves the time).").weak()),
        _ => ui.label(RichText::new("Which countries get this tree: the one with the highest weight. A base factor (usually 0), and a weight added for countries.").weak()),
    };
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if role == Role::Mtth {
            egui::ComboBox::from_id_salt("mtth_unit").selected_text(units[ws.unit]).show_ui(ui, |ui| {
                for (i, u) in units.iter().enumerate() {
                    ui.selectable_value(&mut ws.unit, i, *u);
                }
            });
        } else {
            ui.label("Base factor");
        }
        ui.add(egui::TextEdit::singleline(&mut ws.base).desired_width(70.0));
        let ok = ws.base.trim().parse::<f64>().is_ok();
        let key = if role == Role::Mtth { units[ws.unit] } else { "factor" };
        if ui.add_enabled(ok, egui::Button::new("Set ↓")).on_hover_text("the field's own value is replaced").clicked() {
            let line = format!("{key} = {}", ws.base.trim());
            // one base: the one built before goes
            let s = Script::parse(&ws.built);
            let rest: Vec<String> = s.root.items.iter().filter(|i| !i.key.as_ref().is_some_and(|k| ["factor", "days", "months", "years"].contains(&k.text.as_str()))).map(|i| script::item_source(&ws.built, i)).collect();
            ws.built = wz::append(&line, &rest.join("\n"));
        }
    });
    ui.separator();
    if role == Role::TreeCountry {
        ui.horizontal(|ui| {
            ui.label("Add");
            ui.add(egui::TextEdit::singleline(&mut ws.mult).desired_width(50.0));
            ui.label("for the country");
            let tags = list(cx, &mut ws.lists, Param::Tag);
            choose(ui, egui::Id::unique("tree_country_tag"), &mut ws.tag, &|| tags.clone(), 70.0);
            let ok = ws.mult.trim().parse::<f64>().is_ok() && !ws.tag.trim().is_empty();
            if ui.add_enabled(ok, egui::Button::new("Add ↓")).clicked() {
                let line = format!("modifier = {{\n\tadd = {}\n\ttag = {}\n}}", ws.mult.trim(), ws.tag.trim());
                ws.built = wz::append(&ws.built, &line);
            }
        });
        ui.label(RichText::new("Or a weight when a condition holds:").weak());
    }
    ui.horizontal(|ui| {
        let what = match role {
            Role::TreeCountry => "Add",
            Role::Mtth => "Times the time by",
            _ => "Multiply by",
        };
        ui.label(what);
        ui.add(egui::TextEdit::singleline(&mut ws.mult).desired_width(50.0));
        ui.label("when:");
        if ui.button("🪄 build the condition").clicked() {
            ws.cond_for = Some((CondFor::Modifier, role));
            ws.page = Some(Role::Trigger);
            ws.pick = None;
        }
    });
    ui.add(egui::TextEdit::multiline(&mut ws.cond).code_editor().desired_rows(4).desired_width(f32::INFINITY).hint_text("has_war = yes"));
    let cond_ok = !ws.cond.trim().is_empty() && Script::parse(&ws.cond).errors.is_empty();
    let ok = ws.mult.trim().parse::<f64>().is_ok() && cond_ok;
    ui.horizontal(|ui| {
        if ui.add_enabled(ok, egui::Button::new(RichText::new("Add the modifier ↓").strong())).clicked() {
            let k = if role == Role::TreeCountry { "add" } else { "factor" };
            let body = format!("{k} = {}\n{}", ws.mult.trim(), ws.cond.trim());
            ws.built = wz::append(&ws.built, &wz::wrap("modifier", &body));
            ws.cond.clear();
        }
        if !cond_ok {
            ui.label(RichText::new("write or build a condition").weak().small());
        }
    });
}

/// Battalions of a division template, put into free places.
fn battalion_page(ui: &mut egui::Ui, cx: &mut Cx, ws: &mut WizardState, t: &Target, support: bool) {
    ui.label(RichText::new(if support { "Support companies: one column of up to 5." } else { "Battalions: 5 columns of up to 5. New ones go into the column with their kind, else the first empty one." }).weak());
    ui.horizontal(|ui| {
        ui.label("Kind");
        let mut v = std::mem::take(&mut ws.battalion);
        param_ui(ui, cx, &mut ws.lists, egui::Id::unique("battalion_kind"), Param::Ref(Kind::SubUnit), &mut v);
        ws.battalion = v;
        ui.label("How many");
        ui.add(egui::TextEdit::singleline(&mut ws.count).desired_width(40.0));
    });
    let have = wz::append(&current(cx, t).unwrap_or_default(), &ws.built);
    let n = ws.count.trim().parse::<usize>().unwrap_or(0);
    let placed = if n == 0 || ws.battalion.trim().is_empty() { Err("a kind and how many".to_string()) } else { wz::place(&have, ws.battalion.trim(), n, support) };
    match placed {
        Ok(lines) => {
            if ui.button(RichText::new("Add ↓").strong()).clicked() {
                ws.built = wz::append(&ws.built, &lines);
            }
        }
        Err(e) => {
            ui.label(RichText::new(e).color(WARN).small());
        }
    }
    // the template as it will be
    let slots = wz::slots(&have);
    let cols = if support { 1 } else { 5 };
    egui::Grid::new("battalion_grid").striped(true).show(ui, |ui| {
        for y in 0..5 {
            for x in 0..cols {
                let s = slots.iter().find(|s| s.1 == x && s.2 == y).map(|s| s.0.as_str()).unwrap_or("·");
                ui.add_sized([120.0, 18.0], egui::Label::new(RichText::new(s).small()).truncate());
            }
            ui.end_row();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::gfx::Gfx;
    use crate::content::store::Store;
    use crate::content::ui::{LocCache, Settings};
    use crate::editor::MapFacts;

    const CHARS: &str = "characters = {\n\tTYR_hofer = {\n\t\tname = TYR_hofer\n\t\tcountry_leader = {\n\t\t\tideology = conservatism\n\t\t\ttraits = { popular_figure }\n\t\t\texpire = \"1965.1.1\"\n\t\t\tdesc = TYR_hofer_desc\n\t\t}\n\t}\n}\n";

    fn with_cx<R>(st: &mut Store, f: impl FnOnce(&mut Cx) -> R) -> R {
        let (mut gfx, mut settings, facts, loc) = (Gfx::default(), Settings::default(), MapFacts::default(), LocCache::default());
        let (mut msg, mut nav) = (None, None);
        let mut cx = Cx { st, base: None, gfx: &mut gfx, settings: &mut settings, facts: &facts, pkg: "TYR", nation: None, msg: &mut msg, nav: &mut nav, loc: &loc, game: std::path::Path::new("/none"), user_dir: std::path::Path::new("/none"), docs: &Default::default() };
        f(&mut cx)
    }

    #[test]
    fn a_form_is_read_from_the_definition_and_written_back() {
        let mut st = Store::empty(std::env::temp_dir().join("mh-wizard-none"));
        st.begin("setup");
        st.set_text("TYR", "common/characters/TYR.txt", CHARS.to_string());
        st.commit();
        let key = st.defs_of(Some("TYR"), Kind::Character).into_iter().next().unwrap();
        with_cx(&mut st, |cx| {
            let mut ws = WizardState::default();
            // the field there
            let t = Target { key: key.clone(), field: "country_leader".into(), nth: Some(0), role: Role::Values, cursor: None };
            init(cx, &mut ws, &t);
            assert_eq!(ws.form.vals["ideology"], "conservatism");
            assert_eq!(ws.form.vals["expire"], "1965.1.1");
            ws.form.vals.insert("traits".into(), "popular_figure war_hero".into());
            let shape = sh::shape_of(Kind::Character, "country_leader").unwrap();
            assert_eq!(sh::write(&shape, &ws.form), "ideology = conservatism\ntraits = { popular_figure war_hero }\nexpire = \"1965.1.1\"\ndesc = TYR_hofer_desc");
            // a new one: starts with the character's id and the nation's tag
            let t = Target { key: key.clone(), field: "advisor".into(), nth: None, role: Role::Values, cursor: None };
            init(cx, &mut ws, &t);
            assert_eq!(ws.form.vals["idea_token"], "TYR_hofer");
            assert_eq!(ws.form.vals["allowed"], "original_tag = TYR");
        });
    }
}
