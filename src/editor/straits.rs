//! Straits whose land or sea was re-cut. A strait links two land provinces
//! across a sea province; re-cutting the land at its end ("Generate by
//! terrain", divide) keeps the province's id on the piece that overlaps it
//! most, which may lie inland, and the strait then points at a province that
//! no longer touches the water. Here a strait goes back to the land on both
//! shores at its crossing points: automatically after an edit that moved the
//! pixels of its provinces, and for every broken strait with "Fix straits".

use anyhow::{Result, anyhow, bail};
use rustc_hash::{FxHashMap, FxHashSet};

use super::doc::Doc;
use super::model::{Adjacency, Id, Kind};

/// Old province -> the provinces that got its pixels.
pub type Heirs = FxHashMap<Id, FxHashSet<Id>>;

/// Who has the pixels each province had before a pixel edit (from the
/// edit's pixel log: pixel, old, new).
pub fn heirs(lab: &[Id], pixels: &[(u32, Id, Id)]) -> Heirs {
    let mut out: Heirs = Default::default();
    let mut seen = FxHashSet::default();
    for &(i, old, _) in pixels {
        if seen.insert(i) {
            let now = lab[i as usize];
            if now != old {
                out.entry(old).or_default().insert(now);
            }
        }
    }
    out
}

/// How far (px) from a crossing point a shore is looked for.
const SEARCH_PX: u32 = 300;

impl Doc {
    /// A province with pixels (also between a pixel edit and the clean-up,
    /// when those that lost all their pixels are still listed).
    fn live(&self, p: Id) -> bool {
        self.data.provinces.contains(p) && self.stats.get(p).px > 0
    }

    /// A canal (as vanilla's Panama, Kiel and Suez): a "sea" adjacency
    /// between two sea provinces across a land province. Straits are
    /// between land provinces.
    pub fn is_canal(&self, a: &Adjacency) -> bool {
        a.kind == "sea" && self.data.kind(a.from) == Kind::Sea && self.data.kind(a.to) == Kind::Sea
    }

    /// A strait is right when its ends are two land provinces that both
    /// touch its sea province.
    pub fn strait_ok(&self, a: &Adjacency) -> bool {
        let d = &self.data;
        let land = |p: Id| self.live(p) && d.kind(p) == Kind::Land;
        a.kind == "sea"
            && a.from != a.to
            && land(a.from)
            && land(a.to)
            && a.through.is_some_and(|t| self.live(t) && d.kind(t) == Kind::Sea && self.stats.border_len(a.from, t) > 0 && self.stats.border_len(a.to, t) > 0)
    }

    /// Up to `n` provinces that `ok` accepts, nearest to a map point first
    /// (by their nearest pixel, searched up to `max` px away).
    pub fn provinces_near(&self, at: [f64; 2], max: u32, n: usize, ok: impl Fn(Id) -> bool) -> Vec<Id> {
        let lab = self.labels.read();
        let (w, h) = (lab.w as i64, lab.h as i64);
        let (cx, cy) = (at[0].floor() as i64, at[1].floor() as i64);
        let mut best: FxHashMap<Id, i64> = FxHashMap::default();
        let visit = |x: i64, y: i64, best: &mut FxHashMap<Id, i64>| {
            if x >= 0 && y >= 0 && x < w && y < h {
                let p = lab.px[(y * w + x) as usize];
                if ok(p) {
                    let d = (x - cx).pow(2) + (y - cy).pow(2);
                    let e = best.entry(p).or_insert(d);
                    *e = (*e).min(d);
                }
            }
        };
        for r in 0..=max as i64 {
            // the ring r away (Chebyshev) is at least r away: stop once the
            // n nearest are nearer than that
            if best.len() >= n {
                let mut d: Vec<i64> = best.values().copied().collect();
                d.sort_unstable();
                if d[n - 1] < r * r {
                    break;
                }
            }
            if r == 0 {
                visit(cx, cy, &mut best);
                continue;
            }
            for x in cx - r..=cx + r {
                visit(x, cy - r, &mut best);
                visit(x, cy + r, &mut best);
            }
            for y in cy - r + 1..cy + r {
                visit(cx - r, y, &mut best);
                visit(cx + r, y, &mut best);
            }
        }
        let mut v: Vec<(i64, Id)> = best.into_iter().map(|(p, d)| (d, p)).collect();
        v.sort_unstable();
        v.into_iter().take(n).map(|x| x.1).collect()
    }

