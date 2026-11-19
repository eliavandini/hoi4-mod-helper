//! The Army page of a nation: the order of battle it starts with. Its
//! division templates are made in the division designer (the template's
//! own page); its divisions are placed here, by clicking provinces on a map
//! of the nation, and listed by province to rename, give another template,
//! experience, equipment or men, move or remove. Above the list, the
//! equipment and men the army needs, with what the divisions get at the
//! start and what the stockpile has, and buttons that fill the stockpile.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use eframe::egui::{self, RichText, Vec2};

use super::navy::{commit_text, edit_with, history_file, item_at, line_of};
use super::provmap::{self, Marker};
use super::{Cx, ERR, Nav, OK, WARN};
use crate::content::army::{self, ArmyNeeds, ArmyProblem, Division, Ground};
use crate::content::dlc::{Need, Owned};
use crate::content::kinds::{self, Kind};
use crate::content::loc;
use crate::content::script::{self, Block, Edit, Script, Span};
use crate::content::store::SHARED;
use crate::content::units::{self, Have, Section, Template, UnitDb};
use crate::content::validate::Severity;
use crate::editor::{GONE, MapFacts, WATER};

#[derive(Default)]
pub struct ArmyState {
    /// The template a click on the map places divisions of (None: a click
    /// picks the province).
    place: Option<String>,
    /// How many a click places.
    count: u32,
    /// The province whose divisions are listed (None: all).
    selected: Option<u32>,
    /// A division being moved: a click on the map puts it there.
    moving: Option<(Span, String)>,
    new_template: String,
    /// What the nation has at the start, by (content generation, tag, DLCs
    /// of the player).
    have: Option<((u64, String, Owned), Arc<Have>)>,
}

impl ArmyState {
    /// For screenshots and tests: `place=<template>`, `select=<province>`.
    pub fn open_at(&mut self, what: Option<&str>) {
        for kv in what.unwrap_or("").split(';') {
            match kv.split_once('=') {
                Some(("place", t)) => self.place = Some(t.to_string()),
                Some(("select", p)) => self.selected = p.parse().ok(),
                _ => {}
            }
        }
    }
}

enum Act {
    /// The order of battle made: history/units/<name>.txt (when missing)
    /// and `oob = "<name>"` in the history (when it has none).
    MakeOob(String),
    NewTemplate(String),
    /// A template of another file of the nation moved into the order of
    /// battle.
    MoveTemplate { from: String, name: String },
    Add { province: u32, template: String, n: u32 },
    Rename(Span, String),
    Template(Span, String),
    Factor(Span, &'static str, f64),
    Move(Span, u32),
    Remove(Vec<Span>),
    /// `start_manpower_factor = 1` for every division without it.
    AllMen,
    /// Equipment into the stockpile of the nation's history: (model, how
    /// many, the DLCs a player needs for it); the label of the undo step.
    Stock(Vec<(String, f64, Need)>, String),
}

/// What each province of the map is (by province id).
pub fn ground_of(f: &MapFacts) -> Vec<Option<Ground>> {
    let ps = &f.pixels.province_state;
    ps.iter()
        .enumerate()
        .map(|(p, &s)| match (p, s) {
            (0, _) | (_, GONE) => None,
            (_, WATER) => Some(Ground::Water),
            (_, 0) => Some(Ground::Land(String::new())),
            (_, s) => Some(Ground::Land(f.pixels.state_nation.get(&s).and_then(|n| f.nations.iter().find(|x| x.id == *n)).map(|n| n.tag.clone()).unwrap_or_default())),
        })
        .collect()
}

fn name(cx: &Cx, id: &str) -> String {
    cx.text(id).map(loc::plain).filter(|t| !t.trim().is_empty()).unwrap_or_else(|| loc::prettify(id))
}

/// "6 × Infantry, 2 × Artillery · support: Engineer, Recon".
fn summary(cx: &Cx, b: &Block) -> String {
    let t = Template::read(b);
    let count = |s: Section| {
        let mut n: BTreeMap<&str, usize> = BTreeMap::new();
        for x in t.of(s) {
            *n.entry(x.unit.as_str()).or_default() += 1;
        }
        n
    };
    let regiments: Vec<String> = count(Section::Regiments).into_iter().map(|(u, n)| format!("{n} × {}", name(cx, u))).collect();
    let support: Vec<String> = count(Section::Support).into_keys().map(|u| name(cx, u)).collect();
    let mut s = if regiments.is_empty() { "no battalions".to_string() } else { regiments.join(", ") };
    if !support.is_empty() {
        s.push_str(&format!(" · support: {}", support.join(", ")));
    }
    s
}

/// The template the Army page places when it shows next (from the division
/// designer).
pub fn place_next(ctx: &egui::Context, template: &str) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::unique("army_place_next"), template.to_string()));
}

