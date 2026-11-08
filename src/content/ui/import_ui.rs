//! Importing content into a nation: from the base game, from a mod of the
//! game's mod folder (folder or zip), from any folder laid out like a mod,
//! from a bundle exported before, or from another nation of the project.
//! Pick what to bring (a focus tree, events, ideas, a whole tag's content);
//! the import brings what that uses, renames the source's tag to the
//! nation's, gives clashing ids the nation's tag, and brings the texts,
//! sprites and pictures. A plan shows all of it before anything changes.
//!
//! Exporting: a nation's content as a bundle (a zip another project, or
//! another nation, can import), as plain files, or copied into another
//! nation of the map.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use crossbeam_channel::Receiver;
use eframe::egui::{self, RichText};

use super::code::{self, Lang};
use super::{Cx, ERR, Nav, OK, Page, WARN};
use crate::content::import::{self, Options, Plan};
use crate::content::kinds::{self, Kind};
use crate::content::script::Script;
use crate::content::source::{self, Files, Index, Origin};
use crate::content::store::{DefKey, SHARED};
use crate::content::zip;
use crate::editor::launcher;

/// Where content can come from.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Src {
    Base,
    Mod { name: String, path: PathBuf },
    Folder(PathBuf),
    Bundle(PathBuf),
    Nation(String),
}

impl Src {
    fn label(&self) -> String {
        match self {
            Src::Base => "the base game".into(),
            Src::Mod { name, .. } => format!("mod {name}"),
            Src::Folder(p) => format!("folder {}", p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())),
            Src::Bundle(p) => format!("bundle {}", p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
            Src::Nation(t) => format!("nation {t}"),
        }
    }
}

struct Job {
    src: Src,
    progress: Arc<AtomicU32>,
    rx: Receiver<Result<(Index, Option<serde_json::Value>), String>>,
}

#[derive(Default)]
pub struct ImportState {
    mods: Option<Result<Vec<launcher::ModEntry>, String>>,
    /// Sources read (nations: at a generation of the store).
    loaded: BTreeMap<Src, (u64, Arc<Index>, Option<serde_json::Value>)>,
    errors: BTreeMap<Src, String>,
    job: Option<Job>,
    src: Option<Src>,
    kind: Option<Kind>,
    filter: String,
    tag: Option<String>,
    picked: BTreeSet<usize>,
    preview: Option<(usize, String)>,
    opts: Options,
    opts_ready: bool,
    to: String,
    plan: Option<Plan>,
    plan_filter: String,
    // export
    bundle_name: String,
    copy_to: String,
}

impl ImportState {
    pub fn busy(&self) -> bool {
        self.job.is_some()
    }

    /// Show the base game as the source (for screenshots and tests), with a kind and filter.
    pub fn show_base(&mut self, kind: Option<Kind>, filter: &str) {
        self.select(Src::Base);
        self.kind = kind;
        self.filter = filter.to_string();
    }

    fn start(&mut self, src: Src, ctx: &egui::Context) {
        if self.job.as_ref().is_some_and(|j| j.src == src) {
            return;
        }
        let progress = Arc::new(AtomicU32::new(0));
        let (tx, rx) = crossbeam_channel::bounded(1);
        let (p, c, s) = (progress.clone(), ctx.clone(), src.clone());
        std::thread::spawn(move || {
            let prog = |f: f32| {
                p.store(f.to_bits(), Ordering::Relaxed);
                c.request_repaint();
            };
            let r: anyhow::Result<(Index, Option<serde_json::Value>)> = (|| match &s {
                Src::Mod { name, path } => {
                    let files = if path.is_file() {
                        let b = std::fs::read(path)?;
                        let m: BTreeMap<String, Arc<Vec<u8>>> = zip::read(&b)?.into_iter().map(|(n, d)| (n, Arc::new(d))).collect();
                        Files::Memory(Arc::new(m))
                    } else {
                        Files::Folder(path.clone())
                    };
                    Ok((source::index(name, Origin::Mod(name.clone()), files, &prog)?, None))
                }
                Src::Folder(p) => Ok((source::index(&s.label(), Origin::Folder, Files::Folder(p.clone()), &prog)?, None)),
                Src::Bundle(p) => source::of_bundle(p).map(|(i, m)| (i, Some(m))),
                Src::Base | Src::Nation(_) => anyhow::bail!("not read here"),
            })();
            let _ = tx.send(r.map_err(|e| format!("{e:#}")));
            c.request_repaint();
        });
        self.job = Some(Job { src, progress, rx });
    }

