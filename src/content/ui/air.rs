//! The air page of a nation: the plane designer, as By Blood Alone's (an
//! airframe, a module in each slot, picked from what the nation has
//! researched at the start; the main armament makes the role; thrust and
//! weight, the stats and the checks), the fallback of each design for
//! players without that DLC (a plane of before the designer of the same
//! name, with its upgrades), and the air wings it starts with at the air
//! bases of the map, written for players with the DLC and kept in step for
//! those without it. Each change is one undo step; see
//! [`crate::content::air`] for the rules.

use std::collections::{BTreeMap, BTreeSet};

use eframe::egui::{self, Color32, RichText, Sense, Vec2};

use super::navy::{Kit, colour, commit_text, edit_with, history_file, hull_picture, image, item_at, line_of, name, num, parent_of, slot_card, stat_box, worst};
use super::{Cx, ERR, OK, WARN};
use crate::content::air::{self, Airfield, BBA, Branch, Fallback, WingBase, WingSpec};
use crate::content::dlc::Owned;
use crate::content::navy::{self, Design, EMPTY, FleetProblem, OobLine};
use crate::content::script::{self, Block, Edit, Script, Span};
use crate::content::store::SHARED;
use crate::content::units::{self, Have, Module, UnitDb};
use crate::content::validate::Severity;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Tab {
    #[default]
    Designs,
    Wings,
}

#[derive(Default)]
pub struct AirState {
    pub tab: Tab,
    /// The design shown: (name, airframe).
    sel: Option<(String, String)>,
    new_frame: String,
    new_role: String,
    new_name: String,
    show_locked: bool,
    /// Open the picker of this slot once shown (for screenshots).
    start_slot: Option<String>,
    /// The new wing: its design, how many planes, where, its name.
    wing_design: Option<(String, String)>,
    wing_amount: String,
    wing_base: Option<u32>,
    wing_name: String,
}

impl AirState {
    /// From the start spec: `designs`, `wings`, or a design's name; the
    /// popup `slot:<slot>` opens the picker of that slot.
    pub fn open_at(&mut self, what: Option<&str>, popup: Option<&str>) {
        match what {
            Some("wings") => self.tab = Tab::Wings,
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
    NewDesign { name: String, frame: String, modules: Vec<(String, String)> },
    /// A module; `frame`: the airframe of the role it makes, when another.
    Module { d: Design, slot: String, module: String, frame: Option<String> },
    Frame { d: Design, frame: String, modules: Vec<(String, String)> },
    Rename { d: Design, name: String },
    Obsolete { d: Design, on: bool },
    Duplicate { d: Design, name: String },
    Delete { d: Design },
    /// Technologies for players with By Blood Alone (`true`) or without.
    GiveTechs(Vec<String>, bool),
    NewFallback { name: String, model: String },
    FallbackModel { f: Fallback, model: String },
    Upgrade { f: Fallback, upgrade: String, level: i64 },
    DeleteFallback { f: Fallback },
    MakeOobs,
    /// On the wings of players with By Blood Alone: (package, path).
    Wings(String, String, WingAct),
    RebuildLegacy,
}

enum WingAct {
    Add { state: u32, spec: WingSpec },
    Amount(Span, f64),
    Name { wing: Span, name_span: Option<Span>, name: String },
    Design { wing: Span, equipment: String, version: String },
    Move { wing: Span, name_span: Option<Span>, to: u32 },
    Remove { wing: Span, name_span: Option<Span> },
}

/// What the parts of the page look at.
struct G<'a> {
    db: &'a UnitDb,
    /// What the nation has at the start with By Blood Alone, and without.
    have: &'a Have,
    legacy: &'a Have,
    owned: &'a Owned,
    tag: &'a str,
    designs: &'a [Design],
    fallbacks: &'a [Fallback],
    fields: &'a [Airfield],
}

impl G<'_> {
    fn kit(&self) -> Kit<'_> {
        Kit { db: self.db, have: self.have, tag: self.tag }
    }
    fn fallback_of(&self, d: &Design) -> Option<&Fallback> {
        self.fallbacks.iter().find(|f| f.name == d.name && air::branch_of(&f.need) != Branch::Bba)
    }
    fn role(&self, d: &Design) -> Option<String> {
        let h = self.db.hull(&d.hull)?;
        self.db.plane_role(&h, &self.db.fitted(&h, &d.modules))
    }
    fn planes_of(&self, cx: &Cx, t: &str) -> (Vec<Design>, Vec<Fallback>) {
        if t == self.tag { (self.designs.to_vec(), self.fallbacks.to_vec()) } else { air::planes(self.db, cx.st, t) }
    }
}

pub fn page(ui: &mut egui::Ui, cx: &mut Cx, st: &mut AirState) {
    ui.heading(format!("✈ Air force of {}", cx.nation.map(|n| format!("{} {}", cx.pkg, n.name)).unwrap_or_else(|| cx.pkg.to_string())));
    if cx.pkg == SHARED {
        ui.label("The air force is a nation's: pick a nation on the left.");
        return;
    }
    let Some(base) = cx.base else {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Reading the game's planes for the designer…");
        });
        return;
    };
    let db = units::db(Some(base), cx.st);
    let tag = cx.pkg.to_string();
    let owned = super::division::owned(cx, &db, &tag);
    let have = db.at_start(cx.st, &tag, Some(&air::with_bba(&owned)));
    let legacy = db.at_start(cx.st, &tag, Some(&air::without_bba(&owned)));
    let (designs, fallbacks) = air::planes(&db, cx.st, &tag);
    let fields = super::check::airfields(cx);
    let g = G { db: &db, have: &have, legacy: &legacy, owned: &owned, tag: &tag, designs: &designs, fallbacks: &fallbacks, fields: &fields };
    let oob = air::air_oob(cx.st, &tag, &air::with_bba(&owned));
    let bases: Vec<WingBase> = oob.as_ref().and_then(|(_, f)| f.as_ref()).and_then(|(p, path)| cx.st.file(p, path)).and_then(|f| f.script.as_ref()).map(|s| air::read_wings(&s.root)).unwrap_or_default();
    let wings: usize = bases.iter().map(|b| b.wings.len()).sum();
    let planes: f64 = bases.iter().flat_map(|b| &b.wings).filter_map(|w| w.amount).sum();
    ui.horizontal(|ui| {
        ui.selectable_value(&mut st.tab, Tab::Designs, RichText::new(format!("Plane designer ({} designs)", designs.len())).strong());
        ui.selectable_value(&mut st.tab, Tab::Wings, RichText::new(format!("Air wings ({wings} wings, {} planes)", num(planes))).strong());
    });
    ui.separator();
    let mut acts: Vec<Act> = Vec::new();
    egui::ScrollArea::vertical().id_salt("air_page").auto_shrink([false, false]).show(ui, |ui| match st.tab {
        Tab::Designs => designer(ui, cx, &g, st, &mut acts),
        Tab::Wings => wings_tab(ui, cx, &g, st, oob.as_ref(), &bases, &mut acts),
    });
    apply(cx, &g, acts);
}

// ------------------------------------------------------------- designer

fn designer(ui: &mut egui::Ui, cx: &mut Cx, g: &G, st: &mut AirState, acts: &mut Vec<Act>) {
    ui.label(
        RichText::new(format!(
            "Designs are create_equipment_variant lines of {}'s history, inside if = {{ limit = {{ has_dlc = \"{BBA}\" }} }}: an airframe and a module in each slot, as in the game's plane designer. The pickers list what {} has researched at the start with {BBA}; the main armament makes the role. Players without {BBA} have no plane designer: each design can have a fallback, a plane of before the designer of the same name.",
            g.tag, g.tag
        ))
        .weak()
        .small(),
    );
    ui.add_space(4.0);
    if let Some((n, h)) = &st.sel
        && h.is_empty()
        && let Some(d) = g.designs.iter().find(|d| d.name == *n)
    {
        st.sel = Some((d.name.clone(), d.hull.clone()));
    }
    if st.sel.as_ref().is_none_or(|(n, h)| !g.designs.iter().any(|d| d.name == *n && d.hull == *h)) {
        st.sel = g.designs.first().map(|d| (d.name.clone(), d.hull.clone()));
    }
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.set_width(260.0);
            design_list(ui, cx, g, st);
            ui.add_space(8.0);
            new_design(ui, cx, g, st, acts);
            orphans(ui, cx, g);
        });
        ui.separator();
        ui.vertical(|ui| {
            let d = st.sel.as_ref().and_then(|(n, h)| g.designs.iter().find(|d| d.name == *n && d.hull == *h)).cloned();
            match d {
                Some(d) => design_view(ui, cx, g, st, &d, acts),
                None => {
                    ui.label(RichText::new(format!("{} has no plane designs yet: make one on the left.", g.tag)).weak());
                }
            }
        });
    });
}

/// A role as the designer names it ("Tactical Bomber", not the
/// localisation's "Tactical Bomber models").
fn role_name(cx: &Cx, role: &str) -> String {
    let n = name(cx, role);
    n.strip_suffix(" models").map(String::from).unwrap_or(n)
}

/// A family of airframes ("Small airframes").
fn family_name(cx: &Cx, fam: &str) -> String {
    match fam {
        "small_plane_airframe" => "Small airframes".into(),
        "cv_small_plane_airframe" => "Carrier airframes".into(),
        "medium_plane_airframe" => "Medium airframes".into(),
        "large_plane_airframe" => "Large airframes".into(),
        _ => name(cx, fam),
    }
}

fn family_rank(g: &G, frame: &str) -> usize {
    let f = g.db.base_family(frame);
    air::FRAMES.iter().position(|x| *x == f).unwrap_or(air::FRAMES.len())
}

fn design_list(ui: &mut egui::Ui, cx: &mut Cx, g: &G, st: &mut AirState) {
    ui.label(RichText::new("Designs").strong());
    if g.designs.is_empty() {
        ui.label(RichText::new("none yet").weak());
    }
    let mut order: Vec<&Design> = g.designs.iter().collect();
    order.sort_by_key(|d| family_rank(g, &d.hull));
    let mut last = usize::MAX;
    for d in order {
        let f = family_rank(g, &d.hull);
        if f != last {
            last = f;
            ui.label(RichText::new(family_name(cx, &g.db.base_family(&d.hull))).small().weak());
        }
        let mut probs = g.db.check_plane(d, g.have, g.designs, g.tag, &|i| name(cx, i));
        if let Some(fb) = g.fallback_of(d) {
            probs.extend(g.db.check_fallback(fb, g.legacy, Some(d), g.fallbacks, g.tag, &|i| name(cx, i)));
        }
        let role = g.role(d).map(|r| role_name(cx, &r));
        let on = st.sel.as_ref() == Some(&(d.name.clone(), d.hull.clone()));
        ui.horizontal(|ui| {
            let mark = match worst(&probs) {
                Some(s) => RichText::new(if s == Severity::Error { "✖" } else { "⚠" }).color(colour(s)),
                None => RichText::new("✔").color(OK),
            };
            ui.label(mark);
            let title = if d.name.is_empty() { "(no name)".to_string() } else { d.name.clone() };
            let t = if d.obsolete { RichText::new(title).weak() } else { RichText::new(title) };
            let fb = g.fallback_of(d).map(|f| format!("fallback: {}", name(cx, &f.model))).unwrap_or_else(|| format!("no fallback for players without {BBA}"));
            let r = ui.selectable_label(on, t).on_hover_text(format!("{} · {}{}\n{fb}", name(cx, &g.db.base_airframe(&d.hull)), role.clone().unwrap_or_default(), if d.obsolete { " · obsolete" } else { "" }));
            if r.clicked() {
                st.sel = Some((d.name.clone(), d.hull.clone()));
            }
            ui.label(RichText::new(role.unwrap_or_default()).small().weak());
            if g.fallback_of(d).is_none() {
                ui.label(RichText::new("·").small().color(WARN)).on_hover_text(format!("No fallback for players without {BBA}"));
            }
        });
    }
}

