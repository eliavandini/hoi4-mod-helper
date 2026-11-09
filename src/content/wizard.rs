//! What the wizards know, apart from their windows: which wizard a field of
//! a definition gets, a hand-made catalog of the effects, triggers and
//! modifiers used most (with the kind of value each part takes, so the
//! window can offer pickers), and how what was built goes into a field.
//!
//! The catalog covers the everyday; the game's documentation
//! ([`super::docs`]) has everything else.

use super::kinds::Kind;
use super::script::{self, Script};

/// The wizard a field gets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    Effect,
    Trigger,
    Modifier,
    /// `ai_will_do`, `ai_chance`: a base factor and modifiers that multiply it.
    AiWeight,
    /// `mean_time_to_happen`: days, and modifiers that multiply them.
    Mtth,
    /// A focus tree's `country`: which countries get it.
    TreeCountry,
    /// `prerequisite` / `mutually_exclusive` of a focus.
    Prerequisite,
    Exclusive,
    /// Country history.
    Politics,
    Popularities,
    Technologies,
    Ideas,
    /// An idea's `research_bonus`.
    ResearchBonus,
    /// A focus's `search_filters`.
    SearchFilters,
    /// A division template's battalions and support companies.
    Regiments,
    Support,
    /// Fields built whole from a form of their own ([`super::shapes`]):
    /// named parts, words, weights, quoted texts, yes / no rules, a colour,
    /// equipment bonuses.
    Values,
    Words,
    Weights,
    Texts,
    Rules,
    Colour,
    EquipmentBonus,
}

impl Role {
    pub fn label(self) -> &'static str {
        match self {
            Role::Effect => "Effects",
            Role::Trigger => "Conditions",
            Role::Modifier => "Modifiers",
            Role::AiWeight => "AI weight",
            Role::Mtth => "Mean time to happen",
            Role::TreeCountry => "Countries",
            Role::Prerequisite => "Prerequisites",
            Role::Exclusive => "Mutually exclusive",
            Role::Politics => "Politics",
            Role::Popularities => "Popularities",
            Role::Technologies => "Technologies",
            Role::Ideas => "Ideas",
            Role::ResearchBonus => "Research bonus",
            Role::SearchFilters => "Search filters",
            Role::Regiments => "Battalions",
            Role::Support => "Support companies",
            Role::Values => "Form",
            Role::Words => "List",
            Role::Weights => "Weights",
            Role::Texts => "Texts",
            Role::Rules => "Rules",
            Role::Colour => "Colour",
            Role::EquipmentBonus => "Equipment stats",
        }
    }

    /// One of the three catalogs (the others have a page of their own).
    pub fn is_catalog(self) -> bool {
        matches!(self, Role::Effect | Role::Trigger | Role::Modifier)
    }

    /// What it builds replaces the field (else it is added to it).
    pub fn replaces(self) -> bool {
        matches!(self, Role::Prerequisite | Role::Exclusive | Role::Politics | Role::Popularities | Role::Technologies | Role::Ideas | Role::SearchFilters) || self.is_shape()
    }

    /// Built from a form of its own ([`super::shapes`]).
    pub fn is_shape(self) -> bool {
        matches!(self, Role::Values | Role::Words | Role::Weights | Role::Texts | Role::Rules | Role::Colour | Role::EquipmentBonus)
    }
}

/// The wizard of a field of a definition, if it has one.
pub fn role_of(kind: Kind, key: &str) -> Option<Role> {
    if let Some(s) = super::shapes::shape_of(kind, key) {
        return Some(s.role());
    }
    Some(match key {
        "ai_will_do" | "ai_chance" | "weight" | "chance" => Role::AiWeight,
        "mean_time_to_happen" => Role::Mtth,
        "country" if kind == Kind::FocusTree => Role::TreeCountry,
        "prerequisite" if matches!(kind, Kind::Focus | Kind::SharedFocus) => Role::Prerequisite,
        "mutually_exclusive" if matches!(kind, Kind::Focus | Kind::SharedFocus) => Role::Exclusive,
        "set_politics" => Role::Politics,
        "set_popularities" => Role::Popularities,
        "set_technology" => Role::Technologies,
        "add_ideas" if kind == Kind::CountryHistory => Role::Ideas,
        "research_bonus" => Role::ResearchBonus,
        "search_filters" => Role::SearchFilters,
        "regiments" if kind == Kind::DivisionTemplate => Role::Regiments,
        "support" if kind == Kind::DivisionTemplate => Role::Support,
        "modifier" | "modifiers" | "faction_modifiers" | "targeted_modifier" => Role::Modifier,
        "immediate" | "complete_effect" | "remove_effect" | "timeout_effect" | "on_add" | "on_remove" | "select_effect" | "completion_reward" | "effect" | "option" | "on_cancel" | "on_completion" | "complete_tooltip" => Role::Effect,
        "trigger" | "allowed" | "visible" | "available" | "cancel_trigger" | "target_trigger" | "cancel" | "enable" | "abort" | "remove_trigger" | "allow" | "allow_branch" | "bypass" | "allowed_civil_war" | "show_major" | "limit" | "can_be_removed" | "is_good" => Role::Trigger,
        _ => return None,
    })
}

