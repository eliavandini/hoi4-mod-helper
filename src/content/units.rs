//! Land units for the division designer: the battalions of the game and the
//! mod (`common/units`), their equipment and its modules
//! (`common/units/equipment`), and the technologies that unlock and improve
//! them (`common/technologies`); which battalions a nation can use at the
//! start; the checks of a division template; and its stats, worked out the
//! way the game's designer does.
//!
//! How the stats are made (from the game's files and the wiki):
//! - A battalion's attacks, defence, breakthrough, piercing, armour and
//!   hardness are the sum of those of the equipment it needs (the newest
//!   model the nation has of each), times one plus the battalion's own value
//!   of that stat (`soft_attack = -0.5` halves it) and the technologies'
//!   bonuses to it or to its categories.
//! - Its speed is that of its `transport` equipment, else of its slowest
//!   equipment, times one plus its own and the technologies' bonus.
//! - Organisation, HP, recovery, recon, entrenchment, initiative and the
//!   like are its own values plus the technologies'.
//! - Support companies with `battalion_mult` raise the stats of the line
//!   battalions of a category.
//! - The division sums attacks, defence, breakthrough, HP, recon,
//!   suppression, supply use, weight, manpower and combat width; averages
//!   organisation and recovery over all battalions and companies, hardness
//!   over line battalions; takes 40% of the highest plus 60% of the average
//!   armour and piercing, the slowest speed, the longest training time;
//!   averages terrain bonuses over line battalions and adds the support
//!   companies'.
//!
//! Left out: doctrines, national spirits, generals, experience, and
//! equipment designs of the nation (tanks use the game's default design).
//!
//! What a nation has at the start is what its history gives
//! (`set_technology`, `add_equipment_to_stockpile`), for players with the
//! DLCs its `has_dlc` checks name or without them ([`super::dlc`]): a
//! template is checked for each (the checks that fail only for some say
//! for whom). Research by year keeps to the technologies a player with the
//! chosen DLCs sees (No Step Back has its own tank technologies).

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::sync::{Arc, Mutex};

use super::dlc::{self, Need, Owned};
use super::script::{Block, Script};
use super::source::Index;
use super::store::Store;
use super::validate::Severity;

pub type Stats = BTreeMap<String, f64>;

/// Bonuses in a terrain, as fractions (0.1 is +10%).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Terrain {
    pub attack: f64,
    pub defence: f64,
    pub movement: f64,
}

impl Terrain {
    fn read(b: &Block) -> Terrain {
        Terrain { attack: b.num("attack").unwrap_or(0.0), defence: b.num("defence").or(b.num("defense")).unwrap_or(0.0), movement: b.num("movement").unwrap_or(0.0) }
    }
    fn add(&mut self, o: &Terrain) {
        self.attack += o.attack;
        self.defence += o.defence;
        self.movement += o.movement;
    }
    fn scale(&self, f: f64) -> Terrain {
        Terrain { attack: self.attack * f, defence: self.defence * f, movement: self.movement * f }
    }
    pub fn is_zero(&self) -> bool {
        self.attack.abs() < 1e-9 && self.defence.abs() < 1e-9 && self.movement.abs() < 1e-9
    }
}

/// The terrains (and river crossings, landings, forts) units have bonuses in.
pub const TERRAINS: [&str; 12] = ["plains", "forest", "hills", "mountain", "jungle", "marsh", "urban", "desert", "river", "amphibious", "fort", "snow"];

fn numbers(b: &Block) -> Stats {
    b.items.iter().filter_map(|i| Some((i.key.as_ref()?.text.clone(), i.text()?.parse::<f64>().ok()?))).collect()
}

fn words(b: Option<&Block>) -> Vec<String> {
    b.map(|b| b.values().map(String::from).collect()).unwrap_or_default()
}

fn no(b: &Block, key: &str) -> bool {
    b.atom(key).is_some_and(|v| v.eq_ignore_ascii_case("no"))
}

fn terrains(b: &Block) -> BTreeMap<String, Terrain> {
    b.items.iter().filter_map(|i| Some((i.key.as_ref().filter(|k| TERRAINS.contains(&k.text.as_str()))?.text.clone(), Terrain::read(i.block()?)))).collect()
}

/// A battalion or support company type.
#[derive(Clone, Debug, Default)]
pub struct SubUnit {
    pub id: String,
    pub abbreviation: String,
    /// Its icon is `GFX_unit_<sprite>_icon_medium`.
    pub sprite: String,
    /// infantry, mobile, armor, combat_support, ... support.
    pub group: String,
    pub categories: Vec<String>,
    /// Every nation has it (no technology needed).
    pub active: bool,
    /// Can be a battalion of a column (`regimental = no`: support only).
    pub regimental: bool,
    /// Can be a support company of the division (`divisional = no`).
    pub divisional: bool,
    /// Only in army headquarters (`allow_in_non_army_hq = no`).
    pub army_hq_only: bool,
    /// A regimental support company: one on top of a column.
    pub regimental_support: bool,
    /// Groups of the columns a regimental support company can go on.
    pub allowed_groups: Vec<String>,
    /// Support companies of one such type exclude each other.
    pub same_support_type: Option<String>,
    pub special_forces: bool,
    /// Its speed counts for the division's (`affects_speed = no`: not).
    pub affects_speed: bool,
    /// Equipment it needs: (equipment, how many).
    pub need: Vec<(String, f64)>,
    /// The equipment that moves it (sets its speed).
    pub transport: Option<String>,
    /// Its numbers as written (`combat_width`, `max_organisation`,
    /// `soft_attack` as a bonus, ...).
    pub stats: Stats,
    pub terrain: BTreeMap<String, Terrain>,
    /// As a support company: bonuses to the line battalions of a category.
    pub battalion_mult: Vec<(String, Stats)>,
    /// Defined by the mod's content (else the game).
    pub content: bool,
}

impl SubUnit {
    fn read(id: &str, b: &Block, content: bool) -> SubUnit {
        let categories = words(b.block("categories"));
        SubUnit {
            id: id.to_string(),
            abbreviation: b.atom("abbreviation").unwrap_or("").to_string(),
            sprite: b.atom("sprite").unwrap_or(id).to_string(),
            group: b.atom("group").unwrap_or("infantry").to_string(),
            regimental_support: categories.iter().any(|c| c == "category_regimental_support_battalions"),
            categories,
            active: b.yes("active"),
            regimental: !no(b, "regimental"),
            divisional: !no(b, "divisional"),
            army_hq_only: no(b, "allow_in_non_army_hq"),
            allowed_groups: words(b.block("allowed_battalion_groups")),
            same_support_type: b.atom("same_support_type").map(String::from),
            special_forces: b.yes("special_forces"),
            affects_speed: !no(b, "affects_speed"),
            need: b.block("need").map(|n| n.items.iter().filter_map(|i| Some((i.key.as_ref()?.text.clone(), i.text()?.parse().ok()?))).collect()).unwrap_or_default(),
            transport: b.atom("transport").map(String::from),
            stats: numbers(b),
            terrain: terrains(b),
            battalion_mult: b.blocks("battalion_mult").filter_map(|m| Some((m.atom("category")?.to_string(), numbers(m)))).collect(),
            content,
        }
    }
    /// A land unit: it has a group or a combat width, or is new (nothing
    /// says what it is); planes (`type = fighter`), railway guns and ships
    /// are left out.
    fn is_land(b: &Block) -> bool {
        !matches!(b.atom("map_icon_category"), Some("ship" | "transport" | "uboat")) && (b.get("group").is_some() || b.get("combat_width").is_some() || b.get("type").is_none())
    }
    /// Where it can go in a division template.
    pub fn section(&self) -> Option<Section> {
        if self.army_hq_only {
            None
        } else if self.regimental_support {
            Some(Section::RegimentalSupport)
        } else if self.group == "support" {
            self.divisional.then_some(Section::Support)
        } else {
            self.regimental.then_some(Section::Regiments)
        }
    }
    pub fn stat(&self, k: &str) -> f64 {
        self.stats.get(k).copied().unwrap_or(0.0)
    }
    /// The sprite of its icon in the designer.
    pub fn icon(&self) -> String {
        format!("GFX_unit_{}_icon_medium", self.sprite)
    }
}

/// A model of equipment, or an archetype (the family of models).
#[derive(Clone, Debug, Default)]
pub struct Equipment {
    pub id: String,
    pub archetype: Option<String>,
    pub is_archetype: bool,
    pub year: f64,
    /// Every nation has it.
    pub active: bool,
    pub stats: Stats,
    /// Built of modules (tanks, ships): slot → module of the default design.
    pub modular: bool,
    pub default_modules: BTreeMap<String, String>,
    /// Its own `module_slots` (None: none, or `inherit`: its parent's or
    /// archetype's), slot by slot.
    pub slots: Option<Vec<(String, SlotSrc)>>,
    /// The model it follows (`parent`).
    pub parent: Option<String>,
    /// `type = screen_ship`: what it is in combat.
    pub types: Vec<String>,
    /// Resources one factory takes to make it.
    pub resources: Stats,
    /// `module_count_limit`: at most so many modules of a category (or one
    /// module).
    pub count_limits: Vec<CountLimit>,
    /// The model of before the designer it stands for
    /// (`derived_variant_name`, a plane's `variant_name` of its family):
    /// `small_plane_airframe_1` → `fighter_equipment_1`.
    pub legacy: Option<String>,
    /// A plane family's types a design can take (`allowed_types`), by the
    /// module of the slot `type_override` names.
    pub allowed_types: Vec<String>,
    pub type_override: Option<String>,
    /// The upgrades of a model without designer (`upgrades = { ... }`).
    pub upgrades: Vec<String>,
    /// Made by the game from another family (`duplicate_archetypes`).
    pub derived: bool,
}

/// A slot of a design: whether it must be filled and the categories of
/// modules it takes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SlotDef {
    pub required: bool,
    pub categories: Vec<String>,
}