    fn poll(&mut self) {
        if let Some(j) = &self.job
            && let Ok(r) = j.rx.try_recv()
        {
            let src = j.src.clone();
            self.job = None;
            match r {
                Ok((ix, m)) => {
                    log::info!("read {}: {} definitions, {} texts", ix.label, ix.defs.len(), ix.loc.len());
                    self.loaded.insert(src.clone(), (0, Arc::new(ix), m));
                    self.errors.remove(&src);
                }
                Err(e) => {
                    self.errors.insert(src, e);
                }
            }
        }
    }

    /// The index of a source (None: not read yet; nations and the base game are at hand).
    fn index<'a>(&'a mut self, cx: &'a Cx, src: &Src) -> Option<Arc<Index>> {
        match src {
            Src::Base => None,
            Src::Nation(p) => {
                let fresh = self.loaded.get(src).is_some_and(|x| x.0 == cx.st.generation);
                if !fresh {
                    match source::of_package(cx.st, p) {
                        Ok(ix) => {
                            self.loaded.insert(src.clone(), (cx.st.generation, Arc::new(ix), None));
                        }
                        Err(e) => {
                            self.errors.insert(src.clone(), format!("{e:#}"));
                            return None;
                        }
                    }
                }
                self.loaded.get(src).map(|x| x.1.clone())
            }
            _ => self.loaded.get(src).map(|x| x.1.clone()),
        }
    }

    /// Show a source and pick from it (used by the export page too).
    fn select(&mut self, src: Src) {
        if self.src.as_ref() != Some(&src) {
            self.picked.clear();
            self.preview = None;
            self.plan = None;
            self.tag = None;
            self.kind = None;
        }
        self.src = Some(src);
    }
}

/// The index to use for a source: the base game's is the screen's.
fn index_of<'a>(is: &mut ImportState, cx: &Cx<'a>, src: &Src) -> Option<IndexRef<'a>> {
    match src {
        Src::Base => cx.base.map(IndexRef::Borrowed),
        _ => is.index(cx, src).map(IndexRef::Owned),
    }
}

enum IndexRef<'a> {
    Borrowed(&'a Index),
    Owned(Arc<Index>),
}

impl std::ops::Deref for IndexRef<'_> {
    type Target = Index;
    fn deref(&self) -> &Index {
        match self {
            IndexRef::Borrowed(i) => i,
            IndexRef::Owned(a) => a,
        }
    }
}

// ---------------------------------------------------------------- import

pub fn import_page(ui: &mut egui::Ui, cx: &mut Cx, is: &mut ImportState) {
    is.poll();
    if is.to.is_empty() || (is.to != cx.pkg && is.plan.is_none()) {
        is.to = cx.pkg.to_string();
    }
    if !is.opts_ready {
        is.opts = Options { languages: vec![cx.settings.language.clone()], ..Options::default() };
        if cx.settings.language != "english" {
            is.opts.languages.insert(0, "english".into());
        }
        is.opts_ready = true;
    }
    if is.mods.is_none() {
        is.mods = Some(launcher::scan(cx.user_dir).map_err(|e| format!("{e:#}")));
    }
    egui::Panel::left("import_sources").default_size(230.0).max_size(ui.available_width() * 0.3).show(ui, |ui| super::contained(ui, |ui| sources(ui, cx, is)));
    egui::CentralPanel::default().show(ui, |ui| {
        let Some(src) = is.src.clone() else {
            ui.add_space(20.0);
            ui.heading("Import content");
            ui.label("Pick where it comes from on the left: the base game, a mod, a folder laid out like a mod, a bundle, or another nation.");
            ui.label("Then pick what to bring (a focus tree, events, ideas, all of a tag) and see the plan before anything changes.");
            return;
        };
        if let Some(e) = is.errors.get(&src) {
            ui.label(RichText::new(format!("{}: {e}", src.label())).color(ERR));
            return;
        }
        if let Some(j) = &is.job
            && j.src == src
        {
            let f = f32::from_bits(j.progress.load(Ordering::Relaxed));
            ui.add(egui::ProgressBar::new(f).text(format!("reading {}…", src.label())));
            return;
        }
        let Some(ix) = index_of(is, cx, &src) else {
            if src == Src::Base {
                ui.spinner();
                ui.label("the base game is still being read…");
            } else {
                ui.label("not read yet");
            }
            return;
        };
        if is.plan.is_some() {
            plan_ui(ui, cx, is, &ix);
        } else {
            browse(ui, cx, is, &ix, &src);
        }
    });
}

