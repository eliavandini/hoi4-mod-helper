//! The Objects level: 3D objects on the map, the game's ambient objects
//! (map/ambient_object.txt). A browser of the entities and meshes of the
//! game, its DLCs and the mod; a 3D preview that plays their animations;
//! placing, moving and turning objects on the map, drawn there as seen from
//! above at their size in the game.

use std::collections::BTreeMap;
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::Arc;

use crossbeam_channel::{Receiver, Sender, unbounded};
use eframe::egui::{self, Color32, Key, Modifiers, PointerButton, Pos2, Rect, RichText, Sense, Stroke, Vec2, pos2, vec2};
use rustc_hash::FxHashMap;

use super::model::Ambient;
use super::{ERR, Editor, Tool, WARN};
use crate::layers::Placement;
use crate::models::catalog::{Catalog, Origin, Pick};
use crate::models::{self, Loader, Play, View};
use crate::view::DrawCtx;

/// How close (screen points) a click must be to an object's centre when it
/// is drawn too small to click on.
const PICK: f32 = 9.0;
const SELECTED: Color32 = Color32::from_rgb(255, 225, 60);
const MARKER: Color32 = Color32::from_rgb(255, 150, 60);
/// Pixels of the icons drawn on the map (seen from above).
const ICON_PX: usize = 128;
const THUMB_PX: usize = 80;
/// Pictures made in the background at once.
const JOBS: usize = 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Whose {
    #[default]
    All,
    Game,
    Dlc,
    Mod,
}

impl Whose {
    const ALL: [Whose; 4] = [Whose::All, Whose::Game, Whose::Dlc, Whose::Mod];
    fn name(self) -> &'static str {
        match self {
            Whose::All => "all",
            Whose::Game => "the game",
            Whose::Dlc => "DLCs",
            Whose::Mod => "the mod",
        }
    }
    fn fits(self, o: &Origin) -> bool {
        match self {
            Whose::All => true,
            Whose::Game => matches!(o, Origin::Game | Origin::Integrated(_)),
            Whose::Dlc => matches!(o, Origin::Dlc(_)),
            Whose::Mod => o.is_mod(),
        }
    }
}

enum Icon {
    Pending,
    /// The picture, and for map icons the half width it spans (map pixels
    /// at scale 1).
    Ready(egui::TextureHandle, f32),
    Missing,
}

enum Done {
    Icon(Pick, Option<(egui::ColorImage, f32)>),
    Thumb(Pick, Option<egui::ColorImage>),
}

/// The 3D preview: how it looks at the model, what plays.
pub struct Preview {
    pub view: View,
    pub play: Play,
    pub time: f32,
    pub playing: bool,
    pub speed: f32,
    /// Shown in a window of its own (bigger).
    pub window: bool,
    /// Show the map around the selection instead of the model.
    pub scene: bool,
    pub scene_view: View,
    /// Map pixels shown around the selection.
    pub radius: f32,
    tex: FxHashMap<&'static str, egui::TextureHandle>,
}

fn scene_view() -> View {
    View { yaw: 20.0, pitch: 38.0, zoom: 1.0, ground: false }
}

impl Default for Preview {
    fn default() -> Self {
        let at_start = std::env::var("MOD_HELPER_PREVIEW").unwrap_or_default();
        Preview {
            view: View::default(),
            play: Play::Still,
            time: 0.4,
            playing: true,
            speed: 1.0,
            window: at_start.contains("window"),
            scene: at_start.contains("scene"),
            scene_view: scene_view(),
            radius: 30.0,
            tex: Default::default(),
        }
    }
}

enum Drag {
    /// Moving the selection: where the drag started (map pixel) and where
    /// the objects were.
    Move { start: [f64; 2], orig: Vec<(usize, [f32; 2])> },
    /// Turning one object by its knob.
    Turn(usize),
    /// A box to select in (screen).
    Box(Pos2),
}

type ListKey = (usize, String, Whose, Option<String>, bool);

pub struct ObjectsUi {
    pub(super) loader: Option<Arc<Loader>>,
    loading: Option<(PathBuf, PathBuf, Receiver<Arc<Loader>>)>,
    loaded_for: Option<(PathBuf, PathBuf)>,
    /// Selected objects (indexes into the map's objects).
    pub sel: Vec<usize>,
    /// The model of the browser: what the Place tool puts down and the
    /// preview shows.
    pub pick: Option<Pick>,
    pub search: String,
    pub folder: Option<String>,
    pub whose: Whose,
    pub animated_only: bool,
    pub pv: Preview,
    pub new_scale: f32,
    pub new_turn: f32,
    pub random_turn: bool,
    pub new_visible: bool,
    drag: Option<Drag>,
    /// A widget of the properties is being dragged: its edit is open.
    live: bool,
    /// The name being typed: (object, text).
    name_edit: Option<(usize, String)>,
    list_filter: String,
    icons: FxHashMap<Pick, Icon>,
    thumbs: FxHashMap<Pick, Icon>,
    tx: Sender<Done>,
    rx: Receiver<Done>,
    in_flight: usize,
    /// Rows of the browser that pass the filters, and the folders of those
    /// that pass all but the folder.
    list: Option<(ListKey, Arc<Vec<usize>>, Arc<BTreeMap<String, usize>>)>,
    /// Scroll the browser to the pick once.
    scroll_to_pick: bool,
    /// The last place clicked on the map (the "on the map" preview shows
    /// it when nothing is selected).
    focus: Option<[f32; 2]>,
    /// Select this object once the map is open ($MOD_HELPER_OBJECT, for
    /// screenshots; with $MOD_HELPER_PREVIEW=scene or window).
    start: Option<usize>,
    seed: u64,
    /// The "Import a model…" window.
    pub(super) imp: super::model_import_ui::ImportUi,
    /// Pick this model once the models are read again.
    after_load: Option<Pick>,
}

impl Default for ObjectsUi {
    fn default() -> Self {
        let (tx, rx) = unbounded();
        ObjectsUi {
            loader: None,
            loading: None,
            loaded_for: None,
            sel: Vec::new(),
            pick: None,
            search: String::new(),
            folder: None,
            whose: Whose::All,
            animated_only: false,
            pv: Preview::default(),
            new_scale: 1.0,
            new_turn: 0.0,
            random_turn: false,
            new_visible: false,
            drag: None,
            live: false,
            name_edit: None,
            list_filter: String::new(),
            icons: Default::default(),
            thumbs: Default::default(),
            tx,
            rx,
            in_flight: 0,
            list: None,
            scroll_to_pick: false,
            focus: None,
            start: std::env::var("MOD_HELPER_OBJECT").ok().and_then(|v| v.parse().ok()),
            seed: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1) | 1,
            imp: super::model_import_ui::ImportUi::from_env(),
            after_load: None,
        }
    }
}

fn color_image(f: &models::render::Frame) -> egui::ColorImage {
    egui::ColorImage::from_rgba_unmultiplied([f.w, f.h], &f.rgba())
}

/// What a model plays when picked: its default state when that has an
/// animation, else nothing.
fn default_play(cat: &Catalog, p: &Pick) -> Play {
    match p {
        Pick::Entity(e) => {
            let Some(def) = cat.entity(e) else { return Play::Still };
            match def.default_state.as_deref().and_then(|d| def.state(d)) {
                Some(s) if s.animation.is_some() => Play::State(s.name.clone()),
                _ => Play::Still,
            }
        }
        Pick::Mesh(_) => Play::Still,
    }
}

/// The play of an object, and back.
fn play_of(o: &Ambient) -> Play {
    match &o.animation {
        None => Play::Still,
        Some(a) if o.mesh => Play::Anim(a.clone()),
        Some(a) => Play::State(a.clone()),
    }
}

fn animation_of(p: &Play, mesh: bool) -> Option<String> {
    match p {
        Play::State(s) if !mesh => Some(s.clone()),
        Play::Anim(a) if mesh => Some(a.clone()),
        _ => None,
    }
}

/// A warning when a skinned model is set to stand still: the build makes it
/// play (see `models::is_skinned`).
fn still_note(ui: &mut egui::Ui, loader: &Loader, m: &models::Model, pick: &Pick, play: &Play) {
    if *play != Play::Still || !m.skinned() {
        return;
    }
    let text = match models::still_animation(&loader.cat, pick) {
        Some(a) => format!("Its bones move its vertices, and the game draws such models only animated: the build makes it play {a}."),
        None => "Its bones move its vertices, but it has nothing to play: the game may draw it as stray triangles.".into(),
    };
    ui.label(RichText::new(text).small().color(WARN));
}

pub fn pick_of(o: &Ambient) -> Pick {
    if o.mesh { Pick::Mesh(o.entity.clone()) } else { Pick::Entity(o.entity.clone()) }
}

