//! The navy page of a nation (Man the Guns): the ship designer, as the
//! game's (a hull, a module in each slot, picked from what the nation has
//! researched at the start, with the stats of the design and its checks),
//! and the fleets it starts with: fleets stationed at the naval bases of
//! the map, of task forces of ships of its designs. Each change is one
//! undo step; see [`crate::content::navy`] for the rules.

use std::collections::{BTreeMap, BTreeSet};

use eframe::egui::{self, Color32, RichText, Sense, Stroke, Vec2};

use super::{Cx, ERR, OK, WARN};
use crate::content::loc;
use crate::content::navy::{self, Design, EMPTY, Fleet, FleetProblem, Hull, Port, Problem};
use crate::content::script::{self, Block, Edit, Item, Script, Span};
use crate::content::store::SHARED;
use crate::content::units::{self, Have, Module, UnitDb};
use crate::content::validate::Severity;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Tab {
    #[default]
    Designs,
    Fleets,
}

#[derive(Default)]
pub struct NavyState {
    pub tab: Tab,
    /// The design shown: (name, hull).
    sel: Option<(String, String)>,
    new_hull: String,
    new_name: String,
    /// The pickers list modules the nation cannot use as well.
    show_locked: bool,
    /// Open the picker of this slot once shown (for screenshots).
    start_slot: Option<String>,
    fleet_name: String,
    fleet_base: Option<u32>,
    /// Per task force (its start in the file): the design to add, how many.
    add: BTreeMap<usize, (Option<(String, String)>, String)>,
    /// Per fleet: the name and place of a new task force.
    new_tf: BTreeMap<usize, (String, Option<u32>)>,
}

impl NavyState {
    /// From the start spec: `designs`, `fleets`, or a design's name; the
    /// popup `slot:<slot>` opens the picker of that slot.
    pub fn open_at(&mut self, what: Option<&str>, popup: Option<&str>) {
        match what {
            Some("fleets") => self.tab = Tab::Fleets,
            Some("designs") | None => self.tab = Tab::Designs,
            Some(name) => {
                self.tab = Tab::Designs;
                self.sel = Some((name.replace('_', " "), String::new()));
            }
        }
        if let Some(s) = popup.and_then(|p| p.strip_prefix("slot:")) {
            self.start_slot = Some(s.to_string());
        }
    }
}

/// A change asked for.
enum Act {
    NewDesign { name: String, hull: String, modules: Vec<(String, String)> },
    Module { d: Design, slot: String, module: String },
    Hull { d: Design, hull: String, modules: Vec<(String, String)> },
    Rename { d: Design, name: String },
    Obsolete { d: Design, on: bool },
    Duplicate { d: Design, name: String },
    Delete { d: Design },
    GiveTechs(Vec<String>),
    MakeOob,
    /// In the order of battle: (package, path).
    Fleet(String, String, FleetAct),
}

enum FleetAct {
    AddFleet { name: String, base: u32 },
    FleetName(Span, String),
    FleetBase(Span, u32),
    RemoveFleet(Span),
    AddTaskForce { fleet: Span, name: String, location: u32 },
    TfName(Span, String),
    TfLocation(Span, u32),
    RemoveTf(Span),
    AddShips { tf: Span, ships: Vec<String> },
    ShipName(Span, String),
    ShipKind(Span, String),
    ShipDesign { ship: Span, hull: String, version: String, kind: Option<String> },
    Pride(Span, bool),
    RemoveShip(Span),
    MoveShip { ship: Span, to: Span },
}

/// What the parts of the page look at.
struct G<'a> {
    db: &'a UnitDb,
    have: &'a Have,
    tag: &'a str,
    designs: &'a [Design],
    ports: &'a [Port],
}

impl G<'_> {
    fn kit(&self) -> Kit<'_> {
        Kit { db: self.db, have: self.have, tag: self.tag }
    }
}

/// What the designer's slots and pickers look at (the ship and the plane
/// designers'): the units, what the nation has at the start, its tag.
pub(super) struct Kit<'a> {
    pub db: &'a UnitDb,
    pub have: &'a Have,
    pub tag: &'a str,
}

pub(super) fn name(cx: &Cx, id: &str) -> String {
    cx.shown_text(id).map(|t| loc::plain(&t)).filter(|t| !t.trim().is_empty()).unwrap_or_else(|| loc::prettify(id))
}

/// A module as the designer names it: "Light Battery I" (its short name).
pub(super) fn module_name(cx: &Cx, id: &str) -> String {
    if id == EMPTY {
        return "empty".into();
    }
    cx.shown_text(&format!("{id}_short")).map(|t| loc::plain(&t)).filter(|t| !t.trim().is_empty()).unwrap_or_else(|| name(cx, id))
}

pub(super) fn slot_label(cx: &Cx, slot: &str) -> String {
    cx.shown_text(&format!("EQ_MOD_SLOT_{slot}_TITLE")).map(|t| loc::plain(&t)).filter(|t| !t.trim().is_empty()).unwrap_or_else(|| navy::slot_name(slot))
}

pub(super) fn colour(s: Severity) -> Color32 {
    match s {
        Severity::Error => ERR,
        Severity::Warning => WARN,
        Severity::Info => Color32::GRAY,
    }
}

pub(super) fn num(v: f64) -> String {
    if (v - v.round()).abs() < 0.005 { format!("{}", v.round() as i64) } else if v.abs() >= 10.0 { format!("{v:.1}") } else { format!("{v:.2}") }
}

pub(super) fn image(ui: &mut egui::Ui, rect: egui::Rect, tex: (egui::TextureId, Vec2), tint: Color32) {
    let (t, size) = tex;
    if size.x <= 0.0 || size.y <= 0.0 {
        return;
    }
    let k = (rect.width() / size.x).min(rect.height() / size.y);
    ui.painter().image(t, egui::Rect::from_center_size(rect.center(), size * k), egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), tint);
}

/// The picture of a hull: that of a technology that unlocks it.
pub(super) fn hull_picture(cx: &mut Cx, ctx: &egui::Context, db: &UnitDb, hull: &str) -> Option<(egui::TextureId, Vec2)> {
    let mut names: Vec<String> = db.unlocked_by(hull).iter().map(|t| format!("GFX_{}_medium", t.id)).collect();
    names.push(format!("GFX_{hull}_medium"));
    names.push(format!("GFX_archetype_{}_medium", db.archetype_of(hull)));
    names.into_iter().find_map(|n| cx.texture(ctx, &n).filter(|(_, s)| s.x > 0.0 && s.y > 0.0))
}

/// The history file of the nation designs and the naval order of battle
/// go to: its first, else a new one.
pub(super) fn history_file(cx: &Cx, tag: &str) -> String {
    cx.st.package(tag).and_then(|p| p.files.keys().find(|f| f.starts_with("history/countries/")).cloned()).unwrap_or_else(|| format!("history/countries/{tag}.txt"))
}

pub fn page(ui: &mut egui::Ui, cx: &mut Cx, ns: &mut NavyState) {
    ui.heading(format!("⚓ Navy of {}", cx.nation.map(|n| format!("{} {}", cx.pkg, n.name)).unwrap_or_else(|| cx.pkg.to_string())));
    if cx.pkg == SHARED {
        ui.label("The navy is a nation's: pick a nation on the left.");
        return;
    }
    let Some(base) = cx.base else {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Reading the game's ships for the designer…");
        });
        return;
    };
    let db = units::db(Some(base), cx.st);
    let tag = cx.pkg.to_string();
    let owned = super::division::owned(cx, &db, &tag);
    let have = db.at_start(cx.st, &tag, Some(&owned));
    let designs = navy::designs(&db, cx.st, &tag);
    let ports: Vec<Port> = cx.facts.ports.iter().map(|p| Port { province: p.province, level: p.level, name: p.name.clone(), tag: p.tag.clone() }).collect();
    let g = G { db: &db, have: &have, tag: &tag, designs: &designs, ports: &ports };
    let oob = navy::naval_oob(cx.st, &tag, &owned);
    let fleets: Vec<Fleet> = oob.as_ref().and_then(|(_, f)| f.as_ref()).and_then(|(p, path)| cx.st.file(p, path)).and_then(|f| f.script.as_ref()).map(|s| navy::read_fleets(&s.root)).unwrap_or_default();
    let ships: usize = fleets.iter().flat_map(|f| &f.task_forces).map(|t| t.ships.len()).sum();
    ui.horizontal(|ui| {
        ui.selectable_value(&mut ns.tab, Tab::Designs, RichText::new(format!("Ship designer ({} designs)", designs.len())).strong());
        ui.selectable_value(&mut ns.tab, Tab::Fleets, RichText::new(format!("Fleets ({} fleets, {ships} ships)", fleets.len())).strong());
    });
    ui.separator();
    let mut acts: Vec<Act> = Vec::new();
    egui::ScrollArea::vertical().id_salt("navy_page").auto_shrink([false, false]).show(ui, |ui| match ns.tab {
        Tab::Designs => designer(ui, cx, &g, ns, &mut acts),
        Tab::Fleets => fleets_tab(ui, cx, &g, ns, oob.as_ref(), &fleets, &mut acts),
    });
    apply(cx, &g, ns, acts);
}

