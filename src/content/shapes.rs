//! Fields the wizard builds whole, from a form of their own instead of a
//! catalog: the parts of a block (a character's roles, a focus's offset, a
//! technology's folder), lists of words (categories, targets, the focuses
//! the AI takes), weights (`artillery = 5`, `100 = event.1`), quoted texts
//! (names), yes / no rules, a colour and the equipment bonuses of ideas and
//! traits.
//!
//! [`read`] takes what a field has into a [`Form`]; what the form does not
//! know is kept as it is and written back after the rest. [`write`] makes
//! the field's text again, [`problems`] says what is missing.

use std::collections::BTreeMap;

use super::kinds::Kind;
use super::script::{self, Block, Script};
use super::wizard::{Param, Role, indent, is_date, wrap};

/// How a field is built.
#[derive(Clone, Copy, Debug)]
pub enum Shape {
    /// Named parts, each once: `x = 1 y = 2 trigger = { ... }`.
    Values(&'static [Part]),
    /// Words: `{ a b c }`.
    Words(Words),
    /// `name = number` rows (`number = name` when swapped).
    Weights(Weights),
    /// Quoted texts: `{ "Anna" "Berta" }`; `loc`: localisation keys.
    Texts { loc: bool },
    /// `rule = yes / no`: (rule, what it does).
    Rules(&'static [(&'static str, &'static str)]),
    /// `{ r g b }`.
    Colour,
    /// `equipment = { stat = 0.1 instant = yes }`.
    EquipmentBonus,
}

#[derive(Clone, Copy, Debug)]
pub struct Words {
    pub of: Param,
    /// In the order given (the focuses the AI takes, one by one).
    pub ordered: bool,
    /// Each word has a block of its own: `conservatism = { }` (what is in it is kept).
    pub blocks: bool,
    pub help: &'static str,
}

#[derive(Clone, Copy, Debug)]
pub struct Weights {
    pub of: Param,
    /// `100 = event.1`: the number first.
    pub swap: bool,
    pub what: &'static str,
    pub weight: &'static str,
    pub help: &'static str,
}

/// A named part of a block.
#[derive(Clone, Copy, Debug)]
pub struct Part {
    pub key: &'static str,
    pub label: &'static str,
    pub ty: PartTy,
    /// It has to be there.
    pub need: bool,
    /// What a new field starts with (`$ID`: the definition's id, `$TAG`: the nation's tag).
    pub start: &'static str,
}

#[derive(Clone, Copy, Debug)]
pub enum PartTy {
    One(Param),
    /// A block of script, built with a catalog (or a weight).
    Script(Role),
    /// `traits = { a b }`.
    Words(Param),
    /// `names = { "A" "B" }`.
    Texts,
    /// A block of parts.
    Sub(&'static [Part]),
}

impl Shape {
    /// The page of the wizard it gets.
    pub fn role(&self) -> Role {
        match self {
            Shape::Values(_) => Role::Values,
            Shape::Words(_) => Role::Words,
            Shape::Weights(_) => Role::Weights,
            Shape::Texts { .. } => Role::Texts,
            Shape::Rules(_) => Role::Rules,
            Shape::Colour => Role::Colour,
            Shape::EquipmentBonus => Role::EquipmentBonus,
        }
    }
}

// ---------------------------------------------------------------- what the game has

pub const ADVISOR_SLOTS: &[&str] = &["political_advisor", "theorist", "army_chief", "navy_chief", "air_chief", "high_command"];
pub const RELATIONS: &[&str] = &["military_access", "non_aggression_pact", "guarantee", "docking_rights"];
pub const TECH_FOLDERS: &[&str] = &[
    "infantry_folder", "support_folder", "armour_folder", "nsb_armour_folder", "artillery_folder", "naval_folder", "air_techs_folder", "bba_air_techs_folder", "electronics_folder", "industry_folder", "land_doctrine_folder", "naval_doctrine_folder", "air_doctrine_folder",
];
pub const PEACE_ACTIONS: &[&str] = &["take_states", "puppet", "force_government", "liberate"];
pub const UNIT_TYPES: &[&str] = &[
    "infantry", "motorized", "mechanized", "armor", "artillery", "anti_tank", "anti_air", "rocket", "flame", "amphibious", "railway_gun", "support", "capital_ship", "screen_ship", "submarine", "carrier", "support_ship",
];
pub const UNIT_CATEGORIES: &[&str] = &[
    "category_army", "category_front_line", "category_all_infantry", "category_light_infantry", "category_infantry_and_bicycle", "category_militia", "category_mountaineers", "category_marines", "category_marines_and_amphibious", "category_paratroopers", "category_special_forces",
    "category_special_forces_leg_infantry", "category_rangers", "category_cavalry", "category_bus", "category_vehicle_infantry", "category_mobile_and_mobile_combat_sup", "category_all_armor", "category_tanks", "category_amphibious_tanks", "category_tank_destroyers",
    "category_mobile_anti_tank", "category_tank_destroyer_regimental_support", "category_artillery", "category_line_artillery", "category_support_artillery", "category_self_propelled_artillery", "category_rocket_artillery", "category_regimental_support_artillery",
    "category_anti_tank", "category_anti_air", "category_self_propelled_anti_air", "category_self_propelled_anti_air_regimental_support", "category_recon", "category_support_battalions", "category_divisional_support_battalions",
    "category_regimental_support_battalions", "category_helicopter_support_companies", "category_fighter", "category_heavy_fighter", "category_cas", "category_nav_bomber", "category_tac_bomber", "category_strat_bomber", "category_maritime_patrol_bomber",
    "category_scout_plane", "category_carrier_fighter", "category_carrier_cas", "category_carrier_nav_bomber",
];
pub const EQUIPMENT_STATS: &[&str] = &[
    "build_cost_ic", "reliability", "maximum_speed", "soft_attack", "hard_attack", "ap_attack", "defense", "breakthrough", "armor_value", "hardness", "max_strength", "fuel_consumption", "air_attack", "air_defence", "air_agility", "air_range", "air_ground_attack",
    "air_bombing", "naval_strike_attack", "naval_strike_targetting", "naval_speed", "naval_range", "lg_attack", "lg_armor_piercing", "hg_attack", "hg_armor_piercing", "torpedo_attack", "sub_attack", "anti_air_attack", "surface_detection", "sub_detection",
    "surface_visibility", "sub_visibility", "mines_planting", "mines_sweeping", "carrier_size",
];
/// The kinds of AI strategies (the game's common/ai_strategy/_documentation.md).
pub const AI_STRATEGIES: &[&str] = &[
    "alliance", "antagonize", "avoid_starting_wars", "asking_foreign_garrison", "befriend", "conquer", "consider_weak", "contain", "declare_war", "diplo_action_acceptance", "diplo_action_desire", "dont_join_wars_with", "ignore", "ignore_claim", "influence",
    "prepare_for_war", "protect", "send_lend_lease_desire", "send_volunteers_desire", "support", "area_priority", "dont_defend_ally_borders", "force_defend_ally_borders", "force_concentration_front_factor", "force_concentration_factor",
    "force_concentration_target_weight", "front_armor_score", "front_control", "front_unit_request", "garrison", "garrison_reinforcement_priority", "ignore_army_incompetence", "invasion_unit_request", "invade", "occupation_policy", "put_unit_buffers",
    "scorched_earth_prio", "spare_unit_factor", "theatre_distribution_demand_increase", "naval_avoid_region", "naval_convoy_raid_region", "naval_invasion_focus", "naval_invasion_dominance_weight", "naval_mission_threshold", "strike_force_home_base",
    "naval_dominance", "convoy_raiding_target", "activate_crypto", "agency_ai_base_num_factories_factor", "agency_ai_per_upgrade_factories_factor", "decrypt_target", "intelligence_agency_branch_desire_factor", "intelligence_agency_usable_factories",
    "operation_equipment_priority", "operative_mission", "operative_operation", "become_spymaster", "added_military_to_civilian_factory_ratio", "air_factory_balance", "build_airplane", "build_army", "build_building", "build_ship", "building_target",
    "convoy_efficiency_to_cancel_trades", "dockyard_to_military_factory_ratio", "equipment_production_factor", "equipment_variant_production_factor", "equipment_production_surplus_management", "equipment_production_min_factories",
    "equipment_production_min_factories_archetype", "equipment_stockpile_surplus_ratio", "equipment_market_spend_factories", "equipment_market_for_sale_threshold", "equipment_market_for_sale_factor", "equipment_market_max_for_sale",
    "equipment_market_min_for_sale", "equipment_market_buying_threshold", "equipment_market_buy", "equipment_market_trade_desire", "factory_build_score_factor", "force_build_armies", "fuel_buffer", "min_convoy_efficiency_factor_for_war_support_hit",
    "production_upgrade_desire_offset", "railway_gun_divisions_ratio", "research_tech", "research_weight_factor", "role_ratio", "save_equipment", "stockpile_fuel", "template_prio", "unit_ratio", "land_xp_spend_priority", "air_xp_spend_priority",
    "navy_xp_spend_priority", "pp_spend_amount", "pp_spend_priority", "min_wanted_supply_trucks", "wanted_supply_trucks", "min_wanted_supply_trains", "wanted_supply_trains", "ai_wanted_divisions_factor", "strategic_air_importance", "raid_target_country",
];
/// Rules of ideas and autonomies.
pub const COUNTRY_RULES: &[(&str, &str)] = &[
    ("can_not_declare_war", "cannot declare war"),
    ("can_decline_call_to_war", "may decline a call to arms"),
    ("units_deployed_to_overlord", "its units go to the overlord"),
    ("can_be_spymaster", "can be the spymaster of a faction"),
    ("contributes_operatives", "gives operatives to the spymaster"),
    ("can_create_factions", "can create factions"),
    ("can_join_factions", "can join factions"),
    ("can_join_opposite_factions", "can join factions of other ideologies"),
    ("can_send_volunteers", "can send volunteers"),
    ("can_guarantee_other_ideologies", "can guarantee other ideologies"),
    ("can_boost_other_ideologies", "can boost other ideologies"),
    ("can_create_collaboration_government", "can make collaboration governments"),
    ("can_access_market", "can use the international market"),
    ("can_puppet", "can make puppets"),
    ("can_force_government", "can force a government on the defeated"),
    ("can_lower_tension", "can lower world tension"),
    ("can_only_justify_war_on_threat_country", "justifies war only on threats"),
    ("can_declare_war_on_same_ideology", "can declare war on its own ideology"),
    ("country_lock_all_division_template", "cannot change its division templates"),
];
pub const IDEOLOGY_RULES: &[(&str, &str)] = &[
    ("can_force_government", "can force a government on the defeated"),
    ("can_send_volunteers", "can send volunteers"),
    ("can_puppet", "can make puppets"),
    ("can_lower_tension", "can lower world tension"),
    ("can_only_justify_war_on_threat_country", "justifies war only on threats"),
    ("can_guarantee_other_ideologies", "can guarantee other ideologies"),
    ("can_create_collaboration_government", "can make collaboration governments"),
    ("can_declare_war_on_same_ideology", "can declare war on its own ideology"),
    ("can_boost_other_ideologies", "can boost other ideologies"),
    ("can_use_dynamic_faction_names", "uses the faction names below"),
];

// ---------------------------------------------------------------- the parts

use Param::{Date, Loc, Number, Ref, Tag, Word, YesNo};
use PartTy::{One, Sub};

const fn need(key: &'static str, label: &'static str, ty: PartTy, start: &'static str) -> Part {
    Part { key, label, ty, need: true, start }
}
const fn opt(key: &'static str, label: &'static str, ty: PartTy, start: &'static str) -> Part {
    Part { key, label, ty, need: false, start }
}

const XY: &[Part] = &[need("x", "Across (x)", One(Number), "0"), need("y", "Down (y)", One(Number), "0")];
const CONTINUOUS: &[Part] = &[need("x", "Across (x)", One(Number), "50"), need("y", "Down (y)", One(Number), "1200")];
const INITIAL: &[Part] = &[opt("focus", "At the focus", One(Ref(Kind::Focus)), ""), opt("x", "Or across (x)", One(Number), "0"), opt("y", "and down (y)", One(Number), "0")];
const OFFSET: &[Part] = &[need("x", "Moved across (x)", One(Number), "1"), need("y", "Moved down (y)", One(Number), "0"), need("trigger", "When", PartTy::Script(Role::Trigger), "")];
const FOLDER: &[Part] = &[need("name", "Folder", One(Param::Of(TECH_FOLDERS)), "infantry_folder"), need("position", "Place in it", Sub(XY), "")];
const PATH: &[Part] = &[need("leads_to_tech", "Unlocks", One(Ref(Kind::Technology)), ""), opt("research_cost_coeff", "Cost factor", One(Number), "1")];
const AUTONOMY: &[Part] = &[need("target", "Subject", One(Tag), ""), need("autonomous_state", "Autonomy", One(Ref(Kind::Autonomy)), "autonomy_puppet"), opt("freedom_level", "Freedom (0 to 1)", One(Number), "")];
const RELATION: &[Part] = &[need("country", "With", One(Tag), ""), need("relation", "Relation", One(Param::Of(RELATIONS)), "military_access"), opt("active", "Active", One(YesNo), "yes")];
const LEADER: &[Part] = &[
    need("ideology", "Ideology", One(Param::SubIdeology), ""),
    opt("traits", "Traits", PartTy::Words(Ref(Kind::CountryLeaderTrait)), ""),
    opt("expire", "Leads until", One(Date), "1965.1.1"),
    opt("id", "Number (id)", One(Number), ""),
];
const GENERAL: &[Part] = &[
    opt("traits", "Traits", PartTy::Words(Ref(Kind::UnitLeaderTrait)), ""),
    need("skill", "Skill", One(Number), "1"),
    need("attack_skill", "Attack", One(Number), "1"),
    need("defense_skill", "Defense", One(Number), "1"),
    need("planning_skill", "Planning", One(Number), "1"),
    need("logistics_skill", "Logistics", One(Number), "1"),
    opt("legacy_id", "Legacy id", One(Number), ""),
];
const ADMIRAL: &[Part] = &[
    opt("traits", "Traits", PartTy::Words(Ref(Kind::UnitLeaderTrait)), ""),
    need("skill", "Skill", One(Number), "1"),
    need("attack_skill", "Attack", One(Number), "1"),
    need("defense_skill", "Defense", One(Number), "1"),
    need("maneuvering_skill", "Maneuvering", One(Number), "1"),
    need("coordination_skill", "Coordination", One(Number), "1"),
    opt("legacy_id", "Legacy id", One(Number), ""),
];
const ADVISOR: &[Part] = &[
    need("slot", "Slot", One(Param::Of(ADVISOR_SLOTS)), "political_advisor"),
    need("idea_token", "Idea token", One(Word), "$ID"),
    opt("cost", "Cost (political power)", One(Number), "150"),
    opt("traits", "Traits", PartTy::Words(Ref(Kind::CountryLeaderTrait)), ""),
    opt("allowed", "Who can have them", PartTy::Script(Role::Trigger), "original_tag = $TAG"),
    opt("visible", "Shown when", PartTy::Script(Role::Trigger), ""),
    opt("available", "Can be hired when", PartTy::Script(Role::Trigger), ""),
    opt("ai_will_do", "AI weight", PartTy::Script(Role::AiWeight), "factor = 1"),
];
const SKILLS: &[Part] = &[
    opt("specialization_land", "Land", One(Number), "1"),
    opt("specialization_naval", "Naval", One(Number), ""),
    opt("specialization_air", "Air", One(Number), ""),
    opt("specialization_nuclear", "Nuclear", One(Number), ""),
];
const SCIENTIST: &[Part] = &[opt("traits", "Traits", PartTy::Words(Ref(Kind::ScientistTrait)), ""), need("skills", "Skill levels", Sub(SKILLS), ""), opt("visible", "Shown when", PartTy::Script(Role::Trigger), "")];
/// A design (`create_equipment_variant`); its modules are kept as they
/// are (the Navy page's ship designer fits them).
const VARIANT: &[Part] = &[
    need("name", "Name", One(Param::Text), "$ID Class"),
    need("type", "Hull or model", One(Ref(Kind::Equipment)), "ship_hull_light_1"),
    opt("obsolete", "Obsolete", One(YesNo), ""),
];
const PORTRAIT: &[Part] = &[opt("large", "Large", One(Param::Sprite), ""), opt("small", "Small", One(Param::Sprite), "")];
const PORTRAITS: &[Part] = &[opt("civilian", "As a civilian", Sub(PORTRAIT), ""), opt("army", "In the army", Sub(PORTRAIT), ""), opt("navy", "In the navy", Sub(PORTRAIT), "")];
const TEXT: &[Part] = &[opt("trigger", "When", PartTy::Script(Role::Trigger), ""), need("localization_key", "Text", One(Loc), "")];
const STRATEGY: &[Part] = &[need("type", "Kind", One(Param::Of(AI_STRATEGIES)), "befriend"), opt("id", "Who or what", One(Tag), ""), need("value", "How much", One(Number), "100")];
const LEVEL: &[Part] = &[opt("modifier", "Modifiers", PartTy::Script(Role::Modifier), ""), opt("complete_effect", "When it is reached", PartTy::Script(Role::Effect), "")];
const NAMES: &[Part] = &[need("names", "First names", PartTy::Texts, ""), opt("surnames", "Their own surnames", PartTy::Texts, ""), opt("callsigns", "Their own callsigns", PartTy::Texts, "")];

const fn words(of: Param, help: &'static str) -> Shape {
    Shape::Words(Words { of, ordered: false, blocks: false, help })
}
const fn weights(of: Param, what: &'static str, weight: &'static str, help: &'static str) -> Shape {
    Shape::Weights(Weights { of, swap: false, what, weight, help })
}

/// The shape of a field that is built whole, if it is one.
pub fn shape_of(kind: Kind, key: &str) -> Option<Shape> {
    use Kind as K;
    Some(match (kind, key) {
        (K::FocusTree, "continuous_focus_position") => Shape::Values(CONTINUOUS),
        (K::FocusTree, "initial_show_position") => Shape::Values(INITIAL),
        (K::Focus | K::SharedFocus, "offset") => Shape::Values(OFFSET),
        (K::Technology, "folder") => Shape::Values(FOLDER),
        (K::Technology, "path") => Shape::Values(PATH),
        (K::Technology, "categories") => words(Param::TechCategory, "The categories it is in: research bonuses and the AI go by them."),
        (K::Technology, "enable_equipments") => words(Ref(Kind::Equipment), "The equipment the technology unlocks."),
        (K::Technology, "enable_subunits") => words(Ref(Kind::SubUnit), "The battalions and companies the technology unlocks."),
        (K::SubUnit, "type") => words(Param::Of(UNIT_TYPES), "What it is: modifiers of a type apply to it."),
        (K::SubUnit, "categories") => words(Param::Of(UNIT_CATEGORIES), "Its categories: modifiers and doctrines of a category apply to it."),
        (K::SubUnit, "need") => weights(Ref(Kind::Equipment), "Equipment", "How many", "The equipment a full battalion needs."),
        (K::Equipment, "resources") => weights(Param::Resource, "Resource", "How much", "Resources one factory takes to make it."),
        (K::CountryHistory, "set_autonomy") => Shape::Values(AUTONOMY),
        (K::CountryHistory, "diplomatic_relation") => Shape::Values(RELATION),
        (K::CountryHistory, "create_equipment_variant") => Shape::Values(VARIANT),
        (K::CountryHistory, "set_variable") => weights(Word, "Variable", "Value", "Variables the country starts with."),
        (K::Character, "country_leader") => Shape::Values(LEADER),
        (K::Character, "corps_commander" | "field_marshal") => Shape::Values(GENERAL),
        (K::Character, "navy_leader") => Shape::Values(ADMIRAL),
        (K::Character, "advisor") => Shape::Values(ADVISOR),
        (K::Character, "scientist") => Shape::Values(SCIENTIST),
        (K::Character, "portraits") => Shape::Values(PORTRAITS),
        (K::ScriptedLoc, "text") => Shape::Values(TEXT),
        (K::OnAction, "random_events") => Shape::Weights(Weights { of: Ref(Kind::Event), swap: true, what: "Event (0: nothing)", weight: "Weight", help: "One of these fires, picked by weight; 0 as the event is a chance of nothing." }),
        (K::AiStrategy, "ai_strategy") => Shape::Values(STRATEGY),
        (K::AiStrategyPlan, "ai_national_focuses") => Shape::Words(Words { of: Ref(Kind::Focus), ordered: true, blocks: false, help: "The focuses the AI takes, in this order." }),
        (K::AiStrategyPlan, "research") => weights(Param::TechCategory, "Category", "Weight", "How much the AI researches each category (1 as usual)."),
        (K::AiStrategyPlan, "ideas") => weights(Ref(Kind::Idea), "Idea", "Weight", "How much the AI wants each idea (1 as usual)."),
        (K::Ideology, "types") => Shape::Words(Words { of: Param::SubIdeology, ordered: false, blocks: true, help: "Its sub-ideologies: what a leader's ideology is. New ones get a block of their own (what is in the old ones is kept)." }),
        (K::Ideology, "dynamic_faction_names") => Shape::Texts { loc: true },
        (K::Ideology, "color") => Shape::Colour,
        (K::Ideology, "rules") => Shape::Rules(IDEOLOGY_RULES),
        (K::Idea | K::Autonomy, "rule") => Shape::Rules(COUNTRY_RULES),
        (K::Idea | K::CountryLeaderTrait, "equipment_bonus") => Shape::EquipmentBonus,
        (K::PeaceCost, "peace_action_type") => words(Param::Of(PEACE_ACTIONS), "The peace actions it applies to."),
        (K::Decision, "targets") => words(Tag, "The countries the decision can target."),
        (K::AgencyUpgrade, "level") => Shape::Values(LEVEL),
        (K::Names, "male" | "female") => Shape::Values(NAMES),
        (K::Names, "surnames" | "callsigns" | "ship_names") => Shape::Texts { loc: false },
        _ => return None,
    })
}

// ---------------------------------------------------------------- the form

/// A field as the form has it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Form {
    /// Values of parts by path (`civilian.large`): a word, the inner text of
    /// a block, words with spaces between, texts one per line.
    pub vals: BTreeMap<String, String>,
    /// What the form does not know, by block (`""`: the field, `civilian`):
    /// written back as it is.
    pub others: BTreeMap<String, Vec<String>>,
    pub words: Vec<String>,
    /// What the blocks of words have (sub-ideologies).
    pub bodies: BTreeMap<String, String>,
    /// (name, weight)
    pub rows: Vec<(String, String)>,
    /// One per line.
    pub texts: String,
    pub rules: Vec<(String, Option<bool>)>,
    pub colour: [u8; 3],
    pub groups: Vec<Group>,
    /// The field has comments: they do not come back.
    pub comments: bool,
    /// The field does not parse: it is not to be replaced.
    pub broken: Option<String>,
}

/// The bonus of one equipment.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Group {
    pub equipment: String,
    /// `instant = yes`: for the equipment there, without a new model.
    pub instant: bool,
    pub stats: Vec<(String, String)>,
    pub others: Vec<String>,
}

/// What `$ID` and `$TAG` are in what a new field starts with.
pub struct Fill<'a> {
    pub id: &'a str,
    pub tag: &'a str,
}

