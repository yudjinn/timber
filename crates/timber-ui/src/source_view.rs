//! The source viewer drawer: a source file with syntax highlighting, the referenced node or
//! property highlighted, `#include`s and `&label`s as links, and Open in editor.

use leptos::prelude::*;
use timber_core::vfs::{IncludeResolver, Vfs};
use wasm_bindgen::JsCast;

use crate::backend::BackendHandle;
use crate::source_text::{Tok, highlight_lines, statement_lines};
use crate::state::{AppState, SourceLocation};

fn tok_class(t: Tok) -> &'static str {
    match t {
        Tok::Text => "",
        Tok::Comment => "t-comment",
        Tok::Str => "t-str",
        Tok::Number => "t-num",
        Tok::Directive => "t-dir",
        Tok::Label => "t-label",
        Tok::Ref => "t-ref",
        Tok::NodeName => "t-node",
        Tok::PropName => "t-prop",
    }
}

/// The include target named on an `#include "x"` / `#include <x>` / `/include/ "x"` line.
fn include_target(line: &str) -> Option<(String, bool)> {
    let t = line.trim_start();
    let rest = t
        .strip_prefix("#include")
        .or_else(|| t.strip_prefix("/include/"))?
        .trim_start();
    let (open, close, quoted) = match rest.chars().next()? {
        '"' => ('"', '"', true),
        '<' => ('<', '>', false),
        _ => return None,
    };
    let inner = rest.strip_prefix(open)?;
    Some((inner[..inner.find(close)?].to_string(), quoted))
}

fn go_to_label(state: AppState, label: &str) {
    let Some(doc) = state.document() else { return };
    let label = label.trim_start_matches('&');
    let found = if let Some(path) = label.strip_prefix('{').and_then(|l| l.strip_suffix('}')) {
        doc.tree.find(path)
    } else {
        (0..doc.tree.len()).find(|&id| doc.tree.node(id).labels.iter().any(|l| l == label))
    };
    match found {
        Some(id) => state.go_to(id),
        None => state.notify(format!("&{label} isn't in the tree shown"), false),
    }
}