/// The catalog a whole definition's code is about (the code tab's wizard).
pub fn role_of_kind(kind: Kind) -> Role {
    match kind {
        Kind::ScriptedTrigger => Role::Trigger,
        Kind::StaticModifier | Kind::DynamicModifier | Kind::CountryLeaderTrait | Kind::UnitLeaderTrait | Kind::ScientistTrait => Role::Modifier,
        _ => Role::Effect,
    }
}

// ---------------------------------------------------------------- catalog

/// What a part of an entry takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Param {
    Number,
    Word,
    /// Free text (quoted when it has to be).
    Text,
    Date,
    YesNo,
    /// A country: the map's nations, the base game's tags, ROOT, FROM, ...
    Tag,
    /// A state of the map.
    State,
    /// The id of a definition of a kind (the content's and the base game's).
    Ref(Kind),
    /// A localisation key.
    Loc,
    Building,
    Resource,
    TechCategory,
    WarGoal,
    /// A sprite (`GFX_...`): the content's and the base game's.
    Sprite,
    /// A leader's ideology: the sub-ideologies of the ideologies.
    SubIdeology,
    /// One of these.
    Of(&'static [&'static str]),
    /// The name of a file of a folder of the content, without its extension.
    File(&'static str),
    /// The id of a `block = { id = x }` in the files of a folder, the
    /// content's and the base game's (folder, block).
    IdIn(&'static str, &'static str),
}

/// What a value of a form's field is picked from (None: it is typed).
pub fn param_of(ty: super::kinds::FieldType) -> Option<Param> {
    use super::kinds::FieldType as F;
    match ty {
        F::Ref(k) => Some(Param::Ref(k)),
        F::Tag => Some(Param::Tag),
        F::State => Some(Param::State),
        F::File(folder) => Some(Param::File(folder)),
        F::IdIn(folder, block) => Some(Param::IdIn(folder, block)),
        _ => None,
    }
}

/// One entry of a catalog: `{0}`, `{1}`, ... in the template are the values.
#[derive(Clone, Copy, Debug)]
pub struct Item {
    pub group: &'static str,
    pub label: &'static str,
    /// The effect / trigger / modifier it is (its documentation).
    pub key: &'static str,
    pub params: &'static [(&'static str, Param, &'static str)],
    pub template: &'static str,
}

const fn it(group: &'static str, label: &'static str, key: &'static str, params: &'static [(&'static str, Param, &'static str)], template: &'static str) -> Item {
    Item { group, label, key, params, template }
}

use Param::*;

const AMOUNT: &[(&str, Param, &str)] = &[("Amount", Number, "100")];
const SHARE: &[(&str, Param, &str)] = &[("Amount (-1 to 1)", Number, "0.05")];
const XP: &[(&str, Param, &str)] = &[("Amount", Number, "25")];
const IDEA: &[(&str, Param, &str)] = &[("Idea", Ref(Kind::Idea), "")];
const TAG: &[(&str, Param, &str)] = &[("Country", Tag, "")];
const STATE: &[(&str, Param, &str)] = &[("State", State, "")];
const FLAG: &[(&str, Param, &str)] = &[("Flag", Word, "my_flag")];
const NONE: &[(&str, Param, &str)] = &[];

