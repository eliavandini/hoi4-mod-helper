//! New projects: one project file ([`crate::bundle`]) with a map blank (one
//! province of land or sea, of the size asked for) or imported from the base
//! game or a mod ([`hoi4`]), on a plain raster or a map projection
//! ([`crate::mapproj`]).

pub mod hoi4;

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use serde_json::Map;

use crate::editor::model::{self, Kind, Labels, MapData, Nation, Province, Region, State};
use crate::mapproj::{self, Projection};
use crate::modbuild::{ModMeta, bitmaps};
use crate::bundle::{self, Project};
use crate::project::ProjectFile;
use crate::util::Progress;

/// What a blank map is made of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fill {
    /// One land province, in a state of a nation, in a strategic region.
    Land,
    /// One sea province in a naval region.
    Sea,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Start {
    Blank { w: u32, h: u32, fill: Fill },
    Import(hoi4::Source),
}

/// A new project.
#[derive(Clone, Debug)]
pub struct Request {
    pub name: String,
    /// The project file (its folder is made if missing).
    pub file: PathBuf,
    pub start: Start,
    pub proj: Projection,
    /// The game's folder for the editor (None: the default).
    pub game_dir: Option<PathBuf>,
}

pub struct Created {
    pub project: PathBuf,
    /// What the import brought over and left out.
    pub report: Vec<String>,
}

/// A name for files and the mod folder: letters, digits and _ only.
pub fn slug(name: &str) -> String {
    let ascii = crate::modbuild::countries::ascii_name(name).to_lowercase();
    let mut s = String::new();
    for c in ascii.chars() {
        if c.is_ascii_alphanumeric() {
            s.push(c);
        } else if !s.ends_with('_') {
            s.push('_');
        }
    }
    let s = s.trim_matches('_').to_string();
    if s.is_empty() { "my_mod".into() } else { s }
}

/// Where new projects go by default: `~/Documents/mod-helper/<name>.hoi4proj`
/// (or `~/mod-helper/…` without a Documents folder).
pub fn default_file(name: &str) -> PathBuf {
    let home = std::env::home_dir().unwrap_or_default();
    let docs = home.join("Documents");
    let base = if docs.is_dir() { docs.join("mod-helper") } else { home.join("mod-helper") };
    base.join(format!("{}.{}", slug(name), bundle::EXT))
}

/// The mod's name and folder for a new project.
pub fn mod_meta(name: &str) -> ModMeta {
    ModMeta { name: name.trim().to_string(), folder: slug(name), version: "0.1".into(), ..Default::default() }
}

fn province(id: u32, kind: Kind, rgb: [u8; 3]) -> Province {
    Province { id, rgb, kind, edge: false, coastal: false, name: String::new(), terrain: None, water: false, km2: 0.0, nation: None, extra: Map::new(), state: 0, region: 0 }
}

/// A blank map in `dir` (which must not have one): one province of land
/// (in a state, a nation and a strategic region) or of sea.
pub fn create_blank(dir: &Path, w: u32, h: u32, fill: Fill, proj: &Projection, meta: &ModMeta) -> Result<()> {
    if let Err(e) = mapproj::check_size(w, h) {
        bail!(e);
    }
    if dir.join("provinces.json").exists() || dir.join("provinces.bmp").exists() {
        bail!("{} has a map already", dir.display());
    }
    std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let mut data = MapData::default();
    match fill {
        Fill::Land => {
            let mut p = province(1, Kind::Land, [86, 124, 27]);
            p.state = 1;
            p.region = 1;
            data.provinces.put(1, p);
            data.states.put(
                1,
                State {
                    id: 1,
                    name: "State 1".into(),
                    nation: Some(1),
                    impassable: false,
                    provinces: Vec::new(),
                    manpower: None,
                    category: None,
                    buildings: Default::default(),
                    resources: Default::default(),
                    victory_points: Vec::new(),
                    cores: None,
                    controller: None,
                    extra: Map::new(),
                },
            );
            data.nations.put(
                1,
                Nation {
                    id: 1,
                    name: "Nation 1".into(),
                    place: "city".into(),
                    row: h as f64 / 2.0,
                    col: w as f64 / 2.0,
                    capital_state: 1,
                    states: Vec::new(),
                    language: String::new(),
                    km2: 0.0,
                    tag: None,
                    color: None,
                    ideology: None,
                    no_units: false,
                    extra: Map::new(),
                },
            );
            data.regions.put(1, Region { id: 1, name: "Region 1".into(), naval: false, provinces: Vec::new(), states: Vec::new(), extra: Map::new() });
        }
        Fill::Sea => {
            let mut p = province(1, Kind::Sea, [8, 31, 130]);
            p.region = 1;
            data.provinces.put(1, p);
            data.regions.put(1, Region { id: 1, name: "Sea 1".into(), naval: true, provinces: Vec::new(), states: Vec::new(), extra: Map::new() });
        }
    }
    data.set_mod_meta(meta);
    data.set_map_frame((w, h) == mapproj::VANILLA_SIZE);
    proj.set_header(&mut data.header, w, h);
    data.pixel_m = proj.pixel_m();
    let labels = Labels { w, h, px: vec![1; (w * h) as usize] };
    let stats = model::compute_stats(&labels, data.provinces.cap());
    model::save(dir, &data, &labels, &stats, &vec![bitmaps::AUTO; (w * h) as usize], None)?;
    proj.write_georef(&dir.join("provinces.bmp"), w, h)?;
    Ok(())
}

