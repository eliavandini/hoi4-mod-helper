//! The numbers the game allows: shares from 0 to 1, whole numbers of days
//! and slots, building levels up to the building's cap (read from the game,
//! [`crate::modbuild::buildings`]). For the fields of the definitions, the
//! values inside effects and triggers, and the parts of the wizards' forms:
//! the form does not take a number outside them, the wizards say what is
//! wrong, and the Check page lists those already in the files.

use super::kinds::Kind;
use super::script::{Block, Span, Value};
use crate::modbuild::buildings::Caps;

/// The numbers a value may be.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Range {
    pub min: Option<f64>,
    pub max: Option<f64>,
    /// A whole number.
    pub whole: bool,
    /// One more value it may be (-1: "never", "cannot be removed").
    pub or: Option<f64>,
}

const fn range(min: Option<f64>, max: Option<f64>, whole: bool) -> Range {
    Range { min, max, whole, or: None }
}

/// 0 to 1 (1 is 100%).
pub const SHARE: Range = range(Some(0.0), Some(1.0), false);
/// -1 to 1.
pub const CHANGE: Range = range(Some(-1.0), Some(1.0), false);
/// 0 to 100 (party popularities).
pub const PERCENT: Range = range(Some(0.0), Some(100.0), false);
pub const WHOLE: Range = range(None, None, true);
/// 0, 1, 2, ...
pub const COUNT: Range = range(Some(0.0), None, true);
/// 1, 2, 3, ...
pub const POSITIVE: Range = range(Some(1.0), None, true);
/// 0 or more.
pub const NOT_NEGATIVE: Range = range(Some(0.0), None, false);

fn num(v: f64) -> String {
    let s = format!("{v}");
    s.strip_suffix(".0").map(str::to_string).unwrap_or(s)
}

impl Range {
    pub const fn up_to(self, max: f64) -> Range {
        Range { max: Some(max), ..self }
    }
    pub const fn or(self, v: f64) -> Range {
        Range { or: Some(v), ..self }
    }

    /// "a whole number from 0 to 5", "0 to 1", "-1, or 0 or more".
    pub fn describe(&self) -> String {
        let what = if self.whole { "a whole number" } else { "a number" };
        let base = match (self.min, self.max) {
            (Some(a), Some(b)) if self.whole => format!("{what} from {} to {}", num(a), num(b)),
            (Some(a), Some(b)) => format!("from {} to {}", num(a), num(b)),
            (Some(a), None) => format!("{what}, {} or more", num(a)),
            (None, Some(b)) => format!("{what}, {} or less", num(b)),
            (None, None) => what.to_string(),
        };
        match self.or {
            Some(o) => format!("{} or {base}", num(o)),
            None => base,
        }
    }

    pub fn check(&self, v: f64) -> Result<(), String> {
        if self.or == Some(v) {
            return Ok(());
        }
        let bad = (self.whole && v.fract() != 0.0) || self.min.is_some_and(|m| v < m) || self.max.is_some_and(|m| v > m) || !v.is_finite();
        if bad { Err(self.describe()) } else { Ok(()) }
    }

    /// A typed value: a number, in the range.
    pub fn check_text(&self, s: &str) -> Result<(), String> {
        match s.trim().parse::<f64>() {
            Ok(v) => self.check(v),
            Err(_) => Err("not a number".into()),
        }
    }
}