/// Where the definition of a slot of a model is.
#[derive(Clone, Debug, PartialEq)]
pub enum SlotSrc {
    Own(SlotDef),
    /// `slot = inherit`: that of the model it follows.
    Inherit,
    /// `mid_2_custom_slot = mid_1_custom_slot`: as another slot.
    Like(String),
}

/// `module_count_limit = { category = ship_radar count < 2 }`.
#[derive(Clone, Debug, PartialEq)]
pub struct CountLimit {
    /// A category of modules, or one module.
    pub of: String,
    pub module: bool,
    pub max: u32,
}

impl CountLimit {
    fn read(b: &Block) -> Option<CountLimit> {
        let (of, module) = match (b.atom("category"), b.atom("module")) {
            (Some(c), _) => (c.to_string(), false),
            (None, Some(m)) => (m.to_string(), true),
            _ => return None,
        };
        let it = b.get("count")?;
        let n: f64 = it.text()?.parse().ok()?;
        let max = match it.op.as_ref().map(|o| o.0) {
            Some(super::script::Op::Lt) => n - 1.0,
            Some(super::script::Op::Le | super::script::Op::Eq) => n,
            _ => return None,
        };
        Some(CountLimit { of, module, max: max.max(0.0) as u32 })
    }
}

impl Equipment {
    fn read(id: &str, b: &Block) -> Equipment {
        let slots = b.block("module_slots").map(|s| {
            s.items
                .iter()
                .filter_map(|i| {
                    let k = i.key.as_ref()?.text.clone();
                    let def = match (i.block(), i.text()) {
                        (Some(d), _) => SlotSrc::Own(SlotDef { required: d.yes("required"), categories: words(d.block("allowed_module_categories")) }),
                        (None, Some(v)) if v != "inherit" => SlotSrc::Like(v.to_string()),
                        _ => SlotSrc::Inherit,
                    };
                    Some((k, def))
                })
                .collect()
        });
        let types = match b.get("type") {
            Some(i) => match i.block() {
                Some(t) => t.values().map(String::from).collect(),
                None => i.text().map(|t| vec![t.to_string()]).unwrap_or_default(),
            },
            None => Vec::new(),
        };
        Equipment {
            id: id.to_string(),
            archetype: b.atom("archetype").map(String::from),
            is_archetype: b.yes("is_archetype"),
            year: b.num("year").unwrap_or(0.0),
            active: b.yes("active"),
            stats: numbers(b),
            modular: b.get("module_slots").is_some(),
            default_modules: b.block("default_modules").map(|d| d.items.iter().filter_map(|i| Some((i.key.as_ref()?.text.clone(), i.text()?.to_string()))).collect()).unwrap_or_default(),
            slots,
            parent: b.atom("parent").map(String::from),
            types,
            resources: b.block("resources").map(numbers).unwrap_or_default(),
            count_limits: b.blocks("module_count_limit").filter_map(CountLimit::read).collect(),
            legacy: b.atom("derived_variant_name").map(String::from),
            allowed_types: words(b.block("allowed_types")),
            type_override: b.atom("type_override").map(String::from),
            upgrades: words(b.block("upgrades")),
            derived: false,
        }
    }
}

/// `duplicate_archetypes = { small_plane_cas_airframe = { archetype =
/// small_plane_airframe type = cas ... } }`: a family the game makes of
/// another, with each of its models (`small_plane_airframe_1` →
/// `small_plane_cas_airframe_1`).
#[derive(Clone, Debug, Default)]
pub struct Duplicate {
    pub archetype: String,
    pub types: Vec<String>,
    /// Its own stats (over the family's).
    pub stats: Stats,
    /// `for_each = { air_superiority = { set = 1.25 } }`: set on each model.
    pub set: Stats,
    /// Model → the model of before the designer it stands for.
    pub variant_names: BTreeMap<String, String>,
}

impl Duplicate {
    fn read(b: &Block) -> Option<Duplicate> {
        let types = match b.get("type") {
            Some(i) => i.block().map(|t| t.values().map(String::from).collect()).or_else(|| i.text().map(|t| vec![t.to_string()])).unwrap_or_default(),
            None => Vec::new(),
        };
        let set = b.block("for_each").map(|f| f.items.iter().filter_map(|i| Some((i.key.as_ref()?.text.clone(), i.block()?.num("set")?))).collect()).unwrap_or_default();
        let variant_names = b.block("variant_name").map(|v| v.items.iter().filter_map(|i| Some((i.key.as_ref()?.text.clone(), i.text()?.to_string()))).collect()).unwrap_or_default();
        Some(Duplicate { archetype: b.atom("archetype")?.to_string(), types, stats: numbers(b), set, variant_names })
    }
}

/// A legacy model's upgrade (`common/units/equipment/upgrades`): how far it
/// goes and what each level does to the stats (a fraction per level).
#[derive(Clone, Debug, Default)]
pub struct Upgrade {
    pub max_level: u32,
    pub stats: Stats,
}

/// A module of a design (a tank's gun, engine, armour, a ship's battery ...).
#[derive(Clone, Debug, Default)]
pub struct Module {
    pub add: Stats,
    pub multiply: Stats,
    /// `add_average_stats`: the design gets the average of its modules'.
    pub average: Stats,
    pub category: String,
    /// Manpower it adds (outside `add_stats`).
    pub manpower: f64,
    /// Resources it adds to the cost (`build_cost_resources`).
    pub resources: Stats,
    /// Categories of modules a design with it cannot have.
    pub forbid: Vec<String>,
    /// What it makes the design (`add_equipment_type = capital_ship`, `{
    /// fighter heavy_fighter }`).
    pub add_types: Vec<String>,
    /// Its picture: `GFX_SMI_<gfx>` (else its id).
    pub gfx: Option<String>,
    /// The DLCs of the `limit` of its `equipment_modules` block (the planes'
    /// modules are By Blood Alone's).
    pub need: Need,
    /// `mission_type_stats`: what it adds on some missions (a plane's bombs
    /// weigh on close air support).
    pub missions: Vec<(Vec<String>, Stats)>,
}

impl Module {
    fn read(b: &Block) -> Module {
        let st = |k: &str| b.block(k).map(numbers).unwrap_or_default();
        Module {
            add: st("add_stats"),
            multiply: st("multiply_stats"),
            average: st("add_average_stats"),
            category: b.atom("category").unwrap_or("").to_string(),
            manpower: b.num("manpower").unwrap_or(0.0),
            resources: st("build_cost_resources"),
            forbid: words(b.block("forbid_module_categories")),
            add_types: match b.get("add_equipment_type") {
                Some(i) => i.block().map(|t| t.values().map(String::from).collect()).or_else(|| i.text().map(|t| vec![t.to_string()])).unwrap_or_default(),
                None => Vec::new(),
            },
            gfx: b.atom("gfx").map(String::from),
            need: Need::default(),
            missions: b.blocks("mission_type_stats").map(|m| (words(m.block("limit")), m.block("add_stats").map(numbers).unwrap_or_default())).collect(),
        }
    }
    /// The sprite of its picture in the designer.
    pub fn icon(&self, id: &str) -> String {
        format!("GFX_SMI_{}", self.gfx.as_deref().unwrap_or(id))
    }
}

/// A technology's bonus to the battalions of a type or a category.
#[derive(Clone, Debug, Default)]
pub struct Bonus {
    pub target: String,
    pub stats: Stats,
    pub terrain: BTreeMap<String, Terrain>,
}

#[derive(Clone, Debug, Default)]
pub struct Tech {
    pub id: String,
    pub year: Option<f64>,
    /// `allow = { always = no }`, or no place on the research screen:
    /// only given by focuses and events.
    pub focus_only: bool,
    pub subunits: Vec<String>,
    pub equipment: Vec<String>,
    /// Modules of designs it unlocks (`enable_equipment_modules`).
    pub modules: Vec<String>,
    /// The technologies it leads to on the research screen (`path`).
    pub leads_to: Vec<String>,
    pub bonuses: Vec<Bonus>,
    /// The research folders it is in (`folder = { name = x }`).
    pub folders: Vec<String>,
    /// DLC checks of its `allow_branch`.
    pub allow_branch: Need,
    /// Its research categories (`categories = { infantry_weapons }`), what
    /// research bonuses go to.
    pub categories: Vec<String>,
}

/// Blocks of a technology that are not bonuses to units.
const NOT_BONUS: [&str; 22] = [
    "folder",
    "path",
    "categories",
    "enable_equipments",
    "enable_subunits",
    "enable_equipment_modules",
    "enable_building",
    "allow",
    "allow_branch",
    "ai_will_do",
    "ai_research_weights",
    "on_research_complete",
    "on_research_complete_limit",
    "sub_technologies",
    "special_project_specialization",
    "xor",
    "dependencies",
    "enable_tactic",
    "show_effect_as_desc",
    "modifier",
    "doctrine",
    "research_bonus",
];

impl Tech {
    fn read(id: &str, b: &Block) -> Tech {
        let bonuses = b
            .items
            .iter()
            .filter(|i| i.key.as_ref().is_some_and(|k| !NOT_BONUS.contains(&k.text.as_str())))
            .filter_map(|i| {
                let blk = i.block()?;
                let stats = numbers(blk);
                let terrain = terrains(blk);
                (!stats.is_empty() || !terrain.is_empty()).then(|| Bonus { target: i.key().to_string(), stats, terrain })
            })
            .collect();
        Tech {
            id: id.to_string(),
            year: b.num("start_year"),
            focus_only: b.block("allow").is_some_and(|a| a.atom("always") == Some("no")) || b.get("folder").is_none(),
            subunits: b.blocks("enable_subunits").flat_map(|e| e.values().map(String::from).collect::<Vec<_>>()).collect(),
            equipment: b.blocks("enable_equipments").flat_map(|e| e.values().map(String::from).collect::<Vec<_>>()).collect(),
            modules: b.blocks("enable_equipment_modules").flat_map(|e| e.values().map(String::from).collect::<Vec<_>>()).collect(),
            leads_to: b.blocks("path").filter_map(|p| p.atom("leads_to_tech").map(String::from)).collect(),
            bonuses,
            folders: b.blocks("folder").filter_map(|f| f.atom("name").map(String::from)).collect(),
            allow_branch: b.block("allow_branch").map(|a| dlc::of_trigger(a).0).unwrap_or_default(),
            categories: b.blocks("categories").flat_map(|e| e.values().map(String::from).collect::<Vec<_>>()).collect(),
        }
    }
}

