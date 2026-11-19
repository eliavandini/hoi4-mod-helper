//! The content in the built mod. Each package's files go to their paths in
//! the mod, `shared` first, then the nations in tag order. Where a file of
//! the content has the path of one the build made:
//!
//! - a nation's `history/countries/TAG*.txt` goes after the history the build
//!   wrote for it (capital, technology, politics), so what it says comes later
//!   and wins;
//! - script and localisation files of folders the game reads as a whole
//!   (`common/…/`, `events/`, `localisation/`, `interface/`) are written under
//!   another name, so both are read;
//! - any other file (a flag, a picture, a map file) replaces the built one.
//!
//! A package that is neither `shared` nor the tag of a nation of the map is
//! an error: its content would be for a country that is not in the game.
//!
//! When the map has gaps in its ids (deleted provinces, merged states), the
//! build numbers them 1..n ([`crate::editor::model::compact`]) and the
//! numbers the scripts name provinces and states by are changed along
//! ([`renumber_ids`]); the content itself keeps the editor's.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context as _, Result, bail};

use super::script::{self, Block, Edit, Script};
use super::store::{SHARED, Store};
use crate::editor::model::Renumbering;

/// Folders whose files the game reads all of (the name of a file does not matter).
const MERGED: [&str; 5] = ["common/", "events/", "localisation/", "localization/", "interface/"];