fn sources(ui: &mut egui::Ui, cx: &mut Cx, is: &mut ImportState) {
    ui.heading("From");
    let mut pick: Option<Src> = None;
    let cur = is.src.clone();
    let row = |ui: &mut egui::Ui, s: Src, label: String, hover: String, pick: &mut Option<Src>| {
        if ui.selectable_label(cur.as_ref() == Some(&s), label).on_hover_text(hover).clicked() {
            *pick = Some(s);
        }
    };
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        row(ui, Src::Base, "🎮 the base game".into(), cx.game.display().to_string(), &mut pick);
        ui.separator();
        ui.label(RichText::new("Nations of the project").weak());
        for (p, pk) in &cx.st.packages {
            if *p == cx.pkg || pk.is_empty() {
                continue;
            }
            let name = cx.facts.nations.iter().find(|n| n.tag == *p).map(|n| n.name.clone()).unwrap_or_else(|| if p == SHARED { "shared".into() } else { "(no nation)".into() });
            row(ui, Src::Nation(p.clone()), format!("{p} {name}"), format!("{} files", pk.files.len()), &mut pick);
        }
        ui.separator();
        ui.label(RichText::new("Mods").weak());
        match &is.mods {
            Some(Ok(mods)) => {
                let mut shown = 0;
                for m in mods {
                    let Some(path) = &m.path else { continue };
                    if !m.present {
                        continue;
                    }
                    shown += 1;
                    row(ui, Src::Mod { name: m.name.clone(), path: path.clone() }, m.name.clone(), path.display().to_string(), &mut pick);
                }
                if shown == 0 {
                    ui.label(RichText::new(format!("no mods in {}", cx.user_dir.join("mod").display())).weak().small());
                }
            }
            Some(Err(e)) => {
                ui.label(RichText::new(format!("no mods in {}", cx.user_dir.join("mod").display())).weak().small()).on_hover_text(e.as_str());
            }
            None => {}
        }
        if ui.small_button("look again").clicked() {
            is.mods = None;
        }
        ui.separator();
        ui.label(RichText::new("Folders and bundles").weak());
        let mut forget = None;
        for p in cx.settings.sources.clone() {
            let s = if p.is_file() { Src::Bundle(p.clone()) } else { Src::Folder(p.clone()) };
            ui.horizontal(|ui| {
                row(ui, s, Src::label(&if p.is_file() { Src::Bundle(p.clone()) } else { Src::Folder(p.clone()) }), p.display().to_string(), &mut pick);
                if ui.small_button("✖").on_hover_text("forget it (nothing is deleted)").clicked() {
                    forget = Some(p.clone());
                }
            });
        }
        if let Some(f) = forget {
            cx.settings.sources.retain(|p| *p != f);
        }
        ui.horizontal(|ui| {
            if ui.button("+ Folder…").on_hover_text("a folder laid out like a mod (common/, events/, ...)").clicked()
                && let Some(p) = rfd::FileDialog::new().pick_folder()
            {
                if !cx.settings.sources.contains(&p) {
                    cx.settings.sources.push(p.clone());
                }
                pick = Some(Src::Folder(p));
            }
            if ui.button("+ Bundle…").on_hover_text("a .zip exported by the mod helper").clicked()
                && let Some(p) = rfd::FileDialog::new().add_filter("bundle", &["zip"]).pick_file()
            {
                if !cx.settings.sources.contains(&p) {
                    cx.settings.sources.push(p.clone());
                }
                pick = Some(Src::Bundle(p));
            }
        });
    });
    if let Some(s) = pick {
        let need_read = matches!(s, Src::Mod { .. } | Src::Folder(_) | Src::Bundle(_)) && !is.loaded.contains_key(&s);
        is.errors.remove(&s);
        is.select(s.clone());
        if need_read {
            is.start(s, ui.ctx());
        }
    }
}