pub const EFFECTS: &[Item] = &[
    // politics
    it("Politics", "Political power", "add_political_power", AMOUNT, "add_political_power = {0}"),
    it("Politics", "Stability", "add_stability", SHARE, "add_stability = {0}"),
    it("Politics", "War support", "add_war_support", SHARE, "add_war_support = {0}"),
    it("Politics", "Command power", "add_command_power", &[("Amount", Number, "25")], "add_command_power = {0}"),
    it("Politics", "Popularity of an ideology", "add_popularity", &[("Ideology", Ref(Kind::Ideology), "fascism"), ("Amount (-1 to 1)", Number, "0.1")], "add_popularity = {\n\tideology = {0}\n\tpopularity = {1}\n}"),
    it("Politics", "Change the ruling party", "set_politics", &[("Ideology", Ref(Kind::Ideology), "fascism"), ("Elections allowed", YesNo, "no")], "set_politics = {\n\truling_party = {0}\n\telections_allowed = {1}\n}"),
    it("Politics", "Add a national spirit", "add_ideas", IDEA, "add_ideas = {0}"),
    it("Politics", "Remove a national spirit", "remove_ideas", IDEA, "remove_ideas = {0}"),
    it("Politics", "National spirit for some days", "add_timed_idea", &[("Idea", Ref(Kind::Idea), ""), ("Days", Number, "180")], "add_timed_idea = {\n\tidea = {0}\n\tdays = {1}\n}"),
    it("Politics", "Swap national spirits", "swap_ideas", &[("Remove", Ref(Kind::Idea), ""), ("Add", Ref(Kind::Idea), "")], "swap_ideas = {\n\tremove_idea = {0}\n\tadd_idea = {1}\n}"),
    it("Politics", "Recruit a character", "recruit_character", &[("Character", Ref(Kind::Character), "")], "recruit_character = {0}"),
    it("Politics", "Retire a character", "retire_character", &[("Character", Ref(Kind::Character), "")], "retire_character = {0}"),
    it("Politics", "Make a character the leader", "promote_character", &[("Character", Ref(Kind::Character), "")], "promote_character = {0}"),
    it("Politics", "Dynamic modifier", "add_dynamic_modifier", &[("Modifier", Ref(Kind::DynamicModifier), "")], "add_dynamic_modifier = { modifier = {0} }"),
    it("Politics", "Remove a dynamic modifier", "remove_dynamic_modifier", &[("Modifier", Ref(Kind::DynamicModifier), "")], "remove_dynamic_modifier = { modifier = {0} }"),
    it("Politics", "Cosmetic name and flag", "set_cosmetic_tag", &[("Cosmetic tag", Word, "")], "set_cosmetic_tag = {0}"),
    // military
    it("Military", "Army experience", "army_experience", XP, "army_experience = {0}"),
    it("Military", "Navy experience", "navy_experience", XP, "navy_experience = {0}"),
    it("Military", "Air experience", "air_experience", XP, "air_experience = {0}"),
    it("Military", "Manpower", "add_manpower", &[("Amount", Number, "10000")], "add_manpower = {0}"),
    it("Military", "Equipment", "add_equipment_to_stockpile", &[("Equipment", Ref(Kind::Equipment), "infantry_equipment_1"), ("Amount", Number, "1000")], "add_equipment_to_stockpile = {\n\ttype = {0}\n\tamount = {1}\n}"),
    it("Military", "Load an order of battle", "load_oob", &[("File (history/units)", Text, "")], "load_oob = {0}"),
    it("Military", "Declare war", "declare_war_on", &[("Country", Tag, ""), ("War goal", WarGoal, "annex_everything")], "declare_war_on = {\n\ttarget = {0}\n\ttype = {1}\n}"),
    it("Military", "White peace", "white_peace", TAG, "white_peace = {0}"),
    // research and industry
    it("Research and industry", "Research slot", "add_research_slot", &[("Slots", Number, "1")], "add_research_slot = {0}"),
    it("Research and industry", "Research bonus", "add_tech_bonus", &[("Bonus (0 to 1)", Number, "0.5"), ("Uses", Number, "1"), ("Category", TechCategory, "infantry_weapons")], "add_tech_bonus = {\n\tbonus = {0}\n\tuses = {1}\n\tcategory = {2}\n}"),
    it("Research and industry", "A technology", "set_technology", &[("Technology", Ref(Kind::Technology), "")], "set_technology = { {0} = 1 }"),
    it("Research and industry", "Buildings in a state", "add_building_construction", &[("State", State, ""), ("Building", Building, "industrial_complex"), ("Levels", Number, "1")], "{0} = {\n\tadd_building_construction = {\n\t\ttype = {1}\n\t\tlevel = {2}\n\t\tinstant_build = yes\n\t}\n}"),
    it("Research and industry", "Building slots in a state", "add_extra_state_shared_building_slots", &[("State", State, ""), ("Slots", Number, "1")], "{0} = { add_extra_state_shared_building_slots = {1} }"),
    it("Research and industry", "Factories in the capital (offsite)", "add_offsite_building", &[("Building", Building, "industrial_complex"), ("Levels", Number, "1")], "add_offsite_building = {\n\ttype = {0}\n\tlevel = {1}\n}"),
    it("Research and industry", "Resources of a state", "add_resource", &[("State", State, ""), ("Resource", Resource, "steel"), ("Amount", Number, "8")], "{0} = {\n\tadd_resource = {\n\t\ttype = {1}\n\t\tamount = {2}\n\t}\n}"),
    // states
    it("States", "Core on a state", "add_state_core", STATE, "add_state_core = {0}"),
    it("States", "Remove a core", "remove_state_core", STATE, "remove_state_core = {0}"),
    it("States", "Claim on a state", "add_state_claim", STATE, "add_state_claim = {0}"),
    it("States", "Take a state", "transfer_state", STATE, "transfer_state = {0}"),
    it("States", "Move the capital", "set_capital", STATE, "set_capital = { state = {0} }"),
    // diplomacy
    it("Diplomacy", "Annex a country", "annex_country", TAG, "annex_country = {\n\ttarget = {0}\n\ttransfer_troops = yes\n}"),
    it("Diplomacy", "Make a puppet", "puppet", TAG, "puppet = {0}"),
    it("Diplomacy", "Release a country", "release", TAG, "release = {0}"),
    it("Diplomacy", "Guarantee", "give_guarantee", TAG, "give_guarantee = {0}"),
    it("Diplomacy", "Military access", "give_military_access", TAG, "give_military_access = {0}"),
    it("Diplomacy", "Non-aggression pact", "diplomatic_relation", TAG, "diplomatic_relation = {\n\tcountry = {0}\n\trelation = non_aggression_pact\n\tactive = yes\n}"),
    it("Diplomacy", "Opinion of a country", "add_opinion_modifier", &[("Country", Tag, ""), ("Opinion modifier", Ref(Kind::OpinionModifier), "")], "add_opinion_modifier = {\n\ttarget = {0}\n\tmodifier = {1}\n}"),
    it("Diplomacy", "Create a faction", "create_faction", &[("Name (localisation key)", Loc, "")], "create_faction = {0}"),
    it("Diplomacy", "Add to the faction", "add_to_faction", TAG, "add_to_faction = {0}"),
    it("Diplomacy", "Leave the faction", "leave_faction", NONE, "leave_faction = yes"),
    // events and flow
    it("Events and flags", "Fire an event", "country_event", &[("Event", Ref(Kind::Event), ""), ("Days later", Number, "0")], "country_event = {\n\tid = {0}\n\tdays = {1}\n}"),
    it("Events and flags", "Fire a news event", "news_event", &[("Event", Ref(Kind::Event), "")], "news_event = { id = {0} }"),
    it("Events and flags", "An event for another country", "country_event", &[("Country", Tag, ""), ("Event", Ref(Kind::Event), "")], "{0} = { country_event = {1} }"),
    it("Events and flags", "Set a country flag", "set_country_flag", FLAG, "set_country_flag = {0}"),
    it("Events and flags", "Clear a country flag", "clr_country_flag", FLAG, "clr_country_flag = {0}"),
    it("Events and flags", "Set a global flag", "set_global_flag", FLAG, "set_global_flag = {0}"),
    it("Events and flags", "Set a variable", "set_variable", &[("Variable", Word, "my_var"), ("Value", Number, "1")], "set_variable = { {0} = {1} }"),
    it("Events and flags", "Add to a variable", "add_to_variable", &[("Variable", Word, "my_var"), ("Value", Number, "1")], "add_to_variable = { {0} = {1} }"),
    it("Events and flags", "A tooltip", "custom_effect_tooltip", &[("Text (localisation key)", Loc, "")], "custom_effect_tooltip = {0}"),
    it("Events and flags", "Show a decision as unlocked", "unlock_decision_tooltip", &[("Decision", Ref(Kind::Decision), "")], "unlock_decision_tooltip = {0}"),
    it("Events and flags", "Start a mission", "activate_mission", &[("Mission (decision)", Ref(Kind::Decision), "")], "activate_mission = {0}"),
    it("Events and flags", "Change the focus tree", "load_focus_tree", &[("Focus tree", Ref(Kind::FocusTree), "")], "load_focus_tree = {\n\ttree = {0}\n\tkeep_completed = yes\n}"),
    it("Events and flags", "Complete a focus", "complete_national_focus", &[("Focus", Ref(Kind::Focus), "")], "complete_national_focus = {0}"),
    it("Events and flags", "Run a scripted effect", "", &[("Scripted effect", Ref(Kind::ScriptedEffect), "")], "{0} = yes"),
];

