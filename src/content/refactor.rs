//! Changes across all the project's content, shown before they are made:
//! renaming an id everywhere it is used (and its localisation keys),
//! deleting a definition (and saying what still names it), find & replace,
//! and renaming a nation's tag.
//!
//! A script word counts only as a whole token, so renaming `my_focus`
//! leaves `my_focus_two` alone and `tyr.1` leaves `tyr.14`. Localisation
//! keys belong to an id by the game's own rules: the id, `_desc`, `_tt`,
//! `_shine`, and for events everything after `id.` (`tyr.1.t`, `tyr.1.a`).

use std::collections::BTreeMap;

use anyhow::{Result, bail};
use regex::Regex;

use super::import;
use super::kinds::Kind;
use super::script::{self, Edit};
use super::store::{DefKey, Store};

/// Localisation key suffixes the game derives from an id.
pub const LOC_SUFFIXES: [&str; 4] = ["", "_desc", "_tt", "_shine"];

/// `key` is one of `id`'s own localisation keys.
pub fn loc_key_of(key: &str, id: &str) -> bool {
    key.strip_prefix(id).is_some_and(|rest| LOC_SUFFIXES.contains(&rest) || (rest.starts_with('.') && rest.len() > 1))
}

/// One changed line of a plan.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub line: usize,
    pub before: String,
    /// None: the line goes.
    pub after: Option<String>,
}

/// What a change does to one file.
#[derive(Clone, Debug, PartialEq)]
pub struct FilePlan {
    pub pkg: String,
    pub file: String,
    pub text: String,
    pub lines: Vec<Line>,
    pub count: usize,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    pub label: String,
    pub files: Vec<FilePlan>,
    /// Places that still name what is deleted (left as they are).
    pub dangling: Vec<(String, String, usize, String)>,
    pub notes: Vec<String>,
}

impl Plan {
    pub fn count(&self) -> usize {
        self.files.iter().map(|f| f.count).sum()
    }
}

/// The lines that differ between two texts (common head and tail left out).
pub fn line_diff(old: &str, new: &str) -> Vec<Line> {
    let (a, b): (Vec<&str>, Vec<&str>) = (old.lines().collect(), new.lines().collect());
    // common head and tail, then the middle line by line
    let head = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let tail = a[head..].iter().rev().zip(b[head..].iter().rev()).take_while(|(x, y)| x == y).count();
    let (am, bm) = (&a[head..a.len() - tail], &b[head..b.len() - tail]);
    let mut out = Vec::new();
    for i in 0..am.len().max(bm.len()) {
        match (am.get(i), bm.get(i)) {
            (Some(x), Some(y)) => out.push(Line { line: head + i + 1, before: x.to_string(), after: Some(y.to_string()) }),
            (Some(x), None) => out.push(Line { line: head + i + 1, before: x.to_string(), after: None }),
            (None, Some(y)) => out.push(Line { line: head + i + 1, before: String::new(), after: Some(y.to_string()) }),
            (None, None) => {}
        }
    }
    out
}

fn file_plan(pkg: &str, file: &str, old: &str, new: String, count: usize) -> Option<FilePlan> {
    (new != old).then(|| FilePlan { pkg: pkg.into(), file: file.into(), lines: line_diff(old, &new), text: new, count })
}

/// Text files of the store: (package, path, text).
fn texts(st: &Store) -> Vec<(String, String, String)> {
    let mut v = Vec::new();
    for (n, p) in &st.packages {
        for (path, f) in &p.files {
            if let Some(t) = f.text() {
                v.push((n.clone(), path.clone(), t.to_string()));
            }
        }
    }
    v
}

fn is_loc(path: &str) -> bool {
    path.to_ascii_lowercase().ends_with(".yml")
}

/// Localisation keys renamed by `map` (key → new key).
fn rename_loc_keys(text: &str, map: &dyn Fn(&str) -> Option<String>) -> (String, usize) {
    let mut n = 0;
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let t = line.trim_start();
        let indent = &line[..line.len() - t.len()];
        if let Some(colon) = t.find(':')
            && !t.starts_with('#')
            && let Some(new) = map(&t[..colon])
        {
            out.push_str(indent);
            out.push_str(&new);
            out.push_str(&t[colon..]);
            n += 1;
        } else {
            out.push_str(line);
        }
    }
    (out, n)
}