/// The numbers a field of a definition may be (None: any number).
pub fn of_field(kind: Kind, key: &str) -> Option<Range> {
    use Kind as K;
    Some(match (kind, key) {
        (K::Event, "timeout_days") => COUNT,
        (K::Decision, "cost") => NOT_NEGATIVE,
        (K::Decision, "days_re_enable") => COUNT,
        (K::Decision, "days_remove") => COUNT.or(-1.0),
        (K::Idea, "removal_cost") => NOT_NEGATIVE.or(-1.0),
        (K::Idea, "cost") => NOT_NEGATIVE,
        (K::CountryHistory, "set_research_slots" | "set_convoys") => COUNT,
        (K::CountryHistory, "set_stability" | "set_war_support") => SHARE,
        (K::DivisionTemplate, "priority") => COUNT.up_to(2.0),
        (K::UnitLeaderTrait, "gui_row" | "gui_column") => COUNT,
        (K::UnitLeaderTrait, "attack_skill" | "defense_skill") => WHOLE,
        (K::OpinionModifier, "decay") => NOT_NEGATIVE,
        (K::OpinionModifier, "days" | "months" | "years") => COUNT,
        (K::WarGoal, "generate_base_cost") => NOT_NEGATIVE,
        (K::WarGoal, "take_states_limit") => COUNT,
        (K::Ideology, "ai_ideology_wanted_units_factor") => NOT_NEGATIVE,
        (K::Technology, "start_year") | (K::Equipment, "year") => WHOLE,
        (K::Equipment, "build_cost_ic") => NOT_NEGATIVE,
        (K::SubUnit, "combat_width" | "max_organisation") => NOT_NEGATIVE,
        (K::SubUnit, "manpower") => COUNT,
        (K::PeaceCost, "cost_multiplier") => NOT_NEGATIVE,
        (K::Autonomy, "min_freedom_level") => SHARE,
        (K::Sprite, "noOfFrames") => POSITIVE,
        (K::Focus | K::SharedFocus, "cost") => NOT_NEGATIVE,
        (K::Focus | K::SharedFocus, "x" | "y") => WHOLE,
        (K::CountryLeaderTrait | K::UnitLeaderTrait, "sprite") => COUNT,
        _ => return None,
    })
}

/// The numbers a part of a wizard's form may be, by the part's key.
pub fn of_part(key: &str) -> Option<Range> {
    Some(match key {
        "x" | "y" | "id" | "legacy_id" => WHOLE,
        "freedom_level" => SHARE,
        "skill" => POSITIVE,
        "attack_skill" | "defense_skill" | "planning_skill" | "logistics_skill" | "maneuvering_skill" | "coordination_skill" => COUNT,
        k if k.starts_with("specialization_") => POSITIVE,
        "research_cost_coeff" | "cost" => NOT_NEGATIVE,
        _ => return None,
    })
}

/// The numbers a value in script may be: `key` in a block of `parent`
/// (an effect, a trigger, a part of one; "" at the top).
pub fn of_script(parent: &str, key: &str) -> Option<Range> {
    const EVENTS: [&str; 5] = ["country_event", "news_event", "state_event", "unit_leader_event", "operative_leader_event"];
    Some(match (parent, key) {
        (_, "set_stability" | "set_war_support" | "has_stability" | "has_war_support" | "surrender_progress") => SHARE,
        (_, "add_stability" | "add_war_support") => CHANGE,
        ("add_popularity", "popularity") => CHANGE,
        ("set_popularities", _) | ("set_political_party", "popularity") => PERCENT,
        ("set_politics", "election_frequency") => POSITIVE,
        (_, "set_research_slots" | "set_convoys") => COUNT,
        (_, "add_research_slot" | "add_extra_state_shared_building_slots" | "add_manpower") => WHOLE,
        ("add_tech_bonus", "bonus") => NOT_NEGATIVE,
        ("add_tech_bonus", "uses") => POSITIVE,
        ("add_building_construction", "level") => POSITIVE,
        ("add_offsite_building", "level") => WHOLE,
        ("add_resource", "amount") | ("add_equipment_to_stockpile", "amount") => WHOLE,
        ("add_timed_idea", "days") => POSITIVE,
        (p, "days" | "hours" | "random_days" | "random_hours") if EVENTS.contains(&p) => COUNT,
        ("set_technology", _) => COUNT.up_to(1.0),
        _ => return None,
    })
}

/// A number in script the game does not allow.
#[derive(Clone, Debug, PartialEq)]
pub struct Problem {
    /// The value's bytes.
    pub span: Span,
    pub message: String,
}

/// The numbers of a script (and of the blocks in it) that are not what the
/// game allows; values that are not numbers (`@constant`, `var:x`) pass.
pub fn script_problems(root: &Block, caps: &Caps) -> Vec<Problem> {
    let mut out = Vec::new();
    walk(root, "", caps, &mut out);
    out
}

