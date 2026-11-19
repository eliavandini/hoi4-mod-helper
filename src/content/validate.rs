//! Checks of the project's content against what the game will do with it:
//! files it would misread, ids it keeps only one of, references to what is
//! not there (focuses, events, ideas, characters, traits, technologies,
//! ideologies, sprites, pictures, localisation), focus trees that cannot
//! be played, division templates, the stockpile, DLC (names `has_dlc`
//! does not know; technologies and equipment only some players have,
//! given to all), numbers the game does not allow (a share over 1, a
//! building level over its cap, half a day), and what is about the map
//! (states, provinces, capitals, tags) checked against this map.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use super::focus;
use super::kinds::{self, Kind};
use super::script::{self, Block};
use super::source::Index;
use super::store::{DefKey, SHARED, Store};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "note",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Issue {
    pub severity: Severity,
    pub category: &'static str,
    pub pkg: String,
    pub file: String,
    pub line: usize,
    pub def: Option<DefKey>,
    pub message: String,
}

/// What the map says, for the checks of what content says about it.
#[derive(Clone, Debug, Default)]
pub struct Facts {
    /// Fixed tags of nations → (name, capital state, its states).
    pub nations: BTreeMap<String, (String, u32, BTreeSet<u32>)>,
    /// The largest province id (the ids have gaps where provinces were
    /// deleted: see `ground`).
    pub provinces: u32,
    pub states: BTreeSet<u32>,
    /// All nations of the map have no tag of their own fixed (they get one at the build).
    pub unpinned: usize,
    /// The naval bases of the map.
    pub ports: Vec<super::navy::Port>,
    /// The air bases of the map.
    pub airbases: Vec<super::air::Airfield>,
    /// What each province of the map is (by id; empty: no map).
    pub ground: Vec<Option<super::army::Ground>>,
}

/// How the check of an `else` after an `if` in a history starts (the Check
/// page offers to repair it).
pub const LOOSE_ELSE: &str = "else after an if at the top of a history file";

/// Keys whose value is a state id.
pub(crate) const STATE_KEYS: [&str; 11] =
    ["transfer_state", "set_state_controller", "add_state_core", "remove_state_core", "add_state_claim", "remove_state_claim", "owns_state", "controls_state", "has_full_control_of_state", "is_core_of", "set_capital"];
/// Keys whose value is a tag.
const TAG_KEYS: [&str; 14] = [
    "tag",
    "original_tag",
    "country_exists",
    "exists",
    "is_subject_of",
    "is_in_faction_with",
    "has_war_with",
    "declare_war_on",
    "add_to_faction",
    "give_guarantee",
    "give_military_access",
    "puppet",
    "annex_country",
    "target",
];
const EVENT_KEYS: [&str; 5] = ["country_event", "news_event", "state_event", "unit_leader_event", "operative_leader_event"];

struct Ctx<'a> {
    st: &'a Store,
    base: Option<&'a Index>,
    facts: &'a Facts,
    /// Ids of the content by kind.
    ids: HashMap<Kind, BTreeSet<String>>,
    /// English localisation keys of the content.
    loc: BTreeSet<String>,
    sprites: BTreeSet<String>,
    /// Sub-ideologies of the content.
    ideologies: BTreeSet<String>,
    /// Advisors' idea tokens of the content.
    tokens: BTreeSet<String>,
    /// The game's and the content's land units (with the base game only:
    /// without it battalions of the game would count as unknown).
    units: Option<std::sync::Arc<super::units::UnitDb>>,
    out: Vec<Issue>,
}

impl Ctx<'_> {
    fn has(&self, kind: Kind, id: &str) -> bool {
        self.ids.get(&kind).is_some_and(|s| s.contains(id))
            || self.base.is_some_and(|b| b.has(kind, id))
            || kind == Kind::Idea && (self.tokens.contains(id) || self.base.is_some_and(|b| b.idea_tokens.contains(id)))
    }
    fn has_loc(&self, key: &str) -> bool {
        self.loc.contains(key) || self.base.is_some_and(|b| b.loc.contains_key(key))
    }
    fn has_sprite(&self, name: &str) -> bool {
        self.sprites.contains(name) || self.base.is_some_and(|b| b.sprites.contains_key(name))
    }
    fn has_file(&self, rel: &str) -> bool {
        let rel = rel.replace('\\', "/");
        let stem = rel.rsplit_once('.').map(|x| x.0.to_string()).unwrap_or(rel.clone());
        let names: Vec<String> = std::iter::once(rel.clone()).chain(["dds", "tga", "png"].iter().map(|e| format!("{stem}.{e}"))).collect();
        names.iter().any(|n| self.st.packages.values().any(|p| p.files.contains_key(n)) || self.base.is_some_and(|b| b.files.exists(n)))
    }
    fn tag_known(&self, t: &str) -> bool {
        self.facts.nations.contains_key(t) || self.base.is_some_and(|b| b.tags.contains(t)) || matches!(t, "ROOT" | "FROM" | "PREV" | "THIS" | "OWNER" | "CONTROLLER")
    }
    fn add(&mut self, severity: Severity, category: &'static str, pkg: &str, file: &str, line: usize, def: Option<&DefKey>, message: String) {
        self.out.push(Issue { severity, category, pkg: pkg.to_string(), file: file.to_string(), line, def: def.cloned(), message });
    }
}

/// Run all checks. `base`: the base game (what it has counts as there).
pub fn validate(st: &Store, base: Option<&Index>, facts: &Facts) -> Vec<Issue> {
    let mut ids: HashMap<Kind, BTreeSet<String>> = HashMap::new();
    for k in st.defs() {
        ids.entry(k.kind).or_default().insert(k.id.clone());
    }
    let mut loc = BTreeSet::new();
    let mut sprites = BTreeSet::new();
    let mut ideologies = BTreeSet::new();
    let mut tokens = BTreeSet::new();
    for p in st.packages.values() {
        for (path, f) in &p.files {
            if path.starts_with("common/characters/")
                && let Some(s) = &f.script
            {
                tokens.extend(super::source::idea_tokens(&s.root));
            }
            if let Some(l) = &f.loc
                && path.to_ascii_lowercase().ends_with("_l_english.yml")
            {
                loc.extend(l.entries.iter().map(|e| e.key.clone()));
            }
            for d in &f.defs {
                if d.kind == Kind::Sprite {
                    sprites.insert(d.id.clone());
                }
                if d.kind == Kind::Ideology
                    && let Some(b) = f.script.as_ref().and_then(|s| d.block(s))
                    && let Some(t) = b.block("types")
                {
                    ideologies.extend(t.items.iter().filter_map(|i| i.key.as_ref().map(|k| k.text.clone())));
                }
            }
        }
    }
    let units = base.map(|b| super::units::db(Some(b), st));
    let mut c = Ctx { st, base, facts, ids, loc, sprites, ideologies, tokens, units, out: Vec::new() };
    packages(&mut c);
    syntax(&mut c);
    duplicates(&mut c);
    trees(&mut c);
    definitions(&mut c);
    references(&mut c);
    dlcs(&mut c);
    values(&mut c);
    navy(&mut c);
    air(&mut c);
    army(&mut c);
    loose_elses(&mut c);
    c.out.sort_by(|a, b| (a.severity, &a.pkg, &a.file, a.line).cmp(&(b.severity, &b.pkg, &b.file, b.line)));
    c.out
}

fn packages(c: &mut Ctx) {
    for (name, p) in &c.st.packages {
        if p.is_empty() {
            continue;
        }
        if name != SHARED && !kinds::is_tag(name) {
            c.add(Severity::Error, "packages", name, "", 0, None, format!("content/{name}: not a tag (three capitals or digits) nor \"shared\"; the build skips it"));
        } else if name != SHARED && !c.facts.nations.contains_key(name) {
            c.add(Severity::Error, "packages", name, "", 0, None, format!("no nation of the map has the tag {name} (fix the tag of a nation in the map editor, or rename this content)"));
        }
    }
}

