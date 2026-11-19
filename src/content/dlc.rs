//! DLC: the names `has_dlc` takes, what a player needs (or must not have)
//! for a part of the game ([`Need`]: a technology folder shown only with
//! No Step Back, one shown only without it), and the `has_dlc` checks
//! around a line of script ([`walk`]): inside
//! `if = { limit = { has_dlc = "X" } ... }` a line runs only for players
//! with X, in the `else` after it only for those without.
//!
//! A [`Need`] of what the game has comes from its files: technology folders'
//! `available` and technologies' `allow_branch`
//! ([`super::units::UnitDb::tech_need`]); equipment and battalions need what
//! every technology that unlocks them needs.
//!
//! Some DLCs are part of the base game now ([`INTEGRATED`]: Man the Guns):
//! every player has them, so `has_dlc` on them always holds, what is only
//! for players with them is for everyone, and what is only for players
//! without them (the `else` of such a check) runs for no one.

use std::collections::BTreeSet;

use super::script::{Block, Item};
use super::source::Files;

/// The DLCs the game's own files check with `has_dlc` (and the small packs
/// players may have); others installed are read from the game's `dlc/`
/// folder ([`installed`]).
pub const NAMES: [&str; 19] = [
    "Together for Victory",
    "Death or Dishonor",
    "Waking the Tiger",
    "Man the Guns",
    "La Resistance",
    "Battle for the Bosporus",
    "No Step Back",
    "By Blood Alone",
    "Arms Against Tyranny",
    "Trial of Allegiance",
    "Gotterdammerung",
    "Graveyard of Empires",
    "Thunder at Our Gates",
    "Peace For Our Time",
    "No Compromise, No Surrender",
    "Poland: United and Ready",
    "Anniversary Pack",
    "Historical German Portraits",
    "Rocket Launcher Unit Pack",
];

/// The DLCs a player has.
pub type Owned = BTreeSet<String>;

/// DLCs made part of the base game: every player has them.
pub const INTEGRATED: [&str; 1] = ["Man the Guns"];

/// A DLC every player has ([`INTEGRATED`]).
pub fn integrated(name: &str) -> bool {
    INTEGRATED.contains(&name)
}

/// The names of the DLCs installed with the game (`dlc/*/*.dlc`).
pub fn installed(files: &Files) -> Owned {
    let mut out = Owned::new();
    let Files::Layers(roots) = files else { return out };
    for r in roots.iter() {
        let Ok(rd) = std::fs::read_dir(r) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "dlc")
                && let Ok(t) = std::fs::read_to_string(&p)
                && let Some(name) = t.lines().find_map(|l| l.trim().strip_prefix("name").map(|v| v.trim_start().trim_start_matches('=').trim().trim_matches('"').to_string()))
            {
                out.insert(name);
            }
        }
    }
    out
}

/// Every name `has_dlc` may take: [`NAMES`] and those installed.
pub fn known(files: Option<&Files>) -> Owned {
    let mut s: Owned = NAMES.iter().map(|s| s.to_string()).collect();
    if let Some(f) = files {
        s.extend(installed(f));
    }
    s
}

/// The known name closest to a wrong one (case, spaces, a letter or two).
pub fn closest<'a>(name: &str, known: &'a Owned) -> Option<&'a str> {
    let squash = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
    let n = squash(name);
    known.iter().map(|k| (edit_distance(&n, &squash(k)), k.as_str())).filter(|(d, _)| *d <= 3.max(n.len() / 5)).min_by_key(|x| x.0).map(|x| x.1)
}

fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            cur.push((prev[j] + usize::from(ca != *cb)).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// DLCs a player must have and must not have.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Need {
    pub with: BTreeSet<String>,
    pub without: BTreeSet<String>,
}

