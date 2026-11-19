//! What the game allows of buildings: the highest level of each
//! (`level_cap` in common/buildings), which share the state's building
//! slots, which need a coast, which exclude each other, and the slots and
//! level limits of each state category (common/state_category). Read from
//! the HOI4 install (its DLCs too); the vanilla values when it is not there.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::content::script::{Block, Script};
use crate::content::source::{Files, base_files};

/// What the game allows of one building.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Building {
    /// The highest level in a state (None: no cap of its own).
    pub state_max: Option<u32>,
    /// The highest level in one province (naval bases, bunkers, ...).
    pub province_max: Option<u32>,
    /// It takes the state's building slots (factories, dockyards, ...).
    pub shares_slots: bool,
    /// Only in a state on the coast.
    pub coastal: bool,
    /// It cannot be in the same state as these.
    pub exclusive_with: Vec<String>,
}

impl Building {
    /// Built per province (a naval base, a bunker), not per state.
    pub fn per_province(&self) -> bool {
        self.state_max.is_none() && self.province_max.is_some()
    }
}

/// A state category: its building slots, and lower highest levels of some
/// buildings (islands: naval bases, air bases).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Category {
    pub slots: u32,
    pub max_level: BTreeMap<String, u32>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Caps {
    pub buildings: BTreeMap<String, Building>,
    pub categories: BTreeMap<String, Category>,
}

/// The vanilla values (HOI4 1.16), in the game's own words.
const VANILLA_BUILDINGS: &str = "buildings = {
	infrastructure = { level_cap = { state_max = 5 } }
	arms_factory = { level_cap = { shares_slots = yes state_max = 20 } }
	industrial_complex = { level_cap = { shares_slots = yes state_max = 20 } }
	air_base = { level_cap = { state_max = 10 } }
	supply_node = { level_cap = { province_max = 1 } }
	rail_way = { level_cap = { province_max = 1 } }
	naval_base = { only_costal = yes level_cap = { province_max = 10 } }
	bunker = { level_cap = { province_max = 10 } }
	coastal_bunker = { only_costal = yes level_cap = { province_max = 10 } }
	stronghold_network = { level_cap = { state_max = 1 shares_slots = yes } }
	dockyard = { only_costal = yes level_cap = { shares_slots = yes state_max = 20 } }
	anti_air_building = { level_cap = { state_max = 5 } }
	synthetic_refinery = { level_cap = { shares_slots = yes state_max = 3 } }
	fuel_silo = { level_cap = { shares_slots = yes } }
	radar_station = { level_cap = { state_max = 6 } }
	mega_gun_emplacement = { level_cap = { state_max = 1 shares_slots = yes exclusive_with = rocket_site } }
	rocket_site = { level_cap = { state_max = 3 shares_slots = yes exclusive_with = mega_gun_emplacement } }
	naval_supply_hub = { only_costal = yes level_cap = { province_max = 1 } }
	naval_headquarters = { only_costal = yes level_cap = { province_max = 1 } }
	nuclear_reactor = { level_cap = { state_max = 1 shares_slots = yes } }
}";

const VANILLA_CATEGORIES: &str = "state_categories = {
	wasteland = { local_building_slots = 0 }
	enclave = { local_building_slots = 0 }
	tiny_island = { local_building_slots = 0 buildings_max_level = { naval_base = 3 air_base = 2 } }
	small_island = { local_building_slots = 1 buildings_max_level = { naval_base = 6 air_base = 4 } }
	pastoral = { local_building_slots = 1 }
	rural = { local_building_slots = 2 }
	town = { local_building_slots = 4 }
	large_town = { local_building_slots = 5 }
	city = { local_building_slots = 6 }
	large_city = { local_building_slots = 8 }
	metropolis = { local_building_slots = 10 }
	megalopolis = { local_building_slots = 12 }
	large_island = { local_building_slots = 3 buildings_max_level = { naval_base = 8 air_base = 6 } }
}";

fn whole(b: &Block, key: &str) -> Option<u32> {
    b.num(key).filter(|v| *v >= 0.0).map(|v| v as u32)
}

fn yes(b: &Block, key: &str) -> bool {
    b.atom(key).is_some_and(|v| v.eq_ignore_ascii_case("yes"))
}

