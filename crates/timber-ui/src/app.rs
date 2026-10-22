//! Top-level layout and app-wide behaviour: loading files, theme, shortcuts, overlays.

use std::sync::Arc;
use std::time::Duration;

use leptos::ev;
use leptos::prelude::*;
use timber_core::Severity;
use wasm_bindgen::JsCast;

use crate::actions::{add_files, add_folder, open_files, show_board};
use crate::backend::{BackendHandle, WebBackend, read_file_list};
use crate::graph::GraphView;
use crate::help::{HelpDialog, show_help_on_start};
use crate::kind::Kind;
use crate::panel::NodePanel;
use crate::popup::ChildPopup;
use crate::sources_panel::SourcesPanel;
use crate::state::{AppState, ThemePref};
use crate::toolbar::{SEARCH_INPUT_ID, Toolbar};

const THEME_KEY: &str = "timber.theme";

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

fn prefers_dark() -> Option<web_sys::MediaQueryList> {
    web_sys::window()?
        .match_media("(prefers-color-scheme: dark)")
        .ok()
        .flatten()
}

fn is_typing(ev: &web_sys::KeyboardEvent) -> bool {
    ev.target()
        .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
        .is_some_and(|el| matches!(el.tag_name().as_str(), "INPUT" | "TEXTAREA" | "SELECT"))
}

#[component]
pub fn App() -> impl IntoView {
    let backend = if crate::tauri::in_tauri() {
        BackendHandle(Arc::new(crate::tauri::TauriBackend))
    } else {
        BackendHandle(Arc::new(WebBackend))
    };
    let state = AppState::new(backend.0.capabilities());
    provide_context(state);
    provide_context(backend.clone());
    show_help_on_start(state);
    if state.capabilities.desktop {
        let b = backend.clone();
        crate::tauri::on_folder_changed(move |path| {
            crate::actions::reload_folder_at(state, b.clone(), path)
        });
        crate::actions::run_startup(state, backend.clone());
    }

    // Theme: stored preference, applied to <html data-theme>, resolved against the OS.
    if let Some(saved) = storage().and_then(|s| s.get_item(THEME_KEY).ok().flatten()) {
        state.theme.set(ThemePref::from_key(&saved));
    }
    let os_dark = RwSignal::new(prefers_dark().is_some_and(|m| m.matches()));
    if let Some(mq) = prefers_dark() {
        let cb = wasm_bindgen::closure::Closure::<dyn FnMut(web_sys::MediaQueryListEvent)>::new(
            move |e: web_sys::MediaQueryListEvent| os_dark.set(e.matches()),
        );
        let _ = mq.add_event_listener_with_callback("change", cb.as_ref().unchecked_ref());
        cb.forget(); // lives as long as the page
    }
    Effect::new(move |_| {
        let pref = state.theme.get();
        let dark = match pref {
            ThemePref::Auto => os_dark.get(),
            ThemePref::Light => false,
            ThemePref::Dark => true,
        };
        state.dark.set(dark);
        if let Some(root) = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.document_element())
        {
            let _ = match pref {
                ThemePref::Auto => root.remove_attribute("data-theme"),
                p => root.set_attribute("data-theme", p.key()),
            };
        }
        if let Some(s) = storage() {
            let _ = s.set_item(THEME_KEY, pref.key());
        }
    });

    // Toasts dismiss themselves.
    Effect::new(move |_| {
        if let Some(toast) = state.toast.get() {
            set_timeout(
                move || {
                    if state.toast.get_untracked().as_ref() == Some(&toast) {
                        state.toast.set(None);
                    }
                },
                Duration::from_millis(4500),
            );
        }
    });

    // Global shortcuts (ignored while typing in a field).
    let _ = window_event_listener(ev::keydown, move |ev| {
        let key = ev.key();
        if (key == "k" && (ev.ctrl_key() || ev.meta_key())) || (key == "/" && !is_typing(&ev)) {
            ev.prevent_default();
            if let Some(el) = document().get_element_by_id(SEARCH_INPUT_ID) {
                let _ = el.unchecked_into::<web_sys::HtmlElement>().focus();
            }
            return;
        }
        if is_typing(&ev) || ev.ctrl_key() || ev.meta_key() || ev.alt_key() {
            return;
        }
        if key == "?" {
            state.help_open.update(|o| *o = !*o);
            return;
        }
        if key == "Escape"
            && (state.help_open.get_untracked() || state.sources_open.get_untracked())
        {
            state.help_open.set(false);
            state.sources_open.set(false);
            return;
        }
        let Some(doc) = state.document() else { return };
        match key.as_str() {
            "f" | "F" => state.graph.fit(&doc.tree),
            "Escape" => state.popup.set(None),
            "Delete" | "Backspace" => {
                if let Some(id) = state.selected.get_untracked() {
                    ev.prevent_default();
                    let parent = doc.tree.node(id).parent;
                    state.graph.remove_subtree(&doc.tree, id);
                    if let Some(p) = parent {
                        state.selected.set(Some(p));
                        if state.popup.get_untracked() == Some(id) {
                            state.popup.set(Some(p));
                        }
                    }
                }
            }
            _ => {}
        }
    });

    // Drag and drop files anywhere.
    let dropping = RwSignal::new(false);
    let on_dragover = move |ev: ev::DragEvent| {
        ev.prevent_default();
        dropping.set(true);
    };
    let on_dragleave = move |ev: ev::DragEvent| {
        if ev.related_target().is_none() {
            dropping.set(false);
        }
    };
    let on_drop = move |ev: ev::DragEvent| {
        ev.prevent_default();
        dropping.set(false);
        let Some(files) = ev.data_transfer().and_then(|d| d.files()) else {
            return;
        };
        leptos::task::spawn_local(async move {
            match read_file_list(files).await {
                Ok(files) if !files.is_empty() => add_files(state, files),
                Ok(_) => {}
                Err(e) => state.notify(format!("Couldn't read dropped files: {e}"), true),
            }
        });
    };

    view! {
        <div class="app" class:dropping=move || dropping.get() on:dragover=on_dragover on:dragleave=on_dragleave on:drop=on_drop>
            <Toolbar />
            <main class="workspace">
                <section class="graph-area">
                    <GraphView />
                    <ChildPopup />
                    <Legend />
                    <EmptyState />
                </section>
                <aside class="panel" aria-label="Node details">
                    <NodePanel />
                </aside>
            </main>
            <Diagnostics />
            <ChooseMain />
            <SourcesPanel />
            <HelpDialog />
            <BusyView />
            <ToastView />
            <div class="drop-hint" aria-hidden="true">"Drop .dtb / .dts files to open"</div>
        </div>
    }
}

