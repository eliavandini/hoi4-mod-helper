//! The map's 3D objects in the built mod: `type = { ... }` blocks of
//! map/ambient_object.txt, and the entities they need that the game does
//! not have (gfx/entities/mod_helper_ambient.asset).
//!
//! A block shows one entity, with one scale, animation setting and
//! visibility for all its objects. Objects that differ in these get
//! entities of their own: a clone of the entity (playing another state as
//! its default), or for a mesh without an entity, a new entity showing it.

use std::collections::BTreeMap;

use crate::editor::model::Ambient;
use crate::models::catalog::{Catalog, Origin};

/// What the build writes for the objects.
#[derive(Debug, Default)]
pub struct Built {
    /// `type = { ... }` blocks to append to ambient_object.txt.
    pub blocks: String,
    /// gfx/entities/mod_helper_ambient.asset, when entities are needed.
    pub asset: Option<String>,
    pub types: usize,
    pub warnings: Vec<String>,
}

fn fmt(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

/// The game's `rotation = { x y z }` for the editor's rotation.
///
/// The editor turns clockwise seen from above, like a compass (90 faces
/// east). The game turns the other way: a positive y is counter-clockwise.
/// Vanilla's naval bases (`buildings.txt`) face their sea at a bearing of
/// 180° minus their rotation, and the Golden Gate Bridge of Kaiserreich
/// and The Fire Rises (`0 -60 0`, a model spanning east–west) only crosses
/// the bay's mouth turned that way. Tilt (x) and roll (z) are taken to be
/// mirrored the same way; that is not checked in the game yet.
pub fn game_rotation(r: [f32; 3]) -> [f64; 3] {
    r.map(|a| {
        let g = (-(a as f64)).rem_euclid(360.0);
        if g > 180.0 { g - 360.0 } else { g }
    })
}

/// A name the game reads as one word.
fn ident(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect()
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    entity: String,
    mesh: bool,
    animation: Option<String>,
    /// Scale in thousandths.
    scale: i64,
    always_visible: bool,
}

/// The blocks for `objs`. `ground(x, y)` is the height (game units) of the
/// ground or water at a map pixel; `h` the map's height (the game counts z
/// from the bottom). `cat` tells which entities exist, what they play by
/// default and which DLC they need.
pub fn objects(objs: &[Ambient], h: usize, ground: &dyn Fn(f32, f32) -> f64, cat: Option<&Catalog>) -> Built {
    let mut out = Built::default();
    // groups in order of first appearance
    let mut groups: Vec<(Key, Vec<usize>)> = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        let k = Key {
            entity: o.entity.clone(),
            mesh: o.mesh,
            animation: o.animation.clone().filter(|a| !a.is_empty()),
            scale: (o.scale as f64 * 1000.0).round() as i64,
            always_visible: o.always_visible,
        };
        match groups.iter_mut().find(|g| g.0 == k) {
            Some(g) => g.1.push(i),
            None => groups.push((k, vec![i])),
        }
    }
    let mut asset = String::new();
    let mut used: BTreeMap<String, usize> = BTreeMap::new();
    // objects without a name are numbered per model, across blocks
    let mut numbered: BTreeMap<&str, usize> = BTreeMap::new();
    for (k, members) in &groups {
        // the entity the block shows, made if needed
        let def = if k.mesh { None } else { cat.and_then(|c| c.entity(&k.entity)) };
        if let Some(c) = cat {
            let known = if k.mesh { c.mesh(&k.entity).is_some() } else { def.is_some() };
            if !known {
                out.warnings.push(format!(
                    "ambient objects: {} {} is defined neither in the game nor in the mod; {} object(s) will not show",
                    if k.mesh { "mesh" } else { "entity" },
                    k.entity,
                    members.len()
                ));
            }
        }
        let default_state = def.and_then(|d| d.default_state.clone());
        let plays_default = k.animation.as_ref().is_some_and(|a| default_state.as_ref().is_some_and(|d| d.eq_ignore_ascii_case(a)));
        let base = if k.mesh {
            format!("mh_ambient_{}{}", ident(&k.entity), k.animation.as_ref().map(|a| format!("__{}", ident(a))).unwrap_or_default())
        } else if k.animation.is_some() && !plays_default {
            format!("mh_ambient_{}__{}", ident(&k.entity), ident(k.animation.as_ref().unwrap()))
        } else {
            k.entity.clone()
        };
        let n = used.entry(base.to_lowercase()).or_default();
        *n += 1;
        let name = if *n == 1 { base.clone() } else { format!("{}{}_{}", if base.starts_with("mh_ambient_") { "" } else { "mh_ambient_" }, base, n) };
        let made = name != k.entity;
        if made {
            asset += &format!("entity = {{\n\tname = \"{name}\"\n");
            if k.mesh {
                asset += &format!("\tpdxmesh = \"{}\"\n", k.entity);
                if let Some(a) = &k.animation {
                    asset += &format!("\tdefault_state = \"{a}\"\n\tstate = {{ name = \"{a}\" animation = \"{a}\" animation_blend_time = 0.3 }}\n");
                }
            } else {
                asset += &format!("\tclone = \"{}\"\n", k.entity);
                if let Some(a) = &k.animation {
                    asset += &format!("\tdefault_state = \"{a}\"\n");
                }
            }
            asset += "}\n\n";
        }
        out.blocks += &format!(
            "type={{\n\ttype=\"{name}\"\n\tuse_animation={}\n\tscale={:.6}\n",
            if k.animation.is_some() { "yes" } else { "no" },
            k.scale as f64 / 1000.0
        );
        if k.always_visible {
            out.blocks += "\talways_visible=yes\n";
        }
        // a DLC's entity shows only to those who have the DLC
        let origin = cat.and_then(|c| if k.mesh { c.mesh(&k.entity).and_then(|m| c.roots.get(m.root)) } else { def.and_then(|d| c.roots.get(d.root)) }).map(|r| &r.origin);
        if let Some(Origin::Dlc(dlc)) = origin {
            out.blocks += &format!("\tdlc_allowed = {{ has_dlc = \"{dlc}\" }}\n");
        }
        for &i in members {
            let o = &objs[i];
            let y = ground(o.at[0], o.at[1]) + o.lift as f64;
            let rot = game_rotation(o.rotation);
            let oname = if o.name.trim().is_empty() {
                let n = numbered.entry(k.entity.as_str()).or_default();
                *n += 1;
                format!("{}_{n}", ident(k.entity.trim_end_matches("_entity")))
            } else {
                o.name.trim().replace('"', "'")
            };
            out.blocks += &format!(
                "\tobject={{\n\t\tname=\"{oname}\"\n\t\tposition={{\n\t\t\t{} {} {} \n\t\t}}\n\t\trotation={{\n\t\t\t{} {} {} \n\t\t}}\n\t}}\n",
                fmt(o.at[0] as f64),
                fmt(y),
                fmt(h as f64 - o.at[1] as f64),
                fmt(rot[0]),
                fmt(rot[1]),
                fmt(rot[2]),
            );
        }
        out.blocks += "}\n";
        out.types += 1;
    }
    if !asset.is_empty() {
        out.asset = Some(format!("# Entities for the map's ambient objects (made by mod-helper's build)\n\n{asset}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(entity: &str, at: [f32; 2]) -> Ambient {
        Ambient { entity: entity.into(), mesh: false, at, lift: 0.0, rotation: [0.0; 3], scale: 1.0, animation: None, always_visible: false, name: String::new() }
    }

    #[test]
    fn blocks_and_made_entities() {
        let mut objs = vec![obj("tank_entity", [10.5, 20.0]), obj("tank_entity", [30.0, 40.0])];
        objs[1].rotation = [0.0, 90.0, 0.0];
        objs[1].lift = 0.5;
        // another scale: an entity of its own
        let mut big = obj("tank_entity", [1.0, 1.0]);
        big.scale = 2.0;
        big.always_visible = true;
        objs.push(big);
        // another state
        let mut moving = obj("tank_entity", [2.0, 2.0]);
        moving.animation = Some("move".into());
        objs.push(moving);
        // a mesh with no entity, animated
        let mut tree = obj("tree_mesh", [3.0, 3.0]);
        tree.mesh = true;
        tree.animation = Some("sway".into());
        tree.name = "Old \"oak\"".into();
        objs.push(tree);
        let b = objects(&objs, 100, &|x, _| if x > 20.0 { 12.0 } else { 9.5 }, None);
        assert_eq!(b.types, 4);
        let t = &b.blocks;
        assert!(t.starts_with("type={\n\ttype=\"tank_entity\"\n\tuse_animation=no\n\tscale=1.000000\n"), "{t}");
        assert!(t.contains("\t\tname=\"tank_1\"\n\t\tposition={\n\t\t\t10.5 9.5 80 \n"), "{t}");
        // the editor's 90° (clockwise, east) is the game's -90
        assert!(t.contains("\t\t\t30 12.5 60 \n\t\t}\n\t\trotation={\n\t\t\t0 -90 0 \n"), "{t}");
        assert!(t.contains("type=\"mh_ambient_tank_entity_2\"\n\tuse_animation=no\n\tscale=2.000000\n\talways_visible=yes\n"), "{t}");
        // numbered on from the first block
        assert!(t.contains("name=\"tank_3\"") && t.contains("name=\"tank_4\"") && !t.contains("name=\"tank_5\""), "{t}");
        assert!(t.contains("type=\"mh_ambient_tank_entity__move\"\n\tuse_animation=yes"), "{t}");
        assert!(t.contains("type=\"mh_ambient_tree_mesh__sway\"") && t.contains("name=\"Old 'oak'\""), "{t}");
        assert_eq!(t.matches('{').count(), t.matches('}').count());
        let a = b.asset.unwrap();
        assert!(a.contains("name = \"mh_ambient_tank_entity_2\"\n\tclone = \"tank_entity\"\n}"), "{a}");
        assert!(a.contains("name = \"mh_ambient_tank_entity__move\"\n\tclone = \"tank_entity\"\n\tdefault_state = \"move\""), "{a}");
        assert!(a.contains("pdxmesh = \"tree_mesh\"\n\tdefault_state = \"sway\"\n\tstate = { name = \"sway\" animation = \"sway\""), "{a}");
        // nothing to make for plain entities
        assert!(objects(&objs[..2], 100, &|_, _| 10.0, None).asset.is_none());
        assert_eq!(fmt(-0.0001), "0");
        assert_eq!(game_rotation([0.0, 336.0, 0.0]), [0.0, 24.0, 0.0]);
        assert_eq!(game_rotation([10.0, 57.0, -200.0]), [-10.0, -57.0, -160.0]);
        assert_eq!(game_rotation([0.0, 180.0, 0.0])[1], 180.0);
    }
}
