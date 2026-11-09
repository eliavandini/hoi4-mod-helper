//! The editor of one definition: its fields as a form (from the kind's
//! [`kinds::Spec`]), its code, its localisation in any language, what uses
//! it, and a preview like the game's (events, decisions, ideas, focuses).
//! Every change goes into the file at once as an undo step; what is being
//! typed and does not parse yet waits in the field.

use std::collections::BTreeSet;

use eframe::egui::{self, RichText};

use super::{Cx, ERR, Nav, OK, WARN, code, picker, statemap, wizard};
use crate::content::about;
use crate::content::import;
use crate::content::kinds::{self, Field, FieldType, Kind};
use crate::content::loc;
use crate::content::refactor;
use crate::content::script::{self, Block, Edit, Script};
use crate::content::store::DefKey;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Form,
    Code,
    Loc,
    Uses,
}

#[derive(Default)]
pub struct FormState {
    pub key: Option<DefKey>,
    pub tab: Tab,
    /// The code tab's text while it does not parse (or has not been applied).
    code: Option<(DefKey, String)>,
    code_err: Option<String>,
    rename: Option<(String, Option<Result<refactor::Plan, String>>)>,
    delete: Option<refactor::Plan>,
    copy_to: String,
    refs: Option<(u64, DefKey, Vec<(String, String, usize, String)>)>,
    /// A sprite picker open for a field.
    pub picker: Option<PickTarget>,
    picker_for: Option<PickTarget>,
    pub pick: picker::PickerState,
    pub wizard: wizard::WizardState,
    /// For screenshots: `about:<field>` opens what a field does, `map:<field>`
    /// the map of a state field.
    pub start: Option<String>,
    /// Script from the wizard for the code tab (at its cursor).
    code_insert: Option<String>,
    jump: Option<usize>,
}

/// The field a picture is picked for.
#[derive(Clone, Debug, PartialEq)]
pub struct PickTarget {
    pub key: DefKey,
    pub field: String,
    pub prefix: String,
}

impl FormState {
    pub fn select(&mut self, k: Option<DefKey>) {
        if self.key != k {
            self.key = k;
            self.code = None;
            self.code_err = None;
            self.rename = None;
            self.delete = None;
            self.refs = None;
            self.picker = None;
        }
    }
}

/// The sprite a list shows next to a definition.
pub fn list_sprite(cx: &Cx, key: &DefKey) -> Option<String> {
    if !matches!(key.kind, Kind::Idea | Kind::Decision | Kind::DecisionCategory | Kind::Character | Kind::Sprite | Kind::Focus | Kind::SharedFocus) {
        return None;
    }
    if key.kind == Kind::Sprite {
        return Some(key.id.clone());
    }
    let (d, _, s) = cx.st.def(key)?;
    let b = d.block(s)?;
    if key.kind == Kind::Idea && b.get("picture").is_none() {
        return Some(format!("GFX_idea_{}", key.id));
    }
    kinds::sprites_of(key.kind, b).into_iter().next().map(|x| x.1)
}

// ---------------------------------------------------------------- small widgets

/// A one-line field that keeps what is typed while it has focus; returns the
/// text when it changed and `ok` accepts it.
pub fn line_field(ui: &mut egui::Ui, id: egui::Id, current: &str, width: f32, hint: &str, ok: &dyn Fn(&str) -> bool) -> Option<String> {
    let mut s: String = ui.data(|d| d.get_temp::<String>(id)).unwrap_or_else(|| current.to_string());
    let r = ui.add(egui::TextEdit::singleline(&mut s).id(id).desired_width(width).hint_text(hint));
    let good = ok(&s);
    if r.has_focus() {
        ui.data_mut(|d| d.insert_temp(id, s.clone()));
        if !good {
            r.clone().on_hover_text(RichText::new("not applied: not a valid value").color(ERR));
            ui.painter().rect_stroke(r.rect, 2.0, egui::Stroke::new(1.0, ERR), egui::StrokeKind::Outside);
        }
    } else {
        ui.data_mut(|d| d.remove::<String>(id));
    }
    (r.changed() && good && s != current).then_some(s)
}

/// A block's contents in a code editor; returns the new contents when they
/// changed and parse.
pub fn block_field(ui: &mut egui::Ui, id: egui::Id, current: &str, rows: usize) -> Option<String> {
    let mut s: String = ui.data(|d| d.get_temp::<String>(id)).unwrap_or_else(|| current.to_string());
    let parsed = Script::parse(&s);
    let errs: Vec<usize> = parsed.errors.iter().map(|e| script::line_of(&s, e.at)).collect();
    let mut jump = None;
    let lines = s.lines().count().clamp(1, rows);
    let out = code::editor(ui, id, &mut s, code::Lang::Script, &errs, lines.max(2), &mut jump);
    let reparsed = Script::parse(&s);
    if out.response.has_focus() || !reparsed.errors.is_empty() && s != current {
        ui.data_mut(|d| d.insert_temp(id, s.clone()));
    } else {
        ui.data_mut(|d| d.remove::<String>(id));
    }
    if let Some(e) = reparsed.errors.first() {
        ui.label(RichText::new(format!("not applied — line {}: {}", script::line_of(&s, e.at), e.message)).color(ERR).small());
    }
    (out.changed && reparsed.errors.is_empty() && s != current).then_some(s)
}

fn is_word(s: &str) -> bool {
    !s.trim().is_empty() && !s.contains(['{', '}', '=', '#']) && !s.trim().contains(char::is_whitespace)
}
fn is_number(s: &str) -> bool {
    s.trim().parse::<f64>().is_ok()
}

/// The value as written in script for a field type.
fn render(ty: FieldType, v: &str) -> String {
    let v = v.trim();
    match ty {
        FieldType::Text => script::quote(v),
        _ if script::needs_quotes(v) => script::quote(v),
        _ => v.to_string(),
    }
}

/// `n`th item with `key` in a block.
fn nth<'a>(b: &'a Block, key: &'a str, n: usize) -> Option<&'a script::Item> {
    b.all(key).nth(n)
}

/// Index of the last item with `key` (for adding another after it).
fn last_index(b: &Block, key: &str) -> Option<usize> {
    b.items.iter().rposition(|i| i.is(key))
}

// ---------------------------------------------------------------- fields