/// Fallbacks with no design of their name: planes of players without the
/// DLC alone (the game's own histories have those).
fn orphans(ui: &mut egui::Ui, cx: &mut Cx, g: &G) {
    let lone: Vec<&Fallback> = g.fallbacks.iter().filter(|f| !g.designs.iter().any(|d| d.name == f.name)).collect();
    if lone.is_empty() {
        return;
    }
    ui.add_space(6.0);
    ui.label(RichText::new(format!("Only without {BBA}")).strong()).on_hover_text("Planes of before the designer with no design of their name");
    for f in lone {
        let p = g.db.check_fallback(f, g.legacy, None, g.fallbacks, g.tag, &|i| name(cx, i));
        ui.horizontal(|ui| {
            match worst(&p) {
                Some(s) => ui.label(RichText::new("⚠").color(colour(s))),
                None => ui.label(RichText::new("✔").color(OK)),
            }
            .on_hover_text(p.iter().map(|p| p.message.clone()).collect::<Vec<_>>().join("\n"));
            ui.label(&f.name);
            ui.label(RichText::new(name(cx, &f.model)).small().weak());
        });
    }
}

/// A combo of the designer's airframes; those the nation cannot build are
/// greyed out with why.
fn frame_combo(ui: &mut egui::Ui, cx: &mut Cx, g: &G, id: &str, value: &mut String) -> bool {
    let shown = if value.is_empty() { "an airframe…".to_string() } else { name(cx, value) };
    let mut changed = false;
    egui::ComboBox::from_id_salt(id).selected_text(shown).width(230.0).show_ui(ui, |ui| {
        let mut last = String::new();
        for e in g.db.airframes() {
            let fam = e.archetype.clone().unwrap_or_default();
            if fam != last {
                ui.label(RichText::new(family_name(cx, &fam)).small().weak());
                last = fam;
            }
            let ok = g.have.equipment.contains(&e.id);
            let r = ui.add_enabled(ok, egui::Button::selectable(*value == e.id, format!("{} ({})", name(cx, &e.id), e.year as i64)));
            let r = if ok {
                r
            } else {
                let techs = g.db.unlocked_text(&[e.id.as_str()], &|i| name(cx, i), 2);
                r.on_disabled_hover_text(if techs.is_empty() { format!("{} cannot build it", g.tag) } else { format!("comes with {techs}") })
            };
            if r.clicked() {
                *value = e.id.clone();
                changed = true;
                ui.close();
            }
        }
    });
    changed
}

fn new_design(ui: &mut egui::Ui, cx: &mut Cx, g: &G, st: &mut AirState, acts: &mut Vec<Act>) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.label(RichText::new("New design").strong());
        if frame_combo(ui, cx, g, "air_new_frame", &mut st.new_frame) {
            st.new_role.clear();
        }
        let roles = if st.new_frame.is_empty() { Vec::new() } else { g.db.roles_of(&st.new_frame) };
        if !roles.is_empty() && !roles.contains(&st.new_role) {
            st.new_role = roles[0].clone();
        }
        if !roles.is_empty() {
            egui::ComboBox::from_id_salt("air_new_role").selected_text(role_name(cx, &st.new_role)).width(230.0).show_ui(ui, |ui| {
                for r in &roles {
                    ui.selectable_value(&mut st.new_role, r.clone(), role_name(cx, r));
                }
            });
        }
        ui.add(egui::TextEdit::singleline(&mut st.new_name).hint_text("name, e.g. Falke").desired_width(230.0));
        let taken = air::names(g.designs, g.fallbacks).contains(st.new_name.trim());
        let ok = !st.new_frame.is_empty() && !st.new_name.trim().is_empty() && !taken;
        ui.horizontal(|ui| {
            if ui.add_enabled(ok, egui::Button::new("➕ Make it")).on_hover_text("With the newest main armament of the role the nation has, an engine, the other slots empty").clicked()
                && let Some(h) = g.db.hull(&st.new_frame)
            {
                let role = (!st.new_role.is_empty()).then_some(st.new_role.as_str());
                let modules = g.db.plane_modules(&h, g.have, role, &BTreeMap::new());
                let frame = role_frame(g, &st.new_frame, &modules);
                acts.push(Act::NewDesign { name: st.new_name.trim().to_string(), frame: frame.clone(), modules });
                st.sel = Some((st.new_name.trim().to_string(), frame));
                st.new_name.clear();
            }
            if taken {
                ui.label(RichText::new("taken").color(WARN).small());
            }
        });
    });
}

/// The airframe of the role these modules make on (the generation of)
/// `frame`.
fn role_frame(g: &G, frame: &str, modules: &[(String, String)]) -> String {
    let Some(h) = g.db.hull(frame) else { return frame.to_string() };
    g.db.plane_role(&h, &g.db.fitted(&h, modules)).and_then(|r| g.db.airframe_for(frame, &r)).unwrap_or_else(|| frame.to_string())
}

/// What a plane module brings, in a few words.
fn brief(m: &Module) -> String {
    const SHORT: [(&str, &str); 12] = [
        ("thrust", "thrust"),
        ("weight", "weight"),
        ("air_attack", "air attack"),
        ("air_defence", "defence"),
        ("air_agility", "agility"),
        ("air_ground_attack", "ground attack"),
        ("naval_strike_attack", "naval attack"),
        ("air_bombing", "bombing"),
        ("air_range", "range"),
        ("reliability", "reliability"),
        ("maximum_speed", "speed"),
        ("build_cost_ic", "cost"),
    ];
    let mut v = Vec::new();
    for (k, label) in SHORT {
        if let Some(x) = m.add.get(k).filter(|x| **x != 0.0) {
            v.push(format!("{}{} {label}", if *x > 0.0 { "+" } else { "" }, num(*x)));
        }
        if let Some(x) = m.multiply.get(k).filter(|x| **x != 0.0) {
            v.push(format!("{}{}% {label}", if *x > 0.0 { "+" } else { "" }, num(x * 100.0)));
        }
    }
    for (missions, add) in &m.missions {
        if let Some(w) = add.get("weight") {
            v.push(format!("+{} weight on {}", num(*w), missions.join("/")));
        }
    }
    if !m.add_types.is_empty() {
        v.push(format!("makes {}", m.add_types.join("/")));
    }
    v.truncate(4);
    v.join(" · ")
}