fn play_label(p: &Play) -> String {
    match p {
        Play::Still => "standing still".into(),
        Play::State(s) => format!("state {s}"),
        Play::Anim(a) => format!("animation {a}"),
    }
}

/// An object on the screen: centre, its east and north on the screen
/// (turned with the object, unit long) and half its size in points.
struct OnScreen {
    c: Pos2,
    east: Vec2,
    north: Vec2,
    half: f32,
}

impl OnScreen {
    fn corners(&self) -> [Pos2; 4] {
        let (e, n) = (self.east * self.half, self.north * self.half);
        [self.c + n - e, self.c + n + e, self.c - n + e, self.c - n - e]
    }
    fn hit(&self, p: Pos2) -> Option<f32> {
        let v = p - self.c;
        let inside = v.dot(self.east).abs() <= self.half && v.dot(self.north).abs() <= self.half;
        let d = v.length();
        (inside || d <= PICK).then_some(d)
    }
    /// The knob that turns it.
    fn knob(&self) -> Pos2 {
        self.c + self.north * (self.half.max(8.0) + 16.0)
    }
}

impl ObjectsUi {
    fn rand(&mut self) -> u64 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 7;
        self.seed ^= self.seed << 17;
        self.seed
    }

    /// Ask for a picture in the background (icon from above, or a thumbnail).
    fn request(&mut self, ctx: &egui::Context, pick: &Pick, icon: bool) {
        let have = if icon { self.icons.contains_key(pick) } else { self.thumbs.contains_key(pick) };
        if have || self.in_flight >= JOBS {
            return;
        }
        let Some(loader) = self.loader.clone() else { return };
        if icon { &mut self.icons } else { &mut self.thumbs }.insert(pick.clone(), Icon::Pending);
        self.in_flight += 1;
        let (tx, ctx, pick) = (self.tx.clone(), ctx.clone(), pick.clone());
        rayon::spawn(move || {
            let p2 = pick.clone();
            let made = std::panic::catch_unwind(AssertUnwindSafe(move || {
                let m = loader.model(&p2);
                m.mesh.as_ref()?;
                Some(if icon {
                    let (f, half) = models::top_icon(&m, ICON_PX);
                    (color_image(&f), half)
                } else {
                    let v = View { ground: false, ..View::default() };
                    (color_image(&models::picture(&m, THUMB_PX, THUMB_PX, v, &Play::Still, 0.0, [0, 0, 0, 0])), 0.0)
                })
            }))
            .ok()
            .flatten();
            let done = if icon { Done::Icon(pick, made) } else { Done::Thumb(pick, made.map(|m| m.0)) };
            let _ = tx.send(done);
            ctx.request_repaint();
        });
    }

    fn receive(&mut self, ctx: &egui::Context) {
        while let Ok(d) = self.rx.try_recv() {
            self.in_flight = self.in_flight.saturating_sub(1);
            let (map, pick, img) = match d {
                Done::Icon(p, r) => (&mut self.icons, p, r),
                Done::Thumb(p, r) => (&mut self.thumbs, p, r.map(|i| (i, 0.0))),
            };
            let v = match img {
                Some((img, half)) => Icon::Ready(ctx.load_texture(format!("object {}", pick.name()), img, egui::TextureOptions::LINEAR), half),
                None => Icon::Missing,
            };
            map.insert(pick, v);
        }
    }

    /// Read the models again (files were added to the mod), then pick `pick`.
    pub(super) fn reload(&mut self, pick: Option<Pick>) {
        self.loading = None;
        self.loaded_for = None;
        self.after_load = pick;
    }

    fn set_pick(&mut self, pick: Pick, play: Option<Play>) {
        let play = play.unwrap_or_else(|| self.loader.as_ref().map(|l| default_play(&l.cat, &pick)).unwrap_or_default());
        if self.pick.as_ref() != Some(&pick) || self.pv.play != play {
            self.pv.time = 0.0;
        }
        self.pick = Some(pick);
        self.pv.play = play;
    }
}

impl Editor {
    /// The models of the game and the mod: read in the background the first
    /// time (and again when the game folder or the map changes).
    fn objects_loader(&mut self, ctx: &egui::Context) -> Option<Arc<Loader>> {
        let doc = self.doc.as_ref()?;
        let content = doc.dir.join("content");
        let game = self.dirs().game;
        let ob = &mut self.ob;
        ob.receive(ctx);
        if let Some((g, c, rx)) = &ob.loading {
            if let Ok(l) = rx.try_recv() {
                ob.loaded_for = Some((g.clone(), c.clone()));
                ob.loading = None;
                ob.icons.clear();
                ob.thumbs.clear();
                ob.list = None;
                ob.loader = Some(l);
                if let Some(p) = ob.after_load.take() {
                    // shown in the list whatever the filters were
                    ob.search.clear();
                    ob.folder = None;
                    ob.whose = Whose::All;
                    ob.animated_only = false;
                    ob.set_pick(p, None);
                    ob.scroll_to_pick = true;
                }
            }
        } else if ob.loaded_for.as_ref() != Some(&(game.clone(), content.clone())) {
            let (tx, rx) = unbounded();
            let (g, c, ctx2) = (game.clone(), content.clone(), ctx.clone());
            std::thread::spawn(move || {
                let cat = Catalog::load(Catalog::roots(&g, Some(&c)));
                let _ = tx.send(Arc::new(Loader::new(Arc::new(cat))));
                ctx2.request_repaint();
            });
            ob.loading = Some((game, content, rx));
        }
        ob.loader.clone()
    }

    /// Drop selected objects that are gone (after an undo).
    fn prune_objects(&mut self) {
        let n = self.doc.as_ref().map_or(0, |d| d.data.ambient.len());
        self.ob.sel.retain(|&i| i < n);
        self.ob.sel.dedup();
        if self.ob.name_edit.as_ref().is_some_and(|e| e.0 >= n) {
            self.ob.name_edit = None;
        }
    }

    fn on_screen(&self, dc: &DrawCtx, pl: &Placement, o: &Ambient, px_scale: f64) -> Option<OnScreen> {
        let (x, y) = (o.at[0] as f64, o.at[1] as f64);
        let c = dc.cam.to_screen(dc.rect, pl.pixel_to_world(x, y)?);
        let e = dc.cam.to_screen(dc.rect, pl.pixel_to_world(x + 1.0, y)?) - c;
        let n = dc.cam.to_screen(dc.rect, pl.pixel_to_world(x, y - 1.0)?) - c;
        let (e, n) = (e.normalized(), n.normalized());
        let (s, k) = o.rotation[1].to_radians().sin_cos();
        let half = match self.ob.icons.get(&pick_of(o)) {
            Some(Icon::Ready(_, h)) => *h,
            _ => 0.0,
        };
        Some(OnScreen { c, north: n * k + e * s, east: e * k - n * s, half: half * o.scale * px_scale as f32 })
    }

