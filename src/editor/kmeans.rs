//! Weighted k-means for cutting pixel sets into provinces and grouping
//! provinces / states into states, nations and strategic regions.

use rayon::prelude::*;

/// SplitMix64: small, seedable, good enough for clustering.
#[derive(Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed ^ 0x9E37_79B9_7F4A_7C15)
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    pub fn f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.f64() * n as f64) as usize % n.max(1)
    }
}

/// Uniform grid over the centres for nearest-centre queries.
struct Grid {
    x0: f32,
    y0: f32,
    cell: f32,
    nx: i32,
    ny: i32,
    start: Vec<u32>,
    items: Vec<u32>,
}

impl Grid {
    fn new(c: &[[f32; 2]]) -> Grid {
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for p in c {
            x0 = x0.min(p[0]);
            y0 = y0.min(p[1]);
            x1 = x1.max(p[0]);
            y1 = y1.max(p[1]);
        }
        let area = ((x1 - x0).max(1.0)) * ((y1 - y0).max(1.0));
        let cell = (area / c.len().max(1) as f32).sqrt().max(1.0);
        let nx = (((x1 - x0) / cell) as i32 + 1).max(1);
        let ny = (((y1 - y0) / cell) as i32 + 1).max(1);
        let cell_of = |p: &[f32; 2]| {
            let cx = (((p[0] - x0) / cell) as i32).clamp(0, nx - 1);
            let cy = (((p[1] - y0) / cell) as i32).clamp(0, ny - 1);
            (cy * nx + cx) as usize
        };
        let mut count = vec![0u32; (nx * ny) as usize + 1];
        for p in c {
            count[cell_of(p) + 1] += 1;
        }
        for i in 1..count.len() {
            count[i] += count[i - 1];
        }
        let mut fill = count.clone();
        let mut items = vec![0u32; c.len()];
        for (i, p) in c.iter().enumerate() {
            let k = cell_of(p);
            items[fill[k] as usize] = i as u32;
            fill[k] += 1;
        }
        Grid { x0, y0, cell, nx, ny, start: count, items }
    }

    fn nearest(&self, c: &[[f32; 2]], p: [f32; 2]) -> u32 {
        let cx = (((p[0] - self.x0) / self.cell).floor() as i32).clamp(0, self.nx - 1);
        let cy = (((p[1] - self.y0) / self.cell).floor() as i32).clamp(0, self.ny - 1);
        let (mut best, mut best_d) = (0u32, f32::MAX);
        let rmax = self.nx.max(self.ny);
        for r in 0..=rmax {
            for gy in cy - r..=cy + r {
                if gy < 0 || gy >= self.ny {
                    continue;
                }
                let step = if gy == cy - r || gy == cy + r { 1 } else { 2 * r.max(1) };
                let mut gx = cx - r;
                while gx <= cx + r {
                    if gx >= 0 && gx < self.nx {
                        let k = (gy * self.nx + gx) as usize;
                        for &i in &self.items[self.start[k] as usize..self.start[k + 1] as usize] {
                            let q = c[i as usize];
                            let d = (q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2);
                            if d < best_d || (d == best_d && i < best) {
                                best_d = d;
                                best = i;
                            }
                        }
                    }
                    gx += step;
                }
            }
            // centres not scanned yet lie in rings beyond r: at least r cells from p
            // (also when p is outside the grid and its cell was clamped)
            if best_d < f32::MAX && best_d.sqrt() <= r as f32 * self.cell {
                break;
            }
        }
        best
    }
}

/// Nearest centre for every point.
pub fn assign(pts: &[[f32; 2]], centres: &[[f32; 2]]) -> Vec<u32> {
    if centres.len() <= 1 {
        return vec![0; pts.len()];
    }
    let g = Grid::new(centres);
    pts.par_iter().map(|&p| g.nearest(centres, p)).collect()
}

