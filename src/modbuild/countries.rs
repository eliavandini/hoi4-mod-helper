//! Countries of the mod: tags from settlement names, map colours unlike the
//! neighbours', procedural flags.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;

use anyhow::Result;
use unicode_normalization::UnicodeNormalization as _;

use super::img::{self, Filter};

/// 3-letter words of the script language, and file names Windows reserves
/// (flags are TAG.tga).
pub const RESERVED_TAGS: [&str; 18] =
    ["AND", "NOT", "NOR", "ANY", "ALL", "YES", "LOG", "TAG", "OOB", "MAX", "MIN", "ADD", "SET", "VAR", "CON", "PRN", "AUX", "NUL"];

/// Cities first, then towns, then villages (and anything else).
pub fn place_rank(place: &str) -> u8 {
    match place {
        "city" => 0,
        "town" => 1,
        _ => 2,
    }
}

/// ASCII letters only (accents dropped), for tags, file names and comments.
pub fn ascii_name(t: &str) -> String {
    t.replace('ß', "ss").replace(';', ",").nfkd().filter(char::is_ascii).collect()
}

/// OSM names South Tyrolean places "Majority - minority language"; a country
/// takes the first.
pub fn short_name(name: &str) -> &str {
    name.split(" - ").next().unwrap_or("").trim()
}

/// Seeded random numbers (SplitMix64).
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03)
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in [0, 1).
    pub fn f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    pub fn uniform(&mut self, a: f64, b: f64) -> f64 {
        a + (b - a) * self.f64()
    }
    /// Uniform in 0..n.
    pub fn below(&mut self, n: usize) -> usize {
        ((self.next_u64() as u128 * n as u128) >> 64) as usize
    }
    /// `k` indices of 0..n, distinct if n >= k.
    pub fn choose(&mut self, n: usize, k: usize) -> Vec<usize> {
        if n < k {
            return (0..k).map(|_| self.below(n)).collect();
        }
        let mut idx: Vec<usize> = (0..n).collect();
        for i in 0..k {
            let j = i + self.below(n - i);
            idx.swap(i, j);
        }
        idx.truncate(k);
        idx
    }
}

pub struct TagInput<'a> {
    pub id: u32,
    pub name: &'a str,
    pub place: &'a str,
    pub km2: f64,
}

/// A 3-letter tag per nation from the letters of its name in order (Bolzano:
/// BOL, else BLZ, BOZ, ...), cities and towns first; never one of `taken`.
pub fn make_tags(nations: &[TagInput], taken: &BTreeSet<String>) -> BTreeMap<u32, String> {
    let mut taken = taken.clone();
    let mut order: Vec<&TagInput> = nations.iter().collect();
    order.sort_by(|a, b| place_rank(a.place).cmp(&place_rank(b.place)).then(b.km2.total_cmp(&a.km2)).then(a.id.cmp(&b.id)));
    let az: Vec<u8> = (b'A'..=b'Z').collect();
    let mut out = BTreeMap::new();
    for nat in order {
        let mut letters: Vec<u8> = ascii_name(short_name(nat.name)).to_ascii_uppercase().bytes().filter(u8::is_ascii_uppercase).collect();
        if letters.is_empty() {
            letters = b"X".to_vec();
        }
        let (head, rest) = (letters[0], &letters[1..]);
        let free = |t: &[u8]| t.len() == 3 && !taken.contains(std::str::from_utf8(t).unwrap());
        let mut found: Option<Vec<u8>> = None;
        let first = letters[..letters.len().min(3)].to_vec();
        let consonants: Vec<u8> = rest.iter().copied().filter(|c| !b"AEIOU".contains(c)).take(2).collect();
        let second: Vec<u8> = std::iter::once(head).chain(consonants).collect();
        for c in [first, second] {
            if found.is_none() && free(&c) {
                found = Some(c);
            }
        }
        'pairs: for i in 0..rest.len() {
            for j in i + 1..rest.len() {
                if found.is_some() {
                    break 'pairs;
                }
                let c = vec![head, rest[i], rest[j]];
                if free(&c) {
                    found = Some(c);
                }
            }
        }
        'heads: for &x in &az {
            for &y in &az {
                if found.is_some() {
                    break 'heads;
                }
                let c = vec![head, x, y];
                if free(&c) {
                    found = Some(c);
                }
            }
        }
        'all: for &a in &az {
            for &x in &az {
                for &y in &az {
                    if found.is_some() {
                        break 'all;
                    }
                    let c = vec![a, x, y];
                    if free(&c) {
                        found = Some(c);
                    }
                }
            }
        }
        let tag = String::from_utf8(found.expect("all 17576 tags taken")).unwrap();
        taken.insert(tag.clone());
        out.insert(nat.id, tag);
    }
    out
}

// ------------------------------------------------------------------ colours

