//! Paradox script, the `key = value` / `key = { ... }` language of HOI4's
//! files, read without losing anything: every item keeps the byte span it
//! came from, so an edit replaces only the bytes it changes and the rest of
//! a hand-written file (comments, layout, odd spacing) stays as it was.
//!
//! [`Script::parse`] never fails. What the game would misread (an extra
//! `}`, a `{` never closed, a quote never closed, `key =` with no value) is
//! reported in [`Script::errors`] and the parse goes on, as the game does.
//!
//! The edit helpers ([`set_atom`], [`insert_item`], [`remove_item`], ...)
//! return an [`Edit`]: a byte range and its replacement. [`apply`] applies
//! a list of them; the file is parsed again after.

use std::ops::Range;

pub type Span = Range<usize>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Eq,
    EqEq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    /// `?=`: "equals, if it exists".
    QEq,
}

impl Op {
    pub fn as_str(self) -> &'static str {
        match self {
            Op::Eq => "=",
            Op::EqEq => "==",
            Op::Ne => "!=",
            Op::Lt => "<",
            Op::Le => "<=",
            Op::Gt => ">",
            Op::Ge => ">=",
            Op::QEq => "?=",
        }
    }
}

/// A word or a quoted string. `text` is without the quotes.
#[derive(Clone, Debug, PartialEq)]
pub struct Atom {
    pub text: String,
    pub quoted: bool,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Atom(Atom),
    Block(Block),
    /// `rgb { 1 2 3 }`, `hsv { ... }`: a word before a block.
    Tagged(Atom, Block),
    /// `key =` and nothing after it.
    None,
}

/// One entry of a block: `key = value`, `key < 3`, or a bare value of a list
/// (`{ 1 2 3 }`, `{ { ... } { ... } }`).
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub key: Option<Atom>,
    pub op: Option<(Op, Span)>,
    pub value: Value,
    /// From the key (or the bare value) to the end of the value.
    pub span: Span,
}

/// The items between `{` and `}`; the root block of a file has no braces
/// (`open` is None, `close` the end of the text).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Block {
    pub items: Vec<Item>,
    /// Byte of the `{`.
    pub open: Option<usize>,
    /// Byte of the `}` (the end of the text when it is never closed).
    pub close: usize,
    pub closed: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SyntaxError {
    pub at: usize,
    pub message: String,
}

#[derive(Clone, Debug, Default)]
pub struct Script {
    pub root: Block,
    pub errors: Vec<SyntaxError>,
}

// ------------------------------------------------------------------ items

impl Item {
    pub fn key(&self) -> &str {
        self.key.as_ref().map(|k| k.text.as_str()).unwrap_or("")
    }
    pub fn is(&self, key: &str) -> bool {
        self.key.as_ref().is_some_and(|k| k.text.eq_ignore_ascii_case(key))
    }
    pub fn atom(&self) -> Option<&Atom> {
        match &self.value {
            Value::Atom(a) => Some(a),
            _ => None,
        }
    }
    pub fn text(&self) -> Option<&str> {
        self.atom().map(|a| a.text.as_str())
    }
    pub fn block(&self) -> Option<&Block> {
        match &self.value {
            Value::Block(b) | Value::Tagged(_, b) => Some(b),
            _ => None,
        }
    }
    /// The bytes of the value (the atom with its quotes, or `{ ... }`).
    pub fn value_span(&self) -> Option<Span> {
        match &self.value {
            Value::Atom(a) => Some(a.span.clone()),
            Value::Block(b) => Some(b.open.unwrap_or(b.close)..(b.close + b.closed as usize)),
            Value::Tagged(t, b) => Some(t.span.start..(b.close + b.closed as usize)),
            Value::None => None,
        }
    }
}