fn syntax(c: &mut Ctx) {
    let mut keys: BTreeMap<(String, String), Vec<(String, String)>> = BTreeMap::new();
    for (name, p) in &c.st.packages {
        for (path, f) in &p.files {
            if let (Some(s), Some(t)) = (&f.script, f.text()) {
                for e in &s.errors {
                    c.out.push(Issue { severity: Severity::Error, category: "syntax", pkg: name.clone(), file: path.clone(), line: script::line_of(t, e.at), def: None, message: e.message.clone() });
                }
            }
            if let Some(l) = &f.loc {
                let file_lang = super::loc::language_of_name(path.rsplit('/').next().unwrap_or(path));
                match (file_lang, l.language.as_deref()) {
                    (None, _) => c.out.push(Issue {
                        severity: Severity::Error,
                        category: "localisation",
                        pkg: name.clone(),
                        file: path.clone(),
                        line: 1,
                        def: None,
                        message: "the game reads only files named *_l_<language>.yml".into(),
                    }),
                    (Some(fl), Some(h)) if fl != h => c.out.push(Issue {
                        severity: Severity::Error,
                        category: "localisation",
                        pkg: name.clone(),
                        file: path.clone(),
                        line: 1,
                        def: None,
                        message: format!("the file is named for {fl} but says l_{h}:"),
                    }),
                    (Some(_), None) => c.out.push(Issue {
                        severity: Severity::Error,
                        category: "localisation",
                        pkg: name.clone(),
                        file: path.clone(),
                        line: 1,
                        def: None,
                        message: "no l_<language>: line at the top".into(),
                    }),
                    _ => {}
                }
                if !f.bom {
                    c.out.push(Issue {
                        severity: Severity::Error,
                        category: "localisation",
                        pkg: name.clone(),
                        file: path.clone(),
                        line: 1,
                        def: None,
                        message: "no byte order mark (UTF-8 BOM): the game skips the file".into(),
                    });
                }
                if let Some(lang) = file_lang {
                    for e in &l.entries {
                        keys.entry((lang.to_string(), e.key.clone())).or_default().push((name.clone(), path.clone()));
                    }
                }
            }
            let top = path.split('/').next().unwrap_or("");
            let known = ["common", "events", "history", "localisation", "gfx", "interface", "music", "sound", "portraits", "map", "tutorial", "script", "dlc_metadata"];
            if !known.contains(&top) && path.contains('/') {
                c.out.push(Issue {
                    severity: Severity::Info,
                    category: "files",
                    pkg: name.clone(),
                    file: path.clone(),
                    line: 0,
                    def: None,
                    message: format!("the game does not read a {top}/ folder"),
                });
            }
        }
    }
    for ((lang, key), at) in keys {
        if at.len() > 1 {
            let (pkg, file) = &at[1];
            c.add(Severity::Warning, "localisation", pkg, file, 0, None, format!("{key} ({lang}) is in {} files; the game keeps one", at.len()));
        }
    }
}

fn duplicates(c: &mut Ctx) {
    let mut seen: BTreeMap<(Kind, String), Vec<DefKey>> = BTreeMap::new();
    for k in c.st.defs() {
        if k.kind.spec().unique {
            seen.entry((k.kind, k.id.clone())).or_default().push(k);
        }
    }
    for ((kind, id), v) in seen {
        if v.len() > 1 {
            let k = &v[1];
            let sev = if matches!(kind, Kind::Focus | Kind::Event | Kind::Idea | Kind::Decision | Kind::Character | Kind::FocusTree) { Severity::Error } else { Severity::Warning };
            let line = c.st.def(k).map(|(d, t, _)| script::line_of(t, d.span.start)).unwrap_or(0);
            let wher: Vec<String> = v.iter().map(|k| format!("{}/{}", k.pkg, k.file)).collect();
            c.add(sev, "duplicates", &k.pkg, &k.file, line, Some(k), format!("{} {id} is defined {} times ({}); the game keeps one", kind.spec().one, v.len(), wher.join(", ")));
        } else if let Some(b) = c.base
            && matches!(kind, Kind::Focus | Kind::Event | Kind::Idea | Kind::Decision | Kind::Character | Kind::FocusTree | Kind::SharedFocus)
            && b.has(kind, &id)
        {
            let k = &v[0];
            let line = c.st.def(k).map(|(d, t, _)| script::line_of(t, d.span.start)).unwrap_or(0);
            c.add(Severity::Warning, "duplicates", &k.pkg, &k.file, line, Some(k), format!("the base game has a {} {id} too; one of them is lost (give it another id)", kind.spec().one));
        }
    }
}

fn trees(c: &mut Ctx) {
    // shared focuses of the content and the base game
    let mut shared: Vec<focus::Focus> = Vec::new();
    for p in c.st.packages.values() {
        for f in p.files.values() {
            if let Some(s) = &f.script {
                shared.extend(focus::shared_focuses(s));
            }
        }
    }
    let mut by_tag: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for k in c.st.defs().filter(|k| k.kind == Kind::FocusTree).collect::<Vec<_>>() {
        let Some((d, text, s)) = c.st.def(&k) else { continue };
        let Some(b) = d.block(s) else { continue };
        let mut tree = focus::tree_of(b);
        if !tree.shared.is_empty() {
            let missing: Vec<&String> = tree.shared.iter().filter(|id| !shared.iter().any(|f| &f.id == *id)).collect();
            for m in missing {
                let base_has = c.base.is_some_and(|x| x.has(Kind::SharedFocus, m));
                if !base_has {
                    c.add(Severity::Error, "focus", &k.pkg, &k.file, script::line_of(text, d.span.start), Some(&k), format!("tree {}: shared focus {m} is not defined", tree.id));
                }
            }
            focus::include_shared(&mut tree, &shared);
        }
        let line_of = |id: &str| tree.get(id).map(|f| script::line_of(text, f.span.start)).unwrap_or(script::line_of(text, d.span.start));
        for i in focus::check(&tree) {
            // focuses of the base game's shared branches are not ours to fix
            if !tree.get(&i.focus).is_some_and(|f| !f.shared || c.ids.get(&Kind::SharedFocus).is_some_and(|s| s.contains(&f.id))) {
                continue;
            }
            let fk = c.st.find(Kind::Focus, &i.focus).or_else(|| c.st.find(Kind::SharedFocus, &i.focus));
            let (pkg, file, line) = match &fk {
                Some(f) if f.file != k.file => (f.pkg.clone(), f.file.clone(), 0),
                _ => (k.pkg.clone(), k.file.clone(), line_of(&i.focus)),
            };
            c.add(if i.error { Severity::Error } else { Severity::Warning }, "focus", &pkg, &file, line, fk.as_ref(), format!("tree {}: {}", tree.id, i.message));
        }
        if tree.countries.is_empty() && !tree.default {
            c.add(Severity::Warning, "focus", &k.pkg, &k.file, script::line_of(text, d.span.start), Some(&k), format!("tree {} has no country = {{ modifier = {{ add = 10 tag = X }} }}: no country gets it", tree.id));
        }
        for t in &tree.countries {
            by_tag.entry(t.clone()).or_default().push(tree.id.clone());
            if !c.tag_known(t) {
                c.add(Severity::Warning, "tags", &k.pkg, &k.file, script::line_of(text, d.span.start), Some(&k), format!("tree {} is for {t}, which is neither a nation with a fixed tag nor a tag of the base game", tree.id));
            }
        }
        for f in &tree.focuses {
            if f.shared && !c.ids.get(&Kind::SharedFocus).is_some_and(|s| s.contains(&f.id)) {
                continue;
            }
            let line = script::line_of(text, f.span.start);
            match &f.icon {
                Some(icon) if !c.has_sprite(icon) => c.add(Severity::Warning, "icons", &k.pkg, &k.file, line, Some(&k), format!("focus {}: icon {icon} is not a sprite of the content or the base game", f.id)),
                None => c.add(Severity::Info, "icons", &k.pkg, &k.file, line, Some(&k), format!("focus {} has no icon", f.id)),
                _ => {}
            }
            let key = f.text.clone().unwrap_or_else(|| f.id.clone());
            if !c.has_loc(&key) {
                c.add(Severity::Warning, "localisation", &k.pkg, &k.file, line, Some(&k), format!("focus {} has no name in English ({key}): the game shows the id", f.id));
            }
        }
    }
    for (t, v) in by_tag {
        if v.len() > 1 {
            c.add(Severity::Warning, "focus", SHARED, "", 0, None, format!("{t} has {} focus trees ({}): it gets the one with the highest factor", v.len(), v.join(", ")));
        }
    }
}

