//! terrain.bmp and heightmap.bmp: what the build makes from the land cover
//! and the heights, and what the map editor stores over it: painted terrain
//! (`terrain_paint.bmp`, 255 where the build decides) and an imported
//! heightmap (`heightmap_import.bmp`). The editor previews, exports and
//! imports them with the same code.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use rayon::prelude::*;

use super::img;
use super::vanilla::WATER;
use crate::raster::{self, Pixels};
use crate::util::Progress;

/// Painted terrain over the build's, in the map folder.
pub const PAINT_FILE: &str = "terrain_paint.bmp";
/// A heightmap used instead of the build's, in the map folder.
pub const HEIGHTS_FILE: &str = "heightmap_import.bmp";
/// A pixel of `terrain_paint.bmp` the build decides.
pub const AUTO: u8 = 255;

// land cover classes (CLC+ Backbone, as in landcover_smooth.bmp)
pub const SEALED: u8 = 1;
pub const NEEDLE: u8 = 2;
pub const BROAD: u8 = 3;
pub const BROAD_EVERGREEN: u8 = 4;
pub const LOW_WOODY: u8 = 5;
pub const GRASS: u8 = 6;
pub const CROP: u8 = 7;
pub const MOSS: u8 = 8;
pub const ROCK: u8 = 9;
pub const SNOW: u8 = 11;

// vanilla terrain.bmp palette indices (common/terrain/00_terrain.txt)
pub const T_PLAINS: u8 = 0;
pub const T_FOREST: u8 = 1;
pub const T_FOREST_B: u8 = 4;
pub const T_FARM: u8 = 5;
pub const T_MOUNTAIN: u8 = 6;
pub const T_URBAN: u8 = 13;
pub const T_OCEAN: u8 = 15;
pub const T_SNOW: u8 = 16;
pub const T_HILLS: u8 = 17;
pub const T_ALPINE: u8 = 20;

/// A terrain.bmp index of vanilla's 00_terrain.txt: its name here, its key
/// and type there, and whether it is for land (lakes and ocean are not).
pub struct Texture {
    pub index: u8,
    pub name: &'static str,
    pub key: &'static str,
    pub kind: &'static str,
    pub land: bool,
}

const fn t(index: u8, name: &'static str, key: &'static str, kind: &'static str) -> Texture {
    Texture { index, name, key, kind, land: true }
}

/// The terrain.bmp indices vanilla defines, those the build uses first.
pub const TEXTURES: [Texture; 25] = [
    t(T_PLAINS, "plains (grass)", "terrain_0", "plains"),
    t(T_FARM, "farmland", "terrain_5", "plains"),
    t(T_FOREST, "forest (conifer)", "terrain_1", "forest"),
    t(T_FOREST_B, "forest (broadleaf)", "terrain_4", "forest"),
    t(T_HILLS, "hills", "hills_blend", "hills"),
    t(T_ALPINE, "alpine grass", "mountain_variation_grass", "mountain"),
    t(T_MOUNTAIN, "mountain (rock)", "terrain_6", "mountain"),
    t(T_SNOW, "snow", "snow_16", "mountain"),
    t(T_URBAN, "urban", "forest_13", "urban"),
    t(10, "mountain 2", "terrain_10", "mountain"),
    t(18, "mountain (sand)", "mountain_variation_sand", "mountain"),
    t(27, "mountain 3", "jungle_blend_18 (27)", "mountain"),
    t(19, "snowy plains", "plains_17", "plains"),
    t(9, "marsh", "terrain_9", "marsh"),
    t(2, "desert mountain", "desert_mountain", "hills"),
    t(11, "desert mountain 2", "desert (11)", "mountain"),
    t(31, "desert mountain tops", "desert_mountain_tops", "mountain"),
    t(8, "desert hills", "desert_hills", "desert"),
    t(3, "desert", "desert (3)", "desert"),
    t(7, "desert 2", "terrain_7", "desert"),
    t(12, "desert 3", "desert (12)", "desert"),
    t(21, "jungle", "jungle_18", "jungle"),
    t(22, "jungle (blend)", "jungle_blend_18", "jungle"),
    Texture { index: 14, name: "lakes", key: "forest_14", kind: "lakes", land: false },
    Texture { index: T_OCEAN, name: "ocean", key: "ocean_15", kind: "ocean", land: false },
];

pub fn texture(index: u8) -> Option<&'static Texture> {
    TEXTURES.iter().find(|t| t.index == index)
}

