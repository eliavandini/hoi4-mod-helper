//! Bringing content from a source (the base game, a mod, another nation, a
//! bundle) into a nation: [`plan`] works out what comes (the definitions
//! picked, and what they use when asked), what is renamed so it fits (the
//! source's tag becomes the nation's, ids that would clash get the tag in
//! front, event namespaces get the tag's), their localisation, and the
//! sprites and pictures of a mod. [`apply`] writes it into the store as one
//! undo step. [`insert_def`] puts a definition where the game reads it.
//!
//! [`export_bundle`] writes a nation's content to a .zip that imports back
//! (into the same or any other nation, in this project or another).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use super::kinds::{self, Entry, Kind};
use super::loc;
use super::script::{self, Script};
use super::source::{Index, Origin, SDef};

use super::store::{SHARED, Store};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Options {
    /// The source's tag, renamed to the nation's (None: none).
    pub from_tag: Option<String>,
    /// Also bring what the picked definitions use (events they fire, ideas
    /// they add, characters they recruit, ...).
    pub follow: bool,
    /// Ids that would clash get the nation's tag in front (always on for ids
    /// the base game has, which would otherwise be defined twice).
    pub prefix_clashes: bool,
    /// Languages of localisation to bring.
    pub languages: Vec<String>,
    /// Country history: leave out what is about the source's map (capital,
    /// starting divisions, state changes).
    pub strip_map: bool,
    /// Every file of the source, not only its definitions (copying a nation).
    pub all_files: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options { from_tag: None, follow: true, prefix_clashes: true, languages: vec!["english".into()], strip_map: true, all_files: false }
    }
}

