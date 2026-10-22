//! Timber's UI: a Leptos (client-side) app compiled to wasm. Runs in a browser as the web
//! build and, from M3, inside the Tauri desktop app.

mod actions;
mod app;
mod backend;
mod doc;
mod export;
mod graph;
mod help;
mod kind;
mod panel;
mod popup;
mod search;
mod source_text;
mod source_view;
mod sources;
mod sources_panel;
mod state;
mod tauri;
mod toolbar;
mod util;

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(app::App);
}