pub const TRIGGERS: &[Item] = &[
    // the country
    it("The country", "Is this country", "tag", TAG, "tag = {0}"),
    it("The country", "Was this country at the start", "original_tag", TAG, "original_tag = {0}"),
    it("The country", "A country exists", "country_exists", TAG, "country_exists = {0}"),
    it("The country", "Has a national spirit", "has_idea", IDEA, "has_idea = {0}"),
    it("The country", "Has completed a focus", "has_completed_focus", &[("Focus", Ref(Kind::Focus), "")], "has_completed_focus = {0}"),
    it("The country", "Has a country flag", "has_country_flag", FLAG, "has_country_flag = {0}"),
    it("The country", "Has a global flag", "has_global_flag", FLAG, "has_global_flag = {0}"),
    it("The country", "Ruling party is", "has_government", &[("Ideology", Ref(Kind::Ideology), "fascism")], "has_government = {0}"),
    it("The country", "Stability at least", "has_stability", &[("Share (0 to 1)", Number, "0.5")], "has_stability > {0}"),
    it("The country", "War support at least", "has_war_support", &[("Share (0 to 1)", Number, "0.5")], "has_war_support > {0}"),
    it("The country", "Political power at least", "has_political_power", AMOUNT, "has_political_power > {0}"),
    it("The country", "Factories at least", "num_of_factories", &[("Factories", Number, "20")], "num_of_factories > {0}"),
    it("The country", "Has researched", "has_tech", &[("Technology", Ref(Kind::Technology), "")], "has_tech = {0}"),
    it("The country", "Has a character", "has_character", &[("Character", Ref(Kind::Character), "")], "has_character = {0}"),
    it("The country", "Is a major", "is_major", NONE, "is_major = yes"),
    it("The country", "Is played by the AI", "is_ai", NONE, "is_ai = yes"),
    it("The country", "Has a DLC", "has_dlc", &[("DLC name", Text, "La Resistance")], "has_dlc = {0}"),
    it("The country", "A variable at least", "check_variable", &[("Variable", Word, "my_var"), ("Value", Number, "1")], "check_variable = { {0} > {1} }"),
    // war and diplomacy
    it("War and diplomacy", "Is at war", "has_war", NONE, "has_war = yes"),
    it("War and diplomacy", "Is at war with", "has_war_with", TAG, "has_war_with = {0}"),
    it("War and diplomacy", "Is in a faction", "is_in_faction", NONE, "is_in_faction = yes"),
    it("War and diplomacy", "Is in a faction with", "is_in_faction_with", TAG, "is_in_faction_with = {0}"),
    it("War and diplomacy", "Leads a faction", "is_faction_leader", NONE, "is_faction_leader = yes"),
    it("War and diplomacy", "Is a subject", "is_subject", NONE, "is_subject = yes"),
    it("War and diplomacy", "Is a subject of", "is_subject_of", TAG, "is_subject_of = {0}"),
    it("War and diplomacy", "Is guaranteed by", "is_guaranteed_by", TAG, "is_guaranteed_by = {0}"),
    it("War and diplomacy", "Opinion of a country at least", "has_opinion", &[("Country", Tag, ""), ("Opinion", Number, "50")], "has_opinion = {\n\ttarget = {0}\n\tvalue > {1}\n}"),
    it("War and diplomacy", "Has capitulated", "has_capitulated", NONE, "has_capitulated = yes"),
    it("War and diplomacy", "Surrender progress at least", "surrender_progress", &[("Share (0 to 1)", Number, "0.5")], "surrender_progress > {0}"),
    it("War and diplomacy", "World tension at least", "threat", &[("Share (0 to 1)", Number, "0.5")], "threat > {0}"),
    // states
    it("States", "Owns a state", "owns_state", STATE, "owns_state = {0}"),
    it("States", "Controls a state", "controls_state", STATE, "controls_state = {0}"),
    it("States", "Has a core on a state", "is_core_of", STATE, "{0} = { is_core_of = ROOT }"),
    // time and logic
    it("Time and logic", "After a date", "date", &[("Date", Date, "1936.6.1")], "date > {0}"),
    it("Time and logic", "Always / never", "always", &[("Always", YesNo, "yes")], "always = {0}"),
    it("Time and logic", "A decision was taken", "has_decision", &[("Decision", Ref(Kind::Decision), "")], "has_decision = {0}"),
    it("Time and logic", "Run a scripted trigger", "", &[("Scripted trigger", Ref(Kind::ScriptedTrigger), "")], "{0} = yes"),
];