pub fn page(ui: &mut egui::Ui, cx: &mut Cx, a: &mut ArmyState) {
    if let Some(t) = ui.ctx().data_mut(|d| d.remove_temp::<String>(egui::Id::unique("army_place_next"))) {
        a.place = Some(t);
        a.moving = None;
    }
    ui.heading(format!("🪖 Army of {}", cx.nation.map(|n| format!("{} {}", cx.pkg, n.name)).unwrap_or_else(|| cx.pkg.to_string())));
    if cx.pkg == SHARED {
        ui.label("The army is a nation's: pick a nation on the left.");
        return;
    }
    if a.count == 0 {
        a.count = 1;
    }
    let tag = cx.pkg.to_string();
    ui.label(
        RichText::new(format!(
            "The divisions {tag} starts with: each of a division template (made in the division designer) and placed in a province of the map. They are in the order of battle its history loads with oob = \"...\" (a history/units file); the build places no divisions of its own."
        ))
        .weak(),
    );
    let mut acts: Vec<Act> = Vec::new();
    let oob = army::army_oob(cx.st, &tag);
    let files = army::template_files(cx.st, &tag);
    let file = oob.as_ref().and_then(|(_, f)| f.clone());
    // the order of battle
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Order of battle").strong());
        match &oob {
            Some((line, Some((pkg, path)))) => {
                if ui.link(path).on_hover_text("open the file").clicked() {
                    cx.go(Nav::File { pkg: pkg.clone(), file: path.clone(), line: None });
                }
                ui.label(RichText::new(format!("(oob = \"{}\" in {}:{})", line.name, line.file, line_of(cx, &tag, &line.file, line.span.start))).weak().small());
            }
            Some((line, None)) => {
                ui.colored_label(ERR, format!("oob = \"{}\": there is no history/units/{}.txt in the content", line.name, line.name));
                if ui.button(format!("Make history/units/{}.txt", line.name)).clicked() {
                    acts.push(Act::MakeOob(line.name.clone()));
                }
            }
            None => {
                ui.colored_label(WARN, format!("none: {tag} starts with no divisions"));
                // a file of templates nothing loads yet, else a new one
                for path in files.keys() {
                    let n = army::oob_name(path).to_string();
                    if ui.button(format!("Load {path}")).on_hover_text(format!("oob = \"{n}\" in the history: its templates and divisions are {tag}'s at the start")).clicked() {
                        acts.push(Act::MakeOob(n));
                    }
                }
                let n = format!("{tag}_army");
                if !files.contains_key(&format!("history/units/{n}.txt")) && ui.button(format!("Make history/units/{n}.txt")).on_hover_text(format!("a new order of battle, and oob = \"{n}\" in the history")).clicked() {
                    acts.push(Act::MakeOob(n));
                }
            }
        }
    });
    let Some((pkg, path)) = file else {
        apply(cx, &tag, None, acts, a);
        return;
    };
    let text = cx.st.text(&pkg, &path).unwrap_or("").to_string();
    let s = Script::parse(&text);
    let templates = army::templates_of(&s.root);
    let divs = army::read_divisions(&s.root);
    if a.place.as_ref().is_some_and(|t| !templates.contains(t)) {
        a.place = None;
    }
    if a.place.is_none() && divs.is_empty() {
        a.place = templates.first().cloned();
    }
    // the templates
    ui.add_space(4.0);
    ui.label(RichText::new("Division templates").strong());
    let mut per_template: BTreeMap<&str, usize> = BTreeMap::new();
    for d in &divs {
        *per_template.entry(d.template.as_deref().unwrap_or("")).or_default() += 1;
    }
    egui::Grid::new("army_templates").num_columns(4).spacing([12.0, 4.0]).show(ui, |ui| {
        for b in s.root.blocks("division_template") {
            let Some(t) = b.atom("name") else { continue };
            let on = a.place.as_deref() == Some(t);
            if ui.selectable_label(on, RichText::new(t).strong()).on_hover_text("place divisions of it: click provinces on the map").clicked() {
                a.place = if on { None } else { Some(t.to_string()) };
                a.moving = None;
            }
            ui.label(RichText::new(summary(cx, b)).weak());
            let n = per_template.get(t).copied().unwrap_or(0);
            ui.label(if n == 1 { "1 division".to_string() } else { format!("{n} divisions") });
            if ui.small_button("✏ designer").on_hover_text("open it in the division designer").clicked()
                && let Some(k) = cx.st.find_in(&pkg, &[Kind::DivisionTemplate], t)
            {
                cx.go(Nav::Def(k));
            }
            ui.end_row();
        }
    });
    if templates.is_empty() {
        ui.label(RichText::new("No templates yet: make one below, then design it.").weak());
    }
    // templates the order of battle does not have
    for (other, names) in files.iter().filter(|(p, _)| **p != path) {
        for t in names {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(WARN, format!("\"{t}\" is in {other}, which the history does not load: divisions cannot be of it"));
                if !templates.contains(t) && ui.small_button("move it here").on_hover_text(format!("into {path}")).clicked() {
                    acts.push(Act::MoveTemplate { from: other.clone(), name: t.clone() });
                }
            });
        }
    }
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut a.new_template).desired_width(200.0).hint_text("name of a new template"));
        let n = a.new_template.trim().to_string();
        let taken = templates.contains(&n);
        if ui.add_enabled(!n.is_empty() && !taken, egui::Button::new("➕ New template")).on_hover_text("a template of three infantry battalions and engineers, opened in the division designer").clicked() {
            acts.push(Act::NewTemplate(n.clone()));
            a.new_template.clear();
        }
        if taken {
            ui.colored_label(WARN, "there is a template of that name");
        }
    });
    ui.separator();
    // the map and the divisions
    let mut at: BTreeMap<u32, Vec<&Division>> = BTreeMap::new();
    for d in &divs {
        at.entry(d.location.unwrap_or(0)).or_default().push(d);
    }
    let ground = ground_of(cx.facts);
    let problems = army::check_divisions(&divs, &templates, &ground, &tag);
    let wide = ui.available_width();
    let map_w = (wide * 0.5).clamp(320.0, 760.0);
    let map_h = (ui.available_height() - 40.0).clamp(260.0, 620.0);
    let own: Vec<u32> = cx.nation.map(|n| n.states.clone()).unwrap_or_default();
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.set_width(map_w);
            ui.horizontal_wrapped(|ui| {
                if let Some((_, n)) = &a.moving {
                    ui.colored_label(WARN, format!("Click the province to move \"{n}\" to"));
                    if ui.small_button("stop").clicked() {
                        a.moving = None;
                    }
                } else {
                    ui.label("Click a province:");
                    let label = match &a.place {
                        Some(t) => format!("place {t}"),
                        None => "pick it".to_string(),
                    };
                    egui::ComboBox::from_id_salt("army_place").selected_text(label).show_ui(ui, |ui| {
                        ui.selectable_value(&mut a.place, None, "pick it (list its divisions)");
                        for t in &templates {
                            ui.selectable_value(&mut a.place, Some(t.clone()), format!("place {t}"));
                        }
                    });
                    if a.place.is_some() {
                        ui.add(egui::DragValue::new(&mut a.count).range(1..=24).prefix("× "));
                    }
                }
                if ui.small_button("whole map").clicked() {
                    provmap::reset_view(ui, egui::Id::unique("army_map"), Some(Vec2::new(map_w, map_h)));
                }
                if !own.is_empty() && ui.small_button(format!("{tag}'s states")).clicked() {
                    provmap::reset_view(ui, egui::Id::unique("army_map"), None);
                }
            });
            let markers: Vec<Marker> = at.iter().filter(|(p, _)| **p != 0).map(|(p, ds)| Marker { province: *p, text: ds.len().to_string(), lit: a.selected == Some(*p) }).collect();
            let hover = |p: u32| -> Vec<String> { at.get(&p).map(|ds| ds.iter().map(|d| format!("{} ({})", d.label(), d.template.as_deref().unwrap_or("no template"))).collect()).unwrap_or_default() };
            let hint = match (&a.moving, &a.place) {
                (Some((_, n)), _) => format!("click: move \"{n}\" here"),
                (None, Some(t)) => format!("click: place {} × {t} here · right-click: list its divisions", a.count),
                (None, None) => "click: list its divisions".to_string(),
            };
            let lit: Vec<u32> = a.selected.into_iter().collect();
            let out = provmap::map(ui, cx, egui::Id::unique("army_map"), &own, &lit, &markers, &hover, &hint, Vec2::new(map_w, map_h));
            if let Some(p) = out.clicked {
                match (a.moving.take(), &a.place) {
                    (Some((span, _)), _) => acts.push(Act::Move(span, p)),
                    (None, Some(t)) => {
                        acts.push(Act::Add { province: p, template: t.clone(), n: a.count });
                        a.selected = Some(p);
                    }
                    (None, None) => a.selected = if a.selected == Some(p) { None } else { Some(p) },
                }
            }
            if let Some(p) = out.secondary {
                a.selected = if a.selected == Some(p) { None } else { Some(p) };
            }
            ui.label(RichText::new("Wheel: zoom, drag: move the map. Each number is how many divisions stand in the province.").weak().small());
        });
        ui.separator();
        ui.vertical(|ui| {
            needs_panel(ui, cx, a, &s.root, &divs, &mut acts);
            division_list(ui, cx, a, &templates, &divs, &at, &problems, &mut acts);
        });
    });
    apply(cx, &tag, Some((pkg, path)), acts, a);
}