    /// The strait put back on the land on both shores of its sea where it
    /// crosses: an end that is no longer land touching the sea goes to the
    /// land province touching it nearest to the end's crossing point; a sea
    /// province that is gone or no longer touches both shores, to the sea at
    /// the middle of the crossing. With `heirs`, only the pieces an end (or
    /// the sea) was cut into are taken.
    pub fn repair_strait(&self, a: &Adjacency, heirs: Option<&Heirs>) -> Result<Adjacency> {
        let d = &self.data;
        let alive = |p: Id| self.live(p);
        let land = |p: Id| alive(p) && d.kind(p) == Kind::Land;
        let sea = |p: Id| alive(p) && d.kind(p) == Kind::Sea;
        let allowed = |old: Option<Id>, p: Id| match (heirs, old) {
            (None, _) | (_, None) => true,
            (Some(h), Some(o)) => p == o || h.get(&o).is_some_and(|s| s.contains(&p)),
        };
        let point = |pt: Option<[i64; 2]>, p: Id| pt.map(|q| [q[0] as f64 + 0.5, q[1] as f64 + 0.5]).or_else(|| alive(p).then(|| self.stats.get(p).centre()));
        let (Some(sa), Some(ta)) = (point(a.start, a.from), point(a.stop, a.to)) else {
            bail!("it has no crossing points and its provinces are gone");
        };
        let mid = [(sa[0] + ta[0]) / 2.0, (sa[1] + ta[1]) / 2.0];

        // the land on one shore of `t` nearest to the crossing point
        let end = |old: Id, at: [f64; 2], t: Id, not: Id| -> Option<Id> {
            if land(old) && old != not && self.stats.border_len(old, t) > 0 {
                return Some(old);
            }
            let shore: FxHashSet<Id> = self.stats.neighbours(t).iter().copied().filter(|&p| land(p) && p != not && allowed(Some(old), p)).collect();
            if shore.is_empty() {
                return None;
            }
            self.provinces_near(at, SEARCH_PX, 1, |p| shore.contains(&p)).first().copied().or_else(|| {
                // farther than the search: the shore province with the nearest centre
                shore.iter().copied().min_by(|&x, &y| {
                    let dist = |p: Id| {
                        let c = self.stats.get(p).centre();
                        (c[0] - at[0]).powi(2) + (c[1] - at[1]).powi(2)
                    };
                    dist(x).total_cmp(&dist(y)).then(x.cmp(&y))
                })
            })
        };
        // the sea: the strait's own if it still is sea, then the sea nearest
        // to the middle of the crossing
        let mut seas: Vec<Id> = a.through.filter(|&t| sea(t)).into_iter().collect();
        for t in self.provinces_near(mid, SEARCH_PX, 8, |p| sea(p) && allowed(a.through, p)) {
            if !seas.contains(&t) {
                seas.push(t);
            }
        }
        for t in seas {
            let Some(from) = end(a.from, sa, t, 0) else { continue };
            let Some(to) = end(a.to, ta, t, from) else { continue };
            let mut b = a.clone();
            b.through = Some(t);
            if (from, to) != (a.from, a.to) {
                let (s, e) = self.closest_points(from, to).ok_or_else(|| anyhow!("provinces without pixels"))?;
                b.start = Some([s[0] as i64, s[1] as i64]);
                b.stop = Some([e[0] as i64, e[1] as i64]);
                // a name made from the old ends follows them
                let old_name = format!("{} - {}", d.prov_name(a.from), d.prov_name(a.to));
                if a.name.is_empty() || a.name == old_name {
                    b.name = format!("{} - {}", d.prov_name(from), d.prov_name(to));
                }
                (b.from, b.to) = (from, to);
            }
            return Ok(b);
        }
        bail!("no sea province near its crossing has land on both shores")
    }

