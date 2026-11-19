//! The air force of a nation: plane designs, made of an airframe and
//! modules as in the plane designer of By Blood Alone, a fallback of each
//! for players without that DLC, and the air wings it starts with, at the
//! air bases of the map.
//!
//! - A design is a `create_equipment_variant` of the nation's history with
//!   an airframe (`type = small_plane_airframe_1`) and the module of each
//!   slot (`modules = { fixed_main_weapon_slot = light_mg_2x
//!   engine_type_slot = engine_1_1x }`). The airframes and their slots are
//!   modular equipment as the ship hulls are ([`super::navy`]); the planes'
//!   modules exist only with By Blood Alone (their `equipment_modules` block
//!   has `limit = { has_dlc = "By Blood Alone" }`), so the game's histories
//!   write the designs inside `if = { limit = { has_dlc = "By Blood Alone" }
//!   }`, and so does the Air page.
//! - What a plane is (fighter, close air support, naval bomber ...) is the
//!   module of its family's `type_override` slot (the main armament): the
//!   first of its `add_equipment_type` its family allows (`allowed_types`),
//!   else the family's type. The game makes a family of each role
//!   (`duplicate_archetypes`: `small_plane_cas_airframe` of
//!   `small_plane_airframe`, with each model, `small_plane_cas_airframe_1`
//!   ...), and a design is of the airframe of its role: bombs in the main
//!   armament of a small airframe make `type = small_plane_cas_airframe_1`.
//! - Its stats are those of the ship designer (the airframe's, plus each
//!   module's, the average of `add_average_stats`, times the
//!   `multiply_stats`). The engines give thrust, everything weighs: weight
//!   over thrust is a design the game's designer refuses ("Weight exceeds
//!   Thrust"); bombs weigh more on some missions (`mission_type_stats`).
//! - A fallback is a design of the planes of before the designer (the
//!   models without modules, `fighter_equipment_1`, `CAS_equipment_1` ...,
//!   of the research of players without By Blood Alone) with the same name,
//!   inside `if = { limit = { NOT = { has_dlc = "By Blood Alone" } } }`: a
//!   model and the levels of its `upgrades` (`plane_gun_upgrade = 2`), as
//!   the game's histories (Italy) do.
//! - The air wings are in a `history/units` file the history loads with
//!   `set_air_oob = "<file>"`: `air_wings = { <state> = { <equipment> = {
//!   owner = TAG amount = 24 version_name = "<design>" } name = "<wing>" }
//!   }`, a `name` naming the wing before it. The game's histories load one
//!   file with By Blood Alone and one of the planes without designer
//!   without it (`ENG_1936_air_bba`, `ENG_1936_air_legacy`); the Air page
//!   writes the second from the first, a wing of a design becoming one of
//!   its fallback.
//! - A wing is at a state with an air base (a building of the state, set in
//!   the map editor); each level of air base takes 200 planes
//!   (`AIRBASE_CAPACITY_MULT`).

use std::collections::{BTreeMap, BTreeSet};

use super::dlc::{self, Need, Owned};
use super::navy::{self, Design, FleetProblem, Hull, OobLine, Problem, ShipStats};
use super::script::{self, Block, Span};
use super::store::Store;
use super::units::{Equipment, Have, Stats, UnitDb};
use super::validate::Severity;

pub const BBA: &str = "By Blood Alone";

/// What a plane can be in combat (a family's type).
pub const PLANE_TYPES: [&str; 12] = ["fighter", "interceptor", "cas", "naval_bomber", "suicide", "tactical_bomber", "heavy_fighter", "scout_plane", "strategic_bomber", "maritime_patrol_plane", "air_transport", "missile"];

/// The designer's families of airframes, in its order.
pub const FRAMES: [&str; 4] = ["small_plane_airframe", "cv_small_plane_airframe", "medium_plane_airframe", "large_plane_airframe"];

/// Planes an air base takes per level (`AIRBASE_CAPACITY_MULT`).
pub const PER_LEVEL: u32 = 200;

/// Where to place an air base when there is none.
pub const PLACE_AIRBASE: &str = "set one in the map editor: select a state, its buildings, air_base";

/// A player with what `owned` has, and By Blood Alone.
pub fn with_bba(owned: &Owned) -> Owned {
    let mut o = owned.clone();
    o.insert(BBA.to_string());
    o
}

/// A player with what `owned` has, but not By Blood Alone.
pub fn without_bba(owned: &Owned) -> Owned {
    let mut o = owned.clone();
    o.remove(BBA);
    o
}

/// For whom a line of the history is: players with By Blood Alone, without
/// it, or both.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Branch {
    Bba,
    Legacy,
    Both,
}

pub fn branch_of(need: &Need) -> Branch {
    if need.with.contains(BBA) {
        Branch::Bba
    } else if need.without.contains(BBA) {
        Branch::Legacy
    } else {
        Branch::Both
    }
}

/// The plane designs and fallbacks of a tag.
pub type PlanesOf<'a> = dyn Fn(&str) -> (Vec<Design>, Vec<Fallback>) + 'a;

/// A fallback: a design of a plane without modules for players without By
/// Blood Alone.
#[derive(Clone, Debug, PartialEq)]
pub struct Fallback {
    pub name: String,
    /// The model (`fighter_equipment_1`).
    pub model: String,
    /// The levels of its upgrades as written.
    pub upgrades: Vec<(String, i64)>,
    pub obsolete: bool,
    pub need: Need,
    pub pkg: String,
    pub file: String,
    pub span: Span,
}

impl UnitDb {
    /// The family a model is of, through the families the game makes of
    /// others (`small_plane_cas_airframe_1` → `small_plane_airframe`).
    pub fn base_family(&self, id: &str) -> String {
        let a = if self.equipment.get(id).is_some_and(|e| e.is_archetype) { id } else { self.archetype_of(id) };
        self.duplicates.get(a).map(|d| d.archetype.clone()).unwrap_or_else(|| a.to_string())
    }

    /// The types of a model: its own, else its archetype's.
    pub fn types_of(&self, id: &str) -> Vec<String> {
        let Some(e) = self.equipment.get(id) else { return Vec::new() };
        if !e.types.is_empty() {
            return e.types.clone();
        }
        e.archetype.as_deref().and_then(|a| self.equipment.get(a)).map(|a| a.types.clone()).unwrap_or_default()
    }

    fn is_plane_family(&self, arch: &str) -> bool {
        self.equipment.get(arch).is_some_and(|a| a.types.iter().any(|t| PLANE_TYPES.contains(&t.as_str())))
    }

    /// An airframe of the plane designer (a model with modules, not a
    /// family).
    pub fn is_airframe(&self, id: &str) -> bool {
        self.equipment.get(id).is_some_and(|e| !e.is_archetype && e.modular && self.is_plane_family(self.archetype_of(id)))
    }

    /// A plane of before the designer (a model without modules).
    pub fn is_old_plane(&self, id: &str) -> bool {
        self.equipment.get(id).is_some_and(|e| !e.is_archetype && !e.modular && e.archetype.as_deref().is_some_and(|a| self.is_plane_family(a)))
    }

    /// The airframes the designer lists (those of the base families, the
    /// role's airframe follows from the main armament), by family and year.
    pub fn airframes(&self) -> Vec<&Equipment> {
        let mut v: Vec<&Equipment> = self.equipment.values().filter(|e| !e.derived && self.is_airframe(&e.id)).collect();
        let fam = |e: &Equipment| {
            let a = e.archetype.as_deref().unwrap_or("");
            (FRAMES.iter().position(|x| *x == a).unwrap_or(FRAMES.len()), a.to_string())
        };
        v.sort_by(|a, b| fam(a).cmp(&fam(b)).then(a.year.total_cmp(&b.year)).then(a.id.cmp(&b.id)));
        v
    }

    /// The planes of before the designer, by type and year.
    pub fn old_planes(&self) -> Vec<&Equipment> {
        let mut v: Vec<&Equipment> = self.equipment.values().filter(|e| self.is_old_plane(&e.id)).collect();
        let ty = |e: &Equipment| self.types_of(&e.id).first().map(|t| PLANE_TYPES.iter().position(|x| x == t).unwrap_or(PLANE_TYPES.len())).unwrap_or(PLANE_TYPES.len());
        v.sort_by(|a, b| ty(a).cmp(&ty(b)).then(a.archetype.cmp(&b.archetype)).then(a.year.total_cmp(&b.year)).then(a.id.cmp(&b.id)));
        v
    }