/// What the nation has at the start (cached), for the player's DLCs.
fn have_at_start(cx: &Cx, db: &UnitDb, a: &mut ArmyState) -> Arc<Have> {
    let tag = cx.pkg.to_string();
    let owned = super::division::owned(cx, db, &tag);
    let key = (cx.st.generation, tag, owned);
    if a.have.as_ref().is_none_or(|h| h.0 != key) {
        let h = db.at_start(cx.st, &key.1, Some(&key.2));
        a.have = Some((key, Arc::new(h)));
    }
    a.have.as_ref().map(|h| h.1.clone()).unwrap_or_default()
}

fn count(v: f64) -> String {
    let n = v.round() as i64;
    let s = n.abs().to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 { format!("-{out}") } else { out }
}

/// The equipment and men the divisions need: at full strength, what they
/// get at the start, what they take from the stockpile and what it has;
/// buttons that put what is lacking (or a whole army's worth) in it.
fn needs_panel(ui: &mut egui::Ui, cx: &mut Cx, a: &mut ArmyState, root: &Block, divs: &[Division], acts: &mut Vec<Act>) {
    if divs.is_empty() {
        return;
    }
    let Some(base) = cx.base else {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Reading the game's battalions for the equipment the army needs…");
        });
        return;
    };
    let db = units::db(Some(base), cx.st);
    let have = have_at_start(cx, &db, a);
    let per: BTreeMap<String, units::Division> = root.blocks("division_template").filter_map(|b| Some((b.atom("name")?.to_string(), db.division(&Template::read(b), &have)))).collect();
    let n: ArmyNeeds = army::army_needs(divs, &per);
    let stock = |arch: &str| army::in_stock(&db, &have.stock, arch);
    let tag = cx.pkg.to_string();
    egui::CollapsingHeader::new(RichText::new("Equipment and men the army needs").strong()).id_salt("army_needs").default_open(true).show(ui, |ui| {
        ui.label(
            RichText::new("Each division gets its share of equipment (equip.) at the start, not from the stockpile; it takes the rest from the stockpile, when it has it. Its men (men) are given the same way, else they come out of the reserves.")
                .weak()
                .small(),
        );
        egui::Grid::new("army_needs_grid").num_columns(5).spacing([12.0, 2.0]).striped(true).show(ui, |ui| {
            for (h, tip) in [
                ("", ""),
                ("full strength", "what all the divisions use at full strength"),
                ("given", "what they get at the start (start_equipment_factor, start_manpower_factor)"),
                ("lacking", "what they lack at the start: they take it from the stockpile (men: from the reserves)"),
                ("in the stockpile", "what the history puts in the stockpile (add_equipment_to_stockpile), of every model of it"),
            ] {
                let r = ui.label(RichText::new(h).small().weak());
                if !tip.is_empty() {
                    r.on_hover_text(tip);
                }
            }
            ui.end_row();
            for (arch, e) in &n.equipment {
                match &e.model {
                    Some(m) => ui.label(name(cx, m)).on_hover_text(format!("{m}, of {} ({arch}): the newest model of it {tag} has at the start (with the DLCs picked in the division designer)", name(cx, arch))),
                    None => ui.colored_label(ERR, format!("{}: none", name(cx, arch))).on_hover_text(format!("{tag} has no model of {arch} at the start (no technology, none in the stockpile): its divisions lack it")),
                };
                ui.label(count(e.full));
                ui.label(count(e.given));
                ui.label(count(e.missing()));
                let st = stock(arch);
                let colour = if st + 1e-6 >= e.missing() { OK } else { WARN };
                ui.colored_label(colour, count(st)).on_hover_text(if st + 1e-6 >= e.missing() { "enough for the divisions to fill up".to_string() } else { format!("{} short: the divisions stay below full strength", count(e.missing() - st)) });
                ui.end_row();
            }
            ui.label(RichText::new("Men").strong());
            ui.label(count(n.men));
            ui.label(count(n.men_given)).on_hover_text("given by start_manpower_factor");
            if n.men_from_reserves > 0.0 {
                ui.colored_label(WARN, format!("{} from the reserves", count(n.men_from_reserves))).on_hover_text(format!("divisions without start_manpower_factor{}", army::MEN_FROM_RESERVES));
            } else {
                ui.label(count(n.men - n.men_given));
            }
            ui.end_row();
        });
        if n.unknown > 0 {
            ui.label(RichText::new(format!("{} division(s) of no template of the file are left out.", n.unknown)).weak().small());
        }
        let need_of = |list: Vec<(String, f64, Need)>| -> String {
            list.iter().map(|(m, k, need)| format!("{} × {}{}", count(*k), name(cx, m), if need.is_empty() { String::new() } else { format!(" (only players {})", need.describe()) })).collect::<Vec<_>>().join(", ")
        };
        let with_need = |v: Vec<(String, f64)>| -> Vec<(String, f64, Need)> { v.into_iter().map(|(m, k)| (m.clone(), k, db.need_of(&m))).collect() };
        let lack = with_need(army::stock_additions(&n, &stock, false));
        let whole = with_need(army::stock_additions(&n, &stock, true));
        ui.horizontal_wrapped(|ui| {
            let r = ui.add_enabled(!lack.is_empty(), egui::Button::new("➕ Add what they lack to the stockpile"));
            if lack.is_empty() {
                r.on_disabled_hover_text("the stockpile has all the divisions lack");
            } else if r.on_hover_text(format!("add_equipment_to_stockpile in {tag}'s history: {} (one undo step)", need_of(lack.clone()))).clicked() {
                acts.push(Act::Stock(lack, "what the army lacks".into()));
            }
            if !whole.is_empty()
                && ui
                    .button("➕ Add a whole army's worth")
                    .on_hover_text(format!("as much again as all the divisions use at full strength, for losses and new divisions: {} (one undo step)", need_of(whole.clone())))
                    .clicked()
            {
                acts.push(Act::Stock(whole, "a whole army's worth".into()));
            }
            if n.men_from_reserves > 0.0 && ui.button("Give every division its men").on_hover_text("start_manpower_factor = 1 for the divisions without it: they start with all their men, not out of the reserves").clicked() {
                acts.push(Act::AllMen);
            }
        });
    });
    ui.add_space(4.0);
}

