//! The game's `logs/error.log`: what the game said about the mod when it
//! loaded it. Lines are `[HH:MM:SS][date][source.cpp:line]: message`; long
//! messages go on over the next lines. Repeats are counted once, the noise
//! of the base game's 3D models is left out, and a message that names a
//! file of the mod (`events/x.txt:100`) is the mod's, pointed back to the
//! project's content where the file came from.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use regex::Regex;

/// Messages about the base game's models and animations: not the mod's.
pub const NOISE: [&str; 4] = ["Setting animation failed", "has no attachment node named", "Could not find animation", "texture not found for gfx"];

/// Plain words for the errors made most often in hand-written content.
pub const HINTS: [(&str, &str); 8] = [
    (
        "No valid option for event",
        "Every option of the event has a trigger that is false for the country it fired for, so it shows nothing. Give one option no trigger (or an always-true one).",
    ),
    (
        "does not exist for the creator country",
        "An effect names something (an equipment variant, a unit, a state) this country does not have: a misspelt id, or the effect runs before the thing is made.",
    ),
    ("AI tried to post an invalid command", "Mostly harmless AI noise; if it keeps coming, an ai_strategy may name a focus or war goal that does not exist."),
    ("duplicate", "Two files define the same id; the game keeps the one it read last and drops the other. Check finds these."),
    ("Unknown token", "A field or effect the game does not know: a typo, or something of a DLC that is not checked with has_dlc."),
    ("Failed to load", "The game could not read the file at all, mostly a brace or quote left open earlier in it. Check finds these."),
    ("Unexpected token", "A word where the game expected something else: often a missing = or a brace closed too early."),
    ("Missing localization", "A text has no localisation in the game's language; the key is shown instead."),
];

pub fn hint_for(message: &str) -> Option<&'static str> {
    HINTS.iter().find(|(k, _)| message.contains(k)).map(|x| x.1)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub time: String,
    /// The game's source file and line that reported it.
    pub source: String,
    pub message: String,
    pub count: usize,
    /// The mod file it names: path relative to the mod and line.
    pub file: Option<(String, Option<usize>)>,
    /// The project's package that file comes from (it is the mod's).
    pub pkg: Option<String>,
}

impl Entry {
    /// About the mod (it names a file of the mod).
    pub fn ours(&self) -> bool {
        self.file.is_some()
    }
}

/// Where the log of the game is: `<user dir>/logs/error.log`.
pub fn log_path(user_dir: &Path) -> PathBuf {
    user_dir.join("logs").join("error.log")
}

/// The entries of a log's text. `is_mod_file` says whether a path (relative,
/// with `/`) is a file of the mod; `pkg_of` which package it came from.
pub fn parse(text: &str, is_mod_file: &dyn Fn(&str) -> bool, pkg_of: &dyn Fn(&str) -> Option<String>, limit: usize) -> Vec<Entry> {
    let line_re = Regex::new(r"^\[(\d{2}:\d{2}:\d{2})\]\[([^\]]*)\]\[([^\]]*)\]:\s*(.*)$").unwrap();
    let path_re = Regex::new(r#"([\w./\\-]+\.(?:txt|gui|gfx|yml|asset))(?::(\d+)|"?,? (?:line|near line):? ?(\d+))?"#).unwrap();
    let classify = |m: &str| -> Option<(String, Option<usize>)> {
        for c in path_re.captures_iter(m) {
            let mut p = c[1].replace('\\', "/");
            // absolute paths of the mod: keep what is after the mod folder
            for root in ["common/", "events/", "history/", "localisation/", "interface/", "gfx/", "music/", "map/"] {
                if let Some(i) = p.find(root)
                    && i > 0
                {
                    p = p[i..].to_string();
                    break;
                }
            }
            if is_mod_file(&p) {
                let line = c.get(2).or(c.get(3)).and_then(|x| x.as_str().parse().ok());
                return Some((p, line));
            }
        }
        None
    };
    let mut out: Vec<Entry> = Vec::new();
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut last: Option<usize> = None;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let Some(c) = line_re.captures(line) else {
            // the message goes on: the end often names the file
            if let Some(i) = last {
                let e = &mut out[i];
                seen.remove(&e.message);
                e.message = format!("{} {line}", e.message);
                e.file = classify(&e.message);
                seen.insert(e.message.clone(), i);
            }
            continue;
        };
        let msg = c[4].to_string();
        if NOISE.iter().any(|n| msg.contains(n)) {
            last = None;
            continue;
        }
        if let Some(&i) = seen.get(&msg) {
            out[i].count += 1;
            last = Some(i);
            continue;
        }
        let file = classify(&msg);
        last = Some(out.len());
        seen.insert(msg.clone(), out.len());
        out.push(Entry { time: c[1].to_string(), source: c[3].to_string(), message: msg, count: 1, file, pkg: None });
    }
    for e in &mut out {
        e.pkg = e.file.as_ref().and_then(|(p, _)| pkg_of(p));
    }
    if out.len() > limit {
        // the mod's own survive the cap
        let (mine, others): (Vec<Entry>, Vec<Entry>) = out.into_iter().partition(|e| e.ours());
        let room = limit.saturating_sub(mine.len());
        out = mine.into_iter().chain(others.into_iter().take(room)).collect();
    }
    out
}

/// Read a log from byte `from` (a size seen before a test run; a log that is
/// shorter now was started again by the game and is read whole).
pub fn read(path: &Path, from: u64) -> std::io::Result<String> {
    let b = std::fs::read(path)?;
    let start = if (from as usize) < b.len() { from as usize } else { 0 };
    Ok(String::from_utf8_lossy(&b[start..]).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_log() {
        let log = "[10:00:01][1936.01.01.12][effectimplementation.cpp:123]: events/tyr.txt:100: create_ship equipment_variant does not exist for the creator country\n\
[10:00:01][1936.01.01.12][effectimplementation.cpp:123]: events/tyr.txt:100: create_ship equipment_variant does not exist for the creator country\n\
[10:00:02][1936.01.01.12][pdxmesh.cpp:9]: Setting animation failed for x\n\
[10:00:03][no game date][persistent.cpp:47]: Error: \"Unexpected token: foo, near line: 12\" in file:\n \"common/ideas/tyr_idea.txt\"\n\
[10:00:04][no game date][localization.cpp:5]: Missing localization key GER_x\n";
        let ours = |p: &str| p == "events/tyr.txt" || p == "common/ideas/tyr_idea.txt";
        let pkg = |p: &str| ours(p).then(|| "TYR".to_string());
        let e = parse(log, &ours, &pkg, 100);
        assert_eq!(e.len(), 3, "{e:#?}");
        assert_eq!(e[0].count, 2);
        assert_eq!(e[0].file, Some(("events/tyr.txt".into(), Some(100))));
        assert_eq!(e[0].pkg.as_deref(), Some("TYR"));
        assert!(hint_for(&e[0].message).is_some());
        // the file is on the next line
        assert_eq!(e[1].file.as_ref().map(|f| f.0.as_str()), Some("common/ideas/tyr_idea.txt"));
        assert!(!e[2].ours());
        // a cap keeps the mod's
        let e = parse(log, &ours, &pkg, 1);
        assert_eq!(e.len(), 2);
    }
}
