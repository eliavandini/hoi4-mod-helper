//! The game's own script documentation (`<game>/documentation/`): every
//! effect, trigger and modifier the game knows, with the scopes it works
//! in, its description and, where the documentation has one, an example.
//! The wizards list them next to their hand-made catalog.

use std::path::Path;

/// An effect or a trigger.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Entry {
    pub name: String,
    /// `COUNTRY`, `STATE`, `CHARACTER`, ... or `any`.
    pub scopes: Vec<String>,
    /// What it can be pointed at (`THIS`, `ROOT`, ...); empty: nothing.
    pub targets: Vec<String>,
    /// The documentation's text.
    pub text: String,
    /// Its first example as script (`name = ...`), when the text has one.
    pub example: Option<String>,
}

impl Entry {
    /// It works in `scope` (case does not matter; `any` works everywhere).
    pub fn in_scope(&self, scope: &str) -> bool {
        self.scopes.is_empty() || self.scopes.iter().any(|s| s.eq_ignore_ascii_case(scope) || s.eq_ignore_ascii_case("any"))
    }
}

/// A modifier.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Modifier {
    pub name: String,
    /// Decimal places the game shows (None: a yes/no modifier).
    pub decimals: Option<u8>,
    /// `country`, `state`, `army`, ...
    pub categories: Vec<String>,
    pub text: String,
}

#[derive(Debug, Default)]
pub struct Docs {
    pub effects: Vec<Entry>,
    pub triggers: Vec<Entry>,
    pub modifiers: Vec<Modifier>,
}

impl Docs {
    /// Read `<game>/documentation` (what is missing stays empty).
    pub fn load(game: &Path) -> Docs {
        let dir = game.join("documentation");
        let read = |n: &str| std::fs::read_to_string(dir.join(n)).unwrap_or_default();
        Docs { effects: parse_entries(&read("effects_documentation.md")), triggers: parse_entries(&read("triggers_documentation.md")), modifiers: parse_modifiers(&read("modifiers_documentation.md")) }
    }

    pub fn effect(&self, name: &str) -> Option<&Entry> {
        self.effects.iter().find(|e| e.name == name)
    }
    pub fn trigger(&self, name: &str) -> Option<&Entry> {
        self.triggers.iter().find(|e| e.name == name)
    }
    pub fn modifier(&self, name: &str) -> Option<&Modifier> {
        self.modifiers.iter().find(|e| e.name == name)
    }
}

/// `## name` headings of single words, each with what follows up to the next.
fn sections(md: &str) -> Vec<(&str, Vec<&str>)> {
    let mut out: Vec<(&str, Vec<&str>)> = Vec::new();
    for line in md.lines() {
        if let Some(h) = line.strip_prefix("## ") {
            let h = h.trim();
            if !h.is_empty() && h.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'@' || c == b'^' || c == b':') {
                out.push((h, Vec::new()));
                continue;
            }
            // another heading ("Effects for scope X") ends the entry
            out.push(("", Vec::new()));
            continue;
        }
        if let Some(last) = out.last_mut() {
            last.1.push(line);
        }
    }
    out.retain(|s| !s.0.is_empty());
    out
}

fn list_after(lines: &[&str], label: &str) -> Option<Vec<String>> {
    let l = lines.iter().find_map(|l| l.trim_start().strip_prefix(label))?;
    let v = l.trim_start_matches(':').trim();
    Some(if v.eq_ignore_ascii_case("none") || v.is_empty() { Vec::new() } else { v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect() })
}

/// The description: the lines that are not the bullet lines, without the
/// code fences, trimmed of blank lines at both ends.
fn body(lines: &[&str]) -> String {
    let kept: Vec<&str> = lines.iter().copied().filter(|l| !l.starts_with("* ") && l.trim() != "```").collect();
    let text = kept.join("\n");
    text.trim_matches('\n').trim_end().to_string()
}

/// The first example of `name` in a text: the line that starts with it
/// (`name = ...`, `name > 3`, also after `Example:`), and when it opens a
/// block, the lines up to where the block closes; indented by its braces.
pub fn example_of(name: &str, text: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let first = |l: &str| -> Option<String> {
        let t = l.trim_start();
        let t = t.strip_prefix("Example:").map(str::trim_start).unwrap_or(t);
        let rest = t.strip_prefix(name)?.trim_start();
        (rest.starts_with('=') || rest.starts_with('<') || rest.starts_with('>')).then(|| t.to_string())
    };
    let (start, head) = lines.iter().enumerate().find_map(|(i, l)| first(l).map(|h| (i, h)))?;
    let mut depth = 0i32;
    let mut out = Vec::new();
    for (n, l) in lines[start..].iter().enumerate() {
        let l = if n == 0 { head.as_str() } else { l };
        // braces outside comments
        let code = l.split('#').next().unwrap_or("");
        depth += code.matches('{').count() as i32 - code.matches('}').count() as i32;
        out.push(l.trim());
        if depth <= 0 {
            break;
        }
        if out.len() > 40 {
            return None;
        }
    }
    if depth > 0 {
        return None;
    }
    Some(crate::content::script::format(&out.join("\n")).trim_end().to_string())
}

