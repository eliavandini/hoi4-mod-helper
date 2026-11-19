//! The stockpile of a nation at the start, at the top of its country
//! history's form: each `add_equipment_to_stockpile` of the history (also
//! those inside `if = { limit = { has_dlc = ... } }`), with its model, how
//! many (changed in place), who made it and for which players; whether the
//! nation can make the model itself; a quick fix for a model of a DLC given
//! to every player; and a row to add one.

use std::sync::Arc;

use eframe::egui::{self, Color32, RichText, Sense, Vec2};

use super::{Cx, ERR, OK, WARN};
use crate::content::dlc::{self, Need};
use crate::content::loc;
use crate::content::script::{self, Block, Item, Span};
use crate::content::store::DefKey;
use crate::content::units::{self, UnitDb};

pub const FIELD: &str = "add_equipment_to_stockpile";

#[derive(Default)]
pub struct StockState {
    model: String,
    amount: String,
}

/// One line of the stockpile.
struct Row {
    span: Span,
    need: Need,
    model: String,
    amount: Option<f64>,
    producer: Option<String>,
    variant: Option<String>,
}

/// A change asked for.
enum Act {
    Amount(Span, f64),
    Remove(Span),
    /// Put it inside `if = { limit = { ... } }`.
    Guard(Span, Need),
    Add(String, f64, Need),
}

/// The stockpile lines of a history (its root).
fn rows(b: &Block) -> Vec<Row> {
    let mut out = Vec::new();
    dlc::walk_flow(b, &Need::default(), &mut |i, need| {
        if i.is(FIELD)
            && let Some(blk) = i.block()
        {
            out.push(Row {
                span: i.span.clone(),
                need: need.clone(),
                model: blk.atom("type").unwrap_or("").to_string(),
                amount: blk.num("amount"),
                producer: blk.atom("producer").map(String::from),
                variant: blk.atom("variant_name").map(String::from),
            });
        }
    });
    out
}

fn name(cx: &Cx, id: &str) -> String {
    cx.text(id).map(loc::plain).filter(|t| !t.trim().is_empty()).unwrap_or_else(|| loc::prettify(id))
}

/// The models of equipment to pick from (no families): id → "name · family · year".
pub fn models(cx: &Cx, db: &UnitDb) -> Arc<Vec<(String, String)>> {
    let mut v: Vec<(String, String, f64, String)> = db
        .equipment
        .values()
        .filter(|e| !e.is_archetype)
        .map(|e| {
            let arch = e.archetype.clone().unwrap_or_default();
            let need = db.need_of(&e.id);
            let dlc = if need.is_empty() { String::new() } else { format!(" · only {}", need.describe()) };
            (e.id.clone(), format!("{} · {} · {}{dlc}", name(cx, &e.id), name(cx, &arch), e.year as i64), e.year, arch)
        })
        .collect();
    v.sort_by(|a, b| (&a.3, a.2, &a.0).partial_cmp(&(&b.3, b.2, &b.0)).unwrap_or(std::cmp::Ordering::Equal));
    Arc::new(v.into_iter().map(|(id, l, _, _)| (id, l)).collect())
}

fn num(v: f64) -> String {
    if (v - v.round()).abs() < 1e-9 { format!("{}", v.round() as i64) } else { format!("{v}") }
}

pub fn panel(ui: &mut egui::Ui, cx: &mut Cx, key: &DefKey, ps: &mut StockState) {
    let Some(base) = cx.base else { return };
    let db = units::db(Some(base), cx.st);
    let Some(list) = cx.st.def(key).and_then(|(d, _, s)| d.block(s).map(rows)) else { return };
    let tag = key.pkg.clone();
    let owned = super::division::owned(cx, &db, &tag);
    let have = db.at_start(cx.st, &tag, Some(&owned));
    let mut acts: Vec<Act> = Vec::new();
    egui::CollapsingHeader::new(RichText::new(format!("Stockpile ({})", list.len())).strong()).id_salt("stockpile").default_open(true).show(ui, |ui| {
        ui.label(
            RichText::new(format!(
                "Equipment {tag} has at the start, besides what its divisions carry (add_equipment_to_stockpile). Divisions draw from it, so a model {tag} cannot make yet still equips them; the division designer counts it. Drag an amount to change it."
            ))
            .weak()
            .small(),
        );
        if !list.is_empty() {
            ui.scope(|ui| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                egui::Grid::new("stock_rows").striped(true).num_columns(5).spacing([12.0, 4.0]).show(ui, |ui| {
                    for r in &list {
                        row(ui, cx, &db, &have, &tag, r, &mut acts);
                        ui.end_row();
                    }
                });
            });
        }
        ui.add_space(4.0);
        add_row(ui, cx, &db, ps, &mut acts);
    });
    apply(cx, key, acts);
}

