//! Focus trees: a tree read from its `focus_tree = { ... }` block, where its
//! focuses sit on the grid (x and y, counted from `relative_position_id`
//! when given), the game's rules for which focus can be taken
//! ([`states`]), quick checks ([`check`]), an automatic layout
//! ([`auto_layout`]) and the edits the tree view makes, each a set of
//! [`Edit`]s on the file's text.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use super::kinds::tags_in;
use super::script::{self, Block, Edit, Item, Script, Span};

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Focus {
    pub id: String,
    pub icon: Option<String>,
    /// As written (relative to `rel` when given).
    pub x: i32,
    pub y: i32,
    pub rel: Option<String>,
    pub cost: Option<f64>,
    /// `prerequisite` blocks: all of them, one focus of each.
    pub prereq: Vec<Vec<String>>,
    pub mutex: Vec<String>,
    /// DLCs `allow_branch` checks with has_dlc.
    pub dlc: Vec<String>,
    pub allow_branch: bool,
    /// `text = key`: another localisation key for the name.
    pub text: Option<String>,
    /// A shared focus (from a `shared_focus = { }` block).
    pub shared: bool,
    pub filters: Vec<String>,
    /// Has `offset` blocks (moved when their trigger holds).
    pub offset: bool,
    pub span: Span,
}

#[derive(Clone, Debug, Default)]
pub struct Tree {
    pub id: String,
    /// Tags of its `country` modifiers.
    pub countries: Vec<String>,
    pub default: bool,
    /// `shared_focus = id` lines.
    pub shared: Vec<String>,
    pub focuses: Vec<Focus>,
    pub continuous: Option<(i32, i32)>,
}

fn num(b: &Block, key: &str) -> Option<f64> {
    b.atom(key).and_then(|v| v.parse::<f64>().ok())
}

/// A focus from its block.
pub fn focus_of(b: &Block, span: Span, shared: bool) -> Option<Focus> {
    let id = b.atom("id")?.to_string();
    let prereq = b
        .blocks("prerequisite")
        .map(|p| p.all("focus").filter_map(|i| i.text().map(String::from)).collect::<Vec<_>>())
        .filter(|g| !g.is_empty())
        .collect();
    let mutex = b.blocks("mutually_exclusive").flat_map(|m| m.all("focus").filter_map(|i| i.text().map(String::from)).collect::<Vec<_>>()).collect();
    let mut dlc = Vec::new();
    if let Some(ab) = b.block("allow_branch") {
        ab.walk(&mut |i, _| {
            if i.is("has_dlc")
                && let Some(t) = i.text()
            {
                dlc.push(t.to_string());
            }
        });
    }
    Some(Focus {
        id,
        icon: b.atom("icon").map(String::from),
        x: num(b, "x").unwrap_or(0.0).round() as i32,
        y: num(b, "y").unwrap_or(0.0).round() as i32,
        rel: b.atom("relative_position_id").map(String::from),
        cost: num(b, "cost"),
        prereq,
        mutex,
        dlc,
        allow_branch: b.get("allow_branch").is_some(),
        text: b.atom("text").map(String::from),
        shared,
        filters: b.block("search_filters").map(|s| s.values().map(String::from).collect()).unwrap_or_default(),
        offset: b.get("offset").is_some(),
        span,
    })
}

/// A tree from its block.
pub fn tree_of(b: &Block) -> Tree {
    let focuses = b.items.iter().filter(|i| i.is("focus")).filter_map(|i| focus_of(i.block()?, i.span.clone(), false)).collect();
    Tree {
        id: b.atom("id").unwrap_or("").to_string(),
        countries: b.block("country").map(tags_in).unwrap_or_default(),
        default: b.yes("default"),
        shared: b.all("shared_focus").filter_map(|i| i.text().map(String::from)).collect(),
        focuses,
        continuous: b.block("continuous_focus_position").map(|c| (num(c, "x").unwrap_or(0.0) as i32, num(c, "y").unwrap_or(0.0) as i32)),
    }
}

