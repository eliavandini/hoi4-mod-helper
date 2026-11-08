//! Checks of the content (what the game would trip over: broken braces,
//! ids named but not defined, defined twice, missing texts and pictures,
//! states and tags that are not on the map, focus trees that go round...)
//! and the test play: build the mod, play it, and read what the game said
//! about it in its error log, pointed back to the content.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::{Instant, SystemTime};

use eframe::egui::{self, RichText};

use super::{Cx, ERR, Nav, OK, WARN};
use crate::content::errlog;
use crate::content::validate::{self, Facts, Issue, Severity};
use crate::editor::Editor;

#[derive(Default)]
pub struct CheckState {
    /// The last run: at a generation of the store, with the base game read or not.
    run: Option<((u64, bool, usize), Vec<Issue>)>,
    force: bool,
    show_info: bool,
    show_warn: bool,
    show_err: bool,
    category: Option<String>,
    all_pkgs: bool,
    filter: String,
    took_ms: u128,
    // test play
    log_from: Option<(PathBuf, u64)>,
    log: Option<(Vec<errlog::Entry>, SystemTime, Instant)>,
    log_err: Option<String>,
    mine_only: bool,
    log_filter: String,
}

impl CheckState {
    /// The content or the sources changed: check again when shown.
    pub fn stale(&mut self) {
        self.force = true;
    }
    /// Errors and warnings of the last run.
    pub fn counts(&self) -> (usize, usize) {
        match &self.run {
            Some((_, is)) => {
                let (e, w, _) = validate::summary(is);
                (e, w)
            }
            None => (0, 0),
        }
    }

    fn ensure(&mut self, cx: &Cx) {
        let key = (cx.st.generation, cx.base.is_some(), cx.facts.nations.len() ^ (cx.facts.states.len() << 16));
        if !self.force && self.run.as_ref().is_some_and(|r| r.0 == key) {
            return;
        }
        let t = Instant::now();
        let issues = validate::validate(cx.st, cx.base, &facts_of(cx));
        self.took_ms = t.elapsed().as_millis();
        self.run = Some((key, issues));
        self.force = false;
    }
}

/// What the map says, for the checks.
pub fn facts_of(cx: &Cx) -> Facts {
    let mut f = Facts { provinces: cx.facts.provinces, states: cx.facts.states.iter().copied().collect(), ..Default::default() };
    for n in &cx.facts.nations {
        if n.tag.is_empty() {
            continue;
        }
        f.nations.insert(n.tag.clone(), (n.name.clone(), n.capital_state, n.states.iter().copied().collect()));
        if !n.pinned {
            f.unpinned += 1;
        }
    }
    f
}

