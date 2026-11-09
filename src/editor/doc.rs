//! The open map: data, bitmap, derived statistics and the undo history.
//!
//! Every edit runs in a transaction ([`Doc::begin`] … [`Doc::commit`]): the
//! data before the edit and every changed pixel are kept, so it can be undone.
//! After an edit, provinces without pixels, states without provinces, nations
//! without states and regions without provinces are removed.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use parking_lot::RwLock;

use super::density::{self, DensityParams, Terrain};
use super::model::{self, Id, Index, Kind, Labels, Loaded, MapData, PStat, SaveReport, Stats};
use crate::geo::FileGeoInfo;
use crate::modbuild::bitmaps;

fn opt_ptr_eq<T>(a: &Option<Arc<T>>, b: &Option<Arc<T>>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        (None, None) => true,
        _ => false,
    }
}

const MAX_UNDO: usize = 80;

struct Txn {
    label: String,
    before: MapData,
    pixels: Vec<(u32, Id, Id)>,
    /// Painted terrain.bmp: (pixel, old, new).
    tex: Vec<(u32, u8, u8)>,
    heights: Option<Arc<Vec<u8>>>,
    /// The whole map was replaced: its impassable borders are its own.
    whole: bool,
}

struct Step {
    label: String,
    /// The data on the other side of the step (before it when in the undo
    /// stack, after it when in the redo stack).
    data: MapData,
    /// (pixel, old, new)
    pixels: Vec<(u32, Id, Id)>,
    tex: Vec<(u32, u8, u8)>,
    /// The imported heightmap on the other side of the step.
    heights: Option<Arc<Vec<u8>>>,
}

pub struct Doc {
    pub dir: PathBuf,
    pub data: MapData,
    pub labels: Arc<RwLock<Labels>>,
    pub stats: Stats,
    pub index: Index,
    pub geo: FileGeoInfo,
    pub w: u32,
    pub h: u32,
    pub terrain: Option<Arc<Terrain>>,
    /// terrain.bmp as painted ([`bitmaps::AUTO`]: as the build makes it).
    pub paint: Arc<RwLock<Vec<u8>>>,
    /// A heightmap used instead of the build's.
    pub heights: Option<Arc<Vec<u8>>>,
    /// Bumped when the painted terrain or the imported heightmap change.
    pub bitmap_version: u64,
    density_cache: Option<(DensityParams, Arc<Vec<f32>>)>,
    /// Unsaved changes.
    pub dirty: bool,
    /// Bumped when the data changes (names, memberships, ...).
    pub meta_version: u64,
    /// Bumped when pixels change.
    pub pixel_version: u64,
    txn: Option<Txn>,
    undo: Vec<Step>,
    redo: Vec<Step>,
}

/// Pixel access inside a transaction; every change is logged for undo.
pub struct PixelWriter<'a> {
    pub lab: &'a mut Labels,
    log: &'a mut Vec<(u32, Id, Id)>,
}

impl PixelWriter<'_> {
    #[inline]
    pub fn get(&self, i: usize) -> Id {
        self.lab.px[i]
    }
    #[inline]
    pub fn set(&mut self, i: usize, v: Id) {
        let old = self.lab.px[i];
        if old != v {
            self.log.push((i as u32, old, v));
            self.lab.px[i] = v;
        }
    }
}

impl Doc {
    pub fn new(dir: PathBuf, l: Loaded) -> Doc {
        let (w, h) = (l.labels.w, l.labels.h);
        let stats = model::compute_stats(&l.labels, l.data.provinces.cap());
        let paint = l.extra.paint.unwrap_or_else(|| vec![bitmaps::AUTO; (w * h) as usize]);
        let index = Index::build(&l.data);
        let mut d = Doc {
            dir,
            data: l.data,
            labels: Arc::new(RwLock::new(l.labels)),
            stats,
            index,
            geo: l.geo,
            w,
            h,
            terrain: None,
            paint: Arc::new(RwLock::new(paint)),
            heights: l.extra.heights.map(Arc::new),
            bitmap_version: 1,
            density_cache: None,
            dirty: false,
            meta_version: 1,
            pixel_version: 1,
            txn: None,
            undo: Vec::new(),
            redo: Vec::new(),
        };
        // memberships as loaded, but consistent (e.g. sea provinces have no state)
        d.cleanup();
        d.index = Index::build(&d.data);
        d
    }

