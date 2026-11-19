//! The division designer of a division template: its battalions and
//! companies on the game's grid with their icons, picked from what the
//! nation can use at the start; its problems (for players with and without
//! the DLCs the nation's history checks); and its stats, with the nation's
//! research and stockpile at the start or all research up to a year, for
//! players with the DLCs picked (as the focus tree view: one choice).

use std::collections::BTreeMap;
use std::sync::Arc;

use eframe::egui::{self, Color32, RichText, Sense, Stroke, Vec2};

use super::{Cx, ERR, OK, WARN};
use crate::content::dlc::{self, Owned};
use crate::content::loc;
use crate::content::script;
use crate::content::store::{DefKey, SHARED};
use crate::content::units::{self, Division, Have, Problem, Section, SubUnit, Template, UnitDb, group_label};
use crate::content::validate::Severity;

#[derive(Default)]
pub struct DesignerState {
    /// The stats with all research up to this year (None: the nation's at
    /// the start).
    pub year: Option<u32>,
    /// The pickers list what the nation cannot use as well (greyed out).
    pub show_locked: bool,
    query: String,
    /// The list of this place opens (for screenshots).
    start: Option<(Section, i64, i64)>,
    /// What the nation has at the start and for the stats, by (content
    /// generation, tag, year, DLCs).
    have: Option<(HaveKey, Arc<(Have, Have)>)>,
}

/// Content generation, tag, year, DLCs of the player.
type HaveKey = (u64, String, Option<u32>, Owned);

/// The DLCs the views assume: those picked, else all of them.
pub fn owned(cx: &Cx, db: &UnitDb, tag: &str) -> Owned {
    match &cx.settings.dlcs {
        Some(o) => o.clone(),
        None => {
            let mut all = dlc::known(cx.base.map(|b| &b.files));
            all.extend(db.dlcs());
            all.extend(units::history_dlcs(cx.st, tag));
            all
        }
    }
}

impl DesignerState {
    /// Open the list of a place once shown: `regiments:1:0`.
    pub fn open_at(&mut self, spec: &str) {
        let mut it = spec.split(':');
        let section = it.next().and_then(|k| Section::ALL.into_iter().find(|s| s.key() == k));
        let x = it.next().and_then(|v| v.parse().ok());
        let y = it.next().and_then(|v| v.parse().ok());
        if let (Some(s), Some(x), Some(y)) = (section, x, y) {
            self.start = Some((s, x, y));
        }
    }
}

const SLOT: Vec2 = Vec2::new(80.0, 58.0);

/// A change asked for in the designer.
enum Act {
    Put(Section, i64, i64, String),
    Remove(Section, i64, i64),
    /// The empty places of a column filled with a battalion.
    Fill(i64, String),
    /// A column emptied (with its regimental support).
    Clear(i64),
}

/// What the cells and pickers look at.
struct G<'a> {
    db: &'a UnitDb,
    t: &'a Template,
    /// What the nation has at the start (what it can use).
    start: &'a Have,
    /// The research the stats are worked out with.
    research: &'a Have,
    /// The worst problem of each slot.
    bad: &'a BTreeMap<usize, Severity>,
    problems: &'a [Problem],
    tag: &'a str,
}

/// A name as the game shows it.
fn name(cx: &Cx, id: &str) -> String {
    cx.text(id).map(loc::plain).filter(|t| !t.trim().is_empty()).unwrap_or_else(|| loc::prettify(id))
}

fn colour(s: Severity) -> Color32 {
    match s {
        Severity::Error => ERR,
        Severity::Warning => WARN,
        Severity::Info => Color32::GRAY,
    }
}

fn num(v: f64) -> String {
    if (v - v.round()).abs() < 0.005 { format!("{}", v.round() as i64) } else if v.abs() >= 10.0 { format!("{v:.1}") } else { format!("{v:.2}") }
}

fn pct(v: f64) -> String {
    format!("{}%", num(v * 100.0))
}

pub fn designer(ui: &mut egui::Ui, cx: &mut Cx, key: &DefKey, ds: &mut DesignerState) {
    let Some(base) = cx.base else {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Reading the game's battalions for the designer…");
        });
        return;
    };
    let db = units::db(Some(base), cx.st);
    let Some(t) = cx.st.def(key).and_then(|(d, _, s)| d.block(s).map(Template::read)) else { return };
    let tag = key.pkg.clone();
    let owned = owned(cx, &db, &tag);
    let hkey = (cx.st.generation, tag.clone(), ds.year, owned.clone());
    if ds.have.as_ref().is_none_or(|h| h.0 != hkey) {
        let start = db.at_start(cx.st, &tag, Some(&owned));
        let research = match ds.year {
            Some(y) => db.have(db.up_to(&start.techs, y, Some(&owned))),
            None => start.clone(),
        };
        ds.have = Some((hkey, Arc::new((start, research))));
    }
    let have = ds.have.as_ref().map(|h| h.1.clone()).unwrap_or_default();
    let who = if tag == SHARED { "every nation".to_string() } else { tag.clone() };
    let problems = db.check_for_players(cx.st, &t, &tag, &who, &|id| name(cx, id));
    let mut bad: BTreeMap<usize, Severity> = BTreeMap::new();
    for p in &problems {
        if let Some(i) = p.slot {
            let e = bad.entry(i).or_insert(p.severity);
            *e = (*e).min(p.severity);
        }
    }
    let g = G { db: &db, t: &t, start: &have.0, research: &have.1, bad: &bad, problems: &problems, tag: &who };
    let mut acts: Vec<Act> = Vec::new();
    egui::CollapsingHeader::new(RichText::new("Division designer").strong()).id_salt("div_designer").default_open(true).show(ui, |ui| {
        ui.label(
            RichText::new(format!(
                "Click a place to put a battalion or company there (only those {who} can use at the start are listed), click it again to change or remove it; right-click for the column. Each change is one undo step."
            ))
            .weak()
            .small(),
        );
        grid(ui, cx, &g, ds, &mut acts);
        if tag != SHARED && ui.small_button("🪖 Place divisions of it on the map").on_hover_text("the Army page, with this template to place").clicked() {
            super::army::place_next(ui.ctx(), &key.id);
            cx.go(super::Nav::Page(super::Page::Army));
        }
        ui.add_space(4.0);
        if problems.is_empty() {
            ui.colored_label(OK, "✔ No problems: the game takes this template as it is.");
        }
        for p in &problems {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(p.severity.label()).color(colour(p.severity)).strong());
                ui.label(&p.message);
            });
        }
        ui.add_space(6.0);
        research_row(ui, cx, &db, ds, &who, &tag);
        let div = db.division(&t, &have.1);
        stats(ui, cx, &g, &div);
    });
    apply(cx, key, acts);
}

