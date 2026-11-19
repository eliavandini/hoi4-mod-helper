//! A small game launcher in the "Build mod and play" window: the mods the
//! game can load (the `.mod` files in its user folder) with their version,
//! the game version they are made for, where they come from and their tags.
//! Like the Paradox launcher, it writes the ticked mods to `dlc_load.json`
//! and starts the game the way the game's `launcher-settings.json` says.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use eframe::egui::{self, Color32, RichText};

use crate::modbuild::{self, Dirs};

const OK: Color32 = Color32::from_rgb(120, 220, 120);
const WARN: Color32 = Color32::from_rgb(255, 190, 90);
const ERR: Color32 = Color32::from_rgb(255, 110, 110);

/// How the game is started and where it keeps its user files, from
/// `launcher-settings.json` in the game folder.
#[derive(Clone, Debug)]
pub struct GameInfo {
    /// Program to run, relative to the game folder ("./run_hoi4").
    pub exe: String,
    pub args: Vec<String>,
    /// "1.19.4.1"
    pub version: String,
    /// The folder with `mod/` and `dlc_load.json`.
    pub user_dir: PathBuf,
}

fn data_home() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| std::env::home_dir().unwrap_or_default().join(".local/share"))
}

impl GameInfo {
    pub fn read(game: &Path) -> Result<GameInfo> {
        let path = game.join("launcher-settings.json");
        let text = std::fs::read_to_string(&path).with_context(|| format!("no HOI4 install at {} ({} missing)", game.display(), path.display()))?;
        let v: serde_json::Value = serde_json::from_str(&text).with_context(|| format!("cannot read {}", path.display()))?;
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
        let exe = s("exePath").unwrap_or_else(|| "./run_hoi4".into());
        let args = v.get("exeArgs").and_then(|a| a.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
        let user_dir = s("gameDataPath")
            .map(|p| p.replace("$LINUX_DATA_HOME", &data_home().to_string_lossy()))
            // Windows and macOS name the Documents folder with placeholders of their own
            .filter(|p| !p.contains(['$', '%']))
            .map(PathBuf::from)
            .unwrap_or_else(|| modbuild::default_mods_dir().parent().map(Path::to_path_buf).unwrap_or_default());
        Ok(GameInfo { exe, args, version: s("rawVersion").unwrap_or_default(), user_dir })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Origin {
    /// Built by mod-helper (a local mod).
    ModHelper,
    Local,
    Steam,
    Paradox,
}

impl Origin {
    pub fn label(self) -> &'static str {
        match self {
            Origin::ModHelper => "local (mod-helper)",
            Origin::Local => "local",
            Origin::Steam => "Steam Workshop",
            Origin::Paradox => "Paradox Mods",
        }
    }
}

/// One `.mod` file of the game's mod folder.
#[derive(Clone, Debug)]
pub struct ModEntry {
    /// As dlc_load.json names it: "mod/ugc_123.mod".
    pub id: String,
    pub name: String,
    pub version: String,
    /// Game version it is made for ("1.19.*"); empty if not given.
    pub supported: String,
    pub tags: Vec<String>,
    pub origin: Origin,
    /// Its folder (or zip archive).
    pub path: Option<PathBuf>,
    /// The folder or archive exists.
    pub present: bool,
    /// Mods it loads after when they are enabled (by name; load order only).
    pub dependencies: Vec<String>,
}

/// The enabled mods in load order: as ticked, except that a mod comes after
/// the enabled mods its `dependencies` name.
pub fn load_order(enabled: &[String], mods: &[ModEntry]) -> Vec<String> {
    let by_id: BTreeMap<&str, &ModEntry> = mods.iter().map(|m| (m.id.as_str(), m)).collect();
    let mut by_name: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for id in enabled {
        if let Some(m) = by_id.get(id.as_str()) {
            by_name.entry(m.name.as_str()).or_default().push(id.as_str());
        }
    }
    fn visit<'a>(id: &'a str, by_id: &BTreeMap<&str, &'a ModEntry>, by_name: &BTreeMap<&str, Vec<&'a str>>, seen: &mut BTreeSet<&'a str>, out: &mut Vec<String>) {
        if !seen.insert(id) {
            return;
        }
        if let Some(m) = by_id.get(id) {
            for dep in &m.dependencies {
                for &d in by_name.get(dep.as_str()).into_iter().flatten() {
                    visit(d, by_id, by_name, seen, out);
                }
            }
        }
        out.push(id.to_string());
    }
    let (mut seen, mut out) = (BTreeSet::new(), Vec::new());
    for id in enabled {
        visit(id, &by_id, &by_name, &mut seen, &mut out);
    }
    out
}

/// `key="value"` pairs and `key={ "a" "b" }` lists of a descriptor.
pub fn parse_descriptor(text: &str) -> (BTreeMap<String, String>, BTreeMap<String, Vec<String>>) {
    let (mut values, mut lists) = (BTreeMap::new(), BTreeMap::new());
    let b = text.as_bytes();
    let mut i = 0;
    let skip_ws = |i: &mut usize| {
        while *i < b.len() && b[*i].is_ascii_whitespace() {
            *i += 1;
        }
    };
    let quoted = |i: &mut usize| -> String {
        // at the opening quote
        let start = *i + 1;
        let mut j = start;
        while j < b.len() && b[j] != b'"' {
            j += 1;
        }
        *i = (j + 1).min(b.len());
        text[start..j.min(b.len())].to_string()
    };
    while i < b.len() {
        skip_ws(&mut i);
        if i >= b.len() {
            break;
        }
        if b[i] == b'#' {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        let start = i;
        while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
            i += 1;
        }
        if i == start {
            i += 1; // stray character
            continue;
        }
        let key = text[start..i].to_string();
        skip_ws(&mut i);
        if i >= b.len() || b[i] != b'=' {
            continue;
        }
        i += 1;
        skip_ws(&mut i);
        if i >= b.len() {
            break;
        }
        match b[i] {
            b'"' => {
                values.insert(key, quoted(&mut i));
            }
            b'{' => {
                i += 1;
                let mut items = Vec::new();
                loop {
                    skip_ws(&mut i);
                    if i >= b.len() {
                        break;
                    }
                    match b[i] {
                        b'}' => {
                            i += 1;
                            break;
                        }
                        b'"' => items.push(quoted(&mut i)),
                        _ => {
                            let s = i;
                            while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'}' {
                                i += 1;
                            }
                            items.push(text[s..i].to_string());
                        }
                    }
                }
                lists.insert(key, items);
            }
            _ => {
                let s = i;
                while i < b.len() && !b[i].is_ascii_whitespace() {
                    i += 1;
                }
                values.insert(key, text[s..i].to_string());
            }
        }
    }
    (values, lists)
}

/// The mods in `<user_dir>/mod`, by name.
pub fn scan(user_dir: &Path) -> Result<Vec<ModEntry>> {
    let dir = user_dir.join("mod");
    let mut out = Vec::new();
    for f in modbuild::vanilla::files_with(&dir, "mod")? {
        let Ok(text) = modbuild::vanilla::read_text(&f) else { continue };
        let (v, l) = parse_descriptor(&text);
        let file = f.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let path = v.get("path").or_else(|| v.get("archive")).map(|p| {
            let p = PathBuf::from(p);
            if p.is_absolute() { p } else { user_dir.join(p) }
        });
        let present = path.as_ref().is_some_and(|p| p.exists());
        let ours = path.as_ref().is_some_and(|p| p.join(modbuild::MARKER).exists() || p.join(".made_by_make_map_mod").exists());
        let in_workshop = path.as_ref().is_some_and(|p| p.to_string_lossy().contains("steamapps/workshop/content"));
        let origin = if file.starts_with("ugc_") || in_workshop {
            Origin::Steam
        } else if file.starts_with("pdx_") {
            Origin::Paradox
        } else if ours {
            Origin::ModHelper
        } else {
            Origin::Local
        };
        out.push(ModEntry {
            id: format!("mod/{file}"),
            name: v.get("name").cloned().unwrap_or_else(|| file.trim_end_matches(".mod").to_string()),
            version: v.get("version").cloned().unwrap_or_default(),
            supported: v.get("supported_version").cloned().unwrap_or_default(),
            tags: l.get("tags").cloned().unwrap_or_default(),
            origin,
            path,
            present,
            dependencies: l.get("dependencies").cloned().unwrap_or_default(),
        });
    }
    out.sort_by_key(|m| (m.origin != Origin::ModHelper, m.name.to_lowercase()));
    Ok(out)
}

/// The mods dlc_load.json enables, in load order.
pub fn read_enabled(user_dir: &Path) -> Vec<String> {
    std::fs::read_to_string(user_dir.join("dlc_load.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.get("enabled_mods").and_then(|m| m.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()))
        .unwrap_or_default()
}

/// Write the enabled mods to dlc_load.json, keeping its other fields
/// (disabled DLCs).
pub fn write_enabled(user_dir: &Path, enabled: &[String]) -> Result<()> {
    let path = user_dir.join("dlc_load.json");
    let mut v: serde_json::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .filter(|v: &serde_json::Value| v.is_object())
        .unwrap_or_else(|| serde_json::json!({ "disabled_dlcs": [] }));
    v["enabled_mods"] = serde_json::json!(enabled);
    let tmp = user_dir.join("dlc_load.json.tmp");
    std::fs::write(&tmp, serde_json::to_string(&v)?).with_context(|| format!("cannot write {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("cannot write {}", path.display()))
}

/// Whether a mod made for `supported` ("1.19.*", "v1.19.4") fits the game
/// version `game` ("1.19.4.1"); None if it does not say.
pub fn version_matches(supported: &str, game: &str) -> Option<bool> {
    let s = supported.trim().trim_start_matches(['v', 'V']);
    if s.is_empty() || game.is_empty() {
        return None;
    }
    let g: Vec<&str> = game.trim_start_matches(['v', 'V']).split('.').collect();
    for (i, part) in s.split('.').enumerate() {
        if part == "*" {
            return Some(true);
        }
        if g.get(i) != Some(&part) {
            return Some(false);
        }
    }
    Some(true)
}

/// A process of that name (as /proc/<pid>/comm gives it) is running.
pub fn process_running(name: &str) -> bool {
    let Ok(rd) = std::fs::read_dir("/proc") else { return false };
    rd.filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().bytes().all(|c| c.is_ascii_digit()))
        .any(|e| std::fs::read_to_string(e.path().join("comm")).is_ok_and(|c| c.trim() == name))
}

/// Start the game as the Paradox launcher does: its program with its
/// arguments, in the game folder, in a process group of its own (so it
/// outlives mod-helper and a Ctrl+C in its terminal).
pub fn start_game(game: &Path, info: &GameInfo) -> Result<Child> {
    let exe = game.join(info.exe.trim_start_matches("./"));
    if !exe.exists() {
        bail!("{} does not exist", exe.display());
    }
    let mut cmd = Command::new(&exe);
    cmd.args(&info.args).current_dir(game).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    cmd.spawn().with_context(|| format!("cannot start {}", exe.display()))
}

/// State of the launcher part of the window.
#[derive(Default)]
pub struct Launcher {
    /// Game folder the list was read for.
    loaded_for: Option<PathBuf>,
    info: Option<Result<GameInfo, String>>,
    mods: Vec<ModEntry>,
    /// Ticked mods, in load order.
    enabled: Vec<String>,
    search: String,
    tags: BTreeSet<String>,
    only_enabled: bool,
    /// The last successful build, once its mod is ticked.
    synced: Option<u64>,
    game: Option<Child>,
    /// Running game and Steam (checked once a second).
    procs: Option<(Instant, bool, bool)>,
    msg: Option<(String, bool)>,
}

impl Launcher {
    /// Reap the game once it has quit.
    pub fn poll(&mut self) {
        if let Some(c) = &mut self.game
            && !matches!(c.try_wait(), Ok(None))
        {
            self.game = None;
            self.procs = None;
        }
    }

    /// Read the game's settings and mod list (the ticks from dlc_load.json
    /// the first time, else kept).
    fn reload(&mut self, game: &Path) {
        let first = self.loaded_for.as_deref() != Some(game);
        self.loaded_for = Some(game.to_path_buf());
        self.info = Some(GameInfo::read(game).map_err(|e| format!("{e:#}")));
        self.mods.clear();
        if let Some(Ok(info)) = &self.info {
            match scan(&info.user_dir) {
                Ok(m) => self.mods = m,
                Err(e) => self.msg = Some((format!("{e:#}"), true)),
            }
            if first {
                self.enabled = read_enabled(&info.user_dir);
            }
        }
    }

    /// Mods that pass the search, the tag filter and "ticked only".
    fn visible(&self) -> Vec<usize> {
        let needle = self.search.trim().to_lowercase();
        (0..self.mods.len())
            .filter(|&i| {
                let m = &self.mods[i];
                (needle.is_empty() || m.name.to_lowercase().contains(&needle) || m.id.to_lowercase().contains(&needle))
                    && self.tags.iter().all(|t| m.tags.iter().any(|x| x == t))
                    && (!self.only_enabled || self.enabled.contains(&m.id))
            })
            .collect()
    }

    fn set(&mut self, id: &str, on: bool) {
        if on {
            if !self.enabled.iter().any(|x| x == id) {
                self.enabled.push(id.to_string());
            }
        } else {
            self.enabled.retain(|x| x != id);
        }
    }

    /// Game and Steam running, checked at most once a second.
    fn running(&mut self) -> (bool, bool) {
        if self.procs.is_none_or(|(t, _, _)| t.elapsed() > Duration::from_secs(1)) {
            self.procs = Some((Instant::now(), process_running("hoi4"), process_running("steam")));
        }
        let (_, game, steam) = self.procs.unwrap();
        (game || self.game.is_some(), steam)
    }

    /// The launcher: mod list with search and tag filter, and Play.
    /// `built`: number and mod folder of a build that finished; `failed`: the last build failed.
    pub fn ui(&mut self, ui: &mut egui::Ui, dirs: &Dirs, building: bool, failed: bool, built: Option<(u64, &Path)>) {
        self.poll();
        if self.loaded_for.as_deref() != Some(dirs.game.as_path()) {
            self.reload(&dirs.game);
        }
        // a fresh build is ticked (and its .mod file may be new)
        if let Some((id, dir)) = built
            && self.synced != Some(id)
        {
            self.reload(&dirs.game);
            if let Some(f) = dir.file_name() {
                self.set(&format!("mod/{}.mod", f.to_string_lossy()), true);
            }
            self.synced = Some(id);
        }
        let info = match &self.info {
            Some(Ok(i)) => i.clone(),
            Some(Err(e)) => {
                ui.colored_label(ERR, e);
                ui.label(RichText::new("Set the HOI4 install in Mod details.").small().weak());
                return;
            }
            None => return,
        };
        if dirs.mods != info.user_dir.join("mod") {
            ui.colored_label(WARN, format!("Mods are built into {}, but the game loads them from {}.", dirs.mods.display(), info.user_dir.join("mod").display()));
        }

        // filters
        let mut all_tags: BTreeMap<&str, usize> = BTreeMap::new();
        for m in &self.mods {
            for t in &m.tags {
                *all_tags.entry(t.as_str()).or_default() += 1;
            }
        }
        let mut reload = false;
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.search).hint_text("🔍 search name").desired_width(200.0));
            let label = if self.tags.is_empty() { "Tags ▾".to_string() } else { format!("Tags ({}) ▾", self.tags.len()) };
            ui.menu_button(label, |ui| {
                ui.label(RichText::new("show mods with all ticked tags").small().weak());
                for (t, n) in &all_tags {
                    let mut on = self.tags.contains(*t);
                    if ui.checkbox(&mut on, format!("{t} ({n})")).changed() {
                        if on {
                            self.tags.insert(t.to_string());
                        } else {
                            self.tags.remove(*t);
                        }
                    }
                }
                ui.separator();
                if ui.button("Clear tags").clicked() {
                    self.tags.clear();
                }
            });
            ui.checkbox(&mut self.only_enabled, "ticked only");
            if ui.button("↻").on_hover_text("Read the mod folder again (keeps the ticks)").clicked() {
                reload = true;
            }
        });
        if reload {
            self.reload(&dirs.game);
        }
        let shown = self.visible();
        let known: BTreeSet<&str> = self.mods.iter().map(|m| m.id.as_str()).collect();
        let n_on = self.enabled.iter().filter(|e| known.contains(e.as_str())).count();
        ui.label(RichText::new(format!("{n_on} of {} mods ticked · showing {} · game {}", self.mods.len(), shown.len(), info.version)).small().weak());

        // the list
        let mut toggles: Vec<(String, bool)> = Vec::new();
        egui::ScrollArea::vertical().id_salt("launcher_mods").max_height((ui.available_height() - 64.0).max(120.0)).auto_shrink([false, true]).show(ui, |ui| {
            egui::Grid::new("launcher_grid").num_columns(5).striped(true).spacing([12.0, 4.0]).show(ui, |ui| {
                ui.label("");
                ui.label(RichText::new("Mod").strong());
                ui.label(RichText::new("Version").strong());
                ui.label(RichText::new("For game").strong());
                ui.label(RichText::new("From").strong());
                ui.end_row();
                for &i in &shown {
                    let m = &self.mods[i];
                    let mut on = self.enabled.contains(&m.id);
                    let r = ui.add_enabled(m.present, egui::Checkbox::without_text(&mut on));
                    if r.changed() {
                        toggles.push((m.id.clone(), on));
                    }
                    let mut name = RichText::new(&m.name);
                    if m.origin == Origin::ModHelper {
                        name = name.strong();
                    }
                    if !m.present {
                        name = name.color(ERR);
                    }
                    let mut tip = format!("{}\n{}\n{}", m.name, m.id, m.path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "no path".into()));
                    if !m.present {
                        tip += "\nits folder is missing";
                    }
                    if !m.tags.is_empty() {
                        tip += &format!("\ntags: {}", m.tags.join(", "));
                    }
                    if !m.dependencies.is_empty() {
                        tip += &format!("\nloads after (if ticked): {}", m.dependencies.join(", "));
                    }
                    ui.vertical(|ui| {
                        ui.set_width(360.0);
                        ui.add(egui::Label::new(name).truncate()).on_hover_text(tip);
                        if !m.tags.is_empty() {
                            ui.add(egui::Label::new(RichText::new(m.tags.join(" · ")).small().weak()).truncate());
                        }
                    });
                    ui.label(if m.version.is_empty() { "–" } else { m.version.as_str() });
                    match version_matches(&m.supported, &info.version) {
                        Some(true) => ui.colored_label(OK, &m.supported),
                        Some(false) => ui.colored_label(WARN, &m.supported).on_hover_text(format!("made for another game version than {}", info.version)),
                        None => ui.label("–"),
                    };
                    ui.label(m.origin.label());
                    ui.end_row();
                }
            });
            if shown.is_empty() {
                ui.label(RichText::new("no mods match").weak());
            }
        });
        for (id, on) in toggles {
            self.set(&id, on);
        }

