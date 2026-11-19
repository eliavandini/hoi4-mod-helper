//! Tests on the real game install (ignored: `cargo test --release -- --ignored`).

use std::path::PathBuf;

use super::kinds::Kind;
use super::source::{self, Origin};

pub fn game() -> PathBuf {
    crate::modbuild::default_game_dir()
}

fn out(name: &str) -> PathBuf {
    let d = std::env::var_os("MOD_HELPER_TEST_OUT").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
#[ignore]
fn base_game_index() {
    let t0 = std::time::Instant::now();
    let ix = source::index("base game", Origin::Base, source::base_files(&game()), &|_| {}).unwrap();
    eprintln!("indexed the base game in {:.2} s: {} files, {} definitions, {} loc keys, {} sprites, {} tags", t0.elapsed().as_secs_f32(), ix.paths.len(), ix.defs.len(), ix.loc.len(), ix.sprites.len(), ix.tags.len());
    for k in Kind::all() {
        eprintln!("  {:>6} {}", ix.of_kind(k).count(), k.label());
    }
    assert!(ix.of_kind(Kind::FocusTree).count() > 50);
    assert!(ix.has(Kind::Focus, "GER_remilitarize_the_rhineland"));
    assert!(ix.of_kind(Kind::Equipment).count() > 50);
    assert!(ix.of_kind(Kind::WarGoal).count() >= 10);
    assert!(ix.of_kind(Kind::Event).count() > 3000);
    assert!(ix.sprites.contains_key("GFX_goal_generic_construct_infrastructure"));
    assert!(ix.tags.contains("GER"));
    assert!(ix.loc.contains_key("GER_remilitarize_the_rhineland"));
    assert!(ix.problems.is_empty(), "{:?}", ix.problems);
    let _ = out("base");
}

#[test]
#[ignore]
fn import_german_tree_from_the_base_game() {
    use super::import::{self, Options};
    use super::store::Store;
    use super::validate::{self, Facts};
    let base = source::index("base game", Origin::Base, source::base_files(&game()), &|_| {}).unwrap();
    let dir = out("import_ger");
    let mut st = Store::load(&dir).unwrap();
    let tree = base.defs.iter().position(|d| d.kind == Kind::FocusTree && d.id == "german_focus").expect("the German tree");
    let t0 = std::time::Instant::now();
    let o = Options { from_tag: Some("GER".into()), ..Default::default() };
    let p = import::plan(&base, Some(&base), &st, &[tree], "TYR", &o);
    eprintln!("planned in {:.2} s: {} definitions, {} renames, {} namespaces, {} loc keys, notes {:?}", t0.elapsed().as_secs_f32(), p.items.len(), p.renames.len(), p.namespaces.len(), p.loc.len(), p.notes);
    let mut kinds: std::collections::BTreeMap<Kind, usize> = Default::default();
    for i in &p.items {
        *kinds.entry(i.kind).or_default() += 1;
    }
    eprintln!("  {kinds:?}");
    let msg = import::apply(&p, &base, &mut st, "import").unwrap();
    eprintln!("  {msg}");
    for (path, f) in &st.package("TYR").unwrap().files {
        if let Some(s) = &f.script {
            assert!(s.errors.is_empty(), "{path}: {:?}", s.errors);
        }
    }
    let facts = Facts { nations: [("TYR".to_string(), ("Tirol".to_string(), 1, [1].into()))].into(), provinces: 100, states: [1].into(), unpinned: 0, ..Default::default() };
    let issues = validate::validate(&st, Some(&base), &facts);
    let (e, w, i) = validate::summary(&issues);
    eprintln!("validated: {e} errors, {w} warnings, {i} notes");
    let mut by: std::collections::BTreeMap<(validate::Severity, &str), usize> = Default::default();
    for x in &issues {
        *by.entry((x.severity, x.category)).or_default() += 1;
    }
    eprintln!("  {by:?}");
    for x in issues.iter().filter(|x| x.severity == validate::Severity::Error).take(5).chain(issues.iter().filter(|x| x.severity == validate::Severity::Warning && !x.message.contains("portrait")).take(40)) {
        eprintln!("  {} {}:{} {}", x.severity.label(), x.file, x.line, x.message);
    }
    st.save().unwrap();
    assert!(dir.join("content/TYR/common/national_focus").is_dir());
}

#[test]
#[ignore]
fn units_of_the_base_game() {
    use super::script::Script;
    use super::units::{Template, UnitDb};
    let base = source::index("base game", Origin::Base, source::base_files(&game()), &|_| {}).unwrap();
    let t0 = std::time::Instant::now();
    let db = UnitDb::of_index(&base);
    eprintln!("read in {:.0} ms: {} battalions, {} equipment, {} modules, {} technologies", t0.elapsed().as_secs_f64() * 1000.0, db.subunits.len(), db.equipment.len(), db.modules.len(), db.techs.len());
    let start = db.have(crate::modbuild::START_TECHS.iter().map(|s| s.to_string()).collect());
    let can: Vec<&str> = db.subunits.values().filter(|u| u.section().is_some() && start.units.contains(&u.id)).map(|u| u.id.as_str()).collect();
    eprintln!("at the start: {can:?}");
    assert!(start.units.contains("infantry") && start.units.contains("engineer") && !start.units.contains("motorized") && !start.units.contains("recon"));
    let name = |s: &str| base.loc.get(s).cloned().unwrap_or_else(|| s.to_string());
    // what DLCs change
    eprintln!("DLCs that change research: {:?}", db.dlcs());
    for id in ["basic_light_tank", "basic_light_tank_chassis", "light_tank_equipment_1", "light_tank_chassis_1", "light_armor", "infantry_weapons", "motorised_infantry"] {
        eprintln!("  {id}: {:?}", db.need_of(id).describe());
    }
    assert!(db.need_of("light_tank_equipment_1").describe().contains("without No Step Back") && db.need_of("light_tank_chassis_1").describe().contains("with No Step Back"));
    assert!(db.need_of("infantry_weapons").is_empty() && db.need_of("light_armor").is_empty(), "light tanks come with or without No Step Back");
    for (label, text) in [
        ("9 infantry", "division_template = { name = A regiments = { infantry = { x = 0 y = 0 } infantry = { x = 0 y = 1 } infantry = { x = 0 y = 2 } infantry = { x = 1 y = 0 } infantry = { x = 1 y = 1 } infantry = { x = 1 y = 2 } infantry = { x = 2 y = 0 } infantry = { x = 2 y = 1 } infantry = { x = 2 y = 2 } } }"),
        ("7/2 with engineers and artillery", "division_template = { name = B regiments = { infantry = { x = 0 y = 0 } infantry = { x = 0 y = 1 } infantry = { x = 0 y = 2 } infantry = { x = 1 y = 0 } infantry = { x = 1 y = 1 } infantry = { x = 1 y = 2 } infantry = { x = 1 y = 3 } artillery_brigade = { x = 2 y = 0 } artillery_brigade = { x = 2 y = 1 } } support = { engineer = { x = 0 y = 0 } artillery = { x = 0 y = 1 } } }"),
        ("light tanks", "division_template = { name = C regiments = { light_armor = { x = 0 y = 0 } light_armor = { x = 0 y = 1 } motorized = { x = 1 y = 0 } } support = { recon = { x = 0 y = 0 } } }"),
    ] {
        let s = Script::parse(text);
        let t = Template::read(s.root.block("division_template").unwrap());
        let nsb: super::dlc::Owned = ["No Step Back".to_string()].into();
        let none = super::dlc::Owned::new();
        for (lbl, have) in [("start", start.clone()), ("1940 without DLC", db.have(db.up_to(&start.techs, 1940, Some(&none)))), ("1940 with No Step Back", db.have(db.up_to(&start.techs, 1940, Some(&nsb))))] {
            let d = db.division(&t, &have);
            let g = |k: &str| d.get(k);
            eprintln!(
                "{label} ({lbl}): width {} HP {:.1} org {:.1} rec {:.2} speed {:.1} soft {:.1} hard {:.1} def {:.1} brk {:.1} armor {:.1} pierce {:.1} hard% {:.0} supply {:.2} manpower {} train {} IC {:.0} recon {} entrench {}",
                g("combat_width"), g("max_strength"), g("max_organisation"), g("default_morale"), g("maximum_speed"), g("soft_attack"), g("hard_attack"), g("defense"), g("breakthrough"), g("armor_value"), g("ap_attack"), g("hardness") * 100.0, g("supply_consumption"), g("manpower"), g("training_time"), g("build_cost_ic"), g("recon"), g("entrenchment")
            );
            eprintln!("   equipment {:?}", d.equipment);
        }
        for p in db.check(&t, &start, "TYR", &name) {
            eprintln!("   {} {}", p.severity.label(), p.message);
        }
    }
}
