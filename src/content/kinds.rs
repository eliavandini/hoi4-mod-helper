//! The kinds of content a nation (or the mod) has, and where the game reads
//! them: folders, how a definition sits in a file, the fields the editor
//! shows for it, its localisation keys and a template for a new one.
//!
//! [`defs_of`] finds the definitions of every kind in a parsed file.

use serde::{Deserialize, Serialize};

use super::script::{Block, Item, Script};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Kind {
    FocusTree,
    Focus,
    SharedFocus,
    Event,
    Decision,
    DecisionCategory,
    Idea,
    Character,
    CountryHistory,
    DivisionTemplate,
    CountryLeaderTrait,
    UnitLeaderTrait,
    ScientistTrait,
    OpinionModifier,
    ScriptedEffect,
    ScriptedTrigger,
    ScriptedLoc,
    DynamicModifier,
    StaticModifier,
    OnAction,
    AiStrategy,
    AiStrategyPlan,
    WarGoal,
    Ideology,
    Technology,
    Equipment,
    SubUnit,
    Bookmark,
    PeaceAi,
    PeaceCost,
    AgencyUpgrade,
    Autonomy,
    Song,
    Names,
    Sprite,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entry {
    /// Each child of the block is a definition, its key the id.
    Keyed,
    /// Items with one of these keys are definitions; the id is in the field.
    Field(&'static [&'static str], &'static str),
    /// The whole file is one definition (country history: id = tag).
    File,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldType {
    Word,
    Text,
    Number,
    Bool,
    Block,
    /// A sprite name (`GFX_...`); `prefix` is put in front of a bare name
    /// (ideas: `picture = x` is `GFX_idea_x`).
    Sprite(&'static str),
    /// The id of a definition of this kind.
    Ref(Kind),
    /// A localisation key.
    Loc,
    /// A country tag (`TYR`).
    Tag,
    /// A state of the map (its number).
    State,
    /// The name of a file of this folder without its extension (`oob`:
    /// `history/units`).
    File(&'static str),
    /// The id of a `block = { id = x }` in the files of a folder, the
    /// content's and the base game's (folder, block).
    IdIn(&'static str, &'static str),
}

#[derive(Clone, Copy, Debug)]
pub struct Field {
    pub key: &'static str,
    pub ty: FieldType,
    /// May appear more than once (`option`, `prerequisite`).
    pub repeat: bool,
    pub help: &'static str,
}

const fn f(key: &'static str, ty: FieldType, help: &'static str) -> Field {
    Field { key, ty, repeat: false, help }
}
const fn rep(key: &'static str, ty: FieldType, help: &'static str) -> Field {
    Field { key, ty, repeat: true, help }
}

use FieldType::*;

pub struct Spec {
    pub kind: Kind,
    /// "Events"
    pub label: &'static str,
    /// "event"
    pub one: &'static str,
    /// Folders the game reads (relative to the mod); the first is where new files go.
    pub folders: &'static [&'static str],
    /// File extension.
    pub ext: &'static str,
    /// Blocks to go into from the root ("*": any key, which names the group).
    pub path: &'static [&'static str],
    pub entry: Entry,
    pub fields: &'static [Field],
    /// Localisation keys of a definition ("{id}", "{id}_desc") and what they are.
    pub loc: &'static [(&'static str, &'static str)],
    /// Ids are global (the game keeps one definition of an id).
    pub unique: bool,
    /// Kinds that are usually one nation's (else mostly the mod's).
    pub national: bool,
    pub help: &'static str,
}

const NAME_DESC: &[(&str, &str)] = &[("{id}", "Name"), ("{id}_desc", "Description")];

pub const SPECS: &[Spec] = &[
    Spec {
        kind: Kind::FocusTree,
        label: "Focus trees",
        one: "focus tree",
        folders: &["common/national_focus"],
        ext: "txt",
        path: &[],
        entry: Entry::Field(&["focus_tree"], "id"),
        fields: &[
            f("country", Block, "Which countries get this tree: a factor with modifiers (add = 10 tag = TYR)"),
            f("default", Bool, "The tree for countries no other tree fits"),
            f("reset_on_civilwar", Bool, "The tree starts over when a civil war breaks out"),
            rep("shared_focus", Ref(Kind::SharedFocus), "Shared focuses this tree includes (by their first focus)"),
            f("continuous_focus_position", Block, "Where the continuous focuses panel is: { x = 50 y = 1200 }"),
            f("initial_show_position", Block, "Where the view starts: { focus = X } or { x = 0 y = 0 }"),
        ],
        loc: &[],
        unique: true,
        national: true,
        help: "A national focus tree; its focuses are edited on the tree view.",
    },
    Spec {
        kind: Kind::Focus,
        label: "Focuses",
        one: "focus",
        folders: &["common/national_focus"],
        ext: "txt",
        path: &["focus_tree"],
        entry: Entry::Field(&["focus"], "id"),
        fields: FOCUS_FIELDS,
        loc: NAME_DESC,
        unique: true,
        national: true,
        help: "A focus of a tree.",
    },
    Spec {
        kind: Kind::SharedFocus,
        label: "Shared focuses",
        one: "shared focus",
        folders: &["common/national_focus"],
        ext: "txt",
        path: &[],
        entry: Entry::Field(&["shared_focus"], "id"),
        fields: FOCUS_FIELDS,
        loc: NAME_DESC,
        unique: true,
        national: false,
        help: "A focus several trees include (shared_focus = id in the tree).",
    },
    Spec {
        kind: Kind::Event,
        label: "Events",
        one: "event",
        folders: &["events"],
        ext: "txt",
        path: &[],
        entry: Entry::Field(&["country_event", "news_event", "state_event", "unit_leader_event", "operative_leader_event"], "id"),
        fields: &[
            rep("title", Loc, "Localisation key of the title (or a block with trigger and text)"),
            rep("desc", Loc, "Localisation key of the text (several, each with a trigger, pick one)"),
            f("picture", Sprite(""), "Event picture, GFX_report_event_..."),
            f("is_triggered_only", Bool, "Fires only when something fires it (a focus, a decision, another event)"),
            f("fire_only_once", Bool, "Fires at most once per game"),
            f("hidden", Bool, "Not shown: the immediate block and the first option run"),
            f("major", Bool, "A news event everyone sees"),
            f("show_major", Block, "Who sees a major event as major"),
            f("trigger", Block, "Conditions; with mean_time_to_happen it fires by itself"),
            f("mean_time_to_happen", Block, "{ days = 30 } how soon it fires once the trigger holds"),
            f("immediate", Block, "Effects run when the event fires"),
            rep("option", Block, "A choice: name = key, trigger, ai_chance, then effects"),
            f("timeout_days", Number, "Days before the AI picks for the player"),
        ],
        loc: &[],
        unique: true,
        national: true,
        help: "An event (country, news, state...). The id is namespace.number; the namespace needs add_namespace in the file.",
    },
    Spec {
        kind: Kind::Decision,
        label: "Decisions",
        one: "decision",
        folders: &["common/decisions"],
        ext: "txt",
        path: &["*"],
        entry: Entry::Keyed,
        fields: &[
            f("icon", Sprite("GFX_decision_"), "Icon: GFX_decision_<icon> (or a GFX_ name)"),
            f("cost", Number, "Political power cost"),
            f("fire_only_once", Bool, "Can be taken once"),
            f("days_re_enable", Number, "Days before it can be taken again"),
            f("days_remove", Number, "Days it runs before remove_effect (a mission or a timed decision)"),
            f("allowed", Block, "Checked once at game start: whether the country has it at all"),
            f("visible", Block, "When it is shown"),
            f("available", Block, "When it can be taken"),
            f("complete_effect", Block, "Effects when taken"),
            f("remove_effect", Block, "Effects when days_remove are over"),
            f("timeout_effect", Block, "Missions: effects when the time runs out"),
            f("cancel_trigger", Block, "Cancels it while running"),
            f("target_trigger", Block, "Targeted decisions: which targets"),
            f("targets", Block, "Targeted decisions: the candidate targets"),
            f("ai_will_do", Block, "{ factor = 1 } how much the AI wants it"),
        ],
        loc: NAME_DESC,
        unique: true,
        national: true,
        help: "A decision, in a category (the block around it).",
    },
    Spec {
        kind: Kind::DecisionCategory,
        label: "Decision categories",
        one: "decision category",
        folders: &["common/decisions/categories"],
        ext: "txt",
        path: &[],
        entry: Entry::Keyed,
        fields: &[
            f("icon", Sprite("GFX_decision_category_"), "Icon: GFX_decision_category_<icon> (or a GFX_ name)"),
            f("picture", Sprite(""), "Picture at the top of the category"),
            f("priority", Number, "Higher comes first"),
            f("allowed", Block, "Checked once at game start"),
            f("visible", Block, "When it is shown"),
            f("visibility_type", Word, "map_and_decisions_view / map_only / decisions_view_only"),
        ],
        loc: NAME_DESC,
        unique: true,
        national: true,
        help: "A category of decisions.",
    },
    Spec {
        kind: Kind::Idea,
        label: "Ideas (national spirits)",
        one: "idea",
        folders: &["common/ideas"],
        ext: "txt",
        path: &["ideas", "*"],
        entry: Entry::Keyed,
        fields: &[
            f("picture", Sprite("GFX_idea_"), "Picture: GFX_idea_<picture> (or a GFX_ name)"),
            f("name", Loc, "Another localisation key for the name"),
            f("allowed", Block, "Checked once at game start: who can have it"),
            f("allowed_civil_war", Block, "Kept by the side of a civil war that fits"),
            f("available", Block, "When it can be picked (laws, advisors)"),
            f("visible", Block, "When it is shown"),
            f("cancel", Block, "Removed when this holds"),
            f("on_add", Block, "Effects when added"),
            f("on_remove", Block, "Effects when removed"),
            f("modifier", Block, "Its modifiers: political_power_gain = 0.1 ..."),
            f("targeted_modifier", Block, "Modifiers against a tag"),
            f("research_bonus", Block, "Research bonuses by technology category"),
            f("equipment_bonus", Block, "Stats of equipment"),
            f("rule", Block, "Rules: can_create_factions = yes ..."),
            f("removal_cost", Number, "Political power to remove it (-1: cannot be removed)"),
            f("cost", Number, "Political power to adopt (laws)"),
            f("default", Bool, "The law a country has when it has none of the category"),
            f("ai_will_do", Block, "{ factor = 1 }"),
        ],
        loc: NAME_DESC,
        unique: true,
        national: true,
        help: "An idea: a national spirit, a law, an advisor slot; in a category (country, economy, trade_laws...).",
    },
    Spec {
        kind: Kind::Character,
        label: "Characters",
        one: "character",
        folders: &["common/characters"],
        ext: "txt",
        path: &["characters"],
        entry: Entry::Keyed,
        fields: &[
            f("name", Loc, "Name (a localisation key or the name itself)"),
            f("portraits", Block, "civilian = { large = GFX_... small = ... } army = { ... }"),
            f("gender", Word, "male / female (generated portraits)"),
            f("country_leader", Block, "As a leader: ideology = ..., traits = { ... }, expire = \"1965.1.1\""),
            f("corps_commander", Block, "As a general: traits, skill, attack_skill, defense_skill, planning_skill, logistics_skill"),
            f("field_marshal", Block, "As a field marshal: like corps_commander"),
            f("navy_leader", Block, "As an admiral: traits, skill, attack_skill, defense_skill, maneuvering_skill, coordination_skill"),
            rep("advisor", Block, "As an advisor: slot, idea_token, cost, allowed, traits, ai_will_do"),
            f("scientist", Block, "As a scientist: traits, skills"),
        ],
        loc: &[],
        unique: true,
        national: true,
        help: "A leader, general, admiral, advisor or scientist; recruit_character = id in the country's history gives it to the country.",
    },
    Spec {
        kind: Kind::CountryHistory,
        label: "Country history",
        one: "country history",
        folders: &["history/countries"],
        ext: "txt",
        path: &[],
        entry: Entry::File,
        fields: &[
            f("capital", State, "The capital state; the build writes the map's capital first, this one comes later and wins"),
            f("oob", File("history/units"), "Starting divisions: the name of a history/units file, without .txt"),
            f("set_research_slots", Number, "Research slots"),
            f("set_stability", Number, "Stability, 0 to 1"),
            f("set_war_support", Number, "War support, 0 to 1"),
            f("set_convoys", Number, "Convoys"),
            f("set_politics", Block, "ruling_party, last_election, election_frequency, elections_allowed"),
            f("set_popularities", Block, "Party popularities (sum 100)"),
            f("set_technology", Block, "Starting technologies: tech = 1"),
            rep("add_ideas", Block, "Starting ideas: { idea_a idea_b } or one id"),
            rep("recruit_character", Ref(Kind::Character), "Hires a character: a leader, general, admiral, advisor or scientist of the country"),
            rep("set_country_flag", Word, "A named marker the country carries, for has_country_flag in conditions"),
            rep("add_to_tech_sharing_group", IdIn("common/technology_sharing", "technology_sharing_group"), "Joins a technology sharing group: faster research of what its members have"),
            f("set_variable", Block, "Variables of the country: { name = value }"),
            rep("create_faction", Loc, "Founds a faction led by the country; the value is the faction's name (a localisation key)"),
            rep("add_to_faction", Tag, "Puts that country into this country's faction"),
            rep("give_guarantee", Tag, "Guarantees that country's independence"),
            rep("diplomatic_relation", Block, "relation = military_access country = TAG"),
            rep("set_autonomy", Block, "A subject: target = TAG autonomous_state = ..."),
        ],
        loc: &[],
        unique: true,
        national: true,
        help: "The country's own history lines; the build adds them after the history it generates (capital, techs, politics, militia).",
    },
    Spec {
        kind: Kind::DivisionTemplate,
        label: "Division templates and units",
        one: "division template",
        folders: &["history/units"],
        ext: "txt",
        path: &[],
        entry: Entry::Field(&["division_template"], "name"),
        fields: &[
            f("division_names_group", Word, "Name list for divisions of this template"),
            f("regiments", Block, "Battalions: infantry = { x = 0 y = 0 } (x: column 0-4, y: row)"),
            f("support", Block, "Support companies: engineer = { x = 0 y = 0 }"),
            f("regimental_support", Block, "Regimental support: field_guns = { x = column y = 0 }"),
            f("priority", Number, "Reinforcement priority 0-2"),
            f("is_locked", Bool, "Cannot be edited in game"),
        ],
        loc: &[],
        unique: false,
        national: true,
        help: "Division templates of a history/units file (an OOB); the units = { division = { ... } } block of the file places divisions.",
    },
    Spec {
        kind: Kind::CountryLeaderTrait,
        label: "Leader traits",
        one: "leader trait",
        folders: &["common/country_leader"],
        ext: "txt",
        path: &["leader_traits"],
        entry: Entry::Keyed,
        fields: TRAIT_FIELDS,
        loc: NAME_DESC,
        unique: true,
        national: false,
        help: "Traits of country leaders and advisors.",
    },
    Spec {
        kind: Kind::UnitLeaderTrait,
        label: "General / admiral traits",
        one: "unit leader trait",
        folders: &["common/unit_leader"],
        ext: "txt",
        path: &["leader_traits"],
        entry: Entry::Keyed,
        fields: &[
            f("type", Word, "land / navy / corps_commander / field_marshal / all"),
            f("trait_type", Word, "personality_trait / status_trait / assignable_trait / basic_terrain_trait ..."),
            f("gui_row", Number, "Row on the traits screen"),
            f("gui_column", Number, "Column on the traits screen"),
            f("modifier", Block, "Modifiers"),
            f("attack_skill", Number, "Skill added"),
            f("defense_skill", Number, "Skill added"),
            f("allowed", Block, "Who can have it"),
            f("ai_will_do", Block, "{ factor = 1 }"),
        ],
        loc: NAME_DESC,
        unique: true,
        national: false,
        help: "Traits of generals and admirals.",
    },
    Spec {
        kind: Kind::ScientistTrait,
        label: "Scientist traits",
        one: "scientist trait",
        folders: &["common/scientist_traits"],
        ext: "txt",
        path: &[],
        entry: Entry::Keyed,
        fields: &[f("icon", Sprite(""), "Icon"), f("modifier", Block, "Modifiers")],
        loc: NAME_DESC,
        unique: true,
        national: false,
        help: "Traits of scientists (special projects).",
    },
    Spec {
        kind: Kind::OpinionModifier,
        label: "Opinion modifiers",
        one: "opinion modifier",
        folders: &["common/opinion_modifiers"],
        ext: "txt",
        path: &["opinion_modifiers"],
        entry: Entry::Keyed,
        fields: &[
            f("value", Number, "Opinion change"),
            f("decay", Number, "Lost each day, towards 0"),
            f("min_trust", Number, "Lowest value"),
            f("max_trust", Number, "Highest value"),
            f("days", Number, "How long it lasts"),
            f("months", Number, "How long it lasts"),
            f("years", Number, "How long it lasts"),
            f("trade", Bool, "A trade opinion modifier"),
            f("target", Bool, "Against a target"),
        ],
        loc: &[("{id}", "Name")],
        unique: true,
        national: false,
        help: "Opinion one country has of another (add_opinion_modifier).",
    },
    Spec {
        kind: Kind::ScriptedEffect,
        label: "Scripted effects",
        one: "scripted effect",
        folders: &["common/scripted_effects"],
        ext: "txt",
        path: &[],
        entry: Entry::Keyed,
        fields: &[],
        loc: &[],
        unique: true,
        national: false,
        help: "Effects with a name: id = yes runs them anywhere effects go.",
    },
    Spec {
        kind: Kind::ScriptedTrigger,
        label: "Scripted triggers",
        one: "scripted trigger",
        folders: &["common/scripted_triggers"],
        ext: "txt",
        path: &[],
        entry: Entry::Keyed,
        fields: &[],
        loc: &[],
        unique: true,
        national: false,
        help: "Triggers with a name: id = yes checks them anywhere triggers go.",
    },
    Spec {
        kind: Kind::ScriptedLoc,
        label: "Scripted localisation",
        one: "scripted localisation",
        folders: &["common/scripted_localisation"],
        ext: "txt",
        path: &[],
        entry: Entry::Field(&["defined_text"], "name"),
        fields: &[rep("text", Block, "{ trigger = { ... } localization_key = KEY }: the first whose trigger holds")],
        loc: &[],
        unique: true,
        national: false,
        help: "Text that depends on triggers: [Root.id] in localisation.",
    },
    Spec {
        kind: Kind::DynamicModifier,
        label: "Dynamic modifiers",
        one: "dynamic modifier",
        folders: &["common/dynamic_modifiers"],
        ext: "txt",
        path: &[],
        entry: Entry::Keyed,
        fields: &[
            f("icon", Sprite(""), "Icon"),
            f("enable", Block, "When it applies"),
            f("remove_trigger", Block, "When it goes"),
        ],
        loc: &[("{id}", "Name")],
        unique: true,
        national: false,
        help: "Modifiers whose values can be variables (add_dynamic_modifier).",
    },
    Spec {
        kind: Kind::StaticModifier,
        label: "Static modifiers",
        one: "static modifier",
        folders: &["common/modifiers"],
        ext: "txt",
        path: &[],
        entry: Entry::Keyed,
        fields: &[],
        loc: &[("{id}", "Name")],
        unique: true,
        national: false,
        help: "Modifiers with a name (add_timed_idea, static modifiers of states).",
    },
    Spec {
        kind: Kind::OnAction,
        label: "On actions",
        one: "on action",
        folders: &["common/on_actions"],
        ext: "txt",
        path: &["on_actions"],
        entry: Entry::Keyed,
        fields: &[f("effect", Block, "Effects run on it"), f("random_events", Block, "{ 100 = event.1 0 = 0 }: one of these fires")],
        loc: &[],
        unique: false,
        national: false,
        help: "Effects and events on game happenings (on_startup, on_daily_TAG, on_war...). Several files may add to the same one.",
    },
    Spec {
        kind: Kind::AiStrategy,
        label: "AI strategies",
        one: "AI strategy",
        folders: &["common/ai_strategy"],
        ext: "txt",
        path: &[],
        entry: Entry::Keyed,
        fields: &[
            f("allowed", Block, "Checked once: who has it"),
            f("enable", Block, "When it is active"),
            f("abort", Block, "When it stops"),
            rep("ai_strategy", Block, "type = ..., id = TAG, value = 100"),
        ],
        loc: &[],
        unique: true,
        national: true,
        help: "What the AI of a country wants (alliances, antagonism, production...).",
    },
    Spec {
        kind: Kind::AiStrategyPlan,
        label: "AI strategy plans",
        one: "AI plan",
        folders: &["common/ai_strategy_plans"],
        ext: "txt",
        path: &[],
        entry: Entry::Keyed,
        fields: &[
            f("name", Text, "Name"),
            f("desc", Text, "What it is"),
            f("allowed", Block, "Who can have it"),
            f("enable", Block, "When it is picked"),
            f("abort", Block, "When it stops"),
            f("ai_national_focuses", Block, "The focuses the AI takes, in order"),
            f("research", Block, "Research weights"),
            f("ideas", Block, "Ideas weights"),
            f("weight", Block, "How likely"),
        ],
        loc: &[],
        unique: true,
        national: true,
        help: "A path the AI takes through its focus tree.",
    },
    Spec {
        kind: Kind::WarGoal,
        label: "War goals",
        one: "war goal",
        folders: &["common/wargoals"],
        ext: "txt",
        path: &["wargoal_types"],
        entry: Entry::Keyed,
        fields: &[
            f("war_name", Loc, "Name of wars over it"),
            f("generate_base_cost", Number, "Days to justify"),
            f("take_states_limit", Number, "States it can take"),
            f("allowed", Block, "Who can justify it"),
            f("available", Block, "Against whom"),
            f("on_add", Block, "Effects"),
        ],
        loc: NAME_DESC,
        unique: true,
        national: false,
        help: "A war goal type.",
    },
    Spec {
        kind: Kind::Ideology,
        label: "Ideologies",
        one: "ideology",
        folders: &["common/ideologies"],
        ext: "txt",
        path: &["ideologies"],
        entry: Entry::Keyed,
        fields: &[
            f("types", Block, "Sub-ideologies (leader ideologies)"),
            f("dynamic_faction_names", Block, "Faction names"),
            f("color", Block, "Colour { r g b }"),
            f("rules", Block, "What its countries can do"),
            f("modifiers", Block, "Modifiers of its countries"),
            f("faction_modifiers", Block, "Modifiers of its factions"),
            f("can_host_government_in_exile", Bool, "Can host governments in exile"),
            f("can_be_boosted", Bool, "Its popularity can be boosted"),
            f("can_collaborate", Bool, "Can have collaboration governments"),
            f("ai_ideology_wanted_units_factor", Number, "AI: how much more its countries build units (1 = as usual)"),
        ],
        loc: &[("{id}", "Name"), ("{id}_noun", "Noun"), ("{id}_desc", "Description"), ("{id}_drift", "Drift")],
        unique: true,
        national: false,
        help: "A political ideology (replacing the four is a big change: every ideology is checked all over the game).",
    },
    Spec {
        kind: Kind::Technology,
        label: "Technologies",
        one: "technology",
        folders: &["common/technologies"],
        ext: "txt",
        path: &["technologies"],
        entry: Entry::Keyed,
        fields: &[
            f("research_cost", Number, "Cost (1 = about 100 days)"),
            f("start_year", Number, "Year it is meant for"),
            f("folder", Block, "Where it shows: name = infantry_folder position = { x = 0 y = 0 }"),
            f("path", Block, "Unlocks: leads_to_tech = X research_cost_coeff = 1"),
            f("categories", Block, "Categories (research bonuses apply by them)"),
            f("enable_equipments", Block, "Equipment it unlocks"),
            f("enable_subunits", Block, "Battalions it unlocks"),
            f("allow", Block, "Who can research it"),
            f("allow_branch", Block, "Shown only when this holds"),
            f("ai_will_do", Block, "{ factor = 1 }"),
        ],
        loc: NAME_DESC,
        unique: true,
        national: false,
        help: "A technology of the research screen.",
    },
    Spec {
        kind: Kind::Equipment,
        label: "Equipment",
        one: "equipment",
        folders: &["common/units/equipment"],
        ext: "txt",
        path: &["equipments"],
        entry: Entry::Keyed,
        fields: &[
            f("year", Number, "Year"),
            f("is_archetype", Bool, "An archetype (the family)"),
            f("archetype", Word, "Its archetype"),
            f("parent", Word, "The model it upgrades"),
            f("priority", Number, "Production priority"),
            f("build_cost_ic", Number, "Production cost"),
            f("reliability", Number, "Reliability"),
            f("resources", Block, "Resources per unit of production"),
            f("type", Word, "infantry / support / armor ..."),
        ],
        loc: &[("{id}", "Name"), ("{id}_short", "Short name"), ("{id}_desc", "Description")],
        unique: true,
        national: false,
        help: "Equipment (and its stats).",
    },
    Spec {
        kind: Kind::SubUnit,
        label: "Battalions",
        one: "battalion",
        folders: &["common/units"],
        ext: "txt",
        path: &["sub_units"],
        entry: Entry::Keyed,
        fields: &[
            f("sprite", Word, "Unit sprite"),
            f("map_icon_category", Word, "infantry / armored / ship ..."),
            f("priority", Number, "Priority"),
            f("type", Block, "Types"),
            f("group", Word, "infantry / support / armor ..."),
            f("categories", Block, "Categories"),
            f("combat_width", Number, "Combat width"),
            f("manpower", Number, "Manpower"),
            f("max_organisation", Number, "Organisation"),
            f("need", Block, "Equipment it needs"),
        ],
        loc: NAME_DESC,
        unique: true,
        national: false,
        help: "A battalion or support company type.",
    },
    Spec {
        kind: Kind::Bookmark,
        label: "Bookmarks",
        one: "bookmark",
        folders: &["common/bookmarks"],
        ext: "txt",
        path: &["bookmarks"],
        entry: Entry::Field(&["bookmark"], "name"),
        fields: &[
            f("desc", Loc, "Description key"),
            f("date", Word, "Start date 1936.1.1.12"),
            f("picture", Sprite(""), "Picture"),
            f("default_country", Text, "Selected at the start"),
            f("default", Bool, "The start picked first"),
        ],
        loc: &[],
        unique: true,
        national: false,
        help: "A start date with its featured countries. The build writes one (common/bookmarks/suedtirol.txt); a file of that name here replaces it.",
    },
    Spec {
        kind: Kind::PeaceAi,
        label: "Peace conference AI",
        one: "peace AI rule",
        folders: &["common/peace_conference/ai_peace"],
        ext: "txt",
        path: &[],
        entry: Entry::Keyed,
        fields: &[f("enable", Block, "When it applies"), f("annex_randomness", Number, "How much chance changes whether the AI annexes"), f("liberate_randomness", Number, "How much chance changes whether the AI liberates")],
        loc: &[],
        unique: true,
        national: false,
        help: "How the AI takes its share at a peace conference.",
    },
    Spec {
        kind: Kind::PeaceCost,
        label: "Peace conference costs",
        one: "peace cost modifier",
        folders: &["common/peace_conference/cost_modifiers"],
        ext: "txt",
        path: &[],
        entry: Entry::Keyed,
        fields: &[f("peace_action_type", Block, "take_states / puppet / liberate ..."), f("cost_multiplier", Number, "The cost in peace conference points is multiplied by this"), f("enable", Block, "When it applies")],
        loc: &[],
        unique: true,
        national: false,
        help: "What peace conference actions cost.",
    },
    Spec {
        kind: Kind::AgencyUpgrade,
        label: "Intelligence agency upgrades",
        one: "agency upgrade",
        folders: &["common/intelligence_agency_upgrades"],
        ext: "txt",
        path: &["*"],
        entry: Entry::Keyed,
        fields: &[f("picture", Sprite(""), "Picture"), f("frame", Sprite(""), "Frame"), f("sound", Word, "Sound"), rep("level", Block, "A level: cost, modifiers")],
        loc: NAME_DESC,
        unique: true,
        national: false,
        help: "Upgrades of an intelligence agency (La Résistance).",
    },
    Spec {
        kind: Kind::Autonomy,
        label: "Autonomy levels",
        one: "autonomy level",
        folders: &["common/autonomous_states"],
        ext: "txt",
        path: &[],
        entry: Entry::Field(&["autonomy_state"], "id"),
        fields: &[f("min_freedom_level", Number, "Freedom (0 to 1) a subject needs for this level"), f("manpower_influence", Number, "Share of the subject's manpower the overlord gets"), f("rule", Block, "Rules of subjects at this level: can_decline_call_to_war = yes ..."), f("modifier", Block, "Modifiers of subjects at this level")],
        loc: NAME_DESC,
        unique: true,
        national: false,
        help: "A level of autonomy of subjects.",
    },
    Spec {
        kind: Kind::Song,
        label: "Music",
        one: "song",
        folders: &["music"],
        ext: "txt",
        path: &[],
        entry: Entry::Field(&["music"], "song"),
        fields: &[f("chance", Block, "{ factor = 1 modifier = { ... } }")],
        loc: &[("{id}", "Title")],
        unique: false,
        national: false,
        help: "Songs of a music station (music_station = name at the top of the file).",
    },
    Spec {
        kind: Kind::Names,
        label: "Name lists",
        one: "name list",
        folders: &["common/names"],
        ext: "txt",
        path: &[],
        entry: Entry::Keyed,
        fields: &[
            f("male", Block, "names = { ... } first names of men"),
            f("female", Block, "names = { ... } first names of women"),
            f("surnames", Block, "Surnames"),
            f("callsigns", Block, "Pilot callsigns"),
            f("ship_names", Block, "Ship names"),
        ],
        loc: &[],
        unique: true,
        national: true,
        help: "Names for generated leaders and pilots (the build gives every nation those of AUS, ITA or SWI by language; one here wins).",
    },
    Spec {
        kind: Kind::Sprite,
        label: "Sprites (icons)",
        one: "sprite",
        folders: &["interface"],
        ext: "gfx",
        path: &["spriteTypes"],
        entry: Entry::Field(&["spriteType", "frameAnimatedSpriteType", "corneredTileSpriteType", "textSpriteType", "progressbartype", "maskedShieldType"], "name"),
        fields: &[
            f("texturefile", Text, "The picture: gfx/interface/goals/x.dds (DDS, TGA or PNG)"),
            f("noOfFrames", Number, "Frames side by side in the picture"),
            f("legacy_lazy_load", Bool, "Loaded when first shown"),
        ],
        loc: &[],
        unique: true,
        national: false,
        help: "A picture with a GFX_ name that icons, pictures and portraits use.",
    },
];

const FOCUS_FIELDS: &[Field] = &[
    f("icon", Sprite(""), "Icon, GFX_goal_..."),
    f("cost", Number, "Weeks (×7 days) it takes"),
    f("x", Number, "Column (relative to relative_position_id when given)"),
    f("y", Number, "Row"),
    f("relative_position_id", Ref(Kind::Focus), "x and y count from this focus"),
    rep("prerequisite", Block, "{ focus = A focus = B }: A or B; several blocks: all of them"),
    f("mutually_exclusive", Block, "{ focus = C }: taking one locks the others"),
    f("available", Block, "When it can be taken"),
    f("bypass", Block, "Done at once when this holds"),
    f("allow_branch", Block, "The focus and those after it exist only when this holds (DLC checks)"),
    f("cancel_if_invalid", Bool, "Stops when available no longer holds"),
    f("continue_if_invalid", Bool, "Goes on when available no longer holds"),
    f("available_if_capitulated", Bool, "Can be taken while capitulated"),
    f("cancelable", Bool, "Can be cancelled"),
    f("will_lead_to_war_with", Tag, "AI: it leads to war with this country, which the AI prepares for"),
    f("search_filters", Block, "{ FOCUS_FILTER_POLITICAL ... } for the search of the tree view"),
    f("text", Loc, "Another localisation key for the name"),
    f("select_effect", Block, "Effects when it is started"),
    f("completion_reward", Block, "Effects when it is done"),
    f("complete_tooltip", Block, "Shown instead of the effects"),
    f("ai_will_do", Block, "{ factor = 1 } how much the AI wants it"),
    rep("offset", Block, "{ x = 1 y = 0 trigger = { ... } }: moved when the trigger holds"),
];

const TRAIT_FIELDS: &[Field] = &[
    f("random", Bool, "Given to generated leaders"),
    f("sprite", Number, "Icon frame"),
    f("ai_will_do", Block, "{ factor = 1 }"),
    f("modifier", Block, "Modifiers (the country's)"),
    f("targeted_modifier", Block, "Modifiers against a tag"),
    f("research_bonus", Block, "Research bonuses"),
    f("equipment_bonus", Block, "Equipment stats"),
    f("command_cap_increase", Number, "More command power the country can hold"),
];

pub fn spec(kind: Kind) -> &'static Spec {
    SPECS.iter().find(|s| s.kind == kind).expect("every kind has a spec")
}

impl Kind {
    pub fn spec(self) -> &'static Spec {
        spec(self)
    }
    pub fn label(self) -> &'static str {
        self.spec().label
    }
    pub fn all() -> impl Iterator<Item = Kind> {
        SPECS.iter().map(|s| s.kind)
    }
    /// Kinds listed in the content screen (focuses are on the tree view).
    pub fn listed() -> impl Iterator<Item = Kind> {
        Kind::all().filter(|k| !matches!(k, Kind::Focus))
    }
}

