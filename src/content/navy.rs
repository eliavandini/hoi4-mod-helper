//! The navy of a nation (Man the Guns): ship designs, made of a hull and
//! modules as in the game's ship designer, and the fleets it starts with,
//! of task forces of ships of those designs, stationed at the naval bases
//! of the map.
//!
//! - A design is a `create_equipment_variant` of the nation's history
//!   (`history/countries`): a `name`, a hull (`type = ship_hull_light_1`)
//!   and the module of each slot (`modules = { fixed_ship_battery_slot =
//!   ship_light_battery_1 ... }`; a slot left out keeps the hull's default
//!   module).
//! - The hulls and their slots are the game's (and the mod's) modular ship
//!   equipment (`common/units/equipment/ship_hull_*.txt`): a model takes
//!   the slots of the model it follows (`parent`, else its archetype) where
//!   it says `inherit`; its default modules are those of the archetype and
//!   each model up to it. A slot takes modules of its categories
//!   (`allowed_module_categories`); `module_count_limit` caps a category,
//!   a module's `forbid_module_categories` keeps others off the design.
//! - Hulls and modules come with technologies (`enable_equipments`,
//!   `enable_equipment_modules`): the nation must have them at the start.
//! - A design's stats are the archetype's with the hull's own on top, plus
//!   each module's `add_stats`, plus the average of the modules'
//!   `add_average_stats` (the guns' piercing), times one plus the sum of
//!   their `multiply_stats`. Left out: upgrades, design companies,
//!   doctrines and technologies' bonuses.
//! - What a ship is (`definition` in a fleet: destroyer, light cruiser,
//!   battleship ...) is the naval sub-unit (`common/units`) of the hull's
//!   archetype: of a cruiser, a heavy cruiser when a module makes it a
//!   capital ship (a medium battery); of a heavy hull, a battleship or a
//!   battlecruiser by its armour (`need_equipment_modules`).
//! - The fleets are in a `history/units` file the history loads with
//!   `set_naval_oob = "<file>"`: `units = { fleet = { name naval_base
//!   task_force = { name location ship = { name definition equipment = {
//!   <hull> = { amount = 1 owner = TAG version_name = "<design>" } } } } }
//!   }`. A fleet's base and a task force's place are naval bases of the map
//!   (the port tool of the map editor's supply level), of the nation.

use std::collections::{BTreeMap, BTreeSet};

use super::dlc::{self, Need, Owned};
use super::script::{self, Block, Span};
use super::store::{SHARED, Store};
use super::units::{CountLimit, Equipment, Have, ShipUnit, SlotDef, SlotSrc, Stats, UnitDb};
use super::validate::Severity;

pub const MTG: &str = "Man the Guns";

/// The game's hull families, in the order the designer lists them.
pub const ARCHETYPES: [&str; 5] = ["ship_hull_light", "ship_hull_cruiser", "ship_hull_heavy", "ship_hull_carrier", "ship_hull_submarine"];
/// What a hull can be in combat.
const SHIP_TYPES: [&str; 4] = ["screen_ship", "capital_ship", "submarine", "carrier"];

/// The slots in the order the designer shows them: the fixed ones, then
/// the custom ones front to rear.
pub const SLOT_ORDER: [&str; 17] = [
    "fixed_ship_deck_slot_1",
    "fixed_ship_deck_slot_2",
    "fixed_ship_battery_slot",
    "fixed_ship_anti_air_slot",
    "fixed_ship_fire_control_system_slot",
    "fixed_ship_radar_slot",
    "fixed_ship_torpedo_slot",
    "fixed_ship_engine_slot",
    "fixed_ship_armor_slot",
    "fixed_ship_secondaries_slot",
    "front_1_custom_slot",
    "front_2_custom_slot",
    "mid_1_custom_slot",
    "mid_2_custom_slot",
    "mid_3_custom_slot",
    "rear_1_custom_slot",
    "rear_2_custom_slot",
];

/// The module of a slot left empty.
pub const EMPTY: &str = "empty";

/// A slot of a hull.
#[derive(Clone, Debug, PartialEq)]
pub struct Slot {
    pub key: String,
    pub required: bool,
    pub categories: Vec<String>,
}

/// A hull as the designer uses it: its slots (worked out through
/// `inherit`), default modules and limits.
#[derive(Clone, Debug, Default)]
pub struct Hull {
    pub id: String,
    pub slots: Vec<Slot>,
    /// Slot → module (or `empty`).
    pub defaults: BTreeMap<String, String>,
    pub limits: Vec<CountLimit>,
    /// screen_ship, capital_ship, submarine or carrier.
    pub types: Vec<String>,
}

impl Hull {
    pub fn slot(&self, key: &str) -> Option<&Slot> {
        self.slots.iter().find(|s| s.key == key)
    }
}

/// A design of a nation's history.
#[derive(Clone, Debug, PartialEq)]
pub struct Design {
    pub name: String,
    pub hull: String,
    /// The modules as written (slot, module).
    pub modules: Vec<(String, String)>,
    pub obsolete: bool,
    /// The DLCs of the `if` it is in.
    pub need: Need,
    pub pkg: String,
    pub file: String,
    pub span: Span,
}

/// The stats of a design.
#[derive(Clone, Debug, Default)]
pub struct ShipStats {
    pub stats: Stats,
    pub resources: Stats,
    pub types: Vec<String>,
}

impl ShipStats {
    pub fn get(&self, k: &str) -> f64 {
        self.stats.get(k).copied().unwrap_or(0.0)
    }
}

/// A problem of a design; `slot` is the slot it is about.
#[derive(Clone, Debug, PartialEq)]
pub struct Problem {
    pub severity: Severity,
    pub message: String,
    pub slot: Option<String>,
}

fn slot_rank(k: &str) -> usize {
    SLOT_ORDER.iter().position(|s| *s == k).unwrap_or(SLOT_ORDER.len())
}

impl UnitDb {
    /// The model it follows for slots and defaults: its `parent` (a model
    /// of the game), else its archetype.
    fn up_of<'a>(&'a self, e: &'a Equipment) -> Option<&'a Equipment> {
        if e.is_archetype {
            return None;
        }
        e.parent.as_deref().filter(|p| *p != e.id).and_then(|p| self.equipment.get(p)).or_else(|| e.archetype.as_deref().filter(|a| *a != e.id).and_then(|a| self.equipment.get(a)))
    }

