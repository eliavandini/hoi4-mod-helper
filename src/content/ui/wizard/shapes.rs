//! The pages of fields built whole from a form ([`crate::content::shapes`]):
//! named parts with pickers, ticked lists, weights, names, rules, a colour
//! and equipment bonuses. What the field has that the form does not know is
//! listed at the bottom and kept.

use super::*;
use crate::content::shapes::{Group, Part, PartTy, Shape, Words};

/// A part of the form to build with a catalog: (path, label, catalog).
type Build = Option<(String, String, Role)>;

pub(super) fn page(ui: &mut egui::Ui, cx: &mut Cx, ws: &mut WizardState, t: &Target, shape: Shape) {
    if let Some(e) = &ws.form.broken {
        ui.label(RichText::new(format!("The field does not parse ({e}). Fix it in the form, then open the wizard again.")).color(ERR));
        return;
    }
    if ws.form.comments {
        ui.label(RichText::new("The comments in the field do not come back when it is set.").weak().small());
    }
    match shape {
        Shape::Words(w) => words_page(ui, cx, ws, w),
        _ => {
            egui::ScrollArea::vertical().id_salt("shape_page").auto_shrink([false, false]).show(ui, |ui| {
                match shape {
                    Shape::Values(parts) => {
                        let mut build = None;
                        parts_ui(ui, cx, &mut ws.lists, &mut ws.form, parts, "", &mut build);
                        if let Some((path, label, role)) = build {
                            ws.cond = ws.form.vals.get(&path).cloned().unwrap_or_default();
                            ws.part = (path, label);
                            ws.cond_for = Some((CondFor::Part, t.role));
                            ws.page = Some(role);
                            ws.pick = None;
                        }
                    }
                    Shape::Weights(w) => weights_page(ui, cx, ws, w),
                    Shape::Texts { loc } => texts_page(ui, cx, ws, loc),
                    Shape::Rules(known) => rules_page(ui, ws, known),
                    Shape::Colour => colour_page(ui, ws),
                    Shape::EquipmentBonus => bonus_page(ui, cx, ws),
                    Shape::Words(_) => {}
                }
                kept(ui, &ws.form, "");
            });
        }
    }
}

/// What the form does not know of a block: kept as it is.
fn kept(ui: &mut egui::Ui, form: &sh::Form, block: &str) {
    let Some(o) = form.others.get(block).filter(|o| !o.is_empty()) else { return };
    ui.add_space(6.0);
    let text = o.join("\n");
    egui::CollapsingHeader::new(RichText::new(format!("Kept as it is ({} the form does not know)", o.len())).weak()).id_salt(("shape_kept", block)).show(ui, |ui| {
        code::view(ui, egui::Id::unique(("shape_kept_code", block)), &text, code::Lang::Script, text.lines().count().clamp(1, 10));
    });
}

fn label(ui: &mut egui::Ui, p: &Part) {
    let tip = format!("{} ({})", p.key, if p.need { "needed" } else { "may be left empty" });
    let text = if p.need { RichText::new(p.label) } else { RichText::new(p.label).weak() };
    ui.add_sized([170.0, 18.0], egui::Label::new(text).truncate()).on_hover_text(tip);
}

