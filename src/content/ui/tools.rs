//! The project's tools: localisation (coverage per language, side-by-side
//! translation, unused keys), find & replace, renaming and deleting ids,
//! renaming a nation's tag, event chains, a gallery of pictures, a check
//! of sprites, statistics, what changed (against disk and snapshots), what
//! clashes with another mod, and an icon maker.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use crossbeam_channel::Receiver;
use eframe::egui::{self, Color32, RichText, vec2};
use regex::Regex;

use super::form;
use super::{Cx, ERR, Nav, OK, WARN};
use crate::content::import;
use crate::content::kinds::{self, Kind};
use crate::content::loc::{self, LANGUAGES};
use crate::content::refactor::{self, Find, Plan};
use crate::content::source::{self, Files, Index, Origin};
use crate::content::store::{DefKey, SHARED, Store};
use crate::content::zip;
use crate::editor::launcher;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Find,
    Rename,
    TagRename,
    Events,
    Gallery,
    Sprites,
    Stats,
    Changes,
    Compat,
    IconMaker,
}

impl Tool {
    pub const ALL: [Tool; 10] = [Tool::Find, Tool::Rename, Tool::TagRename, Tool::Events, Tool::Gallery, Tool::Sprites, Tool::IconMaker, Tool::Stats, Tool::Changes, Tool::Compat];
    pub fn label(self) -> &'static str {
        match self {
            Tool::Find => "🔍 Find & replace",
            Tool::Rename => "✏ Rename / delete an id",
            Tool::TagRename => "🏷 Rename the tag",
            Tool::Events => "⛓ Event chains",
            Tool::Gallery => "🖼 Pictures",
            Tool::Sprites => "🧩 Sprite check",
            Tool::IconMaker => "🎨 Icon maker",
            Tool::Stats => "📊 Statistics",
            Tool::Changes => "🕘 What changed",
            Tool::Compat => "⚔ Other mods",
        }
    }
}

#[derive(Default)]
pub struct ToolsState {
    loc: LocState,
    find: FindState,
    rename: RenameState,
    tag: TagState,
    events: EventsState,
    gallery: GalleryState,
    sprites: Option<(u64, Arc<SpriteReport>)>,
    stats: Option<(u64, Arc<Stats>)>,
    changes: ChangesState,
    compat: CompatState,
    icon: IconState,
}

/// A key for caches of what is worked out from the content and the base game
/// (which is read in the background, so it can come later).
fn gen_of(cx: &Cx) -> u64 {
    cx.st.generation * 2 + cx.base.is_some() as u64
}

pub fn page(ui: &mut egui::Ui, cx: &mut Cx, t: Tool, ts: &mut ToolsState) {
    ui.heading(t.label());
    match t {
        Tool::Find => find_page(ui, cx, &mut ts.find),
        Tool::Rename => rename_page(ui, cx, &mut ts.rename),
        Tool::TagRename => tag_page(ui, cx, &mut ts.tag),
        Tool::Events => events_page(ui, cx, &mut ts.events),
        Tool::Gallery => gallery_page(ui, cx, &mut ts.gallery),
        Tool::Sprites => sprites_page(ui, cx, &mut ts.sprites),
        Tool::IconMaker => icon_page(ui, cx, &mut ts.icon),
        Tool::Stats => stats_page(ui, cx, &mut ts.stats),
        Tool::Changes => changes_page(ui, cx, &mut ts.changes),
        Tool::Compat => compat_page(ui, cx, &mut ts.compat),
    }
}

// ---------------------------------------------------------------- localisation

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum LocShow {
    #[default]
    All,
    Missing,
    Untranslated,
    Unused,
}

#[derive(Default)]
struct LocState {
    lang: String,
    other: String,
    show: LocShow,
    filter: String,
    new_key: String,
    new_text: String,
    rows: Option<((u64, String, String, String), Arc<Vec<LocRow>>)>,
}

#[derive(Clone)]
struct LocRow {
    key: String,
    text: Option<String>,
    other: Option<String>,
    /// The definition that needs it (and what it is for).
    needed: Option<(DefKey, String)>,
    /// Named in a script file or needed by a definition.
    used: bool,
}

/// The package's texts in a language: key → text.
fn pkg_texts(st: &Store, pkg: &str, lang: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let marker = format!("_l_{lang}.yml");
    if let Some(p) = st.package(pkg) {
        for (path, f) in &p.files {
            if let Some(l) = &f.loc
                && (l.language.as_deref() == Some(lang) || (l.language.is_none() && path.to_ascii_lowercase().ends_with(&marker)))
            {
                for e in &l.entries {
                    out.insert(e.key.clone(), e.text.clone());
                }
            }
        }
    }
    out
}

fn loc_rows(cx: &Cx, lang: &str, other: &str) -> Vec<LocRow> {
    let pkg = cx.pkg;
    let mine = pkg_texts(cx.st, pkg, lang);
    let theirs = pkg_texts(cx.st, pkg, other);
    let mut needed: BTreeMap<String, (DefKey, String)> = BTreeMap::new();
    for k in cx.st.defs().filter(|k| k.pkg == pkg) {
        let block = cx.st.def(&k).and_then(|(d, _, s)| d.block(s));
        for (key, what) in kinds::loc_keys(k.kind, &k.id, block) {
            needed.entry(key).or_insert((k.clone(), what));
        }
    }
    // words of the script files: keys they name
    let mut words: BTreeSet<String> = BTreeSet::new();
    for p in cx.st.packages.values() {
        for (path, f) in &p.files {
            if path.ends_with(".yml") {
                continue;
            }
            if let Some(t) = f.text() {
                for (a, b) in crate::content::script::words(t) {
                    words.insert(t[a..b].trim_matches('"').to_string());
                }
            }
        }
    }
    let keys: BTreeSet<&String> = mine.keys().chain(theirs.keys()).chain(needed.keys()).collect();
    keys.into_iter()
        .map(|k| {
            let need = needed.get(k).cloned();
            // a needed key the base game has counts as there
            let text = mine.get(k).cloned().or_else(|| if lang == "english" && need.is_some() { cx.base.and_then(|b| b.loc.get(k)).cloned() } else { None });
            LocRow { key: k.clone(), text, other: theirs.get(k).cloned(), used: need.is_some() || words.contains(k.as_str()), needed: need }
        })
        .collect()
}