/// "17 hills", or the bare index for one vanilla does not define.
pub fn texture_name(index: u8) -> String {
    match texture(index) {
        Some(t) => format!("{} {}", t.index, t.name),
        None => format!("{index} (not in vanilla)"),
    }
}

/// Vanilla's terrain.bmp palette (255 colours): grey `i` for index `i`
/// except the terrains.
pub fn vanilla_palette() -> Vec<[u8; 3]> {
    const COLOURS: [(u8, [u8; 3]); 26] = [
        (0, [86, 124, 27]),
        (1, [0, 86, 6]),
        (2, [112, 74, 31]),
        (3, [206, 169, 99]),
        (4, [6, 200, 11]),
        (5, [255, 0, 24]),
        (6, [134, 84, 30]),
        (7, [252, 255, 0]),
        (8, [73, 59, 15]),
        (9, [75, 147, 174]),
        (10, [174, 0, 255]),
        (11, [92, 83, 76]),
        (12, [255, 0, 240]),
        (13, [240, 255, 0]),
        (14, [55, 90, 220]),
        (15, [8, 31, 130]),
        (16, [255, 255, 255]),
        (17, [132, 255, 0]),
        (18, [255, 126, 0]),
        (19, [114, 137, 105]),
        (20, [58, 131, 82]),
        (21, [255, 0, 127]),
        (22, [0, 82, 82]),
        (31, [243, 199, 147]),
        (35, [255, 0, 252]),
        (36, [23, 23, 23]),
    ];
    let mut p: Vec<[u8; 3]> = (0..255u8).map(|i| [i, i, i]).collect();
    for (i, c) in COLOURS {
        p[i as usize] = c;
    }
    p
}

/// The game's terrain.bmp palette, else [`vanilla_palette`].
pub fn game_palette(game: &Path) -> Vec<[u8; 3]> {
    img::read_bmp(&game.join("map/terrain.bmp")).ok().filter(|b| b.bpp == 8).map(|b| b.palette).unwrap_or_else(vanilla_palette)
}

/// The terrain.bmp index the build gives a land pixel from its land cover
/// class and slope (degrees, of the pixel).
pub fn auto_texture(class: u8, slope: f32) -> u8 {
    let gentle = slope < 10.0;
    let steep = slope >= 25.0;
    match class {
        SEALED => T_URBAN,
        SNOW => T_SNOW,
        ROCK | MOSS => T_MOUNTAIN,
        BROAD | BROAD_EVERGREEN => T_FOREST_B,
        NEEDLE => T_FOREST,
        LOW_WOODY if steep => T_ALPINE,
        LOW_WOODY => T_HILLS,
        CROP if !gentle => T_HILLS,
        CROP => T_FARM,
        GRASS if steep => T_ALPINE,
        GRASS if !gentle => T_HILLS,
        _ => T_PLAINS,
    }
}

/// terrain.bmp: ocean on the sea, else the painted index, else the build's.
pub fn final_texture(cover: &[u8], slope: &[f32], sea: &[bool], paint: Option<&[u8]>) -> Vec<u8> {
    (0..sea.len())
        .into_par_iter()
        .map(|i| {
            if sea[i] {
                T_OCEAN
            } else {
                match paint.map(|p| p[i]) {
                    Some(v) if v != AUTO => v,
                    _ => auto_texture(cover[i], slope[i]),
                }
            }
        })
        .collect()
}

/// The name the elevation had in the map folders of old projects.
pub const DEM_FILE: &str = "dem_section.tif";
/// The name the land cover had in the map folders of old projects.
pub const COVER_FILE: &str = "landcover_smooth.bmp";
/// Metres per heightmap step when the heights are made from a heightmap.
pub const STEP_M: f32 = 30.0;
/// The height of a map with no heights at all (m).
pub const FLAT_M: f32 = 200.0;

/// The real-world data a map was made from: files outside the project that
/// it refers to, of the map's size (None: the map has none).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sources {
    /// Heights in metres (a single-band raster).
    pub dem: Option<PathBuf>,
    /// Land cover classes (CLC+ Backbone, 8-bit).
    pub cover: Option<PathBuf>,
}

/// What the build and the editor work from besides the provinces: heights
/// (m) and land cover per pixel.
pub struct Inputs {
    pub dem: Vec<f32>,
    pub cover: Vec<u8>,
    /// What had to be made up (a map without the real data).
    pub notes: Vec<String>,
}

