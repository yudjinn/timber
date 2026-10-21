//! User actions that load trees and change sources.

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use leptos::prelude::*;

use crate::backend::{BackendHandle, Progress};
use crate::doc::{Document, InputFile, Origin, split_opened};
use timber_core::vfs::Vfs;

use crate::sources::{EntryKind, SourceSet, annotate, folder_filter, parse_defines};
use crate::state::AppState;

const BOARD_CHOICE_KEY: &str = "timber.board:";

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

/// Runs `f` after the browser has had a chance to paint (e.g. a "busy" message).
fn after_paint(f: impl FnOnce() + 'static) {
    set_timeout(f, Duration::from_millis(30));
}

fn plural(n: usize, one: &str) -> String {
    format!("{n} {one}{}", if n == 1 { "" } else { "s" })
}

pub fn open_files(state: AppState, backend: BackendHandle) {
    leptos::task::spawn_local(async move {
        match backend.0.open_files().await {
            Ok(Some(files)) => add_files(state, files),
            Ok(None) => {}
            Err(e) => state.notify(format!("Couldn't open files: {e}"), true),
        }
    });
}

/// Handles opened or dropped files: a DTB becomes the tree shown; sources join the
/// "files" entry, and a single new board among them is shown.
pub fn add_files(state: AppState, files: Vec<InputFile>) {
    let opened = split_opened(files);
    if !opened.ignored.is_empty() {
        state.notify(format!("Ignored {}", opened.ignored.join(", ")), false);
    }
    let mut new_boards = Vec::new();
    if !opened.sources.is_empty() {
        for f in &opened.sources {
            let base = f.name.rsplit('/').next().unwrap_or(&f.name);
            if base.ends_with(".dts") || base.ends_with(".dtso") {
                new_boards.push(format!("files/{base}"));
            }
        }
        let files = opened
            .sources
            .into_iter()
            .map(|f| (f.name, f.data))
            .collect();
        let (set, _) = state
            .sources
            .get_untracked()
            .with_files(EntryKind::Files, files);
        state.sources.set(Arc::new(set));
    }
    if let Some(blob) = opened.blobs.into_iter().next() {
        show_blob(state, &blob.name, &blob.data);
        return;
    }
    match new_boards.as_slice() {
        [] => refresh(state),
        [one] => show_board(state, one),
        _ => state.choose.set(Some(new_boards)),
    }
}

pub fn add_folder(state: AppState, backend: BackendHandle) {
    leptos::task::spawn_local(async move {
        let progress: Progress = Rc::new(move |done, total| {
            state
                .busy
                .set(Some(format!("Reading {done} / {total} source files…")))
        });
        let want =
            Rc::new(|paths: &[String]| Box::new(folder_filter(paths)) as Box<dyn Fn(&str) -> bool>);
        let result = backend.0.open_folder(want, progress).await;
        match result {
            Ok(Some(files)) if files.is_empty() => {
                state.busy.set(None);
                state.notify("No .dts, .dtsi, or .h files in that folder.", true);
            }
            Ok(Some(files)) => {
                state.busy.set(Some("Indexing boards…".into()));
                after_paint(move || {
                    let files = files.into_iter().map(|f| (f.name, f.data)).collect();
                    let (set, name) = state
                        .sources
                        .get_untracked()
                        .with_files(EntryKind::Folder, files);
                    let summary = set
                        .entries
                        .iter()
                        .find(|e| e.name == name)
                        .map(|e| {
                            format!(
                                "Added {name}: {}, {}",
                                plural(e.files, "file"),
                                plural(e.boards, "board")
                            )
                        })
                        .unwrap_or_default();
                    state.sources.set(Arc::new(set));
                    state.busy.set(None);
                    state.notify(summary, false);
                    refresh(state);
                });
            }
            Ok(None) => state.busy.set(None),
            Err(e) => {
                state.busy.set(None);
                state.notify(format!("Couldn't read the folder: {e}"), true);
            }
        }
    });
}

pub fn remove_source(state: AppState, name: &str) {
    let set = state.sources.get_untracked().without(name);
    state.sources.set(Arc::new(set));
    refresh(state);
}

pub fn set_defines(state: AppState, text: &str) {
    let defines = parse_defines(text);
    if defines == state.sources.get_untracked().defines {
        return;
    }
    let set = state.sources.get_untracked().with_defines(defines);
    state.sources.set(Arc::new(set));
    refresh(state);
}

fn board_choice(blob_name: &str) -> Option<String> {
    storage()?
        .get_item(&format!("{BOARD_CHOICE_KEY}{blob_name}"))
        .ok()
        .flatten()
}

/// Pins the source board for the blob being shown (`None` returns to automatic matching).
pub fn choose_board_for_blob(state: AppState, board: Option<String>) {
    let Some(doc) = state.document() else { return };
    if let Some(s) = storage() {
        let key = format!("{BOARD_CHOICE_KEY}{}", doc.name);
        let _ = match &board {
            Some(b) => s.set_item(&key, b),
            None => s.remove_item(&key),
        };
    }
    refresh(state);
}

fn linked(state: AppState, doc: &Document, set: &SourceSet) -> Document {
    if set.is_empty() {
        return doc.relinked(None);
    }
    let forced = board_choice(&doc.name).filter(|b| set.board(b).is_some());
    doc.relinked(
        annotate(&doc.tree, &doc.name, set, forced.as_deref()).inspect(|link| {
            if link.confidence < 0.5 {
                state.notify(
                    format!(
                        "Weak source match ({:.0}%). Pick the board in Sources if it's wrong.",
                        link.confidence * 100.0
                    ),
                    false,
                );
            }
        }),
    )
}

pub fn show_blob(state: AppState, name: &str, data: &[u8]) {
    let doc = match Document::from_blob(name, data) {
        Ok(doc) => doc,
        Err(e) => return state.notify(e, true),
    };
    let set = state.sources.get_untracked();
    if set.is_empty() {
        let nodes = doc.tree.len();
        state.set_document(doc);
        state.notify(format!("Opened {name}: {nodes} nodes"), false);
        return;
    }
    state.busy.set(Some("Matching source board…".into()));
    after_paint(move || {
        let doc = linked(state, &doc, &set);
        state.busy.set(None);
        state.set_document(doc);
    });
}

pub fn show_board(state: AppState, path: &str) {
    let set = state.sources.get_untracked();
    let path = path.to_string();
    state.busy.set(Some(format!(
        "Compiling {}…",
        path.rsplit('/').next().unwrap_or(&path)
    )));
    after_paint(move || {
        let doc = Document::from_compiled(&path, set.compile(&path));
        state.busy.set(None);
        state.set_document(doc);
        state.sources_open.set(false);
    });
}

/// Re-derives the tree shown after sources or defines changed.
pub fn refresh(state: AppState) {
    let Some(doc) = state.document() else { return };
    let set = state.sources.get_untracked();
    state.busy.set(Some(match doc.origin {
        Origin::Blob => "Matching source board…".into(),
        Origin::Board => "Recompiling…".into(),
    }));
    after_paint(move || {
        state.busy.set(None);
        match doc.origin {
            Origin::Blob => state.replace_document(linked(state, &doc, &set)),
            Origin::Board if set.vfs.exists(&doc.name) => {
                let new = Document::from_compiled(&doc.name, set.compile(&doc.name));
                // Same shape keeps the web; otherwise node ids may have shifted.
                if new.tree.len() == doc.tree.len() {
                    state.replace_document(new);
                } else {
                    state.set_document(new);
                }
            }
            Origin::Board => {}
        }
    });
}
