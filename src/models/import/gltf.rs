//! glTF 2.0: a `.gltf` (with its `.bin` and pictures) or one `.glb`, as
//! Blender exports it. Read: the scene's meshes with their nodes'
//! transforms (triangles, strips and fans), positions, normals, the first
//! texture coordinates; per material the base colour, normal,
//! metal-roughness and emissive maps and factors. Not read: bones,
//! animations, compressed meshes (Draco, meshopt), texture transforms.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use serde_json::Value;

use super::{Kind, Source, SrcMat, SrcPart, TexSrc};
use crate::models::math::{V3, v3};

/// Column-major 4×4.
type M4 = [f32; 16];
const IDENTITY: M4 = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];

fn mul(a: &M4, b: &M4) -> M4 {
    let mut r = [0f32; 16];
    for c in 0..4 {
        for row in 0..4 {
            r[c * 4 + row] = (0..4).map(|k| a[k * 4 + row] * b[c * 4 + k]).sum();
        }
    }
    r
}

fn col(m: &M4, c: usize) -> V3 {
    v3(m[c * 4], m[c * 4 + 1], m[c * 4 + 2])
}

fn apply(m: &M4, p: V3) -> V3 {
    col(m, 0) * p.x + col(m, 1) * p.y + col(m, 2) * p.z + col(m, 3)
}

/// A normal through `m` (its inverse transpose, by cofactors), and whether
/// `m` mirrors.
fn normal_fn(m: &M4) -> (impl Fn(V3) -> V3 + use<>, bool) {
    let (a0, a1, a2) = (col(m, 0), col(m, 1), col(m, 2));
    let (c0, c1, c2) = (a1.cross(a2), a2.cross(a0), a0.cross(a1));
    let det = a0.dot(c0);
    let s = if det < 0.0 { -1.0 } else { 1.0 };
    (move |n: V3| (c0 * n.x + c1 * n.y + c2 * n.z) * s, det < 0.0)
}

fn nums(v: &Value) -> Option<Vec<f32>> {
    v.as_array().map(|a| a.iter().map(|x| x.as_f64().unwrap_or(0.0) as f32).collect())
}

fn local(node: &Value) -> M4 {
    if let Some(m) = nums(&node["matrix"]).filter(|m| m.len() == 16) {
        return m.try_into().unwrap();
    }
    let t = nums(&node["translation"]).filter(|v| v.len() == 3).unwrap_or(vec![0.0; 3]);
    let q = nums(&node["rotation"]).filter(|v| v.len() == 4).unwrap_or(vec![0.0, 0.0, 0.0, 1.0]);
    let s = nums(&node["scale"]).filter(|v| v.len() == 3).unwrap_or(vec![1.0; 3]);
    let (x, y, z, w) = (q[0], q[1], q[2], q[3]);
    let r = [
        1.0 - 2.0 * (y * y + z * z),
        2.0 * (x * y + z * w),
        2.0 * (x * z - y * w),
        2.0 * (x * y - z * w),
        1.0 - 2.0 * (x * x + z * z),
        2.0 * (y * z + x * w),
        2.0 * (x * z + y * w),
        2.0 * (y * z - x * w),
        1.0 - 2.0 * (x * x + y * y),
    ];
    let mut m = IDENTITY;
    for c in 0..3 {
        for row in 0..3 {
            m[c * 4 + row] = r[c * 3 + row] * s[c];
        }
    }
    m[12] = t[0];
    m[13] = t[1];
    m[14] = t[2];
    m
}

fn base64(s: &str) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b'\n' | b'\r' | b' ' => continue,
            _ => bail!("bad base64 data"),
        } as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Ok(out)
}