pub fn check_page(ui: &mut egui::Ui, cx: &mut Cx, cs: &mut CheckState) {
    if !cs.show_err && !cs.show_warn && !cs.show_info {
        cs.show_err = true;
        cs.show_warn = true;
    }
    cs.ensure(cx);
    let Some((_, issues)) = &cs.run else { return };
    let pkg = cx.pkg.to_string();
    let (ne, nw, ni) = validate::summary(issues);
    ui.horizontal_wrapped(|ui| {
        ui.heading("Check");
        if ne + nw == 0 {
            ui.label(RichText::new("✔ nothing the game would trip over").color(OK));
        }
        ui.label(RichText::new(format!("checked in {} ms", cs.took_ms)).weak().small());
        if ui.button("Check again").clicked() {
            cs.force = true;
        }
        if cx.base.is_none() {
            ui.label(RichText::new("the base game is still being read: what it defines counts as missing until then").color(WARN).small());
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.toggle_value(&mut cs.show_err, RichText::new(format!("{ne} errors")).color(ERR));
        ui.toggle_value(&mut cs.show_warn, RichText::new(format!("{nw} warnings")).color(WARN));
        ui.toggle_value(&mut cs.show_info, format!("{ni} notes"));
        ui.separator();
        ui.selectable_value(&mut cs.all_pkgs, false, format!("{} only", if pkg == crate::content::store::SHARED { "shared" } else { &pkg }));
        ui.selectable_value(&mut cs.all_pkgs, true, "all nations");
        ui.separator();
        let cats: BTreeSet<&str> = issues.iter().map(|i| i.category).collect();
        egui::ComboBox::from_id_salt("check_cat").selected_text(cs.category.clone().unwrap_or_else(|| "every kind".into())).show_ui(ui, |ui| {
            if ui.selectable_label(cs.category.is_none(), "every kind").clicked() {
                cs.category = None;
            }
            for c in cats {
                let n = issues.iter().filter(|i| i.category == c).count();
                if ui.selectable_label(cs.category.as_deref() == Some(c), format!("{c} ({n})")).clicked() {
                    cs.category = Some(c.to_string());
                }
            }
        });
        ui.add(egui::TextEdit::singleline(&mut cs.filter).hint_text("filter").desired_width(160.0));
    });
    let f = cs.filter.to_lowercase();
    let shown: Vec<&Issue> = issues
        .iter()
        .filter(|i| match i.severity {
            Severity::Error => cs.show_err,
            Severity::Warning => cs.show_warn,
            Severity::Info => cs.show_info,
        })
        .filter(|i| cs.all_pkgs || i.pkg == pkg || i.pkg.is_empty())
        .filter(|i| cs.category.as_deref().is_none_or(|c| c == i.category))
        .filter(|i| f.is_empty() || i.message.to_lowercase().contains(&f) || i.file.to_lowercase().contains(&f))
        .collect();
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{} shown", shown.len())).weak());
        if ui.small_button("copy as text").on_hover_text("for a bug report or a note").clicked() {
            let t: String = shown.iter().map(|i| format!("{} [{}] {}/{}:{}: {}\n", i.severity.label(), i.category, i.pkg, i.file, i.line, i.message)).collect();
            ui.ctx().copy_text(t);
        }
    });
    ui.separator();
    let mut go = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, 22.0, shown.len(), |ui, range| {
        for i in &shown[range] {
            ui.horizontal(|ui| {
                let (c, s) = match i.severity {
                    Severity::Error => (ERR, "✖"),
                    Severity::Warning => (WARN, "⚠"),
                    Severity::Info => (ui.visuals().weak_text_color(), "ℹ"),
                };
                ui.label(RichText::new(s).color(c));
                ui.label(RichText::new(i.category).small().weak());
                if !i.file.is_empty() {
                    let at = if cs.all_pkgs { format!("{}/{}:{}", i.pkg, short(&i.file), i.line) } else { format!("{}:{}", short(&i.file), i.line) };
                    if ui.link(RichText::new(at).monospace().small()).on_hover_text(&i.file).clicked() {
                        go = Some(Nav::File { pkg: i.pkg.clone(), file: i.file.clone(), line: Some(i.line) });
                    }
                }
                let r = ui.add(egui::Label::new(RichText::new(&i.message).color(c)).sense(egui::Sense::click()).truncate());
                if let Some(d) = &i.def
                    && r.on_hover_text(format!("{}\n(click: edit {})", i.message, d.id)).clicked()
                {
                    go = Some(Nav::Def(d.clone()));
                }
            });
        }
    });
    if let Some(n) = go {
        cx.go(n);
    }
}

