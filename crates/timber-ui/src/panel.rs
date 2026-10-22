//! The node panel: path, labels, decoded properties, references, and sources.

use std::sync::Arc;

use leptos::prelude::*;
use timber_core::bsp;
use timber_core::format::{Seg, format_prop, format_prop_text};
use timber_core::source_map::LocKind;
use timber_core::source_map::SourceMap;
use timber_core::{NodeId, Tree};

use crate::actions::add_folder;
use crate::backend::BackendHandle;
use crate::doc::{DiffMark, Document};
use crate::kind::Kind;
use crate::state::{AppState, SourceLocation};

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

fn diff_chip(doc: &Document, id: NodeId) -> impl IntoView + use<> {
    let d = doc.diff.as_ref()?;
    let text = match d.marks[id] {
        DiffMark::Differs(k) => format!(
            "{k} difference{} from source",
            if k == 1 { "" } else { "s" }
        ),
        DiffMark::NotInSource => "not in source".to_string(),
        DiffMark::Same if d.below[id] > 0 => format!("{} differ below", d.below[id]),
        DiffMark::Same => return None,
    };
    Some(view! { <span class="chip warn">{text}</span> })
}

fn loc_text(sm: &SourceMap, kind: LocKind, file: u32, line: u32) -> String {
    let file = sm.files.get(file as usize).map_or("?", String::as_str);
    let short = file.rsplit('/').next().unwrap_or(file);
    let verb = match kind {
        LocKind::Defined => "defined",
        LocKind::Overridden => "overridden",
        LocKind::Deleted => "deleted",
    };
    format!("{verb} {short}:{line}")
}

/// A source location as a link that opens the source viewer.
fn loc_link(
    state: AppState,
    sm: &SourceMap,
    loc: &timber_core::source_map::SourceLoc,
) -> impl IntoView + use<> {
    let text = loc_text(sm, loc.kind, loc.file, loc.line);
    let target = SourceLocation {
        path: sm.files.get(loc.file as usize).cloned().unwrap_or_default(),
        line: loc.line,
        col: loc.col,
    };
    let title = format!("{}:{}", target.path, target.line);
    view! {
        <button class="src-link-btn" title=title on:click=move |_| state.open_source(target.clone())>
            {text}
        </button>
    }
}

/// A banner explaining where source information is (or isn't) coming from.
fn source_notice(state: AppState, doc: &Document, id: NodeId) -> Option<AnyView> {
    let backend = use_context::<BackendHandle>();
    if doc.source_map.is_some() {
        return None;
    }
    if state.sources.with_untracked(|s| s.is_empty()) {
        let backend = backend?;
        return Some(
            view! {
                <div class="notice">
                    <span>"No source information for this DTB."</span>
                    <button class="small" on:click=move |_| add_folder(state, backend.clone())>"Add source folder…"</button>
                </div>
            }
            .into_any(),
        );
    }
    match &doc.link {
        None => Some(
            view! {
                <div class="notice">
                    <span>"No source board matched this DTB."</span>
                    <button class="small" on:click=move |_| state.sources_open.set(true)>"Choose board…"</button>
                </div>
            }
            .into_any(),
        ),
        Some(link) if link.map.get(id).copied().flatten().is_none() && !bsp::is_generated(&doc.tree, id) => {
            let board = link.board.rsplit('/').next().unwrap_or(&link.board).to_string();
            Some(
                view! {
                    <div class="notice warn">
                        {format!("Not in {board}: added at boot (U-Boot, an overlay) or the sources don't match.")}
                    </div>
                }
                .into_any(),
            )
        }
        Some(_) => None,
    }
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

    let link = doc.link.as_ref();
    let source_node = link.and_then(|l| l.map.get(id).copied().flatten());
    let comparison = link
        .zip(source_node)
        .map(|(l, sid)| bsp::compare_node(tree, id, &l.tree, sid));
    let path_str = tree.path(id);

    // DTBs built without -@ borrow labels from the linked source (see Document::relinked).
    let from_source = doc.borrowed_labels.get(id).copied().unwrap_or(false);
    let label_list = node.labels.clone();
    let has_labels = !label_list.is_empty();
    let labels = label_list
        .iter()
        .map(|l| {
            let title = if from_source { "label from source" } else { "" };
            view! { <span class="chip label" class:from-source=from_source title=title>{format!("&{l}")}</span> }
        })
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
                .source_for(id)
                .map(|(sm, sid)| sm.prop(sid, &p.name).iter().map(|l| loc_link(state, sm, l)).collect::<Vec<_>>())
                .filter(|v| !v.is_empty());
            let differs = comparison.as_ref().is_some_and(|c| c.differs.contains(&p.name));
            let only_here = comparison.as_ref().is_some_and(|c| c.only_a.contains(&p.name));
            let source_value = differs
                .then(|| {
                    let (l, sid) = link.zip(source_node)?;
                    let q = l.tree.node(sid).props.iter().find(|q| q.name == p.name)?;
                    Some(format_prop_text(&l.tree, sid, q))
                })
                .flatten();
            let bootloader = (differs || only_here) && bsp::likely_bootloader_patched(&path_str, &p.name);
            view! {
                <div class="prop" class:differs=differs || only_here>
                    <div class="prop-name">
                        {p.name.clone()}
                        {differs.then(|| view! { <span class="chip warn">"differs from source"</span> })}
                        {only_here.then(|| view! { <span class="chip warn">"not in source"</span> })}
                        {bootloader.then(|| view! { <span class="chip">"likely set by U-Boot"</span> })}
                    </div>
                    <div class="prop-value">{value}</div>
                    {source_value.map(|v| view! {
                        <div class="prop-source-value">
                            <span class="muted">"source: "</span>
                            {if v.is_empty() { "true".to_string() } else { v }}
                        </div>
                    })}
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

    let node_sources = doc.source_for(id).map(|(sm, sid)| {
        sm.node(sid)
            .iter()
            .map(|l| view! { <li>{loc_link(state, sm, l)}</li> })
            .collect_view()
    });

    // Properties the source sets that the DTB doesn't have.
    let only_in_source = link.zip(source_node).zip(comparison.as_ref()).and_then(|((l, sid), c)| {
        (!c.only_b.is_empty()).then(|| {
            c.only_b
                .iter()
                .filter_map(|name| {
                    let q = l.tree.node(sid).props.iter().find(|q| &q.name == name)?;
                    let v = format_prop_text(&l.tree, sid, q);
                    Some(view! {
                        <div class="prop">
                            <div class="prop-name">{name.clone()}</div>
                            <div class="prop-value muted">{if v.is_empty() { "true".to_string() } else { v }}</div>
                        </div>
                    })
                })
                .collect_view()
        })
    });
    let notice = source_notice(state, &doc, id);

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
                        {diff_chip(&doc, id)}
                    </div>
                </div>
            </header>
            <nav class="crumbs" aria-label="Path">{crumbs}</nav>
            {has_labels.then(|| view! { <div class="chips">{labels}</div> })}
            {notice}
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
            {only_in_source.map(|rows| view! {
                <section>
                    <h3>"Only in source"</h3>
                    <p class="muted small">"Set by the source but missing from this DTB."</p>
                    <div class="props">{rows}</div>
                </section>
            })}
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
