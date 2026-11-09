//! Edits in the side panel that wait to go into the map: the properties of
//! the selected entity, a river's name, the mod details. Each goes in as one
//! undo step once it is done (a text field left, a drag let go), and before
//! anything that would lose it: another selection or level, an action, undo,
//! save, build, quit.

use std::collections::BTreeSet;

use anyhow::{Result, anyhow};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use super::doc::Doc;
use super::model::Adjacency;
use super::{Draft, Editor};

/// Put what was changed in a copy (`new` against `orig`) onto `cur`. Fields
/// left alone keep what other edits made of them meanwhile, and maps
/// (buildings, resources) merge key by key. Returns the changed fields.
pub(super) fn merge_changes<T: Serialize + DeserializeOwned>(cur: &mut T, orig: &T, new: &T) -> Result<Vec<String>> {
    let obj = |v: &T| match serde_json::to_value(v)? {
        Value::Object(m) => Ok(m),
        _ => Err(anyhow!("not an object")),
    };
    let (o, n, mut c) = (obj(orig)?, obj(new)?, obj(cur)?);
    let changed = merge(&mut c, &o, &n);
    if !changed.is_empty() {
        *cur = serde_json::from_value(Value::Object(c))?;
    }
    Ok(changed)
}

fn merge(c: &mut Map<String, Value>, o: &Map<String, Value>, n: &Map<String, Value>) -> Vec<String> {
    let keys: BTreeSet<&String> = o.keys().chain(n.keys()).collect();
    let mut out = Vec::new();
    for k in keys {
        let (a, b) = (o.get(k), n.get(k));
        if a == b {
            continue;
        }
        out.push(k.clone());
        if let (Some(Value::Object(a)), Some(Value::Object(b)), Some(Value::Object(cc))) = (a, b, c.get_mut(k)) {
            merge(cc, a, b);
            continue;
        }
        match b {
            Some(b) => {
                c.insert(k.clone(), b.clone());
            }
            None => {
                c.remove(k);
            }
        }
    }
    out
}

/// A field as the properties panel calls it.
fn field_name(k: &str) -> &str {
    match k {
        "nation" => "owner",
        "capital_state" => "capital",
        "no_units" => "army",
        "place" => "settlement",
        "manpower" => "population",
        "victory_points" => "victory points",
        "color" => "colour",
        "through" => "sea crossed",
        "start" | "stop" => "ends",
        k => k,
    }
}

/// The edits of a properties draft (`new` against `orig`) onto the map.
pub(super) fn apply_draft(d: &mut Doc, orig: &Draft, new: &Draft) -> Result<String> {
    let data = &mut d.data;
    let (what, fields) = match (orig, new) {
        (Draft::Province(id, o), Draft::Province(i, n)) if id == i => {
            let cur = data.provinces.get_mut(*id).ok_or_else(|| anyhow!("the province is gone"))?;
            // not in the file: memberships stay as they are
            let keep = (cur.kind, cur.state, cur.region);
            let f = merge_changes(cur, o, n)?;
            (cur.kind, cur.state, cur.region) = keep;
            (format!("province {}", data.prov_name(*id)), f)
        }
        (Draft::State(id, o), Draft::State(i, n)) if id == i => {
            let cur = data.states.get_mut(*id).ok_or_else(|| anyhow!("the state is gone"))?;
            let f = merge_changes(cur, o, n)?;
            (format!("state {}", data.state_name(*id)), f)
        }
        (Draft::Nation(id, o), Draft::Nation(i, n)) if id == i => {
            let cur = data.nations.get_mut(*id).ok_or_else(|| anyhow!("the nation is gone"))?;
            let f = merge_changes(cur, o, n)?;
            (format!("nation {}", data.nation_name(*id)), f)
        }
        (Draft::Region(id, o), Draft::Region(i, n)) if id == i => {
            let cur = data.regions.get_mut(*id).ok_or_else(|| anyhow!("the zone is gone"))?;
            let f = merge_changes(cur, o, n)?;
            (format!("zone {}", data.region_name(*id)), f)
        }
        (Draft::Adjacency(i, o), Draft::Adjacency(j, n)) if i == j => {
            // adjacencies have no id: find the one the copy was made of
            let list = &mut data.adjacencies;
            let same_ends = |a: &Adjacency| (a.from, a.to) == (o.from, o.to) || (a.from, a.to) == (o.to, o.from);
            let k = if list.get(*i) == Some(o) {
                *i
            } else if let Some(k) = list.iter().position(|a| a == o) {
                k
            } else if list.get(*i).is_some_and(same_ends) {
                *i
            } else {
                anyhow::bail!("the adjacency was changed or removed meanwhile");
            };
            let f = merge_changes(&mut list[k], o, n)?;
            ("adjacency".to_string(), f)
        }
        _ => anyhow::bail!("the panel shows something else now"),
    };
    let fields: Vec<&str> = fields.iter().map(|k| field_name(k)).collect::<BTreeSet<_>>().into_iter().collect();
    Ok(format!("{what}: {}", fields.join(", ")))
}