fn def_label(ix: &Index, i: usize) -> String {
    let d = &ix.defs[i];
    match &d.group {
        Some(g) if matches!(d.kind, Kind::Focus | Kind::Idea | Kind::Decision) => format!("{} ({g})", d.id),
        _ => d.id.clone(),
    }
}

fn browse(ui: &mut egui::Ui, cx: &mut Cx, is: &mut ImportState, ix: &Index, src: &Src) {
    ui.horizontal_wrapped(|ui| {
        ui.heading(src.label());
        ui.label(RichText::new(format!("{} definitions, {} texts, {} sprites", ix.defs.len(), ix.loc.len(), ix.sprites.len())).weak());
        if matches!(src, Src::Mod { .. } | Src::Folder(_) | Src::Bundle(_)) && ui.small_button("read again").clicked() {
            is.loaded.remove(src);
            is.start(src.clone(), ui.ctx());
        }
    });
    if !ix.problems.is_empty() {
        ui.collapsing(RichText::new(format!("{} files could not be read", ix.problems.len())).color(WARN), |ui| {
            for p in ix.problems.iter().take(50) {
                ui.label(RichText::new(p).small());
            }
        });
    }
    // kinds with counts
    let mut by_kind: BTreeMap<Kind, usize> = BTreeMap::new();
    for d in &ix.defs {
        *by_kind.entry(d.kind).or_default() += 1;
    }
    ui.horizontal_wrapped(|ui| {
        if ui.selectable_label(is.kind.is_none(), format!("all ({})", ix.defs.len())).clicked() {
            is.kind = None;
        }
        for (k, n) in &by_kind {
            if ui.selectable_label(is.kind == Some(*k), format!("{} ({n})", k.spec().label)).clicked() {
                is.kind = Some(*k);
            }
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.label("🔍");
        ui.add(egui::TextEdit::singleline(&mut is.filter).hint_text("id or name").desired_width(180.0));
        ui.label("tag");
        egui::ComboBox::from_id_salt("import_tag").selected_text(is.tag.clone().unwrap_or_else(|| "any".into())).height(400.0).show_ui(ui, |ui| {
            if ui.selectable_label(is.tag.is_none(), "any").clicked() {
                is.tag = None;
            }
            for t in &ix.tags {
                let name = ix.loc.get(t).cloned().unwrap_or_default();
                if ui.selectable_label(is.tag.as_deref() == Some(t), format!("{t} {name}")).clicked() {
                    is.tag = Some(t.clone());
                    is.opts.from_tag = Some(t.clone());
                }
            }
        });
        if let Some(t) = is.tag.clone()
            && ui.button(format!("Pick all of {t}")).on_hover_text(format!("every definition of the source that is about {t} (its trees, events, ideas, characters, history...)")).clicked()
        {
            for (i, d) in ix.defs.iter().enumerate() {
                if d.tags.contains(&t) || kinds::tag_prefix(&d.id) == Some(t.as_str()) {
                    is.picked.insert(i);
                }
            }
            is.opts.from_tag = Some(t);
        }
    });
    let f = is.filter.to_lowercase();
    let rows: Vec<usize> = (0..ix.defs.len())
        .filter(|&i| {
            let d = &ix.defs[i];
            is.kind.is_none_or(|k| d.kind == k)
                && is.tag.as_ref().is_none_or(|t| d.tags.iter().any(|x| x == t) || kinds::tag_prefix(&d.id) == Some(t.as_str()))
                && (f.is_empty() || d.id.to_lowercase().contains(&f) || ix.loc.get(&d.id).is_some_and(|n| n.to_lowercase().contains(&f)))
        })
        .collect();
    ui.horizontal(|ui| {
        ui.label(format!("{} shown, {} picked", rows.len(), is.picked.len()));
        if ui.small_button("pick the shown").clicked() {
            is.picked.extend(rows.iter().copied());
        }
        if ui.small_button("unpick the shown").clicked() {
            for r in &rows {
                is.picked.remove(r);
            }
        }
        if !is.picked.is_empty() && ui.small_button("unpick all").clicked() {
            is.picked.clear();
        }
    });
    egui::Panel::bottom("import_opts").show(ui, |ui| options(ui, cx, is, ix));
    egui::Panel::right("import_preview").default_size(420.0).max_size(ui.available_width() * 0.5).show(ui, |ui| super::contained(ui, |ui| preview(ui, cx, is, ix)));
    egui::CentralPanel::default().show(ui, |ui| {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, 20.0, rows.len(), |ui, range| {
            for &i in &rows[range] {
                let d = &ix.defs[i];
                ui.horizontal(|ui| {
                    let mut on = is.picked.contains(&i);
                    if ui.checkbox(&mut on, "").changed() {
                        if on {
                            is.picked.insert(i);
                        } else {
                            is.picked.remove(&i);
                        }
                    }
                    let name = ix.loc.get(&d.id).map(|n| crate::content::loc::plain(n)).filter(|n| !n.is_empty());
                    let sel = is.preview.as_ref().is_some_and(|p| p.0 == i);
                    let label = match &name {
                        Some(n) => format!("{}  {n}", def_label(ix, i)),
                        None => def_label(ix, i),
                    };
                    if ui.selectable_label(sel, RichText::new(label).monospace()).on_hover_text(format!("{} in {}", d.kind.spec().one, ix.path(d))).clicked() {
                        is.preview = ix.text_of(d).map(|t| (i, t));
                    }
                    if is.kind.is_none() {
                        ui.label(RichText::new(d.kind.spec().one).small().weak());
                    }
                });
            }
        });
    });
}