/// Renaming `old` to `new` everywhere in the project.
pub fn plan_rename(st: &Store, old: &str, new: &str) -> Result<Plan> {
    if old.is_empty() || new.is_empty() {
        bail!("an id is empty");
    }
    if old == new {
        bail!("the new id is the same");
    }
    if !new.bytes().all(script::is_ident) || new.contains(':') {
        bail!("{new:?} is not an id: letters, digits, _ and .");
    }
    let mut plan = Plan { label: format!("rename {old} to {new}"), ..Default::default() };
    if let Some(k) = st.defs().find(|k| k.id == new) {
        plan.notes.push(format!("{new} is already a {} ({}/{}): both would have the same id", k.kind.spec().one, k.pkg, k.file));
    }
    let event_like = old.contains('.');
    let script_map = |w: &str| -> Option<String> {
        if w == old {
            Some(new.to_string())
        } else if event_like && w.starts_with(old) && w[old.len()..].starts_with('.') {
            Some(format!("{new}{}", &w[old.len()..]))
        } else {
            None
        }
    };
    let loc_map = |k: &str| -> Option<String> { loc_key_of(k, old).then(|| format!("{new}{}", &k[old.len()..])) };
    for (pkg, file, text) in texts(st) {
        if !text.contains(old) {
            continue;
        }
        let (t, n) = if is_loc(&file) {
            let (a, n1) = rename_loc_keys(&text, &loc_map);
            // [old.GetName] and the like in texts
            let (b, n2) = rename_in_loc_texts(&a, &script_map);
            (b, n1 + n2)
        } else {
            script::rename_words(&text, &script_map)
        };
        if let Some(fp) = file_plan(&pkg, &file, &text, t, n) {
            plan.files.push(fp);
        }
    }
    Ok(plan)
}

/// Words inside the quoted text of localisation lines renamed.
fn rename_in_loc_texts(text: &str, map: &dyn Fn(&str) -> Option<String>) -> (String, usize) {
    let mut edits = Vec::new();
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        if let (Some(q0), Some(q1)) = (line.find('"'), line.rfind('"'))
            && q1 > q0
        {
            let inner = &line[q0 + 1..q1];
            for (s, e) in script::words(inner) {
                if let Some(n) = map(&inner[s..e]) {
                    let base = at + q0 + 1;
                    edits.push(Edit { range: base + s..base + e, text: n });
                }
            }
        }
        at += line.len();
    }
    let n = edits.len();
    (script::apply(text, &edits), n)
}

/// Deleting definitions (and their localisation keys); the places that
/// still name them are listed, not changed.
pub fn plan_delete(st: &Store, keys: &[DefKey]) -> Plan {
    let mut plan = Plan { label: if keys.len() == 1 { format!("delete {}", keys[0].id) } else { format!("delete {} definitions", keys.len()) }, ..Default::default() };
    let ids: Vec<&str> = keys.iter().map(|k| k.id.as_str()).collect();
    let mut by_file: BTreeMap<(String, String), Vec<&DefKey>> = BTreeMap::new();
    for k in keys {
        by_file.entry((k.pkg.clone(), k.file.clone())).or_default().push(k);
    }
    let mut removed_spans: BTreeMap<(String, String), Vec<std::ops::Range<usize>>> = BTreeMap::new();
    for ((pkg, file), ks) in &by_file {
        let Some(f) = st.file(pkg, file) else { continue };
        let (Some(text), Some(s)) = (f.text(), f.script.as_ref()) else { continue };
        let mut edits = Vec::new();
        let mut whole = false;
        for k in ks {
            let Some((d, _, _)) = st.def(k) else { continue };
            if d.path.is_empty() {
                whole = true;
            } else if let Some(it) = d.item(s) {
                edits.push(script::remove_item(text, it));
                removed_spans.entry((pkg.clone(), file.clone())).or_default().push(it.span.clone());
            }
        }
        if whole {
            plan.files.push(FilePlan { pkg: pkg.clone(), file: file.clone(), text: String::new(), lines: vec![Line { line: 1, before: format!("(the whole file {file})"), after: None }], count: 1 });
            removed_spans.entry((pkg.clone(), file.clone())).or_default().push(0..text.len());
            continue;
        }
        if let Some(fp) = file_plan(pkg, file, text, script::apply(text, &edits), edits.len()) {
            plan.files.push(fp);
        }
    }
    // localisation keys
    for (pkg, file, text) in texts(st) {
        if !is_loc(&file) || !ids.iter().any(|id| text.contains(id)) {
            continue;
        }
        let base = plan.files.iter().find(|f| f.pkg == pkg && f.file == file).map(|f| f.text.clone()).unwrap_or(text.clone());
        let mut n = 0;
        let kept: String = base
            .split_inclusive('\n')
            .filter(|line| {
                let t = line.trim_start();
                let key = t.split(':').next().unwrap_or("");
                let mine = !t.starts_with('#') && t.contains(':') && ids.iter().any(|id| loc_key_of(key, id));
                n += mine as usize;
                !mine
            })
            .collect();
        if let Some(fp) = file_plan(&pkg, &file, &text, kept, n) {
            plan.files.retain(|f| !(f.pkg == pkg && f.file == file));
            plan.files.push(fp);
        }
    }
    // what else names them
    for (pkg, file, text) in texts(st) {
        if is_loc(&file) {
            continue;
        }
        let gone = removed_spans.get(&(pkg.clone(), file.clone()));
        for (s, e) in script::words(&text) {
            let w = &text[s..e];
            if ids.contains(&w) && !gone.is_some_and(|g| g.iter().any(|r| r.contains(&s))) {
                let line = script::line_of(&text, s);
                let snippet = text[script::line_start(&text, s)..script::line_end(&text, s)].trim().chars().take(160).collect();
                plan.dangling.push((pkg.clone(), file.clone(), line, snippet));
            }
        }
    }
    plan
}