    // ------------------------------------------------------------ transactions

    pub fn in_txn(&self) -> bool {
        self.txn.is_some()
    }

    pub fn begin(&mut self, label: impl Into<String>) {
        if self.txn.is_some() {
            self.commit();
        }
        self.txn = Some(Txn { label: label.into(), before: self.data.clone(), pixels: Vec::new(), tex: Vec::new(), heights: self.heights.clone(), whole: false });
    }

    /// Finish the edit: refresh derived data, push an undo step.
    pub fn commit(&mut self) -> Option<String> {
        let txn = self.txn.take()?;
        let px = !txn.pixels.is_empty();
        let touched_before = if px { std::mem::take(&mut self.stats.border) } else { Default::default() };
        if px {
            self.restat_pixels();
            // straits follow their re-cut provinces, before the clean-up
            // would drop those whose end province is gone
            if !txn.whole {
                self.carry_straits(&txn.pixels);
            }
        }
        self.refresh(false);
        if px && !txn.whole && self.carry_walls(&txn.before, &touched_before, &txn.pixels) {
            self.refresh(false);
        }
        self.undo.push(Step { label: txn.label.clone(), data: txn.before, pixels: txn.pixels, tex: txn.tex, heights: txn.heights });
        if self.undo.len() > MAX_UNDO {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.dirty = true;
        Some(txn.label)
    }

    /// Abandon the edit and restore the state before it.
    pub fn rollback(&mut self) {
        let Some(txn) = self.txn.take() else { return };
        let px = !txn.pixels.is_empty();
        if px {
            let labels = self.labels.clone();
            let mut lab = labels.write();
            for &(i, old, _) in txn.pixels.iter().rev() {
                lab.px[i as usize] = old;
            }
        }
        self.restore_bitmaps(&txn.tex, true, txn.heights);
        self.data = txn.before;
        self.refresh(px);
    }

    pub fn can_undo(&self) -> Option<&str> {
        self.undo.last().map(|s| s.label.as_str())
    }
    pub fn can_redo(&self) -> Option<&str> {
        self.redo.last().map(|s| s.label.as_str())
    }

    pub fn undo(&mut self) -> Option<String> {
        if self.txn.is_some() {
            return None;
        }
        let mut step = self.undo.pop()?;
        if !step.pixels.is_empty() {
            let labels = self.labels.clone();
            let mut lab = labels.write();
            for &(i, old, _) in step.pixels.iter().rev() {
                lab.px[i as usize] = old;
            }
        }
        step.heights = self.restore_bitmaps(&step.tex, true, step.heights.take());
        std::mem::swap(&mut self.data, &mut step.data);
        self.refresh(!step.pixels.is_empty());
        let label = step.label.clone();
        self.redo.push(step);
        self.dirty = true;
        Some(label)
    }

    pub fn redo(&mut self) -> Option<String> {
        if self.txn.is_some() {
            return None;
        }
        let mut step = self.redo.pop()?;
        if !step.pixels.is_empty() {
            let labels = self.labels.clone();
            let mut lab = labels.write();
            for &(i, _, new) in &step.pixels {
                lab.px[i as usize] = new;
            }
        }
        step.heights = self.restore_bitmaps(&step.tex, false, step.heights.take());
        std::mem::swap(&mut self.data, &mut step.data);
        self.refresh(!step.pixels.is_empty());
        let label = step.label.clone();
        self.undo.push(step);
        self.dirty = true;
        Some(label)
    }

    /// Impassable borders after a pixel edit: where a province of a wall gave
    /// pixels to another (divide, generate, merge, paint), the new contacts
    /// along the wall stay impassable; walls between provinces that no longer
    /// touch go. Contacts that existed before keep what they were. Returns
    /// whether anything changed.
    fn carry_walls(&mut self, before: &MapData, touched_before: &rustc_hash::FxHashMap<(Id, Id), u32>, pixels: &[(u32, Id, Id)]) -> bool {
        let walls: Vec<(Id, Id)> = before.adjacencies.iter().filter(|a| a.kind == "impassable").map(|a| (a.from, a.to)).collect();
        if walls.is_empty() {
            return false;
        }
        // who has the pixels each province had before the edit
        let mut heirs: rustc_hash::FxHashMap<Id, rustc_hash::FxHashSet<Id>> = Default::default();
        {
            let lab = self.labels.read();
            let mut seen = rustc_hash::FxHashSet::default();
            for &(i, old, _) in pixels {
                if seen.insert(i) {
                    let now = lab.px[i as usize];
                    if now != old {
                        heirs.entry(old).or_default().insert(now);
                    }
                }
            }
        }
        let d = &self.data;
        let alive = |p: Id| d.provinces.contains(p);
        let wall = |a: Id, b: Id| d.adjacencies.iter().any(|x| x.kind == "impassable" && ((x.from, x.to) == (a, b) || (x.from, x.to) == (b, a)));
        let mut add: Vec<(Id, Id)> = Vec::new();
        for &(a, b) in &walls {
            let side = |p: Id| -> Vec<Id> { std::iter::once(p).chain(heirs.get(&p).into_iter().flatten().copied()).filter(|&q| alive(q)).collect() };
            for x in side(a) {
                for y in side(b) {
                    let k = (x.min(y), x.max(y));
                    if x != y && self.stats.border_len(x, y) > 0 && !touched_before.contains_key(&k) && !wall(x, y) && !add.contains(&k) {
                        add.push(k);
                    }
                }
            }
        }
        let n0 = self.data.adjacencies.len();
        let stats = &self.stats;
        self.data.adjacencies.retain(|a| a.kind != "impassable" || stats.border_len(a.from, a.to) > 0);
        let mut changed = self.data.adjacencies.len() != n0;
        for (x, y) in add {
            changed |= self.add_adjacency(x, y, "impassable", "", "").is_ok();
        }
        changed
    }

    /// Put the painted terrain back (`back`: as before the logged changes,
    /// else as after them) and set the imported heightmap; returns the one it
    /// replaced.
    fn restore_bitmaps(&mut self, tex: &[(u32, u8, u8)], back: bool, heights: Option<Arc<Vec<u8>>>) -> Option<Arc<Vec<u8>>> {
        if !tex.is_empty() {
            let mut p = self.paint.write();
            if back {
                for &(i, old, _) in tex.iter().rev() {
                    p[i as usize] = old;
                }
            } else {
                for &(i, _, new) in tex {
                    p[i as usize] = new;
                }
            }
        }
        let changed = !tex.is_empty() || !opt_ptr_eq(&heights, &self.heights);
        if changed {
            self.bitmap_version += 1;
        }
        std::mem::replace(&mut self.heights, heights)
    }

    /// Run `f` with write access to the painted terrain (inside a
    /// transaction): `f` gets the pixels and a log to push (pixel, old, new)
    /// to for every change it makes.
    pub fn with_paint<R>(&mut self, f: impl FnOnce(&mut [u8], &mut Vec<(u32, u8, u8)>) -> R) -> R {
        let paint = self.paint.clone();
        let mut p = paint.write();
        let txn = self.txn.as_mut().expect("terrain edit outside a transaction");
        let r = f(&mut p, &mut txn.tex);
        self.bitmap_version += 1;
        r
    }

    /// Use `h` as the heightmap (None: the build's), inside a transaction.
    pub fn set_heights(&mut self, h: Option<Arc<Vec<u8>>>) {
        assert!(self.txn.is_some(), "heightmap edit outside a transaction");
        self.heights = h;
        self.bitmap_version += 1;
    }

    /// Run `f` with write access to the pixels (inside a transaction).
    pub fn with_pixels<R>(&mut self, f: impl FnOnce(&mut PixelWriter, &MapData) -> R) -> R {
        let labels = self.labels.clone();
        let mut lab = labels.write();
        let txn = self.txn.as_mut().expect("pixel edit outside a transaction");
        let mut wr = PixelWriter { lab: &mut lab, log: &mut txn.pixels };
        let r = f(&mut wr, &self.data);
        self.pixel_version += 1;
        r
    }

    /// Replace the whole map with another of the same size (a backup), inside
    /// a transaction: undo brings the current one back. Returns the pixels
    /// that changed.
    pub fn replace_map(&mut self, l: model::Loaded) -> anyhow::Result<usize> {
        anyhow::ensure!((l.labels.w, l.labels.h) == (self.w, self.h), "it is {}×{} px, the map {}×{}", l.labels.w, l.labels.h, self.w, self.h);
        let n = self.with_pixels(|wr, _| {
            let mut n = 0;
            for (i, &v) in l.labels.px.iter().enumerate() {
                if wr.get(i) != v {
                    wr.set(i, v);
                    n += 1;
                }
            }
            n
        });
        self.data = l.data;
        if let Some(t) = self.txn.as_mut() {
            t.whole = true;
        }
        // its painted terrain and heightmap too
        let paint = l.extra.paint.unwrap_or_else(|| vec![bitmaps::AUTO; (self.w * self.h) as usize]);
        self.with_paint(|p, log| {
            for (i, (cur, &new)) in p.iter_mut().zip(&paint).enumerate() {
                if *cur != new {
                    log.push((i as u32, *cur, new));
                    *cur = new;
                }
            }
        });
        self.set_heights(l.extra.heights.map(Arc::new));
        Ok(n)
    }

    /// Statistics of the pixels as they are now, for an operation that goes on
    /// working on pixels it changed. The contacts from before the transaction
    /// stay for `commit`, which compares them with those after.
    pub fn restat(&mut self) {
        let before = std::mem::take(&mut self.stats.border);
        self.stats = model::compute_stats(&self.labels.read(), self.data.provinces.cap());
        if self.txn.is_some() {
            self.stats.border = before;
        }
    }

    /// Statistics of the pixels after they changed.
    fn restat_pixels(&mut self) {
        self.stats = model::compute_stats(&self.labels.read(), self.data.provinces.cap());
        self.pixel_version += 1;
    }

    /// Statistics, clean-up and member lists after a change.
    pub fn refresh(&mut self, pixels_changed: bool) {
        if pixels_changed {
            self.restat_pixels();
        }
        let cap = self.data.provinces.cap();
        if self.stats.prov.len() < cap {
            self.stats.prov.resize(cap, PStat::default());
            self.stats.nb.resize(cap, Vec::new());
        }
        self.cleanup();
        self.index = Index::build(&self.data);
        self.meta_version += 1;
    }

    /// Remove what became empty, fix dangling references.
    fn cleanup(&mut self) {
        let d = &mut self.data;
        // provinces without pixels
        let dead: Vec<Id> = d.provinces.iter().filter(|(id, _)| self.stats.get(*id).px == 0).map(|(id, _)| id).collect();
        for p in &dead {
            d.provinces.remove(*p);
        }
        let states_live: Vec<bool> = (0..d.states.cap()).map(|s| d.states.contains(s as Id)).collect();
        let regions_live: Vec<bool> = (0..d.regions.cap()).map(|r| d.regions.contains(r as Id)).collect();
        let mut state_used = vec![false; d.states.cap()];
        let mut region_used = vec![false; d.regions.cap()];
        for (_, p) in d.provinces.iter_mut() {
            if p.kind != Kind::Land || !states_live.get(p.state as usize).copied().unwrap_or(false) {
                p.state = 0;
            }
            if !regions_live.get(p.region as usize).copied().unwrap_or(false) {
                p.region = 0;
            }
            state_used[p.state as usize] = true;
            region_used[p.region as usize] = true;
        }
        for s in d.states.ids() {
            if !state_used[s as usize] {
                d.states.remove(s);
            }
        }
        for r in d.regions.ids() {
            if !region_used[r as usize] {
                d.regions.remove(r);
            }
        }
        // nations: drop those without states, fix capitals
        let mut nation_states: Vec<Vec<Id>> = vec![Vec::new(); d.nations.cap()];
        for (sid, s) in d.states.iter_mut() {
            match s.nation {
                Some(n) if (n as usize) < nation_states.len() && d.nations.contains(n) => nation_states[n as usize].push(sid),
                _ => s.nation = None,
            }
            s.victory_points.retain(|v| d.provinces.get(v[0]).is_some_and(|p| p.state == sid));
        }
        let mut state_px = vec![0u64; d.states.cap()];
        for (pid, p) in d.provinces.iter() {
            if p.state != 0 {
                state_px[p.state as usize] += self.stats.get(pid).px as u64;
            }
        }
        for n in d.nations.ids() {
            let own = &nation_states[n as usize];
            if own.is_empty() {
                d.nations.remove(n);
                continue;
            }
            let nat = d.nations.get(n).unwrap();
            let cap_ok = own.contains(&nat.capital_state) && !d.states.get(nat.capital_state).is_some_and(|s| s.impassable);
            if !cap_ok {
                let best = own
                    .iter()
                    .copied()
                    .max_by_key(|&s| (!d.states.get(s).is_some_and(|x| x.impassable), state_px[s as usize]))
                    .unwrap();
                d.nations.get_mut(n).unwrap().capital_state = best;
            }
        }
        // cores and occupiers: nations that are still there; held by its owner is not occupied
        let nations = &d.nations;
        for (_, s) in d.states.iter_mut() {
            if let Some(c) = &mut s.cores {
                let mut seen = Vec::new();
                c.retain(|n| nations.contains(*n) && !seen.contains(n) && {
                    seen.push(*n);
                    true
                });
            }
            if s.controller.is_some_and(|c| !nations.contains(c) || Some(c) == s.nation) {
                s.controller = None;
            }
        }
        // adjacencies
        let provinces = &d.provinces;
        d.adjacencies.retain(|a| a.from != a.to && provinces.contains(a.from) && provinces.contains(a.to));
        for a in &mut d.adjacencies {
            if a.through.is_some_and(|t| !provinces.contains(t)) {
                a.through = None;
            }
        }
    }

    pub fn save(&mut self) -> Result<SaveReport> {
        if self.txn.is_some() {
            self.commit();
        }
        let r = model::save(&self.dir, &self.data, &self.labels.read(), &self.stats, &self.paint.read(), self.heights.as_deref().map(|v| v.as_slice()))?;
        self.dirty = false;
        Ok(r)
    }

    // ------------------------------------------------------------ helpers

    /// Pixel indices of a province (scans its bounding box).
    pub fn pixels_of(&self, lab: &Labels, p: Id) -> Vec<usize> {
        let s = self.stats.get(p);
        let mut out = Vec::with_capacity(s.px as usize);
        if s.px == 0 {
            return out;
        }
        let w = lab.w as usize;
        for y in s.y0 as usize..=s.y1 as usize {
            let row = y * w;
            for x in s.x0 as usize..=s.x1 as usize {
                if lab.px[row + x] == p {
                    out.push(row + x);
                }
            }
        }
        out
    }

    /// Province density map for `p` (cached).
    pub fn density(&mut self, p: &DensityParams) -> Option<Arc<Vec<f32>>> {
        let t = self.terrain.as_ref()?;
        if let Some((cp, d)) = &self.density_cache {
            if cp == p {
                return Some(d.clone());
            }
        }
        let d = Arc::new(density::density(t, p));
        self.density_cache = Some((p.clone(), d.clone()));
        Some(d)
    }

    /// Sum of a per-pixel value over the pixels of each province.
    pub fn sum_per_province(&self, v: &[f32]) -> Vec<f64> {
        let lab = self.labels.read();
        let mut out = vec![0f64; self.data.provinces.cap()];
        for (i, &p) in lab.px.iter().enumerate() {
            if let Some(o) = out.get_mut(p as usize) {
                *o += v[i] as f64;
            }
        }
        out
    }

    /// Built-up (sealed) km² per province, if the terrain is loaded.
    pub fn sealed_km2(&self) -> Option<Vec<f64>> {
        let t = self.terrain.as_ref()?;
        let lab = self.labels.read();
        let mut out = vec![0f64; self.data.provinces.cap()];
        let k = self.data.km2_per_px();
        for (i, &p) in lab.px.iter().enumerate() {
            if t.sealed[i] {
                if let Some(o) = out.get_mut(p as usize) {
                    *o += k;
                }
            }
        }
        Some(out)
    }
}
