//! Top bar: global search, file actions, view controls.

use leptos::html;
use leptos::prelude::*;
use timber_core::{NodeId, dtb, format};
use web_sys::KeyboardEvent;

use crate::app::open_files;
use crate::backend::BackendHandle;
use crate::search::{Hit, MatchKind};
use crate::state::AppState;
use crate::util::{file_stem, highlight};

pub const SEARCH_INPUT_ID: &str = "global-search";

#[derive(Debug, Clone, PartialEq)]
struct ResultRow {
    id: NodeId,
    hit: Hit,
    name: String,
    path: String,
    color: &'static str,
}

#[component]
fn GlobalSearch() -> impl IntoView {
    let state = expect_context::<AppState>();
    let query = RwSignal::new(String::new());
    let open = RwSignal::new(false);
    let cursor = RwSignal::new(0usize);
    let input_ref = NodeRef::<html::Input>::new();

    let results = Memo::new(move |_| {
        let q = query.get();
        let Some(doc) = state.doc.get() else {
            return Vec::new();
        };
        let dark = state.dark.get();
        doc.search
            .search(&doc.tree, &q, 40)
            .into_iter()
            .map(|hit| ResultRow {
                id: hit.id,
                name: doc.tree.node(hit.id).name.clone(),
                path: doc.tree.path(hit.id),
                color: doc.kinds[hit.id].color(dark),
                hit,
            })
            .collect::<Vec<_>>()
    });

    let pick = move |id: NodeId| {
        state.go_to(id);
        query.set(String::new());
        open.set(false);
    };

    let on_key = move |ev: KeyboardEvent| {
        let n = results.with_untracked(Vec::len);
        match ev.key().as_str() {
            "ArrowDown" => {
                ev.prevent_default();
                open.set(true);
                cursor.update(|c| *c = (*c + 1).min(n.saturating_sub(1)));
            }
            "ArrowUp" => {
                ev.prevent_default();
                cursor.update(|c| *c = c.saturating_sub(1));
            }
            "Enter" => {
                ev.prevent_default();
                if let Some(row) =
                    results.with_untracked(|r| r.get(cursor.get_untracked()).cloned())
                {
                    pick(row.id);
                }
            }
            "Escape" => {
                query.set(String::new());
                open.set(false);
                if let Some(input) = input_ref.get_untracked() {
                    let _ = input.blur();
                }
            }
            _ => {}
        }
    };

    view! {
        <div class="search">
            <input
                id=SEARCH_INPUT_ID
                node_ref=input_ref
                type="search"
                autocomplete="off"
                spellcheck="false"
                placeholder="Search nodes, labels, aliases, compatibles  /"
                aria-label="Search the tree"
                disabled=move || state.doc.with(Option::is_none)
                prop:value=move || query.get()
                on:input=move |ev| {
                    query.set(event_target_value(&ev));
                    cursor.set(0);
                    open.set(true);
                }
                on:focus=move |_| open.set(true)
                on:blur=move |_| open.set(false)
                on:keydown=on_key
            />
            <Show when=move || open.get() && !query.with(String::is_empty)>
                <ul class="search-results" role="listbox">
                    {move || {
                        let list = results.get();
                        if list.is_empty() {
                            return view! { <li class="popup-empty">"No matches"</li> }.into_any();
                        }
                        list.into_iter()
                            .enumerate()
                            .map(|(i, row)| {
                                let id = row.id;
                                let at = row.name.find('@').map(|b| row.name[..b].chars().count());
                                let name = if row.hit.kind == MatchKind::Name {
                                    highlight(&row.name, &row.hit.positions, at).into_any()
                                } else {
                                    highlight(&row.name, &[], at).into_any()
                                };
                                let other = (row.hit.kind != MatchKind::Name)
                                    .then(|| highlight(&row.hit.text, &row.hit.positions, None));
                                view! {
                                    <li
                                        class="search-row"
                                        class:active=move || cursor.get() == i
                                        role="option"
                                        on:mouseenter=move |_| cursor.set(i)
                                        // mousedown, not click: fires before the input's blur closes the list.
                                        on:mousedown=move |ev| {
                                            ev.prevent_default();
                                            pick(id);
                                        }
                                    >
                                        <span class="dot" style=format!("background: {}", row.color)></span>
                                        <span class="row-main">
                                            <span class="row-name">
                                                {name}
                                                <span class="chip">{row.hit.kind.chip()}</span>
                                                {other}
                                            </span>
                                            <span class="row-path">{row.path.clone()}</span>
                                        </span>
                                    </li>
                                }
                            })
                            .collect_view()
                            .into_any()
                    }}
                </ul>
            </Show>
        </div>
    }
}