pub fn parse_entries(md: &str) -> Vec<Entry> {
    let mut out: Vec<Entry> = sections(md)
        .into_iter()
        .filter(|(_, lines)| lines.iter().any(|l| l.starts_with("* Supported Scopes")))
        .map(|(name, lines)| {
            let text = body(&lines);
            Entry {
                name: name.to_string(),
                scopes: list_after(&lines, "* Supported Scopes").unwrap_or_default(),
                targets: list_after(&lines, "* Supported Targets").unwrap_or_default(),
                example: example_of(name, &text),
                text,
            }
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out.dedup_by(|a, b| a.name == b.name);
    out
}

pub fn parse_modifiers(md: &str) -> Vec<Modifier> {
    let mut out: Vec<Modifier> = sections(md)
        .into_iter()
        .filter(|(_, lines)| lines.iter().any(|l| l.starts_with("* **Categories**") || l.starts_with("* Number with") || l.starts_with("* Boolean")))
        .map(|(name, lines)| {
            let decimals = lines.iter().find_map(|l| l.strip_prefix("* Number with ")).and_then(|r| r.split_whitespace().next()).and_then(|n| n.parse().ok());
            let text = lines.iter().find_map(|l| l.strip_prefix("* **Description**:")).map(|t| t.trim().to_string()).unwrap_or_default();
            Modifier { name: name.to_string(), decimals, categories: list_after(&lines, "* **Categories**").unwrap_or_default(), text }
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out.dedup_by(|a, b| a.name == b.name);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const EFFECTS: &str = "# Effects\n\n## Table of Content\n\n* [COUNTRY](#effects-for-scope-country)\n\n## Effects for scope COUNTRY\n\n* [add_popularity](#add_popularity)\n\n## add_political_power\n\n* Supported Scopes: COUNTRY\n* Supported Targets: none\n\n```\nadd political power to country\n```\n\n## add_popularity\n\n* Supported Scopes: COUNTRY\n* Supported Targets: none\n\n```\nadd popularity to an ideology in a country\n\nExample:\nadd_popularity = {\n\tideology = neutrality\n\tpopularity = 0.05\n}\n```\n\n## load_focus_tree\n\n* Supported Scopes: COUNTRY, STATE\n* Supported Targets: THIS, ROOT\n\nSets what focus tree a country uses.\n ### Examples\n ```\n load_focus_tree = { \n\t tree = persian_focus_tree\n }\n ```\n";

    #[test]
    fn reads_effects() {
        let e = parse_entries(EFFECTS);
        assert_eq!(e.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), ["add_political_power", "add_popularity", "load_focus_tree"]);
        assert_eq!(e[0].scopes, ["COUNTRY"]);
        assert!(e[0].targets.is_empty() && e[0].example.is_none());
        assert_eq!(e[0].text, "add political power to country");
        assert_eq!(e[1].example.as_deref(), Some("add_popularity = {\n\tideology = neutrality\n\tpopularity = 0.05\n}"));
        assert_eq!(e[2].scopes, ["COUNTRY", "STATE"]);
        assert_eq!(e[2].targets, ["THIS", "ROOT"]);
        assert_eq!(e[2].example.as_deref(), Some("load_focus_tree = {\n\ttree = persian_focus_tree\n}"));
        assert!(e[2].in_scope("state") && !e[0].in_scope("STATE"));
    }

    #[test]
    fn reads_modifiers() {
        let md = "# Modifiers\n\n## political_power_factor\n\n* Number with 0 decimal places\n* **Categories**: country\n\n## can_fight\n\n* Boolean\n* **Categories**: army, aggressive\n* **Description**: Whether it fights\n";
        let m = parse_modifiers(md);
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].name, "can_fight");
        assert_eq!((m[0].decimals, m[0].categories.clone(), m[0].text.as_str()), (None, vec!["army".to_string(), "aggressive".to_string()], "Whether it fights"));
        assert_eq!((m[1].decimals, m[1].categories.clone()), (Some(0), vec!["country".to_string()]));
    }

    #[test]
    fn examples_of_triggers_with_comparisons() {
        assert_eq!(example_of("stability", "stability of a country\nExample: stability > 0.5").as_deref(), Some("stability > 0.5"));
        assert_eq!(example_of("add_planning", "Adds planning skill\nExample: add_planning = 1").as_deref(), Some("add_planning = 1"));
        assert_eq!(example_of("x", "no example here"), None);
    }

    #[test]
    #[ignore]
    fn the_games_documentation() {
        let d = Docs::load(&crate::modbuild::default_game_dir());
        assert!(d.effects.len() > 400 && d.triggers.len() > 400 && d.modifiers.len() > 600, "{} {} {}", d.effects.len(), d.triggers.len(), d.modifiers.len());
        let e = d.effect("country_event").unwrap();
        assert!(e.in_scope("COUNTRY") && e.example.as_deref().is_some_and(|x| x.starts_with("country_event = {")), "{e:?}");
        assert!(d.trigger("has_completed_focus").is_some_and(|t| t.in_scope("COUNTRY")));
        assert!(d.modifier("political_power_factor").is_some_and(|m| m.categories.contains(&"country".to_string())));
        let with_examples = d.effects.iter().filter(|e| e.example.is_some()).count();
        eprintln!("{} effects ({with_examples} with examples), {} triggers, {} modifiers", d.effects.len(), d.triggers.len(), d.modifiers.len());
    }
}
