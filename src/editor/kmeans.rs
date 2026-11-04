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
/// every cluster fits `lim` (box and size). Returns a cluster per pixel.
pub fn split_pixels(pts: &[[u32; 2]], w: &[f32], k: usize, lim: &Limits, seed: u64) -> Vec<u32> {
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
    for _ in 0..40 {
        k = k.min(kmax).max(1);
        let centres = kmeans(&spts, &sw, k, 50, &mut rng);
        lbl = assign(&fpts, &centres);
        if fits(&lbl, centres.len()) || k >= kmax {
            break;
        }
        k += (k / 8).max(1);
    }
    lbl
}

/// Group nodes (provinces, states) into `k` contiguous groups of about equal
/// weight. `nb` lists neighbours by node index. Returns a group per node.
pub fn partition(pos: &[[f32; 2]], w: &[f32], nb: &[Vec<usize>], k: usize, seed: u64) -> Vec<u32> {
    let n = pos.len();
    if n == 0 {
        return Vec::new();
    }
    let k = k.clamp(1, n);
    let mut rng = Rng::new(seed);
    let centres = kmeans(pos, w, k, 60, &mut rng);
    let mut lbl = assign(pos, &centres);
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
        let lbl = split_pixels(&pts, &w, 1, &lim, 1);
        let k = *lbl.iter().max().unwrap() as usize + 1;
        assert!(k >= 6, "need at least 6 pieces of < 100 px, got {k}");
        for c in 0..k as u32 {
            let xs: Vec<u32> = pts.iter().zip(&lbl).filter(|(_, l)| **l == c).map(|(p, _)| p[0]).collect();
            if !xs.is_empty() {
                assert!(xs.iter().max().unwrap() - xs.iter().min().unwrap() < 100);
            }
        }
    }

    #[test]
    fn partition_is_contiguous() {
        // a 1-D chain of 30 nodes, split in 3: each group must be one run
        let pos: Vec<[f32; 2]> = (0..30).map(|i| [i as f32 * 10.0, if i % 2 == 0 { 0.0 } else { 3.0 }]).collect();
        let nb: Vec<Vec<usize>> = (0..30usize).map(|i| [i.wrapping_sub(1), i + 1].into_iter().filter(|&j| j < 30).collect()).collect();
        let lbl = partition(&pos, &vec![1.0; 30], &nb, 3, 5);
        let changes = lbl.windows(2).filter(|w| w[0] != w[1]).count();
        let groups: std::collections::HashSet<u32> = lbl.iter().copied().collect();
        assert_eq!(changes + 1, groups.len(), "{lbl:?}");
    }
}