/// The research the stats are worked out with, and the DLCs of the player.
fn research_row(ui: &mut egui::Ui, cx: &mut Cx, db: &UnitDb, ds: &mut DesignerState, who: &str, tag: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Stats").strong());
        ui.label("with");
        let label = |y: Option<u32>| match y {
            None => format!("{who}'s research at the start"),
            Some(y) => format!("all research up to {y}"),
        };
        egui::ComboBox::from_id_salt("div_year")
            .selected_text(label(ds.year))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut ds.year, None, label(None));
                for y in db.years().into_iter().filter(|y| (1936..=1950).contains(y)) {
                    ui.selectable_value(&mut ds.year, Some(y), label(Some(y)));
                }
            })
            .response
            .on_hover_text(
                "At the start: the technologies the build gives every nation and those of its history (set_technology), and its stockpile (add_equipment_to_stockpile). Up to a year: also every technology of the research screen meant for that year or before (not those of focuses and events), with the newest equipment they bring.",
            );
        dlc_menu(ui, cx, db, tag);
    });
}

// ------------------------------------------------------------------ grid

fn grid(ui: &mut egui::Ui, cx: &mut Cx, g: &G, ds: &mut DesignerState, acts: &mut Vec<Act>) {
    let has_rs = g.db.subunits.values().any(|u| u.regimental_support);
    egui::Frame::group(ui.style()).show(ui, |ui| {
        egui::Grid::new("div_grid").spacing([6.0, 4.0]).show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("Support").small().strong());
                ui.label(RichText::new("companies").small().weak());
            });
            for x in 0..5 {
                let width: f64 = g.t.of(Section::Regiments).filter(|s| s.x == x).filter_map(|s| g.db.subunits.get(&s.unit)).map(|u| u.stat("combat_width")).sum();
                let group = g.t.column_group(g.db, x).map(group_label).unwrap_or_default();
                let r = ui.vertical_centered(|ui| {
                    ui.label(RichText::new(format!("Column {}", x + 1)).small().strong());
                    ui.label(RichText::new(if group.is_empty() { "empty".to_string() } else { format!("{group} · width {}", num(width)) }).small().weak());
                });
                r.response.on_hover_text("A column (regiment) holds battalions of one group: infantry, mobile, armor, combat support ... Right-click a battalion of it to fill or empty it.");
            }
            ui.end_row();
            if has_rs {
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new("regimental").small().weak());
                    ui.label(RichText::new("support").small().weak());
                })
                .response
                .on_hover_text("A regimental support company goes on top of a column of battalions (one each) and supports that column only.");
                for x in 0..5 {
                    cell(ui, cx, g, ds, acts, Section::RegimentalSupport, x, 0);
                }
                ui.end_row();
            }
            for y in 0..5 {
                cell(ui, cx, g, ds, acts, Section::Support, 0, y);
                for x in 0..5 {
                    cell(ui, cx, g, ds, acts, Section::Regiments, x, y);
                }
                ui.end_row();
            }
        });
    });
}

