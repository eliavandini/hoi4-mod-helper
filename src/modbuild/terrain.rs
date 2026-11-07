//! The HOI4 terrain type of land provinces (definition.csv) from the land
//! under them: rules checked from the top, each a terrain and the kinds of
//! land (land cover, height, slope) that must make up a share of the
//! province. Stored with the mod details in provinces.json and edited in the
//! map editor's "Mod details" window; the editor previews them with the same
//! code.

use anyhow::{Result, bail};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

/// The terrain types of land provinces.
pub const LAND_TERRAIN: [&str; 8] = ["plains", "forest", "hills", "mountain", "marsh", "urban", "jungle", "desert"];

/// Land cover (CLC+ Backbone classes, as in landcover_smooth.bmp).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cover {
    #[default]
    Any,
    BuiltUp,
    Forest,
    Shrubs,
    Grassland,
    Cropland,
    Bare,
    Water,
    Snow,
}

impl Cover {
    pub const ALL: [Cover; 9] = [Cover::Any, Cover::BuiltUp, Cover::Forest, Cover::Shrubs, Cover::Grassland, Cover::Cropland, Cover::Bare, Cover::Water, Cover::Snow];

    pub fn name(self) -> &'static str {
        match self {
            Cover::Any => "any land",
            Cover::BuiltUp => "built-up",
            Cover::Forest => "forest",
            Cover::Shrubs => "shrubs",
            Cover::Grassland => "grassland",
            Cover::Cropland => "cropland",
            Cover::Bare => "rock and moss",
            Cover::Water => "water",
            Cover::Snow => "snow and ice",
        }
    }

    /// Whether a land cover class is of this cover.
    pub fn matches(self, class: u8) -> bool {
        match self {
            Cover::Any => true,
            Cover::BuiltUp => class == 1,
            Cover::Forest => matches!(class, 2..=4),
            Cover::Shrubs => class == 5,
            Cover::Grassland => class == 6,
            Cover::Cropland => class == 7,
            Cover::Bare => matches!(class, 8 | 9),
            Cover::Water => class == 10,
            Cover::Snow => class == 11,
        }
    }
}

/// A kind of land: pixels of a land cover within a height and slope range
/// (a missing limit is none). Each such pixel counts `weight` towards the
/// rule's share.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LandKind {
    pub cover: Cover,
    /// From this height up (m).
    pub from_m: Option<f64>,
    /// Below this height (m).
    pub below_m: Option<f64>,
    /// From this slope up (degrees, of the pixel).
    pub from_slope: Option<f64>,
    /// Below this slope.
    pub below_slope: Option<f64>,
    pub weight: f64,
}

impl Default for LandKind {
    fn default() -> Self {
        LandKind { cover: Cover::Any, from_m: None, below_m: None, from_slope: None, below_slope: None, weight: 1.0 }
    }
}

impl LandKind {
    pub fn matches(&self, class: u8, height: f32, slope: f32) -> bool {
        let (h, s) = (height as f64, slope as f64);
        self.cover.matches(class)
            && self.from_m.is_none_or(|v| h >= v)
            && self.below_m.is_none_or(|v| h < v)
            && self.from_slope.is_none_or(|v| s >= v)
            && self.below_slope.is_none_or(|v| s < v)
    }

    /// Whether the limits leave nothing (a "from" not below its "below").
    pub fn empty(&self) -> bool {
        matches!((self.from_m, self.below_m), (Some(a), Some(b)) if a >= b) || matches!((self.from_slope, self.below_slope), (Some(a), Some(b)) if a >= b)
    }
}

/// A terrain for provinces whose kinds of land make up at least `min_share`
/// of them (0..1; the weighted pixels of all its kinds over the province's).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TerrainRule {
    pub terrain: String,
    pub min_share: f64,
    pub land: Vec<LandKind>,
}

impl Default for TerrainRule {
    fn default() -> Self {
        TerrainRule { terrain: "hills".into(), min_share: 0.5, land: vec![LandKind::default()] }
    }
}

/// The rules, checked from the top; `other` for land no rule takes. A
/// terrain set on a province wins, and provinces at the map's edge are
/// mountain.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TerrainTypes {
    pub rules: Vec<TerrainRule>,
    pub other: String,
}

impl Default for TerrainTypes {
    /// What the build did before the rules could be edited: urban from 20 %
    /// built-up land, mountain where steep land (25° or more) and half of the
    /// high land (2200 m or more) make up 55 %, plains from 35 % gentle land
    /// (under 10°), forest from 55 % forest, else hills.
    fn default() -> Self {
        TerrainTypes {
            rules: vec![
                TerrainRule { terrain: "urban".into(), min_share: 0.2, land: vec![LandKind { cover: Cover::BuiltUp, ..Default::default() }] },
                TerrainRule {
                    terrain: "mountain".into(),
                    min_share: 0.55,
                    land: vec![LandKind { from_slope: Some(25.0), ..Default::default() }, LandKind { from_m: Some(2200.0), weight: 0.5, ..Default::default() }],
                },
                TerrainRule { terrain: "plains".into(), min_share: 0.35, land: vec![LandKind { below_slope: Some(10.0), ..Default::default() }] },
                TerrainRule { terrain: "forest".into(), min_share: 0.55, land: vec![LandKind { cover: Cover::Forest, ..Default::default() }] },
            ],
            other: "hills".into(),
        }
    }
}

impl TerrainTypes {
    /// The kinds of land of all rules, in order: what [`Self::count`] counts.
    pub fn kinds(&self) -> Vec<LandKind> {
        self.rules.iter().flat_map(|r| r.land.iter().cloned()).collect()
    }

