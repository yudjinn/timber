//! The Sources panel: the tree shown, source folders and files, build defines, and the
//! source board a DTB is matched to (DESIGN §7a).

use leptos::prelude::*;

use crate::actions::{
    add_folder, choose_board_for_blob, open_files, reload_folder, remove_source, set_defines,
    show_board,
};
use crate::backend::{BackendHandle, FolderAccess};
use crate::doc::Origin;
use crate::sources::{EntryKind, format_defines};
use crate::state::AppState;

/// Boards listed at once in a picker.
const PICKER_LIMIT: usize = 60;

#[component]
fn BoardPicker(#[prop(into)] on_pick: Callback<String>, placeholder: String) -> impl IntoView {
    let state = expect_context::<AppState>();
    let query = RwSignal::new(String::new());
    let results = Memo::new(move |_| {
        let q = query.get().to_lowercase();
        let terms: Vec<&str> = q.split_whitespace().collect();
        if terms.is_empty() {
            return Vec::new();
        }
        state.sources.with(|set| {
            let mut hits: Vec<(String, String)> = set
                .boards
                .iter()
                .filter(|b| !b.overlay)
                .filter(|b| {
                    let hay = format!(
                        "{} {} {}",
                        b.path,
                        b.compatible.join(" "),
                        b.model.as_deref().unwrap_or("")
                    )
                    .to_lowercase();
                    terms.iter().all(|t| hay.contains(t))
                })
                .map(|b| {
                    (
                        b.path.clone(),
                        b.model.clone().unwrap_or_else(|| b.compatible.join(", ")),
                    )
                })
                .collect();
            hits.sort();
            hits.truncate(PICKER_LIMIT);
            hits
        })
    });
    view! {
        <div class="board-picker">
            <input
                type="search"
                placeholder=placeholder
                aria-label="Search boards"
                prop:value=move || query.get()
                on:input=move |ev| query.set(event_target_value(&ev))
            />
            <Show when=move || !query.with(String::is_empty)>
                <ul class="picker-list">
                    {move || {
                        let list = results.get();
                        if list.is_empty() {
                            return view! { <li class="popup-empty">"No boards match"</li> }.into_any();
                        }
                        list.into_iter()
                            .map(|(path, detail)| {
                                let p = path.clone();
                                view! {
                                    <li>
                                        <button class="picker-row" on:click=move |_| {
                                            query.set(String::new());
                                            on_pick.run(p.clone());
                                        }>
                                            <span class="mono">{path}</span>
                                            <span class="muted small">{detail}</span>
                                        </button>
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
pub fn SourcesPanel() -> impl IntoView {
    let state = expect_context::<AppState>();
    let backend = expect_context::<BackendHandle>();
    let close = move || state.sources_open.set(false);
    let b_files = backend.clone();
    let b_folder = backend.clone();
    let b_files2 = backend.clone();
    let b_reload = StoredValue::new(backend.clone());
    let privacy = match backend.0.folder_access() {
        FolderAccess::Direct => "Folders are read in place on this computer; nothing is uploaded.",
        FolderAccess::Picker => {
            "Your browser's prompt says \u{201c}upload\u{201d}, but nothing leaves this computer: files are read inside this page. Chrome or Edge read folders in place instead, and can reload them."
        }
    };

    let shown = move || {
        state.doc.with(|d| match d {
            None => view! { <p class="muted">"Nothing open yet."</p> }.into_any(),
            Some(d) => {
                let what = match d.origin {
                    Origin::Blob => "DTB file",
                    Origin::Board => "board compiled from sources",
                };
                view! {
                    <p>
                        <span class="mono">{d.name.clone()}</span>
                        <span class="muted">{format!(" · {what} · {} nodes", d.tree.len())}</span>
                    </p>
                }
                .into_any()
            }
        })
    };

    let entries = move || {
        let set = state.sources.get();
        if set.is_empty() {
            return view! {
                <p class="muted small">
                    "Add the kernel and BSP folders your boards are built from. Only .dts, .dtsi and .h files are read; in a kernel tree, only the dts, dt-bindings, and uapi directories."
                </p>
                <p class="muted small">
                    "Yocto: add " <code>"tmp/work-shared/<MACHINE>/kernel-source"</code>
                    " (it has your layers' DTS patches applied), or the kernel plus your layer's files directories."
                </p>
            }
            .into_any();
        }
        set.entries
            .iter()
            .map(|e| {
                let name = e.name.clone();
                let reload_name = e.name.clone();
                let reloadable = state.folder_handles.with_value(|h| h.contains_key(&e.name));
                let mut parts = Vec::new();
                parts.push(match (e.kind, e.arches.is_empty()) {
                    (EntryKind::Files, _) => "loose files".to_string(),
                    (EntryKind::Folder, false) => format!("kernel tree · {}", e.arches.join(", ")),
                    (EntryKind::Folder, true) => "folder".to_string(),
                });
                let plural = |n: usize, one: &str| format!("{n} {one}{}", if n == 1 { "" } else { "s" });
                parts.push(plural(e.files, "file"));
                parts.push(plural(e.boards, "board"));
                view! {
                    <li class="source-entry">
                        <div>
                            <div class="mono">{format!("{}/", e.name)}</div>
                            <div class="muted small">{parts.join(" · ")}</div>
                        </div>
                        <div class="row tight">
                            {reloadable.then(|| view! {
                                <button
                                    class="small"
                                    title="Re-read the folder to pick up edits"
                                    on:click=move |_| reload_folder(state, b_reload.get_value(), reload_name.clone())
                                >
                                    "Reload"
                                </button>
                            })}
                            <button class="small" on:click=move |_| remove_source(state, b_reload.get_value(), &name)>
                                "Remove"
                            </button>
                        </div>
                    </li>
                }
            })
            .collect_view()
            .into_any()
    };

    let annotation = move || {
        let doc = state.doc.get()?;
        if doc.origin != Origin::Blob || state.sources.with(|s| s.is_empty()) {
            return None;
        }
        let status = match &doc.link {
            Some(link) => {
                let manual = link.manual;
                view! {
                    <p>
                        <span class="mono">{link.board.clone()}</span>
                        <br />
                        <span class="muted small">
                            {format!("{} · {:.0}% match", link.why, link.confidence * 100.0)}
                        </span>
                        {manual.then(|| view! {
                            <button class="small link-button" on:click=move |_| choose_board_for_blob(state, None)>
                                "Back to automatic"
                            </button>
                        })}
                    </p>
                }
                .into_any()
            }
            None => view! { <p class="muted">"No board in the sources matches this DTB. Pick one below."</p> }.into_any(),
        };
        Some(view! {
            <section>
                <h4>{format!("Source board for {}", doc.short_name())}</h4>
                {status}
                {doc.diff.as_ref().map(|d| {
                    let summary = if d.differing + d.not_in_source == 0 {
                        "No differences from source.".to_string()
                    } else {
                        format!("{} nodes differ from source · {} not in source", d.differing, d.not_in_source)
                    };
                    let any = d.differing + d.not_in_source > 0;
                    view! {
                        <p class="small">
                            {summary}
                            {any.then(|| view! {
                                <button class="small link-button" on:click=move |_| crate::actions::show_differences(state)>
                                    "Show differences"
                                </button>
                            })}
                        </p>
                    }
                })}
                <BoardPicker
                    placeholder="Choose a different board…".to_string()
                    on_pick=move |path: String| choose_board_for_blob(state, Some(path))
                />
            </section>
        })
    };

    view! {
        <Show when=move || state.sources_open.get()>
            <div class="modal-backdrop" on:click=move |_| close()>
                <div class="modal wide sources" role="dialog" aria-label="Sources" on:click=|ev| ev.stop_propagation()>
                    <header class="modal-head">
                        <h3>"Sources"</h3>
                        <button class="ghost" aria-label="Close" on:click=move |_| close()>"✕"</button>
                    </header>

                    <section>
                        <h4>"Tree shown"</h4>
                        {shown}
                        <div class="row">
                            <button on:click={
                                let b = b_files.clone();
                                move |_| open_files(state, b.clone())
                            }>"Open DTB / DTS files…"</button>
                        </div>
                        <Show when=move || !state.sources.with(|s| s.boards.is_empty())>
                            <p class="muted small">"Or show a board from the sources:"</p>
                            <BoardPicker
                                placeholder=format!("Search {} boards by path, compatible, or model…", state.sources.with_untracked(|s| s.boards.len()))
                                on_pick=move |path: String| show_board(state, &path)
                            />
                        </Show>
                    </section>

                    {annotation}

                    <section>
                        <h4>"Source folders and files"</h4>
                        <ul class="plain sources-list">{entries}</ul>
                        <div class="row">
                            <button class="primary" on:click={
                                let b = b_folder.clone();
                                move |_| add_folder(state, b.clone())
                            }>"Add folder…"</button>
                            <button on:click={
                                let b = b_files2.clone();
                                move |_| open_files(state, b.clone())
                            }>"Add files…"</button>
                        </div>
                        <p class="muted small">{privacy}</p>
                    </section>

                    <section>
                        <h4>"Build defines"</h4>
                        <input
                            type="text"
                            class="mono"
                            placeholder="BOARD_REV=2, FEATURE_X"
                            aria-label="Build defines"
                            prop:value=move || state.sources.with(|s| format_defines(&s.defines))
                            on:change=move |ev| set_defines(state, &event_target_value(&ev))
                        />
                        <p class="muted small">"Passed as -D to every compile, like your build does."</p>
                    </section>
                </div>
            </div>
        </Show>
    }
}
