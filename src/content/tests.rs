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
    let facts = Facts { nations: [("TYR".to_string(), ("Tirol".to_string(), 1, [1].into()))].into(), provinces: 100, states: [1].into(), unpinned: 0 };
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