pub fn loc_page(ui: &mut egui::Ui, cx: &mut Cx, ts: &mut ToolsState) {
    let s = &mut ts.loc;
    if s.lang.is_empty() {
        s.lang = cx.settings.language.clone();
    }
    if s.other.is_empty() {
        s.other = if s.lang == "english" { "german".into() } else { "english".into() };
    }
    let key = (gen_of(cx), cx.pkg.to_string(), s.lang.clone(), s.other.clone());
    if s.rows.as_ref().is_none_or(|r| r.0 != key) {
        s.rows = Some((key, Arc::new(loc_rows(cx, &s.lang, &s.other))));
    }
    let rows = s.rows.as_ref().unwrap().1.clone();
    ui.horizontal_wrapped(|ui| {
        ui.heading("Localisation");
        ui.label("texts in");
        lang_combo(ui, "loc_lang", &mut s.lang);
        ui.label("next to");
        lang_combo(ui, "loc_other", &mut s.other);
        if ui.small_button("⇄").clicked() {
            std::mem::swap(&mut s.lang, &mut s.other);
        }
        if cx.settings.language != s.lang && ui.small_button(format!("edit in {} everywhere", s.lang)).on_hover_text("the language the other pages show and edit").clicked() {
            cx.settings.language = s.lang.clone();
        }
    });
    let missing = rows.iter().filter(|r| r.text.is_none()).count();
    let untranslated = rows.iter().filter(|r| r.text.is_some() && r.text == r.other && s.lang != s.other).count();
    let unused = rows.iter().filter(|r| !r.used && r.text.is_some()).count();
    let needed_missing = rows.iter().filter(|r| r.text.is_none() && r.needed.is_some()).count();
    ui.horizontal_wrapped(|ui| {
        ui.selectable_value(&mut s.show, LocShow::All, format!("all {}", rows.len()));
        ui.selectable_value(&mut s.show, LocShow::Missing, RichText::new(format!("{missing} missing")).color(if missing > 0 { WARN } else { OK }));
        ui.selectable_value(&mut s.show, LocShow::Untranslated, format!("{untranslated} same as {}", s.other));
        ui.selectable_value(&mut s.show, LocShow::Unused, format!("{unused} unused"));
        ui.add(egui::TextEdit::singleline(&mut s.filter).hint_text("filter").desired_width(160.0));
    });
    let done = rows.len() - missing;
    if !rows.is_empty() {
        ui.add(egui::ProgressBar::new(done as f32 / rows.len() as f32).text(format!("{done} of {} keys have a {} text ({needed_missing} that definitions need are missing)", rows.len(), s.lang)).desired_width(500.0));
    }
    ui.horizontal_wrapped(|ui| {
        let fill: Vec<(String, String)> = rows.iter().filter(|r| r.text.is_none()).filter_map(|r| r.other.clone().map(|o| (r.key.clone(), o))).collect();
        if ui.add_enabled(!fill.is_empty(), egui::Button::new(format!("Fill {} missing from {}", fill.len(), s.other))).on_hover_text(format!("copies the {} text, so players see words instead of keys; they count as 'same as {}' until translated", s.other, s.other)).clicked() {
            let pkg = cx.pkg.to_string();
            cx.st.begin(format!("{} texts from {}", fill.len(), s.other));
            import::set_locs(cx.st, &pkg, &s.lang, &fill);
            cx.st.commit();
            cx.info(format!("filled {} {} texts from {}", fill.len(), s.lang, s.other));
        }
        let pretty: Vec<(String, String)> = rows.iter().filter(|r| r.text.is_none() && r.other.is_none() && r.needed.is_some()).map(|r| (r.key.clone(), loc::prettify(&r.key))).collect();
        if ui.add_enabled(!pretty.is_empty(), egui::Button::new(format!("Name {} from their ids", pretty.len()))).on_hover_text("texts that no language has: 'TYR_free_tyrol' becomes 'Free Tyrol'").clicked() {
            let pkg = cx.pkg.to_string();
            cx.st.begin(format!("{} texts from ids", pretty.len()));
            import::set_locs(cx.st, &pkg, &s.lang, &pretty);
            cx.st.commit();
        }
    });
    ui.horizontal(|ui| {
        ui.label("new key");
        ui.add(egui::TextEdit::singleline(&mut s.new_key).desired_width(180.0));
        ui.add(egui::TextEdit::singleline(&mut s.new_text).desired_width(280.0).hint_text("text"));
        if ui.add_enabled(!s.new_key.trim().is_empty() && !s.new_key.contains(' '), egui::Button::new("Add")).clicked() {
            let pkg = cx.pkg.to_string();
            form::set_text(cx, &pkg, &s.lang.clone(), s.new_key.trim(), &s.new_text);
            s.new_key.clear();
            s.new_text.clear();
        }
    });
    ui.separator();
    let f = s.filter.to_lowercase();
    let shown: Vec<&LocRow> = rows
        .iter()
        .filter(|r| match s.show {
            LocShow::All => true,
            LocShow::Missing => r.text.is_none(),
            LocShow::Untranslated => r.text.is_some() && r.text == r.other,
            LocShow::Unused => !r.used && r.text.is_some(),
        })
        .filter(|r| f.is_empty() || r.key.to_lowercase().contains(&f) || r.text.as_ref().is_some_and(|t| t.to_lowercase().contains(&f)) || r.other.as_ref().is_some_and(|t| t.to_lowercase().contains(&f)))
        .collect();
    let pkg = cx.pkg.to_string();
    let (lang, other) = (s.lang.clone(), s.other.clone());
    let mut edits: Vec<(String, String)> = Vec::new();
    let mut remove: Option<String> = None;
    let mut go = None;
    let w = ui.available_width();
    egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, 24.0, shown.len(), |ui, range| {
        for r in &shown[range] {
            ui.horizontal(|ui| {
                let kw = (w * 0.26).max(160.0);
                let tw = ((w - kw) / 2.0 - 40.0).max(120.0);
                let key_color = if r.text.is_none() { WARN } else { ui.visuals().text_color() };
                let key_label = ui
                    .allocate_ui_with_layout(vec2(kw, 20.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.set_min_width(kw);
                        ui.add(egui::Label::new(RichText::new(&r.key).monospace().small().color(key_color)).truncate().sense(egui::Sense::click()))
                    })
                    .inner;
                let key_label = match &r.needed {
                    Some((k, what)) => key_label.on_hover_text(format!("{what} of {} (click: edit it)", k.id)),
                    None => key_label.on_hover_text(if r.used { "named in a script" } else { "nothing names it" }),
                };
                if key_label.clicked()
                    && let Some((k, _)) = &r.needed
                {
                    go = Some(Nav::Def(k.clone()));
                }
                let cur = r.text.clone().unwrap_or_default();
                if let Some(v) = form::line_field(ui, egui::Id::unique(("loc_row", &lang, &r.key)), &cur, tw, "missing", &|_| true) {
                    edits.push((r.key.clone(), v));
                }
                let o = r.other.clone().unwrap_or_default();
                if let Some(v) = form::line_field(ui, egui::Id::unique(("loc_row", &other, &r.key)), &o, tw, "missing", &|_| true) {
                    edits.push((format!("\u{0}{}", r.key), v));
                }
                if !r.used && ui.small_button("✖").on_hover_text("delete the key in every language of the nation").clicked() {
                    remove = Some(r.key.clone());
                }
            });
        }
    });
    for (k, v) in edits {
        match k.strip_prefix('\u{0}') {
            Some(k) => form::set_text(cx, &pkg, &other, k, &v),
            None => form::set_text(cx, &pkg, &lang, &k, &v),
        }
    }
    if let Some(k) = remove {
        cx.st.begin(format!("delete the text {k}"));
        import::remove_loc(cx.st, &pkg, &k);
        cx.st.commit();
    }
    if let Some(n) = go {
        cx.go(n);
    }
}

fn lang_combo(ui: &mut egui::Ui, id: &str, lang: &mut String) {
    egui::ComboBox::from_id_salt(id).selected_text(lang.clone()).show_ui(ui, |ui| {
        for l in LANGUAGES {
            ui.selectable_value(lang, l.to_string(), l);
        }
    });
}

// ---------------------------------------------------------------- find & replace

#[derive(Default)]
struct FindState {
    q: Option<Find>,
    with: String,
    hits: Option<Result<Vec<refactor::Hit>, String>>,
    plan: Option<Result<Plan, String>>,
    this_pkg: bool,
}

fn find_page(ui: &mut egui::Ui, cx: &mut Cx, s: &mut FindState) {
    let q = s.q.get_or_insert_with(|| Find { needle: String::new(), regex: false, case: false, whole_word: false, pkg: None, scripts: true, loc: true });
    let mut run = false;
    ui.horizontal_wrapped(|ui| {
        let r = ui.add(egui::TextEdit::singleline(&mut q.needle).hint_text("find").desired_width(260.0));
        run |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        run |= r.changed() && q.needle.len() >= 3;
        run |= ui.checkbox(&mut q.case, "case").changed();
        run |= ui.checkbox(&mut q.whole_word, "whole word").changed();
        run |= ui.checkbox(&mut q.regex, "regex").changed();
        run |= ui.checkbox(&mut q.scripts, "scripts").changed();
        run |= ui.checkbox(&mut q.loc, "texts").changed();
        run |= ui.checkbox(&mut s.this_pkg, format!("only {}", cx.tag())).changed();
        if ui.button("Find").clicked() {
            run = true;
        }
    });
    q.pkg = s.this_pkg.then(|| cx.pkg.to_string());
    if run && !q.needle.is_empty() {
        s.hits = Some(refactor::find(cx.st, q, 2000).map_err(|e| format!("{e:#}")));
        s.plan = None;
    }
    ui.horizontal(|ui| {
        ui.label("replace with");
        ui.add(egui::TextEdit::singleline(&mut s.with).desired_width(260.0)).on_hover_text("with regex: $1 is the first group");
        let n = s.hits.as_ref().and_then(|h| h.as_ref().ok()).map_or(0, Vec::len);
        if ui.add_enabled(n > 0, egui::Button::new("Preview the replace")).clicked() {
            s.plan = Some(refactor::plan_replace(cx.st, q, &s.with).map_err(|e| format!("{e:#}")));
        }
    });
    if let Some(p) = &s.plan {
        match p {
            Ok(p) => {
                let mut close = false;
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.label(RichText::new(format!("{} replacements in {} files", p.count(), p.files.len())).strong());
                    form::plan_preview(ui, p, 8);
                    ui.horizontal(|ui| {
                        if ui.button(RichText::new(format!("Replace {}", p.count())).strong()).clicked() {
                            cx.st.begin(p.label.clone());
                            refactor::apply(cx.st, p);
                            cx.st.commit();
                            cx.info(format!("replaced {} (Ctrl+Z takes it back)", p.count()));
                            close = true;
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                    });
                });
                if close {
                    s.plan = None;
                    s.hits = Some(refactor::find(cx.st, q, 2000).map_err(|e| format!("{e:#}")));
                }
            }
            Err(e) => {
                ui.label(RichText::new(e).color(ERR));
            }
        }
    }
    ui.separator();
    match &s.hits {
        None => {
            ui.label(RichText::new("Finds text in every file of the content: ids, keys, texts.").weak());
        }
        Some(Err(e)) => {
            ui.label(RichText::new(e).color(ERR));
        }
        Some(Ok(hits)) => {
            let files: BTreeSet<(&str, &str)> = hits.iter().map(|h| (h.pkg.as_str(), h.file.as_str())).collect();
            ui.label(format!("{} hits in {} files{}", hits.len(), files.len(), if hits.len() >= 2000 { " (the first 2000)" } else { "" }));
            let mut go = None;
            egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, 20.0, hits.len(), |ui, range| {
                for h in &hits[range] {
                    ui.horizontal(|ui| {
                        if ui.link(RichText::new(format!("{}/{}:{}", h.pkg, h.file.rsplit('/').next().unwrap_or(&h.file), h.line)).monospace().small()).on_hover_text(&h.file).clicked() {
                            go = Some(Nav::File { pkg: h.pkg.clone(), file: h.file.clone(), line: Some(h.line) });
                        }
                        let t = h.text.trim();
                        ui.add(egui::Label::new(RichText::new(t).monospace().small()).truncate());
                    });
                }
            });
            if let Some(n) = go {
                cx.go(n);
            }
        }
    }
}