fn definitions(c: &mut Ctx) {
    let keys: Vec<DefKey> = c.st.defs().collect();
    // event namespaces declared in each file
    for k in &keys {
        let Some((d, text, s)) = c.st.def(k) else { continue };
        let line = script::line_of(text, d.span.start);
        let b = d.block(s);
        match k.kind {
            Kind::Event => {
                let Some(b) = b else { continue };
                match d.id.split_once('.') {
                    Some((ns, n)) => {
                        if !s.root.all("add_namespace").any(|i| i.text() == Some(ns)) {
                            c.add(Severity::Error, "events", &k.pkg, &k.file, line, Some(k), format!("event {}: the file has no add_namespace = {ns}", d.id));
                        }
                        if n.parse::<u32>().is_err() {
                            c.add(Severity::Error, "events", &k.pkg, &k.file, line, Some(k), format!("event {}: the part after the dot must be a number", d.id));
                        }
                    }
                    None => c.add(Severity::Warning, "events", &k.pkg, &k.file, line, Some(k), format!("event {}: ids are namespace.number", d.id)),
                }
                if b.get("option").is_none() && !b.yes("hidden") {
                    c.add(Severity::Warning, "events", &k.pkg, &k.file, line, Some(k), format!("event {} has no option: the player cannot close it", d.id));
                }
                if b.get("title").is_none() && !b.yes("hidden") {
                    c.add(Severity::Warning, "events", &k.pkg, &k.file, line, Some(k), format!("event {} has no title", d.id));
                }
                if let Some(pic) = b.atom("picture")
                    && !pic.starts_with('[')
                    && !c.has_sprite(pic)
                {
                    c.add(Severity::Info, "icons", &k.pkg, &k.file, line, Some(k), format!("event {}: picture {pic} is not a sprite", d.id));
                }
            }
            Kind::Decision => {
                let cat = d.group.clone().unwrap_or_default();
                if !c.has(Kind::DecisionCategory, &cat) {
                    c.add(Severity::Error, "decisions", &k.pkg, &k.file, line, Some(k), format!("decision {}: category {cat} is not defined (common/decisions/categories)", d.id));
                }
                if let Some(icon) = b.and_then(|b| b.atom("icon")) {
                    let name = kinds::sprite_name("GFX_decision_", icon);
                    if !c.has_sprite(&name) {
                        c.add(Severity::Info, "icons", &k.pkg, &k.file, line, Some(k), format!("decision {}: icon {name} is not a sprite", d.id));
                    }
                }
            }
            Kind::Idea => {
                if let Some(pic) = b.and_then(|b| b.atom("picture")) {
                    let name = kinds::sprite_name("GFX_idea_", pic);
                    if !c.has_sprite(&name) {
                        c.add(Severity::Info, "icons", &k.pkg, &k.file, line, Some(k), format!("idea {}: picture {name} is not a sprite", d.id));
                    }
                }
            }
            Kind::Character => {
                let Some(b) = b else { continue };
                b.walk(&mut |i, _| {
                    if (i.is("large") || i.is("small"))
                        && let Some(v) = i.text()
                    {
                        let sprite = v.starts_with("GFX_");
                        let ok = if sprite { c.has_sprite(v) } else { c.has_file(v) };
                        if !ok {
                            c.out.push(Issue {
                                // a sprite nothing defines is a generic face (the base game
                                // names some of those); a picture file not there is blank
                                severity: if sprite { Severity::Info } else { Severity::Warning },
                                category: "files",
                                pkg: k.pkg.clone(),
                                file: k.file.clone(),
                                line: script::line_of(text, i.span.start),
                                def: Some(k.clone()),
                                message: if sprite { format!("character {}: portrait {v} is defined nowhere (a generic face)", d.id) } else { format!("character {}: portrait {v} is not there", d.id) },
                            });
                        }
                    }
                });
                for (role, kind) in [("country_leader", Kind::CountryLeaderTrait), ("advisor", Kind::CountryLeaderTrait), ("corps_commander", Kind::UnitLeaderTrait), ("field_marshal", Kind::UnitLeaderTrait), ("navy_leader", Kind::UnitLeaderTrait)] {
                    for r in b.blocks(role) {
                        for t in r.block("traits").into_iter().flat_map(|t| t.values()) {
                            if !c.has(kind, t) {
                                c.add(Severity::Warning, "characters", &k.pkg, &k.file, line, Some(k), format!("character {}: {role} trait {t} is not defined", d.id));
                            }
                        }
                        if role == "country_leader"
                            && let Some(ideo) = r.atom("ideology")
                            && !ideology_known(c, ideo)
                        {
                            c.add(Severity::Warning, "characters", &k.pkg, &k.file, line, Some(k), format!("character {}: leader ideology {ideo} is not a sub-ideology", d.id));
                        }
                    }
                }
            }
            Kind::Sprite => {
                let Some(b) = b else { continue };
                let tex = b.atom("texturefile").or_else(|| b.atom("textureFile")).unwrap_or("");
                if !tex.is_empty() && !c.has_file(tex) {
                    c.add(Severity::Error, "files", &k.pkg, &k.file, line, Some(k), format!("sprite {}: {tex} is in neither the content nor the base game (it shows blank)", d.id));
                }
            }
            Kind::CountryHistory => {
                let tag = &k.pkg;
                let root = &s.root;
                if let Some(cap) = root.atom("capital").and_then(|v| v.parse::<u32>().ok()) {
                    if !c.facts.states.contains(&cap) {
                        c.add(Severity::Error, "map", &k.pkg, &k.file, line, Some(k), format!("capital = {cap}: the map has no state {cap}"));
                    } else if let Some((_, _, states)) = c.facts.nations.get(tag)
                        && !states.contains(&cap)
                    {
                        c.add(Severity::Warning, "map", &k.pkg, &k.file, line, Some(k), format!("capital = {cap} is not a state of {tag} (the build writes the map's capital first; this one wins)"));
                    }
                }
                for i in root.all("oob") {
                    if let Some(v) = i.text() {
                        let rel = format!("history/units/{v}.txt");
                        if !c.st.packages.values().any(|p| p.files.contains_key(&rel)) && c.facts.nations.get(tag).is_none_or(|_| !v.ends_with("_1936")) {
                            c.add(Severity::Error, "files", &k.pkg, &k.file, script::line_of(text, i.span.start), Some(k), format!("oob = {v}: there is no {rel} in the content"));
                        }
                    }
                }
                if let Some(pol) = root.block("set_politics")
                    && let Some(p) = pol.atom("ruling_party")
                    && !c.has(Kind::Ideology, p)
                {
                    c.add(Severity::Warning, "ideologies", &k.pkg, &k.file, line, Some(k), format!("ruling_party = {p} is not an ideology"));
                }
                if let Some(t) = root.block("set_technology") {
                    for i in &t.items {
                        if let Some(key) = i.key.as_ref()
                            && !c.has(Kind::Technology, &key.text)
                        {
                            c.add(Severity::Warning, "technologies", &k.pkg, &k.file, script::line_of(text, i.span.start), Some(k), format!("set_technology: {} is not a technology", key.text));
                        }
                    }
                }
            }
            Kind::DivisionTemplate => {
                let Some(b) = b else { continue };
                if let Some(p) = b.num("priority")
                    && !(0.0..=2.0).contains(&p)
                {
                    c.add(Severity::Error, "divisions", &k.pkg, &k.file, line, Some(k), format!("division template {}: priority is 0, 1 or 2 (reinforcement: low, normal, high)", d.id));
                }
                let Some(db) = c.units.clone() else { continue };
                let base = c.base;
                let name = |id: &str| base.and_then(|b| b.loc.get(id)).map(|t| super::loc::plain(t)).filter(|t| !t.trim().is_empty()).unwrap_or_else(|| id.to_string());
                let t = super::units::Template::read(b);
                for p in db.check_for_players(c.st, &t, &k.pkg, &k.pkg, &name) {
                    let at = p.slot.map(|i| script::line_of(text, t.slots[i].span.start)).unwrap_or(line);
                    c.out.push(Issue { severity: p.severity, category: "divisions", pkg: k.pkg.clone(), file: k.file.clone(), line: at, def: Some(k.clone()), message: format!("division template {}: {}", d.id, p.message) });
                }
            }
            _ => {}
        }
        // localisation of what has a name
        if let Some(b) = b {
            let keys = kinds::loc_keys(k.kind, &d.id, Some(b));
            let missing: Vec<String> = keys
                .into_iter()
                .filter(|(key, what)| {
                    // descriptions may be left out; names may not
                    !c.has_loc(key) && !what.starts_with("Description") && !(k.kind == Kind::Character && !key.contains('_'))
                })
                .map(|(key, what)| format!("{what} ({key})"))
                .collect();
            if !missing.is_empty() && !matches!(k.kind, Kind::Focus | Kind::SharedFocus) {
                let sev = if matches!(k.kind, Kind::Character | Kind::Song) { Severity::Info } else { Severity::Warning };
                c.add(sev, "localisation", &k.pkg, &k.file, line, Some(k), format!("{} {}: no English text for {}", k.kind.spec().one, d.id, missing.join(", ")));
            }
        }
    }
    // provinces of the map in orders of battle (divisions: the army's checks)
    for (name, p) in &c.st.packages {
        for (path, f) in &p.files {
            if !path.starts_with("history/units/") {
                continue;
            }
            let (Some(s), Some(text)) = (&f.script, f.text()) else { continue };
            let mut found = Vec::new();
            // the ids of the map have gaps where provinces were deleted or merged
            let known = |v: u32| match c.facts.ground.get(v as usize) {
                _ if c.facts.ground.is_empty() => (1..=c.facts.provinces).contains(&v),
                Some(g) => g.is_some(),
                None => false,
            };
            s.root.walk(&mut |i, _| {
                if (i.is("location") || i.is("naval_base") || i.is("base"))
                    && let Some(v) = i.text().and_then(|v| v.parse::<u32>().ok())
                    && c.facts.provinces > 0
                    && !known(v)
                {
                    let why = if v == 0 || v > c.facts.provinces { format!("1 to {}", c.facts.provinces) } else { "it was deleted or merged in the map".into() };
                    found.push((script::line_of(text, i.span.start), format!("{} = {v} is not a province of this map ({why})", i.key())));
                }
            });
            for (line, m) in found {
                c.out.push(Issue { severity: Severity::Error, category: "oob", pkg: name.clone(), file: path.clone(), line, def: None, message: m });
            }
        }
    }
}