// ------------------------------------------------------------- designer

fn designer(ui: &mut egui::Ui, cx: &mut Cx, g: &G, ns: &mut NavyState, acts: &mut Vec<Act>) {
    ui.label(
        RichText::new(format!(
            "Designs are create_equipment_variant lines of {}'s history: a hull and a module in each slot, as in the game's ship designer. The pickers list what {} has researched at the start (its history's set_technology); the fleets name a design in version_name.",
            g.tag, g.tag
        ))
        .weak()
        .small(),
    );
    ui.add_space(4.0);
    // the design shown: picked, else the first
    if let Some((n, h)) = &ns.sel
        && h.is_empty()
        && let Some(d) = g.designs.iter().find(|d| d.name == *n)
    {
        ns.sel = Some((d.name.clone(), d.hull.clone()));
    }
    if ns.sel.as_ref().is_none_or(|(n, h)| !g.designs.iter().any(|d| d.name == *n && d.hull == *h)) {
        ns.sel = g.designs.first().map(|d| (d.name.clone(), d.hull.clone()));
    }
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.set_width(250.0);
            design_list(ui, cx, g, ns);
            ui.add_space(8.0);
            new_design(ui, cx, g, ns, acts);
        });
        ui.separator();
        ui.vertical(|ui| {
            let d = ns.sel.as_ref().and_then(|(n, h)| g.designs.iter().find(|d| d.name == *n && d.hull == *h)).cloned();
            match d {
                Some(d) => design_view(ui, cx, g, ns, &d, acts),
                None => {
                    ui.label(RichText::new(format!("{} has no ship designs yet: make one on the left.", g.tag)).weak());
                }
            }
        });
    });
}

pub(super) fn worst(p: &[Problem]) -> Option<Severity> {
    p.iter().map(|p| p.severity).filter(|s| *s != Severity::Info).min()
}

fn design_list(ui: &mut egui::Ui, cx: &mut Cx, g: &G, ns: &mut NavyState) {
    ui.label(RichText::new("Designs").strong());
    if g.designs.is_empty() {
        ui.label(RichText::new("none yet").weak());
    }
    let mut order: Vec<&Design> = g.designs.iter().collect();
    let fam = |d: &Design| {
        let a = g.db.archetype_of(&d.hull);
        navy::ARCHETYPES.iter().position(|x| *x == a).unwrap_or(navy::ARCHETYPES.len())
    };
    order.sort_by_key(|d| fam(d));
    let mut last = usize::MAX;
    for d in order {
        let f = fam(d);
        if f != last {
            last = f;
            ui.label(RichText::new(name(cx, g.db.archetype_of(&d.hull))).small().weak());
        }
        let probs = g.db.check_design(d, g.have, g.designs, g.tag, &|i| name(cx, i));
        let role = g.db.hull(&d.hull).and_then(|h| g.db.role(&h, &g.db.fitted(&h, &d.modules)).map(|u| name(cx, &u.id)));
        let on = ns.sel.as_ref() == Some(&(d.name.clone(), d.hull.clone()));
        ui.horizontal(|ui| {
            let mark = match worst(&probs) {
                Some(s) => RichText::new(if s == Severity::Error { "✖" } else { "⚠" }).color(colour(s)),
                None => RichText::new("✔").color(OK),
            };
            ui.label(mark);
            let title = if d.name.is_empty() { "(no name)".to_string() } else { d.name.clone() };
            let t = if d.obsolete { RichText::new(title).weak() } else { RichText::new(title) };
            let r = ui.selectable_label(on, t).on_hover_text(format!("{} · {}{}", name(cx, &d.hull), role.clone().unwrap_or_else(|| "no kind of ship".into()), if d.obsolete { " · obsolete" } else { "" }));
            if r.clicked() {
                ns.sel = Some((d.name.clone(), d.hull.clone()));
            }
            ui.label(RichText::new(role.unwrap_or_default()).small().weak());
        });
    }
}

/// A combo of hulls; those the nation cannot build are greyed out with why.
fn hull_combo(ui: &mut egui::Ui, cx: &mut Cx, g: &G, id: &str, value: &mut String) -> bool {
    let shown = if value.is_empty() { "a hull…".to_string() } else { name(cx, value) };
    let mut changed = false;
    egui::ComboBox::from_id_salt(id).selected_text(shown).width(220.0).show_ui(ui, |ui| {
        let mut last = String::new();
        for h in g.db.hulls() {
            let arch = h.archetype.clone().unwrap_or_default();
            if arch != last {
                ui.label(RichText::new(name(cx, &arch)).small().weak());
                last = arch;
            }
            let ok = g.have.equipment.contains(&h.id);
            let label = format!("{} ({})", name(cx, &h.id), h.year as i64);
            let r = ui.add_enabled(ok, egui::Button::selectable(*value == h.id, label));
            let r = if ok {
                r
            } else {
                let techs = g.db.unlocked_text(&[h.id.as_str()], &|i| name(cx, i), 2);
                r.on_disabled_hover_text(if techs.is_empty() { format!("{} cannot build it", g.tag) } else { format!("comes with {techs}") })
            };
            if r.clicked() {
                *value = h.id.clone();
                changed = true;
                ui.close();
            }
        }
    });
    changed
}

fn new_design(ui: &mut egui::Ui, cx: &mut Cx, g: &G, ns: &mut NavyState, acts: &mut Vec<Act>) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.label(RichText::new("New design").strong());
        hull_combo(ui, cx, g, "navy_new_hull", &mut ns.new_hull);
        ui.add(egui::TextEdit::singleline(&mut ns.new_name).hint_text("name, e.g. Etsch Class").desired_width(220.0));
        let taken = g.designs.iter().any(|d| d.name == ns.new_name.trim() && d.hull == ns.new_hull);
        let ok = !ns.new_hull.is_empty() && !ns.new_name.trim().is_empty() && !taken;
        ui.horizontal(|ui| {
            if ui.add_enabled(ok, egui::Button::new("➕ Make it")).on_hover_text("With the hull's default modules, and the best the nation has where a slot must be filled").clicked()
                && let Some(h) = g.db.hull(&ns.new_hull)
            {
                let modules = g.db.new_modules(&h, g.have, &BTreeMap::new());
                acts.push(Act::NewDesign { name: ns.new_name.trim().to_string(), hull: ns.new_hull.clone(), modules });
                ns.sel = Some((ns.new_name.trim().to_string(), ns.new_hull.clone()));
                ns.new_name.clear();
            }
            if taken {
                ui.label(RichText::new("taken").color(WARN).small());
            }
        });
    });
}

