//! What the fields of the forms do in the game, at more length than the one
//! line of [`Field::help`]: how the game uses a field, what its value is,
//! and the field as it is written in a file.
//!
//! [`about`] has the explanation of a field of a kind, else of a field of
//! that name in any kind; [`value_of`] says in words what a field takes, and
//! [`example`] gives a line of it (its own, or one made from its type).

use super::kinds::{Field, FieldType, Kind};

/// A field explained.
pub struct About {
    /// What it does and how the game uses it.
    pub text: &'static str,
    /// The field written in a file (`TAG` is the nation's tag).
    pub example: &'static str,
}

const fn a(text: &'static str, example: &'static str) -> About {
    About { text, example }
}

use Kind as K;

/// Fields of one kind.
const OF_KIND: &[(Kind, &str, About)] = &[
    // ---------------------------------------------------------------- country history
    (
        K::CountryHistory,
        "capital",
        a(
            "The state the country's capital is in, by its number on the map (the map button beside the field picks it by clicking it). The capital is the seat of government and where the country's supply starts. The build already writes the capital the nation has in the map editor at the top of the history file; this line comes later in the file, so the game takes this one.",
            "capital = 12",
        ),
    ),
    (
        K::CountryHistory,
        "oob",
        a(
            "Order of battle: the file of division templates and divisions the country starts with, history/units/<name>.txt, written without the .txt. The build writes oob = \"TAG_1936\" with the divisions it places from the map (unless the nation has no units in the map editor); an oob here comes later, and the game uses the last one, so the country starts with the divisions of your file instead. Make the file on the Division templates page. To give divisions later in the game, use the effect load_oob in a focus or an event.",
            "oob = \"TAG_1936\"",
        ),
    ),
    (
        K::CountryHistory,
        "set_research_slots",
        a("How many technologies the country researches at the same time. Minor countries start with 2 or 3, great powers with 4 or 5. The build writes one; this line comes later and wins.", "set_research_slots = 3"),
    ),
    (
        K::CountryHistory,
        "set_stability",
        a("Stability at the start, as a fraction: 0.6 is 60%. Low stability lowers factory output and political power and makes some decisions unavailable. The build writes 0.6; this line comes later and wins.", "set_stability = 0.6"),
    ),
    (
        K::CountryHistory,
        "set_war_support",
        a("War support at the start, as a fraction: 0.2 is 20%. It decides which conscription and economy laws the country can adopt and how well it holds up in a war. The build writes 0.2; this line comes later and wins.", "set_war_support = 0.2"),
    ),
    (
        K::CountryHistory,
        "set_convoys",
        a("Convoys (cargo ships) at the start. They carry traded resources and supply over the sea; a landlocked country needs none.", "set_convoys = 10"),
    ),
    (
        K::CountryHistory,
        "set_politics",
        a(
            "The government at the start: ruling_party is the ideology in power (democratic, fascism, communism, neutrality or an ideology of yours), last_election the date of the last election, election_frequency the months between elections and elections_allowed whether there are any. The build writes one from the nation's ideology in the map editor; this one comes later and wins.",
            "set_politics = {\n\truling_party = neutrality\n\tlast_election = \"1932.1.1\"\n\telection_frequency = 48\n\telections_allowed = no\n}",
        ),
    ),
    (
        K::CountryHistory,
        "set_popularities",
        a("How popular each ideology is at the start, in percent. The numbers must add up to exactly 100; an ideology left out has 0. The ruling party should have the most.", "set_popularities = {\n\tneutrality = 70\n\tdemocratic = 20\n\tfascism = 5\n\tcommunism = 5\n}"),
    ),
    (
        K::CountryHistory,
        "set_technology",
        a("Technologies the country has at the start: the id of each, = 1. The build gives every nation the starting technologies of a minor (infantry weapons, trucks, artillery, support, engineers, trains); these come on top.", "set_technology = {\n\tinfantry_weapons1 = 1\n\tsupport_weapons = 1\n}"),
    ),
    (
        K::CountryHistory,
        "add_ideas",
        a("Ideas the country has at the start: national spirits, laws and the like. One id, or several in braces. A law replaces the law the country had in its category (a conscription law replaces the default one).", "add_ideas = { TAG_mountain_people limited_conscription }"),
    ),
    (
        K::CountryHistory,
        "recruit_character",
        a(
            "Hires a character for the country. A character (Characters page) belongs to no country until one recruits it; then its roles count: a country_leader becomes the leader of the party of its ideology (and leads the country while that party rules), a general or an admiral joins the army or the navy, an advisor can be hired on the politics screen, a scientist works on special projects. The value is the character's id. A character nobody recruits is never in the game.",
            "recruit_character = TAG_andreas_hofer",
        ),
    ),
    (
        K::CountryHistory,
        "set_country_flag",
        a(
            "Sets a flag: a named marker the country carries. It does nothing by itself; your script asks for it with has_country_flag = <name> in a condition (a focus's available, an event's trigger, a decision's visible) and removes it with clr_country_flag. Use it to remember that something has happened or holds for this country. The name is one you make up (letters, digits and _). The block form gives it a number or a time limit: { flag = name value = 1 days = 30 }.",
            "set_country_flag = TAG_tyrol_united",
        ),
    ),
    (
        K::CountryHistory,
        "add_to_tech_sharing_group",
        a(
            "Makes the country a member of a technology sharing group. A member researches a technology faster for every other member that has it already (by research_sharing_per_country_bonus of the group, mostly 10%). The value is a group's id; groups are defined in common/technology_sharing (the list beside the field has the base game's and yours). The group's available decides who may stay in it.",
            "add_to_tech_sharing_group = TAG_alpine_research",
        ),
    ),
    (
        K::CountryHistory,
        "set_variable",
        a("Sets variables of the country: a name and a number. Script reads them with check_variable, changes them with add_to_variable, and a text shows one with [?name].", "set_variable = { TAG_rifles = 100 }"),
    ),
    (
        K::CountryHistory,
        "create_faction",
        a(
            "Founds a faction (an alliance like the Allies or the Axis) with this country as its leader. It is not yes or no: the value is the faction's name as a localisation key. Write a key such as TAG_alpine_league and give it its text on the Localisation page (the faction's name in the game). Others join with add_to_faction = <their tag> in this country's history. With the Deeper Factions DLC the game makes the faction from its default template; create_faction_from_template (in an effect) chooses another.",
            "create_faction = TAG_alpine_league",
        ),
    ),
    (
        K::CountryHistory,
        "add_to_faction",
        a("Puts another country into this country's faction (found it first with create_faction). The value is the other country's tag; it must exist at the start.", "add_to_faction = BOZ"),
    ),
    (
        K::CountryHistory,
        "give_guarantee",
        a("This country guarantees the independence of another: when a country declares war on the guaranteed one, the guarantor goes to war against it. The value is the guaranteed country's tag.", "give_guarantee = BOZ"),
    ),
    (
        K::CountryHistory,
        "diplomatic_relation",
        a("A relation with another country at the start: relation is what it is (military_access: its troops may cross the other's land, docking_rights, non_aggression_pact, guarantee, ...), country the other's tag; active = no ends one.", "diplomatic_relation = {\n\tcountry = BOZ\n\trelation = military_access\n\tactive = yes\n}"),
    ),
    (
        K::CountryHistory,
        "set_autonomy",
        a("Makes another country this country's subject: target is the subject's tag, autonomy_state the level (autonomy_puppet, autonomy_dominion, autonomy_colony, ... or one of the Autonomy levels page), freedom_level from 0 to 1 how near it is to the next level.", "set_autonomy = {\n\ttarget = BOZ\n\tautonomy_state = autonomy_puppet\n}"),
    ),
    // ---------------------------------------------------------------- characters
    (K::Character, "name", a("The character's name: a localisation key (with its text on the Localisation page), or the name itself.", "name = TAG_andreas_hofer")),
    (
        K::Character,
        "portraits",
        a("The character's pictures: civilian for a leader or an advisor (large on the politics screen, small in lists), army for a general, navy for an admiral. Each is a sprite. Without them the game draws a generated face.", "portraits = {\n\tcivilian = {\n\t\tlarge = GFX_portrait_TAG_hofer\n\t\tsmall = GFX_idea_TAG_hofer\n\t}\n}"),
    ),
    (K::Character, "gender", a("male or female: which generated portraits and words the game uses for the character.", "gender = male")),
    (
        K::Character,
        "country_leader",
        a("Makes the character someone who can lead the country: the leader of the party of its ideology (a sub-ideology such as conservatism or despotism). When that party rules, this character is the head of state and its traits apply. The country must recruit the character first.", "country_leader = {\n\tideology = despotism\n\ttraits = { }\n\texpire = \"1965.1.1.1\"\n}"),
    ),
    (
        K::Character,
        "corps_commander",
        a("Makes the character a general, who leads an army of up to 24 divisions. skill is the level (1 to 9); attack, defense, planning and logistics skills add to it; traits give bonuses.", "corps_commander = {\n\tskill = 3\n\tattack_skill = 3\n\tdefense_skill = 2\n\tplanning_skill = 3\n\tlogistics_skill = 2\n}"),
    ),
    (K::Character, "field_marshal", a("Makes the character a field marshal, who commands several armies (an army group). Its skills are like a general's.", "field_marshal = {\n\tskill = 4\n\tattack_skill = 3\n\tdefense_skill = 4\n\tplanning_skill = 3\n\tlogistics_skill = 3\n}")),
    (K::Character, "navy_leader", a("Makes the character an admiral, who leads a fleet. skill is the level; attack, defense, maneuvering and coordination skills add to it.", "navy_leader = {\n\tskill = 2\n\tattack_skill = 2\n\tdefense_skill = 2\n\tmaneuvering_skill = 2\n\tcoordination_skill = 2\n}")),
    (
        K::Character,
        "advisor",
        a("Makes the character an advisor the country can hire for political power: slot is where (political_advisor, high_command, theorist, army_chief, ...), idea_token the id it has as an idea, cost what hiring costs, allowed who may hire it, traits what it gives.", "advisor = {\n\tslot = political_advisor\n\tidea_token = TAG_hofer_advisor\n\tcost = 150\n\tallowed = { original_tag = TAG }\n\ttraits = { silent_workhorse }\n}"),
    ),
    (K::Character, "scientist", a("Makes the character a scientist, who works on special projects; its skill and traits make them faster.", "scientist = {\n\ttraits = { }\n\tskills = { }\n}")),
    // ---------------------------------------------------------------- focuses
    (K::FocusTree, "country", a("Which countries get this tree. Every country takes the tree with the highest factor; a modifier adds to it when its conditions hold. factor = 0 and a modifier with add = 10 and tag = TAG gives it to that country alone.", "country = {\n\tfactor = 0\n\tmodifier = {\n\t\tadd = 10\n\t\ttag = TAG\n\t}\n}")),
    (K::FocusTree, "default", a("The tree of countries no other tree fits (the generic tree). Only one tree should have it.", "default = no")),
    (K::Focus, "cost", a("How long the focus takes, in weeks of 7 days: cost = 10 is 70 days, the usual length.", "cost = 10")),
    (K::Focus, "prerequisite", a("Focuses that must be done first. Within one block any of them is enough (or); each further prerequisite block must be met as well (and). The tree draws lines from them.", "prerequisite = { focus = TAG_a focus = TAG_b }")),
    (K::Focus, "mutually_exclusive", a("Focuses that cannot be taken together with this one: once one of them is taken the others are locked. The other focuses should list this one too.", "mutually_exclusive = { focus = TAG_other_way }")),
    (K::Focus, "available", a("Conditions for starting the focus, besides its prerequisites. While they fail it is greyed out.", "available = {\n\thas_war = no\n}")),
    (K::Focus, "bypass", a("When this holds the focus can be skipped: it counts as done, without its reward, and the focuses after it open.", "bypass = {\n\thas_completed_focus = TAG_other_way\n}")),
    (K::Focus, "allow_branch", a("The focus, and every focus that needs it, is in the tree only when this holds; checked when the game starts. Used for DLC: has_dlc = \"...\".", "allow_branch = {\n\thas_dlc = \"La Resistance\"\n}")),
    (K::Focus, "will_lead_to_war_with", a("Tells the AI that this focus leads to a war with that country, so it gets ready for it. It starts no war by itself.", "will_lead_to_war_with = BOZ")),
    (K::Focus, "select_effect", a("Effects run when the focus is started (not when it is done).", "select_effect = {\n\tadd_political_power = -25\n}")),
    (K::Focus, "completion_reward", a("Effects run when the focus is done: what the focus gives. Most of a focus's script goes here.", "completion_reward = {\n\tadd_political_power = 120\n\tadd_ideas = TAG_mountain_people\n}")),
    (K::Focus, "complete_tooltip", a("Effects shown in the focus's tooltip instead of those of completion_reward, without being run. For a reward that is hidden or too long to show.", "complete_tooltip = {\n\tadd_political_power = 120\n}")),
    (K::Focus, "search_filters", a("The filters of the focus search in the game's tree view this focus shows under.", "search_filters = { FOCUS_FILTER_POLITICAL }")),
    // ---------------------------------------------------------------- events
    (K::Event, "is_triggered_only", a("The event never fires by itself: something fires it, such as country_event = { id = x } in a focus, a decision, another event's option or an on_action. Most events are like this.", "is_triggered_only = yes")),
    (K::Event, "trigger", a("Conditions for the event. For an event that is not is_triggered_only the game checks them again and again, and the event fires by itself once they hold (how soon: mean_time_to_happen).", "trigger = {\n\ttag = TAG\n\thas_war = yes\n}")),
    (K::Event, "immediate", a("Effects run the moment the event fires, before an option is picked.", "immediate = {\n\thidden_effect = { set_country_flag = TAG_event_seen }\n}")),
    (K::Event, "option", a("A choice: a button of the event. name is its text (a localisation key), then the effects run when it is picked; trigger decides who sees it, ai_chance how likely the AI picks it. An event needs at least one.", "option = {\n\tname = TAG_event.1.a\n\tadd_political_power = 50\n}")),
    // ---------------------------------------------------------------- decisions and ideas
    (K::Decision, "days_remove", a("Makes the decision run for this many days after it is taken; then remove_effect runs.", "days_remove = 30")),
    (K::Decision, "complete_effect", a("Effects run the moment the decision is taken.", "complete_effect = {\n\tadd_stability = 0.05\n}")),
    (K::Decision, "target_trigger", a("Targeted decisions: the conditions on each target (FROM is the target). The decision is offered once for each target that passes.", "target_trigger = {\n\tFROM = { is_neighbor_of = ROOT }\n}")),
    (K::Idea, "allowed_civil_war", a("When a civil war splits the country, the side that breaks away keeps the idea as well when this holds.", "allowed_civil_war = {\n\talways = yes\n}")),
    (K::Idea, "targeted_modifier", a("Modifiers that apply only against one country: tag = the country, then the modifiers.", "targeted_modifier = {\n\ttag = BOZ\n\tattack_bonus_against = 0.1\n}")),
    (K::Idea, "research_bonus", a("Faster research in technology categories: a category and how much (0.1 is 10% faster).", "research_bonus = {\n\tinfantry_weapons = 0.1\n}")),
    (K::Idea, "rule", a("Rules: what a country with this idea may or may not do, yes or no each (can_create_factions, can_join_factions, can_guarantee_other_ideologies, ...).", "rule = {\n\tcan_create_factions = no\n}")),    // ---------------------------------------------------------------- focus trees and focuses
    (K::FocusTree, "reset_on_civilwar", a("In a civil war, whether the side that breaks away starts this tree over (yes, the default) or keeps the focuses done so far (no).", "reset_on_civilwar = no")),
    (K::FocusTree, "shared_focus", a("Puts a branch of shared focuses into this tree: the id of the shared focus the branch starts with. Shared focuses are written once and can be in several trees.", "shared_focus = TAG_alpine_cooperation")),
    (K::FocusTree, "continuous_focus_position", a("Where the box of continuous focuses (the ones kept running for a bonus) is drawn on the tree's screen, in pixels from its top left corner.", "continuous_focus_position = { x = 50 y = 1200 }")),
    (K::FocusTree, "initial_show_position", a("Where the tree's screen is scrolled to when it opens: at a focus (focus = id), or at a place (x and y).", "initial_show_position = { focus = TAG_first_focus }")),
    (K::Focus, "x", a("The focus's column on the tree, one focus wide each. With relative_position_id it counts from that focus's column: x = -1 is one column to its left.", "x = 4")),
    (K::Focus, "y", a("The focus's row on the tree: 0 is the top row, one focus high each. With relative_position_id it counts from that focus's row.", "y = 1")),
    (K::Focus, "relative_position_id", a("The focus that x and y count from: when that focus moves, this one moves with it. Usually its prerequisite.", "relative_position_id = TAG_first_focus")),
    (K::Focus, "cancel_if_invalid", a("While the focus is being worked on, it is cancelled when its available no longer holds (yes, the default); no lets it go on.", "cancel_if_invalid = no")),
    (K::Focus, "continue_if_invalid", a("The focus goes on being worked on even when its available no longer holds.", "continue_if_invalid = yes")),
    (K::Focus, "available_if_capitulated", a("The focus can be taken while the country has capitulated (fighting on in exile); otherwise it cannot.", "available_if_capitulated = yes")),
    (K::Focus, "cancelable", a("Whether the player can cancel the focus while it is being worked on (yes, the default).", "cancelable = no")),
    (K::Focus, "text", a("Another localisation key for the focus's name (and that key with _desc for its description), instead of the focus's id. Lets several focuses share a text.", "text = TAG_build_roads")),
    (K::Focus, "offset", a("Moves the focus on the tree while a condition holds: x and y are added to its place when trigger holds. Used to make room when another branch shows.", "offset = {\n\tx = 2\n\ty = 0\n\ttrigger = {\n\t\thas_completed_focus = TAG_other_way\n\t}\n}")),
    (K::Focus, "ai_will_do", a("How much the AI wants this focus when it picks the next one; only the AI reads it. A base factor and modifiers: factor = 0 in a modifier keeps the AI off it while its conditions hold.", "ai_will_do = {\n\tfactor = 10\n\tmodifier = {\n\t\tfactor = 0\n\t\thas_war = yes\n\t}\n}")),
    // ---------------------------------------------------------------- events
    (K::Event, "picture", a("The picture at the top of the event window: a sprite, GFX_report_event_... pick… shows them with their pictures.", "picture = GFX_report_event_generic_rally")),
    (K::Event, "title", a("The event's title: a localisation key. Several title blocks, each with a trigger and a text, show the first whose trigger holds.", "title = TAG_event.1.t")),
    (K::Event, "desc", a("The event's text: a localisation key. Several desc blocks, each with a trigger and a text, show the first whose trigger holds.", "desc = TAG_event.1.d")),
    (K::Event, "fire_only_once", a("The event fires at most once per game for a country; later attempts do nothing.", "fire_only_once = yes")),
    (K::Event, "hidden", a("Nobody sees the event: its immediate effects and its first option run without a window. For script that has to run as an event (a delay, an on_action).", "hidden = yes")),
    (K::Event, "major", a("A news event: it is shown to every country, not only to the one it fires for.", "major = yes")),
    (K::Event, "show_major", a("For a major event: the countries (each as ROOT) that see it as big news; the others get no window.", "show_major = {\n\thas_war = yes\n}")),
    (K::Event, "mean_time_to_happen", a("For an event that fires by itself (not is_triggered_only): about how long after its trigger starts to hold it fires, in days, months or years, with modifiers that make it sooner or later.", "mean_time_to_happen = {\n\tdays = 30\n}")),
    (K::Event, "timeout_days", a("How many days the event waits for a choice; then the AI picks an option for the player.", "timeout_days = 7")),
    // ---------------------------------------------------------------- decisions
    (K::Decision, "icon", a("The decision's icon: GFX_decision_<icon>, or a whole GFX_ name. pick… shows them with their pictures.", "icon = generic_political_discourse")),
    (K::Decision, "cost", a("Political power the decision costs; it is paid when the decision is taken.", "cost = 50")),
    (K::Decision, "fire_only_once", a("The decision can be taken only once per game; afterwards it is gone.", "fire_only_once = yes")),
    (K::Decision, "days_re_enable", a("After it was taken, how many days until it can be taken again.", "days_re_enable = 90")),
    (K::Decision, "remove_effect", a("Effects run when a timed decision's days_remove are over.", "remove_effect = {\n\tadd_stability = -0.05\n}")),
    (K::Decision, "timeout_effect", a("Missions: effects run when the mission's time runs out before it is done.", "timeout_effect = {\n\tadd_political_power = -50\n}")),
    (K::Decision, "cancel_trigger", a("While a timed decision or a mission runs, it is cancelled when this holds.", "cancel_trigger = {\n\thas_war = yes\n}")),
    (K::Decision, "targets", a("Targeted decisions: the countries tried as targets (tags, or an array); target_trigger then picks among them. Without it every country is tried.", "targets = { BOZ MER }")),
    (K::Decision, "allowed", a("Checked once, when the game starts or a save loads: a country for which it fails never has the decision. For the tag or a DLC; what changes goes in visible or available.", "allowed = {\n\toriginal_tag = TAG\n}")),
    (K::Decision, "visible", a("While this does not hold the decision is not in the list.", "visible = {\n\thas_completed_focus = TAG_first_focus\n}")),
    (K::Decision, "available", a("While this does not hold the decision is in the list, greyed out, and cannot be taken.", "available = {\n\thas_war = no\n}")),
    (K::DecisionCategory, "icon", a("The category's icon: GFX_decision_category_<icon>, or a whole GFX_ name.", "icon = generic_political_actions")),
    (K::DecisionCategory, "picture", a("The picture at the top of the category in the decisions screen: a sprite.", "picture = GFX_decision_cat_picture_generic")),
    (K::DecisionCategory, "priority", a("The order of the categories in the decisions screen: higher comes first.", "priority = 100")),
    (K::DecisionCategory, "visibility_type", a("Where the category shows: map_and_decisions_view (the map and the decisions screen), map_only, or decisions_view_only.", "visibility_type = map_and_decisions_view")),
    (K::DecisionCategory, "allowed", a("Checked once, when the game starts or a save loads: a country for which it fails never has the category.", "allowed = {\n\toriginal_tag = TAG\n}")),
    (K::DecisionCategory, "visible", a("While this does not hold the category, with its decisions, is hidden.", "visible = {\n\thas_war = no\n}")),
    // ---------------------------------------------------------------- ideas
    (K::Idea, "picture", a("The idea's picture: GFX_idea_<picture>, or a whole GFX_ name. pick… shows them with their pictures.", "picture = TAG_mountain_people")),
    (K::Idea, "name", a("Another localisation key for the idea's name, instead of its id.", "name = TAG_mountain_people_name")),
    (K::Idea, "equipment_bonus", a("Makes equipment better while the idea applies: for an equipment (or a family of it), the stats it adds, such as soft_attack = 0.1 (10% more).", "equipment_bonus = {\n\tinfantry_equipment = {\n\t\tsoft_attack = 0.1\n\t\tinstant = yes\n\t}\n}")),
    (K::Idea, "removal_cost", a("Political power it costs the player to remove the idea; -1: it cannot be removed.", "removal_cost = -1")),
    (K::Idea, "cost", a("Political power it costs to adopt the idea: a law, or an idea the player picks.", "cost = 150")),
    (K::Idea, "default", a("The law of its category a country has when it has none of them (volunteer_only among the conscription laws).", "default = yes")),
    // ---------------------------------------------------------------- divisions
    (K::DivisionTemplate, "division_names_group", a("The list the template's divisions take their names from (a group of common/units/names_divisions); without it they are only numbered.", "division_names_group = GER_INF_01")),
    (K::DivisionTemplate, "regiments", a("The combat battalions: one line each, the battalion = { x = column y = row }. x is the column (0 to 4), y the row in it. The wand places them on the grid.", "regiments = {\n\tinfantry = { x = 0 y = 0 }\n\tinfantry = { x = 0 y = 1 }\n}")),
    (K::DivisionTemplate, "support", a("Support companies (engineers, recon, artillery, ...): the company = { x = 0 y = row }, one column of up to five.", "support = {\n\tengineer = { x = 0 y = 0 }\n}")),
    (K::DivisionTemplate, "priority", a("Reinforcement priority: 0 low, 1 normal, 2 high. Divisions of a higher priority get new equipment and soldiers first.", "priority = 1")),
    (K::DivisionTemplate, "is_locked", a("The player cannot change the template in the division designer.", "is_locked = yes")),
    // ---------------------------------------------------------------- traits
    (K::CountryLeaderTrait, "random", a("Whether generated leaders and advisors can get the trait by chance; no for traits made for one character.", "random = no")),
    (K::CountryLeaderTrait, "sprite", a("Which icon of the traits' picture it shows: a frame number.", "sprite = 1")),
    (K::CountryLeaderTrait, "targeted_modifier", a("Modifiers that apply only against one country while the leader or advisor is in office: tag = the country, then the modifiers.", "targeted_modifier = {\n\ttag = BOZ\n\tattack_bonus_against = 0.1\n}")),
    (K::CountryLeaderTrait, "research_bonus", a("Faster research in technology categories while the leader or advisor is in office: a category and how much (0.1 is 10% faster).", "research_bonus = {\n\tinfantry_weapons = 0.1\n}")),
    (K::CountryLeaderTrait, "equipment_bonus", a("Better equipment while the leader or advisor is in office: for an equipment (or a family of it), the stats it adds.", "equipment_bonus = {\n\tinfantry_equipment = {\n\t\tsoft_attack = 0.05\n\t}\n}")),
    (K::CountryLeaderTrait, "command_cap_increase", a("How much more command power the country can store.", "command_cap_increase = 20")),
    (K::CountryLeaderTrait, "modifier", a("Modifiers of the country while the leader or advisor with the trait is in office, such as political_power_gain = 0.1.", "modifier = {\n\tpolitical_power_gain = 0.1\n}")),
    (K::CountryLeaderTrait, "ai_will_do", a("How much the AI wants an advisor with this trait; only the AI reads it.", "ai_will_do = {\n\tfactor = 1\n}")),
    (K::UnitLeaderTrait, "type", a("Who can have it: land (generals and field marshals), navy (admirals), corps_commander, field_marshal or all.", "type = land")),
    (K::UnitLeaderTrait, "trait_type", a("What kind of trait: personality_trait (given by events and focuses), status_trait (such as wounded), assignable_trait (the player picks it with experience), basic_terrain_trait and assignable_terrain_trait (terrain experts), basic_trait.", "trait_type = personality_trait")),
    (K::UnitLeaderTrait, "gui_row", a("The row of the trait in the tree of traits the player unlocks, from 0; left out or -1: not in that tree.", "gui_row = 0")),
    (K::UnitLeaderTrait, "gui_column", a("The column of the trait in the tree of traits the player unlocks, from 0; left out or -1: placed by the game.", "gui_column = 0")),
    (K::UnitLeaderTrait, "attack_skill", a("Added to the leader's attack skill (a whole number).", "attack_skill = 1")),
    (K::UnitLeaderTrait, "defense_skill", a("Added to the leader's defense skill (a whole number).", "defense_skill = 1")),
    (K::UnitLeaderTrait, "modifier", a("Modifiers of the troops the leader leads, such as army_attack_factor = 0.05.", "modifier = {\n\tarmy_attack_factor = 0.05\n}")),
    (K::UnitLeaderTrait, "allowed", a("Which leaders can have it: conditions on the unit leader.", "allowed = {\n\thas_trait = brilliant_strategist\n}")),
    (K::UnitLeaderTrait, "ai_will_do", a("How much the AI wants the trait for its leaders; only the AI reads it.", "ai_will_do = {\n\tfactor = 1\n}")),
    (K::ScientistTrait, "modifier", a("Modifiers while a scientist with the trait works on a special project.", "modifier = {\n\tscientist_research_bonus_factor = 0.1\n}")),
    // ---------------------------------------------------------------- opinions, scripted things, on_actions
    (K::OpinionModifier, "value", a("How much it changes one country's opinion of another: 50 is a lot better, -30 worse. The AI's opinion decides what it agrees to.", "value = 50")),
    (K::OpinionModifier, "decay", a("How much of the value fades each day, towards 0.", "decay = 1")),
    (K::OpinionModifier, "min_trust", a("When the modifier is added again and the values add up: the lowest the sum can go.", "min_trust = -50")),
    (K::OpinionModifier, "max_trust", a("When the modifier is added again and the values add up: the highest the sum can go.", "max_trust = 100")),
    (K::OpinionModifier, "days", a("How long it lasts, in days; with no days, months or years it lasts until remove_opinion_modifier.", "days = 180")),
    (K::OpinionModifier, "months", a("How long it lasts, in months.", "months = 6")),
    (K::OpinionModifier, "years", a("How long it lasts, in years.", "years = 1")),
    (K::OpinionModifier, "trade", a("A trade opinion: it counts for trade (who sells resources to whom), not for diplomacy.", "trade = yes")),
    (K::OpinionModifier, "target", a("The game's own guarantee modifier has a version with target = yes; modifiers added with add_opinion_modifier do not need it.", "target = yes")),
    (K::ScriptedLoc, "text", a("One text the scripted localisation can give: trigger is when, localization_key the text. The first text whose trigger holds is used; one without a trigger is the fallback.", "text = {\n\ttrigger = { has_war = yes }\n\tlocalization_key = TAG_at_war\n}")),
    (K::DynamicModifier, "remove_trigger", a("The modifier is taken off the country when this holds.", "remove_trigger = {\n\thas_war = no\n}")),
    (K::DynamicModifier, "enable", a("The modifier stays on the country but works only while this holds.", "enable = {\n\thas_war = yes\n}")),
    (K::OnAction, "effect", a("Effects run each time it happens, in its scope (on_startup: each country when the game starts).", "effect = {\n\tif = {\n\t\tlimit = { tag = TAG }\n\t\tcountry_event = TAG_event.1\n\t}\n}")),
    (K::OnAction, "random_events", a("Events of which one fires at random each time it happens: weight = event id. 0 = 0 is a weight for none at all.", "random_events = {\n\t100 = TAG_event.1\n\t100 = 0\n}")),
    // ---------------------------------------------------------------- AI
    (K::AiStrategy, "ai_strategy", a("One wish of the AI: type is what (befriend, antagonize, alliance, conquer, protect, invade, build_army, ...), id the country or thing it is about, value how strong (more: stronger; negative: the other way).", "ai_strategy = {\n\ttype = befriend\n\tid = BOZ\n\tvalue = 100\n}")),
    (K::AiStrategy, "allowed", a("Checked once, when the game starts: the countries that have these wishes at all.", "allowed = {\n\toriginal_tag = TAG\n}")),
    (K::AiStrategy, "enable", a("The wishes count while this holds; the game checks it again and again.", "enable = {\n\thas_war = no\n}")),
    (K::AiStrategy, "abort", a("The wishes stop when this holds.", "abort = {\n\thas_war = yes\n}")),
    (K::AiStrategyPlan, "name", a("A name for the plan (text; shown only in the game's debug view).", "name = \"Tyrol neutral plan\"")),
    (K::AiStrategyPlan, "desc", a("What the plan is about (text; for you).", "desc = \"Stays out of wars\"")),
    (K::AiStrategyPlan, "allowed", a("Checked once, when the game starts: the countries that can follow the plan.", "allowed = {\n\toriginal_tag = TAG\n}")),
    (K::AiStrategyPlan, "enable", a("When the AI takes up the plan (among the plans that fit, by weight).", "enable = {\n\thas_government = neutrality\n}")),
    (K::AiStrategyPlan, "abort", a("The AI drops the plan when this holds.", "abort = {\n\thas_war = yes\n}")),
    (K::AiStrategyPlan, "ai_national_focuses", a("The focuses the AI takes while it follows the plan, in this order.", "ai_national_focuses = {\n\tTAG_first_focus\n\tTAG_second_focus\n}")),
    (K::AiStrategyPlan, "research", a("Research the AI weighs while it follows the plan: a technology category = a factor (more: sooner).", "research = {\n\tinfantry_weapons = 2\n}")),
    (K::AiStrategyPlan, "ideas", a("Ideas (laws, advisors) the AI wants while it follows the plan: id = factor.", "ideas = {\n\tlimited_conscription = 5\n}")),
    (K::AiStrategyPlan, "weight", a("How strong the plan is against other plans that fit: a factor with modifiers, about 1.", "weight = {\n\tfactor = 1\n}")),
    // ---------------------------------------------------------------- war goals, ideologies
    (K::WarGoal, "war_name", a("The name of wars fought over it: a localisation key.", "war_name = TAG_war_for_tyrol")),
    (K::WarGoal, "generate_base_cost", a("How long justifying it takes before world tension and modifiers change it.", "generate_base_cost = 200")),
    (K::WarGoal, "take_states_limit", a("How many states the war goal lets the winner take.", "take_states_limit = 5")),
    (K::WarGoal, "allowed", a("Which countries can have this war goal at all.", "allowed = {\n\toriginal_tag = TAG\n}")),
    (K::WarGoal, "available", a("Against whom it can be justified: ROOT is the country that justifies it, PREV the target.", "available = {\n\tPREV = { is_neighbor_of = ROOT }\n}")),
    (K::WarGoal, "on_add", a("Effects for when the war goal is gained (the base game's war goals use none).", "on_add = {\n\tadd_political_power = 10\n}")),
    (K::Ideology, "types", a("The sub-ideologies (leader ideologies) of this ideology: a country leader has one of them, such as despotism or conservatism. One block each.", "types = {\n\tTAG_tyrolean_loyalism = { }\n}")),
    (K::Ideology, "dynamic_faction_names", a("Names a faction founded by a country of this ideology can get: localisation keys.", "dynamic_faction_names = {\n\t\"FACTION_NAME_TAG_1\"\n}")),
    (K::Ideology, "color", a("The ideology's colour in the map's ideology mode and the charts: red, green and blue, 0 to 255.", "color = { 120 160 60 }")),
    (K::Ideology, "rules", a("What countries ruled by it may do, yes or no each: can_force_government, can_send_volunteers, can_puppet, can_lower_tension, ...", "rules = {\n\tcan_send_volunteers = yes\n}")),
    (K::Ideology, "modifiers", a("Modifiers of every country ruled by it.", "modifiers = {\n\tgenerate_wargoal_tension = 0.5\n}")),
    (K::Ideology, "faction_modifiers", a("Modifiers of factions led by its countries.", "faction_modifiers = {\n\tfaction_trade_opinion_factor = 0.5\n}")),
    (K::Ideology, "can_host_government_in_exile", a("Its countries can host governments in exile.", "can_host_government_in_exile = yes")),
    (K::Ideology, "can_be_boosted", a("Other countries can boost its popularity in a country (the diplomatic action).", "can_be_boosted = yes")),
    (K::Ideology, "can_collaborate", a("Collaboration governments of this ideology can be set up.", "can_collaborate = yes")),
    (K::Ideology, "ai_ideology_wanted_units_factor", a("AI: how many more units its countries want (1: as many as usual).", "ai_ideology_wanted_units_factor = 1.1")),
    // ---------------------------------------------------------------- technologies, equipment, units
    (K::Technology, "research_cost", a("How long it takes to research: 1 is about 100 days for a country with no bonuses.", "research_cost = 1.5")),
    (K::Technology, "start_year", a("The year it is meant for: researching it before that costs more (ahead of time), after it less.", "start_year = 1938")),
    (K::Technology, "folder", a("Which tab of the research screen it is in (infantry_folder, armour_folder, ...) and where in it (x, y).", "folder = {\n\tname = infantry_folder\n\tposition = { x = 0 y = 2 }\n}")),
    (K::Technology, "path", a("What it leads to: leads_to_tech is the next technology, research_cost_coeff multiplies that one's cost. One path for each; the screen draws lines along them.", "path = {\n\tleads_to_tech = TAG_mountain_rifles2\n\tresearch_cost_coeff = 1\n}")),
    (K::Technology, "categories", a("Its categories: research bonuses (of ideas, focuses, advisors) apply by category, and the AI weighs research by them.", "categories = { infantry_weapons }")),
    (K::Technology, "enable_equipments", a("Equipment the technology unlocks for production.", "enable_equipments = { TAG_mountain_rifle }")),
    (K::Technology, "enable_subunits", a("Battalions and companies it unlocks for division templates.", "enable_subunits = { mountaineers }")),
    (K::Technology, "allow", a("Conditions for researching it (ROOT the country); while they fail it is shown but cannot be researched.", "allow = {\n\ttag = TAG\n}")),
    (K::Technology, "allow_branch", a("The technology, and those after it, are in the research screen only when this holds (for DLC).", "allow_branch = {\n\thas_dlc = \"No Step Back\"\n}")),
    (K::Technology, "ai_will_do", a("How much the AI wants to research it; only the AI reads it.", "ai_will_do = {\n\tfactor = 1\n}")),
    (K::Equipment, "year", a("The model's year, shown in the production and design screens.", "year = 1938")),
    (K::Equipment, "is_archetype", a("An archetype: the family of models (infantry_equipment), not a model that is produced. Models name it with archetype and take its stats as their base.", "is_archetype = yes")),
    (K::Equipment, "archetype", a("The family the model belongs to: the stats it does not give come from there.", "archetype = infantry_equipment")),
    (K::Equipment, "parent", a("The older model this one follows: factories can switch to it and divisions take it instead.", "parent = infantry_equipment_1")),
    (K::Equipment, "priority", a("Which model divisions take first when several fit: higher first.", "priority = 10")),
    (K::Equipment, "build_cost_ic", a("What one unit costs to produce, in factory output (industrial capacity).", "build_cost_ic = 0.5")),
    (K::Equipment, "reliability", a("How seldom it breaks down, 0 to 1 (0.9: 90%); lower means more losses to attrition and accidents.", "reliability = 0.9")),
    (K::Equipment, "resources", a("Resources a factory producing it uses: steel, oil, aluminium, rubber, tungsten, chromium.", "resources = {\n\tsteel = 1\n}")),
    (K::Equipment, "type", a("What kind of equipment it is (infantry, support, armor, artillery, anti_tank, ...): modifiers of that type apply to it.", "type = infantry")),
    (K::SubUnit, "sprite", a("The model shown on the map for the battalion (an entity name).", "sprite = infantry")),
    (K::SubUnit, "map_icon_category", a("The icon a division counter shows on the map: infantry, armored, other, ship ...", "map_icon_category = infantry")),
    (K::SubUnit, "priority", a("Which battalion decides how a division looks on the map: the one with the highest priority gives its icon and model.", "priority = 600")),
    (K::SubUnit, "type", a("Its types (infantry, armor, artillery, ...): modifiers of a type apply to it.", "type = { infantry }")),
    (K::SubUnit, "group", a("Where it is in the division designer: infantry, mobile, armor, support, ...", "group = infantry")),
    (K::SubUnit, "categories", a("Its categories (category_front_line, category_light_infantry, ...): doctrines and modifiers of a category apply to it.", "categories = { category_front_line category_light_infantry category_all_infantry category_army }")),
    (K::SubUnit, "combat_width", a("How much of a battle's front it takes: a division's combat width is the sum of its battalions'.", "combat_width = 2")),
    (K::SubUnit, "manpower", a("Soldiers it needs.", "manpower = 1000")),
    (K::SubUnit, "max_organisation", a("Its organisation: how long it fights before it has to fall back; a division has the average of its battalions'.", "max_organisation = 60")),
    (K::SubUnit, "need", a("Equipment it needs to be at full strength: equipment = amount.", "need = {\n\tinfantry_equipment = 100\n}")),
    // ---------------------------------------------------------------- the rest
    (K::Bookmark, "desc", a("The start's description: a localisation key.", "desc = TAG_START_DESC")),
    (K::Bookmark, "date", a("The date the game starts on: year.month.day.hour.", "date = 1936.1.1.12")),
    (K::Bookmark, "default_country", a("The country selected when the start is picked.", "default_country = \"TAG\"")),
    (K::Bookmark, "default", a("This start is the one selected when the start screen opens.", "default = yes")),
    (K::PeaceAi, "annex_randomness", a("How much chance changes whether the AI annexes at a peace conference (0: always the same choice).", "annex_randomness = 0")),
    (K::PeaceAi, "liberate_randomness", a("How much chance changes whether the AI liberates countries at a peace conference.", "liberate_randomness = 0")),
    (K::PeaceCost, "peace_action_type", a("The peace conference actions whose cost it changes: take_states, puppet, liberate, force_government, ...", "peace_action_type = { take_states }")),
    (K::PeaceCost, "cost_multiplier", a("The actions' cost at the peace conference is multiplied by this while enable holds (0.5: half).", "cost_multiplier = 0.5")),
    (K::AgencyUpgrade, "frame", a("The frame around the upgrade's picture: a sprite.", "frame = GFX_upgrade_frame_army")),
    (K::AgencyUpgrade, "sound", a("The sound played when the upgrade is picked.", "sound = Spy_Agency_Intel_Upgrades_Confirm")),
    (K::AgencyUpgrade, "level", a("One level of the upgrade: what it gives (modifier) and when it can be taken; the levels are taken one after the other.", "level = {\n\tmodifier = {\n\t\tcivilian_intel_factor = 0.25\n\t}\n}")),
    (K::Autonomy, "min_freedom_level", a("The freedom level (0 to 1) at which a subject is at this level of autonomy.", "min_freedom_level = 0.2")),
    (K::Autonomy, "manpower_influence", a("The share (0 to 1) of the subject's manpower the overlord can use: 0.9 for a puppet, 0.5 for a satellite.", "manpower_influence = 0.9")),
    (K::Autonomy, "rule", a("What subjects at this level may do, yes or no each: can_not_declare_war, can_decline_call_to_war, units_deployed_to_overlord, ...", "rule = {\n\tcan_not_declare_war = yes\n}")),
    (K::Autonomy, "modifier", a("Modifiers of subjects at this level, such as autonomy_manpower_share.", "modifier = {\n\tautonomy_manpower_share = 0.5\n}")),
    (K::Song, "chance", a("How likely the song is played next: a factor with modifiers on the player's country (has_war = no for songs of peace).", "chance = {\n\tmodifier = {\n\t\tfactor = 1\n\t\thas_war = no\n\t}\n}")),
    (K::Names, "male", a("First names of men, for generated generals, admirals and leaders: names = { ... }.", "male = {\n\tnames = { Andreas Josef Peter }\n}")),
    (K::Names, "female", a("First names of women: names = { ... }.", "female = {\n\tnames = { Anna Maria Theresia }\n}")),
    (K::Names, "surnames", a("Surnames of generated characters.", "surnames = { Hofer Haspinger Speckbacher }")),
    (K::Names, "callsigns", a("Callsigns of the country's pilots.", "callsigns = { Adler Gams }")),
    (K::Names, "ship_names", a("Names of the country's ships.", "ship_names = { Innsbruck Bozen }")),
    (K::Sprite, "texturefile", a("The picture file, from the mod's folder: gfx/interface/goals/x.dds (DDS, TGA or PNG).", "texturefile = \"gfx/interface/goals/TAG_goal.dds\"")),
    (K::Sprite, "noOfFrames", a("How many pictures are side by side in the file (a strip of icons); 1 for one picture.", "noOfFrames = 1")),
    (K::Sprite, "legacy_lazy_load", a("Whether the picture is loaded only when it is first shown (no: loaded at the start).", "legacy_lazy_load = no")),
];