fn has_comment(text: &str) -> bool {
    let mut quoted = false;
    for c in text.chars() {
        match c {
            '"' => quoted = !quoted,
            '#' if !quoted => return true,
            '\n' => quoted = false,
            _ => {}
        }
    }
    false
}

/// The field's inner text into a form (None: a new field, it starts with
/// the parts' first values).
pub fn read(shape: &Shape, inner: Option<&str>, fill: &Fill) -> Form {
    let text = inner.unwrap_or("");
    let s = Script::parse(text);
    let mut f = Form { comments: has_comment(text), ..Form::default() };
    if let Some(e) = s.errors.first() {
        f.broken = Some(format!("line {}: {}", script::line_of(text, e.at), e.message));
    }
    let b = &s.root;
    let src = |i: &script::Item| script::item_source(text, i);
    let mut others: Vec<String> = Vec::new();
    match shape {
        Shape::Values(parts) => read_parts(parts, b, text, "", inner.is_none(), fill, &mut f),
        Shape::Words(w) => {
            for i in &b.items {
                match (&i.key, i.atom(), i.block()) {
                    (None, Some(a), _) => f.words.push(a.text.clone()),
                    (Some(k), _, Some(bl)) if w.blocks => {
                        f.words.push(k.text.clone());
                        f.bodies.insert(k.text.clone(), script::inner_text(text, bl));
                    }
                    _ => others.push(src(i)),
                }
            }
        }
        Shape::Weights(w) => {
            for i in &b.items {
                match (&i.key, i.atom()) {
                    (Some(k), Some(a)) if i.op.as_ref().is_none_or(|o| o.0 == script::Op::Eq) => f.rows.push(if w.swap { (a.text.clone(), k.text.clone()) } else { (k.text.clone(), a.text.clone()) }),
                    _ => others.push(src(i)),
                }
            }
        }
        Shape::Texts { .. } => {
            let mut lines = Vec::new();
            for i in &b.items {
                match (&i.key, i.atom()) {
                    (None, Some(a)) => lines.push(a.text.clone()),
                    _ => others.push(src(i)),
                }
            }
            f.texts = lines.join("\n");
        }
        Shape::Rules(known) => {
            f.rules = known.iter().map(|(r, _)| (r.to_string(), None)).collect();
            for i in &b.items {
                let v = i.text().map(|v| v.to_ascii_lowercase());
                match (&i.key, v.as_deref()) {
                    (Some(k), Some(v @ ("yes" | "no"))) => {
                        let on = Some(v == "yes");
                        match f.rules.iter_mut().find(|r| r.0 == k.text) {
                            Some(r) if r.1.is_none() => r.1 = on,
                            Some(_) => others.push(src(i)),
                            None => f.rules.push((k.text.clone(), on)),
                        }
                    }
                    _ => others.push(src(i)),
                }
            }
        }
        Shape::Colour => {
            let mut n: Vec<(f64, bool)> = Vec::new();
            for i in &b.items {
                match (&i.key, i.text().and_then(|t| t.parse::<f64>().ok().map(|v| (v, t.contains('.'))))) {
                    (None, Some(v)) if n.len() < 3 => n.push(v),
                    _ => others.push(src(i)),
                }
            }
            // 0..1 written with decimals: of 1, not of 255
            let unit = n.iter().any(|x| x.1) && n.iter().all(|x| x.0 <= 1.0);
            f.colour = [128, 128, 128];
            for (c, (v, _)) in f.colour.iter_mut().zip(&n) {
                *c = (if unit { v * 255.0 } else { *v }).round().clamp(0.0, 255.0) as u8;
            }
        }
        Shape::EquipmentBonus => {
            for i in &b.items {
                let (Some(k), Some(bl)) = (&i.key, i.block()) else {
                    others.push(src(i));
                    continue;
                };
                let mut g = Group { equipment: k.text.clone(), ..Group::default() };
                for x in &bl.items {
                    match (&x.key, x.text()) {
                        (Some(k), Some(v)) if k.text == "instant" => g.instant = v.eq_ignore_ascii_case("yes"),
                        (Some(k), Some(v)) if v.parse::<f64>().is_ok() => g.stats.push((k.text.clone(), v.to_string())),
                        _ => g.others.push(script::item_source(text, x)),
                    }
                }
                f.groups.push(g);
            }
        }
    }
    if !others.is_empty() {
        f.others.insert(String::new(), others);
    }
    f
}

