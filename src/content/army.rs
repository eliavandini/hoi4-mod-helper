//! The army a nation starts with: the order of battle its history loads
//! (`oob = "<file>"`: history/units/<file>.txt), with its division
//! templates and its divisions (`units = { division = { ... } }`), each of a
//! template of that file and placed in a land province of the map.
//!
//! The build places no divisions of its own: a nation starts with those of
//! its content, made on the Army page (templates in the division designer,
//! divisions placed on the map there).
//!
//! How a division starts (the game's rules): `start_experience_factor`
//! (0 to 1, else 0) is its experience, shown as a level ([`level`]);
//! `start_equipment_factor` (else 1) the share of its equipment it gets,
//! not from the stockpile; `start_manpower_factor` the share of its men it
//! gets, and without it the men come out of the nation's reserves, as many
//! as there are. A nation of a few villages has few, so its divisions start
//! short of men, and the soldiers that fill them up later bring no
//! experience: they start weaker and greener than the file says. New
//! divisions are written with all three; [`army_needs`] is the equipment and
//! men an army needs, for the stockpile.

use std::collections::{BTreeMap, BTreeSet};

use super::dlc::{self, Need};
use super::navy::{OobLine, oob_file};
use super::script::{self, Block, Span};
use super::store::Store;
use super::validate::Severity;

/// What new divisions start with (0 to 1): experience ("Trained"),
/// equipment (the rest comes from the stockpile, when it has it) and men
/// (all of them: not out of the reserves).
pub const EXPERIENCE: f64 = 0.2;
pub const EQUIPMENT: f64 = 0.7;
pub const MANPOWER: f64 = 1.0;

/// The experience a division needs for each level after the first (the
/// game's `UNIT_EXP_LEVELS`).
pub const LEVELS: [f64; 4] = [0.1, 0.3, 0.75, 0.9];
/// The names of the levels (the game's `UNIT_LEVEL_0` ... `UNIT_LEVEL_4`).
pub const LEVEL_NAMES: [&str; 5] = ["Green", "Trained", "Regular", "Seasoned", "Veterans"];

/// The level of an experience (0: green ... 4: veterans).
pub fn level(experience: f64) -> usize {
    LEVELS.iter().filter(|l| experience >= **l - 1e-9).count()
}

/// A division of an order of battle.
#[derive(Clone, Debug, PartialEq)]
pub struct Division {
    /// `name = "..."`; None: the next of its template's name list
    /// (`division_name = { name_order = n }`, or nothing).
    pub name: Option<String>,
    /// `division_name = { name_order = n }`.
    pub order: Option<u32>,
    pub template: Option<String>,
    pub location: Option<u32>,
    pub experience: Option<f64>,
    pub equipment: Option<f64>,
    /// `start_manpower_factor`; None: its men out of the reserves.
    pub manpower: Option<f64>,
    pub span: Span,
}

impl Division {
    /// As the lists show it.
    pub fn label(&self) -> String {
        match (&self.name, self.order) {
            (Some(n), _) => n.clone(),
            (None, Some(o)) => format!("no. {o} of its name list"),
            (None, None) => "(named by the game)".into(),
        }
    }
}

/// The divisions of an order of battle (its `units` blocks).
pub fn read_divisions(root: &Block) -> Vec<Division> {
    let mut out = Vec::new();
    for u in root.blocks("units") {
        for i in &u.items {
            let Some(b) = i.block().filter(|_| i.is("division")) else { continue };
            out.push(Division {
                name: b.atom("name").map(String::from),
                order: b.block("division_name").and_then(|d| d.num("name_order")).map(|n| n as u32),
                template: b.atom("division_template").map(String::from),
                location: b.atom("location").and_then(|v| v.parse().ok()),
                experience: b.num("start_experience_factor"),
                equipment: b.num("start_equipment_factor"),
                manpower: b.num("start_manpower_factor"),
                span: i.span.clone(),
            });
        }
    }
    out
}