    /// What a design is: the first type the module of the family's
    /// `type_override` slot adds that the family allows, else the family's
    /// type.
    pub fn plane_role(&self, hull: &Hull, fitted: &BTreeMap<String, String>) -> Option<String> {
        let base = self.equipment.get(&self.base_family(&hull.id))?;
        if let Some(slot) = &base.type_override
            && let Some(m) = fitted.get(slot).and_then(|m| self.modules.get(m))
            && let Some(t) = m.add_types.iter().find(|t| base.allowed_types.contains(t))
        {
            return Some(t.clone());
        }
        base.types.first().cloned()
    }

    /// The airframe of a role of the family of `frame`, of its generation:
    /// (`small_plane_airframe_1`, cas) → `small_plane_cas_airframe_1`.
    pub fn airframe_for(&self, frame: &str, role: &str) -> Option<String> {
        let arch = self.archetype_of(frame).to_string();
        let suffix = frame.strip_prefix(&format!("{arch}_"))?;
        let base = self.base_family(frame);
        let mut fams: Vec<&str> = vec![base.as_str()];
        fams.extend(self.duplicates.iter().filter(|(_, d)| d.archetype == base).map(|(k, _)| k.as_str()));
        let fam = fams.iter().find(|f| self.equipment.get(**f).is_some_and(|e| e.types.iter().any(|t| t == role)))?;
        let id = format!("{fam}_{suffix}");
        self.equipment.contains_key(&id).then_some(id)
    }

    /// The airframe of the base family of the same generation
    /// (`small_plane_cas_airframe_1` → `small_plane_airframe_1`).
    pub fn base_airframe(&self, frame: &str) -> String {
        let arch = self.archetype_of(frame).to_string();
        let base = self.base_family(frame);
        match frame.strip_prefix(&format!("{arch}_")) {
            Some(s) if self.equipment.contains_key(&format!("{base}_{s}")) => format!("{base}_{s}"),
            _ => frame.to_string(),
        }
    }

    /// The roles a design on an airframe can take: the types its family
    /// allows that a module of the main armament gives, the family's own
    /// first.
    pub fn roles_of(&self, frame: &str) -> Vec<String> {
        let Some(base) = self.equipment.get(&self.base_family(frame)) else { return Vec::new() };
        let mut v: Vec<String> = base.types.clone();
        let Some(slot) = base.type_override.as_ref().and_then(|k| self.hull(frame)?.slot(k).cloned()) else { return v };
        for m in self.modules_for(&slot) {
            for t in self.modules.get(m).map(|m| m.add_types.as_slice()).unwrap_or_default() {
                if base.allowed_types.contains(t) && !v.contains(t) {
                    v.push(t.clone());
                }
            }
        }
        v
    }

    /// The modules of a new design of a role on an airframe: the main
    /// armament the newest the nation has of that role, each slot that must
    /// be filled the newest it has (of two of a year, the fewer engines),
    /// the rest empty; `keep`: modules of a design before, kept where they
    /// fit and the nation has them.
    pub fn plane_modules(&self, hull: &Hull, have: &Have, role: Option<&str>, keep: &BTreeMap<String, String>) -> Vec<(String, String)> {
        let main = self.equipment.get(&self.base_family(&hull.id)).and_then(|b| b.type_override.clone());
        let pick = |s: &navy::Slot, role: Option<&str>| -> Option<String> {
            self.modules_for(s)
                .into_iter()
                .filter(|m| have.modules.contains(*m) && role.is_none_or(|r| self.modules.get(*m).is_some_and(|md| md.add_types.iter().any(|t| t == r))))
                .max_by(|a, b| self.module_year(a).total_cmp(&self.module_year(b)).then(b.cmp(a)))
                .map(String::from)
        };
        hull.slots
            .iter()
            .map(|s| {
                let is_main = main.as_deref() == Some(s.key.as_str());
                let kept = keep.get(&s.key).filter(|m| (*m == navy::EMPTY && !s.required) || (self.module_fits(s, m) && have.modules.contains(*m))).filter(|m| !is_main || role.is_none_or(|r| self.modules.get(*m).is_some_and(|md| md.add_types.iter().any(|t| t == r))));
                let m = match kept {
                    Some(k) => k.clone(),
                    None if is_main => pick(s, role).or_else(|| pick(s, None)).unwrap_or_else(|| navy::EMPTY.to_string()),
                    None if s.required => pick(s, None).unwrap_or_else(|| navy::EMPTY.to_string()),
                    None => navy::EMPTY.to_string(),
                };
                (s.key.clone(), m)
            })
            .collect()
    }

    /// Weight and thrust: (thrust, weight, the missions on which the
    /// weight is over the thrust with that weight).
    pub fn plane_weight(&self, hull: &Hull, fitted: &BTreeMap<String, String>, stats: &ShipStats) -> (f64, f64, Vec<(String, f64)>) {
        let (thrust, weight) = (stats.get("thrust"), stats.get("weight"));
        let mut per: BTreeMap<String, f64> = BTreeMap::new();
        for (slot, m) in fitted {
            if hull.slot(slot).is_none() {
                continue;
            }
            let Some(md) = self.modules.get(m) else { continue };
            for (missions, add) in &md.missions {
                if let Some(w) = add.get("weight") {
                    for ms in missions {
                        *per.entry(ms.clone()).or_default() += w;
                    }
                }
            }
        }
        let over = per.into_iter().map(|(k, w)| (k, weight + w)).filter(|(_, w)| *w > thrust + 1e-9).collect();
        (thrust, weight, over)
    }

    /// The stats of a plane without modules with upgrades: each level adds
    /// its fraction of the stat.
    pub fn old_plane_stats(&self, model: &str, upgrades: &[(String, i64)]) -> Stats {
        let Some(e) = self.equipment.get(model) else { return Stats::new() };
        let mut s = self.stats_of(e);
        let mut mult: Stats = Stats::new();
        for (u, lv) in upgrades {
            if let Some(up) = self.upgrades.get(u) {
                for (k, v) in &up.stats {
                    *mult.entry(k.clone()).or_default() += v * (*lv).max(0) as f64;
                }
            }
        }
        for (k, v) in mult {
            if let Some(x) = s.get_mut(&k) {
                *x *= 1.0 + v;
            }
        }
        s
    }

    /// The upgrades a model takes: its own, else those of the model it
    /// follows, else its archetype's.
    pub fn upgrades_of(&self, model: &str) -> Vec<String> {
        let mut cur = self.equipment.get(model);
        let mut n = 0;
        while let Some(e) = cur {
            if !e.upgrades.is_empty() {
                return e.upgrades.clone();
            }
            n += 1;
            if n > 16 {
                break;
            }
            cur = e.parent.as_deref().and_then(|p| self.equipment.get(p));
        }
        self.equipment.get(model).and_then(|e| e.archetype.as_deref()).and_then(|a| self.equipment.get(a)).map(|a| a.upgrades.clone()).unwrap_or_default()
    }

    /// The plane without modules a design falls back to: the model its
    /// airframe stands for, if the nation has it without By Blood Alone,
    /// else the newest of its role the nation has (`have`: without By Blood
    /// Alone), else the model its airframe stands for, else the oldest of
    /// its role.
    pub fn fallback_model(&self, frame: &str, role: Option<&str>, have: &Have) -> Option<String> {
        let own = self.equipment.get(frame).and_then(|e| e.legacy.clone()).filter(|m| self.equipment.contains_key(m));
        if let Some(m) = &own
            && have.equipment.contains(m)
        {
            return own;
        }
        let role = role.map(String::from).or_else(|| self.types_of(frame).first().cloned());
        let of_role: Vec<&Equipment> = self.old_planes().into_iter().filter(|e| role.as_ref().is_some_and(|r| self.types_of(&e.id).contains(r))).collect();
        let best = of_role.iter().filter(|e| have.equipment.contains(&e.id)).max_by(|a, b| a.year.total_cmp(&b.year).then(b.id.cmp(&a.id)));
        best.map(|e| e.id.clone()).or(own).or_else(|| of_role.iter().min_by(|a, b| a.year.total_cmp(&b.year).then(a.id.cmp(&b.id))).map(|e| e.id.clone()))
    }