impl Block {
    pub fn get(&self, key: &str) -> Option<&Item> {
        self.items.iter().find(|i| i.is(key))
    }
    pub fn all<'a>(&'a self, key: &'a str) -> impl Iterator<Item = &'a Item> + 'a {
        self.items.iter().filter(move |i| i.is(key))
    }
    /// The text of the first `key = word`.
    pub fn atom(&self, key: &str) -> Option<&str> {
        self.items.iter().filter(|i| i.is(key)).find_map(|i| i.text())
    }
    pub fn block(&self, key: &str) -> Option<&Block> {
        self.items.iter().filter(|i| i.is(key)).find_map(|i| i.block())
    }
    pub fn blocks<'a>(&'a self, key: &'a str) -> impl Iterator<Item = &'a Block> + 'a {
        self.all(key).filter_map(|i| i.block())
    }
    /// Bare words of a list block (`{ a b c }`).
    pub fn values(&self) -> impl Iterator<Item = &str> {
        self.items.iter().filter(|i| i.key.is_none()).filter_map(|i| i.text())
    }
    /// `key = yes`.
    pub fn yes(&self, key: &str) -> bool {
        self.atom(key).is_some_and(|v| v.eq_ignore_ascii_case("yes"))
    }
    pub fn num(&self, key: &str) -> Option<f64> {
        self.atom(key).and_then(|v| v.parse().ok())
    }
    /// The bytes between the braces (all of the text for the root).
    pub fn inner(&self) -> Span {
        match self.open {
            Some(o) => o + 1..self.close,
            None => 0..self.close,
        }
    }
    /// All items, depth first (the items of nested blocks after their parent).
    pub fn walk(&self, f: &mut dyn FnMut(&Item, usize)) {
        fn go(b: &Block, depth: usize, f: &mut dyn FnMut(&Item, usize)) {
            for i in &b.items {
                f(i, depth);
                if let Some(c) = i.block() {
                    go(c, depth + 1, f);
                }
            }
        }
        go(self, 0, f)
    }
}

// ------------------------------------------------------------------ lexer

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Open,
    Close,
    Op(Op),
    Word(String),
    Str(String),
}

struct Lexer<'a> {
    b: &'a [u8],
    s: &'a str,
    i: usize,
    errors: Vec<SyntaxError>,
}

fn is_word_byte(c: u8) -> bool {
    !(c.is_ascii_whitespace() || matches!(c, b'{' | b'}' | b'=' | b'<' | b'>' | b'#' | b'"'))
}

impl<'a> Lexer<'a> {
    fn skip_space(&mut self) {
        while self.i < self.b.len() {
            let c = self.b[self.i];
            if c.is_ascii_whitespace() {
                self.i += 1;
            } else if c == b'#' {
                while self.i < self.b.len() && self.b[self.i] != b'\n' {
                    self.i += 1;
                }
            } else {
                break;
            }
        }
    }

    /// The next token and its span.
    fn next(&mut self) -> Option<(Tok, Span)> {
        self.skip_space();
        let b = self.b;
        let s = self.i;
        if s >= b.len() {
            return None;
        }
        let two = |c: u8| b.get(s + 1) == Some(&c);
        let c = b[s];
        let (tok, len) = match c {
            b'{' => (Tok::Open, 1),
            b'}' => (Tok::Close, 1),
            b'=' if two(b'=') => (Tok::Op(Op::EqEq), 2),
            b'=' => (Tok::Op(Op::Eq), 1),
            b'<' if two(b'=') => (Tok::Op(Op::Le), 2),
            b'<' => (Tok::Op(Op::Lt), 1),
            b'>' if two(b'=') => (Tok::Op(Op::Ge), 2),
            b'>' => (Tok::Op(Op::Gt), 1),
            b'!' if two(b'=') => (Tok::Op(Op::Ne), 2),
            b'?' if two(b'=') => (Tok::Op(Op::QEq), 2),
            b'"' => {
                let mut j = s + 1;
                while j < b.len() && b[j] != b'"' {
                    if b[j] == b'\\' {
                        j += 1;
                    }
                    j += 1;
                }
                if j >= b.len() {
                    self.errors.push(SyntaxError { at: s, message: "a quote is never closed".into() });
                    let t = self.s[s + 1..].to_string();
                    self.i = b.len();
                    return Some((Tok::Str(t), s..b.len()));
                }
                (Tok::Str(self.s[s + 1..j].to_string()), j + 1 - s)
            }
            _ => {
                let mut j = s;
                if b[j..].starts_with(b"@[") {
                    // inline maths: @[ a + b ] with spaces in it
                    while j < b.len() && b[j] != b']' && b[j] != b'\n' {
                        j += 1;
                    }
                    j = (j + 1).min(b.len());
                } else {
                    while j < b.len() && is_word_byte(b[j]) && !(b[j] == b'!' && b.get(j + 1) == Some(&b'=')) && !(b[j] == b'?' && b.get(j + 1) == Some(&b'=')) {
                        j += 1;
                    }
                    if j == s {
                        j += 1;
                    }
                }
                // a word ends at a byte that starts a char
                while j < b.len() && !self.s.is_char_boundary(j) {
                    j += 1;
                }
                (Tok::Word(self.s[s..j].to_string()), j - s)
            }
        };
        self.i = s + len;
        Some((tok, s..s + len))
    }
}

