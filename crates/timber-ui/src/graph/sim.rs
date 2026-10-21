//! Force-directed layout of the visible part of the tree (the "web").
//!
//! Modelled on d3-force: many-body repulsion, parent–child springs, and a weak pull toward
//! the origin, all scaled by a cooling `alpha` so the layout settles when left alone.

use std::collections::HashMap;
use std::f64::consts::{PI, TAU};

use timber_core::{NodeId, Tree};

const ALPHA_MIN: f64 = 0.001;
const ALPHA_DECAY: f64 = 0.0228;
const VELOCITY_DECAY: f64 = 0.4;
const CHARGE: f64 = -380.0;
const GRAVITY: f64 = 0.02;
const MAX_CHARGE_DIST2: f64 = 900.0 * 900.0;

#[derive(Debug, Clone)]
pub struct VNode {
    pub id: NodeId,
    pub x: f64,
    pub y: f64,
    vx: f64,
    vy: f64,
    /// Held in place (being dragged).
    pub fixed: bool,
}

pub struct Sim {
    pub nodes: Vec<VNode>,
    index: HashMap<NodeId, usize>,
    alpha: f64,
    /// Non-zero keeps the simulation warm, e.g. while dragging.
    pub alpha_target: f64,
    rng: u64,
}

impl Default for Sim {
    fn default() -> Self {
        Self::new()
    }
}

/// Node radius in world units; shared by rendering and hit-testing.
pub fn radius(tree: &Tree, id: NodeId) -> f64 {
    if id == Tree::ROOT {
        return 14.0;
    }
    let children = tree.node(id).children.len() as f64;
    6.0 + (1.0 + children).log2().min(4.0) * 1.5
}

impl Sim {
    pub fn new() -> Self {
        Sim {
            nodes: Vec::new(),
            index: HashMap::new(),
            alpha: 0.0,
            alpha_target: 0.0,
            rng: 0x9e37_79b9_7f4a_7c15,
        }
    }

    /// Clears the web down to the root.
    pub fn reset(&mut self) {
        self.nodes.clear();
        self.index.clear();
        self.push(Tree::ROOT, 0.0, 0.0);
        self.reheat(1.0);
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn contains(&self, id: NodeId) -> bool {
        self.index.contains_key(&id)
    }

    pub fn get(&self, id: NodeId) -> Option<&VNode> {
        self.index.get(&id).map(|&i| &self.nodes[i])
    }

    pub fn pos(&self, id: NodeId) -> Option<(f64, f64)> {
        self.get(id).map(|n| (n.x, n.y))
    }

    pub fn set_pos(&mut self, id: NodeId, x: f64, y: f64, fixed: bool) {
        if let Some(&i) = self.index.get(&id) {
            let n = &mut self.nodes[i];
            (n.x, n.y, n.vx, n.vy, n.fixed) = (x, y, 0.0, 0.0, fixed);
        }
    }

    pub fn release(&mut self, id: NodeId) {
        if let Some(&i) = self.index.get(&id) {
            self.nodes[i].fixed = false;
        }
    }

    pub fn reheat(&mut self, alpha: f64) {
        self.alpha = self.alpha.max(alpha);
    }

    pub fn active(&self) -> bool {
        self.alpha >= ALPHA_MIN || self.alpha_target > 0.0
    }

    fn jitter(&mut self) -> f64 {
        // xorshift: deterministic, so layouts are reproducible.
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng % 10_000) as f64 / 10_000.0 - 0.5
    }

    fn push(&mut self, id: NodeId, x: f64, y: f64) {
        self.index.insert(id, self.nodes.len());
        self.nodes.push(VNode {
            id,
            x,
            y,
            vx: 0.0,
            vy: 0.0,
            fixed: false,
        });
    }

    pub fn visible_children(&self, tree: &Tree, id: NodeId) -> usize {
        tree.node(id)
            .children
            .iter()
            .filter(|&&c| self.contains(c))
            .count()
    }

    fn link_distance(tree: &Tree, parent: NodeId, siblings: usize) -> f64 {
        radius(tree, parent) + 50.0 + 9.0 * (siblings as f64).sqrt()
    }