/// Edit the fields of a definition. `skip`: fields shown elsewhere.
pub fn fields(ui: &mut egui::Ui, cx: &mut Cx, key: &DefKey, list: &[Field], skip: &[&str], st_state: &mut FormState) {
    let Some((d, text, s)) = cx.st.def(key) else { return };
    let Some(b) = d.block(s) else { return };
    // what is needed from the file, before it changes
    struct Row {
        f: Field,
        values: Vec<Value>,
    }
    enum Value {
        Atom(String),
        Block(String),
        Other(String),
    }
    let rows: Vec<Row> = list
        .iter()
        .filter(|f| !skip.contains(&f.key))
        .map(|f| Row {
            f: *f,
            values: b
                .all(f.key)
                .map(|it| match (it.atom(), it.block()) {
                    (Some(a), _) => Value::Atom(a.text.clone()),
                    (_, Some(bl)) => Value::Block(script::inner_text(text, bl)),
                    _ => Value::Other(script::item_text(text, it).to_string()),
                })
                .collect(),
        })
        .collect();
    let known: BTreeSet<&str> = list.iter().map(|f| f.key).chain(skip.iter().copied()).chain(["id"]).collect();
    let others: Vec<String> = b.items.iter().filter(|i| i.key.as_ref().is_none_or(|k| !known.contains(k.text.as_str()))).map(|i| script::item_source(text, i)).collect();
    let _ = s;
    let kid = egui::Id::unique(("field", key));
    let mut pending: Vec<(String, Box<dyn FnOnce(&str, &Block) -> Vec<Edit>>)> = Vec::new();
    // rows of a label of a fixed width and the values (no grid: full-width
    // editors in a grid make it a little wider every frame)
    let label_w = (ui.available_width() * 0.28).clamp(80.0, 180.0);
    for row in rows {
        let f = row.f;
        // what the field does: on the label, and under the row behind its ?
        let about_id = kid.with(("about", f.key));
        let mut about_open = ui.data(|d| d.get_temp::<bool>(about_id)).unwrap_or(false);
        if st_state.start.as_deref() == Some(format!("about:{}", f.key).as_str()) {
            st_state.start = None;
            about_open = true;
            ui.data_mut(|d| d.insert_temp(about_id, true));
        }
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(egui::vec2(label_w, 20.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.set_min_width(label_w);
                let label = ui.scope(|ui| {
                    ui.set_max_width(label_w - 26.0);
                    ui.add(egui::Label::new(RichText::new(f.key).monospace()).truncate().sense(egui::Sense::click()))
                });
                let q = ui.add(egui::Button::new(RichText::new("?").small()).small().selected(about_open));
                let cxr: &Cx = cx;
                let label = label.inner.on_hover_ui(|ui| about_card(ui, cxr, key.kind, &f, false));
                if label.clicked() || q.on_hover_text("what it does, what it takes, how it is written").clicked() {
                    about_open = !about_open;
                    ui.data_mut(|d| d.insert_temp(about_id, about_open));
                }
            });
            ui.vertical(|ui| {
                ui.set_max_width(ui.available_width());
                let n = row.values.len();
                for (i, v) in row.values.into_iter().enumerate() {
                    let fid = kid.with((f.key, i));
                    ui.horizontal(|ui| {
                        match (f.ty, v) {
                            (FieldType::Bool, Value::Atom(a)) => {
                                let mut yes = a == "yes";
                                let label = if yes { "yes" } else { "no" };
                                if ui.checkbox(&mut yes, label).changed() {
                                    let key = f.key.to_string();
                                    pending.push((format!("set {key}"), Box::new(move |_t: &str, b: &Block| nth(b, &key, i).and_then(|it| it.atom()).map(|a| vec![Edit { range: a.span.clone(), text: if yes { "yes" } else { "no" }.into() }]).unwrap_or_default())));
                                }
                            }
                            (FieldType::Block, Value::Block(inner)) => {
                                let role = crate::content::wizard::role_of(key.kind, f.key);
                                ui.vertical(|ui| {
                                    ui.set_max_width(ui.available_width() - if role.is_some() { 84.0 } else { 56.0 });
                                    if let Some(new) = block_field(ui, fid, &inner, 14) {
                                        let key = f.key.to_string();
                                        pending.push((format!("edit {key}"), Box::new(move |t: &str, b: &Block| nth(b, &key, i).and_then(|it| it.block()).map(|bl| vec![script::set_block_inner(t, bl, &new)]).unwrap_or_default())));
                                    }
                                });
                                if let Some(role) = role
                                    && ui.small_button("🪄").on_hover_text(format!("{} wizard: build it from menus", role.label())).clicked()
                                {
                                    let cursor = egui::TextEdit::load_state(ui.ctx(), fid).and_then(|s| s.cursor.char_range()).map(|r| r.primary.index.0);
                                    st_state.wizard.open(wizard::Target { key: key.clone(), field: f.key.to_string(), nth: Some(i), role, cursor });
                                }
                            }
                            (ty, Value::Atom(a)) => {
                                let width = if matches!(ty, FieldType::Number | FieldType::State) { 80.0 } else { (ui.available_width() - 150.0).clamp(80.0, 260.0) };
                                let ok: &dyn Fn(&str) -> bool = match ty {
                                    FieldType::Number | FieldType::State => &is_number,
                                    FieldType::Text | FieldType::File(_) => &|_: &str| true,
                                    _ => &is_word,
                                };
                                let mut new = line_field(ui, fid, &a, width, hint(ty), ok);
                                // picked from a list, or on the map
                                if let Some(p) = crate::content::wizard::param_of(ty) {
                                    let items = wizard::list(cx, &mut st_state.wizard.lists, p);
                                    if let Some(v) = wizard::pick_menu(ui, fid.with("pick"), &a, &|| items.clone()) {
                                        new = Some(v);
                                    }
                                }
                                let open_map = st_state.start.as_deref() == Some(format!("map:{}", f.key).as_str());
                                if open_map {
                                    st_state.start = None;
                                }
                                if ty == FieldType::State
                                    && let Some(s) = statemap::button(ui, cx, fid.with("map"), &a, open_map)
                                {
                                    new = Some(s.to_string());
                                }
                                if let Some(new) = new.filter(|n| *n != a) {
                                    ui.data_mut(|d| d.remove::<String>(fid));
                                    let key = f.key.to_string();
                                    let v = render(ty, &new);
                                    pending.push((format!("set {key}"), Box::new(move |_t: &str, b: &Block| nth(b, &key, i).and_then(|it| it.atom()).map(|at| vec![Edit { range: at.span.clone(), text: if at.quoted && !v.starts_with('"') { script::quote(&v) } else { v.clone() } }]).unwrap_or_default())));
                                }
                                extra(ui, cx, key, f, &a, st_state);
                                // a block written as one word (`peace_action_type = puppet`)
                                if ty == FieldType::Block
                                    && let Some(role) = crate::content::wizard::role_of(key.kind, f.key)
                                    && ui.small_button("🪄").on_hover_text(format!("{} wizard: build it from menus", role.label())).clicked()
                                {
                                    st_state.wizard.open(wizard::Target { key: key.clone(), field: f.key.to_string(), nth: Some(i), role, cursor: None });
                                }
                            }
                            (_, Value::Block(inner)) | (_, Value::Other(inner)) => {
                                // not the shape the field usually has: as code
                                ui.vertical(|ui| {
                                    ui.set_max_width(ui.available_width() - 56.0);
                                    if let Some(new) = block_field(ui, fid, &inner, 10) {
                                        let key = f.key.to_string();
                                        pending.push((format!("edit {key}"), Box::new(move |t: &str, b: &Block| nth(b, &key, i).and_then(|it| it.block()).map(|bl| vec![script::set_block_inner(t, bl, &new)]).unwrap_or_default())));
                                    }
                                });
                            }
                        }
                        if ui.small_button("✖").on_hover_text(format!("remove this {}", f.key)).clicked() {
                            let key = f.key.to_string();
                            pending.push((format!("remove {key}"), Box::new(move |t: &str, b: &Block| nth(b, &key, i).map(|it| vec![script::remove_item(t, it)]).unwrap_or_default())));
                        }
                    });
                }
                if n == 0 || f.repeat {
                    ui.horizontal(|ui| {
                        let label = if n == 0 { "+ add".to_string() } else { format!("+ another {}", f.key) };
                        if ui.small_button(label).on_hover_text(f.help).clicked() {
                            let key = f.key.to_string();
                            let init = default_value(f, cx);
                            pending.push((format!("add {key}"), Box::new(move |t: &str, b: &Block| vec![script::insert_item(t, b, &format!("{key} = {init}"), last_index(b, &key))])));
                        }
                        if f.ty == FieldType::Block
                            && let Some(role) = crate::content::wizard::role_of(key.kind, f.key)
                            && ui.small_button("🪄").on_hover_text(format!("add it with the {} wizard", role.label())).clicked()
                        {
                            st_state.wizard.open(wizard::Target { key: key.clone(), field: f.key.to_string(), nth: None, role, cursor: None });
                        }
                    });
                }
            });
        });
        if about_open {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                about_card(ui, cx, key.kind, &f, true);
            });
        }
        ui.add_space(3.0);
    }
    if !others.is_empty() {
        egui::CollapsingHeader::new(format!("Other fields ({})", others.len())).id_salt(kid.with("others")).show(ui, |ui| {
            ui.label(RichText::new("Fields the form does not know; edit them on the Code tab.").weak().small());
            for o in others.iter().take(60) {
                code::view(ui, kid.with(("other", o.as_str())), o, code::Lang::Script, o.lines().count().min(8));
            }
        });
    }
    for (label, f) in pending {
        let merge = Some(format!("{key:?}:{label}"));
        cx.edit_def(key, &label, merge, |t, b, _| f(t, b));
    }
}