// ------------------------------------------------------------------ parser

struct Parser<'a> {
    lx: Lexer<'a>,
    peeked: Option<Option<(Tok, Span)>>,
}

impl<'a> Parser<'a> {
    fn peek(&mut self) -> Option<&(Tok, Span)> {
        if self.peeked.is_none() {
            self.peeked = Some(self.lx.next());
        }
        self.peeked.as_ref().unwrap().as_ref()
    }
    fn bump(&mut self) -> Option<(Tok, Span)> {
        match self.peeked.take() {
            Some(t) => t,
            None => self.lx.next(),
        }
    }
    fn err(&mut self, at: usize, message: impl Into<String>) {
        self.lx.errors.push(SyntaxError { at, message: message.into() });
    }

    fn atom(tok: Tok, span: Span) -> Option<Atom> {
        match tok {
            Tok::Word(t) => Some(Atom { text: t, quoted: false, span }),
            Tok::Str(t) => Some(Atom { text: t, quoted: true, span }),
            _ => None,
        }
    }

    /// Items until the `}` of a block opened at `open` (None: the root).
    fn block(&mut self, open: Option<usize>, depth: usize) -> Block {
        let mut items = Vec::new();
        loop {
            let Some((tok, span)) = self.bump() else {
                let end = self.lx.b.len();
                if let Some(o) = open {
                    self.err(o, "a '{' is never closed (the game skips the rest of the file)");
                }
                return Block { items, open, close: end, closed: false };
            };
            match tok {
                Tok::Close => {
                    if open.is_some() {
                        return Block { items, open, close: span.start, closed: true };
                    }
                    self.err(span.start, "an extra '}' (the game misreads what follows)");
                }
                Tok::Open => {
                    if depth > 200 {
                        self.err(span.start, "blocks nested too deep");
                        continue;
                    }
                    let b = self.block(Some(span.start), depth + 1);
                    let end = b.close + b.closed as usize;
                    items.push(Item { key: None, op: None, value: Value::Block(b), span: span.start..end });
                }
                Tok::Op(op) => {
                    self.err(span.start, format!("'{}' without a key before it", op.as_str()));
                }
                t @ (Tok::Word(_) | Tok::Str(_)) => {
                    let key = Self::atom(t, span.clone()).unwrap();
                    if let Some((Tok::Op(op), ospan)) = self.peek().cloned() {
                        self.bump();
                        let value = self.value(ospan.end, depth);
                        let end = match &value {
                            Value::None => ospan.end,
                            Value::Atom(a) => a.span.end,
                            Value::Block(b) | Value::Tagged(_, b) => b.close + b.closed as usize,
                        };
                        items.push(Item { key: Some(key), op: Some((op, ospan)), value, span: span.start..end });
                    } else if matches!(self.peek(), Some((Tok::Open, _))) && is_tag_word(&key.text) {
                        // a bare rgb { } in a list
                        let (_, o) = self.bump().unwrap();
                        let b = self.block(Some(o.start), depth + 1);
                        let end = b.close + b.closed as usize;
                        items.push(Item { key: None, op: None, value: Value::Tagged(key, b), span: span.start..end });
                    } else {
                        let sp = key.span.clone();
                        items.push(Item { key: None, op: None, value: Value::Atom(key), span: sp });
                    }
                }
            }
        }
    }

    fn value(&mut self, after: usize, depth: usize) -> Value {
        match self.peek().cloned() {
            Some((Tok::Open, s)) => {
                self.bump();
                Value::Block(self.block(Some(s.start), depth + 1))
            }
            Some((t @ (Tok::Word(_) | Tok::Str(_)), s)) => {
                self.bump();
                let a = Self::atom(t, s).unwrap();
                if !a.quoted && matches!(self.peek(), Some((Tok::Open, _))) {
                    let (_, o) = self.bump().unwrap();
                    let b = self.block(Some(o.start), depth + 1);
                    Value::Tagged(a, b)
                } else {
                    Value::Atom(a)
                }
            }
            _ => {
                self.err(after, "'=' with no value after it");
                Value::None
            }
        }
    }
}