    /// The model, the one it follows, and so on up to the archetype.
    fn chain<'a>(&'a self, id: &str) -> Vec<&'a Equipment> {
        let mut out: Vec<&Equipment> = Vec::new();
        let mut cur = self.equipment.get(id);
        while let Some(e) = cur {
            if out.iter().any(|x| x.id == e.id) || out.len() > 16 {
                break;
            }
            out.push(e);
            cur = self.up_of(e);
        }
        // the archetype last, also when a parent is of another family
        if let Some(a) = out.first().and_then(|e| e.archetype.as_deref()).and_then(|a| self.equipment.get(a))
            && !out.iter().any(|x| x.id == a.id)
        {
            out.push(a);
        }
        out
    }

    /// A modular ship hull a design can be made of (not a family, and not
    /// one of the game's ships without modules).
    pub fn is_hull(&self, id: &str) -> bool {
        let Some(e) = self.equipment.get(id) else { return false };
        if e.is_archetype || !e.modular {
            return false;
        }
        let arch = e.archetype.as_deref().and_then(|a| self.equipment.get(a));
        arch.into_iter().chain([e]).flat_map(|x| &x.types).any(|t| SHIP_TYPES.contains(&t.as_str()))
    }

    /// The hulls, by family (as the designer lists them) and year.
    pub fn hulls(&self) -> Vec<&Equipment> {
        let mut v: Vec<&Equipment> = self.equipment.values().filter(|e| self.is_hull(&e.id)).collect();
        let fam = |e: &Equipment| {
            let a = e.archetype.as_deref().unwrap_or("");
            (ARCHETYPES.iter().position(|x| *x == a).unwrap_or(ARCHETYPES.len()), a.to_string())
        };
        v.sort_by(|a, b| fam(a).cmp(&fam(b)).then(a.year.total_cmp(&b.year)).then(a.id.cmp(&b.id)));
        v
    }

    fn slot_defs(&self, id: &str, depth: u8) -> Vec<(String, SlotDef)> {
        let Some(e) = self.equipment.get(id) else { return Vec::new() };
        if depth > 16 {
            return Vec::new();
        }
        let inherited = || self.up_of(e).map(|u| self.slot_defs(&u.id, depth + 1)).unwrap_or_default();
        match &e.slots {
            None => {
                if e.is_archetype {
                    Vec::new()
                } else {
                    inherited()
                }
            }
            Some(list) => {
                let from = if list.iter().any(|(_, d)| !matches!(d, SlotSrc::Own(_))) { inherited() } else { Vec::new() };
                let mut out: Vec<(String, SlotDef)> = Vec::new();
                for (k, d) in list {
                    let def = match d {
                        SlotSrc::Own(d) => Some(d.clone()),
                        SlotSrc::Inherit => from.iter().find(|x| x.0 == *k).map(|x| x.1.clone()),
                        // as a slot before it here, else of the model it follows
                        SlotSrc::Like(o) => out.iter().chain(&from).find(|x| x.0 == *o).map(|x| x.1.clone()).or_else(|| list.iter().find_map(|(k2, d2)| match d2 {
                            SlotSrc::Own(d2) if k2 == o => Some(d2.clone()),
                            _ => None,
                        })),
                    };
                    if let Some(def) = def {
                        out.push((k.clone(), def));
                    }
                }
                out
            }
        }
    }

    /// A hull with its slots, defaults and limits.
    pub fn hull(&self, id: &str) -> Option<Hull> {
        self.equipment.get(id)?;
        let chain = self.chain(id);
        let mut slots: Vec<Slot> = self.slot_defs(id, 0).into_iter().map(|(key, d)| Slot { key, required: d.required, categories: d.categories }).collect();
        slots.sort_by_key(|s| slot_rank(&s.key));
        let mut defaults: BTreeMap<String, String> = BTreeMap::new();
        let mut limits: Vec<CountLimit> = Vec::new();
        let mut types: Vec<String> = Vec::new();
        for x in chain.iter().rev() {
            defaults.extend(x.default_modules.clone());
            for l in &x.count_limits {
                limits.retain(|o| !(o.of == l.of && o.module == l.module));
                limits.push(l.clone());
            }
            if !x.types.is_empty() {
                types = x.types.clone();
            }
        }
        defaults.retain(|k, _| slots.iter().any(|s| s.key == *k));
        Some(Hull { id: id.to_string(), slots, defaults, limits, types })
    }

    /// A module fits a slot (its category is one the slot takes).
    pub fn module_fits(&self, slot: &Slot, module: &str) -> bool {
        self.modules.get(module).is_some_and(|m| slot.categories.contains(&m.category))
    }

    /// The modules a slot takes, oldest first.
    pub fn modules_for(&self, slot: &Slot) -> Vec<&str> {
        let mut v: Vec<&str> = self.modules.iter().filter(|(_, m)| slot.categories.contains(&m.category)).map(|(id, _)| id.as_str()).collect();
        v.sort_by(|a, b| (slot.categories.iter().position(|c| Some(c) == self.modules.get(*a).map(|m| &m.category)), self.module_year(a), *a).partial_cmp(&(slot.categories.iter().position(|c| Some(c) == self.modules.get(*b).map(|m| &m.category)), self.module_year(b), *b)).unwrap_or(std::cmp::Ordering::Equal));
        v
    }

    /// The year of the first technology that unlocks a module (0: every
    /// nation has it).
    pub fn module_year(&self, module: &str) -> f64 {
        self.module_techs.get(module).map(|ts| ts.iter().filter_map(|t| self.techs.get(t)?.year).fold(f64::INFINITY, f64::min)).filter(|y| y.is_finite()).unwrap_or(0.0)
    }

    /// The modules of a design on a hull: the hull's defaults with those
    /// written on top (slot → module, `empty` for none).
    pub fn fitted(&self, hull: &Hull, written: &[(String, String)]) -> BTreeMap<String, String> {
        let mut out: BTreeMap<String, String> = hull.slots.iter().map(|s| (s.key.clone(), hull.defaults.get(&s.key).cloned().unwrap_or_else(|| EMPTY.to_string()))).collect();
        for (k, v) in written {
            out.insert(k.clone(), v.clone());
        }
        out
    }

    /// The stats of a hull with these modules.
    pub fn ship_stats(&self, hull: &Hull, fitted: &BTreeMap<String, String>) -> ShipStats {
        let Some(e) = self.equipment.get(&hull.id) else { return ShipStats::default() };
        let arch = e.archetype.as_deref().and_then(|a| self.equipment.get(a));
        let mut s = arch.map(|a| a.stats.clone()).unwrap_or_default();
        s.extend(e.stats.clone());
        let mut resources = if e.resources.is_empty() { arch.map(|a| a.resources.clone()).unwrap_or_default() } else { e.resources.clone() };
        let mut mult = Stats::new();
        let mut avg: BTreeMap<String, (f64, u32)> = BTreeMap::new();
        let mut types: Vec<String> = hull.types.clone();
        for (slot, m) in fitted {
            if hull.slot(slot).is_none() {
                continue;
            }
            let Some(md) = self.modules.get(m) else { continue };
            for (k, v) in &md.add {
                *s.entry(k.clone()).or_default() += v;
            }
            if md.manpower != 0.0 {
                *s.entry("manpower".into()).or_default() += md.manpower;
            }
            for (k, v) in &md.multiply {
                *mult.entry(k.clone()).or_default() += v;
            }
            for (k, v) in &md.average {
                let e = avg.entry(k.clone()).or_default();
                e.0 += v;
                e.1 += 1;
            }
            for (k, v) in &md.resources {
                *resources.entry(k.clone()).or_default() += v;
            }
            for t in &md.add_types {
                if !types.contains(t) {
                    types.push(t.clone());
                }
            }
        }
        for (k, (sum, n)) in avg {
            *s.entry(k).or_default() += sum / n as f64;
        }
        for (k, v) in mult {
            if let Some(x) = s.get_mut(&k) {
                *x *= 1.0 + v;
            }
        }
        resources.retain(|_, v| *v > 0.0);
        ShipStats { stats: s, resources, types }
    }

    /// The kinds of ship a hull can be (the naval sub-units of its family).
    pub fn roles(&self, hull: &str) -> Vec<&ShipUnit> {
        let arch = self.archetype_of(hull);
        let mut v: Vec<&ShipUnit> = self.ships.values().filter(|u| u.hulls.iter().any(|h| h == arch || h == hull)).collect();
        v.sort_by_key(|u| u.id.clone());
        v
    }

    /// What a design is: the kind of ship the game makes of it (None: none
    /// fits, a heavy hull without a battleship's or battlecruiser's armour).
    pub fn role(&self, hull: &Hull, fitted: &BTreeMap<String, String>) -> Option<&ShipUnit> {
        let added: Vec<String> = fitted.values().filter_map(|m| self.modules.get(m)).flat_map(|m| m.add_types.clone()).collect();
        let mut v: Vec<(u8, u8, &ShipUnit)> = self
            .roles(&hull.id)
            .into_iter()
            .filter(|u| u.modules_any.is_empty() || u.modules_any.iter().any(|m| fitted.values().any(|f| f == m)))
            .map(|u| {
                let by_module = if u.types.iter().any(|t| added.contains(t)) { 0 } else { 1 };
                let by_hull = if u.types.iter().any(|t| hull.types.contains(t)) { 0 } else { 1 };
                (by_module, by_hull, u)
            })
            .collect();
        v.sort_by(|a, b| (a.0, a.1, a.2.modules_any.is_empty(), &a.2.id).cmp(&(b.0, b.1, b.2.modules_any.is_empty(), &b.2.id)));
        v.first().map(|x| x.2)
    }

    /// The best module a nation has for a slot: the newest.
    pub fn best_module(&self, slot: &Slot, have: &Have) -> Option<&str> {
        self.modules_for(slot).into_iter().filter(|m| have.modules.contains(*m)).max_by(|a, b| self.module_year(a).total_cmp(&self.module_year(b)).then(a.cmp(b)))
    }

    /// The modules of a new design on a hull: the defaults the nation has,
    /// the best it has where a slot must be filled, else empty. `keep`:
    /// modules of a design before (slot → module), kept where they fit.
    pub fn new_modules(&self, hull: &Hull, have: &Have, keep: &BTreeMap<String, String>) -> Vec<(String, String)> {
        hull.slots
            .iter()
            .map(|s| {
                let kept = keep.get(&s.key).filter(|m| (*m == EMPTY && !s.required) || (self.module_fits(s, m) && have.modules.contains(*m)));
                let def = hull.defaults.get(&s.key).filter(|m| (*m == EMPTY && !s.required) || (self.module_fits(s, m) && have.modules.contains(*m)));
                let m = kept.or(def).cloned().or_else(|| s.required.then(|| self.best_module(s, have).map(String::from)).flatten()).unwrap_or_else(|| EMPTY.to_string());
                (s.key.clone(), m)
            })
            .collect()
    }

    /// The technologies to give a nation for these hulls and modules: the
    /// first of the research screen that unlocks each, with those before
    /// it on the screen, that it does not have and a player with `owned`
    /// sees.
    pub fn techs_to_give(&self, ids: &[String], have: &Have, owned: &Owned) -> Vec<String> {
        let mut out: BTreeSet<String> = BTreeSet::new();
        let before: BTreeMap<&str, Vec<&str>> = {
            let mut m: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
            for t in self.techs.values() {
                for l in &t.leads_to {
                    m.entry(l.as_str()).or_default().push(t.id.as_str());
                }
            }
            m
        };
        for id in ids {
            let mut techs: Vec<_> = self.unlocked_by(id).into_iter().filter(|t| !t.focus_only && self.tech_need(&t.id).holds(owned)).collect();
            techs.sort_by(|a, b| a.year.unwrap_or(0.0).total_cmp(&b.year.unwrap_or(0.0)).then(a.id.cmp(&b.id)));
            let Some(t) = techs.first() else { continue };
            if have.techs.contains(&t.id) {
                continue;
            }
            let mut todo = vec![t.id.as_str()];
            while let Some(x) = todo.pop() {
                if have.techs.contains(x) || !out.insert(x.to_string()) {
                    continue;
                }
                for p in before.get(x).into_iter().flatten() {
                    if self.techs.get(*p).is_some_and(|t| !t.focus_only && self.tech_need(&t.id).holds(owned)) {
                        todo.push(p);
                    }
                }
            }
        }
        let mut v: Vec<String> = out.into_iter().collect();
        v.sort_by(|a, b| self.techs.get(a).and_then(|t| t.year).unwrap_or(0.0).total_cmp(&self.techs.get(b).and_then(|t| t.year).unwrap_or(0.0)).then(a.cmp(b)));
        v
    }

    /// The checks of the modules written on a hull or airframe (`frame`:
    /// what it is called): slots it has, modules that fit them and the
    /// nation has, slots that must be filled, the limits of categories and
    /// modules that rule others out.
    pub fn check_slots(&self, hull: &Hull, written: &[(String, String)], have: &Have, tag: &str, frame: &str, name: &dyn Fn(&str) -> String) -> Vec<Problem> {
        let mut out = Vec::new();
        let mut add = |severity: Severity, message: String, slot: Option<&str>| out.push(Problem { severity, message, slot: slot.map(String::from) });
        for (slot, m) in written {
            if m == EMPTY {
                // the game's own designs leave slots of other hulls empty
                continue;
            }
            let Some(s) = hull.slot(slot) else {
                add(Severity::Error, format!("{slot}: the {} {frame} has no such slot", name(&hull.id)), Some(slot));
                continue;
            };
            if !self.modules.contains_key(m) {
                add(Severity::Error, format!("{}: {m} is not a module of the game or the mod", slot_name(slot)), Some(slot));
            } else if !self.module_fits(s, m) {
                add(Severity::Error, format!("{}: {} does not go in this slot (it takes {})", slot_name(slot), name(m), s.categories.join(", ")), Some(slot));
            } else if !have.modules.contains(m) {
                let techs = self.unlocked_text(&[m.as_str()], name, 2);
                add(Severity::Error, if techs.is_empty() { format!("{}: {tag} cannot use {}", slot_name(slot), name(m)) } else { format!("{}: {tag} cannot use {} at the start: it comes with {techs}", slot_name(slot), name(m)) }, Some(slot));
            }
        }
        let fitted = self.fitted(hull, written);
        for s in &hull.slots {
            if s.required && fitted.get(&s.key).is_none_or(|m| m == EMPTY) {
                add(Severity::Error, format!("{}: the slot must have a module", slot_name(&s.key)), Some(&s.key));
            }
        }
        let used: Vec<(&String, &String)> = fitted.iter().filter(|(k, m)| *m != EMPTY && hull.slot(k).is_some()).collect();
        for l in &hull.limits {
            let n = used.iter().filter(|(_, m)| if l.module { **m == l.of } else { self.modules.get(*m).is_some_and(|md| md.category == l.of) }).count() as u32;
            if n > l.max {
                let what = if l.module { name(&l.of) } else { category_name(&l.of) };
                add(Severity::Error, format!("{n} modules of {what}: the {frame} takes at most {}", l.max), None);
            }
        }
        for (slot, m) in &used {
            let Some(md) = self.modules.get(*m) else { continue };
            for (other_slot, o) in &used {
                if other_slot == slot {
                    continue;
                }
                if let Some(od) = self.modules.get(*o)
                    && md.forbid.contains(&od.category)
                {
                    add(Severity::Error, format!("{}: {} cannot be on a design with {} ({})", slot_name(other_slot), name(o), name(m), slot_name(slot)), Some(other_slot));
                }
            }
        }
        out
    }

    /// The checks of a design of a nation that has `have` at the start.
    /// `others`: the nation's designs (for names taken twice).
    pub fn check_design(&self, d: &Design, have: &Have, others: &[Design], tag: &str, name: &dyn Fn(&str) -> String) -> Vec<Problem> {
        let mut out = Vec::new();
        let mut add = |severity: Severity, message: String, slot: Option<&str>| out.push(Problem { severity, message, slot: slot.map(String::from) });
        if d.name.trim().is_empty() {
            add(Severity::Error, "no name: a design needs name = \"...\" (the fleets name it in version_name)".into(), None);
        } else if others.iter().filter(|o| o.name == d.name && o.hull == d.hull).count() > 1 {
            add(Severity::Warning, format!("{tag} has more than one design \"{}\" on this hull: the fleets get the first", d.name), None);
        }
        if d.need.without.contains(MTG) {
            return out;
        }
        let Some(hull) = self.hull(&d.hull) else {
            let m = if d.hull.is_empty() {
                "no hull: a design needs type = <hull> (ship_hull_light_1 ...)".to_string()
            } else if self.equipment.contains_key(&d.hull) {
                format!("{} is not a hull of the ship designer (a family, or a ship without modules)", d.hull)
            } else {
                format!("{} is not equipment of the game or the mod", d.hull)
            };
            add(Severity::Error, m, None);
            return out;
        };
        if !self.is_hull(&d.hull) {
            add(Severity::Error, format!("{} is not a hull of the ship designer (a family, or a ship without modules)", d.hull), None);
            return out;
        }
        if !have.equipment.contains(&d.hull) {
            let techs = self.unlocked_text(&[d.hull.as_str()], name, 2);
            add(Severity::Error, if techs.is_empty() { format!("{tag} cannot build the {} hull", name(&d.hull)) } else { format!("{tag} cannot build the {} hull at the start: it comes with {techs}", name(&d.hull)) }, None);
        }
        for p in self.check_slots(&hull, &d.modules, have, tag, "hull", name) {
            add(p.severity, p.message, p.slot.as_deref());
        }
        let fitted = self.fitted(&hull, &d.modules);
        if self.role(&hull, &fitted).is_none() {
            let need: Vec<String> = self.roles(&d.hull).iter().flat_map(|u| u.modules_any.iter().map(|m| name(m))).collect();
            let m = if need.is_empty() { "no kind of ship of the game is built on this hull".to_string() } else { format!("it is no kind of ship: it needs {}", need.join(" or ")) };
            add(Severity::Warning, m, None);
        }
        out
    }
}

