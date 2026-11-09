//! Wavefront OBJ files and their MTL materials, as Blender and most 3D
//! programs write them: `v`, `vt`, `vn`, `f` (polygons, cut into
//! triangles), `o` / `g`, `usemtl`, `mtllib`; in the MTL `newmtl`, `Kd`,
//! `map_Kd`, `map_Bump` / `bump` / `norm`, `Ke` / `map_Ke`, `Pm`, `Pr`,
//! `Ns`.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context as _, Result, bail};

use super::{Kind, Source, SrcMat, SrcPart, TexSrc, find_picture};
use crate::models::math::{V3, v3};

pub fn read(path: &Path) -> Result<Source> {
    let b = std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    let text = String::from_utf8_lossy(&b);
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut src = parse(&text, &|lib: &str| {
        let f = find_picture(dir, lib)?;
        let t = std::fs::read(&f).ok()?;
        Some((String::from_utf8_lossy(&t).into_owned(), f.parent().map(Path::to_path_buf).unwrap_or_default()))
    })?;
    src.file = path.to_path_buf();
    Ok(src)
}

fn floats(rest: &[&str]) -> Vec<f32> {
    rest.iter().map_while(|t| t.parse::<f32>().ok()).collect()
}

/// Index of an `f` corner: 1-based, or from the end when negative.
fn index(s: &str, len: usize) -> Result<Option<usize>> {
    if s.is_empty() {
        return Ok(None);
    }
    let i: i64 = s.parse().with_context(|| format!("bad index {s}"))?;
    let k = if i > 0 { i - 1 } else { len as i64 + i };
    if k < 0 || k as usize >= len {
        bail!("index {s} names nothing (there are {len})");
    }
    Ok(Some(k as usize))
}

/// A face corner: vertex, texture coordinate and normal indexes.
type Corner = (usize, Option<usize>, Option<usize>);

/// Parse an OBJ; `mtl` reads a material library it names (its text and
/// folder).
pub fn parse(text: &str, mtl: &dyn Fn(&str) -> Option<(String, std::path::PathBuf)>) -> Result<Source> {
    let (mut pos, mut uv, mut nrm): (Vec<V3>, Vec<[f32; 2]>, Vec<V3>) = (Vec::new(), Vec::new(), Vec::new());
    let mut mats: Vec<SrcMat> = Vec::new();
    let mut lib: HashMap<String, SrcMat> = HashMap::new();
    let mut notes = Vec::new();
    let mut parts: Vec<SrcPart> = Vec::new();
    // (group, material) → part and its corners → vertex
    let mut part_of: HashMap<(String, usize), usize> = HashMap::new();
    let mut corners: Vec<HashMap<Corner, u32>> = Vec::new();
    let (mut group, mut mat): (String, Option<usize>) = (String::new(), None);
    let (mut no_uv, mut no_nrm, mut skipped) = (false, false, 0usize);
    let mat_index = |name: &str, mats: &mut Vec<SrcMat>, lib: &HashMap<String, SrcMat>, notes: &mut Vec<String>| -> usize {
        if let Some(i) = mats.iter().position(|m| m.name == name) {
            return i;
        }
        let m = lib.get(name).cloned().unwrap_or_else(|| {
            if !name.is_empty() {
                notes.push(format!("material {name} is in no material library"));
            }
            SrcMat { name: if name.is_empty() { "default".into() } else { name.to_string() }, ..Default::default() }
        });
        mats.push(m);
        mats.len() - 1
    };
    for (ln, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap_or("").trim();
        let mut it = line.split_whitespace();
        let Some(key) = it.next() else { continue };
        let rest: Vec<&str> = it.collect();
        match key {
            "v" => {
                let f = floats(&rest);
                if f.len() < 3 {
                    bail!("line {}: a vertex needs x y z", ln + 1);
                }
                pos.push(v3(f[0], f[1], f[2]));
            }
            "vt" => {
                let f = floats(&rest);
                // v from the bottom in OBJ, from the top in the game
                uv.push([f.first().copied().unwrap_or(0.0), 1.0 - f.get(1).copied().unwrap_or(0.0)]);
            }
            "vn" => {
                let f = floats(&rest);
                if f.len() < 3 {
                    bail!("line {}: a normal needs x y z", ln + 1);
                }
                nrm.push(v3(f[0], f[1], f[2]));
            }
            "o" | "g" => group = rest.join(" "),
            "usemtl" => {
                let name = rest.join(" ");
                mat = Some(mat_index(&name, &mut mats, &lib, &mut notes));
            }
            "mtllib" => {
                // file names may hold spaces: try the whole rest first
                let whole = rest.join(" ");
                let names: Vec<String> = if mtl(&whole).is_some() { vec![whole] } else { rest.iter().map(|s| s.to_string()).collect() };
                for n in names {
                    match mtl(&n) {
                        Some((t, dir)) => parse_mtl(&t, &dir, &mut lib),
                        None => notes.push(format!("material library {n} not found")),
                    }
                }
            }
            "f" => {
                if rest.len() < 3 {
                    skipped += 1;
                    continue;
                }
                let m = match mat {
                    Some(m) => m,
                    None => {
                        let m = mat_index("", &mut mats, &lib, &mut notes);
                        mat = Some(m);
                        m
                    }
                };
                let pi = *part_of.entry((group.clone(), m)).or_insert_with(|| {
                    parts.push(SrcPart { name: group.clone(), mat: m, ..Default::default() });
                    corners.push(HashMap::new());
                    parts.len() - 1
                });
                let mut face = Vec::with_capacity(rest.len());
                for c in &rest {
                    let mut s = c.split('/');
                    let vi = index(s.next().unwrap_or(""), pos.len()).with_context(|| format!("line {}", ln + 1))?.ok_or_else(|| anyhow::anyhow!("line {}: a corner without a vertex", ln + 1))?;
                    let ti = index(s.next().unwrap_or(""), uv.len()).with_context(|| format!("line {}", ln + 1))?;
                    let ni = index(s.next().unwrap_or(""), nrm.len()).with_context(|| format!("line {}", ln + 1))?;
                    no_uv |= ti.is_none();
                    no_nrm |= ni.is_none();
                    let part = &mut parts[pi];
                    let k = *corners[pi].entry((vi, ti, ni)).or_insert_with(|| {
                        part.pos.push(pos[vi]);
                        part.uv.push(ti.map(|t| uv[t]).unwrap_or([0.0, 0.0]));
                        part.nrm.push(ni.map(|n| nrm[n]).unwrap_or(V3::ZERO));
                        part.pos.len() as u32 - 1
                    });
                    face.push(k);
                }
                for i in 1..face.len() - 1 {
                    parts[pi].tri.extend_from_slice(&[face[0], face[i], face[i + 1]]);
                }
            }
            _ => {}
        }
    }
    if no_nrm {
        notes.push("Some faces have no normals: they are drawn flat.".into());
    }
    if no_uv {
        notes.push("Some faces have no texture coordinates: they show one colour of their texture.".into());
    }
    if skipped > 0 {
        notes.push(format!("{skipped} face(s) of fewer than 3 corners left out."));
    }
    for p in &mut parts {
        p.fill_normals();
    }
    Ok(Source { file: Default::default(), kind: Kind::Obj, parts, mats, tree: None, notes })
}