/// The kinds a file of the mod (path relative to the mod, with `/`) holds.
pub fn kinds_of_path(rel: &str) -> Vec<Kind> {
    let (dir, name) = rel.rsplit_once('/').unwrap_or(("", rel));
    let ext = name.rsplit_once('.').map(|x| x.1.to_ascii_lowercase()).unwrap_or_default();
    let mut out: Vec<Kind> = SPECS
        .iter()
        .filter(|s| s.ext == ext && s.folders.iter().any(|f| dir.eq_ignore_ascii_case(f) || (s.kind == Kind::Sprite && (dir.starts_with("interface/") || dir == "gfx"))))
        .map(|s| s.kind)
        .collect();
    // a deeper folder of its own wins (decisions/categories is not decisions)
    if out.contains(&Kind::DecisionCategory) {
        out.retain(|k| *k != Kind::Decision);
    }
    out
}

/// A definition found in a file.
#[derive(Clone, Debug, PartialEq)]
pub struct Def {
    pub kind: Kind,
    pub id: String,
    /// Category, idea category, event namespace, focus tree id...
    pub group: Option<String>,
    /// Indexes of the items from the root of the file to it (empty: the whole file).
    pub path: Vec<usize>,
    pub span: std::ops::Range<usize>,
    /// Tags it names (`tag = X`, `original_tag = X`, a focus tree's country).
    pub tags: Vec<String>,
}