    /// The checks of a plane design of a nation that has `have` (with By
    /// Blood Alone) at the start; `others`: its plane designs.
    pub fn check_plane(&self, d: &Design, have: &Have, others: &[Design], tag: &str, name: &dyn Fn(&str) -> String) -> Vec<Problem> {
        let mut out = Vec::new();
        let mut add = |severity: Severity, message: String, slot: Option<&str>| out.push(Problem { severity, message, slot: slot.map(String::from) });
        if d.name.trim().is_empty() {
            add(Severity::Error, "no name: a design needs name = \"...\" (the air wings name it in version_name)".into(), None);
        } else if others.iter().filter(|o| o.name == d.name && o.hull == d.hull).count() > 1 {
            add(Severity::Warning, format!("{tag} has more than one design \"{}\" on this airframe: the air wings get the first", d.name), None);
        }
        match branch_of(&d.need) {
            Branch::Legacy => {
                add(Severity::Error, format!("it is only for players without {BBA}, who have no plane designer: the game cannot make it (a fallback is of a plane without modules)"), None);
                return out;
            }
            Branch::Both => add(Severity::Warning, format!("not inside if = {{ limit = {{ has_dlc = \"{BBA}\" }} }}: the planes' modules are {BBA}'s, players without it get a design the game cannot make"), None),
            Branch::Bba => {}
        }
        let Some(hull) = self.hull(&d.hull).filter(|_| self.is_airframe(&d.hull)) else {
            let m = if d.hull.is_empty() {
                "no airframe: a design needs type = <airframe> (small_plane_airframe_1 ...)".to_string()
            } else if self.equipment.contains_key(&d.hull) {
                format!("{} is not an airframe of the plane designer (a family, or a plane without modules)", d.hull)
            } else {
                format!("{} is not equipment of the game or the mod", d.hull)
            };
            add(Severity::Error, m, None);
            return out;
        };
        if !have.equipment.contains(&d.hull) {
            let techs = self.unlocked_text(&[d.hull.as_str()], name, 2);
            add(Severity::Error, if techs.is_empty() { format!("{tag} cannot build the {} airframe", name(&d.hull)) } else { format!("{tag} cannot build the {} airframe at the start: it comes with {techs}", name(&d.hull)) }, None);
        }
        for p in self.check_slots(&hull, &d.modules, have, tag, "airframe", name) {
            add(p.severity, p.message, p.slot.as_deref());
        }
        let fitted = self.fitted(&hull, &d.modules);
        let stats = self.ship_stats(&hull, &fitted);
        if let Some(role) = self.plane_role(&hull, &fitted) {
            let own = self.types_of(&d.hull);
            if !own.contains(&role) {
                let slot = self.equipment.get(&self.base_family(&d.hull)).and_then(|b| b.type_override.clone());
                let by = slot.as_ref().and_then(|s| fitted.get(s)).map(|m| name(m)).unwrap_or_default();
                let right = self.airframe_for(&d.hull, &role);
                let m = match &right {
                    Some(r) => format!("its main armament ({by}) makes it a {}, but type = {} is a {} airframe: type = {r}", role_text(name, &role), d.hull, own.first().map(|t| role_text(name, t)).unwrap_or_default()),
                    None => format!("its main armament ({by}) makes it a {}, but type = {} is a {} airframe", role_text(name, &role), d.hull, own.first().map(|t| role_text(name, t)).unwrap_or_default()),
                };
                add(Severity::Warning, m, slot.as_deref());
            }
        }
        let (thrust, weight, over) = self.plane_weight(&hull, &fitted, &stats);
        if weight > thrust + 1e-9 {
            add(Severity::Error, format!("weight exceeds thrust ({} > {}): the game's designer refuses it; a stronger engine, more engines or lighter modules", fmt(weight), fmt(thrust)), Some("engine_type_slot"));
        } else if !over.is_empty() {
            let v: Vec<String> = over.iter().map(|(m, w)| format!("{} ({})", name(m), fmt(*w))).collect();
            add(Severity::Warning, format!("weight exceeds thrust ({}) on these missions: {}", fmt(thrust), v.join(", ")), None);
        }
        out
    }

    /// The checks of a fallback of a nation that has `have` without By
    /// Blood Alone; `design`: the design of the same name, `others`: its
    /// fallbacks.
    pub fn check_fallback(&self, f: &Fallback, have: &Have, design: Option<&Design>, others: &[Fallback], tag: &str, name: &dyn Fn(&str) -> String) -> Vec<Problem> {
        let mut out = Vec::new();
        let mut add = |severity: Severity, message: String| out.push(Problem { severity, message, slot: None });
        if f.name.trim().is_empty() {
            add(Severity::Error, "no name: a design needs name = \"...\"".into());
        } else if others.iter().filter(|o| o.name == f.name).count() > 1 {
            add(Severity::Warning, format!("{tag} has more than one plane \"{}\" without designer: the air wings get the first", f.name));
        }
        match branch_of(&f.need) {
            Branch::Bba => add(Severity::Warning, format!("only for players with {BBA}, who use the designer's planes: it does nothing for those without it")),
            Branch::Both => {
                if design.is_some() {
                    add(Severity::Warning, format!("not inside if = {{ limit = {{ NOT = {{ has_dlc = \"{BBA}\" }} }} }}: players with {BBA} get it too, beside the design of the same name"));
                }
            }
            Branch::Legacy => {}
        }
        if !self.equipment.contains_key(&f.model) {
            add(Severity::Error, format!("{} is not equipment of the game or the mod", f.model));
            return out;
        }
        if !self.is_old_plane(&f.model) {
            add(Severity::Error, format!("{} is not a plane without modules (fighter_equipment_1, CAS_equipment_1 ...)", f.model));
            return out;
        }
        if !have.equipment.contains(&f.model) {
            let techs = self.unlocked_text(&[f.model.as_str()], name, 2);
            add(Severity::Error, if techs.is_empty() { format!("{tag} cannot build the {} without {BBA}", name(&f.model)) } else { format!("{tag} cannot build the {} at the start without {BBA}: it comes with {techs}", name(&f.model)) });
        }
        let allowed = self.upgrades_of(&f.model);
        for (u, lv) in &f.upgrades {
            match self.upgrades.get(u) {
                None => add(Severity::Error, format!("upgrades: {u} is not an upgrade of the game or the mod")),
                Some(up) => {
                    if !allowed.contains(u) {
                        add(Severity::Error, format!("upgrades: the {} takes no {} ({})", name(&f.model), name(u), if allowed.is_empty() { "it takes none".to_string() } else { allowed.join(", ") }));
                    } else if *lv < 0 || *lv as u32 > up.max_level {
                        add(Severity::Error, format!("upgrades: {u} = {lv}, it goes from 0 to {}", up.max_level));
                    }
                }
            }
        }
        if let Some(d) = design
            && let Some(h) = self.hull(&d.hull)
            && let Some(role) = self.plane_role(&h, &self.fitted(&h, &d.modules))
        {
            let own = self.types_of(&f.model);
            if !own.contains(&role) {
                add(Severity::Warning, format!("the design \"{}\" is a {}, its fallback a {}", d.name, role_text(name, &role), own.first().map(|t| role_text(name, t)).unwrap_or_default()));
            }
        }
        out
    }
}

/// A role's name ("Tactical Bomber", not the localisation's "Tactical
/// Bomber models").
fn role_text(name: &dyn Fn(&str) -> String, role: &str) -> String {
    let n = name(role);
    n.strip_suffix(" models").map(String::from).unwrap_or(n)
}

fn fmt(v: f64) -> String {
    if (v - v.round()).abs() < 0.005 { format!("{}", v.round() as i64) } else { format!("{v:.2}") }
}

// ------------------------------------------------------------- designs

/// The plane designs and the fallbacks of a nation's history, in the order
/// written.
pub fn planes(db: &UnitDb, st: &Store, tag: &str) -> (Vec<Design>, Vec<Fallback>) {
    let (mut ds, mut fs) = (Vec::new(), Vec::new());
    let Some(p) = st.package(tag) else { return (ds, fs) };
    for (path, file) in &p.files {
        if !path.starts_with("history/countries/") {
            continue;
        }
        let Some(s) = &file.script else { continue };
        dlc::walk_flow(&s.root, &Need::default(), &mut |i, need| {
            if !i.is("create_equipment_variant") {
                return;
            }
            let Some(b) = i.block() else { return };
            let Some(ty) = b.atom("type") else { return };
            // no name, a design company: the company of the model's own
            // design (an industrial organisation's), not a design
            if b.atom("name").is_none() && b.get("design_team").is_some() {
                return;
            }
            if db.is_airframe(ty) || !db.equipment.contains_key(ty) && ty.contains("_airframe") {
                let modules = b.block("modules").map(|m| m.items.iter().filter_map(|i| Some((i.key.as_ref()?.text.clone(), i.text()?.to_string()))).collect()).unwrap_or_default();
                ds.push(Design { name: b.atom("name").unwrap_or("").to_string(), hull: ty.to_string(), modules, obsolete: b.yes("obsolete"), need: need.clone(), pkg: tag.to_string(), file: path.clone(), span: i.span.clone() });
            } else if db.is_old_plane(ty) {
                let upgrades = b.block("upgrades").map(|u| u.items.iter().filter_map(|i| Some((i.key.as_ref()?.text.clone(), i.text()?.parse::<f64>().ok()? as i64))).collect()).unwrap_or_default();
                fs.push(Fallback { name: b.atom("name").unwrap_or("").to_string(), model: ty.to_string(), upgrades, obsolete: b.yes("obsolete"), need: need.clone(), pkg: tag.to_string(), file: path.clone(), span: i.span.clone() });
            }
        });
    }
    (ds, fs)
}

