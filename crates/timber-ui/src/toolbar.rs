//! Top bar: global search, file actions, view controls.

use leptos::html;
use leptos::prelude::*;
use timber_core::NodeId;
use web_sys::KeyboardEvent;

use crate::backend::BackendHandle;
use crate::search::{Hit, MatchKind};
use crate::state::AppState;
use crate::util::highlight;

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
        // With "Only differences", only nodes that differ themselves.
        let only = state.diff_only.get().then_some(doc.diff.as_ref()).flatten();
        doc.search
            .search(&doc.tree, &q, if only.is_some() { 2000 } else { 40 })
            .into_iter()
            .filter(|hit| only.is_none_or(|d| d.is_marked(hit.id)))
            .take(40)
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
    let has_doc = move || state.doc.with(Option::is_some);
    let desktop = state.capabilities.desktop;
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
                <div class="brand-wrap">
                    <span class="brand" aria-hidden="true">"timber"</span>
                    <a
                        class="repo-link"
                        href=env!("CARGO_PKG_REPOSITORY")
                        target="_blank"
                        rel="noopener noreferrer"
                        title="Timber on GitHub"
                        aria-label="Timber on GitHub"
                        on:click=move |ev| {
                            // The desktop window can't show other sites: open the browser.
                            if desktop {
                                ev.prevent_default();
                                leptos::task::spawn_local(crate::tauri::open_repo());
                            }
                        }
                    >
                        <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true">
                            <path
                                fill="currentColor"
                                d="M8 0c4.42 0 8 3.58 8 8a8.013 8.013 0 0 1-5.45 7.59c-.4.08-.55-.17-.55-.38 0-.27.01-1.13.01-2.2 0-.75-.25-1.23-.54-1.48 1.78-.2 3.65-.88 3.65-3.95 0-.88-.31-1.59-.82-2.15.08-.2.36-1.02-.08-2.12 0 0-.67-.22-2.2.82-.64-.18-1.32-.27-2-.27-.68 0-1.36.09-2 .27-1.53-1.03-2.2-.82-2.2-.82-.44 1.1-.16 1.92-.08 2.12-.51.56-.82 1.28-.82 2.15 0 3.06 1.86 3.75 3.64 3.95-.23.2-.44.55-.51 1.07-.46.21-1.61.55-2.33-.66-.15-.24-.6-.83-1.23-.82-.67.01-.27.38.01.53.34.19.73.9.82 1.13.16.45.68 1.31 2.69.94 0 .67.01 1.3.01 1.49 0 .21-.15.45-.55.38A7.995 7.995 0 0 1 0 8c0-4.42 3.58-8 8-8Z"
                            />
                        </svg>
                    </a>
                </div>
                <GlobalSearch />
                <div class="toolbar-group">
                    <button
                        class="primary"
                        title="Open DTB/DTS files and add source folders"
                        on:click=move |_| state.sources_open.set(true)
                    >
                        "Sources"
                        {move || {
                            let n = state.sources.with(|s| s.entries.len());
                            (n > 0).then(|| view! { <span class="count">{n}</span> })
                        }}
                    </button>
                    <button
                        class:unavailable=!desktop
                        title=if desktop { "Connect to a device over SSH" } else { "Requires the desktop app" }
                        on:click=move |_| {
                            if desktop {
                                state.ssh.open.set(true);
                            } else {
                                state.ssh.get_desktop.set(true);
                            }
                        }
                    >
                        "SSH"
                        {(!desktop).then(|| view! { <span class="badge">"Desktop"</span> })}
                    </button>
                    <crate::ssh::LiveBar />
                </div>
                <div class="spacer"></div>
                {move || state.doc.with(|d| d.as_ref().map(|d| {
                    let title = format!("{} · {} nodes", d.name, d.node_count());
                    view! {
                        <span class="doc-name" title=title>
                            {d.short_name().to_string()}
                            {d.overlay.then(|| view! { <span class="badge">"overlay"</span> })}
                        </span>
                    }
                }))}
                {move || state.doc.with(|d| d.as_ref().and_then(|d| d.diff.as_ref()).map(|diff| {
                    let total = diff.total();
                    let b = diff.b_name().to_string();
                    (total > 0 || !diff.from_source).then(|| view! {
                        <button
                            class="diag"
                            class:same=total == 0
                            aria-pressed=move || state.summary_open.get().to_string()
                            aria-label=if total == 0 { format!("Same as {b}") } else { format!("{total} differ from {b}") }
                            title=format!("Differences from {b}: {}. Click for the summary.", diff.summary())
                            on:click=move |_| state.summary_open.update(|o| *o = !*o)
                        >
                            // The short form is for narrow windows (style.css).
                            <span class="long">{if total == 0 { format!("Same as {b}") } else { format!("{total} differ from {b}") }}</span>
                            <span class="short">{if total == 0 { "Same".to_string() } else { format!("{total} Δ") }}</span>
                        </button>
                    })
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
                    <ExportMenu />
    <button class="ghost icon" title="Settings" aria-label="Settings" on:click=move |_| state.settings_open.set(true)>
                        <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true">
                            <circle cx="8" cy="8" r="2.3" fill="none" stroke="currentColor" stroke-width="1.5" />
                            <path
                                d="M8 1.5v2M8 12.5v2M1.5 8h2M12.5 8h2M3.4 3.4l1.4 1.4M11.2 11.2l1.4 1.4M3.4 12.6l1.4-1.4M11.2 4.8l1.4-1.4"
                                stroke="currentColor"
                                stroke-width="1.5"
                                stroke-linecap="round"
                            />
                        </svg>
                    </button>
                                    <button class="ghost icon" title="Shortcuts (?)" aria-label="Shortcuts" on:click=move |_| state.help_open.set(true)>
                        <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true">
                            <circle cx="8" cy="8" r="6.5" fill="none" stroke="currentColor" stroke-width="1.5" />
                            <path d="M6.2 6.2a1.9 1.9 0 1 1 2.6 1.8c-.5.2-.8.6-.8 1.1v.4" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" />
                            <circle cx="8" cy="11.6" r=".9" fill="currentColor" />
                        </svg>
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

fn save(
    state: AppState,
    backend: BackendHandle,
    file: Result<(String, &'static str, Vec<u8>), String>,
) {
    match file {
        Ok((name, mime, data)) => leptos::task::spawn_local(async move {
            match backend.0.save_file(&name, mime, data).await {
                Ok(true) => state.notify(format!("Saved {name}"), false),
                Ok(false) => {}
                Err(e) => state.notify(format!("Export failed: {e}"), true),
            }
        }),
        Err(e) => state.notify(format!("Export failed: {e}"), true),
    }
}

#[component]
fn ExportMenu() -> impl IntoView {
    use crate::export::{ExportOptions, ExportTheme, ImageFormat, export_image, export_tree};

    let state = expect_context::<AppState>();
    let backend = StoredValue::new(expect_context::<BackendHandle>());
    let open = RwSignal::new(false);
    let opts = RwSignal::new(ExportOptions::default());
    let image = move |format: ImageFormat| {
        open.set(false);
        save(
            state,
            backend.get_value(),
            export_image(state, format, opts.get_untracked()),
        );
    };
    let tree = move |as_dtb: bool| {
        open.set(false);
        save(
            state,
            backend.get_value(),
            export_tree(state, as_dtb).ok_or_else(|| "nothing open".to_string()),
        );
    };
    let theme_value = move || match opts.get().theme {
        ExportTheme::Light => "light",
        ExportTheme::Dark => "dark",
        ExportTheme::Current => "current",
    };

    view! {
        <div class="menu">
            <button
                disabled=move || state.doc.with(Option::is_none)
                aria-haspopup="true"
                aria-expanded=move || open.get().to_string()
                on:click=move |_| open.update(|o| *o = !*o)
            >
                "Export ▾"
            </button>
            <Show when=move || open.get()>
                <div class="menu-backdrop" on:click=move |_| open.set(false)></div>
                <div class="menu-panel" role="menu">
                    <h4>"Image of the web"</h4>
                    <div class="segmented" role="radiogroup" aria-label="What to include">
                        <button class:on=move || opts.get().whole on:click=move |_| opts.update(|o| o.whole = true)>
                            "Whole web"
                        </button>
                        <button class:on=move || !opts.get().whole on:click=move |_| opts.update(|o| o.whole = false)>
                            "Current view"
                        </button>
                    </div>
                    <label class="field">
                        "Theme"
                        <select
                            prop:value=theme_value
                            on:change=move |ev| {
                                let theme = match event_target_value(&ev).as_str() {
                                    "dark" => ExportTheme::Dark,
                                    "current" => ExportTheme::Current,
                                    _ => ExportTheme::Light,
                                };
                                opts.update(|o| o.theme = theme);
                            }
                        >
                            <option value="light">"Light"</option>
                            <option value="dark">"Dark"</option>
                            <option value="current">"Same as app"</option>
                        </select>
                    </label>
                    <label class="check">
                        <input type="checkbox" prop:checked=move || opts.get().title on:change=move |ev| opts.update(|o| o.title = event_target_checked(&ev)) />
                        "Title"
                    </label>
                    <label class="check">
                        <input type="checkbox" prop:checked=move || opts.get().legend on:change=move |ev| opts.update(|o| o.legend = event_target_checked(&ev)) />
                        "Legend"
                    </label>
                    <div class="row">
                        <button class="primary" title="Vector image: scales cleanly in documents" on:click=move |_| image(ImageFormat::Svg)>
                            "SVG"
                        </button>
                        <button title="2× resolution image for sharing" on:click=move |_| image(ImageFormat::Png)>"PNG"</button>
                    </div>
                    <h4>"Device tree"</h4>
                    <div class="row">
                        <button title="Decompiled DTS source" on:click=move |_| tree(false)>".dts source"</button>
                        <button title="Compiled DTB blob" on:click=move |_| tree(true)>".dtb blob"</button>
                    </div>
                </div>
            </Show>
        </div>
    }
}
