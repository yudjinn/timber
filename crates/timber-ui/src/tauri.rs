//! The desktop backend: talks to the Tauri app's commands through its global API
//! (`window.__TAURI__`, enabled by `withGlobalTauri`).

use js_sys::{Array, Object, Reflect, Uint8Array};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};

use crate::backend::{
    Backend, Capabilities, FolderAccess, FolderFilter, FolderHandle, LocalFuture, PickedFolder,
    Progress, await_js, get, js_err, method,
};
use crate::doc::InputFile;

fn tauri() -> Option<JsValue> {
    let window: JsValue = web_sys::window()?.into();
    Some(get(&window, "__TAURI__")).filter(|t| !t.is_undefined())
}

/// Whether the UI is running inside the desktop app.
pub fn in_tauri() -> bool {
    tauri().is_some()
}

fn object(fields: &[(&str, JsValue)]) -> JsValue {
    let obj = Object::new();
    for (k, v) in fields {
        let _ = Reflect::set(&obj, &JsValue::from_str(k), v);
    }
    obj.into()
}

async fn invoke(cmd: &str, args: JsValue) -> Result<JsValue, String> {
    let core = get(&tauri().ok_or("not running in the desktop app")?, "core");
    let promise = method(&core, "invoke")?
        .call2(&core, &JsValue::from_str(cmd), &args)
        .map_err(js_err)?;
    await_js(promise).await
}

/// Unpacks the desktop app's file batch (see `timber-desktop/src/files.rs`). `None` if the
/// user cancelled (empty response).
fn unpack(value: JsValue) -> Result<Option<(String, Vec<InputFile>)>, String> {
    struct Cursor {
        bytes: Vec<u8>,
        pos: usize,
    }
    impl Cursor {
        fn take(&mut self, n: usize) -> Result<&[u8], String> {
            let slice = self
                .bytes
                .get(self.pos..self.pos + n)
                .ok_or("truncated file batch")?;
            self.pos += n;
            Ok(slice)
        }
        fn u32(&mut self) -> Result<usize, String> {
            let b = self.take(4)?;
            Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
        }
        fn string(&mut self) -> Result<String, String> {
            let n = self.u32()?;
            Ok(String::from_utf8_lossy(self.take(n)?).into_owned())
        }
    }
    let bytes = Uint8Array::new(&value).to_vec();
    if bytes.is_empty() {
        return Ok(None);
    }
    let mut c = Cursor { bytes, pos: 0 };
    if c.take(4)? != b"TMBR" {
        return Err("unexpected response from the desktop app".into());
    }
    let _version = c.u32()?;
    let handle = c.string()?;
    let count = c.u32()?;
    let mut files = Vec::with_capacity(count);
    for _ in 0..count {
        let name = c.string()?;
        let len = c.u32()?;
        files.push(InputFile {
            name,
            data: c.take(len)?.to_vec(),
        });
    }
    Ok(Some((handle, files)))
}

pub struct TauriBackend;

impl Backend for TauriBackend {
    fn capabilities(&self) -> Capabilities {
        Capabilities { desktop: true }
    }

    fn folder_access(&self) -> FolderAccess {
        FolderAccess::Direct
    }