fn design_view(ui: &mut egui::Ui, cx: &mut Cx, g: &G, st: &mut AirState, d: &Design, acts: &mut Vec<Act>) {
    let Some(hull) = g.db.hull(&d.hull).filter(|_| g.db.is_airframe(&d.hull)) else {
        ui.colored_label(ERR, format!("{} is not an airframe of the game or the mod", d.hull));
        if ui.button("✖ Delete the design").clicked() {
            acts.push(Act::Delete { d: d.clone() });
        }
        return;
    };
    let fitted = g.db.fitted(&hull, &d.modules);
    let problems = g.db.check_plane(d, g.have, g.designs, g.tag, &|i| name(cx, i));
    let stats = g.db.ship_stats(&hull, &fitted);
    let role = g.db.plane_role(&hull, &fitted);
    let base_frame = g.db.base_airframe(&d.hull);
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(150.0, 64.0), Sense::hover());
        if let Some(t) = hull_picture(cx, ui.ctx(), g.db, &d.hull) {
            image(ui, rect, t, Color32::WHITE);
        }
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.label("Name");
                let id = egui::Id::unique(("air_name", &d.file, d.span.start));
                if let Some(v) = commit_text(ui, id, &d.name, 220.0, "the design's name") {
                    let v = v.trim().to_string();
                    if !v.is_empty() {
                        st.sel = Some((v.clone(), d.hull.clone()));
                        acts.push(Act::Rename { d: d.clone(), name: v });
                    }
                }
                let mut obs = d.obsolete;
                if ui.checkbox(&mut obs, "obsolete").on_hover_text("obsolete = yes: the nation no longer builds it (its planes stay)").changed() {
                    acts.push(Act::Obsolete { d: d.clone(), on: obs });
                }
            });
            ui.horizontal(|ui| {
                ui.label("Airframe");
                let mut f = base_frame.clone();
                if frame_combo(ui, cx, g, "air_frame", &mut f)
                    && f != base_frame
                    && let Some(nh) = g.db.hull(&f)
                {
                    let modules = g.db.plane_modules(&nh, g.have, role.as_deref(), &fitted);
                    let frame = role_frame(g, &f, &modules);
                    st.sel = Some((d.name.clone(), frame.clone()));
                    acts.push(Act::Frame { d: d.clone(), frame, modules });
                }
                let what = match &role {
                    Some(r) => RichText::new(format!("→ {}", role_name(cx, r))).strong(),
                    None => RichText::new("→ no role").color(WARN),
                };
                ui.label(what).on_hover_text(format!("What the game makes of it: the main armament decides (machine guns a fighter, bombs close air support, a torpedo a naval bomber ...). Written as type = {}", d.hull));
            });
            ui.horizontal(|ui| {
                if ui.button("⧉ Duplicate").on_hover_text("A copy of the design, and of its fallback").clicked() {
                    let taken = air::names(g.designs, g.fallbacks);
                    let mut n = 2;
                    let mut new = format!("{} {n}", d.name);
                    while taken.contains(&new) {
                        n += 1;
                        new = format!("{} {n}", d.name);
                    }
                    st.sel = Some((new.clone(), d.hull.clone()));
                    acts.push(Act::Duplicate { d: d.clone(), name: new });
                }
                if ui.button("✖ Delete").on_hover_text("Delete this design and its fallback (wings of it then have no design)").clicked() {
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
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
        for s in &hull.slots {
            if let Some(m) = slot_card(ui, cx, &g.kit(), &mut st.show_locked, &mut st.start_slot, &hull, &fitted, s, &problems, &brief) {
                let mut written: Vec<(String, String)> = d.modules.clone();
                match written.iter_mut().find(|(k, _)| *k == s.key) {
                    Some(x) => x.1 = m.clone(),
                    None => written.push((s.key.clone(), m.clone())),
                }
                let frame = role_frame(g, &d.hull, &written);
                if frame != d.hull {
                    st.sel = Some((d.name.clone(), frame.clone()));
                }
                acts.push(Act::Module { d: d.clone(), slot: s.key.clone(), module: m, frame: (frame != d.hull).then_some(frame) });
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
    let mut locked: Vec<String> = Vec::new();
    if !g.have.equipment.contains(&d.hull) {
        locked.push(d.hull.clone());
    }
    for (slot, m) in &fitted {
        if m != EMPTY && hull.slot(slot).is_some() && g.db.modules.contains_key(m) && !g.have.modules.contains(m) {
            locked.push(m.clone());
        }
    }
    give_techs(ui, cx, g, &locked, true, acts);
    ui.add_space(6.0);
    stats_view(ui, cx, &stats, role.as_deref());
    ui.add_space(8.0);
    fallback_view(ui, cx, g, d, role.as_deref(), acts);
}

/// The button that gives the technologies of what the nation lacks (with
/// By Blood Alone, or without it).
fn give_techs(ui: &mut egui::Ui, cx: &mut Cx, g: &G, locked: &[String], bba: bool, acts: &mut Vec<Act>) {
    if locked.is_empty() {
        return;
    }
    let (have, owned) = if bba { (g.have, air::with_bba(g.owned)) } else { (g.legacy, air::without_bba(g.owned)) };
    let techs = g.db.techs_to_give(locked, have, &owned);
    if techs.is_empty() {
        return;
    }
    let names: Vec<String> = techs.iter().map(|t| name(cx, t)).collect();
    let r = ui.button(format!("Give {} the technologies ({}){}", g.tag, techs.len(), if bba { "" } else { " without the DLC" }));
    let into = if bba { format!("if = {{ limit = {{ has_dlc = \"{BBA}\" }} }}") } else { format!("if = {{ limit = {{ NOT = {{ has_dlc = \"{BBA}\" }} }} }}") };
    if r.on_hover_text(format!("Adds to set_technology in the history's {into}: {}", names.join(", "))).clicked() {
        acts.push(Act::GiveTechs(techs, bba));
    }
}

fn stats_view(ui: &mut egui::Ui, cx: &Cx, s: &navy::ShipStats, role: Option<&str>) {
    ui.label(RichText::new("Stats").strong()).on_hover_text("The airframe's, plus each module's, times the modules' multipliers. Left out: thrust over weight adding speed, mission bonuses, design companies, doctrines and the technologies' bonuses.");
    let v = |k: &str| num(s.get(k));
    let (thrust, weight) = (s.get("thrust"), s.get("weight"));
    ui.horizontal_wrapped(|ui| {
        stat_box(
            ui,
            "Air",
            &[
                ("Air attack", v("air_attack"), "Damage to planes"),
                ("Air defence", v("air_defence"), "How much damage it takes before it is shot down"),
                ("Agility", v("air_agility"), "Dogfights: the more agile hits more and is hit less"),
                ("Speed", format!("{} km/h", v("maximum_speed")), "Top speed"),
                ("Range", format!("{} km", v("air_range")), "How far it flies from its base"),
            ],
        );
        stat_box(
            ui,
            "Ground and sea",
            &[
                ("Ground attack", v("air_ground_attack"), "Close air support: damage to divisions"),
                ("Bombing", v("air_bombing"), "Strategic bombing of buildings"),
                ("Naval attack", v("naval_strike_attack"), "Damage to ships"),
                ("Naval targeting", v("naval_strike_targetting"), "Chance to hit ships"),
            ],
        );
        let tw = if weight > thrust + 1e-9 { RichText::new(format!("{} / {}", num(thrust), num(weight))).color(ERR) } else { RichText::new(format!("{} / {}", num(thrust), num(weight))) };
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new("Engine").strong());
                egui::Grid::new("air_engine_stats").num_columns(2).spacing([12.0, 2.0]).show(ui, |ui| {
                    ui.label("Thrust / weight").on_hover_text("The engines' thrust must reach the weight of the airframe and its modules; thrust over it adds speed");
                    ui.label(tw.strong());
                    ui.end_row();
                    ui.label("Reliability");
                    ui.label(RichText::new(format!("{}%", num(s.get("reliability") * 100.0))).strong());
                    ui.end_row();
                    ui.label("Fuel use");
                    ui.label(RichText::new(v("fuel_consumption")).strong());
                    ui.end_row();
                });
            });
        });
        let res: Vec<String> = s.resources.iter().map(|(k, n)| format!("{} {}", num(*n), name(cx, k))).collect();
        stat_box(
            ui,
            "Production",
            &[
                ("Cost", format!("{} IC", v("build_cost_ic")), "Industrial capacity a military factory spends on one"),
                ("Resources", if res.is_empty() { "none".into() } else { res.join(", ") }, "Resources a factory needs to make it"),
                ("Role", role.map(|r| role_name(cx, r)).unwrap_or_default(), "What the game makes of it, by the main armament"),
            ],
        );
    });
}

/// The fallback of a design for players without By Blood Alone.
fn fallback_view(ui: &mut egui::Ui, cx: &mut Cx, g: &G, d: &Design, role: Option<&str>, acts: &mut Vec<Act>) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.label(RichText::new(format!("Without {BBA}")).strong());
        let Some(f) = g.fallback_of(d) else {
            ui.label(RichText::new(format!("Players without {BBA} have no plane designer, and no plane \"{}\". A fallback is a plane of before the designer with the same name: their air wings get it.", d.name)).small().weak());
            match g.db.fallback_model(&d.hull, role, g.legacy) {
                Some(m) => {
                    let has = g.legacy.equipment.contains(&m);
                    let label = format!("➕ Add a fallback: {} ({m})", name(cx, &m));
                    let r = ui.button(label).on_hover_text(format!("create_equipment_variant = {{ name = \"{}\" type = {m} }} inside if = {{ limit = {{ NOT = {{ has_dlc = \"{BBA}\" }} }} }}{}", d.name, if has { String::new() } else { format!("; {} has not researched it without the DLC", g.tag) }));
                    if r.clicked() {
                        acts.push(Act::NewFallback { name: d.name.clone(), model: m });
                    }
                }
                None => {
                    ui.colored_label(WARN, "The game has no plane without modules of this role.");
                }
            }
            return;
        };
        let problems = g.db.check_fallback(f, g.legacy, Some(d), g.fallbacks, g.tag, &|i| name(cx, i));
        ui.horizontal(|ui| {
            ui.label("Plane");
            let mut model = f.model.clone();
            if old_plane_combo(ui, cx, g, &format!("air_fb_model_{}", f.span.start), &mut model, role) && model != f.model {
                acts.push(Act::FallbackModel { f: f.clone(), model });
            }
            if ui.small_button("✖").on_hover_text("Remove the fallback").clicked() {
                acts.push(Act::DeleteFallback { f: f.clone() });
            }
            ui.label(RichText::new(format!("{}:{}", f.file, line_of(cx, &f.pkg, &f.file, f.span.start))).small().weak());
        });
        // the upgrades
        let ups = g.db.upgrades_of(&f.model);
        if !ups.is_empty() {
            ui.horizontal_wrapped(|ui| {
                for u in &ups {
                    let max = g.db.upgrades.get(u).map(|x| x.max_level).unwrap_or(0) as i64;
                    let lv = f.upgrades.iter().find(|(k, _)| k == u).map(|x| x.1).unwrap_or(0);
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(name(cx, u)).small()).on_hover_text(format!("{u}: 0 to {max}; each level {}", upgrade_text(g, u)));
                            if ui.add_enabled(lv > 0, egui::Button::new("−").small()).clicked() {
                                acts.push(Act::Upgrade { f: f.clone(), upgrade: u.clone(), level: lv - 1 });
                            }
                            ui.label(RichText::new(lv.to_string()).strong());
                            if ui.add_enabled(lv < max, egui::Button::new("+").small()).clicked() {
                                acts.push(Act::Upgrade { f: f.clone(), upgrade: u.clone(), level: lv + 1 });
                            }
                        });
                    });
                }
            });
        }
        let s = g.db.old_plane_stats(&f.model, &f.upgrades);
        let get = |k: &str| s.get(k).copied().unwrap_or(0.0);
        ui.label(
            RichText::new(format!(
                "air attack {} · defence {} · agility {} · speed {} km/h · range {} km · ground attack {} · naval attack {} · bombing {} · reliability {}%",
                num(get("air_attack")),
                num(get("air_defence")),
                num(get("air_agility")),
                num(get("maximum_speed")),
                num(get("air_range")),
                num(get("air_ground_attack")),
                num(get("naval_strike_attack")),
                num(get("air_bombing")),
                num(get("reliability") * 100.0)
            ))
            .small(),
        )
        .on_hover_text("The model's stats with its upgrades (each level adds its share)");
        if problems.is_empty() {
            ui.colored_label(OK, format!("✔ Players without {BBA} get \"{}\" as a {}.", f.name, name(cx, &f.model)));
        }
        for p in &problems {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(p.severity.label()).color(colour(p.severity)).strong());
                ui.label(&p.message);
            });
        }
        if g.db.is_old_plane(&f.model) && !g.legacy.equipment.contains(&f.model) {
            give_techs(ui, cx, g, std::slice::from_ref(&f.model), false, acts);
        }
    });
}

fn upgrade_text(g: &G, u: &str) -> String {
    g.db.upgrades.get(u).map(|x| x.stats.iter().map(|(k, v)| format!("{k} {}{}%", if *v > 0.0 { "+" } else { "" }, num(v * 100.0))).collect::<Vec<_>>().join(", ")).unwrap_or_default()
}

/// A combo of the planes of before the designer, those of the role first;
/// those the nation lacks without the DLC are marked.
fn old_plane_combo(ui: &mut egui::Ui, cx: &mut Cx, g: &G, id: &str, value: &mut String, role: Option<&str>) -> bool {
    let mut changed = false;
    egui::ComboBox::from_id_salt(id).selected_text(name(cx, value)).width(230.0).show_ui(ui, |ui| {
        let mut list = g.db.old_planes();
        list.sort_by_key(|e| role.is_none_or(|r| !g.db.types_of(&e.id).iter().any(|t| t == r)));
        let mut last = String::new();
        for e in list {
            let ty = g.db.types_of(&e.id).first().cloned().unwrap_or_default();
            if ty != last {
                ui.label(RichText::new(role_name(cx, &ty)).small().weak());
                last = ty;
            }
            let ok = g.legacy.equipment.contains(&e.id);
            let t = if ok { RichText::new(format!("{} ({})", name(cx, &e.id), e.year as i64)) } else { RichText::new(format!("{} ({}) · not researched", name(cx, &e.id), e.year as i64)).weak() };
            let r = ui.add(egui::Button::selectable(*value == e.id, t));
            let r = if ok {
                r
            } else {
                let techs = g.db.unlocked_text(&[e.id.as_str()], &|i| name(cx, i), 2);
                r.on_hover_text(if techs.is_empty() { format!("{} cannot build it", g.tag) } else { format!("comes with {techs}") })
            };
            if r.clicked() {
                *value = e.id.clone();
                changed = true;
                ui.close();
            }
        }
    });
    changed
}

// ------------------------------------------------------------- air wings