#[allow(clippy::too_many_arguments)]
fn cell(ui: &mut egui::Ui, cx: &mut Cx, g: &G, ds: &mut DesignerState, acts: &mut Vec<Act>, section: Section, x: i64, y: i64) {
    let idx = g.t.slots.iter().position(|s| s.section == section && s.x == x && s.y == y);
    let slot = idx.map(|i| &g.t.slots[i]);
    let unit = slot.and_then(|s| g.db.subunits.get(&s.unit));
    let open = section != Section::RegimentalSupport || g.t.of(Section::Regiments).any(|s| s.x == x);
    let (rect, resp) = ui.allocate_exact_size(SLOT, if open || slot.is_some() { Sense::click() } else { Sense::hover() });
    let vis = ui.visuals().clone();
    let sev = idx.and_then(|i| g.bad.get(&i).copied());
    let stroke = match sev {
        Some(s @ (Severity::Error | Severity::Warning)) => Stroke::new(2.0, colour(s)),
        _ if resp.hovered() && (open || slot.is_some()) => vis.widgets.hovered.bg_stroke,
        _ => vis.widgets.noninteractive.bg_stroke,
    };
    let fill = if slot.is_some() { vis.widgets.inactive.weak_bg_fill } else { vis.extreme_bg_color };
    ui.painter().rect(rect, 4.0, fill, stroke, egui::StrokeKind::Inside);
    let font = egui::FontId::proportional(11.0);
    match (slot, unit) {
        (Some(_), Some(u)) => {
            if let Some((tex, size)) = cx.texture(ui.ctx(), &u.icon()).filter(|(_, s)| s.x > 0.0 && s.y > 0.0) {
                let max = Vec2::new(rect.width() - 10.0, 32.0);
                let k = (max.x / size.x).min(max.y / size.y);
                let r = egui::Rect::from_center_size(egui::pos2(rect.center().x, rect.top() + 5.0 + max.y / 2.0), size * k);
                ui.painter().image(tex, r, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
            } else {
                ui.painter().text(egui::pos2(rect.center().x, rect.top() + 21.0), egui::Align2::CENTER_CENTER, if u.abbreviation.is_empty() { "?" } else { &u.abbreviation }, egui::FontId::proportional(16.0), vis.strong_text_color());
            }
            let mut n = name(cx, &u.id);
            if n.chars().count() > 14 {
                n = format!("{}…", n.chars().take(13).collect::<String>());
            }
            ui.painter().text(egui::pos2(rect.center().x, rect.bottom() - 10.0), egui::Align2::CENTER_CENTER, n, font, vis.text_color());
        }
        (Some(s), None) => {
            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, &s.unit, font, ERR);
        }
        (None, _) if open => {
            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "+", egui::FontId::proportional(20.0), vis.weak_text_color());
        }
        _ => {}
    }
    let resp = match (unit, idx) {
        (Some(u), Some(i)) => resp.on_hover_ui(|ui| {
            card(ui, cx, g, u);
            for p in g.problems.iter().filter(|p| p.slot == Some(i)) {
                ui.colored_label(colour(p.severity), &p.message);
            }
        }),
        (None, Some(i)) => resp.on_hover_ui(|ui| {
            for p in g.problems.iter().filter(|p| p.slot == Some(i)) {
                ui.colored_label(colour(p.severity), &p.message);
            }
        }),
        (None, None) if !open => resp.on_hover_text("Put battalions in this column first."),
        _ => resp,
    };
    if ds.start == Some((section, x, y)) {
        ds.start = None;
        egui::Popup::open_id(ui.ctx(), egui::Popup::default_response_id(&resp));
    }
    if open || slot.is_some() {
        egui::Popup::from_toggle_button_response(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
            ui.set_min_width(360.0);
            picker(ui, cx, g, ds, acts, section, x, y, slot.map(|s| s.unit.as_str()));
        });
    }
    if section == Section::Regiments {
        resp.context_menu(|ui| {
            if let Some(s) = slot {
                let n = name(cx, &s.unit);
                if ui.button(format!("Fill column {} with {n}", x + 1)).clicked() {
                    acts.push(Act::Fill(x, s.unit.clone()));
                    ui.close();
                }
                if ui.button(format!("Remove {n}")).clicked() {
                    acts.push(Act::Remove(section, x, y));
                    ui.close();
                }
            }
            let has = g.t.slots.iter().any(|s| (s.section == Section::Regiments || s.section == Section::RegimentalSupport) && s.x == x);
            if ui.add_enabled(has, egui::Button::new(format!("Empty column {}", x + 1))).clicked() {
                acts.push(Act::Clear(x));
                ui.close();
            }
        });
    }
}