fn unpercent(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Some(v) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

struct Doc {
    j: Value,
    buffers: Vec<Arc<Vec<u8>>>,
    dir: PathBuf,
    notes: Vec<String>,
}

impl Doc {
    fn view(&self, i: usize) -> Result<(&[u8], usize)> {
        let v = &self.j["bufferViews"][i];
        let buf = v["buffer"].as_u64().context("a buffer view without a buffer")? as usize;
        let data = self.buffers.get(buf).with_context(|| format!("no buffer {buf}"))?;
        let off = v["byteOffset"].as_u64().unwrap_or(0) as usize;
        let len = v["byteLength"].as_u64().context("a buffer view without a length")? as usize;
        if off + len > data.len() {
            bail!("buffer view {i} runs past its buffer");
        }
        Ok((&data[off..off + len], v["byteStride"].as_u64().unwrap_or(0) as usize))
    }

    /// An accessor's values, `comps` per element; integers made 0..1 (or
    /// -1..1) when normalised.
    fn floats(&self, i: usize) -> Result<(Vec<f32>, usize)> {
        let a = &self.j["accessors"][i];
        if a.is_null() {
            bail!("no accessor {i}");
        }
        if !a["sparse"].is_null() {
            bail!("sparse accessors are not read (export without them)");
        }
        let count = a["count"].as_u64().context("an accessor without a count")? as usize;
        let comps = match a["type"].as_str().unwrap_or("") {
            "SCALAR" => 1,
            "VEC2" => 2,
            "VEC3" => 3,
            "VEC4" => 4,
            t => bail!("accessor {i}: type {t} is not read"),
        };
        let ct = a["componentType"].as_u64().unwrap_or(0);
        let size = match ct {
            5120 | 5121 => 1,
            5122 | 5123 => 2,
            5125 | 5126 => 4,
            c => bail!("accessor {i}: component type {c}"),
        };
        let norm = a["normalized"].as_bool().unwrap_or(false);
        let Some(bv) = a["bufferView"].as_u64() else { return Ok((vec![0.0; count * comps], comps)) };
        let (data, stride) = self.view(bv as usize)?;
        let stride = if stride == 0 { comps * size } else { stride };
        let off = a["byteOffset"].as_u64().unwrap_or(0) as usize;
        if count > 0 && off + (count - 1) * stride + comps * size > data.len() {
            bail!("accessor {i} runs past its buffer view");
        }
        let mut out = Vec::with_capacity(count * comps);
        for k in 0..count {
            for c in 0..comps {
                let o = off + k * stride + c * size;
                let b = &data[o..o + size];
                let v = match ct {
                    5126 => f32::from_le_bytes(b.try_into().unwrap()),
                    5121 => b[0] as f32 / if norm { 255.0 } else { 1.0 },
                    5120 => {
                        let v = b[0] as i8 as f32;
                        if norm { (v / 127.0).max(-1.0) } else { v }
                    }
                    5123 => u16::from_le_bytes(b.try_into().unwrap()) as f32 / if norm { 65535.0 } else { 1.0 },
                    5122 => {
                        let v = i16::from_le_bytes(b.try_into().unwrap()) as f32;
                        if norm { (v / 32767.0).max(-1.0) } else { v }
                    }
                    _ => u32::from_le_bytes(b.try_into().unwrap()) as f32,
                };
                out.push(v);
            }
        }
        Ok((out, comps))
    }

    fn indices(&self, i: usize) -> Result<Vec<u32>> {
        let a = &self.j["accessors"][i];
        let count = a["count"].as_u64().context("an accessor without a count")? as usize;
        let ct = a["componentType"].as_u64().unwrap_or(0);
        let size = match ct {
            5121 => 1,
            5123 => 2,
            5125 => 4,
            c => bail!("indexes of component type {c}"),
        };
        let bv = a["bufferView"].as_u64().context("indexes without a buffer view")? as usize;
        let (data, _) = self.view(bv)?;
        let off = a["byteOffset"].as_u64().unwrap_or(0) as usize;
        if off + count * size > data.len() {
            bail!("indexes run past their buffer view");
        }
        Ok((0..count)
            .map(|k| {
                let b = &data[off + k * size..off + (k + 1) * size];
                match size {
                    1 => b[0] as u32,
                    2 => u16::from_le_bytes(b.try_into().unwrap()) as u32,
                    _ => u32::from_le_bytes(b.try_into().unwrap()),
                }
            })
            .collect())
    }

    /// The picture a texture reference names.
    fn texture(&mut self, r: &Value, m: &mut SrcMat) -> Option<TexSrc> {
        let ti = r["index"].as_u64()? as usize;
        if r["texCoord"].as_u64().unwrap_or(0) != 0 {
            self.notes.push(format!("{}: a map uses a second set of texture coordinates; the first is used", m.name));
        }
        if !r["extensions"]["KHR_texture_transform"].is_null() {
            self.notes.push(format!("{}: texture transforms are not applied", m.name));
        }
        let t = &self.j["textures"][ti];
        let Some(si) = t["source"].as_u64() else {
            m.missing.push(format!("texture {ti} (a format this does not read: export PNG or JPEG pictures)"));
            return None;
        };
        let img = self.j["images"][si as usize].clone();
        let name = img["name"].as_str().map(str::to_string).unwrap_or_else(|| format!("image {si}"));
        if let Some(uri) = img["uri"].as_str() {
            if let Some(data) = uri.strip_prefix("data:") {
                let (_, b64) = data.split_once(";base64,")?;
                return base64(b64).ok().map(|b| TexSrc::Embedded { name, bytes: Arc::new(b) });
            }
            let p = self.dir.join(unpercent(uri));
            return if p.is_file() {
                Some(TexSrc::File(p))
            } else {
                m.missing.push(unpercent(uri));
                None
            };
        }
        let bv = img["bufferView"].as_u64()? as usize;
        let ext = match img["mimeType"].as_str() {
            Some("image/jpeg") => ".jpg",
            _ => ".png",
        };
        let name = if name.contains('.') { name } else { format!("{name}{ext}") };
        self.view(bv).ok().map(|(b, _)| TexSrc::Embedded { name, bytes: Arc::new(b.to_vec()) })
    }

    fn material(&mut self, i: Option<usize>) -> SrcMat {
        let Some(i) = i else { return SrcMat { name: "default".into(), color: [1.0; 4], rough: 1.0, ..Default::default() } };
        let m = self.j["materials"][i].clone();
        let mut sm = SrcMat { name: m["name"].as_str().map(str::to_string).unwrap_or_else(|| format!("material {i}")), ..Default::default() };
        let pbr = &m["pbrMetallicRoughness"];
        sm.color = nums(&pbr["baseColorFactor"]).filter(|c| c.len() == 4).map(|c| [c[0], c[1], c[2], c[3]]).unwrap_or([1.0; 4]);
        sm.metal = pbr["metallicFactor"].as_f64().unwrap_or(1.0) as f32;
        sm.rough = pbr["roughnessFactor"].as_f64().unwrap_or(1.0) as f32;
        sm.diffuse = self.texture(&pbr["baseColorTexture"], &mut sm);
        sm.metal_rough = self.texture(&pbr["metallicRoughnessTexture"], &mut sm);
        sm.normal = self.texture(&m["normalTexture"], &mut sm);
        sm.glow = self.texture(&m["emissiveTexture"], &mut sm);
        let strength = m["extensions"]["KHR_materials_emissive_strength"]["emissiveStrength"].as_f64().unwrap_or(1.0) as f32;
        sm.glow_color = nums(&m["emissiveFactor"]).filter(|c| c.len() == 3).map(|c| [c[0] * strength, c[1] * strength, c[2] * strength]).unwrap_or([0.0; 3]);
        sm
    }
}

pub fn read(path: &Path) -> Result<Source> {
    let b = std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let mut src = parse(&b, &dir).with_context(|| format!("{}", path.display()))?;
    src.file = path.to_path_buf();
    Ok(src)
}

/// The JSON and the binary chunk of a GLB, or the JSON of a .gltf.
fn chunks(b: &[u8]) -> Result<(&[u8], Option<&[u8]>)> {
    if !b.starts_with(b"glTF") {
        return Ok((b, None));
    }
    if b.len() < 20 || u32::from_le_bytes(b[4..8].try_into().unwrap()) != 2 {
        bail!("not a glTF 2 binary file");
    }
    let (mut json, mut bin) = (None, None);
    let mut at = 12;
    while at + 8 <= b.len() {
        let len = u32::from_le_bytes(b[at..at + 4].try_into().unwrap()) as usize;
        let kind = &b[at + 4..at + 8];
        let body = b.get(at + 8..at + 8 + len).context("a chunk runs past the end")?;
        match kind {
            b"JSON" => json = Some(body),
            b"BIN\0" => bin = Some(body),
            _ => {}
        }
        at += 8 + len;
    }
    Ok((json.context("no JSON chunk")?, bin))
}

pub fn parse(b: &[u8], dir: &Path) -> Result<Source> {
    let (json, bin) = chunks(b)?;
    let j: Value = serde_json::from_slice(json).context("its JSON")?;
    if !j["asset"]["version"].as_str().unwrap_or("2").starts_with('2') {
        bail!("only glTF 2 is read");
    }
    for e in j["extensionsRequired"].as_array().into_iter().flatten() {
        let e = e.as_str().unwrap_or("");
        if !matches!(e, "KHR_mesh_quantization" | "KHR_texture_transform" | "KHR_materials_emissive_strength") {
            bail!("it needs {e}, which this does not read (export without compression)");
        }
    }
    let mut buffers = Vec::new();
    for (i, buf) in j["buffers"].as_array().into_iter().flatten().enumerate() {
        let data = match buf["uri"].as_str() {
            None if i == 0 => bin.context("buffer 0 is the GLB's binary chunk, which is missing")?.to_vec(),
            None => bail!("buffer {i} has no data"),
            Some(u) => match u.strip_prefix("data:") {
                Some(d) => base64(d.split_once(',').map(|x| x.1).unwrap_or(""))?,
                None => {
                    let p = dir.join(unpercent(u));
                    std::fs::read(&p).with_context(|| format!("cannot read its data {}", p.display()))?
                }
            },
        };
        buffers.push(Arc::new(data));
    }
    let mut doc = Doc { j, buffers, dir: dir.to_path_buf(), notes: Vec::new() };
    if doc.j["skins"].as_array().is_some_and(|a| !a.is_empty()) {
        doc.notes.push("Bones (skins) are not imported: the model stands as in its rest pose.".into());
    }
    if doc.j["animations"].as_array().is_some_and(|a| !a.is_empty()) {
        doc.notes.push("Animations are not imported.".into());
    }
    // the nodes to draw: the scene's, else those no node holds
    let nodes = doc.j["nodes"].as_array().cloned().unwrap_or_default();
    let scene = doc.j["scene"].as_u64().unwrap_or(0) as usize;
    let roots: Vec<usize> = match doc.j["scenes"][scene]["nodes"].as_array() {
        Some(r) => r.iter().filter_map(|v| v.as_u64()).map(|v| v as usize).collect(),
        None => {
            let held: Vec<usize> = nodes.iter().flat_map(|n| n["children"].as_array().into_iter().flatten().filter_map(|c| c.as_u64())).map(|c| c as usize).collect();
            (0..nodes.len()).filter(|i| !held.contains(i)).collect()
        }
    };
    let mut parts: Vec<SrcPart> = Vec::new();
    let mut mats: Vec<SrcMat> = Vec::new();
    let mut mat_of: HashMap<Option<usize>, usize> = HashMap::new();
    let mut stack: Vec<(usize, M4, usize)> = roots.into_iter().map(|r| (r, IDENTITY, 0)).collect();
    let mut skipped = 0;
    while let Some((ni, parent, depth)) = stack.pop() {
        let Some(node) = nodes.get(ni) else { bail!("no node {ni}") };
        if depth > 64 {
            bail!("nodes nest too deep (a loop?)");
        }
        let m = mul(&parent, &local(node));
        for c in node["children"].as_array().into_iter().flatten().filter_map(|c| c.as_u64()) {
            stack.push((c as usize, m, depth + 1));
        }
        let Some(mi) = node["mesh"].as_u64() else { continue };
        let mesh = doc.j["meshes"][mi as usize].clone();
        let name = node["name"].as_str().or(mesh["name"].as_str()).unwrap_or("mesh").to_string();
        let (nfn, mirrored) = normal_fn(&m);
        for prim in mesh["primitives"].as_array().into_iter().flatten() {
            let mode = prim["mode"].as_u64().unwrap_or(4);
            if !matches!(mode, 4..=6) {
                skipped += 1;
                continue;
            }
            let attrs = &prim["attributes"];
            let Some(pa) = attrs["POSITION"].as_u64() else { continue };
            let (p, pc) = doc.floats(pa as usize)?;
            if pc != 3 {
                bail!("positions are not 3D");
            }
            let n_v = p.len() / 3;
            let pos: Vec<V3> = p.chunks_exact(3).map(|c| apply(&m, v3(c[0], c[1], c[2]))).collect();
            let nrm: Vec<V3> = match attrs["NORMAL"].as_u64() {
                Some(a) => {
                    let (n, _) = doc.floats(a as usize)?;
                    n.chunks_exact(3).map(|c| nfn(v3(c[0], c[1], c[2])).norm()).collect()
                }
                None => vec![V3::ZERO; n_v],
            };
            let uv: Vec<[f32; 2]> = match attrs["TEXCOORD_0"].as_u64() {
                Some(a) => doc.floats(a as usize)?.0.chunks_exact(2).map(|c| [c[0], c[1]]).collect(),
                None => vec![[0.0, 0.0]; n_v],
            };
            if nrm.len() != n_v || uv.len() != n_v {
                bail!("{name}: its attributes have different counts");
            }
            let idx = match prim["indices"].as_u64() {
                Some(a) => doc.indices(a as usize)?,
                None => (0..n_v as u32).collect(),
            };
            if idx.iter().any(|&i| i as usize >= n_v) {
                bail!("{name}: an index names a vertex it does not have");
            }
            let mut tri = Vec::new();
            match mode {
                4 => tri.extend(idx.chunks_exact(3).flatten()),
                5 => {
                    for k in 0..idx.len().saturating_sub(2) {
                        let t = if k % 2 == 0 { [idx[k], idx[k + 1], idx[k + 2]] } else { [idx[k + 1], idx[k], idx[k + 2]] };
                        tri.extend(t);
                    }
                }
                _ => {
                    for k in 1..idx.len().saturating_sub(1) {
                        tri.extend([idx[0], idx[k], idx[k + 1]]);
                    }
                }
            }
            // a mirroring transform turns faces inside out: turn them back
            if mirrored {
                for t in tri.chunks_exact_mut(3) {
                    t.swap(1, 2);
                }
            }
            let key = prim["material"].as_u64().map(|v| v as usize);
            let mat = match mat_of.get(&key) {
                Some(&k) => k,
                None => {
                    let sm = doc.material(key);
                    mats.push(sm);
                    mat_of.insert(key, mats.len() - 1);
                    mats.len() - 1
                }
            };
            let mut part = SrcPart { name: name.clone(), pos, nrm, uv, tri, mat };
            part.fill_normals();
            parts.push(part);
        }
    }
    let mut notes = doc.notes;
    if skipped > 0 {
        notes.push(format!("{skipped} part(s) of points or lines left out."));
    }
    Ok(Source { file: Default::default(), kind: Kind::Gltf, parts, mats, tree: None, notes })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A GLB of one quad (two triangles, indexed) facing +z, 2 wide, lifted
    /// by its node 1 up, with a material whose base colour is an embedded
    /// 2×2 PNG and whose metal-roughness factors are set.
    pub fn quad_glb() -> Vec<u8> {
        let mut bin: Vec<u8> = Vec::new();
        let pos: [f32; 12] = [-1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 2.0, 0.0, -1.0, 2.0, 0.0];
        let nrm: [f32; 12] = [0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0];
        let uv: [f32; 8] = [0.0, 1.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0];
        for f in pos.iter().chain(&nrm).chain(&uv) {
            bin.extend_from_slice(&f.to_le_bytes());
        }
        for i in [0u16, 1, 2, 0, 2, 3] {
            bin.extend_from_slice(&i.to_le_bytes());
        }
        let png_at = bin.len();
        let mut png = Vec::new();
        let img = image::RgbaImage::from_raw(2, 2, vec![200, 30, 30, 255, 30, 200, 30, 255, 30, 30, 200, 255, 250, 250, 250, 255]).unwrap();
        image::DynamicImage::ImageRgba8(img).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        bin.extend_from_slice(&png);
        while bin.len() % 4 != 0 {
            bin.push(0);
        }
        let json = serde_json::json!({
            "asset": { "version": "2.0" },
            "scene": 0,
            "scenes": [{ "nodes": [0] }],
            "nodes": [{ "name": "Quad", "mesh": 0, "translation": [0.0, 1.0, 0.0] }],
            "meshes": [{ "name": "Quad", "primitives": [{ "attributes": { "POSITION": 0, "NORMAL": 1, "TEXCOORD_0": 2 }, "indices": 3, "material": 0 }] }],
            "materials": [{ "name": "Painted", "pbrMetallicRoughness": { "baseColorTexture": { "index": 0 }, "metallicFactor": 0.0, "roughnessFactor": 0.25 } }],
            "textures": [{ "source": 0 }],
            "images": [{ "name": "paint", "bufferView": 4, "mimeType": "image/png" }],
            "accessors": [
                { "bufferView": 0, "componentType": 5126, "count": 4, "type": "VEC3" },
                { "bufferView": 1, "componentType": 5126, "count": 4, "type": "VEC3" },
                { "bufferView": 2, "componentType": 5126, "count": 4, "type": "VEC2" },
                { "bufferView": 3, "componentType": 5123, "count": 6, "type": "SCALAR" }
            ],
            "bufferViews": [
                { "buffer": 0, "byteOffset": 0, "byteLength": 48 },
                { "buffer": 0, "byteOffset": 48, "byteLength": 48 },
                { "buffer": 0, "byteOffset": 96, "byteLength": 32 },
                { "buffer": 0, "byteOffset": 128, "byteLength": 12 },
                { "buffer": 0, "byteOffset": png_at, "byteLength": png.len() }
            ],
            "buffers": [{ "byteLength": bin.len() }]
        });
        let mut js = serde_json::to_vec(&json).unwrap();
        while js.len() % 4 != 0 {
            js.push(b' ');
        }
        let mut out = Vec::new();
        out.extend_from_slice(b"glTF");
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&((12 + 8 + js.len() + 8 + bin.len()) as u32).to_le_bytes());
        out.extend_from_slice(&(js.len() as u32).to_le_bytes());
        out.extend_from_slice(b"JSON");
        out.extend_from_slice(&js);
        out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        out.extend_from_slice(b"BIN\0");
        out.extend_from_slice(&bin);
        out
    }

    #[test]
    fn reads_a_glb() {
        let src = parse(&quad_glb(), Path::new("/nowhere")).unwrap();
        assert_eq!(src.parts.len(), 1);
        let p = &src.parts[0];
        assert_eq!(p.tri, vec![0, 1, 2, 0, 2, 3]);
        // the node lifts it by 1
        assert_eq!(p.pos[2], v3(1.0, 3.0, 0.0));
        assert_eq!(p.uv[0], [0.0, 1.0]);
        let m = &src.mats[0];
        assert_eq!(m.name, "Painted");
        assert!(matches!(&m.diffuse, Some(TexSrc::Embedded { name, .. }) if name == "paint.png"));
        assert_eq!(m.diffuse.as_ref().unwrap().decode().unwrap().w, 2);
        assert_eq!((m.metal, m.rough), (0.0, 0.25));
    }

    #[test]
    fn transforms_and_strings() {
        // a node mirrored in x turns its faces back and its normals with it
        let m = local(&serde_json::json!({ "scale": [-1.0, 1.0, 1.0] }));
        let (nf, mirrored) = normal_fn(&m);
        assert!(mirrored);
        assert!((nf(v3(1.0, 0.0, 0.0)).norm() - v3(-1.0, 0.0, 0.0)).len() < 1e-6);
        // a quarter turn about y: +x goes to -z
        let r = local(&serde_json::json!({ "rotation": [0.0, std::f32::consts::FRAC_1_SQRT_2, 0.0, std::f32::consts::FRAC_1_SQRT_2] }));
        assert!((apply(&r, v3(1.0, 0.0, 0.0)) - v3(0.0, 0.0, -1.0)).len() < 1e-5);
        assert_eq!(base64("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(unpercent("my%20file.png"), "my file.png");
    }
}