/// The text of a fallback.
pub fn render_fallback(name: &str, model: &str, upgrades: &[(String, i64)], obsolete: bool) -> String {
    let mut s = format!("create_equipment_variant = {{\n\tname = {}\n\ttype = {model}\n", script::quote(name));
    if !upgrades.is_empty() {
        s.push_str("\tupgrades = {\n");
        for (k, v) in upgrades {
            s.push_str(&format!("\t\t{k} = {v}\n"));
        }
        s.push_str("\t}\n");
    }
    if obsolete {
        s.push_str("\tobsolete = yes\n");
    }
    s.push('}');
    s
}

// ------------------------------------------------------------- air wings

/// An air wing of an order of battle.
#[derive(Clone, Debug, PartialEq)]
pub struct Wing {
    pub equipment: String,
    pub amount: Option<f64>,
    pub owner: Option<String>,
    pub creator: Option<String>,
    /// The design (`version_name`).
    pub version: Option<String>,
    /// The `name = "..."` after it.
    pub name: Option<String>,
    pub span: Span,
    pub name_span: Option<Span>,
}

/// The wings at a state.
#[derive(Clone, Debug, PartialEq)]
pub struct WingBase {
    pub state: u32,
    pub wings: Vec<Wing>,
    pub span: Span,
}

/// The wings of an order of battle (`air_wings = { <state> = { ... } }`).
pub fn read_wings(root: &Block) -> Vec<WingBase> {
    let mut out = Vec::new();
    for aw in root.blocks("air_wings") {
        for st in &aw.items {
            let (Some(k), Some(b)) = (st.key.as_ref(), st.block()) else { continue };
            let Ok(state) = k.text.parse::<u32>() else { continue };
            let mut wings: Vec<Wing> = Vec::new();
            for i in &b.items {
                if i.is("name") {
                    if let Some(w) = wings.last_mut() {
                        w.name = i.text().map(String::from);
                        w.name_span = Some(i.span.clone());
                    }
                    continue;
                }
                if i.is("ace") {
                    continue;
                }
                let Some(eb) = i.block() else { continue };
                wings.push(Wing {
                    equipment: i.key().to_string(),
                    amount: eb.num("amount"),
                    owner: eb.atom("owner").map(String::from),
                    creator: eb.atom("creator").map(String::from),
                    version: eb.atom("version_name").map(String::from),
                    name: None,
                    span: i.span.clone(),
                    name_span: None,
                });
            }
            out.push(WingBase { state, wings, span: st.span.clone() });
        }
    }
    out
}

/// The `set_air_oob` lines of a nation's history.
pub fn air_oobs(st: &Store, tag: &str) -> Vec<OobLine> {
    let mut out = Vec::new();
    let Some(p) = st.package(tag) else { return out };
    for (path, file) in &p.files {
        if !path.starts_with("history/countries/") {
            continue;
        }
        let Some(s) = &file.script else { continue };
        dlc::walk_flow(&s.root, &Need::default(), &mut |i, need| {
            if i.is("set_air_oob")
                && let Some(v) = i.text()
            {
                out.push(OobLine { name: v.to_string(), need: need.clone(), file: path.clone(), span: i.span.clone() });
            }
        });
    }
    out
}

/// The air order of battle of a nation for a player with `owned`: its
/// line, and the package and path of its file (None: not in the content).
pub fn air_oob(st: &Store, tag: &str, owned: &Owned) -> Option<(OobLine, Option<(String, String)>)> {
    let line = air_oobs(st, tag).into_iter().find(|l| l.need.holds(owned))?;
    let file = navy::oob_file(st, tag, &line.name);
    Some((line, file))
}

/// An air base of the map: a state with the building.
#[derive(Clone, Debug, PartialEq)]
pub struct Airfield {
    pub state: u32,
    pub level: u32,
    pub name: String,
    /// The nation of the state ("" for none).
    pub tag: String,
}

impl Airfield {
    pub fn label(&self) -> String {
        if self.name.is_empty() { format!("state {}", self.state) } else { format!("{} (state {})", self.name, self.state) }
    }
}

/// The checks of the air wings of a nation. `legacy`: the file is that of
/// players without By Blood Alone; `planes_of`: the designs and fallbacks
/// of a tag (the owner of a wing may be another nation).
pub fn check_wings(db: &UnitDb, bases: &[WingBase], tag: &str, fields: &[Airfield], legacy: bool, planes_of: &PlanesOf, name: &dyn Fn(&str) -> String) -> Vec<FleetProblem> {
    let mut out = Vec::new();
    let mut add = |severity: Severity, message: String, span: &Span| out.push(FleetProblem { severity, message, span: span.clone() });
    let own: Vec<&Airfield> = fields.iter().filter(|f| f.tag == tag).collect();
    let mut cache: BTreeMap<String, (Vec<Design>, Vec<Fallback>)> = BTreeMap::new();
    let mut seen: BTreeMap<u32, usize> = BTreeMap::new();
    for b in bases {
        *seen.entry(b.state).or_default() += 1;
        match fields.iter().find(|f| f.state == b.state) {
            None if fields.is_empty() => add(Severity::Error, format!("state {}: no air base, and the map has none yet ({PLACE_AIRBASE})", b.state), &b.span),
            None if own.is_empty() => add(Severity::Error, format!("state {}: no air base, and {tag} has none ({PLACE_AIRBASE})", b.state), &b.span),
            None => add(Severity::Error, format!("state {}: no air base ({tag} has {})", b.state, own.iter().map(|f| f.label()).collect::<Vec<_>>().join(", ")), &b.span),
            Some(f) => {
                if f.tag != tag {
                    add(Severity::Warning, format!("{}: an air base of {}, not of {tag} (the wings need the right to use it)", f.label(), if f.tag.is_empty() { "no nation" } else { f.tag.as_str() }), &b.span);
                }
                let planes: f64 = b.wings.iter().filter_map(|w| w.amount).sum();
                let cap = f.level * PER_LEVEL;
                if planes > cap as f64 {
                    add(Severity::Warning, format!("{}: {} planes, its air base (level {}) takes {cap}; the wings over it fight worse", f.label(), fmt(planes), f.level), &b.span);
                }
            }
        }
        if b.wings.is_empty() {
            add(Severity::Warning, format!("state {}: no wings", b.state), &b.span);
        }
        for w in &b.wings {
            let who = match &w.name {
                Some(n) => format!("wing \"{n}\""),
                None => format!("a wing of {}", w.version.clone().unwrap_or_else(|| w.equipment.clone())),
            };
            if !db.equipment.contains_key(&w.equipment) {
                add(Severity::Error, format!("{who}: {} is not equipment of the game or the mod", w.equipment), &w.span);
                continue;
            }
            match w.amount {
                None => add(Severity::Error, format!("{who} has no amount (amount = 24: how many planes)"), &w.span),
                Some(a) if a < 1.0 => add(Severity::Error, format!("{who}: amount = {}, a wing needs planes", fmt(a)), &w.span),
                _ => {}
            }
            let owner = match &w.owner {
                Some(o) => o.clone(),
                None => {
                    add(Severity::Warning, format!("{who} has no owner (owner = {tag}: whose wing it is)"), &w.span);
                    tag.to_string()
                }
            };
            // whose design it is: its creator, else its owner
            let owner = w.creator.clone().unwrap_or(owner);
            let airframe = db.is_airframe(&w.equipment);
            if !airframe && !db.is_old_plane(&w.equipment) {
                add(Severity::Error, format!("{who}: {} is no plane", w.equipment), &w.span);
                continue;
            }
            if legacy && airframe {
                add(Severity::Error, format!("{who}: {} is an airframe of the plane designer, which players without {BBA} do not have: a plane without modules ({})", w.equipment, db.equipment.get(&w.equipment).and_then(|e| e.legacy.clone()).unwrap_or_else(|| "fighter_equipment_1 ...".into())), &w.span);
                continue;
            }
            if !legacy && !airframe && db.need_of(&w.equipment).without.contains(BBA) {
                add(Severity::Warning, format!("{who}: {} is a plane of before the designer, of players without {BBA}", w.equipment), &w.span);
            }
            let (ds, fs) = cache.entry(owner.clone()).or_insert_with(|| planes_of(&owner));
            let Some(v) = &w.version else {
                if airframe {
                    add(Severity::Warning, format!("{who} has no version_name: it gets the airframe's default design"), &w.span);
                }
                continue;
            };
            if airframe {
                let family = db.base_family(&w.equipment);
                let found = ds.iter().find(|d| d.name == *v && d.hull == w.equipment).or_else(|| {
                    let d = ds.iter().find(|d| d.name == *v && db.base_family(&d.hull) == family)?;
                    add(Severity::Warning, format!("{who}: the design \"{v}\" of {owner} is on the {} airframe, not {} ({} = {{ ... }})", name(&d.hull), name(&w.equipment), d.hull), &w.span);
                    Some(d)
                });
                if found.is_none() {
                    let on: Vec<String> = ds.iter().filter(|d| d.hull == w.equipment).map(|d| format!("\"{}\"", d.name)).collect();
                    let has = if on.is_empty() { format!("{owner} has no design on it") } else { format!("{owner} has {}", on.join(", ")) };
                    add(Severity::Error, format!("{who}: {owner} has no design \"{v}\" on the {} airframe ({has})", name(&w.equipment)), &w.span);
                }
            } else if !fs.iter().any(|f| f.name == *v && f.model == w.equipment) {
                let on: Vec<String> = fs.iter().filter(|f| f.model == w.equipment).map(|f| format!("\"{}\"", f.name)).collect();
                let has = if on.is_empty() { String::new() } else { format!(" ({owner} has {})", on.join(", ")) };
                add(Severity::Error, format!("{who}: {owner} has no plane \"{v}\" of the {}{has}", name(&w.equipment)), &w.span);
            }
            if owner != tag {
                add(Severity::Info, format!("{who} is of a design of {owner}"), &w.span);
            }
        }
    }
    for b in bases {
        if seen.get(&b.state).is_some_and(|n| *n > 1) {
            add(Severity::Warning, format!("state {} is written more than once in air_wings", b.state), &b.span);
            seen.remove(&b.state);
        }
    }
    out
}