    /// The object under a screen point (the nearest of those it is on).
    fn object_at(&self, dc: &DrawCtx, pl: &Placement, at: Pos2, px_scale: f64) -> Option<usize> {
        let doc = self.doc.as_ref()?;
        doc.data
            .ambient
            .iter()
            .enumerate()
            .filter_map(|(i, o)| self.on_screen(dc, pl, o, px_scale).and_then(|s| s.hit(at)).map(|d| (i, d)))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|h| h.0)
    }

    fn place_object(&mut self, px: [f64; 2]) {
        let Some(doc) = &self.doc else { return };
        if px[0] < 0.0 || px[1] < 0.0 || px[0] >= doc.w as f64 || px[1] >= doc.h as f64 {
            self.error("objects go on the map");
            return;
        }
        let Some(pick) = self.ob.pick.clone() else {
            self.error("pick a model in the browser first");
            return;
        };
        let (entity, mesh) = match &pick {
            Pick::Entity(e) => (e.clone(), false),
            Pick::Mesh(m) => (m.clone(), true),
        };
        let turn = if self.ob.random_turn { (self.ob.rand() % 360) as f32 } else { self.ob.new_turn };
        let o = Ambient {
            entity: entity.clone(),
            mesh,
            at: [px[0] as f32, px[1] as f32],
            lift: 0.0,
            rotation: [0.0, turn, 0.0],
            scale: self.ob.new_scale,
            animation: animation_of(&self.ob.pv.play, mesh),
            always_visible: self.ob.new_visible,
            name: String::new(),
        };
        let mut idx = None;
        self.run("Place object", |d, _| {
            d.data.ambient.push(o);
            idx = Some(d.data.ambient.len() - 1);
            Ok(format!("placed {entity} at {:.0}, {:.0}", px[0], px[1]))
        });
        if let Some(i) = idx {
            self.ob.sel = vec![i];
        }
    }

    fn select_object(&mut self, i: usize, add: bool) {
        self.flush_object_name();
        if add {
            if let Some(k) = self.ob.sel.iter().position(|&s| s == i) {
                self.ob.sel.remove(k);
            } else {
                self.ob.sel.push(i);
            }
        } else {
            self.ob.sel = vec![i];
        }
        // the browser and the preview show its model
        if let Some(o) = self.doc.as_ref().and_then(|d| d.data.ambient.get(i)).cloned() {
            self.ob.set_pick(pick_of(&o), Some(play_of(&o)));
            self.ob.scroll_to_pick = true;
        }
    }

    fn delete_objects(&mut self) {
        let mut sel = self.ob.sel.clone();
        if sel.is_empty() {
            return;
        }
        self.ob.name_edit = None;
        sel.sort_unstable();
        self.run("Delete objects", |d, _| {
            for &i in sel.iter().rev() {
                if i < d.data.ambient.len() {
                    d.data.ambient.remove(i);
                }
            }
            Ok(format!("deleted {} object(s)", sel.len()))
        });
        self.ob.sel.clear();
    }

    fn duplicate_objects(&mut self) {
        let sel = self.ob.sel.clone();
        if sel.is_empty() {
            return;
        }
        let mut new = Vec::new();
        self.run("Duplicate objects", |d, _| {
            for &i in &sel {
                if let Some(o) = d.data.ambient.get(i).cloned() {
                    let mut c = o;
                    c.at = [c.at[0] + 2.0, c.at[1] + 2.0];
                    d.data.ambient.push(c);
                    new.push(d.data.ambient.len() - 1);
                }
            }
            Ok(format!("{} object(s) duplicated", new.len()))
        });
        self.ob.sel = new;
    }

    /// Change the selected objects: while a widget is dragged the change is
    /// live and goes in as one undo step when it is let go; otherwise at once.
    fn edit_objects(&mut self, resp: &egui::Response, label: &str, f: impl Fn(&mut Ambient)) {
        let sel = self.ob.sel.clone();
        let dragging = resp.dragged() || resp.drag_started();
        if resp.changed() {
            if dragging {
                if let Some(doc) = self.doc.as_mut() {
                    if !self.ob.live {
                        doc.begin(label);
                        self.ob.live = true;
                    }
                    for &i in &sel {
                        if let Some(o) = doc.data.ambient.get_mut(i) {
                            f(o);
                        }
                    }
                    doc.meta_version += 1;
                }
            } else {
                self.run(label, |d, _| {
                    for &i in &sel {
                        if let Some(o) = d.data.ambient.get_mut(i) {
                            f(o);
                        }
                    }
                    Ok(String::new())
                });
            }
        }
        if self.ob.live && (resp.drag_stopped() || !resp.dragged()) && !resp.ctx.input(|i| i.pointer.primary_down()) {
            self.finish_live();
        }
    }

    fn finish_live(&mut self) {
        if !self.ob.live {
            return;
        }
        self.ob.live = false;
        if let Some(doc) = self.doc.as_mut() {
            doc.commit();
        }
        self.after_edit();
    }

    /// Put the typed name into the map.
    pub(super) fn flush_object_name(&mut self) {
        let Some((i, name)) = self.ob.name_edit.take() else { return };
        let Some(old) = self.doc.as_ref().and_then(|d| d.data.ambient.get(i)).map(|o| o.name.clone()) else { return };
        if old != name {
            self.run("Rename object", |d, _| {
                d.data.ambient[i].name = name.trim().to_string();
                Ok(String::new())
            });
        }
    }

    pub(super) fn object_edits_pending(&self) -> bool {
        self.ob.live || self.ob.name_edit.as_ref().is_some_and(|(i, n)| self.doc.as_ref().and_then(|d| d.data.ambient.get(*i)).is_some_and(|o| &o.name != n))
    }

    // ------------------------------------------------------------ map

    /// Map input on the Objects level. Select: click an object to select it
    /// (Shift: add or remove), drag it to move the selection, drag its knob
    /// to turn it, drag elsewhere to select in a box. Place: click to put
    /// down the browser's model (Alt+click picks the model of an object).
    pub(super) fn objects_input(&mut self, dc: &DrawCtx, response: &egui::Response, pl: &Placement, px_scale: f64) -> bool {
        self.objects_loader(dc.ctx);
        self.prune_objects();
        let mods = dc.ctx.input(|i| i.modifiers);
        let to_px = |p: Pos2| pl.world_to_pixel(dc.cam.to_world(dc.rect, p));
        let pointer = response.interact_pointer_pos();
        if response.clicked_by(PointerButton::Primary)
            && let Some(px) = pointer.and_then(to_px)
        {
            self.ob.focus = Some([px[0] as f32, px[1] as f32]);
        }
        if mods.alt && response.clicked_by(PointerButton::Primary) {
            if let Some(i) = pointer.and_then(|p| self.object_at(dc, pl, p, px_scale)) {
                let o = self.doc.as_ref().unwrap().data.ambient[i].clone();
                self.ob.set_pick(pick_of(&o), Some(play_of(&o)));
                self.ob.scroll_to_pick = true;
                self.info(format!("picked {}", o.entity));
            }
            return true;
        }
        if self.settings.tool == Tool::Place {
            if response.clicked_by(PointerButton::Primary)
                && let Some(px) = pointer.and_then(to_px)
            {
                self.place_object(px);
            }
            return true;
        }
        if response.drag_started_by(PointerButton::Primary)
            && let Some(origin) = dc.ctx.input(|i| i.pointer.press_origin())
        {
            let knob = match self.ob.sel.as_slice() {
                [i] => self.doc.as_ref().and_then(|d| d.data.ambient.get(*i)).and_then(|o| self.on_screen(dc, pl, o, px_scale)).is_some_and(|s| s.knob().distance(origin) <= 9.0),
                _ => false,
            };
            if knob {
                if let Some(doc) = self.doc.as_mut() {
                    doc.begin("Turn object");
                }
                self.ob.drag = Some(Drag::Turn(self.ob.sel[0]));
            } else if let Some(i) = self.object_at(dc, pl, origin, px_scale) {
                if !self.ob.sel.contains(&i) {
                    self.select_object(i, mods.shift);
                }
                if let (Some(start), Some(doc)) = (to_px(origin), self.doc.as_mut()) {
                    let orig = self.ob.sel.iter().filter_map(|&k| doc.data.ambient.get(k).map(|o| (k, o.at))).collect();
                    doc.begin("Move objects");
                    self.ob.drag = Some(Drag::Move { start, orig });
                }
            } else {
                self.ob.drag = Some(Drag::Box(origin));
            }
        }
        if let Some(drag) = &self.ob.drag {
            let done = response.drag_stopped_by(PointerButton::Primary) || !dc.ctx.input(|i| i.pointer.primary_down());
            match drag {
                Drag::Move { start, orig } => {
                    if let (Some(px), Some(doc)) = (pointer.and_then(to_px), self.doc.as_mut()) {
                        let (dx, dy) = ((px[0] - start[0]) as f32, (px[1] - start[1]) as f32);
                        let (w, h) = (doc.w as f32, doc.h as f32);
                        for &(k, at) in orig {
                            if let Some(o) = doc.data.ambient.get_mut(k) {
                                o.at = [(at[0] + dx).clamp(0.0, w - 0.01), (at[1] + dy).clamp(0.0, h - 0.01)];
                            }
                        }
                        doc.meta_version += 1;
                    }
                }
                &Drag::Turn(k) => {
                    let s = self.doc.as_ref().and_then(|d| d.data.ambient.get(k)).and_then(|o| self.on_screen(dc, pl, o, px_scale));
                    if let (Some(p), Some(s)) = (pointer, s) {
                        // the screen's north and east, without the object's turn
                        let o = &self.doc.as_ref().unwrap().data.ambient[k];
                        let (sn, cs) = o.rotation[1].to_radians().sin_cos();
                        let north = s.north * cs - s.east * sn;
                        let east = s.east * cs + s.north * sn;
                        let v = p - s.c;
                        let mut deg = v.dot(east).atan2(v.dot(north)).to_degrees().rem_euclid(360.0);
                        deg = if mods.shift { (deg / 15.0).round() * 15.0 } else { deg.round() };
                        if let Some(doc) = self.doc.as_mut() {
                            doc.data.ambient[k].rotation[1] = deg % 360.0;
                            doc.meta_version += 1;
                        }
                    }
                }
                &Drag::Box(from) => {
                    if done && let Some(to) = pointer.or(dc.ctx.input(|i| i.pointer.latest_pos())) {
                        let r = Rect::from_two_pos(from, to);
                        let hits: Vec<usize> = self
                            .doc
                            .as_ref()
                            .map(|d| d.data.ambient.iter().enumerate().filter_map(|(i, o)| self.on_screen(dc, pl, o, px_scale).filter(|s| r.contains(s.c)).map(|_| i)).collect())
                            .unwrap_or_default();
                        if !mods.shift {
                            self.ob.sel.clear();
                        }
                        for i in hits {
                            if !self.ob.sel.contains(&i) {
                                self.ob.sel.push(i);
                            }
                        }
                    }
                }
            }
            if done {
                if !matches!(self.ob.drag, Some(Drag::Box(_)))
                    && let Some(doc) = self.doc.as_mut()
                {
                    doc.commit();
                }
                self.ob.drag = None;
                self.after_edit();
            }
            return true;
        }
        if response.clicked_by(PointerButton::Primary) {
            match pointer.and_then(|p| self.object_at(dc, pl, p, px_scale)) {
                Some(i) => self.select_object(i, mods.shift || mods.command),
                None if !mods.shift => {
                    self.flush_object_name();
                    self.ob.sel.clear();
                }
                None => {}
            }
        }
        true
    }

    pub(super) fn objects_keys(&mut self, ctx: &egui::Context) {
        if ctx.input_mut(|i| i.consume_shortcut(&egui::KeyboardShortcut::new(Modifiers::COMMAND, Key::D))) {
            self.duplicate_objects();
        }
        let pressed = |k| ctx.input(|i| i.key_pressed(k) && !i.modifiers.command);
        if pressed(Key::Delete) {
            self.delete_objects();
        }
        if pressed(Key::Escape) {
            self.flush_object_name();
            self.ob.sel.clear();
        }
        if self.ob.sel.is_empty() {
            return;
        }
        let shift = ctx.input(|i| i.modifiers.shift);
        let step = if shift { 1.0 } else { 15.0 };
        for (k, d) in [(Key::OpenBracket, -step), (Key::CloseBracket, step)] {
            if pressed(k) {
                let sel = self.ob.sel.clone();
                self.run("Turn objects", |doc, _| {
                    for &i in &sel {
                        let r = &mut doc.data.ambient[i].rotation[1];
                        *r = (*r + d).rem_euclid(360.0);
                    }
                    Ok(String::new())
                });
            }
        }
        let n = if shift { 10.0 } else { 1.0 };
        for (k, dx, dy) in [(Key::ArrowLeft, -n, 0.0), (Key::ArrowRight, n, 0.0), (Key::ArrowUp, 0.0, -n), (Key::ArrowDown, 0.0, n)] {
            if ctx.input_mut(|i| i.consume_key(if shift { Modifiers::SHIFT } else { Modifiers::NONE }, k)) {
                let sel = self.ob.sel.clone();
                self.run("Move objects", |doc, _| {
                    let (w, h) = (doc.w as f32, doc.h as f32);
                    for &i in &sel {
                        let a = &mut doc.data.ambient[i].at;
                        *a = [(a[0] + dx).clamp(0.0, w - 0.01), (a[1] + dy).clamp(0.0, h - 0.01)];
                    }
                    Ok(String::new())
                });
            }
        }
    }

    /// The objects on the map, seen from above at their size in the game
    /// (a marker where that is too small to see), the selection and its
    /// turning knob; with the Place tool, the model under the cursor.
    pub(super) fn draw_objects(&mut self, dc: &DrawCtx, pl: &Placement, px_scale: f64, editing: bool) {
        let Some(doc) = self.doc.as_ref() else { return };
        if doc.data.ambient.is_empty() && !(editing && self.settings.tool == Tool::Place) {
            return;
        }
        if self.objects_loader(dc.ctx).is_none() {
            return;
        }
        let doc = self.doc.as_ref().unwrap();
        let view = dc.rect.expand(60.0);
        let mut order: Vec<(usize, OnScreen)> = doc.data.ambient.iter().enumerate().filter_map(|(i, o)| self.on_screen(dc, pl, o, px_scale).map(|s| (i, s))).filter(|(_, s)| view.contains(s.c)).collect();
        // the southern ones in front
        order.sort_by(|a, b| a.1.c.y.total_cmp(&b.1.c.y));
        let mut wanted: Vec<Pick> = Vec::new();
        let alpha = if editing { 255 } else { 210 };
        for (i, s) in &order {
            let o = &doc.data.ambient[*i];
            let pick = pick_of(o);
            let sel = editing && self.ob.sel.contains(i);
            match self.ob.icons.get(&pick) {
                Some(Icon::Ready(tex, _)) if s.half >= 5.0 => {
                    let mut mesh = egui::Mesh::with_texture(tex.id());
                    let tint = Color32::from_white_alpha(alpha);
                    let c = s.corners();
                    for (p, uv) in c.iter().zip([pos2(0.0, 0.0), pos2(1.0, 0.0), pos2(1.0, 1.0), pos2(0.0, 1.0)]) {
                        mesh.vertices.push(egui::epaint::Vertex { pos: *p, uv, color: tint });
                    }
                    mesh.indices.extend([0, 1, 2, 0, 2, 3]);
                    dc.painter.add(egui::Shape::mesh(mesh));
                    if sel {
                        dc.painter.add(egui::Shape::closed_line(c.to_vec(), Stroke::new(1.5, SELECTED)));
                    }
                }
                other => {
                    if other.is_none() {
                        wanted.push(pick.clone());
                    }
                    let fill = if matches!(other, Some(Icon::Missing)) { ERR } else { MARKER };
                    dc.painter.circle(s.c, 4.0, fill.gamma_multiply(alpha as f32 / 255.0), Stroke::new(1.0, Color32::BLACK));
                    dc.painter.line_segment([s.c, s.c + s.north * 9.0], Stroke::new(1.5, fill));
                    if sel {
                        dc.painter.circle_stroke(s.c, 7.0, Stroke::new(1.5, SELECTED));
                    }
                }
            }
        }
        if editing && let [k] = self.ob.sel.as_slice() {
            if let Some(s) = order.iter().find(|o| o.0 == *k).map(|o| &o.1) {
                let knob = s.knob();
                dc.painter.line_segment([s.c, knob], Stroke::new(1.0, SELECTED));
                dc.painter.circle(knob, 5.0, SELECTED, Stroke::new(1.0, Color32::BLACK));
            }
        }
        if let Some(Drag::Box(from)) = &self.ob.drag
            && let Some(to) = dc.ctx.input(|i| i.pointer.latest_pos())
        {
            let r = Rect::from_two_pos(*from, to);
            dc.painter.rect_stroke(r, 0.0, Stroke::new(1.0, SELECTED), egui::StrokeKind::Middle);
        }
        // what the Place tool puts down
        if editing
            && self.settings.tool == Tool::Place
            && let (Some(pick), Some(h)) = (self.ob.pick.clone(), self.hover)
        {
            let ghost = Ambient {
                entity: pick.name().to_string(),
                mesh: matches!(pick, Pick::Mesh(_)),
                at: [h[0] as f32, h[1] as f32],
                lift: 0.0,
                rotation: [0.0, if self.ob.random_turn { 0.0 } else { self.ob.new_turn }, 0.0],
                scale: self.ob.new_scale,
                animation: None,
                always_visible: false,
                name: String::new(),
            };
            if let Some(s) = self.on_screen(dc, pl, &ghost, px_scale) {
                match self.ob.icons.get(&pick) {
                    Some(Icon::Ready(tex, _)) if s.half >= 5.0 => {
                        let mut mesh = egui::Mesh::with_texture(tex.id());
                        for (p, uv) in s.corners().iter().zip([pos2(0.0, 0.0), pos2(1.0, 0.0), pos2(1.0, 1.0), pos2(0.0, 1.0)]) {
                            mesh.vertices.push(egui::epaint::Vertex { pos: *p, uv, color: Color32::from_white_alpha(130) });
                        }
                        mesh.indices.extend([0, 1, 2, 0, 2, 3]);
                        dc.painter.add(egui::Shape::mesh(mesh));
                    }
                    None => wanted.push(pick.clone()),
                    _ => {
                        dc.painter.circle_stroke(s.c, 5.0, Stroke::new(1.5, MARKER));
                    }
                }
            }
        }
        for p in wanted {
            self.ob.request(dc.ctx, &p, true);
        }
    }

    // ------------------------------------------------------------ panel

    pub(super) fn objects_panel(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let loader = self.objects_loader(&ctx);
        self.prune_objects();
        let n = self.doc.as_ref().map_or(0, |d| d.data.ambient.len());
        if loader.is_some()
            && let Some(i) = self.ob.start.take().filter(|&i| i < n)
        {
            self.select_object(i, false);
            let at = self.doc.as_ref().map(|d| d.data.ambient[i].at).unwrap_or_default();
            self.ob.focus = Some(at);
            self.zoom_to = Some([at[0] as f64 - 12.0, at[1] as f64 - 12.0, at[0] as f64 + 12.0, at[1] as f64 + 12.0]);
        }
        ui.label(format!("{n} object(s) on the map"));
        ui.label(
            RichText::new(
                "3D models the game shows on the map: map/ambient_object.txt in the build. 1 map pixel is one unit of the game; \
                 the build sets each object on the ground (or the water) of its pixel.",
            )
            .small()
            .weak(),
        );
        let Some(loader) = loader else {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("reading the models of the game and the mod…");
            });
            self.objects_list_ui(ui);
            return;
        };
        egui::CollapsingHeader::new(RichText::new("Models").strong()).default_open(true).show(ui, |ui| self.browser_ui(ui, &loader));
        egui::CollapsingHeader::new(RichText::new("Preview").strong()).default_open(true).show(ui, |ui| {
            if self.ob.pv.window {
                ui.horizontal(|ui| {
                    ui.label("Shown in the preview window.");
                    if ui.button("Back here").clicked() {
                        self.ob.pv.window = false;
                    }
                });
            } else {
                let w = ui.available_width().max(160.0);
                self.preview_ui(ui, &loader, vec2(w, (w * 0.72).clamp(140.0, 320.0)), "panel");
            }
        });
        egui::CollapsingHeader::new(RichText::new("New objects").strong()).default_open(true).show(ui, |ui| self.new_objects_ui(ui, &loader));
        if !self.ob.sel.is_empty() {
            egui::CollapsingHeader::new(RichText::new(format!("Selected ({})", self.ob.sel.len())).strong()).default_open(true).show(ui, |ui| self.selected_ui(ui, &loader));
        }
        self.objects_list_ui(ui);
    }

    fn browser_ui(&mut self, ui: &mut egui::Ui, loader: &Arc<Loader>) {
        let cat = &loader.cat;
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.ob.search).hint_text("search names and files").desired_width(ui.available_width() - 70.0));
            if ui.small_button("↻").on_hover_text("Read the models again (after adding files to the mod's content)").clicked() {
                self.ob.reload(None);
            }
        });
        if ui
            .button("➕ Import a model…")
            .on_hover_text("A model made in Blender (glTF, OBJ, or .mesh from Paradox's exporter) and its pictures, made into the game's files in the mod")
            .clicked()
        {
            self.ob.imp.open = true;
        }
        ui.horizontal_wrapped(|ui| {
            ui.label("from");
            for w in Whose::ALL {
                ui.selectable_value(&mut self.ob.whose, w, w.name());
            }
            ui.checkbox(&mut self.ob.animated_only, "animated").on_hover_text("Only models with animations");
        });
        let key: ListKey = (Arc::as_ptr(cat) as usize, self.ob.search.to_lowercase(), self.ob.whose, self.ob.folder.clone(), self.ob.animated_only);
        if self.ob.list.as_ref().is_none_or(|l| l.0 != key) {
            let words: Vec<&str> = key.1.split_whitespace().collect();
            let mut folders: BTreeMap<String, usize> = BTreeMap::new();
            let mut rows = Vec::new();
            for (i, it) in cat.items.iter().enumerate() {
                let origin = &cat.roots[it.root].origin;
                if !self.ob.whose.fits(origin) || (self.ob.animated_only && !it.animated) || !words.iter().all(|w| it.haystack.contains(w)) {
                    continue;
                }
                *folders.entry(it.folder.clone()).or_default() += 1;
                if self.ob.folder.as_ref().is_none_or(|f| it.folder == *f || it.folder.starts_with(&format!("{f}/"))) {
                    rows.push(i);
                }
            }
            self.ob.list = Some((key, Arc::new(rows), Arc::new(folders)));
        }
        let (_, rows, folders) = self.ob.list.clone().unwrap();
        // the folders, as a tree
        egui::CollapsingHeader::new(format!("Folders{}", self.ob.folder.as_ref().map(|f| format!(": {f}")).unwrap_or_default())).id_salt("obj_folders").show(ui, |ui| {
            egui::ScrollArea::vertical().id_salt("obj_folder_scroll").max_height(160.0).show(ui, |ui| {
                if ui.selectable_label(self.ob.folder.is_none(), format!("all  {}", folders.values().sum::<usize>())).clicked() {
                    self.ob.folder = None;
                }
                let mut tree = Folder::default();
                for (f, c) in folders.iter() {
                    tree.add(f, *c);
                }
                for (name, f) in &tree.children {
                    folder_ui(ui, name, name, f, &mut self.ob.folder, 0);
                }
            });
        });
        ui.label(RichText::new(format!("{} model(s){}", rows.len(), if rows.len() < cat.items.len() { format!(" of {}", cat.items.len()) } else { String::new() })).small());
        let row_h = 44.0;
        let scroll_to = if self.ob.scroll_to_pick { self.ob.pick.as_ref().and_then(|p| rows.iter().position(|&r| &cat.items[r].pick == p)) } else { None };
        self.ob.scroll_to_pick = false;
        let mut area = egui::ScrollArea::vertical().id_salt("obj_browser").max_height(260.0).auto_shrink([false, true]);
        if let Some(k) = scroll_to {
            area = area.vertical_scroll_offset((k as f32 * (row_h + ui.spacing().item_spacing.y) - 100.0).max(0.0));
        }
        let mut clicked = None;
        let mut want = Vec::new();
        area.show_rows(ui, row_h, rows.len(), |ui, range| {
            for &r in &rows[range] {
                let it = &cat.items[r];
                let on = self.ob.pick.as_ref() == Some(&it.pick);
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click());
                if on {
                    ui.painter().rect_filled(rect, 3.0, ui.visuals().selection.bg_fill);
                } else if resp.hovered() {
                    ui.painter().rect_filled(rect, 3.0, ui.visuals().widgets.hovered.weak_bg_fill);
                }
                let tr = Rect::from_min_size(rect.min + vec2(2.0, 2.0), vec2(row_h - 4.0, row_h - 4.0));
                match self.ob.thumbs.get(&it.pick) {
                    Some(Icon::Ready(t, _)) => {
                        ui.painter().image(t.id(), tr, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                    }
                    Some(Icon::Missing) => {
                        ui.painter().text(tr.center(), egui::Align2::CENTER_CENTER, "–", egui::FontId::proportional(14.0), ui.visuals().weak_text_color());
                    }
                    other => {
                        if other.is_none() {
                            want.push(it.pick.clone());
                        }
                        ui.painter().rect_stroke(tr.shrink(6.0), 2.0, Stroke::new(1.0, ui.visuals().weak_text_color()), egui::StrokeKind::Inside);
                    }
                }
                let tx = tr.right() + 6.0;
                let name = it.pick.name();
                let col = if on { ui.visuals().selection.stroke.color } else { ui.visuals().text_color() };
                ui.painter().text(pos2(tx, rect.top() + 4.0), egui::Align2::LEFT_TOP, name, egui::FontId::proportional(13.0), col);
                let origin = cat.roots[it.root].origin.label();
                let sub = format!("{} · {}{}{}", it.folder, origin, if it.animated { " · animated" } else { "" }, if matches!(it.pick, Pick::Mesh(_)) { " · mesh" } else { "" });
                ui.painter().text(pos2(tx, rect.top() + 23.0), egui::Align2::LEFT_TOP, sub, egui::FontId::proportional(11.0), ui.visuals().weak_text_color());
                if resp.clicked() {
                    clicked = Some(it.pick.clone());
                }
                resp.on_hover_text(match &it.pick {
                    Pick::Entity(e) => format!("entity {e}\nclick: show it in the preview; the Place tool (A) puts it on the map"),
                    Pick::Mesh(m) => format!("mesh {m} (no entity shows it: the build makes one)\nclick: show it in the preview; the Place tool (A) puts it on the map"),
                });
            }
        });
        let ctx = ui.ctx().clone();
        for p in want {
            self.ob.request(&ctx, &p, false);
        }
        if let Some(p) = clicked {
            self.ob.set_pick(p, None);
        }
    }

    /// The 3D preview: the browser's model alone, or the map around the
    /// selection with its objects; drag to turn, wheel to zoom,
    /// double-click to reset the view.
    fn preview_ui(&mut self, ui: &mut egui::Ui, loader: &Arc<Loader>, size: Vec2, which: &'static str) {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.ob.pv.scene, false, "Model").on_hover_text("The model picked in the list");
            ui.selectable_value(&mut self.ob.pv.scene, true, "On the map")
                .on_hover_text("The map around the selected objects (or the last place clicked) in 3D, with the objects there, as the build will set them");
        });
        let ppp = ui.ctx().pixels_per_point();
        let (pw, ph) = (((size.x * ppp) as usize).clamp(32, 1400), ((size.y * ppp) as usize).clamp(32, 1000));
        let bg = if ui.visuals().dark_mode { [34, 37, 44, 255] } else { [200, 205, 212, 255] };
        let dt = ui.input(|i| i.stable_dt).min(0.1);
        let frame = if self.ob.pv.scene {
            ui.horizontal(|ui| {
                ui.add(egui::Slider::new(&mut self.ob.pv.radius, 8.0..=200.0).logarithmic(true).text("px around"));
                if ui.button(if self.ob.pv.playing { "⏸" } else { "▶" }).on_hover_text("Play / pause the animations").clicked() {
                    self.ob.pv.playing = !self.ob.pv.playing;
                }
            });
            if self.ob.pv.playing {
                self.ob.pv.time += dt * self.ob.pv.speed;
                ui.ctx().request_repaint();
            }
            match self.scene_picture(loader, pw, ph, bg) {
                Ok(f) => f,
                Err(e) => {
                    ui.label(RichText::new(e).weak());
                    return;
                }
            }
        } else {
            let Some(pick) = self.ob.pick.clone() else {
                ui.label(RichText::new("Pick a model in the list to see it here.").weak());
                return;
            };
            let m = loader.model(&pick);
            let pv = &mut self.ob.pv;
            // what plays
            let mut options: Vec<Play> = vec![Play::Still];
            match &pick {
                Pick::Entity(_) => options.extend(m.state_names().into_iter().map(Play::State)),
                Pick::Mesh(_) => options.extend(m.anims.iter().filter(|a| a.1.is_some()).map(|a| Play::Anim(a.0.clone()))),
            }
            if !options.contains(&pv.play) {
                pv.play = Play::Still;
            }
            ui.horizontal_wrapped(|ui| {
                ui.label("plays");
                egui::ComboBox::from_id_salt(("obj_play", which)).selected_text(play_label(&pv.play)).width(150.0).show_ui(ui, |ui| {
                    for p in &options {
                        let anim = match p {
                            Play::State(s) => m.entity.as_ref().and_then(|e| e.state(s)).and_then(|s| s.animation.clone()),
                            Play::Anim(a) => Some(a.clone()),
                            Play::Still => None,
                        };
                        let mut label = play_label(p);
                        if let (Play::State(_), Some(a)) = (p, &anim) {
                            label += &format!(" ({a})");
                        }
                        if ui.selectable_label(pv.play == *p, label).clicked() {
                            pv.play = p.clone();
                            pv.time = 0.0;
                        }
                    }
                });
                if m.duration(&pv.play).is_some() {
                    if ui.button(if pv.playing { "⏸" } else { "▶" }).on_hover_text("Play / pause").clicked() {
                        pv.playing = !pv.playing;
                    }
                    ui.add(egui::DragValue::new(&mut pv.speed).range(0.05..=4.0).speed(0.01).suffix("×")).on_hover_text("Speed");
                }
            });
            if let Some(d) = m.duration(&pv.play) {
                if pv.playing {
                    pv.time += dt * pv.speed;
                    // a state that does not loop holds its end a moment, then starts again
                    if pv.time > d + 0.8 {
                        pv.time = 0.0;
                    }
                    ui.ctx().request_repaint();
                }
                ui.add(egui::Slider::new(&mut pv.time, 0.0..=d.max(0.01)).text("s").fixed_decimals(2));
            }
            models::picture(&m, pw, ph, pv.view, &pv.play, pv.time, bg)
        };
        let pv = &mut self.ob.pv;
        let img = color_image(&frame);
        let tex = match pv.tex.get_mut(which) {
            Some(t) => {
                t.set(img, egui::TextureOptions::LINEAR);
                t.clone()
            }
            None => {
                let t = ui.ctx().load_texture(format!("object preview {which}"), img, egui::TextureOptions::LINEAR);
                pv.tex.insert(which, t.clone());
                t
            }
        };
        let view = if pv.scene { &mut pv.scene_view } else { &mut pv.view };
        let resp = ui.add(egui::Image::new((tex.id(), size)).sense(Sense::click_and_drag()));
        if resp.dragged() {
            let d = resp.drag_delta();
            view.yaw = (view.yaw - d.x * 0.6).rem_euclid(360.0);
            view.pitch = (view.pitch + d.y * 0.4).clamp(-15.0, 89.0);
        }
        if resp.hovered() {
            let s = ui.input_mut(|i| std::mem::take(&mut i.smooth_scroll_delta)).y;
            if s != 0.0 {
                view.zoom = (view.zoom * (s * 0.003).exp()).clamp(0.2, 20.0);
            }
        }
        if resp.double_clicked() {
            *view = if pv.scene { scene_view() } else { View { ground: view.ground, ..View::default() } };
        }
        resp.on_hover_text("Drag to turn, wheel to zoom, double-click to reset");
        ui.horizontal_wrapped(|ui| {
            if !pv.scene {
                ui.checkbox(&mut pv.view.ground, "ground");
            }
            if which == "panel" && ui.button("⛶ Bigger…").on_hover_text("Show the preview in a window of its own").clicked() {
                pv.window = true;
            }
        });
        if pv.scene {
            return;
        }
        // what it is
        let Some(pick) = self.ob.pick.clone() else { return };
        let m = loader.model(&pick);
        let (lo, hi) = m.bounds();
        let size_px = hi - lo;
        let origin = loader.cat.origin_of(&pick).map(Origin::label).unwrap_or_default();
        let mut facts = format!(
            "{} · {} · {:.2} × {:.2} map px, {:.2} high",
            match &pick {
                Pick::Entity(_) => "entity",
                Pick::Mesh(_) => "mesh",
            },
            origin,
            size_px.x,
            size_px.z,
            size_px.y
        );
        if let Some(mesh) = &m.mesh {
            facts += &format!(" · {} triangles", mesh.triangles());
            if !mesh.bones.is_empty() {
                facts += &format!(", {} bones", mesh.bones.len());
            }
        }
        if !m.attach.is_empty() {
            facts += &format!(" · {} attached", m.attach.len());
        }
        ui.label(RichText::new(facts).small());
        if let Some(f) = &m.file {
            ui.label(RichText::new(f.display().to_string()).small().weak());
        }
        if m.mesh.is_none() {
            ui.label(RichText::new("No model: a sound or an effect (it can still be placed).").small().color(WARN));
        }
        for p in m.problems.iter().take(4) {
            ui.label(RichText::new(p).small().color(WARN));
        }
    }

    /// The map around the focus in 3D: the heights and terrain.bmp colours
    /// as the build makes them, the water flat at its level, and the objects
    /// there as the game will set them.
    fn scene_picture(&mut self, loader: &Arc<Loader>, pw: usize, ph: usize, bg: [u8; 4]) -> Result<models::render::Frame, String> {
        use crate::models::math::{Affine, v3};
        use crate::models::render::{Camera, Frame, Proj, Texture, Vert};
        use crate::modbuild::{bitmaps, vanilla::WATER};
        let focus = match self.ob.sel.as_slice() {
            [] => self.ob.focus,
            sel => {
                let ds = &self.doc.as_ref().ok_or("no map")?.data.ambient;
                let n = sel.len() as f32;
                Some([sel.iter().map(|&i| ds[i].at[0]).sum::<f32>() / n, sel.iter().map(|&i| ds[i].at[1]).sum::<f32>() / n])
            }
        };
        let Some(focus) = focus else { return Err("Select or place an object, or click the map, to see it here.".into()) };
        let heights = self.heights_now().ok_or("The heights are still loading…")?;
        let auto = self.auto_texture().ok_or("The land cover is still loading…")?;
        let doc = self.doc.as_ref().ok_or("no map")?;
        let (w, h) = (doc.w as i64, doc.h as i64);
        let paint = doc.paint.read().clone();
        let lab = doc.labels.read();
        let water_of: Vec<bool> = (0..doc.data.provinces.cap() as u32).map(|p| doc.data.is_water(p)).collect();
        let r = self.ob.pv.radius.round().max(4.0) as i64;
        let (cx, cy) = (focus[0].floor() as i64, focus[1].floor() as i64);
        let (x0, y0, x1, y1) = ((cx - r).max(0), (cy - r).max(0), (cx + r).min(w - 1), (cy + r).min(h - 1));
        if x0 >= x1 || y0 >= y1 {
            return Err("off the map".into());
        }
        let (pw_, ph_) = ((x1 - x0 + 1) as usize, (y1 - y0 + 1) as usize);
        let palette = bitmaps::vanilla_palette();
        let mut rgba = Vec::with_capacity(pw_ * ph_ * 4);
        let mut ground = vec![0f32; pw_ * ph_];
        for y in y0..=y1 {
            for x in x0..=x1 {
                let i = (y * w + x) as usize;
                let water = water_of.get(lab.px[i] as usize).copied().unwrap_or(false);
                let c = if water {
                    [52, 92, 140]
                } else {
                    let t = if paint.get(i).is_some_and(|&p| p != bitmaps::AUTO) { paint[i] } else { auto[i] };
                    palette.get(t as usize).copied().unwrap_or([120, 120, 110])
                };
                rgba.extend_from_slice(&[c[0], c[1], c[2], 255]);
                ground[((y - y0) * (x1 - x0 + 1) + (x - x0)) as usize] = if water { WATER as f32 / 10.0 } else { heights[i].max(WATER) as f32 / 10.0 };
            }
        }
        drop(lab);
        let tex = Texture::from_rgba(pw_, ph_, &rgba, 1024, false);
        // a vertex at each pixel centre (every step-th), the map's z counted from the bottom
        let step = ((pw_.max(ph_) / 120).max(1)) as i64;
        let gx: Vec<i64> = (x0..=x1).step_by(step as usize).collect();
        let gy: Vec<i64> = (y0..=y1).step_by(step as usize).collect();
        let at = |x: i64, y: i64| ground[((y.clamp(y0, y1) - y0) * (x1 - x0 + 1) + (x.clamp(x0, x1) - x0)) as usize];
        let mut verts = Vec::with_capacity(gx.len() * gy.len());
        for &y in &gy {
            for &x in &gx {
                let n = v3(at(x - 1, y) - at(x + 1, y), 2.0, at(x, y + 1) - at(x, y - 1)).norm();
                verts.push(Vert {
                    p: v3(x as f32 + 0.5, at(x, y), (h - y) as f32 - 0.5),
                    n,
                    uv: [((x - x0) as f32 + 0.5) / pw_ as f32, ((y - y0) as f32 + 0.5) / ph_ as f32],
                });
            }
        }
        let mut tri = Vec::new();
        let cols = gx.len() as u32;
        for j in 0..gy.len().saturating_sub(1) as u32 {
            for i in 0..cols - 1 {
                let a = j * cols + i;
                tri.extend([a, a + 1, a + cols, a + 1, a + cols + 1, a + cols]);
            }
        }
        let centre = v3(focus[0], at(cx, cy), h as f32 - focus[1]);
        let view = self.ob.pv.scene_view;
        let fov_half = 22f32.to_radians();
        let focal = ph.min(pw) as f32 / 2.0 / fov_half.tan();
        let dist = (r as f32 * 1.25 / fov_half.sin() / view.zoom.max(0.05)).max(1.0);
        let cam = Camera::orbit(centre, view.yaw, view.pitch, dist, Proj::Persp { focal, near: dist * 0.01 });
        let mut f = Frame::new(pw, ph, bg);
        f.draw(&cam, &verts, &tri, &tex, [1.0; 3]);
        // the objects on this part of the map
        let time = self.ob.pv.time;
        for o in &doc.data.ambient {
            let (ox, oy) = (o.at[0] as i64, o.at[1] as i64);
            if ox < x0 - 4 || ox > x1 + 4 || oy < y0 - 4 || oy > y1 + 4 {
                continue;
            }
            let m = loader.model(&pick_of(o));
            let base = at(ox, oy) + o.lift;
            let world = Affine::translate(v3(o.at[0], base, h as f32 - o.at[1]))
                .then_after(&Affine::yaw(o.rotation[1]))
                .then_after(&Affine::pitch(o.rotation[0]))
                .then_after(&Affine::roll(o.rotation[2]))
                .then_after(&Affine::scale(o.scale));
            if m.mesh.is_some() {
                m.draw(&mut f, &cam, &world, &play_of(o), time);
            } else {
                // a sound or effect: a small post
                let post = [
                    Vert { p: world.apply(v3(-0.1, 0.0, 0.0)), n: v3(0.0, 0.0, -1.0), uv: [0.0, 0.0] },
                    Vert { p: world.apply(v3(0.1, 0.0, 0.0)), n: v3(0.0, 0.0, -1.0), uv: [0.0, 0.0] },
                    Vert { p: world.apply(v3(0.0, 1.5, 0.0)), n: v3(0.0, 0.0, -1.0), uv: [0.0, 0.0] },
                ];
                f.draw(&cam, &post, &[0, 1, 2], &Texture::solid([255, 150, 60, 255]), [1.0; 3]);
            }
        }
        Ok(f)
    }

    pub(super) fn objects_window(&mut self, ctx: &egui::Context) {
        if !self.ob.pv.window || self.settings.level != super::Level::Objects {
            return;
        }
        let Some(loader) = self.ob.loader.clone() else { return };
        let mut open = true;
        egui::Window::new("Model preview").open(&mut open).default_size([560.0, 520.0]).resizable(true).show(ctx, |ui| {
            let avail = ui.available_size();
            let size = vec2(avail.x.max(200.0), (avail.y - 120.0).max(160.0));
            self.preview_ui(ui, &loader, size, "window");
        });
        if !open {
            self.ob.pv.window = false;
        }
    }

    fn new_objects_ui(&mut self, ui: &mut egui::Ui, loader: &Arc<Loader>) {
        let ob = &mut self.ob;
        match &ob.pick {
            Some(p) => {
                ui.label(format!("The Place tool (A) puts down {} {}, {}.", if matches!(p, Pick::Mesh(_)) { "mesh" } else { "entity" }, p.name(), play_label(&ob.pv.play)));
                still_note(ui, loader, &loader.model(p), p, &ob.pv.play);
            }
            None => {
                ui.label(RichText::new("Pick a model above; the Place tool (A) then puts it on the map.").weak());
            }
        }
        ui.label(RichText::new("New objects play what the preview plays.").small().weak());
        egui::Grid::new("obj_new").num_columns(2).show(ui, |ui| {
            ui.label("scale");
            ui.add(egui::DragValue::new(&mut ob.new_scale).range(0.01..=1000.0).speed(0.01).max_decimals(3));
            ui.end_row();
            ui.label("turn");
            ui.horizontal(|ui| {
                ui.add_enabled(!ob.random_turn, egui::DragValue::new(&mut ob.new_turn).range(0.0..=359.0).speed(1.0).suffix("°"));
                ui.checkbox(&mut ob.random_turn, "random");
            });
            ui.end_row();
        });
        ui.checkbox(&mut ob.new_visible, "always visible").on_hover_text("Seen from any distance (the game hides small objects when zoomed out)");
    }

    fn selected_ui(&mut self, ui: &mut egui::Ui, loader: &Arc<Loader>) {
        let Some(doc) = self.doc.as_ref() else { return };
        let sel = self.ob.sel.clone();
        let objs: Vec<Ambient> = sel.iter().filter_map(|&i| doc.data.ambient.get(i).cloned()).collect();
        if objs.is_empty() {
            return;
        }
        let first = objs[0].clone();
        let same_model = objs.iter().all(|o| o.entity == first.entity && o.mesh == first.mesh);
        if same_model {
            ui.label(RichText::new(format!("{} {}", if first.mesh { "mesh" } else { "entity" }, first.entity)).strong());
        } else {
            ui.label(format!("{} models", objs.iter().map(|o| &o.entity).collect::<std::collections::BTreeSet<_>>().len()));
        }
        if objs.len() == 1 {
            let i = sel[0];
            let mut name = match &self.ob.name_edit {
                Some((k, n)) if *k == i => n.clone(),
                _ => first.name.clone(),
            };
            ui.horizontal(|ui| {
                ui.label("name");
                let r = ui.add(egui::TextEdit::singleline(&mut name).hint_text(format!("{}_…", first.entity.trim_end_matches("_entity"))));
                if r.changed() {
                    self.ob.name_edit = Some((i, name.clone()));
                }
                if r.lost_focus() {
                    self.flush_object_name();
                }
            });
        }
        let mut edits: Vec<(egui::Response, &'static str, Box<dyn Fn(&mut Ambient)>)> = Vec::new();
        egui::Grid::new("obj_sel").num_columns(2).show(ui, |ui| {
            if objs.len() == 1 {
                let (mut x, mut y) = (first.at[0], first.at[1]);
                ui.label("at");
                ui.horizontal(|ui| {
                    let rx = ui.add(egui::DragValue::new(&mut x).speed(0.1).max_decimals(2).prefix("x "));
                    let ry = ui.add(egui::DragValue::new(&mut y).speed(0.1).max_decimals(2).prefix("y "));
                    edits.push((rx, "Move object", Box::new(move |o: &mut Ambient| o.at[0] = x)));
                    edits.push((ry, "Move object", Box::new(move |o: &mut Ambient| o.at[1] = y)));
                });
                ui.end_row();
            }
            let mut turn = first.rotation[1];
            ui.label("turn");
            let r = ui.add(egui::DragValue::new(&mut turn).speed(1.0).range(-360.0..=360.0).suffix("°")).on_hover_text("Clockwise seen from above; [ and ] turn by 15° (Shift: 1°)");
            edits.push((r, "Turn objects", Box::new(move |o: &mut Ambient| o.rotation[1] = turn.rem_euclid(360.0))));
            ui.end_row();
            let (mut tilt, mut roll) = (first.rotation[0], first.rotation[2]);
            ui.label("tilt, roll");
            ui.horizontal(|ui| {
                let a = ui.add(egui::DragValue::new(&mut tilt).speed(0.5).range(-180.0..=180.0).suffix("°")).on_hover_text("About the east–west axis (rotation x)");
                let b = ui.add(egui::DragValue::new(&mut roll).speed(0.5).range(-180.0..=180.0).suffix("°")).on_hover_text("About the north–south axis (rotation z)");
                edits.push((a, "Tilt objects", Box::new(move |o: &mut Ambient| o.rotation[0] = tilt)));
                edits.push((b, "Tilt objects", Box::new(move |o: &mut Ambient| o.rotation[2] = roll)));
            });
            ui.end_row();
            let mut scale = first.scale;
            ui.label("scale");
            let r = ui.add(egui::DragValue::new(&mut scale).speed(0.01).range(0.01..=1000.0).max_decimals(3));
            edits.push((r, "Scale objects", Box::new(move |o: &mut Ambient| o.scale = scale)));
            ui.end_row();
            let mut lift = first.lift;
            ui.label("lift");
            let r = ui
                .add(egui::DragValue::new(&mut lift).speed(0.05).range(-50.0..=200.0).max_decimals(2))
                .on_hover_text("Height above the ground (or the water) in map units; 1 is 10 steps of heightmap.bmp");
            edits.push((r, "Lift objects", Box::new(move |o: &mut Ambient| o.lift = lift)));
            ui.end_row();
        });
        for (r, label, f) in edits {
            self.edit_objects(&r, label, f);
        }
        // what it plays
        if same_model {
            let m = loader.model(&pick_of(&first));
            let mut options: Vec<Play> = vec![Play::Still];
            if first.mesh {
                options.extend(m.anims.iter().map(|a| Play::Anim(a.0.clone())));
            } else {
                options.extend(m.state_names().into_iter().map(Play::State));
            }
            let cur = play_of(&first);
            let mixed = objs.iter().any(|o| o.animation != first.animation);
            ui.horizontal(|ui| {
                ui.label("plays");
                egui::ComboBox::from_id_salt("obj_sel_play").selected_text(if mixed { "(mixed)".into() } else { play_label(&cur) }).show_ui(ui, |ui| {
                    for p in &options {
                        if ui.selectable_label(!mixed && cur == *p, play_label(p)).clicked() {
                            let a = animation_of(p, first.mesh);
                            let sel = sel.clone();
                            self.run("Animate objects", |d, _| {
                                for &i in &sel {
                                    d.data.ambient[i].animation = a.clone();
                                }
                                Ok(String::new())
                            });
                            self.ob.pv.play = p.clone();
                            self.ob.pv.time = 0.0;
                        }
                    }
                });
            });
            if !mixed {
                still_note(ui, loader, &m, &pick_of(&first), &cur);
            }
        }
        let mut vis = first.always_visible;
        let mixed_vis = objs.iter().any(|o| o.always_visible != vis);
        if ui.checkbox(&mut vis, if mixed_vis { "always visible (mixed)" } else { "always visible" }).changed() {
            let sel = sel.clone();
            self.run("Objects always visible", |d, _| {
                for &i in &sel {
                    d.data.ambient[i].always_visible = vis;
                }
                Ok(String::new())
            });
        }
        ui.horizontal_wrapped(|ui| {
            if ui.button("Delete").on_hover_text("Del").clicked() {
                self.delete_objects();
            }
            if ui.button("Duplicate").on_hover_text("Ctrl+D").clicked() {
                self.duplicate_objects();
            }
            let pick = self.ob.pick.clone();
            let differs = pick.as_ref().is_some_and(|p| !objs.iter().all(|o| &pick_of(o) == p));
            if ui
                .add_enabled(differs, egui::Button::new("Use the browser's model"))
                .on_hover_text("Show the model picked in the list (and what the preview plays) instead")
                .clicked()
                && let Some(p) = pick
            {
                let mesh = matches!(p, Pick::Mesh(_));
                let a = animation_of(&self.ob.pv.play, mesh);
                let sel = sel.clone();
                self.run("Change objects' model", |d, _| {
                    for &i in &sel {
                        let o = &mut d.data.ambient[i];
                        o.entity = p.name().to_string();
                        o.mesh = mesh;
                        o.animation = a.clone();
                    }
                    Ok(String::new())
                });
            }
            if ui.button("Zoom to").clicked() {
                let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
                for o in &objs {
                    b = [b[0].min(o.at[0] as f64), b[1].min(o.at[1] as f64), b[2].max(o.at[0] as f64), b[3].max(o.at[1] as f64)];
                }
                self.zoom_to = Some([b[0] - 10.0, b[1] - 10.0, b[2] + 10.0, b[3] + 10.0]);
            }
        });
    }

    fn objects_list_ui(&mut self, ui: &mut egui::Ui) {
        let Some(doc) = self.doc.as_ref() else { return };
        let n = doc.data.ambient.len();
        if n == 0 {
            return;
        }
        let mut select = None;
        let mut zoom = None;
        egui::CollapsingHeader::new(RichText::new(format!("Objects on the map ({n})")).strong()).id_salt("obj_list").show(ui, |ui| {
            ui.add(egui::TextEdit::singleline(&mut self.ob.list_filter).hint_text("filter"));
            let f = self.ob.list_filter.to_lowercase();
            let rows: Vec<usize> = (0..n).filter(|&i| f.is_empty() || doc.data.ambient[i].entity.to_lowercase().contains(&f) || doc.data.ambient[i].name.to_lowercase().contains(&f)).collect();
            egui::ScrollArea::vertical().id_salt("obj_list_scroll").max_height(220.0).show_rows(ui, 18.0, rows.len(), |ui, range| {
                for &i in &rows[range] {
                    let o = &doc.data.ambient[i];
                    let label = format!("{}{} · {:.0}, {:.0}", if o.name.is_empty() { String::new() } else { format!("{} · ", o.name) }, o.entity, o.at[0], o.at[1]);
                    let r = ui.selectable_label(self.ob.sel.contains(&i), label).on_hover_text("click: select; double-click: zoom to it");
                    if r.clicked() {
                        select = Some((i, ui.input(|inp| inp.modifiers.shift || inp.modifiers.command)));
                    }
                    if r.double_clicked() {
                        zoom = Some(o.at);
                    }
                }
            });
        });
        if let Some((i, add)) = select {
            self.select_object(i, add);
        }
        if let Some(at) = zoom {
            self.zoom_to = Some([at[0] as f64 - 15.0, at[1] as f64 - 15.0, at[0] as f64 + 15.0, at[1] as f64 + 15.0]);
        }
    }
}