/// Words that may stand before a block as a value (`color = rgb { ... }`).
fn is_tag_word(w: &str) -> bool {
    matches!(w.to_ascii_lowercase().as_str(), "rgb" | "hsv" | "hsv360" | "hex" | "list")
}

impl Script {
    pub fn parse(text: &str) -> Script {
        let mut p = Parser { lx: Lexer { b: text.as_bytes(), s: text, i: 0, errors: Vec::new() }, peeked: None };
        let root = p.block(None, 0);
        Script { root, errors: p.lx.errors }
    }
}

// ------------------------------------------------------------------ text helpers

/// Line (1-based) of a byte.
pub fn line_of(text: &str, at: usize) -> usize {
    text.as_bytes()[..at.min(text.len())].iter().filter(|&&c| c == b'\n').count() + 1
}

/// Byte where the line of `at` starts.
pub fn line_start(text: &str, at: usize) -> usize {
    text[..at.min(text.len())].rfind('\n').map(|i| i + 1).unwrap_or(0)
}

/// Byte where the line of `at` ends (its `\n`, or the end of the text).
pub fn line_end(text: &str, at: usize) -> usize {
    text[at.min(text.len())..].find('\n').map(|i| at + i).unwrap_or(text.len())
}

/// The whitespace at the start of the line of `at`.
pub fn indent_at(text: &str, at: usize) -> &str {
    let s = line_start(text, at);
    let line = &text[s..];
    let n = line.len() - line.trim_start_matches([' ', '\t']).len();
    &line[..n]
}

/// Only whitespace between the start of its line and `at`.
fn starts_line(text: &str, at: usize) -> bool {
    text[line_start(text, at)..at].trim().is_empty()
}

/// Only whitespace or a comment from `at` to the end of its line.
fn ends_line(text: &str, at: usize) -> bool {
    let rest = &text[at..line_end(text, at)];
    let t = rest.trim_start();
    t.is_empty() || t.starts_with('#')
}

/// Needs quotes to be read back as one word.
pub fn needs_quotes(s: &str) -> bool {
    s.is_empty() || s.bytes().any(|c| !is_word_byte(c) || c == b'!' || c == b'?')
}

pub fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// A word as written in a file: bare if it can be, else quoted.
pub fn word(s: &str) -> String {
    if needs_quotes(s) { quote(s) } else { s.to_string() }
}

/// `line` indented: every line after the first gets `indent` in front.
pub fn reindent(text: &str, indent: &str) -> String {
    let mut out = String::new();
    for (i, l) in text.lines().enumerate() {
        if i > 0 {
            out.push('\n');
            if !l.trim().is_empty() {
                out.push_str(indent);
            }
        }
        out.push_str(l);
    }
    out
}

/// Whitespace and comments fixed up: each line indented by one tab per open
/// brace. Words, strings and comments are left as they are.
pub fn format(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + text.len() / 8);
    let mut depth: i32 = 0;
    let mut in_str = false;
    for line in text.lines() {
        let t = line.trim();
        if in_str {
            out.push_str(line);
        } else if t.is_empty() {
            // keep blank lines, without their spaces
        } else {
            let lead_close = t.starts_with('}') as i32;
            for _ in 0..(depth - lead_close).max(0) {
                out.push('\t');
            }
            out.push_str(t);
        }
        out.push('\n');
        // braces of this line, outside strings and comments
        let b = line.as_bytes();
        let mut i = 0;
        while i < b.len() {
            let c = b[i];
            if in_str {
                if c == b'\\' {
                    i += 1;
                } else if c == b'"' {
                    in_str = false;
                }
            } else {
                match c {
                    b'"' => in_str = true,
                    b'#' => break,
                    b'{' => depth += 1,
                    b'}' => depth = (depth - 1).max(0),
                    _ => {}
                }
            }
            i += 1;
        }
    }
    if !text.ends_with('\n') {
        out.pop();
    }
    out
}

// ------------------------------------------------------------------ edits

/// Replace `range` with `text`.
#[derive(Clone, Debug, PartialEq)]
pub struct Edit {
    pub range: Span,
    pub text: String,
}