// ---------------------------------------------------------------- rename / delete an id

#[derive(Default)]
struct RenameState {
    old: String,
    new: String,
    plan: Option<Result<Plan, String>>,
    refs: Option<(String, Vec<(String, String, usize, String)>)>,
    del: Option<Plan>,
}

fn rename_page(ui: &mut egui::Ui, cx: &mut Cx, s: &mut RenameState) {
    ui.label("Renames an id everywhere: where it is defined, where it is named, the texts of it (its name, _desc...) and the events of a namespace.");
    ui.horizontal(|ui| {
        ui.label("id");
        if ui.add(egui::TextEdit::singleline(&mut s.old).desired_width(220.0)).changed() {
            s.plan = None;
            s.refs = None;
            s.del = None;
        }
        ui.label("→");
        if ui.add(egui::TextEdit::singleline(&mut s.new).desired_width(220.0)).changed() {
            s.plan = None;
        }
        if ui.add_enabled(!s.old.is_empty() && !s.new.is_empty(), egui::Button::new("Preview the rename")).clicked() {
            s.plan = Some(refactor::plan_rename(cx.st, s.old.trim(), s.new.trim()).map_err(|e| format!("{e:#}")));
        }
        if ui.add_enabled(!s.old.is_empty(), egui::Button::new("Where is it named?")).clicked() {
            s.refs = Some((s.old.clone(), refactor::references(cx.st, s.old.trim(), None)));
        }
    });
    let kinds = refactor::kinds_named(cx.st, s.old.trim());
    if !s.old.is_empty() {
        ui.horizontal(|ui| {
            if kinds.is_empty() {
                ui.label(RichText::new("the content defines nothing with this id").weak());
            } else {
                ui.label(format!("defined as: {}", kinds.iter().map(|k| k.spec().one).collect::<Vec<_>>().join(", ")));
                if ui.button(RichText::new("Delete it…").color(ERR)).clicked() {
                    let keys: Vec<DefKey> = cx.st.defs().filter(|k| k.id == s.old.trim()).collect();
                    s.del = Some(refactor::plan_delete(cx.st, &keys));
                }
            }
            if cx.base.is_some_and(|b| !b.kinds_of(s.old.trim()).is_empty()) {
                ui.label(RichText::new("(the base game has it too: renaming makes it a new one)").color(WARN));
            }
        });
    }
    if let Some(p) = &s.plan {
        match p {
            Ok(p) => {
                let mut close = false;
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.label(RichText::new(format!("{}: {} changes in {} files", p.label, p.count(), p.files.len())).strong());
                    for n in &p.notes {
                        ui.label(RichText::new(n).color(WARN).small());
                    }
                    form::plan_preview(ui, p, 8);
                    ui.horizontal(|ui| {
                        if ui.button(RichText::new("Rename").strong()).clicked() {
                            cx.st.begin(p.label.clone());
                            refactor::apply(cx.st, p);
                            cx.st.commit();
                            cx.info(format!("{} (Ctrl+Z takes it back)", p.label));
                            close = true;
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                    });
                });
                if close {
                    s.plan = None;
                    s.old = s.new.clone();
                }
            }
            Err(e) => {
                ui.label(RichText::new(e).color(ERR));
            }
        }
    }
    if let Some(p) = &s.del {
        let mut close = false;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label(RichText::new(format!("{}: {} lines in {} files go", p.label, p.files.iter().map(|f| f.lines.len()).sum::<usize>(), p.files.len())).strong());
            form::plan_preview(ui, p, 6);
            if !p.dangling.is_empty() {
                ui.label(RichText::new(format!("{} places still name it and stay as they are:", p.dangling.len())).color(WARN));
                for (pkg, file, line, text) in p.dangling.iter().take(20) {
                    ui.label(RichText::new(format!("{pkg}/{file}:{line}  {text}")).monospace().small());
                }
            }
            ui.horizontal(|ui| {
                if ui.button(RichText::new("Delete").color(ERR)).clicked() {
                    cx.st.begin(p.label.clone());
                    refactor::apply(cx.st, p);
                    cx.st.commit();
                    cx.info(format!("{} (Ctrl+Z brings it back)", p.label));
                    close = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        if close {
            s.del = None;
        }
    }
    if let Some((id, refs)) = &s.refs {
        ui.separator();
        ui.label(format!("{id} is named in {} places", refs.len()));
        let mut go = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for (pkg, file, line, text) in refs {
                ui.horizontal(|ui| {
                    if ui.link(RichText::new(format!("{pkg}/{}:{line}", file.rsplit('/').next().unwrap_or(file))).monospace().small()).clicked() {
                        go = Some(Nav::File { pkg: pkg.clone(), file: file.clone(), line: Some(*line) });
                    }
                    ui.add(egui::Label::new(RichText::new(text.trim()).monospace().small()).truncate());
                });
            }
        });
        if let Some(n) = go {
            cx.go(n);
        }
    }
}

// ---------------------------------------------------------------- tag rename

#[derive(Default)]
struct TagState {
    to: String,
    plan: Option<Result<(Plan, Vec<(String, String)>), String>>,
}

fn tag_page(ui: &mut egui::Ui, cx: &mut Cx, s: &mut TagState) {
    let from = cx.pkg.to_string();
    if from == SHARED {
        ui.label("The shared content has no tag.");
        return;
    }
    ui.label(format!("Renames {from} in the content of the nation: ids (TAG_x, TAG.x), file names, texts, and the nation's tag in the map."));
    ui.horizontal(|ui| {
        ui.label(format!("{from} →"));
        let r = ui.add(egui::TextEdit::singleline(&mut s.to).desired_width(60.0).char_limit(3));
        if r.changed() {
            s.to = s.to.to_ascii_uppercase();
            s.plan = None;
        }
        let taken = cx.facts.nations.iter().any(|n| n.tag == s.to) || cx.base.is_some_and(|b| b.tags.contains(&s.to));
        if taken {
            ui.label(RichText::new(format!("{} is taken", s.to)).color(ERR));
        }
        if ui.add_enabled(kinds::is_tag(&s.to) && !taken, egui::Button::new("Preview")).clicked() {
            s.plan = Some(refactor::plan_tag_rename(cx.st, &from, &s.to).map_err(|e| format!("{e:#}")));
        }
    });
    let Some(p) = &s.plan else { return };
    match p {
        Ok((plan, moves)) => {
            ui.label(RichText::new(format!("{} changes in {} files; {} files are renamed", plan.count(), plan.files.len(), moves.len())).strong());
            for (a, b) in moves.iter().take(12) {
                ui.label(RichText::new(format!("{a} → {b}")).monospace().small());
            }
            form::plan_preview(ui, plan, 6);
            ui.label(RichText::new("The map gets the new tag too; save both (Ctrl+S) to keep it.").weak());
            if ui.button(RichText::new(format!("Rename {from} to {}", s.to)).strong()).clicked() {
                let to = s.to.clone();
                cx.st.begin(plan.label.clone());
                match refactor::apply_tag_rename(cx.st, plan, moves, &from, &to) {
                    Ok(()) => {
                        cx.st.commit();
                        cx.info(format!("{} (fix the tag in the map: the nation's properties)", plan.label));
                        cx.go(Nav::Pkg(to.clone()));
                        cx.go(Nav::Pkg(to));
                    }
                    Err(e) => {
                        cx.st.rollback();
                        cx.error(format!("{e:#}"));
                    }
                }
                s.plan = None;
            }
        }
        Err(e) => {
            ui.label(RichText::new(e).color(ERR));
        }
    }
}