/// What a nation has: its technologies and what they unlock, and the
/// equipment in its stockpile.
#[derive(Clone, Debug, Default)]
pub struct Have {
    pub techs: BTreeSet<String>,
    pub units: BTreeSet<String>,
    /// Models it can make (what its divisions get, with the stockpile's).
    pub equipment: BTreeSet<String>,
    /// Modules of designs it can use.
    pub modules: BTreeSet<String>,
    /// The stockpile at the start: model → how many.
    pub stock: BTreeMap<String, f64>,
}

/// The land units of the game and the mod.
#[derive(Clone, Debug, Default)]
pub struct UnitDb {
    pub subunits: BTreeMap<String, SubUnit>,
    pub equipment: BTreeMap<String, Equipment>,
    pub modules: BTreeMap<String, Module>,
    pub techs: BTreeMap<String, Tech>,
    /// Land terrains: (name, combat width, width added per further side
    /// attacked from).
    pub widths: Vec<(String, f64, f64)>,
    /// Research folders and the DLCs a player needs to see them (No Step
    /// Back's tanks, the tanks of those without it).
    pub folders: BTreeMap<String, Need>,
    /// Ships: the naval sub-units (`definition` of a ship in an order of
    /// battle).
    pub ships: BTreeMap<String, ShipUnit>,
    /// Module → the technologies that unlock it (worked out when read).
    pub module_techs: BTreeMap<String, Vec<String>>,
    /// The families the game makes of others (`duplicate_archetypes`).
    pub duplicates: BTreeMap<String, Duplicate>,
    /// The upgrades of the models without designer.
    pub upgrades: BTreeMap<String, Upgrade>,
}

/// A kind of ship (`destroyer`, `light_cruiser`, ...): what a ship of a
/// naval order of battle is (`definition`), and the hulls it is built on.
#[derive(Clone, Debug, Default)]
pub struct ShipUnit {
    pub id: String,
    /// The equipment archetypes it needs (`ship_hull_cruiser`).
    pub hulls: Vec<String>,
    /// screen_ship, capital_ship, submarine, carrier.
    pub types: Vec<String>,
    /// `need_equipment_modules = { any = { ... } }`: a design needs one of
    /// these modules to be this kind (a battleship's armour).
    pub modules_any: Vec<String>,
}

/// The `id = { ... }` entries of the `key = { ... }` blocks of a file.
fn entries<'a>(root: &'a Block, key: &str) -> Vec<(&'a str, &'a Block)> {
    let mut out = Vec::new();
    for i in root.items.iter().filter(|i| i.is(key)) {
        if let Some(b) = i.block() {
            out.extend(b.items.iter().filter_map(|e| Some((e.key.as_ref()?.text.as_str(), e.block()?))));
        }
    }
    out
}

/// Folders the units are read from.
pub const FOLDERS: [&str; 7] = ["common/units", "common/units/equipment", "common/units/equipment/modules", "common/units/equipment/upgrades", "common/technologies", "common/terrain", "common/technology_tags"];

impl UnitDb {
    /// What a file of the game or the mod defines (by its folder); a later
    /// definition of an id replaces an earlier one.
    pub fn read_file(&mut self, path: &str, s: &Script, content: bool) {
        let p = path.replace('\\', "/");
        let dir = p.rsplit_once('/').map(|x| x.0).unwrap_or("");
        match dir {
            "common/units/equipment/modules" => {
                for block in s.root.items.iter().filter(|i| i.is("equipment_modules")).filter_map(|i| i.block()) {
                    let need = block.block("limit").map(|l| dlc::of_trigger(l).0).unwrap_or_default();
                    for i in &block.items {
                        if let (Some(k), Some(b)) = (i.key.as_ref(), i.block())
                            && k.text != "limit"
                        {
                            let mut m = Module::read(b);
                            m.need = need.clone();
                            self.modules.insert(k.text.clone(), m);
                        }
                    }
                }
            }
            "common/units/equipment/upgrades" => {
                for (id, b) in entries(&s.root, "upgrades") {
                    let mut stats = numbers(b);
                    stats.remove("max_level");
                    self.upgrades.insert(id.to_string(), Upgrade { max_level: b.num("max_level").unwrap_or(0.0).max(0.0) as u32, stats });
                }
            }
            "common/units/equipment" => {
                for (id, b) in entries(&s.root, "equipments") {
                    self.equipment.insert(id.to_string(), Equipment::read(id, b));
                }
                for (id, b) in entries(&s.root, "duplicate_archetypes") {
                    if let Some(d) = Duplicate::read(b) {
                        self.duplicates.insert(id.to_string(), d);
                    }
                }
                self.expand_duplicates();
            }
            "common/units" => {
                for (id, b) in entries(&s.root, "sub_units") {
                    if SubUnit::is_land(b) {
                        self.subunits.insert(id.to_string(), SubUnit::read(id, b, content));
                    } else if b.atom("map_icon_category") == Some("ship") || b.atom("map_icon_category") == Some("uboat") {
                        let need = |k: &str| b.block(k).map(|n| n.items.iter().filter_map(|i| i.key.as_ref().map(|k| k.text.clone())).collect::<Vec<_>>()).unwrap_or_default();
                        let mut hulls = need("need");
                        hulls.extend(need("need_equipment"));
                        let types = match b.get("type") {
                            Some(i) => i.block().map(|t| t.values().map(String::from).collect()).or_else(|| i.text().map(|t| vec![t.to_string()])).unwrap_or_default(),
                            None => Vec::new(),
                        };
                        let modules_any = b.block("need_equipment_modules").and_then(|m| m.block("any")).map(|a| a.items.iter().filter_map(|i| i.key.as_ref().map(|k| k.text.clone())).collect()).unwrap_or_default();
                        self.ships.insert(id.to_string(), ShipUnit { id: id.to_string(), hulls, types, modules_any });
                    }
                }
            }
            "common/technologies" => {
                for (id, b) in entries(&s.root, "technologies") {
                    let t = Tech::read(id, b);
                    for m in &t.modules {
                        let v = self.module_techs.entry(m.clone()).or_default();
                        if !v.contains(&t.id) {
                            v.push(t.id.clone());
                        }
                    }
                    self.techs.insert(id.to_string(), t);
                }
            }
            "common/technology_tags" => {
                for i in s.root.items.iter().filter(|i| i.is("technology_folders")) {
                    for f in i.block().map(|b| b.items.as_slice()).unwrap_or_default() {
                        if let (Some(k), Some(fb)) = (f.key.as_ref(), f.block()) {
                            self.folders.insert(k.text.clone(), fb.block("available").map(|a| dlc::of_trigger(a).0).unwrap_or_default());
                        }
                    }
                }
            }
            "common/terrain" => {
                for (id, b) in entries(&s.root, "categories") {
                    if let Some(w) = b.num("combat_width").filter(|_| !b.yes("is_water")) {
                        self.widths.retain(|x| x.0 != id);
                        self.widths.push((id.to_string(), w, b.num("combat_support_width").unwrap_or(0.0)));
                    }
                }
            }
            _ => {}
        }
    }

    /// The families of `duplicate_archetypes` made: a copy of the family
    /// (with its own type and stats) and of each of its models, named after
    /// it (`small_plane_airframe_1` → `small_plane_cas_airframe_1`). Models
    /// a file defines itself are kept.
    pub fn expand_duplicates(&mut self) {
        for (id, d) in &self.duplicates {
            let Some(arch) = self.equipment.get(&d.archetype).cloned() else { continue };
            let mut made: Vec<Equipment> = Vec::new();
            let mut a = arch.clone();
            a.id = id.clone();
            a.is_archetype = true;
            a.archetype = None;
            a.derived = true;
            a.legacy = None;
            if !d.types.is_empty() {
                a.types = d.types.clone();
            }
            a.stats.extend(d.stats.clone());
            a.stats.extend(d.set.clone());
            made.push(a);
            let prefix = format!("{}_", d.archetype);
            for e in self.equipment.values().filter(|e| !e.derived && !e.is_archetype && e.archetype.as_deref() == Some(d.archetype.as_str())) {
                let Some(suffix) = e.id.strip_prefix(&prefix) else { continue };
                let mut c = e.clone();
                c.id = format!("{id}_{suffix}");
                c.archetype = Some(id.clone());
                c.parent = e.parent.as_deref().map(|p| p.strip_prefix(&prefix).map(|s| format!("{id}_{s}")).unwrap_or_else(|| p.to_string()));
                c.derived = true;
                c.legacy = d.variant_names.get(&c.id).cloned();
                for (k, v) in &d.set {
                    if let Some(x) = c.stats.get_mut(k) {
                        *x = *v;
                    }
                }
                made.push(c);
            }
            for e in made {
                if self.equipment.get(&e.id).is_none_or(|x| x.derived) {
                    self.equipment.insert(e.id.clone(), e);
                }
            }
        }
    }

    /// The base game's units.
    pub fn of_index(ix: &Index) -> UnitDb {
        let mut db = UnitDb::default();
        for dir in FOLDERS {
            for path in ix.files.list(dir, "txt", false) {
                if let Some(t) = ix.files.text(&path) {
                    db.read_file(&path, &Script::parse(&t), false);
                }
            }
        }
        db
    }