/// The file a map statement names, after its options (`-bm 1.0 file`).
fn map_file(rest: &[&str]) -> String {
    // options and how many values they take at most
    let opts: &[(&str, usize)] = &[
        ("-bm", 1),
        ("-blendu", 1),
        ("-blendv", 1),
        ("-boost", 1),
        ("-cc", 1),
        ("-clamp", 1),
        ("-imfchan", 1),
        ("-mm", 2),
        ("-o", 3),
        ("-s", 3),
        ("-t", 3),
        ("-texres", 1),
        ("-type", 1),
    ];
    let mut i = 0;
    while i < rest.len() {
        let Some((_, n)) = opts.iter().find(|o| o.0.eq_ignore_ascii_case(rest[i])) else { break };
        i += 1;
        let mut k = 0;
        while k < *n && i < rest.len() && (rest[i].parse::<f32>().is_ok() || matches!(rest[i], "on" | "off") || rest[i].len() == 1) {
            i += 1;
            k += 1;
        }
    }
    rest[i.min(rest.len())..].join(" ")
}

fn parse_mtl(text: &str, dir: &Path, lib: &mut HashMap<String, SrcMat>) {
    let mut cur: Option<SrcMat> = None;
    // per material: whether Pr was given (Ns then only a fallback)
    let mut has_pr = false;
    let pic = |rest: &[&str], m: &mut SrcMat| -> Option<TexSrc> {
        let f = map_file(rest);
        match find_picture(dir, &f) {
            Some(p) => Some(TexSrc::File(p)),
            None => {
                if !f.is_empty() {
                    m.missing.push(f);
                }
                None
            }
        }
    };
    let finish = |m: Option<SrcMat>, lib: &mut HashMap<String, SrcMat>| {
        if let Some(mut m) = m {
            // the colour tints only where there is no picture (Blender writes both)
            if m.diffuse.is_some() {
                m.color = [1.0, 1.0, 1.0, m.color[3]];
            }
            lib.insert(m.name.clone(), m);
        }
    };
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        let mut it = line.split_whitespace();
        let Some(key) = it.next() else { continue };
        let rest: Vec<&str> = it.collect();
        if key == "newmtl" {
            finish(cur.take(), lib);
            cur = Some(SrcMat { name: rest.join(" "), ..Default::default() });
            has_pr = false;
            continue;
        }
        let Some(m) = cur.as_mut() else { continue };
        let f = floats(&rest);
        match key.to_ascii_lowercase().as_str() {
            "kd" if f.len() >= 3 => m.color = [f[0], f[1], f[2], m.color[3]],
            "d" if !f.is_empty() => m.color[3] = f[0],
            "tr" if !f.is_empty() => m.color[3] = 1.0 - f[0],
            "ke" if f.len() >= 3 => m.glow_color = [f[0], f[1], f[2]],
            "pm" if !f.is_empty() => m.metal = f[0],
            "pr" if !f.is_empty() => {
                m.rough = f[0];
                has_pr = true;
            }
            // Blender: Ns = (1 - roughness)² × 1000
            "ns" if !f.is_empty() && !has_pr => m.rough = 1.0 - (f[0] / 1000.0).clamp(0.0, 1.0).sqrt(),
            "map_kd" => m.diffuse = pic(&rest, m),
            "map_bump" | "bump" | "norm" | "map_norm" => m.normal = pic(&rest, m),
            "map_ke" => {
                m.glow = pic(&rest, m);
                if m.glow_color.iter().all(|&v| v <= 0.0) {
                    m.glow_color = [1.0; 3];
                }
            }
            _ => {}
        }
    }
    finish(cur.take(), lib);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn faces_materials_and_maps() {
        let dir = std::env::temp_dir().join(format!("mh_obj_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("textures")).unwrap();
        std::fs::write(dir.join("textures/Wall.PNG"), b"x").unwrap();
        let mtl = "newmtl wall\nKd 0.5 0.5 0.5\nNs 250\nmap_Kd -s 1 1 1 C:\\\\work\\\\wall.png\nmap_Bump -bm 1.0 nowhere.png\nnewmtl roof\nKd 1 0 0\nPr 0.2\n";
        let obj = "mtllib house.mtl\no house\nv 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nvt 0 0\nvt 1 0\nvt 1 1\nvt 0 1\nvn 0 0 1\nusemtl wall\nf 1/1/1 2/2/1 3/3/1 4/4/1\nusemtl roof\nf -4 -3 -2\nusemtl ghost\nf 1//1 2//1 4//1\n";
        let d2 = dir.clone();
        let src = parse(obj, &move |n: &str| (n == "house.mtl").then(|| (mtl.to_string(), d2.clone()))).unwrap();
        assert_eq!(src.mats.len(), 3);
        let wall = &src.mats[0];
        assert_eq!(wall.diffuse, Some(TexSrc::File(dir.join("textures/Wall.PNG"))), "found by its name in textures/, case aside");
        assert_eq!(wall.color, [1.0, 1.0, 1.0, 1.0], "the picture is not tinted");
        assert!((wall.rough - 0.5).abs() < 1e-5);
        assert_eq!(wall.missing, vec!["nowhere.png".to_string()]);
        assert_eq!(src.mats[1].color, [1.0, 0.0, 0.0, 1.0]);
        assert!((src.mats[1].rough - 0.2).abs() < 1e-6);
        assert!(src.notes.iter().any(|n| n.contains("ghost")));
        // the quad: two triangles over 4 corners, v turned to count from the top
        let q = &src.parts[0];
        assert_eq!((q.pos.len(), q.tri.clone()), (4, vec![0, 1, 2, 0, 2, 3]));
        assert_eq!(q.uv[0], [0.0, 1.0]);
        // the roof triangle has no normals: flat, facing +z (counter-clockwise)
        let r = &src.parts[1];
        assert_eq!(r.tri.len(), 3);
        assert!((r.nrm[0] - v3(0.0, 0.0, 1.0)).len() < 1e-5);
        assert!(src.notes.iter().any(|n| n.contains("no normals")));
        assert!(parse("f 1 2 3\n", &|_: &str| None).is_err(), "indexes into nothing");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn map_options() {
        assert_eq!(map_file(&["-bm", "0.5", "my", "file.png"]), "my file.png");
        assert_eq!(map_file(&["-o", "0", "0", "-clamp", "on", "a.dds"]), "a.dds");
        assert_eq!(map_file(&["plain.tga"]), "plain.tga");
    }
}