fn field_label(f: &Airfield, used: f64, tag: &str) -> String {
    let owner = if f.tag == tag || f.tag.is_empty() { String::new() } else { format!(" · {}", f.tag) };
    format!("{} · level {} · {}/{} planes{owner}", f.label(), f.level, num(used), f.level * air::PER_LEVEL)
}

/// A combo of the air bases of the map: the nation's first.
fn field_combo(ui: &mut egui::Ui, g: &G, id: String, current: Option<u32>, used: &BTreeMap<u32, f64>) -> Option<u32> {
    let shown = match current {
        None => RichText::new("an air base…"),
        Some(s) => match g.fields.iter().find(|f| f.state == s) {
            Some(f) => RichText::new(field_label(f, used.get(&s).copied().unwrap_or(0.0), g.tag)),
            None => RichText::new(format!("state {s}: no air base")).color(ERR),
        },
    };
    let mut out = None;
    egui::ComboBox::from_id_salt(id).selected_text(shown).width(280.0).show_ui(ui, |ui| {
        if g.fields.is_empty() {
            ui.label(RichText::new(format!("The map has no air bases: {}.", air::PLACE_AIRBASE)).weak());
        }
        let mut list: Vec<&Airfield> = g.fields.iter().collect();
        list.sort_by_key(|f| (f.tag != g.tag, f.tag.clone(), f.name.clone()));
        for f in list {
            let label = field_label(f, used.get(&f.state).copied().unwrap_or(0.0), g.tag);
            let t = if f.tag == g.tag { RichText::new(label) } else { RichText::new(label).weak() };
            if ui.add(egui::Button::selectable(current == Some(f.state), t)).clicked() {
                out = Some(f.state);
                ui.close();
            }
        }
    });
    out.filter(|s| Some(*s) != current)
}

fn design_label(cx: &Cx, d: &Design) -> String {
    format!("{} · {}", d.name, name(cx, &d.hull))
}

#[allow(clippy::too_many_arguments)]
fn wings_tab(ui: &mut egui::Ui, cx: &mut Cx, g: &G, st: &mut AirState, oob: Option<&(OobLine, Option<(String, String)>)>, bases: &[WingBase], acts: &mut Vec<Act>) {
    ui.label(
        RichText::new(format!(
            "The air wings {} starts with, at the air bases of its states. They are in a history/units file its history loads with set_air_oob, one for players with {BBA} (wings of its designs) and one for those without it (of the fallbacks), kept in step here.",
            g.tag
        ))
        .weak()
        .small(),
    );
    let used: BTreeMap<u32, f64> = bases.iter().map(|b| (b.state, b.wings.iter().filter_map(|w| w.amount).sum())).collect();
    let own: Vec<&Airfield> = g.fields.iter().filter(|f| f.tag == g.tag).collect();
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Air bases").strong());
        if own.is_empty() {
            let why = if g.fields.is_empty() { "The map has none yet" } else { "None is in a state of this nation" };
            ui.colored_label(WARN, format!("{why}: {}. Air wings are at air bases.", air::PLACE_AIRBASE));
        } else {
            ui.label(own.iter().map(|f| field_label(f, used.get(&f.state).copied().unwrap_or(0.0), g.tag)).collect::<Vec<_>>().join("  ·  "));
        }
    });
    ui.add_space(4.0);
    let lines = air::air_oobs(cx.st, g.tag);
    let both = lines.iter().find(|l| air::branch_of(&l.need) == Branch::Both);
    let legacy_line = lines.iter().find(|l| air::branch_of(&l.need) == Branch::Legacy);
    let Some((line, file)) = oob else {
        ui.label(format!("{} has no air order of battle for players with {BBA} yet (set_air_oob in its history).", g.tag));
        if ui.button("Make them").on_hover_text(format!("Makes history/units/{0}_air_bba.txt and {0}_air_legacy.txt, and loads them in the history: if = {{ limit = {{ has_dlc = \"{BBA}\" }} set_air_oob = ... else = {{ set_air_oob = ... }} }} (one undo step)", g.tag)).clicked() {
            acts.push(Act::MakeOobs);
        }
        return;
    };
    let Some((pkg, path)) = file else {
        ui.colored_label(ERR, format!("set_air_oob = {}: there is no history/units/{}.txt in the content", line.name, line.name));
        if ui.button(format!("Make history/units/{}.txt", line.name)).clicked() {
            acts.push(Act::MakeOobs);
        }
        return;
    };
    ui.label(RichText::new(format!("{path}{}", if line.need.is_empty() { String::new() } else { format!(" (for players {})", line.need.describe()) })).small().weak());
    if both.is_some() && legacy_line.is_none() {
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(WARN, format!("set_air_oob = {} is loaded for players with and without {BBA}: those without it have no designer's planes.", line.name));
            if ui.button("Split it").on_hover_text(format!("Loads it with {BBA} only, and a file of the fallbacks without it (one undo step)")).clicked() {
                acts.push(Act::MakeOobs);
            }
        });
    }
    let problems = air::check_wings(g.db, bases, g.tag, g.fields, false, &|t| g.planes_of(cx, t), &|i| name(cx, i));
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
    let mut wa: Vec<WingAct> = Vec::new();
    if bases.is_empty() {
        ui.label(RichText::new("No air wings yet.").weak());
    }
    for b in bases {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("✈").strong());
                match g.fields.iter().find(|f| f.state == b.state) {
                    Some(f) => ui.label(RichText::new(field_label(f, used.get(&b.state).copied().unwrap_or(0.0), g.tag)).strong()),
                    None => ui.label(RichText::new(format!("state {}: no air base", b.state)).color(ERR).strong()),
                };
            });
            show(ui, &b.span);
            for w in &b.wings {
                ui.horizontal(|ui| wing_row(ui, cx, g, w, b.state, &used, &mut wa));
                show(ui, &w.span);
            }
        });
        ui.add_space(4.0);
    }
    // a new wing
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("New air wing").strong());
            if st.wing_amount.is_empty() {
                st.wing_amount = "50".into();
            }
            if st.wing_design.as_ref().is_none_or(|(n, h)| !g.designs.iter().any(|d| d.name == *n && d.hull == *h)) {
                st.wing_design = g.designs.iter().find(|d| !d.obsolete).or(g.designs.first()).map(|d| (d.name.clone(), d.hull.clone()));
            }
            ui.add(egui::TextEdit::singleline(&mut st.wing_amount).desired_width(36.0));
            ui.label("×");
            let shown = st.wing_design.as_ref().and_then(|(n, h)| g.designs.iter().find(|d| d.name == *n && d.hull == *h)).map(|d| design_label(cx, d)).unwrap_or_else(|| "a design…".into());
            egui::ComboBox::from_id_salt("air_new_wing_design").selected_text(shown).width(240.0).show_ui(ui, |ui| {
                if g.designs.is_empty() {
                    ui.label(RichText::new("No designs yet: make one in the plane designer.").weak());
                }
                for d in g.designs {
                    if ui.add(egui::Button::selectable(st.wing_design.as_ref() == Some(&(d.name.clone(), d.hull.clone())), design_label(cx, d))).clicked() {
                        st.wing_design = Some((d.name.clone(), d.hull.clone()));
                        ui.close();
                    }
                }
            });
            ui.label("at");
            if st.wing_base.is_none() {
                st.wing_base = own.first().map(|f| f.state);
            }
            if let Some(s) = field_combo(ui, g, "air_new_wing_base".into(), st.wing_base, &used) {
                st.wing_base = Some(s);
            }
            ui.add(egui::TextEdit::singleline(&mut st.wing_name).hint_text("name (optional)").desired_width(150.0));
            let n: f64 = st.wing_amount.trim().parse().unwrap_or(0.0);
            let d = st.wing_design.as_ref().and_then(|(nm, h)| g.designs.iter().find(|d| d.name == *nm && d.hull == *h));
            let ok = d.is_some() && st.wing_base.is_some() && n >= 1.0;
            if ui.add_enabled(ok, egui::Button::new("➕ Add")).on_hover_text(format!("Adds the wing; the file of players without {BBA} gets one of the design's fallback")).clicked()
                && let (Some(d), Some(state)) = (d, st.wing_base)
            {
                let name = st.wing_name.trim();
                let spec = WingSpec { equipment: d.hull.clone(), owner: g.tag.to_string(), creator: None, amount: n.round(), version: Some(d.name.clone()), name: (!name.is_empty()).then(|| name.to_string()) };
                wa.push(WingAct::Add { state, spec });
                st.wing_name.clear();
            }
        });
    });
    let unplaced: Vec<&FleetProblem> = problems.iter().filter(|p| p.severity != Severity::Info && !bases.iter().any(|b| b.span.start == p.span.start || b.wings.iter().any(|w| w.span.start == p.span.start))).collect();
    for p in unplaced {
        ui.colored_label(colour(p.severity), &p.message);
    }
    let (e, w) = (problems.iter().filter(|p| p.severity == Severity::Error).count(), problems.iter().filter(|p| p.severity == Severity::Warning).count());
    if !bases.is_empty() {
        if e + w == 0 {
            ui.colored_label(OK, "✔ No problems in the air wings.");
        } else {
            ui.colored_label(if e > 0 { ERR } else { WARN }, format!("{e} errors, {w} warnings in the air wings (under each line)"));
        }
    }
    ui.add_space(8.0);
    legacy_view(ui, cx, g, bases, acts);
    for a in wa {
        acts.push(Act::Wings(pkg.clone(), path.clone(), a));
    }
}

fn wing_row(ui: &mut egui::Ui, cx: &mut Cx, g: &G, w: &air::Wing, state: u32, used: &BTreeMap<u32, f64>, wa: &mut Vec<WingAct>) {
    let n = w.name.clone().unwrap_or_default();
    if let Some(v) = commit_text(ui, egui::Id::unique(("air_wing_name", w.span.start)), &n, 150.0, "name") {
        wa.push(WingAct::Name { wing: w.span.clone(), name_span: w.name_span.clone(), name: v.trim().to_string() });
    }
    let amount = w.amount.map(num).unwrap_or_default();
    if let Some(v) = commit_text(ui, egui::Id::unique(("air_wing_amount", w.span.start)), &amount, 40.0, "planes")
        && let Ok(x) = v.trim().parse::<f64>()
        && x >= 1.0
    {
        wa.push(WingAct::Amount(w.span.clone(), x.round()));
    }
    ui.label("×");
    let whose = w.creator.clone().or(w.owner.clone()).unwrap_or_else(|| g.tag.to_string());
    let cur = g.designs.iter().find(|d| whose == g.tag && Some(&d.name) == w.version.as_ref() && d.hull == w.equipment);
    let shown = match (cur, &w.version) {
        (Some(d), _) => RichText::new(design_label(cx, d)),
        (None, Some(v)) if whose != g.tag => RichText::new(format!("{v} · {} (of {whose})", name(cx, &w.equipment))),
        (None, Some(v)) => RichText::new(format!("{v} · {}", name(cx, &w.equipment))).color(ERR),
        (None, None) => RichText::new(format!("no design · {}", name(cx, &w.equipment))).color(WARN),
    };
    egui::ComboBox::from_id_salt(("air_wing_design", w.span.start)).selected_text(shown).width(240.0).show_ui(ui, |ui| {
        for d in g.designs {
            if ui.add(egui::Button::selectable(cur.is_some_and(|c| c.name == d.name && c.hull == d.hull), design_label(cx, d))).clicked() {
                wa.push(WingAct::Design { wing: w.span.clone(), equipment: d.hull.clone(), version: d.name.clone() });
                ui.close();
            }
        }
    });
    ui.menu_button("move ▾", |ui| {
        let mut list: Vec<&Airfield> = g.fields.iter().filter(|f| f.state != state).collect();
        list.sort_by_key(|f| (f.tag != g.tag, f.tag.clone(), f.name.clone()));
        if list.is_empty() {
            ui.label(RichText::new("No other air base").weak());
        }
        for f in list {
            if ui.button(field_label(f, used.get(&f.state).copied().unwrap_or(0.0), g.tag)).clicked() {
                wa.push(WingAct::Move { wing: w.span.clone(), name_span: w.name_span.clone(), to: f.state });
                ui.close();
            }
        }
    });
    if ui.small_button("✖").on_hover_text("Remove this wing").clicked() {
        wa.push(WingAct::Remove { wing: w.span.clone(), name_span: w.name_span.clone() });
    }
}