/// The list of what can go in a place.
#[allow(clippy::too_many_arguments)]
fn picker(ui: &mut egui::Ui, cx: &mut Cx, g: &G, ds: &mut DesignerState, acts: &mut Vec<Act>, section: Section, x: i64, y: i64, current: Option<&str>) {
    ui.horizontal(|ui| {
        let what = match section {
            Section::Regiments => format!("Battalion: column {}, row {}", x + 1, y + 1),
            Section::Support => format!("Support company {}", y + 1),
            Section::RegimentalSupport => format!("Regimental support of column {}", x + 1),
        };
        ui.label(RichText::new(what).strong());
        if current.is_some() && ui.button("✖ Remove").clicked() {
            acts.push(Act::Remove(section, x, y));
            ui.close();
        }
    });
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut ds.query).hint_text("search").desired_width(200.0));
        ui.checkbox(&mut ds.show_locked, "all").on_hover_text(format!("Also list what {} cannot use at the start, or what cannot go here (greyed out, with why)", g.tag));
    });
    // the column's group (without this place), the support types taken
    let group: Option<String> = match section {
        Section::Regiments => {
            let mut col: Vec<_> = g.t.of(Section::Regiments).filter(|s| s.x == x && s.y != y).collect();
            col.sort_by_key(|s| s.y);
            col.iter().find_map(|s| g.db.subunits.get(&s.unit)).map(|u| u.group.clone())
        }
        Section::RegimentalSupport => g.t.column_group(g.db, x).map(String::from),
        Section::Support => None,
    };
    let taken: Vec<String> = g
        .t
        .of(Section::Support)
        .filter(|s| !(s.x == x && s.y == y))
        .filter_map(|s| g.db.subunits.get(&s.unit))
        .map(|u| u.same_support_type.clone().unwrap_or_else(|| u.id.clone()))
        .collect();
    let q = ds.query.trim().to_lowercase();
    let mut list: Vec<(String, &SubUnit)> = g.db.subunits.values().filter(|u| u.section() == Some(section)).map(|u| (name(cx, &u.id), u)).collect();
    list.retain(|(n, u)| q.is_empty() || n.to_lowercase().contains(&q) || u.id.contains(&q));
    list.sort_by(|a, b| (group_order(&a.1.group), &a.1.group, &a.0).cmp(&(group_order(&b.1.group), &b.1.group, &b.0)));
    let mut shown = 0;
    egui::ScrollArea::vertical().max_height(340.0).show(ui, |ui| {
        let mut last = String::new();
        for (n, u) in list {
            let why: Option<String> = if !g.start.units.contains(&u.id) {
                let techs = g.db.unlocked_text(&[u.id.as_str()], &|i| name(cx, i), 2);
                Some(if techs.is_empty() { format!("{} cannot use it", g.tag) } else { format!("needs {techs}") })
            } else if section == Section::Regiments && group.as_ref().is_some_and(|gr| *gr != u.group) {
                Some(format!("column {} is of {}", x + 1, group_label(group.as_deref().unwrap_or(""))))
            } else if section == Section::RegimentalSupport && group.as_ref().is_some_and(|gr| !u.allowed_groups.is_empty() && !u.allowed_groups.contains(gr)) {
                Some(format!("not on a column of {}", group_label(group.as_deref().unwrap_or(""))))
            } else if section == Section::Support && taken.contains(&u.same_support_type.clone().unwrap_or_else(|| u.id.clone())) {
                Some("the division has one".into())
            } else {
                None
            };
            if why.is_some() && !ds.show_locked {
                continue;
            }
            shown += 1;
            if u.group != last {
                ui.label(RichText::new(group_label(&u.group)).small().weak());
                last = u.group.clone();
            }
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(Vec2::new(30.0, 20.0), Sense::hover());
                if let Some((tex, size)) = cx.texture(ui.ctx(), &u.icon()).filter(|(_, s)| s.x > 0.0 && s.y > 0.0) {
                    let k = (r.width() / size.x).min(r.height() / size.y);
                    let tint = if why.is_some() { Color32::from_white_alpha(90) } else { Color32::WHITE };
                    ui.painter().image(tex, egui::Rect::from_center_size(r.center(), size * k), egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), tint);
                }
                let b = g.db.battalion(u, g.research);
                let missing: Vec<String> = b.equipment.iter().filter(|(_, m, _)| m.is_none()).map(|(e, _, _)| name(cx, e)).collect();
                let brief = if !missing.is_empty() {
                    format!("width {} · no {} with this research", num(b.get("combat_width")), missing.join(", "))
                } else if section == Section::Regiments {
                    format!("width {} · {} soft · {} hard · {} def · {} org", num(b.get("combat_width")), num(b.get("soft_attack")), num(b.get("hard_attack")), num(b.get("defense")), num(b.get("max_organisation")))
                } else {
                    brief_support(&b, u)
                };
                let r = ui.add_enabled(why.is_none(), egui::Button::selectable(current == Some(u.id.as_str()), n.as_str()));
                let r = r.on_hover_ui(|ui| card(ui, cx, g, u)).on_disabled_hover_ui(|ui| {
                    card(ui, cx, g, u);
                });
                if r.clicked() {
                    acts.push(Act::Put(section, x, y, u.id.clone()));
                    ui.close();
                }
                match &why {
                    Some(w) => ui.label(RichText::new(w).small().color(WARN)),
                    None => ui.label(RichText::new(brief).small().weak()),
                };
            });
        }
    });
    if shown == 0 {
        ui.label(RichText::new(if ds.show_locked || !q.is_empty() { "Nothing found." } else { "Nothing can go here (tick \"all\" to see why)." }).weak());
    }
}