fn row(ui: &mut egui::Ui, cx: &mut Cx, db: &UnitDb, have: &units::Have, tag: &str, r: &Row, acts: &mut Vec<Act>) {
    let e = db.equipment.get(&r.model);
    // the model, with its family's icon
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(36.0, 20.0), Sense::hover());
        let arch = db.archetype_of(&r.model).to_string();
        if let Some((tex, size)) = cx.texture(ui.ctx(), &format!("GFX_archetype_{arch}_medium")).filter(|(_, s)| s.x > 0.0 && s.y > 0.0) {
            let k = (rect.width() / size.x).min(rect.height() / size.y);
            ui.painter().image(tex, egui::Rect::from_center_size(rect.center(), size * k), egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
        }
        ui.label(if r.model.is_empty() { "(no type)".to_string() } else { name(cx, &r.model) }).on_hover_text(&r.model);
    });
    // how many
    match r.amount {
        Some(n) => {
            let mut v = n;
            let speed = (n.abs() / 100.0).max(1.0);
            let resp = ui.add(egui::DragValue::new(&mut v).speed(speed).range(-1.0e7..=1.0e7));
            if resp.changed() && (v - n).abs() > 1e-9 {
                acts.push(Act::Amount(r.span.clone(), v.round()));
            }
        }
        None => {
            ui.label(RichText::new("no amount").color(ERR));
        }
    }
    // who made it, which design, for whom
    let mut who = Vec::new();
    if let Some(p) = &r.producer {
        who.push(format!("made by {p}"));
    }
    if let Some(v) = &r.variant {
        who.push(format!("design \"{v}\""));
    }
    if !r.need.is_empty() {
        who.push(format!("players {}", r.need.describe()));
    }
    ui.label(RichText::new(if who.is_empty() { "every player".to_string() } else { who.join(" · ") }).small());
    // what it says
    let need = db.need_of(&r.model);
    let mut fix = None;
    if e.is_none() {
        ui.label(RichText::new(if r.model.is_empty() { "it needs type = <model>".to_string() } else { "not equipment of the game or the mod".to_string() }).color(ERR).small());
    } else if e.is_some_and(|e| e.is_archetype) {
        ui.label(RichText::new("a family of models: give a model of it (…_1)").color(WARN).small());
    } else if !need.is_empty() && !need.within(&r.need) {
        ui.label(RichText::new(format!("a model only players {} get", need.describe())).color(WARN).small()).on_hover_text(format!(
            "The technologies that unlock {} are in the research tree only for players {}. The game's own history gives such equipment inside if = {{ limit = {{ {} }} }} and another model in its else.",
            r.model,
            need.describe(),
            need.trigger()
        ));
        fix = Some(need.and(&r.need));
    } else if have.equipment.contains(&r.model) {
        ui.label(RichText::new(format!("✔ {tag} can make it")).color(OK).small());
    } else {
        let techs = db.unlocked_text(&[r.model.as_str()], &|i| name(cx, i), 2);
        let t = if techs.is_empty() { format!("{tag} cannot make it") } else { format!("{tag} cannot make it yet: it comes with {techs}") };
        ui.label(RichText::new(t).small().weak());
    }
    ui.horizontal(|ui| {
        if let Some(n) = fix
            && ui.small_button("only for them").on_hover_text(format!("Put it inside if = {{ limit = {{ {} }} }}", n.trigger())).clicked()
        {
            acts.push(Act::Guard(r.span.clone(), n));
        }
        if ui.small_button("✖").on_hover_text("Remove this line").clicked() {
            acts.push(Act::Remove(r.span.clone()));
        }
    });
}