/// "Battery", "Front 1 custom" ... (the name the game shows is in the
/// localisation: `EQ_MOD_SLOT_<slot>_TITLE`).
pub fn slot_name(slot: &str) -> String {
    let s = slot.trim_start_matches("fixed_ship_").trim_start_matches("fixed_").trim_end_matches("_slot").replace('_', " ");
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => s,
    }
}

/// A category of modules as written ("ship light battery").
pub fn category_name(c: &str) -> String {
    c.replace('_', " ")
}

// ------------------------------------------------------------- designs

/// The designs of a nation's history (`create_equipment_variant` with a
/// hull of the designer), in the order written.
pub fn designs(db: &UnitDb, st: &Store, tag: &str) -> Vec<Design> {
    let mut out = Vec::new();
    let Some(p) = st.package(tag) else { return out };
    for (path, file) in &p.files {
        if !path.starts_with("history/countries/") {
            continue;
        }
        let Some(s) = &file.script else { continue };
        dlc::walk_flow(&s.root, &Need::default(), &mut |i, need| {
            if i.is("create_equipment_variant")
                && let Some(b) = i.block()
                && let Some(hull) = b.atom("type")
                && (db.is_hull(hull) || !db.equipment.contains_key(hull) && hull.starts_with("ship_hull"))
            {
                out.push(read_design(b, hull, need, tag, path, i.span.clone()));
            }
        });
    }
    out
}