/// The DLCs the player has, for the stats and what can be picked: all of
/// them, or those ticked (the focus tree view's choice too).
fn dlc_menu(ui: &mut egui::Ui, cx: &mut Cx, db: &UnitDb, tag: &str) {
    let mut names = db.dlcs();
    names.extend(units::history_dlcs(cx.st, tag));
    if names.is_empty() {
        return;
    }
    let installed = cx.base.map(|b| dlc::installed(&b.files)).unwrap_or_default();
    let label = match &cx.settings.dlcs {
        None => "with every DLC".to_string(),
        Some(o) => {
            let on: Vec<&String> = names.iter().filter(|d| o.contains(*d)).collect();
            if on.is_empty() { "with no DLC".to_string() } else if on.len() == names.len() { "with every DLC".to_string() } else { format!("with {}", on.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")) }
        }
    };
    ui.menu_button(format!("{label} ▾"), |ui| {
        ui.label(RichText::new("The DLCs of the player: research by year keeps to the technologies they see (No Step Back and By Blood Alone have their own), and the history's has_dlc checks go by them. The focus tree view uses the same choice.").small().weak());
        let mut all = cx.settings.dlcs.is_none();
        if ui.checkbox(&mut all, "every DLC").changed() {
            cx.settings.dlcs = if all { None } else { Some(names.clone()) };
        }
        if !installed.is_empty() && ui.button("those installed with the game").clicked() {
            cx.settings.dlcs = Some(installed.clone());
        }
        if ui.button("none").clicked() {
            cx.settings.dlcs = Some(Owned::new());
        }
        ui.separator();
        let mut o = cx.settings.dlcs.clone().unwrap_or_else(|| names.clone());
        let mut changed = false;
        for d in &names {
            let mut on = o.contains(d);
            let text = if installed.contains(d) { format!("{d} (installed)") } else { d.clone() };
            if ui.checkbox(&mut on, text).changed() {
                changed = true;
                if on {
                    o.insert(d.clone());
                } else {
                    o.remove(d);
                }
            }
        }
        if changed {
            cx.settings.dlcs = Some(o);
        }
    });
}

/// Infantry first, support last, as the game lists them.
fn group_order(g: &str) -> u8 {
    match g {
        "infantry" => 0,
        "mobile" => 1,
        "armor" => 2,
        "combat_support" => 3,
        "mobile_combat_support" => 4,
        "armor_combat_support" => 5,
        "support" => 7,
        _ => 6,
    }
}

/// What a support company is for, in a few words.
fn brief_support(b: &units::Battalion, u: &SubUnit) -> String {
    let mut v = Vec::new();
    for (k, l) in [("recon", "recon"), ("entrenchment", "entrenchment"), ("initiative", "initiative"), ("casualty_trickleback", "trickleback"), ("equipment_capture_factor", "capture")] {
        if b.get(k).abs() > 1e-9 {
            v.push(format!("{} {l}", if k == "recon" || k == "entrenchment" { num(b.get(k)) } else { pct(b.get(k)) }));
        }
    }
    if b.get("soft_attack") > 0.0 {
        v.push(format!("{} soft", num(b.get("soft_attack"))));
    }
    if b.get("hard_attack") > 0.5 {
        v.push(format!("{} hard", num(b.get("hard_attack"))));
    }
    if b.get("air_attack") > 0.0 {
        v.push(format!("{} air", num(b.get("air_attack"))));
    }
    for (cat, m) in &u.battalion_mult {
        for (k, val) in m {
            v.push(format!("{} {} of {}", pct(*val), loc::prettify(k), group_label(cat.trim_start_matches("category_"))));
        }
    }
    if !b.terrain.is_empty() {
        v.push("terrain bonuses".into());
    }
    v.join(" · ")
}

/// A battalion's card: what it is and does.
fn card(ui: &mut egui::Ui, cx: &Cx, g: &G, u: &SubUnit) {
    ui.set_max_width(340.0);
    ui.label(RichText::new(name(cx, &u.id)).strong());
    let what = match u.section() {
        Some(Section::Regiments) => format!("{} battalion", group_label(&u.group)),
        Some(Section::Support) => "support company".into(),
        Some(Section::RegimentalSupport) => format!("regimental support (columns of {})", u.allowed_groups.iter().map(|g| group_label(g)).collect::<Vec<_>>().join(", ")),
        None => "army headquarters only".into(),
    };
    ui.label(RichText::new(format!("{what} · {}{}{}", u.id, if u.special_forces { " · special forces" } else { "" }, if u.content { " · the mod's" } else { "" })).small().weak());
    if !g.start.units.contains(&u.id) {
        let techs = g.db.unlocked_text(&[u.id.as_str()], &|i| name(cx, i), 4);
        ui.colored_label(WARN, if techs.is_empty() { format!("{} cannot use it at the start", g.tag) } else { format!("{} cannot use it at the start: it comes with {techs}", g.tag) });
    }
    let b = g.db.battalion(u, g.research);
    egui::Grid::new(("card", &u.id)).num_columns(4).spacing([10.0, 1.0]).show(ui, |ui| {
        let rows: Vec<(&str, String)> = [
            ("Combat width", num(b.get("combat_width"))),
            ("HP", num(b.get("max_strength"))),
            ("Organisation", num(b.get("max_organisation"))),
            ("Speed", format!("{} km/h", num(b.get("maximum_speed")))),
            ("Soft attack", num(b.get("soft_attack"))),
            ("Hard attack", num(b.get("hard_attack"))),
            ("Air attack", num(b.get("air_attack"))),
            ("Defence", num(b.get("defense"))),
            ("Breakthrough", num(b.get("breakthrough"))),
            ("Piercing", num(b.get("ap_attack"))),
            ("Armour", num(b.get("armor_value"))),
            ("Hardness", pct(b.get("hardness"))),
            ("Recon", num(b.get("recon"))),
            ("Entrenchment", num(b.get("entrenchment"))),
            ("Supply use", num(b.get("supply_consumption"))),
            ("Manpower", num(b.get("manpower"))),
            ("Training", format!("{} days", num(b.get("training_time")))),
        ]
        .into_iter()
        .filter(|(l, v)| matches!(*l, "Combat width" | "HP" | "Organisation" | "Speed" | "Manpower") || (v != "0" && v != "0%" && v != "0 days"))
        .collect();
        for (i, (l, v)) in rows.iter().enumerate() {
            ui.label(RichText::new(*l).small());
            ui.label(RichText::new(v).small().strong());
            if i % 2 == 1 {
                ui.end_row();
            }
        }
    });
    for (e, model, n) in &b.equipment {
        let m = match model {
            Some(m) => name(cx, m),
            None => "none at this research".into(),
        };
        ui.label(RichText::new(format!("{} × {} ({m})", num(*n), name(cx, e))).small().color(if model.is_some() { ui.visuals().text_color() } else { WARN }));
    }
    for (k, t) in &b.terrain {
        ui.label(RichText::new(format!("{}: {}", loc::prettify(k), terrain_text(t))).small().weak());
    }
}

fn terrain_text(t: &units::Terrain) -> String {
    let mut v = Vec::new();
    for (l, x) in [("attack", t.attack), ("defence", t.defence), ("movement", t.movement)] {
        if x.abs() > 1e-9 {
            v.push(format!("{}{} {l}", if x > 0.0 { "+" } else { "" }, pct(x)));
        }
    }
    v.join(", ")
}

// ------------------------------------------------------------------ stats

/// A framed box of stats with a title.
fn stat_box(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new(title).strong());
            add(ui);
        });
    });
}