/// What a new field starts as.
fn default_value(f: Field, cx: &Cx) -> String {
    let tag = cx.tag();
    match f.ty {
        FieldType::Bool => "yes".into(),
        FieldType::Number => "0".into(),
        FieldType::Block => match f.key {
            "ai_will_do" | "ai_chance" => "{ factor = 1 }".into(),
            "allowed" => format!("{{ original_tag = {tag} }}"),
            "mean_time_to_happen" => "{ days = 30 }".into(),
            "option" => "{\n\tname = new_option\n}".into(),
            _ => "{\n}".into(),
        },
        FieldType::Text => "\"\"".into(),
        FieldType::Sprite(_) => "GFX_goal_unknown".into(),
        FieldType::Loc => format!("{tag}_{}", f.key),
        FieldType::Tag => tag.to_string(),
        FieldType::State => cx.nation.map(|n| n.capital_state).filter(|s| *s != 0).or_else(|| cx.facts.states.first().copied()).unwrap_or(1).to_string(),
        FieldType::File(_) => format!("\"{tag}_1936\""),
        _ => "x".into(),
    }
}

/// The grey text of an empty field: what goes in.
fn hint(ty: FieldType) -> &'static str {
    match ty {
        FieldType::Loc => "localisation key",
        FieldType::Tag => "country tag",
        FieldType::State => "state",
        FieldType::File(_) => "file name",
        FieldType::Ref(_) | FieldType::IdIn(..) => "id",
        FieldType::Number => "number",
        _ => "",
    }
}

/// What a field does in the game, what it takes and how it is written; with
/// the game's own documentation of it when the field is an effect. `full`:
/// under the row (the tooltip has less).
fn about_card(ui: &mut egui::Ui, cx: &Cx, kind: Kind, f: &Field, full: bool) {
    if !full {
        ui.set_max_width(460.0);
        // the name may be cut short on the form
        ui.label(RichText::new(f.key).monospace().strong());
    }
    let about = about::about(kind, f.key);
    ui.label(about.map_or(f.help, |a| a.text));
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Takes").weak());
        ui.label(about::value_of(f.ty));
        if f.repeat {
            ui.label(RichText::new("· can be there more than once").weak());
        }
    });
    let example = about::example(kind, f, cx.tag());
    ui.label(RichText::new("Written in the file:").weak());
    egui::Frame::new().fill(ui.visuals().extreme_bg_color).inner_margin(egui::Margin::symmetric(8, 4)).corner_radius(3.0).show(ui, |ui| {
        ui.label(RichText::new(example.replace('\t', "    ")).monospace());
    });
    // country history is run as effects when the game starts
    if full
        && kind == Kind::CountryHistory
        && let Some(e) = cx.docs.effect(f.key)
    {
        ui.label(RichText::new(format!("The game's documentation of {}:", e.name)).weak());
        ui.label(RichText::new(crate::content::docs::plain(&e.text).replace('\t', "    ")).monospace().small());
    }
    if !full {
        ui.label(RichText::new("click the name or ? to keep this open").weak().small());
    }
}

