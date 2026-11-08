//! A code editor for Paradox script and localisation: colours for keys,
//! strings, numbers, comments, `yes`/`no`, sprites and variables, lines
//! with errors marked, and a jump to a line.

use std::sync::Arc;

use eframe::egui::{self, Color32, FontId, TextFormat, text::LayoutJob};

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
pub enum Lang {
    Script,
    Loc,
    Plain,
}

impl Lang {
    pub fn of_path(path: &str) -> Lang {
        let p = path.to_ascii_lowercase();
        if p.ends_with(".yml") {
            Lang::Loc
        } else if p.ends_with(".txt") || p.ends_with(".gfx") || p.ends_with(".gui") || p.ends_with(".asset") {
            Lang::Script
        } else {
            Lang::Plain
        }
    }
}

struct Palette {
    text: Color32,
    key: Color32,
    string: Color32,
    number: Color32,
    comment: Color32,
    keyword: Color32,
    sprite: Color32,
    var: Color32,
    punct: Color32,
    error_bg: Color32,
}

fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            text: Color32::from_rgb(214, 214, 214),
            key: Color32::from_rgb(120, 180, 255),
            string: Color32::from_rgb(152, 205, 120),
            number: Color32::from_rgb(240, 170, 100),
            comment: Color32::from_rgb(120, 125, 130),
            keyword: Color32::from_rgb(200, 140, 230),
            sprite: Color32::from_rgb(90, 210, 200),
            var: Color32::from_rgb(235, 210, 110),
            punct: Color32::from_rgb(160, 160, 160),
            error_bg: Color32::from_rgba_unmultiplied(200, 40, 40, 70),
        }
    } else {
        Palette {
            text: Color32::from_rgb(30, 30, 30),
            key: Color32::from_rgb(20, 80, 180),
            string: Color32::from_rgb(40, 120, 30),
            number: Color32::from_rgb(170, 90, 0),
            comment: Color32::from_rgb(120, 120, 120),
            keyword: Color32::from_rgb(130, 40, 160),
            sprite: Color32::from_rgb(0, 120, 120),
            var: Color32::from_rgb(140, 110, 0),
            punct: Color32::from_rgb(90, 90, 90),
            error_bg: Color32::from_rgba_unmultiplied(230, 80, 80, 60),
        }
    }
}