#[component]
pub fn SourceDrawer() -> impl IntoView {
    let state = expect_context::<AppState>();
    let backend = StoredValue::new(expect_context::<BackendHandle>());
    let desktop = state.capabilities.desktop;

    // Scroll the target line into view whenever the location changes.
    Effect::new(move |_| {
        if let Some(loc) = state.source_view.get() {
            request_animation_frame(move || {
                if let Some(el) = document().get_element_by_id(&format!("src-L{}", loc.line)) {
                    let opts = web_sys::ScrollIntoViewOptions::new();
                    opts.set_block(web_sys::ScrollLogicalPosition::Center);
                    el.unchecked_ref::<web_sys::Element>()
                        .scroll_into_view_with_scroll_into_view_options(&opts);
                }
            });
        }
    });

    let open_in_editor = move |loc: SourceLocation| {
        let Some(path) = state.disk_path(&loc.path) else {
            return;
        };
        let template = state.editor_cmd.get_untracked();
        let backend = backend.get_value();
        leptos::task::spawn_local(async move {
            if let Err(e) = backend
                .0
                .open_in_editor(&path, loc.line, loc.col, &template)
                .await
            {
                state.notify(format!("Couldn't open the editor: {e}"), true);
            }
        });
    };

    move || {
        let loc = state.source_view.get()?;
        let text = state.sources.with(|s| {
            s.vfs
                .read(&loc.path)
                .map(|d| String::from_utf8_lossy(&d).into_owned())
        });
        let (from, to) = text
            .as_deref()
            .map_or((0, 0), |t| statement_lines(t, loc.line, loc.col));
        let disk = state.disk_path(&loc.path);
        let editor_title = match (desktop, &disk) {
            (false, _) => "Requires the desktop app",
            (true, None) => "This file wasn't opened from disk",
            (true, Some(_)) => "Open in your editor (Settings → Editor command)",
        };
        let loc_for_editor = loc.clone();
        let current = loc.path.clone();

        let body = match text {
            None => view! { <p class="muted source-missing">{format!("{} isn't in the sources.", loc.path)}</p> }.into_any(),
            Some(text) => {
                let raw_lines: Vec<&str> = text.lines().collect();
                highlight_lines(&text)
                    .into_iter()
                    .enumerate()
                    .map(|(i, toks)| {
                        let n = i as u32 + 1;
                        let include = raw_lines.get(i).and_then(|l| include_target(l));
                        let spans = toks
                            .into_iter()
                            .map(|(tok, s)| {
                                let class = tok_class(tok);
                                if tok == Tok::Ref {
                                    let label = s.clone();
                                    return view! {
                                        <span class=format!("{class} src-link") on:click=move |_| go_to_label(state, &label)>{s}</span>
                                    }
                                    .into_any();
                                }
                                if tok == Tok::Str
                                    && let Some((name, quoted)) = include.clone()
                                {
                                    let from_path = current.clone();
                                    return view! {
                                        <span
                                            class=format!("{class} src-link")
                                            title="Open the included file"
                                            on:click=move |_| {
                                                let target = state.sources.with(|s| {
                                                    IncludeResolver::new(&s.vfs).resolve(&name, &from_path, quoted)
                                                });
                                                match target {
                                                    Some(path) => state.open_source(SourceLocation { path, line: 1, col: 1 }),
                                                    None => state.notify(format!("{name} isn't in the sources"), false),
                                                }
                                            }
                                        >
                                            {s}
                                        </span>
                                    }
                                    .into_any();
                                }
                                view! { <span class=class>{s}</span> }.into_any()
                            })
                            .collect_view();
                        view! {
                            <div class="src-line" class:hl={n >= from && n <= to} class:target={n == loc.line} id=format!("src-L{n}")>
                                <span class="ln">{n}</span>
                                <code>{spans}</code>
                            </div>
                        }
                    })
                    .collect_view()
                    .into_any()
            }
        };

        Some(view! {
            <section class="source-drawer" aria-label="Source viewer">
                <header>
                    <button class="ghost small" title="Back" aria-label="Back" on:click=move |_| state.source_step(-1)>"◀"</button>
                    <button class="ghost small" title="Forward" aria-label="Forward" on:click=move |_| state.source_step(1)>"▶"</button>
                    <span class="source-path mono" title=loc.path.clone()>{format!("{}:{}", loc.path, loc.line)}</span>
                    <button
                        class="small"
                        class:unavailable=disk.is_none()
                        title=editor_title
                        on:click=move |_| {
                            if desktop && state.disk_path(&loc_for_editor.path).is_some() {
                                open_in_editor(loc_for_editor.clone());
                            } else {
                                state.notify(editor_title, false);
                            }
                        }
                    >
                        "Open in editor"
                        {(!desktop).then(|| view! { <span class="badge">"Desktop"</span> })}
                    </button>
                    <button class="ghost" aria-label="Close" on:click=move |_| state.source_view.set(None)>"✕"</button>
                </header>
                <div class="source-body">{body}</div>
            </section>
        })
    }
}

#[component]
pub fn SettingsDialog() -> impl IntoView {
    let state = expect_context::<AppState>();
    let desktop = state.capabilities.desktop;
    view! {
        <Show when=move || state.settings_open.get()>
            <div class="modal-backdrop" on:click=move |_| state.settings_open.set(false)>
                <div class="modal wide" role="dialog" aria-label="Settings" on:click=|ev| ev.stop_propagation()>
                    <header class="modal-head">
                        <h3>"Settings"</h3>
                        <button class="ghost" aria-label="Close" on:click=move |_| state.settings_open.set(false)>"✕"</button>
                    </header>
                    <section>
                        <h4>"Editor command"</h4>
                        <input
                            type="text"
                            class="mono"
                            placeholder="code -g {file}:{line}:{col}"
                            aria-label="Editor command"
                            disabled=!desktop
                            prop:value=move || state.editor_cmd.get()
                            on:change=move |ev| state.editor_cmd.set(event_target_value(&ev))
                        />
                        <p class="muted small">
                            "Used by Open in editor. " <code>"{file}"</code> ", " <code>"{line}"</code> " and "
                            <code>"{col}"</code>
                            " are filled in; the command runs directly, not through a shell. Leave empty to use your OS default app (no line jump)."
                        </p>
                        <p class="muted small">
                            "Examples: " <code>"code -g {file}:{line}:{col}"</code> " · " <code>"zed {file}:{line}:{col}"</code>
                            " · " <code>"kitty -e nvim +{line} {file}"</code> " · "
                            <code>"alacritty -e hx {file}:{line}:{col}"</code> ". More in docs/EDITORS.md."
                        </p>
                        {(!desktop).then(|| view! { <p class="muted small">"Opening files in an editor requires the desktop app."</p> })}
                    </section>
                </div>
            </div>
        </Show>
    }
}