/// Modifiers used most, by what they are about (the documentation has the rest).
pub const MODIFIERS: &[(&str, &[&str])] = &[
    ("Politics", &["political_power_gain", "political_power_factor", "stability_factor", "war_support_factor", "drift_defence_factor", "communism_drift", "democratic_drift", "fascism_drift", "neutrality_drift", "command_power_gain", "max_command_power"]),
    ("Industry", &["production_speed_buildings_factor", "industrial_capacity_factory", "industrial_capacity_dockyard", "consumer_goods_factor", "production_factory_max_efficiency_factor", "production_factory_efficiency_gain_factor", "local_resources_factor", "research_speed_factor"]),
    ("Army", &["conscription", "conscription_factor", "army_attack_factor", "army_defence_factor", "army_org_factor", "training_time_army_factor", "experience_gain_army", "land_reinforce_rate"]),
    ("Navy and air", &["navy_org_factor", "naval_speed_factor", "air_superiority_efficiency", "experience_gain_navy", "experience_gain_air"]),
    ("Diplomacy", &["justify_war_goal_time", "generate_wargoal_tension", "opinion_gain_monthly_factor", "faction_trade_opinion_factor"]),
];

pub const BUILDINGS: &[&str] = &[
    "industrial_complex", "arms_factory", "dockyard", "infrastructure", "air_base", "naval_base", "bunker", "coastal_bunker", "anti_air_building", "radar_station", "synthetic_refinery", "fuel_silo", "rocket_site", "nuclear_reactor", "supply_node", "rail_way",
];
pub const RESOURCES: &[&str] = &["oil", "aluminium", "rubber", "tungsten", "steel", "chromium"];
pub const TECH_CATEGORIES: &[&str] = &[
    "infantry_weapons", "infantry_tech", "support_tech", "artillery", "armor", "motorized_equipment", "cat_mechanized_equipment", "night_vision", "land_doctrine", "naval_doctrine", "air_doctrine", "electronics", "computing_tech", "radar_tech", "encryption_tech", "decryption_tech", "industry", "construction_tech", "production_tech", "synth_resources", "excavation_tech", "rocketry", "jet_engine", "nuclear", "naval_equipment", "dd_tech", "cl_tech", "ca_tech", "bb_tech", "cv_tech", "ss_tech", "air_equipment", "light_fighter", "cas_bomber", "nav_bomber", "tactical_bomber", "cat_heavy_fighter", "cat_strategic_bomber", "medium_air", "naval_air", "mountaineers_tech", "marine_tech", "para_tech", "train_tech",
];
pub const WAR_GOALS: &[&str] = &["annex_everything", "take_state", "take_state_focus", "puppet", "topple_government", "independence_war", "civil_war", "liberate", "humiliate"];
pub const SEARCH_FILTERS: &[&str] = &[
    "FOCUS_FILTER_POLITICAL", "FOCUS_FILTER_RESEARCH", "FOCUS_FILTER_INDUSTRY", "FOCUS_FILTER_STABILITY", "FOCUS_FILTER_WAR_SUPPORT", "FOCUS_FILTER_MANPOWER", "FOCUS_FILTER_ANNEXATION", "FOCUS_FILTER_HISTORICAL", "FOCUS_FILTER_ARMY_XP", "FOCUS_FILTER_NAVY_XP", "FOCUS_FILTER_AIR_XP", "FOCUS_FILTER_MILITARY_CHARACTER", "FOCUS_FILTER_POLITICAL_CHARACTER", "FOCUS_FILTER_INTERNAL_AFFAIRS", "FOCUS_FILTER_TFV_AUTONOMY", "FOCUS_FILTER_INTERNATIONAL_TRADE",
];
pub const TARGETS: &[&str] = &["ROOT", "FROM", "PREV", "THIS", "OWNER", "CONTROLLER"];