/// The coloured layout of a text (`error_lines`: 1-based lines to mark).
pub fn highlight(text: &str, lang: Lang, dark: bool, font: &FontId, error_lines: &[usize]) -> LayoutJob {
    let p = palette(dark);
    let mut job = LayoutJob::default();
    let fmt = |c: Color32| TextFormat::simple(font.clone(), c);
    let b = text.as_bytes();
    let mut line_starts = vec![0usize];
    for (i, &c) in b.iter().enumerate() {
        if c == b'\n' {
            line_starts.push(i + 1);
        }
    }
    let err_ranges: Vec<std::ops::Range<usize>> = error_lines
        .iter()
        .filter_map(|&l| {
            let s = *line_starts.get(l.checked_sub(1)?)?;
            let e = line_starts.get(l).copied().unwrap_or(b.len());
            Some(s..e)
        })
        .collect();
    let push = |job: &mut LayoutJob, r: std::ops::Range<usize>, c: Color32| {
        if r.is_empty() {
            return;
        }
        // split where error lines start and end, so they get their background
        let mut cuts = vec![r.start, r.end];
        for e in &err_ranges {
            for x in [e.start, e.end] {
                if x > r.start && x < r.end {
                    cuts.push(x);
                }
            }
        }
        cuts.sort_unstable();
        cuts.dedup();
        for w in cuts.windows(2) {
            let mut f = fmt(c);
            if err_ranges.iter().any(|e| e.start <= w[0] && w[0] < e.end) {
                f.background = p.error_bg;
            }
            job.append(&text[w[0]..w[1]], 0.0, f);
        }
    };
    if lang == Lang::Plain {
        push(&mut job, 0..b.len(), p.text);
        return job;
    }
    let ident = |c: u8| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'@' | b'-' | b':' | b'\'' | b'^') || c >= 0x80;
    let mut i = 0;
    let mut line_start = true;
    while i < b.len() {
        let c = b[i];
        if c == b'#' && (lang == Lang::Script || line_start) {
            let s = i;
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            push(&mut job, s..i, p.comment);
        } else if c == b'"' {
            let s = i;
            i += 1;
            while i < b.len() && b[i] != b'"' && b[i] != b'\n' {
                if b[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
            i = (i + 1).min(b.len());
            if lang == Lang::Loc {
                loc_string(&text[s..i], s, &p, &mut |r, c| push(&mut job, r, c));
            } else {
                push(&mut job, s..i, p.string);
            }
        } else if c == b'\n' {
            push(&mut job, i..i + 1, p.text);
            i += 1;
            line_start = true;
            continue;
        } else if c.is_ascii_whitespace() {
            let s = i;
            while i < b.len() && b[i].is_ascii_whitespace() && b[i] != b'\n' {
                i += 1;
            }
            push(&mut job, s..i, p.text);
            continue;
        } else if ident(c) && (lang == Lang::Script || line_start) {
            let s = i;
            while i < b.len() && ident(b[i]) {
                i += 1;
            }
            let w = &text[s..i];
            if lang == Lang::Loc {
                // key:0
                push(&mut job, s..i, p.key);
            } else {
                let mut j = i;
                while j < b.len() && (b[j] == b' ' || b[j] == b'\t') {
                    j += 1;
                }
                let is_key = j < b.len() && matches!(b[j], b'=' | b'<' | b'>' | b'!' | b'?');
                let col = if is_key {
                    p.key
                } else if w == "yes" || w == "no" {
                    p.keyword
                } else if w.starts_with("GFX_") || w.starts_with("gfx_") {
                    p.sprite
                } else if w.starts_with('@') || w.starts_with("var:") {
                    p.var
                } else if w.trim_start_matches('-').parse::<f64>().is_ok() || w.split('.').all(|x| x.parse::<u32>().is_ok()) {
                    p.number
                } else {
                    p.text
                };
                push(&mut job, s..i, col);
            }
        } else {
            let s = i;
            i += text[i..].chars().next().map_or(1, char::len_utf8);
            push(&mut job, s..i, p.punct);
        }
        line_start = false;
    }
    job
}

/// The parts of a localisation string: §colour codes, [scripted] bits, $variables$.
fn loc_string(s: &str, at: usize, p: &Palette, push: &mut dyn FnMut(std::ops::Range<usize>, Color32)) {
    let b = s.as_bytes();
    let mut i = 0;
    let mut run = 0;
    while i < b.len() {
        let special = match b[i] {
            b'[' => b[i..].iter().position(|&c| c == b']').map(|e| (i + e + 1, p.number)),
            b'$' => b[i + 1..].iter().position(|&c| c == b'$').map(|e| (i + e + 2, p.var)),
            0xC2 if b.get(i + 1) == Some(&0xA7) => Some(((i + 3).min(b.len()), p.keyword)),
            _ => None,
        };
        match special {
            Some((e, col)) => {
                push(at + run..at + i, p.string);
                push(at + i..at + e, col);
                i = e;
                run = e;
            }
            None => i += 1,
        }
    }
    push(at + run..at + b.len(), p.string);
}

#[derive(Default)]
struct Highlighter;

impl egui::cache::ComputerMut<(&str, Lang, bool, u64, &[usize]), Arc<LayoutJob>> for Highlighter {
    fn compute(&mut self, (text, lang, dark, _font, errs): (&str, Lang, bool, u64, &[usize])) -> Arc<LayoutJob> {
        Arc::new(highlight(text, lang, dark, &FontId::monospace(12.5), errs))
    }
}

type Cache = egui::cache::FrameCache<Arc<LayoutJob>, Highlighter>;

/// What the editor did this frame.
pub struct Out {
    pub changed: bool,
    pub response: egui::Response,
    /// The cursor's line (1-based), when it has focus.
    pub cursor_line: Option<usize>,
}

/// The editor. `jump`: a line to put the cursor on and show (taken).
pub fn editor(ui: &mut egui::Ui, id: egui::Id, text: &mut String, lang: Lang, error_lines: &[usize], rows: usize, jump: &mut Option<usize>) -> Out {
    let dark = ui.visuals().dark_mode;
    let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap: f32| {
        let job = ui.ctx().memory_mut(|m| m.caches.cache::<Cache>().get((buf.as_str(), lang, dark, 0u64, error_lines)).clone());
        let mut job = (*job).clone();
        job.wrap.max_width = wrap;
        ui.fonts_mut(|f| f.layout_job(job))
    };
    let row_h = ui.fonts_mut(|f| f.row_height(&FontId::monospace(12.5)));
    let mut scroll = egui::ScrollArea::vertical().id_salt(id.with("scroll")).auto_shrink([false, false]).max_height(row_h * rows as f32 + 12.0);
    if let Some(l) = *jump {
        scroll = scroll.vertical_scroll_offset((l.saturating_sub(4) as f32 * row_h).max(0.0));
    }
    let mut cursor_line = None;
    let mut changed = false;
    let r = scroll.show(ui, |ui| {
        let te = egui::TextEdit::multiline(text)
            .id(id)
            .code_editor()
            .font(FontId::monospace(12.5))
            .desired_width(f32::INFINITY)
            .desired_rows(rows)
            .lock_focus(true)
            .layouter(&mut layouter);
        let mut out = te.show(ui);
        changed = out.response.response.changed();
        if let Some(l) = jump.take() {
            let at = out.galley.text().split_inclusive('\n').take(l.saturating_sub(1)).map(|x| x.chars().count()).sum::<usize>();
            let c = egui::text::CCursor::new(at);
            out.state.cursor.set_char_range(Some(egui::text::CCursorRange::one(c)));
            out.state.clone().store(ui.ctx(), id);
            out.response.response.request_focus();
        }
        if out.response.response.has_focus()
            && let Some(cr) = out.cursor_range
        {
            let idx = cr.primary.index.0;
            cursor_line = Some(out.galley.text().chars().take(idx).filter(|&c| c == '\n').count() + 1);
        }
        out.response.response
    });
    Out { changed, response: r.inner, cursor_line }
}

/// A read-only view of code (a definition in a source): it can be selected and copied.
pub fn view(ui: &mut egui::Ui, id: egui::Id, text: &str, lang: Lang, rows: usize) {
    let dark = ui.visuals().dark_mode;
    let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap: f32| {
        let job = ui.ctx().memory_mut(|m| m.caches.cache::<Cache>().get((buf.as_str(), lang, dark, 0u64, &[][..])).clone());
        let mut job = (*job).clone();
        job.wrap.max_width = wrap;
        ui.fonts_mut(|f| f.layout_job(job))
    };
    let row_h = ui.fonts_mut(|f| f.row_height(&FontId::monospace(12.5)));
    egui::ScrollArea::vertical().id_salt(id.with("scroll")).auto_shrink([false, true]).max_height(row_h * rows as f32 + 12.0).show(ui, |ui| {
        let mut t = text;
        ui.add(egui::TextEdit::multiline(&mut t).id(id).code_editor().font(FontId::monospace(12.5)).desired_width(f32::INFINITY).layouter(&mut layouter));
    });
}