fn preview(ui: &mut egui::Ui, cx: &mut Cx, is: &mut ImportState, ix: &Index) {
    let Some((i, text)) = &is.preview else {
        ui.label(RichText::new("Click a definition to see it.").weak());
        return;
    };
    let d = &ix.defs[*i];
    ui.label(RichText::new(&d.id).strong().monospace());
    ui.label(RichText::new(format!("{} · {}", d.kind.spec().one, ix.path(d))).small().weak());
    if let Some(n) = ix.loc.get(&d.id) {
        ui.label(crate::content::loc::plain(n));
    }
    // its pictures
    let s = Script::parse(text);
    if let Some(b) = s.root.items.first().and_then(|it| it.block()) {
        let sprites = kinds::sprites_of(d.kind, b);
        if !sprites.is_empty() {
            ui.horizontal_wrapped(|ui| {
                for (_, name) in sprites.iter().take(6) {
                    let srcs: Vec<&Index> = std::iter::once(ix).chain(cx.base).collect();
                    let tex = cx.gfx.texture(ui.ctx(), name, cx.st, &srcs);
                    crate::content::gfx::show(ui, tex, egui::vec2(64.0, 64.0)).on_hover_text(name);
                }
            });
        }
    }
    if !d.tags.is_empty() {
        ui.label(RichText::new(format!("about {}", d.tags.join(", "))).small());
    }
    code::view(ui, egui::Id::unique(("import_preview", *i)), text, Lang::Script, 40);
}

fn options(ui: &mut egui::Ui, cx: &mut Cx, is: &mut ImportState, ix: &Index) {
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        ui.label("into");
        super::nation_combo(ui, "import_to", cx.facts, &mut is.to, true);
        ui.separator();
        ui.label("the source's tag");
        let mut ft = is.opts.from_tag.clone().unwrap_or_default();
        egui::ComboBox::from_id_salt("import_from_tag").selected_text(if ft.is_empty() { "(none)".into() } else { ft.clone() }).height(400.0).show_ui(ui, |ui| {
            ui.selectable_value(&mut ft, String::new(), "(none)");
            for t in &ix.tags {
                ui.selectable_value(&mut ft, t.clone(), t);
            }
        });
        is.opts.from_tag = (!ft.is_empty()).then_some(ft);
        if let Some(t) = &is.opts.from_tag {
            ui.label(RichText::new(format!("becomes {}", if is.to == SHARED { "MOD".to_string() } else { is.to.clone() })).weak());
            let _ = t;
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.checkbox(&mut is.opts.follow, "bring what it uses").on_hover_text("events it fires, ideas it adds, characters it recruits, scripted effects and triggers, shared focuses...");
        ui.checkbox(&mut is.opts.prefix_clashes, "rename clashing ids").on_hover_text("ids the project has already get the nation's tag in front (always for the base game's ids, which would be defined twice)");
        ui.checkbox(&mut is.opts.strip_map, "leave out the source's map").on_hover_text("country history: capital, divisions, state changes are about the source's map, not this one");
        ui.checkbox(&mut is.opts.all_files, "all files of the source").on_hover_text("every file, not only definitions (copying a nation or a bundle whole)");
        ui.label("texts in");
        for lang in crate::content::loc::LANGUAGES {
            let mut on = is.opts.languages.iter().any(|l| l == lang);
            if ui.toggle_value(&mut on, lang).changed() {
                if on {
                    is.opts.languages.push(lang.to_string());
                } else {
                    is.opts.languages.retain(|l| l != lang);
                }
            }
        }
    });
    ui.horizontal(|ui| {
        let can = !is.picked.is_empty() || is.opts.all_files;
        if ui.add_enabled(can, egui::Button::new(RichText::new(format!("Plan the import of {} …", is.picked.len())).strong())).clicked() {
            let picked: Vec<usize> = is.picked.iter().copied().collect();
            let base = if ix.origin == Origin::Base { None } else { cx.base };
            is.plan = Some(import::plan(ix, base, cx.st, &picked, &is.to, &is.opts));
            is.plan_filter.clear();
        }
        if !can {
            ui.label(RichText::new("pick something first").weak());
        }
    });
    ui.add_space(4.0);
}