// ---------------------------------------------------------------- event chains

#[derive(Default)]
struct EventsState {
    graph: Option<(u64, Arc<EventGraph>)>,
    filter: String,
    all_pkgs: bool,
}

#[derive(Default)]
struct EventGraph {
    /// Event → events it fires.
    fires: BTreeMap<String, BTreeSet<String>>,
    /// What fires an event: (kind, id, key).
    fired_by: BTreeMap<String, Vec<(Kind, String, DefKey)>>,
    events: BTreeMap<String, DefKey>,
}

fn event_graph(st: &Store) -> EventGraph {
    let re = Regex::new(r"(?s)\b(?:country_event|news_event|state_event|unit_leader_event|operative_leader_event)\s*=\s*(?:\{[^{}]*?\bid\s*=\s*([\w.\-]+)|([\w.\-]+))").unwrap();
    let mut g = EventGraph::default();
    for k in st.defs() {
        if k.kind == Kind::Event {
            g.events.insert(k.id.clone(), k.clone());
        }
    }
    for k in st.defs() {
        if !matches!(k.kind, Kind::Event | Kind::Focus | Kind::SharedFocus | Kind::Decision | Kind::OnAction | Kind::ScriptedEffect | Kind::Idea | Kind::CountryHistory) {
            continue;
        }
        let Some((d, text, _)) = st.def(&k) else { continue };
        let body = &text[d.span.clone()];
        for c in re.captures_iter(body) {
            let id = c.get(1).or(c.get(2)).map(|m| m.as_str().to_string()).unwrap_or_default();
            if id.is_empty() || id == k.id {
                continue;
            }
            if k.kind == Kind::Event {
                g.fires.entry(k.id.clone()).or_default().insert(id.clone());
            }
            g.fired_by.entry(id).or_default().push((k.kind, k.id.clone(), k.clone()));
        }
    }
    g
}

fn events_page(ui: &mut egui::Ui, cx: &mut Cx, s: &mut EventsState) {
    if s.graph.as_ref().is_none_or(|g| g.0 != cx.st.generation) {
        s.graph = Some((cx.st.generation, Arc::new(event_graph(cx.st))));
    }
    let g = s.graph.as_ref().unwrap().1.clone();
    ui.label("Which events fire which: a chain starts where a focus, a decision, an on_action or nothing fires it.");
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut s.filter).hint_text("filter").desired_width(200.0));
        ui.checkbox(&mut s.all_pkgs, "all nations");
    });
    let pkg = cx.pkg.to_string();
    let f = s.filter.to_lowercase();
    // roots: events of the package not fired by another event
    let roots: Vec<&String> = g
        .events
        .iter()
        .filter(|(_, k)| s.all_pkgs || k.pkg == pkg)
        .filter(|(id, _)| !g.fired_by.get(*id).is_some_and(|v| v.iter().any(|x| x.0 == Kind::Event)))
        .filter(|(id, _)| g.fires.contains_key(*id) || g.fired_by.contains_key(*id))
        .map(|(id, _)| id)
        .filter(|id| f.is_empty() || chain_has(&g, id, &f, &mut BTreeSet::new()))
        .collect();
    let lonely = g.events.iter().filter(|(_, k)| s.all_pkgs || k.pkg == pkg).filter(|(id, _)| !g.fires.contains_key(*id) && !g.fired_by.contains_key(*id)).count();
    ui.label(RichText::new(format!("{} chains; {lonely} events neither fire nor are fired by anything", roots.len())).weak());
    ui.separator();
    let mut go = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for r in roots {
            let by = g.fired_by.get(r).map(|v| v.iter().map(|x| format!("{} {}", x.0.spec().one, x.1)).collect::<Vec<_>>().join(", ")).unwrap_or_else(|| "nothing fires it".into());
            ui.label(RichText::new(format!("from {by}")).small().weak());
            chain_ui(ui, cx, &g, r, 0, &mut BTreeSet::new(), &mut go);
            ui.add_space(4.0);
        }
    });
    if let Some(n) = go {
        cx.go(n);
    }
}

fn chain_has(g: &EventGraph, id: &str, f: &str, seen: &mut BTreeSet<String>) -> bool {
    if !seen.insert(id.to_string()) {
        return false;
    }
    id.to_lowercase().contains(f) || g.fires.get(id).is_some_and(|v| v.iter().any(|c| chain_has(g, c, f, seen)))
}

fn chain_ui(ui: &mut egui::Ui, cx: &Cx, g: &EventGraph, id: &str, depth: usize, seen: &mut BTreeSet<String>, go: &mut Option<Nav>) {
    ui.horizontal(|ui| {
        ui.add_space(depth as f32 * 18.0);
        ui.label(if depth == 0 { "●" } else { "↳" });
        let name = cx.text(&format!("{id}.t")).map(loc::plain).unwrap_or_default();
        match g.events.get(id) {
            Some(k) => {
                if ui.link(RichText::new(id).monospace()).clicked() {
                    *go = Some(Nav::Def(k.clone()));
                }
            }
            None => {
                let base = cx.base.is_some_and(|b| b.has(Kind::Event, id));
                ui.label(RichText::new(id).monospace().color(if base { ui.visuals().weak_text_color() } else { ERR })).on_hover_text(if base { "an event of the base game" } else { "no such event" });
            }
        }
        if !name.is_empty() {
            ui.label(RichText::new(name).weak());
        }
    });
    if !seen.insert(id.to_string()) {
        ui.horizontal(|ui| {
            ui.add_space((depth + 1) as f32 * 18.0);
            ui.label(RichText::new("(goes round)").color(WARN).small());
        });
        return;
    }
    if depth > 30 {
        return;
    }
    if let Some(kids) = g.fires.get(id) {
        for k in kids {
            chain_ui(ui, cx, g, k, depth + 1, seen, go);
        }
    }
}

// ---------------------------------------------------------------- gallery

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Pics {
    #[default]
    Focus,
    Idea,
    Event,
    Portrait,
    All,
}

#[derive(Default)]
struct GalleryState {
    which: Pics,
    filter: String,
    base_too: bool,
    names: Option<((u64, bool, Pics, String), Arc<Vec<(String, bool)>>)>,
}

fn pics_of(p: Pics, name: &str, texture: &str) -> bool {
    let n = name.to_ascii_lowercase();
    let t = texture.to_ascii_lowercase();
    // the shining copy of a focus icon (shown on completion) is the same picture
    if p != Pics::All && n.ends_with("_shine") {
        return false;
    }
    match p {
        Pics::Focus => n.starts_with("gfx_goal_") || t.contains("/goals/"),
        Pics::Idea => n.starts_with("gfx_idea_") || t.contains("/ideas/"),
        Pics::Event => t.contains("event_pictures") || n.starts_with("gfx_report_event") || n.starts_with("gfx_news_event"),
        Pics::Portrait => t.contains("/leaders/") || t.contains("portrait") || n.contains("portrait"),
        Pics::All => true,
    }
}