impl Need {
    pub fn with(name: &str) -> Need {
        Need { with: [name.to_string()].into(), without: BTreeSet::new() }
    }
    pub fn without(name: &str) -> Need {
        Need { with: BTreeSet::new(), without: [name.to_string()].into() }
    }
    pub fn is_empty(&self) -> bool {
        self.with.is_empty() && self.without.is_empty()
    }
    /// A player with `owned` (and the DLCs every player has) has what this
    /// needs.
    pub fn holds(&self, owned: &Owned) -> bool {
        let has = |d: &String| owned.contains(d) || integrated(d);
        self.with.iter().all(has) && !self.without.iter().any(has)
    }
    /// Without the DLCs every player has among those it needs: what it
    /// says about players.
    pub fn for_players(&self) -> Need {
        Need { with: self.with.iter().filter(|d| !integrated(d)).cloned().collect(), without: self.without.clone() }
    }
    /// Both.
    pub fn and(&self, o: &Need) -> Need {
        Need { with: self.with.union(&o.with).cloned().collect(), without: self.without.union(&o.without).cloned().collect() }
    }
    /// What both of two needs need (what holds wherever either does is
    /// not this; this is what they share).
    pub fn common(&self, o: &Need) -> Need {
        Need { with: self.with.intersection(&o.with).cloned().collect(), without: self.without.intersection(&o.without).cloned().collect() }
    }
    /// Wherever `guard` holds, this does too.
    pub fn within(&self, guard: &Need) -> bool {
        self.with.iter().all(|d| guard.with.contains(d) || integrated(d)) && self.without.is_subset(&guard.without)
    }
    /// The opposite of a need of one DLC (None for more: "not (A and B)"
    /// is no need of this shape).
    fn negated(&self) -> Option<Need> {
        match (self.with.len(), self.without.len()) {
            (1, 0) => Some(Need::without(self.with.first()?)),
            (0, 1) => Some(Need::with(self.without.first()?)),
            _ => None,
        }
    }
    /// The DLCs it is about.
    pub fn names(&self) -> impl Iterator<Item = &String> {
        self.with.iter().chain(&self.without)
    }
    /// "with No Step Back", "without Man the Guns", "with A and without B".
    pub fn describe(&self) -> String {
        let mut v = Vec::new();
        if !self.with.is_empty() {
            v.push(format!("with {}", self.with.iter().cloned().collect::<Vec<_>>().join(" and ")));
        }
        if !self.without.is_empty() {
            v.push(format!("without {}", self.without.iter().cloned().collect::<Vec<_>>().join(" or ")));
        }
        v.join(" and ")
    }
    /// The trigger lines of it: `has_dlc = "X" NOT = { has_dlc = "Y" }`.
    pub fn trigger(&self) -> String {
        let mut v: Vec<String> = self.with.iter().map(|d| format!("has_dlc = \"{d}\"")).collect();
        v.extend(self.without.iter().map(|d| format!("NOT = {{ has_dlc = \"{d}\" }}")));
        v.join(" ")
    }
}

/// What a trigger block says about DLC: the DLC checks it holds (all of
/// them must hold: `has_dlc = X`, `NOT = { has_dlc = X }`, those in an
/// `AND`), and whether it holds nothing else.
pub fn of_trigger(b: &Block) -> (Need, bool) {
    let mut need = Need::default();
    let mut pure = true;
    for i in &b.items {
        if i.is("has_dlc") {
            match i.text() {
                Some(d) => {
                    need.with.insert(d.to_string());
                }
                None => pure = false,
            }
        } else if i.is("NOT")
            && let Some(nb) = i.block()
            && nb.items.len() == 1
            && nb.items[0].is("has_dlc")
            && let Some(d) = nb.items[0].text()
        {
            need.without.insert(d.to_string());
        } else if i.is("AND")
            && let Some(ab) = i.block()
        {
            let (n, p) = of_trigger(ab);
            need = need.and(&n);
            pure &= p;
        } else {
            pure = false;
        }
    }
    (need, pure)
}