/// One stat: its label, value, and what it is.
fn row(ui: &mut egui::Ui, label: &str, value: String, help: &str) {
    ui.label(label).on_hover_text(help);
    ui.label(RichText::new(value).strong()).on_hover_text(help);
    ui.end_row();
}

fn stats(ui: &mut egui::Ui, cx: &Cx, g: &G, d: &Division) {
    if d.battalions + d.companies == 0 {
        ui.label(RichText::new("No battalions yet.").weak());
        return;
    }
    let v = |k: &str| d.get(k);
    ui.with_layout(egui::Layout::left_to_right(egui::Align::Min).with_main_wrap(true), |ui| {
        stat_box(ui, "Base", |ui| {
            egui::Grid::new("div_base").num_columns(2).spacing([12.0, 2.0]).show(ui, |ui| {
                row(ui, "Speed", format!("{} km/h", num(v("maximum_speed"))), "The speed of the slowest battalion or company (some, like support artillery, do not slow it).");
                row(ui, "HP", num(v("max_strength")), "Sum of all: how much damage it takes before it is destroyed.");
                row(ui, "Organisation", num(v("max_organisation")), "Average of all battalions and companies (those with 0 lower it): how long it fights before it retreats.");
                row(ui, "Recovery rate", num(v("default_morale")), "Average of all: how fast organisation comes back.");
                row(ui, "Recon", num(v("recon")), "Sum of all: better tactics picked in battle.");
                row(ui, "Suppression", num(v("suppression")), "Sum of all: how well it keeps occupied land quiet.");
                row(ui, "Weight", num(v("weight")), "Sum of all: convoys to ship it.");
                row(ui, "Supply use", format!("{} /day", num(v("supply_consumption"))), "Sum of all.");
                row(ui, "Entrenchment", num(v("entrenchment")), "Sum of all: defence bonus when it stays in place.");
                if v("initiative") > 0.0 {
                    row(ui, "Initiative", pct(v("initiative")), "Sum of all: how fast it reinforces battles and plans.");
                }
                if v("casualty_trickleback") > 0.0 {
                    row(ui, "Trickleback", pct(v("casualty_trickleback")), "Sum of all: share of losses that come back as manpower.");
                }
                if v("experience_loss_factor").abs() > 0.0 {
                    row(ui, "Experience loss", pct(v("experience_loss_factor")), "Sum of all.");
                }
                if v("equipment_capture_factor") > 0.0 {
                    row(ui, "Equipment capture", pct(v("equipment_capture_factor")), "Sum of all: share of the enemy's lost equipment taken.");
                }
            });
        });
        stat_box(ui, "Combat", |ui| {
            egui::Grid::new("div_combat").num_columns(2).spacing([12.0, 2.0]).show(ui, |ui| {
                row(ui, "Combat width", num(v("combat_width")), "Sum of the battalions (support companies take none): how much of a battle's front it fills.");
                row(ui, "Soft attack", num(v("soft_attack")), "Sum of all: attacks on unarmoured targets (by the target's softness).");
                row(ui, "Hard attack", num(v("hard_attack")), "Sum of all: attacks on armoured targets (by the target's hardness).");
                row(ui, "Air attack", num(v("air_attack")), "Sum of all: damage to planes over the battle.");
                row(ui, "Defence", num(v("defense")), "Sum of all: attacks it can avoid while defending.");
                row(ui, "Breakthrough", num(v("breakthrough")), "Sum of all: attacks it can avoid while attacking.");
                row(ui, "Armour", num(v("armor_value")), "40% of the highest plus 60% of the average: when above the enemy's piercing, it takes less damage and deals more.");
                row(ui, "Piercing", num(v("ap_attack")), "40% of the highest plus 60% of the average: when below the enemy's armour, its attacks do less.");
                row(ui, "Hardness", pct(v("hardness")), "Average of the battalions (not companies): the share of enemy attacks that are hard (weaker) rather than soft.");
            });
        });
        stat_box(ui, "Cost", |ui| {
            egui::Grid::new("div_cost").num_columns(2).spacing([12.0, 2.0]).show(ui, |ui| {
                row(ui, "Manpower", num(v("manpower")), "Sum of all.");
                row(ui, "Training", format!("{} days", num(v("training_time"))), "The longest of its battalions and companies.");
                row(ui, "Production", format!("{} IC", num(v("build_cost_ic"))), "Production cost of all its equipment (the models it gets).");
            });
            ui.add_space(4.0);
            for (arch, (model, n)) in &d.equipment {
                let stock: f64 = g.start.stock.iter().filter(|(m, _)| g.db.archetype_of(m) == arch || *m == arch).map(|(_, k)| k).sum();
                let more = if stock > 0.0 { format!(" · {} in the stockpile", num(stock)) } else { String::new() };
                let text = match model {
                    Some(m) => format!("{} × {}{more}", num(*n), name(cx, m)),
                    None => format!("{} × {}: none{more}", num(*n), name(cx, arch)),
                };
                let tip = if model.is_some() {
                    format!("The newest model of it with this research or in the stockpile{}", if stock > 0.0 && *n > 0.0 { format!(": the stockpile at the start fills {} divisions of this template", num((stock / n).floor())) } else { String::new() })
                } else {
                    format!("{} has no model of it with this research: the division has none of it", g.tag)
                };
                let c = if model.is_some() { ui.visuals().text_color() } else { WARN };
                ui.label(RichText::new(text).small().color(c)).on_hover_text(tip);
            }
        });
    });
    // how it fits fronts
    let w = v("combat_width");
    if w > 0.0 && !g.db.widths.is_empty() {
        egui::CollapsingHeader::new("Fronts it fits").id_salt("div_fronts").default_open(false).show(ui, |ui| {
            ui.label(RichText::new("How many divisions of it a battle takes in each terrain, and how much of the width they fill (attacked from one side; each further side adds the second width).").small().weak());
            egui::Grid::new("div_widths").striped(true).num_columns(4).show(ui, |ui| {
                ui.label(RichText::new("terrain").small().strong());
                ui.label(RichText::new("width").small().strong());
                ui.label(RichText::new("one side").small().strong());
                ui.label(RichText::new("two sides").small().strong());
                ui.end_row();
                for (t, tw, sw) in &g.db.widths {
                    ui.label(loc::prettify(t));
                    ui.label(format!("{} +{}", num(*tw), num(*sw)));
                    for front in [*tw, tw + sw] {
                        let n = (front / w).floor();
                        let fill = n * w / front;
                        ui.label(RichText::new(format!("{} ({})", num(n), pct(fill))).color(if fill >= 0.9 { OK } else { ui.visuals().text_color() }));
                    }
                    ui.end_row();
                }
            });
        });
    }
    if !d.terrain.is_empty() {
        egui::CollapsingHeader::new("Terrain").id_salt("div_terrain").default_open(false).show(ui, |ui| {
            ui.label(RichText::new("Bonuses of its battalions averaged, its support companies' added.").small().weak());
            for (k, t) in &d.terrain {
                ui.label(format!("{}: {}", loc::prettify(k), terrain_text(t)));
            }
        });
    }
    ui.label(
        RichText::new("Worked out from the game's files the way its designer does; doctrines, national spirits, generals, experience and the nation's own tank designs are left out.")
            .small()
            .weak(),
    );
}