fn gallery_page(ui: &mut egui::Ui, cx: &mut Cx, s: &mut GalleryState) {
    if !s.base_too && s.names.is_none() {
        s.base_too = true;
    }
    ui.horizontal_wrapped(|ui| {
        for (p, l) in [(Pics::Focus, "focus icons"), (Pics::Idea, "idea pictures"), (Pics::Event, "event pictures"), (Pics::Portrait, "portraits"), (Pics::All, "all sprites")] {
            ui.selectable_value(&mut s.which, p, l);
        }
        ui.separator();
        ui.checkbox(&mut s.base_too, "the base game's too");
        ui.add(egui::TextEdit::singleline(&mut s.filter).hint_text("filter").desired_width(160.0));
    });
    let key = (gen_of(cx), s.base_too, s.which, s.filter.to_lowercase());
    if s.names.as_ref().is_none_or(|n| n.0 != key) {
        let own = crate::content::gfx::store_sprites(cx.st);
        let f = s.filter.to_lowercase();
        let mut v: Vec<(String, bool)> = own.iter().filter(|(n, (_, t, _))| pics_of(s.which, n, t)).map(|(n, _)| (n.clone(), true)).collect();
        if s.base_too
            && let Some(b) = cx.base
        {
            v.extend(b.sprites.iter().filter(|(n, (t, _))| !own.contains_key(*n) && pics_of(s.which, n, t)).map(|(n, _)| (n.clone(), false)));
        }
        v.retain(|(n, _)| f.is_empty() || n.to_lowercase().contains(&f));
        v.sort();
        s.names = Some((key, Arc::new(v)));
    }
    let names = s.names.as_ref().unwrap().1.clone();
    ui.label(RichText::new(format!("{} pictures; click one to copy its name (the project's are marked ★)", names.len())).weak());
    let cell = if s.which == Pics::Event { vec2(220.0, 200.0) } else { vec2(110.0, 120.0) };
    let per_row = ((ui.available_width() / (cell.x + 6.0)).floor() as usize).max(1);
    let rows = names.len().div_ceil(per_row);
    egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, cell.y + 6.0, rows, |ui, range| {
        for r in range {
            ui.horizontal(|ui| {
                for (n, own) in names.iter().skip(r * per_row).take(per_row) {
                    ui.allocate_ui(cell, |ui| {
                        ui.vertical_centered(|ui| {
                            let tex = cx.texture(ui.ctx(), n);
                            let resp = crate::content::gfx::show(ui, tex, cell - vec2(10.0, 26.0)).interact(egui::Sense::click());
                            let label = if *own { format!("★ {n}") } else { n.clone() };
                            ui.add(egui::Label::new(RichText::new(label).small()).truncate());
                            if resp.on_hover_text(n).clicked() {
                                ui.ctx().copy_text(n.clone());
                                cx.info(format!("copied {n}"));
                            }
                        });
                    });
                }
            });
        }
    });
}

// ---------------------------------------------------------------- sprite check

#[derive(Default)]
struct SpriteReport {
    /// Sprites named by definitions that nothing defines: (sprite, field, def).
    missing: Vec<(String, String, DefKey)>,
    /// Portraits named that nothing defines (the game shows a generic face;
    /// the base game does this too).
    portraits: Vec<(String, String, DefKey)>,
    /// Sprites of the project whose picture is not there: (sprite, texture, pkg, file).
    no_file: Vec<(String, String, String, String)>,
    /// Sprites of the project nothing names.
    unused: Vec<(String, String)>,
    named: usize,
}

fn sprite_report(cx: &Cx) -> SpriteReport {
    let own = crate::content::gfx::store_sprites(cx.st);
    let mut r = SpriteReport::default();
    let mut named: BTreeSet<String> = BTreeSet::new();
    for k in cx.st.defs() {
        let Some((d, _, s)) = cx.st.def(&k) else { continue };
        let Some(b) = d.block(s) else { continue };
        for (field, name) in kinds::sprites_of(k.kind, b) {
            named.insert(name.clone());
            if !own.contains_key(&name) && !cx.base.is_some_and(|b| b.sprites.contains_key(&name)) {
                let to = if k.kind == Kind::Character { &mut r.portraits } else { &mut r.missing };
                to.push((name, field.to_string(), k.clone()));
            }
        }
    }
    // words of all script files: sprites named elsewhere (interface, scripted gui)
    let mut words: BTreeSet<String> = BTreeSet::new();
    for p in cx.st.packages.values() {
        for (path, f) in &p.files {
            if path.ends_with(".gfx") {
                continue;
            }
            if let Some(t) = f.text() {
                for (a, b) in crate::content::script::words(t) {
                    words.insert(t[a..b].trim_matches('"').to_string());
                }
            }
        }
    }
    let sources: Vec<&Index> = cx.base.into_iter().collect();
    for (name, (pkg, tex, _)) in &own {
        if crate::content::gfx::find_file(tex, cx.st, &sources).is_none() {
            let file = cx.st.find_in(pkg, &[Kind::Sprite], name).map(|k| k.file).unwrap_or_default();
            r.no_file.push((name.clone(), tex.clone(), pkg.clone(), file));
        }
        let short = name.strip_prefix("GFX_").unwrap_or(name);
        if !named.contains(name) && !words.contains(name) && !words.contains(short) && !name.ends_with("_shine") {
            r.unused.push((name.clone(), pkg.clone()));
        }
    }
    r.missing.sort_by(|a, b| a.0.cmp(&b.0));
    r.portraits.sort_by(|a, b| a.0.cmp(&b.0));
    r.no_file.sort();
    r.unused.sort();
    r.named = named.len();
    r
}

fn sprites_page(ui: &mut egui::Ui, cx: &mut Cx, cache: &mut Option<(u64, Arc<SpriteReport>)>) {
    if cache.as_ref().is_none_or(|c| c.0 != gen_of(cx)) {
        *cache = Some((gen_of(cx), Arc::new(sprite_report(cx))));
    }
    let r = cache.as_ref().unwrap().1.clone();
    ui.label(format!("The content names {} sprites.", r.named));
    let mut go = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        let list = |ui: &mut egui::Ui, rows: &[(String, String, DefKey)], go: &mut Option<Nav>| {
            for (name, field, k) in rows {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(name).monospace());
                    if ui.link(format!("{field} of {}", k.id)).clicked() {
                        *go = Some(Nav::Def(k.clone()));
                    }
                });
            }
        };
        ui.collapsing(RichText::new(format!("{} named but defined nowhere (a blank icon in the game)", r.missing.len())).color(if r.missing.is_empty() { OK } else { ERR }), |ui| list(ui, &r.missing, &mut go));
        if !r.portraits.is_empty() {
            ui.collapsing(RichText::new(format!("{} portraits defined nowhere (the game shows a generic face)", r.portraits.len())).color(WARN), |ui| {
                ui.label(RichText::new("The base game names portraits it does not have too. Make one with the icon maker to give the character a face.").weak().small());
                list(ui, &r.portraits, &mut go)
            });
        }
        ui.collapsing(RichText::new(format!("{} defined, but their picture is not there", r.no_file.len())).color(if r.no_file.is_empty() { OK } else { ERR }), |ui| {
            if !r.no_file.is_empty() && ui.button(format!("Remove these {} sprites", r.no_file.len())).on_hover_text("their names become undefined, so the check above shows who still names them").clicked() {
                let keys: Vec<DefKey> = r.no_file.iter().filter_map(|(n, _, p, _)| cx.st.find_in(p, &[Kind::Sprite], n)).collect();
                let plan = refactor::plan_delete(cx.st, &keys);
                cx.st.begin(format!("remove {} sprites with no picture", keys.len()));
                refactor::apply(cx.st, &plan);
                cx.st.commit();
            }
            for (name, tex, pkg, file) in &r.no_file {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(name).monospace());
                    ui.label(RichText::new(tex).small().color(ERR));
                    if !file.is_empty() && ui.small_button("→").clicked() {
                        go = Some(Nav::File { pkg: pkg.clone(), file: file.clone(), line: None });
                    }
                });
            }
        });
        ui.collapsing(format!("{} defined that nothing in the content names", r.unused.len()), |ui| {
            for (name, pkg) in &r.unused {
                ui.horizontal(|ui| {
                    let tex = cx.texture(ui.ctx(), name);
                    crate::content::gfx::show(ui, tex, vec2(32.0, 32.0));
                    ui.label(RichText::new(name).monospace());
                    ui.label(RichText::new(pkg).weak().small());
                });
            }
        });
    });
    if let Some(n) = go {
        cx.go(n);
    }
}

// ---------------------------------------------------------------- statistics

#[derive(Default)]
struct Stats {
    by_pkg: BTreeMap<String, BTreeMap<Kind, usize>>,
    files: usize,
    bytes: u64,
    biggest: Vec<(u64, String, String)>,
    trees: Vec<(String, String, usize, usize, Option<(String, usize)>)>,
    loc: BTreeMap<String, usize>,
}