fn parts_ui(ui: &mut egui::Ui, cx: &mut Cx, lists: &mut Lists, form: &mut sh::Form, parts: &[Part], prefix: &str, build: &mut Build) {
    for p in parts {
        let path = format!("{prefix}{}", p.key);
        if let PartTy::Sub(ps) = p.ty {
            ui.add_space(4.0);
            ui.label(RichText::new(p.label).strong()).on_hover_text(p.key);
            ui.indent(("shape_sub", &path), |ui| parts_ui(ui, cx, lists, form, ps, &format!("{path}."), build));
            continue;
        }
        let id = egui::Id::unique(("shape_part", &path));
        let mut v = form.vals.get(&path).cloned().unwrap_or_default();
        ui.horizontal_top(|ui| {
            label(ui, p);
            match p.ty {
                PartTy::One(Param::Sprite) => sprite_ui(ui, cx, lists, id, &mut v),
                PartTy::One(param) => param_ui(ui, cx, lists, id, param, &mut v),
                PartTy::Script(role) => {
                    ui.vertical(|ui| {
                        ui.set_max_width(ui.available_width() - 40.0);
                        let errs: Vec<usize> = Script::parse(&v).errors.iter().map(|e| script::line_of(&v, e.at)).collect();
                        let mut jump = None;
                        let rows = v.lines().count().clamp(2, 8);
                        code::editor(ui, id, &mut v, code::Lang::Script, &errs, rows, &mut jump);
                        if role == Role::AiWeight {
                            ui.label(RichText::new("factor = 1, and modifier = { factor = 2 <conditions> } blocks").weak().small());
                        }
                    });
                    if role.is_catalog() && ui.small_button("🪄").on_hover_text(format!("build it from the {}", role.label().to_lowercase())).clicked() {
                        *build = Some((path.clone(), p.label.to_string(), role));
                    }
                }
                PartTy::Words(param) => words_part(ui, cx, lists, id, param, &mut v),
                PartTy::Texts => {
                    ui.vertical(|ui| {
                        let rows = v.lines().count().clamp(2, 8);
                        ui.add(egui::TextEdit::multiline(&mut v).id(id).desired_rows(rows).desired_width(320.0).hint_text("one per line"));
                        ui.label(RichText::new(format!("{} (one per line)", v.lines().filter(|l| !l.trim().is_empty()).count())).weak().small());
                    });
                }
                PartTy::Sub(_) => {}
            }
        });
        if let Some(o) = form.others.get(&path).filter(|o| !o.is_empty()) {
            ui.label(RichText::new(format!("    also kept in it: {}", o.join(" "))).weak().small());
        }
        if form.vals.get(&path) != Some(&v) {
            form.vals.insert(path, v);
        }
    }
    let block = prefix.trim_end_matches('.');
    if !block.is_empty() {
        kept(ui, form, block);
    }
}

/// A sprite: its name with a list, and its picture.
fn sprite_ui(ui: &mut egui::Ui, cx: &mut Cx, lists: &mut Lists, id: egui::Id, v: &mut String) {
    let l = list(cx, lists, Param::Sprite);
    choose(ui, id, v, &|| l.clone(), 260.0);
    if !v.trim().is_empty() {
        let tex = cx.texture(ui.ctx(), v.trim());
        let r = crate::content::gfx::show(ui, tex, egui::vec2(40.0, 52.0));
        if tex.is_none() {
            r.on_hover_text("no picture for this sprite");
        }
    }
}

/// A few words: each with ✖, and a field (with a list) to add one.
fn words_part(ui: &mut egui::Ui, cx: &Cx, lists: &mut Lists, id: egui::Id, param: Param, v: &mut String) {
    let mut words: Vec<String> = v.split_whitespace().map(String::from).collect();
    ui.vertical(|ui| {
        if !words.is_empty() {
            ui.horizontal_wrapped(|ui| {
                let mut gone = None;
                for (i, w) in words.iter().enumerate() {
                    let mut r = ui.small_button(format!("{w}  ✖"));
                    if let Param::Ref(k) = param {
                        let known = cx.st.find(k, w).is_some() || cx.base.is_some_and(|b| b.has(k, w));
                        r = r.on_hover_text(if known { cx.name_of(k, w) } else { format!("no {} {w}", k.spec().one) });
                    }
                    if r.clicked() {
                        gone = Some(i);
                    }
                }
                if let Some(i) = gone {
                    words.remove(i);
                }
            });
        }
        ui.horizontal(|ui| {
            let aid = id.with("add");
            let mut add: String = ui.data(|d| d.get_temp(aid.with("text"))).unwrap_or_default();
            let r = ui.add(egui::TextEdit::singleline(&mut add).id(aid).desired_width(180.0).hint_text("add one"));
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let l = list(cx, lists, param);
            if !l.is_empty()
                && let Some(x) = pick_menu(ui, aid, "", &|| l.clone())
                && !words.contains(&x)
            {
                words.push(x);
            }
            if (ui.small_button("+").clicked() || enter) && !add.trim().is_empty() {
                for x in add.split_whitespace() {
                    if !words.iter().any(|w| w == x) {
                        words.push(x.to_string());
                    }
                }
                add.clear();
            }
            ui.data_mut(|d| d.insert_temp(aid.with("text"), add));
        });
    });
    *v = words.join(" ");
}