/// The divisions, by province (only the selected province's when one is).
#[allow(clippy::too_many_arguments)]
fn division_list(ui: &mut egui::Ui, cx: &mut Cx, a: &mut ArmyState, templates: &[String], divs: &[Division], at: &BTreeMap<u32, Vec<&Division>>, problems: &[ArmyProblem], acts: &mut Vec<Act>) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(if divs.len() == 1 { "1 division".to_string() } else { format!("{} divisions", divs.len()) }).strong());
        if let Some(p) = a.selected {
            ui.label(RichText::new(format!("· in {}", provmap::province_label(cx.facts, p))).weak());
            if ui.small_button("all").on_hover_text("list the divisions of every province").clicked() {
                a.selected = None;
            }
        }
    });
    let bad: BTreeMap<usize, Severity> = problems.iter().filter_map(|p| Some((p.span.as_ref()?.start, p.severity))).fold(BTreeMap::new(), |mut m, (s, sev)| {
        let e = m.entry(s).or_insert(sev);
        *e = (*e).min(sev);
        m
    });
    if !divs.is_empty() {
        ui.horizontal(|ui| {
            for (w, h, tip) in [
                (COLS[0], "", ""),
                (COLS[1], "name", "its own name (empty: the next of its template's name list)"),
                (COLS[2], "template", "a template of this file"),
                (COLS[3], "experience", EXPERIENCE_TIP),
                (COLS[4], "equip.", EQUIPMENT_TIP),
                (COLS[5], "men", MEN_TIP),
            ] {
                cell(ui, w, |ui| {
                    let r = ui.label(RichText::new(h).small().weak());
                    if !tip.is_empty() {
                        r.on_hover_text(tip);
                    }
                });
            }
        });
    }
    egui::ScrollArea::vertical().id_salt("army_list").max_height((ui.available_height() - 90.0).max(160.0)).auto_shrink([false, true]).show(ui, |ui| {
        let only = a.selected;
        for (p, ds) in at.iter().filter(|(p, _)| only.is_none_or(|s| s == **p)) {
            ui.horizontal(|ui| {
                let place = if *p == 0 { "no province".to_string() } else { provmap::province_label(cx.facts, *p) };
                if ui.link(RichText::new(format!("📍 {place}")).strong()).on_hover_text("list only these").clicked() {
                    a.selected = Some(*p);
                }
                if ds.len() > 1 && ui.small_button(format!("✖ all {}", ds.len())).on_hover_text("remove the divisions of this province").clicked() {
                    acts.push(Act::Remove(ds.iter().map(|d| d.span.clone()).collect()));
                }
            });
            for d in ds {
                ui.horizontal(|ui| {
                    let id = egui::Id::unique(("army_div", d.span.start));
                    let current = d.name.clone().unwrap_or_default();
                    let hint = d.label();
                    cell(ui, COLS[0], |ui| match bad.get(&d.span.start) {
                        Some(sev) => ui.label(RichText::new(if *sev == Severity::Error { "✖" } else { "⚠" }).color(if *sev == Severity::Error { ERR } else { WARN })),
                        None => ui.label(""),
                    });
                    cell(ui, COLS[1], |ui| {
                        if let Some(n) = commit_text(ui, id.with("name"), &current, COLS[1], &hint).filter(|n| !n.trim().is_empty()) {
                            acts.push(Act::Rename(d.span.clone(), n.trim().to_string()));
                        }
                    });
                    let mut t = d.template.clone().unwrap_or_default();
                    cell(ui, COLS[2], |ui| {
                        egui::ComboBox::from_id_salt(id.with("template")).selected_text(if t.is_empty() { "no template".to_string() } else { t.clone() }).width(COLS[2]).truncate().show_ui(ui, |ui| {
                            for x in templates {
                                ui.selectable_value(&mut t, x.clone(), x);
                            }
                        })
                    });
                    if !t.is_empty() && Some(&t) != d.template.as_ref() {
                        acts.push(Act::Template(d.span.clone(), t));
                    }
                    let set = cell(ui, COLS[3], |ui| match d.experience {
                        Some(v) => commit_pct(ui, id.with("exp"), v, &format!(" {}", level_name(cx, v)), EXPERIENCE_TIP),
                        None => ui
                            .button(RichText::new(format!("{} (none)", level_name(cx, 0.0))).weak())
                            .on_hover_text(format!("{EXPERIENCE_TIP}. Not given: 0, green. Click to make it {} ({}%)", level_name(cx, army::EXPERIENCE), army::EXPERIENCE * 100.0))
                            .clicked()
                            .then_some(army::EXPERIENCE),
                    });
                    if let Some(v) = set {
                        acts.push(Act::Factor(d.span.clone(), "start_experience_factor", v));
                    }
                    let set = cell(ui, COLS[4], |ui| match d.equipment {
                        Some(v) => commit_pct(ui, id.with("equip"), v, "", EQUIPMENT_TIP),
                        None => ui.button(RichText::new("100%").weak()).on_hover_text(format!("{EQUIPMENT_TIP}. Not given: all of it. Click to write it")).clicked().then_some(1.0),
                    });
                    if let Some(v) = set {
                        acts.push(Act::Factor(d.span.clone(), "start_equipment_factor", v));
                    }
                    let set = cell(ui, COLS[5], |ui| match d.manpower {
                        Some(v) => commit_pct(ui, id.with("men"), v, "", MEN_TIP),
                        None => ui.button(RichText::new("reserves").color(WARN)).on_hover_text(format!("Not given{}. Click to give it all its men (start_manpower_factor = 1)", army::MEN_FROM_RESERVES)).clicked().then_some(army::MANPOWER),
                    });
                    if let Some(v) = set {
                        acts.push(Act::Factor(d.span.clone(), "start_manpower_factor", v));
                    }
                    if cell(ui, COLS[6], |ui| ui.small_button("📍").on_hover_text("move it: click a province on the map").clicked()) {
                        a.moving = Some((d.span.clone(), d.label()));
                    }
                    if cell(ui, COLS[7], |ui| ui.small_button("✖").on_hover_text("remove it").clicked()) {
                        acts.push(Act::Remove(vec![d.span.clone()]));
                    }
                });
            }
            ui.add_space(4.0);
        }
        if divs.is_empty() {
            ui.label(RichText::new("No divisions yet: pick a template above and click provinces on the map.").weak());
        }
    });
    // what is wrong
    let mut shown = 0;
    for p in problems {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(p.severity.label()).color(if p.severity == Severity::Error { ERR } else { WARN }).strong());
            ui.label(&p.message);
        });
        shown += 1;
    }
    if shown == 0 && !divs.is_empty() {
        ui.colored_label(OK, "✔ No problems with the divisions.");
    }
}