fn add_row(ui: &mut egui::Ui, cx: &mut Cx, db: &UnitDb, ps: &mut StockState, acts: &mut Vec<Act>) {
    ui.horizontal_wrapped(|ui| {
        ui.label("Add");
        ui.add(egui::TextEdit::singleline(&mut ps.amount).desired_width(60.0).hint_text("1000"));
        ui.label("×");
        let shown = if ps.model.is_empty() { "a model…".to_string() } else { format!("{} ({})", name(cx, &ps.model), ps.model) };
        let id = egui::Id::unique("stock_model");
        ui.label(RichText::new(shown).strong());
        let items = || models(cx, db);
        if let Some(v) = super::wizard::pick_menu(ui, id, &ps.model, &items) {
            ps.model = v;
        }
        let amount = ps.amount.trim().parse::<f64>().ok().or(ps.amount.trim().is_empty().then_some(1000.0));
        let need = db.need_of(&ps.model);
        let ok = db.equipment.get(&ps.model).is_some_and(|e| !e.is_archetype) && amount.is_some_and(|a| a != 0.0);
        if ui.add_enabled(ok, egui::Button::new("Add")).clicked()
            && let Some(a) = amount
        {
            acts.push(Act::Add(ps.model.clone(), a, need.clone()));
            ps.model.clear();
        }
        if !need.is_empty() && ok {
            ui.label(RichText::new(format!("only players {} get it: it goes inside if = {{ limit = {{ {} }} }}", need.describe(), need.trigger())).small().weak());
        }
    });
}

/// The block of a stockpile line.
fn line(model: &str, amount: f64) -> String {
    format!("{FIELD} = {{\n\ttype = {model}\n\tamount = {}\n}}", num(amount))
}

/// `if = { limit = { ... } <body> }`.
fn guarded(need: &Need, body: &str) -> String {
    format!("if = {{\n\tlimit = {{ {} }}\n{}\n}}", need.trigger(), crate::content::wizard::indent(body))
}

/// Equipment put in the stockpile of a history (its `root`): each model
/// (with the DLCs a player needs for it) added to its line at the top when
/// it has a plain one (no producer, no variant), else in a new line after
/// the last at the top (inside `if = { limit = { ... } }` for a model of a
/// DLC).
pub(super) fn add_to(text: &str, root: &Block, adds: &[(String, f64, Need)]) -> Vec<script::Edit> {
    let mut edits = Vec::new();
    let mut new = Vec::new();
    for (model, amount, need) in adds {
        let plain = root.items.iter().filter(|i| i.is(FIELD)).filter_map(|i| i.block()).find(|b| b.atom("type") == Some(model.as_str()) && b.get("producer").is_none() && b.get("variant_name").is_none());
        match plain {
            Some(b) if need.is_empty() => edits.push(script::set_atom(text, b, "amount", &num(b.num("amount").unwrap_or(0.0) + amount))),
            _ => {
                let body = line(model, *amount);
                new.push(if need.is_empty() { body } else { guarded(need, &body) });
            }
        }
    }
    if !new.is_empty() {
        edits.push(script::insert_item(text, root, &new.join("\n"), root.items.iter().rposition(|i| i.is(FIELD))));
    }
    edits
}

/// The item at a span, anywhere in a block.
fn item_at<'a>(b: &'a Block, span: &Span) -> Option<&'a Item> {
    for i in &b.items {
        if i.span == *span {
            return Some(i);
        }
        if let Some(found) = i.block().and_then(|c| item_at(c, span)) {
            return Some(found);
        }
    }
    None
}

fn apply(cx: &mut Cx, key: &DefKey, acts: Vec<Act>) {
    for a in acts {
        let (label, merge) = match &a {
            Act::Amount(span, _) => ("stockpile: amount".to_string(), Some(format!("stock_amount_{}_{}", key.file, span.start))),
            Act::Remove(_) => ("stockpile: remove a line".to_string(), None),
            Act::Guard(_, n) => (format!("stockpile: only players {}", n.describe()), None),
            Act::Add(m, ..) => (format!("stockpile: add {m}"), None),
        };
        cx.edit_def(key, &label, merge, |text, b, _| edits(text, b, &a));
    }
}