/// The script of an entry with its values (`{0}`, `{1}`, ...), each written as
/// its kind of value needs (text quoted, the rest as words).
pub fn render(item: &Item, values: &[String]) -> String {
    let mut out = item.template.to_string();
    for (i, (_, p, _)) in item.params.iter().enumerate() {
        let v = values.get(i).map(|s| s.trim()).unwrap_or("");
        let v = match p {
            Text => script::quote(v),
            Number | YesNo | Date => v.to_string(),
            _ => script::word(v),
        };
        out = out.replace(&format!("{{{i}}}"), &v);
    }
    out
}

/// Every value an entry needs is there, and numbers are numbers.
pub fn missing(item: &Item, values: &[String]) -> Option<String> {
    for (i, (label, p, _)) in item.params.iter().enumerate() {
        let v = values.get(i).map(|s| s.trim()).unwrap_or("");
        if v.is_empty() && *p != Text {
            return Some(format!("{label} is empty"));
        }
        if *p == Number && v.parse::<f64>().is_err() {
            return Some(format!("{label} is not a number"));
        }
        if *p == Date && !is_date(v) {
            return Some(format!("{label}: a date is year.month.day (1936.1.1)"));
        }
    }
    None
}

pub fn is_date(v: &str) -> bool {
    let p: Vec<&str> = v.split('.').collect();
    (2..=4).contains(&p.len()) && p.iter().all(|x| !x.is_empty() && x.bytes().all(|c| c.is_ascii_digit()))
}

/// A modifier's name in the game (`MODIFIER_X` in its localisation), else the id made readable.
pub fn modifier_label(name: &str, loc: &dyn Fn(&str) -> Option<String>) -> String {
    loc(&format!("MODIFIER_{}", name.to_ascii_uppercase())).or_else(|| loc(name)).unwrap_or_else(|| super::loc::prettify(name))
}

// ---------------------------------------------------------------- building up script

/// `lines` inside `key = { ... }`.
pub fn wrap(key: &str, lines: &str) -> String {
    let body = lines.trim();
    if body.is_empty() {
        return format!("{key} = {{ }}");
    }
    if !body.contains('\n') && body.len() < 50 {
        return format!("{key} = {{ {body} }}");
    }
    format!("{key} = {{\n{}\n}}", indent(body))
}

/// Every line one tab further in.
pub fn indent(text: &str) -> String {
    text.lines().map(|l| if l.trim().is_empty() { String::new() } else { format!("\t{l}") }).collect::<Vec<_>>().join("\n")
}

/// `more` added to `text` on lines of its own.
pub fn append(text: &str, more: &str) -> String {
    let t = text.trim_end();
    let m = more.trim();
    if t.is_empty() {
        m.to_string()
    } else if m.is_empty() {
        t.to_string()
    } else {
        format!("{t}\n{m}")
    }
}

/// `snippet` put into `text` after the line the cursor is on (a char
/// index), indented like that line (one more when it opens a block); at the
/// end without a cursor.
pub fn insert_at(text: &str, cursor: Option<usize>, snippet: &str) -> String {
    let snippet = snippet.trim();
    let Some(c) = cursor else { return append(text, snippet) };
    let byte = text.char_indices().nth(c).map(|(b, _)| b).unwrap_or(text.len());
    let start = text[..byte].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let end = text[byte..].find('\n').map(|i| byte + i).unwrap_or(text.len());
    let line = &text[start..end];
    let mut ind: String = line.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
    let code = line.split('#').next().unwrap_or("").trim_end();
    if code.ends_with('{') {
        ind.push('\t');
    } else if code.trim_start().starts_with('}') && !ind.is_empty() && line.trim().is_empty() {
        ind.pop();
    }
    let body = snippet.lines().map(|l| if l.trim().is_empty() { String::new() } else { format!("{ind}{l}") }).collect::<Vec<_>>().join("\n");
    format!("{}\n{}{}", &text[..end], body, &text[end..])
}

/// `snippet` put before the brace that closes a whole definition (the code
/// tab without a cursor).
pub fn insert_before_close(text: &str, snippet: &str) -> String {
    let Some(close) = text.rfind('}') else { return append(text, snippet) };
    let start = text[..close].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let outer: String = text[start..close].chars().take_while(|c| c.is_whitespace()).collect();
    let ind = format!("{outer}\t");
    let body = snippet.trim().lines().map(|l| if l.trim().is_empty() { String::new() } else { format!("{ind}{l}") }).collect::<Vec<_>>().join("\n");
    if text[start..close].trim().is_empty() {
        format!("{}{}\n{}", &text[..start], body, &text[start..])
    } else {
        // `x = { a = 1 }` on one line
        format!("{}\n{}\n{}{}", text[..close].trim_end(), body, outer, &text[close..])
    }
}