fn design_view(ui: &mut egui::Ui, cx: &mut Cx, g: &G, ns: &mut NavyState, d: &Design, acts: &mut Vec<Act>) {
    let Some(hull) = g.db.hull(&d.hull) else {
        ui.colored_label(ERR, format!("{} is not a hull of the game or the mod", d.hull));
        if ui.button("✖ Delete the design").clicked() {
            acts.push(Act::Delete { d: d.clone() });
        }
        return;
    };
    let fitted = g.db.fitted(&hull, &d.modules);
    let problems = g.db.check_design(d, g.have, g.designs, g.tag, &|i| name(cx, i));
    let stats = g.db.ship_stats(&hull, &fitted);
    let role = g.db.role(&hull, &fitted).map(|u| u.id.clone());
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(150.0, 64.0), Sense::hover());
        if let Some(t) = hull_picture(cx, ui.ctx(), g.db, &d.hull) {
            image(ui, rect, t, Color32::WHITE);
        }
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.label("Name");
                let id = egui::Id::unique(("navy_name", &d.file, d.span.start));
                if let Some(v) = commit_text(ui, id, &d.name, 220.0, "the design's name") {
                    let v = v.trim().to_string();
                    if !v.is_empty() {
                        ns.sel = Some((v.clone(), d.hull.clone()));
                        acts.push(Act::Rename { d: d.clone(), name: v });
                    }
                }
                let mut obs = d.obsolete;
                if ui.checkbox(&mut obs, "obsolete").on_hover_text("obsolete = yes: the nation no longer builds it (its ships stay)").changed() {
                    acts.push(Act::Obsolete { d: d.clone(), on: obs });
                }
            });
            ui.horizontal(|ui| {
                ui.label("Hull");
                let mut h = d.hull.clone();
                if hull_combo(ui, cx, g, "navy_hull", &mut h)
                    && h != d.hull
                    && let Some(nh) = g.db.hull(&h)
                {
                    let modules = g.db.new_modules(&nh, g.have, &fitted);
                    ns.sel = Some((d.name.clone(), h.clone()));
                    acts.push(Act::Hull { d: d.clone(), hull: h, modules });
                }
                let what = match &role {
                    Some(r) => RichText::new(format!("→ {}", name(cx, r))).strong(),
                    None => RichText::new("→ no kind of ship").color(WARN),
                };
                ui.label(what).on_hover_text("What the game makes of the design: the kind of ship of its hull's family; a cruiser with a medium battery is a heavy cruiser, a heavy hull a battleship or a battlecruiser by its armour.");
            });
            ui.horizontal(|ui| {
                if ui.button("⧉ Duplicate").clicked() {
                    let taken: BTreeSet<String> = g.designs.iter().map(|d| d.name.clone()).collect();
                    let mut n = 2;
                    let mut new = format!("{} {n}", d.name);
                    while taken.contains(&new) {
                        n += 1;
                        new = format!("{} {n}", d.name);
                    }
                    ns.sel = Some((new.clone(), d.hull.clone()));
                    acts.push(Act::Duplicate { d: d.clone(), name: new });
                }
                if ui.button("✖ Delete").on_hover_text("Delete this design (ships of it in the fleets then have no design)").clicked() {
                    acts.push(Act::Delete { d: d.clone() });
                }
                if !d.need.is_empty() {
                    ui.label(RichText::new(format!("only for players {}", d.need.describe())).small().weak());
                }
                ui.label(RichText::new(format!("{}:{}", d.file, line_of(cx, &d.pkg, &d.file, d.span.start))).small().weak());
            });
        });
    });
    ui.add_space(6.0);
    // the slots
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
        for s in &hull.slots {
            if let Some(m) = slot_card(ui, cx, &g.kit(), &mut ns.show_locked, &mut ns.start_slot, &hull, &fitted, s, &problems, &brief) {
                acts.push(Act::Module { d: d.clone(), slot: s.key.clone(), module: m });
            }
        }
    });
    ui.add_space(4.0);
    if problems.is_empty() {
        ui.colored_label(OK, "✔ No problems: the game takes this design as it is.");
    }
    for p in &problems {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(p.severity.label()).color(colour(p.severity)).strong());
            ui.label(&p.message);
        });
    }
    // missing research: the technologies to give
    let mut locked: Vec<String> = Vec::new();
    if !g.have.equipment.contains(&d.hull) {
        locked.push(d.hull.clone());
    }
    for (slot, m) in &fitted {
        if m != EMPTY && hull.slot(slot).is_some() && g.db.modules.contains_key(m) && !g.have.modules.contains(m) {
            locked.push(m.clone());
        }
    }
    if !locked.is_empty() {
        let owned = super::division::owned(cx, g.db, g.tag);
        let techs = g.db.techs_to_give(&locked, g.have, &owned);
        if !techs.is_empty() {
            let names: Vec<String> = techs.iter().map(|t| name(cx, t)).collect();
            if ui.button(format!("Give {} the technologies ({})", g.tag, techs.len())).on_hover_text(format!("Adds to the history's set_technology: {}", names.join(", "))).clicked() {
                acts.push(Act::GiveTechs(techs));
            }
        }
    }
    ui.add_space(6.0);
    stats_view(ui, cx, &hull, &stats);
}

pub(super) fn line_of(cx: &Cx, pkg: &str, file: &str, at: usize) -> usize {
    cx.st.text(pkg, file).map(|t| script::line_of(t, at)).unwrap_or(0)
}

const CARD: Vec2 = Vec2::new(118.0, 84.0);

/// A slot of a design as a card: the module's picture and name, its
/// problems; a click opens the picker. Hands back the module picked.
#[allow(clippy::too_many_arguments)]
pub(super) fn slot_card(ui: &mut egui::Ui, cx: &mut Cx, g: &Kit, show_locked: &mut bool, start_slot: &mut Option<String>, hull: &Hull, fitted: &BTreeMap<String, String>, s: &navy::Slot, problems: &[Problem], brief: &dyn Fn(&Module) -> String) -> Option<String> {
    let m = fitted.get(&s.key).cloned().unwrap_or_else(|| EMPTY.to_string());
    let (rect, resp) = ui.allocate_exact_size(CARD, Sense::click());
    let vis = ui.visuals().clone();
    let mine: Vec<&Problem> = problems.iter().filter(|p| p.slot.as_deref() == Some(s.key.as_str())).collect();
    let stroke = match worst(&mine.iter().map(|p| (*p).clone()).collect::<Vec<_>>()) {
        Some(sev) => Stroke::new(2.0, colour(sev)),
        None if resp.hovered() => vis.widgets.hovered.bg_stroke,
        None => vis.widgets.noninteractive.bg_stroke,
    };
    let fill = if m == EMPTY { vis.extreme_bg_color } else { vis.widgets.inactive.weak_bg_fill };
    ui.painter().rect(rect, 4.0, fill, stroke, egui::StrokeKind::Inside);
    let small = egui::FontId::proportional(10.5);
    let title = format!("{}{}", slot_label(cx, &s.key), if s.required { " *" } else { "" });
    ui.painter().text(egui::pos2(rect.left() + 6.0, rect.top() + 4.0), egui::Align2::LEFT_TOP, title, small.clone(), vis.weak_text_color());
    let icon_rect = egui::Rect::from_center_size(egui::pos2(rect.center().x, rect.top() + 40.0), Vec2::new(rect.width() - 16.0, 36.0));
    if m == EMPTY {
        ui.painter().text(icon_rect.center(), egui::Align2::CENTER_CENTER, "+", egui::FontId::proportional(20.0), vis.weak_text_color());
    } else if let Some(md) = g.db.modules.get(&m)
        && let Some(t) = module_texture(cx, ui.ctx(), md, &m)
    {
        image(ui, icon_rect, t, Color32::WHITE);
    }
    let mut label = if m == EMPTY { String::new() } else { module_name(cx, &m) };
    if label.chars().count() > 19 {
        label = format!("{}…", label.chars().take(18).collect::<String>());
    }
    ui.painter().text(egui::pos2(rect.center().x, rect.bottom() - 10.0), egui::Align2::CENTER_CENTER, label, egui::FontId::proportional(11.0), if g.db.modules.contains_key(&m) || m == EMPTY { vis.text_color() } else { ERR });
    let resp = resp.on_hover_ui(|ui| {
        ui.label(RichText::new(slot_label(cx, &s.key)).strong());
        ui.label(RichText::new(format!("{} · takes {}", if s.required { "must have a module" } else { "may stay empty" }, s.categories.iter().map(|c| category_label(cx, c)).collect::<Vec<_>>().join(", "))).small());
        if m != EMPTY {
            module_card(ui, cx, g, &m);
        }
        for p in &mine {
            ui.colored_label(colour(p.severity), &p.message);
        }
    });
    if start_slot.as_deref() == Some(s.key.as_str()) {
        *start_slot = None;
        egui::Popup::open_id(ui.ctx(), egui::Popup::default_response_id(&resp));
    }
    let mut out = None;
    egui::Popup::from_toggle_button_response(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        ui.set_min_width(380.0);
        out = picker(ui, cx, g, show_locked, hull, fitted, s, &m, brief);
    });
    out
}

/// The picture of a module in the designer: a ship's `GFX_SMI_<id>`, a
/// plane's `GFX_EMI_<id>`.
fn module_texture(cx: &mut Cx, ctx: &egui::Context, m: &Module, id: &str) -> Option<(egui::TextureId, Vec2)> {
    let icon = m.icon(id);
    let plane = icon.replacen("GFX_SMI_", "GFX_EMI_", 1);
    [icon, plane].into_iter().find_map(|n| cx.texture(ctx, &n).filter(|(_, s)| s.x > 0.0 && s.y > 0.0))
}

pub(super) fn category_label(cx: &Cx, c: &str) -> String {
    cx.shown_text(&format!("EQ_MOD_CAT_{c}_TITLE")).map(|t| loc::plain(&t)).filter(|t| !t.trim().is_empty()).unwrap_or_else(|| navy::category_name(c))
}

/// What a module brings, in a few words.
fn brief(m: &Module) -> String {
    const SHORT: [(&str, &str); 14] = [
        ("lg_attack", "light attack"),
        ("lg_armor_piercing", "light piercing"),
        ("hg_attack", "heavy attack"),
        ("hg_armor_piercing", "heavy piercing"),
        ("torpedo_attack", "torpedo"),
        ("sub_attack", "depth charges"),
        ("anti_air_attack", "anti-air"),
        ("armor_value", "armour"),
        ("surface_detection", "surface detection"),
        ("sub_detection", "sub detection"),
        ("carrier_size", "deck"),
        ("max_strength", "HP"),
        ("naval_speed", "speed"),
        ("build_cost_ic", "cost"),
    ];
    let mut v = Vec::new();
    for (k, label) in SHORT {
        if let Some(x) = m.add.get(k).filter(|x| **x != 0.0) {
            v.push(format!("{}{} {label}", if *x > 0.0 { "+" } else { "" }, num(*x)));
        }
        if let Some(x) = m.average.get(k).filter(|x| **x != 0.0) {
            v.push(format!("{} {label}", num(*x)));
        }
        if let Some(x) = m.multiply.get(k).filter(|x| **x != 0.0 && (k == "naval_speed" || k == "max_strength")) {
            v.push(format!("{}{}% {label}", if *x > 0.0 { "+" } else { "" }, num(x * 100.0)));
        }
    }
    v.truncate(4);
    v.join(" · ")
}

