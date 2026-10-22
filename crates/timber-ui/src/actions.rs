//! User actions that load trees and change sources.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use leptos::prelude::*;

use crate::backend::{BackendHandle, FolderFilter, FolderHandle, PickedFolder, Progress};
use crate::doc::{Document, InputFile, split_opened};
use crate::overlays::{OverlayFrom, OverlaySpec, apply_all};
use timber_core::vfs::Vfs;

use crate::sources::{ConfigSource, EntryKind, SourceSet, annotate, folder_filter, parse_defines};
use crate::state::{AppState, Shown};

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
    if let Some(config) = opened.configs.into_iter().next() {
        apply_config(state, &config.name, &config.data);
    }
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
        // Remember where loose files live on disk (desktop) for "Open in editor".
        state.disk_paths.update_value(|m| {
            for f in &opened.sources {
                if let Some(path) = &f.path {
                    let base = f.name.rsplit('/').next().unwrap_or(&f.name);
                    m.insert(format!("files/{base}"), path.clone());
                }
            }
        });
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

fn folder_progress(state: AppState) -> Progress {
    Rc::new(move |done, total| {
        let msg = if total == 0 {
            format!("Scanning folder… {done} files")
        } else {
            format!("Reading {done} / {total} source files…")
        };
        state.busy.set(Some(msg));
    })
}

fn source_files_only() -> FolderFilter {
    Rc::new(|paths: &[String]| Box::new(folder_filter(paths)) as Box<dyn Fn(&str) -> bool>)
}