/// What comes after a field's value: a sprite's picture and picker, a link
/// to what it names, the text of a localisation key.
fn extra(ui: &mut egui::Ui, cx: &mut Cx, key: &DefKey, f: Field, value: &str, st: &mut FormState) {
    match f.ty {
        FieldType::Sprite(prefix) => {
            let name = kinds::sprite_name(prefix, value);
            let tex = cx.texture(ui.ctx(), &name);
            let r = crate::content::gfx::show(ui, tex, egui::vec2(36.0, 36.0));
            let srcs = cx.sources();
            let problem = cx.gfx.problem(ui.ctx(), &name, cx.st, &srcs);
            if let Some(p) = problem {
                r.on_hover_text(RichText::new(p).color(WARN));
            } else {
                r.on_hover_text(&name);
            }
            if ui.small_button("pick…").clicked() {
                st.picker = Some(PickTarget { key: key.clone(), field: f.key.to_string(), prefix: prefix.to_string() });
                st.pick.find_current = true;
            }
        }
        FieldType::Ref(k) => {
            if let Some(dk) = cx.st.find(k, value) {
                if ui.small_button("→").on_hover_text(format!("open {value}")).clicked() {
                    cx.go(Nav::Def(dk));
                }
            } else if cx.base.is_some_and(|b| b.has(k, value)) {
                ui.label(RichText::new("base game").weak().small());
            } else if !value.is_empty() {
                ui.label(RichText::new(format!("no {} {value}", k.spec().one)).color(WARN).small());
            }
        }
        FieldType::Loc => {
            if let Some(t) = cx.text(value) {
                ui.label(RichText::new(loc::plain(t)).weak().small());
            } else if !value.is_empty() {
                ui.label(RichText::new("no text yet").color(WARN).small()).on_hover_text("give it a text on the Localisation page");
            }
        }
        FieldType::Tag => {
            if let Some(n) = cx.facts.nations.iter().find(|n| n.tag == value) {
                ui.label(RichText::new(&n.name).weak().small());
            } else if cx.base.is_some_and(|b| b.tags.contains(value)) {
                ui.label(RichText::new(cx.text(value).map(loc::plain).unwrap_or_else(|| "base game".into())).weak().small());
            } else if !value.is_empty() && !crate::content::wizard::TARGETS.contains(&value) {
                ui.label(RichText::new(format!("no country {value}")).color(WARN).small());
            }
        }
        FieldType::State => match value.parse::<u32>().ok().and_then(|s| cx.facts.state_names.get(&s)) {
            Some(n) => {
                ui.label(RichText::new(n).weak().small());
            }
            None => {
                ui.label(RichText::new("not a state of the map").color(WARN).small());
            }
        },
        FieldType::File(folder) => {
            let rel = format!("{folder}/{}.txt", value.trim_matches('"'));
            let ours = cx.st.packages.values().any(|p| p.files.contains_key(&rel));
            let built = folder == "history/units" && cx.facts.nations.iter().any(|n| format!("{}_1936", n.tag) == value.trim_matches('"'));
            if built && !ours {
                ui.label(RichText::new("made by the build").weak().small()).on_hover_text("the starting divisions the build places from the map");
            } else if !ours {
                ui.label(RichText::new(format!("no {rel}")).color(WARN).small());
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------- localisation

/// Edit the localisation keys of a definition in the screen's language.
pub fn loc_section(ui: &mut egui::Ui, cx: &mut Cx, key: &DefKey) {
    let Some((d, _, s)) = cx.st.def(key) else { return };
    let keys = kinds::loc_keys(key.kind, &key.id, d.block(s));
    let lang = cx.settings.language.clone();
    ui.horizontal(|ui| {
        ui.label("language");
        egui::ComboBox::from_id_salt("loc_lang").selected_text(&lang).show_ui(ui, |ui| {
            for l in loc::LANGUAGES {
                ui.selectable_value(&mut cx.settings.language, l.to_string(), l);
            }
        });
    });
    if keys.is_empty() {
        ui.label(RichText::new("This kind has no texts of its own.").weak());
        return;
    }
    let mut sets: Vec<(String, String)> = Vec::new();
    egui::Grid::new(("loc", key)).num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
        for (k, what) in &keys {
            ui.vertical(|ui| {
                ui.label(what);
                ui.label(RichText::new(k).monospace().small().weak());
            });
            let own = cx.loc.own(k).map(|x| x.2.clone());
            let base = cx.base.and_then(|b| b.loc.get(k)).cloned();
            let cur = own.clone().unwrap_or_default();
            ui.vertical(|ui| {
                let id = egui::Id::unique(("loc_text", key, k, &lang));
                let mut s: String = ui.data(|d| d.get_temp::<String>(id)).unwrap_or_else(|| cur.clone());
                let hint = base.clone().map(|b| format!("base game: {b}")).unwrap_or_else(|| "no text: the key is shown in the game".into());
                let r = ui.add(egui::TextEdit::multiline(&mut s).id(id).desired_rows(if what.contains("escri") || what.contains("Text") { 3 } else { 1 }).desired_width(f32::INFINITY).hint_text(hint));
                if r.has_focus() {
                    ui.data_mut(|d| d.insert_temp(id, s.clone()));
                } else {
                    ui.data_mut(|d| d.remove::<String>(id));
                }
                if r.changed() && s != cur {
                    sets.push((k.clone(), s.replace('\n', "\\n")));
                }
                if own.is_none() {
                    ui.horizontal(|ui| {
                        if let Some(b) = &base {
                            ui.label(RichText::new("from the base game").weak().small());
                            if ui.small_button("copy here").clicked() {
                                sets.push((k.clone(), b.clone()));
                            }
                        } else {
                            ui.label(RichText::new("missing").color(WARN).small());
                            if ui.small_button("fill in from the id").clicked() {
                                sets.push((k.clone(), loc::prettify(&key.id)));
                            }
                        }
                    });
                }
            });
            ui.end_row();
        }
    });
    for (k, v) in sets {
        set_text(cx, &key.pkg, &lang, &k, &v);
    }
}

/// Set a localisation text: where the key is already (in this language),
/// else in the package's file for the language.
pub fn set_text(cx: &mut Cx, pkg: &str, lang: &str, k: &str, v: &str) {
    cx.st.begin_merge(format!("text of {k}"), Some(format!("loc:{lang}:{k}")));
    match cx.loc.own(k).cloned() {
        Some((p, file, _)) => {
            let old = cx.st.text(&p, &file).unwrap_or("").to_string();
            cx.st.set_text(&p, &file, loc::set(&old, lang, k, v));
        }
        None => import::set_loc(cx.st, pkg, lang, k, v),
    }
    cx.st.commit();
}

// ---------------------------------------------------------------- previews

/// Something like what the game shows for it.
fn preview(ui: &mut egui::Ui, cx: &mut Cx, key: &DefKey) {
    let Some((d, _, s)) = cx.st.def(key) else { return };
    let Some(b) = d.block(s) else { return };
    let txt = |cx: &Cx, k: &str| cx.shown_text(k).unwrap_or_else(|| k.to_string());
    match key.kind {
        Kind::Event => {
            let title = b.atom("title").or_else(|| b.block("title").and_then(|t| t.atom("text"))).map(|k| txt(cx, k)).unwrap_or_default();
            let desc = b.atom("desc").or_else(|| b.block("desc").and_then(|t| t.atom("text"))).map(|k| txt(cx, k)).unwrap_or_default();
            let pic = b.atom("picture").map(String::from);
            let options: Vec<String> = b.blocks("option").map(|o| o.atom("name").map(|k| txt(cx, k)).unwrap_or_else(|| "(an option with no name)".into())).collect();
            let hidden = b.yes("hidden");
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_max_width(440.0);
                if hidden {
                    ui.label(RichText::new("hidden event: the player does not see it").weak());
                }
                ui.label(RichText::new(title).heading());
                if let Some(p) = pic {
                    let tex = cx.texture(ui.ctx(), &p);
                    crate::content::gfx::show(ui, tex, egui::vec2(210.0, 176.0));
                }
                ui.label(desc);
                for o in options {
                    let _ = ui.add_enabled(false, egui::Button::new(o).min_size(egui::vec2(400.0, 0.0)));
                }
            });
        }
        Kind::Idea | Kind::Decision | Kind::DecisionCategory | Kind::Focus | Kind::SharedFocus | Kind::Character => {
            let sprite = list_sprite(cx, key);
            let name = cx.name_of(key.kind, &key.id);
            let desc_key = kinds::loc_keys(key.kind, &key.id, Some(b)).into_iter().nth(1).map(|x| x.0);
            let desc = desc_key.and_then(|k| cx.shown_text(&k));
            let cost = b.num("cost");
            let modifiers: Vec<String> = b
                .block("modifier")
                .map(|m| m.items.iter().take(12).filter_map(|it| Some(format!("{}: {}", loc::prettify(&it.key.as_ref()?.text), it.text()?))).collect())
                .unwrap_or_default();
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_max_width(440.0);
                ui.horizontal(|ui| {
                    if let Some(sp) = sprite {
                        let tex = cx.texture(ui.ctx(), &sp);
                        crate::content::gfx::show(ui, tex, egui::vec2(64.0, 64.0)).on_hover_text(&sp);
                    }
                    ui.vertical(|ui| {
                        ui.label(RichText::new(name).strong());
                        if let Some(c) = cost {
                            let unit = match key.kind {
                                Kind::Focus | Kind::SharedFocus => format!("{} days", (c * 7.0).round()),
                                _ => format!("{} political power", crate::content::focus::fmt_num(c)),
                            };
                            ui.label(RichText::new(unit).weak());
                        }
                        if let Some(dsc) = desc {
                            ui.label(dsc);
                        }
                    });
                });
                for m in &modifiers {
                    ui.label(RichText::new(m).small());
                }
            });
        }
        _ => {}
    }
}