/// How find & replace matches.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Find {
    pub needle: String,
    pub regex: bool,
    pub case: bool,
    pub whole_word: bool,
    /// Only this package (None: all).
    pub pkg: Option<String>,
    /// Script files, localisation files, or both.
    pub scripts: bool,
    pub loc: bool,
}

impl Find {
    fn pattern(&self) -> Result<Regex> {
        let mut p = if self.regex { self.needle.clone() } else { regex::escape(&self.needle) };
        if self.whole_word {
            p = format!(r"(?:^|[^\w.@:'-])({p})(?:$|[^\w.@:'-])");
        } else {
            p = format!("({p})");
        }
        regex::RegexBuilder::new(&p).case_insensitive(!self.case).multi_line(true).build().map_err(|e| anyhow::anyhow!("{e}"))
    }
}

/// A hit: package, file, line, the line's text, and the byte range.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub pkg: String,
    pub file: String,
    pub line: usize,
    pub text: String,
    pub range: std::ops::Range<usize>,
}

pub fn find(st: &Store, q: &Find, limit: usize) -> Result<Vec<Hit>> {
    if q.needle.is_empty() {
        return Ok(Vec::new());
    }
    let re = q.pattern()?;
    let mut out = Vec::new();
    for (pkg, file, text) in texts(st) {
        if q.pkg.as_ref().is_some_and(|p| *p != pkg) || (is_loc(&file) && !q.loc) || (!is_loc(&file) && !q.scripts) {
            continue;
        }
        for c in re.captures_iter(&text) {
            let m = c.get(1).unwrap();
            let s = m.start();
            out.push(Hit {
                pkg: pkg.clone(),
                file: file.clone(),
                line: script::line_of(&text, s),
                text: text[script::line_start(&text, s)..script::line_end(&text, s)].to_string(),
                range: m.range(),
            });
            if out.len() >= limit {
                return Ok(out);
            }
        }
    }
    Ok(out)
}

/// Replacing what `q` finds with `with` (`$1`.. for regex groups).
pub fn plan_replace(st: &Store, q: &Find, with: &str) -> Result<Plan> {
    let hits = find(st, q, usize::MAX)?;
    let re = if q.regex { Some(regex::RegexBuilder::new(&q.needle).case_insensitive(!q.case).multi_line(true).build()?) } else { None };
    let mut by_file: BTreeMap<(String, String), Vec<std::ops::Range<usize>>> = BTreeMap::new();
    for h in hits {
        by_file.entry((h.pkg, h.file)).or_default().push(h.range);
    }
    let mut plan = Plan { label: format!("replace {:?} with {with:?}", q.needle), ..Default::default() };
    for ((pkg, file), ranges) in by_file {
        let text = st.text(&pkg, &file).unwrap_or("");
        let edits: Vec<Edit> = ranges
            .iter()
            .map(|r| {
                let found = &text[r.clone()];
                let t = match &re {
                    Some(re) => re.replace(found, with).into_owned(),
                    None => with.to_string(),
                };
                Edit { range: r.clone(), text: t }
            })
            .collect();
        if let Some(fp) = file_plan(&pkg, &file, text, script::apply(text, &edits), edits.len()) {
            plan.files.push(fp);
        }
    }
    Ok(plan)
}