pub fn add_folder(state: AppState, backend: BackendHandle) {
    leptos::task::spawn_local(async move {
        let result = backend
            .0
            .open_folder(source_files_only(), folder_progress(state))
            .await;
        match result {
            Ok(Some(picked)) if picked.files.is_empty() => {
                state.busy.set(None);
                state.notify("No .dts, .dtsi, or .h files in that folder.", true);
            }
            Ok(Some(picked)) => {
                state.busy.set(Some("Indexing boards…".into()));
                after_paint(move || {
                    let summary = install_folder(state, &backend, picked);
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

/// Adds a picked folder to the sources (watching it where the backend can) and returns a
/// summary for the user.
fn install_folder(state: AppState, backend: &BackendHandle, picked: PickedFolder) -> String {
    let files = picked.files.into_iter().map(|f| (f.name, f.data)).collect();
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
    if let Some(handle) = &picked.handle {
        backend.0.watch_folder(handle);
    }
    // Store the handle before the set, so the panel shows Reload right away.
    state.folder_handles.update_value(|h| {
        match picked.handle {
            Some(handle) => h.insert(name.clone(), handle),
            None => h.remove(&name),
        };
    });
    state.sources.set(Arc::new(set));
    summary
}

/// Reloads the folder entry whose desktop path is `path` (after a change on disk).
pub fn reload_folder_at(state: AppState, backend: BackendHandle, path: String) {
    let name = state.folder_handles.with_value(|h| {
        h.iter()
            .find(|(_, handle)| matches!(handle, FolderHandle::Native(p) if *p == path))
            .map(|(n, _)| n.clone())
    });
    if let Some(name) = name {
        reload_folder(state, backend, name);
    }
}

/// Re-reads a folder opened with direct access, picking up edits made on disk.
pub fn reload_folder(state: AppState, backend: BackendHandle, name: String) {
    let Some(handle) = state.folder_handles.with_value(|h| h.get(&name).cloned()) else {
        return;
    };
    leptos::task::spawn_local(async move {
        match backend
            .0
            .reload_folder(handle, source_files_only(), folder_progress(state))
            .await
        {
            Ok(files) => {
                state.busy.set(Some("Indexing boards…".into()));
                after_paint(move || {
                    let files = files.into_iter().map(|f| (f.name, f.data)).collect();
                    let set = state
                        .sources
                        .get_untracked()
                        .with_folder_reloaded(&name, files);
                    state.sources.set(Arc::new(set));
                    state.busy.set(None);
                    state.notify(format!("Reloaded {name}"), false);
                    refresh(state);
                    RELOADS.with(|r| r.set(r.get() + 1));
                });
            }
            Err(e) => {
                state.busy.set(None);
                state.notify(format!("Couldn't reload {name}: {e}"), true);
            }
        }
    });
}

pub fn add_config(state: AppState, backend: BackendHandle) {
    leptos::task::spawn_local(async move {
        match backend.0.open_config().await {
            Ok(Some(file)) => {
                apply_config(state, &file.name, &file.data);
                refresh(state);
            }
            Ok(None) => {}
            Err(e) => state.notify(format!("Couldn't open the config: {e}"), true),
        }
    });
}

/// Uses a kernel config or Yocto conf to decide which boards are built.
pub fn apply_config(state: AppState, name: &str, data: &[u8]) {
    let name = name.rsplit(['/', '\\']).next().unwrap_or(name);
    match ConfigSource::parse(name, &String::from_utf8_lossy(data)) {
        Ok(config) => {
            let set = state.sources.get_untracked().with_config(Some(config));
            let built = set
                .kbuild
                .targets
                .iter()
                .filter(|t| t.built == Some(true))
                .count();
            state.sources.set(Arc::new(set));
            state.notify(
                format!("Using {name}: {} built", plural(built, "device tree")),
                false,
            );
        }
        Err(e) => state.notify(e, true),
    }
}

pub fn remove_config(state: AppState) {
    let set = state.sources.get_untracked().with_config(None);
    state.sources.set(Arc::new(set));
    refresh(state);
}

pub fn remove_source(state: AppState, backend: BackendHandle, name: &str) {
    state.folder_handles.update_value(|h| {
        if let Some(handle) = h.remove(name) {
            backend.0.unwatch_folder(&handle);
        }
    });
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

/// Uses a composite board (base + overlays) as the source for the DTB shown: the base is
/// pinned and its overlays become the "applied at boot" list.
pub fn choose_composite_for_blob(state: AppState, dtb: &str) {
    let set = state.sources.get_untracked();
    let Some(parts) = set
        .kbuild
        .targets
        .iter()
        .find(|t| t.dtb == dtb)
        .and_then(|t| t.composite.clone())
    else {
        return;
    };
    let Some((base, rest)) = parts.split_first() else {
        return;
    };
    let overlays = rest
        .iter()
        .map(|p| OverlaySpec::source(&format!("{}.dtso", p.trim_end_matches(".dtbo"))))
        .collect();
    state.boot_overlays.set(overlays);
    choose_board_for_blob(
        state,
        Some(format!("{}.dts", base.trim_end_matches(".dtb"))),
    );
}

fn linked(state: AppState, doc: &Document, set: &SourceSet) -> Document {
    if set.is_empty() {
        return doc.relinked(None);
    }
    let forced = board_choice(&doc.name).filter(|b| set.board(b).is_some());
    let boot = state.boot_overlays.get_untracked();
    doc.relinked(
        annotate(&doc.tree, &doc.name, set, forced.as_deref(), &boot).inspect(|link| {
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

/// Builds the document for what's shown: the base (DTB or compiled board), the overlays
/// attached to it, then for DTBs the link to their source board.
fn build(state: AppState) -> Option<Result<Document, String>> {
    let shown = state.shown.get_value()?;
    let set = state.sources.get_untracked();
    let overlays = state.overlays.get_untracked();
    let overlaid = |doc: Document| {
        if overlays.is_empty() {
            doc
        } else {
            let applied = apply_all(doc.tree.clone(), &set, &overlays);
            doc.with_overlays(applied)
        }
    };
    Some(match shown {
        Shown::Blob { name, data } => {
            Document::from_blob(&name, &data).map(|doc| linked(state, &overlaid(doc), &set))
        }
        Shown::Board { path } if !set.vfs.exists(&path) => {
            Err(format!("{path} is no longer in the sources"))
        }
        Shown::Board { path } => {
            // Boards the Makefile builds with `dtc -@` compile the same way; overlays need it too.
            let symbols = set.target(&path).is_some_and(|t| t.symbols_flag) || !overlays.is_empty();
            Ok(overlaid(Document::from_compiled(
                &path,
                set.compile_as(&path, symbols),
            )))
        }
    })
}

/// Rebuilds the document. `fresh` starts a new view (new tree); otherwise the web is kept
/// when the tree's shape is unchanged.
fn rebuild(state: AppState, fresh: bool, busy: &str) {
    state.busy.set(Some(busy.to_string()));
    after_paint(move || {
        let result = build(state);
        state.busy.set(None);
        match result {
            None => {}
            Some(Err(e)) => state.notify(e, true),
            Some(Ok(doc)) => {
                let errors = doc.diagnostics.iter().any(|d| d.line == 0 && d.col == 0);
                let same_shape = state
                    .document()
                    .is_some_and(|d| d.tree.len() == doc.tree.len());
                if fresh {
                    state.set_document(doc);
                } else if !same_shape {
                    state.set_document_keeping_web(doc);
                } else {
                    state.replace_document(doc);
                }
                if errors {
                    state.notify(
                        "Some overlays couldn't be applied; see the compiler messages",
                        true,
                    );
                    state.diagnostics_open.set(true);
                }
            }
        }
    });
}

pub fn show_blob(state: AppState, name: &str, data: &[u8]) {
    crate::ssh::disconnect(state);
    show_live_blob(state, name, Arc::new(data.to_vec()));
}

/// Shows a tree read from a device (without ending its live session).
pub fn show_live_blob(state: AppState, name: &str, data: Arc<Vec<u8>>) {
    state.shown.set_value(Some(Shown::Blob {
        name: name.to_string(),
        data,
    }));
    state.overlays.set(Vec::new());
    state.boot_overlays.set(Vec::new());
    let busy = if state.sources.with_untracked(|s| s.is_empty()) {
        "Opening…"
    } else {
        "Matching source board…"
    };
    rebuild(state, true, busy);
}

/// A device's tree changed: swap it in, keeping overlays and the web.
pub fn replace_live_blob(state: AppState, name: &str, data: Arc<Vec<u8>>) {
    state.shown.set_value(Some(Shown::Blob {
        name: name.to_string(),
        data,
    }));
    rebuild(state, false, "Updating…");
}

pub fn show_board(state: AppState, path: &str) {
    crate::ssh::disconnect(state);
    state.shown.set_value(Some(Shown::Board {
        path: path.to_string(),
    }));
    state.overlays.set(Vec::new());
    state.sources_open.set(false);
    rebuild(
        state,
        true,
        &format!("Compiling {}…", path.rsplit('/').next().unwrap_or(path)),
    );
}

/// Shows a composite board from the Makefiles (`name-dtbs := base.dtb x.dtbo …`): the base
/// with its overlays attached.
pub fn show_composite(state: AppState, dtb: &str) {
    let set = state.sources.get_untracked();
    let Some(parts) = set
        .kbuild
        .targets
        .iter()
        .find(|t| t.dtb == dtb)
        .and_then(|t| t.composite.clone())
    else {
        return state.notify(format!("{dtb} isn't a composite board"), true);
    };
    let Some((base, rest)) = parts.split_first() else {
        return;
    };
    let base_src = format!("{}.dts", base.trim_end_matches(".dtb"));
    let mut overlays = Vec::new();
    for part in rest {
        let src = format!("{}.dtso", part.trim_end_matches(".dtbo"));
        if !set.vfs.exists(&src) {
            return state.notify(format!("{src} isn't in the sources"), true);
        }
        overlays.push(OverlaySpec::source(&src));
    }
    crate::ssh::disconnect(state);
    state.shown.set_value(Some(Shown::Board { path: base_src }));
    state.overlays.set(overlays);
    state.sources_open.set(false);
    rebuild(
        state,
        true,
        &format!("Building {}…", dtb.rsplit('/').next().unwrap_or(dtb)),
    );
}

/// Picks overlay files (`.dtbo`, or `.dtso` added to the sources) and attaches them to the
/// tree shown, or with `boot`, to a DTB's source board.
pub fn add_overlay_files(state: AppState, backend: BackendHandle, boot: bool) {
    leptos::task::spawn_local(async move {
        let files = match backend.0.open_files().await {
            Ok(Some(files)) => files,
            Ok(None) => return,
            Err(e) => return state.notify(format!("Couldn't open files: {e}"), true),
        };
        let mut specs = Vec::new();
        let mut sources = Vec::new();
        for f in files {
            if timber_core::dtb::is_dtb(&f.data) {
                specs.push(OverlaySpec {
                    name: f.name.clone(),
                    from: OverlayFrom::Blob(Arc::new(f.data)),
                });
            } else if f.name.ends_with(".dtso") {
                let base = f.name.rsplit('/').next().unwrap_or(&f.name).to_string();
                specs.push(OverlaySpec::source(&format!("files/{base}")));
                sources.push((f.name, f.data));
            } else {
                state.notify(
                    format!("{} isn't an overlay (.dtbo or .dtso)", f.name),
                    true,
                );
            }
        }
        if !sources.is_empty() {
            let (set, _) = state
                .sources
                .get_untracked()
                .with_files(EntryKind::Files, sources);
            state.sources.set(Arc::new(set));
        }
        let list = if boot {
            state.boot_overlays
        } else {
            state.overlays
        };
        list.update(|l| l.extend(specs));
        refresh(state);
    });
}

/// Replaces an overlay list (reorder or remove) and rebuilds.
pub fn set_overlay_list(state: AppState, boot: bool, list: Vec<OverlaySpec>) {
    if boot {
        state.boot_overlays.set(list);
    } else {
        state.overlays.set(list);
    }
    refresh(state);
}

/// Re-derives the tree shown after sources, defines, or overlays changed.
pub fn refresh(state: AppState) {
    rebuild(state, false, "Updating…");
}

/// Adds every node that differs from source (and the path to it) to the web, then fits.
pub fn show_differences(state: AppState) {
    let Some(doc) = state.document() else { return };
    let Some(diff) = &doc.diff else { return };
    let marked: Vec<_> = (0..doc.tree.len())
        .filter(|&id| diff.is_marked(id))
        .collect();
    for &id in &marked {
        state.graph.reveal(&doc.tree, id);
    }
    state.sources_open.set(false);
    state.popup.set(None);
    let tree = doc.tree.clone();
    set_timeout(move || state.graph.fit(&tree), Duration::from_millis(600));
    state.notify(
        format!("Showing {} nodes that differ from source", marked.len()),
        false,
    );
}

pub(crate) async fn sleep(ms: i32) {
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        if let Some(w) = web_sys::window() {
            let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms);
        }
    });
    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
}

/// Desktop: loads what the app was launched with (`timber FILE… --source DIR…`). With
/// `--smoke`, prints a summary of what loaded and exits (an end-to-end test of the app).
pub fn run_startup(state: AppState, backend: BackendHandle) {
    leptos::task::spawn_local(async move {
        let Some(start) = crate::tauri::startup().await else {
            return;
        };
        for dir in &start.sources {
            let handle = FolderHandle::Native(dir.clone());
            match backend
                .0
                .reload_folder(handle.clone(), source_files_only(), folder_progress(state))
                .await
            {
                Ok(files) if !files.is_empty() => {
                    let summary = install_folder(
                        state,
                        &backend,
                        PickedFolder {
                            files,
                            handle: Some(handle),
                        },
                    );
                    state.notify(summary, false);
                }
                Ok(_) => state.notify(format!("No .dts, .dtsi, or .h files in {dir}"), true),
                Err(e) => state.notify(format!("Couldn't read {dir}: {e}"), true),
            }
        }
        state.busy.set(None);
        if let Some(config) = &start.config {
            match crate::tauri::read_paths(std::slice::from_ref(config)).await {
                Ok(files) => {
                    if let Some(f) = files.into_iter().next() {
                        apply_config(state, &f.name, &f.data);
                    }
                }
                Err(e) => state.notify(format!("Couldn't read {config}: {e}"), true),
            }
        }
        if !start.files.is_empty() {
            match crate::tauri::read_paths(&start.files).await {
                Ok(files) => add_files(state, files),
                Err(e) => state.notify(format!("Couldn't open files: {e}"), true),
            }
        }
        let mut ssh_error = None;
        if let Some(request) = start.ssh.clone() {
            ssh_error = crate::ssh::connect_at_start(state, request).await.err();
        }
        if let Some(e) = &ssh_error
            && (start.smoke || start.smoke_watch)
        {
            crate::tauri::report(&format!("timber-smoke ssh-error={e}")).await;
        }
        let expect_doc = !start.files.is_empty() || (start.ssh.is_some() && ssh_error.is_none());
        if start.smoke || start.smoke_watch {
            smoke_report(state, expect_doc).await;
        }
        if start.smoke_watch {
            // Wait for a watched folder to reload, then report again.
            let reloads = RELOADS.with(Cell::get);
            while RELOADS.with(Cell::get) == reloads {
                sleep(100).await;
            }
            smoke_report(state, expect_doc).await;
        }
        if start.smoke || start.smoke_watch {
            crate::tauri::quit().await;
        }
    });
}

async fn smoke_report(state: AppState, expect_doc: bool) {
    for _ in 0..600 {
        sleep(100).await;
        let settled = state.busy.get_untracked().is_none()
            && (!expect_doc || state.doc.with_untracked(Option::is_some));
        if settled {
            break;
        }
    }
    let sources = state.sources.get_untracked();
    let mut line = format!(
        "timber-smoke sources={} boards={}",
        sources.entries.len(),
        sources.boards.len()
    );
    if sources.config.is_some() {
        let built: Vec<_> = sources
            .kbuild
            .targets
            .iter()
            .filter(|t| t.built == Some(true))
            .collect();
        let composites = built.iter().filter(|t| t.composite.is_some()).count();
        line.push_str(&format!(" built={} composites={composites}", built.len()));
    }
    if let Some(live) = state.ssh.live.get_untracked() {
        line.push_str(&format!(" live={} from={}", live.label, live.path));
    }
    if let Some(doc) = state.document() {
        line.push_str(&format!(
            " doc={} nodes={} errors={}",
            doc.short_name(),
            doc.tree.len(),
            doc.error_count()
        ));
        if let Some(link) = &doc.link {
            line.push_str(&format!(
                " board={} match={:.0}%",
                link.board,
                link.confidence * 100.0
            ));
        }
        if let Some(d) = &doc.diff {
            line.push_str(&format!(
                " differ={} not_in_source={}",
                d.differing, d.not_in_source
            ));
        }
    }
    crate::tauri::report(&line).await;
}

thread_local! {
    /// Completed folder reloads, so the `--smoke-watch` run can wait for one.
    static RELOADS: Cell<u32> = const { Cell::new(0) };
}