impl Def {
    /// The item of the definition in a parse of its file.
    pub fn item<'a>(&self, script: &'a Script) -> Option<&'a Item> {
        let mut b = &script.root;
        let mut it = None;
        for &i in &self.path {
            let x = b.items.get(i)?;
            it = Some(x);
            match x.block() {
                Some(c) => b = c,
                None => break,
            }
        }
        it
    }
    /// The block of the definition (the root for a whole-file one).
    pub fn block<'a>(&self, script: &'a Script) -> Option<&'a Block> {
        if self.path.is_empty() {
            return Some(&script.root);
        }
        self.item(script)?.block()
    }
}

/// Tags a block names: `tag = X`, `original_tag = X`.
pub fn tags_in(b: &Block) -> Vec<String> {
    let mut out = Vec::new();
    b.walk(&mut |i, _| {
        if (i.is("tag") || i.is("original_tag"))
            && let Some(t) = i.text()
            && is_tag(t)
            && !out.iter().any(|x| x == t)
        {
            out.push(t.to_string());
        }
    });
    out
}

/// Three capitals or digits, starting with a letter.
pub fn is_tag(t: &str) -> bool {
    let b = t.as_bytes();
    b.len() == 3 && b[0].is_ascii_uppercase() && b.iter().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

/// The tag an id starts with (`TYR_focus` → TYR).
pub fn tag_prefix(id: &str) -> Option<&str> {
    let (p, _) = id.split_once('_')?;
    is_tag(p).then_some(p)
}

/// The definitions of the given kinds in a file. `file_name` is the name of
/// the file (country history: the tag it starts with).
pub fn defs_of(script: &Script, kinds: &[Kind], file_name: &str) -> Vec<Def> {
    let mut out = Vec::new();
    for &k in kinds {
        let s = k.spec();
        if s.entry == Entry::File {
            let id = file_name.split([' ', '-', '.']).next().unwrap_or("").to_string();
            out.push(Def { kind: k, id, group: None, path: Vec::new(), span: 0..script.root.close, tags: Vec::new() });
            continue;
        }
        walk_path(&script.root, s.path, &mut Vec::new(), None, &mut |block, path, group| {
            for (i, it) in block.items.iter().enumerate() {
                let Some(key) = it.key.as_ref() else { continue };
                let Some(b) = it.block() else { continue };
                let id = match s.entry {
                    Entry::Keyed => {
                        if key.text.starts_with('@') {
                            continue;
                        }
                        key.text.clone()
                    }
                    Entry::Field(keys, field) => {
                        if !keys.iter().any(|x| key.text.eq_ignore_ascii_case(x)) {
                            continue;
                        }
                        match b.atom(field) {
                            Some(v) => v.to_string(),
                            None => continue,
                        }
                    }
                    Entry::File => unreachable!(),
                };
                let mut p = path.to_vec();
                p.push(i);
                let mut tags = tags_in(b);
                if k == Kind::FocusTree
                    && let Some(c) = b.block("country")
                {
                    tags = tags_in(c);
                }
                let group = match k {
                    Kind::Event => id.split_once('.').map(|x| x.0.to_string()),
                    _ => group.map(str::to_string),
                };
                out.push(Def { kind: k, id, group, path: p, span: it.span.clone(), tags });
            }
        });
    }
    out
}

/// Calls `f` with each block at `path` from `b` (with the item indexes to it
/// and the group: the key of a `*` step, or the id of the block around).
fn walk_path(b: &Block, path: &[&str], at: &mut Vec<usize>, group: Option<&str>, f: &mut dyn FnMut(&Block, &[usize], Option<&str>)) {
    let Some((first, rest)) = path.split_first() else {
        f(b, at, group);
        return;
    };
    for (i, it) in b.items.iter().enumerate() {
        let Some(key) = it.key.as_ref() else { continue };
        let Some(c) = it.block() else { continue };
        if *first == "*" || key.text.eq_ignore_ascii_case(first) {
            let g = if *first == "*" { Some(key.text.as_str()) } else { c.atom("id").or(group) };
            at.push(i);
            walk_path(c, rest, at, g, f);
            at.pop();
        }
    }
}

/// Localisation keys of a definition: (key, what it is).
pub fn loc_keys(kind: Kind, id: &str, block: Option<&Block>) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = kind.spec().loc.iter().map(|(p, what)| (p.replace("{id}", id), what.to_string())).collect();
    let Some(b) = block else { return out };
    match kind {
        Kind::Event => {
            for (k, what) in [("title", "Title"), ("desc", "Text")] {
                for it in b.all(k) {
                    if let Some(t) = it.text() {
                        out.push((t.to_string(), what.into()));
                    } else if let Some(t) = it.block().and_then(|x| x.atom("text")) {
                        out.push((t.to_string(), format!("{what} (with a trigger)")));
                    }
                }
            }
            for (n, o) in b.blocks("option").enumerate() {
                if let Some(t) = o.atom("name") {
                    out.push((t.to_string(), format!("Option {}", n + 1)));
                }
            }
        }
        Kind::Focus | Kind::SharedFocus | Kind::Idea => {
            let field = if kind == Kind::Idea { "name" } else { "text" };
            if let Some(t) = b.atom(field) {
                out[0] = (t.to_string(), "Name".into());
                if kind != Kind::Idea {
                    out[1] = (format!("{t}_desc"), "Description".into());
                }
            }
        }
        Kind::Character => {
            if let Some(t) = b.atom("name") {
                out.push((t.to_string(), "Name".into()));
            }
            if let Some(t) = b.block("country_leader").and_then(|c| c.atom("desc")) {
                out.push((t.to_string(), "Leader description".into()));
            }
        }
        Kind::Bookmark => {
            out.push((id.to_string(), "Name".into()));
            if let Some(t) = b.atom("desc") {
                out.push((t.to_string(), "Description".into()));
            }
        }
        Kind::WarGoal => {
            if let Some(t) = b.atom("war_name") {
                out.push((t.to_string(), "War name".into()));
            }
        }
        _ => {}
    }
    out.retain(|(k, _)| !k.is_empty() && !k.starts_with('['));
    out.dedup();
    out
}