/// Copy the content of `map_dir` into the mod at `out`. `history`: tag → the
/// history file the build wrote for it (relative to `out`); `ids`: how the
/// build numbered the map's ids, when it changed them. Returns lines for the
/// build report.
pub fn copy_into(out: &Path, map_dir: &Path, history: &BTreeMap<String, String>, ids: Option<&Renumbering>) -> Result<Vec<String>> {
    let st = Store::load(map_dir).context("cannot read the nation content")?;
    let pkgs: Vec<&String> = st.packages.iter().filter(|(_, p)| !p.is_empty()).map(|(n, _)| n).collect();
    if pkgs.is_empty() {
        return Ok(Vec::new());
    }
    let orphans: Vec<&str> = pkgs.iter().filter(|n| n.as_str() != SHARED && !history.contains_key(n.as_str())).map(|n| n.as_str()).collect();
    if !orphans.is_empty() {
        bail!(
            "the content of {} is for no nation of the map (the nations are {}); rename it to a nation's tag in the content screen, or move it into shared",
            orphans.join(", "),
            history.keys().cloned().collect::<Vec<_>>().join(", ")
        );
    }
    // shared first, so a nation's files come after it
    let mut order: Vec<&String> = pkgs.iter().copied().filter(|n| n.as_str() == SHARED).collect();
    order.extend(pkgs.iter().copied().filter(|n| n.as_str() != SHARED));

    let mut written: BTreeSet<String> = BTreeSet::new();
    let (mut files, mut appended, mut renamed, mut replaced) = (0usize, Vec::new(), Vec::new(), Vec::new());
    let (mut numbers, mut numbered) = (0usize, 0usize);
    for pkg in order {
        let p = &st.packages[pkg];
        for rel in p.files.keys() {
            let Some(mut bytes) = st.bytes(pkg, rel).map(|b| b.to_vec()) else { continue };
            let lower = rel.to_ascii_lowercase();
            if let Some(ids) = ids
                && lower.ends_with(".txt")
                && let Ok(text) = std::str::from_utf8(&bytes)
                && let Some((new, n)) = renumber_ids(rel, text, ids)
            {
                bytes = new.into_bytes();
                (numbers, numbered) = (numbers + n, numbered + 1);
            }
            if lower.ends_with(".yml") && !bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
                // the game reads localisation only with the byte order mark
                bytes.splice(0..0, [0xef, 0xbb, 0xbf]);
            }
            // a nation's history after the built one
            if let Some(tag) = history_tag(rel)
                && let Some(built) = history.get(&tag)
            {
                let path = out.join(built);
                let mut text = std::fs::read_to_string(&path).unwrap_or_default();
                if !text.ends_with('\n') && !text.is_empty() {
                    text.push('\n');
                }
                let body = String::from_utf8_lossy(&bytes);
                text.push_str(&format!("\n# ---- from the content: {pkg}/{rel}\n"));
                // an else after an if goes inside it: the game drops it there
                text.push_str(&super::dlc::nest_elses(body.strip_prefix('\u{feff}').unwrap_or(&body)));
                write(&path, text.as_bytes())?;
                appended.push(format!("{pkg}/{rel}"));
                files += 1;
                continue;
            }
            if lower.starts_with("history/countries/") && lower.ends_with(".txt") {
                let body = String::from_utf8_lossy(&bytes).into_owned();
                let nested = super::dlc::nest_elses(&body);
                if nested != body {
                    bytes = nested.into_bytes();
                }
            }
            let mut to = rel.clone();
            if out.join(&to).exists() {
                if MERGED.iter().any(|m| lower.starts_with(m)) && (lower.ends_with(".txt") || lower.ends_with(".yml") || lower.ends_with(".gfx") || lower.ends_with(".gui")) {
                    to = free_name(out, rel, pkg);
                    renamed.push(format!("{pkg}/{rel} → {to}"));
                } else {
                    let by = if written.contains(&to) { "another package's" } else { "the built" };
                    replaced.push(format!("{pkg}/{rel} (replaces {by} one)"));
                }
            }
            write(&out.join(&to), &bytes)?;
            written.insert(to);
            files += 1;
        }
    }
    let mut report = vec![format!("content: {files} files of {}", pkgs.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "))];
    if numbers > 0 {
        report.push(format!("content: {numbers} province and state numbers in {numbered} files changed to the game's ids"));
    }
    if !appended.is_empty() {
        report.push(format!("content: history added after the built one: {}", appended.join(", ")));
    }
    if !renamed.is_empty() {
        report.push(format!("content: written under another name, as the build has a file there: {}", renamed.join(", ")));
    }
    for r in replaced {
        report.push(format!("content: {r}"));
    }
    Ok(report)
}

/// Keys whose value is a province id.
const PROVINCE_KEYS: [&str; 5] = ["province", "controls_province", "set_province_controller", "prioritize_location", "reset_province_name"];
/// Keys whose value is a province id in an order of battle (`history/units/`).
const OOB_PROVINCE_KEYS: [&str; 3] = ["location", "naval_base", "base"];

/// The script `text` of the file `rel` with the numbers that name provinces
/// and states changed as `ids` says, and how many changed (None: none did).
/// What counts as a province or a state:
/// - the value of `capital`, `state` and the state effects and triggers
///   (`owns_state`, `transfer_state`, …), a state;
/// - the value of `province`, `controls_province`, … and of `id` in a
///   `province = { … }` or `set_province_name = { … }` block, a province;
///   in an order of battle also `location`, `naval_base` and `base`; in
///   `adjacency_rules.txt` the `icon`;
/// - a number before a block (`123 = { add_core_of = TYR }`, the states of
///   `air_wings`), a state; in `buildings = { … }` a province, and in
///   `random_list` a weight, left as it is.
///
/// A number the map has nothing for stays as it is (the content check
/// reports it).
pub fn renumber_ids(rel: &str, text: &str, ids: &Renumbering) -> Option<(String, usize)> {
    struct Walk<'a> {
        ids: &'a Renumbering,
        oob: bool,
        rules: bool,
        edits: Vec<Edit>,
    }
    impl Walk<'_> {
        fn block(&mut self, b: &Block, parent: &str) {
            for i in &b.items {
                let key = i.key().to_ascii_lowercase();
                if let Some(a) = i.atom().filter(|a| !a.quoted)
                    && let Ok(v) = a.text.parse::<u32>()
                {
                    let state = key == "capital" || key == "state" || super::validate::STATE_KEYS.contains(&key.as_str());
                    let province = PROVINCE_KEYS.contains(&key.as_str())
                        || (self.oob && OOB_PROVINCE_KEYS.contains(&key.as_str()))
                        || (self.rules && key == "icon")
                        || (key == "id" && matches!(parent, "province" | "set_province_name"));
                    let new = if state { self.ids.state(v) } else if province { self.ids.province(v) } else { None };
                    if let Some(n) = new.filter(|&n| n != v) {
                        self.edits.push(Edit { range: a.span.clone(), text: n.to_string() });
                    }
                }
                let Some(c) = i.block() else { continue };
                if let Some(k) = i.key.as_ref().filter(|k| !k.quoted && parent != "random_list")
                    && let Ok(v) = k.text.parse::<u32>()
                {
                    let new = if parent == "buildings" { self.ids.province(v) } else { self.ids.state(v) };
                    if let Some(n) = new.filter(|&n| n != v) {
                        self.edits.push(Edit { range: k.span.clone(), text: n.to_string() });
                    }
                }
                self.block(c, &key);
            }
        }
    }
    let lower = rel.to_ascii_lowercase();
    let body = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut w = Walk { ids, oob: lower.starts_with("history/units/"), rules: lower.ends_with("adjacency_rules.txt"), edits: Vec::new() };
    w.block(&Script::parse(body).root, "");
    if w.edits.is_empty() {
        return None;
    }
    let n = w.edits.len();
    Some((format!("{}{}", &text[..text.len() - body.len()], script::apply(body, &w.edits)), n))
}