/// The names of the division templates of an order of battle, in order.
pub fn templates_of(root: &Block) -> Vec<String> {
    root.blocks("division_template").filter_map(|b| b.atom("name")).map(String::from).collect()
}

/// The `oob` lines of a nation's history.
pub fn army_oobs(st: &Store, tag: &str) -> Vec<OobLine> {
    let mut out = Vec::new();
    let Some(p) = st.package(tag) else { return out };
    for (path, file) in &p.files {
        if !path.starts_with("history/countries/") {
            continue;
        }
        let Some(s) = &file.script else { continue };
        dlc::walk_flow(&s.root, &Need::default(), &mut |i, need| {
            if i.is("oob")
                && let Some(v) = i.text()
            {
                out.push(OobLine { name: v.to_string(), need: need.clone(), file: path.clone(), span: i.span.clone() });
            }
        });
    }
    out
}

/// The order of battle a nation starts with: the last `oob` of its history
/// (the game takes the last), and the package and path of its file (None:
/// not in the content).
pub fn army_oob(st: &Store, tag: &str) -> Option<(OobLine, Option<(String, String)>)> {
    let line = army_oobs(st, tag).into_iter().last()?;
    let file = oob_file(st, tag, &line.name);
    Some((line, file))
}

/// The files of a nation's package with division templates: path →
/// template names.
pub fn template_files(st: &Store, tag: &str) -> BTreeMap<String, Vec<String>> {
    let mut out = BTreeMap::new();
    let Some(p) = st.package(tag) else { return out };
    for (path, f) in &p.files {
        if path.starts_with("history/units/")
            && let Some(s) = &f.script
        {
            let t = templates_of(&s.root);
            if !t.is_empty() {
                out.insert(path.clone(), t);
            }
        }
    }
    out
}

/// The name of an order of battle file (history/units/<name>.txt).
pub fn oob_name(path: &str) -> &str {
    path.trim_start_matches("history/units/").trim_end_matches(".txt")
}

/// The text of a division.
pub fn render_division(name: &str, template: &str, location: u32, experience: f64, equipment: f64, manpower: f64) -> String {
    format!(
        "division = {{\n\tname = {}\n\tlocation = {location}\n\tdivision_template = {}\n\tstart_experience_factor = {}\n\tstart_equipment_factor = {}\n\tstart_manpower_factor = {}\n}}",
        script::quote(name),
        script::quote(template),
        num(experience),
        num(equipment),
        num(manpower)
    )
}