/// Fields of the same name in any kind.
const ANY: &[(&str, About)] = &[
    ("allowed", a("Checked once, when the game starts or a save loads: a country for which it fails never has this at all. It is for what does not change, such as the tag or a DLC; what changes belongs in visible or available.", "allowed = {\n\toriginal_tag = TAG\n}")),
    ("visible", a("While this does not hold it is hidden. Conditions on the country (ROOT).", "visible = {\n\thas_war = no\n}")),
    ("available", a("While this does not hold it is shown greyed out and cannot be taken.", "available = {\n\thas_political_power > 50\n}")),
    ("ai_will_do", a("How much the AI wants it; only the AI reads it. A base factor and modifiers: factor = 0 in a modifier stops the AI while its conditions hold. 0: never.", "ai_will_do = {\n\tfactor = 1\n\tmodifier = {\n\t\tfactor = 0\n\t\thas_war = yes\n\t}\n}")),
    ("modifier", a("Modifiers: numbers that change the country while this lasts, such as political_power_gain = 0.25 (0.25 political power more a day) or stability_factor = 0.05 (5% more stability). The wand beside the field lists all of them with what they do.", "modifier = {\n\tpolitical_power_gain = 0.25\n\tstability_factor = 0.05\n}")),
    ("on_add", a("Effects run when it is added.", "on_add = {\n\tadd_stability = 0.05\n}")),
    ("on_remove", a("Effects run when it is removed.", "on_remove = {\n\tadd_stability = -0.05\n}")),
    ("cancel", a("It is removed when this holds.", "cancel = {\n\thas_war = no\n}")),
    ("picture", a("A sprite: a GFX_ name that an interface/*.gfx file gives a picture (the Sprites page). pick… shows them with their pictures.", "picture = GFX_idea_TAG_mountain_people")),
    ("icon", a("A sprite: a GFX_ name that an interface/*.gfx file gives a picture (the Sprites page). pick… shows them with their pictures.", "icon = GFX_goal_generic_political_pressure")),
    ("enable", a("Conditions under which it is active; the game checks them again and again.", "enable = {\n\thas_war = yes\n}")),
    ("abort", a("Conditions under which it stops.", "abort = {\n\thas_war = no\n}")),
];