/// Make the project file: its map (blank or imported) and the settings that
/// open it in the map editor.
pub fn create(req: &Request, progress: &Progress) -> Result<(Project, Created)> {
    if req.name.trim().is_empty() {
        bail!("the project needs a name");
    }
    if req.file.exists() {
        bail!("{} exists already", req.file.display());
    }
    if let Start::Blank { w, h, .. } = req.start
        && let Err(e) = mapproj::check_size(w, h)
    {
        bail!(e);
    }
    let meta = mod_meta(&req.name);
    let editor = crate::editor::Settings { active: true, game_dir: req.game_dir.clone(), ..Default::default() };
    let pf = ProjectFile { editor: Some(editor), ..ProjectFile::new(req.proj.crs()) };
    let (p, report) = Project::create(&req.file, &pf, |dir| match &req.start {
        Start::Blank { w, h, fill } => {
            progress.set_stage("making the map");
            create_blank(dir, *w, *h, *fill, &req.proj, &meta)?;
            Ok(vec![format!("a blank map of {w} × {h} px ({})", req.proj.describe(*w, *h))])
        }
        Start::Import(src) => hoi4::import(src, dir, &meta, &req.proj, progress),
    })?;
    let created = Created { project: p.path.clone(), report };
    Ok((p, created))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geo::Crs;

    #[test]
    fn slugs() {
        assert_eq!(slug("Südtirol map!"), "sudtirol_map");
        assert_eq!(slug("  "), "my_mod");
        assert_eq!(slug("Kaiserreich--2"), "kaiserreich_2");
    }

    /// A blank project opens in the editor and its map loads.
    #[test]
    fn blank_projects() {
        let tmp = std::env::temp_dir().join(format!("mh-newproject-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        for (i, (fill, proj)) in [
            (Fill::Land, Projection::Plain { pixel_m: 2000.0 }),
            (Fill::Sea, Projection::Geo { crs: Crs::LAEA_EUROPE, center: [11.0, 46.5], pixel: 500.0 }),
        ]
        .into_iter()
        .enumerate()
        {
            let req = Request { name: format!("Test {i}"), file: tmp.join(format!("p{i}/test_{i}.hoi4proj")), start: Start::Blank { w: 512, h: 256, fill }, proj, game_dir: None };
            let (_, c) = create(&req, &Progress::default()).unwrap();
            assert!(c.project.is_file());
            let (p, pf) = Project::open(&c.project).unwrap();
            assert_eq!(pf.crs, proj.crs());
            let ed = pf.editor.unwrap();
            assert!(ed.active && ed.map_dir.as_deref() == Some(p.map_dir().as_path()));
            let l = model::load(&p.map_dir(), &Progress::default()).unwrap();
            assert_eq!((l.labels.w, l.labels.h, l.data.provinces.count()), (512, 256, 1));
            assert_eq!(l.data.provinces.get(1).unwrap().kind, if fill == Fill::Land { Kind::Land } else { Kind::Sea });
            assert_eq!(l.data.states.count(), (fill == Fill::Land) as usize);
            assert!((l.data.pixel_m - proj.pixel_m()).abs() < 1e-9);
            assert_eq!(l.data.mod_meta().folder, format!("test_{i}"));
            assert!(!l.data.map_frame());
            // the georeference reads back
            assert_eq!(Projection::of_map(&l.geo, 512, 256, l.data.pixel_m).crs(), proj.crs());
            // the terrain inputs stand in for the real data
            let t = crate::editor::density::load_terrain(&Default::default(), 512, 256, l.data.pixel_m, None, &Progress::default()).unwrap();
            assert_eq!(t.dem.len(), 512 * 256);
            // again in the same place: refused
            assert!(create(&req, &Progress::default()).is_err());
        }
        // sizes HOI4 does not take are refused
        let bad = Request { name: "Bad".into(), file: tmp.join("bad.hoi4proj"), start: Start::Blank { w: 500, h: 256, fill: Fill::Land }, proj: Projection::Plain { pixel_m: 1.0 }, game_dir: None };
        assert!(create(&bad, &Progress::default()).is_err());
        assert!(!tmp.join("bad.hoi4proj").exists());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