/// Apply edits (ranges must not overlap); returns the new text.
pub fn apply(text: &str, edits: &[Edit]) -> String {
    let mut e: Vec<&Edit> = edits.iter().collect();
    e.sort_by_key(|x| (x.range.start, x.range.end));
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for x in e {
        let s = x.range.start.max(at);
        out.push_str(&text[at..s]);
        out.push_str(&x.text);
        at = x.range.end.max(s);
    }
    out.push_str(&text[at.min(text.len())..]);
    out
}

/// The indentation for a new item of `block`: that of its first item on a
/// line of its own, else one tab more than the line of its `{`.
pub fn child_indent(text: &str, block: &Block) -> String {
    if let Some(i) = block.items.iter().find(|i| starts_line(text, i.span.start)) {
        return indent_at(text, i.span.start).to_string();
    }
    match block.open {
        Some(o) => format!("{}\t", indent_at(text, o)),
        None => String::new(),
    }
}

/// Insert `rendered` (an item, maybe several lines indented relative to its
/// first) into `block`, after its item `after` (None: at the end).
pub fn insert_item(text: &str, block: &Block, rendered: &str, after: Option<usize>) -> Edit {
    let indent = child_indent(text, block);
    let body = reindent(rendered.trim_end(), &indent);
    let anchor = match after {
        Some(i) if i < block.items.len() => Some(block.items[i].span.end),
        _ => block.items.last().map(|i| i.span.end),
    };
    match (block.open, anchor) {
        (None, Some(end)) => {
            let e = line_end(text, end);
            Edit { range: e..e, text: format!("\n{body}") }
        }
        (None, None) => {
            let at = text.len();
            let sep = if text.is_empty() || text.ends_with('\n') { "" } else { "\n" };
            Edit { range: at..at, text: format!("{sep}{body}\n") }
        }
        (Some(o), _) => {
            let inline = line_start(text, o) == line_start(text, block.close) && block.closed;
            if inline && !rendered.contains('\n') && block.items.len() < 4 {
                // a one-line block stays one line: { a = b NEW }
                let pos = match anchor {
                    Some(end) => end,
                    None => o + 1,
                };
                return Edit { range: pos..pos, text: format!(" {}", rendered.trim()) };
            }
            match anchor {
                Some(end) if !inline => {
                    let e = if ends_line(text, end) { line_end(text, end) } else { end };
                    Edit { range: e..e, text: format!("\n{indent}{body}") }
                }
                _ => {
                    // an empty block, or a one-line one: the item goes on a line of its own before the `}`
                    let close_indent = indent_at(text, o).to_string();
                    let before = text[..block.close].trim_end().len().max(o + 1);
                    let indent = if block.items.is_empty() || inline { format!("{close_indent}\t") } else { indent };
                    let body = reindent(rendered.trim_end(), &indent);
                    Edit { range: before..block.close, text: format!("\n{indent}{body}\n{close_indent}") }
                }
            }
        }
    }
}

/// Remove an item; when it has its line(s) to itself the lines go too.
pub fn remove_item(text: &str, item: &Item) -> Edit {
    let (s, e) = (item.span.start, item.span.end);
    if starts_line(text, s) && ends_line(text, e) {
        let ls = line_start(text, s);
        let le = line_end(text, e);
        let le = if le < text.len() { le + 1 } else { le };
        // a trailing comment on the line goes with it
        return Edit { range: ls..le, text: String::new() };
    }
    let mut e2 = e;
    while e2 < text.len() && text.as_bytes()[e2] == b' ' {
        e2 += 1;
    }
    Edit { range: s..e2, text: String::new() }
}

/// Replace an item with `rendered` (indented like the item).
pub fn replace_item(text: &str, item: &Item, rendered: &str) -> Edit {
    let indent = indent_at(text, item.span.start).to_string();
    Edit { range: item.span.clone(), text: reindent(rendered.trim_end(), &indent) }
}

