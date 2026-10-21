//! The keyboard and mouse shortcuts popup (D-020).

use leptos::prelude::*;

use crate::state::AppState;

const SEEN_KEY: &str = "timber.help.hide";

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

/// Opens the help on first visit unless the user turned that off.
pub fn show_help_on_start(state: AppState) {
    let hidden = storage()
        .and_then(|s| s.get_item(SEEN_KEY).ok().flatten())
        .is_some();
    state.help_open.set(!hidden);
}

const GROUPS: &[(&str, &[(&str, &str)])] = &[
    (
        "Anywhere",
        &[
            (
                "/  or  Ctrl K",
                "Search nodes, labels, aliases, compatibles",
            ),
            ("F", "Fit the web to the window"),
            ("Esc", "Close the child popup"),
            ("Del", "Remove the selected node from the web"),
            ("?", "Show this help"),
        ],
    ),
    (
        "Child popup",
        &[
            ("type", "Filter the children"),
            ("↑  ↓", "Move through the list"),
            ("⏎", "Add the child and move onto it"),
            ("Ctrl ⏎", "Add the child and stay"),
            ("Ctrl A", "Add all children shown"),
            ("⌫", "Go up to the parent (empty filter)"),
        ],
    ),
    (
        "Mouse",
        &[
            ("Click node", "Select it and open its children"),
            ("Double-click", "Collapse, or expand all children"),
            ("Drag node", "Move it"),
            ("Drag background", "Pan"),
            ("Scroll", "Zoom"),
        ],
    ),
];

#[component]
pub fn HelpDialog() -> impl IntoView {
    let state = expect_context::<AppState>();
    let hide_on_start = RwSignal::new(
        storage()
            .and_then(|s| s.get_item(SEEN_KEY).ok().flatten())
            .is_some(),
    );
    let close = move || state.help_open.set(false);
    let toggle_start = move |ev: leptos::ev::Event| {
        let hide = event_target_checked(&ev);
        hide_on_start.set(hide);
        if let Some(s) = storage() {
            let _ = if hide {
                s.set_item(SEEN_KEY, "1")
            } else {
                s.remove_item(SEEN_KEY)
            };
        }
    };

    let groups = GROUPS
        .iter()
        .map(|(title, rows)| {
            let rows = rows
                .iter()
                .map(|(keys, what)| {
                    let keys = keys
                        .split("  ")
                        .map(|k| view! { <kbd>{k.to_string()}</kbd> })
                        .collect_view();
                    view! {
                        <tr>
                            <td class="keys">{keys}</td>
                            <td>{*what}</td>
                        </tr>
                    }
                })
                .collect_view();
            view! {
                <section>
                    <h4>{*title}</h4>
                    <table class="shortcuts">{rows}</table>
                </section>
            }
        })
        .collect_view();

    view! {
        <Show when=move || state.help_open.get()>
            <div class="modal-backdrop" on:click=move |_| close()>
                <div class="modal wide" role="dialog" aria-label="Keyboard shortcuts" on:click=|ev| ev.stop_propagation()>
                    <header class="modal-head">
                        <h3>"Getting around"</h3>
                        <button class="ghost" aria-label="Close" on:click=move |_| close()>"✕"</button>
                    </header>
                    <p class="muted small">
                        "Click a node to see its children, pick the ones to add, and grow the web from there."
                    </p>
                    <div class="help-grid">{groups.clone()}</div>
                    <footer class="modal-foot">
                        <label class="check">
                            <input type="checkbox" prop:checked=move || hide_on_start.get() on:change=toggle_start />
                            "Don't show on startup"
                        </label>
                        <button class="primary" on:click=move |_| close()>"Got it"</button>
                    </footer>
                </div>
            </div>
        </Show>
    }
}
