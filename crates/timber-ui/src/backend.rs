//! Where files come from and go to. The web build reads through browser APIs; the desktop
//! build (M3) adds a Tauri implementation with native dialogs, SSH, and editor launch.

use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;

use js_sys::{Array, Promise, Uint8Array};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use wasm_bindgen_futures::JsFuture;
use web_sys::{File, FileList, HtmlInputElement};

use crate::doc::InputFile;

pub type LocalFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

/// Reports `(done, total)` while reading many files.
pub type Progress = Rc<dyn Fn(usize, usize)>;

/// Given every path in a picked folder, returns which ones to read.
pub type FolderFilter = Rc<dyn Fn(&[String]) -> Box<dyn Fn(&str) -> bool>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    /// SSH, opening files in an editor, folder watching.
    pub desktop: bool,
}

/// How folders are opened, which decides what the UI tells the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderAccess {
    /// Read in place and can be re-read (directory access API, native dialogs).
    Direct,
    /// The browser's folder picker, whose prompt says "upload" even though nothing is.
    Picker,
}

/// A way to re-read a folder that was opened earlier.
#[derive(Clone)]
pub enum FolderHandle {
    /// A `FileSystemDirectoryHandle`.
    Browser(JsValue),
    /// A folder path on disk (desktop app).
    Native(String),
}

pub struct PickedFolder {
    /// Paths start with the folder's own name.
    pub files: Vec<InputFile>,
    pub handle: Option<FolderHandle>,
}

pub trait Backend: Send + Sync {
    fn capabilities(&self) -> Capabilities;
    fn folder_access(&self) -> FolderAccess;
    /// Prompts for files. `Ok(None)` if the user cancelled.
    fn open_files(&self) -> LocalFuture<'_, Result<Option<Vec<InputFile>>, String>>;
    /// Prompts for a folder and reads the files `want` accepts (it sees every path first,
    /// relative to the folder's parent, so it can decide by layout). File names in the
    /// result are those relative paths.
    fn open_folder(
        &self,
        want: FolderFilter,
        progress: Progress,
    ) -> LocalFuture<'_, Result<Option<PickedFolder>, String>>;
    /// Re-reads a folder opened earlier (after edits on disk).
    fn reload_folder(
        &self,
        handle: FolderHandle,
        want: FolderFilter,
        progress: Progress,
    ) -> LocalFuture<'_, Result<Vec<InputFile>, String>>;
    /// Saves a file; `Ok(false)` if the user cancelled.
    fn save_file(
        &self,
        name: &str,
        mime: &str,
        data: Vec<u8>,
    ) -> LocalFuture<'_, Result<bool, String>>;
    /// Starts reporting changes to a folder (desktop only).
    fn watch_folder(&self, _handle: &FolderHandle) {}
    fn unwatch_folder(&self, _handle: &FolderHandle) {}
    /// Opens a file on disk in the user's editor (`template` per docs/EDITORS.md; empty uses
    /// the OS default application).
    fn open_in_editor(
        &self,
        _path: &str,
        _line: u32,
        _col: u32,
        _template: &str,
    ) -> LocalFuture<'_, Result<(), String>> {
        Box::pin(async { Err("opening files in an editor requires the desktop app".to_string()) })
    }
}

#[derive(Clone)]
pub struct BackendHandle(pub Arc<dyn Backend>);

pub const OPEN_ACCEPT: &str = ".dtb,.dtbo,.dts,.dtsi,.dtso,.h";

/// Files read concurrently per batch.
const READ_BATCH: usize = 64;

pub struct WebBackend;

pub(crate) fn js_err(e: JsValue) -> String {
    e.as_string()
        .or_else(|| js_sys::JSON::stringify(&e).ok().and_then(|s| s.as_string()))
        .unwrap_or_else(|| "unknown error".into())
}

/// Reads files in concurrent batches, reporting progress.
async fn read_files(
    files: Vec<(String, File)>,
    progress: Option<Progress>,
) -> Result<Vec<InputFile>, String> {
    let total = files.len();
    let mut out = Vec::with_capacity(total);
    for batch in files.chunks(READ_BATCH) {
        let promises: Array = batch
            .iter()
            .map(|(_, f)| JsValue::from(f.array_buffer()))
            .collect();
        let buffers: Array = JsFuture::from(Promise::all(&promises))
            .await
            .map_err(js_err)?
            .unchecked_into();
        for ((name, _), buf) in batch.iter().zip(buffers.iter()) {
            out.push(InputFile {
                name: name.clone(),
                data: Uint8Array::new(&buf).to_vec(),
                path: None,
            });
        }
        if let Some(p) = &progress {
            p(out.len(), total);
        }
    }
    Ok(out)
}

/// Reads every file in a browser `FileList` (from an `<input>` or a drop).
pub async fn read_file_list(list: FileList) -> Result<Vec<InputFile>, String> {
    let files = (0..list.length())
        .filter_map(|i| list.item(i))
        .map(|f| (f.name(), f))
        .collect();
    read_files(files, None).await
}