fn plan_ui(ui: &mut egui::Ui, cx: &mut Cx, is: &mut ImportState, ix: &Index) {
    let Some(p) = &mut is.plan else { return };
    let inc = p.items.iter().filter(|i| i.include).count();
    let replaces = p.items.iter().filter(|i| i.include && i.replaces).count();
    let renamed = p.items.iter().filter(|i| i.include && i.old_id != i.new_id).count();
    ui.horizontal_wrapped(|ui| {
        ui.heading(format!("Import into {}", if p.to == SHARED { "shared" } else { &p.to }));
        ui.label(format!("{inc} definitions, {} texts, {} sprites, {} pictures and other files", p.loc.len(), p.sprites.len(), p.files.len()));
    });
    if let Some(t) = &p.from_tag {
        ui.label(format!("{t} becomes {} in ids, files and texts", p.to));
    }
    if renamed > 0 {
        ui.label(RichText::new(format!("{renamed} ids are renamed so they do not clash")).color(WARN));
    }
    if replaces > 0 {
        ui.label(RichText::new(format!("{replaces} definitions the nation has already are replaced")).color(WARN));
    }
    for n in &p.notes {
        ui.label(RichText::new(format!("• {n}")).small());
    }
    let mut apply = false;
    let mut back = false;
    ui.horizontal(|ui| {
        if ui.add_enabled(inc > 0 || !p.files.is_empty(), egui::Button::new(RichText::new("📥 Import").strong())).clicked() {
            apply = true;
        }
        if ui.button("Back").clicked() {
            back = true;
        }
        ui.separator();
        if ui.small_button("include all").clicked() {
            p.items.iter_mut().for_each(|i| i.include = true);
        }
        if ui.small_button("only the picked").clicked() {
            p.items.iter_mut().for_each(|i| i.include = i.why == "picked");
        }
        ui.add(egui::TextEdit::singleline(&mut is.plan_filter).hint_text("filter").desired_width(140.0));
    });
    ui.separator();
    let f = is.plan_filter.to_lowercase();
    let rows: Vec<usize> = (0..p.items.len()).filter(|&i| f.is_empty() || p.items[i].old_id.to_lowercase().contains(&f) || p.items[i].new_id.to_lowercase().contains(&f)).collect();
    egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, 20.0, rows.len(), |ui, range| {
        for &i in &rows[range] {
            let it = &mut p.items[i];
            ui.horizontal(|ui| {
                ui.checkbox(&mut it.include, "");
                ui.label(RichText::new(it.kind.spec().one).small().weak());
                if it.old_id == it.new_id {
                    ui.label(RichText::new(&it.new_id).monospace());
                } else {
                    ui.label(RichText::new(format!("{} → {}", it.old_id, it.new_id)).monospace().color(WARN));
                }
                if it.replaces {
                    ui.label(RichText::new("replaces").small().color(WARN));
                }
                ui.label(RichText::new(if it.why == "picked" { "picked".to_string() } else { format!("used by {}", it.why) }).small().weak());
            });
        }
    });
    if back {
        is.plan = None;
        return;
    }
    if apply {
        let label = format!("import from {}", ix.label);
        let to = p.to.clone();
        match import::apply(p, ix, cx.st, &label) {
            Ok(m) => {
                cx.info(m);
                is.plan = None;
                is.picked.clear();
                if to != cx.pkg {
                    cx.go(Nav::Pkg(to));
                }
                cx.go(Nav::Page(Page::Overview));
            }
            Err(e) => cx.error(format!("the import failed (nothing changed): {e:#}")),
        }
    }
}