    /// The content's unit files laid over these.
    fn with_content(&self, st: &Store) -> UnitDb {
        let mut db = self.clone();
        for p in st.packages.values() {
            for (path, f) in &p.files {
                if let Some(s) = &f.script {
                    db.read_file(path, s, true);
                }
            }
        }
        db
    }

    /// What these technologies (with what every nation has) unlock.
    pub fn have(&self, techs: BTreeSet<String>) -> Have {
        let mut units: BTreeSet<String> = self.subunits.values().filter(|u| u.active).map(|u| u.id.clone()).collect();
        let mut equipment: BTreeSet<String> = self.equipment.values().filter(|e| e.active).map(|e| e.id.clone()).collect();
        // modules no technology unlocks: everyone's
        let mut modules: BTreeSet<String> = self.modules.keys().filter(|m| !self.module_techs.contains_key(*m)).cloned().collect();
        for t in techs.iter().filter_map(|t| self.techs.get(t)) {
            units.extend(t.subunits.iter().cloned());
            equipment.extend(t.equipment.iter().cloned());
            modules.extend(t.modules.iter().cloned());
        }
        Have { techs, units, equipment, modules, stock: BTreeMap::new() }
    }

    /// What a nation has at the start: what its history gives, for a
    /// player with `owned` (None: every line of the history, whatever DLC
    /// it checks).
    pub fn at_start(&self, st: &Store, tag: &str, owned: Option<&Owned>) -> Have {
        let mut h = self.have(start_techs(st, tag, owned));
        h.stock = start_stock(st, tag, owned);
        h
    }

    /// The DLCs a player needs (or must not have) to research a technology:
    /// those of its folder (when all its folders agree) and `allow_branch`.
    pub fn tech_need(&self, id: &str) -> Need {
        let Some(t) = self.techs.get(id) else { return Need::default() };
        let mut it = t.folders.iter().map(|f| self.folders.get(f).cloned().unwrap_or_default());
        let folder = match it.next() {
            Some(first) => it.fold(first, |a, b| a.common(&b)),
            None => Need::default(),
        };
        // what every player has is no need
        folder.and(&t.allow_branch).for_players()
    }

    /// The DLCs a player needs (or must not have) for a technology, a
    /// battalion or a model: a battalion or model, what every technology
    /// that unlocks it needs (nothing when every nation has it).
    pub fn need_of(&self, id: &str) -> Need {
        if self.techs.contains_key(id) {
            return self.tech_need(id);
        }
        if self.subunits.get(id).is_some_and(|u| u.active) || self.equipment.get(id).is_some_and(|e| e.active) {
            return Need::default();
        }
        let mut it = self.unlocked_by(id).into_iter().map(|t| self.tech_need(&t.id));
        match it.next() {
            Some(first) => it.fold(first, |a, b| a.common(&b)),
            None => Need::default(),
        }
    }

    /// The DLCs that change what can be researched: those research folders
    /// and technologies check.
    pub fn dlcs(&self) -> BTreeSet<String> {
        self.folders.values().chain(self.techs.values().map(|t| &t.allow_branch)).flat_map(|n| n.names().cloned()).filter(|d| !dlc::integrated(d)).collect()
    }

    /// "Basic Light Tank (only without No Step Back) or Light Tank I (only
    /// with No Step Back)": the technologies that unlock something.
    pub fn unlocked_text(&self, ids: &[&str], name: &dyn Fn(&str) -> String, limit: usize) -> String {
        let mut techs: Vec<&Tech> = ids.iter().flat_map(|i| self.unlocked_by(i)).collect();
        techs.sort_by_key(|t| (t.focus_only, t.year.map(|y| y as i64), t.id.clone()));
        techs.dedup_by_key(|t| t.id.clone());
        let ts: Vec<&Tech> = if techs.iter().any(|t| !t.focus_only) { techs.into_iter().filter(|t| !t.focus_only).take(limit).collect() } else { techs.into_iter().take(limit).collect() };
        let v: Vec<String> = ts
            .iter()
            .map(|t| {
                let n = self.tech_need(&t.id);
                if n.is_empty() { name(&t.id) } else { format!("{} (only {})", name(&t.id), n.describe()) }
            })
            .collect();
        v.join(" or ")
    }

    /// The technologies that unlock a battalion or a model of equipment.
    pub fn unlocked_by(&self, id: &str) -> Vec<&Tech> {
        self.techs.values().filter(|t| t.subunits.iter().any(|s| s == id) || t.equipment.iter().any(|s| s == id) || t.modules.iter().any(|s| s == id)).collect()
    }

    /// The technologies up to a year (those of focuses and events left
    /// out, and those a player with `owned` does not see), with `start`.
    pub fn up_to(&self, start: &BTreeSet<String>, year: u32, owned: Option<&Owned>) -> BTreeSet<String> {
        let mut s = start.clone();
        s.extend(self.techs.values().filter(|t| !t.focus_only && t.year.is_none_or(|y| y <= year as f64) && owned.is_none_or(|o| self.tech_need(&t.id).holds(o))).map(|t| t.id.clone()));
        s
    }

    /// Years technologies are meant for, to pick from.
    pub fn years(&self) -> Vec<u32> {
        let mut v: Vec<u32> = self.techs.values().filter_map(|t| t.year).filter(|y| (1930.0..1960.0).contains(y)).map(|y| y as u32).collect();
        v.sort();
        v.dedup();
        v
    }

    /// The model a battalion gets for what it needs: the newest the nation
    /// has of that archetype (or that very model), if any.
    pub fn best(&self, need: &str, have: &Have) -> Option<&Equipment> {
        let e = self.equipment.get(need)?;
        let has = |id: &str| have.equipment.contains(id) || have.stock.get(id).is_some_and(|n| *n > 0.0);
        if !e.is_archetype {
            return has(need).then_some(e);
        }
        // the newest; of two of a year, the one with stats of its own (the
        // game's tanks without a design, as without No Step Back)
        self.equipment
            .values()
            .filter(|v| !v.is_archetype && v.archetype.as_deref() == Some(need) && has(&v.id))
            .max_by(|a, b| a.year.total_cmp(&b.year).then((!a.modular).cmp(&!b.modular)))
    }

    /// Where a model's archetype is the family (else the model itself).
    pub fn archetype_of<'a>(&'a self, id: &'a str) -> &'a str {
        self.equipment.get(id).and_then(|e| e.archetype.as_deref()).unwrap_or(id)
    }

    /// A model's stats: its archetype's, then its own; a design of modules
    /// (a tank chassis) adds its default modules'.
    pub fn stats_of(&self, e: &Equipment) -> Stats {
        let arch = e.archetype.as_deref().and_then(|a| self.equipment.get(a));
        let mut s = arch.map(|a| a.stats.clone()).unwrap_or_default();
        s.extend(e.stats.clone());
        if e.modular {
            let mut mods = arch.map(|a| a.default_modules.clone()).unwrap_or_default();
            mods.extend(e.default_modules.clone());
            let mut mult = Stats::new();
            for m in mods.values().filter_map(|m| self.modules.get(m)) {
                for (k, v) in &m.add {
                    *s.entry(k.clone()).or_default() += v;
                }
                for (k, v) in &m.multiply {
                    *mult.entry(k.clone()).or_default() += v;
                }
            }
            for (k, v) in mult {
                if let Some(x) = s.get_mut(&k) {
                    *x *= 1.0 + v;
                }
            }
        }
        s
    }
}

/// Stats a battalion takes from its equipment; its own values of these and
/// the technologies' are bonuses (0.1 is +10%).
pub const FROM_EQUIPMENT: [&str; 8] = ["soft_attack", "hard_attack", "air_attack", "defense", "breakthrough", "ap_attack", "armor_value", "hardness"];
/// A battalion's own stats; technologies add to them.
const OWN: [&str; 13] = [
    "max_strength",
    "max_organisation",
    "default_morale",
    "recon",
    "entrenchment",
    "initiative",
    "casualty_trickleback",
    "experience_loss_factor",
    "equipment_capture_factor",
    "combat_width",
    "manpower",
    "training_time",
    "weight",
];

/// A battalion or support company as it comes out for a nation.
#[derive(Clone, Debug, Default)]
pub struct Battalion {
    pub id: String,
    pub stats: Stats,
    pub terrain: BTreeMap<String, Terrain>,
    /// What it needs: (equipment, the model it gets, how many).
    pub equipment: Vec<(String, Option<String>, f64)>,
}

impl Battalion {
    pub fn get(&self, k: &str) -> f64 {
        self.stats.get(k).copied().unwrap_or(0.0)
    }
}

impl UnitDb {
    /// A battalion with the nation's equipment and technologies.
    pub fn battalion(&self, u: &SubUnit, have: &Have) -> Battalion {
        let mut tb = Stats::new();
        let mut terrain = u.terrain.clone();
        for t in have.techs.iter().filter_map(|t| self.techs.get(t)) {
            for b in t.bonuses.iter().filter(|b| b.target == u.id || u.categories.contains(&b.target)) {
                for (k, v) in &b.stats {
                    *tb.entry(k.clone()).or_default() += v;
                }
                for (k, v) in &b.terrain {
                    terrain.entry(k.clone()).or_default().add(v);
                }
            }
        }
        let bonus = |k: &str| tb.get(k).copied().unwrap_or(0.0);
        let mut eq = Stats::new();
        let (mut slowest, mut transport): (Option<f64>, Option<f64>) = (None, None);
        let mut equipment = Vec::new();
        for (need, n) in &u.need {
            let model = self.best(need, have);
            if let Some(e) = model {
                let s = self.stats_of(e);
                for k in FROM_EQUIPMENT {
                    *eq.entry(k.to_string()).or_default() += s.get(k).copied().unwrap_or(0.0);
                }
                let sp = s.get("maximum_speed").copied().unwrap_or(0.0);
                if u.transport.as_deref().is_some_and(|t| t == need || self.archetype_of(t) == need) {
                    transport = Some(sp);
                } else if sp > 0.0 {
                    slowest = Some(slowest.map_or(sp, |x: f64| x.min(sp)));
                }
            }
            equipment.push((need.clone(), model.map(|e| e.id.clone()), *n));
        }
        let mut s = Stats::new();
        for k in FROM_EQUIPMENT {
            s.insert(k.to_string(), eq.get(k).copied().unwrap_or(0.0) * (1.0 + u.stat(k) + bonus(k)).max(0.0));
        }
        let speed = transport.or(slowest).unwrap_or(0.0);
        s.insert("maximum_speed".into(), speed * (1.0 + u.stat("maximum_speed") + bonus("maximum_speed")).max(0.0));
        for k in OWN {
            s.insert(k.to_string(), u.stat(k) + bonus(k));
        }
        *s.entry("default_morale".into()).or_default() += bonus("recovery");
        s.insert("suppression".into(), u.stat("suppression") * (1.0 + u.stat("suppression_factor") + bonus("suppression_factor")));
        s.insert("supply_consumption".into(), u.stat("supply_consumption") * (1.0 + u.stat("supply_consumption_factor") + bonus("supply_consumption_factor") + bonus("supply_consumption")).max(0.0));
        Battalion { id: u.id.clone(), stats: s, terrain, equipment }
    }
}