// ------------------------------------------------------------------ wizard

/// A list of the battalions (or support companies) a nation can use at the
/// start, for the wizard: why `value` cannot be used, if it cannot.
pub fn unit_combo(ui: &mut egui::Ui, cx: &mut Cx, tag: &str, section: Section, value: &mut String) -> Option<String> {
    let Some(base) = cx.base else {
        ui.add(egui::TextEdit::singleline(value).desired_width(160.0));
        return None;
    };
    let db = units::db(Some(base), cx.st);
    let o = owned(cx, &db, tag);
    let have = db.at_start(cx.st, tag, Some(&o));
    let mut list: Vec<(String, String)> = db.subunits.values().filter(|u| u.section() == Some(section) && have.units.contains(&u.id)).map(|u| (u.id.clone(), name(cx, &u.id))).collect();
    list.sort_by(|a, b| a.1.cmp(&b.1));
    let shown = list.iter().find(|x| x.0 == *value).map(|x| x.1.clone()).unwrap_or_else(|| value.clone());
    egui::ComboBox::from_id_salt("wz_unit").selected_text(shown).height(360.0).show_ui(ui, |ui| {
        for (id, n) in &list {
            ui.selectable_value(value, id.clone(), n).on_hover_text(id);
        }
    });
    let who = if tag == SHARED { "every nation" } else { tag };
    match db.subunits.get(value.as_str()) {
        None => Some(format!("{value} is not a battalion of the game or the mod")),
        Some(u) if u.section() != Some(section) => Some(format!("{} does not go in {}", name(cx, &u.id), section.key())),
        Some(u) if !have.units.contains(&u.id) => Some(format!("{who} cannot use {} at the start", name(cx, &u.id))),
        _ => None,
    }
}

/// `n` regimental support companies of `unit` on the columns of the
/// template that have battalions it supports and no such company yet (in
/// `have`, the field's text as it will be).
pub fn place_regimental(cx: &mut Cx, key: &DefKey, have: &str, unit: &str, n: usize) -> Result<String, String> {
    let db = units::db(cx.base, cx.st);
    let t = cx.st.def(key).and_then(|(d, _, s)| d.block(s).map(Template::read)).unwrap_or_default();
    let taken: Vec<i64> = crate::content::wizard::slots(have).iter().map(|s| s.1).collect();
    let allowed = db.subunits.get(unit).map(|u| u.allowed_groups.clone()).unwrap_or_default();
    let cols: Vec<i64> = (0..5).filter(|x| !taken.contains(x) && t.column_group(&db, *x).is_some_and(|g| allowed.is_empty() || allowed.iter().any(|a| a == g))).take(n).collect();
    if cols.len() < n {
        return Err(format!("only {} columns of battalions it supports are free (one company each)", cols.len()));
    }
    Ok(cols.iter().map(|x| place(unit, *x, 0)).collect::<Vec<_>>().join("\n"))
}

