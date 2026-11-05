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
#[ignore = "needs the mod's map files (cargo test -- --ignored)"]
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
    let dirs = crate::modbuild::Dirs { game: crate::modbuild::default_game_dir(), mods: out_dir("edits-mods") };
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
#[ignore = "needs the mod's map files (cargo test -- --ignored)"]
fn real_map_province_from_image() {
    use super::from_image::{Channel, Coverage, Source, in_shape, sample};
    let dir = out_dir("from-image");
    copy_map(&dir);
    let mut doc = open(&dir, false);
    let maps = default_map_dir().unwrap().parent().unwrap().to_path_buf();

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
#[ignore = "needs the mod's map files (cargo test -- --ignored)"]
fn real_map_removed_land_from_image() {
    use super::from_image::{Channel, Source, in_shape, sample};
    let dir = out_dir("removed-from-image");
    copy_map(&dir);
    let mut doc = open(&dir, false);
    let maps = default_map_dir().unwrap().parent().unwrap().to_path_buf();
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
#[ignore = "needs the mod's map files (cargo test -- --ignored)"]
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
    let terr = supply::province_terrain(&doc.data, &doc.labels.read(), &t);
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

    let dirs = crate::modbuild::Dirs { game: crate::modbuild::default_game_dir(), mods: out_dir("rivers-supply-mods") };
    if dirs.game.join("map/provinces.bmp").exists() {
        let mod_dir = crate::modbuild::build(&dir, &dirs, &|l| println!("{l}")).expect("build");
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