/// Weighted k-means++ initialisation and Lloyd iterations.
pub fn kmeans(pts: &[[f32; 2]], w: &[f32], k: usize, iters: usize, rng: &mut Rng) -> Vec<[f32; 2]> {
    let n = pts.len();
    let k = k.clamp(1, n.max(1));
    if n == 0 {
        return Vec::new();
    }
    let wsum: f64 = w.iter().map(|&x| x.max(1e-9) as f64).sum();
    let pick = |rng: &mut Rng, weights: &dyn Fn(usize) -> f64, total: f64| {
        let mut t = rng.f64() * total;
        for i in 0..n {
            t -= weights(i);
            if t <= 0.0 {
                return i;
            }
        }
        n - 1
    };
    let mut centres = vec![pts[pick(rng, &|i| w[i].max(1e-9) as f64, wsum)]];
    let mut d2: Vec<f64> = pts.iter().map(|p| dist2(*p, centres[0])).collect();
    while centres.len() < k {
        let total: f64 = (0..n).map(|i| d2[i] * w[i].max(1e-9) as f64).sum();
        let i = if total > 0.0 { pick(rng, &|i| d2[i] * w[i].max(1e-9) as f64, total) } else { rng.below(n) };
        let c = pts[i];
        centres.push(c);
        for (j, p) in pts.iter().enumerate() {
            d2[j] = d2[j].min(dist2(*p, c));
        }
    }
    for _ in 0..iters {
        let lbl = assign(pts, &centres);
        let mut acc = vec![[0f64; 3]; k];
        for (i, &l) in lbl.iter().enumerate() {
            let ww = w[i].max(1e-9) as f64;
            let a = &mut acc[l as usize];
            a[0] += pts[i][0] as f64 * ww;
            a[1] += pts[i][1] as f64 * ww;
            a[2] += ww;
        }
        let mut moved = 0f64;
        for (c, a) in centres.iter_mut().zip(&acc) {
            if a[2] > 0.0 {
                let nc = [(a[0] / a[2]) as f32, (a[1] / a[2]) as f32];
                moved = moved.max(dist2(*c, nc));
                *c = nc;
            }
        }
        if moved < 0.01 {
            break;
        }
    }
    centres
}

fn dist2(a: [f32; 2], b: [f32; 2]) -> f64 {
    let (dx, dy) = ((a[0] - b[0]) as f64, (a[1] - b[1]) as f64);
    dx * dx + dy * dy
}

/// Size limits of a province, in pixels.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_w: u32,
    pub max_h: u32,
    pub max_px: u32,
    pub min_px: u32,
}

