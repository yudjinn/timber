//! Canvas drawing for the web.

use timber_core::{NodeId, Tree};
use wasm_bindgen::JsValue;
use web_sys::CanvasRenderingContext2d;

use super::sim::{Sim, radius};
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
}

fn dash(ctx: &CanvasRenderingContext2d, pattern: &[f64]) {
    let arr = js_sys::Array::new();
    for p in pattern {
        arr.push(&JsValue::from_f64(*p));
    }
    let _ = ctx.set_line_dash(&arr);
}

pub fn draw(ctx: &CanvasRenderingContext2d, f: &Frame) {
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
    ctx.set_fill_style_str(pal.bg);
    ctx.fill_rect(0.0, 0.0, w, h);

    let screen = |id: NodeId| sim.pos(id).map(|(x, y)| cam.to_screen(x, y, w, h));
    let screen_r = |id: NodeId| (radius(tree, id) * cam.k).max(2.5);

    // Parent–child edges.
    ctx.set_stroke_style_str(pal.edge);
    ctx.set_line_width(1.4);
    ctx.begin_path();
    for (p, c) in sim.links(tree) {
        let (a, b) = (&sim.nodes[p], &sim.nodes[c]);
        let (ax, ay) = cam.to_screen(a.x, a.y, w, h);
        let (bx, by) = cam.to_screen(b.x, b.y, w, h);
        ctx.move_to(ax, ay);
        ctx.line_to(bx, by);
    }
    ctx.stroke();

    // References from the selected node: dashed, gently curved.
    if let Some((sx, sy)) = f.selected.and_then(screen) {
        ctx.set_stroke_style_str(pal.reference);
        ctx.set_line_width(1.6);
        dash(ctx, &[5.0, 4.0]);
        for &target in f.references {
            if let Some((tx, ty)) = screen(target) {
                let (mx, my) = ((sx + tx) / 2.0, (sy + ty) / 2.0);
                let (dx, dy) = (tx - sx, ty - sy);
                ctx.begin_path();
                ctx.move_to(sx, sy);
                ctx.quadratic_curve_to(mx - dy * 0.15, my + dx * 0.15, tx, ty);
                ctx.stroke();
            }
        }
        dash(ctx, &[]);
    }

    // Nodes.
    for n in &sim.nodes {
        let (x, y) = cam.to_screen(n.x, n.y, w, h);
        let r = screen_r(n.id);
        if x < -r - 200.0 || y < -r - 50.0 || x > w + r + 200.0 || y > h + r + 50.0 {
            continue;
        }
        let enabled = tree.is_enabled(n.id);
        ctx.set_global_alpha(if enabled { 1.0 } else { 0.38 });
        ctx.begin_path();
        let _ = ctx.arc(x, y, r, 0.0, std::f64::consts::TAU);
        ctx.set_fill_style_str(kinds[n.id].color(pal.dark));
        ctx.fill();
        ctx.set_stroke_style_str(pal.outline);
        ctx.set_line_width(1.0);
        if !enabled {
            dash(ctx, &[2.0, 2.0]);
        }
        ctx.stroke();
        dash(ctx, &[]);
        ctx.set_global_alpha(1.0);

        let ring = |gap: f64, width: f64, pattern: &[f64]| {
            ctx.begin_path();
            let _ = ctx.arc(x, y, r + gap, 0.0, std::f64::consts::TAU);
            ctx.set_stroke_style_str(pal.accent);
            ctx.set_line_width(width);
            dash(ctx, pattern);
            ctx.stroke();
            dash(ctx, &[]);
        };
        if f.selected == Some(n.id) {
            ring(4.0, 2.2, &[]);
        } else if f.popup == Some(n.id) {
            ring(4.0, 1.5, &[3.0, 3.0]);
        } else if f.hovered == Some(n.id) {
            ring(3.0, 1.2, &[]);
        }
    }

    // Labels and hidden-children badges, drawn last so they sit above edges and nodes.
    // Labels are placed in priority order and skipped where they'd collide with one
    // already placed; zooming in reveals them.
    let show_all_labels = cam.k >= 0.45;
    let emphasised = |id: NodeId| [f.selected, f.hovered, f.popup].contains(&Some(id));
    let mut order: Vec<&super::sim::VNode> = sim
        .nodes
        .iter()
        .filter(|n| {
            let (x, y) = cam.to_screen(n.x, n.y, w, h);
            x > -200.0 && y > -50.0 && x < w + 200.0 && y < h + 50.0
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
    ctx.set_text_baseline("alphabetic");
    ctx.set_line_join("round");
    ctx.set_text_align("left");
    for n in order {
        let (x, y) = cam.to_screen(n.x, n.y, w, h);
        let r = screen_r(n.id);
        let strong = emphasised(n.id);

        let hidden = tree.node(n.id).children.len() - sim.visible_children(tree, n.id);
        if hidden > 0 && (cam.k >= 0.35 || strong) {
            let text = format!("+{hidden}");
            ctx.set_font("600 10px system-ui, sans-serif");
            let tw = ctx.measure_text(&text).map(|m| m.width()).unwrap_or(10.0);
            let (bx, by) = (x + r * 0.7 + 2.0, y - r * 0.7 - 7.0);
            ctx.set_fill_style_str(pal.badge_bg);
            ctx.begin_path();
            let _ = ctx.round_rect_with_f64(bx, by, tw + 8.0, 14.0, 7.0);
            ctx.fill();
            ctx.set_fill_style_str(pal.badge_text);
            let _ = ctx.fill_text(&text, bx + 4.0, by + 10.5);
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

        ctx.set_font("600 12px system-ui, sans-serif");
        let bw = ctx.measure_text(base).map(|m| m.width()).unwrap_or(0.0);
        ctx.set_font("11px system-ui, sans-serif");
        let aw = if addr.is_empty() {
            0.0
        } else {
            ctx.measure_text(&addr).map(|m| m.width()).unwrap_or(0.0)
        };
        let left = x - (bw + aw) / 2.0;
        let rect = (left - 2.0, ly - 12.0, left + bw + aw + 2.0, ly + 4.0);
        if !strong && overlaps(&placed, rect) {
            continue;
        }
        placed.push(rect);

        ctx.set_line_width(4.0);
        ctx.set_stroke_style_str(pal.bg);
        ctx.set_font("600 12px system-ui, sans-serif");
        let _ = ctx.stroke_text(base, left, ly);
        ctx.set_fill_style_str(pal.text);
        let _ = ctx.fill_text(base, left, ly);
        if !addr.is_empty() {
            ctx.set_font("11px system-ui, sans-serif");
            let _ = ctx.stroke_text(&addr, left + bw, ly);
            ctx.set_fill_style_str(pal.muted);
            let _ = ctx.fill_text(&addr, left + bw, ly);
        }
    }
}