/// Shows a file picker; resolves to the chosen files, or `None` if cancelled.
async fn pick(directory: bool) -> Result<Option<FileList>, String> {
    let document = web_sys::window()
        .and_then(|w| w.document())
        .ok_or("no document")?;
    let input: HtmlInputElement = document
        .create_element("input")
        .map_err(js_err)?
        .unchecked_into();
    input.set_type("file");
    if directory {
        input.set_webkitdirectory(true);
    } else {
        input.set_multiple(true);
        input.set_accept(OPEN_ACCEPT);
    }
    let picked = Promise::new(&mut |resolve, _reject| {
        let on_change = {
            let input = input.clone();
            let resolve = resolve.clone();
            Closure::once_into_js(move || {
                let files = input.files().map_or(JsValue::NULL, JsValue::from);
                let _ = resolve.call1(&JsValue::NULL, &files);
            })
        };
        let on_cancel = Closure::once_into_js(move || {
            let _ = resolve.call1(&JsValue::NULL, &JsValue::NULL);
        });
        input.set_onchange(Some(on_change.unchecked_ref()));
        let _ = input.add_event_listener_with_callback("cancel", on_cancel.unchecked_ref());
    });
    input.click();
    let result = JsFuture::from(picked).await.map_err(js_err)?;
    Ok((!result.is_null()).then(|| result.unchecked_into()))
}

fn relative_path(file: &File) -> String {
    js_sys::Reflect::get(file, &JsValue::from_str("webkitRelativePath"))
        .ok()
        .and_then(|v| v.as_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| file.name())
}

// ---- directory access API (Chromium), called dynamically: web-sys only exposes it behind
// its unstable-API flag ----

pub(crate) fn get(v: &JsValue, key: &str) -> JsValue {
    js_sys::Reflect::get(v, &JsValue::from_str(key)).unwrap_or(JsValue::UNDEFINED)
}

pub(crate) fn method(v: &JsValue, name: &str) -> Result<js_sys::Function, String> {
    get(v, name)
        .dyn_into()
        .map_err(|_| format!("{name} is not supported"))
}

pub(crate) fn call0(v: &JsValue, name: &str) -> Result<JsValue, String> {
    method(v, name)?.call0(v).map_err(js_err)
}

pub(crate) fn call1(v: &JsValue, name: &str, arg: &JsValue) -> Result<JsValue, String> {
    method(v, name)?.call1(v, arg).map_err(js_err)
}

pub(crate) async fn await_js(v: JsValue) -> Result<JsValue, String> {
    let promise: Promise = v.dyn_into().map_err(|_| "expected a promise".to_string())?;
    JsFuture::from(promise).await.map_err(js_err)
}

fn read_mode() -> JsValue {
    let opts = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&opts, &"mode".into(), &"read".into());
    opts.into()
}

pub fn has_directory_picker() -> bool {
    web_sys::window().is_some_and(|w| get(&w, "showDirectoryPicker").is_function())
}

async fn pick_directory() -> Result<Option<JsValue>, String> {
    let window: JsValue = web_sys::window().ok_or("no window")?.into();
    let opts = read_mode();
    // Lets the browser reopen the picker where the user last chose a source folder.
    let _ = js_sys::Reflect::set(&opts, &"id".into(), &"timber-sources".into());
    let promise = call1(&window, "showDirectoryPicker", &opts)?;
    match JsFuture::from(
        promise
            .dyn_into::<Promise>()
            .map_err(|_| "expected a promise")?,
    )
    .await
    {
        Ok(handle) => Ok(Some(handle)),
        Err(e) if get(&e, "name").as_string().as_deref() == Some("AbortError") => Ok(None),
        Err(e) => Err(js_err(e)),
    }
}

/// Walks a directory handle, skipping hidden directories and, inside a kernel tree,
/// everything dts builds don't use. Returns `(path, file handle)` with paths starting at
/// the folder's own name.
async fn walk_directory(
    root: &JsValue,
    progress: &Progress,
) -> Result<Vec<(String, JsValue)>, String> {
    let root_name = get(root, "name")
        .as_string()
        .unwrap_or_else(|| "folder".into());
    // (directory handle, path, path relative to an enclosing kernel root)
    let mut stack: Vec<(JsValue, String, Option<String>)> = vec![(root.clone(), root_name, None)];
    let mut files = Vec::new();
    while let Some((dir, path, kernel)) = stack.pop() {
        let entries = call0(&dir, "values")?;
        let mut children = Vec::new();
        loop {
            let step = await_js(call0(&entries, "next")?).await?;
            if get(&step, "done").as_bool() == Some(true) {
                break;
            }
            let entry = get(&step, "value");
            let name = get(&entry, "name").as_string().unwrap_or_default();
            let is_dir = get(&entry, "kind").as_string().as_deref() == Some("directory");
            children.push((is_dir, name, entry));
        }
        let listing: Vec<(bool, &str)> =
            children.iter().map(|(d, n, _)| (*d, n.as_str())).collect();
        let kernel =
            kernel.or_else(|| timber_core::folder::is_kernel_root(&listing).then(String::new));
        for (is_dir, name, entry) in children {
            let child_path = format!("{path}/{name}");
            if !is_dir {
                files.push((child_path, entry));
                if files.len() % 500 == 0 {
                    progress(files.len(), 0);
                }
                continue;
            }
            if name.starts_with('.') {
                continue;
            }
            let child_kernel = kernel.as_ref().map(|k| {
                if k.is_empty() {
                    name.clone()
                } else {
                    format!("{k}/{name}")
                }
            });
            if child_kernel
                .as_deref()
                .is_some_and(|rel| !timber_core::folder::kernel_dir_wanted(rel))
            {
                continue;
            }
            stack.push((entry, child_path, child_kernel));
        }
    }
    Ok(files)
}