        // play
        ui.separator();
        let (game_running, steam) = self.running();
        if game_running || self.game.is_some() {
            ui.ctx().request_repaint_after(Duration::from_secs(1));
        }
        ui.horizontal(|ui| {
            let can = !building && !game_running;
            let play = ui
                .add_enabled(can, egui::Button::new(RichText::new("▶ Play").strong()).min_size(egui::vec2(90.0, 26.0)))
                .on_hover_text("Write the ticked mods to dlc_load.json and start Hearts of Iron IV")
                .on_disabled_hover_text(if building { "wait for the build to finish" } else { "Hearts of Iron IV is running" });
            if play.clicked() {
                let ticked: Vec<String> = self.enabled.iter().filter(|e| self.mods.iter().any(|m| &m.id == *e && m.present)).cloned().collect();
                let ids = load_order(&ticked, &self.mods);
                self.msg = Some(match write_enabled(&info.user_dir, &ids).and_then(|_| start_game(&dirs.game, &info)) {
                    Ok(child) => {
                        self.game = Some(child);
                        self.procs = None;
                        (format!("Hearts of Iron IV starting with {} mods", ids.len()), false)
                    }
                    Err(e) => (format!("{e:#}"), true),
                });
            }
            if building {
                ui.spinner();
                ui.label("building…");
            } else if game_running {
                ui.colored_label(OK, "Hearts of Iron IV is running");
            } else if failed {
                ui.colored_label(WARN, "the last build failed: the game gets the mod as it is in the mod folder");
            }
            if !steam {
                ui.colored_label(WARN, "Steam is not running; the game needs it");
            }
        });
        if let Some((m, err)) = &self.msg {
            ui.label(RichText::new(m).small().color(if *err { ERR } else { ui.visuals().weak_text_color() }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptors() {
        let (v, l) = parse_descriptor(
            "version=\"1.2\"\ntags={\n\t\"Map\"\n\t\"Alternative History\"\n}\nname=\"A mod\"\n# comment = \"x\"\nsupported_version=1.19.*\ndependencies={ \"Kaiserreich\" Other }\npath=\"mod/a\"\n",
        );
        assert_eq!(v["version"], "1.2");
        assert_eq!(v["name"], "A mod");
        assert_eq!(v["supported_version"], "1.19.*");
        assert_eq!(v["path"], "mod/a");
        assert!(!v.contains_key("comment"));
        assert_eq!(l["tags"], vec!["Map", "Alternative History"]);
        assert_eq!(l["dependencies"], vec!["Kaiserreich", "Other"]);
    }

    #[test]
    fn order_follows_dependencies() {
        let m = |id: &str, name: &str, deps: &[&str]| ModEntry {
            id: id.into(),
            name: name.into(),
            version: String::new(),
            supported: String::new(),
            tags: Vec::new(),
            origin: Origin::Local,
            path: None,
            present: true,
            dependencies: deps.iter().map(|d| d.to_string()).collect(),
        };
        let mods = [m("mod/a.mod", "A", &["B", "Not ticked"]), m("mod/b.mod", "B", &[]), m("mod/c.mod", "C", &["A"])];
        let ids = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(load_order(&ids(&["mod/c.mod", "mod/a.mod", "mod/b.mod"]), &mods), ids(&["mod/b.mod", "mod/a.mod", "mod/c.mod"]));
        assert_eq!(load_order(&ids(&["mod/b.mod", "mod/c.mod"]), &mods), ids(&["mod/b.mod", "mod/c.mod"]), "ticked order otherwise");
    }

    /// The window's launcher on a fake game: ticks from dlc_load.json, a new
    /// build ticked once, search and tag filter.
    #[test]
    fn launcher_state() {
        let root = std::env::temp_dir().join(format!("mod-helper-launcher-ui-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (game, user) = (root.join("game"), root.join("user"));
        std::fs::create_dir_all(user.join("mod/ours")).unwrap();
        std::fs::create_dir_all(user.join("mod/other")).unwrap();
        std::fs::create_dir_all(&game).unwrap();
        std::fs::write(game.join("launcher-settings.json"), format!(r#"{{"rawVersion":"1.19.4.1","gameDataPath":"{}"}}"#, user.display())).unwrap();
        std::fs::write(user.join("mod/other.mod"), "name=\"Other Map\"\ntags={ \"Map\" \"Graphics\" }\npath=\"mod/other\"\n").unwrap();
        std::fs::write(user.join("dlc_load.json"), r#"{"disabled_dlcs":[],"enabled_mods":["mod/other.mod"]}"#).unwrap();
        let dirs = Dirs { game: game.clone(), mods: user.join("mod"), sources: Default::default() };
        let mut l = Launcher::default();
        let ctx = egui::Context::default();
        let frame = |l: &mut Launcher, built: Option<(u64, &Path)>| {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| l.ui(ui, &dirs, false, false, built));
            out.textures_delta.clear(); // no renderer here
        };
        frame(&mut l, None);
        assert_eq!(l.enabled, vec!["mod/other.mod"], "ticks from dlc_load.json");
        assert_eq!(l.mods.len(), 1);
        // a build writes ours.mod; the launcher reads it and ticks it
        std::fs::write(user.join("mod/ours.mod"), "name=\"Südtirol\"\ntags={ \"Map\" }\npath=\"mod/ours\"\n").unwrap();
        frame(&mut l, Some((1, &user.join("mod/ours"))));
        assert_eq!(l.mods.len(), 2);
        assert_eq!(l.enabled, vec!["mod/other.mod", "mod/ours.mod"]);
        l.set("mod/ours.mod", false);
        frame(&mut l, Some((1, &user.join("mod/ours"))));
        assert!(!l.enabled.contains(&"mod/ours.mod".to_string()), "unticked stays unticked");
        frame(&mut l, Some((2, &user.join("mod/ours"))));
        assert!(l.enabled.contains(&"mod/ours.mod".to_string()), "the next build ticks it again");
        // filters
        let names = |l: &Launcher| l.visible().into_iter().map(|i| l.mods[i].name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&l), vec!["Other Map", "Südtirol"]);
        l.search = "SÜD".into();
        assert_eq!(names(&l), vec!["Südtirol"], "search ignores case");
        l.search.clear();
        l.tags.insert("Graphics".into());
        assert_eq!(names(&l), vec!["Other Map"]);
        l.tags.insert("Map".into());
        assert_eq!(names(&l), vec!["Other Map"], "all ticked tags");
        l.tags.clear();
        l.set("mod/other.mod", false);
        l.only_enabled = true;
        assert_eq!(names(&l), vec!["Südtirol"]);
        assert_eq!(std::fs::read_to_string(user.join("dlc_load.json")).unwrap(), r#"{"disabled_dlcs":[],"enabled_mods":["mod/other.mod"]}"#, "nothing written before Play");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn versions() {
        assert_eq!(version_matches("1.19.*", "1.19.4.1"), Some(true));
        assert_eq!(version_matches("v1.19.4.*", "1.19.4.1"), Some(true));
        assert_eq!(version_matches("1.*", "1.19.4.1"), Some(true));
        assert_eq!(version_matches("1.18.*", "1.19.4.1"), Some(false));
        assert_eq!(version_matches("1.19.4", "1.19.4.1"), Some(true));
        assert_eq!(version_matches("1.19.5", "1.19.4.1"), Some(false));
        assert_eq!(version_matches("", "1.19.4.1"), None);
    }

    #[test]
    fn mod_folder_and_dlc_load() {
        let user = std::env::temp_dir().join(format!("mod-helper-launcher-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&user);
        std::fs::create_dir_all(user.join("mod/ours")).unwrap();
        std::fs::create_dir_all(user.join("workshop/steamapps/workshop/content/394360/42")).unwrap();
        std::fs::write(user.join("mod/ours").join(modbuild::MARKER), "").unwrap();
        std::fs::write(user.join("mod/ours.mod"), "name=\"Ours\"\nsupported_version=\"1.19.*\"\npath=\"mod/ours\"\n").unwrap();
        std::fs::write(user.join("mod/ugc_42.mod"), format!("name=\"Workshop\"\ntags={{ \"Map\" }}\npath=\"{}\"\n", user.join("workshop/steamapps/workshop/content/394360/42").display())).unwrap();
        std::fs::write(user.join("mod/gone.mod"), "name=\"Gone\"\npath=\"mod/gone\"\n").unwrap();
        let mods = scan(&user).unwrap();
        let by = |id: &str| mods.iter().find(|m| m.id == id).unwrap();
        assert_eq!(mods[0].id, "mod/ours.mod", "mod-helper's own mods first");
        assert_eq!(by("mod/ours.mod").origin, Origin::ModHelper);
        assert_eq!(by("mod/ugc_42.mod").origin, Origin::Steam);
        assert_eq!(by("mod/ugc_42.mod").tags, vec!["Map"]);
        assert_eq!(by("mod/gone.mod").origin, Origin::Local);
        assert!(!by("mod/gone.mod").present && by("mod/ours.mod").present);

        assert!(read_enabled(&user).is_empty());
        std::fs::write(user.join("dlc_load.json"), r#"{"disabled_dlcs":["dlc/x.dlc"],"enabled_mods":["mod/a.mod"]}"#).unwrap();
        assert_eq!(read_enabled(&user), vec!["mod/a.mod"]);
        write_enabled(&user, &["mod/ours.mod".into(), "mod/ugc_42.mod".into()]).unwrap();
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(user.join("dlc_load.json")).unwrap()).unwrap();
        assert_eq!(v["disabled_dlcs"][0], "dlc/x.dlc", "other fields are kept");
        assert_eq!(read_enabled(&user), vec!["mod/ours.mod", "mod/ugc_42.mod"]);
        let _ = std::fs::remove_dir_all(&user);
    }

    #[test]
    fn starts_the_program_of_launcher_settings() {
        let game = std::env::temp_dir().join(format!("mod-helper-game-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&game);
        std::fs::create_dir_all(&game).unwrap();
        std::fs::write(
            game.join("launcher-settings.json"),
            r#"{"rawVersion":"1.19.4.1","exePath":"./run_game","exeArgs":["-gdpr-compliant"],"gameDataPath":"$LINUX_DATA_HOME/Paradox Interactive/Hearts of Iron IV"}"#,
        )
        .unwrap();
        let script = game.join("run_game");
        std::fs::write(&script, "#!/bin/sh\necho \"$PWD $@\" > started.txt\n").unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let info = GameInfo::read(&game).unwrap();
        assert_eq!(info.version, "1.19.4.1");
        assert!(info.user_dir.ends_with("Paradox Interactive/Hearts of Iron IV") && !info.user_dir.to_string_lossy().contains('$'));
        let mut child = start_game(&game, &info).unwrap();
        assert!(child.wait().unwrap().success());
        assert_eq!(std::fs::read_to_string(game.join("started.txt")).unwrap().trim(), format!("{} -gdpr-compliant", game.display()));
        // a placeholder of another system: the default user folder
        std::fs::write(game.join("launcher-settings.json"), r#"{"gameDataPath":"%USER_DOCUMENTS%/Paradox Interactive/Hearts of Iron IV"}"#).unwrap();
        assert_eq!(GameInfo::read(&game).unwrap().user_dir, modbuild::default_mods_dir().parent().unwrap());
        let _ = std::fs::remove_dir_all(&game);
    }
}
