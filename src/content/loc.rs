//! Localisation: the `localisation/<language>/*_l_<language>.yml` files,
//! `l_english:` and then one ` KEY:0 "text"` per line. The game reads up to
//! the last quote of a line, so quotes inside a text need no escape; the
//! text is kept as written (`\n`, `§Y...§!`, `[Root.GetName]`).

/// The languages HOI4 has.
pub const LANGUAGES: [&str; 10] = ["english", "braz_por", "french", "german", "japanese", "polish", "russian", "spanish", "simp_chinese", "korean"];

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub key: String,
    /// The number after the colon (often 0, or missing).
    pub version: Option<u32>,
    pub text: String,
    /// Byte range of the line (without its `\n`).
    pub line: std::ops::Range<usize>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LocFile {
    /// From the `l_<language>:` header ("english").
    pub language: Option<String>,
    pub entries: Vec<Entry>,
}

/// The language a file is for by its name (`*_l_german.yml`).
pub fn language_of_name(name: &str) -> Option<&'static str> {
    let n = name.to_ascii_lowercase();
    LANGUAGES.iter().copied().find(|l| n.ends_with(&format!("_l_{l}.yml")))
}

pub fn parse(text: &str) -> LocFile {
    let mut out = LocFile::default();
    let mut at = 0;
    for raw in text.split_inclusive('\n') {
        let start = at;
        at += raw.len();
        let line = raw.trim_end_matches(['\n', '\r']);
        let range = start..start + line.len();
        let t = line.trim_start_matches('\u{feff}').trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if out.language.is_none() && t.starts_with("l_") && t.ends_with(':') {
            out.language = Some(t[2..t.len() - 1].to_string());
            continue;
        }
        let Some(colon) = t.find(':') else { continue };
        let key = t[..colon].trim();
        if key.is_empty() || key.contains(char::is_whitespace) {
            continue;
        }
        let rest = &t[colon + 1..];
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        let rest = rest[digits.len()..].trim_start();
        let Some(q) = rest.strip_prefix('"') else { continue };
        // up to the last quote, not counting one in a comment after it
        let end = last_quote(q);
        let text = match end {
            Some(e) => &q[..e],
            None => q,
        };
        out.entries.push(Entry { key: key.to_string(), version: digits.parse().ok(), text: text.to_string(), line: range });
    }
    out
}

/// The closing quote of a text: the last one, unless a `#` comment follows
/// an earlier quote and holds the later ones.
fn last_quote(s: &str) -> Option<usize> {
    let last = s.rfind('"')?;
    // `"text" # a "comment"`: the quote before the comment
    let mut best = last;
    for (i, _) in s.match_indices('"') {
        let after = s[i + 1..].trim_start();
        if after.starts_with('#') {
            best = i;
            break;
        }
    }
    Some(best)
}

/// One line of a file.
pub fn line(key: &str, text: &str) -> String {
    format!(" {key}:0 \"{}\"", text.replace(['\n', '\r'], "\\n"))
}

/// A whole file.
pub fn render(language: &str, entries: &[(String, String)]) -> String {
    let mut s = format!("\u{feff}l_{language}:\n");
    for (k, v) in entries {
        s.push_str(&line(k, v));
        s.push('\n');
    }
    s
}

/// Set a key in a file's text: its line is replaced, or the line is added at the end.
pub fn set(text: &str, language: &str, key: &str, value: &str) -> String {
    let f = parse(text);
    if let Some(e) = f.entries.iter().find(|e| e.key == key) {
        let mut out = text.to_string();
        let new = match e.version {
            Some(v) => format!(" {key}:{v} \"{}\"", value.replace(['\n', '\r'], "\\n")),
            None => format!(" {key}: \"{}\"", value.replace(['\n', '\r'], "\\n")),
        };
        out.replace_range(e.line.clone(), &new);
        return out;
    }
    let mut out = if text.trim().is_empty() { format!("\u{feff}l_{language}:\n") } else { text.to_string() };
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&line(key, value));
    out.push('\n');
    out
}

/// Remove a key's line from a file's text.
pub fn remove(text: &str, key: &str) -> Option<String> {
    let f = parse(text);
    let e = f.entries.iter().find(|e| e.key == key)?;
    let mut out = text.to_string();
    let end = (e.line.end + 1).min(text.len());
    out.replace_range(e.line.start..end, "");
    Some(out)
}

/// A readable name from an id: `TYR_four_year_plan` → "Four Year Plan"
/// (the tag goes; `.t` / `.d` endings are not words).
pub fn prettify(key: &str) -> String {
    let base = key.split('.').next().unwrap_or(key);
    let mut parts: Vec<&str> = base.split('_').filter(|w| !w.is_empty()).collect();
    if parts.len() > 1 && parts[0].len() == 3 && parts[0].chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) {
        parts.remove(0);
    }
    if parts.first().is_some_and(|p| p.eq_ignore_ascii_case("focus") || p.eq_ignore_ascii_case("idea")) && parts.len() > 1 {
        parts.remove(0);
    }
    let words: Vec<String> = parts
        .iter()
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect();
    if words.is_empty() { key.to_string() } else { words.join(" ") }
}

/// Text with the game's colour codes (`§Y`, `§!`) and icons (`£x`) taken
/// out and `\n` as line breaks: how it reads on screen, about.
pub fn plain(text: &str) -> String {
    let mut out = String::new();
    let mut it = text.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '§' => {
                it.next();
            }
            '£' => {
                while it.peek().is_some_and(|c| c.is_alphanumeric() || *c == '_' || *c == '|') {
                    it.next();
                }
                if it.peek() == Some(&'£') {
                    it.next();
                }
            }
            '\\' if it.peek() == Some(&'n') => {
                it.next();
                out.push('\n');
            }
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_and_writes() {
        let t = "\u{feff}l_english:\n # comment\n TYR_a:0 \"Text with \"quotes\" inside\"\n TYR_b: \"no number\" # trailing \"comment\"\n TYR_c:1 \"x\"\n";
        let f = parse(t);
        assert_eq!(f.language.as_deref(), Some("english"));
        let got: Vec<(&str, &str)> = f.entries.iter().map(|e| (e.key.as_str(), e.text.as_str())).collect();
        assert_eq!(got, [("TYR_a", "Text with \"quotes\" inside"), ("TYR_b", "no number"), ("TYR_c", "x")]);
        assert_eq!(f.entries[1].version, None);
        let t2 = set(t, "english", "TYR_c", "y\nz");
        assert!(t2.contains(" TYR_c:1 \"y\\nz\"\n"), "{t2}");
        let t3 = set(&t2, "english", "TYR_new", "n");
        assert!(t3.ends_with(" TYR_new:0 \"n\"\n"));
        let t4 = remove(&t3, "TYR_a").unwrap();
        assert!(!t4.contains("TYR_a") && t4.contains("TYR_b"));
        assert_eq!(set("", "german", "k", "v"), "\u{feff}l_german:\n k:0 \"v\"\n");
        assert_eq!(language_of_name("tyr_focus_l_german.yml"), Some("german"));
        assert_eq!(language_of_name("x.yml"), None);
    }

    #[test]
    fn names_from_ids() {
        assert_eq!(prettify("TYR_four_year_plan"), "Four Year Plan");
        assert_eq!(prettify("my_events.1.t"), "My Events");
        assert_eq!(plain("§YBold§! and £pol_power new\\nline"), "Bold and  new\nline");
    }
}
