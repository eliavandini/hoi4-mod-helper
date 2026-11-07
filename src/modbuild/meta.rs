//! Mod metadata (name, version, author, tags, thumbnail, ...) and build
//! settings, kept in provinces.json under "mod"; descriptor.mod from them.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};

/// Tags the Paradox launcher / Steam Workshop offer for HOI4 mods.
pub const KNOWN_TAGS: [&str; 15] = [
    "Alternative History",
    "Balance",
    "Events",
    "Fixes",
    "Gameplay",
    "Graphics",
    "Historical",
    "Ideologies",
    "Map",
    "Military",
    "National Focuses",
    "Sound",
    "Technologies",
    "Translation",
    "Utilities",
];

/// Folders the mod replaces instead of adding to.
pub const REPLACE_PATHS: [&str; 6] = ["history/states", "history/countries", "history/units", "map/strategicregions", "map/supplyareas", "common/bookmarks"];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModMeta {
    /// Name in the launcher.
    pub name: String,
    /// Folder (and .mod file) name in the game's mod folder.
    pub folder: String,
    pub version: String,
    pub author: String,
    /// Description for the README (and to paste into the Workshop page).
    pub description: String,
    pub tags: Vec<String>,
    /// Game version the mod is made for ("1.19.*"); empty: the installed game's.
    pub supported_version: String,
    /// Thumbnail (PNG or JPEG), relative to the map folder or absolute.
    pub picture: String,
    /// Steam Workshop id, once uploaded.
    pub remote_file_id: String,
    /// Names of mods this one loads after when they are enabled (load order).
    pub dependencies: Vec<String>,
    // build settings
    /// Default country of the bookmark: a tag or part of a settlement name;
    /// empty: the first city.
    pub player: String,
    /// Population of a nation's capital state besides its towns (a quarter in other states).
    pub manpower_base: f64,
    /// Population per km² of sealed land.
    pub manpower_urban: f64,
    /// Slope exaggeration of the normal map.
    pub relief: f64,
    /// Seed of the random choices (colours, flags, building positions).
    pub seed: u64,
    /// Terrain types of land provinces without one set.
    pub terrain: super::terrain::TerrainTypes,
}

impl Default for ModMeta {
    fn default() -> Self {
        ModMeta {
            name: "Südtirol map test".into(),
            folder: "suedtirol_test".into(),
            version: "0.1".into(),
            author: String::new(),
            description: String::new(),
            tags: vec!["Map".into()],
            supported_version: String::new(),
            picture: String::new(),
            remote_file_id: String::new(),
            dependencies: Vec::new(),
            player: String::new(),
            manpower_base: 100_000.0,
            manpower_urban: 50_000.0,
            relief: 0.5,
            seed: 1,
            terrain: Default::default(),
        }
    }
}

/// Text in a descriptor: no quotes or line breaks.
fn clean(s: &str) -> String {
    s.replace('"', "'").replace(['\n', '\r'], " ").trim().to_string()
}

impl ModMeta {
    /// Problems that stop a build.
    pub fn check(&self) -> Result<()> {
        if self.name.trim().is_empty() {
            bail!("the mod needs a name");
        }
        if self.folder.is_empty() || !self.folder.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
            bail!("mod folder {:?}: use letters, digits, _ and - only", self.folder);
        }
        if !self.remote_file_id.is_empty() && !self.remote_file_id.chars().all(|c| c.is_ascii_digit()) {
            bail!("Steam Workshop id {:?}: digits only", self.remote_file_id);
        }
        self.terrain.check()
    }

    pub fn picture_path(&self, map_dir: &Path) -> Option<PathBuf> {
        let p = self.picture.trim();
        (!p.is_empty()).then(|| map_dir.join(p))
    }

    /// descriptor.mod (without `path`).
    pub fn descriptor(&self, game_version: &str, picture: bool) -> String {
        let mut d = format!("version=\"{}\"\nname=\"{}\"\n", clean(&self.version), clean(&self.name));
        let tags: Vec<String> = self.tags.iter().map(|t| clean(t)).filter(|t| !t.is_empty()).collect();
        if !tags.is_empty() {
            d += "tags={\n";
            for t in tags {
                d += &format!("\t\"{t}\"\n");
            }
            d += "}\n";
        }
        if picture {
            d += "picture=\"thumbnail.png\"\n";
        }
        let v = if self.supported_version.trim().is_empty() { game_version.to_string() } else { clean(&self.supported_version) };
        d += &format!("supported_version=\"{v}\"\n");
        if !self.remote_file_id.is_empty() {
            d += &format!("remote_file_id=\"{}\"\n", self.remote_file_id);
        }
        let deps: Vec<String> = self.dependencies.iter().map(|t| clean(t)).filter(|t| !t.is_empty()).collect();
        if !deps.is_empty() {
            d += "dependencies={\n";
            for t in deps {
                d += &format!("\t\"{t}\"\n");
            }
            d += "}\n";
        }
        for p in REPLACE_PATHS {
            d += &format!("replace_path=\"{p}\"\n");
        }
        d
    }

    /// README.txt for the mod folder (HOI4's descriptor has no author or
    /// description); None if there is nothing to say.
    pub fn readme(&self) -> Option<String> {
        if self.author.trim().is_empty() && self.description.trim().is_empty() {
            return None;
        }
        let mut t = format!("{}\nversion {}\n", self.name.trim(), self.version.trim());
        if !self.author.trim().is_empty() {
            t += &format!("by {}\n", self.author.trim());
        }
        if !self.description.trim().is_empty() {
            t += &format!("\n{}\n", self.description.trim());
        }
        Some(t)
    }
}

/// The thumbnail as PNG (Steam wants it under 1 MB); a warning if it is bigger.
pub fn write_thumbnail(src: &Path, dst: &Path) -> Result<Option<String>> {
    let img = image::open(src).with_context(|| format!("cannot read the thumbnail {}", src.display()))?;
    // the launcher shows it small; very large pictures are scaled down
    let img = if img.width() > 1024 || img.height() > 1024 { img.resize(1024, 1024, image::imageops::FilterType::Lanczos3) } else { img };
    img.to_rgba8().save_with_format(dst, image::ImageFormat::Png).with_context(|| format!("cannot write {}", dst.display()))?;
    let size = std::fs::metadata(dst).map(|m| m.len()).unwrap_or(0);
    Ok((size > 1_000_000).then(|| format!("thumbnail.png is {:.1} MB; the Steam Workshop takes at most 1 MB", size as f64 / 1e6)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_fields() {
        let mut m = ModMeta { tags: vec!["Map".into(), "Total \"Conversion\"".into()], remote_file_id: "123".into(), ..Default::default() };
        let d = m.descriptor("1.19.*", true);
        assert!(d.starts_with("version=\"0.1\"\nname=\"Südtirol map test\"\ntags={\n\t\"Map\"\n\t\"Total 'Conversion'\"\n}\n"));
        assert!(d.contains("picture=\"thumbnail.png\"\nsupported_version=\"1.19.*\"\nremote_file_id=\"123\"\n"));
        assert!(d.ends_with("replace_path=\"common/bookmarks\"\n"));
        m.supported_version = "1.16.*".into();
        assert!(m.descriptor("1.19.*", false).contains("supported_version=\"1.16.*\""));
        assert!(m.readme().is_none());
        m.author = "Someone".into();
        assert!(m.readme().unwrap().contains("by Someone"));
        m.folder = "bad name".into();
        assert!(m.check().is_err());
    }
}