/// The explanation of `key` in `kind`, else of a field of that name in any kind.
pub fn about(kind: Kind, key: &str) -> Option<&'static About> {
    let kind = if kind == Kind::SharedFocus { Kind::Focus } else { kind };
    OF_KIND.iter().find(|(k, f, _)| *k == kind && *f == key).map(|x| &x.2).or_else(|| ANY.iter().find(|(f, _)| *f == key).map(|x| &x.1))
}

/// What a field takes, in words.
pub fn value_of(ty: FieldType) -> String {
    match ty {
        FieldType::Word => "a word: letters, digits and _, no spaces".into(),
        FieldType::Text => "text in quotes".into(),
        FieldType::Number => "a number".into(),
        FieldType::Bool => "yes or no".into(),
        FieldType::Block => "a block: lines of script in braces { … } (the wand builds it)".into(),
        FieldType::Sprite(p) if !p.is_empty() => format!("a sprite: a GFX_ name, or the end of one ({p}<name>)"),
        FieldType::Sprite(_) => "a sprite: a GFX_ name".into(),
        FieldType::Ref(k) => format!("the id of a {} ({} page)", k.spec().one, k.spec().label),
        FieldType::Loc => "a localisation key: its text, in each language, is on the Localisation page".into(),
        FieldType::Tag => "a country tag: three capital letters or digits (TYR)".into(),
        FieldType::State => "a state of the map, by its number (the map button picks it)".into(),
        FieldType::File(folder) => format!("the name of a file in {folder}, without .txt"),
        FieldType::IdIn(folder, block) => format!("the id of a {block} of {folder}"),
    }
}