#[component]
fn EmptyState() -> impl IntoView {
    let state = expect_context::<AppState>();
    let backend = expect_context::<BackendHandle>();
    let b2 = backend.clone();
    view! {
        <Show when=move || state.doc.with(Option::is_none)>
            <div class="empty">
                <div class="empty-card">
                    <h1>"Open a device tree"</h1>
                    <p>
                        "Open a compiled " <code>".dtb"</code> " / " <code>".dtbo"</code> " or " <code>".dts"</code>
                        " files, and add the kernel or BSP folders they come from to see where every node is defined."
                    </p>
                    <div class="empty-actions">
                        <button class="primary" on:click={
                            let backend = backend.clone();
                            move |_| open_files(state, backend.clone())
                        }>"Open DTB / DTS files…"</button>
                        <button on:click={
                            let backend = b2.clone();
                            move |_| add_folder(state, backend.clone())
                        }>"Add source folder…"</button>
                    </div>
                    <p class="muted small">"Or drop files anywhere in the window. Press ? for shortcuts."</p>
                    {(!state.capabilities.desktop).then(|| view! {
                        <p class="muted small">
                            "Files are read inside this page and never leave your computer. The desktop app adds SSH to live devices and opening sources in your editor."
                        </p>
                    })}
                </div>
            </div>
        </Show>
    }
}

