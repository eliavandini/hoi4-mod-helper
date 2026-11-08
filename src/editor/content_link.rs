//! What the nation-content screen ([`crate::content`]) needs of the map
//! editor: the nations with the tags the build gives them, pinning a tag,
//! saving, and the build window.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::model::Id;
use super::{Act, Editor};
use crate::modbuild::{self, countries};

/// A nation of the map, as the content screen shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct NationInfo {
    pub id: Id,
    pub name: String,
    /// city / town / village
    pub place: String,
    /// The tag the build gives it (its own, or made from the name).
    pub tag: String,
    /// The tag is fixed in the map (else the build makes it, and it can change
    /// when nations are added or renamed).
    pub pinned: bool,
    pub capital_state: Id,
    pub states: Vec<Id>,
    pub language: String,
}

/// The nations of the map and its states and provinces, at one edit.
#[derive(Clone, Debug, Default)]
pub struct MapFacts {
    pub nations: Vec<NationInfo>,
    pub states: BTreeSet<Id>,
    pub state_names: BTreeMap<Id, String>,
    pub provinces: u32,
    /// Why the tags could not be made (a bad tag of a nation).
    pub tag_error: Option<String>,
}

impl Editor {
    /// The folder of the open map.
    pub fn map_dir(&self) -> Option<&Path> {
        self.doc.as_ref().map(|d| d.dir.as_path())
    }

    /// The map is being read, or why it could not be.
    pub fn load_state(&self) -> (bool, Option<&str>) {
        (self.loading.is_some(), self.load_err.as_deref())
    }

    /// The edit counter of the open map: changes whenever the map does.
    pub fn map_generation(&self) -> Option<(PathBuf, u64)> {
        self.doc.as_ref().map(|d| (d.dir.clone(), d.meta_version))
    }

    /// Nations, states and provinces of the open map. `vanilla` are the game's
    /// tags (none known: the made tags may differ from the build's).
    pub fn map_facts(&self, vanilla: &BTreeSet<String>) -> Option<MapFacts> {
        let doc = self.doc.as_ref()?;
        let d = &doc.data;
        let nations: Vec<&super::model::Nation> = d.nations.iter().map(|(_, n)| n).collect();
        let (tags, tag_error) = match countries::nation_tags(&nations, vanilla) {
            Ok(t) => (t, None),
            Err(e) => (BTreeMap::new(), Some(format!("{e:#}"))),
        };
        let mut out = MapFacts { tag_error, provinces: d.provinces.cap().saturating_sub(1) as u32, ..Default::default() };
        for (id, s) in d.states.iter() {
            out.states.insert(id);
            out.state_names.insert(id, s.name.clone());
        }
        for (id, n) in d.nations.iter() {
            let own = n.tag.clone().filter(|t| !t.is_empty());
            out.nations.push(NationInfo {
                id,
                name: n.name.clone(),
                place: n.place.clone(),
                pinned: own.is_some(),
                tag: own.or_else(|| tags.get(&id).cloned()).unwrap_or_default(),
                capital_state: n.capital_state,
                states: doc.index.states_of_nation(id).to_vec(),
                language: n.language.clone(),
            });
        }
        Some(out)
    }

    /// Fix the tag of nation `id` in the map (one undo step of the map).
    pub fn pin_tag(&mut self, id: Id, tag: &str) -> Result<(), String> {
        if !countries::tag_ok(tag) {
            return Err(format!("{tag:?} is not a tag: three capitals or digits, starting with a letter"));
        }
        let Some(doc) = self.doc.as_ref() else { return Err("no map is open".into()) };
        if let Some((_, other)) = doc.data.nations.iter().find(|(i, n)| *i != id && n.tag.as_deref() == Some(tag)) {
            return Err(format!("{} has the tag {tag} already", other.name));
        }
        // the properties panel may hold a copy of the nation: apply it first
        self.flush_pending();
        let tag = tag.to_string();
        let mut err = None;
        self.run("fix the tag", |d, _| {
            let n = d.data.nations.get_mut(id).ok_or_else(|| anyhow::anyhow!("the nation is gone"))?;
            n.tag = Some(tag.clone());
            Ok(format!("{} has the tag {tag} now (save the map to keep it)", n.name))
        });
        if let Some((m, true, _)) = &self.msg {
            err = Some(m.clone());
        }
        // the panel shows the nation again as it is now
        self.draft = None;
        self.draft_orig = None;
        err.map_or(Ok(()), Err)
    }

    /// Save the map (with what is pending in the panels) and build the mod;
    /// the build window shows the log and the launcher.
    pub fn build_and_show(&mut self, ctx: &eframe::egui::Context) {
        self.apply(Act::Build, ctx);
    }

    /// Show the build window (its last build, and the launcher).
    pub fn show_build_window(&mut self) {
        self.show_build = true;
    }

    /// The build is running.
    pub fn building(&self) -> bool {
        self.build.as_ref().is_some_and(|b| b.state() == super::build::RUNNING)
    }

    /// Where the HOI4 install is and where mods are built.
    pub fn game_dirs(&self) -> modbuild::Dirs {
        self.dirs()
    }
}