/// Set `key = value` in `block`: the first such item's value is replaced (an
/// atom keeps its quotes if it had them), or the item is added at the end.
/// `value` is written as given (use [`word`] or [`quote`]).
pub fn set_atom(text: &str, block: &Block, key: &str, value: &str) -> Edit {
    if let Some(it) = block.get(key) {
        if let Some(a) = it.atom() {
            let v = if a.quoted && !value.starts_with('"') { quote(value) } else { value.to_string() };
            return Edit { range: a.span.clone(), text: v };
        }
        if let Some(sp) = it.value_span() {
            return Edit { range: sp, text: value.to_string() };
        }
        let end = it.span.end;
        return Edit { range: end..end, text: format!(" {value}") };
    }
    insert_item(text, block, &format!("{key} = {value}"), None)
}

/// The contents of a block replaced by `inner` (lines indented one level in).
pub fn set_block_inner(text: &str, block: &Block, inner: &str) -> Edit {
    let Some(o) = block.open else {
        return Edit { range: 0..text.len(), text: inner.to_string() };
    };
    let outer = indent_at(text, o).to_string();
    let inner = inner.trim();
    if inner.is_empty() {
        return Edit { range: o + 1..block.close, text: " ".into() };
    }
    if !inner.contains('\n') && inner.len() < 60 && line_start(text, o) == line_start(text, block.close) {
        return Edit { range: o + 1..block.close, text: format!(" {inner} ") };
    }
    let ind = format!("{outer}\t");
    let body: Vec<String> = dedent(inner).lines().map(|l| if l.trim().is_empty() { String::new() } else { format!("{ind}{l}") }).collect();
    Edit { range: o + 1..block.close, text: format!("\n{}\n{outer}", body.join("\n")) }
}

/// Lines with their common leading whitespace removed (the first line, often
/// the rest of a `{` line, is not counted).
pub fn dedent(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let common = lines
        .iter()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start_matches([' ', '\t']).len())
        .min()
        .unwrap_or(0);
    let mut out = String::new();
    for (i, l) in lines.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        if i == 0 {
            out.push_str(l.trim_start());
        } else if l.len() >= common {
            out.push_str(&l[common..]);
        } else {
            out.push_str(l.trim_start());
        }
    }
    out
}

/// The text inside a block, dedented: what an editor shows for it.
pub fn inner_text(text: &str, block: &Block) -> String {
    let r = block.inner();
    let s = &text[r.start.min(text.len())..r.end.min(text.len())];
    dedent(s.trim_matches(['\n', '\r']).trim_end()).trim().to_string()
}

/// The bytes of an item.
pub fn item_text<'a>(text: &'a str, item: &Item) -> &'a str {
    &text[item.span.start.min(text.len())..item.span.end.min(text.len())]
}

/// An item as it would be pasted elsewhere: its first line as is, the next
/// ones without the indentation of the line it starts on.
pub fn item_source(text: &str, item: &Item) -> String {
    let indent = indent_at(text, item.span.start);
    let raw = item_text(text, item);
    let mut out = String::new();
    for (i, l) in raw.lines().enumerate() {
        if i > 0 {
            out.push('\n');
            out.push_str(l.strip_prefix(indent).unwrap_or(l.trim_start()));
        } else {
            out.push_str(l);
        }
    }
    out
}

/// Words of a token list in a text (identifiers, numbers, `ns.12` event ids):
/// (start, end) of each word outside strings and comments, for renames.
pub fn words(text: &str) -> Vec<(usize, usize)> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'#' {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if c == b'"' {
            // strings: the words in them count too (focus = "x", has_dlc = "...")
            let s = i + 1;
            i += 1;
            while i < b.len() && b[i] != b'"' && b[i] != b'\n' {
                i += 1;
            }
            out.extend(words(&text[s..i]).into_iter().map(|(a, z)| (a + s, z + s)));
            i += 1;
        } else if is_ident(c) {
            let s = i;
            while i < b.len() && is_ident(b[i]) {
                i += 1;
            }
            out.push((s, i));
        } else {
            i += 1;
        }
    }
    out
}

/// Bytes of an identifier as the game reads one (`GER_focus`, `ns.12`, `@var`).
pub fn is_ident(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'@' | b'-' | b':' | b'\'') || c >= 0x80
}