fn read_buildings(text: &str, out: &mut BTreeMap<String, Building>) {
    let s = Script::parse(text);
    for list in s.root.blocks("buildings") {
        for item in &list.items {
            let (Some(k), Some(b)) = (item.key.as_ref(), item.block()) else { continue };
            let cap = b.block("level_cap");
            let mut exclusive = Vec::new();
            if let Some(c) = cap {
                for e in c.all("exclusive_with") {
                    match e.block() {
                        Some(list) => exclusive.extend(list.values().map(str::to_string)),
                        None => exclusive.extend(e.text().map(str::to_string)),
                    }
                }
            }
            out.insert(
                k.text.clone(),
                Building {
                    state_max: cap.and_then(|c| whole(c, "state_max")),
                    province_max: cap.and_then(|c| whole(c, "province_max")),
                    shares_slots: cap.is_some_and(|c| yes(c, "shares_slots")),
                    coastal: yes(b, "only_costal"),
                    exclusive_with: exclusive,
                },
            );
        }
    }
}

fn read_categories(text: &str, out: &mut BTreeMap<String, Category>) {
    let s = Script::parse(text);
    for list in s.root.blocks("state_categories") {
        for item in &list.items {
            let (Some(k), Some(b)) = (item.key.as_ref(), item.block()) else { continue };
            let max_level = b
                .block("buildings_max_level")
                .map(|m| m.items.iter().filter_map(|i| Some((i.key.as_ref()?.text.clone(), i.text()?.parse::<f64>().ok()? as u32))).collect())
                .unwrap_or_default();
            out.insert(k.text.clone(), Category { slots: whole(b, "local_building_slots").unwrap_or(0), max_level });
        }
    }
}

impl Caps {
    pub fn vanilla() -> Caps {
        let mut c = Caps::default();
        read_buildings(VANILLA_BUILDINGS, &mut c.buildings);
        read_categories(VANILLA_CATEGORIES, &mut c.categories);
        c
    }

    /// From the game's files; what they lack (no install) is vanilla's.
    pub fn read(files: &Files) -> Caps {
        let mut c = Caps::default();
        for p in files.list("common/buildings", "txt", false) {
            if let Some(t) = files.text(&p) {
                read_buildings(&t, &mut c.buildings);
            }
        }
        for p in files.list("common/state_category", "txt", false) {
            if let Some(t) = files.text(&p) {
                read_categories(&t, &mut c.categories);
            }
        }
        let v = Caps::vanilla();
        if c.buildings.is_empty() {
            c.buildings = v.buildings;
        }
        if c.categories.is_empty() {
            c.categories = v.categories;
        }
        c
    }

    pub fn building(&self, b: &str) -> Option<&Building> {
        self.buildings.get(b)
    }

    /// The highest level of a building: its own cap (a state's, else a
    /// province's), lowered by the state category (islands). None: no cap
    /// of its own, or a building the game does not have.
    pub fn max_level(&self, b: &str, category: Option<&str>) -> Option<u32> {
        let x = self.buildings.get(b)?;
        let own = x.state_max.or(x.province_max);
        let by_cat = category.and_then(|c| self.categories.get(c)).and_then(|c| c.max_level.get(b).copied());
        match (own, by_cat) {
            (Some(a), Some(c)) => Some(a.min(c)),
            (a, c) => a.or(c),
        }
    }

    pub fn slots(&self, category: &str) -> Option<u32> {
        self.categories.get(category).map(|c| c.slots)
    }

    /// The buildings of a state that take its building slots, and how many.
    pub fn used_slots(&self, buildings: &BTreeMap<String, u32>) -> u32 {
        buildings.iter().filter(|(k, _)| self.buildings.get(*k).is_some_and(|b| b.shares_slots)).map(|(_, v)| *v).sum()
    }

    /// One level of a building that is too high, else None.
    pub fn level_problem(&self, b: &str, level: u32, category: Option<&str>) -> Option<String> {
        let max = self.max_level(b, category)?;
        (level > max).then(|| {
            let island = category.filter(|c| self.categories.get(*c).is_some_and(|x| x.max_level.contains_key(b)));
            match island {
                Some(c) => format!("{b} {level}: at most {max} in a {c} state"),
                None => format!("{b} {level}: at most {max}"),
            }
        })
    }

    /// What the game does not allow of a state's buildings: levels over
    /// the cap (`category` None: not known), a coastal building inland, two
    /// that exclude each other, a building the game does not have.
    pub fn problems(&self, buildings: &BTreeMap<String, u32>, category: Option<&str>, coastal: bool) -> Vec<String> {
        let mut out = Vec::new();
        for (k, &v) in buildings {
            let Some(b) = self.buildings.get(k) else {
                out.push(format!("{k}: the game has no such building"));
                continue;
            };
            out.extend(self.level_problem(k, v, category));
            if b.coastal && !coastal && v > 0 {
                out.push(format!("{k}: only in a state on the coast"));
            }
            for e in &b.exclusive_with {
                if k < e && v > 0 && buildings.get(e).is_some_and(|&w| w > 0) {
                    out.push(format!("{k} and {e} cannot be in the same state"));
                }
            }
        }
        out
    }