/// The heights and land cover of [`Sources`]. A map made without real data
/// (blank, or imported from a mod) has neither: its heights then come from
/// its imported heightmap ([`STEP_M`] per step from the water plane), or it
/// is flat, and its land cover is grass (the painted terrain decides).
pub fn inputs(src: &Sources, w: usize, h: usize, heights: Option<&[u8]>, progress: &Progress) -> Result<Inputs> {
    let mut notes = Vec::new();
    let read = |path: &Path, what: &str| -> Result<raster::Raster> {
        if !path.is_file() {
            bail!("the {what} file {} is missing (set it in the map editor's Settings)", path.display());
        }
        let (r, _) = raster::load(path, progress).with_context(|| format!("cannot read the {what} file {}", path.display()))?;
        if (r.width as usize, r.height as usize) != (w, h) {
            bail!("the {what} file {} is {} × {} px, the map {w} × {h}", path.display(), r.width, r.height);
        }
        Ok(r)
    };
    let dem: Vec<f32> = if let Some(path) = &src.dem {
        match read(path, "elevation")?.pixels {
            Pixels::GrayF32(v) => v,
            Pixels::Gray16(v) => v.into_iter().map(|x| x as f32).collect(),
            Pixels::Gray8(v) => v.into_iter().map(|x| x as f32).collect(),
            _ => bail!("the elevation file {} is not a single-band height map", path.display()),
        }
    } else if let Some(v) = heights.filter(|v| v.len() == w * h) {
        notes.push(format!("no elevation file: heights from the imported heightmap ({STEP_M} m per step)"));
        v.par_iter().map(|&x| (x as f32 - WATER as f32) * STEP_M).collect()
    } else {
        notes.push("no elevation file and no imported heightmap: the land is flat".to_string());
        vec![FLAT_M; w * h]
    };
    let cover: Vec<u8> = if let Some(path) = &src.cover {
        match read(path, "land cover")?.pixels {
            Pixels::Indexed8 { data, .. } | Pixels::Gray8(data) => data,
            _ => bail!("the land cover file {} is not an 8-bit class map", path.display()),
        }
    } else {
        notes.push("no land cover file: the land is grass where no terrain is painted".to_string());
        vec![GRASS; w * h]
    };
    Ok(Inputs { dem, cover, notes })
}

/// The build's heightmap: land `WATER + 1`..255 for the lowest..highest
/// land, sea from 4 below the water plane at the shore (so 3–7 px channels
/// stay under water in the terrain mesh) to 10 below. Also the land's
/// lowest and highest height (m).
pub fn heightmap(dem: &[f32], sea: &[bool], w: usize, h: usize) -> (Vec<u8>, f32, f32) {
    let (lo, hi) = dem
        .par_iter()
        .zip(sea)
        .filter(|(_, s)| !**s)
        .map(|(&d, _)| (d, d))
        .reduce(|| (f32::INFINITY, f32::NEG_INFINITY), |a, b| (a.0.min(b.0), a.1.max(b.1)));
    let (lo32, span) = (lo, ((hi as f64 - lo as f64) as f32).max(1e-6));
    let depth2 = img::edt_sq(sea, w, h);
    let height: Vec<u8> = (0..w * h)
        .into_par_iter()
        .map(|i| {
            if sea[i] {
                let depth = (depth2[i] as f64).sqrt();
                (WATER as f64 - 4.0 - ((depth - 1.0) / 2.0).min(6.0)).round_ties_even() as u8
            } else {
                ((dem[i] - lo32) / span * (254 - WATER) as f32 + (WATER + 1) as f32).round_ties_even().clamp((WATER + 1) as f32, 255.0) as u8
            }
        })
        .collect();
    (height, lo, hi)
}

/// Metres per heightmap step on land, for heights `lo`..`hi`.
pub fn metres_per_step(lo: f32, hi: f32) -> f32 {
    ((hi as f64 - lo as f64) as f32).max(1e-6) / (254 - WATER) as f32
}

/// An imported heightmap made to fit the sea: land at or below the water
/// plane goes just above it, sea at or above it takes the build's depth.
/// Also how many land and sea pixels changed.
pub fn fit_to_sea(imported: &[u8], built: &[u8], sea: &[bool]) -> (Vec<u8>, usize, usize) {
    let out: Vec<u8> = (0..sea.len())
        .into_par_iter()
        .map(|i| {
            let v = imported[i];
            if sea[i] {
                if v >= WATER { built[i] } else { v }
            } else {
                v.max(WATER + 1)
            }
        })
        .collect();
    let (land, water) = (0..sea.len()).fold((0, 0), |(l, s), i| (l + (!sea[i] && imported[i] <= WATER) as usize, s + (sea[i] && imported[i] >= WATER) as usize));
    (out, land, water)
}