/// A field to add words not in the list.
fn add_words(ui: &mut egui::Ui, id: &str, words: &mut Vec<String>) {
    ui.horizontal(|ui| {
        let aid = egui::Id::unique(("shape_add_words", id));
        let mut add: String = ui.data(|d| d.get_temp(aid.with("text"))).unwrap_or_default();
        let r = ui.add(egui::TextEdit::singleline(&mut add).id(aid).desired_width(220.0).hint_text("one not in the list"));
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if (ui.button("+ add").clicked() || enter) && !add.trim().is_empty() {
            for x in add.split_whitespace() {
                if !words.iter().any(|w| w == x) {
                    words.push(x.to_string());
                }
            }
            add.clear();
        }
        ui.data_mut(|d| d.insert_temp(aid.with("text"), add));
    });
}

fn words_page(ui: &mut egui::Ui, cx: &mut Cx, ws: &mut WizardState, w: Words) {
    ui.label(RichText::new(w.help).weak());
    add_words(ui, "page", &mut ws.form.words);
    if w.ordered {
        ui.label(RichText::new("In order (the first is taken first):").strong());
        let mut swap = None;
        let mut gone = None;
        let n = ws.form.words.len();
        egui::ScrollArea::vertical().id_salt("shape_order").max_height(180.0).auto_shrink([false, true]).show(ui, |ui| {
            for (i, x) in ws.form.words.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{:>3}.", i + 1)).monospace().weak());
                    if ui.add_enabled(i > 0, egui::Button::new("↑").small()).clicked() {
                        swap = Some((i, i - 1));
                    }
                    if ui.add_enabled(i + 1 < n, egui::Button::new("↓").small()).clicked() {
                        swap = Some((i, i + 1));
                    }
                    if ui.small_button("✖").clicked() {
                        gone = Some(i);
                    }
                    let name = if let Param::Ref(k) = w.of { cx.name_of(k, x) } else { String::new() };
                    ui.label(RichText::new(x).monospace());
                    ui.label(RichText::new(name).weak().small());
                });
            }
        });
        if let Some((a, b)) = swap {
            ws.form.words.swap(a, b);
        }
        if let Some(i) = gone {
            ws.form.words.remove(i);
        }
        ui.separator();
        ui.label(RichText::new("Tick to add at the end:").weak());
    }
    let items = list(cx, &mut ws.lists, w.of);
    let mut search = std::mem::take(&mut ws.search);
    tick_list(ui, "shape_words", &items, &mut ws.form.words, &ws.first, &mut search);
    ws.search = search;
    kept(ui, &ws.form, "");
}

fn weights_page(ui: &mut egui::Ui, cx: &mut Cx, ws: &mut WizardState, w: sh::Weights) {
    ui.label(RichText::new(w.help).weak());
    ui.add_space(4.0);
    let mut gone = None;
    egui::Grid::new("shape_weights").num_columns(3).striped(true).show(ui, |ui| {
        ui.label(RichText::new(w.what).strong());
        ui.label(RichText::new(w.weight).strong());
        ui.label("");
        ui.end_row();
        for (i, (what, n)) in ws.form.rows.iter_mut().enumerate() {
            ui.horizontal(|ui| param_ui(ui, cx, &mut ws.lists, egui::Id::unique(("shape_weight_what", i)), w.of, what));
            ui.add(egui::TextEdit::singleline(n).id(egui::Id::unique(("shape_weight_n", i))).desired_width(70.0));
            if ui.small_button("✖").clicked() {
                gone = Some(i);
            }
            ui.end_row();
        }
    });
    if let Some(i) = gone {
        ws.form.rows.remove(i);
    }
    if ui.button("+ add a row").clicked() {
        ws.form.rows.push((String::new(), if w.swap { "100" } else { "1" }.to_string()));
    }
}

fn texts_page(ui: &mut egui::Ui, cx: &Cx, ws: &mut WizardState, loc: bool) {
    ui.label(RichText::new(if loc { "Localisation keys of the names, one per line (their texts are on the Localisation page)." } else { "One per line; they are written in quotes." }).weak());
    let n = ws.form.texts.lines().filter(|l| !l.trim().is_empty()).count();
    ui.label(RichText::new(format!("{n} of them")).weak().small());
    ui.horizontal_top(|ui| {
        ui.add(egui::TextEdit::multiline(&mut ws.form.texts).id(egui::Id::unique("shape_texts")).desired_rows(18).desired_width(if loc { 300.0 } else { 420.0 }));
        if loc {
            ui.vertical(|ui| {
                for k in ws.form.texts.lines().map(str::trim).filter(|l| !l.is_empty()).take(40) {
                    match cx.text(k) {
                        Some(t) => ui.label(RichText::new(crate::content::loc::plain(t)).small()),
                        None => ui.label(RichText::new(format!("{k}: no text yet")).color(WARN).small()),
                    };
                }
            });
        }
    });
}

