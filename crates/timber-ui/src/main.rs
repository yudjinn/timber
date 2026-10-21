//! Timber's UI: a Leptos (client-side) app compiled to wasm. Runs in a browser as the web
//! build and, from M3, inside the Tauri desktop app.

mod app;
mod backend;
mod doc;
mod graph;
mod kind;
mod panel;
mod popup;
mod search;
mod state;
mod toolbar;
mod util;

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(app::App);
}
