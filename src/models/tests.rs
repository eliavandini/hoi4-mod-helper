use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::anim::tests::bend_anim;
use super::catalog::{Catalog, Pick};
use super::mesh::tests::two_bone_mesh;
use super::*;

fn write(dir: &Path, rel: &str, bytes: &[u8]) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, bytes).unwrap();
}

/// A game folder with a bending bar entity (mesh, texture, animation) and a
/// knob attached to its tip.
pub fn fake_game(tag: &str) -> PathBuf {
    let g = std::env::temp_dir().join(format!("mh_models_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&g);
    write(&g, "gfx/models/test/bar.mesh", &two_bone_mesh());
    write(&g, "gfx/models/test/bar_bend.anim", &bend_anim());
    // red on the left half, see-through on the right
    let mut rgba = Vec::new();
    for _ in 0..4 {
        rgba.extend_from_slice(&[220, 30, 30, 255, 220, 30, 30, 255, 0, 0, 0, 0, 0, 0, 0, 0]);
    }
    write(&g, "gfx/models/test/bar_diffuse.dds", &crate::modbuild::img::dds_rgba_bytes(4, 4, &rgba));
    write(
        &g,
        "gfx/entities/test.asset",
        b"pdxmesh = { name = \"bar_mesh\" file = \"gfx/models/test/bar.mesh\" scale = 2\n animation = { id = \"bend\" type = \"bar_bend_animation\" } }\n\
          animation = { name = \"bar_bend_animation\" file = \"bar_bend.anim\" }\n\
          entity = { name = \"bar_entity\" pdxmesh = \"bar_mesh\" default_state = \"idle\"\n state = { name = \"idle\" }\n state = { name = \"bend\" animation = \"bend\" animation_speed = 0.5 looping = no }\n attach = { name = \"knob\" top_node = \"knob_entity\" } }\n\
          entity = { name = \"knob_entity\" pdxmesh = \"bar_mesh\" scale = 0.25 }\n\
          entity = { name = \"broken_entity\" pdxmesh = \"nowhere_mesh\" }\n",
    );
    // the animation definition sits next to the .anim file in the game
    let asset = std::fs::read_to_string(g.join("gfx/entities/test.asset")).unwrap().replace("animation = { name = \"bar_bend_animation\" file = \"bar_bend.anim\" }\n", "");
    write(&g, "gfx/entities/test.asset", asset.as_bytes());
    write(&g, "gfx/models/test/animation.asset", b"animation = { name = \"bar_bend_animation\" file = \"bar_bend.anim\" }");
    g
}

#[test]
fn loads_poses_and_draws_an_entity() {
    let g = fake_game("draw");
    let cat = Arc::new(Catalog::load(Catalog::roots(&g, None)));
    let l = Loader::new(cat.clone());
    let m = l.model(&Pick::Entity("bar_entity".into()));
    assert!(m.problems.is_empty(), "{:?}", m.problems);
    assert_eq!(m.scale, 2.0);
    assert_eq!(m.state_names(), ["idle", "bend"]);
    assert_eq!(m.attach.len(), 1);
    // the bend state runs at half speed: 2 s
    assert!((m.duration(&Play::State("bend".into())).unwrap() - 2.0).abs() < 1e-5);
    assert!(m.duration(&Play::Still).is_none(), "idle plays nothing");
    let g_end = m.globals(&Play::State("bend".into()), 5.0);
    assert!((g_end[1].dir(math::v3(0.0, 1.0, 0.0)) - math::v3(-1.0, 0.0, 0.0)).len() < 1e-4, "held at the end");
    assert!(!l.model(&Pick::Entity("broken_entity".into())).problems.is_empty());
    // the texture's right half is a hole
    assert!(m.tex[0].cutout);

    // straight on: the bar (2 × 2 units scaled by 2) in red, its right half cut out
    let cam = render::Camera::look_at(math::v3(0.0, 2.0, -20.0), math::v3(0.0, 2.0, 0.0), render::Proj::Ortho { scale: 10.0 });
    let mut f = render::Frame::new(64, 64, [0, 0, 0, 0]);
    m.draw(&mut f, &cam, &math::Affine::IDENTITY, &Play::Still, 0.0);
    let at = |x: usize, y: usize| f.color[y * 64 + x];
    assert!(at(24, 32)[0] > 150 && at(24, 32)[3] == 255, "{:?}", at(24, 32));
    assert_eq!(at(40, 32)[3], 0);
    // the knob (a quarter of the bar) rides on top: 4 + 2 units up
    let top = (0..64).find(|&y| (0..64).any(|x| f.color[y * 64 + x][3] == 255)).unwrap();
    assert!((8..=13).contains(&top), "top at {top}");

    let (icon, half) = top_icon(&m, 32);
    assert!((half - 2.1).abs() < 1e-4, "{half}");
    assert_eq!(icon.w, 32);
    let pic = picture(&m, 48, 40, View::default(), &Play::State("bend".into()), 1.0, [0, 0, 0, 255]);
    assert!(pic.color.iter().any(|c| c[0] > 150 && c[1] < 80));
    let _ = std::fs::remove_dir_all(&g);
}

#[test]
fn cutout_textures() {
    let px = |a: &[u8]| a.iter().flat_map(|&a| [100, 100, 100, a]).collect::<Vec<u8>>();
    assert!(is_cutout(&px(&[0, 0, 255, 255, 255, 255, 0, 255])));
    assert!(!is_cutout(&px(&[255; 8])), "opaque");
    assert!(!is_cutout(&px(&[0; 8])), "a mask of zeros is no picture of holes");
    assert!(!is_cutout(&px(&[0, 128, 200, 90, 40, 255, 160, 10])), "a gradient mask");
}

// ------------------------------------------------------------ real game

fn game() -> PathBuf {
    crate::modbuild::default_game_dir()
}

fn out_dir() -> PathBuf {
    std::env::var_os("MOD_HELPER_TEST_OUT").map(PathBuf::from).unwrap_or_else(std::env::temp_dir)
}

fn save_png(f: &render::Frame, name: &str) {
    let p = out_dir().join(name);
    image::save_buffer(&p, &f.rgba(), f.w as u32, f.h as u32, image::ColorType::Rgba8).unwrap();
    eprintln!("wrote {}", p.display());
}

#[test]
#[ignore = "needs the game"]
fn real_catalog_and_models() {
    let t = std::time::Instant::now();
    let cat = Arc::new(Catalog::load(Catalog::roots(&game(), None)));
    eprintln!(
        "catalog: {} entities, {} meshes, {} animations, {} items, {} roots, {:?}",
        cat.entities.len(),
        cat.meshes.len(),
        cat.anims.len(),
        cat.items.len(),
        cat.roots.len(),
        t.elapsed()
    );
    assert!(cat.entities.len() > 800 && cat.meshes.len() > 800 && cat.anims.len() > 500);
    let folders = cat.folders();
    eprintln!("{} folders: {:?}", folders.len(), &folders[..folders.len().min(40)]);
    let l = Loader::new(cat.clone());
    for (name, play) in [
        ("infantry_entity", Play::State("idle".into())),
        ("GER_infantry_entity", Play::State("move".into())),
        ("GER_light_armor_entity", Play::Still),
        ("chinese_wall_entity", Play::Still),
        ("building_landmark_big_ben", Play::Still),
        ("frame_border_logo_entity", Play::Still),
    ] {
        let pick = Pick::Entity(name.into());
        if cat.entity(name).is_none() {
            eprintln!("{name}: not in this game version");
            continue;
        }
        let t = std::time::Instant::now();
        let m = l.model(&pick);
        let loaded = t.elapsed();
        let t = std::time::Instant::now();
        let f = picture(&m, 320, 240, View::default(), &play, 0.4, [40, 44, 52, 255]);
        eprintln!(
            "{name}: {} triangles, {} parts, {} bones, {} anims, {} attached, scale {}, cutout {:?}, load {loaded:?}, draw {:?}, problems {:?}",
            m.mesh.as_ref().map_or(0, |m| m.triangles()),
            m.tex.len(),
            m.mesh.as_ref().map_or(0, |m| m.bones.len()),
            m.anims.len(),
            m.attach.len(),
            m.scale,
            m.tex.iter().map(|t| t.cutout).collect::<Vec<_>>(),
            t.elapsed(),
            m.problems
        );
        assert!(m.mesh.is_some(), "{name}: {:?}", m.problems);
        save_png(&f, &format!("model_{name}.png"));
        let (icon, half) = top_icon(&m, 96);
        eprintln!("  top icon: half {half}");
        save_png(&icon, &format!("icon_{name}.png"));
    }
    // a tree mesh without an entity
    let tree = cat.items.iter().find(|i| i.folder.contains("trees") && matches!(i.pick, Pick::Mesh(_))).expect("tree meshes");
    let m = l.model(&tree.pick);
    eprintln!("{}: cutout {:?} problems {:?}", tree.pick.name(), m.tex.iter().map(|t| t.cutout).collect::<Vec<_>>(), m.problems);
    save_png(&picture(&m, 320, 240, View::default(), &Play::Still, 0.0, [40, 44, 52, 255]), "model_tree.png");

    // every model loads (problems are counted, not fatal: the game has a few broken ones)
    let t = std::time::Instant::now();
    let mut broken = 0;
    let mut seen = 0;
    for it in &cat.items {
        let m = l.model(&it.pick);
        seen += 1;
        if m.mesh.is_none() && it.folder != "(no model)" {
            broken += 1;
            if broken < 15 {
                eprintln!("  {}: {:?}", it.pick.name(), m.problems);
            }
        }
    }
    eprintln!("{seen} models loaded in {:?}, {broken} without a mesh", t.elapsed());
    assert!(broken * 20 < seen);
}

#[test]
#[ignore = "needs the game"]
fn real_texture_alpha() {
    let g = game();
    for f in [
        "gfx/models/mapitems/trees/beech_diffuse.dds",
        "gfx/models/mapitems/trees/leaf_diffuse.dds",
        "gfx/models/mapitems/trees/palmblad_diffuse.dds",
        "gfx/models/units/western_european_infantry_diffuse.dds",
        "gfx/models/units/asian_infantry_diffuse.dds",
    ] {
        let Ok(b) = std::fs::read(g.join(f)) else {
            eprintln!("{f}: missing");
            continue;
        };
        let (w, h, rgba) = crate::modbuild::img::read_dds_bytes(&b, f).unwrap();
        let mut hist = [0usize; 8];
        for a in rgba.chunks_exact(4).map(|c| c[3]) {
            hist[a as usize / 32] += 1;
        }
        let n = (w * h) as f64;
        eprintln!("{f} {w}x{h}: cutout {} alpha by eighths {:?}", is_cutout(&rgba), hist.map(|v| (v as f64 / n * 100.0).round()));
    }
}
