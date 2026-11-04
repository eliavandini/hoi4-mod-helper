//! What the build takes from the HOI4 install: texture sizes and palettes,
//! the game version, the colours / trees / city look of the vanilla Alps,
//! country tags, name lists and localisation keys.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context as _, Result, bail};
use regex::Regex;

use super::img::{self, Filter};

/// Heightmap value of the water plane (Y = 9.5).
pub const WATER: u8 = 95;

/// map/terrain textures whose size depends on the map size.
pub const DDS_FILES: [&str; 5] = [
    "colormap_rgb_cityemissivemask_a.dds",
    "colormap_water_0.dds",
    "colormap_water_1.dds",
    "colormap_water_2.dds",
    "fow_rgb_waterspec_a.dds",
];

/// An indexed vanilla bitmap: size, palette, most common index.
pub struct Indexed {
    pub size: (u32, u32),
    pub palette: Vec<[u8; 3]>,
    pub bg: u8,
}

/// A vanilla terrain texture: size and median colour over land and water.
pub struct DdsSample {
    pub size: (u32, u32),
    pub land: [u8; 4],
    pub water: [u8; 4],
}

pub struct Vanilla {
    pub map_size: (u32, u32),
    pub terrain: Indexed,
    pub rivers: Indexed,
    pub trees: Indexed,
    pub cities: Indexed,
    pub normal_size: (u32, u32),
    pub dds: BTreeMap<&'static str, DdsSample>,
    /// "1.19.*" from launcher-settings.json.
    pub version: String,
    /// Colour of each terrain.bmp index around Innsbruck (else anywhere).
    pub colours: Vec<Option<[u8; 3]>>,
    /// trees.bmp index of the Alpine forest, cities.bmp index around Innsbruck.
    pub tree_index: u8,
    pub city_index: u8,
}

/// Read text leniently (BOM, invalid UTF-8 replaced).
pub fn read_text(path: &Path) -> Result<String> {
    let b = std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    let s = String::from_utf8_lossy(&b).into_owned();
    Ok(s.strip_prefix('\u{feff}').map(str::to_string).unwrap_or(s))
}

fn most_common(data: &[u8]) -> u8 {
    let mut hist = [0usize; 256];
    for &v in data {
        hist[v as usize] += 1;
    }
    (0..256).max_by_key(|&i| (hist[i], std::cmp::Reverse(i))).unwrap() as u8
}

/// numpy's median of 8-bit values, truncated to an integer.
fn median(hist: &[usize; 256]) -> u8 {
    let n: usize = hist.iter().sum();
    if n == 0 {
        return 0;
    }
    let nth = |k: usize| -> usize {
        let mut acc = 0;
        for (v, &c) in hist.iter().enumerate() {
            acc += c;
            if acc > k {
                return v;
            }
        }
        255
    };
    if n % 2 == 1 { nth(n / 2) as u8 } else { ((nth(n / 2 - 1) + nth(n / 2)) / 2) as u8 }
}

/// Per-channel medians of the first `ch` channels; None without pixels.
fn medians<'a>(px: impl Iterator<Item = &'a [u8]>, ch: usize) -> Option<Vec<u8>> {
    let mut hist = vec![[0usize; 256]; ch];
    let mut n = 0usize;
    for p in px {
        n += 1;
        for c in 0..ch {
            hist[c][p[c] as usize] += 1;
        }
    }
    (n > 0).then(|| hist.iter().map(median).collect())
}

fn indexed(game: &Path, name: &str) -> Result<(Indexed, img::Bmp)> {
    let f = game.join("map").join(format!("{name}.bmp"));
    let b = img::read_bmp(&f)?;
    if b.bpp != 8 {
        bail!("{} is not an indexed bitmap", f.display());
    }
    Ok((Indexed { size: (b.w, b.h), palette: b.palette.clone(), bg: most_common(&b.data) }, b))
}