impl Editor {
    /// The properties panel holds edits that are not in the map yet.
    pub(super) fn draft_pending(&self) -> bool {
        matches!((&self.draft, &self.draft_orig), (Some(a), Some(b)) if a != b)
    }

    /// Edits in the panels or the Mod details window that are not in the
    /// map yet (they count as unsaved).
    pub(super) fn edits_pending(&self) -> bool {
        self.draft_pending() || self.river_name_pending() || self.object_edits_pending() || self.meta.as_ref().is_some_and(|e| e.m != e.orig)
    }

    /// Put the properties panel's edits into the map (one undo step).
    pub(super) fn flush_draft(&mut self) {
        if !self.draft_pending() {
            return;
        }
        let (Some(new), Some(orig)) = (self.draft.clone(), self.draft_orig.clone()) else { return };
        // done with this copy, even if the map no longer has what it was made of
        self.draft_orig = Some(new.clone());
        let label = match &new {
            Draft::Province(..) => "Edit province",
            Draft::State(..) => "Edit state",
            Draft::Nation(..) => "Edit nation",
            Draft::Region(..) => "Edit zone",
            Draft::Adjacency(..) => "Edit adjacency",
        };
        self.run(label, move |d, _| apply_draft(d, &orig, &new));
    }

    /// Put the panels' edits into the map, unless a drag on the map is under
    /// way (then they follow once it ends).
    pub(super) fn flush_pending(&mut self) {
        if self.doc.as_ref().is_none_or(|d| d.in_txn()) {
            return;
        }
        self.flush_draft();
        self.flush_river_name();
        self.flush_object_name();
    }

    /// Mod details changed in their window but not applied: apply them if
    /// they are valid. False if they are not.
    pub(super) fn flush_meta(&mut self) -> bool {
        let Some(m) = self.meta.as_ref().filter(|e| e.m != e.orig).map(|e| e.m.clone()) else { return true };
        if let Err(e) = m.check() {
            self.error(format!("Mod details are not applied: {e:#}"));
            return false;
        }
        self.set_mod_meta(m);
        true
    }