/// Copy definitions of the project to another nation (renaming the tag;
/// what they use comes along when it is in the same nation).
pub fn copy_defs(cx: &mut Cx, keys: &[DefKey], to: &str) -> bool {
    let Some(from) = keys.first().map(|k| k.pkg.clone()) else { return false };
    if from == to {
        cx.error("that is the nation it is in");
        return false;
    }
    let ix = match source::of_package(cx.st, &from) {
        Ok(i) => i,
        Err(e) => {
            cx.error(format!("{e:#}"));
            return false;
        }
    };
    let picked: Vec<usize> = keys.iter().filter_map(|k| ix.defs.iter().position(|d| d.kind == k.kind && d.id == k.id)).collect();
    if picked.is_empty() {
        cx.error("nothing to copy");
        return false;
    }
    let opts = Options { from_tag: (from != SHARED).then(|| from.clone()), follow: true, prefix_clashes: true, languages: crate::content::loc::LANGUAGES.iter().map(|s| s.to_string()).collect(), strip_map: true, all_files: false };
    let plan = import::plan(&ix, cx.base, cx.st, &picked, to, &opts);
    let n = plan.items.iter().filter(|i| i.include).count();
    match import::apply(&plan, &ix, cx.st, &format!("copy {} to {to}", keys.iter().map(|k| k.id.as_str()).collect::<Vec<_>>().join(", "))) {
        Ok(_) => {
            cx.info(format!("copied {n} definitions to {to} (Ctrl+Z takes it back)"));
            true
        }
        Err(e) => {
            cx.error(format!("copying failed: {e:#}"));
            false
        }
    }
}

// ---------------------------------------------------------------- export