    /// "Fix straits": every strait that is not right goes back on the land
    /// at its crossing points ([`Self::repair_strait`]). A strait that then
    /// links the same provinces as another one goes. Returns what changed and
    /// what could not be fixed.
    pub fn fix_straits(&mut self) -> (Vec<String>, Vec<String>) {
        let (mut fixed, mut failed) = (Vec::new(), Vec::new());
        let label = |d: &Doc, a: &Adjacency| if a.name.is_empty() { format!("{} - {}", d.data.prov_name(a.from), d.data.prov_name(a.to)) } else { a.name.clone() };
        for i in 0..self.data.adjacencies.len() {
            let a = self.data.adjacencies[i].clone();
            if a.kind != "sea" || self.is_canal(&a) || self.strait_ok(&a) {
                continue;
            }
            match self.repair_strait(&a, None) {
                Ok(b) => {
                    fixed.push(format!("{}: {} - {} across {}", label(self, &a), self.data.prov_name(b.from), self.data.prov_name(b.to), self.data.prov_name(b.through.unwrap_or(0))));
                    self.data.adjacencies[i] = b;
                }
                Err(e) => failed.push(format!("{}: {e}", label(self, &a))),
            }
        }
        let gone = self.drop_double_straits();
        fixed.extend(gone.into_iter().map(|n| format!("{n}: removed, another strait links the same provinces")));
        (fixed, failed)
    }

    /// Remove straits that link the same two provinces as an earlier one;
    /// returns their names.
    fn drop_double_straits(&mut self) -> Vec<String> {
        let mut seen = FxHashSet::default();
        let mut gone = Vec::new();
        self.data.adjacencies.retain(|a| {
            if a.kind != "sea" || seen.insert((a.from.min(a.to), a.from.max(a.to))) {
                true
            } else {
                gone.push(a.name.clone());
                false
            }
        });
        gone
    }