    /// Everything pending, before saving or building: an open drag, the
    /// panels' edits, the Mod details. False if the Mod details are invalid.
    pub(super) fn flush_all(&mut self) -> bool {
        if let Some(d) = self.doc.as_mut()
            && d.in_txn()
        {
            d.commit();
        }
        self.flush_pending();
        self.flush_meta()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use serde_json::json;

    use super::super::model::{self, Labels, Loaded, MapData, River, RiverPoint, State};
    use super::super::{Act, Editor, Level, MetaEdit, Settings};
    use super::*;

    fn out_dir(name: &str) -> PathBuf {
        let base = std::env::var_os("MOD_HELPER_TEST_OUT").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let d = base.join(format!("mod-helper-test-{name}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn from<T: DeserializeOwned>(v: Value) -> T {
        serde_json::from_value(v).unwrap()
    }

    /// Three land provinces in a row: state A (1, 2) of nation N1 and state
    /// B (3) of nation N2, all in one air zone.
    fn tiny(dir: PathBuf) -> Doc {
        let mut data = MapData { pixel_m: 100.0, ..Default::default() };
        for id in 1..=3u32 {
            data.provinces.put(id, from(json!({"id": id, "rgb": [40 * id, 0, 0], "type": "land", "name": format!("p{id}")})));
        }
        data.states.put(1, from(json!({"id": 1, "name": "A", "nation": 1, "buildings": {"infrastructure": 2}})));
        data.states.put(2, from(json!({"id": 2, "name": "B", "nation": 2})));
        data.nations.put(1, from(json!({"id": 1, "name": "N1", "capital_state": 1, "language": "de", "place": "town"})));
        data.nations.put(2, from(json!({"id": 2, "name": "N2", "capital_state": 2, "language": "it", "place": "city"})));
        data.regions.put(1, from(json!({"id": 1, "name": "Air"})));
        for (p, s) in [(1, 1), (2, 1), (3, 2)] {
            let pr = data.provinces.get_mut(p).unwrap();
            (pr.state, pr.region) = (s, 1);
        }
        data.rivers.push(River { name: String::new(), points: vec![RiverPoint(0.0, 0.0, 1), RiverPoint(2.0, 0.0, 1)] });
        let geo = crate::geo::FileGeoInfo { transform: None, crs: None, nodata: None };
        Doc::new(dir, Loaded { data, labels: Labels { w: 3, h: 1, px: vec![1, 2, 3] }, geo, extra: Default::default() })
    }

    fn state_draft(ed: &mut Editor) -> &mut State {
        match ed.draft.as_mut() {
            Some(Draft::State(_, s)) => s,
            _ => panic!("no state in the panel"),
        }
    }

    #[test]
    fn only_the_changed_fields_go_in() {
        let orig: State = from(json!({"id": 1, "name": "A", "nation": 1, "manpower": 1000, "buildings": {"infrastructure": 2}}));
        let mut new = orig.clone();
        new.name = "Bozen".into();
        new.manpower = None; // back to auto
        new.buildings.insert("air_base".into(), 1);
        // meanwhile another edit gave the state away and raised its infrastructure
        let mut cur = orig.clone();
        cur.nation = Some(2);
        cur.buildings.insert("infrastructure".into(), 4);
        let f = merge_changes(&mut cur, &orig, &new).unwrap();
        assert_eq!(f, vec!["buildings", "manpower", "name"]);
        assert_eq!(cur.name, "Bozen");
        assert_eq!(cur.nation, Some(2), "the other edit was undone");
        assert_eq!(cur.manpower, None);
        assert_eq!(cur.buildings, BTreeMap::from([("air_base".to_string(), 1), ("infrastructure".to_string(), 4)]));
        assert!(merge_changes(&mut cur, &orig, &orig).unwrap().is_empty());
    }

    /// The panel's edits survive selecting something else, changing level,
    /// undo and saving, as the user makes them (on a copy, as the panel does).
    #[test]
    fn panel_edits_are_kept_and_saved() {
        let dir = out_dir("pending-edits");
        let mut ed = Editor::new(Settings::default());
        ed.doc = Some(tiny(dir.clone()));
        let ctx = eframe::egui::Context::default();
        let data = |ed: &Editor| ed.doc.as_ref().unwrap().data.clone();

        // rename state 1 and give it to N2, then click state 2
        ed.settings.level = Level::States;
        ed.sel = vec![1];
        ed.sync_draft();
        let s = state_draft(&mut ed);
        s.name = "Bozen".into();
        s.nation = Some(2);
        s.buildings.insert("air_base".into(), 1);
        assert!(ed.is_dirty(), "a pending edit counts as unsaved");
        ed.sel = vec![2];
        ed.sync_draft();
        let d = data(&ed);
        let a = d.states.get(1).unwrap();
        assert_eq!((a.name.as_str(), a.nation), ("Bozen", Some(2)));
        assert_eq!(a.buildings.get("infrastructure"), Some(&2));
        assert_eq!(a.buildings.get("air_base"), Some(&1));
        assert!(!d.nations.contains(1), "N1 has no state left");
        assert_eq!(state_draft(&mut ed).name, "B", "the panel shows state 2 now");

        // a name typed and then undone: the state keeps its old name
        state_draft(&mut ed).name = "typo".into();
        ed.apply(Act::Undo, &ctx);
        assert_eq!(data(&ed).states.get(2).unwrap().name, "B");
        ed.sync_draft();
        assert_eq!(state_draft(&mut ed).name, "B");

        // a province's name and terrain; another level then takes them in
        ed.settings.level = Level::Provinces;
        ed.sel = vec![3];
        ed.sync_draft();
        if let Some(Draft::Province(_, p)) = ed.draft.as_mut() {
            p.name = "Brixen".into();
            p.terrain = Some("hills".into());
        }
        ed.set_level(Level::States);
        let p = data(&ed).provinces.get(3).unwrap().clone();
        assert_eq!((p.name.as_str(), p.terrain.as_deref(), p.state, p.region), ("Brixen", Some("hills"), 2, 1));

        // pending when saving: the state's name, a river's name, the mod details
        ed.sel = vec![2];
        ed.sync_draft();
        state_draft(&mut ed).name = "Brixen - Bressanone".into();
        ed.rv.name_edit = Some((0, "Eisack".into(), String::new()));
        let mut m = data(&ed).mod_meta();
        let mut e = MetaEdit::new(m.clone());
        m.name = "Tirol test".into();
        e.m = m;
        ed.meta = Some(e);
        assert!(ed.is_dirty());
        ed.save();
        assert!(!ed.is_dirty(), "something is still pending");

        let l = model::load(&dir, &crate::util::Progress::default()).unwrap();
        let names: Vec<String> = l.data.states.iter().map(|(_, s)| s.name.clone()).collect();
        assert_eq!(names, ["Bozen", "Brixen - Bressanone"]);
        let owners: Vec<Option<u32>> = l.data.states.iter().map(|(_, s)| s.nation).collect();
        assert_eq!(owners, [Some(1), Some(1)], "both states belong to N2 (renumbered to 1)");
        assert_eq!(l.data.nations.get(1).unwrap().name, "N2");
        assert_eq!(l.data.provinces.get(3).unwrap().name, "Brixen");
        assert_eq!(l.data.rivers[0].name, "Eisack");
        assert_eq!(l.data.mod_meta().name, "Tirol test");
    }

    /// Typing in the panel for real (egui events): the name goes in when the
    /// field is left with Enter, not while typing.
    #[test]
    fn typed_name_goes_in_on_enter() {
        use eframe::egui::{self, Event, Key, Modifiers, RawInput};
        let mut ed = Editor::new(Settings::default());
        ed.doc = Some(tiny(out_dir("pending-typing")));
        ed.settings.level = Level::States;
        ed.sel = vec![1];
        let ctx = egui::Context::default();
        let key = |k| Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE };
        let frame = |ed: &mut Editor, events: Vec<Event>| {
            let input = RawInput { events, screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 1200.0))), ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| {
                let mut acts = Vec::new();
                ed.properties_ui(ui, &mut acts);
                for a in acts {
                    ed.apply(a, ui.ctx());
                }
            });
            out.textures_delta.clear();
        };
        let name = |ed: &Editor| ed.doc.as_ref().unwrap().data.states.get(1).unwrap().name.clone();
        frame(&mut ed, vec![]);
        // Tab to the first text field: the name (after the panel's header)
        for _ in 0..5 {
            if ctx.text_edit_focused() {
                break;
            }
            frame(&mut ed, vec![key(Key::Tab)]);
            frame(&mut ed, vec![]);
        }
        assert!(ctx.text_edit_focused(), "the name field did not get the focus");
        frame(&mut ed, vec![Event::Text("Bozen".into())]);
        frame(&mut ed, vec![]);
        assert_eq!(name(&ed), "A", "applied while typing");
        assert!(ed.is_dirty());
        frame(&mut ed, vec![key(Key::Enter)]);
        frame(&mut ed, vec![]);
        assert!(!ctx.text_edit_focused());
        assert!(name(&ed).ends_with("Bozen"), "not applied after Enter: {:?}", name(&ed));
        assert_eq!(ed.doc.as_ref().unwrap().can_undo(), Some("Edit state"));
    }

    #[test]
    fn reload_asks_before_dropping_changes() {
        let dir = out_dir("pending-reload");
        let mut ed = Editor::new(Settings::default());
        ed.doc = Some(tiny(dir));
        let ctx = eframe::egui::Context::default();
        ed.settings.level = Level::States;
        ed.sel = vec![1];
        ed.sync_draft();
        state_draft(&mut ed).name = "Bozen".into();
        ed.apply(Act::Reload, &ctx);
        assert!(matches!(ed.ask_discard, Some(Act::Reload)), "Reload did not ask");
        let d = &ed.doc.as_ref().unwrap().data;
        assert_eq!(d.states.get(1).unwrap().name, "Bozen", "the edit is in the map while the question is open");
    }
}