pub(super) fn module_card(ui: &mut egui::Ui, cx: &Cx, g: &Kit, id: &str) {
    let Some(m) = g.db.modules.get(id) else { return };
    ui.label(RichText::new(format!("{} ({})", name(cx, id), module_name(cx, id))).strong());
    ui.label(RichText::new(format!("{} · {id}", category_label(cx, &m.category))).small().weak());
    let line = |ui: &mut egui::Ui, what: &str, s: &units::Stats, pct: bool| {
        let v: Vec<String> = s.iter().filter(|(_, x)| **x != 0.0).map(|(k, x)| if pct { format!("{k} {}{}%", if *x > 0.0 { "+" } else { "" }, num(x * 100.0)) } else { format!("{k} {}", num(*x)) }).collect();
        if !v.is_empty() {
            ui.label(RichText::new(format!("{what}: {}", v.join(", "))).small());
        }
    };
    line(ui, "adds", &m.add, false);
    line(ui, "average", &m.average, false);
    line(ui, "times", &m.multiply, true);
    if !m.forbid.is_empty() {
        ui.label(RichText::new(format!("not with {}", m.forbid.iter().map(|c| category_label(cx, c)).collect::<Vec<_>>().join(", "))).small());
    }
    if !g.have.modules.contains(id) {
        let techs = g.db.unlocked_text(&[id], &|i| name(cx, i), 2);
        ui.colored_label(WARN, if techs.is_empty() { format!("{} cannot use it", g.tag) } else { format!("comes with {techs}") });
    }
}

#[allow(clippy::too_many_arguments)]
fn picker(ui: &mut egui::Ui, cx: &mut Cx, g: &Kit, show_locked: &mut bool, hull: &Hull, fitted: &BTreeMap<String, String>, s: &navy::Slot, current: &str, brief: &dyn Fn(&Module) -> String) -> Option<String> {
    let mut out = None;
    ui.horizontal(|ui| {
        ui.label(RichText::new(slot_label(cx, &s.key)).strong());
        ui.checkbox(show_locked, "all").on_hover_text(format!("Also list modules {} has not researched, and those another module of the design rules out (greyed out, with why)", g.tag));
    });
    // the categories other modules of the design rule out, and those this
    // slot's would rule out of others
    let others: Vec<(&String, &Module)> = fitted.iter().filter(|(k, m)| *k != &s.key && *m != EMPTY && hull.slot(k).is_some()).filter_map(|(k, m)| Some((k, g.db.modules.get(m)?))).collect();
    let mut shown = 0;
    egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
        if !s.required {
            let r = ui.add(egui::Button::selectable(current == EMPTY, "empty"));
            if r.clicked() {
                out = Some(EMPTY.to_string());
                ui.close();
            }
        }
        let mut last = String::new();
        for id in g.db.modules_for(s) {
            let Some(m) = g.db.modules.get(id) else { continue };
            let ruled = others.iter().find(|(_, o)| o.forbid.contains(&m.category) || m.forbid.contains(&o.category));
            let why: Option<String> = if !g.have.modules.contains(id) {
                let techs = g.db.unlocked_text(&[id], &|i| name(cx, i), 2);
                Some(if techs.is_empty() { format!("{} cannot use it", g.tag) } else { format!("needs {techs}") })
            } else {
                ruled.map(|(k, o)| format!("not with {} ({})", category_label(cx, &o.category), slot_label(cx, k)))
            };
            if why.is_some() && !*show_locked {
                continue;
            }
            shown += 1;
            if m.category != last {
                ui.label(RichText::new(category_label(cx, &m.category)).small().weak());
                last = m.category.clone();
            }
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(Vec2::new(36.0, 22.0), Sense::hover());
                if let Some(t) = module_texture(cx, ui.ctx(), m, id) {
                    image(ui, r, t, if why.is_some() { Color32::from_white_alpha(90) } else { Color32::WHITE });
                }
                let r = ui.add_enabled(why.is_none(), egui::Button::selectable(current == id, module_name(cx, id)));
                let r = r.on_hover_ui(|ui| module_card(ui, cx, g, id)).on_disabled_hover_ui(|ui| module_card(ui, cx, g, id));
                if r.clicked() {
                    out = Some(id.to_string());
                    ui.close();
                }
                match &why {
                    Some(w) => ui.label(RichText::new(w).small().color(WARN)),
                    None => ui.label(RichText::new(brief(m)).small().weak()),
                };
            });
        }
    });
    if shown == 0 {
        ui.label(RichText::new(if *show_locked { "No module of the game goes here." } else { "Nothing researched goes here (tick \"all\" to see why)." }).weak());
    }
    out.filter(|m| m != current)
}

pub(super) fn stat_box(ui: &mut egui::Ui, title: &str, rows: &[(&str, String, &str)]) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new(title).strong());
            egui::Grid::new(("navy_stats", title)).num_columns(2).spacing([12.0, 2.0]).show(ui, |ui| {
                for (label, v, help) in rows {
                    ui.label(*label).on_hover_text(*help);
                    ui.label(RichText::new(v).strong()).on_hover_text(*help);
                    ui.end_row();
                }
            });
        });
    });
}

fn stats_view(ui: &mut egui::Ui, cx: &Cx, hull: &Hull, s: &navy::ShipStats) {
    ui.label(RichText::new("Stats").strong()).on_hover_text("The hull's, plus each module's, plus the average of the guns' piercing, times the modules' multipliers. Left out: refits and upgrades, design companies, doctrines and the technologies' bonuses.");
    let v = |k: &str| num(s.get(k));
    ui.horizontal_wrapped(|ui| {
        stat_box(
            ui,
            "Attack",
            &[
                ("Light attack", v("lg_attack"), "Damage of light guns per hit (screens and light cruisers' main guns)"),
                ("Light piercing", v("lg_armor_piercing"), "Light guns hurt fully a ship with less armour than this"),
                ("Heavy attack", v("hg_attack"), "Damage of heavy guns (capital ships' main guns)"),
                ("Heavy piercing", v("hg_armor_piercing"), "Heavy guns hurt fully a ship with less armour than this"),
                ("Torpedo attack", v("torpedo_attack"), "Damage of torpedoes"),
                ("Depth charges", v("sub_attack"), "Attack against submarines"),
                ("Anti-air", v("anti_air_attack"), "Attack against planes"),
            ],
        );
        stat_box(
            ui,
            "Defence",
            &[
                ("HP", v("max_strength"), "Structure: damage the ship takes before it sinks"),
                ("Armour", v("armor_value"), "Guns with less piercing do less damage"),
                ("Reliability", format!("{}%", num(s.get("reliability") * 100.0)), "Chance to survive critical hits and keep going"),
                ("Surface visibility", v("surface_visibility"), "How easily the ship is seen by surface ships"),
                ("Sub visibility", v("sub_visibility"), "How easily a submarine is seen"),
            ],
        );
        let mut mv = vec![
            ("Speed", format!("{} kn", v("naval_speed")), "Top speed in knots"),
            ("Range", format!("{} km", v("naval_range")), "How far it goes from its base"),
            ("Surface detection", v("surface_detection"), "Spotting of surface ships"),
            ("Sub detection", v("sub_detection"), "Spotting of submarines"),
            ("Fuel use", v("fuel_consumption"), "Fuel per hour at sea"),
        ];
        if s.get("carrier_size") > 0.0 || hull.types.iter().any(|t| t == "carrier") {
            mv.push(("Deck size", v("carrier_size"), "Planes the carrier takes"));
        }
        stat_box(ui, "Movement and detection", &mv);
        let res: Vec<String> = s.resources.iter().map(|(k, n)| format!("{} {}", num(*n), name(cx, k))).collect();
        stat_box(
            ui,
            "Production",
            &[
                ("Cost", format!("{} IC", v("build_cost_ic")), "Industrial capacity a dockyard spends on one"),
                ("Manpower", v("manpower"), "Crew"),
                ("Resources", if res.is_empty() { "none".into() } else { res.join(", ") }, "Resources a dockyard needs to build it"),
                ("Type", s.types.iter().map(|t| name(cx, t)).collect::<Vec<_>>().join(", "), "screen, capital ship, submarine or carrier in battle"),
            ],
        );
    });
}

// ------------------------------------------------------------- fleets

/// A text field that hands back its text when it loses focus changed.
pub(super) fn commit_text(ui: &mut egui::Ui, id: egui::Id, current: &str, width: f32, hint: &str) -> Option<String> {
    let mut buf: String = ui.data_mut(|d| d.get_temp::<String>(id)).unwrap_or_else(|| current.to_string());
    let r = ui.add(egui::TextEdit::singleline(&mut buf).id(id).desired_width(width).hint_text(hint));
    if r.lost_focus() {
        ui.data_mut(|d| d.remove::<String>(id));
        return (buf != current).then_some(buf);
    }
    if r.has_focus() {
        ui.data_mut(|d| d.insert_temp(id, buf));
    } else {
        ui.data_mut(|d| d.remove::<String>(id));
    }
    None
}

fn port_label(p: &Port, tag: &str) -> String {
    let owner = if p.tag == tag || p.tag.is_empty() { String::new() } else { format!(" · {}", p.tag) };
    let n = if p.name.is_empty() { format!("province {}", p.province) } else { format!("{} ({})", p.name, p.province) };
    format!("{n} · level {}{owner}", p.level)
}