/// Sprite names a definition shows: (field, sprite).
/// Sprite fields written by a short name (`picture = GER_x`): (value, sprite).
pub fn short_sprites_of(kind: Kind, b: &Block) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for fd in kind.spec().fields {
        if let Sprite(prefix) = fd.ty
            && !prefix.is_empty()
        {
            for it in b.all(fd.key) {
                if let Some(v) = it.text()
                    && !v.starts_with("GFX_")
                    && !v.starts_with("gfx_")
                    && !v.starts_with('[')
                {
                    out.push((v.to_string(), sprite_name(prefix, v)));
                }
            }
        }
    }
    out
}

pub fn sprites_of(kind: Kind, b: &Block) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    for fd in kind.spec().fields {
        if let Sprite(prefix) = fd.ty {
            for it in b.all(fd.key) {
                // `[GetPicture]` is scripted localisation that picks one
                if let Some(v) = it.text()
                    && !v.starts_with('[')
                {
                    out.push((fd.key, sprite_name(prefix, v)));
                }
            }
        }
    }
    if kind == Kind::Character {
        b.walk(&mut |i, _| {
            if (i.is("large") || i.is("small"))
                && let Some(v) = i.text()
                && v.starts_with("GFX_")
            {
                out.push(("portraits", v.to_string()));
            }
        });
    }
    out
}

