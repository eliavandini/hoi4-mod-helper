//! Tests on a copy of the mod's real map (ignored by default, they need the
//! map files: `cargo test --release -- --ignored editor`). The copy goes to
//! `$MOD_HELPER_TEST_OUT` (or the system temp folder); the originals are only
//! read.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::doc::Doc;
use super::model::{self, Id, Kind};
use super::ops::{Constraint, GenParams, Retile};
use super::validate::{self, Issue, Severity, Snapshot};
use super::{default_map_dir, density};
use crate::util::Progress;

fn out_dir(name: &str) -> PathBuf {
    let base = std::env::var_os("MOD_HELPER_TEST_OUT").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let d = base.join(format!("mod-helper-test-{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Copy the map's own files; link the (read-only) terrain inputs.
fn copy_map(to: &Path) {
    let src = default_map_dir().expect("maps folder not found");
    for f in ["provinces.bmp", "provinces.json", "provinces.wld", "provinces.bmp.aux.xml"] {
        if src.join(f).exists() {
            std::fs::copy(src.join(f), to.join(f)).unwrap();
        }
    }
    for f in ["dem_section.tif", "landcover_smooth.bmp", "landcover_smooth.wld", "landcover_smooth.bmp.aux.xml"] {
        if src.join(f).exists() {
            std::os::unix::fs::symlink(src.join(f), to.join(f)).unwrap();
        }
    }
}

fn open(dir: &Path, terrain: bool) -> Doc {
    let p = Progress::default();
    let l = model::load(dir, &p).unwrap();
    let mut doc = Doc::new(dir.to_path_buf(), l);
    if terrain {
        let t = density::load_terrain(dir, doc.w, doc.h, doc.data.pixel_m, &p).unwrap();
        doc.terrain = Some(Arc::new(t));
    }
    doc
}

fn issues(doc: &Doc) -> Vec<Issue> {
    validate::validate(&Snapshot::of(doc))
}

fn errors(doc: &Doc) -> Vec<String> {
    issues(doc).into_iter().filter(|i| i.severity == Severity::Error).map(|i| i.msg).collect()
}

/// Run an edit as a transaction; then the map must have no more errors than before.
fn step(doc: &mut Doc, label: &str, f: impl FnOnce(&mut Doc) -> anyhow::Result<String>) {
    let before = errors(doc);
    doc.begin(label);
    let t0 = std::time::Instant::now();
    let r = f(doc);
    match r {
        Ok(m) => {
            doc.commit();
            println!("{label}: {m} ({:.2}s)", t0.elapsed().as_secs_f32());
        }
        Err(e) => {
            doc.rollback();
            panic!("{label} failed: {e:#}");
        }
    }
    let after = errors(doc);
    let new: Vec<&String> = after.iter().filter(|e| !before.contains(e)).collect();
    assert!(new.is_empty() || after.len() <= before.len(), "{label} introduced errors: {new:#?}");
}

#[test]
#[ignore = "needs the mod's map files (cargo test -- --ignored)"]
fn real_map_loads_validates_and_round_trips() {
    let dir = out_dir("roundtrip");
    copy_map(&dir);
    let doc = open(&dir, false);
    let all = issues(&doc);
    let errs: Vec<&Issue> = all.iter().filter(|i| i.severity == Severity::Error).collect();
    println!(
        "{} provinces, {} states, {} nations, {} zones, {} adjacencies; {} errors, {} warnings",
        doc.data.provinces.count(),
        doc.data.states.count(),
        doc.data.nations.count(),
        doc.data.regions.count(),
        doc.data.adjacencies.len(),
        errs.len(),
        all.len() - errs.len()
    );
    for i in all.iter().take(15) {
        println!("  {:?} {}", i.severity, i.msg);
    }
    assert!(errs.is_empty(), "the generated map has errors");

    // save unchanged, read again: the same map
    let mut doc = doc;
    let labels0 = doc.labels.read().px.clone();
    let data0 = doc.data.clone();
    doc.dirty = true;
    let r = doc.save().unwrap();
    assert!(!r.renumbered, "ids of the generated map are 1..n already");
    let doc2 = open(&dir, false);
    assert_eq!(doc2.labels.read().px, labels0);
    assert_eq!(doc2.data.provinces.count(), data0.provinces.count());
    for (p, pr) in data0.provinces.iter() {
        let q = doc2.data.provinces.get(p).unwrap();
        assert_eq!(
            (pr.rgb, pr.kind, &pr.name, pr.state, pr.region, pr.coastal, pr.edge, pr.nation),
            (q.rgb, q.kind, &q.name, q.state, q.region, q.coastal, q.edge, q.nation),
            "province {p}"
        );
    }
    for (s, st) in data0.states.iter() {
        let t = doc2.data.states.get(s).unwrap();
        assert_eq!((&st.name, st.nation, st.impassable), (&t.name, t.nation, t.impassable), "state {s}");
    }
    assert_eq!(doc2.data.adjacencies.len(), data0.adjacencies.len());
    assert!(dir.join("backups").is_dir());
}

#[test]
#[ignore = "needs the mod's map files (cargo test -- --ignored)"]
fn real_map_edits_keep_it_valid() {
    let dir = out_dir("edits");
    copy_map(&dir);
    let mut doc = open(&dir, true);
    let g = GenParams::default();
    let labels0 = doc.labels.read().px.clone();
    let (np0, ns0, nn0, nr0) = (doc.data.provinces.count(), doc.data.states.count(), doc.data.nations.count(), doc.data.regions.count());
    let d = &doc.data;
    let km2 = d.km2_per_px();

    // the nation with the most provinces, and its largest province
    let big_nation = d.nations.ids().into_iter().max_by_key(|&n| doc.index.states_of_nation(n).iter().map(|&s| doc.index.provs_of_state(s).len()).sum::<usize>()).unwrap();
    let nation_provs: Vec<Id> = doc.index.states_of_nation(big_nation).iter().flat_map(|&s| doc.index.provs_of_state(s).to_vec()).collect();
    println!("nation {} with {} provinces", d.nation_name(big_nation), nation_provs.len());

    // provinces
    step(&mut doc, "generate by terrain", |d| {
        let (a, b) = d.retile(&nation_provs, Retile::Density, &g)?;
        Ok(format!("{a} -> {b}"))
    });
    let p = doc.data.provinces.ids().into_iter().filter(|&p| doc.data.kind(p) == Kind::Land).max_by_key(|&p| doc.stats.get(p).px).unwrap();
    step(&mut doc, "divide", |d| {
        let (a, b) = d.retile(&[p], Retile::Count(3), &g)?;
        Ok(format!("{a} -> {b}"))
    });
    let (a, b) = {
        let d = &doc.data;
        let a = d.provinces.ids().into_iter().find(|&p| d.kind(p) == Kind::Land && !d.impassable(p) && doc.stats.neighbours(p).iter().any(|&q| d.state_of(q) == d.state_of(p) && q != p)).unwrap();
        let b = *doc.stats.neighbours(a).iter().find(|&&q| d.state_of(q) == d.state_of(a) && q != a).unwrap();
        (a, b)
    };
    step(&mut doc, "merge", |d| Ok(format!("{}", d.merge_provinces(&[a, b])?)));
    let victim = doc.data.provinces.ids().into_iter().find(|&p| doc.data.kind(p) == Kind::Land && doc.stats.get(p).px > 500).unwrap();
    step(&mut doc, "dissolve", |d| Ok(format!("{}", d.dissolve_provinces(&[victim])?)));
    // paint a stroke with the first land province over its surroundings
    let target = doc.data.provinces.ids().into_iter().find(|&p| doc.data.kind(p) == Kind::Land && doc.stats.get(p).px > 2000).unwrap();
    let c = doc.stats.get(target).centre();
    step(&mut doc, "paint", |d| Ok(format!("{} px", d.paint([c[0] - 60.0, c[1]], [c[0] + 60.0, c[1] + 30.0], 8.0, target, Constraint::default()))));
    step(&mut doc, "fix crossings", |d| Ok(format!("{} px", d.fix_crossings(None))));

    // states and nations
    let nation = doc.data.nations.ids().into_iter().max_by_key(|&n| doc.index.states_of_nation(n).len()).unwrap();
    step(&mut doc, "generate states", |d| Ok(format!("{}", d.generate_states(&[nation], &g)?)));
    let s = doc.index.states_of_nation(nation).iter().copied().max_by_key(|&s| doc.index.provs_of_state(s).len()).unwrap();
    step(&mut doc, "divide state", |d| Ok(format!("{:?}", d.divide_state(s, 2)?)));
    let pair: Vec<Id> = doc.index.states_of_nation(nation).iter().copied().take(2).collect();
    step(&mut doc, "merge states", |d| Ok(format!("{}", d.merge_states(&pair)?)));
    let s1 = *doc.index.states_of_nation(nation).iter().find(|&&s| !doc.data.states.get(s).unwrap().impassable).unwrap();
    let mut new_nation = 0;
    step(&mut doc, "new nation", |d| {
        new_nation = d.new_nation_from(&[s1])?;
        Ok(d.data.nation_name(new_nation))
    });
    step(&mut doc, "merge nations", |d| Ok(format!("{}", d.merge_nations(&[nation, new_nation])?)));
    let neighbours: Vec<Id> = {
        let mut v = vec![nation];
        for &s in doc.index.states_of_nation(nation) {
            for &p in doc.index.provs_of_state(s) {
                for &q in doc.stats.neighbours(p) {
                    let n = doc.data.nation_of(q);
                    if n != 0 && !v.contains(&n) && v.len() < 4 {
                        v.push(n);
                    }
                }
            }
        }
        v
    };
    step(&mut doc, "share out nations", |d| Ok(format!("{}", d.generate_nations(&neighbours)?)));

    // air and sea zones
    let air: Vec<Id> = doc.data.regions.iter().filter(|(_, r)| !r.naval).map(|(r, _)| r).collect();
    step(&mut doc, "generate air zones", |d| Ok(format!("{}", d.generate_regions(&air, &g, None)?)));
    let sea: Vec<Id> = doc.data.regions.iter().filter(|(_, r)| r.naval).map(|(r, _)| r).collect();
    step(&mut doc, "generate sea zones", |d| Ok(format!("{}", d.generate_regions(&sea, &g, Some(sea.len().max(2)))?)));

    // inaccessible zones: the highest provinces
    let t = doc.terrain.clone().unwrap();
    let mut high: Vec<(f32, Id)> = doc
        .data
        .provinces
        .iter()
        .filter(|(_, pr)| pr.kind == Kind::Land)
        .map(|(p, _)| {
            let c = doc.stats.get(p).centre();
            (t.dem[c[1] as usize * t.w as usize + c[0] as usize], p)
        })
        .collect();
    high.sort_by(|a, b| b.0.total_cmp(&a.0));
    let highest: Vec<Id> = high.iter().take(30).map(|x| x.1).collect();
    let next: Vec<Id> = high.iter().skip(30).take(30).map(|x| x.1).collect();
    step(&mut doc, "make impassable", |d| Ok(format!("{}", d.make_impassable(&highest, true, &g)?)));
    step(&mut doc, "remove from play", |d| Ok(format!("{}", d.remove_from_play(&next, true, &g)?)));
    let lakes = doc.data.provinces.iter().filter(|(_, p)| p.kind == Kind::Lake).count();
    assert!(lakes > 0, "remove from play made no lake provinces");

    // adjacency: a strait between two land provinces across a sea province
    let found = doc.data.provinces.iter().filter(|(_, p)| p.kind == Kind::Sea).find_map(|(s, _)| {
        let lands: Vec<Id> = doc.stats.neighbours(s).iter().copied().filter(|&q| doc.data.kind(q) == Kind::Land && !doc.data.impassable(q)).collect();
        lands.iter().enumerate().find_map(|(i, &x)| {
            lands[i + 1..]
                .iter()
                .copied()
                .find(|&y| doc.stats.border_len(x, y) == 0 && !doc.data.adjacencies.iter().any(|a| (a.from, a.to) == (x, y) || (a.from, a.to) == (y, x)))
                .map(|y| (x, y))
        })
    });
    if let Some((x, y)) = found {
        step(&mut doc, "add strait", |d| {
            let i = d.add_adjacency(x, y, "sea", "", "")?;
            let a = &d.data.adjacencies[i];
            anyhow::ensure!(a.through.is_some(), "strait without a sea province");
            Ok(a.name.clone())
        });
    }

    // impassable borders (the border tool): the land province with the most land neighbours
    // and its longest land border
    let (x, y) = {
        let d = &doc.data;
        let land = |q: Id| q != 0 && d.kind(q) == Kind::Land && !d.impassable(q);
        let x = d.provinces.ids().into_iter().filter(|&p| land(p)).max_by_key(|&p| doc.stats.neighbours(p).iter().filter(|&&q| land(q)).count()).unwrap();
        let y = doc.stats.neighbours(x).iter().copied().filter(|&q| land(q)).max_by_key(|&q| doc.stats.border_len(x, q)).unwrap();
        (x, y)
    };
    let lines = doc.border_lines(x, y);
    let len: f64 = lines.iter().flat_map(|l| l.windows(2)).map(|w| (w[1][0] - w[0][0]).abs() + (w[1][1] - w[0][1]).abs()).sum();
    assert_eq!(len as u32, doc.stats.border_len(x, y), "the border lines cover the whole border");
    // the middle of the first pixel edge, and the pixel centres on either side of it
    let (s0, s1) = (lines[0][0], lines[0][1]);
    let m = [(s0[0] + s1[0]) / 2.0, (s0[1] + s1[1]) / 2.0];
    let sign = |v: f64| if v > 0.0 { 1.0 } else if v < 0.0 { -1.0 } else { 0.0 }; // 0.0.signum() is 1
    let unit = [sign(s1[0] - s0[0]), sign(s1[1] - s0[1])];
    let side = |k: f64| [m[0] - unit[1] * 0.5 * k, m[1] + unit[0] * 0.5 * k];
    let near = doc.border_near(m, 2.0).expect("a border next to the click");
    assert_eq!((near.0.min(near.1), near.0.max(near.1)), (x.min(y), x.max(y)));
    let crossed = doc.borders_crossed(side(-1.0), side(1.0));
    assert_eq!(crossed.len(), 1, "{crossed:?}");
    assert_eq!((crossed[0].0.min(crossed[0].1), crossed[0].0.max(crossed[0].1)), (x.min(y), x.max(y)));
    step(&mut doc, "impassable border (click)", |d| Ok(format!("{}", d.set_border_impassable(x, y, true)?)));
    assert!(doc.impassable_border(y, x).is_some());
    assert!(!doc.set_border_impassable(x, y, true).unwrap(), "blocking a blocked border changes nothing");
    step(&mut doc, "open border (stroke)", |d| {
        let n = d.borders_crossed(side(-1.0), side(1.0)).into_iter().filter(|&(a, b)| d.set_border_impassable(a, b, false).unwrap_or(false)).count();
        Ok(format!("{n} opened"))
    });
    assert!(doc.impassable_border(x, y).is_none());
    step(&mut doc, "impassable border again", |d| Ok(format!("{}", d.set_border_impassable(y, x, true)?)));
    // the wall follows the pixels: divide x, and every piece along y stays walled off
    let touched: Vec<(Id, Id)> = doc.stats.border.keys().copied().collect();
    step(&mut doc, "divide a walled province", |d| {
        let (a, b) = d.retile(&[x], Retile::Count(3), &g)?;
        Ok(format!("{a} -> {b}"))
    });
    let pieces: Vec<Id> = doc.stats.neighbours(y).iter().copied().filter(|&q| q == x || !touched.contains(&(q.min(y), q.max(y)))).collect();
    assert!(pieces.len() > 1, "the divided province meets y in several pieces: {pieces:?}");
    for q in pieces {
        assert!(doc.impassable_border(q, y).is_some(), "a gap in the wall between {q} and {y}");
    }
    let blocked = doc.data.adjacencies.iter().filter(|a| a.kind == "impassable").count();
    // the frame and the mod details as the map has them, then changed
    let frame0 = doc.data.map_frame();
    step(&mut doc, "map frame toggled", |d| {
        d.data.set_map_frame(!frame0);
        Ok(String::new())
    });
    let meta0 = doc.data.mod_meta();
    let meta = crate::modbuild::ModMeta {
        name: "Edited \"test\" map".into(),
        folder: "mod_helper_test".into(),
        version: "0.2".into(),
        author: "tests".into(),
        description: "made by the editor tests".into(),
        tags: vec!["Map".into(), "Alternative History".into(), "Own tag".into()],
        dependencies: vec!["Some other mod".into()],
        seed: 7,
        ..Default::default()
    };
    step(&mut doc, "mod details", |d| {
        d.data.set_mod_meta(&meta);
        Ok(String::new())
    });
    doc.undo();
    assert_eq!(doc.data.mod_meta(), meta0, "undo restores the mod details");
    doc.redo();
    assert_eq!(doc.data.mod_meta(), meta);

    let n_after = (doc.data.provinces.count(), doc.data.states.count(), doc.data.nations.count(), doc.data.regions.count());
    println!("after edits: {n_after:?} (before {:?})", (np0, ns0, nn0, nr0));
    let errs = errors(&doc);
    assert!(errs.is_empty(), "errors after the edits: {errs:#?}");
    // box limit of generated provinces
    for (p, _) in doc.data.provinces.iter() {
        let s = doc.stats.get(p);
        assert!(s.box_w() < doc.w / 8 && s.box_h() < doc.h / 8, "province {p} too large");
        assert!(s.px >= 8, "province {p} too small");
    }
    let _ = km2;

    // save and read again
    doc.save().unwrap();
    let doc2 = open(&dir, false);
    let errs2 = errors(&doc2);
    assert!(errs2.is_empty(), "errors after save + load: {errs2:#?}");
    assert_eq!(doc2.data.provinces.count(), n_after.0);
    assert_eq!(doc2.data.provinces.iter().filter(|(_, p)| p.kind == Kind::Lake).count(), lakes);
    assert_eq!(doc2.data.adjacencies.iter().filter(|a| a.kind == "impassable").count(), blocked);
    assert_eq!(doc2.data.map_frame(), !frame0, "the map frame setting is saved");
    assert_eq!(doc2.data.mod_meta(), meta, "the mod details are saved");

    // the edited map makes a mod that passes the checks
    let dirs = crate::modbuild::Dirs { game: crate::modbuild::default_game_dir(), mods: out_dir("edits-mods") };
    if dirs.game.join("map/provinces.bmp").exists() {
        let mod_dir = crate::modbuild::build(&dir, &dirs, &|l| println!("{l}")).expect("build");
        assert_eq!(mod_dir, dirs.mods.join("mod_helper_test"));
        let errors = crate::modbuild::validate::validate(&mod_dir, &dirs.game, &|l| println!("{l}")).unwrap();
        assert!(errors.is_empty(), "the built mod fails checks: {errors:#?}");
        let desc = std::fs::read_to_string(mod_dir.join("descriptor.mod")).unwrap();
        assert!(desc.contains("name=\"Edited 'test' map\"") && desc.contains("\"Own tag\"") && desc.contains("dependencies={"), "{desc}");
        assert!(std::fs::read_to_string(mod_dir.join("README.txt")).unwrap().contains("by tests"));
        let frame_built = std::fs::read_to_string(mod_dir.join("map/ambient_object.txt")).unwrap().contains("frame_border");
        assert_eq!(frame_built, !frame0, "the frame as set");
        // building again replaces the folder it made; a folder it did not make is left alone
        crate::modbuild::build(&dir, &dirs, &|_| {}).expect("rebuild");
        std::fs::remove_file(mod_dir.join(crate::modbuild::MARKER)).unwrap();
        assert!(crate::modbuild::build(&dir, &dirs, &|_| {}).is_err(), "a folder not made by mod-helper is replaced");
        assert!(mod_dir.join("descriptor.mod").exists(), "the failed build removed the folder");
    } else {
        println!("HOI4 not found at {}: build not tested", dirs.game.display());
    }

    // undo everything: the original pixels again
    while doc.undo().is_some() {}
    assert_eq!(doc.labels.read().px, labels0, "undo did not restore the pixels");
    assert_eq!(
        (doc.data.provinces.count(), doc.data.states.count(), doc.data.nations.count(), doc.data.regions.count()),
        (np0, ns0, nn0, nr0)
    );
    // and redo all of it
    while doc.redo().is_some() {}
    assert_eq!(doc.data.provinces.count(), n_after.0);
}