async fn read_directory(
    dir: &JsValue,
    want: FolderFilter,
    progress: Progress,
) -> Result<Vec<InputFile>, String> {
    let handles = walk_directory(dir, &progress).await?;
    let paths: Vec<String> = handles.iter().map(|(p, _)| p.clone()).collect();
    let keep = want(&paths);
    let wanted: Vec<(String, JsValue)> = handles.into_iter().filter(|(p, _)| keep(p)).collect();
    let mut files = Vec::with_capacity(wanted.len());
    for batch in wanted.chunks(READ_BATCH) {
        let getters: Array = batch
            .iter()
            .map(|(_, h)| call0(h, "getFile"))
            .collect::<Result<_, _>>()?;
        let blobs: Array = await_js(Promise::all(&getters).into())
            .await?
            .unchecked_into();
        files.extend(
            batch
                .iter()
                .map(|(p, _)| p.clone())
                .zip(blobs.iter().map(|b| b.unchecked_into::<File>())),
        );
    }
    read_files(files, Some(progress)).await
}

impl Backend for WebBackend {
    fn capabilities(&self) -> Capabilities {
        Capabilities { desktop: false }
    }

    fn open_files(&self) -> LocalFuture<'_, Result<Option<Vec<InputFile>>, String>> {
        Box::pin(async {
            let Some(list) = pick(false).await? else {
                return Ok(None);
            };
            let files = read_file_list(list).await?;
            Ok((!files.is_empty()).then_some(files))
        })
    }

    fn folder_access(&self) -> FolderAccess {
        if has_directory_picker() {
            FolderAccess::Direct
        } else {
            FolderAccess::Picker
        }
    }

    fn open_folder(
        &self,
        want: FolderFilter,
        progress: Progress,
    ) -> LocalFuture<'_, Result<Option<PickedFolder>, String>> {
        Box::pin(async move {
            if has_directory_picker() {
                let Some(dir) = pick_directory().await? else {
                    return Ok(None);
                };
                let files = read_directory(&dir, want, progress).await?;
                return Ok(Some(PickedFolder {
                    files,
                    handle: Some(FolderHandle::Browser(dir)),
                }));
            }
            let Some(list) = pick(true).await? else {
                return Ok(None);
            };
            let all: Vec<(String, File)> = (0..list.length())
                .filter_map(|i| list.item(i))
                .map(|f| (relative_path(&f), f))
                .collect();
            let paths: Vec<String> = all.iter().map(|(p, _)| p.clone()).collect();
            let keep = want(&paths);
            let wanted: Vec<(String, File)> = all.into_iter().filter(|(p, _)| keep(p)).collect();
            let files = read_files(wanted, Some(progress)).await?;
            Ok(Some(PickedFolder {
                files,
                handle: None,
            }))
        })
    }

    fn reload_folder(
        &self,
        handle: FolderHandle,
        want: FolderFilter,
        progress: Progress,
    ) -> LocalFuture<'_, Result<Vec<InputFile>, String>> {
        Box::pin(async move {
            let FolderHandle::Browser(dir) = handle else {
                return Err("this folder can't be reloaded in the browser".into());
            };
            // Permission lapses between sessions; ask again (this runs from a click).
            let opts = read_mode();
            let state = await_js(call1(&dir, "queryPermission", &opts)?).await?;
            if state.as_string().as_deref() != Some("granted") {
                let state = await_js(call1(&dir, "requestPermission", &opts)?).await?;
                if state.as_string().as_deref() != Some("granted") {
                    return Err("permission to read the folder was not granted".into());
                }
            }
            read_directory(&dir, want, progress).await
        })
    }

    fn save_file(
        &self,
        name: &str,
        mime: &str,
        data: Vec<u8>,
    ) -> LocalFuture<'_, Result<bool, String>> {
        let (name, mime) = (name.to_string(), mime.to_string());
        Box::pin(async move {
            let document = web_sys::window()
                .and_then(|w| w.document())
                .ok_or("no document")?;
            let parts = js_sys::Array::of1(&Uint8Array::from(data.as_slice()));
            let options = web_sys::BlobPropertyBag::new();
            options.set_type(&mime);
            let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &options)
                .map_err(js_err)?;
            let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(js_err)?;
            let a: web_sys::HtmlAnchorElement = document
                .create_element("a")
                .map_err(js_err)?
                .unchecked_into();
            a.set_href(&url);
            a.set_download(&name);
            a.click();
            let _ = web_sys::Url::revoke_object_url(&url);
            Ok(true)
        })
    }
}
