//! The child popup: opens beside a clicked node, lists its children, and filters as you type.
//!
//! Keys: ↑/↓ move · Enter add and move onto it · Ctrl+Enter add and stay · Ctrl+A add all
//! shown · Backspace (empty filter) go up to the parent · Esc close.

use leptos::html;
use leptos::prelude::*;
use timber_core::NodeId;
use web_sys::KeyboardEvent;

use crate::doc::DiffMark;
use crate::search::{MatchKind, match_node};
use crate::state::AppState;
use crate::util::highlight;

const WIDTH: f64 = 300.0;
const MAX_HEIGHT: f64 = 400.0;

#[derive(Debug, Clone, PartialEq)]
struct Row {
    id: NodeId,
    name: String,
    color: &'static str,
    children: usize,
    enabled: bool,
    in_web: bool,
    /// Highlight positions in `name`, when the name matched.
    name_hit: Vec<usize>,
    /// A label or compatible match, shown under the name.
    other_hit: Option<(MatchKind, String, Vec<usize>)>,
    /// Difference marker: the node's own (`Δ2`, `new`) or how many differ below it.
    diff: Option<(String, bool)>,
}

#[component]
pub fn ChildPopup() -> impl IntoView {
    let state = expect_context::<AppState>();
    let query = RwSignal::new(String::new());
    let cursor = RwSignal::new(0usize);
    let input_ref = NodeRef::<html::Input>::new();
    let list_ref = NodeRef::<html::Ul>::new();

    // A new popup node starts with an empty, focused filter.
    Effect::new(move |prev: Option<Option<NodeId>>| {
        let node = state.popup.get();
        if prev != Some(node) {
            query.set(String::new());
            cursor.set(0);
            if node.is_some() {
                request_animation_frame(move || {
                    if let Some(input) = input_ref.get_untracked() {
                        let _ = input.focus();
                    }
                });
            }
        }
        node
    });

    let rows = Memo::new(move |_| {
        let Some(parent) = state.popup.get() else {
            return Vec::new();
        };
        let Some(doc) = state.doc.get() else {
            return Vec::new();
        };
        let q = query.get().trim().to_lowercase();
        let dark = state.dark.get();
        state.graph.version.track();
        let tree = &doc.tree;
        let mut rows: Vec<_> =
            tree.node(parent)
                .children
                .iter()
                .filter_map(|&id| {
                    let hit = if q.is_empty() {
                        None
                    } else {
                        Some(match_node(tree, id, &q)?)
                    };
                    let node = tree.node(id);
                    let (name_hit, other_hit) = match hit.as_ref() {
                        Some(h) if h.kind == MatchKind::Name => (h.positions.clone(), None),
                        Some(h) => (
                            Vec::new(),
                            Some((h.kind, h.text.clone(), h.positions.clone())),
                        ),
                        None => (Vec::new(), None),
                    };
                    let row = Row {
                        id,
                        name: node.name.clone(),
                        color: doc.kinds[id].color(dark),
                        children: node.children.len(),
                        enabled: tree.is_enabled(id),
                        in_web: state.graph.contains_untracked(id),
                        name_hit,
                        other_hit,
                        diff: doc.diff.as_ref().and_then(|d| match d.marks[id] {
                            DiffMark::Differs(k) => Some((format!("Δ{k}"), true)),
                            DiffMark::NotInSource => Some(("new".to_string(), true)),
                            DiffMark::Same => (d.below[id] > 0)
                                .then(|| (format!("{}Δ below", d.below[id]), false)),
                        }),
                    };
                    Some((hit.map(|h| h.quality), row))
                })
                .collect();
        if !q.is_empty() {
            rows.sort_by_key(|(quality, _)| *quality);
        }
        rows.into_iter().map(|(_, r)| r).collect::<Vec<_>>()
    });

    // Keep the cursor on a row and scrolled into view.
    Effect::new(move |_| {
        let n = rows.with(Vec::len);
        if cursor.get() >= n && n > 0 {
            cursor.set(n - 1);
        }
    });
    Effect::new(move |_| {
        let i = cursor.get();
        if let Some(list) = list_ref.get()
            && let Some(row) = list.children().item(i as u32)
        {
            let opts = web_sys::ScrollIntoViewOptions::new();
            opts.set_block(web_sys::ScrollLogicalPosition::Nearest);
            row.scroll_into_view_with_scroll_into_view_options(&opts);
        }
    });

    let add = move |ids: &[NodeId]| {
        if let (Some(doc), Some(parent)) = (state.document(), state.popup.get_untracked()) {
            state.graph.add_children(&doc.tree, parent, ids);
        }
    };
    let toggle = move |row: &Row| {
        let Some(doc) = state.document() else { return };
        if row.in_web {
            state.graph.remove_subtree(&doc.tree, row.id);
        } else {
            add(&[row.id]);
        }
    };

    let on_key = move |ev: KeyboardEvent| {
        let list = rows.get_untracked();
        let ctrl = ev.ctrl_key() || ev.meta_key();
        match ev.key().as_str() {
            "ArrowDown" => {
                ev.prevent_default();
                cursor.update(|c| *c = (*c + 1).min(list.len().saturating_sub(1)));
            }
            "ArrowUp" => {
                ev.prevent_default();
                cursor.update(|c| *c = c.saturating_sub(1));
            }
            "Enter" => {
                ev.prevent_default();
                if let Some(row) = list.get(cursor.get_untracked()) {
                    if ctrl {
                        add(&[row.id]);
                    } else {
                        state.drill(row.id);
                    }
                }
            }
            "a" | "A" if ctrl => {
                ev.prevent_default();
                add(&list.iter().map(|r| r.id).collect::<Vec<_>>());
            }
            "Backspace" if query.get_untracked().is_empty() => {
                ev.prevent_default();
                let parent = state
                    .popup
                    .get_untracked()
                    .and_then(|p| state.document().and_then(|d| d.tree.node(p).parent));
                if let Some(parent) = parent {
                    state.selected.set(Some(parent));
                    state.popup.set(Some(parent));
                }
            }
            "Escape" => {
                ev.prevent_default();
                state.popup.set(None);
            }
            _ => {}
        }
    };

    let style = move || {
        let Some((x, y, r, w, h)) = state.anchor.get() else {
            return "display: none".to_string();
        };
        let width = WIDTH.min(w - 16.0);
        let mut left = x + r + 14.0;
        if left + width > w - 8.0 {
            left = x - r - 14.0 - width;
        }
        let left = left.clamp(8.0, (w - width - 8.0).max(8.0));
        let top = (y - 28.0).clamp(8.0, (h - MAX_HEIGHT - 8.0).max(8.0));
        format!("left: {left}px; top: {top}px; width: {width}px")
    };

    let header = move || {
        let doc = state.doc.get()?;
        let id = state.popup.get()?;
        state.graph.version.track();
        let node = doc.tree.node(id);
        let name = if id == timber_core::Tree::ROOT {
            "/".to_string()
        } else {
            node.name.clone()
        };
        let total = node.children.len();
        let shown = node
            .children
            .iter()
            .filter(|&&c| state.graph.contains_untracked(c))
            .count();
        Some(view! {
            <div class="popup-head">
                <div class="popup-title" title=doc.tree.path(id)>{name}</div>
                <div class="popup-sub">
                    {format!("{total} {} · {shown} in web", if total == 1 { "child" } else { "children" })}
                </div>
            </div>
        })
    };

    view! {
        <Show when=move || state.popup.get().is_some() && state.doc.with(Option::is_some)>
            <div class="popup" style=style role="dialog" aria-label="Child nodes">
                {header}
                <div class="popup-filter">
                    <input
                        node_ref=input_ref
                        type="text"
                        placeholder="Type to filter children…"
                        aria-label="Filter children"
                        prop:value=move || query.get()
                        on:input=move |ev| {
                            query.set(event_target_value(&ev));
                            cursor.set(0);
                        }
                        on:keydown=on_key
                    />
                    <button
                        class="ghost small"
                        title="Add every child shown (Ctrl+A)"
                        on:click=move |_| add(&rows.get_untracked().iter().map(|r| r.id).collect::<Vec<_>>())
                    >
                        "Add all"
                    </button>
                </div>
                <ul class="popup-list" node_ref=list_ref role="listbox">
                    {move || {
                        let list = rows.get();
                        if list.is_empty() {
                            let msg = if query.with(String::is_empty) { "No child nodes" } else { "No matches" };
                            return view! { <li class="popup-empty">{msg}</li> }.into_any();
                        }
                        list.into_iter()
                            .enumerate()
                            .map(|(i, row)| {
                                let r = row.clone();
                                let r2 = row.clone();
                                let at = row.name.find('@').map(|b| row.name[..b].chars().count());
                                view! {
                                    <li
                                        class="popup-row"
                                        class:active=move || cursor.get() == i
                                        class:in-web=row.in_web
                                        class:disabled=!row.enabled
                                        role="option"
                                        aria-selected=move || (cursor.get() == i).to_string()
                                        on:mouseenter=move |_| cursor.set(i)
                                        on:click=move |_| toggle(&r)
                                        on:dblclick=move |_| state.drill(r2.id)
                                    >
                                        <span class="dot" style=format!("background: {}", row.color)></span>
                                        <span class="row-main">
                                            <span class="row-name">{highlight(&row.name, &row.name_hit, at)}</span>
                                            {row.other_hit.clone().map(|(kind, text, pos)| view! {
                                                <span class="row-match">
                                                    <span class="chip">{kind.chip()}</span>
                                                    {highlight(&text, &pos, None)}
                                                </span>
                                            })}
                                        </span>
                                        {row.diff.clone().map(|(text, own)| view! {
                                            <span class="diff-pill" class:below=!own>{text}</span>
                                        })}
                                        {(row.children > 0).then(|| view! { <span class="row-count">{row.children}</span> })}
                                        <span class="row-check" aria-label=if row.in_web { "in web" } else { "" }>
                                            {if row.in_web { "✓" } else { "" }}
                                        </span>
                                    </li>
                                }
                            })
                            .collect_view()
                            .into_any()
                    }}
                </ul>
                <div class="popup-hint">
                    <kbd>"⏎"</kbd>" open · "<kbd>"Ctrl ⏎"</kbd>" add · "<kbd>"⌫"</kbd>" up · click toggles"
                </div>
            </div>
        </Show>
    }
}