// ---------------------------------------------------------------- the editor

pub fn def_editor(ui: &mut egui::Ui, cx: &mut Cx, key: &DefKey, st: &mut FormState) {
    let Some((d, text, _)) = cx.st.def(key) else {
        ui.label(RichText::new("It is gone (undone or renamed).").weak());
        return;
    };
    let line = script::line_of(text, d.span.start);
    let group = d.group.clone();
    let spec = key.kind.spec();
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(spec.one).weak());
        ui.label(RichText::new(&key.id).heading().monospace());
        if let Some(g) = &group {
            ui.label(RichText::new(format!("in {g}")).weak());
        }
        if ui.link(RichText::new(format!("{}:{line}", key.file)).small()).clicked() {
            cx.go(Nav::File { pkg: key.pkg.clone(), file: key.file.clone(), line: Some(line) });
        }
    });
    // actions
    ui.horizontal_wrapped(|ui| {
        if key.kind != Kind::CountryHistory && ui.button("✏ Rename…").on_hover_text("Rename it everywhere it is used, with its texts").clicked() {
            st.rename = Some((key.id.clone(), None));
        }
        if key.kind != Kind::CountryHistory && ui.button("⧉ Duplicate").clicked() {
            duplicate(cx, key);
        }
        if ui.button("🗑 Delete…").clicked() {
            st.delete = Some(refactor::plan_delete(cx.st, std::slice::from_ref(key)));
        }
        ui.separator();
        super::nation_combo(ui, "copy_to", cx.facts, &mut st.copy_to, true);
        if ui.add_enabled(!st.copy_to.is_empty() && st.copy_to != key.pkg, egui::Button::new("Copy to it")).on_hover_text("With what it uses; the tag in it becomes the other nation's").clicked() {
            let to = st.copy_to.clone();
            super::import_ui::copy_defs(cx, std::slice::from_ref(key), &to);
        }
    });
    rename_ui(ui, cx, key, st);
    delete_ui(ui, cx, st);
    ui.separator();
    ui.horizontal(|ui| {
        ui.selectable_value(&mut st.tab, Tab::Form, "Form");
        ui.selectable_value(&mut st.tab, Tab::Code, "Code");
        ui.selectable_value(&mut st.tab, Tab::Loc, "Texts");
        ui.selectable_value(&mut st.tab, Tab::Uses, "Used by");
    });
    ui.separator();
    egui::ScrollArea::vertical().id_salt(("def_scroll", st.tab as u8)).auto_shrink([false, false]).show(ui, |ui| match st.tab {
        Tab::Form => {
            preview(ui, cx, key);
            ui.add_space(6.0);
            fields(ui, cx, key, spec.fields, &[], st);
            ui.add_space(6.0);
            egui::CollapsingHeader::new("Texts").default_open(true).show(ui, |ui| loc_section(ui, cx, key));
        }
        Tab::Code => code_tab(ui, cx, key, st),
        Tab::Loc => loc_section(ui, cx, key),
        Tab::Uses => uses_tab(ui, cx, key, st),
    });
    popups(ui, cx, st);
}