// ------------------------------------------------------------- templates

/// The parts of a division template.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Section {
    Regiments,
    Support,
    RegimentalSupport,
}

impl Section {
    pub const ALL: [Section; 3] = [Section::Regiments, Section::Support, Section::RegimentalSupport];
    /// The key of its block in the template.
    pub fn key(self) -> &'static str {
        match self {
            Section::Regiments => "regiments",
            Section::Support => "support",
            Section::RegimentalSupport => "regimental_support",
        }
    }
    pub fn one(self) -> &'static str {
        match self {
            Section::Regiments => "battalion",
            Section::Support => "support company",
            Section::RegimentalSupport => "regimental support company",
        }
    }
}

/// A battalion or company of a template, where it is.
#[derive(Clone, Debug, PartialEq)]
pub struct Slot {
    pub section: Section,
    pub unit: String,
    /// Column and row (-1: not given).
    pub x: i64,
    pub y: i64,
    /// The bytes of its `unit = { x = .. y = .. }` in the file.
    pub span: Range<usize>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Template {
    pub slots: Vec<Slot>,
}

impl Template {
    /// A `division_template = { ... }` block.
    pub fn read(b: &Block) -> Template {
        let mut slots = Vec::new();
        for section in Section::ALL {
            for blk in b.blocks(section.key()) {
                for it in &blk.items {
                    let Some(k) = it.key.as_ref() else { continue };
                    let pos = |c: &str| it.block().and_then(|p| p.atom(c)).and_then(|v| v.parse::<i64>().ok()).unwrap_or(-1);
                    slots.push(Slot { section, unit: k.text.clone(), x: pos("x"), y: pos("y"), span: it.span.clone() });
                }
            }
        }
        Template { slots }
    }
    pub fn at(&self, section: Section, x: i64, y: i64) -> Option<&Slot> {
        self.slots.iter().find(|s| s.section == section && s.x == x && s.y == y)
    }
    pub fn of(&self, section: Section) -> impl Iterator<Item = &Slot> {
        self.slots.iter().filter(move |s| s.section == section)
    }
    /// The group of a column: that of its first battalion the game knows.
    pub fn column_group<'a>(&self, db: &'a UnitDb, x: i64) -> Option<&'a str> {
        let mut col: Vec<&Slot> = self.of(Section::Regiments).filter(|s| s.x == x).collect();
        col.sort_by_key(|s| s.y);
        col.iter().find_map(|s| db.subunits.get(&s.unit)).map(|u| u.group.as_str())
    }
}

/// A problem of a template; `slot` indexes [`Template::slots`].
#[derive(Clone, Debug, PartialEq)]
pub struct Problem {
    pub severity: Severity,
    pub message: String,
    pub slot: Option<usize>,
}

/// The technologies a nation starts with: those the build gives every
/// nation, and the `set_technology` of its history in the content.
pub fn start_techs(st: &Store, tag: &str, owned: Option<&Owned>) -> BTreeSet<String> {
    let mut out: BTreeSet<String> = crate::modbuild::START_TECHS.iter().map(|s| s.to_string()).collect();
    history_lines(st, tag, &mut |i, need| {
        if i.is("set_technology")
            && owned.is_none_or(|o| need.holds(o))
            && let Some(b) = i.block()
        {
            for it in &b.items {
                if let (Some(k), Some(v)) = (it.key.as_ref(), it.text())
                    && v != "0"
                    && k.text != "popup"
                {
                    out.insert(k.text.clone());
                }
            }
        }
    });
    out
}

/// The technologies each country of the game starts with (tag → ids): the
/// `set_technology` of its history, in and out of its `has_dlc` checks,
/// without the dated blocks of the later starts (`1939.1.1 = { ... }`).
/// Read once per game.
pub fn game_start_techs(base: &Index) -> Arc<BTreeMap<String, BTreeSet<String>>> {
    type Cache = Option<((usize, usize), Arc<BTreeMap<String, BTreeSet<String>>>)>;
    static CACHE: Mutex<Cache> = Mutex::new(None);
    let key = (base as *const Index as usize, base.paths.len());
    let mut c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((k, m)) = c.as_ref()
        && *k == key
    {
        return m.clone();
    }
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for path in base.files.list("history/countries", "txt", false) {
        let tag: String = path.rsplit('/').next().unwrap_or("").chars().take(3).collect();
        if tag.len() != 3 || !tag.chars().all(|c| c.is_ascii_alphanumeric()) {
            continue;
        }
        if let Some(text) = base.files.text(&path) {
            set_technology_in(&Script::parse(&text).root, out.entry(tag.to_ascii_uppercase()).or_default());
        }
    }
    let m = Arc::new(out);
    *c = Some((key, m.clone()));
    m
}

/// The technologies of the `set_technology` lines of a history, those in
/// `if` / `else_if` / `else` too.
fn set_technology_in(b: &Block, out: &mut BTreeSet<String>) {
    for i in &b.items {
        let Some(body) = i.block() else { continue };
        if i.is("set_technology") {
            for it in &body.items {
                if let (Some(k), Some(v)) = (it.key.as_ref(), it.text())
                    && v != "0"
                    && k.text != "popup"
                {
                    out.insert(k.text.clone());
                }
            }
        } else if i.is("if") || i.is("else_if") || i.is("else") {
            set_technology_in(body, out);
        }
    }
}

/// The equipment a nation's history puts in its stockpile
/// (`add_equipment_to_stockpile`), for a player with `owned` (None: all of
/// it): model → how many.
pub fn start_stock(st: &Store, tag: &str, owned: Option<&Owned>) -> BTreeMap<String, f64> {
    let mut out: BTreeMap<String, f64> = BTreeMap::new();
    history_lines(st, tag, &mut |i, need| {
        if i.is("add_equipment_to_stockpile")
            && owned.is_none_or(|o| need.holds(o))
            && let Some(b) = i.block()
            && let (Some(t), Some(n)) = (b.atom("type"), b.num("amount"))
        {
            *out.entry(t.to_string()).or_default() += n;
        }
    });
    out.retain(|_, n| *n > 0.0);
    out
}

/// The DLCs the `has_dlc` checks of a nation's history name.
pub fn history_dlcs(st: &Store, tag: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    history_lines(st, tag, &mut |_, need| out.extend(need.names().filter(|d| !dlc::integrated(d)).cloned()));
    out
}

/// Each line of a nation's history (`history/countries`) that is about
/// the nation itself (not another country's scope in it), with the DLCs
/// it runs for.
pub fn history_lines(st: &Store, tag: &str, f: &mut dyn FnMut(&super::script::Item, &Need)) {
    let Some(p) = st.package(tag) else { return };
    for (path, file) in &p.files {
        if path.starts_with("history/countries/")
            && let Some(s) = &file.script
        {
            dlc::walk_flow(&s.root, &Need::default(), f);
        }
    }
}

/// A group as said in the designer.
pub fn group_label(g: &str) -> String {
    g.replace('_', " ")
}