/// A combo of the naval bases of the map: the nation's first.
fn port_combo(ui: &mut egui::Ui, g: &G, id: String, current: Option<u32>) -> Option<u32> {
    let shown = match current {
        None => RichText::new("a naval base…"),
        Some(p) => match g.ports.iter().find(|x| x.province == p) {
            Some(x) => RichText::new(port_label(x, g.tag)),
            None => RichText::new(format!("province {p}: no naval base")).color(ERR),
        },
    };
    let mut out = None;
    egui::ComboBox::from_id_salt(id).selected_text(shown).width(230.0).show_ui(ui, |ui| {
        if g.ports.is_empty() {
            ui.label(RichText::new(format!("The map has no naval bases: {}.", navy::PLACE_PORT)).weak());
        }
        let mut list: Vec<&Port> = g.ports.iter().collect();
        list.sort_by_key(|p| (p.tag != g.tag, p.tag.clone(), p.name.clone()));
        for p in list {
            let t = if p.tag == g.tag { RichText::new(port_label(p, g.tag)) } else { RichText::new(port_label(p, g.tag)).weak() };
            let r = ui.add(egui::Button::selectable(current == Some(p.province), t));
            let r = if p.tag == g.tag { r } else { r.on_hover_text(format!("A naval base of {}: a fleet of {} needs docking rights there", if p.tag.is_empty() { "no nation" } else { &p.tag }, g.tag)) };
            if r.clicked() {
                out = Some(p.province);
                ui.close();
            }
        }
    });
    out.filter(|p| Some(*p) != current)
}

fn design_label(cx: &Cx, d: &Design) -> String {
    format!("{} · {}", d.name, name(cx, &d.hull))
}

#[allow(clippy::too_many_arguments)]
fn fleets_tab(ui: &mut egui::Ui, cx: &mut Cx, g: &G, ns: &mut NavyState, oob: Option<&(navy::OobLine, Option<(String, String)>)>, fleets: &[Fleet], acts: &mut Vec<Act>) {
    ui.label(
        RichText::new(format!(
            "The fleets {} starts with: each stationed at a naval base, of task forces of ships of its designs. They are in a history/units file its history loads with set_naval_oob.",
            g.tag
        ))
        .weak()
        .small(),
    );
    // the naval bases
    let own: Vec<&Port> = g.ports.iter().filter(|p| p.tag == g.tag).collect();
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Naval bases").strong());
        if own.is_empty() {
            let why = if g.ports.is_empty() { "The map has none yet" } else { "None is in a state of this nation" };
            ui.colored_label(WARN, format!("{why}: {}. Fleets can only be stationed at a naval base.", navy::PLACE_PORT));
        } else {
            ui.label(own.iter().map(|p| port_label(p, g.tag)).collect::<Vec<_>>().join("  ·  "));
        }
    });
    ui.add_space(4.0);
    let Some((line, file)) = oob else {
        ui.label(format!("{} has no naval order of battle yet (set_naval_oob in its history).", g.tag));
        if ui.button(format!("Make it: history/units/{}_naval.txt", g.tag)).on_hover_text("Makes the file and adds set_naval_oob to the nation's history (one undo step)").clicked() {
            acts.push(Act::MakeOob);
        }
        return;
    };
    let Some((pkg, path)) = file else {
        ui.colored_label(ERR, format!("set_naval_oob = {}: there is no history/units/{}.txt in the content", line.name, line.name));
        if ui.button(format!("Make history/units/{}.txt", line.name)).clicked() {
            acts.push(Act::MakeOob);
        }
        return;
    };
    ui.label(RichText::new(format!("{path}{}", if line.need.is_empty() { String::new() } else { format!(" (for players {})", line.need.describe()) })).small().weak());
    let problems = navy::check_fleets(g.db, fleets, g.tag, g.ports, &|t| if t == g.tag { g.designs.to_vec() } else { navy::designs(g.db, cx.st, t) }, &|i| name(cx, i));
    let by_span: BTreeMap<usize, Vec<&FleetProblem>> = problems.iter().fold(BTreeMap::new(), |mut m, p| {
        m.entry(p.span.start).or_default().push(p);
        m
    });
    let show = |ui: &mut egui::Ui, span: &Span| {
        for p in by_span.get(&span.start).into_iter().flatten() {
            if p.severity == Severity::Info {
                continue;
            }
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(p.severity.label()).color(colour(p.severity)).small().strong());
                ui.label(RichText::new(&p.message).small());
            });
        }
    };
    let tfs: Vec<(String, Span)> = fleets.iter().flat_map(|f| f.task_forces.iter().map(move |t| (format!("{} / {}", f.name, t.name), t.span.clone()))).collect();
    let mut fa: Vec<FleetAct> = Vec::new();
    if fleets.is_empty() {
        ui.label(RichText::new("No fleets yet.").weak());
    }
    for f in fleets {
        let ships: usize = f.task_forces.iter().map(|t| t.ships.len()).sum();
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("⚓ Fleet").strong());
                if let Some(v) = commit_text(ui, egui::Id::unique(("navy_fleet", f.span.start)), &f.name, 180.0, "name") {
                    fa.push(FleetAct::FleetName(f.span.clone(), v));
                }
                ui.label("at");
                if let Some(p) = port_combo(ui, g, format!("navy_fleet_base_{}", f.span.start), f.base) {
                    fa.push(FleetAct::FleetBase(f.span.clone(), p));
                }
                ui.label(RichText::new(format!("{} task forces, {ships} ships", f.task_forces.len())).small().weak());
                if ui.small_button("✖").on_hover_text("Remove this fleet with its ships").clicked() {
                    fa.push(FleetAct::RemoveFleet(f.span.clone()));
                }
            });
            show(ui, &f.span);
            for t in &f.task_forces {
                ui.indent(("navy_tf", t.span.start), |ui| {
                    task_force(ui, cx, g, ns, t, &tfs, &show, &mut fa);
                });
            }
            ui.indent(("navy_new_tf", f.span.start), |ui| {
                ui.horizontal(|ui| {
                    let e = ns.new_tf.entry(f.span.start).or_insert_with(|| (String::new(), f.base));
                    ui.label("New task force");
                    ui.add(egui::TextEdit::singleline(&mut e.0).hint_text("name").desired_width(140.0));
                    ui.label("at");
                    if let Some(p) = port_combo(ui, g, format!("navy_tf_new_loc_{}", f.span.start), e.1) {
                        e.1 = Some(p);
                    }
                    let ok = !e.0.trim().is_empty() && e.1.is_some();
                    if ui.add_enabled(ok, egui::Button::new("➕ Add")).clicked()
                        && let Some(l) = e.1
                    {
                        fa.push(FleetAct::AddTaskForce { fleet: f.span.clone(), name: e.0.trim().to_string(), location: l });
                        e.0.clear();
                    }
                });
            });
        });
        ui.add_space(4.0);
    }
    // a new fleet
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("New fleet").strong());
        ui.add(egui::TextEdit::singleline(&mut ns.fleet_name).hint_text("name, e.g. Gardasee-Flottille").desired_width(180.0));
        ui.label("stationed at");
        if ns.fleet_base.is_none() {
            ns.fleet_base = own.first().map(|p| p.province);
        }
        if let Some(p) = port_combo(ui, g, "navy_new_fleet_base".into(), ns.fleet_base) {
            ns.fleet_base = Some(p);
        }
        let ok = !ns.fleet_name.trim().is_empty() && ns.fleet_base.is_some();
        if ui.add_enabled(ok, egui::Button::new("➕ Add")).on_hover_text("A fleet with one task force of the same name at its base").clicked()
            && let Some(b) = ns.fleet_base
        {
            fa.push(FleetAct::AddFleet { name: ns.fleet_name.trim().to_string(), base: b });
            ns.fleet_name.clear();
        }
    });
    // what the checks say about the file as a whole
    let unplaced: Vec<&FleetProblem> = problems.iter().filter(|p| p.severity != Severity::Info && !fleets.iter().any(|f| f.span.start == p.span.start || f.task_forces.iter().any(|t| t.span.start == p.span.start || t.ships.iter().any(|s| s.span.start == p.span.start)))).collect();
    for p in unplaced {
        ui.colored_label(colour(p.severity), &p.message);
    }
    let (e, w) = (problems.iter().filter(|p| p.severity == Severity::Error).count(), problems.iter().filter(|p| p.severity == Severity::Warning).count());
    if !fleets.is_empty() {
        if e + w == 0 {
            ui.colored_label(OK, "✔ No problems in the fleets.");
        } else {
            ui.colored_label(if e > 0 { ERR } else { WARN }, format!("{e} errors, {w} warnings in the fleets (under each line)"));
        }
    }
    for a in fa {
        acts.push(Act::Fleet(pkg.clone(), path.clone(), a));
    }
}