#[derive(Default)]
struct Folder {
    count: usize,
    own: usize,
    children: BTreeMap<String, Folder>,
}

impl Folder {
    fn add(&mut self, path: &str, n: usize) {
        self.count += n;
        match path.split_once('/') {
            None => {
                let c = self.children.entry(path.to_string()).or_default();
                c.count += n;
                c.own += n;
            }
            Some((head, rest)) => self.children.entry(head.to_string()).or_default().add(rest, n),
        }
    }
}

fn folder_ui(ui: &mut egui::Ui, name: &str, path: &str, f: &Folder, sel: &mut Option<String>, depth: usize) {
    let label = format!("{name}  {}", f.count);
    let on = sel.as_deref() == Some(path);
    if f.children.is_empty() {
        if ui.selectable_label(on, label).clicked() {
            *sel = if on { None } else { Some(path.to_string()) };
        }
        return;
    }
    let id = ui.make_persistent_id(("obj_folder", path));
    egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false)
        .show_header(ui, |ui| {
            if ui.selectable_label(on, label).clicked() {
                *sel = if on { None } else { Some(path.to_string()) };
            }
        })
        .body(|ui| {
            for (n, c) in &f.children {
                folder_ui(ui, n, &format!("{path}/{n}"), c, sel, depth + 1);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_count_up() {
        let mut t = Folder::default();
        t.add("units", 3);
        t.add("units/tanks", 2);
        t.add("buildings/landmarks", 4);
        assert_eq!(t.count, 9);
        assert_eq!((t.children["units"].count, t.children["units"].own), (5, 3));
        assert_eq!(t.children["buildings"].children["landmarks"].count, 4);
    }

    #[test]
    fn plays_map_to_animations() {
        let mut o = Ambient { entity: "x".into(), mesh: false, at: [0.0; 2], lift: 0.0, rotation: [0.0; 3], scale: 1.0, animation: Some("idle".into()), always_visible: false, name: String::new() };
        assert_eq!(play_of(&o), Play::State("idle".into()));
        o.mesh = true;
        assert_eq!(play_of(&o), Play::Anim("idle".into()));
        assert_eq!(animation_of(&Play::State("move".into()), true), None, "a mesh has no states");
        assert_eq!(animation_of(&Play::Anim("sway".into()), true).as_deref(), Some("sway"));
        assert_eq!(animation_of(&Play::Still, false), None);
    }
}