/// The sprite a field value names: GFX_ names as they are, else with the prefix.
pub fn sprite_name(prefix: &str, value: &str) -> String {
    if value.starts_with("GFX_") || value.starts_with("gfx_") || prefix.is_empty() { value.to_string() } else { format!("{prefix}{value}") }
}

/// A new definition: the text of its item.
pub fn template(kind: Kind, id: &str, tag: &str) -> String {
    let t = tag;
    match kind {
        Kind::FocusTree => format!(
            "focus_tree = {{\n\tid = {id}\n\tcountry = {{\n\t\tfactor = 0\n\t\tmodifier = {{\n\t\t\tadd = 10\n\t\t\ttag = {t}\n\t\t}}\n\t}}\n\tdefault = no\n\tcontinuous_focus_position = {{ x = 50 y = 1200 }}\n\n\tfocus = {{\n\t\tid = {t}_first_steps\n\t\ticon = GFX_goal_generic_political_pressure\n\t\tx = 0\n\t\ty = 0\n\t\tcost = 10\n\t\tavailable_if_capitulated = yes\n\t\tcompletion_reward = {{\n\t\t\tadd_political_power = 120\n\t\t}}\n\t\tai_will_do = {{ factor = 1 }}\n\t}}\n}}"
        ),
        Kind::Focus | Kind::SharedFocus => format!(
            "{} = {{\n\tid = {id}\n\ticon = GFX_goal_unknown\n\tx = 0\n\ty = 0\n\tcost = 10\n\tcompletion_reward = {{\n\t\tadd_political_power = 50\n\t}}\n\tai_will_do = {{ factor = 1 }}\n}}",
            if kind == Kind::Focus { "focus" } else { "shared_focus" }
        ),
        Kind::Event => format!(
            "country_event = {{\n\tid = {id}\n\ttitle = {id}.t\n\tdesc = {id}.d\n\tpicture = GFX_report_event_generic_read_write\n\tis_triggered_only = yes\n\n\toption = {{\n\t\tname = {id}.a\n\t\tai_chance = {{ factor = 1 }}\n\t\tadd_political_power = 25\n\t}}\n}}"
        ),
        Kind::Decision => format!(
            "{id} = {{\n\ticon = generic_political_discourse\n\tcost = 50\n\tfire_only_once = yes\n\tavailable = {{\n\t\thas_war = no\n\t}}\n\tcomplete_effect = {{\n\t\tadd_stability = 0.05\n\t}}\n\tai_will_do = {{ factor = 1 }}\n}}"
        ),
        Kind::DecisionCategory => format!("{id} = {{\n\ticon = generic_independence\n\tallowed = {{\n\t\toriginal_tag = {t}\n\t}}\n}}"),
        Kind::Idea => format!(
            "{id} = {{\n\tpicture = generic_morale_bonus\n\tallowed = {{\n\t\toriginal_tag = {t}\n\t}}\n\tallowed_civil_war = {{ always = yes }}\n\tremoval_cost = -1\n\tmodifier = {{\n\t\tstability_factor = 0.05\n\t}}\n}}"
        ),
        Kind::Character => format!(
            "{id} = {{\n\tname = {id}\n\tportraits = {{\n\t\tcivilian = {{\n\t\t\tlarge = GFX_portrait_unknown\n\t\t}}\n\t}}\n\tcountry_leader = {{\n\t\tideology = despotism\n\t\ttraits = {{ }}\n\t\texpire = \"1965.1.1.1\"\n\t\tid = -1\n\t}}\n}}"
        ),
        Kind::CountryHistory => "set_research_slots = 3\nset_stability = 0.6\nset_war_support = 0.2\n".to_string(),
        Kind::DivisionTemplate => format!(
            "division_template = {{\n\tname = \"{id}\"\n\tregiments = {{\n\t\tinfantry = {{ x = 0 y = 0 }}\n\t\tinfantry = {{ x = 0 y = 1 }}\n\t\tinfantry = {{ x = 1 y = 0 }}\n\t}}\n\tsupport = {{\n\t\tengineer = {{ x = 0 y = 0 }}\n\t}}\n}}"
        ),
        Kind::OpinionModifier => format!("{id} = {{\n\tvalue = 25\n\tdecay = 1\n}}"),
        Kind::ScriptedEffect | Kind::ScriptedTrigger => format!("{id} = {{\n\talways = yes\n}}"),
        Kind::ScriptedLoc => format!("defined_text = {{\n\tname = {id}\n\ttext = {{\n\t\ttrigger = {{ always = yes }}\n\t\tlocalization_key = {id}_text\n\t}}\n}}"),
        Kind::DynamicModifier | Kind::StaticModifier => format!("{id} = {{\n\tstability_factor = 0.05\n}}"),
        Kind::OnAction => "on_startup = {\n\teffect = {\n\t}\n}".to_string(),
        Kind::AiStrategy => format!("{id} = {{\n\tallowed = {{\n\t\toriginal_tag = {t}\n\t}}\n\tenable = {{ always = yes }}\n\tabort = {{ always = no }}\n\tai_strategy = {{\n\t\ttype = alliance\n\t\tid = \"AUS\"\n\t\tvalue = 50\n\t}}\n}}"),
        Kind::AiStrategyPlan => format!("{id} = {{\n\tname = \"{id}\"\n\tdesc = \"\"\n\tallowed = {{\n\t\toriginal_tag = {t}\n\t}}\n\tenable = {{ always = yes }}\n\tai_national_focuses = {{\n\t}}\n\tweight = {{\n\t\tfactor = 1\n\t}}\n}}"),
        Kind::WarGoal => format!("{id} = {{\n\twar_name = {id}_war_name\n\tgenerate_base_cost = 30\n\ttake_states_limit = 1\n}}"),
        Kind::Ideology => format!("{id} = {{\n\ttypes = {{\n\t\t{id}_subtype = {{ }}\n\t}}\n\tdynamic_faction_names = {{ \"FACTION_NAME_{id}_1\" }}\n\tcolor = {{ 100 100 100 }}\n\trules = {{\n\t}}\n\tmodifiers = {{\n\t}}\n}}"),
        Kind::Technology => format!("{id} = {{\n\tresearch_cost = 1\n\tstart_year = 1936\n\tfolder = {{\n\t\tname = infantry_folder\n\t\tposition = {{ x = 0 y = 0 }}\n\t}}\n\tcategories = {{\n\t}}\n\tai_will_do = {{ factor = 1 }}\n}}"),
        Kind::Song => format!("music = {{\n\tsong = \"{id}\"\n\tchance = {{\n\t\tfactor = 1\n\t}}\n}}"),
        Kind::Names => format!("{t} = {{\n\tmale = {{\n\t\tnames = {{ }}\n\t}}\n\tfemale = {{\n\t\tnames = {{ }}\n\t}}\n\tsurnames = {{ }}\n\tcallsigns = {{ }}\n}}"),
        Kind::Sprite => format!("spriteType = {{\n\tname = \"{id}\"\n\ttexturefile = \"gfx/interface/goals/{}.dds\"\n}}", id.trim_start_matches("GFX_")),
        Kind::Bookmark => format!("bookmark = {{\n\tname = \"{id}\"\n\tdesc = \"{id}_DESC\"\n\tdate = 1936.1.1.12\n\tpicture = \"GFX_select_date_1936\"\n\tdefault_country = \"{t}\"\n}}"),
        _ => format!("{id} = {{\n}}"),
    }
}