#[allow(clippy::too_many_arguments)]
fn task_force(ui: &mut egui::Ui, cx: &mut Cx, g: &G, ns: &mut NavyState, t: &navy::TaskForce, tfs: &[(String, Span)], show: &dyn Fn(&mut egui::Ui, &Span), fa: &mut Vec<FleetAct>) {
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Task force").strong());
        if let Some(v) = commit_text(ui, egui::Id::unique(("navy_tf_name", t.span.start)), &t.name, 160.0, "name") {
            fa.push(FleetAct::TfName(t.span.clone(), v));
        }
        ui.label("at");
        if let Some(p) = port_combo(ui, g, format!("navy_tf_loc_{}", t.span.start), t.location) {
            fa.push(FleetAct::TfLocation(t.span.clone(), p));
        }
        if ui.small_button("✖").on_hover_text("Remove this task force with its ships").clicked() {
            fa.push(FleetAct::RemoveTf(t.span.clone()));
        }
    });
    show(ui, &t.span);
    for sh in &t.ships {
        ui.horizontal(|ui| ship_row(ui, cx, g, sh, t, tfs, fa));
        show(ui, &sh.span);
    }
    // add ships of a design
    ui.horizontal_wrapped(|ui| {
        let e = ns.add.entry(t.span.start).or_insert_with(|| (g.designs.iter().find(|d| !d.obsolete).or(g.designs.first()).map(|d| (d.name.clone(), d.hull.clone())), "1".into()));
        ui.label("Add");
        ui.add(egui::TextEdit::singleline(&mut e.1).desired_width(28.0));
        ui.label("×");
        let shown = e.0.as_ref().and_then(|(n, h)| g.designs.iter().find(|d| d.name == *n && d.hull == *h)).map(|d| design_label(cx, d)).unwrap_or_else(|| "a design…".into());
        egui::ComboBox::from_id_salt(("navy_add_design", t.span.start)).selected_text(shown).width(240.0).show_ui(ui, |ui| {
            if g.designs.is_empty() {
                ui.label(RichText::new("No designs yet: make one in the ship designer.").weak());
            }
            for d in g.designs {
                if ui.add(egui::Button::selectable(e.0.as_ref() == Some(&(d.name.clone(), d.hull.clone())), design_label(cx, d))).clicked() {
                    e.0 = Some((d.name.clone(), d.hull.clone()));
                    ui.close();
                }
            }
        });
        let n: usize = e.1.trim().parse().unwrap_or(0);
        let d = e.0.as_ref().and_then(|(nm, h)| g.designs.iter().find(|d| d.name == *nm && d.hull == *h));
        if ui.add_enabled(d.is_some() && (1..=99).contains(&n), egui::Button::new("➕ Add ships")).clicked()
            && let Some(d) = d
        {
            let kind = g.db.hull(&d.hull).and_then(|h| g.db.role(&h, &g.db.fitted(&h, &d.modules)).map(|u| u.id.clone())).or_else(|| g.db.roles(&d.hull).first().map(|u| u.id.clone())).unwrap_or_default();
            let taken: BTreeSet<String> = all_ship_names(cx, g);
            let ships = navy::ship_names(&d.name, &taken, n).into_iter().map(|nm| navy::render_ship(&nm, &kind, &d.hull, g.tag, &d.name, false)).collect();
            fa.push(FleetAct::AddShips { tf: t.span.clone(), ships });
        }
    });
}

/// The names of the nation's ships (in its naval order of battle).
fn all_ship_names(cx: &Cx, g: &G) -> BTreeSet<String> {
    let owned = super::division::owned(cx, g.db, g.tag);
    let Some((_, Some((p, path)))) = navy::naval_oob(cx.st, g.tag, &owned) else { return BTreeSet::new() };
    let Some(s) = cx.st.file(&p, &path).and_then(|f| f.script.as_ref()) else { return BTreeSet::new() };
    navy::read_fleets(&s.root).iter().flat_map(|f| &f.task_forces).flat_map(|t| &t.ships).map(|s| s.name.clone()).collect()
}

fn ship_row(ui: &mut egui::Ui, cx: &mut Cx, g: &G, s: &navy::Ship, t: &navy::TaskForce, tfs: &[(String, Span)], fa: &mut Vec<FleetAct>) {
    if let Some(v) = commit_text(ui, egui::Id::unique(("navy_ship", s.span.start)), &s.name, 150.0, "name") {
        fa.push(FleetAct::ShipName(s.span.clone(), v));
    }
    // the design
    let cur = g.designs.iter().find(|d| Some(&d.name) == s.version.as_ref() && d.hull == s.hull);
    let shown = match (cur, &s.version) {
        (Some(d), _) => RichText::new(design_label(cx, d)),
        (None, Some(v)) => RichText::new(format!("{v} · {}", name(cx, &s.hull))).color(ERR),
        (None, None) => RichText::new(format!("no design · {}", name(cx, &s.hull))).color(WARN),
    };
    egui::ComboBox::from_id_salt(("navy_ship_design", s.span.start)).selected_text(shown).width(230.0).show_ui(ui, |ui| {
        for d in g.designs {
            if ui.add(egui::Button::selectable(cur.is_some_and(|c| c.name == d.name && c.hull == d.hull), design_label(cx, d))).clicked() {
                let fits = g.db.roles(&d.hull).iter().any(|r| r.id == s.definition);
                let kind = if fits { None } else { g.db.hull(&d.hull).and_then(|h| g.db.role(&h, &g.db.fitted(&h, &d.modules)).map(|u| u.id.clone())) };
                fa.push(FleetAct::ShipDesign { ship: s.span.clone(), hull: d.hull.clone(), version: d.name.clone(), kind });
                ui.close();
            }
        }
    });
    // the kind of ship
    let kinds = g.db.roles(&s.hull);
    let kind_text = if s.definition.is_empty() { RichText::new("no kind").color(ERR) } else { RichText::new(name(cx, &s.definition)) };
    egui::ComboBox::from_id_salt(("navy_ship_kind", s.span.start)).selected_text(kind_text).width(120.0).show_ui(ui, |ui| {
        for k in kinds {
            if ui.add(egui::Button::selectable(s.definition == k.id, name(cx, &k.id))).clicked() {
                fa.push(FleetAct::ShipKind(s.span.clone(), k.id.clone()));
                ui.close();
            }
        }
    });
    let mut pride = s.pride;
    if ui.checkbox(&mut pride, "pride").on_hover_text("pride_of_the_fleet: the nation's flagship (one per nation)").changed() {
        fa.push(FleetAct::Pride(s.span.clone(), pride));
    }
    ui.menu_button("move ▾", |ui| {
        for (label, span) in tfs {
            if span.start != t.span.start && ui.button(label).clicked() {
                fa.push(FleetAct::MoveShip { ship: s.span.clone(), to: span.clone() });
                ui.close();
            }
        }
    });
    if ui.small_button("✖").on_hover_text("Remove this ship").clicked() {
        fa.push(FleetAct::RemoveShip(s.span.clone()));
    }
}

// ------------------------------------------------------------- edits

/// The item at a span, anywhere in a block.
pub(super) fn item_at<'a>(b: &'a Block, span: &Span) -> Option<&'a Item> {
    for i in &b.items {
        if i.span == *span {
            return Some(i);
        }
        if let Some(found) = i.block().and_then(|c| item_at(c, span)) {
            return Some(found);
        }
    }
    None
}

/// The edits of a change to a design (its block in `text`).
fn design_edits(text: &str, root: &Block, a: &Act) -> Vec<Edit> {
    let item = |d: &Design| item_at(root, &d.span).filter(|i| i.is("create_equipment_variant"));
    match a {
        Act::NewDesign { name, hull, modules } => {
            let after = root.items.iter().rposition(|i| i.is("create_equipment_variant"));
            vec![script::insert_item(text, root, &navy::render_design(name, hull, modules, false), after)]
        }
        Act::Module { d, slot, module } => {
            let Some(b) = item(d).and_then(|i| i.block()) else { return Vec::new() };
            match b.block("modules") {
                Some(m) => vec![script::set_atom(text, m, slot, module)],
                None => vec![script::insert_item(text, b, &format!("modules = {{\n\t{slot} = {module}\n}}"), None)],
            }
        }
        Act::Hull { d, hull, modules } => {
            let Some(b) = item(d).and_then(|i| i.block()) else { return Vec::new() };
            let lines = modules.iter().map(|(k, v)| format!("{k} = {v}")).collect::<Vec<_>>().join("\n");
            let mut out = vec![script::set_atom(text, b, "type", hull)];
            match b.block("modules") {
                Some(m) => out.push(script::set_block_inner(text, m, &lines)),
                None => out.push(script::insert_item(text, b, &format!("modules = {{\n\t{}\n}}", lines.replace('\n', "\n\t")), None)),
            }
            out
        }
        Act::Rename { d, name } => {
            let Some(b) = item(d).and_then(|i| i.block()) else { return Vec::new() };
            vec![script::set_atom(text, b, "name", &script::quote(name))]
        }
        Act::Obsolete { d, on } => {
            let Some(b) = item(d).and_then(|i| i.block()) else { return Vec::new() };
            match (on, b.get("obsolete")) {
                (true, _) => vec![script::set_atom(text, b, "obsolete", "yes")],
                (false, Some(o)) => vec![script::remove_item(text, o)],
                (false, None) => Vec::new(),
            }
        }
        Act::Duplicate { d, name } => {
            let Some(i) = item(d) else { return Vec::new() };
            let src = script::item_source(text, i);
            let s = Script::parse(&src);
            let Some(b) = s.root.items.first().and_then(|x| x.block()) else { return Vec::new() };
            let copy = script::apply(&src, &[script::set_atom(&src, b, "name", &script::quote(name))]);
            // after the design, in the block it is in
            let (block, idx) = parent_of(root, &d.span).unwrap_or((root, None));
            vec![script::insert_item(text, block, &copy, idx)]
        }
        Act::Delete { d } => item(d).map(|i| vec![script::remove_item(text, i)]).unwrap_or_default(),
        Act::GiveTechs(techs) => {
            let lines: Vec<String> = techs.iter().map(|t| format!("{t} = 1")).collect();
            match root.block("set_technology") {
                Some(b) => {
                    let have: BTreeSet<&str> = b.items.iter().filter_map(|i| i.key.as_ref().map(|k| k.text.as_str())).collect();
                    let new: Vec<String> = techs.iter().filter(|t| !have.contains(t.as_str())).map(|t| format!("{t} = 1")).collect();
                    if new.is_empty() { Vec::new() } else { vec![script::insert_item(text, b, &new.join("\n"), None)] }
                }
                None => vec![script::insert_item(text, root, &format!("set_technology = {{\n\t{}\n}}", lines.join("\n\t")), None)],
            }
        }
        _ => Vec::new(),
    }
}

