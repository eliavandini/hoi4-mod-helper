//! The build's terrain rules in the editor: the rules editor (in "Mod
//! details"), the terrain each province gets with them (colour mode
//! "terrain type", the supply estimate) and how many provinces each rule
//! takes.

use std::sync::Arc;

use eframe::egui::{self, Color32, RichText};

use super::model::{Id, Kind};
use super::{Editor, WARN, limit, supply, version};
use crate::modbuild::terrain::{Cover, LAND_TERRAIN, LandKind, TerrainRule, TerrainTypes};

/// The terrain of every province for some rules.
pub struct TerrainResult {
    pub terrain: Arc<Vec<String>>,
    /// The rule that set it ([`supply::province_terrain`]).
    pub rule: Vec<Option<usize>>,
    /// Land provinces the rules decide, per rule, and last those no rule
    /// takes ("other land").
    pub by_rule: Vec<usize>,
    /// Land provinces with a terrain set on them.
    pub own: usize,
    /// Changes when `terrain` does.
    pub rev: u64,
    /// Pixels of each kind of land per province, and the number of kinds.
    counts: Arc<Vec<u64>>,
    kinds: usize,
}

type Counts = ((u64, Vec<LandKind>), Arc<Vec<u64>>);

#[derive(Default)]
pub struct TerrainCache {
    counts: Option<Counts>,
    pub result: Option<((u64, TerrainTypes), Arc<TerrainResult>)>,
}

/// Colour of a terrain in the "terrain type" colour mode.
pub fn terrain_colour(t: &str) -> [u8; 3] {
    match t {
        "plains" => [196, 210, 120],
        "forest" => [50, 112, 52],
        "hills" => [178, 142, 88],
        "mountain" => [128, 100, 96],
        "marsh" => [78, 140, 124],
        "urban" => [156, 156, 168],
        "jungle" => [30, 150, 70],
        "desert" => [232, 206, 140],
        "ocean" => [42, 72, 120],
        "lakes" => [92, 150, 160],
        _ => [90, 90, 90],
    }
}

fn swatch(ui: &mut egui::Ui, t: &str) {
    let c = terrain_colour(t);
    ui.label(RichText::new("■").color(Color32::from_rgb(c[0], c[1], c[2])));
}

impl Editor {
    /// The terrain rules in use: as edited in an open "Mod details" window,
    /// else as stored with the map.
    pub(super) fn terrain_types_now(&self) -> TerrainTypes {
        match &self.meta {
            Some(e) => e.m.terrain.clone(),
            None => self.doc.as_ref().map(|d| d.data.mod_meta().terrain).unwrap_or_default(),
        }
    }

    /// The terrain of every province with `types` (cached); None until the
    /// land cover is loaded.
    pub(super) fn terrain_result(&mut self, types: &TerrainTypes) -> Option<Arc<TerrainResult>> {
        let doc = self.doc.as_ref()?;
        let t = doc.terrain.clone()?;
        let key = (doc.pixel_version, types.kinds());
        let counts = match &self.tt.counts {
            Some((k, c)) if *k == key => c.clone(),
            _ => {
                let c = Arc::new(supply::land_counts(&doc.labels.read(), &t, &key.1, doc.data.provinces.cap()));
                self.tt.counts = Some((key, c.clone()));
                c
            }
        };
        let rk = (version(doc), types.clone());
        if let Some((k, r)) = &self.tt.result
            && *k == rk
            && Arc::ptr_eq(&r.counts, &counts)
        {
            return Some(r.clone());
        }
        let (terrain, rule) = supply::province_terrain(&doc.data, &doc.stats, types, &counts);
        let mut by_rule = vec![0; types.rules.len() + 1];
        let mut own = 0;
        for (p, pr) in doc.data.provinces.iter() {
            if pr.kind != Kind::Land || pr.edge {
                continue;
            }
            if pr.terrain.as_ref().is_some_and(|t| !t.is_empty()) {
                own += 1;
            } else {
                by_rule[rule[p as usize].unwrap_or(types.rules.len())] += 1;
            }
        }
        let rev = match &self.tt.result {
            Some((_, r)) if *r.terrain == terrain => r.rev,
            Some((_, r)) => r.rev + 1,
            None => 1,
        };
        let r = Arc::new(TerrainResult { terrain: Arc::new(terrain), rule, by_rule, own, rev, counts, kinds: types.kinds().len() });
        self.tt.result = Some((rk, r.clone()));
        Some(r)
    }

    /// The terrain of every province with the rules in use.
    pub(super) fn province_terrains(&mut self) -> Option<Arc<TerrainResult>> {
        let t = self.terrain_types_now();
        self.terrain_result(&t)
    }