fn short(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

// ---------------------------------------------------------------- test play

fn read_log(cx: &Cx, cs: &mut CheckState) {
    let path = errlog::log_path(cx.user_dir);
    let from = cs.log_from.as_ref().filter(|(p, _)| *p == path).map(|x| x.1).unwrap_or(0);
    match errlog::read(&path, from) {
        Ok(text) => {
            let files: BTreeMap<String, String> = cx.st.packages.iter().flat_map(|(n, p)| p.files.keys().map(move |f| (f.clone(), n.clone()))).collect();
            let is_mod = |p: &str| files.contains_key(p);
            let pkg = |p: &str| files.get(p).cloned();
            let entries = errlog::parse(&text, &is_mod, &pkg, 3000);
            let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
            cs.log = Some((entries, mtime, Instant::now()));
            cs.log_err = None;
        }
        Err(e) => {
            cs.log = None;
            cs.log_err = Some(format!("cannot read {}: {e}", path.display()));
        }
    }
}

pub fn play_page(ui: &mut egui::Ui, cx: &mut Cx, cs: &mut CheckState, editor: &mut Editor) {
    ui.heading("Test play");
    ui.label("Build the mod with the content, play it, and see here what the game said about it.");
    cs.ensure(cx);
    let (ne, _) = cs.counts();
    let log_path = errlog::log_path(cx.user_dir);
    ui.add_space(6.0);
    egui::Grid::new("play_steps").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
        ui.label(RichText::new("1").strong());
        ui.horizontal(|ui| {
            if ne > 0 {
                ui.label(RichText::new(format!("Check finds {ne} errors: the game will likely complain about them.")).color(ERR));
                if ui.small_button("show").clicked() {
                    cx.go(Nav::Page(super::Page::Check));
                }
            } else {
                ui.label(RichText::new("✔ Check finds no errors").color(OK));
            }
        });
        ui.end_row();
        ui.label(RichText::new("2").strong());
        ui.horizontal(|ui| {
            let busy = editor.building();
            if ui.add_enabled(!busy, egui::Button::new("💾 Save and build the mod")).on_hover_text("Saves the content and the map, then builds the mod (the build window shows how it goes)").clicked() {
                if cx.st.dirty()
                    && let Err(e) = cx.st.save()
                {
                    cx.error(format!("saving the content failed: {e:#}"));
                } else {
                    // what the log has now is not about this build
                    let size = std::fs::metadata(&log_path).map(|m| m.len()).unwrap_or(0);
                    cs.log_from = Some((log_path.clone(), size));
                    cs.log = None;
                    editor.build_and_show(ui.ctx());
                }
            }
            if busy {
                ui.spinner();
                ui.label("building…");
            }
        });
        ui.end_row();
        ui.label(RichText::new("3").strong());
        ui.horizontal(|ui| {
            ui.label("Start the game with the mod: the build window has the launcher.");
            if ui.button("Build window").clicked() {
                editor.show_build_window();
            }
        });
        ui.end_row();
        ui.label(RichText::new("4").strong());
        ui.horizontal(|ui| {
            if ui.button("📄 Read the error log").on_hover_text(log_path.display().to_string()).clicked() {
                read_log(cx, cs);
            }
            if cs.log_from.is_some() {
                ui.label(RichText::new("(what the game wrote since the build)").weak());
                if ui.small_button("the whole log").clicked() {
                    cs.log_from = None;
                    read_log(cx, cs);
                }
            }
        });
        ui.end_row();
    });
    // the log changed on disk: read it again
    if let Some((_, mtime, at)) = &cs.log
        && at.elapsed().as_secs() >= 2
    {
        let now = std::fs::metadata(&log_path).and_then(|m| m.modified()).ok();
        if now.is_some_and(|n| n != *mtime) {
            read_log(cx, cs);
        } else if let Some((e, m, _)) = cs.log.take() {
            cs.log = Some((e, m, Instant::now()));
        }
        ui.ctx().request_repaint_after(std::time::Duration::from_secs(2));
    }
    if let Some(e) = &cs.log_err {
        ui.label(RichText::new(e).color(ERR));
    }
    let Some((entries, mtime, _)) = &cs.log else { return };
    ui.separator();
    let mine = entries.iter().filter(|e| e.ours()).count();
    let when = mtime.elapsed().map(|d| if d.as_secs() < 120 { format!("{} s ago", d.as_secs()) } else { format!("{} min ago", d.as_secs() / 60) }).unwrap_or_default();
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(format!("{} messages, {mine} about the content", entries.len())).strong());
        ui.label(RichText::new(format!("the log was written {when}")).weak());
        ui.separator();
        ui.selectable_value(&mut cs.mine_only, true, "about the content");
        ui.selectable_value(&mut cs.mine_only, false, "all");
        ui.add(egui::TextEdit::singleline(&mut cs.log_filter).hint_text("filter").desired_width(160.0));
    });
    let f = cs.log_filter.to_lowercase();
    let shown: Vec<&errlog::Entry> = entries.iter().filter(|e| !cs.mine_only || e.ours()).filter(|e| f.is_empty() || e.message.to_lowercase().contains(&f)).collect();
    if shown.is_empty() {
        ui.label(RichText::new(if cs.mine_only { "✔ the game said nothing about the content" } else { "the log is empty" }).color(OK));
        return;
    }
    let mut go = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for e in shown.iter().take(1500) {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(&e.time).monospace().small().weak());
                if e.count > 1 {
                    ui.label(RichText::new(format!("×{}", e.count)).small().color(WARN));
                }
                if let Some((file, line)) = &e.file {
                    let label = format!("{}{}", short(file), line.map(|l| format!(":{l}")).unwrap_or_default());
                    let r = ui.link(RichText::new(label).monospace().small());
                    if r.clicked()
                        && let Some(p) = &e.pkg
                    {
                        go = Some(Nav::File { pkg: p.clone(), file: file.clone(), line: *line });
                    }
                }
                let c = if e.ours() { ERR } else { ui.visuals().text_color() };
                ui.label(RichText::new(&e.message).color(c)).on_hover_text(&e.source);
            });
            if let Some(h) = errlog::hint_for(&e.message) {
                ui.indent(("hint", &e.message), |ui| {
                    ui.label(RichText::new(format!("💡 {h}")).small().weak());
                });
            }
        }
    });
    if let Some(n) = go {
        cx.go(n);
    }
}