fn stats_of(cx: &Cx) -> Stats {
    let mut s = Stats::default();
    for k in cx.st.defs() {
        *s.by_pkg.entry(k.pkg.clone()).or_default().entry(k.kind).or_default() += 1;
    }
    for (pkg, p) in &cx.st.packages {
        for (path, f) in &p.files {
            s.files += 1;
            let n = cx.st.bytes(pkg, path).map_or(0, |b| b.len() as u64);
            s.bytes += n;
            s.biggest.push((n, pkg.clone(), path.clone()));
            if let Some(l) = &f.loc {
                let lang = l.language.clone().or_else(|| loc::language_of_name(path).map(String::from)).unwrap_or_else(|| "?".into());
                *s.loc.entry(lang).or_default() += l.entries.len();
            }
        }
    }
    s.biggest.sort_by(|a, b| b.cmp(a));
    s.biggest.truncate(12);
    for k in cx.st.defs_of(None, Kind::FocusTree) {
        let Some((d, _, sc)) = cx.st.def(&k) else { continue };
        let Some(b) = d.block(sc) else { continue };
        let t = crate::content::focus::tree_of(b);
        let depth = t.positions().values().map(|p| p.1).max().unwrap_or(0) as usize + 1;
        let widest = t.focuses.iter().map(|f| (f.id.clone(), t.children(&f.id).len())).max_by_key(|x| x.1).filter(|x| x.1 > 0);
        s.trees.push((k.pkg.clone(), k.id.clone(), t.focuses.len(), depth, widest));
    }
    s
}

fn stats_page(ui: &mut egui::Ui, cx: &mut Cx, cache: &mut Option<(u64, Arc<Stats>)>) {
    if cache.as_ref().is_none_or(|c| c.0 != cx.st.generation) {
        *cache = Some((cx.st.generation, Arc::new(stats_of(cx))));
    }
    let s = cache.as_ref().unwrap().1.clone();
    ui.label(format!("{} files, {:.1} MB", s.files, s.bytes as f64 / 1e6));
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        let kinds: BTreeSet<Kind> = s.by_pkg.values().flat_map(|m| m.keys().copied()).collect();
        egui::Grid::new("stats_grid").striped(true).show(ui, |ui| {
            ui.label(RichText::new("nation").strong());
            for k in &kinds {
                ui.label(RichText::new(k.spec().label).strong().small());
            }
            ui.end_row();
            for (pkg, m) in &s.by_pkg {
                ui.label(pkg);
                for k in &kinds {
                    ui.label(m.get(k).map(|n| n.to_string()).unwrap_or_default());
                }
                ui.end_row();
            }
        });
        ui.add_space(8.0);
        ui.label(RichText::new("Focus trees").strong());
        for (pkg, id, n, depth, widest) in &s.trees {
            ui.label(format!("{pkg} {id}: {n} focuses, {depth} rows{}", widest.as_ref().map(|(f, c)| format!(", {f} opens the most ({c})")).unwrap_or_default()));
        }
        ui.add_space(8.0);
        ui.label(RichText::new("Texts by language").strong());
        for (l, n) in &s.loc {
            ui.label(format!("{l}: {n}"));
        }
        ui.add_space(8.0);
        ui.label(RichText::new("The biggest files").strong());
        for (n, pkg, path) in &s.biggest {
            ui.label(RichText::new(format!("{:>8.1} kB  {pkg}/{path}", *n as f64 / 1e3)).monospace().small());
        }
    });
}

// ---------------------------------------------------------------- what changed

#[derive(Default)]
struct ChangesState {
    sel: Option<(String, String)>,
    snapshots: Option<Vec<PathBuf>>,
    snap: Option<(PathBuf, Vec<(String, String, Option<String>, Option<String>)>)>,
}

fn snapshot_dir(st: &Store) -> PathBuf {
    st.root.join(".snapshots")
}

fn changes_page(ui: &mut egui::Ui, cx: &mut Cx, s: &mut ChangesState) {
    // files changed since the last save
    let mut changed: Vec<(String, String, &'static str)> = Vec::new();
    for (pkg, p) in &cx.st.packages {
        for (path, f) in &p.files {
            if f.dirty() {
                changed.push((pkg.clone(), path.clone(), if f.disk.is_none() { "new" } else { "changed" }));
            }
        }
        for r in &p.removed {
            changed.push((pkg.clone(), r.clone(), "deleted"));
        }
    }
    ui.label(RichText::new(format!("Not saved yet: {} files", changed.len())).strong());
    egui::Panel::left("changes_files").default_size(300.0).max_size(ui.available_width() * 0.4).show(ui, |ui| {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for (pkg, path, what) in &changed {
                let sel = s.sel.as_ref() == Some(&(pkg.clone(), path.clone()));
                let c = match *what {
                    "new" => OK,
                    "deleted" => ERR,
                    _ => WARN,
                };
                if ui.selectable_label(sel, RichText::new(format!("{what}: {pkg}/{path}")).color(c).small()).clicked() {
                    s.sel = Some((pkg.clone(), path.clone()));
                }
            }
            ui.separator();
            ui.label(RichText::new("Snapshots").strong());
            ui.label(RichText::new("All texts of the content as they are now, to go back to later.").weak().small());
            if ui.button("Take a snapshot").clicked() {
                match take_snapshot(cx.st) {
                    Ok(p) => {
                        cx.info(format!("snapshot {}", p.display()));
                        s.snapshots = None;
                    }
                    Err(e) => cx.error(format!("{e:#}")),
                }
            }
            if s.snapshots.is_none() {
                let mut v: Vec<PathBuf> = std::fs::read_dir(snapshot_dir(cx.st)).map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "zip")).collect()).unwrap_or_default();
                v.sort();
                v.reverse();
                s.snapshots = Some(v);
            }
            for p in s.snapshots.clone().unwrap_or_default() {
                let name = p.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                if ui.selectable_label(s.snap.as_ref().is_some_and(|x| x.0 == p), name).clicked() {
                    match snapshot_diff(cx.st, &p) {
                        Ok(d) => s.snap = Some((p.clone(), d)),
                        Err(e) => cx.error(format!("{e:#}")),
                    }
                }
            }
        });
    });
    egui::CentralPanel::default().show(ui, |ui| {
        if let Some((p, diff)) = &s.snap {
            ui.label(RichText::new(format!("Snapshot {}: {} files differ from now", p.file_stem().unwrap_or_default().to_string_lossy(), diff.len())).strong());
            let mut close = false;
            ui.horizontal(|ui| {
                if ui.add_enabled(!diff.is_empty(), egui::Button::new("Go back to it")).on_hover_text("one step: Ctrl+Z comes back here; nothing is written until you save").clicked() {
                    cx.st.begin(format!("back to the snapshot {}", p.file_stem().unwrap_or_default().to_string_lossy()));
                    for (pkg, file, then, _) in diff {
                        match then {
                            Some(t) => cx.st.set_text(pkg, file, t.clone()),
                            None => cx.st.remove(pkg, file),
                        }
                    }
                    cx.st.commit();
                    close = true;
                }
                if ui.button("Close").clicked() {
                    close = true;
                }
            });
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                for (pkg, file, then, now) in diff {
                    let lines = refactor::line_diff(now.as_deref().unwrap_or(""), then.as_deref().unwrap_or(""));
                    ui.collapsing(format!("{pkg}/{file}: {} lines", lines.len()), |ui| diff_lines(ui, &lines, 200));
                }
            });
            if close {
                s.snap = None;
            }
            return;
        }
        let Some((pkg, path)) = &s.sel else {
            ui.label(RichText::new("Pick a file to see what changed in it.").weak());
            return;
        };
        let f = cx.st.file(pkg, path);
        let now = f.and_then(|f| f.text()).unwrap_or("");
        let disk = f.and_then(|f| f.disk.as_ref()).and_then(|d| d.text()).map(str::to_string).or_else(|| std::fs::read_to_string(cx.st.root.join(pkg).join(path)).ok()).unwrap_or_default();
        let lines = refactor::line_diff(&disk, now);
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{pkg}/{path}: {} lines differ from the disk", lines.len())).strong());
            if ui.small_button("open").clicked() {
                cx.go(Nav::File { pkg: pkg.clone(), file: path.clone(), line: lines.first().map(|l| l.line) });
            }
        });
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| diff_lines(ui, &lines, 2000));
    });
}