/// A grey image as values 0..255: an 8-bit BMP (its palette's grey, or the
/// red of a colour palette), a 24-bit BMP (red), or anything the viewer
/// reads (PNG, TIFF; 16-bit scaled to 8). Also its width and height.
pub fn read_grey(path: &Path) -> Result<(u32, u32, Vec<u8>)> {
    let is_bmp = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("bmp"));
    if is_bmp {
        let b = img::read_bmp(path)?;
        let v = if b.bpp == 8 { b.data.par_iter().map(|&i| b.palette.get(i as usize).map_or(i, |c| c[0])).collect() } else { b.data.par_chunks(3).map(|c| c[0]).collect() };
        return Ok((b.w, b.h, v));
    }
    let (r, _) = raster::load(path, &Progress::default())?;
    let v: Vec<u8> = match r.pixels {
        Pixels::Gray8(v) => v,
        Pixels::Gray16(v) => v.par_iter().map(|&x| ((x as u32 + 128) / 257) as u8).collect(),
        Pixels::Indexed8 { data, palette } => data.par_iter().map(|&i| palette.get(i as usize).map_or(i, |c| c[0])).collect(),
        Pixels::Rgb8(v) => v.par_chunks(3).map(|c| c[0]).collect(),
        Pixels::Rgba8(v) => v.par_chunks(4).map(|c| c[0]).collect(),
        Pixels::GrayF32(_) => bail!("{}: a float image; heightmaps are 0..255", path.display()),
    };
    Ok((r.width, r.height, v))
}

/// terrain.bmp indices from a bitmap: an 8-bit BMP as its indices, a 24-bit
/// one by its colours in `palette`. Also its width and height.
pub fn read_texture(path: &Path, palette: &[[u8; 3]]) -> Result<(u32, u32, Vec<u8>)> {
    let b = img::read_bmp(path)?;
    if b.bpp == 8 {
        return Ok((b.w, b.h, b.data));
    }
    let mut by_colour = rustc_hash::FxHashMap::default();
    for (i, c) in palette.iter().enumerate().rev() {
        // the terrains win over the greys that share their colour
        if texture(i as u8).is_some() || !by_colour.contains_key(c) {
            by_colour.insert(*c, i as u8);
        }
    }
    let mut out = Vec::with_capacity(b.data.len() / 3);
    for (k, c) in b.data.chunks(3).enumerate() {
        match by_colour.get(&[c[0], c[1], c[2]]) {
            Some(&i) => out.push(i),
            None => bail!("{}: the colour {c:?} at x {}, y {} is not a terrain.bmp colour (save it as an 8-bit indexed bitmap with the game's palette)", path.display(), k % b.w as usize, k / b.w as usize),
        }
    }
    Ok((b.w, b.h, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_and_names() {
        let p = vanilla_palette();
        assert_eq!(p.len(), 255);
        assert_eq!(p[17], [132, 255, 0]);
        assert_eq!(p[40], [40, 40, 40]);
        assert_eq!(texture_name(20), "20 alpine grass");
        let mut idx: Vec<u8> = TEXTURES.iter().map(|t| t.index).collect();
        idx.sort_unstable();
        idx.dedup();
        assert_eq!(idx.len(), TEXTURES.len(), "an index twice");
        // the build's choices are land textures
        for c in 0..=12 {
            for s in [0.0, 12.0, 30.0] {
                assert!(texture(auto_texture(c, s)).is_some_and(|t| t.land));
            }
        }
    }

    #[test]
    fn painted_terrain_and_sea() {
        let cover = [CROP, CROP, NEEDLE, GRASS];
        let slope = [2.0, 15.0, 2.0, 30.0];
        let sea = [false, false, false, true];
        assert_eq!(final_texture(&cover, &slope, &sea, None), [T_FARM, T_HILLS, T_FOREST, T_OCEAN]);
        let paint = [AUTO, 9, AUTO, 9];
        assert_eq!(final_texture(&cover, &slope, &sea, Some(&paint)), [T_FARM, 9, T_FOREST, T_OCEAN]);
    }

    #[test]
    fn imported_heights_fit_the_sea() {
        let sea = [true, true, false, false];
        let built = [91, 89, 120, 200];
        let (v, land, water) = fit_to_sea(&[90, 100, 95, 180], &built, &sea);
        assert_eq!(v, [90, 89, 96, 180]);
        assert_eq!((land, water), (1, 1));
    }
}