/// The block an item is in, and its index there.
pub(super) fn parent_of<'a>(b: &'a Block, span: &Span) -> Option<(&'a Block, Option<usize>)> {
    for (n, i) in b.items.iter().enumerate() {
        if i.span == *span {
            return Some((b, Some(n)));
        }
        if let Some(found) = i.block().and_then(|c| parent_of(c, span)) {
            return Some(found);
        }
    }
    None
}

/// The edits of a change to the order of battle.
fn fleet_edits(text: &str, root: &Block, tag: &str, a: &FleetAct) -> Vec<Edit> {
    let at = |span: &Span| item_at(root, span);
    let block_at = |span: &Span| at(span).and_then(|i| i.block());
    match a {
        FleetAct::AddFleet { name, base } => {
            let fleet = navy::render_fleet(name, *base, &navy::render_task_force(name, *base));
            match root.block("units") {
                Some(u) => vec![script::insert_item(text, u, &fleet, None)],
                None => vec![script::insert_item(text, root, &format!("units = {{\n\t{}\n}}", fleet.replace('\n', "\n\t")), None)],
            }
        }
        FleetAct::FleetName(span, n) | FleetAct::TfName(span, n) | FleetAct::ShipName(span, n) => block_at(span).map(|b| vec![script::set_atom(text, b, "name", &script::quote(n))]).unwrap_or_default(),
        FleetAct::FleetBase(span, p) => block_at(span).map(|b| vec![script::set_atom(text, b, "naval_base", &p.to_string())]).unwrap_or_default(),
        FleetAct::TfLocation(span, p) => block_at(span).map(|b| vec![script::set_atom(text, b, "location", &p.to_string())]).unwrap_or_default(),
        FleetAct::RemoveFleet(span) | FleetAct::RemoveTf(span) | FleetAct::RemoveShip(span) => at(span).map(|i| vec![script::remove_item(text, i)]).unwrap_or_default(),
        FleetAct::AddTaskForce { fleet, name, location } => block_at(fleet).map(|b| vec![script::insert_item(text, b, &navy::render_task_force(name, *location), None)]).unwrap_or_default(),
        FleetAct::AddShips { tf, ships } => block_at(tf).map(|b| vec![script::insert_item(text, b, &ships.join("\n"), None)]).unwrap_or_default(),
        FleetAct::ShipKind(span, k) => block_at(span).map(|b| vec![script::set_atom(text, b, "definition", k)]).unwrap_or_default(),
        FleetAct::ShipDesign { ship, hull, version, kind } => {
            let Some(b) = block_at(ship) else { return Vec::new() };
            let owner = b.block("equipment").and_then(|e| e.items.iter().find_map(|i| i.block()?.atom("owner"))).unwrap_or(tag).to_string();
            let eq = format!("{hull} = {{ amount = 1 owner = {owner} version_name = {} }}", script::quote(version));
            let mut out = match b.block("equipment") {
                Some(e) => vec![script::set_block_inner(text, e, &eq)],
                None => vec![script::insert_item(text, b, &format!("equipment = {{ {eq} }}"), None)],
            };
            if let Some(k) = kind {
                out.push(script::set_atom(text, b, "definition", k));
            }
            out
        }
        FleetAct::Pride(span, on) => {
            let Some(b) = block_at(span) else { return Vec::new() };
            match (on, b.get("pride_of_the_fleet")) {
                (true, _) => vec![script::set_atom(text, b, "pride_of_the_fleet", "yes")],
                (false, Some(p)) => vec![script::remove_item(text, p)],
                (false, None) => Vec::new(),
            }
        }
        FleetAct::MoveShip { ship, to } => {
            let (Some(s), Some(tb)) = (at(ship), block_at(to)) else { return Vec::new() };
            if to.start <= ship.start && ship.end <= to.end {
                return Vec::new();
            }
            let src = script::item_text(text, s).to_string();
            vec![script::remove_item(text, s), script::insert_item(text, tb, &src, None)]
        }
    }
}

/// Edit a file with edits worked out from its text (one undo step); a
/// missing file starts empty.
pub(super) fn edit_with(cx: &mut Cx, pkg: &str, path: &str, label: &str, merge: Option<String>, f: impl FnOnce(&str, &Block) -> Vec<Edit>) -> bool {
    cx.edit_file(pkg, path, label, merge, |text, s| {
        let parsed;
        let root = match s {
            Some(s) => &s.root,
            None => {
                parsed = Script::parse(text);
                &parsed.root
            }
        };
        let e = f(text, root);
        (!e.is_empty()).then(|| script::apply(text, &e))
    })
}

fn apply(cx: &mut Cx, g: &G, ns: &mut NavyState, acts: Vec<Act>) {
    let tag = g.tag.to_string();
    for a in acts {
        match &a {
            Act::Fleet(pkg, path, fa) => {
                let label = match fa {
                    FleetAct::AddFleet { name, .. } => format!("navy: new fleet {name}"),
                    FleetAct::AddTaskForce { name, .. } => format!("navy: new task force {name}"),
                    FleetAct::AddShips { ships, .. } => format!("navy: add {} ships", ships.len()),
                    FleetAct::RemoveFleet(_) => "navy: remove a fleet".into(),
                    FleetAct::RemoveTf(_) => "navy: remove a task force".into(),
                    FleetAct::RemoveShip(_) => "navy: remove a ship".into(),
                    FleetAct::MoveShip { .. } => "navy: move a ship".into(),
                    FleetAct::FleetBase(..) | FleetAct::TfLocation(..) => "navy: naval base".into(),
                    FleetAct::ShipDesign { version, .. } => format!("navy: ship of {version}"),
                    FleetAct::ShipKind(_, k) => format!("navy: ship is a {k}"),
                    FleetAct::Pride(..) => "navy: pride of the fleet".into(),
                    FleetAct::FleetName(..) | FleetAct::TfName(..) | FleetAct::ShipName(..) => "navy: rename".into(),
                };
                if let FleetAct::AddShips { tf, .. } = fa {
                    ns.add.remove(&tf.start);
                }
                edit_with(cx, pkg, path, &label, None, |text, root| fleet_edits(text, root, &tag, fa));
            }
            Act::MakeOob => make_oob(cx, &tag),
            Act::Rename { d, name } => {
                // the design, and the ships of it in the nation's fleets
                cx.st.begin(format!("navy: rename the design {} to {name}", d.name));
                let file = d.file.clone();
                cx.st.edit_text(&d.pkg, &file, |text| {
                    let s = Script::parse(text);
                    script::apply(text, &design_edits(text, &s.root, &a))
                });
                let owned = super::division::owned(cx, g.db, &tag);
                if let Some((_, Some((p, path)))) = navy::naval_oob(cx.st, &tag, &owned) {
                    cx.st.edit_text(&p, &path, |text| {
                        let s = Script::parse(text);
                        let mut e = Vec::new();
                        for f in navy::read_fleets(&s.root) {
                            for t in &f.task_forces {
                                for sh in &t.ships {
                                    if sh.hull == d.hull && sh.version.as_deref() == Some(d.name.as_str()) && sh.owner.as_deref().is_none_or(|o| o == tag)
                                        && let Some(eb) = item_at(&s.root, &sh.span).and_then(|i| i.block()).and_then(|b| b.block("equipment")).and_then(|e| e.items.iter().find_map(|i| i.block()))
                                    {
                                        e.push(script::set_atom(text, eb, "version_name", &script::quote(name)));
                                    }
                                }
                            }
                        }
                        script::apply(text, &e)
                    });
                }
                cx.st.commit();
            }
            _ => {
                let (file, pkg) = match &a {
                    Act::Module { d, .. } | Act::Hull { d, .. } | Act::Obsolete { d, .. } | Act::Duplicate { d, .. } | Act::Delete { d } => (d.file.clone(), d.pkg.clone()),
                    _ => (history_file(cx, &tag), tag.clone()),
                };
                let label = match &a {
                    Act::NewDesign { name, .. } => format!("navy: new design {name}"),
                    Act::Module { slot, module, .. } => format!("navy: {} = {module}", navy::slot_name(slot)),
                    Act::Hull { hull, .. } => format!("navy: hull {hull}"),
                    Act::Obsolete { d, .. } => format!("navy: {} obsolete", d.name),
                    Act::Duplicate { name, .. } => format!("navy: new design {name}"),
                    Act::Delete { d } => format!("navy: delete the design {}", d.name),
                    Act::GiveTechs(t) => format!("navy: give {} technologies", t.len()),
                    _ => "navy".into(),
                };
                if !edit_with(cx, &pkg, &file, &label, None, |text, root| design_edits(text, root, &a)) {
                    log::warn!("navy: nothing to change for {label}");
                }
            }
        }
    }
}

