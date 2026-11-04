//! Railways: shortest paths between victory points, a minimum spanning tree
//! over neighbouring states, and the rail links chained into lines.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};

#[derive(PartialEq)]
struct Item(f64, usize);
impl Eq for Item {}
impl PartialOrd for Item {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Item {
    fn cmp(&self, o: &Self) -> Ordering {
        o.0.total_cmp(&self.0).then(o.1.cmp(&self.1)) // min-heap
    }
}

pub const NONE: usize = usize::MAX;

/// Distances and predecessors (NONE for the source and unreachable nodes)
/// from `src` over a graph given as adjacency lists of (node, weight).
pub fn dijkstra(adj: &[Vec<(usize, f64)>], src: usize) -> (Vec<f64>, Vec<usize>) {
    let mut dist = vec![f64::INFINITY; adj.len()];
    let mut pred = vec![NONE; adj.len()];
    let mut heap = BinaryHeap::new();
    dist[src] = 0.0;
    heap.push(Item(0.0, src));
    while let Some(Item(d, u)) = heap.pop() {
        if d > dist[u] {
            continue;
        }
        for &(v, w) in &adj[u] {
            let nd = d + w;
            if nd < dist[v] {
                dist[v] = nd;
                pred[v] = u;
                heap.push(Item(nd, v));
            }
        }
    }
    (dist, pred)
}

/// Kruskal's minimum spanning forest of the undirected edges (i, j, weight).
pub fn spanning_tree(n: usize, edges: &[(usize, usize, f64)]) -> Vec<(usize, usize)> {
    let mut order: Vec<usize> = (0..edges.len()).collect();
    order.sort_by(|&a, &b| edges[a].2.total_cmp(&edges[b].2).then(a.cmp(&b)));
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(p: &mut [usize], mut x: usize) -> usize {
        while p[x] != x {
            p[x] = p[p[x]];
            x = p[x];
        }
        x
    }
    let mut out = Vec::new();
    for k in order {
        let (i, j, _) = edges[k];
        let (a, b) = (find(&mut parent, i), find(&mut parent, j));
        if a != b {
            parent[a] = b;
            out.push((i, j));
        }
    }
    out
}

/// Province pairs -> railway lines: maximal runs through provinces with two
/// links; ends and forks first, then loops.
pub fn rail_chains(edges: &BTreeSet<(u32, u32)>) -> Vec<Vec<u32>> {
    let mut adj: BTreeMap<u32, BTreeSet<u32>> = BTreeMap::new();
    for &(a, b) in edges {
        adj.entry(a).or_default().insert(b);
        adj.entry(b).or_default().insert(a);
    }
    let key = |a: u32, b: u32| (a.min(b), a.max(b));
    let mut starts: Vec<u32> = adj.keys().copied().collect();
    starts.sort_by_key(|p| (adj[p].len() == 2, *p));
    let mut used: BTreeSet<(u32, u32)> = BTreeSet::new();
    let mut out = Vec::new();
    for s in starts {
        for &t in &adj[&s] {
            if !used.insert(key(s, t)) {
                continue;
            }
            let mut path = vec![s, t];
            loop {
                let last = *path.last().unwrap();
                if adj[&last].len() != 2 {
                    break;
                }
                let Some(&next) = adj[&last].iter().find(|&&q| !used.contains(&key(last, q))) else { break };
                used.insert(key(last, next));
                path.push(next);
            }
            out.push(path);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortest_paths() {
        // 0 -1- 1 -1- 2, and 0 -5- 2
        let adj = vec![vec![(1, 1.0), (2, 5.0)], vec![(0, 1.0), (2, 1.0)], vec![(1, 1.0), (0, 5.0)]];
        let (d, p) = dijkstra(&adj, 0);
        assert_eq!(d, vec![0.0, 1.0, 2.0]);
        assert_eq!(p[2], 1);
        assert_eq!(p[0], NONE);
    }

    #[test]
    fn tree_takes_cheapest() {
        let t = spanning_tree(3, &[(0, 1, 1.0), (1, 2, 2.0), (0, 2, 1.5)]);
        assert_eq!(t, vec![(0, 1), (0, 2)]);
    }

    #[test]
    fn chains() {
        // a line 1-2-3-4 with a branch 2-5
        let e: BTreeSet<(u32, u32)> = [(1, 2), (2, 3), (3, 4), (2, 5)].into_iter().collect();
        let c = rail_chains(&e);
        assert_eq!(c, vec![vec![1, 2], vec![2, 3, 4], vec![2, 5]]);
    }
}