pub fn load(game: &Path, log: &dyn Fn(&str)) -> Result<Vanilla> {
    let m = game.join("map");
    if !m.join("provinces.bmp").exists() {
        bail!("{} not found: is the game folder right?", m.join("provinces.bmp").display());
    }
    let ph = img::bmp_header(&m.join("provinces.bmp"))?;
    let map_size = (ph.w, ph.h);
    let (terrain, terr_bmp) = indexed(game, "terrain")?;
    let (rivers, _) = indexed(game, "rivers")?;
    let (trees, trees_bmp) = indexed(game, "trees")?;
    let (cities, cities_bmp) = indexed(game, "cities")?;
    let nh = img::bmp_header(&m.join("world_normal.bmp"))?;
    let hm = img::read_bmp(&m.join("heightmap.bmp"))?;
    let height: Vec<u8> = if hm.bpp == 8 { hm.data.iter().map(|&i| hm.palette.get(i as usize).map_or(i, |c| c[0])).collect() } else { hm.data.chunks(3).map(|c| c[0]).collect() };

    let mut dds = BTreeMap::new();
    let mut colormap = None;
    for name in DDS_FILES {
        let f = m.join("terrain").join(name);
        let (w, h, rgba) = match img::read_dds(&f) {
            Ok(x) => x,
            Err(e) => {
                log(&format!("note: cannot read {name} ({e:#}); using defaults"));
                continue;
            }
        };
        let hs = img::resize_u8(&height, hm.w as usize, hm.h as usize, 1, w as usize, h as usize, Filter::Bilinear);
        let land = medians(rgba.chunks(4).zip(&hs).filter(|(_, h)| **h > WATER).map(|(p, _)| p), 4);
        let water = medians(rgba.chunks(4).zip(&hs).filter(|(_, h)| **h <= WATER).map(|(p, _)| p), 4);
        let pick = |v: Option<Vec<u8>>| -> [u8; 4] { v.map_or([128; 4], |v| [v[0], v[1], v[2], v[3]]) };
        dds.insert(name, DdsSample { size: (w, h), land: pick(land), water: pick(water) });
        if name == DDS_FILES[0] {
            colormap = Some((w, h, rgba));
        }
    }

    let version = game_version(game);

    // the vanilla Alps: around the victory point named Innsbruck
    let (vw, vh) = (map_size.0 as usize, map_size.1 as usize);
    let (mut r0, mut c0) = (vh / 3, vw / 2);
    let loc = read_text(&game.join("localisation/english/victory_points_l_english.yml")).unwrap_or_default();
    if let Some(cap) = Regex::new(r#"VICTORY_POINTS_(\d+):\d*\s*"Innsbruck""#).unwrap().captures(&loc) {
        let id = &cap[1];
        let defs = std::fs::read(m.join("definition.csv")).unwrap_or_default();
        let defs: String = defs.iter().map(|&b| b as char).collect(); // latin-1
        if let Some(rgb) = defs.lines().map(|l| l.split(';').collect::<Vec<_>>()).find(|f| f[0] == id).and_then(|f| {
            let p = |i: usize| f.get(i).and_then(|v| v.trim().parse::<u8>().ok());
            Some([p(1)?, p(2)?, p(3)?])
        }) {
            let prov = img::read_bmp(&m.join("provinces.bmp"))?;
            let (mut sr, mut sc, mut n) = (0u64, 0u64, 0u64);
            for (i, px) in prov.data.chunks(3).enumerate() {
                if px == rgb {
                    sr += (i / vw) as u64;
                    sc += (i % vw) as u64;
                    n += 1;
                }
            }
            if n > 0 {
                (r0, c0) = ((sr as f64 / n as f64) as usize, (sc as f64 / n as f64) as usize);
            }
        }
    }
    // rows / columns of the window, for an image `k` times the map size
    let win = |k: f64, h: usize, w: usize| -> (std::ops::Range<usize>, std::ops::Range<usize>) {
        let r = ((r0 as f64 - 250.0) * k).max(0.0) as usize..(((r0 as f64 + 250.0) * k) as usize).min(h);
        let c = ((c0 as f64 - 400.0) * k).max(0.0) as usize..(((c0 as f64 + 400.0) * k) as usize).min(w);
        (r, c)
    };
    let mut colours = vec![None; 256];
    if let Some((cw, ch, cmap)) = &colormap {
        let (cw, ch) = (*cw as usize, *ch as usize);
        let k = cw as f64 / vw as f64;
        let step = (1.0 / k).round().max(1.0) as usize;
        // vanilla terrain at the colour map's resolution
        let small: Vec<u8> = (0..ch).flat_map(|r| (0..cw).map(move |c| (r, c))).map(|(r, c)| {
            let (rr, cc) = (r * step, c * step);
            if rr < vh && cc < vw { terr_bmp.data[rr * vw + cc] } else { 255 }
        }).collect();
        let (wr, wc) = win(k, ch, cw);
        let mut count_win = [0usize; 256];
        for r in wr.clone() {
            for c in wc.clone() {
                count_win[small[r * cw + c] as usize] += 1;
            }
        }
        for t in 0..256 {
            let sel: Vec<&[u8]> = if count_win[t] > 50 {
                wr.clone().flat_map(|r| wc.clone().map(move |c| r * cw + c)).filter(|&i| small[i] as usize == t).map(|i| &cmap[i * 4..i * 4 + 4]).collect()
            } else {
                (0..cw * ch).filter(|&i| small[i] as usize == t).map(|i| &cmap[i * 4..i * 4 + 4]).collect()
            };
            if sel.len() > 50 {
                colours[t] = medians(sel.into_iter(), 3).map(|m| [m[0], m[1], m[2]]);
            }
        }
    }
    let (tw, th) = (trees_bmp.w as usize, trees_bmp.h as usize);
    let (wr, wc) = win(tw as f64 / vw as f64, th, tw);
    let mut tc = [0usize; 256];
    for r in wr {
        for c in wc.clone() {
            tc[trees_bmp.data[r * tw + c] as usize] += 1;
        }
    }
    let tree_index = [3u8, 4, 7, 10].into_iter().fold((3u8, 0usize), |best, i| if tc[i as usize] > best.1 { (i, tc[i as usize]) } else { best }).0;
    let (cw, ch) = (cities_bmp.w as usize, cities_bmp.h as usize);
    let (wr, wc) = win(cw as f64 / vw as f64, ch, cw);
    let city_px: Vec<u8> = wr.flat_map(|r| wc.clone().map(move |c| (r, c))).map(|(r, c)| cities_bmp.data[r * cw + c]).collect();
    let city_index = most_common(&city_px);

    Ok(Vanilla {
        map_size,
        terrain,
        rivers,
        trees,
        cities,
        normal_size: (nh.w, nh.h),
        dds,
        version,
        colours,
        tree_index,
        city_index,
    })
}