/// Renaming a nation's tag in its package: the tag in its texts (`TYR`,
/// `TYR_x`, `TYR.x`), its file names, and the package itself.
pub fn plan_tag_rename(st: &Store, from: &str, to: &str) -> Result<(Plan, Vec<(String, String)>)> {
    if !super::kinds::is_tag(to) {
        bail!("{to:?} is not a tag");
    }
    let mut plan = Plan { label: format!("rename the tag {from} to {to}"), ..Default::default() };
    let Some(p) = st.package(from) else { bail!("{from} has no content") };
    let mut moves = Vec::new();
    for (path, f) in &p.files {
        let np = import::replace_path_tag(path, from, to);
        if np != *path {
            moves.push((path.clone(), np.clone()));
        }
        let Some(text) = f.text() else { continue };
        let (t, n) = if is_loc(path) {
            let (a, n1) = rename_loc_keys(text, &|k| import::tag_word(k, from, to));
            let (b, n2) = rename_in_loc_texts(&a, &|w| import::tag_word(w, from, to));
            (b, n1 + n2)
        } else {
            script::rename_words(text, &|w| import::tag_word(w, from, to))
        };
        if let Some(fp) = file_plan(from, path, text, t, n) {
            plan.files.push(fp);
        }
    }
    // other packages that name the tag
    for (pkg, file, text) in texts(st) {
        if pkg == from || is_loc(&file) {
            continue;
        }
        let n = script::words(&text).iter().filter(|(s, e)| &text[*s..*e] == from).count();
        if n > 0 {
            plan.notes.push(format!("{pkg}/{file} names {from} {n} time(s); it is left as it is"));
        }
    }
    if st.package(to).is_some_and(|p| !p.is_empty()) {
        bail!("{to} has content already");
    }
    Ok((plan, moves))
}

/// Make a plan's changes (in the open undo step).
pub fn apply(st: &mut Store, plan: &Plan) {
    for f in &plan.files {
        if f.text.is_empty() && f.lines.first().is_some_and(|l| l.before.starts_with("(the whole file")) {
            st.remove(&f.pkg, &f.file);
        } else {
            st.set_text(&f.pkg, &f.file, f.text.clone());
        }
    }
}

/// Make a tag rename: the texts, the file names, then the package.
pub fn apply_tag_rename(st: &mut Store, plan: &Plan, moves: &[(String, String)], from: &str, to: &str) -> Result<()> {
    apply(st, plan);
    for (a, b) in moves {
        if let Some(bytes) = st.bytes(from, a) {
            match st.text(from, a).map(str::to_string) {
                Some(t) => st.set_text(from, b, t),
                None => st.set_bytes(from, b, (*bytes).clone()),
            }
            st.remove(from, a);
        }
    }
    st.rename_package(from, to)
}

/// Where an id is named in the project (outside its own definition):
/// package, file, line, the line.
pub fn references(st: &Store, id: &str, skip: Option<&DefKey>) -> Vec<(String, String, usize, String)> {
    let skip_span = skip.and_then(|k| st.def(k).map(|(d, _, _)| (k.pkg.clone(), k.file.clone(), d.span.clone())));
    let mut out = Vec::new();
    for (pkg, file, text) in texts(st) {
        if !text.contains(id) {
            continue;
        }
        if is_loc(&file) {
            for (i, line) in text.lines().enumerate() {
                let key = line.trim_start().split(':').next().unwrap_or("");
                if loc_key_of(key, id) {
                    out.push((pkg.clone(), file.clone(), i + 1, line.trim().to_string()));
                }
            }
            continue;
        }
        for (s, e) in script::words(&text) {
            if &text[s..e] == id && !skip_span.as_ref().is_some_and(|(p, f, r)| *p == pkg && *f == file && r.contains(&s)) {
                let line = script::line_of(&text, s);
                out.push((pkg.clone(), file.clone(), line, text[script::line_start(&text, s)..script::line_end(&text, s)].trim().to_string()));
            }
        }
    }
    out
}

