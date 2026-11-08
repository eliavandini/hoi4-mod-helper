//! The picture picker: every sprite of the content and the base game as a
//! grid of pictures, with a sidebar of the folders their pictures are in
//! (with counts) and filters: what kind of picture, whose (the content's or
//! the base game's), and words of the name or the file.

use std::collections::BTreeMap;
use std::sync::Arc;

use eframe::egui::{self, RichText};

use super::Cx;
use crate::content::kinds::Kind;

/// What kind of picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Cat {
    /// Named with the field's prefix (`GFX_idea_…`).
    Prefix,
    Focus,
    Idea,
    Event,
    Portrait,
    Decision,
    #[default]
    All,
}

impl Cat {
    fn label(self, prefix: &str) -> String {
        match self {
            Cat::Prefix => format!("{prefix}…"),
            Cat::Focus => "focus icons".into(),
            Cat::Idea => "idea pictures".into(),
            Cat::Event => "event pictures".into(),
            Cat::Portrait => "portraits".into(),
            Cat::Decision => "decision icons".into(),
            Cat::All => "all".into(),
        }
    }

    /// The kind of picture a field of a definition takes.
    pub fn of_field(kind: Kind, field: &str, prefix: &str) -> Cat {
        match (kind, field) {
            (Kind::Focus | Kind::SharedFocus, "icon") => Cat::Focus,
            (Kind::Event, "picture") => Cat::Event,
            (Kind::Character, _) => Cat::Portrait,
            _ if !prefix.is_empty() && prefix != "GFX_" => Cat::Prefix,
            _ => Cat::All,
        }
    }

    fn matches(self, name: &str, texture: &str, prefix: &str) -> bool {
        let n = name.to_ascii_lowercase();
        let t = texture.to_ascii_lowercase();
        // the shining copy of a focus icon is the same picture
        if self != Cat::All && n.ends_with("_shine") {
            return false;
        }
        match self {
            Cat::Prefix => n.starts_with(&prefix.to_ascii_lowercase()),
            Cat::Focus => n.starts_with("gfx_goal_") || n.starts_with("gfx_focus_") || t.contains("/goals/"),
            Cat::Idea => n.starts_with("gfx_idea_") || t.contains("/ideas/"),
            Cat::Event => t.contains("event_pictures") || n.starts_with("gfx_report_event") || n.starts_with("gfx_news_event"),
            Cat::Portrait => t.contains("/leaders/") || t.contains("portrait") || n.contains("portrait"),
            Cat::Decision => n.starts_with("gfx_decision") || t.contains("/decisions/"),
            Cat::All => true,
        }
    }
}

/// Whose pictures.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Whose {
    #[default]
    Any,
    Content,
    Base,
}

struct Sprite {
    name: String,
    texture: String,
    /// The package it is in (None: the base game).
    pkg: Option<String>,
}

#[derive(Default)]
struct Folder {
    /// Sprites below it (with the filters).
    count: usize,
    /// Sprites right in it.
    own: usize,
    children: BTreeMap<String, Folder>,
}

type ListKey = (u64, bool, Whose, Cat, String, String);

#[derive(Default)]
pub struct PickerState {
    pub filter: String,
    pub cat: Option<Cat>,
    pub whose: Whose,
    /// The folder picked in the sidebar ("gfx/interface/goals").
    pub folder: Option<String>,
    pub size: f32,
    /// Scroll to the field's sprite once it is listed (set when the picker opens).
    pub find_current: bool,
    all: Option<((u64, bool), Sprites)>,
    /// The sprites that pass the filters (indexes into `all`) and their folders.
    list: Option<(ListKey, Listed)>,
}

type Sprites = Arc<Vec<Sprite>>;
/// Indexes into the sprites that pass the filters, and their folders.
type Listed = Arc<(Vec<usize>, Folder)>;

fn folder_of(texture: &str) -> &str {
    texture.rsplit_once('/').map(|x| x.0).unwrap_or("")
}