fn ideology_known(c: &Ctx, ideo: &str) -> bool {
    if c.ideologies.contains(ideo) || c.has(Kind::Ideology, ideo) {
        return true;
    }
    // the base game's sub-ideologies: in its ideology blocks' types
    let Some(b) = c.base else { return true };
    b.of_kind(Kind::Ideology).any(|d| b.text_of(d).is_some_and(|t| t.contains(ideo)))
}

/// What each content file names: events fired, ideas added, characters
/// recruited, states and tags.
fn references(c: &mut Ctx) {
    let mut fired: BTreeSet<String> = BTreeSet::new();
    let mut found: Vec<Issue> = Vec::new();
    for (name, p) in &c.st.packages {
        for (path, f) in &p.files {
            let (Some(s), Some(text)) = (&f.script, f.text()) else { continue };
            if path.ends_with(".gfx") || path.ends_with(".gui") {
                continue;
            }
            let mut add = |sev: Severity, cat: &'static str, at: usize, m: String| {
                found.push(Issue { severity: sev, category: cat, pkg: name.clone(), file: path.clone(), line: script::line_of(text, at), def: None, message: m });
            };
            let mut check_event = |id: &str, at: usize, fired: &mut BTreeSet<String>| {
                fired.insert(id.to_string());
                if !c.has(Kind::Event, id) && id.contains('.') {
                    add(Severity::Warning, "events", at, format!("fires event {id}, which is not defined"));
                }
            };
            s.root.walk(&mut |i, _| {
                let key = i.key();
                if EVENT_KEYS.iter().any(|k| key.eq_ignore_ascii_case(k)) {
                    if let Some(v) = i.text() {
                        check_event(v, i.span.start, &mut fired);
                    } else if let Some(b) = i.block()
                        && let Some(id) = b.atom("id")
                        && b.get("title").is_none()
                        && b.get("option").is_none()
                    {
                        check_event(id, i.span.start, &mut fired);
                    }
                }
                if i.is("random_events")
                    && let Some(b) = i.block()
                {
                    for e in &b.items {
                        if let Some(v) = e.text().filter(|v| v.contains('.')) {
                            check_event(v, e.span.start, &mut fired);
                        }
                    }
                }
            });
            let mut check = |b: &Block| {
                b.walk(&mut |i, _| {
                    let key = i.key();
                    let lk = key.to_ascii_lowercase();
                    let vals: Vec<(&str, usize)> = match (i.text(), i.block()) {
                        (Some(v), _) => vec![(v, i.span.start)],
                        (None, Some(b)) if lk == "add_ideas" || lk == "remove_ideas" => b.items.iter().filter_map(|x| x.text().map(|t| (t, x.span.start))).collect(),
                        (None, Some(b)) if lk == "add_timed_idea" || lk == "swap_ideas" => {
                            b.items.iter().filter(|x| x.is("idea") || x.is("add_idea") || x.is("remove_idea")).filter_map(|x| x.text().map(|t| (t, x.span.start))).collect()
                        }
                        _ => Vec::new(),
                    };
                    for (v, at) in vals {
                        match lk.as_str() {
                            "add_ideas" | "remove_ideas" | "has_idea" | "idea" | "add_idea" | "remove_idea" | "add_timed_idea" if !c.has(Kind::Idea, v) && !v.starts_with('[') && !v.starts_with('@') => {
                                add(Severity::Warning, "ideas", at, format!("{key} = {v}: no such idea"))
                            }
                            "recruit_character" | "retire_character" | "promote_character" | "has_character" if !c.has(Kind::Character, v) && !v.starts_with('[') && v != "yes" && v != "no" => {
                                add(Severity::Warning, "characters", at, format!("{key} = {v}: no such character"))
                            }
                            "has_completed_focus" | "complete_national_focus" | "unlock_national_focus" | "focus" if !c.has(Kind::Focus, v) && !c.has(Kind::SharedFocus, v) => {
                                add(Severity::Warning, "focus", at, format!("{key} = {v}: no such focus"))
                            }
                            "add_opinion_modifier" | "remove_opinion_modifier" | "has_opinion_modifier" => {}
                            k if STATE_KEYS.contains(&k) => {
                                if let Ok(n) = v.parse::<u32>()
                                    && !c.facts.states.is_empty()
                                    && !c.facts.states.contains(&n)
                                {
                                    add(Severity::Error, "map", at, format!("{key} = {n}"));
                                }
                            }
                            k if TAG_KEYS.contains(&k) && kinds::is_tag(v) && !c.tag_known(v) => {
                                add(Severity::Warning, "tags", at, format!("{key} = {v}: no nation has the fixed tag {v} and the base game has no such tag"));
                            }
                            _ => {}
                        }
                    }
                    if lk == "add_opinion_modifier"
                        && let Some(b) = i.block()
                        && let Some(m) = b.atom("modifier")
                        && !c.has(Kind::OpinionModifier, m)
                    {
                        add(Severity::Warning, "references", i.span.start, format!("opinion modifier {m} is not defined"));
                    }
                });
            };
            // focus trees' `focus = x` are prerequisites, checked with the tree
            if path.starts_with("common/national_focus/") {
                for it in &s.root.items {
                    if let Some(b) = it.block() {
                        for f in b.blocks("focus").chain(std::iter::once(b)) {
                            for k in ["completion_reward", "select_effect", "available", "bypass", "allow_branch"] {
                                if let Some(x) = f.block(k) {
                                    check(x);
                                }
                            }
                        }
                    }
                }
            } else {
                check(&s.root);
            }
        }
    }
    // states that are not on the map: one issue per file
    let mut map: BTreeMap<(String, String), Vec<Issue>> = BTreeMap::new();
    for i in found {
        if i.category == "map" {
            map.entry((i.pkg.clone(), i.file.clone())).or_default().push(i);
        } else {
            c.out.push(i);
        }
    }
    for ((pkg, file), v) in map {
        let shown: Vec<&str> = v.iter().take(8).map(|i| i.message.as_str()).collect();
        let more = if v.len() > 8 { format!(" and {} more", v.len() - 8) } else { String::new() };
        let m = if v.len() == 1 { format!("{}: the map has no such state", v[0].message) } else { format!("{} states that are not on this map: {}{more} (lines {}…)", v.len(), shown.join(", "), v.iter().take(3).map(|i| i.line.to_string()).collect::<Vec<_>>().join(", ")) };
        c.out.push(Issue { severity: Severity::Error, category: "map", pkg, file, line: v[0].line, def: None, message: m });
    }
    // events that nothing fires
    for k in c.st.defs().filter(|k| k.kind == Kind::Event).collect::<Vec<_>>() {
        let Some((d, text, s)) = c.st.def(&k) else { continue };
        let Some(b) = d.block(s) else { continue };
        if b.yes("is_triggered_only") && !fired.contains(&d.id) {
            c.add(Severity::Info, "events", &k.pkg, &k.file, script::line_of(text, d.span.start), Some(&k), format!("event {} is_triggered_only and nothing in the content fires it", d.id));
        }
    }
}

