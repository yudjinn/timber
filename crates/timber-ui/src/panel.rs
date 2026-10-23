//! The node panel: path, labels, decoded properties, references, and sources.

use std::sync::Arc;

use leptos::prelude::*;
use timber_core::bsp;
use timber_core::diff::{Mark, Token, space_between, value_diff};
use timber_core::format::{Seg, format_prop, format_prop_text};
use timber_core::source_map::LocKind;
use timber_core::source_map::SourceMap;
use timber_core::{NodeId, Tree};

use crate::actions::add_folder;
use crate::backend::BackendHandle;
use crate::doc::Document;
use crate::kind::Kind;
use crate::state::{AppState, SourceLocation};

#[component]
pub fn NodePanel() -> impl IntoView {
    let state = expect_context::<AppState>();
    move || {
        let doc = state.doc.get()?;
        let view = match state.selected.get() {
            Some(id) if doc.is_ghost(id) => ghost_view(state, doc, id).into_any(),
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
    let b = d.b_name();
    let text = match d.marks[id] {
        Mark::Changed(k) => format!("{k} difference{} from {b}", if k == 1 { "" } else { "s" }),
        Mark::OnlyA => format!("not in {b}"),
        Mark::OnlyB => format!("only in {b}"),
        Mark::Same if d.below[id] > 0 => format!("{} differ below", d.below[id]),
        Mark::Same => return None,
    };
    Some(view! { <span class="chip warn">{text}</span> })
}

/// A value's tokens, with the ones that differ from the other side highlighted.
fn tokens_view(tokens: Vec<Token>, present: bool) -> AnyView {
    if !present {
        return view! { <span class="muted">"(not set)"</span> }.into_any();
    }
    if tokens.is_empty() {
        return view! { <span class="bool">"true"</span> }.into_any();
    }
    let mut prev: Option<String> = None;
    let mut out: Vec<AnyView> = Vec::new();
    for t in tokens {
        if prev.as_deref().is_some_and(|p| space_between(p, &t.text)) {
            out.push(" ".into_any());
        }
        prev = Some(t.text.clone());
        out.push(view! { <span class:tok-changed=t.changed>{t.text}</span> }.into_any());
    }
    out.collect_view().into_any()
}

/// A property of this tree and of the one compared with, one above the other.
fn compare_rows(doc: &Document, id: NodeId, name: &str) -> Option<AnyView> {
    let d = doc.diff.as_ref()?;
    let bid = d.b_node(id)?;
    let pa = doc.tree.node(id).props.iter().find(|p| p.name == name);
    let pb = d.b.node(bid).props.iter().find(|p| p.name == name);
    let (ta, tb) = value_diff(&doc.tree, id, pa, &d.b, bid, pb);
    let (a_name, b_name) = (doc.short_name().to_string(), d.against.clone());
    let (a_title, b_title) = (a_name.clone(), b_name.clone());
    Some(
        view! {
            <div class="prop-compare">
                <div class="cmp-side" title=a_title>{a_name}</div>
                <div class="cmp-val">{tokens_view(ta, pa.is_some())}</div>
                <div class="cmp-side" title=b_title>{b_name}</div>
                <div class="cmp-val">{tokens_view(tb, pb.is_some())}</div>
            </div>
        }
        .into_any(),
    )
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

/// What overlays did to a node (`prop: None`) or one of its properties, as source links:
/// overlays on the tree shown, and the "applied at boot" ones on a DTB's source board.
fn overlay_links(state: AppState, doc: &Document, id: NodeId, prop: Option<&str>) -> Vec<AnyView> {
    let source_node = doc
        .link
        .as_ref()
        .and_then(|l| l.map.get(id).copied().flatten());
    let boot = doc
        .link
        .iter()
        .flat_map(|l| &l.marks)
        .filter(|m| Some(m.node) == source_node)
        .map(|m| (m, true));
    let shown = doc
        .overlay_marks
        .iter()
        .filter(|m| m.node == id)
        .map(|m| (m, false));
    let mut seen: Vec<(String, bool)> = Vec::new();
    shown
        .chain(boot)
        .filter(|(m, _)| m.prop.as_deref() == prop)
        // One link per overlay (a node can be merged into by several fragments).
        .filter(|(m, boot)| {
            let key = (m.overlay.clone(), *boot);
            !seen.contains(&key) && {
                seen.push(key);
                true
            }
        })
        .map(|(m, boot)| {
            let verb = match (prop, m.created) {
                (Some(_), _) => "set by",
                (None, true) => "added by",
                (None, false) => "changed by",
            };
            let at = if boot { " (at boot)" } else { "" };
            let line = m.loc.as_ref().map(|l| format!(":{}", l.line)).unwrap_or_default();
            let text = format!("{verb} {}{line}{at}", m.overlay);
            match m.loc.clone() {
                Some(loc) => view! {
                    <button class="src-link-btn" title=format!("{}:{}", loc.path, loc.line) on:click=move |_| state.open_source(loc.clone())>
                        {text}
                    </button>
                }
                .into_any(),
                None => view! { <span class="src-link-btn plain">{text}</span> }.into_any(),
            }
        })
        .collect()
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

    let cmp = doc.diff.as_ref();
    let b_node = cmp.and_then(|d| d.b_node(id));
    let comparison = cmp
        .zip(b_node)
        .map(|(d, bid)| bsp::compare_node(tree, id, &d.b, bid));
    let b_name = cmp.map(|d| d.b_name().to_string()).unwrap_or_default();
    let from_source_cmp = cmp.is_some_and(|d| d.from_source);
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
            let mut links: Vec<AnyView> = doc
                .source_for(id)
                .map(|(sm, sid)| sm.prop(sid, &p.name).iter().map(|l| loc_link(state, sm, l).into_any()).collect())
                .unwrap_or_default();
            links.extend(overlay_links(state, &doc, id, Some(&p.name)));
            let sources = (!links.is_empty()).then_some(links);
            let differs = comparison.as_ref().is_some_and(|c| c.differs.contains(&p.name));
            let only_here = comparison.as_ref().is_some_and(|c| c.only_a.contains(&p.name));
            let bootloader =
                from_source_cmp && (differs || only_here) && bsp::likely_bootloader_patched(&path_str, &p.name);
            let side_by_side = if differs { compare_rows(&doc, id, &p.name) } else { None };
            let value = match side_by_side {
                Some(rows) => rows,
                None => view! { <div class="prop-value">{value}</div> }.into_any(),
            };
            view! {
                <div class="prop" class:differs=differs || only_here>
                    <div class="prop-name">
                        {p.name.clone()}
                        {differs.then(|| view! { <span class="chip warn">{format!("differs from {b_name}")}</span> })}
                        {only_here.then(|| view! { <span class="chip warn">{format!("not in {b_name}")}</span> })}
                        {bootloader.then(|| view! { <span class="chip">"likely set by U-Boot"</span> })}
                    </div>
                    {value}
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

    let mut node_links: Vec<AnyView> = doc
        .source_for(id)
        .map(|(sm, sid)| {
            sm.node(sid)
                .iter()
                .map(|l| loc_link(state, sm, l).into_any())
                .collect()
        })
        .unwrap_or_default();
    node_links.extend(overlay_links(state, &doc, id, None));
    let node_sources = (!node_links.is_empty()).then(|| {
        node_links
            .into_iter()
            .map(|v| view! { <li>{v}</li> })
            .collect_view()
    });
    let added_by = doc
        .overlay_marks
        .iter()
        .find(|m| m.node == id && m.created)
        .map(|m| m.overlay.clone());

    // Properties the other tree sets that this one doesn't have.
    let only_in_b = cmp.zip(b_node).zip(comparison.as_ref()).and_then(|((d, bid), c)| {
        (!c.only_b.is_empty()).then(|| {
            c.only_b
                .iter()
                .filter_map(|name| {
                    let q = d.b.node(bid).props.iter().find(|q| &q.name == name)?;
                    let v = format_prop_text(&d.b, bid, q);
                    Some(view! {
                        <div class="prop differs">
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
                        {added_by.map(|o| view! { <span class="chip label from-source">{format!("added by {o}")}</span> })}
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
            {only_in_b.map(|rows| {
                let (heading, note) = if from_source_cmp {
                    ("Only in source".to_string(), "Set by the source but missing from this DTB.".to_string())
                } else {
                    (format!("Only in {b_name}"), format!("Set in {b_name} but not here."))
                };
                view! {
                    <section>
                        <h3>{heading}</h3>
                        <p class="muted small">{note}</p>
                        <div class="props">{rows}</div>
                    </section>
                }
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

/// A node only in the tree compared with (D-033): its properties come from that tree.
fn ghost_view(state: AppState, doc: Arc<Document>, id: NodeId) -> impl IntoView {
    let d = doc.diff.as_ref().expect("ghosts come from a comparison");
    let bid = d.b_node(id).expect("ghosts map to the other tree");
    let b = &d.b;
    let dark = state.dark.get_untracked();
    let kind = doc.kinds[id];
    let tree = &doc.tree;
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
    let labels = tree
        .node(id)
        .labels
        .iter()
        .map(|l| view! { <span class="chip label">{format!("&{l}")}</span> })
        .collect_view();
    let props = b
        .node(bid)
        .props
        .iter()
        .map(|p| {
            let v = format_prop_text(b, bid, p);
            view! {
                <div class="prop">
                    <div class="prop-name">{p.name.clone()}</div>
                    <div class="prop-value">{if v.is_empty() { "true".to_string() } else { v }}</div>
                </div>
            }
        })
        .collect_view();
    let n_props = b.node(bid).props.len();
    let children = tree.node(id).children.len();
    let where_ = d.b_name().to_string();
    let path = tree.path(id);
    let graph = state.graph;
    let doc2 = doc.clone();
    let doc3 = doc.clone();
    view! {
        <div class="panel-inner">
            <header class="node-head">
                <span class="dot big ghost" style=format!("border-color: {}", kind.color(dark))></span>
                <div>
                    <h2 class="node-title">{tree.node(id).name.clone()}</h2>
                    <div class="node-kind">{kind.label()}{diff_chip(&doc, id)}</div>
                </div>
            </header>
            <nav class="crumbs" aria-label="Path">{crumbs}</nav>
            <div class="chips">{labels}</div>
            <div class="notice warn">{format!("Only in {where_}: this node isn't in the tree shown.")}</div>
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
            <section>
                <h3>{format!("Properties in {where_} ({n_props})")}</h3>
                <div class="props">{props}</div>
            </section>
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
            <p class="muted">{format!("{} nodes", doc.node_count())}</p>
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
