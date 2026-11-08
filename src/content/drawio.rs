//! Focus trees planned in draw.io (diagrams.net): boxes are focuses, arrows
//! go from a prerequisite to what it unlocks. The file is XML; a page may
//! be compressed (raw deflate, base64, URL-escaped), as draw.io saves by
//! default. Pixel positions become grid cells by clustering the box
//! centres, so boxes that line up share a column or a row whatever spacing
//! the diagram used.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Read as _;

use anyhow::{Result, bail};
use regex::Regex;

/// Boxes closer than this share of the median box size share a column / row.
const CLUSTER: f64 = 0.6;

#[derive(Clone, Debug, PartialEq)]
pub struct Box_ {
    pub id: String,
    pub label: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// A focus from the diagram.
#[derive(Clone, Debug, PartialEq)]
pub struct Planned {
    pub id: String,
    pub label: String,
    pub x: i32,
    pub y: i32,
    pub prereq: Vec<String>,
}

fn attr(tag: &str, name: &str) -> Option<String> {
    let re = Regex::new(&format!(r#"(?:^|\s){name}="([^"]*)""#)).unwrap();
    re.captures(tag).map(|c| unescape(&c[1]))
}

fn unescape(s: &str) -> String {
    let mut out = s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'").replace("&apos;", "'").replace("&nbsp;", " ").replace("&#10;", " ").replace("&#xa;", " ");
    let num = Regex::new(r"&#(\d+);").unwrap();
    out = num.replace_all(&out, |c: &regex::Captures| c[1].parse::<u32>().ok().and_then(char::from_u32).map(String::from).unwrap_or_default()).into_owned();
    out.replace("&amp;", "&")
}

/// A label without its HTML.
fn clean_label(v: &str) -> String {
    let t = v.replace("<br>", " ").replace("<br/>", " ").replace("<br />", " ");
    let t = Regex::new(r"<[^>]+>").unwrap().replace_all(&t, "");
    let t = unescape(&t);
    t.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16)
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

fn base64(s: &str) -> Option<Vec<u8>> {
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => (c - b'A') as u32,
            b'a'..=b'z' => (c - b'a' + 26) as u32,
            b'0'..=b'9' => (c - b'0' + 52) as u32,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        })
    };
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0);
    for &c in s.as_bytes() {
        if c == b'=' || c.is_ascii_whitespace() {
            continue;
        }
        acc = (acc << 6) | val(c)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// The XML of a compressed page.
fn inflate_page(body: &str) -> Option<String> {
    let raw = base64(body.trim())?;
    let mut v = Vec::new();
    flate2::read::DeflateDecoder::new(raw.as_slice()).read_to_end(&mut v).ok()?;
    Some(percent_decode(&String::from_utf8_lossy(&v)))
}

/// The pages of a file: (name, mxGraphModel XML).
pub fn pages(text: &str) -> Result<Vec<(String, String)>> {
    if !text.contains("<mx") {
        bail!("this is not a draw.io file (no mxfile or mxGraphModel in it)");
    }
    let diagram = Regex::new(r"(?s)<diagram([^>]*)>(.*?)</diagram>").unwrap();
    let mut out = Vec::new();
    for (i, c) in diagram.captures_iter(text).enumerate() {
        let name = attr(&c[1], "name").unwrap_or_else(|| format!("Page-{}", i + 1));
        let body = c[2].trim();
        if body.contains("<mxGraphModel") {
            out.push((name, body.to_string()));
        } else if !body.is_empty() {
            match inflate_page(body) {
                Some(x) => out.push((name, x)),
                None => bail!("page {name:?} is compressed in a way that cannot be read; in draw.io untick File → Properties → Compressed and save again"),
            }
        }
    }
    if out.is_empty() && text.contains("<mxGraphModel") {
        out.push(("Page-1".into(), text.to_string()));
    }
    if out.is_empty() {
        bail!("no diagram pages in the file");
    }
    Ok(out)
}

/// Boxes and arrows (source, target) of a page.
pub fn parse_page(xml: &str) -> (Vec<Box_>, Vec<(String, String)>) {
    // <mxCell .../> or <mxCell ...> ... </mxCell> (the geometry inside)
    let cell = Regex::new(r#"(?s)<mxCell\b([^>]*?)(/>|>(.*?)</mxCell>)"#).unwrap();
    let geo = Regex::new(r#"<mxGeometry\b([^>]*)"#).unwrap();
    let mut boxes = Vec::new();
    let mut arrows = Vec::new();
    for c in cell.captures_iter(xml) {
        let a = &c[1];
        let Some(id) = attr(a, "id") else { continue };
        if attr(a, "edge").as_deref() == Some("1") {
            if let (Some(s), Some(t)) = (attr(a, "source"), attr(a, "target")) {
                arrows.push((s, t));
            }
            continue;
        }
        if attr(a, "vertex").as_deref() != Some("1") {
            continue;
        }
        let Some(g) = c.get(3).and_then(|inner| geo.captures(inner.as_str())) else { continue };
        let num = |k: &str, d: f64| attr(&g[1], k).and_then(|v| v.parse::<f64>().ok()).unwrap_or(d);
        boxes.push(Box_ { id, label: clean_label(&attr(a, "value").unwrap_or_default()), x: num("x", 0.0), y: num("y", 0.0), w: num("width", 120.0), h: num("height", 60.0) });
    }
    (boxes, arrows)
}

fn median(mut v: Vec<f64>) -> f64 {
    if v.is_empty() {
        return 1.0;
    }
    v.sort_by(f64::total_cmp);
    let m = v.len() / 2;
    if v.len() % 2 == 1 { v[m] } else { (v[m - 1] + v[m]) / 2.0 }
}

/// Index of each value's cluster (values closer than `tol` share one).
fn cluster(values: &[f64], tol: f64) -> Vec<i32> {
    let mut order: Vec<usize> = (0..values.len()).collect();
    order.sort_by(|&a, &b| values[a].total_cmp(&values[b]));
    let mut out = vec![0; values.len()];
    let mut idx = 0;
    let mut last: Option<f64> = None;
    for i in order {
        if let Some(l) = last
            && values[i] - l > tol
        {
            idx += 1;
        }
        out[i] = idx;
        last = Some(values[i]);
    }
    out
}

/// Grid columns of box centres: centres a little apart (drawn by hand) are
/// one column; a box is two columns wide, so a box drawn between two others
/// lands between them, as in the game's trees.
fn columns(cx: &[f64], box_w: f64) -> Vec<i32> {
    let groups = cluster(cx, box_w * 0.25);
    let n = groups.iter().copied().max().map_or(0, |m| m as usize + 1);
    let mut sum = vec![(0.0, 0usize); n];
    for (i, &g) in groups.iter().enumerate() {
        sum[g as usize].0 += cx[i];
        sum[g as usize].1 += 1;
    }
    let means: Vec<f64> = sum.iter().map(|(s, k)| s / (*k).max(1) as f64).collect();
    let min = means.first().copied().unwrap_or(0.0);
    let step = (box_w / 2.0).max(1.0);
    groups.iter().map(|&g| ((means[g as usize] - min) / step).round() as i32).collect()
}

/// An id from a label: lower case words joined by `_`, with the prefix, unique.
pub fn make_id(label: &str, prefix: &str, taken: &BTreeSet<String>) -> String {
    let mut base: String = label.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    base = base.split('_').filter(|s| !s.is_empty()).collect::<Vec<_>>().join("_");
    if base.is_empty() {
        base = "focus".into();
    }
    if !prefix.is_empty() {
        base = format!("{}_{base}", prefix.trim_end_matches('_'));
    }
    let mut c = base.clone();
    let mut n = 2;
    while taken.contains(&c) {
        c = format!("{base}_{n}");
        n += 1;
    }
    c
}

/// Focuses from boxes and arrows (`flip`: arrows point from a focus to its prerequisite).
pub fn focuses(boxes: &[Box_], arrows: &[(String, String)], prefix: &str, flip: bool, taken: &BTreeSet<String>) -> Vec<Planned> {
    if boxes.is_empty() {
        return Vec::new();
    }
    let cx: Vec<f64> = boxes.iter().map(|b| b.x + b.w / 2.0).collect();
    let cy: Vec<f64> = boxes.iter().map(|b| b.y + b.h / 2.0).collect();
    let cols = columns(&cx, median(boxes.iter().map(|b| b.w).collect()));
    let rows = cluster(&cy, median(boxes.iter().map(|b| b.h).collect()) * CLUSTER);
    let mut taken = taken.clone();
    let mut ids: HashMap<&str, String> = HashMap::new();
    for b in boxes {
        let id = make_id(if b.label.is_empty() { &b.id } else { &b.label }, prefix, &taken);
        taken.insert(id.clone());
        ids.insert(&b.id, id);
    }
    let mut pre: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (s, t) in arrows {
        let (s, t) = if flip { (t, s) } else { (s, t) };
        if let (Some(a), Some(b)) = (ids.get(s.as_str()), ids.get(t.as_str()))
            && a != b
        {
            pre.entry(b.clone()).or_default().push(a.clone());
        }
    }
    let mut out: Vec<Planned> = boxes
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let id = ids[b.id.as_str()].clone();
            Planned { label: if b.label.is_empty() { id.clone() } else { b.label.clone() }, x: cols[i], y: rows[i], prereq: pre.get(&id).cloned().unwrap_or_default(), id }
        })
        .collect();
    out.sort_by_key(|f| (f.y, f.x));
    out
}

/// The text of a tree of planned focuses.
pub fn tree_text(tree_id: &str, tag: &str, fs: &[Planned]) -> String {
    let mut s = format!("focus_tree = {{\n\tid = {tree_id}\n\tcountry = {{\n\t\tfactor = 0\n\t\tmodifier = {{\n\t\t\tadd = 10\n\t\t\ttag = {tag}\n\t\t}}\n\t}}\n\tdefault = no\n");
    for f in fs {
        s += &format!("\n\tfocus = {{\n\t\tid = {}\n\t\ticon = GFX_goal_unknown\n\t\tx = {}\n\t\ty = {}\n\t\tcost = 10\n", f.id, f.x, f.y);
        for p in &f.prereq {
            s += &format!("\t\tprerequisite = {{ focus = {p} }}\n");
        }
        s += "\t\tavailable_if_capitulated = yes\n\t\tcompletion_reward = {\n\t\t}\n\t\tai_will_do = { factor = 1 }\n\t}\n";
    }
    s += "}\n";
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAIN: &str = r#"<mxfile><diagram name="Tree"><mxGraphModel><root>
<mxCell id="0"/><mxCell id="1" parent="0"/>
<mxCell id="a" value="Reform &lt;b&gt;the&lt;/b&gt; Army" vertex="1" parent="1"><mxGeometry x="100" y="20" width="120" height="60" as="geometry"/></mxCell>
<mxCell id="b" value="Tanks" vertex="1" parent="1"><mxGeometry x="40" y="140" width="120" height="60" as="geometry"/></mxCell>
<mxCell id="c" value="Planes" vertex="1" parent="1"><mxGeometry x="200" y="145" width="120" height="60" as="geometry"/></mxCell>
<mxCell id="e1" edge="1" source="a" target="b" parent="1"><mxGeometry relative="1" as="geometry"/></mxCell>
<mxCell id="e2" edge="1" source="a" target="c" parent="1"/>
</root></mxGraphModel></diagram></mxfile>"#;

    #[test]
    fn reads_plain_and_compressed() {
        let p = pages(PLAIN).unwrap();
        assert_eq!(p[0].0, "Tree");
        let (b, a) = parse_page(&p[0].1);
        assert_eq!(b.len(), 3);
        assert_eq!(b[0].label, "Reform the Army");
        assert_eq!(a.len(), 2);
        let f = focuses(&b, &a, "TYR", false, &BTreeSet::new());
        assert_eq!(f[0].id, "TYR_reform_the_army");
        assert_eq!((f[0].x, f[0].y), (1, 0));
        assert_eq!((f[1].id.as_str(), f[1].x, f[1].y), ("TYR_tanks", 0, 1));
        assert_eq!((f[2].id.as_str(), f[2].x), ("TYR_planes", 3), "a box is two columns wide");
        assert_eq!(f[2].y, 1, "boxes 5 px apart share a row");
        assert_eq!(f[1].prereq, ["TYR_reform_the_army"]);
        let t = tree_text("tyr_drawn", "TYR", &f);
        assert!(crate::content::script::Script::parse(&t).errors.is_empty());

        // compressed: the model, URL-escaped, deflated, base64
        let model = &p[0].1;
        let esc: String = model.bytes().map(|c| if c.is_ascii_alphanumeric() { (c as char).to_string() } else { format!("%{c:02X}") }).collect();
        let mut enc = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut enc, esc.as_bytes()).unwrap();
        let raw = enc.finish().unwrap();
        const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut b64 = String::new();
        for ch in raw.chunks(3) {
            let n = (ch[0] as u32) << 16 | (*ch.get(1).unwrap_or(&0) as u32) << 8 | *ch.get(2).unwrap_or(&0) as u32;
            for k in 0..4 {
                if k <= ch.len() {
                    b64.push(A[(n >> (18 - 6 * k) & 63) as usize] as char);
                } else {
                    b64.push('=');
                }
            }
        }
        let file = format!("<mxfile><diagram name=\"Z\">{b64}</diagram></mxfile>");
        let p2 = pages(&file).unwrap();
        assert_eq!(parse_page(&p2[0].1).0.len(), 3);
        assert!(pages("<html/>").is_err());
    }
}