    /// Adds children of a visible `parent`, fanned out away from the grandparent.
    /// Returns how many were added.
    pub fn add_children(&mut self, tree: &Tree, parent: NodeId, ids: &[NodeId]) -> usize {
        let Some((px, py)) = self.pos(parent) else {
            return 0;
        };
        let new: Vec<NodeId> = ids
            .iter()
            .copied()
            .filter(|&id| !self.contains(id) && tree.node(id).parent == Some(parent))
            .collect();
        if new.is_empty() {
            return 0;
        }
        let total = self.visible_children(tree, parent) + new.len();
        let dist = Self::link_distance(tree, parent, total);
        let outward = tree
            .node(parent)
            .parent
            .and_then(|gp| self.pos(gp))
            .map(|(gx, gy)| (py - gy).atan2(px - gx));
        let (start, step) = match outward {
            // Root: a full circle.
            None => (-PI / 2.0, TAU / new.len() as f64),
            // Elsewhere: an arc facing away from the grandparent, wider for more children.
            Some(dir) => {
                let arc = (0.5 * new.len() as f64).min(PI * 1.4);
                let step = if new.len() > 1 {
                    arc / (new.len() - 1) as f64
                } else {
                    0.0
                };
                (dir - arc / 2.0, step)
            }
        };
        for (i, id) in new.iter().enumerate() {
            let a = start + step * i as f64 + self.jitter() * 0.1;
            let d = dist * (1.0 + self.jitter() * 0.1);
            self.push(*id, px + a.cos() * d, py + a.sin() * d);
        }
        self.reheat(0.6);
        new.len()
    }

    /// Makes `id` visible by adding every missing node on the path from the root.
    pub fn reveal(&mut self, tree: &Tree, id: NodeId) {
        let chain = tree.ancestors(id);
        for pair in chain.windows(2) {
            if !self.contains(pair[1]) {
                self.add_children(tree, pair[0], &[pair[1]]);
            }
        }
    }

    /// Removes the visible descendants of `id`, keeping `id`.
    pub fn collapse(&mut self, tree: &Tree, id: NodeId) -> usize {
        self.retain(|n| n == id || !tree.is_ancestor(id, n))
    }

    /// Removes `id` and its visible descendants. The root is collapsed instead.
    pub fn remove_subtree(&mut self, tree: &Tree, id: NodeId) -> usize {
        if id == Tree::ROOT {
            return self.collapse(tree, id);
        }
        self.retain(|n| !tree.is_ancestor(id, n))
    }