/// The legacy file: what it is, whether it follows the wings above, its
/// problems.
fn legacy_view(ui: &mut egui::Ui, cx: &mut Cx, g: &G, bases: &[WingBase], acts: &mut Vec<Act>) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.label(RichText::new(format!("Players without {BBA}")).strong());
        let without = air::without_bba(g.owned);
        let Some((line, file)) = air::air_oob(cx.st, g.tag, &without) else {
            ui.colored_label(WARN, format!("No set_air_oob for players without {BBA}: they start with no air wings."));
            if ui.button("Make their file").on_hover_text("A file of the fallbacks' wings, loaded for players without the DLC (one undo step)").clicked() {
                acts.push(Act::MakeOobs);
            }
            return;
        };
        let Some((lp, lpath)) = file else {
            ui.colored_label(ERR, format!("set_air_oob = {}: there is no history/units/{}.txt in the content", line.name, line.name));
            if ui.button(format!("Make history/units/{}.txt", line.name)).clicked() {
                acts.push(Act::MakeOobs);
            }
            return;
        };
        if air::branch_of(&line.need) == Branch::Both {
            ui.label(RichText::new(format!("{lpath}: the same file as players with {BBA}")).small().weak());
            return;
        }
        let lbases = cx.st.file(&lp, &lpath).and_then(|f| f.script.as_ref()).map(|s| air::read_wings(&s.root)).unwrap_or_default();
        let (made, skipped) = air::legacy_wings(g.db, bases, g.tag, &|t| g.planes_of(cx, t), g.legacy);
        let cur = air::specs(&lbases, g.tag);
        let n: usize = cur.iter().map(|x| x.1.len()).sum();
        let planes: f64 = cur.iter().flat_map(|x| &x.1).map(|w| w.amount).sum();
        ui.label(RichText::new(format!("{lpath}: {n} wings, {} planes", num(planes))).small());
        if cur == made {
            ui.colored_label(OK, "✔ In step with the wings above: each design's wing is one of its fallback (else of the plane its airframe stands for).");
        } else {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(WARN, "It differs from the wings above (edited by hand, or written before): changes above leave it as it is.");
                if ui.button("Rewrite it from the wings above").on_hover_text("Its air_wings made of the wings above, a design's wing becoming one of its fallback (one undo step)").clicked() {
                    acts.push(Act::RebuildLegacy);
                }
            });
        }
        for s in &skipped {
            ui.colored_label(WARN, s);
        }
        let problems = air::check_wings(g.db, &lbases, g.tag, g.fields, true, &|t| g.planes_of(cx, t), &|i| name(cx, i));
        let (e, w) = (problems.iter().filter(|p| p.severity == Severity::Error).count(), problems.iter().filter(|p| p.severity == Severity::Warning).count());
        for p in problems.iter().filter(|p| p.severity != Severity::Info).take(8) {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(p.severity.label()).color(colour(p.severity)).small().strong());
                ui.label(RichText::new(format!("{}:{}: {}", lpath, line_of(cx, &lp, &lpath, p.span.start), p.message)).small());
            });
        }
        if e + w > 8 {
            ui.label(RichText::new(format!("… {} more (the Check page lists them)", e + w - 8)).small().weak());
        }
    });
}

// ------------------------------------------------------------- edits

/// A history file's edit that puts `rendered` in the block of players with
/// (`bba`) or without By Blood Alone (made at the end when there is none),
/// after its last design.
fn into_branch(text: &str, root: &Block, bba: bool, rendered: &str) -> Vec<Edit> {
    match air::branch_block(root, bba) {
        Some(b) => {
            let after = b.items.iter().rposition(|i| i.is("create_equipment_variant"));
            vec![script::insert_item(text, b, rendered, after)]
        }
        None => vec![script::insert_item(text, root, &air::render_branch(bba, rendered), None)],
    }
}

/// The edits of a change to one history file.
fn design_edits(text: &str, root: &Block, a: &Act) -> Vec<Edit> {
    let block = |span: &Span| item_at(root, span).filter(|i| i.is("create_equipment_variant")).and_then(|i| i.block());
    match a {
        Act::NewDesign { name, frame, modules } => into_branch(text, root, true, &navy::render_design(name, frame, modules, false)),
        Act::Module { d, slot, module, frame } => {
            let Some(b) = block(&d.span) else { return Vec::new() };
            let mut out = match b.block("modules") {
                Some(m) => vec![script::set_atom(text, m, slot, module)],
                None => vec![script::insert_item(text, b, &format!("modules = {{\n\t{slot} = {module}\n}}"), None)],
            };
            if let Some(f) = frame {
                out.push(script::set_atom(text, b, "type", f));
            }
            out
        }
        Act::Frame { d, frame, modules } => {
            let Some(b) = block(&d.span) else { return Vec::new() };
            let lines = modules.iter().map(|(k, v)| format!("{k} = {v}")).collect::<Vec<_>>().join("\n");
            let mut out = vec![script::set_atom(text, b, "type", frame)];
            match b.block("modules") {
                Some(m) => out.push(script::set_block_inner(text, m, &lines)),
                None => out.push(script::insert_item(text, b, &format!("modules = {{\n\t{}\n}}", lines.replace('\n', "\n\t")), None)),
            }
            out
        }
        Act::Obsolete { d, on } => {
            let Some(b) = block(&d.span) else { return Vec::new() };
            match (on, b.get("obsolete")) {
                (true, _) => vec![script::set_atom(text, b, "obsolete", "yes")],
                (false, Some(o)) => vec![script::remove_item(text, o)],
                (false, None) => Vec::new(),
            }
        }
        Act::GiveTechs(techs, bba) => {
            let lines: Vec<String> = techs.iter().map(|t| format!("{t} = 1")).collect();
            match air::branch_block(root, *bba) {
                Some(b) => match b.block("set_technology") {
                    Some(stb) => {
                        let have: BTreeSet<&str> = stb.items.iter().filter_map(|i| i.key.as_ref().map(|k| k.text.as_str())).collect();
                        let new: Vec<String> = techs.iter().filter(|t| !have.contains(t.as_str())).map(|t| format!("{t} = 1")).collect();
                        if new.is_empty() { Vec::new() } else { vec![script::insert_item(text, stb, &new.join("\n"), None)] }
                    }
                    None => vec![script::insert_item(text, b, &format!("set_technology = {{\n\t{}\n}}", lines.join("\n\t")), None)],
                },
                None => vec![script::insert_item(text, root, &air::render_branch(*bba, &format!("set_technology = {{\n\t{}\n}}", lines.join("\n\t"))), None)],
            }
        }
        Act::NewFallback { name, model } => into_branch(text, root, false, &air::render_fallback(name, model, &[], false)),
        Act::FallbackModel { f, model } => {
            let Some(b) = block(&f.span) else { return Vec::new() };
            let mut out = vec![script::set_atom(text, b, "type", model)];
            if let Some(u) = b.get("upgrades") {
                out.push(script::remove_item(text, u));
            }
            out
        }
        Act::Upgrade { f, upgrade, level } => {
            let Some(b) = block(&f.span) else { return Vec::new() };
            match b.block("upgrades") {
                Some(u) => match (u.get(upgrade), *level) {
                    (Some(i), 0) => vec![script::remove_item(text, i)],
                    _ => vec![script::set_atom(text, u, upgrade, &level.to_string())],
                },
                None if *level == 0 => Vec::new(),
                None => vec![script::insert_item(text, b, &format!("upgrades = {{\n\t{upgrade} = {level}\n}}"), None)],
            }
        }
        _ => Vec::new(),
    }
}