/// The file a new definition of a kind goes to in a package of `tag`
/// (shared: "mod"), relative to the mod.
pub fn default_file(kind: Kind, tag: &str, group: Option<&str>) -> String {
    let low = tag.to_ascii_lowercase();
    let s = kind.spec();
    let dir = s.folders[0];
    match kind {
        Kind::FocusTree | Kind::Focus => format!("{dir}/{low}_{}.txt", group.map(|g| g.trim_start_matches(&format!("{tag}_"))).unwrap_or("focus")),
        Kind::Event => {
            let g = group.unwrap_or("events");
            if g.eq_ignore_ascii_case(&low) || g.starts_with(&format!("{low}_")) { format!("{dir}/{g}.txt") } else { format!("{dir}/{low}_{g}.txt") }
        }
        Kind::Character => format!("{dir}/{tag}.txt"),
        Kind::CountryHistory => format!("{dir}/{tag}.txt"),
        Kind::DivisionTemplate => format!("{dir}/{tag}_content.txt"),
        Kind::Sprite => format!("{dir}/{low}_content.gfx"),
        Kind::Song => format!("{dir}/{low}_songs.txt"),
        _ => format!("{dir}/{low}_{}.txt", s.one.replace([' ', '/'], "_").to_ascii_lowercase()),
    }
}