/// Cut pixels into about `k` compact clusters of equal weight, raising k until
/// every cluster fits `lim` (box and size). With `organic` > 0 the borders
/// wind (see [`Warp`]) instead of running straight between the centres.
/// Returns a cluster per pixel.
pub fn split_pixels(pts: &[[u32; 2]], w: &[f32], k: usize, lim: &Limits, seed: u64, organic: f32) -> Vec<u32> {
    let n = pts.len();
    let fits = |lbl: &[u32], k: usize| {
        let mut b = vec![[u32::MAX, u32::MAX, 0u32, 0u32, 0u32]; k];
        for (p, &l) in pts.iter().zip(lbl) {
            let e = &mut b[l as usize];
            e[0] = e[0].min(p[0]);
            e[1] = e[1].min(p[1]);
            e[2] = e[2].max(p[0]);
            e[3] = e[3].max(p[1]);
            e[4] += 1;
        }
        b.iter().all(|e| e[4] == 0 || (e[2] - e[0] < lim.max_w && e[3] - e[1] < lim.max_h && e[4] <= lim.max_px))
    };
    let mut k = k.max(1);
    if k == 1 && fits(&vec![0; n], 1) {
        return vec![0; n];
    }
    let fpts: Vec<[f32; 2]> = pts.iter().map(|p| [p[0] as f32, p[1] as f32]).collect();
    let mut rng = Rng::new(seed);
    // centres from a sample, assignment of all pixels
    const SAMPLE: usize = 24_000;
    let sample: Vec<usize> = if n > SAMPLE { (0..SAMPLE).map(|_| rng.below(n)).collect() } else { (0..n).collect() };
    let spts: Vec<[f32; 2]> = sample.iter().map(|&i| fpts[i]).collect();
    let sw: Vec<f32> = sample.iter().map(|&i| w[i]).collect();
    let kmax = (n / lim.min_px.max(1) as usize).max(1);
    let mut lbl = vec![0; n];
    let mut centres = Vec::new();
    let mut straight_fits = false;
    for _ in 0..40 {
        k = k.min(kmax).max(1);
        centres = kmeans(&spts, &sw, k, 50, &mut rng);
        lbl = assign(&fpts, &centres);
        straight_fits = fits(&lbl, centres.len());
        if straight_fits || k >= kmax {
            break;
        }
        k += (k / 8).max(1);
    }
    if organic <= 0.0 || centres.len() < 2 {
        return lbl;
    }
    // the same centres, but each pixel finds its nearest one from a displaced
    // position: the borders bend. Kept if the pieces still fit (else with a
    // few more pieces; else straight borders).
    let warp = Warp { strength: organic, seed: seed ^ 0x0516_A11C_B0DE_5EED };
    let wsum: f64 = w.iter().map(|&x| x.max(0.0) as f64).sum();
    let top = lim.max_w.max(lim.max_h) as f32;
    for _ in 0..6 {
        let kc = centres.len() as f64;
        // cells are about sqrt(px per cell) across: from the weights where
        // there are any (finer where the density is higher), else even
        let per = wsum / kc;
        let even = (n as f64 / kc).sqrt() as f32;
        let bent: Vec<[f32; 2]> = fpts
            .par_iter()
            .zip(w.par_iter())
            .map(|(&p, &wi)| {
                let s = if per > 0.0 && wi > 0.0 { (per / wi as f64).sqrt() as f32 } else { even };
                warp.apply(p, s.clamp(4.0, top), FINE_PX)
            })
            .collect();
        let l2 = assign(&bent, &centres);
        if !straight_fits || fits(&l2, centres.len()) {
            return l2;
        }
        if k >= kmax {
            break;
        }
        k = (k + (k / 8).max(1)).min(kmax);
        centres = kmeans(&spts, &sw, k, 50, &mut rng);
    }
    lbl
}

/// Smallest bends of province borders (px).
const FINE_PX: f32 = 4.0;

/// Smooth random displacement of points ("domain warp"). Points that pick
/// their nearest centre from a displaced position draw cells whose borders
/// wind like natural ones instead of running straight: bends at every
/// scale from the cell size down, each about as deep as it is long times
/// [`Warp::DEPTH`].
#[derive(Clone, Copy, Debug)]
pub struct Warp {
    /// 0: straight borders; 1: the default bends.
    pub strength: f32,
    pub seed: u64,
}

impl Warp {
    /// Depth of a bend per length at strength 1.
    pub const DEPTH: f32 = 0.22;
    /// Small bends are shallower than large ones by (length ratio)^SMOOTH.
    pub const SMOOTH: f32 = 1.35;

    /// `p` moved, where cells are about `scale` across; bends shorter than
    /// `fine` are left out. Every point of one cut must use the same `fine`.
    pub fn apply(&self, p: [f32; 2], scale: f32, fine: f32) -> [f32; 2] {
        if self.strength <= 0.0 {
            return p;
        }
        let (mut dx, mut dy) = (0f32, 0f32);
        let mut len = fine.max(1.0);
        let mut octave = 0u64;
        // bends up to twice the cell size, fading out above the cell size
        while len < 2.0 * scale {
            let fade = (2.0 - len / scale).clamp(0.0, 1.0);
            let a = Self::DEPTH * self.strength * scale * (len.min(scale) / scale).powf(Self::SMOOTH) * fade;
            let s = self.seed.wrapping_add(octave.wrapping_mul(0x632B_E59B_D9B4_E019));
            let (x, y) = (p[0] / len, p[1] / len);
            dx += a * noise(s, x, y);
            dy += a * noise(s ^ 0xA076_1D64_78BD_642F, x + 0.37, y - 0.61);
            len *= 2.0;
            octave += 1;
        }
        [p[0] + dx, p[1] + dy]
    }
}