/// The navy of each nation ([`super::navy`]): its ship designs against
/// its research at the start, its naval order of battle (the file
/// set_naval_oob loads), and its fleets against its designs, the kinds of
/// ship and the naval bases of the map.
fn navy(c: &mut Ctx) {
    use super::navy;
    let Some(db) = c.units.clone() else { return };
    let base = c.base;
    let name = |id: &str| base.and_then(|b| b.loc.get(id)).map(|t| super::loc::plain(t)).filter(|t| !t.trim().is_empty()).unwrap_or_else(|| id.to_string());
    let mut found: Vec<Issue> = Vec::new();
    let mut known = super::dlc::known(base.map(|b| &b.files));
    known.extend(db.dlcs());
    for tag in c.st.packages.keys().filter(|t| *t != SHARED) {
        let mut owned = known.clone();
        owned.extend(super::units::history_dlcs(c.st, tag));
        let designs = navy::designs(&db, c.st, tag);
        let lines = navy::naval_oobs(c.st, tag);
        if designs.is_empty() && lines.is_empty() {
            continue;
        }
        let have = db.at_start(c.st, tag, Some(&owned));
        let text_of = |pkg: &str, file: &str| c.st.text(pkg, file).unwrap_or("").to_string();
        for d in &designs {
            let text = text_of(&d.pkg, &d.file);
            for p in db.check_design(d, &have, &designs, tag, &name) {
                found.push(Issue { severity: p.severity, category: "navy", pkg: d.pkg.clone(), file: d.file.clone(), line: script::line_of(&text, d.span.start), def: None, message: format!("ship design \"{}\": {}", d.name, p.message) });
            }
        }
        for l in &lines {
            let text = text_of(tag, &l.file);
            let line = script::line_of(&text, l.span.start);
            let Some((pkg, path)) = navy::oob_file(c.st, tag, &l.name) else {
                found.push(Issue { severity: Severity::Error, category: "navy", pkg: tag.clone(), file: l.file.clone(), line, def: None, message: format!("set_naval_oob = {}: there is no history/units/{}.txt in the content", l.name, l.name) });
                continue;
            };
            let Some(f) = c.st.file(&pkg, &path) else { continue };
            let (Some(s), Some(otext)) = (&f.script, f.text()) else { continue };
            let fleets = navy::read_fleets(&s.root);
            if fleets.is_empty() {
                found.push(Issue { severity: Severity::Warning, category: "navy", pkg: pkg.clone(), file: path.clone(), line: 1, def: None, message: format!("{path}: no fleets (units = {{ fleet = {{ ... }} }})") });
            }
            for p in navy::check_fleets(&db, &fleets, tag, &c.facts.ports, &|t| navy::designs(&db, c.st, t), &name) {
                found.push(Issue { severity: p.severity, category: "navy", pkg: pkg.clone(), file: path.clone(), line: script::line_of(otext, p.span.start), def: None, message: p.message });
            }
        }
    }
    c.out.extend(found);
}

/// `else` / `else_if` after an `if` at the top of a nation's history: the
/// game drops it there (the build moves it inside the `if`, where the game
/// takes it; "repair" does so in the content).
fn loose_elses(c: &mut Ctx) {
    let mut found = Vec::new();
    for (pkg, p) in &c.st.packages {
        for (path, f) in &p.files {
            if !path.starts_with("history/countries/") {
                continue;
            }
            let (Some(s), Some(text)) = (&f.script, f.text()) else { continue };
            for i in super::dlc::loose_elses(&s.root) {
                found.push(Issue {
                    severity: Severity::Warning,
                    category: "history",
                    pkg: pkg.clone(),
                    file: path.clone(),
                    line: script::line_of(text, i.span.start),
                    def: None,
                    message: format!("{LOOSE_ELSE}: the game takes {} only inside the if before it (the build moves it there; \"repair\" does so here)", i.key()),
                });
            }
        }
    }
    c.out.extend(found);
}

/// The army ([`super::army`]): the divisions of each order of battle of the
/// content (templates of their file, places on the map, as the Army page
/// checks them), files of templates nothing loads, and nations that start
/// with no divisions (the build places none).
fn army(c: &mut Ctx) {
    use super::army;
    let loaded = army::loaded_oobs(c.st);
    // which nation loads a file (its divisions should stand in its land)
    let mut loader: BTreeMap<String, String> = BTreeMap::new();
    for tag in c.st.packages.keys().filter(|t| *t != SHARED) {
        for l in army::army_oobs(c.st, tag) {
            loader.entry(l.name).or_insert_with(|| tag.clone());
        }
    }
    let mut found: Vec<Issue> = Vec::new();
    for (pkg, p) in &c.st.packages {
        for (path, f) in &p.files {
            if !path.starts_with("history/units/") {
                continue;
            }
            let (Some(s), Some(text)) = (&f.script, f.text()) else { continue };
            let name = army::oob_name(path);
            let divs = army::read_divisions(&s.root);
            let templates = army::templates_of(&s.root);
            let tag = loader.get(name).cloned().unwrap_or_else(|| pkg.clone());
            for pr in army::check_divisions(&divs, &templates, &c.facts.ground, &tag) {
                let line = pr.span.map(|s| script::line_of(text, s.start)).unwrap_or(1);
                found.push(Issue { severity: pr.severity, category: "army", pkg: pkg.clone(), file: path.clone(), line, def: None, message: pr.message });
            }
            if !templates.is_empty() && !loaded.contains(name) {
                let what = if divs.is_empty() { "templates" } else { "templates and divisions" };
                found.push(Issue {
                    severity: Severity::Warning,
                    category: "army",
                    pkg: pkg.clone(),
                    file: path.clone(),
                    line: 1,
                    def: None,
                    message: format!("{path}: no oob or load_oob loads it, so the game does not have its {what} (load it on the Army page)"),
                });
            }
        }
    }
    for tag in c.st.packages.keys().filter(|t| *t != SHARED) {
        if army::army_oobs(c.st, tag).is_empty() && c.st.package(tag).is_some_and(|p| p.files.keys().any(|f| f.starts_with("history/countries/"))) {
            let file = c.st.package(tag).and_then(|p| p.files.keys().find(|f| f.starts_with("history/countries/")).cloned()).unwrap_or_default();
            found.push(Issue { severity: Severity::Info, category: "army", pkg: tag.clone(), file, line: 1, def: None, message: format!("{tag} starts with no divisions: its history has no oob, and the build places none (the Army page makes them)") });
        }
    }
    c.out.extend(found);
}