    fn retain(&mut self, keep: impl Fn(NodeId) -> bool) -> usize {
        let before = self.nodes.len();
        self.nodes.retain(|n| keep(n.id));
        self.index = self
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.id, i))
            .collect();
        if self.nodes.len() != before {
            self.reheat(0.4);
        }
        before - self.nodes.len()
    }

    /// Visible parent–child links as `(parent index, child index)`.
    pub fn links(&self, tree: &Tree) -> Vec<(usize, usize)> {
        self.nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| {
                tree.node(n.id)
                    .parent
                    .and_then(|p| self.index.get(&p))
                    .map(|&p| (p, i))
            })
            .collect()
    }

    /// Advances the layout one step. Returns false once it has settled.
    pub fn tick(&mut self, tree: &Tree) -> bool {
        if !self.active() {
            return false;
        }
        self.alpha += (self.alpha_target - self.alpha) * ALPHA_DECAY;
        let alpha = self.alpha;
        let n = self.nodes.len();

        let links = self.links(tree);
        let mut degree = vec![0usize; n];
        let mut visible_children = vec![0usize; n];
        for &(p, c) in &links {
            degree[p] += 1;
            degree[c] += 1;
            visible_children[p] += 1;
        }

        // Springs between parent and child (d3 forceLink).
        for &(p, c) in &links {
            let (a, b) = (&self.nodes[p], &self.nodes[c]);
            let mut dx = b.x + b.vx - a.x - a.vx;
            let mut dy = b.y + b.vy - a.y - a.vy;
            if dx == 0.0 && dy == 0.0 {
                dx = 0.01;
                dy = 0.01;
            }
            let dist = (dx * dx + dy * dy).sqrt();
            let want = Self::link_distance(tree, a.id, visible_children[p]);
            let strength = 1.0 / degree[p].min(degree[c]).max(1) as f64;
            let l = (dist - want) / dist * alpha * strength;
            let bias = degree[p] as f64 / (degree[p] + degree[c]) as f64;
            let (fx, fy) = (dx * l, dy * l);
            self.nodes[c].vx -= fx * bias;
            self.nodes[c].vy -= fy * bias;
            self.nodes[p].vx += fx * (1.0 - bias);
            self.nodes[p].vy += fy * (1.0 - bias);
        }

        // Many-body repulsion (O(n²); fine for the hundreds of nodes a web shows).
        for i in 0..n {
            for j in i + 1..n {
                let mut dx = self.nodes[j].x - self.nodes[i].x;
                let mut dy = self.nodes[j].y - self.nodes[i].y;
                let mut d2 = dx * dx + dy * dy;
                if d2 > MAX_CHARGE_DIST2 {
                    continue;
                }
                if d2 < 1.0 {
                    dx = self.jitter();
                    dy = self.jitter();
                    d2 = (dx * dx + dy * dy).max(1.0);
                }
                let w = CHARGE * alpha / d2;
                self.nodes[i].vx += dx * w;
                self.nodes[i].vy += dy * w;
                self.nodes[j].vx -= dx * w;
                self.nodes[j].vy -= dy * w;
            }
        }

        for node in &mut self.nodes {
            if node.fixed {
                node.vx = 0.0;
                node.vy = 0.0;
                continue;
            }
            node.vx -= node.x * GRAVITY * alpha;
            node.vy -= node.y * GRAVITY * alpha;
            node.vx *= 1.0 - VELOCITY_DECAY;
            node.vy *= 1.0 - VELOCITY_DECAY;
            node.x += node.vx;
            node.y += node.vy;
        }
        true
    }

    /// The visible node under a world-space point, if any.
    pub fn hit(&self, tree: &Tree, x: f64, y: f64, slop: f64) -> Option<NodeId> {
        self.nodes
            .iter()
            .rev()
            .map(|n| (n.id, (n.x - x).hypot(n.y - y) - radius(tree, n.id)))
            .filter(|&(_, d)| d <= slop)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(id, _)| id)
    }

    /// World-space bounding box of the visible nodes, padded by their radii.
    pub fn bounds(&self, tree: &Tree) -> Option<(f64, f64, f64, f64)> {
        self.nodes.iter().fold(None, |acc, n| {
            let r = radius(tree, n.id) + 40.0;
            let b = (n.x - r, n.y - r, n.x + r, n.y + r);
            Some(match acc {
                None => b,
                Some((a, c, d, e)) => (
                    f64::min(a, b.0),
                    f64::min(c, b.1),
                    f64::max(d, b.2),
                    f64::max(e, b.3),
                ),
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> Tree {
        timber_core::dts::compile_str(
            "t.dts",
            "/dts-v1/;\n/ { a { a1 {}; a2 { deep {}; }; }; b {}; c {}; };\n",
        )
        .tree
    }

    #[test]
    fn add_reveal_collapse_remove() {
        let t = tree();
        let mut sim = Sim::new();
        sim.reset();
        let root_children = t.node(Tree::ROOT).children.clone();
        assert_eq!(sim.add_children(&t, Tree::ROOT, &root_children), 3);
        assert_eq!(
            sim.add_children(&t, Tree::ROOT, &root_children),
            0,
            "already visible"
        );

        let deep = t.find("/a/a2/deep").unwrap();
        sim.reveal(&t, deep);
        assert!(sim.contains(deep) && sim.contains(t.find("/a/a2").unwrap()));
        assert_eq!(sim.len(), 6);

        let a = t.find("/a").unwrap();
        assert_eq!(sim.collapse(&t, a), 2);
        assert!(sim.contains(a));
        assert_eq!(sim.remove_subtree(&t, a), 1);
        assert_eq!(
            sim.remove_subtree(&t, Tree::ROOT),
            2,
            "root collapses instead"
        );
        assert!(sim.contains(Tree::ROOT));
    }

    #[test]
    fn layout_settles_and_separates_nodes() {
        let t = tree();
        let mut sim = Sim::new();
        sim.reset();
        let kids = t.node(Tree::ROOT).children.clone();
        sim.add_children(&t, Tree::ROOT, &kids);
        let mut steps = 0;
        while sim.tick(&t) {
            steps += 1;
            assert!(steps < 1000, "never settled");
        }
        for i in 0..sim.len() {
            for j in i + 1..sim.len() {
                let (a, b) = (&sim.nodes[i], &sim.nodes[j]);
                assert!((a.x - b.x).hypot(a.y - b.y) > 30.0, "nodes overlap");
            }
        }
        let (x, y) = sim.pos(kids[0]).unwrap();
        assert_eq!(sim.hit(&t, x, y, 0.0), Some(kids[0]));
        assert_eq!(sim.hit(&t, x + 500.0, y, 0.0), None);
    }
}