/// The DLC checks of the triggers that decide whether a whole definition
/// is there or can happen (a focus's `allow_branch`, a decision's
/// `allowed`, ...).
pub fn of_definition(b: &Block) -> Need {
    let mut need = Need::default();
    for k in ["allow_branch", "allowed", "allow", "available", "visible", "potential", "trigger", "limit"] {
        for t in b.blocks(k) {
            need = need.and(&of_trigger(t).0);
        }
    }
    need
}

/// Each item of a block of effects (and those inside) with the DLCs it
/// runs for (`need` and those of the `if` / `else_if` / `else` around it).
pub fn walk(b: &Block, need: &Need, f: &mut dyn FnMut(&Item, &Need)) {
    walk_in(b, need, &|_| true, f, None)
}

/// Like [`walk`], into `if` / `else_if` / `else`, `hidden_effect` and
/// date blocks (`1939.1.1 = { ... }`) only: the lines about the scope
/// itself, not those of another country or state in it.
pub fn walk_flow(b: &Block, need: &Need, f: &mut dyn FnMut(&Item, &Need)) {
    walk_in(b, need, &|i| i.is("hidden_effect") || i.key().starts_with(|c: char| c.is_ascii_digit()), f, None)
}

/// The `else` / `else_if` blocks at the top of a history file (or of a
/// dated block in it) that follow an `if`: the game takes them there only
/// inside the `if` ("Else/else if not following an if"), as its own
/// histories write them: `if = { limit = { ... } a = 1 else = { a = 2 } }`.
pub fn loose_elses(root: &Block) -> Vec<&Item> {
    let mut out = Vec::new();
    let mut scopes = vec![root];
    while let Some(b) = scopes.pop() {
        let mut after_if = false;
        for i in &b.items {
            if (i.is("else") || i.is("else_if")) && after_if {
                out.push(i);
                continue;
            }
            after_if = i.is("if") && i.block().is_some();
            if i.key().starts_with(|c: char| c.is_ascii_digit())
                && let Some(d) = i.block()
            {
                scopes.push(d);
            }
        }
    }
    out.sort_by_key(|i| i.span.start);
    out
}

/// The text with each of [`loose_elses`] moved to the end of the `if` before
/// it: the same branches, written as the game reads them.
pub fn nest_elses(text: &str) -> String {
    use super::script::{self, Script};
    let s = Script::parse(text);
    let mut edits = Vec::new();
    let mut scopes = vec![&s.root];
    while let Some(b) = scopes.pop() {
        let mut run: Option<(&Block, Vec<&Item>)> = None;
        let flush = |run: &mut Option<(&Block, Vec<&Item>)>, edits: &mut Vec<script::Edit>| {
            if let Some((target, items)) = run.take().filter(|(_, v)| !v.is_empty()) {
                // each item's lines as if it stood at the top
                let moved: Vec<String> = items
                    .iter()
                    .map(|i| {
                        let own = script::indent_at(text, i.span.start);
                        script::item_text(text, i).lines().enumerate().map(|(n, l)| if n == 0 { l } else { l.strip_prefix(own).unwrap_or(l) }).collect::<Vec<_>>().join("\n")
                    })
                    .collect();
                edits.push(script::insert_item(text, target, &moved.join("\n"), None));
                edits.extend(items.iter().map(|i| script::remove_item(text, i)));
            }
        };
        for i in &b.items {
            if (i.is("else") || i.is("else_if")) && run.is_some() {
                if let Some((_, v)) = run.as_mut() {
                    v.push(i);
                }
                continue;
            }
            flush(&mut run, &mut edits);
            if i.is("if")
                && let Some(body) = i.block()
            {
                run = Some((body, Vec::new()));
            }
            if i.key().starts_with(|c: char| c.is_ascii_digit())
                && let Some(d) = i.block()
            {
                scopes.push(d);
            }
        }
        flush(&mut run, &mut edits);
    }
    if edits.is_empty() { text.to_string() } else { script::apply(text, &edits) }
}