fn read_design(b: &Block, hull: &str, need: &Need, pkg: &str, file: &str, span: Span) -> Design {
    let modules = b.block("modules").map(|m| m.items.iter().filter_map(|i| Some((i.key.as_ref()?.text.clone(), i.text()?.to_string()))).collect()).unwrap_or_default();
    Design { name: b.atom("name").unwrap_or("").to_string(), hull: hull.to_string(), modules, obsolete: b.yes("obsolete"), need: need.clone(), pkg: pkg.to_string(), file: file.to_string(), span }
}

/// The text of a design.
pub fn render_design(name: &str, hull: &str, modules: &[(String, String)], obsolete: bool) -> String {
    let mut s = format!("create_equipment_variant = {{\n\tname = {}\n\ttype = {hull}\n", script::quote(name));
    if !modules.is_empty() {
        s.push_str("\tmodules = {\n");
        for (k, v) in modules {
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

// ------------------------------------------------------------- fleets

/// A ship of a task force.
#[derive(Clone, Debug, PartialEq)]
pub struct Ship {
    pub name: String,
    pub definition: String,
    /// The hull of its `equipment`.
    pub hull: String,
    pub amount: Option<f64>,
    pub owner: Option<String>,
    /// The design (`version_name`).
    pub version: Option<String>,
    pub pride: bool,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TaskForce {
    pub name: String,
    pub location: Option<u32>,
    pub ships: Vec<Ship>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Fleet {
    pub name: String,
    pub base: Option<u32>,
    pub task_forces: Vec<TaskForce>,
    pub span: Span,
}

fn province(b: &Block, key: &str) -> Option<u32> {
    b.atom(key).and_then(|v| v.parse().ok())
}

/// The fleets of an order of battle (`units = { fleet = { ... } }`).
pub fn read_fleets(root: &Block) -> Vec<Fleet> {
    let mut out = Vec::new();
    for u in root.blocks("units") {
        for f in u.all("fleet") {
            let Some(fb) = f.block() else { continue };
            let mut tfs = Vec::new();
            for t in fb.all("task_force") {
                let Some(tb) = t.block() else { continue };
                let mut ships = Vec::new();
                for s in tb.all("ship") {
                    let Some(sb) = s.block() else { continue };
                    let eq = sb.block("equipment").and_then(|e| e.items.iter().find(|i| i.block().is_some()));
                    let eb = eq.and_then(|e| e.block());
                    ships.push(Ship {
                        name: sb.atom("name").unwrap_or("").to_string(),
                        definition: sb.atom("definition").unwrap_or("").to_string(),
                        hull: eq.map(|e| e.key().to_string()).unwrap_or_default(),
                        amount: eb.and_then(|b| b.num("amount")),
                        owner: eb.and_then(|b| b.atom("owner")).map(String::from),
                        version: eb.and_then(|b| b.atom("version_name")).map(String::from),
                        pride: sb.yes("pride_of_the_fleet"),
                        span: s.span.clone(),
                    });
                }
                tfs.push(TaskForce { name: tb.atom("name").unwrap_or("").to_string(), location: province(tb, "location"), ships, span: t.span.clone() });
            }
            out.push(Fleet { name: fb.atom("name").unwrap_or("").to_string(), base: province(fb, "naval_base"), task_forces: tfs, span: f.span.clone() });
        }
    }
    out
}

/// A `set_naval_oob` line of a nation's history.
#[derive(Clone, Debug, PartialEq)]
pub struct OobLine {
    /// The file's name (history/units/<name>.txt).
    pub name: String,
    /// The DLCs it is loaded for.
    pub need: Need,
    pub file: String,
    pub span: Span,
}

/// The `set_naval_oob` lines of a nation's history.
pub fn naval_oobs(st: &Store, tag: &str) -> Vec<OobLine> {
    let mut out = Vec::new();
    let Some(p) = st.package(tag) else { return out };
    for (path, file) in &p.files {
        if !path.starts_with("history/countries/") {
            continue;
        }
        let Some(s) = &file.script else { continue };
        dlc::walk_flow(&s.root, &Need::default(), &mut |i, need| {
            if i.is("set_naval_oob")
                && let Some(v) = i.text()
            {
                out.push(OobLine { name: v.to_string(), need: need.clone(), file: path.clone(), span: i.span.clone() });
            }
        });
    }
    out
}

/// The package that has a file of an order of battle (the nation's own
/// first, then the shared content's).
pub fn oob_file(st: &Store, tag: &str, name: &str) -> Option<(String, String)> {
    let path = format!("history/units/{name}.txt");
    let pkg = [tag, SHARED].into_iter().map(String::from).chain(st.packages.keys().cloned()).find(|p| st.file(p, &path).is_some())?;
    Some((pkg, path))
}

/// The naval order of battle of a nation for a player with `owned`: its
/// line, and the package and path of its file (None: not in the content).
pub fn naval_oob(st: &Store, tag: &str, owned: &Owned) -> Option<(OobLine, Option<(String, String)>)> {
    let line = naval_oobs(st, tag).into_iter().find(|l| l.need.holds(owned))?;
    let file = oob_file(st, tag, &line.name);
    Some((line, file))
}

/// The text of a ship of a task force.
pub fn render_ship(name: &str, definition: &str, hull: &str, owner: &str, version: &str, pride: bool) -> String {
    let pride = if pride { " pride_of_the_fleet = yes" } else { "" };
    format!("ship = {{ name = {} definition = {definition}{pride} equipment = {{ {hull} = {{ amount = 1 owner = {owner} version_name = {} }} }} }}", script::quote(name), script::quote(version))
}

pub fn render_task_force(name: &str, location: u32) -> String {
    format!("task_force = {{\n\tname = {}\n\tlocation = {location}\n}}", script::quote(name))
}

pub fn render_fleet(name: &str, base: u32, task_force: &str) -> String {
    format!("fleet = {{\n\tname = {}\n\tnaval_base = {base}\n\t{}\n}}", script::quote(name), task_force.replace('\n', "\n\t"))
}

/// Names for `n` new ships of a design: "<design> 1", "<design> 2" ...
/// (" Class" left out) that no ship of the nation has.
pub fn ship_names(design: &str, taken: &BTreeSet<String>, n: usize) -> Vec<String> {
    let base = design.trim().trim_end_matches(" Class").trim_end_matches(" class").trim();
    let base = if base.is_empty() { "Ship" } else { base };
    let mut out = Vec::new();
    let mut i = 1;
    while out.len() < n {
        let s = format!("{base} {i}");
        if !taken.contains(&s) {
            out.push(s);
        }
        i += 1;
    }
    out
}

/// A naval base of the map.
#[derive(Clone, Debug, PartialEq)]
pub struct Port {
    pub province: u32,
    pub level: u8,
    pub name: String,
    /// The nation of its state ("" for none).
    pub tag: String,
}

/// A problem of a fleet; `span` is the fleet's, task force's or ship's.
#[derive(Clone, Debug, PartialEq)]
pub struct FleetProblem {
    pub severity: Severity,
    pub message: String,
    pub span: Span,
}

/// Where to place a naval base when there is none.
pub const PLACE_PORT: &str = "place one in the map editor: Supply level, the port tool (N), on a coastal province";

fn port_name(p: &Port) -> String {
    if p.name.is_empty() { format!("province {}", p.province) } else { format!("{} (province {})", p.name, p.province) }
}

/// The checks of the fleets of a nation. `designs_of`: the designs of a
/// tag (the owner of a ship may be another nation).
pub fn check_fleets(db: &UnitDb, fleets: &[Fleet], tag: &str, ports: &[Port], designs_of: &dyn Fn(&str) -> Vec<Design>, name: &dyn Fn(&str) -> String) -> Vec<FleetProblem> {
    let mut out = Vec::new();
    let mut add = |severity: Severity, message: String, span: &Span| out.push(FleetProblem { severity, message, span: span.clone() });
    let own: Vec<&Port> = ports.iter().filter(|p| p.tag == tag).collect();
    let where_ = |what: &str, at: u32| -> Option<(Severity, String)> {
        match ports.iter().find(|p| p.province == at) {
            None if ports.is_empty() => Some((Severity::Error, format!("{what} = {at} is no naval base: the map has none yet ({PLACE_PORT})"))),
            None if own.is_empty() => Some((Severity::Error, format!("{what} = {at} is no naval base, and {tag} has none ({PLACE_PORT})"))),
            None => Some((Severity::Error, format!("{what} = {at} is no naval base of the map ({tag} has {})", own.iter().map(|p| port_name(p)).collect::<Vec<_>>().join(", ")))),
            Some(p) if p.tag != tag => Some((Severity::Warning, format!("{what} = {at}: {} is a naval base of {}, not of {tag} (the fleet needs docking rights there)", port_name(p), if p.tag.is_empty() { "no nation" } else { p.tag.as_str() }))),
            Some(_) => None,
        }
    };
    let mut names: BTreeMap<&str, usize> = BTreeMap::new();
    let mut prides = 0;
    let mut cache: BTreeMap<String, Vec<Design>> = BTreeMap::new();
    for f in fleets {
        if f.name.trim().is_empty() {
            add(Severity::Warning, "a fleet with no name".into(), &f.span);
        }
        match f.base {
            None => add(Severity::Error, format!("fleet \"{}\" has no naval_base: the province of the naval base it is stationed at", f.name), &f.span),
            Some(b) => {
                if let Some((s, m)) = where_("naval_base", b) {
                    add(s, format!("fleet \"{}\": {m}", f.name), &f.span);
                }
            }
        }
        if f.task_forces.is_empty() {
            add(Severity::Warning, format!("fleet \"{}\" has no task forces", f.name), &f.span);
        }
        for t in &f.task_forces {
            match t.location {
                None => add(Severity::Error, format!("task force \"{}\" has no location: the province of the naval base it starts at", t.name), &t.span),
                Some(l) => {
                    if let Some((s, m)) = where_("location", l) {
                        add(s, format!("task force \"{}\": {m}", t.name), &t.span);
                    }
                }
            }
            if t.ships.is_empty() {
                add(Severity::Warning, format!("task force \"{}\" has no ships", t.name), &t.span);
            }
            for s in &t.ships {
                let who = if s.name.is_empty() { "a ship".to_string() } else { format!("ship \"{}\"", s.name) };
                *names.entry(s.name.as_str()).or_default() += 1;
                if s.pride {
                    prides += 1;
                }
                let unit = db.ships.get(&s.definition);
                if s.definition.is_empty() {
                    add(Severity::Error, format!("{who} has no definition (destroyer, light_cruiser, battleship ...)"), &s.span);
                } else if unit.is_none() {
                    let kinds: Vec<&str> = db.ships.keys().map(|k| k.as_str()).collect();
                    add(Severity::Error, format!("{who}: definition = {} is no kind of ship ({})", s.definition, kinds.join(", ")), &s.span);
                }
                if s.hull.is_empty() {
                    add(Severity::Error, format!("{who} has no equipment = {{ <hull> = {{ amount = 1 owner = {tag} version_name = \"...\" }} }}"), &s.span);
                    continue;
                }
                if !db.equipment.contains_key(&s.hull) {
                    add(Severity::Error, format!("{who}: {} is not equipment of the game or the mod", s.hull), &s.span);
                    continue;
                }
                if let Some(u) = unit
                    && !db.roles(&s.hull).iter().any(|r| r.id == u.id)
                {
                    add(Severity::Error, format!("{who}: a {} is not built on the {} hull", name(&u.id), name(&s.hull)), &s.span);
                }
                if s.amount.is_some_and(|a| a != 1.0) {
                    add(Severity::Warning, format!("{who}: amount is {}; a ship is one hull (amount = 1)", s.amount.unwrap_or(0.0)), &s.span);
                }
                let owner = match &s.owner {
                    Some(o) => o.clone(),
                    None => {
                        add(Severity::Warning, format!("{who} has no owner (owner = {tag}: whose design it is)"), &s.span);
                        tag.to_string()
                    }
                };
                let Some(v) = &s.version else {
                    if db.is_hull(&s.hull) {
                        add(Severity::Warning, format!("{who} has no version_name: it gets the hull's default modules"), &s.span);
                    }
                    continue;
                };
                if !db.is_hull(&s.hull) {
                    continue;
                }
                let list = cache.entry(owner.clone()).or_insert_with(|| designs_of(&owner));
                let family = db.archetype_of(&s.hull).to_string();
                let found = list.iter().find(|d| d.name == *v && d.hull == s.hull).or_else(|| {
                    // on another hull of the family: the game takes it (its
                    // own fleets do this), but it is not what is written
                    let d = list.iter().find(|d| d.name == *v && db.archetype_of(&d.hull) == family)?;
                    add(Severity::Warning, format!("{who}: the design \"{v}\" of {owner} is on the {} hull, not {} (equipment = {{ {} = ... }})", name(&d.hull), name(&s.hull), d.hull), &s.span);
                    Some(d)
                });
                match found {
                    None => {
                        let on: Vec<String> = list.iter().filter(|d| d.hull == s.hull).map(|d| format!("\"{}\"", d.name)).collect();
                        let has = if on.is_empty() { format!("{owner} has no design on it") } else { format!("{owner} has {}", on.join(", ")) };
                        add(Severity::Error, format!("{who}: {owner} has no design \"{v}\" on the {} hull ({has})", name(&s.hull)), &s.span);
                    }
                    Some(d) => {
                        if let (Some(h), Some(u)) = (db.hull(&d.hull), unit) {
                            let fitted = db.fitted(&h, &d.modules);
                            if let Some(r) = db.role(&h, &fitted)
                                && r.id != u.id
                            {
                                add(Severity::Warning, format!("{who}: the design \"{v}\" makes a {}, not a {}", name(&r.id), name(&u.id)), &s.span);
                            }
                        }
                        if owner != tag {
                            add(Severity::Info, format!("{who} is of a design of {owner}"), &s.span);
                        }
                    }
                }
            }
        }
    }
    for f in fleets {
        for t in &f.task_forces {
            for s in &t.ships {
                if !s.name.is_empty() && names.get(s.name.as_str()).is_some_and(|n| *n > 1) {
                    add(Severity::Warning, format!("{tag} has more than one ship \"{}\"", s.name), &s.span);
                }
                if s.pride && prides > 1 {
                    add(Severity::Warning, format!("ship \"{}\": {tag} has {prides} prides of the fleet; the game takes one", s.name), &s.span);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::script::Script;

    const HULLS: &str = r#"equipments = {
	ship_hull_light = {
		year = 1922
		is_archetype = yes
		type = screen_ship
		module_slots = {
			fixed_ship_battery_slot = { required = yes allowed_module_categories = { ship_light_battery } }
			fixed_ship_radar_slot = { required = no allowed_module_categories = { ship_radar } }
			fixed_ship_engine_slot = { required = yes allowed_module_categories = { light_ship_engine } }
			mid_1_custom_slot = { required = no allowed_module_categories = { ship_torpedo ship_radar } }
		}
		module_count_limit = { category = ship_radar count < 2 }
		default_modules = {
			fixed_ship_battery_slot = empty
			fixed_ship_radar_slot = empty
			fixed_ship_engine_slot = light_ship_engine_1
			mid_1_custom_slot = empty
		}
		lg_attack = 0
		naval_speed = 32
		build_cost_ic = 400
		resources = { steel = 2 }
		manpower = 250
	}
	ship_hull_light_1 = {
		year = 1922
		archetype = ship_hull_light
		module_slots = inherit
	}
	ship_hull_light_2 = {
		year = 1936
		archetype = ship_hull_light
		parent = ship_hull_light_1
		module_slots = {
			fixed_ship_battery_slot = inherit
			fixed_ship_radar_slot = inherit
			fixed_ship_engine_slot = inherit
			front_1_custom_slot = { required = no allowed_module_categories = { ship_light_battery } }
			mid_1_custom_slot = { required = no allowed_module_categories = { ship_torpedo ship_light_battery ship_radar } }
		}
		default_modules = { fixed_ship_engine_slot = light_ship_engine_2 }
		build_cost_ic = 500
	}
	ship_hull_cruiser = {
		is_archetype = yes
		type = screen_ship
		module_slots = {
			fixed_ship_battery_slot = { required = yes allowed_module_categories = { ship_light_battery ship_medium_battery } }
		}
		naval_speed = 30
	}
	ship_hull_cruiser_1 = { archetype = ship_hull_cruiser module_slots = inherit active = yes }
	destroyer_1 = { archetype = destroyer year = 1922 }
}
"#;
    const MODULES: &str = r#"equipment_modules = {
	limit = { has_dlc = "Man the Guns" }
	ship_light_battery_1 = { category = ship_light_battery add_stats = { lg_attack = 1 build_cost_ic = 90 } multiply_stats = { naval_speed = -0.01 } add_average_stats = { lg_armor_piercing = 1 } }
	ship_light_battery_2 = { category = ship_light_battery add_stats = { lg_attack = 2 } add_average_stats = { lg_armor_piercing = 3 } }
	ship_medium_battery_1 = { category = ship_medium_battery add_equipment_type = capital_ship add_stats = { hg_attack = 6 } forbid_module_categories = { ship_light_battery } }
	ship_radar_1 = { category = ship_radar add_stats = { surface_detection = 10 } }
	ship_torpedo_1 = { category = ship_torpedo add_stats = { torpedo_attack = 20 } build_cost_resources = { chromium = 1 } }
	light_ship_engine_1 = { category = light_ship_engine add_stats = { fuel_consumption = 7 } multiply_stats = { naval_speed = 0.1 build_cost_ic = 0.125 } }
	light_ship_engine_2 = { category = light_ship_engine multiply_stats = { naval_speed = 0.15 } }
}
"#;
    const UNITS: &str = r#"sub_units = {
	destroyer = { map_icon_category = ship type = { screen_ship } need = { ship_hull_light = 1 } active = yes }
	light_cruiser = { map_icon_category = ship type = { screen_ship } need = { ship_hull_cruiser = 1 } active = yes }
	heavy_cruiser = { map_icon_category = ship type = { capital_ship } need = { ship_hull_cruiser = 1 } active = yes }
}
"#;
    const TECHS: &str = r#"technologies = {
	early_ship_hull_light = { enable_equipments = { ship_hull_light_1 } enable_equipment_modules = { light_ship_engine_1 } path = { leads_to_tech = basic_ship_hull_light } start_year = 1922 folder = { name = naval_folder } }
	basic_ship_hull_light = { enable_equipments = { ship_hull_light_2 } enable_equipment_modules = { light_ship_engine_2 } start_year = 1936 folder = { name = naval_folder } }
	basic_battery = { enable_equipment_modules = { ship_light_battery_1 ship_medium_battery_1 } start_year = 1922 folder = { name = naval_folder } }
	improved_battery = { enable_equipment_modules = { ship_light_battery_2 } start_year = 1936 folder = { name = naval_folder } }
	early_radar = { enable_equipment_modules = { ship_radar_1 } start_year = 1936 folder = { name = naval_folder } }
	torpedo = { enable_equipment_modules = { ship_torpedo_1 } start_year = 1922 folder = { name = naval_folder } }
}
"#;

    fn db() -> UnitDb {
        let mut db = UnitDb::default();
        db.read_file("common/units/equipment/ship_hull_light.txt", &Script::parse(HULLS), false);
        db.read_file("common/units/equipment/modules/00_ship_modules.txt", &Script::parse(MODULES), false);
        db.read_file("common/units/ships.txt", &Script::parse(UNITS), false);
        db.read_file("common/technologies/naval.txt", &Script::parse(TECHS), false);
        db
    }

    fn design(hull: &str, modules: &[(&str, &str)]) -> Design {
        Design { name: "Test Class".into(), hull: hull.into(), modules: modules.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect(), obsolete: false, need: Need::default(), pkg: "TYR".into(), file: "history/countries/TYR.txt".into(), span: 0..0 }
    }

    fn id(s: &str) -> String {
        s.to_string()
    }

    #[test]
    fn hulls_take_slots_and_defaults_through_inherit() {
        let db = db();
        assert_eq!(db.hulls().iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["ship_hull_light_1", "ship_hull_light_2", "ship_hull_cruiser_1"]);
        assert!(!db.is_hull("destroyer_1") && !db.is_hull("ship_hull_light"));
        let h1 = db.hull("ship_hull_light_1").unwrap();
        assert_eq!(h1.slots.iter().map(|s| s.key.as_str()).collect::<Vec<_>>(), ["fixed_ship_battery_slot", "fixed_ship_radar_slot", "fixed_ship_engine_slot", "mid_1_custom_slot"]);
        assert_eq!(h1.defaults.get("fixed_ship_engine_slot").map(|s| s.as_str()), Some("light_ship_engine_1"));
        assert_eq!(h1.limits.len(), 1);
        let h2 = db.hull("ship_hull_light_2").unwrap();
        // in the designer's order: fixed, then front to rear
        assert_eq!(h2.slots.iter().map(|s| s.key.as_str()).collect::<Vec<_>>(), ["fixed_ship_battery_slot", "fixed_ship_radar_slot", "fixed_ship_engine_slot", "front_1_custom_slot", "mid_1_custom_slot"]);
        assert!(h2.slot("fixed_ship_battery_slot").unwrap().required);
        assert!(h2.slot("mid_1_custom_slot").unwrap().categories.contains(&id("ship_light_battery")));
        assert_eq!(h2.defaults.get("fixed_ship_engine_slot").map(|s| s.as_str()), Some("light_ship_engine_2"));
        assert_eq!(h2.types, ["screen_ship"]);
    }

    #[test]
    fn stats_add_average_and_multiply() {
        let db = db();
        let h = db.hull("ship_hull_light_1").unwrap();
        let f = db.fitted(&h, &[(id("fixed_ship_battery_slot"), id("ship_light_battery_1")), (id("mid_1_custom_slot"), id("ship_torpedo_1"))]);
        let s = db.ship_stats(&h, &f);
        // 32 knots × (1 + 0.1 engine - 0.01 battery)
        assert!((s.get("naval_speed") - 32.0 * 1.09).abs() < 1e-9, "{:?}", s.stats);
        assert!((s.get("build_cost_ic") - 490.0 * 1.125).abs() < 1e-9);
        assert_eq!((s.get("lg_attack"), s.get("lg_armor_piercing"), s.get("torpedo_attack"), s.get("manpower")), (1.0, 1.0, 20.0, 250.0));
        assert_eq!(s.resources, [(id("steel"), 2.0), (id("chromium"), 1.0)].into());
        // two guns: piercing is their average
        let h2 = db.hull("ship_hull_light_2").unwrap();
        let f = db.fitted(&h2, &[(id("fixed_ship_battery_slot"), id("ship_light_battery_1")), (id("front_1_custom_slot"), id("ship_light_battery_2"))]);
        let s = db.ship_stats(&h2, &f);
        assert_eq!((s.get("lg_attack"), s.get("lg_armor_piercing")), (3.0, 2.0));
    }

    #[test]
    fn what_a_design_is() {
        let db = db();
        let h = db.hull("ship_hull_cruiser_1").unwrap();
        let light = db.fitted(&h, &[(id("fixed_ship_battery_slot"), id("ship_light_battery_1"))]);
        let heavy = db.fitted(&h, &[(id("fixed_ship_battery_slot"), id("ship_medium_battery_1"))]);
        assert_eq!(db.role(&h, &light).map(|u| u.id.as_str()), Some("light_cruiser"));
        assert_eq!(db.role(&h, &heavy).map(|u| u.id.as_str()), Some("heavy_cruiser"));
        let hl = db.hull("ship_hull_light_1").unwrap();
        assert_eq!(db.role(&hl, &db.fitted(&hl, &[])).map(|u| u.id.as_str()), Some("destroyer"));
    }

    #[test]
    fn designs_are_checked_against_research_and_slots() {
        let db = db();
        let have = db.have(["early_ship_hull_light".to_string(), "basic_battery".to_string()].into());
        let msgs = |d: &Design| db.check_design(d, &have, std::slice::from_ref(d), "TYR", &|i| i.to_string()).into_iter().map(|p| p.message).collect::<Vec<_>>();
        assert!(msgs(&design("ship_hull_light_1", &[("fixed_ship_battery_slot", "ship_light_battery_1")])).is_empty());
        assert_eq!(msgs(&design("ship_hull_light_1", &[])), ["Battery: the slot must have a module"]);
        assert_eq!(
            msgs(&design("ship_hull_light_2", &[("fixed_ship_battery_slot", "ship_light_battery_2"), ("fixed_ship_radar_slot", "ship_radar_1"), ("mid_1_custom_slot", "ship_radar_1"), ("front_1_custom_slot", "ship_torpedo_1")])),
            [
                "TYR cannot build the ship_hull_light_2 hull at the start: it comes with basic_ship_hull_light",
                "Battery: TYR cannot use ship_light_battery_2 at the start: it comes with improved_battery",
                "Radar: TYR cannot use ship_radar_1 at the start: it comes with early_radar",
                "Mid 1 custom: TYR cannot use ship_radar_1 at the start: it comes with early_radar",
                "Front 1 custom: ship_torpedo_1 does not go in this slot (it takes ship_light_battery)",
                "2 modules of ship radar: the hull takes at most 1",
            ]
        );
        let mixed = design("ship_hull_cruiser_1", &[("fixed_ship_battery_slot", "ship_medium_battery_1")]);
        assert!(msgs(&mixed).is_empty(), "{:?}", msgs(&mixed));
        assert_eq!(msgs(&design("ship_hull_light_1", &[("fixed_ship_battery_slot", "ship_light_battery_1"), ("rear_9_slot", "ship_torpedo_1"), ("rear_8_slot", "empty")])), ["rear_9_slot: the ship_hull_light_1 hull has no such slot"]);
        // the hull's defaults are written in; a default the nation lacks is a problem
        let d2 = design("ship_hull_light_2", &[("fixed_ship_battery_slot", "ship_light_battery_1")]);
        assert!(msgs(&d2).iter().any(|m| m.starts_with("TYR cannot build")));
        // the technologies to give: the hull's and the one before it
        let owned: Owned = [MTG.to_string()].into();
        assert_eq!(db.techs_to_give(&[id("ship_hull_light_2"), id("ship_radar_1")], &db.have(BTreeSet::new()), &owned), ["early_ship_hull_light", "basic_ship_hull_light", "early_radar"]);
        assert_eq!(db.techs_to_give(&[id("ship_hull_light_2")], &have, &owned), ["basic_ship_hull_light"]);
    }

    #[test]
    fn new_designs_get_what_the_nation_has() {
        let db = db();
        let have = db.have(["early_ship_hull_light".to_string(), "basic_battery".to_string(), "improved_battery".to_string()].into());
        let h = db.hull("ship_hull_light_1").unwrap();
        let m = db.new_modules(&h, &have, &BTreeMap::new());
        assert_eq!(m.iter().find(|x| x.0 == "fixed_ship_battery_slot").map(|x| x.1.as_str()), Some("ship_light_battery_2"));
        assert_eq!(m.iter().find(|x| x.0 == "fixed_ship_engine_slot").map(|x| x.1.as_str()), Some("light_ship_engine_1"));
        assert_eq!(m.iter().find(|x| x.0 == "mid_1_custom_slot").map(|x| x.1.as_str()), Some("empty"));
        let t = render_design("Etsch Class", "ship_hull_light_1", &m, false);
        let s = Script::parse(&t);
        let d = read_design(s.root.items[0].block().unwrap(), "ship_hull_light_1", &Need::default(), "TYR", "f", 0..0);
        assert_eq!((d.name.as_str(), d.modules.len()), ("Etsch Class", 4));
        assert!(db.check_design(&d, &have, std::slice::from_ref(&d), "TYR", &|i| i.to_string()).is_empty());
    }

    const OOB: &str = r#"units = {
	fleet = {
		name = "Lake Fleet"
		naval_base = 12
		task_force = {
			name = "First"
			location = 12
			ship = { name = "Etsch 1" definition = destroyer pride_of_the_fleet = yes equipment = { ship_hull_light_1 = { amount = 1 owner = TYR version_name = "Etsch Class" } } }
			ship = { name = "Etsch 1" definition = light_cruiser equipment = { ship_hull_light_1 = { amount = 1 owner = TYR version_name = "Other" } } }
		}
		task_force = { name = "Empty" location = 30 }
	}
	fleet = { name = "Nowhere" }
}
"#;

    #[test]
    fn fleets_are_read_and_checked_against_ports() {
        let db = db();
        let s = Script::parse(OOB);
        let fleets = read_fleets(&s.root);
        assert_eq!(fleets.len(), 2);
        assert_eq!((fleets[0].name.as_str(), fleets[0].base, fleets[0].task_forces.len()), ("Lake Fleet", Some(12), 2));
        let sh = &fleets[0].task_forces[0].ships[0];
        assert_eq!((sh.name.as_str(), sh.definition.as_str(), sh.hull.as_str(), sh.owner.as_deref(), sh.version.as_deref(), sh.pride), ("Etsch 1", "destroyer", "ship_hull_light_1", Some("TYR"), Some("Etsch Class"), true));
        let ports = vec![Port { province: 12, level: 3, name: "Riva".into(), tag: "TYR".into() }, Port { province: 30, level: 1, name: "Salò".into(), tag: "ITA".into() }];
        let d = design("ship_hull_light_1", &[("fixed_ship_battery_slot", "ship_light_battery_1")]);
        let designs = |t: &str| if t == "TYR" { vec![Design { name: "Etsch Class".into(), ..d.clone() }] } else { Vec::new() };
        let m: Vec<String> = check_fleets(&db, &fleets, "TYR", &ports, &designs, &|i| i.to_string()).into_iter().map(|p| format!("{} {}", p.severity.label(), p.message)).collect();
        let want = [
            "error ship \"Etsch 1\": a light_cruiser is not built on the ship_hull_light_1 hull",
            "error ship \"Etsch 1\": TYR has no design \"Other\" on the ship_hull_light_1 hull (TYR has \"Etsch Class\")",
            "warning task force \"Empty\": location = 30: Salò (province 30) is a naval base of ITA, not of TYR (the fleet needs docking rights there)",
            "warning task force \"Empty\" has no ships",
            "error fleet \"Nowhere\" has no naval_base: the province of the naval base it is stationed at",
            "warning fleet \"Nowhere\" has no task forces",
            "warning TYR has more than one ship \"Etsch 1\"",
            "warning TYR has more than one ship \"Etsch 1\"",
        ];
        assert_eq!(m, want.map(String::from));
        // no ports on the map: where to place one
        let m = check_fleets(&db, &fleets[..1], "TYR", &[], &designs, &|i| i.to_string());
        assert!(m.iter().any(|p| p.message.contains("the map has none yet") && p.message.contains("port tool")), "{m:?}");
    }

    #[test]
    fn ships_get_free_names() {
        let taken: BTreeSet<String> = ["Etsch 1".to_string()].into();
        assert_eq!(ship_names("Etsch Class", &taken, 2), ["Etsch 2", "Etsch 3"]);
        let t = format!("units = {{\n\t{}\n}}\n", render_fleet("F", 5, &render_task_force("T", 5)));
        let s = Script::parse(&t);
        let f = read_fleets(&s.root);
        assert_eq!((f[0].name.as_str(), f[0].base, f[0].task_forces[0].location), ("F", Some(5), Some(5)));
        let ship = render_ship("Etsch 2", "destroyer", "ship_hull_light_1", "TYR", "Etsch Class", false);
        let s = Script::parse(&format!("units = {{ fleet = {{ task_force = {{ {ship} }} }} }}"));
        let f = read_fleets(&s.root);
        assert_eq!(f[0].task_forces[0].ships[0].version.as_deref(), Some("Etsch Class"));
    }

    /// The game's own designs of the great powers fit the game's hulls, and
    /// the ships of their 1936 fleets the kinds of ship and the designs
    /// (with the game installed).
    #[test]
    #[ignore]
    fn game_designs_fit() {
        let game = crate::modbuild::default_game_dir();
        if !game.join("common/units").is_dir() {
            println!("HOI4 not found at {}: not tested", game.display());
            return;
        }
        let files = crate::content::source::base_files(&game);
        let mut db = UnitDb::default();
        for dir in crate::content::units::FOLDERS {
            for path in files.list(dir, "txt", false) {
                if let Some(t) = files.text(&path) {
                    db.read_file(&path, &Script::parse(&t), false);
                }
            }
        }
        assert!(db.hulls().len() > 20);
        let all: BTreeSet<String> = db.techs.keys().cloned().collect();
        let have = db.have(all);
        let owned: Owned = [MTG.to_string()].into();
        let mut by_tag: BTreeMap<String, Vec<Design>> = BTreeMap::new();
        let mut oobs: Vec<(String, String)> = Vec::new();
        for path in files.list("history/countries", "txt", false) {
            let tag = path.rsplit('/').next().unwrap_or("").chars().take(3).collect::<String>();
            let Some(text) = files.text(&path) else { continue };
            let s = Script::parse(&text);
            let ds = by_tag.entry(tag.clone()).or_default();
            dlc::walk_flow(&s.root, &Need::default(), &mut |i, need| {
                if i.is("create_equipment_variant")
                    && let Some(b) = i.block()
                    && let Some(h) = b.atom("type")
                    && db.is_hull(h)
                {
                    ds.push(read_design(b, h, need, &tag, "x", i.span.clone()));
                }
                if i.is("set_naval_oob")
                    && need.holds(&owned)
                    && let Some(v) = i.text()
                {
                    oobs.push((tag.clone(), v.to_string()));
                }
            });
        }
        let mut n = 0;
        for (tag, ds) in &by_tag {
            for d in ds {
                n += 1;
                let p: Vec<_> = db.check_design(d, &have, ds, tag, &|i| i.to_string()).into_iter().filter(|p| p.severity == Severity::Error).collect();
                assert!(p.is_empty(), "{tag} {}: {p:?}", d.name);
            }
        }
        assert!(n > 100, "{n}");
        let eng = &by_tag["ENG"];
        let dd = eng.iter().find(|d| d.hull.starts_with("ship_hull_light")).unwrap();
        let h = db.hull(&dd.hull).unwrap();
        let st = db.ship_stats(&h, &db.fitted(&h, &dd.modules));
        assert!((28.0..40.0).contains(&st.get("naval_speed")), "{:?}", st.stats);
        assert_eq!(db.role(&h, &db.fitted(&h, &dd.modules)).map(|u| u.id.as_str()), Some("destroyer"));
        // the fleets: kinds and designs (ports are the game map's: not checked)
        let designs = |t: &str| by_tag.get(t).cloned().unwrap_or_default();
        let mut ships = 0;
        let mut wrong_kind = Vec::new();
        for (tag, oob) in &oobs {
            let Some(text) = files.text(&format!("history/units/{oob}.txt")) else { continue };
            let fleets = read_fleets(&Script::parse(&text).root);
            ships += fleets.iter().flat_map(|f| &f.task_forces).map(|t| t.ships.len()).sum::<usize>();
            let ports: Vec<Port> = fleets.iter().flat_map(|f| f.base.into_iter().chain(f.task_forces.iter().filter_map(|t| t.location))).map(|p| Port { province: p, level: 1, name: String::new(), tag: tag.clone() }).collect();
            for p in check_fleets(&db, &fleets, tag, &ports, &designs, &|i| i.to_string()) {
                // the game's files have a few ships of designs of another
                // nation (Canada's of Quebec's): a slip of theirs
                if p.severity == Severity::Error && !p.message.contains("has no design \"Saguenay") {
                    panic!("{tag} {oob}: {}", p.message);
                }
                if p.severity != Severity::Info {
                    wrong_kind.push(format!("{tag} {oob}: {}", p.message));
                }
            }
        }
        assert!(ships > 300, "{ships}");
        println!("{} ships; {} the checks find:\n{}", ships, wrong_kind.len(), wrong_kind.join("\n"));
    }
}