fn start_value(p: &Part, fill: &Fill) -> String {
    p.start.replace("$ID", fill.id).replace("$TAG", fill.tag)
}

fn read_parts(parts: &[Part], b: &Block, text: &str, prefix: &str, new: bool, fill: &Fill, f: &mut Form) {
    let mut used = vec![false; b.items.len()];
    for p in parts {
        let path = format!("{prefix}{}", p.key);
        let at = b.items.iter().position(|i| i.is(p.key));
        let it = at.map(|n| &b.items[n]);
        let fits = match (p.ty, it) {
            (_, None) => false,
            (One(_), Some(i)) => i.atom().is_some(),
            (PartTy::Script(_), Some(i)) | (Sub(_), Some(i)) => i.block().is_some(),
            (PartTy::Words(_) | PartTy::Texts, Some(i)) => i.atom().is_some() || i.block().is_some(),
        };
        if let (true, Some(n)) = (fits, at) {
            used[n] = true;
        }
        let it = it.filter(|_| fits);
        match p.ty {
            Sub(ps) => {
                let empty = Block::default();
                let inner = it.and_then(|i| i.block()).unwrap_or(&empty);
                read_parts(ps, inner, text, &format!("{path}."), new && it.is_none(), fill, f);
                continue;
            }
            _ if it.is_none() => {
                if new {
                    f.vals.insert(path, start_value(p, fill));
                }
                continue;
            }
            _ => {}
        }
        let it = it.unwrap();
        let v = match p.ty {
            One(_) => it.text().unwrap_or("").to_string(),
            PartTy::Script(_) => it.block().map(|bl| script::inner_text(text, bl)).unwrap_or_default(),
            PartTy::Words(_) | PartTy::Texts => {
                let sep = if matches!(p.ty, PartTy::Texts) { "\n" } else { " " };
                match it.block() {
                    Some(bl) => {
                        let (vals, rest): (Vec<&script::Item>, Vec<&script::Item>) = bl.items.iter().partition(|x| x.key.is_none() && x.atom().is_some());
                        if !rest.is_empty() {
                            // `traits = { a b c = d }`: the odd ones stay with the field
                            f.others.entry(path.clone()).or_default().extend(rest.iter().map(|x| script::item_source(text, x)));
                        }
                        vals.iter().filter_map(|x| x.text()).collect::<Vec<_>>().join(sep)
                    }
                    None => it.text().unwrap_or("").to_string(),
                }
            }
            Sub(_) => unreachable!(),
        };
        f.vals.insert(path, v);
    }
    let rest: Vec<String> = b.items.iter().zip(&used).filter(|(_, u)| !**u).map(|(i, _)| script::item_source(text, i)).collect();
    if !rest.is_empty() {
        f.others.entry(prefix.trim_end_matches('.').to_string()).or_default().extend(rest);
    }
}

