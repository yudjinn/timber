//! Where files come from and go to. The web build reads through browser APIs; the desktop
//! build (M3) adds a Tauri implementation with native dialogs, SSH, and editor launch.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use js_sys::{Promise, Uint8Array};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use wasm_bindgen_futures::JsFuture;
use web_sys::{FileList, HtmlInputElement};

use crate::doc::InputFile;

pub type LocalFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    /// SSH, opening files in an editor, folder watching.
    pub desktop: bool,
}

pub trait Backend: Send + Sync {
    fn capabilities(&self) -> Capabilities;
    /// Prompts for files. `Ok(None)` if the user cancelled.
    fn open_files(&self) -> LocalFuture<'_, Result<Option<Vec<InputFile>>, String>>;
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

pub struct WebBackend;

fn js_err(e: JsValue) -> String {
    e.as_string()
        .or_else(|| js_sys::JSON::stringify(&e).ok().and_then(|s| s.as_string()))
        .unwrap_or_else(|| "unknown error".into())
}

/// Reads every file in a browser `FileList` (from an `<input>` or a drop).
pub async fn read_file_list(list: FileList) -> Result<Vec<InputFile>, String> {
    let mut out = Vec::new();
    for i in 0..list.length() {
        let Some(file) = list.item(i) else { continue };
        let buf = JsFuture::from(file.array_buffer()).await.map_err(js_err)?;
        out.push(InputFile {
            name: file.name(),
            data: Uint8Array::new(&buf).to_vec(),
        });
    }
    Ok(out)
}

impl Backend for WebBackend {
    fn capabilities(&self) -> Capabilities {
        Capabilities { desktop: false }
    }

    fn open_files(&self) -> LocalFuture<'_, Result<Option<Vec<InputFile>>, String>> {
        Box::pin(async {
            let document = web_sys::window()
                .and_then(|w| w.document())
                .ok_or("no document")?;
            let input: HtmlInputElement = document
                .create_element("input")
                .map_err(js_err)?
                .unchecked_into();
            input.set_type("file");
            input.set_multiple(true);
            input.set_accept(OPEN_ACCEPT);
            // Resolve with the FileList on change, or null if the picker is cancelled.
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
            if result.is_null() {
                return Ok(None);
            }
            let files = read_file_list(result.unchecked_into()).await?;
            Ok((!files.is_empty()).then_some(files))
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