fn rules_page(ui: &mut egui::Ui, ws: &mut WizardState, known: &[(&str, &str)]) {
    ui.label(RichText::new("What the countries can do: yes, no, or as the game decides (left out).").weak());
    ui.add_space(4.0);
    egui::Grid::new("shape_rules").num_columns(3).striped(true).show(ui, |ui| {
        for (rule, v) in ws.form.rules.iter_mut() {
            let what = known.iter().find(|k| k.0 == rule.as_str()).map(|k| k.1).unwrap_or("");
            ui.label(RichText::new(rule.as_str()).monospace());
            ui.label(RichText::new(what).weak());
            ui.horizontal(|ui| {
                ui.selectable_value(v, None, "—");
                ui.selectable_value(v, Some(true), "yes");
                ui.selectable_value(v, Some(false), "no");
            });
            ui.end_row();
        }
    });
    let mut add: Vec<String> = Vec::new();
    add_words(ui, "rules", &mut add);
    for r in add {
        if !ws.form.rules.iter().any(|x| x.0 == r) {
            ws.form.rules.push((r, Some(true)));
        }
    }
}

fn colour_page(ui: &mut egui::Ui, ws: &mut WizardState) {
    ui.label(RichText::new("The colour of the ideology on the map and in the charts.").weak());
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.color_edit_button_srgb(&mut ws.form.colour);
        for (i, l) in ["r ", "g ", "b "].iter().enumerate() {
            ui.add(egui::DragValue::new(&mut ws.form.colour[i]).range(0..=255).prefix(*l));
        }
    });
    let (r, _) = ui.allocate_exact_size(egui::vec2(160.0, 40.0), egui::Sense::hover());
    let [red, green, blue] = ws.form.colour;
    ui.painter().rect_filled(r, 4.0, egui::Color32::from_rgb(red, green, blue));
}

fn bonus_page(ui: &mut egui::Ui, cx: &mut Cx, ws: &mut WizardState) {
    ui.label(RichText::new("Stats of equipment while the idea or trait is there: 0.1 is +10 %, -0.1 makes it 10 % cheaper.").weak());
    ui.add_space(4.0);
    let eq = list(cx, &mut ws.lists, Param::Ref(Kind::Equipment));
    let stats = list(cx, &mut ws.lists, Param::Of(sh::EQUIPMENT_STATS));
    let mut gone = None;
    for (gi, g) in ws.form.groups.iter_mut().enumerate() {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("Equipment");
                choose(ui, egui::Id::unique(("shape_bonus_eq", gi)), &mut g.equipment, &|| eq.clone(), 220.0);
                ui.checkbox(&mut g.instant, "instant").on_hover_text("instant = yes: the equipment already there gets it at once (else only models designed after)");
                if ui.small_button("✖ remove").clicked() {
                    gone = Some(gi);
                }
            });
            let mut sgone = None;
            for (si, (k, v)) in g.stats.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.add_space(20.0);
                    choose(ui, egui::Id::unique(("shape_bonus_stat", gi, si)), k, &|| stats.clone(), 200.0);
                    ui.add(egui::TextEdit::singleline(v).id(egui::Id::unique(("shape_bonus_v", gi, si))).desired_width(70.0));
                    if ui.small_button("✖").clicked() {
                        sgone = Some(si);
                    }
                });
            }
            if let Some(si) = sgone {
                g.stats.remove(si);
            }
            ui.horizontal(|ui| {
                ui.add_space(20.0);
                if ui.small_button("+ a stat").clicked() {
                    g.stats.push(("build_cost_ic".into(), "-0.1".into()));
                }
            });
            if !g.others.is_empty() {
                ui.label(RichText::new(format!("also kept in it: {}", g.others.join(" "))).weak().small());
            }
        });
    }
    if let Some(gi) = gone {
        ws.form.groups.remove(gi);
    }
    if ui.button("+ an equipment").clicked() {
        ws.form.groups.push(Group { equipment: "infantry_equipment".into(), instant: true, stats: vec![("build_cost_ic".into(), "-0.1".into())], others: Vec::new() });
    }
}