fn edits(text: &str, b: &Block, a: &Act) -> Vec<script::Edit> {
    match a {
        Act::Amount(span, v) => {
            let Some(it) = item_at(b, span) else { return Vec::new() };
            let Some(blk) = it.block() else { return Vec::new() };
            vec![script::set_atom(text, blk, "amount", &num(*v))]
        }
        Act::Remove(span) => item_at(b, span).map(|it| vec![script::remove_item(text, it)]).unwrap_or_default(),
        Act::Guard(span, need) => {
            let Some(it) = item_at(b, span) else { return Vec::new() };
            vec![script::replace_item(text, it, &guarded(need, &script::item_source(text, it)))]
        }
        Act::Add(model, amount, need) => {
            let body = line(model, *amount);
            let rendered = if need.is_empty() { body } else { guarded(need, &body) };
            // after the last stockpile line at the top, else at the end
            let after = b.items.iter().rposition(|i| i.is(FIELD));
            vec![script::insert_item(text, b, &rendered, after)]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::script::Script;

    fn run(text: &str, a: Act) -> String {
        let s = Script::parse(text);
        script::apply(text, &edits(text, &s.root, &a))
    }

    const H: &str = "capital = 5\nadd_equipment_to_stockpile = { type = infantry_equipment_1 amount = 500 } # rifles\nif = {\n\tlimit = { has_dlc = \"No Step Back\" }\n\tadd_equipment_to_stockpile = {\n\t\ttype = light_tank_chassis_1\n\t\tamount = 20\n\t}\n}\n";

    #[test]
    fn stockpile_lines_are_read_with_their_players() {
        let s = Script::parse(H);
        let r = rows(&s.root);
        assert_eq!(r.len(), 2);
        assert_eq!((r[0].model.as_str(), r[0].amount, r[0].need.is_empty()), ("infantry_equipment_1", Some(500.0), true));
        assert_eq!((r[1].model.as_str(), r[1].need.describe()), ("light_tank_chassis_1", "with No Step Back".to_string()));
    }

    #[test]
    fn equipment_is_added_to_the_lines_there_are() {
        let text = "capital = 3\nadd_equipment_to_stockpile = { type = infantry_equipment_1 amount = 100 }\nadd_equipment_to_stockpile = { type = artillery_equipment_1 amount = 5 producer = GER }\nset_stability = 0.5\n";
        let s = Script::parse(text);
        let dlc = Need::with("No Step Back");
        let t = script::apply(text, &add_to(text, &s.root, &[("infantry_equipment_1".into(), 50.0, Need::default()), ("artillery_equipment_1".into(), 12.0, Need::default()), ("light_tank_chassis_1".into(), 3.0, dlc)]));
        let r = rows(&Script::parse(&t).root);
        let got: Vec<(String, Option<f64>, bool)> = r.iter().map(|r| (r.model.clone(), r.amount, r.need.is_empty())).collect();
        assert_eq!(got, [("infantry_equipment_1".to_string(), Some(150.0), true), ("artillery_equipment_1".into(), Some(5.0), true), ("artillery_equipment_1".into(), Some(12.0), true), ("light_tank_chassis_1".into(), Some(3.0), false)], "{t}");
        assert!(t.ends_with("set_stability = 0.5\n"), "after the last line at the top:\n{t}");
    }

    #[test]
    fn stockpile_edits_keep_the_rest() {
        let s = Script::parse(H);
        let r = rows(&s.root);
        assert_eq!(run(H, Act::Amount(r[0].span.clone(), 750.0)), H.replace("amount = 500", "amount = 750"));
        assert_eq!(run(H, Act::Amount(r[1].span.clone(), 40.0)), H.replace("amount = 20", "amount = 40"));
        assert_eq!(run(H, Act::Remove(r[0].span.clone())), H.replace("add_equipment_to_stockpile = { type = infantry_equipment_1 amount = 500 } # rifles\n", ""));
        let added = run(H, Act::Add("artillery_equipment_1".into(), 100.0, Need::default()));
        assert!(added.contains("# rifles\nadd_equipment_to_stockpile = {\n\ttype = artillery_equipment_1\n\tamount = 100\n}\nif = {"), "{added}");
        let added = run("capital = 5\n", Act::Add("light_tank_equipment_1".into(), 20.0, Need::without("No Step Back")));
        assert_eq!(added, "capital = 5\nif = {\n\tlimit = { NOT = { has_dlc = \"No Step Back\" } }\n\tadd_equipment_to_stockpile = {\n\t\ttype = light_tank_equipment_1\n\t\tamount = 20\n\t}\n}\n");
        let t = "add_equipment_to_stockpile = { type = light_tank_chassis_1 amount = 5 }\n";
        let r = rows(&Script::parse(t).root);
        assert_eq!(run(t, Act::Guard(r[0].span.clone(), Need::with("No Step Back"))), "if = {\n\tlimit = { has_dlc = \"No Step Back\" }\n\tadd_equipment_to_stockpile = { type = light_tank_chassis_1 amount = 5 }\n}\n");
    }
}