/// Replace whole words: each word of `text` found in `map` becomes its value.
pub fn rename_words(text: &str, map: &dyn Fn(&str) -> Option<String>) -> (String, usize) {
    let mut edits = Vec::new();
    for (s, e) in words(text) {
        if let Some(n) = map(&text[s..e]) {
            edits.push(Edit { range: s..e, text: n });
        }
    }
    let n = edits.len();
    (apply(text, &edits), n)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# a comment\nfocus_tree = {\n\tid = my_tree\n\tcountry = { factor = 0 modifier = { add = 10 tag = TYR } }\n\tfocus = {\n\t\tid = TYR_one\n\t\ticon = GFX_goal_generic_construct_infrastructure\n\t\tx = 3 y = 0\n\t\tcost = 10\n\t\tprerequisite = { focus = TYR_zero focus = \"TYR_alt\" }\n\t\tcompletion_reward = {\n\t\t\tadd_political_power = 120 # pp\n\t\t}\n\t}\n}\ncolor = rgb { 10 20 30 }\nlist = { 1 2 3 }\nlimit = { num_of_factories > 5 has_war ?= yes }\n";

    #[test]
    fn parses_and_keeps_spans() {
        let s = Script::parse(SAMPLE);
        assert!(s.errors.is_empty(), "{:?}", s.errors);
        let tree = s.root.block("focus_tree").unwrap();
        assert_eq!(tree.atom("id"), Some("my_tree"));
        let f = tree.block("focus").unwrap();
        assert_eq!(f.atom("id"), Some("TYR_one"));
        assert_eq!(f.num("x"), Some(3.0));
        assert_eq!(f.num("y"), Some(0.0));
        let pre = f.block("prerequisite").unwrap();
        let ids: Vec<&str> = pre.all("focus").filter_map(|i| i.text()).collect();
        assert_eq!(ids, ["TYR_zero", "TYR_alt"]);
        assert!(pre.get("focus").unwrap().text().is_some());
        let c = s.root.get("color").unwrap();
        assert!(matches!(&c.value, Value::Tagged(t, b) if t.text == "rgb" && b.values().count() == 3));
        assert_eq!(s.root.block("list").unwrap().values().collect::<Vec<_>>(), ["1", "2", "3"]);
        let lim = s.root.block("limit").unwrap();
        assert_eq!(lim.items[0].op.as_ref().unwrap().0, Op::Gt);
        assert_eq!(lim.items[1].op.as_ref().unwrap().0, Op::QEq);
        // spans give back the bytes
        let item = tree.get("focus").unwrap();
        assert!(item_text(SAMPLE, item).starts_with("focus = {") && item_text(SAMPLE, item).ends_with('}'));
        assert_eq!(&SAMPLE[f.get("icon").unwrap().atom().unwrap().span.clone()], "GFX_goal_generic_construct_infrastructure");
    }

    #[test]
    fn errors_are_reported_not_fatal() {
        let s = Script::parse("a = { b = c\nd = e");
        assert_eq!(s.errors.len(), 1);
        assert!(s.errors[0].message.contains("never closed"));
        assert_eq!(s.root.items.len(), 1);
        assert_eq!(s.root.block("a").unwrap().items.len(), 2);
        let s = Script::parse("a = b }\nc = d");
        assert!(s.errors[0].message.contains("extra"));
        assert_eq!(s.root.items.len(), 2);
        let s = Script::parse("a = \nb = c");
        // `a = b`, then `= c`: a key with no value is how the game reads it too
        assert_eq!(s.root.atom("a"), Some("b"));
        let s = Script::parse("x = \"never closed\ny = 2");
        assert!(s.errors[0].message.contains("quote"));
        let s = Script::parse("x = { }\n}");
        assert_eq!(s.errors.len(), 1);
        let s = Script::parse("v = @[ a + 1 ]\nw = 2");
        assert_eq!(s.root.atom("v"), Some("@[ a + 1 ]"));
        assert_eq!(s.root.atom("w"), Some("2"));
    }

    #[test]
    fn edits_touch_only_their_bytes() {
        let s = Script::parse(SAMPLE);
        let f = s.root.block("focus_tree").unwrap().block("focus").unwrap();
        let e = vec![set_atom(SAMPLE, f, "x", "7"), set_atom(SAMPLE, f, "cost", "5")];
        let t = apply(SAMPLE, &e);
        assert_eq!(t, SAMPLE.replace("x = 3", "x = 7").replace("cost = 10", "cost = 5"));
        // a new field goes on its own line, indented like the others
        let t2 = apply(SAMPLE, &[set_atom(SAMPLE, f, "available_if_capitulated", "yes")]);
        assert!(t2.contains("\t\t}\n\t\tavailable_if_capitulated = yes\n\t}"), "{t2}");
        let s2 = Script::parse(&t2);
        assert!(s2.errors.is_empty());
        assert!(s2.root.block("focus_tree").unwrap().block("focus").unwrap().yes("available_if_capitulated"));
        // removing a line-of-its-own item takes the line
        let cr = f.get("completion_reward").unwrap();
        let t3 = apply(SAMPLE, &[remove_item(SAMPLE, cr)]);
        assert!(!t3.contains("completion_reward") && !t3.contains("add_political_power"));
        assert!(t3.contains("\t\tprerequisite = { focus = TYR_zero focus = \"TYR_alt\" }\n\t}"), "{t3}");
        // removing an item sharing its line keeps the line
        let t4 = apply(SAMPLE, &[remove_item(SAMPLE, f.get("x").unwrap())]);
        assert!(t4.contains("\t\ty = 0\n"), "{t4}");
        // quoted values stay quoted
        let pre = f.block("prerequisite").unwrap();
        let alt = pre.all("focus").nth(1).unwrap();
        let t5 = apply(SAMPLE, &[Edit { range: alt.atom().unwrap().span.clone(), text: quote("TYR_b") }]);
        assert!(t5.contains("focus = \"TYR_b\""));
        // block contents
        let t6 = apply(SAMPLE, &[set_block_inner(SAMPLE, f.block("completion_reward").unwrap(), "add_stability = 0.1\nadd_war_support = 0.05")]);
        assert!(t6.contains("completion_reward = {\n\t\t\tadd_stability = 0.1\n\t\t\tadd_war_support = 0.05\n\t\t}"), "{t6}");
    }

    #[test]
    fn inserting_into_blocks() {
        let text = "a = {\n\tb = 1\n}\nc = { }\nd = { x = 1 }\n";
        let s = Script::parse(text);
        let t = apply(text, &[insert_item(text, s.root.block("a").unwrap(), "e = {\n\tf = 2\n}", None)]);
        assert_eq!(t, "a = {\n\tb = 1\n\te = {\n\t\tf = 2\n\t}\n}\nc = { }\nd = { x = 1 }\n");
        let t = apply(text, &[insert_item(text, s.root.block("c").unwrap(), "y = 2", None)]);
        assert!(t.contains("c = { y = 2 }"), "{t}");
        let t = apply(text, &[insert_item(text, s.root.block("c").unwrap(), "y = {\n\tz = 1\n}", None)]);
        assert!(t.contains("c = {\n\ty = {\n\t\tz = 1\n\t}\n}"), "{t}");
        let t = apply(text, &[insert_item(text, s.root.block("d").unwrap(), "y = 2", None)]);
        assert!(t.contains("d = { x = 1 y = 2 }"), "{t}");
        let t = apply(text, &[insert_item(text, &s.root, "z = 9", None)]);
        assert!(t.ends_with("d = { x = 1 }\nz = 9\n"), "{t:?}");
        assert_eq!(apply("", &[insert_item("", &Script::parse("").root, "k = v", None)]), "k = v\n");
    }

    #[test]
    fn formatting_and_words() {
        let t = "a = {\nb = {\n  c = 1\n}\n   }\n";
        assert_eq!(format(t), "a = {\n\tb = {\n\t\tc = 1\n\t}\n}\n");
        let t = "x = \"a { b\"\ny = { # { not counted\nz = 1\n}\n";
        assert_eq!(format(t), "x = \"a { b\"\ny = { # { not counted\n\tz = 1\n}\n");
        let text = "focus = GER_rhineland # GER_rhineland\nhas_completed_focus = \"GER_rhineland\"\nx = GER_rhineland_desc";
        let (out, n) = rename_words(text, &|w| (w == "GER_rhineland").then(|| "TYR_rhineland".to_string()));
        assert_eq!(n, 2);
        assert_eq!(out, "focus = TYR_rhineland # GER_rhineland\nhas_completed_focus = \"TYR_rhineland\"\nx = GER_rhineland_desc");
        assert_eq!(word("a b"), "\"a b\"");
        assert_eq!(word("GFX_x"), "GFX_x");
        assert_eq!(dedent("{\n\t\ta = 1\n\t\tb = {\n\t\t\tc\n\t\t}"), "{\na = 1\nb = {\n\tc\n}");
    }
}
