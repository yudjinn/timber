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
            path: None,
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
            let picked = invoke("pick_files", object(&[("any", JsValue::FALSE)])).await?;
            Ok(unpack(picked)?.map(|(_, files)| with_disk_paths(files)))
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

    fn open_config(&self) -> LocalFuture<'_, Result<Option<InputFile>, String>> {
        Box::pin(async {
            let picked = unpack(invoke("pick_files", object(&[("any", JsValue::TRUE)])).await?)?;
            Ok(picked.and_then(|(_, files)| with_disk_paths(files).into_iter().next()))
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

    fn open_in_editor(
        &self,
        path: &str,
        line: u32,
        col: u32,
        template: &str,
    ) -> LocalFuture<'_, Result<(), String>> {
        let args = object(&[
            ("path", JsValue::from_str(path)),
            ("line", JsValue::from(line)),
            ("col", JsValue::from(col)),
            ("template", JsValue::from_str(template)),
        ]);
        Box::pin(async move { invoke("open_in_editor", args).await.map(|_| ()) })
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

/// What the app was launched with: `timber [FILE…] [--source DIR…] [--ssh TARGET] [--smoke]`.
pub struct Startup {
    pub files: Vec<String>,
    pub sources: Vec<String>,
    pub config: Option<String>,
    pub smoke: bool,
    pub smoke_watch: bool,
    /// `--ssh TARGET`, with `--ssh-options` and `--tree-path`.
    pub ssh: Option<SshRequest>,
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
        config: get(&s, "config").as_string(),
        smoke: get(&s, "smoke").as_bool().unwrap_or(false),
        smoke_watch: get(&s, "smoke_watch").as_bool().unwrap_or(false),
        ssh: get(&s, "ssh").as_string().map(|target| SshRequest {
            target,
            auth: "auto".into(),
            options: get(&s, "ssh_options").as_string().unwrap_or_default(),
            tree_path: get(&s, "tree_path").as_string().unwrap_or_default(),
            ..SshRequest::default()
        }),
    })
}

/// Reads files given by path (command-line arguments).
pub async fn read_paths(paths: &[String]) -> Result<Vec<InputFile>, String> {
    let list: Array = paths.iter().map(|p| JsValue::from_str(p)).collect();
    let value = invoke("read_paths", object(&[("paths", list.into())])).await?;
    Ok(unpack(value)?
        .map(|(_, files)| with_disk_paths(files))
        .unwrap_or_default())
}