    /// Pixels of each kind of land ([`Self::kinds`]) per province:
    /// `out[p * k + j]` for province `p` and kind `j` (`k` kinds), from the
    /// province of each pixel and its land cover class, height and slope.
    pub fn count(kinds: &[LandKind], provinces: &[u32], cover: &[u8], height: &[f32], slope: &[f32], cap: usize) -> Vec<u64> {
        let k = kinds.len();
        if k == 0 {
            return Vec::new();
        }
        let n = provinces.len().min(cover.len()).min(height.len()).min(slope.len());
        (0..n)
            .into_par_iter()
            .fold(
                || vec![0u64; cap * k],
                |mut acc, i| {
                    let p = provinces[i] as usize;
                    if p < cap {
                        for (j, kind) in kinds.iter().enumerate() {
                            if kind.matches(cover[i], height[i], slope[i]) {
                                acc[p * k + j] += 1;
                            }
                        }
                    }
                    acc
                },
            )
            .reduce(
                || vec![0u64; cap * k],
                |mut a, b| {
                    for (x, y) in a.iter_mut().zip(&b) {
                        *x += y;
                    }
                    a
                },
            )
    }

    /// The weighted share of each rule's land in a province of `size`
    /// pixels, from its counts of each kind (as [`Self::count`] gives them).
    pub fn shares(&self, counts: &[u64], size: u64) -> Vec<f64> {
        let mut j = 0;
        self.rules
            .iter()
            .map(|r| {
                let mut s = 0.0;
                for kind in &r.land {
                    s += kind.weight * (counts.get(j).copied().unwrap_or(0) as f64 / size.max(1) as f64);
                    j += 1;
                }
                s
            })
            .collect()
    }

    /// The rule a province takes (None: `other`).
    pub fn rule_of(&self, counts: &[u64], size: u64) -> Option<usize> {
        self.shares(counts, size).iter().zip(&self.rules).position(|(&s, r)| s >= r.min_share)
    }

    pub fn terrain_of(&self, counts: &[u64], size: u64) -> &str {
        self.rule_of(counts, size).map_or(self.other.as_str(), |i| self.rules[i].terrain.as_str())
    }

    /// Problems that stop a build.
    pub fn check(&self) -> Result<()> {
        for (i, r) in self.rules.iter().enumerate() {
            if !LAND_TERRAIN.contains(&r.terrain.as_str()) {
                bail!("terrain rule {}: {:?} is not a land terrain ({})", i + 1, r.terrain, LAND_TERRAIN.join(", "));
            }
        }
        if !LAND_TERRAIN.contains(&self.other.as_str()) {
            bail!("terrain of other land: {:?} is not a land terrain ({})", self.other, LAND_TERRAIN.join(", "));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The old fixed rule, as the build had it.
    fn old(sealed: f64, steep: f64, high: f64, gentle: f64, forest: f64) -> &'static str {
        if sealed >= 0.2 {
            "urban"
        } else if steep + 0.5 * high >= 0.55 {
            "mountain"
        } else if gentle >= 0.35 {
            "plains"
        } else if forest >= 0.55 {
            "forest"
        } else {
            "hills"
        }
    }

    #[test]
    fn defaults_are_the_old_rule() {
        let t = TerrainTypes::default();
        t.check().unwrap();
        assert_eq!(t.kinds().len(), 5);
        // provinces of random land, also right at the thresholds (sizes of 20 px)
        let mut seed = 7u64;
        let mut rnd = |m: u64| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 33) % m
        };
        for _ in 0..20_000 {
            let size = [20, 100, 7, 1 + rnd(400)][rnd(4) as usize];
            let c: Vec<u64> = (0..5).map(|_| rnd(size + 1)).collect();
            // kinds: built-up, steep, high, gentle, forest
            let sh = |x: u64| x as f64 / size as f64;
            assert_eq!(t.terrain_of(&c, size), old(sh(c[0]), sh(c[1]), sh(c[2]), sh(c[3]), sh(c[4])), "{c:?} of {size}");
        }
        // the pixel tests of the old rule
        let k = t.kinds();
        assert!(k[0].matches(1, 0.0, 0.0) && !k[0].matches(2, 0.0, 0.0));
        assert!(k[1].matches(6, 0.0, 25.0) && !k[1].matches(6, 0.0, 24.99));
        assert!(k[2].matches(6, 2200.0, 0.0) && !k[2].matches(6, 2199.9, 0.0));
        assert!(k[3].matches(6, 0.0, 9.99) && !k[3].matches(6, 0.0, 10.0));
        assert!(k[4].matches(3, 0.0, 0.0) && !k[4].matches(5, 0.0, 0.0));
    }

    #[test]
    fn counting_and_stored_rules() {
        let t = TerrainTypes::default();
        let kinds = t.kinds();
        // province 1: two steep built-up pixels; province 2: one gentle forest pixel
        let c = TerrainTypes::count(&kinds, &[1, 1, 2], &[1, 1, 3], &[500.0, 500.0, 900.0], &[30.0, 30.0, 2.0], 3);
        assert_eq!(&c[5..10], &[2, 2, 0, 0, 0]);
        assert_eq!(&c[10..15], &[0, 0, 0, 1, 1]);
        assert_eq!(t.terrain_of(&c[5..10], 2), "urban");
        assert_eq!(t.terrain_of(&c[10..15], 1), "plains");
        // stored and read back; settings without rules get the defaults
        let v: TerrainTypes = serde_json::from_str(&serde_json::to_string(&t).unwrap()).unwrap();
        assert_eq!(v, t);
        assert_eq!(serde_json::from_str::<TerrainTypes>("{}").unwrap(), t);
        let bad = TerrainTypes { other: "lava".into(), ..t };
        assert!(bad.check().is_err());
    }
}