fn num(v: f64) -> String {
    let s = format!("{:.2}", v);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Names for `n` new divisions of a template: "1. <template>", "2. ..."
/// with numbers no division of the nation has yet.
pub fn division_names(template: &str, taken: &BTreeSet<String>, n: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 1;
    while out.len() < n {
        let name = format!("{i}. {}", template.trim());
        if !taken.contains(&name) {
            out.push(name);
        }
        i += 1;
    }
    out
}

/// What a province of the map is, for the checks.
#[derive(Clone, Debug, PartialEq)]
pub enum Ground {
    /// Land; the tag of the nation that owns its state ("" for none).
    Land(String),
    Water,
}

/// A problem of a division (or of the file, `span` None).
#[derive(Clone, Debug, PartialEq)]
pub struct ArmyProblem {
    pub severity: Severity,
    pub message: String,
    pub span: Option<Span>,
}

/// The checks of the divisions of an order of battle of `tag`: their
/// templates (`templates`: those of the file) and places (`ground` by
/// province id; empty: no map).
pub fn check_divisions(divs: &[Division], templates: &[String], ground: &[Option<Ground>], tag: &str) -> Vec<ArmyProblem> {
    let mut out = Vec::new();
    let mut add = |severity: Severity, message: String, span: &Span| out.push(ArmyProblem { severity, message, span: Some(span.clone()) });
    let mut names: BTreeMap<&str, usize> = BTreeMap::new();
    for d in divs {
        let who = d.name.as_deref().map(|n| format!("division \"{n}\"")).unwrap_or_else(|| "a division".into());
        match &d.template {
            None => add(Severity::Error, format!("{who}: no division_template"), &d.span),
            Some(t) if !templates.contains(t) => add(Severity::Error, format!("{who}: its template \"{t}\" is not in this file (the game only takes the templates of the file)"), &d.span),
            _ => {}
        }
        match d.location {
            None => add(Severity::Error, format!("{who}: no location (a province)"), &d.span),
            Some(p) if !ground.is_empty() => match ground.get(p as usize).cloned().flatten() {
                // not a province of the map: the checks of the orders of battle say so
                None => {}
                Some(Ground::Water) => add(Severity::Error, format!("{who}: location = {p} is sea or a lake; divisions stand on land"), &d.span),
                Some(Ground::Land(o)) if o.is_empty() => add(Severity::Warning, format!("{who}: location = {p} is in no nation's state"), &d.span),
                Some(Ground::Land(o)) if o != tag => add(Severity::Warning, format!("{who}: location = {p} is {o}'s; it starts there, in another nation's land"), &d.span),
                _ => {}
            },
            _ => {}
        }
        for (what, v) in [("start_experience_factor", d.experience), ("start_equipment_factor", d.equipment), ("start_manpower_factor", d.manpower)] {
            if let Some(v) = v.filter(|v| !(0.0..=1.0).contains(v)) {
                add(Severity::Warning, format!("{who}: {what} = {} is outside 0 to 1", num(v)), &d.span);
            }
        }
        if let Some(n) = &d.name {
            *names.entry(n).or_default() += 1;
        }
    }
    for (n, k) in names {
        if k > 1 {
            out.push(ArmyProblem { severity: Severity::Warning, message: format!("{k} divisions are called \"{n}\""), span: None });
        }
    }
    let unset = divs.iter().filter(|d| d.manpower.is_none()).count();
    if unset > 0 {
        let which = if unset == divs.len() { if unset == 1 { "the division has".to_string() } else { format!("all {unset} divisions have") } } else if unset == 1 { "1 division has".to_string() } else { format!("{unset} divisions have") };
        out.push(ArmyProblem {
            severity: Severity::Warning,
            message: format!("{which} no start_manpower_factor{MEN_FROM_RESERVES}"),
            span: None,
        });
    }
    out
}

/// What a division without `start_manpower_factor` does.
pub const MEN_FROM_RESERVES: &str = ": their men come out of the nation's reserves, only as many as it has (few for a small nation), so they start short of men, and the soldiers that fill them up later bring no experience";

/// What an army needs of one kind of equipment.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EquipmentNeed {
    /// The model its divisions get (None: the nation has none of it).
    pub model: Option<String>,
    /// For every division at full strength.
    pub full: f64,
    /// What they start with (`start_equipment_factor`): not from the
    /// stockpile.
    pub given: f64,
}

impl EquipmentNeed {
    /// What they lack at the start: they take it from the stockpile.
    pub fn missing(&self) -> f64 {
        (self.full - self.given).max(0.0)
    }
}

/// The equipment and men an army needs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ArmyNeeds {
    /// By archetype (`infantry_equipment`, ...).
    pub equipment: BTreeMap<String, EquipmentNeed>,
    /// Men of every division at full strength.
    pub men: f64,
    /// Men they get (`start_manpower_factor`).
    pub men_given: f64,
    /// Men of the divisions without it: out of the reserves.
    pub men_from_reserves: f64,
    /// Divisions of no template of the file (left out).
    pub unknown: usize,
}