/// A wing as written, without where it is in the file.
#[derive(Clone, Debug, PartialEq)]
pub struct WingSpec {
    pub equipment: String,
    pub owner: String,
    /// The nation whose design it is, when not the owner.
    pub creator: Option<String>,
    pub amount: f64,
    pub version: Option<String>,
    pub name: Option<String>,
}

/// The wings of the bases as specs (`tag`: the owner of those without).
pub fn specs(bases: &[WingBase], tag: &str) -> Vec<(u32, Vec<WingSpec>)> {
    bases
        .iter()
        .map(|b| (b.state, b.wings.iter().map(|w| WingSpec { equipment: w.equipment.clone(), owner: w.owner.clone().unwrap_or_else(|| tag.to_string()), creator: w.creator.clone(), amount: w.amount.unwrap_or(0.0), version: w.version.clone(), name: w.name.clone() }).collect()))
        .collect()
}

/// The wings for players without By Blood Alone made of those of players
/// with it: a wing of a design becomes one of its fallback, else of the
/// plane without modules it stands for (`have`: what the nation has without
/// By Blood Alone); planes without modules stay. Also what could not be
/// made.
pub fn legacy_wings(db: &UnitDb, bases: &[WingBase], tag: &str, planes_of: &PlanesOf, have: &Have) -> (Vec<(u32, Vec<WingSpec>)>, Vec<String>) {
    let mut out = Vec::new();
    let mut skipped = Vec::new();
    let mut cache: BTreeMap<String, (Vec<Design>, Vec<Fallback>)> = BTreeMap::new();
    for (state, wings) in specs(bases, tag) {
        let mut v = Vec::new();
        for w in wings {
            if !db.is_airframe(&w.equipment) {
                v.push(w);
                continue;
            }
            let whose = w.creator.clone().unwrap_or_else(|| w.owner.clone());
            let (ds, fs) = cache.entry(whose.clone()).or_insert_with(|| planes_of(&whose));
            let fb = w.version.as_ref().and_then(|n| fs.iter().find(|f| f.name == *n && branch_of(&f.need) != Branch::Bba));
            if let Some(f) = fb {
                v.push(WingSpec { equipment: f.model.clone(), version: Some(f.name.clone()), ..w });
                continue;
            }
            let d = w.version.as_ref().and_then(|n| ds.iter().find(|d| d.name == *n && d.hull == w.equipment));
            let role = d.and_then(|d| db.hull(&d.hull).and_then(|h| db.plane_role(&h, &db.fitted(&h, &d.modules))));
            match db.fallback_model(&w.equipment, role.as_deref(), have) {
                Some(m) => v.push(WingSpec { equipment: m, version: None, ..w }),
                None => skipped.push(format!("state {state}: no plane without modules for {}", w.version.clone().unwrap_or(w.equipment.clone()))),
            }
        }
        if !v.is_empty() {
            out.push((state, v));
        }
    }
    (out, skipped)
}

/// The text of a wing (and its name).
pub fn render_wing(w: &WingSpec) -> String {
    let v = w.version.as_ref().map(|v| format!(" version_name = {}", script::quote(v))).unwrap_or_default();
    let c = w.creator.as_ref().map(|c| format!(" creator = {}", script::quote(c))).unwrap_or_default();
    let mut s = format!("{} = {{ owner = {} amount = {}{c}{v} }}", w.equipment, script::quote(&w.owner), fmt(w.amount));
    if let Some(n) = &w.name {
        s.push_str(&format!("\nname = {}", script::quote(n)));
    }
    s
}

/// The text of a state's wings.
pub fn render_base(state: u32, wings: &[WingSpec]) -> String {
    let inner: Vec<String> = wings.iter().map(|w| format!("\t{}", render_wing(w).replace('\n', "\n\t"))).collect();
    format!("{state} = {{\n{}\n}}", inner.join("\n"))
}

/// The inside of `air_wings = { ... }` of these.
pub fn render_bases(bases: &[(u32, Vec<WingSpec>)]) -> String {
    bases.iter().map(|(s, w)| render_base(*s, w)).collect::<Vec<_>>().join("\n")
}

/// A history file's block for players with (`bba`) or without By Blood
/// Alone, at its top: an `if` whose limit is that alone, or (without) the
/// `else` after or inside such an `if` with By Blood Alone.
pub fn branch_block(root: &Block, bba: bool) -> Option<&Block> {
    let exact = |b: &Block, with: bool| {
        b.block("limit").is_some_and(|l| {
            let n = dlc::of_trigger(l).0;
            if with { n.with.len() == 1 && n.with.contains(BBA) && n.without.is_empty() } else { n.without.len() == 1 && n.without.contains(BBA) && n.with.is_empty() }
        })
    };
    let items = &root.items;
    for (k, i) in items.iter().enumerate() {
        let Some(b) = i.block() else { continue };
        if !i.is("if") {
            continue;
        }
        if exact(b, bba) {
            return Some(b);
        }
        if !bba && exact(b, true) {
            if let Some(e) = b.block("else") {
                return Some(e);
            }
            if let Some(next) = items.get(k + 1).filter(|n| n.is("else"))
                && let Some(e) = next.block()
            {
                return Some(e);
            }
        }
    }
    None
}

/// The `if` of a branch, to add to a history file that has none.
pub fn render_branch(bba: bool, inner: &str) -> String {
    let limit = if bba { format!("has_dlc = {}", script::quote(BBA)) } else { format!("NOT = {{ has_dlc = {} }}", script::quote(BBA)) };
    format!("if = {{\n\tlimit = {{ {limit} }}\n\t{}\n}}", inner.replace('\n', "\n\t"))
}