pub fn export_page(ui: &mut egui::Ui, cx: &mut Cx, is: &mut ImportState) {
    let pkg = cx.pkg.to_string();
    let files = cx.st.files_of(&pkg);
    let name = cx.nation.map(|n| n.name.clone()).unwrap_or_else(|| pkg.clone());
    if is.bundle_name.is_empty() {
        is.bundle_name = name.clone();
    }
    ui.heading(format!("Export {}", if pkg == SHARED { "the shared content" } else { &name }));
    if files.is_empty() {
        ui.label("It has no content yet.");
    } else {
        ui.label(format!("{} files: {} definitions", files.len(), cx.st.defs().filter(|k| k.pkg == pkg).count()));
    }
    ui.add_space(8.0);
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.label(RichText::new("As a bundle").strong());
        ui.label("A zip with the files and what they are; any project (or another nation of this one) can import it.");
        ui.horizontal(|ui| {
            ui.label("name");
            ui.add(egui::TextEdit::singleline(&mut is.bundle_name).desired_width(200.0));
            if ui.add_enabled(!files.is_empty(), egui::Button::new("Save the bundle…")).clicked()
                && let Some(p) = rfd::FileDialog::new().set_file_name(format!("{}_{}.zip", pkg, slug(&is.bundle_name))).add_filter("bundle", &["zip"]).save_file()
            {
                match import::export_bundle(cx.st, &pkg, &is.bundle_name).and_then(|b| std::fs::write(&p, b).map_err(Into::into)) {
                    Ok(()) => cx.info(format!("saved {}", p.display())),
                    Err(e) => cx.error(format!("cannot save {}: {e:#}", p.display())),
                }
            }
        });
    });
    ui.add_space(8.0);
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.label(RichText::new("As plain files").strong());
        ui.label("The files as they go into the mod (common/, events/, localisation/ ...), into a folder you pick.");
        if ui.add_enabled(!files.is_empty(), egui::Button::new("Write the files…")).clicked()
            && let Some(dir) = rfd::FileDialog::new().pick_folder()
        {
            let mut n = 0;
            let mut skipped = Vec::new();
            for (rel, bytes) in cx.st.package_bytes(&pkg) {
                let dst = dir.join(&rel);
                if dst.exists() {
                    skipped.push(rel);
                    continue;
                }
                let r = dst.parent().map(std::fs::create_dir_all).unwrap_or(Ok(())).and_then(|_| std::fs::write(&dst, bytes.as_slice()));
                match r {
                    Ok(()) => n += 1,
                    Err(e) => {
                        cx.error(format!("cannot write {}: {e}", dst.display()));
                        return;
                    }
                }
            }
            if skipped.is_empty() {
                cx.info(format!("wrote {n} files into {}", dir.display()));
            } else {
                cx.error(format!("wrote {n} files into {}; {} were there already and are left as they were (first: {})", dir.display(), skipped.len(), skipped[0]));
            }
        }
    });
    ui.add_space(8.0);
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.label(RichText::new("Into another nation").strong());
        ui.label(format!("Copies all of it into another nation of the map: {pkg} becomes its tag in ids, files and texts. A plan shows what comes first."));
        ui.horizontal(|ui| {
            ui.label("to");
            if is.copy_to.is_empty() || is.copy_to == pkg {
                is.copy_to = cx.facts.nations.iter().map(|n| n.tag.clone()).find(|t| *t != pkg).unwrap_or_default();
            }
            super::nation_combo(ui, "copy_nation_to", cx.facts, &mut is.copy_to, false);
            if ui.add_enabled(!files.is_empty() && !is.copy_to.is_empty() && is.copy_to != pkg, egui::Button::new("Plan the copy…")).clicked() {
                let src = Src::Nation(pkg.clone());
                is.select(src.clone());
                if let Some(ix) = is.index(cx, &src) {
                    let opts = Options { from_tag: (pkg != SHARED).then(|| pkg.clone()), follow: true, prefix_clashes: true, languages: crate::content::loc::LANGUAGES.iter().map(|s| s.to_string()).collect(), strip_map: true, all_files: true };
                    let picked: Vec<usize> = (0..ix.defs.len()).collect();
                    is.to = is.copy_to.clone();
                    is.picked = picked.iter().copied().collect();
                    is.plan = Some(import::plan(&ix, cx.base, cx.st, &picked, &is.copy_to, &opts));
                    is.opts = opts;
                    cx.go(Nav::Page(Page::Import));
                }
            }
        });
    });
    ui.add_space(8.0);
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.label(RichText::new("Bring a bundle in").strong());
        ui.label("Import a bundle exported before into this nation (or any other).");
        if ui.button("Open a bundle…").clicked()
            && let Some(p) = rfd::FileDialog::new().add_filter("bundle", &["zip"]).pick_file()
        {
            match source::of_bundle(&p) {
                Ok((ix, manifest)) => {
                    let src = Src::Bundle(p.clone());
                    if !cx.settings.sources.contains(&p) {
                        cx.settings.sources.push(p.clone());
                    }
                    let tag = manifest.get("tag").and_then(|t| t.as_str()).map(str::to_string);
                    let ix = Arc::new(ix);
                    is.loaded.insert(src.clone(), (0, ix.clone(), Some(manifest)));
                    is.select(src);
                    is.to = pkg.clone();
                    let opts = Options { from_tag: tag.filter(|t| *t != SHARED && *t != pkg), all_files: true, ..is.opts.clone() };
                    let picked: Vec<usize> = (0..ix.defs.len()).collect();
                    is.picked = picked.iter().copied().collect();
                    is.plan = Some(import::plan(&ix, cx.base, cx.st, &picked, &pkg, &opts));
                    is.opts = opts;
                    cx.go(Nav::Page(Page::Import));
                }
                Err(e) => cx.error(format!("{e:#}")),
            }
        }
    });
    if !files.is_empty() {
        ui.add_space(8.0);
        ui.collapsing(format!("The {} files", files.len()), |ui| {
            for f in &files {
                ui.label(RichText::new(f).monospace().small());
            }
        });
    }
    let _ = OK;
}

fn slug(s: &str) -> String {
    let s: String = s.chars().map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).collect();
    s.trim_matches('_').to_string()
}