fn walk(b: &Block, parent: &str, caps: &Caps, out: &mut Vec<Problem>) {
    for i in &b.items {
        let key = i.key();
        match &i.value {
            Value::Atom(a) => {
                let Ok(v) = a.text.parse::<f64>() else { continue };
                let Some(r) = of_script(parent, key) else { continue };
                if let Err(why) = r.check(v) {
                    // (a rule of any block: the key alone; `123 = { set_stability = 2 }`)
                    let what = if of_script("", key).is_some() { key.to_string() } else { format!("{parent} {key}") };
                    let op = i.op.as_ref().map_or("=", |o| o.0.as_str());
                    out.push(Problem { span: a.span.clone(), message: format!("{what} {op} {}: {why}", a.text) });
                }
            }
            Value::Block(inner) | Value::Tagged(_, inner) => {
                if i.is("add_building_construction") {
                    building_level(inner, caps, out);
                }
                walk(inner, key, caps, out);
            }
            Value::None => {}
        }
    }
}

/// `add_building_construction = { type = x level = n }`: the level over
/// the building's cap, a building the game does not have.
fn building_level(b: &Block, caps: &Caps, out: &mut Vec<Problem>) {
    let Some(ty) = b.get("type").and_then(|i| i.atom()) else { return };
    if ty.text.starts_with(['[', '@', '$']) || ty.text.contains(':') {
        return;
    }
    if caps.building(&ty.text).is_none() {
        out.push(Problem { span: ty.span.clone(), message: format!("add_building_construction: the game has no building {}", ty.text) });
        return;
    }
    let Some(level) = b.get("level").and_then(|i| i.atom()) else { return };
    let (Ok(v), Some(max)) = (level.text.parse::<f64>(), caps.max_level(&ty.text, None)) else { return };
    if v > max as f64 {
        let per = if caps.building(&ty.text).is_some_and(|x| x.per_province()) { " in a province" } else { " in a state" };
        out.push(Problem { span: level.span.clone(), message: format!("add_building_construction level = {}: {} goes to {max} at most{per}", level.text, ty.text) });
    }
}

#[cfg(test)]
mod tests {
    use super::super::script::Script;
    use super::*;

    #[test]
    fn ranges() {
        assert_eq!(SHARE.check(0.5), Ok(()));
        assert_eq!(SHARE.check(1.5).unwrap_err(), "from 0 to 1");
        assert_eq!(COUNT.up_to(2.0).check(1.5).unwrap_err(), "a whole number from 0 to 2");
        assert_eq!(NOT_NEGATIVE.or(-1.0).check(-1.0), Ok(()));
        assert_eq!(NOT_NEGATIVE.or(-1.0).check(-2.0).unwrap_err(), "-1 or a number, 0 or more");
        assert_eq!(POSITIVE.check_text("abc").unwrap_err(), "not a number");
        assert_eq!(POSITIVE.check_text(" 3 "), Ok(()));
        assert_eq!(of_field(Kind::CountryHistory, "set_stability"), Some(SHARE));
        assert_eq!(of_field(Kind::CountryHistory, "capital"), None);
        assert_eq!(of_part("freedom_level"), Some(SHARE));
    }

    #[test]
    fn in_script() {
        let caps = Caps::vanilla();
        let text = "set_stability = 1.4\nadd_war_support = 0.1\n123 = {\n\tadd_building_construction = { type = infrastructure level = 7 instant_build = yes }\n\tadd_building_construction = { type = naval_base level = 12 province = 5 }\n\tadd_building_construction = { type = castle level = 1 }\n\tadd_building_construction = { type = arms_factory level = 2.5 }\n}\nadd_popularity = { ideology = fascism popularity = 20 }\nset_popularities = { fascism = 30 democratic = 120 }\ncountry_event = { id = x.1 days = -3 }\nadd_days_mission_timeout = { mission = m days = -30 }\nset_war_support = @ws\n";
        let s = Script::parse(text);
        let p: Vec<String> = script_problems(&s.root, &caps).into_iter().map(|p| p.message).collect();
        assert_eq!(
            p,
            [
                "set_stability = 1.4: from 0 to 1",
                "add_building_construction level = 7: infrastructure goes to 5 at most in a state",
                "add_building_construction level = 12: naval_base goes to 10 at most in a province",
                "add_building_construction: the game has no building castle",
                "add_building_construction level = 2.5: a whole number, 1 or more",
                "add_popularity popularity = 20: from -1 to 1",
                "set_popularities democratic = 120: from 0 to 100",
                "country_event days = -3: a whole number, 0 or more",
            ]
        );
        let spans: Vec<&str> = script_problems(&s.root, &caps).iter().map(|p| &text[p.span.clone()]).collect();
        assert_eq!(spans[0], "1.4");
    }
}