    /// Hover rows of the "terrain type" colour mode: the terrain, and the
    /// share of each rule's land in the province.
    pub(super) fn terrain_hover_rows(&self, ui: &mut egui::Ui, p: Id) {
        let (Some(doc), Some((_, r))) = (&self.doc, &self.tt.result) else { return };
        let Some(t) = r.terrain.get(p as usize).filter(|t| !t.is_empty()) else { return };
        let types = self.terrain_types_now();
        ui.label("terrain type");
        let why = match (r.rule.get(p as usize).copied().flatten(), doc.data.provinces.get(p)) {
            (_, Some(pr)) if pr.kind != Kind::Land => String::new(),
            (_, Some(pr)) if pr.terrain.as_ref().is_some_and(|t| !t.is_empty()) => " (set on the province)".into(),
            (_, Some(pr)) if pr.edge => " (map edge)".into(),
            (Some(i), _) => format!(" (rule {})", i + 1),
            (None, _) => " (other land)".into(),
        };
        ui.label(format!("{t}{why}"));
        ui.end_row();
        let k = r.kinds;
        if k > 0 && doc.data.kind(p) == Kind::Land && (p as usize + 1) * k <= r.counts.len() {
            let shares = types.shares(&r.counts[p as usize * k..(p as usize + 1) * k], doc.stats.get(p).px as u64);
            let text: Vec<String> = types.rules.iter().zip(&shares).map(|(rule, s)| format!("{} {:.0}/{:.0} %", rule.terrain, s * 100.0, rule.min_share * 100.0)).collect();
            ui.label("");
            ui.label(RichText::new(text.join(" · ")).small());
            ui.end_row();
        }
    }
}

impl Editor {
    /// Legend of the "terrain type" colour mode: provinces of each terrain.
    /// True: the rules are to be shown.
    pub(super) fn terrain_legend_ui(&self, ui: &mut egui::Ui) -> bool {
        let (Some(doc), Some((_, r))) = (&self.doc, &self.tt.result) else {
            ui.colored_label(WARN, "needs the land cover (still loading?)");
            return false;
        };
        let mut n: Vec<(&str, usize)> = Vec::new();
        for (p, _) in doc.data.provinces.iter() {
            let t = r.terrain.get(p as usize).map_or("", |t| t.as_str());
            match n.iter_mut().find(|x| x.0 == t) {
                Some(x) => x.1 += 1,
                None => n.push((t, 1)),
            }
        }
        let order = |t: &str| LAND_TERRAIN.iter().chain(&["lakes", "ocean"]).position(|x| *x == t).unwrap_or(99);
        n.sort_by_key(|x| (order(x.0), x.0));
        ui.horizontal_wrapped(|ui| {
            for (t, c) in &n {
                let k = terrain_colour(t);
                ui.label(super::texture_ui::swatch_text(ui, Color32::from_rgb(k[0], k[1], k[2]), &format!("{t} {c}")));
            }
        });
        let mut open = false;
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("provinces as the build sets them (lakes: also land out of play)").small().weak());
            open = ui.small_button("Terrain rules…").on_hover_text("Mod details → Terrain types").clicked();
        });
        open
    }
}

/// A terrain picked from the land terrains.
fn terrain_combo(ui: &mut egui::Ui, id: impl std::hash::Hash + std::fmt::Debug, t: &mut String) {
    egui::ComboBox::from_id_salt(id).width(90.0).selected_text(t.as_str()).show_ui(ui, |ui| {
        for x in LAND_TERRAIN {
            ui.horizontal(|ui| {
                swatch(ui, x);
                ui.selectable_value(t, x.to_string(), x);
            });
        }
    });
}

