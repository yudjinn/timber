//! The node panel: path, labels, decoded properties, references, and sources.

use std::sync::Arc;

use leptos::prelude::*;
use timber_core::format::{Seg, format_prop};
use timber_core::source_map::LocKind;
use timber_core::{NodeId, Tree};

use crate::doc::Document;
use crate::kind::Kind;
use crate::state::AppState;

#[component]
pub fn NodePanel() -> impl IntoView {
    let state = expect_context::<AppState>();
    move || {
        let doc = state.doc.get()?;
        let view = match state.selected.get() {
            Some(id) if id < doc.tree.len() => node_view(state, doc, id).into_any(),
            _ => summary_view(state, doc).into_any(),
        };
        Some(view)
    }
}

fn node_link(state: AppState, doc: &Document, id: NodeId, text: String) -> impl IntoView + use<> {
    let path = doc.tree.path(id);
    view! {
        <button class="link" title=path on:click=move |_| state.go_to(id)>
            {text}
        </button>
    }
}

fn status_chip(doc: &Document, id: NodeId) -> impl IntoView + use<> {
    let status = doc.tree.prop_str(id, "status").map(str::to_string);
    status.map(|s| {
        let class = if doc.tree.is_enabled(id) {
            "chip ok"
        } else {
            "chip off"
        };
        view! { <span class=class>{s}</span> }
    })
}

fn loc_text(doc: &Document, kind: LocKind, file: u32, line: u32) -> String {
    let sm = doc.source_map.as_ref();
    let file = sm
        .and_then(|s| s.files.get(file as usize))
        .map_or("?", String::as_str);
    let short = file.rsplit('/').next().unwrap_or(file);
    let verb = match kind {
        LocKind::Defined => "defined",
        LocKind::Overridden => "overridden",
        LocKind::Deleted => "deleted",
    };
    format!("{verb} {short}:{line}")
}