/// The naval order of battle of a nation made: its file, and the
/// set_naval_oob line in its history (one undo step). A set_naval_oob to
/// a missing file gets that file.
fn make_oob(cx: &mut Cx, tag: &str) {
    let lines = navy::naval_oobs(cx.st, tag);
    let name = lines.first().map(|l| l.name.clone()).unwrap_or_else(|| format!("{tag}_naval"));
    let path = format!("history/units/{name}.txt");
    cx.st.begin(format!("navy: naval order of battle {name}"));
    if cx.st.file(tag, &path).is_none() {
        cx.st.set_text(tag, &path, "units = {\n}\n".into());
    }
    if lines.is_empty() {
        let hist = history_file(cx, tag);
        cx.st.edit_text(tag, &hist, |text| {
            let s = Script::parse(text);
            let after = s.root.items.iter().rposition(|i| i.is("oob"));
            script::apply(text, &[script::insert_item(text, &s.root, &format!("set_naval_oob = {}", script::quote(&name)), after)])
        });
    }
    cx.st.commit();
    cx.info(format!("{tag} has a naval order of battle: {path}"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::dlc::Need;

    fn d(text: &str) -> Design {
        let s = Script::parse(text);
        let i = s.root.items.iter().find(|i| i.is("create_equipment_variant")).unwrap();
        let b = i.block().unwrap();
        Design {
            name: b.atom("name").unwrap().to_string(),
            hull: b.atom("type").unwrap().to_string(),
            modules: b.block("modules").map(|m| m.items.iter().map(|i| (i.key().to_string(), i.text().unwrap().to_string())).collect()).unwrap_or_default(),
            obsolete: b.yes("obsolete"),
            need: Need::default(),
            pkg: "TYR".into(),
            file: "h".into(),
            span: i.span.clone(),
        }
    }

    fn run(text: &str, a: &Act) -> String {
        let s = Script::parse(text);
        script::apply(text, &design_edits(text, &s.root, a))
    }

    const H: &str = "capital = 1\ncreate_equipment_variant = {\n\tname = \"Etsch Class\"\n\ttype = ship_hull_light_1\n\tmodules = {\n\t\tfixed_ship_battery_slot = ship_light_battery_1 # guns\n\t\tfixed_ship_engine_slot = light_ship_engine_1\n\t}\n}\n";

    #[test]
    fn design_edits_keep_the_rest() {
        let x = d(H);
        assert_eq!(run(H, &Act::Module { d: x.clone(), slot: "fixed_ship_battery_slot".into(), module: "ship_light_battery_2".into() }), H.replace("ship_light_battery_1 # guns", "ship_light_battery_2 # guns"));
        let t = run(H, &Act::Module { d: x.clone(), slot: "mid_1_custom_slot".into(), module: "ship_torpedo_1".into() });
        assert!(t.contains("\t\tfixed_ship_engine_slot = light_ship_engine_1\n\t\tmid_1_custom_slot = ship_torpedo_1\n\t}"), "{t}");
        assert_eq!(run(H, &Act::Rename { d: x.clone(), name: "Eisack Class".into() }), H.replace("Etsch Class", "Eisack Class"));
        let t = run(H, &Act::Obsolete { d: x.clone(), on: true });
        assert!(t.contains("\tobsolete = yes\n}"), "{t}");
        assert_eq!(run(&t, &Act::Obsolete { d: d(&t), on: false }), H);
        let t = run(H, &Act::Duplicate { d: x.clone(), name: "Etsch Class 2".into() });
        assert_eq!(t.matches("create_equipment_variant").count(), 2);
        assert!(t.contains("name = \"Etsch Class 2\""), "{t}");
        assert_eq!(run(H, &Act::Delete { d: x.clone() }), "capital = 1\n");
        let t = run(H, &Act::Hull { d: x.clone(), hull: "ship_hull_light_2".into(), modules: vec![("fixed_ship_battery_slot".into(), "ship_light_battery_1".into())] });
        assert!(t.contains("type = ship_hull_light_2") && !t.contains("light_ship_engine_1"), "{t}");
        let t = run(H, &Act::GiveTechs(vec!["early_ship_hull_light".into(), "basic_battery".into()]));
        assert!(t.contains("set_technology = {\n\tearly_ship_hull_light = 1\n\tbasic_battery = 1\n}"), "{t}");
        let t2 = Script::parse(&t);
        assert_eq!(script::apply(&t, &design_edits(&t, &t2.root, &Act::GiveTechs(vec!["basic_battery".into(), "torpedo".into()]))).matches(" = 1\n").count(), 4, "capital = 1 and three technologies");
        let t = run("capital = 1\n", &Act::NewDesign { name: "A".into(), hull: "ship_hull_light_1".into(), modules: vec![("fixed_ship_battery_slot".into(), "ship_light_battery_1".into())] });
        assert_eq!(t, "capital = 1\ncreate_equipment_variant = {\n\tname = \"A\"\n\ttype = ship_hull_light_1\n\tmodules = {\n\t\tfixed_ship_battery_slot = ship_light_battery_1\n\t}\n}\n");
    }

    fn frun(text: &str, a: &FleetAct) -> String {
        let s = Script::parse(text);
        script::apply(text, &fleet_edits(text, &s.root, "TYR", a))
    }

    #[test]
    fn fleet_edits_keep_the_rest() {
        let t = frun("", &FleetAct::AddFleet { name: "Gardasee".into(), base: 12 });
        assert_eq!(t, "units = {\n\tfleet = {\n\t\tname = \"Gardasee\"\n\t\tnaval_base = 12\n\t\ttask_force = {\n\t\t\tname = \"Gardasee\"\n\t\t\tlocation = 12\n\t\t}\n\t}\n}\n");
        let f = navy::read_fleets(&Script::parse(&t).root);
        let ship = navy::render_ship("Etsch 1", "destroyer", "ship_hull_light_1", "TYR", "Etsch Class", false);
        let t = frun(&t, &FleetAct::AddShips { tf: f[0].task_forces[0].span.clone(), ships: vec![ship.clone(), ship.replace("Etsch 1", "Etsch 2")] });
        let f = navy::read_fleets(&Script::parse(&t).root);
        assert_eq!(f[0].task_forces[0].ships.len(), 2, "{t}");
        let s0 = f[0].task_forces[0].ships[0].span.clone();
        let t2 = frun(&t, &FleetAct::ShipDesign { ship: s0.clone(), hull: "ship_hull_light_2".into(), version: "Eisack Class".into(), kind: Some("destroyer".into()) });
        let f2 = navy::read_fleets(&Script::parse(&t2).root);
        assert_eq!((f2[0].task_forces[0].ships[0].hull.as_str(), f2[0].task_forces[0].ships[0].version.as_deref(), f2[0].task_forces[0].ships[0].owner.as_deref()), ("ship_hull_light_2", Some("Eisack Class"), Some("TYR")));
        let t2 = frun(&t, &FleetAct::Pride(s0.clone(), true));
        assert!(navy::read_fleets(&Script::parse(&t2).root)[0].task_forces[0].ships[0].pride, "{t2}");
        let t2 = frun(&t, &FleetAct::RemoveShip(s0.clone()));
        assert_eq!(navy::read_fleets(&Script::parse(&t2).root)[0].task_forces[0].ships.len(), 1);
        // a second task force, a ship moved there
        let t3 = frun(&t, &FleetAct::AddTaskForce { fleet: f[0].span.clone(), name: "Second".into(), location: 30 });
        let f3 = navy::read_fleets(&Script::parse(&t3).root);
        let t4 = frun(&t3, &FleetAct::MoveShip { ship: f3[0].task_forces[0].ships[1].span.clone(), to: f3[0].task_forces[1].span.clone() });
        let f4 = navy::read_fleets(&Script::parse(&t4).root);
        assert_eq!((f4[0].task_forces[0].ships.len(), f4[0].task_forces[1].ships.len(), f4[0].task_forces[1].ships[0].name.as_str()), (1, 1, "Etsch 2"), "{t4}");
        let t5 = frun(&t4, &FleetAct::TfLocation(f4[0].task_forces[1].span.clone(), 12));
        assert_eq!(navy::read_fleets(&Script::parse(&t5).root)[0].task_forces[1].location, Some(12));
    }
}