/// One definition that comes.
#[derive(Clone, Debug)]
pub struct Item {
    /// Index in the source's definitions.
    pub src: usize,
    pub kind: Kind,
    pub old_id: String,
    pub new_id: String,
    pub group: Option<String>,
    pub include: bool,
    /// "picked", or what uses it.
    pub why: String,
    /// The nation has it already (it is replaced).
    pub replaces: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Plan {
    pub to: String,
    pub items: Vec<Item>,
    /// Words renamed (ids, namespaces).
    pub renames: BTreeMap<String, String>,
    pub namespaces: BTreeMap<String, String>,
    pub from_tag: Option<String>,
    /// Localisation: new key → (old key, text by language).
    pub loc: BTreeMap<String, (String, BTreeMap<String, String>)>,
    /// Sprites that come (old name, new name) and their texture files (source → nation).
    pub sprites: Vec<(String, String)>,
    pub files: Vec<(String, String)>,
    pub notes: Vec<String>,
    pub strip_map: bool,
    /// Ids of the source that stay as they are (not renamed as `renamed_id_x`).
    pub keep: std::collections::HashSet<String>,
    /// Short sprite names (`picture = GER_x`) of sprites the game has: never renamed.
    pub fixed: std::collections::HashSet<String>,
}

/// Kinds followed from what uses them.
const FOLLOW: [Kind; 15] = [
    Kind::Event,
    Kind::Idea,
    Kind::Decision,
    Kind::DecisionCategory,
    Kind::Character,
    Kind::ScriptedEffect,
    Kind::ScriptedTrigger,
    Kind::ScriptedLoc,
    Kind::OpinionModifier,
    Kind::DynamicModifier,
    Kind::StaticModifier,
    Kind::CountryLeaderTrait,
    Kind::UnitLeaderTrait,
    Kind::SharedFocus,
    Kind::AiStrategyPlan,
];

/// Items of country history about the map of the source.
const MAP_KEYS: [&str; 12] = [
    "capital",
    "oob",
    "set_naval_oob",
    "set_air_oob",
    "load_oob",
    "add_state_core",
    "remove_state_core",
    "transfer_state",
    "set_state_owner",
    "set_state_controller",
    "add_state_claim",
    "owns_state",
];

/// `w` with the tag renamed: the tag itself, `TAG_x`, `TAG.x`.
pub fn tag_word(w: &str, from: &str, to: &str) -> Option<String> {
    if w == from {
        return Some(to.to_string());
    }
    let rest = w.strip_prefix(from)?;
    if rest.starts_with('_') || rest.starts_with('.') {
        return Some(format!("{to}{rest}"));
    }
    None
}

/// Words of a text renamed by the plan: ids, then namespaces, then the tag,
/// then keys made from a renamed id (`id_desc`).
fn rename_word(p: &Plan, w: &str) -> Option<String> {
    if let Some(n) = p.renames.get(w) {
        return Some(n.clone());
    }
    if let Some((ns, rest)) = w.split_once('.')
        && let Some(n) = p.namespaces.get(ns)
    {
        return Some(format!("{n}.{rest}"));
    }
    if p.fixed.contains(w) {
        return None;
    }
    if let Some(t) = &p.from_tag
        && let Some(n) = tag_word(w, t, &p.to)
    {
        return Some(n);
    }
    // the longest renamed id the word starts with, followed by _ or .
    // (`id_desc`), unless the word is an id of its own
    if p.keep.contains(w) {
        return None;
    }
    let b = w.as_bytes();
    (1..b.len()).rev().filter(|&i| matches!(b[i], b'_' | b'.')).find_map(|i| p.renames.get(&w[..i]).map(|new| format!("{new}{}", &w[i..])))
}

pub fn rename_text(p: &Plan, text: &str) -> String {
    script::rename_words(text, &|w| rename_word(p, w)).0
}

/// Localisation text: only the tag ([GER.GetName]) and namespaces.
fn rename_loc_text(p: &Plan, text: &str) -> String {
    script::rename_words(text, &|w| {
        if let Some(t) = &p.from_tag
            && let Some(n) = tag_word(w, t, &p.to)
        {
            return Some(n);
        }
        None
    })
    .0
}

/// Texture paths quoted in a text ("gfx/leaders/GER/x.dds").
fn texture_refs(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for part in text.split('"').skip(1).step_by(2) {
        let l = part.to_ascii_lowercase();
        if (l.ends_with(".dds") || l.ends_with(".tga") || l.ends_with(".png")) && !part.contains('\n') {
            out.push(part.replace('\\', "/"));
        }
    }
    out
}

/// The project's ids (all packages) and the nation's own, by kind.
fn project_ids(st: &Store, to: &str) -> (BTreeSet<(Kind, String)>, BTreeSet<(Kind, String)>) {
    let (mut all, mut own) = (BTreeSet::new(), BTreeSet::new());
    for k in st.defs() {
        if k.pkg == to {
            own.insert((k.kind, k.id.clone()));
        } else {
            all.insert((k.kind, k.id.clone()));
        }
    }
    (all, own)
}

/// What importing `picked` (indexes of the source's definitions) into
/// package `to` would do. `base`: the base game, to tell what the game has.
pub fn plan(src: &Index, base: Option<&Index>, st: &Store, picked: &[usize], to: &str, o: &Options) -> Plan {
    let from_base = src.origin == Origin::Base;
    let mut p = Plan { to: to.to_string(), from_tag: o.from_tag.clone().filter(|t| t != to && to != SHARED), strip_map: o.strip_map, ..Default::default() };
    // what comes: the picked ones, then (when asked) what they use
    let mut chosen: Vec<(usize, String)> = picked.iter().map(|&i| (i, "picked".to_string())).collect();
    let mut seen: BTreeSet<usize> = picked.iter().copied().collect();
    let mut texts: HashMap<usize, String> = HashMap::new();
    let def_text = |i: usize, texts: &mut HashMap<usize, String>| -> String { texts.entry(i).or_insert_with(|| src.text_of(&src.defs[i]).unwrap_or_default()).clone() };
    let shared_all: Vec<usize> = (0..src.defs.len()).filter(|&i| src.defs[i].kind == Kind::SharedFocus).collect();
    let mut i = 0;
    while o.follow && i < chosen.len() && chosen.len() < 5000 {
        let (di, _) = chosen[i].clone();
        let d = &src.defs[di];
        let text = def_text(di, &mut texts);
        let add = |j: usize, why: String, chosen: &mut Vec<(usize, String)>, seen: &mut BTreeSet<usize>| {
            if seen.insert(j) {
                chosen.push((j, why));
            }
        };
        for (s, e) in script::words(&text) {
            let w = &text[s..e];
            if w == d.id {
                continue;
            }
            for &k in &FOLLOW {
                let Some(j) = src.find_index(k, w) else { continue };
                let t = &src.defs[j];
                let belongs = !from_base || k == Kind::Event || k == Kind::SharedFocus || p.from_tag.as_ref().is_some_and(|tag| src.tags_of(t).contains(tag));
                let in_base = !from_base && base.is_some_and(|b| b.has(k, w));
                if belongs && !in_base {
                    add(j, format!("used by {}", d.id), &mut chosen, &mut seen);
                }
            }
        }
        // a decision brings its category
        if d.kind == Kind::Decision
            && let Some(g) = &d.group
            && let Some(j) = src.find_index(Kind::DecisionCategory, g)
            && (if from_base { p.from_tag.as_ref().is_some_and(|tag| src.tags_of(&src.defs[j]).contains(tag)) } else { !base.is_some_and(|b| b.has(Kind::DecisionCategory, g)) })
        {
            add(j, format!("category of {}", d.id), &mut chosen, &mut seen);
        }
        // a shared focus brings the shared focuses after it
        if d.kind == Kind::SharedFocus {
            for &j in &shared_all {
                if !seen.contains(&j) {
                    let t = def_text(j, &mut texts);
                    let s = Script::parse(&t);
                    let needs = s.root.items.first().and_then(|x| x.block()).is_some_and(|b| b.blocks("prerequisite").any(|g| g.all("focus").any(|f| f.text() == Some(d.id.as_str()))));
                    if needs {
                        add(j, format!("after {}", d.id), &mut chosen, &mut seen);
                    }
                }
            }
        }
        i += 1;
    }
    if chosen.len() >= 5000 {
        p.notes.push("stopped following at 5000 definitions".into());
    }
    // renames: namespaces of events, then ids that would clash
    let (theirs, own) = project_ids(st, to);
    let low = to.to_ascii_lowercase();
    let tagged = |id: &str| p.from_tag.as_ref().and_then(|t| tag_word(id, t, to));
    for (di, _) in &chosen {
        let d = &src.defs[*di];
        if d.kind == Kind::Event
            && let Some((ns, _)) = d.id.split_once('.')
            && !p.namespaces.contains_key(ns)
        {
            let renamed = p.from_tag.as_ref().and_then(|t| tag_word(ns, &t.to_ascii_lowercase(), &low));
            let clash = from_base || base.is_some_and(|b| b.defs.iter().any(|x| x.kind == Kind::Event && x.group.as_deref() == Some(ns)));
            let new = match renamed {
                Some(r) => r,
                None if clash || o.prefix_clashes && theirs.iter().any(|(k, id)| *k == Kind::Event && id.starts_with(&format!("{ns}."))) => {
                    if ns.starts_with(&format!("{low}_")) { ns.to_string() } else { format!("{low}_{ns}") }
                }
                None => ns.to_string(),
            };
            if new != ns {
                p.namespaces.insert(ns.to_string(), new);
            }
        }
    }
    // the focuses of a tree that comes are renamed with it
    let mut renamed: Vec<usize> = chosen.iter().map(|x| x.0).collect();
    for (di, _) in &chosen {
        let d = &src.defs[*di];
        if d.kind == Kind::FocusTree {
            renamed.extend((0..src.defs.len()).filter(|&j| src.defs[j].kind == Kind::Focus && src.defs[j].file == d.file && src.defs[j].group.as_deref() == Some(d.id.as_str())));
        }
    }
    for di in &renamed {
        let d = &src.defs[*di];
        if d.kind == Kind::Event || d.kind == Kind::CountryHistory {
            continue;
        }
        let after_tag = tagged(&d.id).unwrap_or_else(|| d.id.clone());
        let clash_base = from_base && after_tag == d.id;
        let clash_project = theirs.contains(&(d.kind, after_tag.clone()));
        let new = if (clash_base || clash_project) && (o.prefix_clashes || clash_base) && to != SHARED {
            let n = format!("{to}_{}", after_tag.trim_start_matches(&format!("{to}_")));
            if n == after_tag { format!("{to}_{after_tag}") } else { n }
        } else {
            after_tag.clone()
        };
        if new != d.id && !p.renames.contains_key(&d.id) && tagged(&d.id).as_deref() != Some(new.as_str()) {
            p.renames.insert(d.id.clone(), new);
        }
    }
    // an id renamed that the source also has as something else (a focus and
    // the idea it adds): the other one comes too, renamed the same
    let chosen_set: BTreeSet<usize> = renamed.iter().copied().collect();
    let mut extra = Vec::new();
    for old in p.renames.keys() {
        for &k in &FOLLOW {
            if let Some(j) = src.find_index(k, old)
                && !chosen_set.contains(&j)
                && seen.insert(j)
            {
                extra.push((j, format!("same id as {old}")));
            }
        }
    }
    chosen.extend(extra);
    p.keep = src.defs.iter().filter(|d| !p.renames.contains_key(&d.id)).map(|d| d.id.clone()).collect();
    // the plan's items
    for (di, why) in &chosen {
        let d = &src.defs[*di];
        let new_id = match d.kind {
            Kind::CountryHistory => to.to_string(),
            _ => rename_word(&p, &d.id).unwrap_or_else(|| d.id.clone()),
        };
        let group = d.group.as_ref().map(|g| rename_word(&p, g).unwrap_or_else(|| g.clone()));
        let replaces = own.contains(&(d.kind, new_id.clone()));
        p.items.push(Item { src: *di, kind: d.kind, old_id: d.id.clone(), new_id, group, include: true, why: why.clone(), replaces });
        if d.kind == Kind::Focus && why == "picked" {
            p.notes.push(format!("{} is a focus without its tree: it goes into the nation's first focus tree", d.id));
        }
    }
    // localisation: the keys of each definition and those its text names
    let mut keys: BTreeSet<String> = BTreeSet::new();
    for it in &p.items {
        let text = def_text(it.src, &mut texts);
        let s = Script::parse(&text);
        let b = s.root.items.first().and_then(|x| x.block()).or(if it.kind == Kind::CountryHistory { Some(&s.root) } else { None });
        for (k, _) in kinds::loc_keys(it.kind, &it.old_id, b) {
            keys.insert(k);
        }
        if it.kind == Kind::FocusTree {
            for f in s.root.items.first().and_then(|x| x.block()).into_iter().flat_map(|b| b.blocks("focus")) {
                if let Some(id) = f.atom("id") {
                    for (k, _) in kinds::loc_keys(Kind::Focus, id, Some(f)) {
                        keys.insert(k);
                    }
                }
            }
        }
        for (s, e) in script::words(&text) {
            if src.loc.contains_key(&text[s..e]) {
                keys.insert(text[s..e].to_string());
            }
        }
    }
    let langs: Vec<(String, HashMap<String, String>)> = o.languages.iter().map(|l| (l.clone(), src.loc_in(l))).collect();
    let mut missing = 0;
    for k in keys {
        let mut by_lang = BTreeMap::new();
        for (l, m) in &langs {
            if let Some(t) = m.get(&k) {
                by_lang.insert(l.clone(), rename_loc_text(&p, t));
            }
        }
        if by_lang.is_empty() {
            missing += 1;
            continue;
        }
        let new = rename_word(&p, &k).unwrap_or_else(|| k.clone());
        p.loc.insert(new, (k, by_lang));
    }
    if missing > 0 {
        p.notes.push(format!("{missing} localisation keys the definitions use are not in the source (or in the base game's own files)"));
    }
    // pictures named by a short name of a sprite the game has keep that name
    // (renamed, `GFX_idea_TYR_x` would be defined nowhere)
    for it in &p.items {
        let text = def_text(it.src, &mut texts);
        let s = Script::parse(&text);
        if let Some(b) = s.root.items.first().and_then(|x| x.block()) {
            for (v, n) in kinds::short_sprites_of(it.kind, b) {
                if base.is_some_and(|b| b.sprites.contains_key(&n)) || (from_base && src.sprites.contains_key(&n)) {
                    p.fixed.insert(v);
                }
            }
        }
    }
    // sprites and pictures of a mod (the base game's are in the game)
    if !from_base {
        let mut names: BTreeSet<String> = BTreeSet::new();
        let mut tex: BTreeSet<String> = BTreeSet::new();
        for it in &p.items {
            let text = def_text(it.src, &mut texts);
            let s = Script::parse(&text);
            if let Some(b) = s.root.items.first().and_then(|x| x.block()) {
                for (_, n) in kinds::sprites_of(it.kind, b) {
                    names.insert(n);
                }
                if it.kind == Kind::FocusTree {
                    for f in b.blocks("focus") {
                        if let Some(icon) = f.atom("icon") {
                            names.insert(icon.to_string());
                        }
                    }
                }
            }
            for (s, e) in script::words(&text) {
                if text[s..e].starts_with("GFX_") {
                    names.insert(text[s..e].to_string());
                }
            }
            tex.extend(texture_refs(&text));
        }
        for n in names {
            if base.is_some_and(|b| b.sprites.contains_key(&n)) {
                continue;
            }
            if let Some((t, _)) = src.sprites.get(&n) {
                p.sprites.push((n.clone(), rename_word(&p, &n).unwrap_or(n.clone())));
                tex.insert(t.clone());
            }
        }
        for t in tex {
            if let Some(found) = super::source::texture_path(&src.files, &t) {
                let in_base = base.is_some_and(|b| b.files.exists(&found));
                if !in_base {
                    let dst = p.from_tag.as_ref().map(|tag| replace_path_tag(&found, tag, to)).unwrap_or(found.clone());
                    p.files.push((found, dst));
                }
            }
        }
    }
    if o.all_files {
        let have: BTreeSet<String> = p.files.iter().map(|x| x.0.clone()).collect();
        for f in src.files.all() {
            let l = f.to_ascii_lowercase();
            let script_or_loc = l.ends_with(".txt") || l.ends_with(".yml") || l.ends_with(".gfx");
            if !script_or_loc && !have.contains(&f) {
                let dst = p.from_tag.as_ref().map(|tag| replace_path_tag(&f, tag, to)).unwrap_or(f.clone());
                p.files.push((f, dst));
            }
        }
    }
    p
}

/// A path with the tag renamed in its parts (`gfx/flags/GER.tga`, `GER - Germany.txt`).
pub fn replace_path_tag(path: &str, from: &str, to: &str) -> String {
    path.split('/')
        .map(|part| {
            let (stem, ext) = part.rsplit_once('.').map(|(a, b)| (a, Some(b))).unwrap_or((part, None));
            let mut s = if stem == from {
                to.to_string()
            } else if let Some(rest) = stem.strip_prefix(from).filter(|r| r.starts_with(['_', ' ', '-', '.'])) {
                format!("{to}{rest}")
            } else {
                stem.to_string()
            };
            if let Some(e) = ext {
                s.push('.');
                s.push_str(e);
            }
            s
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Write the plan into the store (one undo step). Returns what was done.
pub fn apply(p: &Plan, src: &Index, st: &mut Store, label: &str) -> Result<String> {
    if p.to.is_empty() {
        bail!("no nation to import into");
    }
    st.begin(label.to_string());
    let r = apply_inner(p, src, st);
    match r {
        Ok(s) => {
            st.commit();
            Ok(s)
        }
        Err(e) => {
            st.rollback();
            Err(e)
        }
    }
}

fn apply_inner(p: &Plan, src: &Index, st: &mut Store) -> Result<String> {
    let included: Vec<&Item> = p.items.iter().filter(|i| i.include).collect();
    // focus trees first, so lone focuses have somewhere to go
    let mut order: Vec<&Item> = included.clone();
    order.sort_by_key(|i| (i.kind != Kind::FocusTree, i.kind));
    let mut texts: HashMap<u32, String> = HashMap::new();
    let mut new_defs = Vec::new();
    for it in order {
        let d = &src.defs[it.src];
        let raw = match texts.get(&d.file) {
            Some(t) => t.clone(),
            None => {
                let Some(t) = src.files.text(src.path(d)) else { bail!("cannot read {}", src.path(d)) };
                texts.insert(d.file, t.clone());
                t
            }
        };
        let text = if d.kind == Kind::CountryHistory {
            let mut t = raw.clone();
            if p.strip_map {
                let s = Script::parse(&t);
                let edits: Vec<script::Edit> = s.root.items.iter().filter(|i| MAP_KEYS.iter().any(|k| i.is(k))).map(|i| script::remove_item(&t, i)).collect();
                t = script::apply(&t, &edits);
            }
            t
        } else {
            source_of(&raw, d)
        };
        new_defs.push(NewDef { kind: it.kind, group: it.group.clone(), id: it.new_id.clone(), text: rename_text(p, &text) });
    }
    let n = new_defs.len();
    // sprites (their texture file where it was copied to)
    let path_map: BTreeMap<&str, &str> = p.files.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    for (old, new) in &p.sprites {
        if let Some(d) = src.find(Kind::Sprite, old)
            && let Some(t) = src.text_of(d)
        {
            let mut t = rename_text(p, &t);
            if let Some((tex, _)) = src.sprites.get(old)
                && let Some(found) = super::source::texture_path(&src.files, tex)
                && let Some(dst) = path_map.get(found.as_str())
            {
                t = t.replace(tex.as_str(), dst);
            }
            new_defs.push(NewDef { kind: Kind::Sprite, group: None, id: new.clone(), text: t });
        }
    }
    insert_defs(st, &p.to, new_defs)?;
    for new in p.namespaces.values() {
        ensure_namespace(st, &p.to, new);
    }
    // localisation, one file per language
    let mut by_lang: BTreeMap<&str, Vec<(String, String)>> = BTreeMap::new();
    for (key, (_, texts)) in &p.loc {
        for (lang, text) in texts {
            by_lang.entry(lang.as_str()).or_default().push((key.clone(), text.clone()));
        }
    }
    for (lang, v) in &by_lang {
        set_locs(st, &p.to, lang, v);
    }
    let mut copied = 0;
    for (from, to) in &p.files {
        if let Some(b) = src.files.read(from) {
            st.set_bytes(&p.to, to, (*b).clone());
            copied += 1;
        }
    }
    Ok(format!(
        "{n} definitions, {} localisation keys, {} sprites and {copied} files into {}{}",
        p.loc.len(),
        p.sprites.len(),
        p.to,
        if p.renames.is_empty() && p.namespaces.is_empty() { String::new() } else { format!(" ({} ids and {} namespaces renamed)", p.renames.len(), p.namespaces.len()) }
    ))
}

/// The text of a source definition, as it would be pasted elsewhere.
fn source_of(raw: &str, d: &SDef) -> String {
    let s = Script::parse(raw);
    match s.root.items.iter().find(|x| x.span == d.span) {
        Some(x) => script::item_source(raw, x),
        None => locate(&s, raw, d).unwrap_or_else(|| raw.get(d.span.clone()).unwrap_or("").to_string()),
    }
}

/// The source text of a nested definition (a focus in its tree, an idea in
/// its category) by its span.
fn locate(s: &Script, raw: &str, d: &SDef) -> Option<String> {
    let mut found = None;
    s.root.walk(&mut |i, _| {
        if found.is_none() && i.span == d.span {
            found = Some(script::item_source(raw, i));
        }
    });
    found
}

/// `add_namespace = ns` at the top of the nation's event file of that namespace.
fn ensure_namespace(st: &mut Store, pkg: &str, ns: &str) {
    for (path, f) in st.package(pkg).map(|p| p.files.iter().collect::<Vec<_>>()).unwrap_or_default() {
        if path.starts_with("events/") && f.script.as_ref().is_some_and(|s| s.root.all("add_namespace").any(|i| i.text() == Some(ns))) {
            return;
        }
    }
    let file = kinds::default_file(Kind::Event, pkg, Some(ns));
    st.edit_text(pkg, &file, |t| format!("add_namespace = {ns}\n{t}"));
}

/// The nation's localisation file of a language.
pub fn loc_file(pkg: &str, lang: &str) -> String {
    format!("localisation/{lang}/{}_l_{lang}.yml", pkg.to_ascii_lowercase())
}

/// Set a localisation key of a package: where the package has it, else in
/// its own file of that language.
pub fn set_loc(st: &mut Store, pkg: &str, lang: &str, key: &str, text: &str) {
    set_locs(st, pkg, lang, &[(key.to_string(), text.to_string())]);
}

/// [`set_loc`] of many keys (each file read and written once).
pub fn set_locs(st: &mut Store, pkg: &str, lang: &str, keys: &[(String, String)]) {
    let suffix = format!("_l_{lang}.yml");
    // where each key is now
    let mut at: BTreeMap<String, String> = BTreeMap::new();
    if let Some(p) = st.package(pkg) {
        for (path, f) in &p.files {
            if path.to_ascii_lowercase().ends_with(&suffix)
                && let Some(l) = &f.loc
            {
                for e in &l.entries {
                    at.entry(e.key.clone()).or_insert_with(|| path.clone());
                }
            }
        }
    }
    let own = loc_file(pkg, lang);
    let mut per_file: BTreeMap<String, Vec<&(String, String)>> = BTreeMap::new();
    for kv in keys {
        per_file.entry(at.get(&kv.0).cloned().unwrap_or_else(|| own.clone())).or_default().push(kv);
    }
    for (file, kvs) in per_file {
        st.edit_text(pkg, &file, |t| {
            let f = loc::parse(t);
            let lines: HashMap<&str, &loc::Entry> = f.entries.iter().map(|e| (e.key.as_str(), e)).collect();
            let mut edits = Vec::new();
            let mut add = String::new();
            for (k, v) in kvs {
                match lines.get(k.as_str()) {
                    Some(e) => edits.push(script::Edit { range: e.line.clone(), text: loc::line(k, v).replacen(":0 ", &format!(":{} ", e.version.unwrap_or(0)), 1) }),
                    None => {
                        add.push_str(&loc::line(k, v));
                        add.push('\n');
                    }
                }
            }
            let mut out = script::apply(t, &edits);
            if out.trim().is_empty() {
                out = format!("\u{feff}l_{lang}:\n");
            }
            if !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(&add);
            out
        });
    }
}

/// Remove a localisation key from every file of a package.
pub fn remove_loc(st: &mut Store, pkg: &str, key: &str) {
    let files: Vec<String> = st
        .package(pkg)
        .map(|p| p.files.iter().filter(|(_, f)| f.loc.as_ref().is_some_and(|l| l.entries.iter().any(|e| e.key == key))).map(|(k, _)| k.clone()).collect())
        .unwrap_or_default();
    for f in files {
        st.edit_text(pkg, &f, |t| loc::remove(t, key).unwrap_or_else(|| t.to_string()));
    }
}

/// A definition to put into a package: its item's text.
pub struct NewDef {
    pub kind: Kind,
    pub group: Option<String>,
    pub id: String,
    pub text: String,
}

/// Put a definition (its item's text) into a package where the game reads
/// it: replacing one of the same kind and id, else into the package's file
/// for the kind (made with the blocks around when missing).
pub fn insert_def(st: &mut Store, pkg: &str, kind: Kind, group: Option<&str>, id: &str, text: &str) -> Result<()> {
    insert_defs(st, pkg, vec![NewDef { kind, group: group.map(String::from), id: id.to_string(), text: text.to_string() }])
}

/// [`insert_def`] of many: new ones are grouped by file and block, so each
/// file is read and written once.
pub fn insert_defs(st: &mut Store, pkg: &str, defs: Vec<NewDef>) -> Result<()> {
    // file → block path → (kind, namespace, texts)
    let mut groups: BTreeMap<String, Vec<(Vec<String>, Kind, Option<String>, Vec<String>)>> = BTreeMap::new();
    for d in defs {
        let NewDef { kind, group, id, text } = d;
        let group = group.as_deref();
        // the same definition there already: replaced
        if let Some(k) = st.find_in(pkg, &[kind], &id) {
            let (def, t, s) = st.def(&k).unwrap();
            let new = if def.path.is_empty() {
                text
            } else {
                let item = def.item(s).unwrap();
                script::apply(t, &[script::replace_item(t, item, &text)])
            };
            st.set_text(pkg, &k.file, new);
            continue;
        }
        let spec = kind.spec();
        if spec.entry == Entry::File {
            let file = kinds::default_file(kind, pkg, None);
            st.set_text(pkg, &file, text);
            continue;
        }
        if kind == Kind::Focus {
            // into the package's first tree (or the tree named by the group)
            let tree = group.and_then(|g| st.find_in(pkg, &[Kind::FocusTree], g)).or_else(|| st.defs_of(Some(pkg), Kind::FocusTree).into_iter().next());
            let Some(tk) = tree else { bail!("{id}: a focus needs a focus tree of the nation to go into") };
            let (def, t, s) = st.def(&tk).unwrap();
            let b = def.block(s).unwrap();
            let new = script::apply(t, &[script::insert_item(t, b, &text, None)]);
            st.set_text(pkg, &tk.file, new);
            continue;
        }
        let group_name = match kind {
            Kind::Event => id.split_once('.').map(|x| x.0.to_string()),
            Kind::FocusTree => Some(id.clone()),
            _ => group.map(String::from),
        };
        // the file: one of the package's with this kind (and group), else the default
        let default = kinds::default_file(kind, pkg, group_name.as_deref());
        let file = if kind == Kind::Event || kind == Kind::FocusTree {
            default
        } else {
            st.package(pkg)
                .and_then(|p| {
                    p.files
                        .iter()
                        .find(|(path, f)| kinds::kinds_of_path(path).contains(&kind) && f.defs.iter().any(|d| d.kind == kind && (d.group.as_deref() == group || group.is_none())))
                        .map(|(k, _)| k.clone())
                })
                .unwrap_or(default)
        };
        let path: Vec<String> = spec.path.iter().map(|p| if *p == "*" { group.unwrap_or("country").to_string() } else { p.to_string() }).collect();
        let ns = if kind == Kind::Event { group_name.clone() } else { None };
        let v = groups.entry(file).or_default();
        match v.iter_mut().find(|(p, k, n, _)| *p == path && *k == kind && *n == ns) {
            Some(g) => g.3.push(text),
            None => v.push((path, kind, ns, vec![text])),
        }
    }
    for (file, blocks) in groups {
        let (k0, g0) = (blocks[0].1, blocks[0].0.last().cloned());
        let mut cur = st.text(pkg, &file).map(str::to_string).unwrap_or_default();
        if cur.trim().is_empty() {
            cur = kinds::file_skeleton(k0, if k0 == Kind::Event { blocks[0].2.as_deref() } else { g0.as_deref() });
        }
        for (path, kind, ns, texts) in blocks {
            // the blocks it goes into, made when missing
            let mut idx: Vec<usize> = Vec::new();
            for key in &path {
                let s = Script::parse(&cur);
                let parent = block_at(&s.root, &idx);
                match parent.items.iter().position(|i| i.is(key) && i.block().is_some()) {
                    Some(i) => idx.push(i),
                    None => {
                        let e = script::insert_item(&cur, parent, &format!("{key} = {{\n}}"), None);
                        cur = script::apply(&cur, &[e]);
                        let s = Script::parse(&cur);
                        let i = block_at(&s.root, &idx).items.iter().rposition(|i| i.is(key)).expect("just added");
                        idx.push(i);
                    }
                }
            }
            let s = Script::parse(&cur);
            let parent = block_at(&s.root, &idx);
            let joined = texts.iter().map(|t| t.trim_end()).collect::<Vec<_>>().join(if path.is_empty() { "\n\n" } else { "\n" });
            let e = script::insert_item(&cur, parent, &joined, None);
            cur = script::apply(&cur, &[e]);
            if kind == Kind::Event
                && let Some(ns) = ns
                && !Script::parse(&cur).root.all("add_namespace").any(|i| i.text() == Some(ns.as_str()))
            {
                cur = format!("add_namespace = {ns}\n{cur}");
            }
        }
        st.set_text(pkg, &file, cur);
    }
    Ok(())
}

fn block_at<'a>(root: &'a script::Block, path: &[usize]) -> &'a script::Block {
    let mut b = root;
    for &i in path {
        b = b.items[i].block().expect("a block");
    }
    b
}

/// A nation's content as a bundle: `manifest.json` and its files under `files/`.
pub fn export_bundle(st: &Store, pkg: &str, name: &str) -> Result<Vec<u8>> {
    let files = st.package_bytes(pkg);
    if files.is_empty() {
        bail!("{pkg} has no content");
    }
    let manifest = serde_json::json!({
        "format": "mod-helper nation content",
        "version": 1,
        "tag": pkg,
        "name": name,
        "files": files.len(),
    });
    let mut v = vec![("manifest.json".to_string(), serde_json::to_vec_pretty(&manifest)?)];
    v.extend(files.into_iter().map(|(p, b)| (format!("files/{p}"), (*b).clone())));
    super::zip::write(&v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::source::{self, Files};
    use std::sync::Arc;

    fn mem(files: &[(&str, &str)]) -> Files {
        Files::Memory(Arc::new(files.iter().map(|(k, v)| (k.to_string(), Arc::new(v.as_bytes().to_vec()))).collect()))
    }

    const GER: &[(&str, &str)] = &[
        (
            "common/national_focus/germany.txt",
            "focus_tree = {\n\tid = german_focus\n\tcountry = { factor = 0 modifier = { add = 10 tag = GER } }\n\tfocus = {\n\t\tid = GER_rhineland\n\t\ticon = GFX_goal_rhineland\n\t\tx = 0 y = 0\n\t\tcompletion_reward = {\n\t\t\tcountry_event = germany.1\n\t\t\tadd_ideas = GER_spirit\n\t\t\trecruit_character = GER_hans\n\t\t\tcustom_effect_tooltip = GER_tt\n\t\t}\n\t}\n\tfocus = {\n\t\tid = army_effort\n\t\tprerequisite = { focus = GER_rhineland }\n\t\tx = 1 y = 1\n\t}\n}\n",
        ),
        ("events/germany.txt", "add_namespace = germany\ncountry_event = {\n\tid = germany.1\n\ttitle = germany.1.t\n\toption = { name = germany.1.a country_event = { id = germany.2 } }\n}\ncountry_event = { id = germany.2 }\n"),
        ("common/ideas/GER.txt", "ideas = {\n\tcountry = {\n\t\tGER_spirit = {\n\t\t\tpicture = spirit\n\t\t\tallowed = { original_tag = GER }\n\t\t}\n\t\tGER_other = { }\n\t}\n}\n"),
        ("common/characters/GER.txt", "characters = {\n\tGER_hans = {\n\t\tname = GER_hans\n\t\tportraits = { civilian = { large = \"gfx/leaders/GER/hans.dds\" } }\n\t}\n}\n"),
        ("interface/goals.gfx", "spriteTypes = {\n\tspriteType = { name = \"GFX_goal_rhineland\" texturefile = \"gfx/interface/goals/rhine.dds\" }\n\tspriteType = { name = \"GFX_idea_spirit\" texturefile = \"gfx/interface/ideas/spirit.dds\" }\n}\n"),
        ("gfx/interface/goals/rhine.dds", "DDS rhine"),
        ("gfx/interface/ideas/spirit.dds", "DDS spirit"),
        ("gfx/leaders/GER/hans.dds", "DDS hans"),
        ("localisation/english/ger_l_english.yml", "\u{feff}l_english:\n GER_rhineland:0 \"Rhineland\"\n GER_rhineland_desc:0 \"Take it [GER.GetName]\"\n army_effort:0 \"Army\"\n army_effort_desc:0 \"Army desc\"\n germany.1.t:0 \"Title\"\n germany.1.a:0 \"OK\"\n GER_spirit:0 \"Spirit\"\n GER_hans:0 \"Hans\"\n GER_tt:0 \"Tooltip\"\n"),
        ("history/countries/GER - Germany.txt", "capital = 64\noob = \"GER_1936\"\nset_research_slots = 4\nrecruit_character = GER_hans\n"),
    ];

    #[test]
    fn imports_a_tree_with_what_it_uses() {
        let src = source::index("mod", Origin::Mod("Test".into()), mem(GER), &|_| {}).unwrap();
        let mut st = Store::empty(std::env::temp_dir().join("mh-import-none"));
        let tree = src.defs.iter().position(|d| d.kind == Kind::FocusTree).unwrap();
        let o = Options { from_tag: Some("GER".into()), ..Default::default() };
        let p = plan(&src, None, &st, &[tree], "TYR", &o);
        let ids: Vec<(&str, &str)> = p.items.iter().map(|i| (i.old_id.as_str(), i.new_id.as_str())).collect();
        assert_eq!(ids, [("german_focus", "german_focus"), ("germany.1", "germany.1"), ("GER_spirit", "TYR_spirit"), ("GER_hans", "TYR_hans"), ("germany.2", "germany.2")]);
        assert!(p.namespaces.is_empty(), "nothing has the namespace: it stays");
        assert!(p.loc.contains_key("TYR_rhineland") && p.loc.contains_key("germany.1.t") && p.loc.contains_key("TYR_tt") && p.loc.contains_key("army_effort_desc"));
        assert_eq!(p.loc["TYR_rhineland_desc"].1["english"], "Take it [TYR.GetName]");
        assert_eq!(p.sprites.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(), ["GFX_goal_rhineland", "GFX_idea_spirit"]);
        assert!(p.files.contains(&("gfx/leaders/GER/hans.dds".into(), "gfx/leaders/TYR/hans.dds".into())));
        let msg = apply(&p, &src, &mut st, "import").unwrap();
        assert!(msg.starts_with("5 definitions"), "{msg}");
        let tree_file = st.text("TYR", "common/national_focus/tyr_german_focus.txt").unwrap();
        assert!(tree_file.contains("tag = TYR") && tree_file.contains("country_event = germany.1") && tree_file.contains("add_ideas = TYR_spirit") && tree_file.contains("focus = TYR_rhineland"), "{tree_file}");
        let ev = st.text("TYR", "events/tyr_germany.txt").unwrap();
        assert!(ev.starts_with("add_namespace = germany\n"), "{ev}");
        assert!(ev.contains("id = germany.1") && ev.contains("id = germany.2") && ev.contains("name = germany.1.a"));
        assert_eq!(st.defs_of(Some("TYR"), Kind::Event).len(), 2);
        let ideas = st.text("TYR", "common/ideas/tyr_idea.txt").unwrap();
        assert!(ideas.contains("ideas = {\n\tcountry = {\n\t\tTYR_spirit = {") && ideas.contains("original_tag = TYR"), "{ideas}");
        assert!(st.text("TYR", "common/characters/TYR.txt").unwrap().contains("large = \"gfx/leaders/TYR/hans.dds\""));
        assert!(st.text("TYR", "interface/tyr_content.gfx").unwrap().contains("GFX_goal_rhineland"));
        assert!(st.file("TYR", "gfx/interface/goals/rhine.dds").is_some());
        let l = st.text("TYR", "localisation/english/tyr_l_english.yml").unwrap();
        assert!(l.contains(" TYR_rhineland:0 \"Rhineland\"") && l.contains(" germany.1.t:0 \"Title\""), "{l}");
        assert!(Script::parse(tree_file).errors.is_empty() && Script::parse(ev).errors.is_empty() && Script::parse(ideas).errors.is_empty());
        // undo takes it all back
        st.undo();
        assert!(st.package("TYR").is_none_or(|p| p.is_empty()));
    }

    #[test]
    fn pictures_of_the_game_keep_their_names() {
        let files: &[(&str, &str)] = &[
            ("common/ideas/GER.txt", "ideas = {\n\tcountry = {\n\t\tGER_trade_ns = {\n\t\t\tpicture = GER_trade\n\t\t}\n\t\tGER_own = {\n\t\t\tpicture = GER_own\n\t\t}\n\t}\n}\n"),
            ("interface/ideas.gfx", "spriteTypes = {\n\tspriteType = { name = \"GFX_idea_GER_trade\" texturefile = \"gfx/interface/ideas/trade.dds\" }\n}\n"),
        ];
        let src = source::index("base", Origin::Base, mem(files), &|_| {}).unwrap();
        let mut st = Store::empty(std::env::temp_dir().join("mh-import-none-pics"));
        let picks: Vec<usize> = src.defs.iter().enumerate().filter(|(_, d)| d.kind == Kind::Idea).map(|(i, _)| i).collect();
        let o = Options { from_tag: Some("GER".into()), ..Default::default() };
        let p = plan(&src, None, &st, &picks, "TYR", &o);
        apply(&p, &src, &mut st, "import").unwrap();
        let ideas = st.text("TYR", "common/ideas/tyr_idea.txt").unwrap();
        // GFX_idea_GER_trade is the game's: renamed, it would be defined nowhere
        assert!(ideas.contains("TYR_trade_ns = {") && ideas.contains("picture = GER_trade\n"), "{ideas}");
        // a picture the game does not have is the nation's to make, with its tag
        assert!(ideas.contains("picture = TYR_own"), "{ideas}");
    }

    #[test]
    fn base_game_ids_get_the_tag_and_history_loses_the_map() {
        let src = source::index("base", Origin::Base, mem(GER), &|_| {}).unwrap();
        let st = Store::empty(std::env::temp_dir().join("mh-import-none2"));
        let tree = src.defs.iter().position(|d| d.kind == Kind::FocusTree).unwrap();
        let o = Options { from_tag: Some("GER".into()), ..Default::default() };
        let p = plan(&src, None, &st, &[tree], "TYR", &o);
        assert_eq!(p.renames.get("german_focus").map(String::as_str), Some("TYR_german_focus"));
        assert_eq!(p.renames.get("army_effort").map(String::as_str), Some("TYR_army_effort"));
        assert!(p.loc.contains_key("TYR_army_effort_desc"));
        assert_eq!(p.namespaces["germany"], "tyr_germany", "the base game has the namespace");
        assert!(p.loc.contains_key("tyr_germany.1.t"));
        assert!(p.sprites.is_empty() && p.files.is_empty(), "the base game's pictures are in the game");
        // GER_other is not used: not followed; from the base game only the tag's things are followed
        assert!(!p.items.iter().any(|i| i.old_id == "GER_other"));
        let hist = src.defs.iter().position(|d| d.kind == Kind::CountryHistory).unwrap();
        let mut st = st;
        let p = plan(&src, None, &st, &[hist], "TYR", &Options { from_tag: Some("GER".into()), follow: false, ..Default::default() });
        apply(&p, &src, &mut st, "import").unwrap();
        let h = st.text("TYR", "history/countries/TYR.txt").unwrap();
        assert!(!h.contains("capital") && !h.contains("oob") && h.contains("recruit_character = TYR_hans") && h.contains("set_research_slots = 4"), "{h}");
    }

    #[test]
    fn copies_between_nations_and_bundles() {
        let mut st = Store::empty(std::env::temp_dir().join("mh-import-none3"));
        st.begin("x");
        st.set_text("TYR", "common/ideas/tyr.txt", "ideas = { country = { TYR_a = { allowed = { original_tag = TYR } } } }\n".into());
        st.set_text("TYR", "localisation/english/tyr_l_english.yml", loc::render("english", &[("TYR_a".into(), "A of [TYR.GetName]".into())]));
        st.set_bytes("TYR", "gfx/flags/TYR.tga", vec![1, 2]);
        st.commit();
        let src = source::of_package(&st, "TYR").unwrap();
        let all: Vec<usize> = (0..src.defs.len()).collect();
        let p = plan(&src, None, &st, &all, "BOZ", &Options { from_tag: Some("TYR".into()), follow: false, all_files: true, ..Default::default() });
        apply(&p, &src, &mut st, "copy").unwrap();
        assert!(st.text("BOZ", "common/ideas/boz_idea.txt").unwrap().contains("BOZ_a = { allowed = { original_tag = BOZ } }"));
        assert!(st.text("BOZ", "localisation/english/boz_l_english.yml").unwrap().contains("BOZ_a:0 \"A of [BOZ.GetName]\""));
        assert_eq!(st.bytes("BOZ", "gfx/flags/BOZ.tga").unwrap().as_slice(), &[1, 2]);
        let z = export_bundle(&st, "TYR", "Tirol").unwrap();
        let path = std::env::temp_dir().join(format!("mh-bundle-{}.zip", std::process::id()));
        std::fs::write(&path, z).unwrap();
        let (ix, man) = source::of_bundle(&path).unwrap();
        assert_eq!(man["tag"], "TYR");
        assert!(ix.has(Kind::Idea, "TYR_a"));
        let _ = std::fs::remove_file(path);
        assert_eq!(replace_path_tag("history/countries/TYR - Tirol.txt", "TYR", "BOZ"), "history/countries/BOZ - Tirol.txt");
    }
}
