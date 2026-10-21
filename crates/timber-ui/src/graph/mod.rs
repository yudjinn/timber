//! The web: a canvas showing the visible part of the tree as a force-directed graph.

pub mod painter;
pub mod render;
pub mod sim;

use std::cell::RefCell;
use std::rc::Rc;

use leptos::html;
use leptos::prelude::*;
use timber_core::{NodeId, format};
use wasm_bindgen::{JsCast, closure::Closure};
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement, PointerEvent, WheelEvent};

use crate::state::{AppState, MAX_ZOOM, MIN_ZOOM};

/// Pointer movement (CSS px) beyond which a press becomes a drag rather than a click.
const DRAG_THRESHOLD: f64 = 4.0;

#[derive(Clone, Copy, Default)]
enum Drag {
    #[default]
    None,
    Node {
        id: NodeId,
        sx0: f64,
        sy0: f64,
        moved: bool,
    },
    Pan {
        sx0: f64,
        sy0: f64,
        cx0: f64,
        cy0: f64,
        moved: bool,
    },
}

#[component]
pub fn GraphView() -> impl IntoView {
    let state = expect_context::<AppState>();
    let graph = state.graph;
    let canvas_ref = NodeRef::<html::Canvas>::new();
    let hovered = StoredValue::new_local(None::<NodeId>);
    let drag = StoredValue::new_local(Drag::None);
    let started = StoredValue::new_local(false);

    // Anything drawn from reactive state forces a redraw.
    Effect::new(move |_| {
        state.selected.track();
        state.popup.track();
        state.dark.track();
        state.doc.track();
        graph.redraw();
    });

    Effect::new(move |_| {
        if let Some(canvas) = canvas_ref.get()
            && !started.get_value()
        {
            started.set_value(true);
            start_loop(canvas, state, hovered);
        }
    });

    let world_at = move |ev: &PointerEvent| {
        let (w, h) = graph.size.get_value();
        let (sx, sy) = (ev.offset_x() as f64, ev.offset_y() as f64);
        let (wx, wy) = graph.cam.with_value(|c| c.to_world(sx, sy, w, h));
        (sx, sy, wx, wy)
    };
    let hit = move |wx: f64, wy: f64| {
        let doc = state.document()?;
        let k = graph.cam.with_value(|c| c.k);
        graph.sim.with_value(|s| s.hit(&doc.tree, wx, wy, 4.0 / k))
    };

    let on_down = move |ev: PointerEvent| {
        if ev.button() != 0 || state.document().is_none() {
            return;
        }
        let (sx, sy, wx, wy) = world_at(&ev);
        if let Some(target) = ev
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
        {
            let _ = target.set_pointer_capture(ev.pointer_id());
        }
        drag.set_value(match hit(wx, wy) {
            Some(id) => Drag::Node {
                id,
                sx0: sx,
                sy0: sy,
                moved: false,
            },
            None => {
                let (cx0, cy0) = graph.cam.with_value(|c| (c.x, c.y));
                Drag::Pan {
                    sx0: sx,
                    sy0: sy,
                    cx0,
                    cy0,
                    moved: false,
                }
            }
        });
    };

    let on_move = move |ev: PointerEvent| {
        let (sx, sy, wx, wy) = world_at(&ev);
        match drag.get_value() {
            Drag::Node {
                id,
                sx0,
                sy0,
                moved,
            } => {
                if moved || (sx - sx0).hypot(sy - sy0) > DRAG_THRESHOLD {
                    drag.set_value(Drag::Node {
                        id,
                        sx0,
                        sy0,
                        moved: true,
                    });
                    graph.sim.update_value(|s| {
                        s.set_pos(id, wx, wy, true);
                        s.alpha_target = 0.3;
                        s.reheat(0.3);
                    });
                    graph.redraw();
                }
            }
            Drag::Pan {
                sx0,
                sy0,
                cx0,
                cy0,
                moved,
            } => {
                if moved || (sx - sx0).hypot(sy - sy0) > DRAG_THRESHOLD {
                    drag.set_value(Drag::Pan {
                        sx0,
                        sy0,
                        cx0,
                        cy0,
                        moved: true,
                    });
                    graph.cam.update_value(|c| {
                        c.target = None;
                        c.x = cx0 - (sx - sx0) / c.k;
                        c.y = cy0 - (sy - sy0) / c.k;
                    });
                    graph.redraw();
                }
            }
            Drag::None => {
                let over = hit(wx, wy);
                if over != hovered.get_value() {
                    hovered.set_value(over);
                    graph.redraw();
                    if let Some(canvas) = canvas_ref.get_untracked() {
                        let _ = web_sys::HtmlElement::style(&canvas).set_property(
                            "cursor",
                            if over.is_some() { "pointer" } else { "grab" },
                        );
                    }
                }
            }
        }
    };

    let on_up = move |_: PointerEvent| {
        match drag.get_value() {
            Drag::Node {
                id, moved: true, ..
            } => graph.sim.update_value(|s| {
                s.release(id);
                s.alpha_target = 0.0;
            }),
            Drag::Node {
                id, moved: false, ..
            } => {
                state.selected.set(Some(id));
                state.popup.set(Some(id));
            }
            Drag::Pan { moved: false, .. } => state.popup.set(None),
            _ => {}
        }
        drag.set_value(Drag::None);
    };

    let on_cancel = move |_: PointerEvent| {
        if let Drag::Node { id, .. } = drag.get_value() {
            graph.sim.update_value(|s| {
                s.release(id);
                s.alpha_target = 0.0;
            });
        }
        drag.set_value(Drag::None);
    };

    let on_leave = move |_: PointerEvent| {
        if hovered.get_value().is_some() {
            hovered.set_value(None);
            graph.redraw();
        }
    };

    // Double-click: collapse a node's visible subtree, or expand all of its children.
    let on_dblclick = move |ev: web_sys::MouseEvent| {
        let Some(doc) = state.document() else { return };
        let (w, h) = graph.size.get_value();
        let (wx, wy) = graph
            .cam
            .with_value(|c| c.to_world(ev.offset_x() as f64, ev.offset_y() as f64, w, h));
        let Some(id) = hit(wx, wy) else { return };
        if graph.sim.with_value(|s| s.visible_children(&doc.tree, id)) > 0 {
            graph.collapse(&doc.tree, id);
        } else {
            let children = doc.tree.node(id).children.clone();
            graph.add_children(&doc.tree, id, &children);
        }
    };

    let on_wheel = move |ev: WheelEvent| {
        ev.prevent_default();
        let scale = match ev.delta_mode() {
            1 => 16.0,  // lines
            2 => 400.0, // pages
            _ => 1.0,
        };
        let factor = (-ev.delta_y() * scale * 0.0015).exp();
        let (w, h) = graph.size.get_value();
        let (sx, sy) = (ev.offset_x() as f64, ev.offset_y() as f64);
        graph.cam.update_value(|c| {
            c.target = None;
            let (wx, wy) = c.to_world(sx, sy, w, h);
            c.k = (c.k * factor).clamp(MIN_ZOOM, MAX_ZOOM);
            c.x = wx - (sx - w / 2.0) / c.k;
            c.y = wy - (sy - h / 2.0) / c.k;
        });
        graph.redraw();
    };

    view! {
        <canvas
            node_ref=canvas_ref
            class="graph-canvas"
            aria-label="Device tree graph"
            on:pointerdown=on_down
            on:pointermove=on_move
            on:pointerup=on_up
            on:pointercancel=on_cancel
            on:pointerleave=on_leave
            on:dblclick=on_dblclick
            on:wheel=on_wheel
        ></canvas>
    }
}