/// colorsys.hsv_to_rgb
fn hsv_to_rgb(h: f64, s: f64, v: f64) -> [f64; 3] {
    if s == 0.0 {
        return [v, v, v];
    }
    let i = (h * 6.0) as i64;
    let f = h * 6.0 - i as f64;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    match i.rem_euclid(6) {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

/// sRGB 0..255 -> CIELAB (D65).
fn to_lab(rgb: [f64; 3]) -> [f64; 3] {
    let c = rgb.map(|v| {
        let v = v / 255.0;
        if v > 0.04045 { ((v + 0.055) / 1.055).powf(2.4) } else { v / 12.92 }
    });
    let m = [[0.4124, 0.3576, 0.1805], [0.2126, 0.7152, 0.0722], [0.0193, 0.1192, 0.9505]];
    let white = [0.95047, 1.0, 1.08883];
    let f: Vec<f64> = (0..3)
        .map(|r| {
            let x = (m[r][0] * c[0] + m[r][1] * c[1] + m[r][2] * c[2]) / white[r];
            if x > 0.008856 { x.cbrt() } else { 7.787 * x + 16.0 / 116.0 }
        })
        .collect();
    [116.0 * f[1] - 16.0, 500.0 * (f[0] - f[1]), 200.0 * (f[1] - f[2])]
}

/// Map colours: each nation takes, of some random muted colours, the one
/// farthest from the colours of its neighbours (and, weaker, of their
/// neighbours).
pub fn nation_colours(adj: &BTreeMap<u32, BTreeSet<u32>>, rng: &mut Rng) -> BTreeMap<u32, [u8; 3]> {
    const TRIES: usize = 96;
    let mut order: Vec<u32> = adj.keys().copied().collect();
    order.sort_by_key(|i| (std::cmp::Reverse(adj[i].len()), *i));
    let mut lab: BTreeMap<u32, [f64; 3]> = BTreeMap::new();
    let mut out = BTreeMap::new();
    for nid in order {
        let cand: Vec<[f64; 3]> = (0..TRIES)
            .map(|_| {
                let (h, s, v) = (rng.f64(), rng.uniform(0.3, 0.75), rng.uniform(0.45, 0.9));
                hsv_to_rgb(h, s, v).map(|x| x * 255.0)
            })
            .collect();
        let cl: Vec<[f64; 3]> = cand.iter().map(|&c| to_lab(c)).collect();
        let near = &adj[&nid];
        let second: BTreeSet<u32> = near.iter().flat_map(|x| adj.get(x).into_iter().flatten().copied()).filter(|x| !near.contains(x) && *x != nid).collect();
        let mut score = vec![1e9f64; TRIES];
        for (x, weight) in near.iter().map(|&x| (x, 1.0)).chain(second.iter().map(|&x| (x, 1.7))) {
            if let Some(l) = lab.get(&x) {
                for (s, c) in score.iter_mut().zip(&cl) {
                    let d = ((c[0] - l[0]).powi(2) + (c[1] - l[1]).powi(2) + (c[2] - l[2]).powi(2)).sqrt();
                    *s = s.min(weight * d);
                }
            }
        }
        let k = (0..TRIES).fold(0, |b, i| if score[i] > score[b] { i } else { b });
        lab.insert(nid, cl[k]);
        out.insert(nid, cand[k].map(|v| v.round_ties_even().clamp(0.0, 255.0) as u8));
    }
    out
}

// ------------------------------------------------------------------ flags

/// Flag tinctures: a metal always lies next to a colour.
const METALS: [[u8; 3]; 2] = [[242, 242, 236], [240, 190, 35]];
const COLOURS: [[u8; 3]; 9] =
    [[200, 30, 45], [25, 70, 160], [20, 115, 55], [28, 28, 28], [115, 40, 120], [110, 170, 225], [135, 25, 35], [215, 110, 25], [20, 110, 120]];
const PATTERNS: [&str; 17] = [
    "fess", "pale", "tri_fess", "tri_pale", "tri_fess3", "tri_pale3", "cross", "nordic", "saltire", "quarterly", "bend",
    "bend_sinister", "chief", "bordure", "disc", "canton", "hoist_triangle",
];

/// Pattern and its tinctures (field, charge, third stripe).
pub type FlagKey = (usize, [u8; 3], [u8; 3], [u8; 3]);

/// A procedural flag (82×52 RGB) in heraldic tinctures, unlike those in `used`.
pub fn flag_rgb(rng: &mut Rng, used: &mut HashSet<FlagKey>) -> Vec<u8> {
    let (w, h, ss) = (82usize, 52usize, 4usize);
    let mut key: FlagKey = (0, [0; 3], [0; 3], [0; 3]);
    for _ in 0..10_000 {
        let pat = rng.below(PATTERNS.len());
        let metal = METALS[rng.below(METALS.len())];
        let pick = rng.choose(COLOURS.len(), 2);
        let (c1, c2) = (COLOURS[pick[0]], COLOURS[pick[1]]);
        key = if PATTERNS[pat].ends_with('3') {
            (pat, c1, metal, c2)
        } else if rng.f64() < 0.5 {
            (pat, metal, c1, [0; 3])
        } else {
            (pat, c1, metal, [0; 3])
        };
        if !used.contains(&key) {
            break;
        }
    }
    used.insert(key);
    let pat = PATTERNS[key.0];
    let (wf, hf) = (w as f64, h as f64);
    let arm = 0.1 * hf;
    let (bw, bh) = (w * ss, h * ss);
    let mut big = vec![0u8; bw * bh * 3];
    for r in 0..bh {
        for c in 0..bw {
            let (y, x) = ((r as f64 + 0.5) / ss as f64, (c as f64 + 0.5) / ss as f64);
            let (u, v) = (x / wf, y / hf);
            let charge = match pat {
                "fess" => v >= 0.5,
                "pale" => u >= 0.5,
                "tri_fess" => (v - 0.5).abs() < 1.0 / 6.0,
                "tri_pale" => (u - 0.5).abs() < 1.0 / 6.0,
                "tri_fess3" => v >= 1.0 / 3.0,
                "tri_pale3" => u >= 1.0 / 3.0,
                "cross" => (y - hf / 2.0).abs() < arm || (x - wf / 2.0).abs() < arm,
                "nordic" => (y - hf / 2.0).abs() < arm || (x - 0.36 * wf).abs() < arm,
                "saltire" => (v - u).abs() < 0.12 || (v - (1.0 - u)).abs() < 0.12,
                "quarterly" => (u >= 0.5) != (v >= 0.5),
                "bend" => v > u,
                "bend_sinister" => v > 1.0 - u,
                "chief" => v < 1.0 / 3.0,
                "bordure" => x < 6.0 || x > wf - 6.0 || y < 6.0 || y > hf - 6.0,
                "disc" => (x - wf / 2.0).hypot(y - hf / 2.0) < 0.3 * hf,
                "canton" => u < 0.45 && v < 0.5,
                _ => x < 0.45 * wf * (1.0 - (2.0 * v - 1.0).abs()), // hoist_triangle
            };
            let third = match pat {
                "tri_fess3" => v >= 2.0 / 3.0,
                "tri_pale3" => u >= 2.0 / 3.0,
                _ => false,
            };
            let col = if third {
                key.3
            } else if charge {
                key.2
            } else {
                key.1
            };
            big[(r * bw + c) * 3..][..3].copy_from_slice(&col);
        }
    }
    img::resize_u8(&big, bw, bh, 3, w, h, Filter::Box)
}

/// gfx/flags/TAG.tga 82×52, medium/ 41×26, small/ 10×7: 32-bit TGA like vanilla.
pub fn save_flags(rgb: &[u8], root: &Path, tag: &str) -> Result<()> {
    let rgba: Vec<u8> = rgb.chunks(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect();
    img::write_tga_rgba(&root.join(format!("{tag}.tga")), 82, 52, &rgba)?;
    for (sub, w, h) in [("medium", 41usize, 26usize), ("small", 10, 7)] {
        let small = img::resize_u8(&rgba, 82, 52, 4, w, h, Filter::Lanczos);
        img::write_tga_rgba(&root.join(sub).join(format!("{tag}.tga")), w as u32, h as u32, &small)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(ascii_name("Sëlva - Wolkenstein"), "Selva - Wolkenstein");
        assert_eq!(ascii_name("Straße;Weg"), "Strasse,Weg");
        assert_eq!(short_name("Brixen - Bressanone"), "Brixen");
    }

    #[test]
    fn tags_from_names() {
        let taken: BTreeSet<String> = ["BOL".to_string()].into_iter().collect();
        let nations = [
            TagInput { id: 1, name: "Bolzano - Bozen", place: "city", km2: 50.0 },
            TagInput { id: 2, name: "Brixen - Bressanone", place: "town", km2: 80.0 },
            TagInput { id: 3, name: "Bruneck - Brunico", place: "town", km2: 40.0 },
            TagInput { id: 4, name: "Brixental", place: "village", km2: 90.0 },
        ];
        let t = make_tags(&nations, &taken);
        assert_eq!(t[&1], "BLZ"); // BOL is taken: B + consonants
        assert_eq!(t[&2], "BRI");
        assert_eq!(t[&3], "BRU");
        assert_eq!(t[&4], "BRX"); // BRI taken: B + consonants
    }

    #[test]
    fn rng_choose_distinct() {
        let mut r = Rng::new(1);
        for _ in 0..100 {
            let v = r.choose(9, 2);
            assert_ne!(v[0], v[1]);
            assert!(v.iter().all(|&i| i < 9));
        }
        let x = r.f64();
        assert!((0.0..1.0).contains(&x));
    }

    #[test]
    fn colours_differ_from_neighbours() {
        let mut adj: BTreeMap<u32, BTreeSet<u32>> = BTreeMap::new();
        for (a, b) in [(1, 2), (2, 3), (1, 3)] {
            adj.entry(a).or_default().insert(b);
            adj.entry(b).or_default().insert(a);
        }
        let c = nation_colours(&adj, &mut Rng::new(3));
        assert_eq!(c.len(), 3);
        assert!(c[&1] != c[&2] && c[&2] != c[&3]);
    }
}
