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

pub trait Backend: Send + Sync {
    fn capabilities(&self) -> Capabilities;
    /// Prompts for files. `Ok(None)` if the user cancelled.
    fn open_files(&self) -> LocalFuture<'_, Result<Option<Vec<InputFile>>, String>>;
    /// Prompts for a folder and reads the files `want` accepts (it sees every path first,
    /// relative to the folder's parent, so it can decide by layout). File names in the
    /// result are those relative paths.
    fn open_folder(
        &self,
        want: FolderFilter,
        progress: Progress,
    ) -> LocalFuture<'_, Result<Option<Vec<InputFile>>, String>>;
    fn save_file(
        &self,
        name: &str,
        mime: &str,
        data: Vec<u8>,
    ) -> LocalFuture<'_, Result<(), String>>;
}

#[derive(Clone)]
pub struct BackendHandle(pub Arc<dyn Backend>);

pub const OPEN_ACCEPT: &str = ".dtb,.dtbo,.dts,.dtsi,.dtso,.h";

/// Files read concurrently per batch.
const READ_BATCH: usize = 64;

pub struct WebBackend;

fn js_err(e: JsValue) -> String {
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

    fn open_folder(
        &self,
        want: FolderFilter,
        progress: Progress,
    ) -> LocalFuture<'_, Result<Option<Vec<InputFile>>, String>> {
        Box::pin(async move {
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
            read_files(wanted, Some(progress)).await.map(Some)
        })
    }

    fn save_file(
        &self,
        name: &str,
        mime: &str,
        data: Vec<u8>,
    ) -> LocalFuture<'_, Result<(), String>> {
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
            Ok(())
        })
    }
}
