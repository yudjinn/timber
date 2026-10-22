//! Watching source folders so the UI can reload them when sources change on disk.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use tauri::{AppHandle, Emitter, State};

/// Quiet period after the last change before telling the UI (editors save in bursts).
const DEBOUNCE: Duration = Duration::from_millis(400);

pub struct Watchers {
    app: AppHandle,
    active: Mutex<HashMap<String, RecommendedWatcher>>,
}

impl Watchers {
    pub fn new(app: AppHandle) -> Self {
        Watchers {
            app,
            active: Mutex::new(HashMap::new()),
        }
    }
}

fn is_source(path: &Path) -> bool {
    timber_core::folder::is_source_file(&path.to_string_lossy())
}

/// Starts watching `path`; the UI gets a `folder-changed` event (payload: the path) after
/// source files in it change.
#[tauri::command]
pub fn watch_folder(path: String, watchers: State<'_, Watchers>) -> Result<(), String> {
    let mut active = watchers.active.lock().map_err(|e| e.to_string())?;
    if active.contains_key(&path) {
        return Ok(());
    }
    let (tx, rx) = channel::<()>();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if let Ok(event) = event
            && !event.kind.is_access()
            && event.paths.iter().any(|p| is_source(p))
        {
            let _ = tx.send(());
        }
    })
    .map_err(|e| e.to_string())?;
    watcher
        .watch(Path::new(&path), RecursiveMode::Recursive)
        .map_err(|e| format!("{path}: {e}"))?;

    let app = watchers.app.clone();
    let folder = path.clone();
    std::thread::spawn(move || {
        // Each change starts (or extends) a quiet period; when it ends, notify once. The
        // thread exits when the watcher (and with it the sender) is dropped.
        while rx.recv().is_ok() {
            loop {
                match rx.recv_timeout(DEBOUNCE) {
                    Ok(()) => continue,
                    Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
            let _ = app.emit("folder-changed", folder.clone());
        }
    });
    active.insert(path, watcher);
    Ok(())
}

#[tauri::command]
pub fn unwatch_folder(path: String, watchers: State<'_, Watchers>) -> Result<(), String> {
    watchers
        .active
        .lock()
        .map_err(|e| e.to_string())?
        .remove(&path);
    Ok(())
}