/// A change to several items, maybe of several files.
enum Job {
    Remove(Span),
    /// A copy of a design, renamed, after it.
    Copy(Span, String),
    /// `key = value` in the block of the item.
    Set(Span, &'static str, String),
}

/// Run jobs on files as one undo step.
fn run_jobs(cx: &mut Cx, label: String, jobs: Vec<(String, String, Job)>) {
    let mut by_file: BTreeMap<(String, String), Vec<Job>> = BTreeMap::new();
    for (p, f, j) in jobs {
        by_file.entry((p, f)).or_default().push(j);
    }
    if by_file.is_empty() {
        return;
    }
    cx.st.begin(label);
    for ((pkg, file), jobs) in by_file {
        cx.st.edit_text(&pkg, &file, |text| {
            let s = Script::parse(text);
            let mut e: Vec<Edit> = Vec::new();
            for j in &jobs {
                match j {
                    Job::Remove(span) => {
                        if let Some(i) = item_at(&s.root, span) {
                            e.push(script::remove_item(text, i));
                        }
                    }
                    Job::Copy(span, name) => {
                        let Some(i) = item_at(&s.root, span) else { continue };
                        let src = script::item_source(text, i);
                        let cs = Script::parse(&src);
                        let Some(b) = cs.root.items.first().and_then(|x| x.block()) else { continue };
                        let copy = script::apply(&src, &[script::set_atom(&src, b, "name", &script::quote(name))]);
                        let (block, idx) = parent_of(&s.root, span).unwrap_or((&s.root, None));
                        e.push(script::insert_item(text, block, &copy, idx));
                    }
                    Job::Set(span, key, value) => {
                        if let Some(b) = item_at(&s.root, span).and_then(|i| i.block()) {
                            e.push(script::set_atom(text, b, key, value));
                        }
                    }
                }
            }
            script::apply(text, &e)
        });
    }
    cx.st.commit();
}

/// The wings of a file of the nation's to rename a design in: (package,
/// path, wing) where the version is `old` and the design the nation's.
fn wings_named(cx: &Cx, g: &G, old: &str, frame: Option<&str>) -> Vec<(String, String, Job)> {
    let mut out = Vec::new();
    let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
    for l in air::air_oobs(cx.st, g.tag) {
        let Some((p, path)) = navy::oob_file(cx.st, g.tag, &l.name) else { continue };
        if !seen.insert((p.clone(), path.clone())) {
            continue;
        }
        let Some(s) = cx.st.file(&p, &path).and_then(|f| f.script.as_ref()) else { continue };
        for b in air::read_wings(&s.root) {
            for w in b.wings {
                let whose = w.creator.clone().or(w.owner.clone()).unwrap_or_else(|| g.tag.to_string());
                let fits = frame.is_none_or(|f| w.equipment == f || !g.db.is_airframe(&w.equipment));
                if whose == g.tag && w.version.as_deref() == Some(old) && fits {
                    out.push((p.clone(), path.clone(), Job::Set(w.span.clone(), "version_name", String::new())));
                }
            }
        }
    }
    out
}

/// The edits of a change to the wings (`text`: the file of players with
/// By Blood Alone).
fn wing_edits(text: &str, root: &Block, a: &WingAct) -> Vec<Edit> {
    let at = |span: &Span| item_at(root, span);
    let wings_block = root.block("air_wings");
    let state_block = |state: u32| wings_block.and_then(|w| w.items.iter().find(|i| i.key() == state.to_string()).and_then(|i| i.block()));
    let add = |state: u32, spec: &WingSpec| -> Edit {
        match (wings_block, state_block(state)) {
            (_, Some(b)) => script::insert_item(text, b, &air::render_wing(spec), None),
            (Some(w), None) => script::insert_item(text, w, &air::render_base(state, std::slice::from_ref(spec)), None),
            (None, None) => script::insert_item(text, root, &format!("air_wings = {{\n\t{}\n}}", air::render_base(state, std::slice::from_ref(spec)).replace('\n', "\n\t")), None),
        }
    };
    // the wing and its name gone; the state too when it was its last
    let remove = |wing: &Span, name_span: &Option<Span>| -> Vec<Edit> {
        let Some((parent, _)) = parent_of(root, wing) else { return Vec::new() };
        let others = parent.items.iter().filter(|i| i.block().is_some() && !i.is("ace") && i.span != *wing).count();
        if others == 0
            && let Some(w) = wings_block
            && let Some(si) = w.items.iter().find(|i| i.block().is_some_and(|b| std::ptr::eq(b, parent)))
        {
            return vec![script::remove_item(text, si)];
        }
        let mut out: Vec<Edit> = at(wing).map(|i| script::remove_item(text, i)).into_iter().collect();
        if let Some(n) = name_span.as_ref().and_then(at) {
            out.push(script::remove_item(text, n));
        }
        out
    };
    let spec_of = |wing: &Span, name_span: &Option<Span>| -> Option<WingSpec> {
        let i = at(wing)?;
        let b = i.block()?;
        Some(WingSpec {
            equipment: i.key().to_string(),
            owner: b.atom("owner").unwrap_or("").to_string(),
            creator: b.atom("creator").map(String::from),
            amount: b.num("amount").unwrap_or(1.0),
            version: b.atom("version_name").map(String::from),
            name: name_span.as_ref().and_then(at).and_then(|n| n.text()).map(String::from),
        })
    };
    match a {
        WingAct::Add { state, spec } => vec![add(*state, spec)],
        WingAct::Amount(span, n) => at(span).and_then(|i| i.block()).map(|b| vec![script::set_atom(text, b, "amount", &num(*n))]).unwrap_or_default(),
        WingAct::Name { wing, name_span, name } => match (name_span.as_ref().and_then(at), name.is_empty()) {
            (Some(n), true) => vec![script::remove_item(text, n)],
            (Some(n), false) => vec![script::replace_item(text, n, &format!("name = {}", script::quote(name)))],
            (None, true) => Vec::new(),
            (None, false) => match parent_of(root, wing) {
                Some((b, idx)) => vec![script::insert_item(text, b, &format!("name = {}", script::quote(name)), idx)],
                None => Vec::new(),
            },
        },
        WingAct::Design { wing, equipment, version } => {
            let Some(spec) = spec_of(wing, &None) else { return Vec::new() };
            let Some(i) = at(wing) else { return Vec::new() };
            let spec = WingSpec { equipment: equipment.clone(), version: Some(version.clone()), creator: None, name: None, ..spec };
            vec![script::replace_item(text, i, &air::render_wing(&spec))]
        }
        WingAct::Move { wing, name_span, to } => {
            let Some(spec) = spec_of(wing, name_span) else { return Vec::new() };
            let mut out = remove(wing, name_span);
            out.push(add(*to, &spec));
            out
        }
        WingAct::Remove { wing, name_span } => remove(wing, name_span),
    }
}

/// The air_wings of a file's text made these.
fn set_wings(text: &str, bases: &[(u32, Vec<WingSpec>)]) -> String {
    let s = Script::parse(text);
    let inner = air::render_bases(bases);
    let e = match s.root.block("air_wings") {
        Some(b) => script::set_block_inner(text, b, &inner),
        None => script::insert_item(text, &s.root, &format!("air_wings = {{\n\t{}\n}}", inner.replace('\n', "\n\t")), None),
    };
    script::apply(text, &[e])
}

/// The file of players without By Blood Alone: (package, path, its text).
fn legacy_file(cx: &Cx, g: &G) -> Option<(String, String, String)> {
    let (line, file) = air::air_oob(cx.st, g.tag, &air::without_bba(g.owned))?;
    if air::branch_of(&line.need) == Branch::Both {
        return None;
    }
    let (p, path) = file?;
    let text = cx.st.text(&p, &path).unwrap_or("").to_string();
    Some((p, path, text))
}

fn apply(cx: &mut Cx, g: &G, acts: Vec<Act>) {
    let tag = g.tag.to_string();
    for a in acts {
        match &a {
            Act::Wings(pkg, path, wa) => {
                let label = match wa {
                    WingAct::Add { spec, .. } => format!("air: new wing of {}", spec.version.clone().unwrap_or_default()),
                    WingAct::Amount(..) => "air: planes of a wing".into(),
                    WingAct::Name { .. } => "air: name of a wing".into(),
                    WingAct::Design { version, .. } => format!("air: wing of {version}"),
                    WingAct::Move { .. } => "air: move a wing".into(),
                    WingAct::Remove { .. } => "air: remove a wing".into(),
                };
                let text = cx.st.text(pkg, path).unwrap_or("").to_string();
                let s = Script::parse(&text);
                let e = wing_edits(&text, &s.root, wa);
                if e.is_empty() {
                    continue;
                }
                let new = script::apply(&text, &e);
                // the wings of players without the DLC follow when they did
                let planes_of = |t: &str| g.planes_of(cx, t);
                let follow = legacy_file(cx, g).and_then(|(lp, lpath, ltext)| {
                    let cur = air::specs(&air::read_wings(&Script::parse(&ltext).root), &tag);
                    let before = air::legacy_wings(g.db, &air::read_wings(&s.root), &tag, &planes_of, g.legacy).0;
                    (cur == before || cur.is_empty()).then(|| {
                        let after = air::legacy_wings(g.db, &air::read_wings(&Script::parse(&new).root), &tag, &planes_of, g.legacy).0;
                        (lp, lpath, set_wings(&ltext, &after))
                    })
                });
                cx.st.begin(label);
                cx.st.set_text(pkg, path, new);
                if let Some((lp, lpath, lt)) = follow {
                    cx.st.set_text(&lp, &lpath, lt);
                }
                cx.st.commit();
            }
            Act::RebuildLegacy => {
                let Some((_, Some((p, path)))) = air::air_oob(cx.st, &tag, &air::with_bba(g.owned)) else { continue };
                let Some((lp, lpath, ltext)) = legacy_file(cx, g) else { continue };
                let bases = cx.st.file(&p, &path).and_then(|f| f.script.as_ref()).map(|s| air::read_wings(&s.root)).unwrap_or_default();
                let made = air::legacy_wings(g.db, &bases, &tag, &|t| g.planes_of(cx, t), g.legacy).0;
                let new = set_wings(&ltext, &made);
                cx.edit_file(&lp, &lpath, "air: wings without By Blood Alone", None, |_, _| Some(new));
            }
            Act::MakeOobs => make_oobs(cx, g),
            Act::Rename { d, name } => {
                let mut jobs = vec![(d.pkg.clone(), d.file.clone(), Job::Set(d.span.clone(), "name", script::quote(name)))];
                if let Some(f) = g.fallback_of(d) {
                    jobs.push((f.pkg.clone(), f.file.clone(), Job::Set(f.span.clone(), "name", script::quote(name))));
                }
                for (p, path, j) in wings_named(cx, g, &d.name, None) {
                    if let Job::Set(span, k, _) = j {
                        jobs.push((p, path, Job::Set(span, k, script::quote(name))));
                    }
                }
                run_jobs(cx, format!("air: rename the design {} to {name}", d.name), jobs);
            }
            Act::Duplicate { d, name } => {
                let mut jobs = vec![(d.pkg.clone(), d.file.clone(), Job::Copy(d.span.clone(), name.clone()))];
                if let Some(f) = g.fallback_of(d) {
                    jobs.push((f.pkg.clone(), f.file.clone(), Job::Copy(f.span.clone(), name.clone())));
                }
                run_jobs(cx, format!("air: new design {name}"), jobs);
            }
            Act::Delete { d } => {
                let mut jobs = vec![(d.pkg.clone(), d.file.clone(), Job::Remove(d.span.clone()))];
                if let Some(f) = g.fallback_of(d) {
                    jobs.push((f.pkg.clone(), f.file.clone(), Job::Remove(f.span.clone())));
                }
                run_jobs(cx, format!("air: delete the design {}", d.name), jobs);
            }
            Act::DeleteFallback { f } => run_jobs(cx, format!("air: remove the fallback of {}", f.name), vec![(f.pkg.clone(), f.file.clone(), Job::Remove(f.span.clone()))]),
            _ => {
                let (file, pkg) = match &a {
                    Act::Module { d, .. } | Act::Frame { d, .. } | Act::Obsolete { d, .. } => (d.file.clone(), d.pkg.clone()),
                    Act::FallbackModel { f, .. } | Act::Upgrade { f, .. } => (f.file.clone(), f.pkg.clone()),
                    Act::NewFallback { name, .. } => g.designs.iter().find(|d| d.name == *name).map(|d| (d.file.clone(), d.pkg.clone())).unwrap_or_else(|| (history_file(cx, &tag), tag.clone())),
                    _ => (planes_file(cx, g), tag.clone()),
                };
                let label = match &a {
                    Act::NewDesign { name, .. } => format!("air: new design {name}"),
                    Act::Module { slot, module, .. } => format!("air: {} = {module}", navy::slot_name(slot)),
                    Act::Frame { frame, .. } => format!("air: airframe {frame}"),
                    Act::Obsolete { d, .. } => format!("air: {} obsolete", d.name),
                    Act::GiveTechs(t, _) => format!("air: give {} technologies", t.len()),
                    Act::NewFallback { name, model } => format!("air: fallback of {name}: {model}"),
                    Act::FallbackModel { model, .. } => format!("air: fallback {model}"),
                    Act::Upgrade { upgrade, level, .. } => format!("air: {upgrade} = {level}"),
                    _ => "air".into(),
                };
                if !edit_with(cx, &pkg, &file, &label, None, |text, root| design_edits(text, root, &a)) {
                    log::warn!("air: nothing to change for {label}");
                }
            }
        }
    }
}

/// The history file new designs go to: that of the nation's plane designs,
/// else its first.
fn planes_file(cx: &Cx, g: &G) -> String {
    g.designs.first().map(|d| d.file.clone()).unwrap_or_else(|| history_file(cx, g.tag))
}

/// A change of a file's text.
type Rewrite = Box<dyn FnOnce(&str) -> String>;

/// The air orders of battle made (one undo step): with no set_air_oob, a
/// file for players with By Blood Alone and one for those without, loaded
/// by `if`/`else`; one loaded for both split (its wings become the legacy
/// file's through the fallbacks); a missing line or file added.
fn make_oobs(cx: &mut Cx, g: &G) {
    let tag = g.tag.to_string();
    let lines = air::air_oobs(cx.st, &tag);
    let find = |b: Branch| lines.iter().find(|l| air::branch_of(&l.need) == b).cloned();
    let (bba, legacy, both) = (find(Branch::Bba), find(Branch::Legacy), find(Branch::Both));
    let hist = lines.first().map(|l| l.file.clone()).unwrap_or_else(|| history_file(cx, &tag));
    let legacy_name = |n: &str| n.strip_suffix("_bba").map(|s| format!("{s}_legacy")).unwrap_or_else(|| format!("{n}_legacy"));
    let wings_of = |cx: &Cx, name: &str| -> Vec<WingBase> { navy::oob_file(cx.st, &tag, name).and_then(|(p, path)| cx.st.file(&p, &path)).and_then(|f| f.script.as_ref()).map(|s| air::read_wings(&s.root)).unwrap_or_default() };
    let empty = "air_wings = {\n}\n".to_string();
    // (name of the file for players with the DLC, of that without it)
    let mut new_files: Vec<(String, String)> = Vec::new();
    let mut history: Option<Rewrite> = None;
    let mut bba_name = bba.as_ref().map(|l| l.name.clone());
    match (&bba, &legacy, &both) {
        (None, None, None) => {
            let (b, l) = (format!("{tag}_air_bba"), format!("{tag}_air_legacy"));
            let lines = both_lines(&b, &l);
            history = Some(Box::new(move |text: &str| {
                let s = Script::parse(text);
                let after = s.root.items.iter().rposition(|i| i.is("oob") || i.is("set_naval_oob"));
                script::apply(text, &[script::insert_item(text, &s.root, &lines, after)])
            }));
            bba_name = Some(b);
        }
        (None, None, Some(x)) => {
            let l = legacy_name(&x.name);
            let rendered = both_lines(&x.name, &l);
            let span = x.span.clone();
            history = Some(Box::new(move |text: &str| {
                let s = Script::parse(text);
                match item_at(&s.root, &span) {
                    Some(i) => script::apply(text, &[script::replace_item(text, i, &rendered)]),
                    None => text.to_string(),
                }
            }));
            bba_name = Some(x.name.clone());
        }
        (Some(b), None, _) => {
            let l = legacy_name(&b.name);
            let rendered = air::render_branch(false, &format!("set_air_oob = {}", script::quote(&l)));
            history = Some(Box::new(move |text: &str| {
                let s = Script::parse(text);
                script::apply(text, &[script::insert_item(text, &s.root, &rendered, None)])
            }));
        }
        (None, Some(_), _) => {
            let b = format!("{tag}_air_bba");
            let rendered = air::render_branch(true, &format!("set_air_oob = {}", script::quote(&b)));
            history = Some(Box::new(move |text: &str| {
                let s = Script::parse(text);
                script::apply(text, &[script::insert_item(text, &s.root, &rendered, None)])
            }));
            bba_name = Some(b);
        }
        _ => {}
    }
    // the files: that of the DLC's players if missing, that of the others
    // made of its wings
    let bname = bba_name.clone().unwrap_or_else(|| format!("{tag}_air_bba"));
    let lname = legacy.as_ref().map(|l| l.name.clone()).unwrap_or_else(|| legacy_name(&bname));
    if navy::oob_file(cx.st, &tag, &bname).is_none() {
        new_files.push((format!("history/units/{bname}.txt"), empty.clone()));
    }
    if navy::oob_file(cx.st, &tag, &lname).is_none() {
        let bases = wings_of(cx, &bname);
        let made = air::legacy_wings(g.db, &bases, &tag, &|t| g.planes_of(cx, t), g.legacy).0;
        new_files.push((format!("history/units/{lname}.txt"), set_wings(&empty, &made)));
    }
    if history.is_none() && new_files.is_empty() {
        return;
    }
    cx.st.begin(format!("air: orders of battle {bname} and {lname}"));
    for (path, text) in &new_files {
        cx.st.set_text(&tag, path, text.clone());
    }
    if let Some(h) = history {
        if cx.st.file(&tag, &hist).is_none() {
            cx.st.set_text(&tag, &hist, String::new());
        }
        cx.st.edit_text(&tag, &hist, |t| h(t));
    }
    cx.st.commit();
    cx.info(format!("{tag}: air wings in history/units/{bname}.txt, for players without {BBA} in {lname}.txt"));
}

/// The lines that load `bba` for players with By Blood Alone and `legacy`
/// for the others: the `else` inside the `if`, as the game's histories
/// have it (after it, the game drops it).
fn both_lines(bba: &str, legacy: &str) -> String {
    format!("if = {{\n\tlimit = {{ has_dlc = {} }}\n\tset_air_oob = {}\n\telse = {{\n\t\tset_air_oob = {}\n\t}}\n}}", script::quote(BBA), script::quote(bba), script::quote(legacy))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::dlc::Need;

    const H: &str = "capital = 1\nif = {\n\tlimit = { has_dlc = \"By Blood Alone\" }\n\tcreate_equipment_variant = {\n\t\tname = \"Falke\"\n\t\ttype = small_plane_airframe_1\n\t\tmodules = {\n\t\t\tfixed_main_weapon_slot = light_mg_2x\n\t\t\tengine_type_slot = engine_1_1x\n\t\t}\n\t}\n}\n";

    fn d(text: &str) -> Design {
        let s = Script::parse(text);
        let mut out = None;
        crate::content::dlc::walk_flow(&s.root, &Need::default(), &mut |i, need| {
            if i.is("create_equipment_variant") && out.is_none() {
                let b = i.block().unwrap();
                out = Some(Design {
                    name: b.atom("name").unwrap().to_string(),
                    hull: b.atom("type").unwrap().to_string(),
                    modules: b.block("modules").map(|m| m.items.iter().map(|i| (i.key().to_string(), i.text().unwrap().to_string())).collect()).unwrap_or_default(),
                    obsolete: b.yes("obsolete"),
                    need: need.clone(),
                    pkg: "TYR".into(),
                    file: "h".into(),
                    span: i.span.clone(),
                });
            }
        });
        out.unwrap()
    }

    fn run(text: &str, a: &Act) -> String {
        let s = Script::parse(text);
        script::apply(text, &design_edits(text, &s.root, a))
    }

    #[test]
    fn design_edits_go_in_the_branch() {
        // a new design: in the DLC's block, after the last design
        let t = run(H, &Act::NewDesign { name: "Adler".into(), frame: "small_plane_cas_airframe_1".into(), modules: vec![("fixed_main_weapon_slot".into(), "bomb_locks".into())] });
        let s = Script::parse(&t);
        let b = air::branch_block(&s.root, true).unwrap();
        assert_eq!(b.all("create_equipment_variant").count(), 2, "{t}");
        // without a block: one is made
        let t = run("capital = 1\n", &Act::NewDesign { name: "Adler".into(), frame: "small_plane_airframe_1".into(), modules: Vec::new() });
        assert!(t.contains("if = {\n\tlimit = { has_dlc = \"By Blood Alone\" }\n\tcreate_equipment_variant = {\n\t\tname = \"Adler\""), "{t}");
        // a module that makes another role changes the airframe too
        let x = d(H);
        let t = run(H, &Act::Module { d: x.clone(), slot: "fixed_main_weapon_slot".into(), module: "bomb_locks".into(), frame: Some("small_plane_cas_airframe_1".into()) });
        assert!(t.contains("type = small_plane_cas_airframe_1") && t.contains("fixed_main_weapon_slot = bomb_locks"), "{t}");
        // a fallback: in a block of the players without the DLC
        let t = run(H, &Act::NewFallback { name: "Falke".into(), model: "fighter_equipment_1".into() });
        let s = Script::parse(&t);
        let lb = air::branch_block(&s.root, false).expect(&t);
        assert_eq!(lb.block("create_equipment_variant").and_then(|b| b.atom("type")), Some("fighter_equipment_1"));
        // its upgrades
        let mut fb = None;
        crate::content::dlc::walk_flow(&s.root, &Need::default(), &mut |i, need| {
            if i.is("create_equipment_variant") && need.without.contains(BBA) {
                fb = Some(Fallback { name: "Falke".into(), model: "fighter_equipment_1".into(), upgrades: Vec::new(), obsolete: false, need: need.clone(), pkg: "TYR".into(), file: "h".into(), span: i.span.clone() });
            }
        });
        let f = fb.unwrap();
        let t2 = run(&t, &Act::Upgrade { f: f.clone(), upgrade: "plane_gun_upgrade".into(), level: 2 });
        assert!(t2.contains("upgrades = {\n\t\t\tplane_gun_upgrade = 2\n\t\t}"), "{t2}");
        // technologies of the DLC's players in their block
        let t3 = run(H, &Act::GiveTechs(vec!["basic_small_airframe".into()], true));
        let s3 = Script::parse(&t3);
        assert!(air::branch_block(&s3.root, true).and_then(|b| b.block("set_technology")).is_some_and(|b| b.get("basic_small_airframe").is_some()), "{t3}");
    }

    const W: &str = "air_wings = {\n\t3 = {\n\t\tsmall_plane_airframe_1 = { owner = \"TYR\" amount = 50 version_name = \"Falke\" }\n\t\tname = \"1. Staffel\"\n\t\tsmall_plane_airframe_1 = { owner = \"TYR\" amount = 20 version_name = \"Falke\" }\n\t}\n}\n";

    fn wrun(text: &str, a: &WingAct) -> String {
        let s = Script::parse(text);
        script::apply(text, &wing_edits(text, &s.root, a))
    }

    #[test]
    fn wing_edits_keep_the_rest() {
        let spec = WingSpec { equipment: "small_plane_airframe_1".into(), owner: "TYR".into(), creator: None, amount: 30.0, version: Some("Falke".into()), name: Some("2. Staffel".into()) };
        let t = wrun("", &WingAct::Add { state: 7, spec: spec.clone() });
        let b = air::read_wings(&Script::parse(&t).root);
        assert_eq!(air::specs(&b, "TYR"), vec![(7, vec![spec.clone()])], "{t}");
        let t = wrun(W, &WingAct::Add { state: 3, spec: spec.clone() });
        assert_eq!(air::read_wings(&Script::parse(&t).root)[0].wings.len(), 3, "{t}");
        let w = air::read_wings(&Script::parse(W).root);
        let (w0, w1) = (&w[0].wings[0], &w[0].wings[1]);
        let t = wrun(W, &WingAct::Amount(w0.span.clone(), 80.0));
        assert!(t.contains("amount = 80 version_name"), "{t}");
        let t = wrun(W, &WingAct::Name { wing: w1.span.clone(), name_span: None, name: "Zweite".into() });
        let back = air::read_wings(&Script::parse(&t).root);
        assert_eq!((back[0].wings[0].name.as_deref(), back[0].wings[1].name.as_deref()), (Some("1. Staffel"), Some("Zweite")), "{t}");
        let t = wrun(W, &WingAct::Name { wing: w0.span.clone(), name_span: w0.name_span.clone(), name: String::new() });
        assert_eq!(air::read_wings(&Script::parse(&t).root)[0].wings[0].name, None, "{t}");
        // moved: the wing and its name go, the state stays for the other
        let t = wrun(W, &WingAct::Move { wing: w0.span.clone(), name_span: w0.name_span.clone(), to: 9 });
        let back = air::specs(&air::read_wings(&Script::parse(&t).root), "TYR");
        assert_eq!(back.iter().map(|(s, w)| (*s, w.len())).collect::<Vec<_>>(), [(3, 1), (9, 1)], "{t}");
        assert_eq!(back[1].1[0].name.as_deref(), Some("1. Staffel"));
        // the last wing of a state takes the state with it
        let one = air::read_wings(&Script::parse(&t).root);
        let t2 = wrun(&t, &WingAct::Remove { wing: one[1].wings[0].span.clone(), name_span: one[1].wings[0].name_span.clone() });
        assert_eq!(air::read_wings(&Script::parse(&t2).root).iter().map(|b| b.state).collect::<Vec<_>>(), [3], "{t2}");
        // another design
        let t = wrun(W, &WingAct::Design { wing: w1.span.clone(), equipment: "small_plane_cas_airframe_1".into(), version: "Adler".into() });
        let back = air::read_wings(&Script::parse(&t).root);
        assert_eq!((back[0].wings[1].equipment.as_str(), back[0].wings[1].version.as_deref(), back[0].wings[1].amount), ("small_plane_cas_airframe_1", Some("Adler"), Some(20.0)), "{t}");
        // the legacy file's wings set
        let l = set_wings("air_wings = {\n}\n", &[(3, vec![WingSpec { equipment: "fighter_equipment_1".into(), version: None, ..spec }])]);
        assert_eq!(air::specs(&air::read_wings(&Script::parse(&l).root), "TYR")[0].1[0].equipment, "fighter_equipment_1", "{l}");
    }

    use crate::content::gfx::Gfx;
    use crate::content::store::Store;
    use crate::content::ui::{LocCache, MapFacts, Settings};

    const HIST: &str = "history/countries/TYR - Tyrol.txt";

    /// The page's changes, as the page makes them: G worked out of the
    /// store, then the acts applied.
    fn step(st: &mut Store, db: &UnitDb, acts: impl FnOnce(&G) -> Vec<Act>) {
        let (mut gfx, mut settings, facts, loc) = (Gfx::default(), Settings::default(), MapFacts::default(), LocCache::default());
        let (mut msg, mut nav) = (None, None);
        let owned: Owned = Owned::new();
        let have = db.have(["iw_small_airframe", "basic_small_airframe", "armour"].map(String::from).into());
        let legacy = db.have(["early_fighter", "fighter1", "CAS1"].map(String::from).into());
        let (designs, fallbacks) = air::planes(db, st, "TYR");
        let fields = [Airfield { state: 3, level: 1, name: "Bozen".into(), tag: "TYR".into() }];
        let g = G { db, have: &have, legacy: &legacy, owned: &owned, tag: "TYR", designs: &designs, fallbacks: &fallbacks, fields: &fields };
        let a = acts(&g);
        let mut cx = Cx { st, base: None, gfx: &mut gfx, settings: &mut settings, facts: &facts, pkg: "TYR", nation: None, msg: &mut msg, nav: &mut nav, loc: &loc, game: std::path::Path::new("/none"), user_dir: std::path::Path::new("/none"), docs: &Default::default() };
        apply(&mut cx, &g, a);
    }

    fn wings_of(st: &Store, name: &str) -> Vec<(u32, Vec<WingSpec>)> {
        let s = Script::parse(st.text("TYR", &format!("history/units/{name}.txt")).unwrap_or(""));
        air::specs(&air::read_wings(&s.root), "TYR")
    }

    #[test]
    fn the_page_makes_designs_fallbacks_and_wings() {
        let db = crate::content::air::tests::db();
        let mut st = Store::empty(std::env::temp_dir().join("mh-air-none"));
        st.begin("setup");
        st.set_text("TYR", HIST, "capital = 3\n".into());
        st.commit();
        let falke = vec![("fixed_main_weapon_slot".to_string(), "light_mg_2x".to_string()), ("engine_type_slot".to_string(), "engine_1_1x".to_string())];
        step(&mut st, &db, |_| vec![Act::NewDesign { name: "Falke".into(), frame: "small_plane_airframe_0".into(), modules: falke.clone() }]);
        step(&mut st, &db, |_| vec![Act::NewFallback { name: "Falke".into(), model: "fighter_equipment_0".into() }]);
        let (ds, fs) = air::planes(&db, &st, "TYR");
        assert_eq!((ds.len(), fs.len()), (1, 1), "{}", st.text("TYR", HIST).unwrap());
        assert_eq!((air::branch_of(&ds[0].need), air::branch_of(&fs[0].need)), (Branch::Bba, Branch::Legacy));
        // the orders of battle: two files, loaded by if and else
        step(&mut st, &db, |_| vec![Act::MakeOobs]);
        let lines = air::air_oobs(&st, "TYR");
        assert_eq!(lines.iter().map(|l| (l.name.as_str(), air::branch_of(&l.need))).collect::<Vec<_>>(), [("TYR_air_bba", Branch::Bba), ("TYR_air_legacy", Branch::Legacy)], "{}", st.text("TYR", HIST).unwrap());
        // a wing: the file without the DLC follows with the fallback
        let bba = "history/units/TYR_air_bba.txt".to_string();
        let spec = WingSpec { equipment: "small_plane_airframe_0".into(), owner: "TYR".into(), creator: None, amount: 40.0, version: Some("Falke".into()), name: Some("1. Staffel".into()) };
        step(&mut st, &db, |_| vec![Act::Wings("TYR".into(), bba.clone(), WingAct::Add { state: 3, spec: spec.clone() })]);
        assert_eq!(wings_of(&st, "TYR_air_bba"), vec![(3, vec![spec.clone()])]);
        let legacy = wings_of(&st, "TYR_air_legacy");
        assert_eq!((legacy[0].1[0].equipment.as_str(), legacy[0].1[0].version.as_deref(), legacy[0].1[0].amount), ("fighter_equipment_0", Some("Falke"), 40.0));
        // renamed: the design, its fallback and the wings of both files, one step
        step(&mut st, &db, |g| vec![Act::Rename { d: g.designs[0].clone(), name: "Sperber".into() }]);
        let (ds, fs) = air::planes(&db, &st, "TYR");
        assert_eq!((ds[0].name.as_str(), fs[0].name.as_str()), ("Sperber", "Sperber"));
        assert_eq!((wings_of(&st, "TYR_air_bba")[0].1[0].version.as_deref(), wings_of(&st, "TYR_air_legacy")[0].1[0].version.as_deref()), (Some("Sperber"), Some("Sperber")));
        st.undo();
        assert_eq!(air::planes(&db, &st, "TYR").1[0].name, "Falke", "one undo step");
        assert_eq!(wings_of(&st, "TYR_air_legacy")[0].1[0].version.as_deref(), Some("Falke"));
        // the main armament makes the role: bombs make a CAS airframe
        step(&mut st, &db, |g| vec![Act::Module { d: g.designs[0].clone(), slot: "fixed_main_weapon_slot".into(), module: "bomb_locks".into(), frame: Some(role_frame(g, &g.designs[0].hull, &[("fixed_main_weapon_slot".into(), "bomb_locks".into())])) }]);
        assert_eq!(air::planes(&db, &st, "TYR").0[0].hull, "small_plane_cas_airframe_0");
        // the file without the DLC edited by hand: left as it is
        let t = st.text("TYR", "history/units/TYR_air_legacy.txt").unwrap().replace("amount = 40", "amount = 41");
        st.begin("by hand");
        st.set_text("TYR", "history/units/TYR_air_legacy.txt", t);
        st.commit();
        let w = air::read_wings(&Script::parse(st.text("TYR", &bba).unwrap()).root);
        step(&mut st, &db, |_| vec![Act::Wings("TYR".into(), bba.clone(), WingAct::Amount(w[0].wings[0].span.clone(), 60.0))]);
        assert_eq!(wings_of(&st, "TYR_air_bba")[0].1[0].amount, 60.0);
        assert_eq!(wings_of(&st, "TYR_air_legacy")[0].1[0].amount, 41.0, "hand edits kept");
        step(&mut st, &db, |_| vec![Act::RebuildLegacy]);
        assert_eq!(wings_of(&st, "TYR_air_legacy")[0].1[0].amount, 60.0, "rewritten on asking");
        // the design deleted with its fallback
        step(&mut st, &db, |g| vec![Act::Delete { d: g.designs[0].clone() }]);
        assert_eq!(air::planes(&db, &st, "TYR").0.len() + air::planes(&db, &st, "TYR").1.len(), 0, "{}", st.text("TYR", HIST).unwrap());
    }

    #[test]
    fn one_file_for_all_is_split() {
        let db = crate::content::air::tests::db();
        let mut st = Store::empty(std::env::temp_dir().join("mh-air-none"));
        st.begin("setup");
        st.set_text("TYR", HIST, "capital = 3\nset_air_oob = \"TYR_air\"\n".into());
        st.set_text("TYR", "history/units/TYR_air.txt", "air_wings = {\n\t3 = {\n\t\tsmall_plane_airframe_1 = { owner = \"TYR\" amount = 30 }\n\t}\n}\n".into());
        st.commit();
        step(&mut st, &db, |_| vec![Act::MakeOobs]);
        let lines = air::air_oobs(&st, "TYR");
        assert_eq!(lines.iter().map(|l| (l.name.as_str(), air::branch_of(&l.need))).collect::<Vec<_>>(), [("TYR_air", Branch::Bba), ("TYR_air_legacy", Branch::Legacy)], "{}", st.text("TYR", HIST).unwrap());
        assert_eq!(wings_of(&st, "TYR_air_legacy")[0].1[0].equipment, "fighter_equipment_1", "the plane its airframe stands for");
        assert!(st.undo().is_some());
        assert_eq!(air::air_oobs(&st, "TYR").len(), 1, "one undo step");
    }
}