/// What the divisions need, with each template's stats (`per_template`:
/// [`super::units::UnitDb::division`]).
pub fn army_needs(divs: &[Division], per_template: &BTreeMap<String, super::units::Division>) -> ArmyNeeds {
    let mut out = ArmyNeeds::default();
    for d in divs {
        let Some(t) = d.template.as_ref().and_then(|t| per_template.get(t)) else {
            out.unknown += 1;
            continue;
        };
        let equip = d.equipment.unwrap_or(1.0).clamp(0.0, 1.0);
        for (arch, (model, n)) in &t.equipment {
            let e = out.equipment.entry(arch.clone()).or_default();
            if e.model.is_none() {
                e.model = model.clone();
            }
            e.full += n;
            e.given += n * equip;
        }
        let men = t.get("manpower");
        out.men += men;
        match d.manpower {
            Some(f) => out.men_given += men * f.clamp(0.0, 1.0),
            None => out.men_from_reserves += men,
        }
    }
    out
}

/// What the stockpile `stock` (model → how many) has of an archetype.
pub fn in_stock(db: &super::units::UnitDb, stock: &BTreeMap<String, f64>, arch: &str) -> f64 {
    stock.iter().filter(|(m, _)| *m == arch || db.archetype_of(m) == arch).map(|(_, n)| n).sum()
}

/// What to put in the stockpile: of each kind the model the divisions get,
/// as many as they lack beyond what it has (`whole`: as many as the whole
/// army needs, again), rounded up.
pub fn stock_additions(needs: &ArmyNeeds, have: &dyn Fn(&str) -> f64, whole: bool) -> Vec<(String, f64)> {
    needs
        .equipment
        .iter()
        .filter_map(|(arch, e)| {
            let model = e.model.clone()?;
            let n = if whole { e.full } else { e.missing() - have(arch) };
            (n.ceil() >= 1.0).then(|| (model, n.ceil()))
        })
        .collect()
}