// ------------------------------------------------------------------ edits

/// The changes asked for, into the template (one undo step each).
fn apply(cx: &mut Cx, key: &DefKey, acts: Vec<Act>) {
    for a in acts {
        let label = match &a {
            Act::Put(s, _, _, u) => format!("designer: {} {u}", s.one()),
            Act::Remove(s, ..) => format!("designer: remove a {}", s.one()),
            Act::Fill(x, u) => format!("designer: fill column {} with {u}", x + 1),
            Act::Clear(x) => format!("designer: empty column {}", x + 1),
        };
        cx.edit_def(key, &label, None, |text, b, _| edits(text, b, &a));
    }
}

fn place(unit: &str, x: i64, y: i64) -> String {
    format!("{unit} = {{ x = {x} y = {y} }}")
}

/// The edits of a change: items replaced, added or removed, so the rest
/// of the template (comments too) stays.
fn edits(text: &str, b: &script::Block, a: &Act) -> Vec<script::Edit> {
    let t = Template::read(b);
    let item_at = |section: Section, x: i64, y: i64| -> Option<&script::Item> {
        let s = t.at(section, x, y)?;
        b.blocks(section.key()).flat_map(|blk| blk.items.iter()).find(|i| i.span == s.span)
    };
    let block_of = |section: Section| b.block(section.key());
    match a {
        Act::Put(section, x, y, unit) => {
            if let Some(it) = item_at(*section, *x, *y) {
                return vec![script::replace_item(text, it, &place(unit, *x, *y))];
            }
            match block_of(*section) {
                Some(blk) => vec![script::insert_item(text, blk, &place(unit, *x, *y), None)],
                None => vec![script::insert_item(text, b, &format!("{} = {{\n\t{}\n}}", section.key(), place(unit, *x, *y)), None)],
            }
        }
        Act::Remove(section, x, y) => {
            let Some(it) = item_at(*section, *x, *y) else { return Vec::new() };
            // a support block left empty goes too
            if *section != Section::Regiments
                && let Some(whole) = b.all(section.key()).find(|i| i.block().is_some_and(|blk| blk.items.len() == 1 && blk.items[0].span == it.span))
            {
                return vec![script::remove_item(text, whole)];
            }
            vec![script::remove_item(text, it)]
        }
        Act::Fill(x, unit) => {
            let free: Vec<i64> = (0..5).filter(|y| t.at(Section::Regiments, *x, *y).is_none()).collect();
            if free.is_empty() {
                return Vec::new();
            }
            let lines = free.iter().map(|y| place(unit, *x, *y)).collect::<Vec<_>>().join("\n");
            match block_of(Section::Regiments) {
                Some(blk) => vec![script::insert_item(text, blk, &lines, None)],
                None => vec![script::insert_item(text, b, &format!("regiments = {{\n\t{}\n}}", lines.replace('\n', "\n\t")), None)],
            }
        }
        Act::Clear(x) => {
            let mut out = Vec::new();
            for section in [Section::Regiments, Section::RegimentalSupport] {
                for s in t.of(section).filter(|s| s.x == *x) {
                    if let Some(it) = item_at(section, s.x, s.y) {
                        out.push(script::remove_item(text, it));
                    }
                }
            }
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::script::Script;

    fn run(text: &str, a: Act) -> String {
        let s = Script::parse(text);
        let item = &s.root.items[0];
        let b = item.block().unwrap();
        script::apply(text, &edits(text, b, &a))
    }

    const T: &str = "division_template = {\n\tname = \"Inf\"\n\tregiments = {\n\t\tinfantry = { x = 0 y = 0 } # the first\n\t\tinfantry = { x = 0 y = 1 }\n\t}\n}\n";

    #[test]
    fn the_designer_edits_only_what_changes() {
        // put in a free place, in the block
        assert_eq!(
            run(T, Act::Put(Section::Regiments, 1, 0, "artillery_brigade".into())),
            "division_template = {\n\tname = \"Inf\"\n\tregiments = {\n\t\tinfantry = { x = 0 y = 0 } # the first\n\t\tinfantry = { x = 0 y = 1 }\n\t\tartillery_brigade = { x = 1 y = 0 }\n\t}\n}\n"
        );
        // replace keeps the comment
        assert_eq!(run(T, Act::Put(Section::Regiments, 0, 0, "mountaineers".into())), T.replace("infantry = { x = 0 y = 0 }", "mountaineers = { x = 0 y = 0 }"));
        // a support block is made when there is none, and goes when emptied
        let with = run(T, Act::Put(Section::Support, 0, 0, "engineer".into()));
        assert!(with.contains("\tsupport = {\n\t\tengineer = { x = 0 y = 0 }\n\t}\n}"), "{with}");
        assert_eq!(run(&with, Act::Remove(Section::Support, 0, 0)), T);
        // remove one; fill the column; empty it
        assert_eq!(run(T, Act::Remove(Section::Regiments, 0, 1)), T.replace("\t\tinfantry = { x = 0 y = 1 }\n", ""));
        let full = run(T, Act::Fill(0, "infantry".into()));
        assert_eq!(Template::read(Script::parse(&full).root.items[0].block().unwrap()).slots.len(), 5, "{full}");
        let empty = run(&full, Act::Clear(0));
        assert_eq!(Template::read(Script::parse(&empty).root.items[0].block().unwrap()).slots.len(), 0, "{empty}");
    }
}
