//! Top-level layout and app-wide behaviour: loading files, theme, shortcuts, overlays.

use std::sync::Arc;
use std::time::Duration;

use leptos::ev;
use leptos::prelude::*;
use timber_core::Severity;
use wasm_bindgen::JsCast;

use crate::backend::{BackendHandle, WebBackend, read_file_list};
use crate::doc::{InputFile, Loaded, load};
use crate::graph::GraphView;
use crate::kind::Kind;
use crate::panel::NodePanel;
use crate::popup::ChildPopup;
use crate::state::{AppState, ThemePref};
use crate::toolbar::{SEARCH_INPUT_ID, Toolbar};

const THEME_KEY: &str = "timber.theme";

/// Loads files into the app, asking which is the main source when that's ambiguous.
pub fn load_files(state: AppState, files: Vec<InputFile>, main: Option<String>) {
    match load(&files, main.as_deref()) {
        Ok(Loaded::Doc(doc)) => {
            let name = doc.name.clone();
            let nodes = doc.tree.len();
            let errors = doc.error_count();
            state.set_document(*doc);
            if errors == 0 {
                state.notify(format!("Opened {name}: {nodes} nodes"), false);
            }
        }
        Ok(Loaded::Choose(names)) => state.choose.set(Some((files, names))),
        Err(e) => state.notify(e, true),
    }
}

pub fn open_files(state: AppState, backend: BackendHandle) {
    leptos::task::spawn_local(async move {
        match backend.0.open_files().await {
            Ok(Some(files)) => load_files(state, files, None),
            Ok(None) => {}
            Err(e) => state.notify(format!("Couldn't open files: {e}"), true),
        }
    });
}

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
    let backend = BackendHandle(Arc::new(WebBackend));
    let state = AppState::new(backend.0.capabilities());
    provide_context(state);
    provide_context(backend.clone());

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
                Ok(files) if !files.is_empty() => load_files(state, files, None),
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
            <ToastView />
            <div class="drop-hint" aria-hidden="true">"Drop .dtb / .dts files to open"</div>
        </div>
    }
}

#[component]
fn EmptyState() -> impl IntoView {
    let state = expect_context::<AppState>();
    let backend = expect_context::<BackendHandle>();
    view! {
        <Show when=move || state.doc.with(Option::is_none)>
            <div class="empty">
                <div class="empty-card">
                    <h1>"Open a device tree"</h1>
                    <p>
                        "A compiled " <code>".dtb"</code> " / " <code>".dtbo"</code> ", or a " <code>".dts"</code>
                        " source with the " <code>".dtsi"</code> " and header files it includes."
                    </p>
                    <button class="primary" on:click={
                        let backend = backend.clone();
                        move |_| open_files(state, backend.clone())
                    }>"Open files…"</button>
                    <p class="muted small">"Or drop files anywhere in the window."</p>
                    {(!state.capabilities.desktop).then(|| view! {
                        <p class="muted small">
                            "The desktop app adds SSH to live devices, BSP folders, and opening sources in your editor."
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
        let (_, names) = state.choose.get()?;
        let items = names
            .into_iter()
            .map(|name| {
                let n = name.clone();
                view! {
                    <li>
                        <button on:click=move |_| {
                            if let Some((files, _)) = state.choose.get_untracked() {
                                state.choose.set(None);
                                load_files(state, files, Some(n.clone()));
                            }
                        }>{name}</button>
                    </li>
                }
            })
            .collect_view();
        Some(view! {
            <div class="modal-backdrop" on:click=move |_| state.choose.set(None)>
                <div class="modal" role="dialog" aria-label="Choose the main file" on:click=|ev| ev.stop_propagation()>
                    <h3>"Which file is the board?"</h3>
                    <p class="muted small">"The others stay available for #include."</p>
                    <ul class="choose">{items}</ul>
                </div>
            </div>
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