/// All shared focuses of a parsed file.
pub fn shared_focuses(s: &Script) -> Vec<Focus> {
    s.root.items.iter().filter(|i| i.is("shared_focus")).filter_map(|i| focus_of(i.block()?, i.span.clone(), true)).collect()
}

/// The shared focuses a tree takes in: those it names, and those that need
/// them (the branch below), from all shared focuses there are.
pub fn include_shared(tree: &mut Tree, all: &[Focus]) {
    let mut want: BTreeSet<&str> = tree.shared.iter().map(String::as_str).collect();
    loop {
        let before = want.len();
        for f in all {
            if !want.contains(f.id.as_str()) && f.prereq.iter().flatten().any(|p| want.contains(p.as_str())) {
                want.insert(&f.id);
            }
        }
        if want.len() == before {
            break;
        }
    }
    let have: BTreeSet<String> = tree.focuses.iter().map(|f| f.id.clone()).collect();
    for f in all {
        if want.contains(f.id.as_str()) && !have.contains(&f.id) {
            tree.focuses.push(f.clone());
        }
    }
}

impl Tree {
    pub fn get(&self, id: &str) -> Option<&Focus> {
        self.focuses.iter().find(|f| f.id == id)
    }

    /// Grid position of every focus: x/y, plus the position of the focus they
    /// are relative to (when it is in the tree).
    pub fn positions(&self) -> HashMap<String, (i32, i32)> {
        let by_id: HashMap<&str, &Focus> = self.focuses.iter().map(|f| (f.id.as_str(), f)).collect();
        let mut out: HashMap<String, (i32, i32)> = HashMap::new();
        fn pos<'a>(id: &'a str, by_id: &HashMap<&'a str, &'a Focus>, out: &mut HashMap<String, (i32, i32)>, depth: usize) -> (i32, i32) {
            if let Some(p) = out.get(id) {
                return *p;
            }
            let Some(f) = by_id.get(id) else { return (0, 0) };
            let base = match &f.rel {
                Some(r) if depth < 64 && by_id.contains_key(r.as_str()) && r != id => pos(r, by_id, out, depth + 1),
                _ => (0, 0),
            };
            let p = (base.0 + f.x, base.1 + f.y);
            out.insert(id.to_string(), p);
            p
        }
        for f in &self.focuses {
            pos(&f.id, &by_id, &mut out, 0);
        }
        out
    }

    /// Focuses that need `id` (directly).
    pub fn children(&self, id: &str) -> Vec<&str> {
        self.focuses.iter().filter(|f| f.prereq.iter().flatten().any(|p| p == id)).map(|f| f.id.as_str()).collect()
    }

    /// `id` and everything after it (the focuses that need it, and so on).
    pub fn branch(&self, id: &str) -> Vec<String> {
        let mut out = vec![id.to_string()];
        let mut seen: BTreeSet<String> = out.iter().cloned().collect();
        let mut i = 0;
        while i < out.len() {
            let cur = out[i].clone();
            for c in self.children(&cur) {
                if seen.insert(c.to_string()) {
                    out.push(c.to_string());
                }
            }
            i += 1;
        }
        out
    }

    /// The DLCs some `allow_branch` asks for.
    pub fn dlcs(&self) -> BTreeSet<String> {
        self.focuses.iter().flat_map(|f| f.dlc.iter().cloned()).collect()
    }

    /// Focuses hidden when only `owned` DLCs are there: those whose
    /// allow_branch needs another, and the branches after them.
    pub fn dlc_hidden(&self, owned: &BTreeSet<String>) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        for f in &self.focuses {
            if f.dlc.iter().any(|d| !owned.contains(d)) {
                out.extend(self.branch(&f.id));
            }
        }
        out
    }
}

// ------------------------------------------------------------------ the game's rules

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Completed,
    Available,
    Locked,
    /// Mutually exclusive with a completed one, or every way to it is.
    Hidden,
}

