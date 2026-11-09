//! Tests on a copy of the mod's real map (ignored by default, they need the
//! map files: `MOD_HELPER_TEST_MAPS=<the maps folder> cargo test --release --
//! --ignored editor`). The copy goes to `$MOD_HELPER_TEST_OUT` (or the system
//! temp folder); the originals are only read.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::doc::Doc;
use super::model::{self, Id, Kind};
use super::ops::{Constraint, GenParams, Retile};
use super::validate::{self, Issue, Severity, Snapshot};
use super::density;
use crate::modbuild::bitmaps::{self, Sources};
use crate::util::Progress;

fn out_dir(name: &str) -> PathBuf {
    let base = std::env::var_os("MOD_HELPER_TEST_OUT").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let d = base.join(format!("mod-helper-test-{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// The real map's folder.
fn map_src() -> PathBuf {
    crate::util::test_maps_dir().join("processed maps/suedtirol")
}

/// The real map's elevation and land cover (read where they are).
fn sources() -> Sources {
    let src = map_src();
    Sources { dem: Some(src.join(bitmaps::DEM_FILE)), cover: Some(src.join(bitmaps::COVER_FILE)) }
}

/// Where the HOI4 install is, mods go to `out_dir(mods)`, the real map's data.
fn dirs(mods: &str) -> crate::modbuild::Dirs {
    crate::modbuild::Dirs { game: crate::modbuild::default_game_dir(), mods: out_dir(mods), sources: sources() }
}

/// Copy the map's own files.
fn copy_map(to: &Path) {
    let src = map_src();
    for f in ["provinces.bmp", "provinces.json", "provinces.wld", "provinces.bmp.aux.xml"] {
        if src.join(f).exists() {
            std::fs::copy(src.join(f), to.join(f)).unwrap();
        }
    }
}

fn open(dir: &Path, terrain: bool) -> Doc {
    let p = Progress::default();
    let l = model::load(dir, &p).unwrap();
    let mut doc = Doc::new(dir.to_path_buf(), l);
    if terrain {
        let t = density::load_terrain(&sources(), doc.w, doc.h, doc.data.pixel_m, doc.heights.as_deref().map(|v| v.as_slice()), &p).unwrap();
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

/// Provinces under HOI4's 8 px.
fn tiny(doc: &Doc) -> usize {
    errors(doc).iter().filter(|e| e.contains("HOI4 needs at least 8")).count()
}

/// Points where four provinces meet or that touch only diagonally.
fn crossings(doc: &Doc) -> usize {
    errors(doc).iter().filter(|e| e.contains("four provinces meet")).map(|e| e.split(' ').next().unwrap().parse::<usize>().unwrap()).sum()
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
#[ignore = "needs the mod's map files (MOD_HELPER_TEST_MAPS=<maps folder> cargo test -- --ignored)"]
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
    // a map being worked on may have errors; it must keep the same ones
    let msgs0: Vec<String> = all.iter().map(|i| i.msg.clone()).collect();

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
    let msgs: Vec<String> = issues(&doc2).into_iter().map(|i| i.msg).collect();
    assert_eq!(msgs, msgs0, "the checks differ after save + load");

    // "Backups" in the editor: an edit after the save, new mod details, then
    // load the backup the save made: the edit goes, the mod details stay,
    // and it is one step to undo
    let mut doc3 = doc2;
    let p1 = doc3.data.provinces.ids()[0];
    let name0 = doc3.data.provinces.get(p1).unwrap().name.clone();
    doc3.begin("rename");
    doc3.data.provinces.get_mut(p1).unwrap().name = "Changed".into();
    let mut meta = doc3.data.mod_meta();
    meta.name = "Kept".into();
    doc3.data.set_mod_meta(&meta);
    doc3.commit();
    let mut ed = super::Editor::new(super::Settings::default());
    ed.doc = Some(doc3);
    let list = model::backups(&dir);
    assert_eq!(list.len(), 1);
    let (tx, rx) = crossbeam_channel::bounded(1);
    tx.send(model::load(&list[0].dir, &Progress::default()).map_err(|e| e.to_string())).unwrap();
    ed.backups = Some(list.clone());
    ed.restoring = Some((list[0].clone(), rx));
    let ctx = eframe::egui::Context::default();
    let mut out = ctx.run_ui(eframe::egui::RawInput::default(), |ui| ed.backups_window(ui.ctx()));
    out.textures_delta.clear(); // no renderer here
    assert!(ed.restoring.is_none() && ed.backups.is_none(), "the backup was not loaded");
    let d = ed.doc.as_mut().unwrap();
    assert_eq!(d.data.provinces.get(p1).unwrap().name, name0, "the edit after the save is still there");
    assert_eq!(d.data.mod_meta().name, "Kept", "loading a backup changed the mod details");
    assert_eq!(d.can_undo(), Some(format!("Load backup {}", list[0].name).as_str()));
    assert!(d.dirty);
    d.undo();
    assert_eq!(d.data.provinces.get(p1).unwrap().name, "Changed");
}

#[test]
#[ignore = "needs the mod's map files (MOD_HELPER_TEST_MAPS=<maps folder> cargo test -- --ignored)"]
fn real_map_edits_keep_it_valid() {
    let dir = out_dir("edits");
    copy_map(&dir);
    let mut doc = open(&dir, true);
    let g = GenParams::default();
    // a map being worked on may have errors: then the edits must not add any
    let errs0 = errors(&doc);
    let clean = errs0.is_empty();
    println!("{} errors at the start", errs0.len());
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
    step(&mut doc, "divide state", |d| Ok(format!("{:?}", d.divide_state(s, 2, g.organic)?)));
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

    // inaccessible zones: the highest provinces (not in capital states: taking
    // a nation's capital out of play is an error until another is chosen)
    let t = doc.terrain.clone().unwrap();
    let capitals: Vec<Id> = doc.data.nations.iter().map(|(_, n)| n.capital_state).collect();
    let mut high: Vec<(f32, Id)> = doc
        .data
        .provinces
        .iter()
        .filter(|(_, pr)| pr.kind == Kind::Land && !capitals.contains(&pr.state))
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

    // impassable borders (the border tool): the longest border between two land provinces
    let (x, y) = {
        let d = &doc.data;
        let land = |q: Id| q != 0 && d.kind(q) == Kind::Land && !d.impassable(q);
        // a long border between two land provinces where dividing one of them
        // (as below, tried and undone) leaves several pieces along the other
        let mut pairs: Vec<(Id, Id)> = d
            .provinces
            .ids()
            .into_iter()
            .filter(|&p| land(p))
            .flat_map(|p| doc.stats.neighbours(p).iter().copied().filter(move |&q| q != p && land(q)).map(move |q| (p, q)))
            .collect();
        pairs.sort_by_key(|&(p, q)| (std::cmp::Reverse(doc.stats.border_len(p, q)), p, q));
        pairs
            .into_iter()
            .take(40)
            .find(|&(x, y)| {
                let cap = doc.data.provinces.cap() as Id;
                doc.begin("try");
                let ok = doc.retile(&[x], Retile::Count(3), &g).is_ok() && {
                    doc.restat();
                    doc.stats.neighbours(y).iter().filter(|&&q| q == x || q >= cap).count() > 1
                };
                doc.rollback();
                ok
            })
            .expect("no border where a divided province meets its neighbour in several pieces")
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
    if clean {
        assert!(errs.is_empty(), "errors after the edits: {errs:#?}");
        // box limit of generated provinces
        for (p, _) in doc.data.provinces.iter() {
            let s = doc.stats.get(p);
            assert!(s.box_w() < doc.w / 8 && s.box_h() < doc.h / 8, "province {p} too large");
            assert!(s.px >= 8, "province {p} too small");
        }
    } else {
        assert!(errs.len() <= errs0.len(), "{} errors after the edits, {} before", errs.len(), errs0.len());
    }
    let _ = km2;

    // save and read again
    doc.save().unwrap();
    let doc2 = open(&dir, false);
    let errs2 = errors(&doc2);
    assert_eq!(errs2.len(), errs.len(), "errors after save + load: {errs2:#?}");
    assert_eq!(doc2.data.provinces.count(), n_after.0);
    assert_eq!(doc2.data.provinces.iter().filter(|(_, p)| p.kind == Kind::Lake).count(), lakes);
    assert_eq!(doc2.data.adjacencies.iter().filter(|a| a.kind == "impassable").count(), blocked);
    assert_eq!(doc2.data.map_frame(), !frame0, "the map frame setting is saved");
    assert_eq!(doc2.data.mod_meta(), meta, "the mod details are saved");

    // the edited map makes a mod that passes the checks
    let dirs = dirs("edits-mods");
    if dirs.game.join("map/provinces.bmp").exists() {
        let mod_dir = crate::modbuild::build(&dir, &dirs, &|l| println!("{l}")).expect("build");
        assert_eq!(mod_dir, dirs.mods.join("mod_helper_test"));
        let errors = crate::modbuild::validate::validate(&mod_dir, &dirs.game, &|l| println!("{l}")).unwrap();
        assert!(errors.is_empty() || !clean, "the built mod fails checks: {errors:#?}");
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

    // the backup the save made is the map from before the edits; loading it
    // is one edit, which undo takes back
    let backups = model::backups(&dir);
    assert_eq!(backups.len(), 1, "{backups:?}");
    assert_eq!(backups[0].provinces, Some(np0));
    let edited = doc.labels.read().px.clone();
    doc.begin("load backup");
    let n = doc.replace_map(model::load(&backups[0].dir, &Progress::default()).unwrap()).unwrap();
    doc.commit();
    println!("load backup: {n} px differ");
    assert!(n > 0);
    assert_eq!(doc.labels.read().px, labels0, "the backup is the map before the edits");
    assert_eq!((doc.data.provinces.count(), doc.data.states.count(), doc.data.nations.count()), (np0, ns0, nn0));
    doc.undo();
    assert_eq!(doc.labels.read().px, edited, "undo brings the edited map back");
    assert_eq!(doc.data.provinces.count(), n_after.0);
}

/// "Province from image" on the real map: the georeferenced 45 m sea mask
/// lands on the map's sea; a province from a height band of height.bmp
/// (slopes below the peaks, which may be out of play already) takes exactly
/// the land pixels of the shape, and undo gives them back.
#[test]
#[ignore = "needs the mod's map files (MOD_HELPER_TEST_MAPS=<maps folder> cargo test -- --ignored)"]
fn real_map_province_from_image() {
    use super::from_image::{Channel, Coverage, Source, in_shape, sample};
    let dir = out_dir("from-image");
    copy_map(&dir);
    let mut doc = open(&dir, false);
    let maps = map_src().parent().unwrap().to_path_buf();

    let sea = Source::load(&maps.join("sea_45m.bmp")).unwrap();
    let (v, how) = sample(&sea, Channel::Red, doc.w, doc.h, &doc.geo);
    assert!(how.contains("georeference"), "{how}");
    let c = Coverage::of(&doc, &v, 128, true);
    let share = c.sea as f64 / (c.land + c.sea + c.lake) as f64;
    println!("sea mask: {c:?}, {share:.3} on sea provinces");
    assert!(share > 0.9, "the sea mask does not lie on the map's sea: {c:?}");

    let height = Source::load(&maps.join("height.bmp")).unwrap();
    let (v, _) = sample(&height, Channel::Brightness, doc.w, doc.h, &doc.geo);
    let band = |i: usize| in_shape(v[i], 130, false) && in_shape(v[i], 150, true);
    let before = doc.labels.read().px.clone();
    let expect: Vec<usize> = (0..before.len()).filter(|&i| band(i) && doc.data.kind(before[i]) == Kind::Land).collect();
    assert!(expect.len() > 1000, "too few high pixels: {}", expect.len());
    let n0 = doc.data.provinces.count();
    let crossings0 = crossings(&doc);
    let tiny0 = tiny(&doc);
    // not `step`: a province this big is meant to show up in the checks
    let t0 = std::time::Instant::now();
    doc.begin("province from image");
    let (made, n) = doc.province_from_shape(band, Kind::Land, true, "Slopes".into(), None).unwrap();
    doc.commit();
    assert_eq!(made.len(), 1, "land from an image is one province");
    let made = made[0];
    println!("province from image: {made}, {n} px in {:.2}s", t0.elapsed().as_secs_f32());
    // specks under 8 px stay, leftovers under 8 px go with the shape
    assert!(n.abs_diff(expect.len()) as f64 <= 0.01 * expect.len() as f64, "{n} px taken of {}", expect.len());
    assert!(tiny(&doc) <= tiny0, "{} provinces under 8 px, {tiny0} before", tiny(&doc));
    let lab = doc.labels.read().px.clone();
    // crossings along the edge move a few pixels to a neighbour of the same kind
    let kept = expect.iter().filter(|&&i| lab[i] == made).count();
    let outside = lab.iter().zip(&before).enumerate().filter(|&(i, (a, b))| a != b && !(band(i) && doc.data.kind(*b) == Kind::Land)).count();
    println!("{kept} of {} shape pixels kept, {outside} pixels outside the shape changed (crossing fixes)", expect.len());
    assert!(kept as f64 >= 0.995 * expect.len() as f64, "the province lost too much of the shape");
    assert!((outside as f64) <= 0.005 * expect.len() as f64, "too many pixels outside the shape changed");
    assert!(lab.iter().zip(&before).all(|(a, b)| a == b || doc.data.kind(*a) == doc.data.kind(*b)), "a pixel changed kind");
    let p = doc.data.provinces.get(made).unwrap();
    assert_eq!((p.kind, p.name.as_str()), (Kind::Land, "Slopes"));
    assert!(p.state != 0 && p.region != 0, "the province got no state or zone");
    assert!(crossings(&doc) <= crossings0, "new crossings: {} (map had {crossings0})", crossings(&doc));
    println!("{} provinces -> {} (entirely covered ones are gone)", n0, doc.data.provinces.count());
    doc.undo();
    assert_eq!(doc.labels.read().px, before, "undo did not give the pixels back");
}

/// "Province from image" as removed land works like "Remove from play": a
/// height band of height.bmp plus a dot on the sea become removed provinces
/// that keep their air zones, sea stays sea, and with "merge" they are cut to
/// HOI4's size limits.
#[test]
#[ignore = "needs the mod's map files (MOD_HELPER_TEST_MAPS=<maps folder> cargo test -- --ignored)"]
fn real_map_removed_land_from_image() {
    use super::from_image::{Channel, Source, in_shape, sample};
    let dir = out_dir("removed-from-image");
    copy_map(&dir);
    let mut doc = open(&dir, false);
    let maps = map_src().parent().unwrap().to_path_buf();
    let height = Source::load(&maps.join("height.bmp")).unwrap();
    let (v, _) = sample(&height, Channel::Brightness, doc.w, doc.h, &doc.geo);
    let before = doc.labels.read().px.clone();
    let w = doc.w as usize;
    // a dot of 20 px radius in the middle of the biggest sea province
    let sea = doc.data.provinces.iter().filter(|(_, p)| p.kind == Kind::Sea).map(|(id, _)| id).max_by_key(|&p| doc.stats.get(p).px).unwrap();
    let s = doc.stats.get(sea);
    let (cx, cy) = (((s.x0 + s.x1) / 2) as usize, ((s.y0 + s.y1) / 2) as usize);
    let dot = |i: usize| ((i % w) as i64 - cx as i64).pow(2) + ((i / w) as i64 - cy as i64).pow(2) <= 400;
    let shape = |i: usize| in_shape(v[i], 110, false) && in_shape(v[i], 140, true) || dot(i);
    assert!((0..before.len()).any(|i| dot(i) && doc.data.kind(before[i]) == Kind::Sea), "the dot is not on the sea");
    let crossings0 = crossings(&doc);
    let tiny0 = tiny(&doc);
    let zone_before: Vec<Id> = (0..doc.data.provinces.cap() as Id).map(|p| doc.data.region_of(p)).collect();
    let land: Vec<usize> = (0..before.len()).filter(|&i| shape(i) && doc.data.kind(before[i]) != Kind::Sea).collect();
    let mut zone_px: std::collections::BTreeMap<Id, usize> = Default::default();
    for &i in &land {
        *zone_px.entry(zone_before[before[i] as usize]).or_default() += 1;
    }
    println!("{} px of land and removed land by air zone: {zone_px:?}", land.len());
    let zones: std::collections::BTreeSet<Id> = zone_px.iter().filter(|(_, n)| **n >= 10_000).map(|(z, _)| *z).collect();
    assert!(zones.len() > 1, "the test needs a shape over several air zones");

    for coarsen in [false, true] {
        let g = GenParams::default();
        let t0 = std::time::Instant::now();
        doc.begin("removed land from image");
        let (made, n) = doc.province_from_shape(shape, Kind::Lake, true, "Slopes".into(), coarsen.then_some(&g)).unwrap();
        doc.commit();
        let made: Vec<Id> = made.into_iter().filter(|&p| doc.data.provinces.contains(p)).collect();
        println!("coarsen {coarsen}: {} provinces, {n} px in {:.2}s", made.len(), t0.elapsed().as_secs_f32());
        assert!(n.abs_diff(land.len()) as f64 <= 0.01 * land.len() as f64, "{n} px taken of {}", land.len());
        assert!(tiny(&doc) <= tiny0, "{} provinces under 8 px, {tiny0} before", tiny(&doc));
        let lab = doc.labels.read().px.clone();
        assert!(lab.iter().zip(&before).all(|(a, b)| a == b || doc.data.kind(*a) != Kind::Sea && doc.data.kind(*b) != Kind::Sea), "sea changed");
        for &p in &made {
            let pr = doc.data.provinces.get(p).unwrap();
            assert_eq!((pr.kind, pr.state, pr.name.as_str()), (Kind::Lake, 0, "Slopes"));
        }
        let mine: std::collections::HashSet<Id> = made.iter().copied().collect();
        let got = land.iter().filter(|&&i| mine.contains(&lab[i])).count();
        assert!(got as f64 >= 0.995 * land.len() as f64, "only {got} of {} px", land.len());
        // each pixel stays in its air zone (crossing fixes aside)
        let moved = land.iter().filter(|&&i| mine.contains(&lab[i]) && doc.data.region_of(lab[i]) != zone_before[before[i] as usize]).count();
        println!("{moved} px changed air zone");
        assert!(moved as f64 <= 0.001 * land.len() as f64, "{moved} px changed air zone");
        let per_zone: std::collections::BTreeSet<Id> = made.iter().map(|&p| doc.data.region_of(p)).collect();
        assert!(zones.is_subset(&per_zone) && per_zone.iter().all(|z| zone_px.contains_key(z)), "one province (or more) per air zone: {per_zone:?}");
        let errs = errors(&doc);
        let big: Vec<&String> = errs.iter().filter(|e| e.starts_with("Slopes") && e.contains("spans")).collect();
        println!("{} size errors: {big:?}", big.len());
        if coarsen {
            assert!(made.len() > zones.len(), "merge must cut the big parts");
            assert!(big.is_empty(), "cut provinces are still too big");
        } else {
            assert_eq!(made.len(), per_zone.len(), "without merge: one province per zone");
        }
        assert!(crossings(&doc) <= crossings0, "new crossings: {} (map had {crossings0})", crossings(&doc));
        doc.undo();
        assert_eq!(doc.labels.read().px, before, "undo did not give the pixels back");
    }
}

/// Rivers and supply on the real map: rivers from the heights follow the
/// rules of rivers.bmp; railways made editable lay the same links as the
/// automatic ones; a new railway, hubs and ports survive saving, and the mod
/// built from it passes the checks.
#[test]
#[ignore = "needs the mod's map files (MOD_HELPER_TEST_MAPS=<maps folder> cargo test -- --ignored)"]
fn real_map_rivers_and_supply() {
    use super::model::{Port, Railway};
    use super::{rivers, supply};
    let dir = out_dir("rivers-supply");
    copy_map(&dir);
    let mut doc = open(&dir, true);
    let (w, h) = (doc.w as usize, doc.h as usize);
    let t = doc.terrain.clone().unwrap();
    let sea: Vec<bool> = doc.labels.read().px.iter().map(|&p| doc.data.kind(p) == Kind::Sea).collect();
    let lake: Vec<bool> = doc.labels.read().px.iter().map(|&p| doc.data.kind(p) == Kind::Lake).collect();

    // rivers from the heights
    let t0 = std::time::Instant::now();
    let found = rivers::generate(&t.dem, &sea, &lake, w, h, doc.data.km2_per_px(), 40.0);
    let km: f64 = found.iter().map(|r| r.points.windows(2).map(|s| ((s[1].0 - s[0].0) as f64).hypot((s[1].1 - s[0].1) as f64)).sum::<f64>()).sum::<f64>() * doc.data.pixel_m / 1000.0;
    println!("{} rivers ({km:.0} km) from the heights in {:.2}s", found.len(), t0.elapsed().as_secs_f32());
    assert!(found.len() > 10, "{} rivers", found.len());
    let widest = found.iter().map(|r| r.points.iter().map(|p| p.2).max().unwrap()).max().unwrap();
    assert!(widest >= 6, "the main rivers are wide: {widest}");
    step(&mut doc, "Generate rivers", |d| {
        d.data.rivers = found;
        Ok(String::new())
    });
    let t0 = std::time::Instant::now();
    let ras = rivers::rasterize(&doc.data.rivers, w, h, supply::ground(&doc.data, &doc.labels.read()));
    let img = ras.image(w, h, |i| sea[i]);
    let breaks = rivers::check(&img, w, h);
    println!("rivers.bmp: {} px, {} notes, in {:.2}s; {:?}", ras.px.len(), ras.problems.len(), t0.elapsed().as_secs_f32(), ras.problems.iter().take(5).map(|p| &p.msg).collect::<Vec<_>>());
    assert!(breaks.is_empty(), "{} breaks: {:?}", breaks.len(), &breaks[..breaks.len().min(10)]);
    assert!((0..w * h).all(|i| !sea[i] || !rivers::is_river(img[i])), "no river pixel in the sea");

    // supply: the map's own railways, hubs and ports (as last saved) ...
    let urban = doc.sealed_km2().unwrap();
    let own = supply::plan(&doc.data, &doc.stats, &doc.labels.read(), &Default::default());
    println!(
        "the map's supply: railways {} ({} links, {} faults), hubs {} ({} off land), {} ports",
        if own.custom_rail { "laid" } else { "automatic" },
        own.lines.level.len(),
        own.lines.faults.len(),
        own.hubs.len(),
        own.bad_hubs.len(),
        own.ports.len()
    );
    // ... then automatic (on this copy), then editable
    step(&mut doc, "Automatic supply", |d| {
        d.data.railways = None;
        d.data.supply_hubs = None;
        d.data.ports.clear();
        Ok(String::new())
    });
    let types = doc.data.mod_meta().terrain;
    let counts = supply::land_counts(&doc.labels.read(), &t, &types.kinds(), doc.data.provinces.cap());
    let (terr, _) = supply::province_terrain(&doc.data, &doc.stats, &types, &counts);
    let t0 = std::time::Instant::now();
    let v = supply::view(&doc.data, &doc.stats, &doc.labels.read(), &urban, terr.clone());
    println!("supply view in {:.2}s: {} railways ({} links), {} hubs", t0.elapsed().as_secs_f32(), v.plan.auto_links.len(), v.plan.lines.level.len(), v.plan.hubs.len());
    assert!(!v.plan.custom_rail && v.plan.lines.faults.is_empty());
    let t0 = std::time::Instant::now();
    let est = v.estimate(&doc.data, supply::Scope::EachNation);
    let land: Vec<usize> = (0..v.plan.net.passable.len()).filter(|&p| v.plan.net.passable[p]).collect();
    let full = land.iter().filter(|&&p| est.flow[p] >= 1.0).count();
    println!("estimate in {:.2}s: {full} of {} passable provinces fully supplied, {} of {} hubs connected", t0.elapsed().as_secs_f32(), land.len(), est.hubs.iter().filter(|h| h.is_some()).count(), est.hubs.len());
    assert!(full > 0);
    let auto_links = v.plan.lines.level.clone();
    let inside = |doc: &Doc, p: Id| supply::inside_point(&doc.labels.read().px, w, &doc.stats.get(p), p);
    let rails: Vec<Railway> = v.plan.auto_links.iter().map(|&(a, b)| Railway { level: 1, stops: vec![inside(&doc, a), inside(&doc, b)] }).collect();
    let hubs: Vec<[f32; 2]> = v.plan.hubs.iter().map(|&p| inside(&doc, p)).collect();
    // a level-4 line between two far apart hubs, and a port per coastal state
    let (a, b) = (v.plan.hubs[0], *v.plan.hubs.last().unwrap());
    let coastal: Vec<Id> = land.iter().map(|&p| p as Id).filter(|&p| supply::coastal(&doc.data, &doc.stats, p)).take(3).collect();
    assert!(!coastal.is_empty(), "the map has coasts");
    let ports: Vec<Port> = coastal.iter().map(|&p| Port { at: inside(&doc, p), level: 3 }).collect();
    let (ra, rb) = (inside(&doc, a), inside(&doc, b));
    step(&mut doc, "Supply by hand", |d| {
        let mut r = rails;
        r.push(Railway { level: 4, stops: vec![ra, rb] });
        d.data.railways = Some(r);
        d.data.supply_hubs = Some(hubs);
        d.data.ports = ports;
        Ok(String::new())
    });
    let v2 = supply::view(&doc.data, &doc.stats, &doc.labels.read(), &urban, terr);
    assert!(v2.plan.custom_rail && v2.plan.lines.faults.is_empty(), "{:?}", v2.plan.lines.faults);
    let fours = v2.plan.lines.level.values().filter(|&&l| l == 4).count();
    assert!(fours > 3, "the new line: {fours} links");
    for (k, l) in &auto_links {
        assert!(v2.plan.lines.level.get(k).is_some_and(|x| x >= l), "the automatic link {k:?} is kept");
    }
    assert_eq!(v2.plan.hubs, v.plan.hubs, "the same hubs");
    assert_eq!(v2.plan.ports.len(), coastal.len());

    // saved and read again
    doc.save().unwrap();
    let doc2 = open(&dir, false);
    assert_eq!(doc2.data.rivers, doc.data.rivers);
    assert_eq!(doc2.data.railways, doc.data.railways);
    assert_eq!(doc2.data.supply_hubs, doc.data.supply_hubs);
    assert_eq!(doc2.data.ports, doc.data.ports);

    let dirs = dirs("rivers-supply-mods");
    if dirs.game.join("map/provinces.bmp").exists() {
        let log = std::sync::Mutex::new(Vec::<String>::new());
        let mod_dir = crate::modbuild::build(&dir, &dirs, &|l| {
            println!("{l}");
            log.lock().unwrap().push(l.to_string());
        })
        .expect("build");
        // the editor's "terrain type" colours are the build's terrain
        let built: std::collections::BTreeMap<String, usize> = log
            .lock()
            .unwrap()
            .iter()
            .flat_map(|l| l.lines())
            .find_map(|l| l.trim().strip_prefix("gameplay terrain: ").map(str::to_string))
            .expect("no terrain in the build summary")
            .split(", ")
            .map(|kv| {
                let (k, v) = kv.rsplit_once(' ').unwrap();
                (k.to_string(), v.parse().unwrap())
            })
            .collect();
        let types = doc.data.mod_meta().terrain;
        let counts = supply::land_counts(&doc.labels.read(), &t, &types.kinds(), doc.data.provinces.cap());
        let (shown, _) = supply::province_terrain(&doc.data, &doc.stats, &types, &counts);
        let mut editor: std::collections::BTreeMap<String, usize> = Default::default();
        for (p, _) in doc.data.provinces.iter() {
            *editor.entry(shown[p as usize].clone()).or_default() += 1;
        }
        assert_eq!(editor, built, "the editor's terrain is not the build's");
        println!("the editor shows the build's terrain: {editor:?}");
        let errors = crate::modbuild::validate::validate(&mod_dir, &dirs.game, &|l| println!("{l}")).unwrap();
        assert!(errors.is_empty(), "the built mod fails checks: {errors:#?}");
        let rails = std::fs::read_to_string(mod_dir.join("map/railways.txt")).unwrap();
        assert!(rails.lines().any(|l| l.starts_with("4 ")), "a level-4 line");
        let naval = std::fs::read_dir(mod_dir.join("history/states")).unwrap().filter(|f| std::fs::read_to_string(f.as_ref().unwrap().path()).unwrap().contains("naval_base = 3")).count();
        assert_eq!(naval, coastal.iter().map(|&p| doc.data.state_of(p)).collect::<std::collections::BTreeSet<_>>().len());
    } else {
        println!("HOI4 not found at {}: build not tested", dirs.game.display());
    }
}

#[test]
#[ignore = "needs the mod's map files (MOD_HELPER_TEST_MAPS=<maps folder> cargo test -- --ignored)"]
fn real_map_recut_keeps_straits() {
    let dir = out_dir("straits");
    copy_map(&dir);
    let mut doc = open(&dir, true);
    let g = GenParams::default();
    let straits = |doc: &Doc| doc.data.adjacencies.iter().filter(|a| a.kind == "sea").cloned().collect::<Vec<_>>();
    let broken = |doc: &Doc| straits(doc).iter().filter(|a| !doc.strait_ok(a)).map(|a| a.name.clone()).collect::<Vec<_>>();
    let n0 = straits(&doc).len();
    let bad0 = broken(&doc);
    println!("{n0} straits, {} broken at the start: {bad0:?}", bad0.len());

    // re-cut all the land, then all the sea ("Generate by terrain" on the whole map)
    for kind in [Kind::Land, Kind::Sea] {
        let ids: Vec<Id> = doc.data.provinces.iter().filter(|(_, p)| p.kind == kind).map(|(i, _)| i).collect();
        step(&mut doc, &format!("generate {kind:?} by terrain"), |d| {
            let (a, b) = d.retile(&ids, Retile::Density, &g)?;
            Ok(format!("{a} -> {b}"))
        });
        let bad = broken(&doc);
        println!("after re-cutting the {kind:?}: {} straits, broken {bad:?}", straits(&doc).len());
        assert!(bad.len() <= bad0.len(), "re-cutting the {kind:?} broke straits: {bad:?}");
        assert_eq!(straits(&doc).len(), n0, "straits lost");
    }
    // each strait still crosses where it did (its ends near the old crossing points)
    let dist = |a: [i64; 2], b: [i64; 2]| (((a[0] - b[0]).pow(2) + (a[1] - b[1]).pow(2)) as f64).sqrt();
    let before: Vec<_> = {
        let l = model::load(&dir, &Progress::default()).unwrap();
        l.data.adjacencies.into_iter().filter(|a| a.kind == "sea").collect()
    };
    for (a, b) in before.iter().zip(straits(&doc)) {
        if let (Some(s0), Some(s1)) = (a.start, b.start) {
            assert!(dist(s0, s1) < 40.0, "{} moved from {s0:?} to {s1:?}", b.name);
        }
    }

    // the tool: every strait broken by hand (an end moved to land away from
    // the water, as re-cutting did before), then fixed
    let mut moved = 0;
    for i in 0..doc.data.adjacencies.len() {
        let a = doc.data.adjacencies[i].clone();
        if a.kind != "sea" {
            continue;
        }
        let inland = doc.stats.neighbours(a.from).iter().copied().find(|&q| doc.data.kind(q) == Kind::Land && q != a.to && a.through.is_some_and(|t| doc.stats.border_len(q, t) == 0));
        if let Some(q) = inland {
            doc.data.adjacencies[i].from = q;
            moved += 1;
        }
    }
    println!("{moved} straits broken by hand: {} broken", broken(&doc).len());
    assert!(moved > 10);
    let (fixed, failed) = doc.fix_straits();
    println!("fixed {}: {:?}; not fixed: {failed:?}", fixed.len(), &fixed[..fixed.len().min(3)]);
    assert!(failed.is_empty());
    assert!(broken(&doc).is_empty(), "still broken: {:?}", broken(&doc));
    assert_eq!(straits(&doc).len(), n0);
}

#[test]
#[ignore = "needs the mod's map files (MOD_HELPER_TEST_MAPS=<maps folder> cargo test -- --ignored)"]
fn real_map_terrain_bitmaps() {
    use super::{Editor, Settings};
    use crate::modbuild::{self, bitmaps, img};
    let dir = out_dir("bitmaps");
    copy_map(&dir);
    let dirs = dirs("bitmaps-mods");
    if !dirs.game.join("map/provinces.bmp").exists() {
        println!("HOI4 not found at {}: not tested", dirs.game.display());
        return;
    }
    let build = |what: &str| {
        let m = modbuild::build(&dir, &dirs, &|l| {
            if l.contains("heightmap") || l.contains("terrain.bmp") {
                println!("{what}: {l}")
            }
        })
        .expect("build");
        let errors = modbuild::validate::validate(&m, &dirs.game, &|_| {}).unwrap();
        assert!(errors.is_empty(), "{what}: the built mod fails checks: {errors:#?}");
        let read = |f: &str| std::fs::read(m.join("map").join(f)).unwrap();
        (read("terrain.bmp"), read("heightmap.bmp"), read("world_normal.bmp"))
    };
    let (t0, h0, n0) = build("as made");

    let mut ed = Editor::new(Settings::default());
    ed.doc = Some(open(&dir, true));
    let (w, h) = (ed.doc.as_ref().unwrap().w as usize, ed.doc.as_ref().unwrap().h as usize);
    // exported: the build's own files
    let (tf, hf) = (dir.join("export_terrain.bmp"), dir.join("export_heightmap.bmp"));
    ed.export_texture(&tf).unwrap();
    ed.export_heights(&hf).unwrap();
    assert!(std::fs::read(&tf).unwrap() == t0, "exported terrain.bmp is not the build's");
    assert!(std::fs::read(&hf).unwrap() == h0, "exported heightmap.bmp is not the build's");
    // imported back: nothing painted, the build's heights
    ed.import_texture(&tf);
    assert_eq!(ed.painted_count(), 0);
    ed.import_heights(&hf);
    assert!(ed.doc.as_ref().unwrap().heights.is_some());
    ed.doc.as_mut().unwrap().save().unwrap();
    let (t1, h1, n1) = build("imported back");
    assert!(t1 == t0 && h1 == h0 && n1 == n0, "a round trip changed the build");

    // a marsh stroke across the middle of the map
    let doc = ed.doc.as_mut().unwrap();
    doc.begin("Paint terrain.bmp");
    let painted = super::texture_ui::paint_stroke(doc, [w as f64 * 0.3, h as f64 * 0.5], [w as f64 * 0.7, h as f64 * 0.5], 20.0, 9);
    doc.commit();
    println!("{painted} px painted marsh");
    assert!(painted > 1000);
    // an image editor lowered a box of land by 20 and pushed a land pixel under water
    let mut heights = img::read_bmp(&hf).unwrap().data;
    let (bx, by) = (w / 2, h / 3);
    for y in by..by + 100 {
        for x in bx..bx + 100 {
            heights[y * w + x] = heights[y * w + x].saturating_sub(20);
        }
    }
    let land = (0..w * h).find(|&i| heights[i] > 150).unwrap();
    heights[land] = 40;
    img::write_bmp_gray(&hf, w as u32, h as u32, &heights).unwrap();
    ed.import_heights(&hf);
    ed.doc.as_mut().unwrap().save().unwrap();
    let (t2, h2, n2) = build("painted and edited");
    let tex = |b: &[u8]| img::read_bmp_bytes(b).unwrap().data;
    let (a, b) = (tex(&t0), tex(&t2));
    let paint = ed.doc.as_ref().unwrap().paint.read().clone();
    let sea: Vec<bool> = ed.doc.as_ref().unwrap().labels.read().px.iter().map(|&p| ed.doc.as_ref().unwrap().data.kind(p) == Kind::Sea).collect();
    for i in 0..w * h {
        let want = if paint[i] != bitmaps::AUTO && !sea[i] { 9 } else { a[i] };
        assert_eq!(b[i], want, "terrain.bmp at {i}");
    }
    let (hb, ha) = (tex(&h2), tex(&h0));
    assert_eq!(hb[land], crate::modbuild::vanilla::WATER + 1, "land under water was raised");
    let changed = (0..w * h).filter(|&i| hb[i] != ha[i]).count();
    assert!(changed > 5000, "{changed} heights changed");
    assert!(n2 != n0, "the shading does not follow the heights");
}

#[test]
#[ignore = "needs the mod's map files (MOD_HELPER_TEST_MAPS=<maps folder> cargo test -- --ignored)"]
fn real_map_lakes_of_water() {
    use crate::modbuild::{self, bitmaps, img, vanilla::WATER};
    let dir = out_dir("lakes");
    copy_map(&dir);
    let dirs = dirs("lakes-mods");
    if !dirs.game.join("map/provinces.bmp").exists() {
        println!("HOI4 not found at {}: not tested", dirs.game.display());
        return;
    }
    let mut doc = open(&dir, false);
    let errors_before = errors(&doc);
    // an inland land province of a state with others, in no adjacency, without
    // a victory point, not in a capital state
    let d = &doc.data;
    let capitals: Vec<Id> = d.nations.iter().map(|(_, n)| n.capital_state).collect();
    let mut cands: Vec<Id> = d
        .provinces
        .iter()
        .filter(|&(p, pr)| {
            pr.kind == Kind::Land
                && !pr.edge
                && !capitals.contains(&pr.state)
                && doc.index.provs_of_state(pr.state).len() >= 3
                && d.states.get(pr.state).is_some_and(|s| !s.impassable && s.victory_points.iter().all(|v| v[0] != p))
                && !d.adjacencies.iter().any(|a| a.from == p || a.to == p || a.through == Some(p))
                && doc.stats.neighbours(p).iter().all(|&q| q != 0 && d.kind(q) == Kind::Land)
        })
        .map(|(p, _)| p)
        .collect();
    cands.sort_by_key(|&p| std::cmp::Reverse(doc.stats.get(p).px));
    let lake = cands[cands.len() / 2];
    println!("lake: {} ({lake}), {} px, {} candidates", d.prov_name(lake), doc.stats.get(lake).px, cands.len());
    let removed: Vec<Id> = d.provinces.iter().filter(|(_, pr)| pr.kind == Kind::Lake && !pr.water).map(|(p, _)| p).collect();
    let lakes = d.provinces.iter().filter(|(_, pr)| pr.kind == Kind::Lake && pr.water).count();

    doc.begin("Make lake");
    assert_eq!(doc.make_lakes(&[lake]).unwrap(), 1);
    doc.commit();
    let new_errors: Vec<String> = errors(&doc).into_iter().filter(|e| !errors_before.contains(e)).collect();
    assert!(new_errors.is_empty(), "a lake made new errors: {new_errors:?}");
    // the pixels of the lake, and of land taken out of play
    let px: Vec<usize> = doc.labels.read().px.iter().enumerate().filter(|&(_, &p)| p == lake).map(|(i, _)| i).collect();
    let removed_px: Vec<usize> = doc.labels.read().px.iter().enumerate().filter(|&(_, &p)| removed.contains(&p)).map(|(i, _)| i).collect();
    doc.save().unwrap();

    // read back: still a lake of water
    let back = open(&dir, false);
    let p = back.labels.read().px[px[0]];
    assert!(back.data.kind(p) == Kind::Lake && back.data.is_water(p), "the lake was not saved");

    let lines = std::sync::Mutex::new(Vec::<String>::new());
    let m = modbuild::build(&dir, &dirs, &|l| lines.lock().unwrap().push(l.to_string())).expect("build");
    let errors = modbuild::validate::validate(&m, &dirs.game, &|l| lines.lock().unwrap().push(l.to_string())).unwrap();
    assert!(errors.is_empty(), "the built mod fails checks: {errors:#?}");
    let lines = lines.into_inner().unwrap();
    for l in lines.iter().filter(|l| l.contains("lakes")) {
        println!("{l}");
    }
    let want = format!(" {} lakes of water", lakes + 1);
    assert!(lines.iter().any(|l| l.contains(&want)), "the check does not see the lake under water");

    let map = m.join("map");
    let bmp = |f: &str| img::read_bmp(&map.join(f)).unwrap();
    let (height, rivers, terrain, provinces) = (bmp("heightmap.bmp"), bmp("rivers.bmp"), bmp("terrain.bmp"), bmp("provinces.bmp"));
    for &i in &px {
        assert!(height.data[i] < WATER, "the lake is not under water at {i}: {}", height.data[i]);
        assert_eq!(rivers.data[i], 254, "rivers.bmp at {i}");
        assert_eq!(terrain.data[i], bitmaps::T_OCEAN, "terrain.bmp at {i}");
    }
    // deeper inside than at the shore
    assert!(px.iter().map(|&i| height.data[i]).min() < px.iter().map(|&i| height.data[i]).max(), "the lake is flat");
    for &i in &removed_px {
        assert!(height.data[i] > WATER, "land out of play under water at {i}");
    }
    // definition.csv: a lake province
    let rgb = &provinces.data[px[0] * 3..px[0] * 3 + 3];
    let def = std::fs::read_to_string(map.join("definition.csv")).unwrap();
    let row: Vec<&str> = def.lines().map(|l| l.split(';').collect::<Vec<_>>()).find(|r| r.len() > 7 && r[1..4] == [rgb[0], rgb[1], rgb[2]].map(|c| c.to_string())).expect("lake not in definition.csv");
    assert_eq!(row[4..8], ["lake", "false", "lakes", "0"], "{row:?}");
}

#[test]
#[ignore = "needs the mod's map files (MOD_HELPER_TEST_MAPS=<maps folder> cargo test -- --ignored)"]
fn real_map_lasso() {
    use super::{Editor, Level, Settings, Tool};
    let dir = out_dir("lasso");
    copy_map(&dir);
    let mut ed = Editor::new(Settings { tool: Tool::Lasso, ..Settings::default() });
    ed.doc = Some(open(&dir, false));
    let before = errors(ed.doc.as_ref().unwrap());
    let new_errors = |ed: &Editor| -> Vec<String> { errors(ed.doc.as_ref().unwrap()).into_iter().filter(|e| !before.contains(e)).collect() };
    // the biggest inland land province of a state of several
    let (p, c, r) = {
        let doc = ed.doc.as_ref().unwrap();
        let d = &doc.data;
        let p = d
            .provinces
            .iter()
            .filter(|&(p, pr)| pr.kind == Kind::Land && !pr.edge && doc.index.provs_of_state(pr.state).len() >= 4 && doc.stats.neighbours(p).iter().all(|&q| q != 0 && d.kind(q) == Kind::Land))
            .map(|(p, _)| p)
            .max_by_key(|&p| doc.stats.get(p).px)
            .unwrap();
        (p, doc.stats.get(p).centre(), (doc.stats.get(p).px as f64 / std::f64::consts::PI).sqrt())
    };
    let circle = |r: f64| -> Vec<[f64; 2]> { (0..64).map(|k| k as f64 * std::f64::consts::TAU / 64.0).map(|a| [c[0] + r * a.cos(), c[1] + r * a.sin()]).collect() };
    let name = ed.doc.as_ref().unwrap().data.prov_name(p);
    println!("lasso around {name} ({p}), radius {r:.0} px");

    // a new province over the middle of it and some of its neighbours
    ed.finish_lasso(&circle(r));
    let made = ed.sel[0];
    let doc = ed.doc.as_ref().unwrap();
    println!("{:?}: {} px", ed.message(), doc.stats.get(made).px);
    assert!(made != p && doc.data.kind(made) == Kind::Land && doc.stats.get(made).px as f64 > 2.5 * r * r);
    assert!(new_errors(&ed).is_empty(), "{:?}", new_errors(&ed));
    // then the old province takes back the middle of it
    ed.settings.lasso_onto = true;
    ed.sel = vec![p];
    let px = ed.doc.as_ref().unwrap().stats.get(p).px;
    ed.finish_lasso(&circle(r / 2.0));
    let doc = ed.doc.as_ref().unwrap();
    println!("{:?}", ed.message());
    assert!(doc.stats.get(p).px > px);
    assert!(new_errors(&ed).is_empty(), "{:?}", new_errors(&ed));

    // a new state of the provinces mostly inside a wider loop, then more of it
    ed.settings.level = Level::States;
    ed.settings.lasso_onto = false;
    ed.finish_lasso(&circle(3.0 * r));
    let s = ed.sel[0];
    let doc = ed.doc.as_ref().unwrap();
    println!("{:?}", ed.message());
    assert!(doc.data.state_of(made) == s && doc.index.provs_of_state(s).len() > 2);
    ed.settings.lasso_onto = true;
    let n = doc.index.provs_of_state(s).len();
    ed.finish_lasso(&circle(5.0 * r));
    println!("{:?}", ed.message());
    assert!(ed.doc.as_ref().unwrap().index.provs_of_state(s).len() > n);
    assert!(new_errors(&ed).is_empty(), "{:?}", new_errors(&ed));
    // each one undo step
    for _ in 0..4 {
        ed.doc.as_mut().unwrap().undo();
    }
    assert!(errors(ed.doc.as_ref().unwrap()) == before && ed.doc.as_ref().unwrap().can_undo().is_none());
}

#[test]
#[ignore = "needs the mod's map files (MOD_HELPER_TEST_MAPS=<maps folder> cargo test -- --ignored)"]
fn real_map_cut() {
    use super::{Editor, Level, Settings, Tool};
    let dir = out_dir("cut");
    copy_map(&dir);
    let mut ed = Editor::new(Settings { tool: Tool::Cut, ..Settings::default() });
    ed.doc = Some(open(&dir, false));
    let before = errors(ed.doc.as_ref().unwrap());
    let new_errors = |ed: &Editor| -> Vec<String> { errors(ed.doc.as_ref().unwrap()).into_iter().filter(|e| !before.contains(e)).collect() };
    // the biggest inland land province of a state of several
    let (p, s, c, b) = {
        let doc = ed.doc.as_ref().unwrap();
        let d = &doc.data;
        let p = d
            .provinces
            .iter()
            .filter(|&(p, pr)| pr.kind == Kind::Land && !pr.edge && doc.index.provs_of_state(pr.state).len() >= 6 && doc.stats.neighbours(p).iter().all(|&q| q != 0 && d.kind(q) == Kind::Land))
            .map(|(p, _)| p)
            .max_by_key(|&p| doc.stats.get(p).px)
            .unwrap();
        let st = doc.stats.get(p);
        (p, d.state_of(p), st.centre(), [st.x0 as f64, st.y0 as f64, st.x1 as f64 + 1.0, st.y1 as f64 + 1.0])
    };
    let name = ed.doc.as_ref().unwrap().data.prov_name(p);
    let px = ed.doc.as_ref().unwrap().stats.get(p).px;
    println!("cutting {name} ({p}, {px} px) of state {s}");

    // across it, a little wavy, from 3 px outside its box to 3 px outside
    let line: Vec<[f64; 2]> = (0..=40).map(|k| b[0] - 3.0 + (b[2] - b[0] + 6.0) * k as f64 / 40.0).map(|x| [x, c[1] + 2.0 * (x / 7.0).sin()]).collect();
    ed.finish_cut(&line, 3.0);
    let doc = ed.doc.as_ref().unwrap();
    println!("{:?}", ed.message());
    assert_eq!(doc.can_undo(), Some("Cut provinces"), "{:?}", ed.message());
    let q = *ed.sel.iter().find(|&&x| x != p).unwrap();
    let (a, n) = (doc.stats.get(p).px, doc.stats.get(q).px);
    assert!(doc.data.state_of(q) == s && doc.data.kind(q) == Kind::Land && a >= n && n > px / 8, "{a} + {n} of {px}");
    assert!(new_errors(&ed).is_empty(), "{:?}", new_errors(&ed));
    ed.doc.as_mut().unwrap().undo();

    // the state, down through the province: each province whole, then cut along
    let (sb, nvp, vps) = {
        let doc = ed.doc.as_ref().unwrap();
        let provs = doc.index.provs_of_state(s);
        let sb = provs.iter().map(|&p| doc.stats.get(p)).fold([f64::MAX, f64::MAX, 0.0f64, 0.0f64], |b, st| [b[0].min(st.x0 as f64), b[1].min(st.y0 as f64), b[2].max(st.x1 as f64 + 1.0), b[3].max(st.y1 as f64 + 1.0)]);
        let total_vps: usize = doc.data.states.iter().map(|(_, st)| st.victory_points.len()).sum();
        (sb, provs.len(), total_vps)
    };
    ed.settings.level = Level::States;
    let down: Vec<[f64; 2]> = (0..=40).map(|k| sb[1] - 3.0 + (sb[3] - sb[1] + 6.0) * k as f64 / 40.0).map(|y| [c[0] + 2.0 * (y / 9.0).sin(), y]).collect();
    ed.finish_cut(&down, 3.0);
    let doc = ed.doc.as_ref().unwrap();
    println!("{:?}", ed.message());
    assert_eq!(doc.can_undo(), Some("Cut states"), "{:?}", ed.message());
    let t = *ed.sel.iter().find(|&&x| x != s).unwrap();
    let (ns, nt) = (doc.index.provs_of_state(s).len(), doc.index.provs_of_state(t).len());
    assert!(ns + nt == nvp && nt > 0 && doc.data.states.get(t).unwrap().nation == doc.data.states.get(s).unwrap().nation, "{ns} + {nt} of {nvp}");
    assert_eq!(doc.data.states.iter().map(|(_, st)| st.victory_points.len()).sum::<usize>(), vps, "victory points stay");
    assert!(new_errors(&ed).is_empty(), "{:?}", new_errors(&ed));
    ed.doc.as_mut().unwrap().undo();

    ed.settings.cut_provinces = true;
    ed.finish_cut(&down, 3.0);
    let doc = ed.doc.as_ref().unwrap();
    println!("{:?}", ed.message());
    assert!(doc.index.provs_of_state(s).len() + doc.index.provs_of_state(*ed.sel.iter().find(|&&x| x != s).unwrap()).len() > nvp, "provinces were cut too");
    assert!(new_errors(&ed).is_empty(), "{:?}", new_errors(&ed));
    ed.doc.as_mut().unwrap().undo();
    assert!(errors(ed.doc.as_ref().unwrap()) == before && ed.doc.as_ref().unwrap().can_undo().is_none());
}

/// The base game imported as a new map: the editor's checks find no errors,
/// and the mod built from it passes the build's checks.
#[test]
#[ignore = "needs the game (cargo test -- --ignored)"]
fn imported_base_game_builds() {
    let game = crate::modbuild::default_game_dir();
    if !game.join("map/provinces.bmp").exists() {
        println!("HOI4 not found at {}: not tested", game.display());
        return;
    }
    // as a new project: one file, opened again
    let file = out_dir("import-base").join("Base game test.hoi4proj");
    let src = crate::newproject::hoi4::Source { game: game.clone(), mod_dir: None };
    let proj = crate::mapproj::Projection::Plain { pixel_m: 40_075_000.0 / 5632.0 };
    let req = crate::newproject::Request { name: "Base game test".into(), file: file.clone(), start: crate::newproject::Start::Import(src), proj, game_dir: None };
    let (_, c) = crate::newproject::create(&req, &Progress::default()).unwrap();
    println!("{}", c.report.join("\n"));
    let (p, _) = crate::bundle::Project::open(&file).unwrap();
    let dir = p.map_dir();
    let mut doc = open(&dir, false);
    // no real-world data: heights from the imported heightmap, grass
    let t = density::load_terrain(&Sources::default(), doc.w, doc.h, doc.data.pixel_m, doc.heights.as_deref().map(|v| v.as_slice()), &Progress::default()).unwrap();
    doc.terrain = Some(Arc::new(t));
    assert!(doc.data.provinces.count() > 13_000 && doc.data.states.count() > 1000 && doc.data.nations.count() > 50);
    assert!(doc.data.rivers.len() > 100 && doc.data.railways.as_ref().is_some_and(|r| r.len() > 100));
    let errs: Vec<Issue> = validate::validate(&Snapshot::of(&doc)).into_iter().filter(|i| i.severity == Severity::Error).collect();
    let mut kinds: std::collections::BTreeMap<String, usize> = Default::default();
    for e in &errs {
        let k: String = e.msg.chars().map(|c| if c.is_ascii_digit() { '#' } else { c }).collect();
        *kinds.entry(k.split(':').next().unwrap_or("").to_string()).or_default() += 1;
    }
    assert!(errs.is_empty(), "{} errors in the editor's checks: {kinds:#?}", errs.len());
    let dirs = crate::modbuild::Dirs { game, mods: out_dir("import-base-mods"), sources: Sources::default() };
    let mod_dir = crate::modbuild::build(&dir, &dirs, &|l| println!("{l}")).expect("build");
    let errors = crate::modbuild::validate::validate(&mod_dir, &dirs.game, &|l| println!("{l}")).unwrap();
    assert!(errors.is_empty(), "the built mod fails checks: {errors:#?}");
}