/// The windows a form opens: the picture picker and the wizard. Called once a
/// frame by whatever shows a form.
pub fn popups(ui: &mut egui::Ui, cx: &mut Cx, st: &mut FormState) {
    if let Some(pt) = st.picker.clone() {
        let mut open = true;
        let cat = picker::Cat::of_field(pt.key.kind, &pt.field, &pt.prefix);
        let current = cx.st.def(&pt.key).and_then(|(d, _, s)| d.block(s).and_then(|b| b.atom(&pt.field)).map(|v| kinds::sprite_name(&pt.prefix, v))).unwrap_or_default();
        if st.pick.cat.is_some() && st.picker_for.as_ref() != Some(&pt) {
            // another field: its own kind of picture
            st.pick.cat = None;
            st.pick.folder = None;
        }
        st.picker_for = Some(pt.clone());
        let picked = picker::window(ui.ctx(), cx, &mut st.pick, &pt.prefix, cat, &current, &mut open);
        if let Some(p) = picked {
            // ideas and decisions name the picture without the prefix
            let prefix = &pt.prefix;
            let value = if !prefix.is_empty() && prefix != "GFX_" { p.strip_prefix(prefix.as_str()).map(String::from).unwrap_or(p.clone()) } else { p.clone() };
            let field = pt.field.clone();
            cx.edit_def(&pt.key, &format!("picture {p}"), None, |t, b, _| vec![script::set_atom(t, b, &field, &script::word(&value))]);
            open = false;
        }
        if !open {
            st.picker = None;
        }
    }
    if let Some((t, out)) = wizard::window(ui.ctx(), cx, &mut st.wizard) {
        apply_wizard(cx, st, t, out);
    }
}

/// What the wizard built, into its field (one undo step).
fn apply_wizard(cx: &mut Cx, st: &mut FormState, t: wizard::Target, out: wizard::Out) {
    use crate::content::wizard as wz;
    if t.field.is_empty() {
        // the code tab puts it at its cursor
        let (wizard::Out::Add(text) | wizard::Out::Replace(text)) = out;
        st.code_insert = Some(text);
        st.tab = Tab::Code;
        return;
    }
    let field = t.field.clone();
    let label = format!("{} wizard: {field}", t.role.label());
    let ok = cx.edit_def(&t.key, &label, None, |text, b, _| {
        let Some(n) = t.nth else {
            // a new field
            let body = match &out {
                wizard::Out::Add(x) | wizard::Out::Replace(x) => x.clone(),
            };
            return vec![script::insert_item(text, b, &wz::wrap(&field, &body), last_index(b, &field))];
        };
        let Some(it) = b.all(&field).nth(n) else { return Vec::new() };
        match (it.block(), &out) {
            (Some(bl), wizard::Out::Replace(x)) => vec![script::set_block_inner(text, bl, x)],
            (Some(bl), wizard::Out::Add(x)) => vec![script::set_block_inner(text, bl, &wz::merge(&script::inner_text(text, bl), x, t.cursor))],
            // `add_ideas = x`: becomes a block
            (None, o) => {
                let x = match o {
                    wizard::Out::Add(x) => wz::append(it.text().unwrap_or(""), x),
                    wizard::Out::Replace(x) => x.clone(),
                };
                match it.value_span() {
                    Some(sp) => vec![script::Edit { range: sp, text: wz::wrap("", &x).trim_start_matches(" = ").to_string() }],
                    None => Vec::new(),
                }
            }
        }
    });
    if ok {
        cx.info(format!("{label}: done (Ctrl+Z undoes it)"));
    }
}

fn code_tab(ui: &mut egui::Ui, cx: &mut Cx, key: &DefKey, st: &mut FormState) {
    let Some((d, text, s)) = cx.st.def(key) else { return };
    let current = match d.item(s) {
        Some(it) => script::item_source(text, it),
        None => text.to_string(),
    };
    let whole = d.path.is_empty();
    if st.code.as_ref().is_some_and(|(k, _)| k != key) {
        st.code = None;
    }
    let mut buf = st.code.as_ref().map(|x| x.1.clone()).unwrap_or_else(|| current.clone());
    let code_id = egui::Id::unique(("def_code", key));
    // script from the wizard: at the cursor, else before the closing brace
    let mut inserted = false;
    if let Some(snippet) = st.code_insert.take() {
        let cursor = egui::TextEdit::load_state(ui.ctx(), code_id).and_then(|s| s.cursor.char_range()).map(|r| r.primary.index.0);
        buf = match cursor {
            Some(c) => crate::content::wizard::insert_at(&buf, Some(c), &snippet),
            None if whole => crate::content::wizard::append(&buf, &snippet),
            None => crate::content::wizard::insert_before_close(&buf, &snippet),
        };
        inserted = true;
    }
    let parsed = Script::parse(&buf);
    let errs: Vec<usize> = parsed.errors.iter().map(|e| script::line_of(&buf, e.at)).collect();
    ui.horizontal(|ui| {
        let role = crate::content::wizard::role_of_kind(key.kind);
        if ui.small_button("🪄 Insert…").on_hover_text("Build effects, conditions or modifiers from menus; they go in after the line of the cursor").clicked() {
            let cursor = egui::TextEdit::load_state(ui.ctx(), code_id).and_then(|s| s.cursor.char_range()).map(|r| r.primary.index.0);
            st.wizard.open(wizard::Target { key: key.clone(), field: String::new(), nth: None, role, cursor });
        }
        if ui.small_button("Format").clicked() {
            buf = script::format(&buf);
            st.code = Some((key.clone(), buf.clone()));
        }
        if st.code.is_some() && ui.small_button("Throw away the changes").clicked() {
            st.code = None;
            buf = current.clone();
        }
        if let Some(e) = &st.code_err {
            ui.label(RichText::new(e).color(ERR).small());
        } else if st.code.is_some() {
            ui.label(RichText::new("not applied yet").color(WARN).small());
        } else {
            ui.label(RichText::new("changes go into the file as you type, when they parse").weak().small());
        }
    });
    let rows = ((ui.available_height() - 20.0) / 15.0).max(10.0) as usize;
    let out = code::editor(ui, code_id, &mut buf, code::Lang::Script, &errs, rows, &mut st.jump);
    if out.changed || inserted {
        st.code = Some((key.clone(), buf.clone()));
        let p = Script::parse(&buf);
        st.code_err = None;
        if let Some(e) = p.errors.first() {
            st.code_err = Some(format!("line {}: {}", script::line_of(&buf, e.at), e.message));
        } else if whole {
            cx.st.begin_merge(format!("edit {}", key.id), Some(format!("code:{key:?}")));
            cx.st.set_text(&key.pkg, &key.file, buf.clone());
            cx.st.commit();
            st.code = None;
        } else if p.root.items.len() != 1 {
            st.code_err = Some("the code must be one definition".into());
        } else {
            // its id from the new text
            let kinds_here = kinds::kinds_of_path(&key.file);
            let new_id = new_id_of(key.kind, &p, &kinds_here);
            match new_id {
                None => st.code_err = Some(format!("no {} id in it", key.kind.spec().one)),
                Some(nid) => {
                    let nk = DefKey { id: nid.clone(), ..key.clone() };
                    let ok = cx.edit_def(key, &format!("edit {}", key.id), Some(format!("code:{}", nid)), |t, _, it| vec![script::replace_item(t, it, &buf)]);
                    if ok {
                        st.code = None;
                        if nid != key.id {
                            let found = cx.st.find_in(&key.pkg, &[key.kind], &nid).unwrap_or(nk);
                            cx.go(Nav::Def(found));
                        }
                    }
                }
            }
        }
    }
}