const EXPERIENCE_TIP: &str = "Experience at the start (start_experience_factor). The game's levels: Green, Trained from 10%, Regular from 30%, Seasoned from 75%, Veterans from 90%";
const EQUIPMENT_TIP: &str = "Its share of equipment at the start (start_equipment_factor), not from the stockpile; it takes the rest from the stockpile, when the stockpile has it";
const MEN_TIP: &str = "Its share of men at the start (start_manpower_factor), not out of the reserves; without it, its men come out of the reserves";

/// The widths of the columns of the division list: problem, name,
/// template, experience, equipment, men, move, remove.
const COLS: [f32; 8] = [14.0, 140.0, 118.0, 104.0, 50.0, 66.0, 20.0, 20.0];

/// A cell of the division list: `w` wide, whatever it holds, so the rows of
/// every province line up under the titles.
fn cell<R>(ui: &mut egui::Ui, w: f32, f: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let h = ui.spacing().interact_size.y;
    ui.allocate_ui_with_layout(Vec2::new(w, h), egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.set_min_size(Vec2::new(w, h));
        ui.set_max_width(w);
        f(ui)
    })
    .inner
}

/// The game's name of the level of an experience.
fn level_name(cx: &Cx, v: f64) -> String {
    let l = army::level(v);
    cx.text(&format!("UNIT_LEVEL_{l}")).map(loc::plain).filter(|t| !t.trim().is_empty()).unwrap_or_else(|| army::LEVEL_NAMES[l].to_string())
}

/// A share in % (0 to 100) that is handed back once dragging or typing ends
/// (`after`: after the %); as wide as its place.
fn commit_pct(ui: &mut egui::Ui, id: egui::Id, current: f64, after: &str, tip: &str) -> Option<f64> {
    let mut v: f64 = ui.data(|d| d.get_temp(id)).unwrap_or(current * 100.0);
    let size = Vec2::new(ui.available_width(), ui.spacing().interact_size.y);
    let r = ui.add_sized(size, egui::DragValue::new(&mut v).range(0.0..=100.0).speed(1.0).max_decimals(0).suffix(format!("%{after}"))).on_hover_text(tip);
    if r.dragged() || r.has_focus() {
        ui.data_mut(|d| d.insert_temp(id, v));
        return None;
    }
    ui.data_mut(|d| d.remove::<f64>(id));
    ((v - current * 100.0).abs() > 1e-6).then_some(v / 100.0)
}

/// The edits of a change to the divisions (the order of battle's `text`).
fn edits(text: &str, root: &Block, act: &Act, names: &BTreeSet<String>) -> Vec<Edit> {
    let block_at = |span: &Span| item_at(root, span).and_then(|i| i.block());
    match act {
        Act::Add { province, template, n } => {
            let new: Vec<String> = army::division_names(template, names, *n as usize).iter().map(|name| army::render_division(name, template, *province, army::EXPERIENCE, army::EQUIPMENT, army::MANPOWER)).collect();
            match root.blocks("units").last() {
                Some(u) => vec![script::insert_item(text, u, &new.join("\n"), None)],
                None => vec![script::insert_item(text, root, &format!("units = {{\n\t{}\n}}", new.join("\n").replace('\n', "\n\t")), None)],
            }
        }
        Act::Rename(span, n) => {
            let Some(b) = block_at(span) else { return Vec::new() };
            let mut e = vec![script::set_atom(text, b, "name", &script::quote(n))];
            // a name of its own instead of the next of the list
            if let Some(i) = b.get("division_name") {
                e.push(script::remove_item(text, i));
            }
            e
        }
        Act::Template(span, t) => block_at(span).map(|b| vec![script::set_atom(text, b, "division_template", &script::quote(t))]).unwrap_or_default(),
        Act::Factor(span, key, v) => block_at(span).map(|b| vec![script::set_atom(text, b, key, &format!("{}", (v * 100.0).round() / 100.0))]).unwrap_or_default(),
        Act::Move(span, p) => block_at(span).map(|b| vec![script::set_atom(text, b, "location", &p.to_string())]).unwrap_or_default(),
        Act::Remove(spans) => spans.iter().filter_map(|s| item_at(root, s)).map(|i| script::remove_item(text, i)).collect(),
        Act::AllMen => army::read_divisions(root).iter().filter(|d| d.manpower.is_none()).filter_map(|d| block_at(&d.span)).map(|b| script::set_atom(text, b, "start_manpower_factor", "1")).collect(),
        Act::NewTemplate(n) => vec![insert_template(text, root, &kinds::template(Kind::DivisionTemplate, n, ""))],
        _ => Vec::new(),
    }
}

