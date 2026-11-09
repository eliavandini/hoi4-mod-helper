use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::*;
use crate::models::catalog::{Catalog, Pick};
use crate::models::{Loader, mesh};

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("mh_import_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// The checks every made mesh passes, as the game's own do: faces wound
/// so their corners turn the way the normals face, tangents along u, and
/// cross(normal, tangent) × w along v.
fn check_conventions(bytes: &[u8]) {
    let root = pdxbin::parse(bytes).unwrap();
    let object = root.child("object").unwrap();
    let mut n_tri = 0;
    for shape in &object.children {
        for m in shape.children.iter().filter(|c| c.name == "mesh") {
            let p: Vec<V3> = m.floats("p").chunks_exact(3).map(V3::from_slice).collect();
            let n: Vec<V3> = m.floats("n").chunks_exact(3).map(V3::from_slice).collect();
            let ta = m.floats("ta");
            let u = m.floats("u0");
            assert_eq!(ta.len(), p.len() * 4);
            assert_eq!(u.len(), p.len() * 2);
            let aabb = m.child("aabb").unwrap();
            assert_eq!(aabb.floats("min").len(), 3);
            for t in m.ints("tri").chunks_exact(3) {
                let (a, b, c) = (t[0] as usize, t[1] as usize, t[2] as usize);
                let face = (p[b] - p[a]).cross(p[c] - p[a]);
                assert!(face.dot(n[a] + n[b] + n[c]) > 0.0, "a triangle wound against its normals");
                let (du1, dv1, du2, dv2) = (u[b * 2] - u[a * 2], u[b * 2 + 1] - u[a * 2 + 1], u[c * 2] - u[a * 2], u[c * 2 + 1] - u[a * 2 + 1]);
                let r = du1 * dv2 - du2 * dv1;
                if r.abs() > 1e-9 {
                    let su = ((p[b] - p[a]) * dv2 - (p[c] - p[a]) * dv1) * (1.0 / r);
                    let sv = ((p[c] - p[a]) * du1 - (p[b] - p[a]) * du2) * (1.0 / r);
                    let t3 = V3::from_slice(&ta[a * 4..a * 4 + 3]);
                    assert!(t3.dot(su) > 0.0, "the tangent runs against u");
                    assert!(n[a].cross(t3).dot(sv) * ta[a * 4 + 3] > 0.0, "the bitangent runs against v");
                }
                n_tri += 1;
            }
            let mat = m.child("material").unwrap();
            assert!(mat.str("shader").unwrap().starts_with("PdxMeshAdvanced"));
        }
    }
    assert!(n_tri > 0);
}

#[test]
fn a_glb_becomes_a_model() {
    let dir = tmp("glb");
    let file = dir.join("Shrine Gate.glb");
    std::fs::write(&file, gltf::tests::quad_glb()).unwrap();
    let src = read(&file).unwrap();
    let mut o = Options::for_source(&src);
    assert_eq!(o.name, "shrine_gate");
    assert_eq!(o.mats[0].surface, SurfaceFrom::Values);
    assert!((o.mats[0].gloss - 0.75).abs() < 1e-6);
    o.scale = 0.5;
    let made = make(&src, &o).unwrap();
    let names: Vec<&str> = made.files.iter().map(|f| f.0.as_str()).collect();
    assert_eq!(
        names,
        [
            "gfx/models/custom/shrine_gate.mesh",
            "gfx/models/custom/shrine_gate_diffuse.dds",
            "gfx/models/custom/shrine_gate_normal.dds",
            "gfx/models/custom/shrine_gate_specular.dds",
            "gfx/entities/shrine_gate.gfx",
            "gfx/entities/shrine_gate.asset",
        ]
    );
    check_conventions(&made.files[0].1);
    // standing on its origin: the node's lift is gone, the bottom at 0
    let (lo, hi) = made.bounds();
    assert!((lo.y).abs() < 1e-6 && (hi.y - 1.0).abs() < 1e-6, "2 high × 0.5: {lo:?} {hi:?}");
    assert!((lo.x + 0.5).abs() < 1e-6 && (hi.x - 0.5).abs() < 1e-6);
    // facing +z in Blender's export, it faces -z in the game's space (seen the same)
    assert!(made.mesh.parts[0].nrm[0].z < -0.99);
    // the textures read back: the picture, a flat normal map, the surface values
    let (w, h, px) = crate::modbuild::img::read_dds_bytes(&made.files[1].1, "d").unwrap();
    assert_eq!((w, h), (2, 2));
    assert_eq!(&px[..4], &[200, 30, 30, 255]);
    let (_, _, nm) = crate::modbuild::img::read_dds_bytes(&made.files[2].1, "n").unwrap();
    assert_eq!(&nm[..4], &[128, 128, 0, 128]);
    let (_, _, sp) = crate::modbuild::img::read_dds_bytes(&made.files[3].1, "s").unwrap();
    assert_eq!(sp[3], 191, "gloss 0.75");

    // installed into a content package, the game's catalog finds it and draws it whole
    let content = dir.join("content");
    let written = install(&made, &content, "shared").unwrap();
    assert_eq!(written.len(), 6);
    let game = dir.join("game");
    std::fs::create_dir_all(&game).unwrap();
    let cat = Catalog::load(Catalog::roots(&game, Some(&content)));
    assert!(cat.problems.is_empty(), "{:?}", cat.problems);
    let e = cat.entity("shrine_gate_entity").expect("the entity");
    assert_eq!(e.pdxmesh.as_deref(), Some("shrine_gate_mesh"));
    let loader = Loader::new(Arc::new(cat));
    let m = loader.model(&Pick::Entity("shrine_gate_entity".into()));
    assert!(m.problems.is_empty(), "{:?}", m.problems);
    assert_eq!(m.scale, 0.5);
    assert_eq!(m.tex.len(), 1);
    // again: the old files are kept in the backups
    install(&made, &content, "shared").unwrap();
    let backups: Vec<_> = std::fs::read_dir(content.join(".backups")).unwrap().collect();
    assert_eq!(backups.len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn obj_axes_materials_and_bumps() {
    let dir = tmp("obj");
    // a floor quad in Blender's own axes (z up), two materials
    std::fs::write(dir.join("m.mtl"), "newmtl Stone\nKd 0.5 0.5 0.5\nnewmtl Gold Leaf\nKd 1 0.8 0\nPm 1\nPr 0.1\nKe 1 1 0\n").unwrap();
    std::fs::write(
        dir.join("floor.obj"),
        "mtllib m.mtl\nv 0 0 0\nv 2 0 0\nv 2 3 0\nv 0 3 0\nv 0 0 1\nvt 0 0\nvt 1 0\nvt 1 1\nvt 0 1\nvn 0 0 1\nusemtl Stone\nf 1/1/1 2/2/1 3/3/1 4/4/1\nusemtl Gold Leaf\nf 1/1 2/2 5/3\n",
    )
    .unwrap();
    let src = read(&dir.join("floor.obj")).unwrap();
    let mut o = Options::for_source(&src);
    o.up = Up::Z;
    o.ground = false;
    o.mats[0].normal = NormalFrom::Bumps;
    assert_eq!(o.mats[1].glow, GlowFrom::Model);
    assert!((o.mats[1].metal - 1.0).abs() < 1e-6);
    let made = make(&src, &o).unwrap();
    check_conventions(&made.files[0].1);
    let names: Vec<&str> = made.files.iter().map(|f| f.0.as_str()).collect();
    assert!(names.contains(&"gfx/models/custom/floor_stone_diffuse.dds"));
    assert!(names.contains(&"gfx/models/custom/floor_gold_leaf_specular.dds"));
    // z up: the floor lies flat (y 0), facing up; Blender's +y (3 deep) is the game's +z
    let p0 = &made.mesh.parts[0];
    assert!(p0.pos.iter().all(|p| p.y.abs() < 1e-6));
    assert!(p0.nrm[0].y > 0.99);
    assert!(p0.pos.iter().any(|p| (p.z - 3.0).abs() < 1e-6));
    // the gold glows: its normal map's blue is lit, the stone's is not
    let normal_of = |n: &str| crate::modbuild::img::read_dds_bytes(&made.files.iter().find(|f| f.0.ends_with(n)).unwrap().1, n).unwrap().2;
    assert!(normal_of("floor_gold_leaf_normal.dds")[2] > 200);
    assert_eq!(normal_of("floor_stone_normal.dds")[2], 0);
    let gfx = String::from_utf8(made.files.iter().find(|f| f.0.ends_with(".gfx")).unwrap().1.clone()).unwrap();
    assert!(gfx.contains("name = \"floorShape\"\n\t\t\tindex = 1"));
    assert!(gfx.contains("scale = 1\n"));
    assert!(make(&src, &Options { name: "Bad Name".into(), ..o.clone() }).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_mesh_keeps_its_bones() {
    let dir = tmp("mesh");
    std::fs::write(dir.join("bar.mesh"), mesh::tests::two_bone_mesh()).unwrap();
    let mut rgba = Vec::new();
    for _ in 0..16 {
        rgba.extend_from_slice(&[10, 200, 10, 255]);
    }
    std::fs::write(dir.join("BAR_DIFFUSE.dds"), crate::modbuild::img::dds_rgba_bytes(4, 4, &rgba)).unwrap();
    let src = read(&dir.join("bar.mesh")).unwrap();
    assert!(src.mats[0].diffuse.is_some(), "found regardless of case");
    assert!(src.notes.iter().any(|n| n.contains("bones")));
    let mut o = Options::for_source(&src);
    o.snow = true;
    let made = make(&src, &o).unwrap();
    assert_eq!(made.mesh.bones.len(), 2, "the skeleton stays");
    assert!(made.mesh.parts[0].skin.is_some());
    let root = pdxbin::parse(&made.files[0].1).unwrap();
    let mat = root.child("object").unwrap().children[0].child("mesh").unwrap().child("material").unwrap();
    assert_eq!(mat.str("diff"), Some("bar_diffuse.dds"));
    assert_eq!(mat.str("shader"), Some("PdxMeshAdvancedSnow"));
    assert_eq!(made.diffuse[0].rgba[..4], [10, 200, 10, 255]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn big_meshes_are_cut() {
    // a strip of quads with more vertices than one mesh holds
    let n = 40_000usize;
    let mut part = SrcPart::default();
    for i in 0..n {
        let x = i as f32;
        part.pos.extend([v3(x, 0.0, 0.0), v3(x, 1.0, 0.0)]);
        part.nrm.extend([v3(0.0, 0.0, 1.0); 2]);
        part.uv.extend([[0.0, 0.0], [0.0, 1.0]]);
    }
    for i in 0..n as u32 - 1 {
        let (a, b, c, d) = (i * 2, i * 2 + 2, i * 2 + 3, i * 2 + 1);
        part.tri.extend([a, b, c, a, c, d]);
    }
    let src = Source { file: Path::new("strip.obj").into(), kind: Kind::Obj, parts: vec![part], mats: vec![SrcMat::default()], tree: None, notes: vec![] };
    let made = make(&src, &Options::for_source(&src)).unwrap();
    assert_eq!(made.mesh.parts.len(), 2);
    assert!(made.mesh.parts.iter().all(|p| p.pos.len() <= MAX_VERTS));
    assert_eq!(made.mesh.triangles(), (n - 1) * 2);
    assert!(made.notes.iter().any(|n| n.contains("Cut in 2")));
    let gfx = String::from_utf8(made.files.last().map(|f| f.1.clone()).unwrap()).unwrap();
    assert!(gfx.contains("strip_entity"));
}

#[test]
fn names() {
    assert_eq!(slug("Andreas Hofer Statue (v2)"), "andreas_hofer_statue_v2");
    assert_eq!(slug("3d"), "m_3d");
    assert_eq!(slug("..."), "model");
    assert!(check_name("castle_tirol").is_ok());
    assert!(check_name("Castle").is_err());
    assert!(check_folder("buildings/castles").is_ok());
    assert!(check_folder("../x").is_err());
    assert_eq!(num(1.0), "1");
    assert_eq!(num(0.125), "0.125");
    assert_eq!(num(2.50001), "2.5");
}

/// The game's own models made again from their .mesh and textures: what
/// comes out reads back with every part textured, its textures in the
/// game's layout as they were.
#[test]
#[ignore = "needs the game"]
fn game_meshes_import_again() {
    let game = crate::modbuild::default_game_dir();
    let dir = tmp("game");
    for rel in ["gfx/models/raid_dynamite.mesh", "gfx/models/buildings/landmarks/landmark_big_ben.mesh", "gfx/models/naval_mine_enemy.mesh"] {
        let src = read(&game.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e:#}"));
        let mut o = Options::for_source(&src);
        o.name = format!("again_{}", slug(src.file.file_stem().unwrap().to_str().unwrap()));
        let made = make(&src, &o).unwrap_or_else(|e| panic!("{rel}: {e:#}"));
        let orig = Mesh::parse(&std::fs::read(game.join(rel)).unwrap()).unwrap();
        assert_eq!(made.mesh.triangles(), orig.triangles(), "{rel}");
        assert_eq!(made.mesh.bones.len(), orig.bones.len(), "{rel}");
        let content = dir.join("content");
        install(&made, &content, "shared").unwrap();
        let cat = Catalog::load(Catalog::roots(&game, Some(&content)));
        let loader = Loader::new(Arc::new(cat));
        let m = loader.model(&Pick::Entity(made.entity.clone()));
        assert!(m.problems.is_empty(), "{rel}: {:?}", m.problems);
        // the game's normal maps stay as they are (already its layout)
        if let Some(n) = src.mats.iter().find_map(|m| m.normal.clone()) {
            let before = n.decode().unwrap();
            let name = made.files.iter().find(|f| f.0.ends_with("_normal.dds")).unwrap();
            let (w, h, after) = crate::modbuild::img::read_dds_bytes(&name.1, "n").unwrap();
            if (w as usize, h as usize) == (before.w, before.h) {
                assert_eq!(after, before.rgba, "{rel}: the normal map changed");
            }
        }
        println!("{rel}: {} triangles, {} files, notes {:?}", made.mesh.triangles(), made.files.len(), made.notes);
    }
    let _ = std::fs::remove_dir_all(&dir);
}