    fn open_files(&self) -> LocalFuture<'_, Result<Option<Vec<InputFile>>, String>> {
        Box::pin(async {
            Ok(unpack(invoke("pick_files", object(&[])).await?)?.map(|(_, files)| files))
        })
    }

    // The desktop app applies the same folder rules natively, so `want` isn't needed.
    fn open_folder(
        &self,
        _want: FolderFilter,
        progress: Progress,
    ) -> LocalFuture<'_, Result<Option<PickedFolder>, String>> {
        Box::pin(async move {
            let picked = invoke("pick_folder", object(&[])).await?;
            progress(0, 0);
            Ok(unpack(picked)?.map(|(path, files)| PickedFolder {
                files,
                handle: Some(FolderHandle::Native(path)),
            }))
        })
    }

    fn reload_folder(
        &self,
        handle: FolderHandle,
        _want: FolderFilter,
        _progress: Progress,
    ) -> LocalFuture<'_, Result<Vec<InputFile>, String>> {
        Box::pin(async move {
            let FolderHandle::Native(path) = handle else {
                return Err("not a desktop folder".into());
            };
            let value =
                invoke("read_folder", object(&[("path", JsValue::from_str(&path))])).await?;
            Ok(unpack(value)?.map(|(_, files)| files).unwrap_or_default())
        })
    }

    fn save_file(
        &self,
        name: &str,
        _mime: &str,
        data: Vec<u8>,
    ) -> LocalFuture<'_, Result<bool, String>> {
        let name = name.to_string();
        Box::pin(async move {
            // Raw body (no JSON number array); the name travels in a header.
            let core = get(&tauri().ok_or("not running in the desktop app")?, "core");
            let options = object(&[(
                "headers",
                object(&[("x-file-name", JsValue::from_str(&name))]),
            )]);
            let body: JsValue = Uint8Array::from(data.as_slice()).into();
            let invoke = method(&core, "invoke")?;
            let promise = invoke
                .call3(&core, &JsValue::from_str("save_file"), &body, &options)
                .map_err(js_err)?;
            Ok(await_js(promise).await?.as_bool().unwrap_or(false))
        })
    }

    fn watch_folder(&self, handle: &FolderHandle) {
        if let FolderHandle::Native(path) = handle {
            let path = path.clone();
            leptos::task::spawn_local(async move {
                let _ = invoke(
                    "watch_folder",
                    object(&[("path", JsValue::from_str(&path))]),
                )
                .await;
            });
        }
    }

    fn unwatch_folder(&self, handle: &FolderHandle) {
        if let FolderHandle::Native(path) = handle {
            let path = path.clone();
            leptos::task::spawn_local(async move {
                let _ = invoke(
                    "unwatch_folder",
                    object(&[("path", JsValue::from_str(&path))]),
                )
                .await;
            });
        }
    }
}

/// What the app was launched with: `timber [FILE…] [--source DIR…] [--smoke]`.
pub struct Startup {
    pub files: Vec<String>,
    pub sources: Vec<String>,
    pub smoke: bool,
    pub smoke_watch: bool,
}

fn strings(v: &JsValue) -> Vec<String> {
    v.dyn_ref::<Array>()
        .map(|a| a.iter().filter_map(|s| s.as_string()).collect())
        .unwrap_or_default()
}

pub async fn startup() -> Option<Startup> {
    let s = invoke("startup", object(&[])).await.ok()?;
    Some(Startup {
        files: strings(&get(&s, "files")),
        sources: strings(&get(&s, "sources")),
        smoke: get(&s, "smoke").as_bool().unwrap_or(false),
        smoke_watch: get(&s, "smoke_watch").as_bool().unwrap_or(false),
    })
}

/// Reads files given by path (command-line arguments).
pub async fn read_paths(paths: &[String]) -> Result<Vec<InputFile>, String> {
    let list: Array = paths.iter().map(|p| JsValue::from_str(p)).collect();
    let value = invoke("read_paths", object(&[("paths", list.into())])).await?;
    Ok(unpack(value)?.map(|(_, files)| files).unwrap_or_default())
}

/// Calls `on_change(path)` when a watched folder's sources change on disk.
pub fn on_folder_changed(on_change: impl Fn(String) + 'static) {
    let Some(event) = tauri().map(|t| get(&t, "event")) else {
        return;
    };
    let handler = Closure::<dyn Fn(JsValue)>::new(move |e: JsValue| {
        if let Some(path) = get(&e, "payload").as_string() {
            on_change(path);
        }
    });
    if let Ok(listen) = method(&event, "listen") {
        let _ = listen.call2(
            &event,
            &JsValue::from_str("folder-changed"),
            handler.as_ref().unchecked_ref(),
        );
    }
    handler.forget(); // lives as long as the window
}

pub async fn report(line: &str) {
    let _ = invoke("report", object(&[("line", JsValue::from_str(line))])).await;
}

pub async fn quit() {
    let _ = invoke("quit", object(&[])).await;
}