/// The air force of each nation ([`super::air`]): its plane designs
/// (checked for a player with By Blood Alone), their fallbacks (for one
/// without it), and the air wings of the files its history loads, against
/// the air bases of the map.
fn air(c: &mut Ctx) {
    use super::air::{self, Branch};
    use super::navy;
    let Some(db) = c.units.clone() else { return };
    let base = c.base;
    let name = |id: &str| base.and_then(|b| b.loc.get(id)).map(|t| super::loc::plain(t)).filter(|t| !t.trim().is_empty()).unwrap_or_else(|| id.to_string());
    let mut found: Vec<Issue> = Vec::new();
    let mut known = super::dlc::known(base.map(|b| &b.files));
    known.extend(db.dlcs());
    for tag in c.st.packages.keys().filter(|t| *t != SHARED) {
        let (ds, fs) = air::planes(&db, c.st, tag);
        let lines = air::air_oobs(c.st, tag);
        if ds.is_empty() && fs.is_empty() && lines.is_empty() {
            continue;
        }
        let mut owned = known.clone();
        owned.extend(super::units::history_dlcs(c.st, tag));
        let have = db.at_start(c.st, tag, Some(&air::with_bba(&owned)));
        let legacy = db.at_start(c.st, tag, Some(&air::without_bba(&owned)));
        let text_of = |pkg: &str, file: &str| c.st.text(pkg, file).unwrap_or("").to_string();
        let mut issue = |severity: Severity, pkg: &str, file: &str, at: usize, message: String| {
            let line = script::line_of(&text_of(pkg, file), at);
            found.push(Issue { severity, category: "air", pkg: pkg.to_string(), file: file.to_string(), line, def: None, message });
        };
        for d in &ds {
            for p in db.check_plane(d, &have, &ds, tag, &name) {
                issue(p.severity, &d.pkg, &d.file, d.span.start, format!("plane design \"{}\": {}", d.name, p.message));
            }
            if air::branch_of(&d.need) == Branch::Bba && !fs.iter().any(|f| f.name == d.name) {
                issue(Severity::Info, &d.pkg, &d.file, d.span.start, format!("plane design \"{}\" has no fallback: players without {} have no plane of that name (the Air page adds one)", d.name, air::BBA));
            }
        }
        for f in &fs {
            let d = ds.iter().find(|d| d.name == f.name);
            for p in db.check_fallback(f, &legacy, d, &fs, tag, &name) {
                issue(p.severity, &f.pkg, &f.file, f.span.start, format!("fallback \"{}\": {}", f.name, p.message));
            }
        }
        let planes_of = |t: &str| air::planes(&db, c.st, t);
        for l in &lines {
            let Some((pkg, path)) = navy::oob_file(c.st, tag, &l.name) else {
                issue(Severity::Error, tag, &l.file, l.span.start, format!("set_air_oob = {}: there is no history/units/{}.txt in the content", l.name, l.name));
                continue;
            };
            let Some(f) = c.st.file(&pkg, &path) else { continue };
            let Some(s) = &f.script else { continue };
            let bases = air::read_wings(&s.root);
            if bases.is_empty() {
                issue(Severity::Warning, &pkg, &path, 0, format!("{path}: no air wings (air_wings = {{ <state> = {{ ... }} }})"));
            }
            let branch = air::branch_of(&l.need);
            if branch == Branch::Both && bases.iter().flat_map(|b| &b.wings).any(|w| db.is_airframe(&w.equipment)) {
                issue(Severity::Warning, tag, &l.file, l.span.start, format!("set_air_oob = {}: loaded for players with and without {}, its wings of the designer's airframes are not for those without it; load a file of planes without modules for them (the Air page splits it)", l.name, air::BBA));
            }
            for p in air::check_wings(&db, &bases, tag, &c.facts.airbases, branch == Branch::Legacy, &planes_of, &name) {
                issue(p.severity, &pkg, &path, p.span.start, p.message);
            }
        }
        if !lines.is_empty() && !lines.iter().any(|l| l.need.holds(&air::without_bba(&owned))) {
            let l = &lines[0];
            issue(Severity::Warning, tag, &l.file, l.span.start, format!("no set_air_oob for players without {}: they start with no air wings (the Air page makes their file)", air::BBA));
        }
    }
    c.out.extend(found);
}

/// Numbers the game does not allow ([`super::limits`]): in the fields of
/// the definitions, in effects and triggers, building levels over the
/// building's cap (the base game's, else vanilla's).
fn values(c: &mut Ctx) {
    use super::limits;
    let caps = match c.base {
        Some(b) => crate::modbuild::buildings::of_files(&b.files),
        None => std::sync::Arc::new(crate::modbuild::buildings::Caps::vanilla()),
    };
    let mut seen: BTreeSet<(String, String, usize)> = BTreeSet::new();
    let mut found: Vec<Issue> = Vec::new();
    let keys: Vec<DefKey> = c.st.defs().collect();
    for k in &keys {
        let Some((d, text, s)) = c.st.def(k) else { continue };
        let Some(b) = d.block(s) else { continue };
        for f in k.kind.spec().fields.iter().filter(|f| f.ty == kinds::FieldType::Number) {
            let Some(r) = limits::of_field(k.kind, f.key) else { continue };
            for a in b.all(f.key).filter_map(|i| i.atom()) {
                let Ok(v) = a.text.parse::<f64>() else { continue };
                if let Err(why) = r.check(v)
                    && seen.insert((k.pkg.clone(), k.file.clone(), a.span.start))
                {
                    let message = format!("{} {}: {} = {}: {why}", k.kind.spec().one, d.id, f.key, a.text);
                    found.push(Issue { severity: Severity::Warning, category: "values", pkg: k.pkg.clone(), file: k.file.clone(), line: script::line_of(text, a.span.start), def: Some(k.clone()), message });
                }
            }
        }
    }
    for (name, p) in &c.st.packages {
        for (path, f) in &p.files {
            let (Some(s), Some(text)) = (&f.script, f.text()) else { continue };
            if !path.ends_with(".txt") {
                continue;
            }
            for pr in limits::script_problems(&s.root, &caps) {
                if !seen.insert((name.clone(), path.clone(), pr.span.start)) {
                    continue;
                }
                // the definition it is in, to go to
                let def = f.defs.iter().enumerate().rfind(|(_, d)| d.span.contains(&pr.span.start)).map(|(i, d)| DefKey {
                    pkg: name.clone(),
                    file: path.clone(),
                    kind: d.kind,
                    id: d.id.clone(),
                    nth: f.defs[..i].iter().filter(|x| x.kind == d.kind && x.id == d.id).count(),
                });
                found.push(Issue { severity: Severity::Warning, category: "values", pkg: name.clone(), file: path.clone(), line: script::line_of(text, pr.span.start), def, message: pr.message });
            }
        }
    }
    c.out.extend(found);
}

/// DLC and the stockpile, through every script of the content with the
/// `has_dlc` checks around each line ([`super::dlc::walk`]):
/// - `has_dlc` of a name that is no DLC (the closest one said);
/// - technologies (`set_technology`) and models of equipment
///   (`add_equipment_to_stockpile`, `create_equipment_variant`) that only
///   players with a DLC (or without one) have, given outside a `has_dlc`
///   check for it: No Step Back's tanks, the old tanks of those without it;
/// - `add_equipment_to_stockpile` without a type or an amount, of what is
///   not equipment, made by an unknown country;
/// - the content's technologies in a research folder that does not exist,
///   or that only some players see.
fn dlcs(c: &mut Ctx) {
    use super::dlc::{self, Need};
    let known = dlc::known(c.base.map(|b| &b.files));
    let db = c.units.clone();
    let mut found: Vec<Issue> = Vec::new();
    for (name, p) in &c.st.packages {
        for (path, f) in &p.files {
            let (Some(s), Some(text)) = (&f.script, f.text()) else { continue };
            if !path.ends_with(".txt") {
                continue;
            }
            let mut add = |sev: Severity, cat: &'static str, at: usize, m: String| {
                found.push(Issue { severity: sev, category: cat, pkg: name.clone(), file: path.clone(), line: script::line_of(text, at), def: None, message: m });
            };
            let tags = |t: &str| c.tag_known(t);
            dlc::walk(&s.root, &Need::default(), &mut |i, need| {
                if i.is("has_dlc")
                    && let Some(v) = i.text()
                    && !known.contains(v)
                    && !v.starts_with('[')
                {
                    match dlc::closest(v, &known) {
                        Some(k) => add(Severity::Error, "dlc", i.span.start, format!("has_dlc = \"{v}\": no DLC is called that; did you mean \"{k}\"? (has_dlc takes the name exactly)")),
                        None => add(Severity::Warning, "dlc", i.span.start, format!("has_dlc = \"{v}\" is not a DLC the game names nor one installed: a typo? (has_dlc takes the name exactly, as \"No Step Back\")")),
                    }
                }
                let Some(db) = &db else { return };
                // what only some players have, given to all
                let mut given = |id: &str, at: usize, what: &str| {
                    let n = db.need_of(id);
                    if !n.is_empty() && !n.within(need) {
                        let wher = if what == "technology" {
                            let folders = db.techs.get(id).map(|t| t.folders.join(", ")).unwrap_or_default();
                            format!("is in the research tree only for players {} (folder {folders})", n.describe())
                        } else {
                            format!("is a model only players {} get", n.describe())
                        };
                        add(Severity::Warning, "dlc", at, format!("{what} {id} {wher}: give it inside if = {{ limit = {{ {} }} }}, and the others theirs in an else, as the game's history does", n.trigger()));
                    }
                };
                if i.is("set_technology")
                    && let Some(b) = i.block()
                {
                    for t in &b.items {
                        if let Some(k) = t.key.as_ref().filter(|k| k.text != "popup") {
                            given(&k.text, t.span.start, "technology");
                        }
                    }
                }
                if (i.is("add_equipment_to_stockpile") || i.is("create_equipment_variant"))
                    && let Some(b) = i.block()
                    && let Some(ty) = b.get("type")
                    && let Some(t) = ty.text()
                {
                    match db.equipment.get(t) {
                        None if !t.starts_with('[') && !t.contains(':') => add(Severity::Error, "equipment", ty.span.start, format!("{} type = {t}: not equipment of the game or the mod", i.key())),
                        Some(_) => given(t, ty.span.start, "equipment"),
                        _ => {}
                    }
                }
                if i.is("add_equipment_to_stockpile")
                    && let Some(b) = i.block()
                {
                    if b.get("type").is_none() {
                        add(Severity::Error, "equipment", i.span.start, "add_equipment_to_stockpile without type = <model of equipment>".into());
                    }
                    match b.get("amount") {
                        None => add(Severity::Error, "equipment", i.span.start, "add_equipment_to_stockpile without amount = <how many>".into()),
                        Some(a) if a.block().is_some() => add(Severity::Error, "equipment", a.span.start, "add_equipment_to_stockpile: amount is a number".into()),
                        _ => {}
                    }
                    if let Some(p) = b.atom("producer")
                        && super::kinds::is_tag(p)
                        && !tags(p)
                    {
                        add(Severity::Warning, "tags", i.span.start, format!("add_equipment_to_stockpile: producer = {p}: no nation has the fixed tag {p} and the base game has no such tag"));
                    }
                }
            });
        }
    }
    // the content's technologies: where they show
    if let Some(db) = &db
        && !db.folders.is_empty()
    {
        for k in c.st.defs().filter(|k| k.kind == Kind::Technology).collect::<Vec<_>>() {
            let Some((d, text, sc)) = c.st.def(&k) else { continue };
            let Some(b) = d.block(sc) else { continue };
            let line = script::line_of(text, d.span.start);
            let folders: Vec<&str> = b.blocks("folder").filter_map(|f| f.atom("name")).collect();
            for f in &folders {
                if !db.folders.contains_key(*f) {
                    found.push(Issue { severity: Severity::Warning, category: "technologies", pkg: k.pkg.clone(), file: k.file.clone(), line, def: Some(k.clone()), message: format!("technology {}: folder {f} is no research folder of the game or the mod (common/technology_tags): it shows nowhere", d.id) });
                }
            }
            let need = db.tech_need(&d.id);
            if !need.is_empty() && !folders.is_empty() {
                found.push(Issue {
                    severity: Severity::Info,
                    category: "dlc",
                    pkg: k.pkg.clone(),
                    file: k.file.clone(),
                    line,
                    def: Some(k.clone()),
                    message: format!("technology {} shows only for players {} (folder {})", d.id, need.describe(), folders.join(", ")),
                });
            }
        }
    }
    c.out.extend(found);
}