/// A value as written.
fn value_text(p: Param, v: &str) -> String {
    match p {
        Param::Number => v.to_string(),
        Param::Date | Param::Text => script::quote(v),
        _ => script::word(v),
    }
}

/// Words on lines of about `width` characters.
fn fill_lines(words: &[String], width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for w in words {
        if !cur.is_empty() && cur.len() + 1 + w.len() > width {
            lines.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(w);
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

fn quoted_lines(texts: &str) -> Vec<String> {
    let q: Vec<String> = texts.lines().map(str::trim).filter(|t| !t.is_empty()).map(|t| script::quote(t.trim_matches('"'))).collect();
    fill_lines(&q, 90)
}

/// `key = { a b }` on one line, or a block of lines.
fn list_block(key: &str, lines: &[String]) -> String {
    match lines {
        [] => format!("{key} = {{ }}"),
        [one] if one.len() < 70 => format!("{key} = {{ {one} }}"),
        _ => format!("{key} = {{\n{}\n}}", indent(&lines.join("\n"))),
    }
}

/// The field's inner text from the form.
pub fn write(shape: &Shape, f: &Form) -> String {
    let mut lines: Vec<String> = match shape {
        Shape::Values(parts) => write_parts(parts, f, ""),
        Shape::Words(w) => {
            let ws: Vec<String> = f.words.iter().map(|x| script::word(x)).collect();
            if w.blocks {
                f.words.iter().map(|x| wrap(&script::word(x), f.bodies.get(x).map(String::as_str).unwrap_or(""))).collect()
            } else if w.ordered {
                ws
            } else {
                fill_lines(&ws, 90)
            }
        }
        Shape::Weights(w) => f.rows.iter().filter(|r| !r.0.trim().is_empty()).map(|(what, n)| if w.swap { format!("{} = {}", n.trim(), script::word(what.trim())) } else { format!("{} = {}", script::word(what.trim()), n.trim()) }).collect(),
        Shape::Texts { .. } => quoted_lines(&f.texts),
        Shape::Rules(_) => f.rules.iter().filter_map(|(r, v)| v.map(|v| format!("{r} = {}", if v { "yes" } else { "no" }))).collect(),
        Shape::Colour => vec![format!("{} {} {}", f.colour[0], f.colour[1], f.colour[2])],
        Shape::EquipmentBonus => f
            .groups
            .iter()
            .filter(|g| !g.equipment.trim().is_empty())
            .map(|g| {
                let mut body: Vec<String> = g.stats.iter().filter(|s| !s.0.trim().is_empty()).map(|(k, v)| format!("{} = {}", k.trim(), v.trim())).collect();
                if g.instant {
                    body.push("instant = yes".into());
                }
                body.extend(g.others.iter().cloned());
                wrap(&script::word(g.equipment.trim()), &body.join("\n"))
            })
            .collect(),
    };
    lines.extend(f.others.get("").cloned().unwrap_or_default());
    lines.join("\n")
}

fn write_parts(parts: &[Part], f: &Form, prefix: &str) -> Vec<String> {
    let mut lines = Vec::new();
    for p in parts {
        let path = format!("{prefix}{}", p.key);
        let v = f.vals.get(&path).map(|s| s.trim()).unwrap_or("");
        let odd = f.others.get(&path).cloned().unwrap_or_default();
        match p.ty {
            One(param) => {
                if !v.is_empty() {
                    lines.push(format!("{} = {}", p.key, value_text(param, v)));
                }
            }
            PartTy::Script(_) => {
                if !v.is_empty() || p.need {
                    lines.push(wrap(p.key, v));
                }
            }
            PartTy::Words(_) => {
                let mut ws: Vec<String> = v.split_whitespace().map(script::word).collect();
                ws.extend(odd);
                if !ws.is_empty() || p.need {
                    lines.push(list_block(p.key, &fill_lines(&ws, 80)));
                }
            }
            PartTy::Texts => {
                let mut ls = quoted_lines(v);
                ls.extend(odd);
                if !ls.is_empty() || p.need {
                    lines.push(list_block(p.key, &ls));
                }
            }
            Sub(ps) => {
                let mut inner = write_parts(ps, f, &format!("{path}."));
                inner.extend(f.others.get(&path).cloned().unwrap_or_default());
                // short parts of words on one line: `position = { x = 0 y = -1 }`
                let short = inner.iter().all(|l| !l.contains('\n')) && inner.iter().map(|l| l.len() + 1).sum::<usize>() < 50;
                if short && !inner.is_empty() {
                    lines.push(format!("{} = {{ {} }}", p.key, inner.join(" ")));
                } else if !inner.is_empty() || p.need {
                    lines.push(wrap(p.key, &inner.join("\n")));
                }
            }
        }
    }
    lines
}

/// What keeps the form from being written: parts missing, numbers that are
/// not numbers, script that does not parse.
pub fn problems(shape: &Shape, f: &Form) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(e) = &f.broken {
        out.push(format!("the field does not parse ({e}): fix it in the form first"));
        return out;
    }
    let number = |v: &str| v.trim().parse::<f64>().is_ok();
    match shape {
        Shape::Values(parts) => parts_problems(parts, f, "", &mut out),
        Shape::Weights(w) => {
            for (what, n) in &f.rows {
                if what.trim().is_empty() && !n.trim().is_empty() {
                    out.push(format!("a row without a {}", w.what.to_lowercase()));
                } else if !what.trim().is_empty() && !number(n) {
                    out.push(format!("{}: the {} is a number", what.trim(), w.weight.to_lowercase()));
                }
            }
        }
        Shape::EquipmentBonus => {
            for g in &f.groups {
                if g.equipment.trim().is_empty() && !g.stats.is_empty() {
                    out.push("a bonus without its equipment".into());
                }
                for (k, v) in &g.stats {
                    if !k.trim().is_empty() && !number(v) {
                        out.push(format!("{} {}: a number", g.equipment.trim(), k.trim()));
                    }
                }
            }
        }
        _ => {}
    }
    out
}

fn parts_problems(parts: &[Part], f: &Form, prefix: &str, out: &mut Vec<String>) {
    for p in parts {
        let path = format!("{prefix}{}", p.key);
        let v = f.vals.get(&path).map(|s| s.trim()).unwrap_or("");
        match p.ty {
            Sub(ps) => parts_problems(ps, f, &format!("{path}."), out),
            _ if v.is_empty() => {
                if p.need {
                    out.push(format!("{}: missing", p.label));
                }
            }
            One(Number) if v.parse::<f64>().is_err() => out.push(format!("{}: a number", p.label)),
            One(Number) => {
                if let Some(Err(why)) = super::limits::of_part(p.key).map(|r| r.check_text(v)) {
                    out.push(format!("{}: {why}", p.label));
                }
            }
            One(Date) if !is_date(v) => out.push(format!("{}: a date like 1936.1.1", p.label)),
            PartTy::Script(_) => {
                if let Some(e) = Script::parse(v).errors.first() {
                    out.push(format!("{}: line {}: {}", p.label, script::line_of(v, e.at), e.message));
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fill() -> Fill<'static> {
        Fill { id: "TYR_andreas_hofer", tag: "TYR" }
    }

    fn round(kind: Kind, key: &str, inner: &str) -> (Form, String) {
        let s = shape_of(kind, key).unwrap();
        let f = read(&s, Some(inner), &fill());
        let out = write(&s, &f);
        (f, out)
    }

    #[test]
    fn every_block_field_has_a_wizard() {
        use crate::content::kinds::FieldType;
        let mut none = Vec::new();
        for k in Kind::all() {
            for f in k.spec().fields {
                if matches!(f.ty, FieldType::Block) && crate::content::wizard::role_of(k, f.key).is_none() {
                    none.push(format!("{k:?}.{}", f.key));
                }
            }
        }
        assert!(none.is_empty(), "block fields without a wizard: {none:?}");
    }

    #[test]
    fn parts_come_back_with_what_the_form_does_not_know() {
        let (f, out) = round(Kind::Character, "country_leader", "ideology = nazism\ntraits = { GER_der_fuhrer }\nexpire = \"1965.1.1.1\"\nid = -1\ndesc = GER_desc");
        assert_eq!(f.vals["ideology"], "nazism");
        assert_eq!(f.vals["traits"], "GER_der_fuhrer");
        assert_eq!(f.others[""], vec!["desc = GER_desc".to_string()]);
        assert_eq!(out, "ideology = nazism\ntraits = { GER_der_fuhrer }\nexpire = \"1965.1.1.1\"\nid = -1\ndesc = GER_desc");
        // nested parts, blocks of script
        let (f, out) = round(Kind::Technology, "folder", "name = infantry_folder\nposition = { x = 0 y = -1 }");
        assert_eq!((f.vals["position.x"].as_str(), f.vals["position.y"].as_str()), ("0", "-1"));
        assert_eq!(out, "name = infantry_folder\nposition = { x = 0 y = -1 }");
        let (f, out) = round(Kind::Focus, "offset", "x = 2\ny = 0\ntrigger = {\n\thas_war = yes\n}");
        assert_eq!(f.vals["trigger"], "has_war = yes");
        assert_eq!(out, "x = 2\ny = 0\ntrigger = { has_war = yes }");
        // portraits: parts of parts, the empty ones left out
        let (_, out) = round(Kind::Character, "portraits", "civilian = {\n\tlarge = GFX_a\n}\narmy = { small = GFX_b }");
        assert_eq!(out, "civilian = { large = GFX_a }\narmy = { small = GFX_b }");
    }

    #[test]
    fn a_new_field_starts_filled() {
        let s = shape_of(Kind::Character, "advisor").unwrap();
        let f = read(&s, None, &fill());
        assert_eq!(f.vals["idea_token"], "TYR_andreas_hofer");
        let out = write(&s, &f);
        assert!(out.starts_with("slot = political_advisor\nidea_token = TYR_andreas_hofer\ncost = 150\nallowed = { original_tag = TYR }"), "{out}");
        assert!(problems(&s, &f).is_empty());
        let s = shape_of(Kind::Focus, "offset").unwrap();
        let f = read(&s, None, &fill());
        assert_eq!(problems(&s, &f), vec!["When: missing".to_string()]);
        let mut f = f;
        f.vals.insert("x".into(), "two".into());
        f.vals.insert("trigger".into(), "has_war = {".into());
        let p = problems(&s, &f);
        assert!(p[0].contains("a number") && p[1].starts_with("When: line 1"), "{p:?}");
        // numbers the game allows
        f.vals.insert("x".into(), "1.5".into());
        f.vals.insert("trigger".into(), "has_war = yes".into());
        assert_eq!(problems(&s, &f), vec!["Moved across (x): a whole number".to_string()]);
        let s = shape_of(Kind::CountryHistory, "set_autonomy").unwrap();
        let mut f = read(&s, None, &fill());
        f.vals.insert("target".into(), "TYR".into());
        f.vals.insert("freedom_level".into(), "1.5".into());
        assert_eq!(problems(&s, &f), vec!["Freedom (0 to 1): from 0 to 1".to_string()]);
        let s = shape_of(Kind::Character, "corps_commander").unwrap();
        let mut f = read(&s, None, &fill());
        f.vals.insert("skill".into(), "0".into());
        assert_eq!(problems(&s, &f), vec!["Skill: a whole number, 1 or more".to_string()]);
    }

    #[test]
    fn lists_weights_texts_rules_colours() {
        let (f, out) = round(Kind::Technology, "categories", "infantry_weapons\nartillery");
        assert_eq!(f.words, vec!["infantry_weapons", "artillery"]);
        assert_eq!(out, "infantry_weapons artillery");
        // one word without braces
        let (f, _) = round(Kind::PeaceCost, "peace_action_type", "force_government");
        assert_eq!(f.words, vec!["force_government"]);
        // the focuses of the AI, one per line, in order
        let (_, out) = round(Kind::AiStrategyPlan, "ai_national_focuses", "a b c");
        assert_eq!(out, "a\nb\nc");
        // sub-ideologies keep what they have
        let s = shape_of(Kind::Ideology, "types").unwrap();
        let mut f = read(&s, Some("conservatism = {\n}\nliberalism = { can_be_randomly_selected = no }"), &fill());
        f.words.push("tyrolean_patriotism".into());
        assert_eq!(write(&s, &f), "conservatism = { }\nliberalism = { can_be_randomly_selected = no }\ntyrolean_patriotism = { }");
        // weights, the event ones the other way round
        let (f, out) = round(Kind::OnAction, "random_events", "100 = tyr.1\n50 = 0");
        assert_eq!(f.rows, vec![("tyr.1".to_string(), "100".to_string()), ("0".to_string(), "50".to_string())]);
        assert_eq!(out, "100 = tyr.1\n50 = 0");
        let s = shape_of(Kind::Equipment, "resources").unwrap();
        let mut f = read(&s, Some("steel = 2"), &fill());
        f.rows.push(("oil".into(), "x".into()));
        assert_eq!(problems(&s, &f), vec!["oil: the how much is a number".to_string()]);
        // texts are quoted
        let (f, out) = round(Kind::Names, "surnames", "\"Hofer\" \"Speckbacher\" Haspinger");
        assert_eq!(f.texts, "Hofer\nSpeckbacher\nHaspinger");
        assert_eq!(out, "\"Hofer\" \"Speckbacher\" \"Haspinger\"");
        let (f, out) = round(Kind::Names, "male", "names = { \"Andreas\" \"Josef\" }\nsurnames = { }");
        assert_eq!(f.vals["names"], "Andreas\nJosef");
        assert_eq!(out, "names = { \"Andreas\" \"Josef\" }");
        // rules: the known ones in their order, others after
        let (f, out) = round(Kind::Idea, "rule", "can_create_factions = no\nmy_rule = yes\ncan_not_declare_war = yes");
        assert_eq!(f.rules.iter().filter(|r| r.1.is_some()).count(), 3);
        assert_eq!(out, "can_not_declare_war = yes\ncan_create_factions = no\nmy_rule = yes");
        let (f, out) = round(Kind::Ideology, "color", "0 0 255");
        assert_eq!(f.colour, [0, 0, 255]);
        assert_eq!(out, "0 0 255");
        let (f, _) = round(Kind::Ideology, "color", "0.5 1.0 0");
        assert_eq!(f.colour, [128, 255, 0]);
    }

    #[test]
    fn equipment_bonuses() {
        let (f, out) = round(Kind::Idea, "equipment_bonus", "infantry_equipment = {\n\tbuild_cost_ic = -0.05 instant = yes\n}\nartillery_equipment = { soft_attack = 0.1 }");
        assert_eq!(f.groups.len(), 2);
        assert!(f.groups[0].instant && !f.groups[1].instant);
        assert_eq!(out, "infantry_equipment = {\n\tbuild_cost_ic = -0.05\n\tinstant = yes\n}\nartillery_equipment = { soft_attack = 0.1 }");
    }

    #[test]
    fn broken_fields_are_not_replaced() {
        let s = shape_of(Kind::Character, "navy_leader").unwrap();
        let f = read(&s, Some("skill = { 3"), &fill());
        assert!(f.broken.is_some());
        assert!(problems(&s, &f)[0].contains("does not parse"));
        assert!(read(&s, Some("skill = 3 # the best"), &fill()).comments);
    }
}