/// Keys a block has once (a base factor, a time): set where the field has
/// them already, instead of added a second time.
const SINGLE: &[&str] = &["factor", "base", "add", "days", "months", "years", "ruling_party", "last_election", "election_frequency", "elections_allowed"];

/// What a wizard built, merged into the inner text of a field: keys the
/// field has once take the new value; the rest is added at the cursor (or
/// at the end).
pub fn merge(inner: &str, built: &str, cursor: Option<usize>) -> String {
    let field = Script::parse(inner);
    let new = Script::parse(built);
    if !field.errors.is_empty() || !new.errors.is_empty() {
        return insert_at(inner, cursor, built);
    }
    let mut out = inner.to_string();
    let mut rest: Vec<String> = Vec::new();
    // edits from the end, so the spans before stay right
    let mut edits: Vec<(std::ops::Range<usize>, String)> = Vec::new();
    for item in &new.root.items {
        let key = item.key.as_ref().map(|k| k.text.as_str()).unwrap_or("");
        let value = item.atom().map(|a| a.text.clone());
        if let (true, Some(v)) = (SINGLE.contains(&key), value)
            && let Some(old) = field.root.get(key).and_then(|i| i.atom())
        {
            edits.push((old.span.clone(), script::word(&v)));
            continue;
        }
        rest.push(script::item_source(built, item));
    }
    edits.sort_by_key(|e| std::cmp::Reverse(e.0.start));
    let moved = edits.iter().any(|e| cursor.is_some_and(|c| e.0.start < out.char_indices().nth(c).map(|x| x.0).unwrap_or(usize::MAX)));
    for (r, t) in edits {
        out.replace_range(r, &t);
    }
    if rest.is_empty() {
        return out;
    }
    // a cursor before a changed value may be off by a few characters: then at the end
    insert_at(&out, if moved { None } else { cursor }, &rest.join("\n"))
}

/// The battalions of a division template block: (type, x, y).
pub fn slots(inner: &str) -> Vec<(String, i64, i64)> {
    let s = Script::parse(inner);
    s.root
        .items
        .iter()
        .filter_map(|i| {
            let b = i.block()?;
            let k = i.key.as_ref()?.text.clone();
            Some((k, b.atom("x")?.parse().ok()?, b.atom("y")?.parse().ok()?))
        })
        .collect()
}

