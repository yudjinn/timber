//! Canvas drawing for the web.

use timber_core::{NodeId, Tree};

use super::painter::{Font, Painter};
use super::sim::{Sim, radius};
use crate::doc::{DiffInfo, DiffMark};
use crate::kind::Kind;
use crate::state::Camera;

pub struct Palette {
    pub bg: &'static str,
    pub edge: &'static str,
    pub text: &'static str,
    pub muted: &'static str,
    pub accent: &'static str,
    pub reference: &'static str,
    pub outline: &'static str,
    pub badge_bg: &'static str,
    pub badge_text: &'static str,
    /// Differences from source: pill fill, pill text, and ring stroke.
    pub diff_bg: &'static str,
    pub diff_text: &'static str,
    pub diff_stroke: &'static str,
    pub dark: bool,
}

impl Palette {
    /// Mirrors the tokens in style.css.
    pub fn new(dark: bool) -> Self {
        if dark {
            Palette {
                bg: "#161615",
                edge: "rgba(240, 239, 236, 0.22)",
                text: "#f0efec",
                muted: "#a3a29b",
                accent: "#f0efec",
                reference: "#9085e9",
                outline: "rgba(255, 255, 255, 0.28)",
                badge_bg: "#2c2c2a",
                badge_text: "#d6d5d0",
                diff_bg: "#5a4210",
                diff_text: "#f5d27a",
                diff_stroke: "#f0c43a",
                dark,
            }
        } else {
            Palette {
                bg: "#fcfcfb",
                edge: "rgba(11, 11, 11, 0.20)",
                text: "#0b0b0b",
                muted: "#6a6964",
                accent: "#0b0b0b",
                reference: "#6250d6",
                outline: "rgba(0, 0, 0, 0.38)",
                badge_bg: "#ecebe7",
                badge_text: "#3d3c39",
                diff_bg: "#fbe3b0",
                diff_text: "#6b4200",
                diff_stroke: "#c98500",
                dark,
            }
        }
    }
}

pub struct Frame<'a> {
    pub tree: &'a Tree,
    pub kinds: &'a [Kind],
    pub sim: &'a Sim,
    pub cam: &'a Camera,
    pub w: f64,
    pub h: f64,
    pub selected: Option<NodeId>,
    pub hovered: Option<NodeId>,
    pub popup: Option<NodeId>,
    /// Nodes the selected node references (drawn as dashed edges when visible).
    pub references: &'a [NodeId],
    pub pal: &'a Palette,
    /// Where the graph area starts on the painter (exports put a title above it).
    pub origin: (f64, f64),
    /// Label every node regardless of zoom (exports); overlapping labels are still culled.
    pub all_labels: bool,
    /// Differences from the linked source, when there is one.
    pub diff: Option<&'a DiffInfo>,
}

const BADGE_FONT: Font = Font::new(10.0, true);
const NAME_FONT: Font = Font::new(12.0, true);
const ADDR_FONT: Font = Font::new(11.0, false);