/// The names of the plane designs and fallbacks of a nation, to keep new
/// names free.
pub fn names(ds: &[Design], fs: &[Fallback]) -> BTreeSet<String> {
    ds.iter().map(|d| d.name.clone()).chain(fs.iter().map(|f| f.name.clone())).collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::content::script::Script;
    use crate::content::store::SHARED;

    const FRAMES_TXT: &str = r#"equipments = {
	small_plane_airframe = {
		year = 1933
		is_archetype = yes
		type = fighter
		type_override = fixed_main_weapon_slot
		allowed_types = { fighter interceptor cas naval_bomber suicide }
		air_superiority = 1
		reliability = 0.8
		build_cost_ic = 4
		resources = { aluminium = 2 rubber = 1 }
		module_slots = {
			fixed_main_weapon_slot = { required = yes allowed_module_categories = { fighter_weapon cas_weapon } }
			engine_type_slot = { required = yes allowed_module_categories = { plane_engine_type } }
			special_type_slot_1 = { required = no allowed_module_categories = { plane_special_module_small } }
		}
		module_count_limit = { module = armor_plate_small count < 2 }
	}
	small_plane_airframe_0 = {
		year = 1933
		archetype = small_plane_airframe
		derived_variant_name = fighter_equipment_0
		module_slots = inherit
		weight = 3
		maximum_speed = 300
	}
	small_plane_airframe_1 = {
		year = 1936
		archetype = small_plane_airframe
		parent = small_plane_airframe_0
		derived_variant_name = fighter_equipment_1
		weight = 4
		maximum_speed = 425
		module_slots = {
			fixed_main_weapon_slot = inherit
			engine_type_slot = inherit
			special_type_slot_1 = inherit
			special_type_slot_2 = special_type_slot_1
		}
	}
	fighter_equipment_0 = {
		year = 1933
		archetype = small_plane_airframe
		air_attack = 9
		maximum_speed = 330
		upgrades = { plane_gun_upgrade plane_engine_upgrade }
	}
	fighter_equipment_1 = { year = 1936 archetype = small_plane_airframe parent = fighter_equipment_0 air_attack = 18 maximum_speed = 500 }
	CAS_equipment_1 = { year = 1936 archetype = small_plane_cas_airframe air_ground_attack = 12 upgrades = { plane_cas_upgrade } }
}
duplicate_archetypes = {
	small_plane_cas_airframe = {
		archetype = small_plane_airframe
		type = cas
		variant_name = { small_plane_cas_airframe_1 = CAS_equipment_1 }
		for_each = { air_superiority = { set = 0 } }
	}
}
"#;
    const MODULES: &str = r#"equipment_modules = {
	limit = { has_dlc = "By Blood Alone" }
	engine_1_1x = { category = plane_engine_type add_stats = { thrust = 11 build_cost_ic = 12 } }
	light_mg_2x = { category = fighter_weapon add_equipment_type = { fighter heavy_fighter } add_stats = { air_attack = 5 weight = 1 } }
	light_mg_4x = { category = fighter_weapon add_equipment_type = { fighter heavy_fighter } add_stats = { air_attack = 10 weight = 2 } }
	bomb_locks = { category = cas_weapon add_equipment_type = cas add_stats = { build_cost_ic = 1 } mission_type_stats = { limit = { cas attack_logistics } add_stats = { weight = 9 air_ground_attack = 6 } } }
	armor_plate_small = { category = plane_special_module_small add_stats = { air_defence = 2 weight = 1 } }
	heavy_armor = { category = plane_special_module_small add_stats = { weight = 9 } }
}
"#;
    const TECHS: &str = r#"technologies = {
	iw_small_airframe = { enable_equipments = { small_plane_airframe_0 small_plane_cas_airframe_0 } enable_equipment_modules = { engine_1_1x light_mg_2x bomb_locks } start_year = 1933 folder = { name = bba_air_techs_folder } }
	basic_small_airframe = { enable_equipments = { small_plane_airframe_1 small_plane_cas_airframe_1 } start_year = 1936 folder = { name = bba_air_techs_folder } }
	armour = { enable_equipment_modules = { armor_plate_small light_mg_4x heavy_armor } start_year = 1936 folder = { name = bba_air_techs_folder } }
	early_fighter = { enable_equipments = { fighter_equipment_0 } start_year = 1933 folder = { name = air_techs_folder } }
	fighter1 = { enable_equipments = { fighter_equipment_1 } start_year = 1936 folder = { name = air_techs_folder } }
	CAS1 = { enable_equipments = { CAS_equipment_1 } start_year = 1936 folder = { name = air_techs_folder } }
}
"#;
    const TAGS: &str = r#"technology_folders = {
	air_techs_folder = { available = { NOT = { has_dlc = "By Blood Alone" } } }
	bba_air_techs_folder = { available = { has_dlc = "By Blood Alone" } }
}
"#;
    const UPGRADES: &str = "upgrades = {\n\tplane_gun_upgrade = { max_level = 5 air_attack = 0.1 maximum_speed = -0.01 }\n\tplane_engine_upgrade = { max_level = 5 maximum_speed = 0.02 }\n\tplane_cas_upgrade = { max_level = 5 air_ground_attack = 0.1 }\n}\n";

    /// Small airframes, a few modules, research with and without the DLC
    /// (also for the Air page's tests).
    pub(crate) fn db() -> UnitDb {
        let mut db = UnitDb::default();
        db.read_file("common/units/equipment/plane_airframes.txt", &Script::parse(FRAMES_TXT), false);
        db.read_file("common/units/equipment/modules/00_plane_modules.txt", &Script::parse(MODULES), false);
        db.read_file("common/technologies/air.txt", &Script::parse(TECHS), false);
        db.read_file("common/technology_tags/00_technology.txt", &Script::parse(TAGS), false);
        db.read_file("common/units/equipment/upgrades/air_upgrades.txt", &Script::parse(UPGRADES), false);
        db
    }

    fn design(hull: &str, modules: &[(&str, &str)], need: Need) -> Design {
        Design { name: "Falke".into(), hull: hull.into(), modules: modules.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect(), obsolete: false, need, pkg: "TYR".into(), file: "history/countries/TYR.txt".into(), span: 0..0 }
    }

    fn bba() -> Need {
        Need { with: [BBA.to_string()].into(), ..Default::default() }
    }

    fn not_bba() -> Need {
        Need { without: [BBA.to_string()].into(), ..Default::default() }
    }

    fn id(s: &str) -> String {
        s.to_string()
    }

    const EMPTY_SLOT: &str = navy::EMPTY;

    #[test]
    fn the_game_makes_families_of_roles() {
        let d = db();
        let cas = d.equipment.get("small_plane_cas_airframe_1").expect("made");
        assert_eq!((cas.archetype.as_deref(), cas.parent.as_deref(), cas.legacy.as_deref()), (Some("small_plane_cas_airframe"), Some("small_plane_cas_airframe_0"), Some("CAS_equipment_1")));
        assert_eq!(d.types_of("small_plane_cas_airframe_1"), vec![id("cas")]);
        assert_eq!(d.equipment["small_plane_cas_airframe"].stats.get("air_superiority"), Some(&0.0), "for_each sets");
        assert!(!d.equipment.contains_key("fighter_cas_equipment_0"), "only the family's own models");
        assert!(d.is_airframe("small_plane_cas_airframe_1") && d.is_airframe("small_plane_airframe_0") && !d.is_airframe("small_plane_airframe"));
        assert!(d.is_old_plane("fighter_equipment_1") && d.is_old_plane("CAS_equipment_1") && !d.is_old_plane("small_plane_airframe_1"));
        assert_eq!(d.airframes().iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["small_plane_airframe_0", "small_plane_airframe_1"]);
        let h = d.hull("small_plane_cas_airframe_1").unwrap();
        assert_eq!(h.slots.iter().map(|s| s.key.as_str()).collect::<Vec<_>>(), ["fixed_main_weapon_slot", "engine_type_slot", "special_type_slot_1", "special_type_slot_2"]);
        assert_eq!(d.modules["light_mg_2x"].need, bba(), "the limit of the block");
        assert_eq!(d.modules["light_mg_2x"].add_types, vec![id("fighter"), id("heavy_fighter")]);
    }

    #[test]
    fn the_main_armament_makes_the_role() {
        let d = db();
        let h = d.hull("small_plane_airframe_1").unwrap();
        let f = d.fitted(&h, &[(id("fixed_main_weapon_slot"), id("light_mg_2x"))]);
        assert_eq!(d.plane_role(&h, &f).as_deref(), Some("fighter"));
        let f = d.fitted(&h, &[(id("fixed_main_weapon_slot"), id("bomb_locks"))]);
        assert_eq!(d.plane_role(&h, &f).as_deref(), Some("cas"));
        assert_eq!(d.airframe_for("small_plane_airframe_1", "cas").as_deref(), Some("small_plane_cas_airframe_1"));
        assert_eq!(d.airframe_for("small_plane_cas_airframe_1", "fighter").as_deref(), Some("small_plane_airframe_1"));
        assert_eq!(d.airframe_for("small_plane_airframe_1", "suicide"), None, "no family of it");
        assert_eq!(d.base_airframe("small_plane_cas_airframe_0"), "small_plane_airframe_0");
    }

    #[test]
    fn new_designs_take_the_role() {
        let d = db();
        let have = d.have(["iw_small_airframe".to_string(), "armour".to_string()].into());
        let h = d.hull("small_plane_airframe_0").unwrap();
        assert_eq!(d.roles_of("small_plane_airframe_0"), vec![id("fighter"), id("cas")]);
        let m = d.plane_modules(&h, &have, Some("cas"), &BTreeMap::new());
        assert_eq!(m, vec![(id("fixed_main_weapon_slot"), id("bomb_locks")), (id("engine_type_slot"), id("engine_1_1x")), (id("special_type_slot_1"), id(EMPTY_SLOT))]);
        let m = d.plane_modules(&h, &have, Some("fighter"), &BTreeMap::new());
        assert_eq!(m[0].1, "light_mg_4x", "the newest of the role");
        // kept where it fits the role
        let keep: BTreeMap<String, String> = [(id("fixed_main_weapon_slot"), id("light_mg_2x")), (id("special_type_slot_1"), id("armor_plate_small"))].into();
        let m = d.plane_modules(&h, &have, Some("fighter"), &keep);
        assert_eq!((m[0].1.as_str(), m[2].1.as_str()), ("light_mg_2x", "armor_plate_small"));
        assert_eq!(d.plane_modules(&h, &have, Some("cas"), &keep)[0].1, "bomb_locks");
    }

    #[test]
    fn designs_are_checked() {
        let d = db();
        let all = with_bba(&Owned::new());
        let have = d.have(["iw_small_airframe".to_string()].into());
        let ok = design("small_plane_airframe_0", &[("fixed_main_weapon_slot", "light_mg_2x"), ("engine_type_slot", "engine_1_1x")], bba());
        assert_eq!(d.check_plane(&ok, &have, std::slice::from_ref(&ok), "TYR", &id), Vec::new());
        let msgs = |x: &Design| d.check_plane(x, &have, std::slice::from_ref(x), "TYR", &id).into_iter().map(|p| (p.severity, p.message)).collect::<Vec<_>>();
        // bombs on a fighter airframe; armour not researched; too heavy
        let m = msgs(&design("small_plane_airframe_0", &[("fixed_main_weapon_slot", "bomb_locks"), ("engine_type_slot", "engine_1_1x"), ("special_type_slot_1", "heavy_armor")], bba()));
        assert!(m.iter().any(|(s, t)| *s == Severity::Error && t.contains("cannot use heavy_armor at the start: it comes with armour")), "{m:?}");
        assert!(m.iter().any(|(s, t)| *s == Severity::Warning && t.contains("type = small_plane_cas_airframe_0")), "{m:?}");
        assert!(m.iter().any(|(s, t)| *s == Severity::Error && t.contains("weight exceeds thrust (12 > 11)")), "{m:?}");
        // the bombs weigh on close air support
        let m = msgs(&design("small_plane_cas_airframe_0", &[("fixed_main_weapon_slot", "bomb_locks"), ("engine_type_slot", "engine_1_1x")], bba()));
        assert_eq!(m, vec![(Severity::Warning, "weight exceeds thrust (11) on these missions: attack_logistics (12), cas (12)".to_string())]);
        // outside the DLC's if; not researched airframe
        let m = msgs(&design("small_plane_airframe_1", &[("fixed_main_weapon_slot", "light_mg_2x"), ("engine_type_slot", "engine_1_1x")], Need::default()));
        assert!(m.iter().any(|(s, t)| *s == Severity::Warning && t.contains("not inside if")), "{m:?}");
        assert!(m.iter().any(|(s, t)| *s == Severity::Error && t.contains("cannot build the small_plane_airframe_1 airframe at the start: it comes with basic_small_airframe")), "{m:?}");
        let m = msgs(&design("small_plane_airframe_0", &[], not_bba()));
        assert!(m[0].1.contains("no plane designer"), "{m:?}");
        // the technologies to give, for a player with the DLC
        assert_eq!(d.techs_to_give(&[id("small_plane_airframe_1"), id("armor_plate_small")], &have, &all), vec![id("armour"), id("basic_small_airframe")]);
    }

    #[test]
    fn fallbacks_are_planes_without_modules() {
        let d = db();
        let have = d.have(["early_fighter".to_string()].into());
        let fb = |model: &str, ups: &[(&str, i64)]| Fallback { name: "Falke".into(), model: model.into(), upgrades: ups.iter().map(|(a, b)| (a.to_string(), *b)).collect(), obsolete: false, need: not_bba(), pkg: "TYR".into(), file: "h".into(), span: 0..0 };
        let ok = fb("fighter_equipment_0", &[("plane_gun_upgrade", 2)]);
        let fighter = design("small_plane_airframe_0", &[("fixed_main_weapon_slot", "light_mg_2x"), ("engine_type_slot", "engine_1_1x")], bba());
        assert_eq!(d.check_fallback(&ok, &have, Some(&fighter), std::slice::from_ref(&ok), "TYR", &id), Vec::new());
        let msgs = |f: &Fallback, des: Option<&Design>| d.check_fallback(f, &have, des, std::slice::from_ref(f), "TYR", &id).into_iter().map(|p| p.message).collect::<Vec<_>>();
        let m = msgs(&fb("fighter_equipment_0", &[("plane_gun_upgrade", 6), ("plane_cas_upgrade", 1), ("x_upgrade", 1)]), None);
        assert_eq!(m.len(), 3, "{m:?}");
        assert!(m[0].contains("goes from 0 to 5") && m[1].contains("takes no plane_cas_upgrade") && m[2].contains("not an upgrade"), "{m:?}");
        let m = msgs(&fb("CAS_equipment_1", &[]), Some(&fighter));
        assert!(m.iter().any(|t| t.contains("cannot build the CAS_equipment_1 at the start without By Blood Alone: it comes with CAS1")), "{m:?}");
        assert!(m.iter().any(|t| t.contains("is a fighter, its fallback a cas")), "{m:?}");
        assert!(msgs(&fb("small_plane_airframe_0", &[]), None)[0].contains("not a plane without modules"));
        // the stats with upgrades; the model to fall back to
        let s = d.old_plane_stats("fighter_equipment_0", &[(id("plane_gun_upgrade"), 2), (id("plane_engine_upgrade"), 1)]);
        assert!((s["air_attack"] - 9.0 * 1.2).abs() < 1e-9 && (s["maximum_speed"] - 330.0 * 1.0).abs() < 1e-9, "{s:?}");
        assert_eq!(d.upgrades_of("fighter_equipment_1"), vec![id("plane_gun_upgrade"), id("plane_engine_upgrade")], "of the model it follows");
        assert_eq!(d.fallback_model("small_plane_airframe_1", Some("fighter"), &have).as_deref(), Some("fighter_equipment_0"), "the one it has");
        let more = d.have(["early_fighter".to_string(), "fighter1".to_string()].into());
        assert_eq!(d.fallback_model("small_plane_airframe_1", Some("fighter"), &more).as_deref(), Some("fighter_equipment_1"));
        assert_eq!(d.fallback_model("small_plane_cas_airframe_1", Some("cas"), &have).as_deref(), Some("CAS_equipment_1"), "the one it stands for, though not researched");
    }

    const HIST: &str = r#"capital = 1