/// The kinds a word names in the project.
pub fn kinds_named(st: &Store, id: &str) -> Vec<Kind> {
    let mut v: Vec<Kind> = st.defs().filter(|k| k.id == id).map(|k| k.kind).collect();
    v.dedup();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        let mut st = Store::empty(std::env::temp_dir().join("mh-refactor-none"));
        st.begin("setup");
        st.set_text("TYR", "common/national_focus/tyr.txt", "focus_tree = {\n\tid = tyr\n\tfocus = {\n\t\tid = TYR_a\n\t}\n\tfocus = {\n\t\tid = TYR_a_two\n\t\tprerequisite = { focus = TYR_a }\n\t}\n}\n".into());
        st.set_text("TYR", "events/tyr.txt", "add_namespace = tyr\ncountry_event = {\n\tid = tyr.1\n\ttitle = tyr.1.t\n\toption = { name = tyr.1.a }\n}\ncountry_event = { id = tyr.14 title = tyr.14.t }\n".into());
        st.set_text(
            "TYR",
            "localisation/english/tyr_l_english.yml",
            "l_english:\n TYR_a:0 \"A\"\n TYR_a_desc:0 \"About [TYR_a]\"\n TYR_a_two:0 \"Two\"\n tyr.1.t:0 \"One\"\n tyr.14.t:0 \"Fourteen\"\n".into(),
        );
        st.set_text("shared", "common/scripted_effects/x.txt", "fx = { has_completed_focus = TYR_a TYR = { add_stability = 1 } }\n".into());
        st.commit();
        st
    }

    #[test]
    fn renames_and_deletes() {
        let mut st = store();
        let p = plan_rename(&st, "TYR_a", "TYR_start").unwrap();
        // tree (2), loc (2 keys + 1 in a text), shared effect (1)
        assert_eq!(p.count(), 6, "{p:#?}");
        st.begin("rename");
        apply(&mut st, &p);
        st.commit();
        let t = st.text("TYR", "common/national_focus/tyr.txt").unwrap();
        assert!(t.contains("id = TYR_start") && t.contains("focus = TYR_start") && t.contains("id = TYR_a_two"), "{t}");
        let l = st.text("TYR", "localisation/english/tyr_l_english.yml").unwrap();
        assert!(l.contains(" TYR_start_desc:0 \"About [TYR_start]\"") && l.contains(" TYR_a_two:0"), "{l}");

        let p = plan_rename(&st, "tyr.1", "tyr.2").unwrap();
        st.begin("rename event");
        apply(&mut st, &p);
        st.commit();
        let e = st.text("TYR", "events/tyr.txt").unwrap();
        assert!(e.contains("id = tyr.2\n\ttitle = tyr.2.t") && e.contains("name = tyr.2.a") && e.contains("id = tyr.14 title = tyr.14.t"), "{e}");
        assert!(st.text("TYR", "localisation/english/tyr_l_english.yml").unwrap().contains(" tyr.2.t:0 \"One\"\n tyr.14.t:0"));

        let k = st.find(Kind::Focus, "TYR_start").unwrap();
        let p = plan_delete(&st, &[k]);
        assert_eq!(p.dangling.len(), 2, "{:#?}", p.dangling); // the prerequisite and the shared effect
        st.begin("delete");
        apply(&mut st, &p);
        st.commit();
        assert!(st.find(Kind::Focus, "TYR_start").is_none());
        assert!(!st.text("TYR", "localisation/english/tyr_l_english.yml").unwrap().contains("TYR_start"));

        // find & replace
        let q = Find { needle: "add_stability".into(), scripts: true, loc: true, ..Default::default() };
        assert_eq!(find(&st, &q, 10).unwrap().len(), 1);
        let p = plan_replace(&st, &Find { needle: r"add_(\w+) = 1".into(), regex: true, scripts: true, ..Default::default() }, "add_$1 = 2").unwrap();
        assert_eq!(p.count(), 1);
        assert!(p.files[0].text.contains("add_stability = 2"));
        let ww = Find { needle: "tyr.1".into(), whole_word: true, scripts: true, ..Default::default() };
        assert!(find(&st, &ww, 10).unwrap().is_empty());

        // tag rename
        let (p, moves) = plan_tag_rename(&st, "TYR", "TRL").unwrap();
        assert!(moves.is_empty());
        assert!(p.notes.iter().any(|n| n.contains("shared/")));
        st.begin("tag");
        apply_tag_rename(&mut st, &p, &moves, "TYR", "TRL").unwrap();
        st.commit();
        assert!(st.package("TYR").is_none_or(|p| p.is_empty()));
        assert!(st.text("TRL", "common/national_focus/tyr.txt").unwrap().contains("id = TRL_a_two"));
        assert!(st.text("TRL", "localisation/english/tyr_l_english.yml").unwrap().contains(" TRL_a_two:0"));
        st.undo();
        assert!(st.text("TYR", "common/national_focus/tyr.txt").is_some());
    }
}
