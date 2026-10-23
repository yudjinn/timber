//! Noticing a newer build (D-036). Trunk names the wasm after its content hash, so the app
//! compares the wasm it's running with the one the server's `index.html` now references.
//!
//! - Desktop in dev mode (UI from `trunk serve`): reload straight away.
//! - Desktop release: the UI is built into the binary and can't be out of date; no check.
//! - Web: on load, when the window regains focus, and every 15 minutes, until a newer build
//!   is found. Then ask; **Later** leaves a small badge and stops checking until the next
//!   load.

use leptos::prelude::*;
use wasm_bindgen::{JsCast, JsValue};

use crate::backend::{await_js, call0, get};
use crate::state::AppState;

const EVERY_MS: i32 = 15 * 60 * 1000;
/// The build a dev-mode reload was for, so a reload that didn't take doesn't loop.
const RELOADED_KEY: &str = "timber.reloaded-for";

#[derive(Clone, Copy)]
pub struct UpdateState {
    /// A newer build is on the server.
    pub available: RwSignal<bool>,
    /// The "new version" notice is showing (cleared by Later).
    pub asking: RwSignal<bool>,
}

impl UpdateState {
    pub fn new() -> Self {
        UpdateState {
            available: RwSignal::new(false),
            asking: RwSignal::new(false),
        }
    }
}

fn wasm_name(text: &str) -> Option<String> {
    let end = text.find("_bg.wasm")? + "_bg.wasm".len();
    let start = text[..end].rfind(['/', '\'', '"'])? + 1;
    Some(text[start..end].to_string())
}

/// The wasm this page loaded.
fn running() -> Option<String> {
    let doc = web_sys::window()?.document()?;
    let link = doc
        .query_selector("link[rel=preload][href$='_bg.wasm']")
        .ok()??;
    wasm_name(&link.get_attribute("href")?)
}

/// The wasm the server's page references now.
async fn latest() -> Option<String> {
    let window = web_sys::window()?;
    let href = window.location().href().ok()?;
    let page = &href[..href.rfind('/')? + 1];
    let opts = js_sys::Object::new();
    js_sys::Reflect::set(&opts, &"cache".into(), &"no-store".into()).ok()?;
    let fetch = get(&window, "fetch").dyn_into::<js_sys::Function>().ok()?;
    let response = await_js(fetch.call2(&window, &JsValue::from_str(page), &opts).ok()?)
        .await
        .ok()?;
    if !get(&response, "ok").as_bool().unwrap_or(false) {
        return None;
    }
    let text = await_js(call0(&response, "text").ok()?).await.ok()?;
    wasm_name(&text.as_string()?)
}

/// In the desktop app, whether the UI comes from the dev server rather than the binary.
fn desktop_dev() -> bool {
    web_sys::window()
        .and_then(|w| w.location().hostname().ok())
        .is_some_and(|h| h == "127.0.0.1" || h == "localhost")
}

fn session() -> Option<web_sys::Storage> {
    web_sys::window()?.session_storage().ok().flatten()
}

pub fn reload() {
    if let Some(w) = web_sys::window() {
        let _ = w.location().reload();
    }
}

/// Whether a newer build is out, and if so what to do about it. `true` once found.
async fn check(state: AppState) -> bool {
    let (Some(now), Some(new)) = (running(), latest().await) else {
        return false;
    };
    if now == new {
        return false;
    }
    if state.capabilities.desktop {
        // Dev mode: reload, unless that was already tried for this build.
        let tried = session().and_then(|s| s.get_item(RELOADED_KEY).ok().flatten());
        if tried.as_deref() != Some(new.as_str()) {
            if let Some(s) = session() {
                let _ = s.set_item(RELOADED_KEY, &new);
            }
            reload();
            return true;
        }
        state.update.available.set(true);
        return true;
    }
    state.update.available.set(true);
    state.update.asking.set(true);
    true
}

/// Starts watching for newer builds.
pub fn watch(state: AppState) {
    if state.capabilities.desktop && !desktop_dev() {
        return;
    }
    let checking = StoredValue::new(false);
    let run = move || {
        if state.update.available.get_untracked() || checking.get_value() {
            return;
        }
        checking.set_value(true);
        leptos::task::spawn_local(async move {
            check(state).await;
            checking.set_value(false);
        });
    };
    run();
    if state.capabilities.desktop {
        return;
    }
    let handle = window_event_listener(leptos::ev::focus, move |_| run());
    // Lives as long as the page.
    std::mem::forget(handle);
    leptos::task::spawn_local(async move {
        while !state.update.available.get_untracked() {
            crate::actions::sleep(EVERY_MS).await;
            run();
        }
    });
}

/// "A new version of Timber is available", and the badge left after Later.
#[component]
pub fn UpdateNotice() -> impl IntoView {
    let state = expect_context::<AppState>();
    let u = state.update;
    view! {
        <Show when=move || u.asking.get()>
            <div class="modal-backdrop">
                <div class="modal update" role="alertdialog" aria-label="New version">
                    <h3>"A new version of Timber is available"</h3>
                    <p class="muted small">
                        "Reloading closes the tree and sources you have open; you'll need to open them again."
                    </p>
                    <div class="row">
                        <button class="primary" on:click=move |_| reload()>"Reload"</button>
                        <button on:click=move |_| u.asking.set(false)>"Later"</button>
                    </div>
                </div>
            </div>
        </Show>
        <Show when=move || u.available.get() && !u.asking.get()>
            <button
                class="update-badge"
                title="A newer version of Timber is available. Click to reload."
                on:click=move |_| if state.capabilities.desktop { reload() } else { u.asking.set(true) }
            >
                "Update available"
            </button>
        </Show>
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn finds_the_wasm_name() {
        let page = "import init from '/timber-ui-feb8.js';\nconst wasm = await init({ module_or_path: '/timber-ui-9bc9550a15f68328_bg.wasm' });";
        assert_eq!(
            super::wasm_name(page).as_deref(),
            Some("timber-ui-9bc9550a15f68328_bg.wasm")
        );
        assert_eq!(
            super::wasm_name("<link href=\"./app-1_bg.wasm\">").as_deref(),
            Some("app-1_bg.wasm")
        );
        assert_eq!(super::wasm_name("nothing here"), None);
    }
}