pub fn draw(p: &mut dyn Painter, f: &Frame) {
    let Frame {
        tree,
        kinds,
        sim,
        cam,
        w,
        h,
        pal,
        ..
    } = *f;
    let (ox, oy) = f.origin;
    p.rect(ox, oy, w, h, pal.bg);

    let to_screen = |x: f64, y: f64| {
        let (sx, sy) = cam.to_screen(x, y, w, h);
        (sx + ox, sy + oy)
    };
    let screen = |id: NodeId| sim.pos(id).map(|(x, y)| to_screen(x, y));
    let screen_r = |id: NodeId| (radius(tree, id) * cam.k).max(2.5);
    let on_screen = |x: f64, y: f64, margin: f64| {
        x > ox - margin && y > oy - margin && x < ox + w + margin && y < oy + h + margin
    };

    // Parent–child edges.
    let segments: Vec<(f64, f64, f64, f64)> = sim
        .links(tree)
        .into_iter()
        .map(|(a, b)| {
            let (ax, ay) = to_screen(sim.nodes[a].x, sim.nodes[a].y);
            let (bx, by) = to_screen(sim.nodes[b].x, sim.nodes[b].y);
            (ax, ay, bx, by)
        })
        .collect();
    p.lines(&segments, pal.edge, 1.4);

    // References from the selected node: dashed, gently curved.
    if let Some((sx, sy)) = f.selected.and_then(screen) {
        for &target in f.references {
            if let Some((tx, ty)) = screen(target) {
                let (mx, my) = ((sx + tx) / 2.0, (sy + ty) / 2.0);
                let (dx, dy) = (tx - sx, ty - sy);
                p.curve(
                    (sx, sy),
                    (mx - dy * 0.15, my + dx * 0.15),
                    (tx, ty),
                    pal.reference,
                    1.6,
                    &[5.0, 4.0],
                );
            }
        }
    }

    // Nodes.
    for n in &sim.nodes {
        let (x, y) = to_screen(n.x, n.y);
        let r = screen_r(n.id);
        if !on_screen(x, y, r + 200.0) {
            continue;
        }
        let enabled = tree.is_enabled(n.id);
        let dash: &[f64] = if enabled { &[] } else { &[2.0, 2.0] };
        p.circle(
            x,
            y,
            r,
            Some(kinds[n.id].color(pal.dark)),
            Some((pal.outline, 1.0)),
            dash,
            if enabled { 1.0 } else { 0.38 },
        );
        if f.diff
            .is_some_and(|d| d.marks[n.id] == DiffMark::NotInSource)
        {
            p.circle(
                x,
                y,
                r + 2.5,
                None,
                Some((pal.diff_stroke, 1.6)),
                &[3.0, 2.0],
                1.0,
            );
        }
        if f.selected == Some(n.id) {
            p.circle(x, y, r + 4.0, None, Some((pal.accent, 2.2)), &[], 1.0);
        } else if f.popup == Some(n.id) {
            p.circle(
                x,
                y,
                r + 4.0,
                None,
                Some((pal.accent, 1.5)),
                &[3.0, 3.0],
                1.0,
            );
        } else if f.hovered == Some(n.id) {
            p.circle(x, y, r + 3.0, None, Some((pal.accent, 1.2)), &[], 1.0);
        }
    }

    // Labels and hidden-children badges, drawn last so they sit above edges and nodes.
    // Labels are placed in priority order and skipped where they'd collide with one
    // already placed; zooming in reveals them.
    let show_all_labels = f.all_labels || cam.k >= 0.45;
    let emphasised = |id: NodeId| [f.selected, f.hovered, f.popup].contains(&Some(id));
    let mut order: Vec<&super::sim::VNode> = sim
        .nodes
        .iter()
        .filter(|n| {
            let (x, y) = to_screen(n.x, n.y);
            on_screen(x, y, 120.0)
        })
        .collect();
    order.sort_by_key(|n| {
        (
            !emphasised(n.id),
            n.id != Tree::ROOT,
            sim.visible_children(tree, n.id) == 0,
            std::cmp::Reverse(tree.node(n.id).children.len()),
        )
    });
    let mut placed: Vec<(f64, f64, f64, f64)> = Vec::new();
    let overlaps = |placed: &[(f64, f64, f64, f64)], r: (f64, f64, f64, f64)| {
        placed
            .iter()
            .any(|p| r.0 < p.2 && r.2 > p.0 && r.1 < p.3 && r.3 > p.1)
    };
    for n in order {
        let (x, y) = to_screen(n.x, n.y);
        let r = screen_r(n.id);
        let strong = emphasised(n.id);

        let badges = cam.k >= 0.35 || strong;
        let hidden = tree.node(n.id).children.len() - sim.visible_children(tree, n.id);
        if hidden > 0 && badges {
            // Differences hidden below collapsed children turn the badge amber.
            let hidden_diffs: usize = f.diff.map_or(0, |d| {
                tree.node(n.id)
                    .children
                    .iter()
                    .filter(|&&c| !sim.contains(c))
                    .map(|&c| d.in_subtree(c))
                    .sum()
            });
            let (text, bg, fg) = if hidden_diffs > 0 {
                (
                    format!("+{hidden} · {hidden_diffs}Δ"),
                    pal.diff_bg,
                    pal.diff_text,
                )
            } else {
                (format!("+{hidden}"), pal.badge_bg, pal.badge_text)
            };
            let tw = p.measure(&text, BADGE_FONT);
            let (bx, by) = (x + r * 0.7 + 2.0, y - r * 0.7 - 7.0);
            p.rounded_rect(bx, by, tw + 8.0, 14.0, 7.0, bg, None);
            p.text(bx + 4.0, by + 10.5, &text, BADGE_FONT, fg, None);
        }
        let mark = f.diff.map_or(DiffMark::Same, |d| d.marks[n.id]);
        if mark != DiffMark::Same && badges {
            let text = match mark {
                DiffMark::Differs(k) => format!("Δ{k}"),
                _ => "new".to_string(),
            };
            let tw = p.measure(&text, BADGE_FONT);
            let (bx, by) = (x - r * 0.7 - 2.0 - tw - 8.0, y - r * 0.7 - 7.0);
            p.rounded_rect(
                bx,
                by,
                tw + 8.0,
                14.0,
                7.0,
                pal.diff_bg,
                Some((pal.diff_stroke, 1.0)),
            );
            p.text(bx + 4.0, by + 10.5, &text, BADGE_FONT, pal.diff_text, None);
        }

        if !(show_all_labels || strong) {
            continue;
        }
        let node = tree.node(n.id);
        let base = if n.id == Tree::ROOT {
            "/"
        } else {
            node.base_name()
        };
        let addr = node
            .unit_address()
            .map(|a| format!("@{a}"))
            .unwrap_or_default();
        let ly = y + r + 14.0;
        let bw = p.measure(base, NAME_FONT);
        let aw = if addr.is_empty() {
            0.0
        } else {
            p.measure(&addr, ADDR_FONT)
        };
        let left = x - (bw + aw) / 2.0;
        let rect = (left - 2.0, ly - 12.0, left + bw + aw + 2.0, ly + 4.0);
        if !strong && overlaps(&placed, rect) {
            continue;
        }
        placed.push(rect);
        p.text(left, ly, base, NAME_FONT, pal.text, Some((pal.bg, 4.0)));
        if !addr.is_empty() {
            p.text(
                left + bw,
                ly,
                &addr,
                ADDR_FONT,
                pal.muted,
                Some((pal.bg, 4.0)),
            );
        }
    }
}