/// The field written in a file: its example, else one made from its type,
/// with the nation's tag for `TAG`.
pub fn example(kind: Kind, f: &Field, tag: &str) -> String {
    if let Some(x) = about(kind, f.key) {
        return x.example.replace("TAG", tag);
    }
    let v = match f.ty {
        FieldType::Word => "x".to_string(),
        FieldType::Text => "\"…\"".into(),
        FieldType::Number => "1".into(),
        FieldType::Bool => "yes".into(),
        // the help says how a block goes when it starts with it
        FieldType::Block => {
            // up to the brace that closes the first
            let start = f.help.find('{');
            let mut depth = 0;
            let end = start.and_then(|i| {
                f.help[i..].char_indices().find_map(|(j, c)| {
                    depth += match c {
                        '{' => 1,
                        '}' => -1,
                        _ => 0,
                    };
                    (depth == 0).then_some(i + j + 1)
                })
            });
            match (start, end) {
                (Some(i), Some(e)) => f.help[i..e].to_string(),
                _ => "{ … }".into(),
            }
        }
        FieldType::Sprite(p) => format!("{}example", if p.is_empty() { "GFX_" } else { p }),
        FieldType::Ref(k) => format!("{tag}_{}", k.spec().one.replace(' ', "_")),
        FieldType::Loc => format!("{tag}_{}", f.key),
        FieldType::Tag => tag.to_string(),
        FieldType::State => "12".into(),
        FieldType::File(_) => format!("\"{tag}_1936\""),
        FieldType::IdIn(..) => format!("{tag}_{}", f.key),
    };
    format!("{} = {v}", f.key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::kinds::SPECS;
    use crate::content::script::Script;

    #[test]
    fn every_field_says_what_it_is() {
        for s in SPECS {
            for f in s.fields {
                assert!(!f.help.trim().is_empty(), "{:?} {} has no help", s.kind, f.key);
                let x = example(s.kind, f, "TYR");
                let parsed = Script::parse(&x);
                assert!(parsed.errors.is_empty(), "{:?} {}: {x}", s.kind, f.key);
                assert!(parsed.root.items.first().is_some_and(|i| i.is(f.key)), "{:?} {}: the example is of another field: {x}", s.kind, f.key);
            }
        }
    }

    #[test]
    fn every_field_is_explained() {
        for s in SPECS {
            for f in s.fields {
                assert!(about(s.kind, f.key).is_some(), "{:?} {} has no explanation", s.kind, f.key);
            }
        }
        for k in [Kind::CountryHistory, Kind::Character] {
            for f in k.spec().fields {
                assert!(OF_KIND.iter().any(|(kk, key, _)| *kk == k && *key == f.key), "{k:?} {} has no explanation of its own", f.key);
            }
        }
        // one explanation for each field
        let mut seen = std::collections::BTreeSet::new();
        for (k, key, _) in OF_KIND {
            assert!(seen.insert((*k, *key)), "{k:?} {key} is explained twice");
        }
        // every explanation is of a field the form has
        for (k, key, _) in OF_KIND {
            assert!(k.spec().fields.iter().any(|f| f.key == *key), "{k:?} has no field {key}");
        }
        let cf = about(Kind::CountryHistory, "create_faction").unwrap();
        assert!(cf.text.contains("not yes or no") && cf.example == "create_faction = TAG_alpine_league");
        assert_eq!(example(Kind::CountryHistory, Kind::CountryHistory.spec().fields.iter().find(|f| f.key == "create_faction").unwrap(), "TYR"), "create_faction = TYR_alpine_league");
        // shared focuses are explained as focuses; names of any kind
        assert!(about(Kind::SharedFocus, "bypass").is_some() && about(Kind::Decision, "allowed").is_some());
    }
}