fn diff_lines(ui: &mut egui::Ui, lines: &[refactor::Line], max: usize) {
    for l in lines.iter().take(max) {
        if !l.before.is_empty() {
            ui.label(RichText::new(format!("{:>5} - {}", l.line, l.before)).monospace().small().color(ERR));
        }
        if let Some(a) = &l.after {
            ui.label(RichText::new(format!("{:>5} + {a}", l.line)).monospace().small().color(OK));
        }
    }
    if lines.len() > max {
        ui.label(RichText::new(format!("… {} more", lines.len() - max)).weak());
    }
}

fn take_snapshot(st: &Store) -> anyhow::Result<PathBuf> {
    let mut files = Vec::new();
    for (pkg, p) in &st.packages {
        for (path, f) in &p.files {
            if let Some(t) = f.text() {
                files.push((format!("{pkg}/{path}"), t.as_bytes().to_vec()));
            }
        }
    }
    let dir = snapshot_dir(st);
    std::fs::create_dir_all(&dir)?;
    let mut p = dir.join(format!("{}.zip", crate::editor::model::timestamp()));
    let mut k = 2;
    while p.exists() {
        p = dir.join(format!("{}-{k}.zip", crate::editor::model::timestamp()));
        k += 1;
    }
    std::fs::write(&p, zip::write(&files)?)?;
    Ok(p)
}

/// Files that differ between a snapshot and now: (pkg, file, then, now).
fn snapshot_diff(st: &Store, path: &std::path::Path) -> anyhow::Result<Vec<(String, String, Option<String>, Option<String>)>> {
    let entries = zip::read(&std::fs::read(path)?)?;
    let mut then: BTreeMap<(String, String), String> = BTreeMap::new();
    for (name, data) in entries {
        if let Some((pkg, file)) = name.split_once('/') {
            then.insert((pkg.to_string(), file.to_string()), String::from_utf8_lossy(&data).into_owned());
        }
    }
    let mut now: BTreeMap<(String, String), String> = BTreeMap::new();
    for (pkg, p) in &st.packages {
        for (file, f) in &p.files {
            if let Some(t) = f.text() {
                now.insert((pkg.clone(), file.clone()), t.to_string());
            }
        }
    }
    let keys: BTreeSet<&(String, String)> = then.keys().chain(now.keys()).collect();
    Ok(keys
        .into_iter()
        .filter(|k| then.get(*k) != now.get(*k))
        .map(|k| (k.0.clone(), k.1.clone(), then.get(k).cloned(), now.get(k).cloned()))
        .collect())
}

// ---------------------------------------------------------------- other mods

#[derive(Default)]
struct CompatState {
    mods: Option<Result<Vec<launcher::ModEntry>, String>>,
    pick: Option<usize>,
    job: Option<(PathBuf, Arc<AtomicU32>, Receiver<Result<Index, String>>)>,
    result: Option<(String, Arc<Index>)>,
    err: Option<String>,
}

fn compat_page(ui: &mut egui::Ui, cx: &mut Cx, s: &mut CompatState) {
    ui.label("What the content shares with another mod: ids both define (the one loaded last wins), files both have, tags both make. Nothing is changed.");
    if s.mods.is_none() {
        s.mods = Some(launcher::scan(cx.user_dir).map_err(|e| format!("{e:#}")));
    }
    if let Some((_, _, rx)) = &s.job
        && let Ok(r) = rx.try_recv()
    {
        let label = s.job.as_ref().map(|j| j.0.display().to_string()).unwrap_or_default();
        s.job = None;
        match r {
            Ok(ix) => s.result = Some((label, Arc::new(ix))),
            Err(e) => s.err = Some(e),
        }
    }
    let mods = match &s.mods {
        Some(Ok(m)) => m.iter().filter(|m| m.present && m.path.is_some()).cloned().collect::<Vec<_>>(),
        Some(Err(e)) => {
            ui.label(RichText::new(e).color(ERR));
            return;
        }
        None => Vec::new(),
    };
    ui.horizontal(|ui| {
        ui.label("mod");
        let cur = s.pick.and_then(|i| mods.get(i)).map(|m| m.name.clone()).unwrap_or_else(|| "pick one".into());
        egui::ComboBox::from_id_salt("compat_mod").selected_text(cur).height(400.0).show_ui(ui, |ui| {
            for (i, m) in mods.iter().enumerate() {
                ui.selectable_value(&mut s.pick, Some(i), &m.name);
            }
        });
        if ui.add_enabled(s.pick.is_some() && s.job.is_none(), egui::Button::new("Compare")).clicked()
            && let Some(m) = s.pick.and_then(|i| mods.get(i))
            && let Some(path) = m.path.clone()
        {
            let progress = Arc::new(AtomicU32::new(0));
            let (tx, rx) = crossbeam_channel::bounded(1);
            let (p, c, name, path2) = (progress.clone(), ui.ctx().clone(), m.name.clone(), path.clone());
            std::thread::spawn(move || {
                let r = (|| -> anyhow::Result<Index> {
                    let files = if path2.is_file() {
                        let m: BTreeMap<String, Arc<Vec<u8>>> = zip::read(&std::fs::read(&path2)?)?.into_iter().map(|(n, d)| (n, Arc::new(d))).collect();
                        Files::Memory(Arc::new(m))
                    } else {
                        Files::Folder(path2.clone())
                    };
                    source::index(&name, Origin::Mod(name.clone()), files, &|f| {
                        p.store(f.to_bits(), Ordering::Relaxed);
                        c.request_repaint();
                    })
                })();
                let _ = tx.send(r.map_err(|e| format!("{e:#}")));
                c.request_repaint();
            });
            s.job = Some((path, progress, rx));
            s.err = None;
            s.result = None;
        }
    });
    if let Some((_, p, _)) = &s.job {
        ui.add(egui::ProgressBar::new(f32::from_bits(p.load(Ordering::Relaxed))).text("reading the mod…"));
        return;
    }
    if let Some(e) = &s.err {
        ui.label(RichText::new(e).color(ERR));
    }
    let Some((label, ix)) = &s.result else { return };
    // ids both define
    let mut ids: Vec<(Kind, String, DefKey)> = Vec::new();
    for k in cx.st.defs() {
        if k.kind.spec().unique && ix.has(k.kind, &k.id) {
            ids.push((k.kind, k.id.clone(), k.clone()));
        }
    }
    let mut files: Vec<(String, String)> = Vec::new();
    for (pkg, p) in &cx.st.packages {
        for path in p.files.keys() {
            if ix.files.exists(path) {
                files.push((pkg.clone(), path.clone()));
            }
        }
    }
    let ours: BTreeSet<&str> = cx.facts.nations.iter().map(|n| n.tag.as_str()).collect();
    let base_tags: BTreeSet<&String> = cx.base.map(|b| b.tags.iter().collect()).unwrap_or_default();
    let tags: Vec<&String> = ix.tags.iter().filter(|t| ours.contains(t.as_str()) && !base_tags.contains(t)).collect();
    ui.label(RichText::new(format!("{label}: {} ids, {} files, {} tags in common", ids.len(), files.len(), tags.len())).strong().color(if ids.is_empty() && files.is_empty() && tags.is_empty() { OK } else { WARN }));
    let mut go = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.collapsing(format!("{} ids both define", ids.len()), |ui| {
            for (k, id, key) in &ids {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(k.spec().one).small().weak());
                    if ui.link(RichText::new(id).monospace()).clicked() {
                        go = Some(Nav::Def(key.clone()));
                    }
                });
            }
        });
        ui.collapsing(format!("{} files both have (the one loaded last replaces the other whole)", files.len()), |ui| {
            for (pkg, f) in &files {
                ui.label(RichText::new(format!("{pkg}/{f}")).monospace().small());
            }
        });
        ui.collapsing(format!("{} tags of the map the mod uses too", tags.len()), |ui| {
            for t in &tags {
                ui.label(t.as_str());
            }
        });
    });
    if let Some(n) = go {
        cx.go(n);
    }
}

// ---------------------------------------------------------------- icon maker

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
enum IconKind {
    #[default]
    Focus,
    Idea,
    Event,
    Portrait,
}

impl IconKind {
    fn size(self) -> (u32, u32) {
        match self {
            IconKind::Focus => (100, 88),
            IconKind::Idea => (64, 64),
            IconKind::Event => (210, 176),
            IconKind::Portrait => (156, 210),
        }
    }
    fn prefix(self) -> &'static str {
        match self {
            IconKind::Focus => "GFX_goal_",
            IconKind::Idea => "GFX_idea_",
            IconKind::Event => "GFX_",
            IconKind::Portrait => "GFX_portrait_",
        }
    }
    fn dir(self, tag: &str) -> String {
        match self {
            IconKind::Focus => "gfx/interface/goals".into(),
            IconKind::Idea => "gfx/interface/ideas".into(),
            IconKind::Event => "gfx/event_pictures".into(),
            IconKind::Portrait => format!("gfx/leaders/{tag}"),
        }
    }
}