if = {
	limit = { has_dlc = "By Blood Alone" }
	create_equipment_variant = { name = "Falke" type = small_plane_airframe_0 modules = { fixed_main_weapon_slot = light_mg_2x engine_type_slot = engine_1_1x } }
	set_air_oob = "TYR_air_bba"
}
else = {
	create_equipment_variant = { name = "Falke" type = fighter_equipment_0 upgrades = { plane_gun_upgrade = 1 } }
	set_air_oob = "TYR_air_legacy"
}
"#;

    #[test]
    fn designs_fallbacks_and_branches_are_read() {
        let d = db();
        let s = Script::parse(HIST);
        let mut st = Store::empty(std::path::PathBuf::from("/nonexistent"));
        st.set_text("TYR", "history/countries/TYR.txt", HIST.into());
        let (ds, fs) = planes(&d, &st, "TYR");
        assert_eq!((ds.len(), fs.len()), (1, 1));
        assert_eq!((branch_of(&ds[0].need), branch_of(&fs[0].need)), (Branch::Bba, Branch::Legacy));
        assert_eq!(fs[0].upgrades, vec![(id("plane_gun_upgrade"), 1)]);
        let lines = air_oobs(&st, "TYR");
        assert_eq!(lines.iter().map(|l| (l.name.as_str(), branch_of(&l.need))).collect::<Vec<_>>(), [("TYR_air_bba", Branch::Bba), ("TYR_air_legacy", Branch::Legacy)]);
        assert!(branch_block(&s.root, true).is_some_and(|b| b.get("set_air_oob").is_some()));
        assert!(branch_block(&s.root, false).is_some_and(|b| b.atom("set_air_oob") == Some("\"TYR_air_legacy\"") || b.atom("set_air_oob") == Some("TYR_air_legacy")));
        assert!(branch_block(&Script::parse("capital = 1").root, true).is_none());
        let t = render_branch(false, "set_air_oob = \"X\"");
        assert!(branch_block(&Script::parse(&t).root, false).is_some(), "{t}");
    }

    const OOB: &str = r#"air_wings = {
	3 = {
		small_plane_airframe_0 = { owner = "TYR" amount = 150 version_name = "Falke" }
		name = "1. Jagdgeschwader"
		ace = { name = "X" }
		small_plane_airframe_0 = { owner = "TYR" amount = 100 version_name = "Adler" }
	}
	9 = {
		small_plane_cas_airframe_0 = { amount = 0 }
	}
}
"#;

    #[test]
    fn wings_are_read_and_checked_against_air_bases() {
        let d = db();
        let s = Script::parse(OOB);
        let bases = read_wings(&s.root);
        assert_eq!(bases.iter().map(|b| (b.state, b.wings.len())).collect::<Vec<_>>(), [(3, 2), (9, 1)]);
        assert_eq!((bases[0].wings[0].name.as_deref(), bases[0].wings[1].name.as_deref()), (Some("1. Jagdgeschwader"), None));
        let falke = design("small_plane_airframe_0", &[("fixed_main_weapon_slot", "light_mg_2x"), ("engine_type_slot", "engine_1_1x")], bba());
        let planes_of = |_: &str| (vec![falke.clone()], Vec::new());
        let fields = [Airfield { state: 3, level: 1, name: "Bozen".into(), tag: "TYR".into() }];
        let p: Vec<String> = check_wings(&d, &bases, "TYR", &fields, false, &planes_of, &id).into_iter().filter(|p| p.severity != Severity::Info).map(|p| p.message).collect();
        assert!(p.iter().any(|m| m.contains("250 planes, its air base (level 1) takes 200")), "{p:?}");
        assert!(p.iter().any(|m| m.contains("no design \"Adler\"")), "{p:?}");
        assert!(p.iter().any(|m| m.contains("state 9: no air base (TYR has Bozen (state 3))")), "{p:?}");
        assert!(p.iter().any(|m| m.contains("amount = 0")) && p.iter().any(|m| m.contains("no owner")), "{p:?}");
        // the legacy file has no airframes
        let p = check_wings(&d, &bases[..1], "TYR", &fields, true, &planes_of, &id);
        assert!(p.iter().any(|p| p.severity == Severity::Error && p.message.contains("players without By Blood Alone do not have: a plane without modules (fighter_equipment_0)")), "{p:?}");
    }

    #[test]
    fn the_wings_without_the_dlc_follow_those_with_it() {
        let d = db();
        let bases = read_wings(&Script::parse(OOB).root);
        let falke = design("small_plane_airframe_0", &[("fixed_main_weapon_slot", "light_mg_2x"), ("engine_type_slot", "engine_1_1x")], bba());
        let fb = Fallback { name: "Falke".into(), model: "fighter_equipment_0".into(), upgrades: Vec::new(), obsolete: false, need: not_bba(), pkg: "TYR".into(), file: "h".into(), span: 0..0 };
        let have = d.have(["early_fighter".to_string()].into());
        let planes_of = |_: &str| (vec![falke.clone()], vec![fb.clone()]);
        let (w, skipped) = legacy_wings(&d, &bases, "TYR", &planes_of, &have);
        assert!(skipped.is_empty(), "{skipped:?}");
        assert_eq!(w[0].1[0], WingSpec { equipment: "fighter_equipment_0".into(), owner: "TYR".into(), creator: None, amount: 150.0, version: Some("Falke".into()), name: Some("1. Jagdgeschwader".into()) });
        assert_eq!((w[0].1[1].equipment.as_str(), w[0].1[1].version.as_deref()), ("fighter_equipment_0", None), "no fallback: the plane it stands for");
        assert_eq!(w[1].1[0].equipment, "CAS_equipment_1");
        // written and read back
        let text = format!("air_wings = {{\n\t{}\n}}\n", render_bases(&w).replace('\n', "\n\t"));
        let back = specs(&read_wings(&Script::parse(&text).root), "TYR");
        assert_eq!(back, w, "{text}");
    }

    /// The game's plane designs and air wings: each design fits its
    /// airframe, each wing names a design of its owner. Run with
    /// `cargo test -- --ignored game_planes_fit`.
    #[test]
    #[ignore]
    fn game_planes_fit() {
        let game = crate::modbuild::default_game_dir();
        if !game.join("common/units").is_dir() {
            println!("HOI4 not found at {}: not tested", game.display());
            return;
        }
        let files = crate::content::source::base_files(&game);
        let mut d = UnitDb::default();
        for dir in crate::content::units::FOLDERS {
            for path in files.list(dir, "txt", false) {
                if let Some(t) = files.text(&path) {
                    d.read_file(&path, &Script::parse(&t), false);
                }
            }
        }
        let mut st = Store::empty(std::path::PathBuf::from("/nonexistent"));
        let mut tags: BTreeSet<String> = BTreeSet::new();
        for path in files.list("history/countries", "txt", false) {
            let tag = path.rsplit('/').next().unwrap_or("").chars().take(3).collect::<String>();
            if let Some(t) = files.text(&path) {
                st.set_text(&tag, &path, t);
                tags.insert(tag);
            }
        }
        for path in files.list("history/units", "txt", false) {
            if let Some(t) = files.text(&path) {
                st.set_text(SHARED, &path, t);
            }
        }
        let (mut designs, mut wings, mut bad) = (0, 0, Vec::new());
        for tag in &tags {
            let (ds, fs) = planes(&d, &st, tag);
            // the game's designs use modules their nations have not
            // researched (the game makes them all the same): the
            // structure is checked, not the research
            let have = d.have(d.techs.keys().cloned().collect());
            let legacy = have.clone();
            for x in &ds {
                designs += 1;
                for p in d.check_plane(x, &have, &ds, tag, &id).into_iter().filter(|p| p.severity == Severity::Error) {
                    bad.push(format!("{tag} {}: {}", x.name, p.message));
                }
            }
            for f in &fs {
                for p in d.check_fallback(f, &legacy, None, &fs, tag, &id).into_iter().filter(|p| p.severity == Severity::Error) {
                    bad.push(format!("{tag} fallback {}: {}", f.name, p.message));
                }
            }
            for l in air_oobs(&st, tag) {
                let Some((pkg, path)) = navy::oob_file(&st, tag, &l.name) else { continue };
                let Some(s) = st.file(&pkg, &path).and_then(|f| f.script.as_ref()) else { continue };
                let bases = read_wings(&s.root);
                wings += bases.iter().map(|b| b.wings.len()).sum::<usize>();
                let planes_of = |t: &str| planes(&d, &st, t);
                // the game's files put Germany's Do 17 on a small airframe
                // in two wings of other nations: a slip of theirs
                for p in check_wings(&d, &bases, tag, &[], branch_of(&l.need) == Branch::Legacy, &planes_of, &id).into_iter().filter(|p| p.severity == Severity::Error && !p.message.contains("no air base") && !p.message.contains("no design \"Do 17\"")) {
                    bad.push(format!("{tag} {path}: {}", p.message));
                }
            }
        }
        eprintln!("{designs} designs, {wings} wings, {} errors", bad.len());
        assert!(designs > 100 && wings > 500, "{designs} {wings}");
        assert!(bad.is_empty(), "{}", bad.join("\n"));
    }
}