impl UnitDb {
    /// The checks of a template of a nation that has `have` at the start.
    /// `name` gives the names of battalions, equipment and technologies.
    pub fn check(&self, t: &Template, have: &Have, tag: &str, name: &dyn Fn(&str) -> String) -> Vec<Problem> {
        let mut out = Vec::new();
        let mut add = |severity: Severity, message: String, slot: Option<usize>| out.push(Problem { severity, message, slot });
        if t.of(Section::Regiments).next().is_none() {
            add(Severity::Error, "no battalions: a template needs at least one".into(), None);
        }
        let mut seen: BTreeMap<(Section, i64, i64), usize> = BTreeMap::new();
        let mut support_types: BTreeMap<String, usize> = BTreeMap::new();
        let mut regimental_cols: BTreeMap<i64, usize> = BTreeMap::new();
        for (i, s) in t.slots.iter().enumerate() {
            let Some(u) = self.subunits.get(&s.unit) else {
                add(Severity::Error, format!("{} is not a battalion of the game or the mod", s.unit), Some(i));
                continue;
            };
            let n = name(&s.unit);
            // where it is
            let (cols, rows) = match s.section {
                Section::Regiments => (0..=4, 0..=4),
                Section::Support => (0..=0, 0..=4),
                Section::RegimentalSupport => (0..=4, 0..=4),
            };
            if !cols.contains(&s.x) || !rows.contains(&s.y) {
                let where_ = match s.section {
                    Section::Regiments => "battalions go in columns x = 0 to 4, rows y = 0 to 4",
                    Section::Support => "support companies are one column: x = 0, y = 0 to 4",
                    Section::RegimentalSupport => "regimental support goes on a column: x = 0 to 4, y = 0",
                };
                add(Severity::Error, format!("{n} at x = {} y = {}: {where_}", s.x, s.y), Some(i));
            } else if let Some(&j) = seen.get(&(s.section, s.x, s.y)) {
                add(Severity::Error, format!("{n} and {} are both at x = {} y = {} of {}: the game keeps one", name(&t.slots[j].unit), s.x, s.y, s.section.key()), Some(i));
            } else {
                seen.insert((s.section, s.x, s.y), i);
            }
            // what goes there
            match (u.section(), s.section) {
                (None, _) => add(Severity::Error, format!("{n} is for army headquarters, not divisions"), Some(i)),
                (Some(a), b) if a != b => add(Severity::Error, format!("{n} is a {} and goes in {}, not {}", a.one(), a.key(), b.key()), Some(i)),
                _ => {}
            }
            // the nation has it
            if !have.units.contains(&s.unit) {
                let techs = self.unlocked_text(&[s.unit.as_str()], name, 3);
                let how = if techs.is_empty() { "no technology unlocks it".to_string() } else { format!("it comes with {techs}") };
                add(Severity::Error, format!("{n} is not available to {tag} at the start: {how} (give it in the nation's history: set_technology = {{ ... }})"), Some(i));
            }
            match s.section {
                Section::Support => {
                    let ty = u.same_support_type.clone().unwrap_or_else(|| u.id.clone());
                    if let Some(&j) = support_types.get(&ty) {
                        let other = &t.slots[j].unit;
                        let msg = if *other == s.unit { format!("{n} twice: a division takes one of each support company") } else { format!("{n} and {}: a division takes one {} company", name(other), group_label(&ty)) };
                        add(Severity::Error, msg, Some(i));
                    } else {
                        support_types.insert(ty, i);
                    }
                }
                Section::RegimentalSupport => {
                    if let Some(&j) = regimental_cols.get(&s.x) {
                        add(Severity::Error, format!("{n} and {} on column {} (x = {}): a column takes one regimental support company", name(&t.slots[j].unit), s.x + 1, s.x), Some(i));
                    } else {
                        regimental_cols.insert(s.x, i);
                    }
                    match t.column_group(self, s.x) {
                        None => add(Severity::Error, format!("{n} is on column {} (x = {}), which has no battalions", s.x + 1, s.x), Some(i)),
                        Some(g) if !u.allowed_groups.is_empty() && !u.allowed_groups.iter().any(|a| a == g) => {
                            let ok: Vec<String> = u.allowed_groups.iter().map(|g| group_label(g)).collect();
                            add(Severity::Error, format!("{n} cannot go on a column of {} (only {})", group_label(g), ok.join(", ")), Some(i));
                        }
                        _ => {}
                    }
                }
                Section::Regiments => {}
            }
        }
        // one group per column
        for x in 0..5 {
            let mut groups: Vec<&str> = Vec::new();
            let mut first = None;
            for (i, s) in t.slots.iter().enumerate().filter(|(_, s)| s.section == Section::Regiments && s.x == x) {
                if let Some(u) = self.subunits.get(&s.unit).filter(|u| u.section() == Some(Section::Regiments))
                    && !groups.contains(&u.group.as_str())
                {
                    groups.push(&u.group);
                    if groups.len() == 2 {
                        first = Some(i);
                    }
                }
            }
            if groups.len() > 1 {
                let g: Vec<String> = groups.iter().map(|g| group_label(g)).collect();
                add(Severity::Warning, format!("column {} (x = {x}) mixes {}: the game's designer keeps one group per column", x + 1, g.join(" and ")), first);
            }
        }
        // equipment the nation has none of
        let mut needs: BTreeMap<&str, usize> = BTreeMap::new();
        for (i, s) in t.slots.iter().enumerate() {
            if let Some(u) = self.subunits.get(&s.unit) {
                for (e, _) in &u.need {
                    needs.entry(e.as_str()).or_insert(i);
                }
            }
        }
        for (e, i) in needs {
            if self.equipment.contains_key(e) && self.best(e, have).is_none() {
                let models: Vec<&str> = self.equipment.values().filter(|m| m.archetype.as_deref() == Some(e) || m.id == e).map(|m| m.id.as_str()).collect();
                let techs = self.unlocked_text(&models, name, 2);
                let how = if techs.is_empty() { String::new() } else { format!(" (it comes with {techs}, or put some in its stockpile)") };
                add(Severity::Warning, format!("{tag} has no {} at the start: its divisions of this template start without it{how}", name(e)), Some(i));
            } else if !self.equipment.contains_key(e) {
                add(Severity::Error, format!("{e} is not equipment of the game or the mod"), Some(i));
            }
        }
        out
    }
}

impl UnitDb {
    /// The checks of a template of a nation for players with and without
    /// each DLC its history checks with `has_dlc` (each mix of up to three;
    /// with more, all of them and none). A problem only some players have
    /// says whom: "players without No Step Back: ...". `who` is the nation
    /// as said in the messages.
    pub fn check_for_players(&self, st: &Store, t: &Template, tag: &str, who: &str, name: &dyn Fn(&str) -> String) -> Vec<Problem> {
        let rel: Vec<String> = history_dlcs(st, tag).into_iter().collect();
        if rel.is_empty() {
            return self.check(t, &self.at_start(st, tag, None), who, name);
        }
        let cases: Vec<Owned> = if rel.len() <= 3 {
            (0..1usize << rel.len()).map(|m| rel.iter().enumerate().filter(|(i, _)| m >> i & 1 == 1).map(|(_, d)| d.clone()).collect()).collect()
        } else {
            vec![rel.iter().cloned().collect(), Owned::new()]
        };
        let mut found: Vec<(Problem, Vec<usize>)> = Vec::new();
        for (ci, o) in cases.iter().enumerate() {
            for p in self.check(t, &self.at_start(st, tag, Some(o)), who, name) {
                match found.iter_mut().find(|(q, _)| q.message == p.message && q.slot == p.slot) {
                    Some((_, v)) => v.push(ci),
                    None => found.push((p, vec![ci])),
                }
            }
        }
        found
            .into_iter()
            .map(|(mut p, v)| {
                if v.len() < cases.len() {
                    p.message = format!("players {}: {}", for_whom(&rel, &cases, &v), p.message);
                }
                p
            })
            .collect()
    }
}

/// Who the cases `v` of `cases` are: "with No Step Back", "without Man the
/// Guns", else each mix.
fn for_whom(rel: &[String], cases: &[Owned], v: &[usize]) -> String {
    let set: BTreeSet<usize> = v.iter().copied().collect();
    for d in rel {
        if set == (0..cases.len()).filter(|i| cases[*i].contains(d)).collect() {
            return format!("with {d}");
        }
        if set == (0..cases.len()).filter(|i| !cases[*i].contains(d)).collect() {
            return format!("without {d}");
        }
    }
    let mix = |o: &Owned| Need { with: rel.iter().filter(|d| o.contains(*d)).cloned().collect(), without: rel.iter().filter(|d| !o.contains(*d)).cloned().collect() }.describe();
    v.iter().map(|i| mix(&cases[*i])).collect::<Vec<_>>().join("; ")
}

// ------------------------------------------------------------- division

/// A division's stats (keys as the game's: `soft_attack`, ...).
#[derive(Clone, Debug, Default)]
pub struct Division {
    pub stats: Stats,
    pub terrain: BTreeMap<String, Terrain>,
    /// Equipment: archetype → (the model it gets, how many).
    pub equipment: BTreeMap<String, (Option<String>, f64)>,
    pub battalions: usize,
    pub companies: usize,
}

impl Division {
    pub fn get(&self, k: &str) -> f64 {
        self.stats.get(k).copied().unwrap_or(0.0)
    }
}

/// Stats summed over all battalions and companies.
const SUMMED: [&str; 18] = [
    "max_strength",
    "soft_attack",
    "hard_attack",
    "air_attack",
    "defense",
    "breakthrough",
    "recon",
    "suppression",
    "supply_consumption",
    "weight",
    "entrenchment",
    "initiative",
    "casualty_trickleback",
    "experience_loss_factor",
    "equipment_capture_factor",
    "manpower",
    "combat_width",
    "build_cost_ic",
];