/// Every sprite of the content and the base game.
fn all_sprites(cx: &mut Cx) -> Vec<Sprite> {
    let own = cx.gfx.own(cx.st).clone();
    let mut out: Vec<Sprite> = own.iter().map(|(n, (pkg, tex, _))| Sprite { name: n.clone(), texture: tex.replace('\\', "/"), pkg: Some(pkg.clone()) }).collect();
    if let Some(b) = cx.base {
        out.extend(b.sprites.iter().filter(|(n, _)| !own.contains_key(*n)).map(|(n, (tex, _))| Sprite { name: n.clone(), texture: tex.replace('\\', "/"), pkg: None }));
    }
    out.sort_by_key(|a| a.name.to_ascii_lowercase());
    out
}

fn add_to_tree(root: &mut Folder, dir: &str) {
    root.count += 1;
    let mut f = root;
    if dir.is_empty() {
        f.own += 1;
        return;
    }
    for part in dir.split('/').filter(|p| !p.is_empty()) {
        f = f.children.entry(part.to_string()).or_default();
        f.count += 1;
    }
    f.own += 1;
}

/// The folder tree in the sidebar: a folder with one sub-folder and no
/// pictures of its own shows as one (`gfx/interface`).
fn tree_ui(ui: &mut egui::Ui, name: &str, path: &str, f: &Folder, sel: &mut Option<String>, depth: usize) {
    let (mut name, mut path, mut f) = (name.to_string(), path.to_string(), f);
    while f.own == 0 && f.children.len() == 1 {
        let (n, c) = f.children.iter().next().unwrap();
        name = if name.is_empty() { n.clone() } else { format!("{name}/{n}") };
        path = if path.is_empty() { n.clone() } else { format!("{path}/{n}") };
        f = c;
    }
    let label = RichText::new(format!("{name}  {}", f.count));
    let on = sel.as_deref() == Some(path.as_str());
    if f.children.is_empty() {
        if ui.selectable_label(on, label).on_hover_text(&path).clicked() {
            *sel = if on { None } else { Some(path) };
        }
        return;
    }
    let id = ui.make_persistent_id(("picker_folder", &path));
    egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, depth == 0 || on)
        .show_header(ui, |ui| {
            if ui.selectable_label(on, label).on_hover_text(&path).clicked() {
                *sel = if on { None } else { Some(path.clone()) };
            }
        })
        .body(|ui| {
            for (n, c) in &f.children {
                let p = format!("{path}/{n}");
                tree_ui(ui, n, &p, c, sel, depth + 1);
            }
        });
}