/// The names `oob` and `load_oob` load, anywhere in the content.
pub fn loaded_oobs(st: &Store) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for p in st.packages.values() {
        for f in p.files.values() {
            let Some(s) = &f.script else { continue };
            s.root.walk(&mut |i, _| {
                if (i.is("oob") || i.is("load_oob"))
                    && let Some(v) = i.text()
                {
                    out.insert(v.to_string());
                }
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::script::Script;

    const OOB: &str = "division_template = {\n\tname = \"Schuetzen\"\n\tregiments = { infantry = { x = 0 y = 0 } }\n}\nunits = {\n\tdivision = {\n\t\tname = \"1. Schuetzen\"\n\t\tlocation = 10\n\t\tdivision_template = \"Schuetzen\"\n\t\tstart_experience_factor = 0.2\n\t\tstart_equipment_factor = 0.7\n\t}\n\tdivision = { division_name = { is_name_ordered = yes name_order = 3 } location = 11 division_template = \"Jaeger\" start_equipment_factor = 1.5 }\n\tdivision = { name = \"1. Schuetzen\" location = 12 division_template = \"Schuetzen\" }\n\tdivision = { location = 13 }\n\tfleet = { name = \"x\" }\n}\n";

    #[test]
    fn divisions_are_read() {
        let s = Script::parse(OOB);
        let d = read_divisions(&s.root);
        assert_eq!(d.len(), 4, "fleets are not divisions");
        assert_eq!((d[0].name.as_deref(), d[0].template.as_deref(), d[0].location, d[0].experience, d[0].equipment), (Some("1. Schuetzen"), Some("Schuetzen"), Some(10), Some(0.2), Some(0.7)));
        assert_eq!((d[1].label(), d[1].order), ("no. 3 of its name list".to_string(), Some(3)));
        assert_eq!(templates_of(&s.root), ["Schuetzen"]);
        // what the page writes reads back
        let text = render_division("2. Schuetzen", "Schuetzen", 14, 0.25, 1.0, 0.5);
        let r = read_divisions(&Script::parse(&format!("units = {{\n{text}\n}}")).root);
        assert_eq!((r[0].name.as_deref(), r[0].location, r[0].experience, r[0].equipment, r[0].manpower), (Some("2. Schuetzen"), Some(14), Some(0.25), Some(1.0), Some(0.5)));
        assert_eq!(d[0].manpower, None);
    }

    #[test]
    fn divisions_are_checked() {
        let s = Script::parse(OOB);
        let d = read_divisions(&s.root);
        let mut ground = vec![None; 16];
        ground[10] = Some(Ground::Land("TYR".into()));
        ground[11] = Some(Ground::Land("BOZ".into()));
        ground[12] = Some(Ground::Water);
        ground[13] = Some(Ground::Land(String::new()));
        let m: Vec<String> = check_divisions(&d, &templates_of(&s.root), &ground, "TYR").into_iter().map(|p| format!("{} {}", p.severity.label(), p.message)).collect();
        assert_eq!(
            m,
            [
                "error a division: its template \"Jaeger\" is not in this file (the game only takes the templates of the file)",
                "warning a division: location = 11 is BOZ's; it starts there, in another nation's land",
                "warning a division: start_equipment_factor = 1.5 is outside 0 to 1",
                "error division \"1. Schuetzen\": location = 12 is sea or a lake; divisions stand on land",
                "error a division: no division_template",
                "warning a division: location = 13 is in no nation's state",
                "warning 2 divisions are called \"1. Schuetzen\"",
                &format!("warning all 4 divisions have no start_manpower_factor{MEN_FROM_RESERVES}"),
            ]
        );
        // without a map only what the file says
        let mut one = d[..1].to_vec();
        assert_eq!(check_divisions(&one, &templates_of(&s.root), &[], "TYR").len(), 1, "its men");
        one[0].manpower = Some(1.0);
        assert_eq!(check_divisions(&one, &templates_of(&s.root), &[], "TYR"), vec![]);
    }

    #[test]
    fn levels_are_the_games() {
        assert_eq!([0.0, 0.09, 0.1, 0.2, 0.3, 0.74, 0.75, 0.9, 1.0].map(level), [0, 0, 1, 1, 2, 2, 3, 4, 4]);
        assert_eq!(LEVEL_NAMES[level(EXPERIENCE)], "Trained");
    }

    #[test]
    fn the_army_needs_are_added_up() {
        use crate::content::units;
        let mut inf = units::Division::default();
        inf.equipment.insert("infantry_equipment".into(), (Some("infantry_equipment_1".into()), 300.0));
        inf.equipment.insert("artillery_equipment".into(), (None, 12.0));
        inf.stats.insert("manpower".into(), 3000.0);
        let per: BTreeMap<String, units::Division> = [("Schuetzen".to_string(), inf)].into();
        let s = Script::parse(OOB);
        let mut d = read_divisions(&s.root);
        d[0].manpower = Some(0.5);
        let n = army_needs(&d, &per);
        // 1. Schuetzen at 0.7 of its equipment, the other at 1 (unset); one of Jaeger, one of none
        assert_eq!(n.unknown, 2);
        let e = &n.equipment["infantry_equipment"];
        assert_eq!((e.model.as_deref(), e.full, e.given, e.missing()), (Some("infantry_equipment_1"), 600.0, 510.0, 90.0));
        assert_eq!((n.men, n.men_given, n.men_from_reserves), (6000.0, 1500.0, 3000.0));
        // what the stockpile lacks; no model, nothing to add
        let have = |a: &str| if a == "infantry_equipment" { 40.0 } else { 0.0 };
        assert_eq!(stock_additions(&n, &have, false), [("infantry_equipment_1".to_string(), 50.0)]);
        assert_eq!(stock_additions(&n, &have, true), [("infantry_equipment_1".to_string(), 600.0)]);
        assert!(stock_additions(&n, &|_| 1000.0, false).is_empty());
    }

    #[test]
    fn new_divisions_are_numbered() {
        let taken: BTreeSet<String> = ["1. Schuetzen", "3. Schuetzen"].map(String::from).into();
        assert_eq!(division_names("Schuetzen", &taken, 3), ["2. Schuetzen", "4. Schuetzen", "5. Schuetzen"]);
    }
}