#[component]
fn Legend() -> impl IntoView {
    let state = expect_context::<AppState>();
    move || {
        let doc = state.doc.get()?;
        let dark = state.dark.get();
        let items = Kind::ALL
            .iter()
            .filter(|k| doc.kinds.contains(k))
            .map(|k| {
                view! {
                    <li>
                        <span class="dot" style=format!("background: {}", k.color(dark))></span>
                        {k.label()}
                    </li>
                }
            })
            .collect_view();
        // Collapsed by default on narrow screens, where it would cover the graph.
        let wide = web_sys::window()
            .and_then(|w| w.inner_width().ok())
            .and_then(|v| v.as_f64())
            .is_none_or(|w| w > 760.0);
        Some(view! {
            <details class="legend" open=wide>
                <summary>"Legend"</summary>
                <ul>{items}</ul>
                <p class="muted small">"Dimmed: status disabled · +N: hidden children · dashed: references"</p>
                {doc.diff.is_some().then(|| view! {
                    <p class="muted small">
                        <span class="diff-pill">"Δ2"</span>" differs from source · "
                        <span class="diff-pill">"new"</span>" not in source · amber +N: differences hidden below"
                    </p>
                })}
            </details>
        })
    }
}

#[component]
fn Diagnostics() -> impl IntoView {
    let state = expect_context::<AppState>();
    move || {
        if !state.diagnostics_open.get() {
            return None;
        }
        let doc = state.doc.get()?;
        if doc.diagnostics.is_empty() {
            return None;
        }
        let items = doc
            .diagnostics
            .iter()
            .map(|d| {
                let class = if d.severity == Severity::Error { "diag-item error" } else { "diag-item" };
                view! {
                    <li class=class>
                        <div class="diag-loc">{format!("{}:{}:{}", d.file, d.line, d.col)}</div>
                        <div class="diag-msg">{d.message.clone()}</div>
                        {(!d.snippet.is_empty()).then(|| view! { <pre class="diag-snippet">{d.snippet.clone()}</pre> })}
                    </li>
                }
            })
            .collect_view();
        Some(view! {
            <section class="drawer" aria-label="Compiler messages">
                <header>
                    <h3>{format!("Compiler messages ({})", doc.diagnostics.len())}</h3>
                    <span class="muted small">"The tree below is best-effort where errors occurred."</span>
                    <button class="ghost" on:click=move |_| state.diagnostics_open.set(false) aria-label="Close">"✕"</button>
                </header>
                <ul>{items}</ul>
            </section>
        })
    }
}

#[component]
fn ChooseMain() -> impl IntoView {
    let state = expect_context::<AppState>();
    move || {
        let names = state.choose.get()?;
        let items = names
            .into_iter()
            .map(|path| {
                let p = path.clone();
                let shown = path.strip_prefix("files/").unwrap_or(&path).to_string();
                view! {
                    <li>
                        <button on:click=move |_| {
                            state.choose.set(None);
                            show_board(state, &p);
                        }>{shown}</button>
                    </li>
                }
            })
            .collect_view();
        Some(view! {
            <div class="modal-backdrop" on:click=move |_| state.choose.set(None)>
                <div class="modal" role="dialog" aria-label="Choose the board to show" on:click=|ev| ev.stop_propagation()>
                    <h3>"Which board should be shown?"</h3>
                    <p class="muted small">"All the files were added to Sources; the others stay available for #include."</p>
                    <ul class="choose">{items}</ul>
                </div>
            </div>
        })
    }
}

#[component]
fn BusyView() -> impl IntoView {
    let state = expect_context::<AppState>();
    move || {
        state.busy.get().map(|msg| {
            view! {
                <div class="busy" role="status">
                    <span class="spinner" aria-hidden="true"></span>
                    {msg}
                </div>
            }
        })
    }
}

#[component]
fn ToastView() -> impl IntoView {
    let state = expect_context::<AppState>();
    move || {
        state.toast.get().map(|t| {
            view! {
                <div class="toast" class:error=t.error role="status" on:click=move |_| state.toast.set(None)>
                    {t.message}
                </div>
            }
        })
    }
}