/// The id of the one definition in a parsed snippet.
fn new_id_of(kind: Kind, s: &Script, _kinds: &[Kind]) -> Option<String> {
    let it = s.root.items.first()?;
    match kind.spec().entry {
        kinds::Entry::Keyed => it.key.as_ref().map(|k| k.text.clone()),
        kinds::Entry::Field(_, f) => it.block()?.atom(f).map(String::from),
        kinds::Entry::File => None,
    }
}

fn uses_tab(ui: &mut egui::Ui, cx: &mut Cx, key: &DefKey, st: &mut FormState) {
    let fresh = st.refs.as_ref().is_some_and(|(g, k, _)| *g == cx.st.generation && k == key);
    if !fresh {
        st.refs = Some((cx.st.generation, key.clone(), refactor::references(cx.st, &key.id, Some(key))));
    }
    let refs = st.refs.as_ref().unwrap().2.clone();
    if refs.is_empty() {
        let hint = match key.kind {
            Kind::Event => "Nothing fires it: give it a mean_time_to_happen, or fire it from a focus, decision or another event.",
            _ => "Nothing in the project names it.",
        };
        ui.label(RichText::new(hint).weak());
    }
    for (pkg, file, line, text) in refs.iter().take(300) {
        ui.horizontal(|ui| {
            if ui.link(format!("{pkg}/{file}:{line}")).clicked() {
                cx.go(Nav::File { pkg: pkg.clone(), file: file.clone(), line: Some(*line) });
            }
            ui.label(RichText::new(text).monospace().small());
        });
    }
}

fn duplicate(cx: &mut Cx, key: &DefKey) {
    let Some((d, text, s)) = cx.st.def(key) else { return };
    let Some(it) = d.item(s) else { return };
    let src = script::item_source(text, it);
    let group = d.group.clone();
    let taken: BTreeSet<String> = cx.st.defs().filter(|k| k.kind == key.kind).map(|k| k.id).collect();
    let new = if key.kind == Kind::Event {
        let ns = key.id.split('.').next().unwrap_or("");
        cx.new_id(Kind::Event, Some(ns))
    } else {
        (1..).map(|n| if n == 1 { format!("{}_copy", key.id) } else { format!("{}_copy{n}", key.id) }).find(|x| !taken.contains(x)).unwrap()
    };
    let old = key.id.clone();
    let (renamed, _) = script::rename_words(&src, &|w| {
        if w == old {
            Some(new.clone())
        } else if key.kind == Kind::Event && w.starts_with(&format!("{old}.")) {
            Some(format!("{new}{}", &w[old.len()..]))
        } else {
            None
        }
    });
    cx.st.begin(format!("duplicate {old}"));
    let r = import::insert_def(cx.st, &key.pkg, key.kind, group.as_deref(), &new, &renamed);
    match r {
        Ok(()) => {
            cx.st.commit();
            if let Some(k) = cx.st.find_in(&key.pkg, &[key.kind], &new) {
                cx.go(Nav::Def(k));
            }
            cx.info(format!("{new} is a copy of {old}"));
        }
        Err(e) => {
            cx.st.rollback();
            cx.error(format!("{e:#}"));
        }
    }
}

/// The rename box and the preview of what it changes.
pub fn rename_ui(ui: &mut egui::Ui, cx: &mut Cx, key: &DefKey, st: &mut FormState) {
    let Some((new, plan)) = &mut st.rename else { return };
    let mut close = false;
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label("new id");
            if ui.add(egui::TextEdit::singleline(new).desired_width(260.0)).changed() {
                *plan = None;
            }
            if ui.button("Preview").clicked() {
                *plan = Some(refactor::plan_rename(cx.st, &key.id, new.trim()).map_err(|e| format!("{e:#}")));
            }
            if ui.button("Cancel").clicked() {
                close = true;
            }
        });
        match plan {
            Some(Err(e)) => {
                ui.label(RichText::new(e.as_str()).color(ERR));
            }
            Some(Ok(p)) => {
                for n in &p.notes {
                    ui.label(RichText::new(n).color(WARN));
                }
                ui.label(format!("{} changes in {} files", p.count(), p.files.len()));
                plan_preview(ui, p, 12);
                if ui.button(RichText::new(format!("Rename to {}", new.trim())).color(OK)).clicked() {
                    cx.st.begin(p.label.clone());
                    refactor::apply(cx.st, p);
                    cx.st.commit();
                    let nk = cx.st.find_in(&key.pkg, &[key.kind], new.trim());
                    cx.info(format!("{}: {} changes", p.label, p.count()));
                    if let Some(k) = nk {
                        cx.go(Nav::Def(k));
                    }
                    close = true;
                }
            }
            None => {}
        }
    });
    if close {
        st.rename = None;
    }
}

/// The lines a plan changes (`max` per file).
pub fn plan_preview(ui: &mut egui::Ui, p: &refactor::Plan, max: usize) {
    egui::ScrollArea::vertical().id_salt(("plan", &p.label)).max_height(240.0).show(ui, |ui| {
        for f in &p.files {
            ui.label(RichText::new(format!("{}/{}  ({})", f.pkg, f.file, f.count)).strong().small());
            for l in f.lines.iter().take(max) {
                if !l.before.is_empty() {
                    ui.label(RichText::new(format!("{:>5} - {}", l.line, l.before.trim())).monospace().small().color(ERR));
                }
                if let Some(a) = &l.after {
                    ui.label(RichText::new(format!("{:>5} + {}", l.line, a.trim())).monospace().small().color(OK));
                }
            }
            if f.lines.len() > max {
                ui.label(RichText::new(format!("… {} more lines", f.lines.len() - max)).small().weak());
            }
        }
    });
}