/// The game version as mods give it ("1.19.*"), from launcher-settings.json.
pub fn game_version(game: &Path) -> String {
    let raw = std::fs::read_to_string(game.join("launcher-settings.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.get("rawVersion").and_then(|r| r.as_str()).map(str::to_string));
    if let Some(raw) = raw {
        let parts: Vec<&str> = raw.trim_start_matches('v').split('.').collect();
        let num = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
        if parts.len() >= 2 && num(parts[0]) && num(parts[1]) {
            return format!("{}.{}.*", parts[0], parts[1]);
        }
    }
    "1.*".to_string()
}

/// Country tags of the game.
pub fn tags(game: &Path) -> Result<BTreeSet<String>> {
    let re = Regex::new(r"(?m)^\s*([A-Z][A-Z0-9]{2})\s*=").unwrap();
    let mut out = BTreeSet::new();
    let dir = game.join("common/country_tags");
    for f in txt_files(&dir)? {
        for c in re.captures_iter(&read_text(&f)?) {
            out.insert(c[1].to_string());
        }
    }
    Ok(out)
}

/// `*.txt` (or other extension) files of a folder, sorted.
pub fn files_with(dir: &Path, ext: &str) -> Result<Vec<std::path::PathBuf>> {
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .with_context(|| format!("cannot list {}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == ext))
        .collect();
    v.sort();
    Ok(v)
}

pub fn txt_files(dir: &Path) -> Result<Vec<std::path::PathBuf>> {
    files_with(dir, "txt")
}

/// The bodies of vanilla name lists (first names, surnames, callsigns...) of `tags`.
pub fn name_blocks(game: &Path, tags: &[&str]) -> Result<BTreeMap<String, String>> {
    let text = read_text(&game.join("common/names/00_names.txt"))?;
    let b = text.as_bytes();
    let mut out = BTreeMap::new();
    for &tag in tags {
        let re = Regex::new(&format!(r"(?m)^{tag}\s*=\s*\{{")).unwrap();
        let Some(m) = re.find(&text) else { bail!("no {tag} block in common/names/00_names.txt") };
        let (mut i, mut depth) = (m.end(), 1);
        while depth > 0 {
            if i >= b.len() {
                bail!("unterminated {tag} block in common/names/00_names.txt");
            }
            match b[i] {
                b'#' => i = text[i..].find('\n').map_or(b.len(), |k| i + k),
                b'"' => i = text[i + 1..].find('"').map_or(b.len(), |k| i + 1 + k),
                b'{' => depth += 1,
                b'}' => depth -= 1,
                _ => {}
            }
            i += 1;
        }
        out.insert(tag.to_string(), text[m.end()..i - 1].to_string());
    }
    Ok(out)
}

/// Vanilla localises states per owner (AUS_STATE_4); per state id, the tags
/// that have such a key.
pub fn owner_state_keys(game: &Path) -> Result<BTreeMap<u32, BTreeSet<String>>> {
    let re = Regex::new(r"(?m)^\s*([A-Z][A-Z0-9]{2})_STATE_(\d+):").unwrap();
    let mut out: BTreeMap<u32, BTreeSet<String>> = BTreeMap::new();
    for f in files_with(&game.join("localisation/english"), "yml")? {
        for c in re.captures_iter(&read_text(&f)?) {
            if let Ok(id) = c[2].parse() {
                out.entry(id).or_default().insert(c[1].to_string());
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn medians_like_numpy() {
        let mut h = [0usize; 256];
        h[10] = 1;
        h[13] = 1;
        assert_eq!(median(&h), 11); // 11.5 truncated
        h[200] = 1;
        assert_eq!(median(&h), 13);
    }
}