/// Counts by severity.
pub fn summary(issues: &[Issue]) -> (usize, usize, usize) {
    let n = |s| issues.iter().filter(|i| i.severity == s).count();
    (n(Severity::Error), n(Severity::Warning), n(Severity::Info))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::loc;

    /// Division templates: battalions the nation cannot use at the start are
    /// errors until its history gives the technology.
    #[test]
    fn division_templates_use_what_the_nation_has() {
        use crate::content::source::{self, Files, Origin};
        let files: std::collections::BTreeMap<String, std::sync::Arc<Vec<u8>>> = [
            ("common/units/infantry.txt", "sub_units = {\n\tinfantry = { group = infantry active = yes combat_width = 2 need = { infantry_equipment = 100 } }\n\tmotorized = { group = mobile active = no combat_width = 2 need = { infantry_equipment = 100 } }\n}\n"),
            ("common/units/equipment/infantry.txt", "equipments = {\n\tinfantry_equipment = { is_archetype = yes active = yes soft_attack = 3 }\n\tinfantry_equipment_0 = { archetype = infantry_equipment active = yes }\n}\n"),
            ("common/technologies/infantry.txt", "technologies = {\n\tmotorised_infantry = { folder = { name = infantry_folder } start_year = 1938 enable_subunits = { motorized } }\n}\n"),
            ("localisation/english/units_l_english.yml", "\u{feff}l_english:\n motorized: \"Motorized Infantry\"\n motorised_infantry: \"Motorised Infantry\"\n"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), std::sync::Arc::new(v.as_bytes().to_vec())))
        .collect();
        let base = source::index("base", Origin::Base, Files::Memory(std::sync::Arc::new(files.into_iter().collect())), &|_| {}).unwrap();
        let mut st = Store::empty(std::env::temp_dir().join("mh-validate-divisions"));
        st.begin("x");
        st.set_text("TYR", "history/units/TYR_army.txt", "division_template = {\n\tname = \"Mot\"\n\tpriority = 5\n\tregiments = {\n\t\tinfantry = { x = 0 y = 0 }\n\t\tmotorized = { x = 1 y = 0 }\n\t}\n}\n".into());
        st.commit();
        let facts = Facts { nations: [("TYR".to_string(), ("Tirol".to_string(), 1, [1].into()))].into(), provinces: 10, states: [1].into(), unpinned: 0, ..Default::default() };
        let msgs = |st: &Store| -> Vec<String> { validate(st, Some(&base), &facts).into_iter().filter(|i| i.category == "divisions").map(|i| format!("{}:{} {}", i.file, i.line, i.message)).collect() };
        assert_eq!(
            msgs(&st),
            vec![
                "history/units/TYR_army.txt:1 division template Mot: priority is 0, 1 or 2 (reinforcement: low, normal, high)".to_string(),
                "history/units/TYR_army.txt:6 division template Mot: Motorized Infantry is not available to TYR at the start: it comes with Motorised Infantry (give it in the nation's history: set_technology = { ... })".to_string(),
            ]
        );
        st.begin("tech");
        st.set_text("TYR", "history/units/TYR_army.txt", st.text("TYR", "history/units/TYR_army.txt").unwrap().replace("priority = 5", "priority = 2"));
        st.set_text("TYR", "history/countries/TYR.txt", "set_technology = { motorised_infantry = 1 }\n".into());
        st.commit();
        assert_eq!(msgs(&st), Vec::<String>::new());
    }

    /// DLC: technologies and models only some players have, given to all;
    /// names has_dlc does not know; the stockpile; templates checked for
    /// players with and without the DLC the history checks.
    #[test]
    fn numbers_the_game_allows() {
        let mut st = Store::empty(std::env::temp_dir().join("mh-validate-values"));
        st.begin("x");
        st.set_text("TYR", "history/countries/TYR.txt", "set_stability = 1.5\nset_research_slots = 2.5\nset_war_support = 0.3\n1 = {\n\tadd_building_construction = { type = infrastructure level = 8 instant_build = yes }\n}\n".into());
        st.set_text("TYR", "common/decisions/tyr.txt", "TYR_cat = {\n\tTYR_build = {\n\t\tcost = -5\n\t\tcomplete_effect = { add_stability = 5 }\n\t}\n}\n".into());
        st.set_text("TYR", "common/ideas/tyr.txt", "ideas = {\n\tcountry = {\n\t\tTYR_spirit = { removal_cost = -1 }\n\t\tTYR_law = { removal_cost = -2 cost = @law_cost }\n\t}\n}\n".into());
        st.commit();
        let facts = Facts { nations: [("TYR".to_string(), ("Tirol".to_string(), 1, [1].into()))].into(), provinces: 10, states: [1].into(), unpinned: 0, ..Default::default() };
        let got: Vec<String> = validate(&st, None, &facts).into_iter().filter(|i| i.category == "values").map(|i| format!("{}:{} {} [{}]", i.file, i.line, i.message, i.def.map(|d| d.id).unwrap_or_default())).collect();
        assert_eq!(
            got,
            [
                "common/decisions/tyr.txt:3 decision TYR_build: cost = -5: a number, 0 or more [TYR_build]",
                "common/decisions/tyr.txt:4 add_stability = 5: from -1 to 1 [TYR_build]",
                "common/ideas/tyr.txt:4 idea TYR_law: removal_cost = -2: -1 or a number, 0 or more [TYR_law]",
                "history/countries/TYR.txt:1 country history TYR: set_stability = 1.5: from 0 to 1 [TYR]",
                "history/countries/TYR.txt:2 country history TYR: set_research_slots = 2.5: a whole number, 0 or more [TYR]",
                "history/countries/TYR.txt:5 add_building_construction level = 8: infrastructure goes to 5 at most in a state [TYR]",
            ]
        );
    }

    #[test]
    fn dlc_and_stockpile_checks() {
        use crate::content::source::{self, Files, Origin};
        let files: std::collections::BTreeMap<String, std::sync::Arc<Vec<u8>>> = [
            ("common/technology_tags/00_technology.txt", "technology_folders = {\n\tinfantry_folder = { ledger = army }\n\tarmour_folder = { available = { NOT = { has_dlc = \"No Step Back\" } } }\n\tnsb_armour_folder = { available = { has_dlc = \"No Step Back\" } }\n}\n"),
            ("common/units/armor.txt", "sub_units = {\n\tinfantry = { group = infantry active = yes combat_width = 2 need = { infantry_equipment = 100 } }\n\tlight_armor = { group = armor active = no combat_width = 2 need = { light_tank_chassis = 60 } }\n}\n"),
            (
                "common/units/equipment/tanks.txt",
                "equipments = {\n\tinfantry_equipment = { is_archetype = yes active = yes }\n\tinfantry_equipment_0 = { archetype = infantry_equipment active = yes }\n\tlight_tank_chassis = { is_archetype = yes soft_attack = 5 }\n\tlight_tank_equipment_1 = { archetype = light_tank_chassis year = 1934 }\n\tlight_tank_chassis_1 = { archetype = light_tank_chassis year = 1934 module_slots = inherit }\n}\n",
            ),
            (
                "common/technologies/armor.txt",
                "technologies = {\n\tbasic_light_tank = { folder = { name = armour_folder } enable_subunits = { light_armor } enable_equipments = { light_tank_equipment_1 } }\n\tnsb_light_tank = { folder = { name = nsb_armour_folder } enable_subunits = { light_armor } enable_equipments = { light_tank_chassis_1 } }\n}\n",
            ),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), std::sync::Arc::new(v.as_bytes().to_vec())))
        .collect();
        let base = source::index("base", Origin::Base, Files::Memory(std::sync::Arc::new(files.into_iter().collect())), &|_| {}).unwrap();
        let mut st = Store::empty(std::env::temp_dir().join("mh-validate-dlc"));
        st.begin("x");
        let history = "set_technology = { basic_light_tank = 1 }