    /// After a pixel edit, before the clean-up (which drops adjacencies
    /// whose province is gone): straits whose provinces gave pixels away and
    /// are no longer right go to the pieces at their crossing points. Returns
    /// whether anything changed.
    pub(super) fn carry_straits(&mut self, pixels: &[(u32, Id, Id)]) -> bool {
        let heirs = heirs(&self.labels.read().px, pixels);
        if heirs.is_empty() {
            return false;
        }
        let mut changed = false;
        for i in 0..self.data.adjacencies.len() {
            let a = &self.data.adjacencies[i];
            let cut = [a.from, a.to].into_iter().chain(a.through).any(|p| heirs.contains_key(&p));
            if a.kind != "sea" || !cut || self.is_canal(a) || self.strait_ok(a) {
                continue;
            }
            let a = a.clone();
            if let Ok(b) = self.repair_strait(&a, Some(&heirs)) {
                self.data.adjacencies[i] = b;
                changed = true;
            }
        }
        if changed {
            self.drop_double_straits();
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::json;

    use super::super::model::{Labels, Loaded, MapData};
    use super::*;

    /// Three rows of `row` (province ids); 1, 2, 4 and 5 are land, 3 and 6
    /// sea; a strait 1 - 2 across 3 from (1, 1) to (3, 1).
    fn map(row: [Id; 5]) -> Doc {
        let mut data = MapData { pixel_m: 100.0, ..Default::default() };
        for id in row.into_iter().collect::<FxHashSet<_>>() {
            let kind = if matches!(id, 3 | 6) { "sea" } else { "land" };
            data.provinces.put(id, serde_json::from_value(json!({"id": id, "rgb": [40 * id, 0, 0], "type": kind, "name": format!("p{id}")})).unwrap());
        }
        data.adjacencies.push(serde_json::from_value(json!({"from": 1, "to": 2, "type": "sea", "through": 3, "start": [1, 1], "stop": [3, 1], "name": "p1 - p2"})).unwrap());
        let geo = crate::geo::FileGeoInfo { transform: None, crs: None, nodata: None };
        Doc::new(PathBuf::new(), Loaded { data, labels: Labels { w: 5, h: 3, px: row.repeat(3) }, geo, extra: Default::default() })
    }

    /// Give column `x` (all rows) to a new province like `like`.
    fn recut(doc: &mut Doc, x: usize, like: Id) -> Id {
        let kind = doc.data.kind(like);
        let id = doc.new_province(kind, 0, 0, format!("cut of p{like}"), Some(like));
        doc.with_pixels(|wr, _| {
            for y in 0..3 {
                wr.set(y * 5 + x, id);
            }
        });
        id
    }

    fn strait(doc: &Doc) -> (Id, Id, Option<Id>) {
        let a = &doc.data.adjacencies[0];
        (a.from, a.to, a.through)
    }

    #[test]
    fn recut_land_keeps_the_strait_on_the_shore() {
        // the old id stays on the inland piece: the strait goes to the shore piece
        let mut doc = map([1, 1, 3, 2, 2]);
        assert!(doc.strait_ok(&doc.data.adjacencies[0]));
        doc.begin("re-cut");
        let shore = recut(&mut doc, 1, 1);
        doc.commit();
        assert_eq!(strait(&doc), (shore, 2, Some(3)));
        assert!(doc.strait_ok(&doc.data.adjacencies[0]));
        assert_eq!(doc.data.adjacencies[0].name, "cut of p1 - p2", "the name follows its ends");

        // the old province is gone entirely (the clean-up dropped the strait)
        let mut doc = map([1, 1, 3, 2, 2]);
        doc.begin("re-cut");
        let inland = recut(&mut doc, 0, 1);
        let shore = recut(&mut doc, 1, 1);
        doc.commit();
        assert!(!doc.data.provinces.contains(1));
        assert_ne!(inland, shore);
        assert_eq!(doc.data.adjacencies.len(), 1);
        assert_eq!(strait(&doc), (shore, 2, Some(3)));
        doc.undo();
        assert_eq!(strait(&doc), (1, 2, Some(3)), "undo brings the strait back");
    }

    #[test]
    fn recut_sea_moves_the_strait_to_the_new_sea() {
        let mut doc = map([1, 1, 3, 2, 2]);
        doc.begin("re-cut sea");
        let sea = recut(&mut doc, 2, 3);
        doc.commit();
        assert!(!doc.data.provinces.contains(3));
        assert_eq!(strait(&doc), (1, 2, Some(sea)));
        assert!(doc.strait_ok(&doc.data.adjacencies[0]));
    }

    #[test]
    fn fix_straits_repairs_any_broken_one() {
        // an end on land away from the water (as a re-cut before this fix left it)
        let mut doc = map([5, 1, 3, 2, 2]);
        doc.data.adjacencies[0].from = 5;
        assert!(!doc.strait_ok(&doc.data.adjacencies[0]));
        let (fixed, failed) = doc.fix_straits();
        assert_eq!((fixed.len(), failed.len()), (1, 0), "{fixed:?} {failed:?}");
        assert_eq!(strait(&doc), (1, 2, Some(3)));
        // nothing to do now
        assert_eq!(doc.fix_straits(), (vec![], vec![]));

        // no sea province with land on both shores: left as it is
        let mut doc = map([1, 3, 6, 6, 2]);
        doc.data.adjacencies[0].through = Some(6);
        let (fixed, failed) = doc.fix_straits();
        assert!(fixed.is_empty() && failed.len() == 1, "{fixed:?} {failed:?}");
        assert_eq!(strait(&doc), (1, 2, Some(6)));
    }

    #[test]
    fn nearest_provinces_by_pixel() {
        let doc = map([5, 1, 3, 2, 2]);
        assert_eq!(doc.provinces_near([0.5, 1.5], 10, 1, |p| p == 2 || p == 1), vec![1]);
        assert_eq!(doc.provinces_near([4.5, 0.5], 10, 3, |p| p != 3), vec![2, 1, 5]);
        assert!(doc.provinces_near([0.5, 0.5], 10, 1, |p| p == 9).is_empty());
    }
}