/// `n` battalions of `kind` put into free places of a template: a column
/// that has that kind already, else the first empty one; down the rows.
/// Support companies are one column (`support`).
pub fn place(inner: &str, kind: &str, n: usize, support: bool) -> Result<String, String> {
    let taken = slots(inner);
    let free = |x: i64, y: i64, more: &[(i64, i64)]| !taken.iter().any(|t| t.1 == x && t.2 == y) && !more.contains(&(x, y));
    let mut placed: Vec<(i64, i64)> = Vec::new();
    let cols: Vec<i64> = if support {
        vec![0]
    } else {
        let mut c: Vec<i64> = taken.iter().filter(|t| t.0 == kind).map(|t| t.1).collect();
        c.sort();
        c.dedup();
        c.extend((0..5).filter(|x| !taken.iter().any(|t| t.1 == *x)));
        c.extend(0..5);
        c
    };
    'outer: for x in cols {
        for y in 0..5 {
            if placed.len() == n {
                break 'outer;
            }
            if free(x, y, &placed) {
                placed.push((x, y));
            }
        }
    }
    if placed.len() < n {
        return Err(format!("only {} free places for {kind} (a template has 5 × 5, support 5)", placed.len()));
    }
    Ok(placed.iter().map(|(x, y)| format!("{kind} = {{ x = {x} y = {y} }}")).collect::<Vec<_>>().join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles() {
        assert_eq!(role_of(Kind::Focus, "completion_reward"), Some(Role::Effect));
        assert_eq!(role_of(Kind::Focus, "available"), Some(Role::Trigger));
        assert_eq!(role_of(Kind::Idea, "modifier"), Some(Role::Modifier));
        assert_eq!(role_of(Kind::Decision, "ai_will_do"), Some(Role::AiWeight));
        assert_eq!(role_of(Kind::Focus, "prerequisite"), Some(Role::Prerequisite));
        assert_eq!(role_of(Kind::FocusTree, "country"), Some(Role::TreeCountry));
        assert_eq!(role_of(Kind::Event, "country"), None);
        assert_eq!(role_of(Kind::Character, "portraits"), Some(Role::Values));
        assert_eq!(role_of(Kind::Focus, "complete_tooltip"), Some(Role::Effect));
        assert!(Role::Words.replaces() && Role::Words.is_shape() && !Role::Words.is_catalog());
        assert!(Role::Politics.replaces() && !Role::Effect.replaces());
    }

    #[test]
    fn renders_values() {
        let i = EFFECTS.iter().find(|i| i.key == "add_timed_idea").unwrap();
        assert_eq!(render(i, &["TYR_spirit".into(), "180".into()]), "add_timed_idea = {\n\tidea = TYR_spirit\n\tdays = 180\n}");
        let l = EFFECTS.iter().find(|i| i.key == "load_oob").unwrap();
        assert_eq!(render(l, &["TYR_1936".into()]), "load_oob = \"TYR_1936\"");
        assert_eq!(missing(i, &["x".into(), "soon".into()]).as_deref(), Some("Days is not a number"));
        assert_eq!(missing(i, &["".into(), "1".into()]).as_deref(), Some("Idea is empty"));
        let d = TRIGGERS.iter().find(|i| i.key == "date").unwrap();
        assert!(missing(d, &["1936.1.1".into()]).is_none() && missing(d, &["1936".into()]).is_some());
        // every template uses its values
        for i in EFFECTS.iter().chain(TRIGGERS) {
            for n in 0..i.params.len() {
                assert!(i.template.contains(&format!("{{{n}}}")), "{}", i.label);
            }
            let v: Vec<String> = i.params.iter().map(|p| if p.1 == Number { "1".into() } else if p.1 == Date { "1936.1.1".into() } else { "x".into() }).collect();
            assert!(Script::parse(&render(i, &v)).errors.is_empty(), "{}", i.label);
        }
    }

    #[test]
    fn wraps_and_inserts() {
        assert_eq!(wrap("NOT", "tag = GER"), "NOT = { tag = GER }");
        assert_eq!(wrap("OR", "tag = GER\ntag = ITA\nhas_war = yes\nis_major = yes"), "OR = {\n\ttag = GER\n\ttag = ITA\n\thas_war = yes\n\tis_major = yes\n}");
        assert_eq!(append("a = 1\n", "b = 2"), "a = 1\nb = 2");
        let t = "a = 1\nif = {\n\tlimit = { x = 1 }\n}\nc = 3";
        // cursor on `if = {`: inside the block
        assert_eq!(insert_at(t, Some(7), "b = 2"), "a = 1\nif = {\n\tb = 2\n\tlimit = { x = 1 }\n}\nc = 3");
        assert_eq!(insert_at(t, Some(0), "b = {\n\tz = 1\n}"), "a = 1\nb = {\n\tz = 1\n}\nif = {\n\tlimit = { x = 1 }\n}\nc = 3");
        assert_eq!(insert_at(t, None, "d = 4"), format!("{t}\nd = 4"));
        assert_eq!(insert_before_close("id = {\n\ta = 1\n}", "b = 2"), "id = {\n\ta = 1\n\tb = 2\n}");
        assert_eq!(insert_before_close("id = { a = 1 }", "b = 2"), "id = { a = 1\n\tb = 2\n}");
    }

    #[test]
    fn merges_single_keys() {
        let inner = "factor = 1\nmodifier = {\n\tfactor = 0\n\thas_war = yes\n}";
        let m = merge(inner, "factor = 10\nmodifier = {\n\tfactor = 2\n\ttag = GER\n}", None);
        assert_eq!(m, "factor = 10\nmodifier = {\n\tfactor = 0\n\thas_war = yes\n}\nmodifier = {\n\tfactor = 2\n\ttag = GER\n}");
        assert_eq!(merge("", "days = 30", None), "days = 30");
        assert_eq!(merge("days = 30", "months = 2", None), "days = 30\nmonths = 2");
    }

    #[test]
    fn places_battalions() {
        let inner = "infantry = { x = 0 y = 0 }\ninfantry = { x = 0 y = 1 }\nartillery_brigade = { x = 1 y = 0 }";
        assert_eq!(place(inner, "infantry", 2, false).unwrap(), "infantry = { x = 0 y = 2 }\ninfantry = { x = 0 y = 3 }");
        assert_eq!(place(inner, "light_armor", 1, false).unwrap(), "light_armor = { x = 2 y = 0 }");
        assert_eq!(place("", "engineer", 2, true).unwrap(), "engineer = { x = 0 y = 0 }\nengineer = { x = 0 y = 1 }");
        assert!(place("", "engineer", 6, true).is_err());
        // a full column of its kind goes on in the next free one
        let full = (0..5).map(|y| format!("infantry = {{ x = 0 y = {y} }}")).collect::<Vec<_>>().join("\n");
        assert_eq!(place(&full, "infantry", 1, false).unwrap(), "infantry = { x = 1 y = 0 }");
    }

    #[test]
    #[ignore]
    fn the_catalog_is_in_the_games_documentation() {
        let d = super::super::docs::Docs::load(&crate::modbuild::default_game_dir());
        for i in EFFECTS.iter().filter(|i| !i.key.is_empty()) {
            assert!(d.effect(i.key).is_some(), "effect {} ({})", i.key, i.label);
        }
        for i in TRIGGERS.iter().filter(|i| !i.key.is_empty()) {
            assert!(d.trigger(i.key).is_some(), "trigger {} ({})", i.key, i.label);
        }
        // `<ideology>_drift` comes from the ideologies, not the documentation
        let missing: Vec<&str> = MODIFIERS.iter().flat_map(|g| g.1.iter().copied()).filter(|m| d.modifier(m).is_none() && !m.ends_with("_drift")).collect();
        assert!(missing.is_empty(), "modifiers not in the documentation: {missing:?}");
    }
}