/// The rules editor ("Mod details" → Terrain types). `res`: the provinces
/// each rule takes with these rules (when the land cover is loaded).
pub(super) fn terrain_rules_ui(ui: &mut egui::Ui, t: &mut TerrainTypes, res: Option<&TerrainResult>) {
    ui.label(
        RichText::new(
            "The terrain of a land province (supply, combat, movement): the first rule from the top whose land makes up at least its share of the province. \
             The land of a rule is one or more kinds (land cover, height, slope of the pixel); each pixel of a kind counts as much as its weight. \
             A terrain set on a province wins, and provinces at the map's edge are mountain. Colour the map by \"terrain type\" to see the result.",
        )
        .small()
        .weak(),
    );
    let n = t.rules.len();
    let mut action: Option<(usize, i8)> = None; // -1 up, 1 down, 0 delete
    for (i, r) in t.rules.iter_mut().enumerate() {
        egui::Frame::group(ui.style()).inner_margin(4.0).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{}.", i + 1)).strong());
                swatch(ui, &r.terrain);
                terrain_combo(ui, ("tt_rule", i), &mut r.terrain);
                ui.label("from");
                let mut pct = r.min_share * 100.0;
                if ui.add(egui::DragValue::new(&mut pct).range(0.0..=1000.0).speed(0.5).suffix(" %")).on_hover_text("of the province").changed() {
                    r.min_share = pct / 100.0;
                }
                if ui.add_enabled(i > 0, egui::Button::new("⏶").small()).on_hover_text("check earlier").clicked() {
                    action = Some((i, -1));
                }
                if ui.add_enabled(i + 1 < n, egui::Button::new("⏷").small()).on_hover_text("check later").clicked() {
                    action = Some((i, 1));
                }
                if ui.small_button("🗑").on_hover_text("delete the rule").clicked() {
                    action = Some((i, 0));
                }
                if let Some(c) = res.and_then(|r| r.by_rule.get(i)) {
                    ui.label(RichText::new(format!("{c} provinces")).small().weak());
                }
            });
            let mut remove = None;
            for (j, k) in r.land.iter_mut().enumerate() {
                ui.horizontal_wrapped(|ui| {
                    ui.add_space(14.0);
                    egui::ComboBox::from_id_salt(("tt_cover", i, j)).width(96.0).selected_text(k.cover.name()).show_ui(ui, |ui| {
                        for c in Cover::ALL {
                            ui.selectable_value(&mut k.cover, c, c.name());
                        }
                    });
                    ui.label("height");
                    limit(ui, &mut k.from_m, 2000.0, -500.0..=9000.0, 10.0, " m");
                    ui.label("to");
                    limit(ui, &mut k.below_m, 2000.0, -500.0..=9000.0, 10.0, " m");
                    ui.label("slope");
                    limit(ui, &mut k.from_slope, 25.0, 0.0..=90.0, 0.2, "°");
                    ui.label("to");
                    limit(ui, &mut k.below_slope, 10.0, 0.0..=90.0, 0.2, "°");
                    ui.add(egui::DragValue::new(&mut k.weight).range(0.0..=10.0).speed(0.01).prefix("× ")).on_hover_text("how much a pixel of this kind counts");
                    if ui.small_button("🗑").on_hover_text("remove this kind of land").clicked() {
                        remove = Some(j);
                    }
                });
                if k.empty() {
                    ui.colored_label(WARN, "    matches nothing: a \"from\" is not below its \"to\"");
                }
            }
            if let Some(j) = remove {
                r.land.remove(j);
            }
            ui.horizontal(|ui| {
                ui.add_space(14.0);
                if ui.small_button("+ kind of land").clicked() {
                    r.land.push(LandKind::default());
                }
                if r.land.is_empty() {
                    ui.colored_label(WARN, if r.min_share <= 0.0 { "takes every province that gets this far" } else { "no land: applies nowhere" });
                } else if res.and_then(|r| r.by_rule.get(i)) == Some(&0) {
                    ui.colored_label(WARN, "applies nowhere (rules above take this land?)");
                }
            });
        });
    }
    match action {
        Some((i, 0)) => {
            t.rules.remove(i);
        }
        Some((i, d)) => t.rules.swap(i, (i as i64 + d as i64) as usize),
        None => {}
    }
    ui.horizontal(|ui| {
        ui.label("other land");
        swatch(ui, &t.other);
        terrain_combo(ui, "tt_other", &mut t.other);
        if let Some(c) = res.and_then(|r| r.by_rule.last()) {
            ui.label(RichText::new(format!("{c} provinces")).small().weak());
        }
    });
    if let Some(r) = res
        && r.own > 0
    {
        ui.label(RichText::new(format!("{} provinces have a terrain set on them", r.own)).small().weak());
    }
    if res.is_none() {
        ui.label(RichText::new("(how many provinces each rule takes shows once the land cover is loaded)").small().weak());
    }
    ui.horizontal(|ui| {
        if ui.button("Add rule").on_hover_text("a rule at the top (it is checked first)").clicked() {
            t.rules.insert(0, TerrainRule::default());
        }
        if ui.button("Reset to defaults").on_hover_text("the rules the build had before they could be edited").clicked() {
            *t = TerrainTypes::default();
        }
    });
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use serde_json::{Value, json};

    use super::super::density::Terrain;
    use super::super::doc::Doc;
    use super::super::model::{Labels, Loaded, MapData};
    use super::super::{MapMode, MetaEdit, Settings};
    use super::*;

    fn from<T: serde::de::DeserializeOwned>(v: Value) -> T {
        serde_json::from_value(v).unwrap()
    }

    /// Five land provinces of two pixels each, on land that the default
    /// rules make urban, mountain, plains, forest and hills.
    fn five() -> Doc {
        let mut data = MapData { pixel_m: 100.0, ..Default::default() };
        for id in 1..=5u32 {
            data.provinces.put(id, from(json!({"id": id, "rgb": [40 * id, 0, 0], "type": "land", "name": format!("p{id}")})));
            data.provinces.get_mut(id).unwrap().state = 1;
        }
        data.states.put(1, from(json!({"id": 1, "name": "A", "nation": 1})));
        data.nations.put(1, from(json!({"id": 1, "name": "N1", "capital_state": 1, "language": "de", "place": "town"})));
        let geo = crate::geo::FileGeoInfo { transform: None, crs: None, nodata: None };
        let px: Vec<u32> = (1..=5).flat_map(|p| [p, p]).collect();
        let mut doc = Doc::new(PathBuf::from("/nonexistent"), Loaded { data, labels: Labels { w: 10, h: 1, px }, geo, extra: Default::default() });
        // land cover, slope: built-up flat, grass steep, crop flat, forest 15°, grass 15°
        let (cover, slope): (Vec<u8>, Vec<f32>) = [(1, 5.0), (6, 30.0), (7, 3.0), (3, 15.0), (6, 15.0)].iter().flat_map(|&c| [c, c]).unzip();
        let n = 10;
        doc.terrain = Some(Arc::new(Terrain { w: 10, h: 1, dem: vec![500.0; n], slope: vec![0; n], urban: vec![0; n], sealed: vec![false; n], cover, slope_px: slope }));
        doc
    }

    fn terrains(r: &TerrainResult) -> Vec<&str> {
        (1..=5).map(|p| r.terrain[p].as_str()).collect()
    }

    #[test]
    fn rules_preview_the_build_terrain() {
        let mut ed = Editor::new(Settings::default());
        ed.doc = Some(five());
        let r = ed.province_terrains().unwrap();
        assert_eq!(terrains(&r), ["urban", "mountain", "plains", "forest", "hills"]);
        assert_eq!(r.by_rule, [1, 1, 1, 1, 1]);
        assert_eq!(r.rule[4], Some(3));
        assert!(Arc::ptr_eq(&r, &ed.province_terrains().unwrap()), "not cached");
        assert_eq!(ed.supply_view().unwrap().terrain[4], "forest");

        // rules edited in the open "Mod details" window: shown before they are applied
        let mut m = ed.doc.as_ref().unwrap().data.mod_meta();
        m.terrain.rules.retain(|r| r.terrain != "forest");
        m.terrain.rules[1].land[0].from_slope = Some(35.0); // grass at 30° is no longer steep
        ed.meta = Some(MetaEdit::new(m.clone()));
        let r2 = ed.province_terrains().unwrap();
        assert_eq!(terrains(&r2), ["urban", "hills", "plains", "hills", "hills"]);
        assert_eq!(r2.by_rule, [1, 0, 1, 3]);
        assert!(r2.rev > r.rev);
        assert_eq!(ed.supply_view().unwrap().terrain[2], "hills", "supply does not follow the rules");
        ed.settings.mode = MapMode::Terrain;
        ed.refresh_style();
        assert_eq!(ed.style.fill[2][..3], terrain_colour("hills"));
        assert_eq!(ed.style.fill[1][..3], terrain_colour("urban"));

        // a terrain set on a province wins; supply follows that too
        ed.run("Set terrain", |d, _| {
            d.data.provinces.get_mut(5).unwrap().terrain = Some("marsh".into());
            Ok(String::new())
        });
        let r3 = ed.province_terrains().unwrap();
        assert_eq!(r3.terrain[5], "marsh");
        assert_eq!((r3.own, r3.by_rule.iter().sum::<usize>()), (1, 4));
        assert_eq!(ed.supply_view().unwrap().terrain[5], "marsh", "supply kept the old terrain");

        // closed without applying: the map's rules again
        ed.meta = None;
        assert_eq!(terrains(&ed.province_terrains().unwrap()), ["urban", "mountain", "plains", "forest", "marsh"]);
        // applied: stored with the map
        ed.set_mod_meta(m);
        assert_eq!(terrains(&ed.province_terrains().unwrap()), ["urban", "hills", "plains", "hills", "marsh"]);
        assert_eq!(ed.doc.as_ref().unwrap().data.mod_meta().terrain.rules.len(), 3);
    }
}