/// The state of every focus with `completed` done. Inside one prerequisite
/// block one focus is enough; every block is needed. Completing a focus
/// locks those mutually exclusive with it, and what can only be reached
/// through them. `available` triggers are not checked: they depend on the
/// game.
pub fn states(focuses: &[Focus], completed: &BTreeSet<String>) -> HashMap<String, State> {
    let by_id: HashMap<&str, &Focus> = focuses.iter().map(|f| (f.id.as_str(), f)).collect();
    let done: BTreeSet<&str> = completed.iter().map(String::as_str).filter(|c| by_id.contains_key(c)).collect();
    let mut hidden: BTreeSet<&str> = BTreeSet::new();
    for _ in 0..=focuses.len() {
        let mut changed = false;
        for f in focuses {
            let id = f.id.as_str();
            if done.contains(id) || hidden.contains(id) {
                continue;
            }
            let excluded = f.mutex.iter().any(|m| done.contains(m.as_str())) || done.iter().any(|c| by_id[c].mutex.iter().any(|m| m == id));
            let unreachable = f.prereq.iter().any(|g| !g.is_empty() && !g.iter().any(|o| by_id.contains_key(o.as_str()) && !hidden.contains(o.as_str())));
            if excluded || unreachable {
                hidden.insert(id);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    focuses
        .iter()
        .map(|f| {
            let id = f.id.as_str();
            let s = if done.contains(id) {
                State::Completed
            } else if hidden.contains(id) {
                State::Hidden
            } else if f.prereq.iter().all(|g| g.iter().any(|o| done.contains(o.as_str()))) {
                State::Available
            } else {
                State::Locked
            };
            (f.id.clone(), s)
        })
        .collect()
}

// ------------------------------------------------------------------ checks

#[derive(Clone, Debug, PartialEq)]
pub struct Issue {
    pub error: bool,
    pub focus: String,
    pub message: String,
}

/// Quick checks of a tree: what it names that is not in it, cycles, and
/// focuses on the same spot.
pub fn check(tree: &Tree) -> Vec<Issue> {
    let mut out = Vec::new();
    let ids: BTreeSet<&str> = tree.focuses.iter().map(|f| f.id.as_str()).collect();
    let mut seen = BTreeSet::new();
    for f in &tree.focuses {
        if !seen.insert(f.id.as_str()) {
            out.push(Issue { error: true, focus: f.id.clone(), message: format!("{} is in the tree twice", f.id) });
        }
        for r in f.prereq.iter().flatten() {
            if !ids.contains(r.as_str()) {
                out.push(Issue { error: true, focus: f.id.clone(), message: format!("{} needs {r}, which is not in this tree", f.id) });
            }
        }
        for r in &f.mutex {
            if !ids.contains(r.as_str()) {
                out.push(Issue { error: true, focus: f.id.clone(), message: format!("{} is mutually exclusive with {r}, which is not in this tree", f.id) });
            } else if tree.get(r).is_some_and(|o| !o.mutex.contains(&f.id)) {
                out.push(Issue { error: false, focus: f.id.clone(), message: format!("{} excludes {r} but {r} does not exclude it (the game wants both ways)", f.id) });
            }
        }
        if let Some(r) = &f.rel
            && !ids.contains(r.as_str())
        {
            out.push(Issue { error: false, focus: f.id.clone(), message: format!("{} is placed relative to {r}, which is not in this tree", f.id) });
        }
        if f.prereq.iter().flatten().any(|p| p == &f.id) {
            out.push(Issue { error: true, focus: f.id.clone(), message: format!("{} needs itself", f.id) });
        }
    }
    for c in cycles(&tree.focuses) {
        out.push(Issue { error: true, focus: c[0].clone(), message: format!("prerequisites go round: {} (none of them can be taken)", c.join(" → ")) });
    }
    let pos = tree.positions();
    let mut at: BTreeMap<(i32, i32), Vec<&str>> = BTreeMap::new();
    // DLC alternatives (allow_branch) and focuses that move (offset) share spots on purpose
    for f in tree.focuses.iter().filter(|f| !f.allow_branch && !f.offset) {
        if let Some(p) = pos.get(&f.id) {
            at.entry(*p).or_default().push(&f.id);
        }
    }
    for ((x, y), v) in at {
        if v.len() > 1 {
            out.push(Issue { error: false, focus: v[0].to_string(), message: format!("{} are all at x = {x}, y = {y}", v.join(", ")) });
        }
    }
    out
}

/// Prerequisite cycles (each once, starting at its smallest id).
pub fn cycles(focuses: &[Focus]) -> Vec<Vec<String>> {
    let by_id: HashMap<&str, &Focus> = focuses.iter().map(|f| (f.id.as_str(), f)).collect();
    // colour DFS over "needs" edges
    let mut colour: HashMap<&str, u8> = HashMap::new();
    let mut out: Vec<Vec<String>> = Vec::new();
    let mut found: BTreeSet<Vec<String>> = BTreeSet::new();
    fn dfs<'a>(
        id: &'a str,
        by_id: &HashMap<&'a str, &'a Focus>,
        colour: &mut HashMap<&'a str, u8>,
        stack: &mut Vec<&'a str>,
        found: &mut BTreeSet<Vec<String>>,
        out: &mut Vec<Vec<String>>,
    ) {
        colour.insert(id, 1);
        stack.push(id);
        if let Some(f) = by_id.get(id) {
            for p in f.prereq.iter().flatten() {
                let p = p.as_str();
                let Some((&pk, _)) = by_id.get_key_value(p) else { continue };
                if pk == id {
                    continue;
                }
                match colour.get(pk).copied().unwrap_or(0) {
                    0 => dfs(pk, by_id, colour, stack, found, out),
                    1 => {
                        let s = stack.iter().position(|x| *x == pk).unwrap();
                        let mut c: Vec<String> = stack[s..].iter().map(|x| x.to_string()).collect();
                        let m = c.iter().enumerate().min_by_key(|(_, x)| x.as_str()).map(|(i, _)| i).unwrap();
                        c.rotate_left(m);
                        if found.insert(c.clone()) {
                            let mut shown = c.clone();
                            shown.push(c[0].clone());
                            out.push(shown);
                        }
                    }
                    _ => {}
                }
            }
        }
        stack.pop();
        colour.insert(id, 2);
    }
    for f in focuses {
        if colour.get(f.id.as_str()).copied().unwrap_or(0) == 0 {
            dfs(&f.id, &by_id, &mut colour, &mut Vec::new(), &mut found, &mut out);
        }
    }
    out
}

// ------------------------------------------------------------------ layout

/// A grid position for every focus from the prerequisites alone: rows by
/// the longest way down to a focus, each row ordered so a focus sits near
/// its parents (barycenter passes), nothing on the same cell, rows centred.
/// `gap`: columns from one focus to the next.
pub fn auto_layout(focuses: &[Focus], gap: i32) -> HashMap<String, (i32, i32)> {
    if focuses.is_empty() {
        return HashMap::new();
    }
    let ids: BTreeSet<&str> = focuses.iter().map(|f| f.id.as_str()).collect();
    let parents = |f: &Focus| -> Vec<String> { f.prereq.iter().flatten().filter(|p| ids.contains(p.as_str()) && **p != f.id).cloned().collect() };
    let mut depth: HashMap<String, i32> = focuses.iter().map(|f| (f.id.clone(), 0)).collect();
    for _ in 0..=focuses.len() {
        let mut changed = false;
        for f in focuses {
            let ps = parents(f);
            let d = if ps.is_empty() { 0 } else { ps.iter().map(|p| depth[p]).max().unwrap() + 1 };
            if d != depth[&f.id] {
                depth.insert(f.id.clone(), d);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let mut rows: BTreeMap<i32, Vec<&Focus>> = BTreeMap::new();
    for f in focuses {
        rows.entry(depth[&f.id]).or_default().push(f);
    }
    let mut col: HashMap<String, f64> = HashMap::new();
    for row in rows.values_mut() {
        row.sort_by(|a, b| a.x.cmp(&b.x).then(a.id.cmp(&b.id)));
        for (i, f) in row.iter().enumerate() {
            col.insert(f.id.clone(), i as f64);
        }
    }
    for _ in 0..4 {
        for (&d, row) in rows.iter_mut() {
            if d == 0 {
                continue;
            }
            let key = |f: &Focus| {
                let ps = parents(f);
                if ps.is_empty() { col[&f.id] } else { ps.iter().map(|p| col[p]).sum::<f64>() / ps.len() as f64 }
            };
            let mut keyed: Vec<(f64, &Focus)> = row.iter().map(|f| (key(f), *f)).collect();
            keyed.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.id.cmp(&b.1.id)));
            *row = keyed.into_iter().map(|x| x.1).collect();
            for (i, f) in row.iter().enumerate() {
                col.insert(f.id.clone(), i as f64);
            }
        }
    }
    let widest = rows.values().map(|r| r.len()).max().unwrap_or(0) as f64;
    let mut out = HashMap::new();
    for (&d, row) in &rows {
        let offset = (widest - row.len() as f64) / 2.0;
        for (i, f) in row.iter().enumerate() {
            out.insert(f.id.clone(), (((offset + i as f64) * gap as f64).round() as i32, d));
        }
    }
    out
}

/// A free cell at or to the right of `at`.
#[cfg(test)]
pub fn free_cell(taken: &BTreeSet<(i32, i32)>, at: (i32, i32)) -> (i32, i32) {
    let mut p = at;
    while taken.contains(&p) {
        p.0 += 1;
    }
    p
}

// ------------------------------------------------------------------ edits

/// The focus block with this id in a file: in a `focus_tree` (any) or a
/// top-level `shared_focus`.
pub fn find<'a>(s: &'a Script, id: &str) -> Option<(&'a Item, &'a Block)> {
    for it in &s.root.items {
        if it.is("shared_focus")
            && let Some(b) = it.block()
            && b.atom("id") == Some(id)
        {
            return Some((it, b));
        }
        if it.is("focus_tree")
            && let Some(t) = it.block()
        {
            for f in t.all("focus") {
                if let Some(b) = f.block()
                    && b.atom("id") == Some(id)
                {
                    return Some((f, b));
                }
            }
        }
    }
    None
}

/// The focus_tree block with this id.
pub fn find_tree<'a>(s: &'a Script, id: &str) -> Option<(&'a Item, &'a Block)> {
    s.root.items.iter().filter(|i| i.is("focus_tree")).find_map(|i| i.block().filter(|b| b.atom("id") == Some(id)).map(|b| (i, b)))
}

fn int(v: i32) -> String {
    v.to_string()
}

/// x and y of a focus set (as written: relative when it has relative_position_id).
pub fn set_xy(text: &str, b: &Block, x: i32, y: i32) -> Vec<Edit> {
    vec![script::set_atom(text, b, "x", &int(x)), script::set_atom(text, b, "y", &int(y))]
}

/// `parent` added to the prerequisites of a focus: a block of its own (both
/// needed) or into block `group` (either is enough).
pub fn add_prereq(text: &str, b: &Block, parent: &str, group: Option<usize>) -> Vec<Edit> {
    if let Some(g) = group.and_then(|g| b.blocks("prerequisite").nth(g)) {
        if g.all("focus").any(|i| i.text() == Some(parent)) {
            return Vec::new();
        }
        return vec![script::insert_item(text, g, &format!("focus = {parent}"), None)];
    }
    // after the last prerequisite block, else after y / x / icon / id
    let at = b
        .items
        .iter()
        .rposition(|i| i.is("prerequisite"))
        .or_else(|| ["mutually_exclusive", "relative_position_id", "y", "x", "cost", "icon", "id"].iter().find_map(|k| b.items.iter().rposition(|i| i.is(k))));
    vec![script::insert_item(text, b, &format!("prerequisite = {{ focus = {parent} }}"), at)]
}

/// `parent` taken out of the prerequisites (a block left empty goes).
pub fn remove_prereq(text: &str, b: &Block, parent: &str) -> Vec<Edit> {
    let mut out = Vec::new();
    for it in b.all("prerequisite") {
        let Some(g) = it.block() else { continue };
        let hits: Vec<&Item> = g.all("focus").filter(|i| i.text() == Some(parent)).collect();
        if hits.is_empty() {
            continue;
        }
        if hits.len() == g.items.len() {
            out.push(script::remove_item(text, it));
        } else {
            out.extend(hits.into_iter().map(|h| script::remove_item(text, h)));
        }
    }
    out
}

/// `other` added to or taken out of a focus's mutually_exclusive.
pub fn set_mutex(text: &str, b: &Block, other: &str, on: bool) -> Vec<Edit> {
    let blocks: Vec<&Item> = b.all("mutually_exclusive").collect();
    let has = blocks.iter().filter_map(|i| i.block()).any(|m| m.all("focus").any(|i| i.text() == Some(other)));
    if on {
        if has {
            return Vec::new();
        }
        if let Some(m) = blocks.first().and_then(|i| i.block()) {
            return vec![script::insert_item(text, m, &format!("focus = {other}"), None)];
        }
        let at = b.items.iter().rposition(|i| i.is("prerequisite")).or_else(|| ["y", "x", "cost", "icon", "id"].iter().find_map(|k| b.items.iter().rposition(|i| i.is(k))));
        return vec![script::insert_item(text, b, &format!("mutually_exclusive = {{ focus = {other} }}"), at)];
    }
    let mut out = Vec::new();
    for it in blocks {
        let Some(m) = it.block() else { continue };
        let hits: Vec<&Item> = m.all("focus").filter(|i| i.text() == Some(other)).collect();
        if hits.is_empty() {
            continue;
        }
        if hits.len() == m.items.len() {
            out.push(script::remove_item(text, it));
        } else {
            out.extend(hits.into_iter().map(|h| script::remove_item(text, h)));
        }
    }
    out
}

/// Everything in a focus that names `id` taken out (prerequisites, mutually
/// exclusive), and relative_position_id = id replaced by the position it
/// gave (`anchor`).
pub fn forget(text: &str, b: &Block, id: &str, anchor: (i32, i32)) -> Vec<Edit> {
    let mut out = remove_prereq(text, b, id);
    out.extend(set_mutex(text, b, id, false));
    if b.atom("relative_position_id") == Some(id) {
        let (x, y) = (num(b, "x").unwrap_or(0.0) as i32, num(b, "y").unwrap_or(0.0) as i32);
        out.push(script::remove_item(text, b.get("relative_position_id").unwrap()));
        out.extend(set_xy(text, b, x + anchor.0, y + anchor.1));
    }
    out
}

/// The text of a new focus.
pub fn new_focus(id: &str, x: i32, y: i32, icon: &str, cost: f64, prereq: &[Vec<String>], shared: bool) -> String {
    let mut s = format!("{} = {{\n\tid = {id}\n\ticon = {icon}\n\tx = {x}\n\ty = {y}\n\tcost = {}\n", if shared { "shared_focus" } else { "focus" }, fmt_num(cost));
    for g in prereq {
        s += &format!("\tprerequisite = {{ {} }}\n", g.iter().map(|p| format!("focus = {p}")).collect::<Vec<_>>().join(" "));
    }
    s += "\tavailable_if_capitulated = yes\n\tcompletion_reward = {\n\t}\n\tai_will_do = { factor = 1 }\n}";
    s
}

pub fn fmt_num(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e9 { format!("{}", v as i64) } else { format!("{v}") }
}

/// Copies of focus blocks (their source text) with their ids renamed by
/// `rename` (old id → new; ids not in it stay) and moved by `dx`, `dy` when
/// they are not relative to a copied focus.
pub fn copy_blocks(text: &str, s: &Script, ids: &[String], rename: &BTreeMap<String, String>, dx: i32, dy: i32) -> Vec<String> {
    let mut out = Vec::new();
    for id in ids {
        let Some((item, b)) = find(s, id) else { continue };
        let src = script::item_source(text, item);
        let rel_inside = b.atom("relative_position_id").is_some_and(|r| rename.contains_key(r));
        let sub = Script::parse(&src);
        let Some(sb) = sub.root.items.first().and_then(|i| i.block()) else { continue };
        let mut edits = Vec::new();
        if !rel_inside && (dx != 0 || dy != 0) {
            let (x, y) = (num(sb, "x").unwrap_or(0.0) as i32, num(sb, "y").unwrap_or(0.0) as i32);
            edits.extend(set_xy(&src, sb, x + dx, y + dy));
        }
        let moved = script::apply(&src, &edits);
        let (renamed, _) = script::rename_words(&moved, &|w| rename.get(w).cloned());
        out.push(renamed);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(id: &str, x: i32, y: i32, prereq: &[&[&str]], mutex: &[&str]) -> Focus {
        Focus {
            id: id.into(),
            x,
            y,
            prereq: prereq.iter().map(|g| g.iter().map(|s| s.to_string()).collect()).collect(),
            mutex: mutex.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn game_rules() {
        // a → b / c (exclusive) → d needs b or c; e needs b and c
        let fs = vec![f("a", 0, 0, &[], &[]), f("b", 0, 1, &[&["a"]], &["c"]), f("c", 1, 1, &[&["a"]], &["b"]), f("d", 0, 2, &[&["b", "c"]], &[]), f("e", 1, 2, &[&["b"], &["c"]], &[])];
        let st = |done: &[&str]| {
            let c: BTreeSet<String> = done.iter().map(|s| s.to_string()).collect();
            let m = states(&fs, &c);
            ["a", "b", "c", "d", "e"].map(|i| m[i])
        };
        use State::*;
        assert_eq!(st(&[]), [Available, Locked, Locked, Locked, Locked]);
        assert_eq!(st(&["a"]), [Completed, Available, Available, Locked, Locked]);
        assert_eq!(st(&["a", "b"]), [Completed, Completed, Hidden, Available, Hidden]);
    }

    #[test]
    fn positions_checks_layout() {
        let mut fs = vec![f("a", 4, 0, &[], &[]), f("b", -1, 1, &[&["a"]], &["c"]), f("c", 1, 1, &[&["a"]], &[]), f("d", 0, 1, &[&["b"]], &[])];
        fs[1].rel = Some("a".into());
        fs[2].rel = Some("a".into());
        fs[3].rel = Some("b".into());
        let t = Tree { id: "t".into(), focuses: fs.clone(), ..Default::default() };
        let p = t.positions();
        assert_eq!((p["a"], p["b"], p["c"], p["d"]), ((4, 0), (3, 1), (5, 1), (3, 2)));
        let issues = check(&t);
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert!(!issues[0].error && issues[0].message.contains("does not exclude"));
        assert_eq!(t.branch("a"), ["a", "b", "c", "d"]);
        assert_eq!(t.branch("b"), ["b", "d"]);

        let bad = Tree { focuses: vec![f("x", 0, 0, &[&["z"]], &[]), f("y", 0, 0, &[&["x"]], &["q"]), f("z", 1, 0, &[&["y"]], &[])], ..Default::default() };
        let msgs: Vec<String> = check(&bad).into_iter().map(|i| i.message).collect();
        assert!(msgs.iter().any(|m| m.contains("go round: x → z → y → x")), "{msgs:?}");
        assert!(msgs.iter().any(|m| m.contains("q, which is not")));
        assert!(msgs.iter().any(|m| m.contains("x, y are all at")));

        let l = auto_layout(&fs, 2);
        assert_eq!(l["a"].1, 0);
        assert_eq!(l["b"].1, 1);
        assert_eq!(l["d"].1, 2);
        let cells: BTreeSet<(i32, i32)> = l.values().copied().collect();
        assert_eq!(cells.len(), 4);
        assert_eq!(free_cell(&cells, l["a"]), (l["a"].0 + 1, 0));
    }

    const TREE: &str = "focus_tree = {\n\tid = tyr\n\tcountry = { factor = 0 modifier = { add = 10 tag = TYR } }\n\tfocus = {\n\t\tid = A\n\t\tx = 0\n\t\ty = 0\n\t}\n\tfocus = {\n\t\tid = B\n\t\tx = 1\n\t\ty = 1\n\t\trelative_position_id = A\n\t\tprerequisite = { focus = A }\n\t}\n}\nshared_focus = {\n\tid = S\n\tprerequisite = { focus = Q }\n\tx = 5\n\ty = 5\n}\nshared_focus = { id = S2 prerequisite = { focus = S } }\n";

    #[test]
    fn reads_and_edits_trees() {
        let s = Script::parse(TREE);
        let (_, tb) = find_tree(&s, "tyr").unwrap();
        let mut t = tree_of(tb);
        assert_eq!(t.countries, ["TYR"]);
        assert_eq!(t.focuses.len(), 2);
        t.shared = vec!["S".into()];
        include_shared(&mut t, &shared_focuses(&s));
        assert_eq!(t.focuses.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), ["A", "B", "S", "S2"]);

        let (_, b) = find(&s, "B").unwrap();
        let t1 = script::apply(TREE, &add_prereq(TREE, b, "C", None));
        assert!(t1.contains("prerequisite = { focus = A }\n\t\tprerequisite = { focus = C }"), "{t1}");
        let t2 = script::apply(TREE, &add_prereq(TREE, b, "C", Some(0)));
        assert!(t2.contains("prerequisite = { focus = A focus = C }"), "{t2}");
        let s2 = Script::parse(&t2);
        let (_, b2) = find(&s2, "B").unwrap();
        let t3 = script::apply(&t2, &remove_prereq(&t2, b2, "A"));
        assert!(t3.contains("prerequisite = { focus = C }"), "{t3}");
        let t4 = script::apply(&t2, &remove_prereq(&t2, b2, "A").into_iter().chain(remove_prereq(&t2, b2, "C")).collect::<Vec<_>>());
        let _ = t4;
        let t5 = script::apply(TREE, &remove_prereq(TREE, b, "A"));
        assert!(!t5.contains("prerequisite = { focus = A }"));
        let t6 = script::apply(TREE, &set_mutex(TREE, b, "S", true));
        assert!(t6.contains("mutually_exclusive = { focus = S }"));
        let s6 = Script::parse(&t6);
        let (_, b6) = find(&s6, "B").unwrap();
        assert!(!script::apply(&t6, &set_mutex(&t6, b6, "S", false)).contains("mutually_exclusive"));
        // forgetting A: no prerequisite, and B's position made absolute
        let t7 = script::apply(TREE, &forget(TREE, b, "A", (0, 0)));
        let s7 = Script::parse(&t7);
        let (_, b7) = find(&s7, "B").unwrap();
        assert!(b7.get("relative_position_id").is_none() && b7.get("prerequisite").is_none());
        assert_eq!((b7.atom("x"), b7.atom("y")), (Some("1"), Some("1")));
        // copies
        let rename: BTreeMap<String, String> = [("A".to_string(), "A2".to_string()), ("B".to_string(), "B2".to_string())].into();
        let c = copy_blocks(TREE, &s, &["A".into(), "B".into()], &rename, 3, 0);
        assert!(c[0].contains("id = A2") && c[0].contains("x = 3"));
        assert!(c[1].contains("id = B2") && c[1].contains("relative_position_id = A2") && c[1].contains("x = 1"), "{}", c[1]);
        assert!(Script::parse(&new_focus("N", 1, 2, "GFX_goal_unknown", 10.0, &[vec!["A".into()]], false)).errors.is_empty());
    }
}