/// 16 unit gradient directions.
#[allow(clippy::approx_constant)]
const GRAD: [[f32; 2]; 16] = [
    [1.0, 0.0],
    [0.92388, 0.382683],
    [0.707107, 0.707107],
    [0.382683, 0.92388],
    [0.0, 1.0],
    [-0.382683, 0.92388],
    [-0.707107, 0.707107],
    [-0.92388, 0.382683],
    [-1.0, 0.0],
    [-0.92388, -0.382683],
    [-0.707107, -0.707107],
    [-0.382683, -0.92388],
    [0.0, -1.0],
    [0.382683, -0.92388],
    [0.707107, -0.707107],
    [0.92388, -0.382683],
];

fn lattice(seed: u64, x: i32, y: i32) -> [f32; 2] {
    let mut z = seed ^ (x as u32 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (y as u32 as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    GRAD[((z ^ (z >> 31)) & 15) as usize]
}

/// Gradient (Perlin) noise with features about 1 unit apart, roughly -1..1.
fn noise(seed: u64, x: f32, y: f32) -> f32 {
    let (x0, y0) = (x.floor(), y.floor());
    let (ix, iy) = (x0 as i32, y0 as i32);
    let (fx, fy) = (x - x0, y - y0);
    let dot = |dx: i32, dy: i32| {
        let g = lattice(seed, ix + dx, iy + dy);
        g[0] * (fx - dx as f32) + g[1] * (fy - dy as f32)
    };
    let ease = |t: f32| t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
    let (u, v) = (ease(fx), ease(fy));
    let (a, b) = (dot(0, 0), dot(1, 0));
    let (c, d) = (dot(0, 1), dot(1, 1));
    let top = a + u * (b - a);
    let bottom = c + u * (d - c);
    (top + v * (bottom - top)) * 1.4
}

/// Group nodes (provinces, states) into `k` contiguous groups of about equal
/// weight. `nb` lists neighbours by node index. Returns a group per node.
/// `w` is the area of each node (px); with `organic` > 0 the outlines of the
/// groups wind (see [`Warp`]).
pub fn partition(pos: &[[f32; 2]], w: &[f32], nb: &[Vec<usize>], k: usize, seed: u64, organic: f32) -> Vec<u32> {
    let n = pos.len();
    if n == 0 {
        return Vec::new();
    }
    let k = k.clamp(1, n);
    let mut rng = Rng::new(seed);
    let centres = kmeans(pos, w, k, 60, &mut rng);
    let mut lbl = if organic > 0.0 && k > 1 {
        // groups are about sqrt(area / k) across, nodes sqrt(area / n): bends
        // from the node size up
        let area: f64 = w.iter().map(|&x| x.max(0.0) as f64).sum();
        let scale = (area / k as f64).sqrt() as f32;
        let fine = (area / n as f64).sqrt() as f32;
        let warp = Warp { strength: organic, seed: seed ^ 0x0516_A11C_B0DE_5EED };
        let bent: Vec<[f32; 2]> = pos.iter().map(|&p| warp.apply(p, scale.max(1.0), fine.max(1.0))).collect();
        assign(&bent, &centres)
    } else {
        assign(pos, &centres)
    };
    make_contiguous(&mut lbl, nb, w);
    lbl
}

/// Every group keeps its largest connected part; the other parts join the
/// neighbouring group they share most links with.
pub fn make_contiguous(lbl: &mut [u32], nb: &[Vec<usize>], w: &[f32]) {
    let n = lbl.len();
    for _ in 0..20 {
        // components of equal label
        let mut comp = vec![usize::MAX; n];
        let mut comps: Vec<(u32, f32, Vec<usize>)> = Vec::new();
        for s in 0..n {
            if comp[s] != usize::MAX {
                continue;
            }
            let id = comps.len();
            let mut stack = vec![s];
            comp[s] = id;
            let mut members = Vec::new();
            let mut weight = 0.0;
            while let Some(v) = stack.pop() {
                members.push(v);
                weight += w[v].max(1e-6);
                for &u in &nb[v] {
                    if comp[u] == usize::MAX && lbl[u] == lbl[s] {
                        comp[u] = id;
                        stack.push(u);
                    }
                }
            }
            comps.push((lbl[s], weight, members));
        }
        let mut best: std::collections::HashMap<u32, (f32, usize)> = std::collections::HashMap::new();
        for (i, c) in comps.iter().enumerate() {
            let e = best.entry(c.0).or_insert((c.1, i));
            if c.1 > e.0 {
                *e = (c.1, i);
            }
        }
        let mut changed = false;
        for (i, (l, _, members)) in comps.iter().enumerate() {
            if best[l].1 == i {
                continue;
            }
            let mut links: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
            for &v in members {
                for &u in &nb[v] {
                    if lbl[u] != *l {
                        *links.entry(lbl[u]).or_insert(0) += 1;
                    }
                }
            }
            if let Some((&to, _)) = links.iter().max_by_key(|(k, c)| (**c, std::cmp::Reverse(**k))) {
                for &v in members {
                    lbl[v] = to;
                }
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assign_matches_brute_force() {
        let mut rng = Rng::new(3);
        let centres: Vec<[f32; 2]> = (0..57).map(|_| [rng.f64() as f32 * 900.0, rng.f64() as f32 * 300.0]).collect();
        let pts: Vec<[f32; 2]> = (0..4000).map(|_| [rng.f64() as f32 * 1200.0 - 150.0, rng.f64() as f32 * 500.0 - 100.0]).collect();
        let fast = assign(&pts, &centres);
        for (p, &f) in pts.iter().zip(&fast) {
            let best = (0..centres.len()).min_by(|&a, &b| dist2(*p, centres[a]).total_cmp(&dist2(*p, centres[b]))).unwrap();
            assert!((dist2(*p, centres[best]) - dist2(*p, centres[f as usize])).abs() < 1e-3, "{p:?}");
        }
    }

    #[test]
    fn split_respects_box_limit() {
        let pts: Vec<[u32; 2]> = (0..200).flat_map(|y| (0..300).map(move |x| [x, y])).collect();
        let w = vec![1.0 / pts.len() as f32; pts.len()];
        let lim = Limits { max_w: 100, max_h: 100, max_px: u32::MAX, min_px: 8 };
        let lbl = split_pixels(&pts, &w, 1, &lim, 1, 1.0);
        let k = *lbl.iter().max().unwrap() as usize + 1;
        assert!(k >= 6, "need at least 6 pieces of < 100 px, got {k}");
        for c in 0..k as u32 {
            let xs: Vec<u32> = pts.iter().zip(&lbl).filter(|(_, l)| **l == c).map(|(p, _)| p[0]).collect();
            if !xs.is_empty() {
                assert!(xs.iter().max().unwrap() - xs.iter().min().unwrap() < 100);
            }
        }
    }

    /// Mean distance (px) of border pixels from the straight line through the
    /// border of each pair of clusters (pairs with at least 20 border pixels).
    fn crookedness(lbl: &[u32], w: usize) -> f64 {
        let mut by_pair: std::collections::HashMap<(u32, u32), Vec<[f64; 2]>> = Default::default();
        for i in 0..lbl.len() {
            for j in [i + 1, i + w] {
                if j < lbl.len() && (j != i + 1 || i % w + 1 < w) && lbl[i] != lbl[j] {
                    let key = (lbl[i].min(lbl[j]), lbl[i].max(lbl[j]));
                    by_pair.entry(key).or_default().push([(i % w) as f64, (i / w) as f64]);
                }
            }
        }
        let mut dev = Vec::new();
        for pts in by_pair.values().filter(|v| v.len() >= 20) {
            let n = pts.len() as f64;
            let (mx, my) = (pts.iter().map(|p| p[0]).sum::<f64>() / n, pts.iter().map(|p| p[1]).sum::<f64>() / n);
            let (mut sxx, mut syy, mut sxy) = (0.0, 0.0, 0.0);
            for p in pts {
                let (dx, dy) = (p[0] - mx, p[1] - my);
                sxx += dx * dx;
                syy += dy * dy;
                sxy += dx * dy;
            }
            // distance from the main axis: the smaller eigenvalue of the covariance
            let tr = sxx + syy;
            let det = sxx * syy - sxy * sxy;
            let small = tr / 2.0 - ((tr * tr / 4.0 - det).max(0.0)).sqrt();
            dev.push((small / n).max(0.0).sqrt());
        }
        dev.iter().sum::<f64>() / dev.len().max(1) as f64
    }

    #[test]
    fn organic_borders_wind() {
        let (w, h) = (300u32, 200u32);
        let pts: Vec<[u32; 2]> = (0..h).flat_map(|y| (0..w).map(move |x| [x, y])).collect();
        let wt = vec![30.0 / pts.len() as f32; pts.len()];
        let lim = Limits { max_w: 1000, max_h: 1000, max_px: u32::MAX, min_px: 9 };
        let straight = split_pixels(&pts, &wt, 30, &lim, 3, 0.0);
        let bent = split_pixels(&pts, &wt, 30, &lim, 3, 1.0);
        assert_eq!(bent, split_pixels(&pts, &wt, 30, &lim, 3, 1.0), "same seed, same cut");
        let count = |l: &[u32]| l.iter().collect::<std::collections::HashSet<_>>().len();
        assert_eq!(count(&straight), count(&bent));
        let (a, b) = (crookedness(&straight, w as usize), crookedness(&bent, w as usize));
        assert!(a < 0.5 && b > 2.0 * a.max(0.2), "borders stray from straight lines by {a:.2} px, winding ones by {b:.2} px");
        // the pieces stay about the same size
        let sizes = |l: &[u32]| {
            let mut s = vec![0usize; 30];
            l.iter().for_each(|&c| s[c as usize] += 1);
            s
        };
        let (s0, s1) = (sizes(&straight), sizes(&bent));
        for c in 0..30 {
            assert!((s1[c] as f32 - s0[c] as f32).abs() < s0[c] as f32 * 0.5, "cluster {c}: {} -> {}", s0[c], s1[c]);
        }
    }

    #[test]
    fn warp_is_smooth() {
        let warp = Warp { strength: 1.0, seed: 9 };
        let mut max_move = 0f32;
        for y in 0..60 {
            for x in 0..60 {
                let p = [x as f32 * 3.7, y as f32 * 2.9];
                let (a, b) = (warp.apply(p, 40.0, 4.0), warp.apply([p[0] + 1.0, p[1]], 40.0, 4.0));
                max_move = max_move.max((a[0] - p[0]).hypot(a[1] - p[1]));
                // neighbouring pixels move nearly alike
                assert!((b[0] - a[0] - 1.0).hypot(b[1] - a[1]) < 1.5, "{p:?}");
            }
        }
        assert!(max_move > 2.0 && max_move < 40.0, "{max_move}");
        assert_eq!(Warp { strength: 0.0, seed: 9 }.apply([5.0, 7.0], 40.0, 4.0), [5.0, 7.0]);
    }

    #[test]
    fn partition_is_contiguous() {
        // a 1-D chain of 30 nodes, split in 3: each group must be one run
        let pos: Vec<[f32; 2]> = (0..30).map(|i| [i as f32 * 10.0, if i % 2 == 0 { 0.0 } else { 3.0 }]).collect();
        let nb: Vec<Vec<usize>> = (0..30usize).map(|i| [i.wrapping_sub(1), i + 1].into_iter().filter(|&j| j < 30).collect()).collect();
        let lbl = partition(&pos, &vec![1.0; 30], &nb, 3, 5, 1.0);
        let changes = lbl.windows(2).filter(|w| w[0] != w[1]).count();
        let groups: std::collections::HashSet<u32> = lbl.iter().copied().collect();
        assert_eq!(changes + 1, groups.len(), "{lbl:?}");
    }
}