/// Loose files arrive named by absolute path: keep that as the disk path, name by file name.
fn with_disk_paths(files: Vec<InputFile>) -> Vec<InputFile> {
    files
        .into_iter()
        .map(|f| {
            let base = f
                .name
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or(&f.name)
                .to_string();
            InputFile {
                path: Some(f.name),
                name: base,
                data: f.data,
            }
        })
        .collect()
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

/// Why an SSH step failed (`timber-desktop/src/ssh/client.rs`, `SshError`).
#[derive(Debug, Clone, PartialEq)]
pub struct SshFailure {
    pub kind: String,
    pub message: String,
    pub host_key: Option<Box<HostKey>>,
    pub key_file: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HostKey {
    pub host: String,
    pub port: u16,
    pub algorithm: String,
    pub fingerprint: String,
    pub old_fingerprint: Option<String>,
    pub key: String,
}

impl SshFailure {
    fn from_error(e: String) -> Self {
        let parsed = js_sys::JSON::parse(&e).ok().filter(|v| v.is_object());
        let Some(v) = parsed else {
            return SshFailure {
                kind: "connect".into(),
                message: e,
                host_key: None,
                key_file: None,
            };
        };
        let s = |v: &JsValue, k: &str| get(v, k).as_string();
        let hk = get(&v, "host_key");
        SshFailure {
            kind: s(&v, "kind").unwrap_or_default(),
            message: s(&v, "message").unwrap_or(e),
            host_key: hk.is_object().then(|| {
                Box::new(HostKey {
                    host: s(&hk, "host").unwrap_or_default(),
                    port: get(&hk, "port").as_f64().unwrap_or(22.0) as u16,
                    algorithm: s(&hk, "algorithm").unwrap_or_default(),
                    fingerprint: s(&hk, "fingerprint").unwrap_or_default(),
                    old_fingerprint: s(&hk, "old_fingerprint"),
                    key: s(&hk, "key").unwrap_or_default(),
                })
            }),
            key_file: s(&v, "key_file"),
        }
    }
}

/// The connect dialog's fields (`Request` in the desktop app).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SshRequest {
    pub target: String,
    pub user: String,
    pub port: Option<u16>,
    /// `auto`, `password`, or `key`.
    pub auth: String,
    pub password: Option<String>,
    pub key_file: String,
    pub passphrase: Option<String>,
    pub options: String,
    pub tree_path: String,
}

impl SshRequest {
    fn to_js(&self) -> JsValue {
        let opt = |s: &str| {
            if s.trim().is_empty() {
                JsValue::NULL
            } else {
                JsValue::from_str(s.trim())
            }
        };
        let secret = |s: &Option<String>| s.as_deref().map_or(JsValue::NULL, JsValue::from_str);
        object(&[
            ("target", JsValue::from_str(self.target.trim())),
            ("user", opt(&self.user)),
            ("port", self.port.map_or(JsValue::NULL, JsValue::from)),
            ("auth", JsValue::from_str(&self.auth)),
            ("password", secret(&self.password)),
            ("key_file", opt(&self.key_file)),
            ("passphrase", secret(&self.passphrase)),
            ("options", JsValue::from_str(&self.options)),
            ("tree_path", opt(&self.tree_path)),
        ])
    }
}

pub struct SshConnected {
    pub id: u32,
    /// `user@host`.
    pub label: String,
    pub host: String,
}

/// A read of the device's tree.
pub struct SshTree {
    /// 0 tar, 1 od (both the running tree), 2 the boot FDT.
    pub method: u8,
    pub has_config: bool,
    /// Where on the device it was read from.
    pub path: String,
    pub dtb: Vec<u8>,
}

pub async fn ssh_connect(request: &SshRequest) -> Result<SshConnected, SshFailure> {
    let v = invoke("ssh_connect", object(&[("request", request.to_js())]))
        .await
        .map_err(SshFailure::from_error)?;
    Ok(SshConnected {
        id: get(&v, "id").as_f64().unwrap_or(0.0) as u32,
        label: get(&v, "label").as_string().unwrap_or_default(),
        host: get(&v, "host").as_string().unwrap_or_default(),
    })
}

pub async fn ssh_fetch(id: u32) -> Result<SshTree, SshFailure> {
    let v = invoke("ssh_fetch", object(&[("id", JsValue::from(id))]))
        .await
        .map_err(SshFailure::from_error)?;
    let bytes = Uint8Array::new(&v).to_vec();
    let bad = || SshFailure::from_error("unexpected response from the desktop app".into());
    if bytes.len() < 10 || &bytes[..4] != b"TMBL" {
        return Err(bad());
    }
    let len = u32::from_le_bytes([bytes[6], bytes[7], bytes[8], bytes[9]]) as usize;
    let path = bytes.get(10..10 + len).ok_or_else(bad)?;
    Ok(SshTree {
        method: bytes[4],
        has_config: bytes[5] == 1,
        path: String::from_utf8_lossy(path).into_owned(),
        dtb: bytes[10 + len..].to_vec(),
    })
}

pub async fn ssh_device_config(id: u32) -> Result<Vec<u8>, SshFailure> {
    let v = invoke("ssh_device_config", object(&[("id", JsValue::from(id))]))
        .await
        .map_err(SshFailure::from_error)?;
    Ok(Uint8Array::new(&v).to_vec())
}

pub async fn ssh_disconnect(id: u32) {
    let _ = invoke("ssh_disconnect", object(&[("id", JsValue::from(id))])).await;
}

pub async fn ssh_trust_host(key: &HostKey) -> Result<(), String> {
    invoke(
        "ssh_trust_host",
        object(&[
            ("host", JsValue::from_str(&key.host)),
            ("port", JsValue::from(key.port)),
            ("key", JsValue::from_str(&key.key)),
        ]),
    )
    .await
    .map(|_| ())
}