impl UnitDb {
    /// The stats of a division of this template (what the game does not
    /// know is left out).
    pub fn division(&self, t: &Template, have: &Have) -> Division {
        let mut line: Vec<Battalion> = Vec::new();
        let mut support: Vec<(Battalion, &SubUnit)> = Vec::new();
        for s in &t.slots {
            let Some(u) = self.subunits.get(&s.unit) else { continue };
            let mut b = self.battalion(u, have);
            let ic: f64 = b.equipment.iter().map(|(_, m, n)| m.as_ref().and_then(|m| self.equipment.get(m)).map(|e| self.stats_of(e).get("build_cost_ic").copied().unwrap_or(0.0)).unwrap_or(0.0) * n).sum();
            b.stats.insert("build_cost_ic".into(), ic);
            if s.section == Section::Regiments {
                line.push(b);
            } else {
                support.push((b, u));
            }
        }
        // support companies' bonuses to the line battalions of a category
        for (_, su) in &support {
            for (cat, mult) in &su.battalion_mult {
                for b in line.iter_mut().filter(|b| self.subunits.get(&b.id).is_some_and(|u| u.categories.contains(cat))) {
                    for (k, v) in mult {
                        let key = if k == "suppression_factor" { "suppression" } else { k.as_str() };
                        if let Some(x) = b.stats.get_mut(key) {
                            *x *= 1.0 + v;
                        }
                    }
                }
            }
        }
        let all: Vec<&Battalion> = line.iter().chain(support.iter().map(|(b, _)| b)).collect();
        let mut d = Division { battalions: line.len(), companies: support.len(), ..Default::default() };
        if all.is_empty() {
            return d;
        }
        let n = all.len() as f64;
        for k in SUMMED {
            d.stats.insert(k.into(), all.iter().map(|b| b.get(k)).sum());
        }
        d.stats.insert("max_organisation".into(), all.iter().map(|b| b.get("max_organisation")).sum::<f64>() / n);
        d.stats.insert("default_morale".into(), all.iter().map(|b| b.get("default_morale")).sum::<f64>() / n);
        if !line.is_empty() {
            d.stats.insert("hardness".into(), line.iter().map(|b| b.get("hardness")).sum::<f64>() / line.len() as f64);
        }
        for k in ["armor_value", "ap_attack"] {
            let max = all.iter().map(|b| b.get(k)).fold(0.0, f64::max);
            let avg = all.iter().map(|b| b.get(k)).sum::<f64>() / n;
            d.stats.insert(k.into(), 0.4 * max + 0.6 * avg);
        }
        let speeds = line.iter().map(|b| b.get("maximum_speed")).chain(support.iter().filter(|(_, u)| u.affects_speed).map(|(b, _)| b.get("maximum_speed"))).filter(|v| *v > 0.0);
        d.stats.insert("maximum_speed".into(), speeds.fold(f64::INFINITY, f64::min).min(1e9));
        if d.get("maximum_speed") >= 1e9 {
            d.stats.insert("maximum_speed".into(), 0.0);
        }
        d.stats.insert("training_time".into(), all.iter().map(|b| b.get("training_time")).fold(0.0, f64::max));
        // terrain: averaged over line battalions, support companies' added
        let mut terr: BTreeMap<String, Terrain> = BTreeMap::new();
        for b in &line {
            for (k, v) in &b.terrain {
                terr.entry(k.clone()).or_default().add(&v.scale(1.0 / line.len().max(1) as f64));
            }
        }
        for (b, _) in &support {
            for (k, v) in &b.terrain {
                terr.entry(k.clone()).or_default().add(v);
            }
        }
        terr.retain(|_, v| !v.is_zero());
        d.terrain = terr;
        for b in &all {
            for (e, m, k) in &b.equipment {
                let arch = self.archetype_of(e).to_string();
                let entry = d.equipment.entry(arch).or_insert((m.clone(), 0.0));
                entry.1 += k;
                if entry.0.is_none() {
                    entry.0 = m.clone();
                }
            }
        }
        d
    }
}

// ------------------------------------------------------------- the cache