/// The tag a `history/countries/` file is for (`TYR - Tyrol.txt`, `TYR.txt`).
fn history_tag(rel: &str) -> Option<String> {
    let name = rel.strip_prefix("history/countries/")?;
    if name.contains('/') || !name.to_ascii_lowercase().ends_with(".txt") {
        return None;
    }
    let tag: String = name.chars().take(3).collect();
    let next = name[tag.len()..].chars().next();
    (crate::modbuild::countries::tag_ok(&tag) && matches!(next, Some(' ' | '-' | '.' | '_'))).then_some(tag)
}

/// A name next to `rel` that nothing has: `x_TYR.txt`, `x_TYR_2.txt`; for
/// localisation the `_l_<language>.yml` ending stays.
fn free_name(out: &Path, rel: &str, pkg: &str) -> String {
    let (dir, name) = rel.rsplit_once('/').map(|(d, n)| (format!("{d}/"), n)).unwrap_or((String::new(), rel));
    let lower = name.to_ascii_lowercase();
    let cut = lower.rfind("_l_").filter(|_| lower.ends_with(".yml")).or_else(|| name.rfind('.')).unwrap_or(name.len());
    let (stem, end) = name.split_at(cut);
    (1..)
        .map(|i| if i == 1 { format!("{dir}{stem}_{pkg}{end}") } else { format!("{dir}{stem}_{pkg}_{i}{end}") })
        .find(|c| !out.join(c).exists())
        .unwrap()
}

fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).with_context(|| format!("cannot create {}", d.display()))?;
    }
    std::fs::write(path, bytes).with_context(|| format!("cannot write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(root: &Path, rel: &str, text: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    fn dirs(name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let t = std::env::temp_dir().join(format!("mh-content-build-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        (t.join("map"), t.join("mod"))
    }

    #[test]
    fn copies_merges_and_renames() {
        let (map, out) = dirs("copy");
        put(&out, "history/countries/TYR - Tyrol.txt", "capital = 3\nset_research_slots = 3\n");
        put(&out, "localisation/english/suedtirol_l_english.yml", "\u{feff}l_english:\n STATE_1:0 \"Bozen\"\n");
        put(&out, "gfx/flags/TYR.tga", "built flag");
        put(&map, "content/TYR/history/countries/TYR - Tyrol.txt", "set_politics = { ruling_party = fascism }\n");
        put(&map, "content/TYR/common/national_focus/tyr.txt", "focus_tree = { id = tyr }\n");
        put(&map, "content/TYR/localisation/english/suedtirol_l_english.yml", "l_english:\n TYR_x:0 \"X\"\n");
        put(&map, "content/TYR/gfx/flags/TYR.tga", "my flag");
        put(&map, "content/shared/common/ideologies/x.txt", "x = {}\n");
        put(&map, "content/.snapshots/a/b.txt", "not a package");
        let hist: BTreeMap<String, String> = [("TYR".to_string(), "history/countries/TYR - Tyrol.txt".to_string())].into();
        let report = copy_into(&out, &map, &hist, None).unwrap();
        let h = std::fs::read_to_string(out.join("history/countries/TYR - Tyrol.txt")).unwrap();
        assert!(h.starts_with("capital = 3\n") && h.trim_end().ends_with("set_politics = { ruling_party = fascism }"), "{h}");
        assert!(out.join("common/national_focus/tyr.txt").is_file());
        assert!(out.join("common/ideologies/x.txt").is_file());
        // both localisation files are there, the content's with the byte order mark
        assert!(std::fs::read_to_string(out.join("localisation/english/suedtirol_l_english.yml")).unwrap().contains("Bozen"));
        let mine = std::fs::read(out.join("localisation/english/suedtirol_TYR_l_english.yml")).unwrap();
        assert!(mine.starts_with(&[0xef, 0xbb, 0xbf]));
        assert_eq!(std::fs::read_to_string(out.join("gfx/flags/TYR.tga")).unwrap(), "my flag");
        assert!(!out.join("a").exists() && !out.join(".snapshots").exists());
        assert!(report.iter().any(|l| l.contains("replaces the built one")), "{report:?}");
        assert!(report[0].starts_with("content: 5 files"), "{report:?}");
    }

    #[test]
    fn a_package_for_no_nation_is_an_error() {
        let (map, out) = dirs("orphan");
        put(&map, "content/GER/events/x.txt", "add_namespace = x\n");
        let hist: BTreeMap<String, String> = [("TYR".to_string(), "history/countries/TYR - Tyrol.txt".to_string())].into();
        let e = copy_into(&out, &map, &hist, None).unwrap_err().to_string();
        assert!(e.contains("GER") && e.contains("TYR"), "{e}");
    }

    #[test]
    fn no_content_is_nothing() {
        let (map, out) = dirs("none");
        assert!(copy_into(&out, &map, &BTreeMap::new(), None).unwrap().is_empty());
    }

    /// Province 2 and state 2 were deleted: 3 and up move down by one in the
    /// game; the numbers that name them in the scripts follow, the others stay.
    #[test]
    fn ids_follow_the_games_numbering() {
        let ids = Renumbering { provinces: vec![0, 1, 0, 2, 3, 4], states: vec![0, 1, 0, 2, 3] };
        let oob = "units = {\n\tdivision = { location = 5 division_template = \"Inf\" start_experience_factor = 3 }\n\tfleet = { naval_base = 4 task_force = { location = 3 } }\n}\nair_wings = { 4 = { fighter_equipment_0 = { owner = TYR amount = 3 } } }\n";
        let (t, n) = renumber_ids("history/units/TYR_1936.txt", oob, &ids).unwrap();
        assert_eq!(t, oob.replace("location = 5", "location = 4").replace("naval_base = 4", "naval_base = 3").replace("location = 3 }", "location = 2 }").replace("air_wings = { 4", "air_wings = { 3"));
        assert_eq!(n, 4);
        let eff = "\u{feff}capital = 3\nset_research_slots = 3\nif = { limit = { owns_state = 4 } 4 = { add_core_of = TYR } }\nrandom_list = { 3 = { transfer_state = 3 } 4 = { } }\nadd_building_construction = { type = naval_base province = 5 level = 1 }\nset_province_name = { id = 3 name = \"4\" }\nbuildings = { 5 = { naval_base = 3 } }\n";
        let (t, _) = renumber_ids("common/national_focus/x.txt", eff, &ids).unwrap();
        assert_eq!(
            t,
            "\u{feff}capital = 2\nset_research_slots = 3\nif = { limit = { owns_state = 3 } 3 = { add_core_of = TYR } }\nrandom_list = { 3 = { transfer_state = 2 } 4 = { } }\nadd_building_construction = { type = naval_base province = 4 level = 1 }\nset_province_name = { id = 2 name = \"4\" }\nbuildings = { 4 = { naval_base = 3 } }\n"
        );
        // a number the map has nothing for, and a file without any: as they are
        assert_eq!(renumber_ids("history/units/x.txt", "units = { division = { location = 2 } }", &ids), None);
        assert_eq!(renumber_ids("common/ideas/x.txt", "ideas = { country = { x = { cost = 4 } } }", &ids), None);

        let (map, out) = dirs("renumber");
        put(&out, "history/countries/TYR - Tyrol.txt", "capital = 2\n");
        put(&map, "content/TYR/history/countries/TYR - Tyrol.txt", "capital = 3\n");
        put(&map, "content/TYR/history/units/TYR_1936.txt", oob);
        let hist: BTreeMap<String, String> = [("TYR".to_string(), "history/countries/TYR - Tyrol.txt".to_string())].into();
        let report = copy_into(&out, &map, &hist, Some(&ids)).unwrap();
        assert!(std::fs::read_to_string(out.join("history/countries/TYR - Tyrol.txt")).unwrap().ends_with("capital = 2\n"));
        assert!(std::fs::read_to_string(out.join("history/units/TYR_1936.txt")).unwrap().contains("location = 4"));
        assert!(report.iter().any(|l| l.contains("5 province and state numbers in 2 files")), "{report:?}");
        assert_eq!(std::fs::read_to_string(map.join("content/TYR/history/units/TYR_1936.txt")).unwrap(), oob, "the content keeps the editor's ids");
    }

    #[test]
    fn history_tags() {
        assert_eq!(history_tag("history/countries/TYR - Tyrol.txt").as_deref(), Some("TYR"));
        assert_eq!(history_tag("history/countries/B01.txt").as_deref(), Some("B01"));
        assert_eq!(history_tag("history/countries/Tyrol.txt"), None);
        assert_eq!(history_tag("history/states/1-x.txt"), None);
    }
}