#[component]
pub fn Toolbar() -> impl IntoView {
    let state = expect_context::<AppState>();
    let backend = expect_context::<BackendHandle>();
    let has_doc = move || state.doc.with(Option::is_some);
    let desktop = state.capabilities.desktop;
    let backend_store = StoredValue::new(backend.clone());

    let export = move |as_dtb: bool| {
        let Some(doc) = state.document() else { return };
        let backend = backend_store.get_value();
        let stem = file_stem(&doc.name).to_string();
        let (name, mime, data) = if as_dtb {
            (
                format!("{stem}.dtb"),
                "application/octet-stream",
                dtb::write(&doc.tree),
            )
        } else {
            (
                format!("{stem}.dts"),
                "text/plain",
                format::to_dts(&doc.tree).into_bytes(),
            )
        };
        leptos::task::spawn_local(async move {
            if let Err(e) = backend.0.save_file(&name, mime, data).await {
                state.notify(format!("Export failed: {e}"), true);
            }
        });
    };

    let diag_label = move || {
        state.doc.with(|d| {
            let d = d.as_ref()?;
            let (e, w) = (d.error_count(), d.warning_count());
            (e + w > 0).then(|| {
                let text = match (e, w) {
                    (0, w) => format!("{w} warning{}", if w == 1 { "" } else { "s" }),
                    (e, 0) => format!("{e} error{}", if e == 1 { "" } else { "s" }),
                    (e, w) => format!("{e} errors · {w} warnings"),
                };
                (text, e > 0)
            })
        })
    };

    view! {
        <header class="toolbar">
            <div class="brand" aria-hidden="true">"timber"</div>
            <GlobalSearch />
            <div class="toolbar-group">
                <button class="primary" on:click=move |_| open_files(state, backend.clone())>"Open…"</button>
                <button
                    class:unavailable=!desktop
                    aria-disabled=(!desktop).to_string()
                    title=if desktop { "Connect to a device over SSH" } else { "Requires the desktop app" }
                    on:click=move |_| {
                        if !desktop {
                            state.notify(
                                "SSH requires the desktop app: browsers can't open network connections to your devices.",
                                false,
                            );
                        }
                    }
                >
                    "SSH"
                    {(!desktop).then(|| view! { <span class="badge">"Desktop"</span> })}
                </button>
            </div>
            <div class="spacer"></div>
            {move || state.doc.with(|d| d.as_ref().map(|d| {
                let title = format!("{} · {} nodes", d.name, d.tree.len());
                view! {
                    <span class="doc-name" title=title>
                        {d.name.clone()}
                        {d.overlay.then(|| view! { <span class="badge">"overlay"</span> })}
                    </span>
                }
            }))}
            {move || diag_label().map(|(text, error)| view! {
                <button class="diag" class:error=error on:click=move |_| state.diagnostics_open.update(|o| *o = !*o)>
                    {text}
                </button>
            })}
            <div class="toolbar-group">
                <button
                    disabled=move || !has_doc()
                    title="Fit the web to the window (F)"
                    on:click=move |_| {
                        if let Some(doc) = state.document() {
                            state.graph.fit(&doc.tree);
                        }
                    }
                >
                    "Fit"
                </button>
                <button
                    disabled=move || !has_doc()
                    title="Back to just the root"
                    on:click=move |_| {
                        state.graph.reset();
                        state.selected.set(Some(timber_core::Tree::ROOT));
                        state.popup.set(Some(timber_core::Tree::ROOT));
                    }
                >
                    "Reset"
                </button>
                <button disabled=move || !has_doc() title="Download as DTS source" on:click=move |_| export(false)>
                    ".dts"
                </button>
                <button disabled=move || !has_doc() title="Download as a compiled DTB" on:click=move |_| export(true)>
                    ".dtb"
                </button>
                <button
                    class="ghost icon"
                    title=move || state.theme.get().label()
                    aria-label=move || state.theme.get().label()
                    on:click=move |_| state.theme.update(|t| *t = t.next())
                >
                    {move || theme_icon(state.theme.get())}
                </button>
            </div>
        </header>
    }
}

fn theme_icon(pref: crate::state::ThemePref) -> impl IntoView {
    use crate::state::ThemePref;
    match pref {
        ThemePref::Auto => view! {
            <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true">
                <circle cx="8" cy="8" r="6" fill="none" stroke="currentColor" stroke-width="1.5" />
                <path d="M8 2a6 6 0 0 1 0 12z" fill="currentColor" />
            </svg>
        }
        .into_any(),
        ThemePref::Light => view! {
            <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true">
                <circle cx="8" cy="8" r="3" fill="currentColor" />
                <path
                    d="M8 1v2M8 13v2M1 8h2M13 8h2M3 3l1.4 1.4M11.6 11.6L13 13M3 13l1.4-1.4M11.6 4.4L13 3"
                    stroke="currentColor"
                    stroke-width="1.5"
                    stroke-linecap="round"
                />
            </svg>
        }
        .into_any(),
        ThemePref::Dark => view! {
            <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true">
                <path d="M13.5 10.2A6 6 0 0 1 5.8 2.5a6 6 0 1 0 7.7 7.7z" fill="currentColor" />
            </svg>
        }
        .into_any(),
    }
}