#[derive(Default)]
struct IconState {
    kind: IconKind,
    src: Option<(PathBuf, image::RgbaImage)>,
    name: String,
    frame: bool,
    cover: bool,
    zoom: f32,
    dx: f32,
    dy: f32,
    out: Option<(image::RgbaImage, egui::TextureHandle)>,
    dirty: bool,
}

/// The picture fitted into the icon, framed like the game's focus icons
/// (a bronze bevel around a darkened, slightly greyed inside) when asked.
fn make_icon(s: &IconState) -> Option<image::RgbaImage> {
    let (_, src) = s.src.as_ref()?;
    let (w, h) = s.kind.size();
    let mut out = image::RgbaImage::new(w, h);
    let frame = s.frame && matches!(s.kind, IconKind::Focus | IconKind::Idea);
    let inset = if frame { if s.kind == IconKind::Focus { 9.0 } else { 5.0 } } else { 0.0 };
    let (iw, ih) = (w as f32 - 2.0 * inset, h as f32 - 2.0 * inset);
    let (sw, sh) = (src.width() as f32, src.height() as f32);
    let k = if s.cover { (iw / sw).max(ih / sh) } else { (iw / sw).min(ih / sh) } * s.zoom.max(0.1);
    let (dw, dh) = (sw * k, sh * k);
    let ox = inset + (iw - dw) / 2.0 + s.dx * iw / 2.0;
    let oy = inset + (ih - dh) / 2.0 + s.dy * ih / 2.0;
    let scaled = image::imageops::resize(src, dw.round().max(1.0) as u32, dh.round().max(1.0) as u32, image::imageops::FilterType::Lanczos3);
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let inside = fx >= inset && fy >= inset && fx < w as f32 - inset && fy < h as f32 - inset;
            let mut px = [0u8; 4];
            if inside {
                let (sx, sy) = ((fx - ox) as i64, (fy - oy) as i64);
                if sx >= 0 && sy >= 0 && (sx as u32) < scaled.width() && (sy as u32) < scaled.height() {
                    px = scaled.get_pixel(sx as u32, sy as u32).0;
                }
                if frame {
                    // darker and a little grey, as the game's icons
                    let l = 0.3 * px[0] as f32 + 0.59 * px[1] as f32 + 0.11 * px[2] as f32;
                    for c in px.iter_mut().take(3) {
                        *c = ((*c as f32 * 0.75 + l * 0.25) * 0.82) as u8;
                    }
                    if px[3] == 0 {
                        px = [40, 38, 32, 255];
                    }
                }
            }
            if frame && !inside {
                // the bevel: light on the top left, dark on the bottom right
                let (dl, dt, dr, db) = (fx, fy, w as f32 - fx, h as f32 - fy);
                let m = dl.min(dt).min(dr).min(db);
                let d = (m / inset).clamp(0.0, 1.0);
                let light = if m == dl || m == dt { 1.15 } else { 0.7 };
                let base = [176.0, 132.0, 72.0];
                let shade = (0.6 + 0.4 * (1.0 - (d - 0.5).abs() * 2.0)) * light;
                px = [(base[0] * shade).min(255.0) as u8, (base[1] * shade).min(255.0) as u8, (base[2] * shade).min(255.0) as u8, 255];
                if d < 0.15 {
                    px = [30, 24, 16, 255];
                }
            }
            out.put_pixel(x, y, image::Rgba(px));
        }
    }
    Some(out)
}

fn icon_page(ui: &mut egui::Ui, cx: &mut Cx, s: &mut IconState) {
    if s.zoom == 0.0 {
        s.zoom = 1.0;
        s.frame = true;
        s.cover = true;
    }
    ui.label("Any picture as a focus icon, an idea picture, an event picture or a portrait, at the game's size, with its sprite.");
    ui.horizontal_wrapped(|ui| {
        for (k, l) in [(IconKind::Focus, "focus icon"), (IconKind::Idea, "idea"), (IconKind::Event, "event picture"), (IconKind::Portrait, "portrait")] {
            s.dirty |= ui.selectable_value(&mut s.kind, k, l).changed();
        }
        if ui.button("Picture…").clicked()
            && let Some(p) = rfd::FileDialog::new().add_filter("pictures", &["png", "jpg", "jpeg", "bmp", "tga", "dds", "webp"]).pick_file()
        {
            let r = std::fs::read(&p).map_err(|e| e.to_string()).and_then(|b| crate::content::gfx::decode(&b, &p.to_string_lossy()));
            match r {
                Ok((w, h, px)) => {
                    if let Some(img) = image::RgbaImage::from_raw(w, h, px) {
                        if s.name.is_empty() {
                            s.name = p.file_stem().map(|n| n.to_string_lossy().to_ascii_lowercase().replace([' ', '-'], "_")).unwrap_or_default();
                        }
                        s.src = Some((p, img));
                        s.dirty = true;
                    }
                }
                Err(e) => cx.error(format!("cannot read {}: {e}", p.display())),
            }
        }
    });
    if s.src.is_none() {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        if matches!(s.kind, IconKind::Focus | IconKind::Idea) {
            s.dirty |= ui.checkbox(&mut s.frame, "frame").changed();
        }
        s.dirty |= ui.checkbox(&mut s.cover, "fill (cut the edges)").changed();
        s.dirty |= ui.add(egui::Slider::new(&mut s.zoom, 0.3..=3.0).text("zoom")).changed();
        s.dirty |= ui.add(egui::Slider::new(&mut s.dx, -1.0..=1.0).text("left/right")).changed();
        s.dirty |= ui.add(egui::Slider::new(&mut s.dy, -1.0..=1.0).text("up/down")).changed();
    });
    if s.dirty || s.out.is_none() {
        s.dirty = false;
        if let Some(img) = make_icon(s) {
            let ci = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
            let tex = ui.ctx().load_texture("icon_maker", ci, egui::TextureOptions::LINEAR);
            s.out = Some((img, tex));
        }
    }
    if let Some((img, tex)) = &s.out {
        ui.horizontal(|ui| {
            let size = vec2(img.width() as f32, img.height() as f32);
            ui.image((tex.id(), size));
            ui.image((tex.id(), size * 2.0));
            let _ = Color32::WHITE;
        });
    }
    let tag = cx.tag().to_string();
    let sprite = format!("{}{}", s.kind.prefix(), s.name.trim());
    let dir = s.kind.dir(&tag);
    let file = format!("{dir}/{}.dds", s.name.trim());
    let taken = crate::content::gfx::store_sprites(cx.st).contains_key(&sprite) || cx.base.is_some_and(|b| b.sprites.contains_key(&sprite));
    ui.horizontal(|ui| {
        ui.label("name");
        ui.add(egui::TextEdit::singleline(&mut s.name).desired_width(200.0));
        ui.label(RichText::new(format!("{sprite} → {file}")).monospace().small());
    });
    if taken {
        ui.label(RichText::new(format!("{sprite} is there already: making it replaces the picture of the project's (or hides the base game's)")).color(WARN));
    }
    let ok = !s.name.trim().is_empty() && s.name.trim().chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if ui.add_enabled(ok && s.out.is_some(), egui::Button::new(RichText::new(format!("Make {sprite}")).strong())).clicked()
        && let Some((img, _)) = &s.out
    {
        let bytes = crate::modbuild::img::dds_rgba_bytes(img.width(), img.height(), img.as_raw());
        let pkg = cx.pkg.to_string();
        let def = format!("spriteType = {{\n\tname = \"{sprite}\"\n\ttexturefile = \"{file}\"\n}}");
        cx.st.begin(format!("picture {sprite}"));
        cx.st.set_bytes(&pkg, &file, bytes);
        match import::insert_def(cx.st, &pkg, Kind::Sprite, None, &sprite, &def) {
            Ok(()) => {
                cx.st.commit();
                cx.gfx.clear();
                cx.info(format!("made {sprite}: pick it in an icon field"));
                let _ = HashMap::<(), ()>::new();
            }
            Err(e) => {
                cx.st.rollback();
                cx.error(format!("{e:#}"));
            }
        }
    }
}