/// The units of the base game and the content (built once per change of
/// either).
pub fn db(base: Option<&Index>, st: &Store) -> Arc<UnitDb> {
    type BaseCache = Option<((usize, usize, usize), Arc<UnitDb>)>;
    type FullCache = Option<((usize, usize, usize), std::path::PathBuf, u64, Arc<UnitDb>)>;
    static BASE: Mutex<BaseCache> = Mutex::new(None);
    static FULL: Mutex<FullCache> = Mutex::new(None);
    let bkey = base.map(|b| (b as *const Index as usize, b.defs.len(), b.paths.len())).unwrap_or((0, 0, 0));
    let base_db = {
        let mut c = BASE.lock().unwrap_or_else(|e| e.into_inner());
        match c.as_ref() {
            Some((k, d)) if *k == bkey => d.clone(),
            _ => {
                let d = Arc::new(base.map(UnitDb::of_index).unwrap_or_default());
                *c = Some((bkey, d.clone()));
                d
            }
        }
    };
    let has_units = st.packages.values().any(|p| p.files.keys().any(|f| FOLDERS.iter().any(|d| f.rsplit_once('/').is_some_and(|x| x.0 == *d))));
    if !has_units {
        return base_db;
    }
    let mut c = FULL.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((k, root, generation, d)) = c.as_ref()
        && *k == bkey
        && *root == st.root
        && *generation == st.generation
    {
        return d.clone();
    }
    let d = Arc::new(base_db.with_content(st));
    *c = Some((bkey, st.root.clone(), st.generation, d.clone()));
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_history_starts_with_its_technologies_not_those_of_later_starts() {
        let s = Script::parse("set_technology = { infantry_weapons = 1 popup = no }\nif = { limit = { has_dlc = \"By Blood Alone\" } set_technology = { iw_small_airframe = 1 } }\nelse = { set_technology = { early_fighter = 1 } }\nset_technology = { gone = 0 }\n1939.1.1 = { set_technology = { infantry_weapons1 = 1 } }\n");
        let mut out = BTreeSet::new();
        set_technology_in(&s.root, &mut out);
        assert_eq!(out, ["early_fighter", "infantry_weapons", "iw_small_airframe"].map(String::from).into());
    }

    const UNITS: &str = "sub_units = {
	infantry = {
		abbreviation = \"INF\"
		sprite = infantry
		active = no
		group = infantry
		categories = { category_front_line category_light_infantry category_army }
		combat_width = 2
		max_strength = 25
		max_organisation = 60
		default_morale = 0.3
		manpower = 1000
		training_time = 90
		suppression = 1.5
		weight = 0.5
		supply_consumption = 0.06
		need = { infantry_equipment = 100 }
		forest = { attack = 0.1 movement = 0.1 }
	}
	motorized = {
		sprite = motorized
		active = no
		group = mobile
		combat_width = 2
		max_strength = 25
		max_organisation = 60
		default_morale = 0.3
		manpower = 1200
		training_time = 90
		transport = motorized_equipment
		need = { infantry_equipment = 100 motorized_equipment = 35 }
	}
	artillery_brigade = {
		sprite = artillery
		active = yes
		group = combat_support
		combat_width = 3
		max_strength = 0.6
		max_organisation = 0
		default_morale = 0.3
		manpower = 500
		training_time = 120
		need = { artillery_equipment = 12 }
	}
	artillery = {
		sprite = artillery
		active = yes
		affects_speed = no
		group = support
		regimental = no
		combat_width = 0
		max_organisation = 0
		manpower = 300
		training_time = 120
		soft_attack = -0.5
		need = { artillery_equipment = 12 }
	}
	engineer = {
		sprite = engineer
		active = no
		group = support
		regimental = no
		same_support_type = engineer
		combat_width = 0
		max_organisation = 20
		entrenchment = 2
		forest = { attack = 0.1 }
		need = { support_equipment = 10 }
		battalion_mult = { category = category_light_infantry max_strength = 0.1 }
	}
	field_guns = {
		sprite = field_guns
		active = yes
		group = support
		divisional = no
		allowed_battalion_groups = { infantry mobile }
		categories = { category_regimental_support_battalions }
		need = { artillery_equipment = 4 }
	}
	hq_infantry = {
		active = yes
		group = infantry
		regimental = no
		allow_in_army_hq = yes
		allow_in_non_army_hq = no
		need = { infantry_equipment = 10 }
	}
	destroyer = {
		map_icon_category = ship
		active = yes
	}
	fighter = { sprite = light_plane active = yes type = fighter need = { small_plane_airframe = 1 } }
	railway_gun = { map_icon_category = other active = yes type = { railway_gun } }
	TYR_new = {
	}
}
";
    const EQUIPMENT: &str = "equipments = {
	infantry_equipment = { year = 1936 is_archetype = yes active = yes maximum_speed = 4 defense = 20 breakthrough = 2 soft_attack = 3 hard_attack = 0.5 ap_attack = 1 build_cost_ic = 0.43 }
	infantry_equipment_0 = { year = 1918 archetype = infantry_equipment active = yes }
	infantry_equipment_1 = { year = 1936 archetype = infantry_equipment defense = 22 breakthrough = 3 soft_attack = 6 hard_attack = 1 ap_attack = 4 build_cost_ic = 0.5 }
	motorized_equipment = { year = 1936 is_archetype = yes maximum_speed = 12 hardness = 0.1 build_cost_ic = 2.5 }
	motorized_equipment_1 = { year = 1936 archetype = motorized_equipment }
	artillery_equipment = { year = 1936 is_archetype = yes maximum_speed = 4 defense = 10 breakthrough = 6 soft_attack = 25 hard_attack = 2 ap_attack = 5 build_cost_ic = 3.5 }
	artillery_equipment_1 = { year = 1934 archetype = artillery_equipment }
	support_equipment = { year = 1936 is_archetype = yes build_cost_ic = 4 }
	support_equipment_1 = { year = 1936 archetype = support_equipment }
}
";
    const TECHS: &str = "technologies = {
	infantry_weapons = { folder = { name = infantry_folder } enable_equipments = { infantry_equipment_0 } enable_subunits = { infantry } }
	infantry_weapons1 = { folder = { name = infantry_folder } start_year = 1936 enable_equipments = { infantry_equipment_1 } category_light_infantry = { soft_attack = 0.05 } }
	tech_engineers = { folder = { name = support_folder } start_year = 1936 enable_subunits = { engineer } }
	tech_support = { folder = { name = support_folder } start_year = 1936 enable_equipments = { support_equipment_1 } }
	gw_artillery = { folder = { name = artillery_folder } enable_equipments = { artillery_equipment_1 } }
	motorised_infantry = { folder = { name = infantry_folder } start_year = 1938 enable_subunits = { motorized } enable_equipments = { motorized_equipment_1 } }
	secret = { allow = { always = no } folder = { name = infantry_folder } start_year = 1936 infantry = { max_organisation = 10 } }
	mass_assault = { infantry = { combat_width = -0.2 } }
}
";

    fn db() -> UnitDb {
        let mut d = UnitDb::default();
        d.read_file("common/units/infantry.txt", &Script::parse(UNITS), false);
        d.read_file("common/units/equipment/infantry.txt", &Script::parse(EQUIPMENT), false);
        d.read_file("common/technologies/infantry.txt", &Script::parse(TECHS), false);
        d.read_file("common/terrain/00_terrain.txt", &Script::parse("categories = { ocean = { is_water = yes } plains = { combat_width = 70 combat_support_width = 35 } forest = { combat_width = 60 combat_support_width = 30 } }"), false);
        d
    }

    fn start(d: &UnitDb) -> Have {
        d.have(["infantry_weapons", "infantry_weapons1", "tech_support", "gw_artillery"].iter().map(|s| s.to_string()).collect())
    }

    fn template(text: &str) -> Template {
        let s = Script::parse(text);
        Template::read(s.root.block("division_template").unwrap())
    }

    #[test]
    fn units_are_read_with_where_they_go() {
        let d = db();
        assert!(!d.subunits.contains_key("destroyer") && !d.subunits.contains_key("fighter") && !d.subunits.contains_key("railway_gun"), "ships, planes and railway guns are left out");
        assert_eq!(d.subunits["TYR_new"].section(), Some(Section::Regiments), "a new battalion is an infantry one until it says else");
        assert_eq!(d.subunits["infantry"].section(), Some(Section::Regiments));
        assert_eq!(d.subunits["artillery"].section(), Some(Section::Support));
        assert_eq!(d.subunits["field_guns"].section(), Some(Section::RegimentalSupport));
        assert_eq!(d.subunits["hq_infantry"].section(), None);
        assert_eq!(d.subunits["infantry"].icon(), "GFX_unit_infantry_icon_medium");
        let h = start(&d);
        assert!(h.units.contains("infantry") && h.units.contains("artillery_brigade") && !h.units.contains("engineer") && !h.units.contains("motorized"));
        assert_eq!(d.best("infantry_equipment", &h).map(|e| e.id.as_str()), Some("infantry_equipment_1"), "the newest model");
        assert!(d.best("motorized_equipment", &h).is_none());
        let later = d.have(d.up_to(&h.techs, 1938, None));
        assert!(later.units.contains("motorized") && later.units.contains("engineer"));
        assert!(!later.techs.contains("secret") && !later.techs.contains("mass_assault"), "focus technologies are not counted by year");
        assert_eq!(d.years(), vec![1936, 1938]);
        assert_eq!(d.widths, vec![("plains".to_string(), 70.0, 35.0), ("forest".to_string(), 60.0, 30.0)]);
    }

    #[test]
    fn a_battalion_takes_its_equipment_and_bonuses() {
        let d = db();
        let h = start(&d);
        let inf = d.battalion(&d.subunits["infantry"], &h);
        assert!((inf.get("soft_attack") - 6.0 * 1.05).abs() < 1e-9, "{inf:?}");
        assert_eq!(inf.get("defense"), 22.0);
        assert_eq!(inf.get("maximum_speed"), 4.0);
        assert_eq!(inf.get("max_organisation"), 60.0);
        let art = d.battalion(&d.subunits["artillery"], &h);
        assert_eq!(art.get("soft_attack"), 12.5, "the company's -50%");
        let later = d.have(d.up_to(&h.techs, 1938, None));
        let mot = d.battalion(&d.subunits["motorized"], &later);
        assert_eq!(mot.get("maximum_speed"), 12.0, "its transport sets the speed");
        assert!((mot.get("hardness") - 0.1).abs() < 1e-9);
    }

    #[test]
    fn a_division_adds_and_averages() {
        let d = db();
        let h = d.have(d.up_to(&start(&d).techs, 1936, None));
        let t = template(
            "division_template = {
	name = \"Inf\"
	regiments = {
		infantry = { x = 0 y = 0 }
		infantry = { x = 0 y = 1 }
		artillery_brigade = { x = 1 y = 0 }
	}
	support = { engineer = { x = 0 y = 0 } }
}",
        );
        let div = d.division(&t, &h);
        assert_eq!(div.get("combat_width"), 7.0);
        assert_eq!((div.battalions, div.companies), (3, 1));
        assert!((div.get("max_strength") - (25.0 * 1.1 * 2.0 + 0.6)).abs() < 1e-9, "engineers give infantry +10% HP: {}", div.get("max_strength"));
        assert_eq!(div.get("max_organisation"), (60.0 + 60.0 + 0.0 + 20.0) / 4.0);
        assert_eq!(div.get("maximum_speed"), 4.0);
        assert_eq!(div.get("training_time"), 120.0);
        assert_eq!(div.get("manpower"), 2500.0);
        assert_eq!(div.get("entrenchment"), 2.0);
        assert!((div.get("soft_attack") - (6.3 * 2.0 + 25.0)).abs() < 1e-9);
        let ap = [4.0, 4.0, 5.0, 0.0];
        assert!((div.get("ap_attack") - (0.4 * 5.0 + 0.6 * ap.iter().sum::<f64>() / 4.0)).abs() < 1e-9);
        assert_eq!(div.equipment["infantry_equipment"], (Some("infantry_equipment_1".to_string()), 200.0));
        assert!((div.get("build_cost_ic") - (200.0 * 0.5 + 12.0 * 3.5 + 10.0 * 4.0)).abs() < 1e-9);
        // forest: infantry 0.1 averaged over 3 line battalions, engineers' 0.1 added
        assert!((div.terrain["forest"].attack - (0.2 / 3.0 + 0.1)).abs() < 1e-9);
    }

    /// Research by year keeps to what a player with these DLCs sees; the
    /// stockpile equips divisions.
    #[test]
    fn dlc_folders_and_the_stockpile() {
        let mut d = db();
        d.read_file("common/technology_tags/00_technology.txt", &Script::parse("technology_folders = { infantry_folder = { } support_folder = { } artillery_folder = { } nsb_folder = { available = { has_dlc = \"No Step Back\" } } mtg_folder = { available = { has_dlc = \"Man the Guns\" } } }"), false);
        d.read_file("common/technologies/x.txt", &Script::parse("technologies = { nsb_trucks = { folder = { name = nsb_folder } start_year = 1936 enable_equipments = { motorized_equipment_2 } } mtg_trucks = { folder = { name = mtg_folder } start_year = 1936 enable_equipments = { motorized_equipment_2 } } }"), false);
        d.read_file("common/units/equipment/x.txt", &Script::parse("equipments = { motorized_equipment_2 = { year = 1938 archetype = motorized_equipment } }"), false);
        assert_eq!(d.tech_need("nsb_trucks").describe(), "with No Step Back");
        // Man the Guns is part of the game: every player has it
        assert!(d.tech_need("mtg_trucks").is_empty());
        assert!(d.need_of("motorized_equipment_2").is_empty(), "mtg_trucks unlocks it for everyone");
        assert!(d.need_of("motorized_equipment_1").is_empty() && d.need_of("infantry_equipment_1").is_empty(), "unlocked by what every player sees");
        assert_eq!(d.dlcs(), ["No Step Back".to_string()].into());
        let start = start(&d).techs;
        let none = Owned::new();
        let nsb: Owned = ["No Step Back".to_string()].into();
        assert!(!d.up_to(&start, 1936, Some(&none)).contains("nsb_trucks") && d.up_to(&start, 1936, Some(&nsb)).contains("nsb_trucks") && d.up_to(&start, 1936, None).contains("nsb_trucks"));
        assert!(d.up_to(&start, 1936, Some(&none)).contains("mtg_trucks"));
        // a model in the stockpile equips divisions without the technology
        let mut h = d.have(start);
        assert!(d.best("motorized_equipment", &h).is_none());
        h.stock.insert("motorized_equipment_1".into(), 50.0);
        assert_eq!(d.best("motorized_equipment", &h).map(|e| e.id.as_str()), Some("motorized_equipment_1"));
    }

    #[test]
    fn templates_are_checked() {
        let d = db();
        let h = start(&d);
        let name = |s: &str| s.to_string();
        let msgs = |text: &str| -> Vec<String> { d.check(&template(text), &h, "TYR", &name).into_iter().map(|p| format!("{} {}", p.severity.label(), p.message)).collect() };
        assert_eq!(msgs("division_template = { name = A regiments = { infantry = { x = 0 y = 0 } } }"), Vec::<String>::new());
        let m = msgs(
            "division_template = {
	name = B
	regiments = {
		infantry = { x = 0 y = 0 }
		infantry = { x = 0 y = 0 }
		artillery_brigade = { x = 0 y = 1 }
		motorized = { x = 5 y = 0 }
		artillery = { x = 2 y = 0 }
		hq_infantry = { x = 3 y = 0 }
		tank = { x = 4 y = 0 }
	}
	support = {
		engineer = { x = 0 y = 0 }
		artillery = { x = 0 y = 1 }
		artillery = { x = 1 y = 1 }
		infantry = { x = 0 y = 3 }
	}
	regimental_support = {
		field_guns = { x = 0 y = 0 }
		field_guns = { x = 0 y = 1 }
		field_guns = { x = 4 y = 0 }
	}
}",
        );
        let want = [
            "error infantry and infantry are both at x = 0 y = 0 of regiments: the game keeps one",
            "error motorized at x = 5 y = 0: battalions go in columns x = 0 to 4, rows y = 0 to 4",
            "error motorized is not available to TYR at the start: it comes with motorised_infantry (give it in the nation's history: set_technology = { ... })",
            "error artillery is a support company and goes in support, not regiments",
            "error hq_infantry is for army headquarters, not divisions",
            "error tank is not a battalion of the game or the mod",
            "error engineer is not available to TYR at the start: it comes with tech_engineers (give it in the nation's history: set_technology = { ... })",
            "error artillery at x = 1 y = 1: support companies are one column: x = 0, y = 0 to 4",
            "error artillery twice: a division takes one of each support company",
            "error infantry is a battalion and goes in regiments, not support",
            "error field_guns and field_guns on column 1 (x = 0): a column takes one regimental support company",
            "error field_guns is on column 5 (x = 4), which has no battalions",
            "warning column 1 (x = 0) mixes infantry and combat support: the game's designer keeps one group per column",
            "warning TYR has no motorized_equipment at the start: its divisions of this template start without it (it comes with motorised_infantry, or put some in its stockpile)",
        ];
        for w in want {
            assert!(m.iter().any(|x| x == w), "missing {w:?} in\n{}", m.join("\n"));
        }
        assert_eq!(m.len(), want.len(), "{}", m.join("\n"));
        assert_eq!(msgs("division_template = { name = C support = { artillery = { x = 0 y = 0 } } }"), vec!["error no battalions: a template needs at least one".to_string()]);
    }
}