/// The per-frame closure, shared so it can re-schedule itself.
type FrameCallback = Rc<RefCell<Option<Closure<dyn FnMut()>>>>;

fn request_frame(f: &Closure<dyn FnMut()>) {
    if let Some(w) = web_sys::window() {
        let _ = w.request_animation_frame(f.as_ref().unchecked_ref());
    }
}

/// Runs the simulation and redraws on every animation frame that has something to show.
fn start_loop(
    canvas: HtmlCanvasElement,
    state: AppState,
    hovered: StoredValue<Option<NodeId>, LocalStorage>,
) {
    let Some(ctx) = canvas
        .get_context("2d")
        .ok()
        .flatten()
        .and_then(|c| c.dyn_into::<CanvasRenderingContext2d>().ok())
    else {
        return;
    };
    let tick: FrameCallback = Rc::new(RefCell::new(None));
    let next = tick.clone();
    *tick.borrow_mut() = Some(Closure::new(move || {
        frame(&canvas, &ctx, state, hovered);
        if let Some(f) = next.borrow().as_ref() {
            request_frame(f);
        }
    }));
    if let Some(f) = tick.borrow().as_ref() {
        request_frame(f);
    }
}

fn frame(
    canvas: &HtmlCanvasElement,
    ctx: &CanvasRenderingContext2d,
    state: AppState,
    hovered: StoredValue<Option<NodeId>, LocalStorage>,
) {
    let graph = state.graph;
    let dpr = web_sys::window().map_or(1.0, |w| w.device_pixel_ratio());
    let (w, h) = (canvas.client_width() as f64, canvas.client_height() as f64);
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let (bw, bh) = ((w * dpr).round() as u32, (h * dpr).round() as u32);
    if canvas.width() != bw || canvas.height() != bh {
        canvas.set_width(bw);
        canvas.set_height(bh);
        graph.redraw();
    }
    graph.size.set_value((w, h));

    let Some(doc) = state.doc.get_untracked() else {
        if graph.dirty.get_value() {
            graph.dirty.set_value(false);
            let _ = ctx.set_transform(dpr, 0.0, 0.0, dpr, 0.0, 0.0);
            ctx.set_fill_style_str(render::Palette::new(state.dark.get_untracked()).bg);
            ctx.fill_rect(0.0, 0.0, w, h);
        }
        return;
    };

    let moving = graph
        .sim
        .try_update_value(|s| s.tick(&doc.tree))
        .unwrap_or(false);
    let panning = graph.cam.try_update_value(|c| c.step()).unwrap_or(false);
    if !(moving || panning || graph.dirty.get_value()) {
        return;
    }
    graph.dirty.set_value(false);

    let selected = state.selected.get_untracked();
    let popup = state.popup.get_untracked();
    let references: Vec<NodeId> = selected
        .map(|id| {
            doc.tree
                .node(id)
                .props
                .iter()
                .flat_map(|p| format::prop_references(&doc.tree, id, p))
                .collect()
        })
        .unwrap_or_default();
    let pal = render::Palette::new(state.dark.get_untracked());
    let cam = graph.cam.get_value();

    let _ = ctx.set_transform(dpr, 0.0, 0.0, dpr, 0.0, 0.0);
    graph.sim.with_value(|sim| {
        render::draw(
            &mut painter::CanvasPainter::new(ctx),
            &render::Frame {
                tree: &doc.tree,
                kinds: &doc.kinds,
                sim,
                cam: &cam,
                w,
                h,
                selected,
                hovered: hovered.get_value(),
                popup,
                references: &references,
                pal: &pal,
                origin: (0.0, 0.0),
                all_labels: false,
            },
        );
    });

    // Keep the child popup attached to its node.
    let anchor = popup.and_then(|id| {
        let (x, y) = graph.sim.with_value(|s| s.pos(id))?;
        let (sx, sy) = cam.to_screen(x, y, w, h);
        let r = (sim::radius(&doc.tree, id) * cam.k).max(2.5);
        Some((sx, sy, r, w, h))
    });
    let current = state.anchor.get_untracked();
    let moved = match (current, anchor) {
        (Some(a), Some(b)) => {
            (a.0 - b.0).abs() > 0.5 || (a.1 - b.1).abs() > 0.5 || a.3 != b.3 || a.4 != b.4
        }
        (a, b) => a.is_some() != b.is_some(),
    };
    if moved {
        state.anchor.set(anchor);
    }
}
