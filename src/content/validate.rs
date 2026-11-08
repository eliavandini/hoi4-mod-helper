//! Checks of the project's content against what the game will do with it:
//! files it would misread, ids it keeps only one of, references to what is
//! not there (focuses, events, ideas, characters, traits, technologies,
//! ideologies, sprites, pictures, localisation), focus trees that cannot
//! be played, and what is about the map (states, provinces, capitals,
//! tags) checked against this map.

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
    /// Number of provinces (ids 1..=n).
    pub provinces: u32,
    pub states: BTreeSet<u32>,
    /// All nations of the map have no tag of their own fixed (they get one at the build).
    pub unpinned: usize,
}

/// Keys whose value is a state id.
const STATE_KEYS: [&str; 10] =
    ["transfer_state", "add_state_core", "remove_state_core", "add_state_claim", "remove_state_claim", "owns_state", "controls_state", "has_full_control_of_state", "is_core_of", "set_capital"];
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
    let mut c = Ctx { st, base, facts, ids, loc, sprites, ideologies, tokens, out: Vec::new() };
    packages(&mut c);
    syntax(&mut c);
    duplicates(&mut c);
    trees(&mut c);
    definitions(&mut c);
    references(&mut c);
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
            Kind::DivisionTemplate => {}
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
    // starting divisions: provinces of the map, templates that are defined
    for (name, p) in &c.st.packages {
        for (path, f) in &p.files {
            if !path.starts_with("history/units/") {
                continue;
            }
            let (Some(s), Some(text)) = (&f.script, f.text()) else { continue };
            let templates: BTreeSet<&str> = s.root.blocks("division_template").filter_map(|b| b.atom("name")).collect();
            let mut found = Vec::new();
            s.root.walk(&mut |i, _| {
                if (i.is("location") || i.is("naval_base") || i.is("base"))
                    && let Some(v) = i.text().and_then(|v| v.parse::<u32>().ok())
                    && (v == 0 || v > c.facts.provinces)
                    && c.facts.provinces > 0
                {
                    found.push((script::line_of(text, i.span.start), format!("{} = {v} is not a province of this map (1 to {})", i.key(), c.facts.provinces)));
                }
                if i.is("division_template")
                    && let Some(t) = i.text()
                    && !templates.contains(t)
                {
                    found.push((script::line_of(text, i.span.start), format!("division_template = \"{t}\" is not defined in this file")));
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

/// Counts by severity.
pub fn summary(issues: &[Issue]) -> (usize, usize, usize) {
    let n = |s| issues.iter().filter(|i| i.severity == s).count();
    (n(Severity::Error), n(Severity::Warning), n(Severity::Info))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::loc;

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
        let facts = Facts { nations: [("TYR".to_string(), ("Tirol".to_string(), 1, [1, 2].into()))].into(), provinces: 100, states: [1, 2].into(), unpinned: 0 };
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
            "error division_template = \"Other\" is not defined",
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