    /// More buildings that take slots than the category has (extra slots
    /// can come from add_extra_state_shared_building_slots).
    pub fn slot_problem(&self, buildings: &BTreeMap<String, u32>, category: &str) -> Option<String> {
        let slots = self.slots(category)?;
        let used = self.used_slots(buildings);
        (used > slots).then(|| format!("{used} buildings take building slots; a {category} state has {slots}"))
    }
}

static CACHE: Mutex<Option<(PathBuf, Arc<Caps>)>> = Mutex::new(None);

/// The limits of the HOI4 install at `game` (read once, then kept).
pub fn for_game(game: &Path) -> Arc<Caps> {
    let mut c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((p, caps)) = c.as_ref()
        && p == game
    {
        return caps.clone();
    }
    let caps = Arc::new(Caps::read(&base_files(game)));
    *c = Some((game.to_path_buf(), caps.clone()));
    caps
}

/// The limits of the game whose files these are (an install: kept).
pub fn of_files(files: &Files) -> Arc<Caps> {
    match files {
        Files::Layers(roots) if !roots.is_empty() => for_game(&roots[0]),
        _ => Arc::new(Caps::read(files)),
    }
}

/// The limits read last (those of the install the editor uses), else the
/// default install's.
pub fn current() -> Arc<Caps> {
    if let Some((_, caps)) = CACHE.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        return caps.clone();
    }
    for_game(&super::default_game_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(v: &[(&str, u32)]) -> BTreeMap<String, u32> {
        v.iter().map(|(k, n)| (k.to_string(), *n)).collect()
    }

    #[test]
    fn vanilla_caps() {
        let c = Caps::vanilla();
        assert_eq!(c.max_level("infrastructure", None), Some(5));
        assert_eq!(c.max_level("air_base", Some("town")), Some(10));
        assert_eq!(c.max_level("air_base", Some("small_island")), Some(4), "islands lower it");
        assert_eq!(c.max_level("naval_base", None), Some(10));
        assert!(c.building("naval_base").unwrap().per_province());
        assert_eq!(c.max_level("fuel_silo", None), None, "no cap of its own: the slots are");
        assert_eq!(c.slots("town"), Some(4));
        assert_eq!(c.building("rocket_site").unwrap().exclusive_with, ["mega_gun_emplacement"]);
        // the slot table of the build is the same
        for (k, n) in super::super::CATEGORY_SLOTS {
            assert_eq!(c.slots(k), Some(n), "{k}");
        }
        for k in super::super::SLOT_BUILDINGS {
            assert!(c.building(k).unwrap().shares_slots, "{k}");
        }
    }

    #[test]
    fn state_problems() {
        let c = Caps::vanilla();
        assert!(c.problems(&map(&[("infrastructure", 5), ("industrial_complex", 2), ("arms_factory", 2)]), Some("town"), false).is_empty());
        let b = map(&[("infrastructure", 7), ("industrial_complex", 3), ("arms_factory", 2), ("dockyard", 1)]);
        assert_eq!(c.problems(&b, Some("town"), false), ["dockyard: only in a state on the coast", "infrastructure 7: at most 5"]);
        assert_eq!(c.slot_problem(&b, "town").as_deref(), Some("6 buildings take building slots; a town state has 4"));
        assert_eq!(c.slot_problem(&b, "metropolis"), None);
        let b = map(&[("air_base", 6), ("rocket_site", 1), ("mega_gun_emplacement", 1), ("castle", 1)]);
        assert_eq!(
            c.problems(&b, Some("small_island"), true),
            ["air_base 6: at most 4 in a small_island state", "castle: the game has no such building", "mega_gun_emplacement and rocket_site cannot be in the same state"]
        );
        assert_eq!(c.slot_problem(&b, "small_island").as_deref(), Some("2 buildings take building slots; a small_island state has 1"));
        // the category not known: only its own cap
        assert!(c.problems(&map(&[("industrial_complex", 9)]), None, false).is_empty());
        assert_eq!(c.problems(&map(&[("air_base", 6)]), None, false), Vec::<String>::new());
    }

    #[test]
    fn game_caps() {
        let game = super::super::default_game_dir();
        if !game.join("common/buildings").is_dir() {
            println!("HOI4 not found at {}: not tested", game.display());
            return;
        }
        let c = Caps::read(&base_files(&game));
        let v = Caps::vanilla();
        for (k, b) in &v.buildings {
            assert_eq!(c.building(k), Some(b), "{k} differs from the game's");
        }
        for (k, x) in &v.categories {
            assert_eq!(c.categories.get(k), Some(x), "{k} differs from the game's");
        }
    }
}