/// The text of a new file for a kind (the blocks around its definitions).
pub fn file_skeleton(kind: Kind, group: Option<&str>) -> String {
    match kind {
        Kind::Event => format!("add_namespace = {}\n\n", group.unwrap_or("events")),
        Kind::Idea => format!("ideas = {{\n\t{} = {{\n\t}}\n}}\n", group.unwrap_or("country")),
        Kind::Decision => format!("{} = {{\n}}\n", group.unwrap_or("political_actions")),
        Kind::Song => "music_station = \"mod\"\n\n".to_string(),
        _ => match kind.spec().path.first() {
            Some(w) if *w != "*" => format!("{w} = {{\n}}\n"),
            _ => String::new(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_definitions() {
        let text = "add_namespace = tyr\ncountry_event = { id = tyr.1 title = tyr.1.t desc = tyr.1.d option = { name = tyr.1.a } trigger = { tag = TYR } }\nnews_event = { id = tyr.2 }\n";
        let s = Script::parse(text);
        let d = defs_of(&s, &kinds_of_path("events/tyr.txt"), "tyr.txt");
        assert_eq!(d.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), ["tyr.1", "tyr.2"]);
        assert_eq!(d[0].group.as_deref(), Some("tyr"));
        assert_eq!(d[0].tags, ["TYR"]);
        let b = d[0].block(&s).unwrap();
        assert_eq!(b.atom("title"), Some("tyr.1.t"));
        let keys: Vec<String> = loc_keys(Kind::Event, "tyr.1", Some(b)).into_iter().map(|x| x.0).collect();
        assert_eq!(keys, ["tyr.1.t", "tyr.1.d", "tyr.1.a"]);

        let text = "ideas = { country = { TYR_spirit = { picture = x modifier = { stability_factor = 0.1 } } } economy = { TYR_eco = { } } }";
        let s = Script::parse(text);
        let d = defs_of(&s, &kinds_of_path("common/ideas/TYR.txt"), "TYR.txt");
        assert_eq!(d.iter().map(|x| (x.id.as_str(), x.group.as_deref())).collect::<Vec<_>>(), [("TYR_spirit", Some("country")), ("TYR_eco", Some("economy"))]);
        assert_eq!(sprites_of(Kind::Idea, d[0].block(&s).unwrap()), [("picture", "GFX_idea_x".to_string())]);

        let text = "focus_tree = { id = tyr_tree country = { factor = 0 modifier = { add = 10 tag = TYR } } focus = { id = TYR_a } focus = { id = TYR_b } }\nshared_focus = { id = S_a }";
        let s = Script::parse(text);
        let d = defs_of(&s, &kinds_of_path("common/national_focus/tyr.txt"), "tyr.txt");
        let got: Vec<(Kind, &str, Option<&str>)> = d.iter().map(|x| (x.kind, x.id.as_str(), x.group.as_deref())).collect();
        assert_eq!(got, [(Kind::FocusTree, "tyr_tree", None), (Kind::Focus, "TYR_a", Some("tyr_tree")), (Kind::Focus, "TYR_b", Some("tyr_tree")), (Kind::SharedFocus, "S_a", None)]);
        assert_eq!(d[0].tags, ["TYR"]);

        assert_eq!(kinds_of_path("common/decisions/categories/x.txt"), [Kind::DecisionCategory]);
        assert_eq!(kinds_of_path("common/decisions/x.txt"), [Kind::Decision]);
        assert_eq!(kinds_of_path("interface/x.gfx"), [Kind::Sprite]);
        assert_eq!(kinds_of_path("history/countries/TYR - Tirol.txt"), [Kind::CountryHistory]);
        let s = Script::parse("capital = 1");
        let d = defs_of(&s, &[Kind::CountryHistory], "TYR - Tirol.txt");
        assert_eq!(d[0].id, "TYR");
        assert!(d[0].block(&s).is_some());
    }

    #[test]
    fn templates_parse() {
        for k in Kind::all() {
            let t = template(k, "TYR_new", "TYR");
            let s = Script::parse(&t);
            assert!(s.errors.is_empty(), "{k:?}: {:?}", s.errors);
            let sk = file_skeleton(k, Some("tyr"));
            assert!(Script::parse(&sk).errors.is_empty(), "{k:?}");
        }
        assert_eq!(default_file(Kind::Idea, "TYR", None), "common/ideas/tyr_idea.txt");
        assert_eq!(default_file(Kind::Event, "TYR", Some("tyr")), "events/tyr.txt");
        assert_eq!(default_file(Kind::Event, "TYR", Some("tyr_germany")), "events/tyr_germany.txt");
        assert_eq!(default_file(Kind::Event, "TYR", Some("germany")), "events/tyr_germany.txt");
    }
}