if = {
	limit = { has_dlc = \"No Step Back\" }
	set_technology = { nsb_light_tank = 1 }
	add_equipment_to_stockpile = { type = light_tank_chassis_1 amount = 20 }
}
else = {
	add_equipment_to_stockpile = { type = light_tank_equipment_1 amount = 20 }
}
add_equipment_to_stockpile = { type = light_tank_equipment_1 amount = 10 }
add_equipment_to_stockpile = { type = tank_of_dreams amount = 5 producer = ZZZ }
add_equipment_to_stockpile = { type = infantry_equipment_0 }
";
        st.set_text("TYR", "history/countries/TYR.txt", history.into());
        st.set_text("TYR", "history/units/TYR_army.txt", "division_template = {\n\tname = \"Tanks\"\n\tregiments = {\n\t\tlight_armor = { x = 0 y = 0 }\n\t}\n}\n".into());
        st.set_text("TYR", "common/scripted_effects/tyr.txt", "TYR_effect = { if = { limit = { has_dlc = \"No Step back\" } add_political_power = 1 } }\n".into());
        st.set_text("TYR", "common/technologies/tyr.txt", "technologies = {\n\tTYR_tank = { folder = { name = nsb_armour_folder } }\n\tTYR_lost = { folder = { name = no_such_folder } }\n}\n".into());
        st.commit();
        let facts = Facts { nations: [("TYR".to_string(), ("Tirol".to_string(), 1, [1].into()))].into(), provinces: 10, states: [1].into(), unpinned: 0, ..Default::default() };
        let msgs = |st: &Store| -> Vec<String> {
            validate(st, Some(&base), &facts).into_iter().filter(|i| matches!(i.category, "dlc" | "equipment" | "divisions" | "technologies") || i.message.contains("producer")).map(|i| format!("{} {}:{} {}", i.severity.label(), i.file, i.line, i.message)).collect()
        };
        let m = msgs(&st);
        let want = [
            "warning history/countries/TYR.txt:1 technology basic_light_tank is in the research tree only for players without No Step Back (folder armour_folder): give it inside if = { limit = { NOT = { has_dlc = \"No Step Back\" } } }, and the others theirs in an else",
            "warning history/countries/TYR.txt:10 equipment light_tank_equipment_1 is a model only players without No Step Back get",
            "error history/countries/TYR.txt:11 add_equipment_to_stockpile type = tank_of_dreams: not equipment of the game or the mod",
            "warning history/countries/TYR.txt:11 add_equipment_to_stockpile: producer = ZZZ: no nation has the fixed tag ZZZ",
            "error history/countries/TYR.txt:12 add_equipment_to_stockpile without amount",
            "error common/scripted_effects/tyr.txt:1 has_dlc = \"No Step back\": no DLC is called that; did you mean \"No Step Back\"?",
            "note common/technologies/tyr.txt:2 technology TYR_tank shows only for players with No Step Back (folder nsb_armour_folder)",
            "warning common/technologies/tyr.txt:3 technology TYR_lost: folder no_such_folder is no research folder",
        ];
        for w in want {
            assert!(m.iter().any(|x| x.starts_with(w)), "missing {w:?} in\n{}", m.join("\n"));
        }
        assert_eq!(m.len(), want.len(), "the guarded lines are fine, and so is the template:\n{}", m.join("\n"));
        // without the tank technology for the others, the template is for players with No Step Back only
        st.begin("y");
        st.set_text("TYR", "history/countries/TYR.txt", history.replace("set_technology = { basic_light_tank = 1 }\n", ""));
        st.commit();
        let m = msgs(&st);
        assert!(
            m.iter().any(|x| x.starts_with("error history/units/TYR_army.txt:4 division template Tanks: players without No Step Back: light_armor is not available to TYR at the start: it comes with basic_light_tank (only without No Step Back) or nsb_light_tank (only with No Step Back)")),
            "{}",
            m.join("\n")
        );
    }

    #[test]
    fn finds_what_the_game_would_trip_over() {
        let mut st = Store::empty(std::env::temp_dir().join("mh-validate-none"));
        st.begin("x");
        st.set_text(
            "TYR",
            "common/national_focus/tyr.txt",
            "focus_tree = {\n\tid = tyr\n\tcountry = { factor = 0 modifier = { add = 10 tag = TYR } }\n\tfocus = {\n\t\tid = TYR_a\n\t\ticon = GFX_nope\n\t\tx = 0\n\t\ty = 0\n\t\tcompletion_reward = { country_event = tyr.2 add_ideas = TYR_none transfer_state = 999 }\n\t}\n\tfocus = {\n\t\tid = TYR_b\n\t\tx = 0\n\t\ty = 0\n\t\tprerequisite = { focus = TYR_x }\n\t}\n}\n"
                .into(),
        );
        st.set_text("TYR", "events/tyr.txt", "country_event = { id = tyr.1 title = tyr.1.t is_triggered_only = yes }\n".into());
        st.set_text("TYR", "common/decisions/tyr.txt", "no_category = { TYR_d = { icon = x } }\n".into());
        st.set_text("TYR", "history/units/TYR_1936.txt", "division_template = { name = \"Inf\" }\nunits = { division = { location = 70000 division_template = \"Other\" } }\n".into());
        st.set_text("TYR", "history/countries/TYR.txt", "capital = 77\n".into());
        st.set_text("TYR", "localisation/english/tyr_l_english.yml", loc::render("english", &[("TYR_a".into(), "A".into())]));
        st.set_text("TYR", "localisation/english/bad.yml", "l_english:\n".into());
        st.set_text("BAD", "events/x.txt", "x = { \n".into());
        st.commit();
        let facts = Facts { nations: [("TYR".to_string(), ("Tirol".to_string(), 1, [1, 2].into()))].into(), provinces: 100, states: [1, 2].into(), unpinned: 0, ..Default::default() };
        let issues = validate(&st, None, &facts);
        let msgs: Vec<String> = issues.iter().map(|i| format!("{} {}", i.severity.label(), i.message)).collect();
        let want = [
            "error no nation of the map has the tag BAD",
            "error a '{' is never closed",
            "error the game reads only files named",
            "error tree tyr: TYR_b needs TYR_x",
            "warning tree tyr: TYR_a, TYR_b are all at x = 0, y = 0",
            "warning focus TYR_a: icon GFX_nope",
            "warning focus TYR_b has no name",
            "error event tyr.1: the file has no add_namespace = tyr",
            "warning event tyr.1 has no option",
            "error decision TYR_d: category no_category",
            "error capital = 77: the map has no state 77",
            "error location = 70000 is not a province",
            "error a division: its template \"Other\" is not in this file",
            "warning history/units/TYR_1936.txt: no oob or load_oob loads it",
            "warning fires event tyr.2, which is not defined",
            "warning add_ideas = TYR_none: no such idea",
            "error transfer_state = 999: the map has no such state",
            "note event tyr.1 is_triggered_only and nothing",
        ];
        for w in want {
            assert!(msgs.iter().any(|m| m.starts_with(w)), "missing {w:?} in\n{}", msgs.join("\n"));
        }
        let (e, w, _) = summary(&issues);
        assert!(e >= 10 && w >= 5);
    }
}