/// A template put after the last of the file's, else at its top.
pub(super) fn insert_template(text: &str, root: &Block, rendered: &str) -> Edit {
    match root.items.iter().rposition(|i| i.is("division_template")) {
        Some(n) => script::insert_item(text, root, rendered, Some(n)),
        None if root.items.is_empty() => script::insert_item(text, root, rendered, None),
        None => Edit { range: 0..0, text: format!("{}\n\n", rendered.trim_end()) },
    }
}

fn apply(cx: &mut Cx, tag: &str, file: Option<(String, String)>, acts: Vec<Act>, a: &mut ArmyState) {
    for act in acts {
        match act {
            Act::MakeOob(n) => make_oob(cx, tag, &n),
            Act::Stock(adds, what) => {
                let hist = history_file(cx, tag);
                if edit_with(cx, tag, &hist, &format!("army: {what} into the stockpile"), None, |text, root| super::stockpile::add_to(text, root, &adds)) {
                    cx.info(format!("{what} is in {tag}'s stockpile ({hist})"));
                }
            }
            Act::MoveTemplate { from, name } => {
                let Some((pkg, path)) = &file else { continue };
                let Some(src) = cx.st.text(tag, &from).map(str::to_string) else { continue };
                let s = Script::parse(&src);
                let Some(item) = s.root.items.iter().find(|i| i.is("division_template") && i.block().and_then(|b| b.atom("name")) == Some(name.as_str())) else { continue };
                let rendered = script::item_text(&src, item).to_string();
                cx.st.begin(format!("army: move the template {name} into {path}"));
                cx.st.set_text(tag, &from, script::apply(&src, &[script::remove_item(&src, item)]));
                cx.st.edit_text(pkg, path, |text| {
                    let s = Script::parse(text);
                    script::apply(text, &[insert_template(text, &s.root, &rendered)])
                });
                cx.st.commit();
            }
            act => {
                let Some((pkg, path)) = &file else { continue };
                let names: BTreeSet<String> = cx.st.text(pkg, path).map(|t| army::read_divisions(&Script::parse(t).root).into_iter().filter_map(|d| d.name).collect()).unwrap_or_default();
                let (label, merge) = match &act {
                    Act::Add { template, n, province } => (format!("army: {n} × {template} in province {province}"), None),
                    Act::Rename(_, n) => (format!("army: rename a division {n}"), None),
                    Act::Template(_, t) => (format!("army: a division of {t}"), None),
                    Act::Factor(span, key, _) => (format!("army: {key}"), Some(format!("army {key} {}", span.start))),
                    Act::Move(_, p) => (format!("army: move a division to province {p}"), None),
                    Act::Remove(s) => (format!("army: remove {} division(s)", s.len()), None),
                    Act::AllMen => ("army: every division its men".to_string(), None),
                    Act::NewTemplate(n) => (format!("army: new template {n}"), None),
                    _ => ("army".into(), None),
                };
                if edit_with(cx, pkg, path, &label, merge, |text, root| edits(text, root, &act, &names))
                    && let Act::NewTemplate(n) = &act
                {
                    a.place = Some(n.clone());
                    if let Some(k) = cx.st.find_in(pkg, &[Kind::DivisionTemplate], n) {
                        cx.go(Nav::Def(k));
                    }
                }
            }
        }
    }
}