fn node_view(state: AppState, doc: Arc<Document>, id: NodeId) -> impl IntoView {
    let tree = &doc.tree;
    let node = tree.node(id);
    let dark = state.dark.get_untracked();
    let kind = doc.kinds[id];
    let title = if id == Tree::ROOT {
        "/".to_string()
    } else {
        node.name.clone()
    };

    let crumbs = tree
        .ancestors(id)
        .into_iter()
        .map(|a| {
            let text = if a == Tree::ROOT {
                "/".to_string()
            } else {
                tree.node(a).name.clone()
            };
            let link = node_link(state, &doc, a, text);
            view! { <span class="crumb">{link}</span> }
        })
        .collect_view();

    let labels = node
        .labels
        .iter()
        .map(|l| view! { <span class="chip label">{format!("&{l}")}</span> })
        .collect_view();

    let compatible = tree
        .prop_strings(id, "compatible")
        .into_iter()
        .map(|c| view! { <li>{c.to_string()}</li> })
        .collect_view();

    let props = node
        .props
        .iter()
        .map(|p| {
            let segs = format_prop(tree, id, p);
            let value = if segs.is_empty() {
                view! { <span class="bool">"true"</span> }.into_any()
            } else {
                segs.into_iter()
                    .map(|s| match s {
                        Seg::Text(t) => view! { <span>{t}</span> }.into_any(),
                        Seg::Ref { text, target } => {
                            node_link(state, &doc, target, text).into_any()
                        }
                    })
                    .collect_view()
                    .into_any()
            };
            let sources = doc
                .source_map
                .as_ref()
                .map(|sm| {
                    sm.prop(id, &p.name)
                        .iter()
                        .map(|l| loc_text(&doc, l.kind, l.file, l.line))
                        .collect::<Vec<_>>()
                        .join(" · ")
                })
                .filter(|s| !s.is_empty());
            view! {
                <div class="prop">
                    <div class="prop-name">{p.name.clone()}</div>
                    <div class="prop-value">{value}</div>
                    {sources.map(|s| view! { <div class="prop-src">{s}</div> })}
                </div>
            }
        })
        .collect_view();

    let referenced_by = doc.referenced_by[id]
        .iter()
        .map(|(src, prop)| {
            let text = tree.path(*src);
            let link = node_link(state, &doc, *src, text);
            view! { <li>{link}<span class="muted">{format!(" · {prop}")}</span></li> }
        })
        .collect_view();
    let has_refs = !doc.referenced_by[id].is_empty();

    let node_sources = doc.source_map.as_ref().map(|sm| {
        sm.node(id)
            .iter()
            .map(|l| view! { <li>{loc_text(&doc, l.kind, l.file, l.line)}</li> })
            .collect_view()
    });

    let children = node.children.len();
    let path = tree.path(id);
    let graph = state.graph;
    let doc2 = doc.clone();
    let doc3 = doc.clone();

    view! {
        <div class="panel-inner">
            <header class="node-head">
                <span class="dot big" style=format!("background: {}", kind.color(dark))></span>
                <div>
                    <h2 class="node-title">{title}</h2>
                    <div class="node-kind">
                        {kind.label()}
                        {status_chip(&doc, id)}
                    </div>
                </div>
            </header>
            <nav class="crumbs" aria-label="Path">{crumbs}</nav>
            {(!node.labels.is_empty()).then(|| view! { <div class="chips">{labels}</div> })}
            <div class="actions">
                <button
                    disabled=children == 0
                    on:click=move |_| {
                        let kids = doc2.tree.node(id).children.clone();
                        graph.add_children(&doc2.tree, id, &kids);
                    }
                >
                    {format!("Expand {children}")}
                </button>
                <button on:click=move |_| { graph.collapse(&doc3.tree, id); }>"Collapse"</button>
                <button on:click=move |_| state.go_to(id)>"Focus"</button>
                <button on:click=move |_| copy_text(state, path.clone())>"Copy path"</button>
            </div>
            {(kind != Kind::Root && !tree.prop_strings(id, "compatible").is_empty())
                .then(|| view! {
                    <section>
                        <h3>"Compatible"</h3>
                        <ul class="plain mono">{compatible}</ul>
                    </section>
                })}
            <section>
                <h3>{format!("Properties ({})", node.props.len())}</h3>
                <div class="props">{props}</div>
            </section>
            {has_refs.then(|| view! {
                <section>
                    <h3>"Referenced by"</h3>
                    <ul class="plain mono">{referenced_by}</ul>
                </section>
            })}
            {node_sources.map(|s| view! {
                <section>
                    <h3>"Sources"</h3>
                    <ul class="plain mono">{s}</ul>
                </section>
            })}
        </div>
    }
}

fn summary_view(state: AppState, doc: Arc<Document>) -> impl IntoView {
    let dark = state.dark.get_untracked();
    let mut counts: Vec<(Kind, usize)> = Kind::ALL
        .iter()
        .map(|&k| (k, doc.kinds.iter().filter(|&&x| x == k).count()))
        .filter(|(_, n)| *n > 0)
        .collect();
    counts.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    let rows = counts
        .into_iter()
        .map(|(k, n)| {
            view! {
                <li>
                    <span class="dot" style=format!("background: {}", k.color(dark))></span>
                    {k.label()}
                    <span class="muted">{format!(" {n}")}</span>
                </li>
            }
        })
        .collect_view();
    view! {
        <div class="panel-inner">
            <h2 class="node-title">{doc.name.clone()}</h2>
            <p class="muted">{format!("{} nodes", doc.tree.len())}</p>
            <section>
                <h3>"Node kinds"</h3>
                <ul class="plain">{rows}</ul>
            </section>
        </div>
    }
}

fn copy_text(state: AppState, text: String) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let promise = window.navigator().clipboard().write_text(&text);
    leptos::task::spawn_local(async move {
        match wasm_bindgen_futures::JsFuture::from(promise).await {
            Ok(_) => state.notify(format!("Copied {text}"), false),
            Err(_) => state.notify("Couldn't copy to the clipboard", true),
        }
    });
}