fn delete_ui(ui: &mut egui::Ui, cx: &mut Cx, st: &mut FormState) {
    let Some(p) = &st.delete else { return };
    let mut close = false;
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.label(RichText::new(format!("{}: {} lines in {} files go", p.label, p.files.iter().map(|f| f.lines.len()).sum::<usize>(), p.files.len())).strong());
        plan_preview(ui, p, 6);
        if !p.dangling.is_empty() {
            ui.label(RichText::new(format!("{} places still name it and are left as they are (fix them after):", p.dangling.len())).color(WARN));
            for (pkg, file, line, text) in p.dangling.iter().take(20) {
                ui.label(RichText::new(format!("{pkg}/{file}:{line}  {text}")).monospace().small());
            }
        }
        ui.horizontal(|ui| {
            if ui.button(RichText::new("Delete").color(ERR)).clicked() {
                cx.st.begin(p.label.clone());
                refactor::apply(cx.st, p);
                cx.st.commit();
                cx.info(format!("{} (Ctrl+Z brings it back)", p.label));
                close = true;
            }
            if ui.button("Cancel").clicked() {
                close = true;
            }
        });
    });
    if close {
        st.delete = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::gfx::Gfx;
    use crate::content::store::Store;
    use crate::content::ui::{LocCache, Settings};
    use crate::content::wizard::Role;
    use crate::editor::MapFacts;

    const DECISIONS: &str = "TYR_cat = {\n\tTYR_d = {\n\t\tavailable = {\n\t\t\thas_war = yes\n\t\t}\n\t\tcomplete_effect = {\n\t\t\tadd_stability = 0.1\n\t\t}\n\t\tai_will_do = {\n\t\t\tfactor = 1\n\t\t}\n\t}\n}\n";
    const HISTORY: &str = "capital = 1\nadd_ideas = first_idea\n";

    fn store() -> Store {
        let mut st = Store::empty(std::env::temp_dir().join("mh-form-none"));
        st.begin("setup");
        st.set_text("TYR", "common/decisions/tyr.txt", DECISIONS.to_string());
        st.set_text("TYR", "history/countries/TYR - Tyrol.txt", HISTORY.to_string());
        st.commit();
        st
    }

    fn with_cx<R>(st: &mut Store, f: impl FnOnce(&mut Cx) -> R) -> R {
        let (mut gfx, mut settings, facts, loc) = (Gfx::default(), Settings::default(), MapFacts::default(), LocCache::default());
        let (mut msg, mut nav) = (None, None);
        let mut cx = Cx { st, base: None, gfx: &mut gfx, settings: &mut settings, facts: &facts, pkg: "TYR", nation: None, msg: &mut msg, nav: &mut nav, loc: &loc, game: std::path::Path::new("/none"), user_dir: std::path::Path::new("/none"), docs: &Default::default() };
        f(&mut cx)
    }

    fn key(st: &Store, kind: Kind) -> DefKey {
        st.defs_of(Some("TYR"), kind).into_iter().next().expect("a definition")
    }

    /// The field's source in the definition (empty: it has none).
    fn field(st: &Store, key: &DefKey, f: &str) -> String {
        let (d, text, s) = st.def(key).unwrap();
        d.block(s).and_then(|b| b.get(f)).map(|it| script::item_source(text, it)).unwrap_or_default()
    }

    fn target(key: &DefKey, field: &str, nth: Option<usize>, role: Role) -> wizard::Target {
        wizard::Target { key: key.clone(), field: field.into(), nth, role, cursor: None }
    }

    #[test]
    fn wizards_write_into_fields() {
        let mut st = store();
        let d = key(&st, Kind::Decision);
        with_cx(&mut st, |cx| {
            let mut fs = FormState::default();
            // an effect added at the end of the block
            apply_wizard(cx, &mut fs, target(&d, "complete_effect", Some(0), Role::Effect), wizard::Out::Add("add_political_power = 50".into()));
            // a factor replaces the one there
            apply_wizard(cx, &mut fs, target(&d, "ai_will_do", Some(0), Role::AiWeight), wizard::Out::Add("factor = 3\nmodifier = {\n\tfactor = 2\n\thas_war = yes\n}".into()));
            // a field it does not have yet
            apply_wizard(cx, &mut fs, target(&d, "cancel_trigger", None, Role::Trigger), wizard::Out::Add("has_war = no".into()));
        });
        assert_eq!(field(&st, &d, "complete_effect"), "complete_effect = {\n\tadd_stability = 0.1\n\tadd_political_power = 50\n}");
        // indented as the file is
        assert!(st.text("TYR", "common/decisions/tyr.txt").unwrap().contains("\t\t\tadd_stability = 0.1\n\t\t\tadd_political_power = 50\n\t\t}"));
        let ai = field(&st, &d, "ai_will_do");
        assert!(ai.contains("factor = 3") && !ai.contains("factor = 1") && ai.contains("modifier = {"), "{ai}");
        assert_eq!(field(&st, &d, "cancel_trigger"), "cancel_trigger = { has_war = no }");
        assert!(st.text("TYR", "common/decisions/tyr.txt").is_some_and(|t| crate::content::script::Script::parse(t).errors.is_empty()));
        // each is one step to undo
        for _ in 0..3 {
            st.undo();
        }
        assert_eq!(st.text("TYR", "common/decisions/tyr.txt"), Some(DECISIONS));
    }

    #[test]
    fn a_value_becomes_a_block_and_lists_are_replaced() {
        let mut st = store();
        let h = key(&st, Kind::CountryHistory);
        with_cx(&mut st, |cx| {
            let mut fs = FormState::default();
            apply_wizard(cx, &mut fs, target(&h, "add_ideas", Some(0), Role::Ideas), wizard::Out::Add("second_idea".into()));
        });
        assert_eq!(field(&st, &h, "add_ideas"), "add_ideas = {\n\tfirst_idea\n\tsecond_idea\n}");
        with_cx(&mut st, |cx| {
            let mut fs = FormState::default();
            apply_wizard(cx, &mut fs, target(&h, "add_ideas", Some(0), Role::Ideas), wizard::Out::Replace("third_idea".into()));
        });
        assert_eq!(field(&st, &h, "add_ideas"), "add_ideas = {\n\tthird_idea\n}");
    }

    #[test]
    fn the_code_tab_gets_it_at_its_cursor() {
        let mut st = store();
        let d = key(&st, Kind::Decision);
        with_cx(&mut st, |cx| {
            let mut fs = FormState::default();
            apply_wizard(cx, &mut fs, target(&d, "", None, Role::Effect), wizard::Out::Add("add_stability = 0.05".into()));
            assert_eq!(fs.code_insert.as_deref(), Some("add_stability = 0.05"));
            assert!(fs.tab == Tab::Code);
        });
        assert_eq!(st.text("TYR", "common/decisions/tyr.txt"), Some(DECISIONS));
    }
}