/// The picker window. `prefix`: the field's (`GFX_idea_`); `cat`: the kind of
/// picture the field takes; `current`: the sprite it has. Returns the one picked.
pub fn window(ctx: &egui::Context, cx: &mut Cx, ps: &mut PickerState, prefix: &str, cat: Cat, current: &str, open: &mut bool) -> Option<String> {
    let mut picked = None;
    let c = *ps.cat.get_or_insert(cat);
    if ps.size < 32.0 {
        ps.size = 64.0;
    }
    let gen_key = (cx.st.generation, cx.base.is_some());
    if ps.all.as_ref().is_none_or(|a| a.0 != gen_key) {
        ps.all = Some((gen_key, Arc::new(all_sprites(cx))));
        ps.list = None;
    }
    let all = ps.all.as_ref().unwrap().1.clone();
    let key: ListKey = (gen_key.0, gen_key.1, ps.whose, c, prefix.to_string(), ps.filter.to_lowercase());
    if ps.list.as_ref().is_none_or(|l| l.0 != key) {
        let f = ps.filter.to_lowercase();
        let words: Vec<&str> = f.split_whitespace().collect();
        let mut root = Folder::default();
        let idx: Vec<usize> = all
            .iter()
            .enumerate()
            .filter(|(_, s)| match ps.whose {
                Whose::Any => true,
                Whose::Content => s.pkg.is_some(),
                Whose::Base => s.pkg.is_none(),
            })
            .filter(|(_, s)| c.matches(&s.name, &s.texture, prefix))
            .filter(|(_, s)| {
                let hay = format!("{} {}", s.name.to_lowercase(), s.texture.to_lowercase());
                words.iter().all(|w| hay.contains(w))
            })
            .map(|(i, _)| i)
            .collect();
        for &i in &idx {
            add_to_tree(&mut root, folder_of(&all[i].texture));
        }
        ps.list = Some((key, Arc::new((idx, root))));
    }
    let list = ps.list.as_ref().unwrap().1.clone();
    let (idx, root) = (&list.0, &list.1);
    let shown: Vec<usize> = match &ps.folder {
        Some(fo) => idx.iter().copied().filter(|&i| {
            let d = folder_of(&all[i].texture);
            d == fo || d.starts_with(&format!("{fo}/"))
        })
        .collect(),
        None => idx.clone(),
    };
    egui::Window::new("Pick a picture").id(egui::Id::unique("sprite_picker_window")).open(open).default_size([980.0, 640.0]).show(ctx, |ui| {
        // filters
        ui.horizontal_wrapped(|ui| {
            ui.label("🔍");
            ui.add(egui::TextEdit::singleline(&mut ps.filter).desired_width(200.0).hint_text("words of the name or the file"));
            ui.separator();
            let mut cats = vec![Cat::Focus, Cat::Idea, Cat::Event, Cat::Portrait, Cat::Decision, Cat::All];
            if !prefix.is_empty() && prefix != "GFX_" {
                cats.insert(0, Cat::Prefix);
            }
            let mut cur = c;
            for k in cats {
                ui.selectable_value(&mut cur, k, k.label(prefix));
            }
            if cur != c {
                ps.cat = Some(cur);
                ps.folder = None;
            }
            ui.separator();
            let mut w = ps.whose;
            ui.selectable_value(&mut w, Whose::Any, "everyone's");
            ui.selectable_value(&mut w, Whose::Content, "the content's");
            ui.selectable_value(&mut w, Whose::Base, "the base game's");
            if w != ps.whose {
                ps.whose = w;
                ps.folder = None;
            }
            ui.separator();
            ui.add(egui::Slider::new(&mut ps.size, 40.0..=140.0).show_value(false).text("size"));
        });
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{} of {} pictures", shown.len(), all.len())).weak());
            if let Some(f) = ps.folder.clone() {
                ui.label(RichText::new(format!("in {f}")).weak());
                if ui.small_button("✖ all folders").clicked() {
                    ps.folder = None;
                }
            }
            if (!ps.filter.is_empty() || ps.whose != Whose::Any || ps.cat != Some(cat)) && ui.small_button("reset filters").clicked() {
                ps.filter.clear();
                ps.whose = Whose::Any;
                ps.cat = Some(cat);
                ps.folder = None;
            }
            if cx.base.is_none() {
                ui.label(RichText::new("(the base game is still being read)").weak().small());
            }
        });
        ui.separator();
        // the folders
        let w = ui.available_width();
        egui::Panel::left("picker_folders").resizable(true).default_size(240.0_f32.min(w * 0.3)).min_size(140.0).max_size(w * 0.5).show(ui, |ui| {
            super::contained(ui, |ui| {
                egui::ScrollArea::vertical().id_salt("picker_folders_scroll").auto_shrink([false, false]).show(ui, |ui| {
                    if ui.selectable_label(ps.folder.is_none(), RichText::new(format!("all folders  {}", root.count)).strong()).clicked() {
                        ps.folder = None;
                    }
                    for (n, f) in &root.children {
                        tree_ui(ui, n, n, f, &mut ps.folder, 0);
                    }
                    if root.own > 0 {
                        ui.label(RichText::new(format!("{} with a picture in no folder", root.own)).weak().small());
                    }
                });
            });
        });
        // the pictures
        let cell = ps.size + 26.0;
        let per_row = ((ui.available_width() / (cell + 6.0)) as usize).max(1);
        let rows = shown.len().div_ceil(per_row);
        let mut grid = egui::ScrollArea::vertical().id_salt("picker_grid").auto_shrink([false, false]);
        if ps.find_current {
            match shown.iter().position(|&i| all[i].name == current) {
                Some(n) => {
                    let row_h = cell + 22.0 + ui.spacing().item_spacing.y;
                    grid = grid.vertical_scroll_offset(((n / per_row) as f32 * row_h - row_h).max(0.0));
                    ps.find_current = false;
                }
                // the base game's may still be on its way
                None if cx.base.is_some() || current.is_empty() => ps.find_current = false,
                None => {}
            }
        }
        grid.show_rows(ui, cell + 22.0, rows, |ui, range| {
            for r in range {
                ui.horizontal(|ui| {
                    for &i in shown.iter().skip(r * per_row).take(per_row) {
                        let s = &all[i];
                        ui.vertical(|ui| {
                            ui.set_width(cell);
                            let tex = cx.texture(ui.ctx(), &s.name);
                            let resp = crate::content::gfx::show(ui, tex, egui::vec2(ps.size, ps.size)).interact(egui::Sense::click());
                            if s.name == current {
                                ui.painter().rect_stroke(resp.rect.expand(2.0), 3.0, egui::Stroke::new(2.0, super::OK), egui::StrokeKind::Outside);
                            }
                            let short = if !prefix.is_empty() { s.name.strip_prefix(prefix).unwrap_or(&s.name) } else { s.name.strip_prefix("GFX_").unwrap_or(&s.name) };
                            let lr = ui.add(egui::Label::new(RichText::new(short).small()).truncate().sense(egui::Sense::click()));
                            if resp.clicked() || lr.clicked() {
                                picked = Some(s.name.clone());
                            }
                            let whose = s.pkg.as_deref().map(|p| format!("the content ({p})")).unwrap_or_else(|| "the base game".into());
                            resp.on_hover_text(format!("{}\n{}\nfrom {whose}", s.name, s.texture));
                        });
                    }
                });
            }
        });
    });
    picked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categories() {
        assert!(Cat::Focus.matches("GFX_goal_generic_x", "gfx/interface/goals/x.dds", ""));
        assert!(!Cat::Focus.matches("GFX_goal_generic_x_shine", "gfx/interface/goals/x.dds", ""));
        assert!(Cat::All.matches("GFX_goal_generic_x_shine", "", ""));
        assert!(Cat::Prefix.matches("GFX_idea_x", "", "GFX_idea_") && !Cat::Prefix.matches("GFX_goal_x", "", "GFX_idea_"));
        assert!(Cat::Portrait.matches("GFX_portrait_x", "gfx/leaders/GER/x.dds", ""));
        assert_eq!(Cat::of_field(Kind::Focus, "icon", ""), Cat::Focus);
        assert_eq!(Cat::of_field(Kind::Idea, "picture", "GFX_idea_"), Cat::Prefix);
        assert_eq!(Cat::of_field(Kind::Event, "picture", ""), Cat::Event);
    }

    #[test]
    fn folder_counts() {
        let mut root = Folder::default();
        for d in ["gfx/interface/goals", "gfx/interface/goals", "gfx/interface/ideas", "gfx/leaders/GER", ""] {
            add_to_tree(&mut root, d);
        }
        assert_eq!((root.count, root.own), (5, 1));
        let gfx = &root.children["gfx"];
        assert_eq!((gfx.count, gfx.own), (4, 0));
        assert_eq!(gfx.children["interface"].count, 3);
        assert_eq!(gfx.children["interface"].children["goals"].own, 2);
    }
}