/// The order of battle `name` made: its file when missing, and `oob =
/// "<name>"` in the history (after its capital) when it has no oob line
/// (one undo step).
fn make_oob(cx: &mut Cx, tag: &str, name: &str) {
    let path = format!("history/units/{name}.txt");
    cx.st.begin(format!("army: order of battle {name}"));
    if cx.st.file(tag, &path).is_none() {
        cx.st.set_text(tag, &path, "units = {\n}\n".into());
    }
    if army::army_oobs(cx.st, tag).is_empty() {
        let hist = history_file(cx, tag);
        cx.st.edit_text(tag, &hist, |text| {
            let s = Script::parse(text);
            let after = s.root.items.iter().position(|i| i.is("capital"));
            let line = format!("oob = {}", script::quote(name));
            match after {
                Some(n) => script::apply(text, &[script::insert_item(text, &s.root, &line, Some(n))]),
                None => format!("{line}\n{text}"),
            }
        });
    }
    cx.st.commit();
    cx.info(format!("{tag} starts with the order of battle {path}"));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(text: &str, act: &Act) -> String {
        let s = Script::parse(text);
        let names = army::read_divisions(&s.root).into_iter().filter_map(|d| d.name).collect();
        script::apply(text, &edits(text, &s.root, act, &names))
    }

    const OOB: &str = "division_template = {\n\tname = \"Schuetzen\"\n\tregiments = {\n\t\tinfantry = { x = 0 y = 0 }\n\t}\n}\n\nunits = {\n\tdivision = {\n\t\tname = \"1. Schuetzen\"\n\t\tlocation = 10\n\t\tdivision_template = \"Schuetzen\"\n\t\tstart_experience_factor = 0.2\n\t\tstart_equipment_factor = 0.7\n\t}\n\tdivision = { division_name = { is_name_ordered = yes name_order = 2 } location = 11 division_template = \"Schuetzen\" }\n}\n";

    #[test]
    fn divisions_are_placed_and_changed() {
        // two more in a province, numbered after the first
        let t = run(OOB, &Act::Add { province: 12, template: "Schuetzen".into(), n: 2 });
        let d = army::read_divisions(&Script::parse(&t).root);
        assert_eq!(d.iter().map(|d| (d.label(), d.location)).collect::<Vec<_>>(), [("1. Schuetzen".to_string(), Some(10)), ("no. 2 of its name list".into(), Some(11)), ("2. Schuetzen".into(), Some(12)), ("3. Schuetzen".into(), Some(12))]);
        assert_eq!((d[2].experience, d[2].equipment, d[2].manpower), (Some(army::EXPERIENCE), Some(army::EQUIPMENT), Some(army::MANPOWER)), "new divisions get their men, not the reserves'");
        assert!(t.starts_with(&OOB[..OOB.len() - 2]), "the rest kept:\n{t}");
        // a file without units gets them
        let t = run("division_template = { name = \"A\" }\n", &Act::Add { province: 3, template: "A".into(), n: 1 });
        assert_eq!(army::read_divisions(&Script::parse(&t).root)[0].location, Some(3), "{t}");
        // renamed: its own name instead of the list's
        let s = Script::parse(OOB);
        let d = army::read_divisions(&s.root);
        let t = run(OOB, &Act::Rename(d[1].span.clone(), "Grenzschutz".into()));
        let r = army::read_divisions(&Script::parse(&t).root);
        assert_eq!((r[1].name.as_deref(), r[1].order), (Some("Grenzschutz"), None), "{t}");
        // moved, other template, experience
        let t = run(OOB, &Act::Move(d[0].span.clone(), 99));
        assert_eq!(army::read_divisions(&Script::parse(&t).root)[0].location, Some(99));
        let t = run(OOB, &Act::Factor(d[0].span.clone(), "start_experience_factor", 0.456));
        assert_eq!(army::read_divisions(&Script::parse(&t).root)[0].experience, Some(0.46));
        let t = run(OOB, &Act::Template(d[0].span.clone(), "Jaeger".into()));
        assert_eq!(army::read_divisions(&Script::parse(&t).root)[0].template.as_deref(), Some("Jaeger"));
        let t = run(OOB, &Act::Factor(d[0].span.clone(), "start_manpower_factor", 0.5));
        assert_eq!(army::read_divisions(&Script::parse(&t).root)[0].manpower, Some(0.5));
        // every division its men: those without, the others kept
        let t = run(&t, &Act::AllMen);
        assert_eq!(army::read_divisions(&Script::parse(&t).root).iter().map(|d| d.manpower).collect::<Vec<_>>(), [Some(0.5), Some(1.0)], "{t}");
        // removed, both
        let t = run(OOB, &Act::Remove(d.iter().map(|d| d.span.clone()).collect()));
        assert!(army::read_divisions(&Script::parse(&t).root).is_empty());
        assert!(t.contains("units = {") && t.contains("Schuetzen"), "{t}");
    }

    #[test]
    fn new_templates_go_after_the_others() {
        let t = run(OOB, &Act::NewTemplate("Jaeger".into()));
        let s = Script::parse(&t);
        assert_eq!(army::templates_of(&s.root), ["Schuetzen", "Jaeger"]);
        assert!(s.root.items.iter().position(|i| i.is("units")) > s.root.items.iter().rposition(|i| i.is("division_template")), "{t}");
        // into a file of only divisions: at the top
        let t = run("units = {\n}\n", &Act::NewTemplate("Jaeger".into()));
        assert!(t.starts_with("division_template = {\n\tname = \"Jaeger\""), "{t}");
        assert_eq!(army::templates_of(&Script::parse(&t).root), ["Jaeger"]);
    }

    use crate::content::gfx::Gfx;
    use crate::content::store::Store;
    use crate::content::ui::{LocCache, MapFacts, Settings};

    const HIST: &str = "history/countries/TYR - Tyrol.txt";

    /// The page's changes, as the page makes them.
    fn step(st: &mut Store, acts: Vec<Act>) -> Option<Nav> {
        let (mut gfx, mut settings, facts, loc) = (Gfx::default(), Settings::default(), MapFacts::default(), LocCache::default());
        let (mut msg, mut nav) = (None, None);
        let mut cx = Cx { st, base: None, gfx: &mut gfx, settings: &mut settings, facts: &facts, pkg: "TYR", nation: None, msg: &mut msg, nav: &mut nav, loc: &loc, game: std::path::Path::new("/none"), user_dir: std::path::Path::new("/none"), docs: &Default::default() };
        let file = army::army_oob(cx.st, "TYR").and_then(|(_, f)| f);
        apply(&mut cx, "TYR", file, acts, &mut ArmyState::default());
        nav
    }

    fn divisions(st: &Store) -> Vec<(String, u32, String)> {
        let (_, f) = army::army_oob(st, "TYR").unwrap();
        let (pkg, path) = f.unwrap();
        army::read_divisions(&Script::parse(st.text(&pkg, &path).unwrap()).root).into_iter().map(|d| (d.name.unwrap_or_default(), d.location.unwrap_or(0), d.template.unwrap_or_default())).collect()
    }

    #[test]
    fn the_page_loads_its_file_and_places_divisions() {
        let mut st = Store::empty(std::env::temp_dir().join("mh-army-none"));
        st.begin("setup");
        st.set_text("TYR", HIST, "capital = 3\nset_stability = 0.5\n".into());
        st.set_text("TYR", "history/units/TYR_content.txt", "division_template = {\n\tname = \"Inf\"\n\tregiments = { infantry = { x = 0 y = 0 } }\n}\n".into());
        st.set_text("TYR", "history/units/TYR_old.txt", "division_template = {\n\tname = \"Jaeger\"\n\tregiments = { mountaineers = { x = 0 y = 0 } }\n}\n".into());
        st.commit();
        assert!(army::army_oob(&st, "TYR").is_none());
        // the file of templates loaded: the oob line after the capital
        step(&mut st, vec![Act::MakeOob("TYR_content".into())]);
        assert_eq!(st.text("TYR", HIST).unwrap(), "capital = 3\noob = \"TYR_content\"\nset_stability = 0.5\n");
        // placed: two, numbered; one undo step
        step(&mut st, vec![Act::Add { province: 7, template: "Inf".into(), n: 2 }]);
        step(&mut st, vec![Act::Add { province: 8, template: "Inf".into(), n: 1 }]);
        assert_eq!(divisions(&st), [("1. Inf".to_string(), 7, "Inf".to_string()), ("2. Inf".into(), 7, "Inf".into()), ("3. Inf".into(), 8, "Inf".into())]);
        st.undo();
        assert_eq!(divisions(&st).len(), 2);
        // a template of a file nothing loads moved in (both files, one step)
        step(&mut st, vec![Act::MoveTemplate { from: "history/units/TYR_old.txt".into(), name: "Jaeger".into() }]);
        let s = Script::parse(st.text("TYR", "history/units/TYR_content.txt").unwrap());
        assert_eq!(army::templates_of(&s.root), ["Inf", "Jaeger"]);
        assert!(army::templates_of(&Script::parse(st.text("TYR", "history/units/TYR_old.txt").unwrap()).root).is_empty());
        assert!(s.root.items.iter().position(|i| i.is("units")) > s.root.items.iter().rposition(|i| i.is("division_template")), "templates before the divisions");
        st.undo();
        assert_eq!(army::templates_of(&Script::parse(st.text("TYR", "history/units/TYR_old.txt").unwrap()).root), ["Jaeger"]);
        // a new template opens in the designer
        let nav = step(&mut st, vec![Act::NewTemplate("Gebirgsjaeger".into())]);
        assert!(matches!(nav, Some(Nav::Def(k)) if k.id == "Gebirgsjaeger" && k.file == "history/units/TYR_content.txt"));
        // moved, removed
        let spans: Vec<Span> = {
            let t = st.text("TYR", "history/units/TYR_content.txt").unwrap();
            army::read_divisions(&Script::parse(t).root).into_iter().map(|d| d.span).collect()
        };
        step(&mut st, vec![Act::Move(spans[1].clone(), 9)]);
        assert_eq!(divisions(&st)[1].1, 9);
        step(&mut st, vec![Act::Remove(vec![spans[0].clone()])]);
        assert_eq!(divisions(&st), [("2. Inf".to_string(), 9, "Inf".to_string())]);
    }

    #[test]
    fn a_new_template_of_the_list_goes_into_the_order_of_battle() {
        let mut st = Store::empty(std::env::temp_dir().join("mh-army-none"));
        st.begin("setup");
        st.set_text("TYR", HIST, "oob = \"TYR_army\"\n".into());
        st.set_text("TYR", "history/units/TYR_army.txt", "units = {\n\tdivision = { location = 3 division_template = \"Inf\" }\n}\n".into());
        st.commit();
        let (mut gfx, mut settings, facts, loc) = (Gfx::default(), Settings::default(), MapFacts::default(), LocCache::default());
        let (mut msg, mut nav) = (None, None);
        let mut cx = Cx { st: &mut st, base: None, gfx: &mut gfx, settings: &mut settings, facts: &facts, pkg: "TYR", nation: None, msg: &mut msg, nav: &mut nav, loc: &loc, game: std::path::Path::new("/none"), user_dir: std::path::Path::new("/none"), docs: &Default::default() };
        let k = cx.create(Kind::DivisionTemplate, None, "Inf").unwrap();
        assert_eq!(k.file, "history/units/TYR_army.txt");
        assert!(st.text("TYR", "history/units/TYR_army.txt").unwrap().starts_with("division_template = {"));
    }

    #[test]
    fn the_stockpile_gets_what_the_army_lacks() {
        let mut st = Store::empty(std::env::temp_dir().join("mh-army-none"));
        st.begin("setup");
        st.set_text("TYR", HIST, "capital = 3\nadd_equipment_to_stockpile = { type = infantry_equipment_1 amount = 100 }\n".into());
        st.commit();
        step(&mut st, vec![Act::Stock(vec![("infantry_equipment_1".into(), 50.0, Need::default()), ("artillery_equipment_1".into(), 12.0, Need::default())], "what the army lacks".into())]);
        assert_eq!(st.text("TYR", HIST).unwrap(), "capital = 3\nadd_equipment_to_stockpile = { type = infantry_equipment_1 amount = 150 }\nadd_equipment_to_stockpile = {\n\ttype = artillery_equipment_1\n\tamount = 12\n}\n");
        assert_eq!(crate::content::units::start_stock(&st, "TYR", None).get("artillery_equipment_1"), Some(&12.0));
        // one undo step
        st.undo();
        assert_eq!(st.text("TYR", HIST).unwrap(), "capital = 3\nadd_equipment_to_stockpile = { type = infantry_equipment_1 amount = 100 }\n");
    }

    #[test]
    fn a_new_order_of_battle_is_made() {
        let mut st = Store::empty(std::env::temp_dir().join("mh-army-none"));
        st.begin("setup");
        st.set_text("TYR", HIST, "set_stability = 0.5\n".into());
        st.commit();
        step(&mut st, vec![Act::MakeOob("TYR_army".into())]);
        assert_eq!(st.text("TYR", HIST).unwrap(), "oob = \"TYR_army\"\nset_stability = 0.5\n");
        assert_eq!(st.text("TYR", "history/units/TYR_army.txt"), Some("units = {\n}\n"));
        let nav = step(&mut st, vec![Act::NewTemplate("Inf".into())]);
        assert!(matches!(nav, Some(Nav::Def(_))));
        step(&mut st, vec![Act::Add { province: 4, template: "Inf".into(), n: 1 }]);
        let t = st.text("TYR", "history/units/TYR_army.txt").unwrap();
        assert!(t.starts_with("division_template = {\n\tname = \"Inf\""), "{t}");
        assert_eq!(divisions(&st), [("1. Inf".to_string(), 4, "Inf".to_string())]);
        // one undo step each: the file and the line go together
        st.undo();
        st.undo();
        st.undo();
        assert!(army::army_oob(&st, "TYR").is_none() && st.text("TYR", "history/units/TYR_army.txt").is_none());
    }
}