/// `up`: in the body of an `if`, the need around it and that of its
/// `else` (the game also takes an `else` or `else_if` inside the body of
/// an `if`, not after another `if` there, as the other way of that `if`:
/// `if = { limit = { ... } a = 1 else = { a = 2 } }`).
fn walk_in(b: &Block, need: &Need, descend: &dyn Fn(&Item) -> bool, f: &mut dyn FnMut(&Item, &Need), up: Option<(&Need, Option<Need>)>) {
    // after an `if` / `else_if`: the need around the chain and that of an
    // `else` after it (None: it says nothing about DLC, or more than one
    // each)
    let mut chain: Option<(Need, Option<Need>)> = None;
    for i in &b.items {
        f(i, need);
        let Some(body) = i.block() else {
            chain = None;
            continue;
        };
        let limit = || body.block("limit").map(of_trigger).unwrap_or_default();
        // where an `else_if` / `else` goes on from
        let mut from = || chain.take().or_else(|| up.as_ref().map(|(n, e)| ((*n).clone(), e.clone()))).unwrap_or_else(|| (need.clone(), None));
        if i.is("if") {
            let (n, pure) = limit();
            let other = if pure { n.negated() } else { None };
            walk_in(body, &need.and(&n), descend, f, Some((need, other.clone())));
            chain = Some((need.clone(), other));
        } else if i.is("else_if") {
            let (base, before) = from();
            let (n, pure) = limit();
            let here = base.and(&before.clone().unwrap_or_default()).and(&n);
            let other = match (before, pure.then(|| n.negated()).flatten()) {
                (Some(a), Some(b)) => Some(a.and(&b)),
                _ => None,
            };
            walk_in(body, &here, descend, f, Some((&base, other.clone())));
            chain = Some((base, other));
        } else if i.is("else") {
            let (base, before) = from();
            walk_in(body, &base.and(&before.unwrap_or_default()), descend, f, None);
        } else {
            if descend(i) {
                // a focus with allow_branch = { has_dlc = ... }: its reward too
                walk_in(body, &need.and(&of_definition(body)), descend, f, None);
            }
            chain = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elses_after_an_if_go_inside_it() {
        use crate::content::script::Script;
        let t = "capital = 3\nif = {\n\tlimit = { has_dlc = \"By Blood Alone\" }\n\tset_air_oob = \"A\"\n}\nelse = {\n\tset_air_oob = \"B\"\n}\nset_stability = 0.5\n1939.1.1 = {\n\tif = {\n\t\tlimit = { has_dlc = \"No Step Back\" }\n\t\tx = 1\n\t}\n\telse_if = {\n\t\tlimit = { has_dlc = \"Man the Guns\" }\n\t\tx = 2\n\t}\n\telse = {\n\t\tx = 3\n\t}\n}\n";
        let s = Script::parse(t);
        assert_eq!(loose_elses(&s.root).len(), 3);
        let n = nest_elses(t);
        assert_eq!(
            n,
            "capital = 3\nif = {\n\tlimit = { has_dlc = \"By Blood Alone\" }\n\tset_air_oob = \"A\"\n\telse = {\n\t\tset_air_oob = \"B\"\n\t}\n}\nset_stability = 0.5\n1939.1.1 = {\n\tif = {\n\t\tlimit = { has_dlc = \"No Step Back\" }\n\t\tx = 1\n\t\telse_if = {\n\t\t\tlimit = { has_dlc = \"Man the Guns\" }\n\t\t\tx = 2\n\t\t}\n\t\telse = {\n\t\t\tx = 3\n\t\t}\n\t}\n}\n"
        );
        let s = Script::parse(&n);
        assert!(loose_elses(&s.root).is_empty());
        // the same branches as before
        let needs = |t: &str| {
            let s = Script::parse(t);
            let mut v = Vec::new();
            walk_flow(&s.root, &Need::default(), &mut |i, n| {
                if i.is("set_air_oob") || i.is("x") {
                    v.push((i.text().unwrap_or("").to_string(), n.clone()));
                }
            });
            v
        };
        assert_eq!(needs(t), needs(&n));
        assert_eq!(nest_elses(&n), n, "nothing more to do");
    }
    use crate::content::script::Script;

    fn needs(text: &str, key: &str) -> Vec<String> {
        let s = Script::parse(text);
        let mut out = Vec::new();
        walk(&s.root, &Need::default(), &mut |i, n| {
            if i.is(key) {
                out.push(format!("{} {}", i.text().unwrap_or("{}"), n.describe()).trim().to_string());
            }
        });
        out
    }

    #[test]
    fn guards_follow_if_and_else() {
        let t = r#"
x = a
if = { limit = { has_dlc = "No Step Back" } x = b }
else = { x = c }
if = { limit = { NOT = { has_dlc = "Man the Guns" } } x = d }
else_if = { limit = { has_dlc = "La Resistance" } x = e }
else = { x = f }
if = { limit = { tag = GER has_dlc = "By Blood Alone" } x = g }
else = { x = h }
"#;
        assert_eq!(
            needs(t, "x"),
            [
                "a",
                "b with No Step Back",
                "c without No Step Back",
                "d without Man the Guns",
                "e with La Resistance and Man the Guns",
                "f with Man the Guns and without La Resistance",
                "g with By Blood Alone",
                // not GER, or no By Blood Alone: nothing sure about the DLC
                "h",
            ]
        );
    }

    #[test]
    fn integrated_dlcs_are_everyones() {
        let none = Owned::new();
        assert!(Need::with("Man the Guns").holds(&none));
        assert!(!Need::without("Man the Guns").holds(&none));
        assert!(Need::with("Man the Guns").within(&Need::default()));
        assert!(!Need::with("No Step Back").within(&Need::default()));
        assert!(Need::with("Man the Guns").and(&Need::with("No Step Back")).for_players() == Need::with("No Step Back"));
    }

    #[test]
    fn an_else_inside_the_if() {
        // as vanilla writes the naval OOB of a country
        let t = r#"
if = { limit = { has_dlc = "Man the Guns" } x = a else = { x = b } }
if = { limit = { has_dlc = "No Step Back" } x = c if = { limit = { tag = GER } x = d } else = { x = e } }
if = { limit = { has_dlc = "La Resistance" } else_if = { limit = { has_dlc = "By Blood Alone" } x = f } else = { x = g } }
"#;
        assert_eq!(
            needs(t, "x"),
            [
                "a with Man the Guns",
                "b without Man the Guns",
                "c with No Step Back",
                "d with No Step Back",
                // the else of the `if = { limit = { tag = GER } }` before it
                "e with No Step Back",
                "f with By Blood Alone and without La Resistance",
                "g without By Blood Alone or La Resistance",
            ]
        );
    }

    #[test]
    fn needs_hold_and_cover() {
        let nsb = Need::with("No Step Back");
        let owned: Owned = ["No Step Back".to_string()].into();
        assert!(nsb.holds(&owned) && !nsb.holds(&Owned::new()));
        assert!(Need::without("No Step Back").holds(&Owned::new()));
        assert!(nsb.within(&nsb.and(&Need::with("Man the Guns"))));
        assert!(!nsb.within(&Need::default()));
        assert!(Need::default().within(&nsb));
        assert_eq!(nsb.trigger(), "has_dlc = \"No Step Back\"");
        let (n, pure) = of_trigger(&Script::parse("AND = { has_dlc = \"A\" not = { has_dlc = \"B\" } }").root);
        assert!(pure);
        assert_eq!(n.describe(), "with A and without B");
    }

    #[test]
    fn wrong_names_get_the_closest() {
        let k = known(None);
        assert_eq!(closest("No Step back", &k), Some("No Step Back"));
        assert_eq!(closest("Gotterdämmerung", &k), Some("Gotterdammerung"));
        assert_eq!(closest("Man The Guns", &k), Some("Man the Guns"));
        assert_eq!(closest("Something else entirely", &k), None);
    }
}
