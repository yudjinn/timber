//! SSH to a device (desktop, M5, DESIGN §6): the connect dialog with recent devices and
//! host-key prompts, and the live session: Refresh, auto-refresh, and offering the
//! device's kernel config.

use std::sync::Arc;

use js_sys::{Array, Object, Reflect};
use leptos::html;
use leptos::prelude::*;
use wasm_bindgen::{JsCast, JsValue};

use crate::actions;
use crate::backend::{BackendHandle, get};
use crate::state::AppState;
use crate::tauri::{self, HostKey, SshFailure, SshRequest, SshTree};

const RECENT_KEY: &str = "timber.ssh.recent";
const RECENT_MAX: usize = 8;

/// The device whose tree is shown.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveDevice {
    pub id: u32,
    /// `user@host`.
    pub label: String,
    pub host: String,
    /// The tree's name: `host · live` or `host · boot FDT`.
    pub name: String,
    /// Where on the device it was read from.
    pub path: String,
    pub data: Arc<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LiveStatus {
    /// Read at this time.
    Read(String),
    Refreshing,
    /// The last refresh failed.
    Lost(String),
}

#[derive(Clone, Copy)]
pub struct SshState {
    pub open: RwSignal<bool>,
    pub live: RwSignal<Option<LiveDevice>>,
    pub status: RwSignal<LiveStatus>,
    /// Auto-refresh interval in seconds; 0 is off.
    pub auto: RwSignal<u32>,
    /// The device has `/proc/config.gz` and hasn't been asked about yet (D-030).
    pub config_offer: RwSignal<bool>,
    /// Bumped to stop a running auto-refresh loop.
    timer: StoredValue<u64>,
    /// The connect dialog's fields, and why the last attempt failed.
    pub form: RwSignal<SshRequest>,
    pub failure: RwSignal<Option<SshFailure>>,
}

impl SshState {
    pub fn new() -> Self {
        SshState {
            open: RwSignal::new(false),
            live: RwSignal::new(None),
            status: RwSignal::new(LiveStatus::Refreshing),
            auto: RwSignal::new(0),
            config_offer: RwSignal::new(false),
            timer: StoredValue::new(0),
            form: RwSignal::new(SshRequest {
                auth: "auto".into(),
                ..SshRequest::default()
            }),
            failure: RwSignal::new(None),
        }
    }
}

fn now() -> String {
    let t: String = js_sys::Date::new_0().to_time_string().into();
    t.chars().take(8).collect()
}

fn tree_name(host: &str, method: u8) -> String {
    if method == 2 {
        format!("{host} · boot FDT")
    } else {
        format!("{host} · live")
    }
}

/// Connects, reads the tree, and shows it.
async fn connect(state: AppState, request: SshRequest) -> Result<(), SshFailure> {
    let connected = tauri::ssh_connect(&request).await?;
    let tree = match tauri::ssh_fetch(connected.id).await {
        Ok(t) => t,
        Err(e) => {
            tauri::ssh_disconnect(connected.id).await;
            return Err(e);
        }
    };
    disconnect(state);
    let name = tree_name(&connected.host, tree.method);
    if tree.method == 2 {
        state.notify(
            format!(
                "{} has no /proc/device-tree; showing the boot FDT from /sys/firmware/fdt",
                connected.host
            ),
            false,
        );
    }
    let data = Arc::new(tree.dtb);
    state.ssh.live.set(Some(LiveDevice {
        id: connected.id,
        label: connected.label,
        host: connected.host,
        name: name.clone(),
        path: tree.path,
        data: data.clone(),
    }));
    state.ssh.status.set(LiveStatus::Read(now()));
    actions::show_live_blob(state, &name, data);
    state.ssh.config_offer.set(tree.has_config);
    schedule(state);
    Ok(())
}

/// `timber --ssh TARGET`: connects at launch; if that needs the user (a password, a new
/// host key), opens the dialog with the problem shown.
pub async fn connect_at_start(state: AppState, request: SshRequest) -> Result<(), String> {
    state
        .busy
        .set(Some(format!("Connecting to {}…", request.target)));
    match connect(state, request.clone()).await {
        Ok(()) => {
            remember(&request);
            Ok(())
        }
        Err(f) => {
            state.busy.set(None);
            let message = f.message.clone();
            state.ssh.form.set(request);
            state.ssh.failure.set(Some(f));
            state.ssh.open.set(true);
            Err(message)
        }
    }
}

/// Ends the live session (the tree stays shown as a snapshot).
pub fn disconnect(state: AppState) {
    let Some(live) = state.ssh.live.get_untracked() else {
        return;
    };
    state.ssh.live.set(None);
    state.ssh.config_offer.set(false);
    state.ssh.timer.update_value(|t| *t += 1);
    leptos::task::spawn_local(async move { tauri::ssh_disconnect(live.id).await });
}

/// Re-reads the device. `manual`: the user asked, so say when nothing changed.
pub fn refresh(state: AppState, manual: bool) {
    let Some(live) = state.ssh.live.get_untracked() else {
        return;
    };
    if state.ssh.status.get_untracked() == LiveStatus::Refreshing {
        return;
    }
    let was_lost = matches!(state.ssh.status.get_untracked(), LiveStatus::Lost(_));
    state.ssh.status.set(LiveStatus::Refreshing);
    leptos::task::spawn_local(async move {
        let result = tauri::ssh_fetch(live.id).await;
        // Disconnected (or reconnected elsewhere) while reading.
        if state.ssh.live.with_untracked(|l| l.as_ref().map(|l| l.id)) != Some(live.id) {
            return;
        }
        match result {
            Ok(tree) => {
                state.ssh.status.set(LiveStatus::Read(now()));
                if was_lost {
                    state.notify(format!("{} is back", live.host), false);
                }
                apply(state, live, tree, manual);
            }
            Err(e) => {
                // Only the first failure in a row pops up; the live bar keeps showing it.
                if manual || !was_lost {
                    state.notify(e.message.clone(), true);
                }
                state.ssh.status.set(LiveStatus::Lost(e.message));
            }
        }
    });
}

fn apply(state: AppState, live: LiveDevice, tree: SshTree, manual: bool) {
    let name = tree_name(&live.host, tree.method);
    if tree.dtb == *live.data && name == live.name {
        if manual {
            state.notify("No changes on the device", false);
        }
        return;
    }
    let data = Arc::new(tree.dtb);
    state.ssh.live.set(Some(LiveDevice {
        name: name.clone(),
        path: tree.path,
        data: data.clone(),
        ..live
    }));
    actions::replace_live_blob(state, &name, data);
    state.notify("The device's tree changed; updated", false);
}

/// (Re)starts the auto-refresh loop for the current interval.
fn schedule(state: AppState) {
    state.ssh.timer.update_value(|t| *t += 1);
    let generation = state.ssh.timer.get_value();
    let secs = state.ssh.auto.get_untracked();
    if secs == 0 {
        return;
    }
    leptos::task::spawn_local(async move {
        loop {
            actions::sleep(secs as i32 * 1000).await;
            if state.ssh.timer.get_value() != generation
                || state.ssh.live.with_untracked(Option::is_none)
            {
                return;
            }
            // Don't pile up behind a busy rebuild.
            if state.busy.with_untracked(Option::is_none) {
                refresh(state, false);
            }
        }
    });
}

fn use_device_config(state: AppState) {
    let Some(live) = state.ssh.live.get_untracked() else {
        return;
    };
    state.ssh.config_offer.set(false);
    leptos::task::spawn_local(async move {
        match tauri::ssh_device_config(live.id).await {
            Ok(text) => {
                actions::apply_config(state, &format!("config.gz from {}", live.host), &text);
                actions::refresh(state);
            }
            Err(e) => state.notify(e.message, true),
        }
    });
}

// ---- recent devices ----

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

fn load_recent() -> Vec<SshRequest> {
    let Some(text) = storage().and_then(|s| s.get_item(RECENT_KEY).ok().flatten()) else {
        return Vec::new();
    };
    let Ok(list) = js_sys::JSON::parse(&text) else {
        return Vec::new();
    };
    let Some(list) = list.dyn_ref::<Array>() else {
        return Vec::new();
    };
    let s = |v: &JsValue, k: &str| get(v, k).as_string().unwrap_or_default();
    list.iter()
        .map(|v| SshRequest {
            target: s(&v, "target"),
            user: s(&v, "user"),
            port: get(&v, "port").as_f64().map(|p| p as u16),
            auth: Some(s(&v, "auth"))
                .filter(|a| !a.is_empty())
                .unwrap_or("auto".into()),
            password: None,
            key_file: s(&v, "key_file"),
            passphrase: None,
            options: s(&v, "options"),
            tree_path: s(&v, "tree_path"),
        })
        .filter(|r| !r.target.is_empty())
        .collect()
}

/// The same device and settings (passwords aside).
fn same_device(a: &SshRequest, b: &SshRequest) -> bool {
    (
        a.target.trim(),
        a.user.trim(),
        a.port,
        a.options.trim(),
        a.tree_path.trim(),
    ) == (
        b.target.trim(),
        b.user.trim(),
        b.port,
        b.options.trim(),
        b.tree_path.trim(),
    )
}

fn save_recent(list: &[SshRequest]) {
    let arr = Array::new();
    for r in list {
        let o = Object::new();
        let set = |k: &str, v: JsValue| {
            let _ = Reflect::set(&o, &JsValue::from_str(k), &v);
        };
        set("target", r.target.trim().into());
        set("user", r.user.trim().into());
        set("port", r.port.map_or(JsValue::NULL, JsValue::from));
        set("auth", r.auth.as_str().into());
        set("key_file", r.key_file.trim().into());
        set("options", r.options.trim().into());
        set("tree_path", r.tree_path.trim().into());
        arr.push(&o);
    }
    if let (Some(s), Ok(text)) = (storage(), js_sys::JSON::stringify(&arr)) {
        let _ = s.set_item(RECENT_KEY, &String::from(text));
    }
}

/// Moves `request` to the top of the recent list (without its secrets).
fn remember(request: &SshRequest) -> Vec<SshRequest> {
    let mut list = load_recent();
    list.retain(|r| !same_device(r, request));
    list.insert(
        0,
        SshRequest {
            password: None,
            passphrase: None,
            ..request.clone()
        },
    );
    list.truncate(RECENT_MAX);
    save_recent(&list);
    list
}

fn recent_label(r: &SshRequest) -> String {
    let mut s = String::new();
    if !r.user.trim().is_empty() && !r.target.contains('@') {
        s.push_str(r.user.trim());
        s.push('@');
    }
    s.push_str(r.target.trim());
    if let Some(p) = r.port {
        s.push_str(&format!(":{p}"));
    }
    s
}

fn recent_detail(r: &SshRequest) -> String {
    let mut parts = Vec::new();
    match r.auth.as_str() {
        "password" => parts.push("password".to_string()),
        "key" => parts.push(format!(
            "key {}",
            r.key_file.rsplit(['/', '\\']).next().unwrap_or(&r.key_file)
        )),
        _ => {}
    }
    if !r.options.trim().is_empty() {
        parts.push(r.options.trim().to_string());
    }
    if !r.tree_path.trim().is_empty() {
        parts.push(r.tree_path.trim().to_string());
    }
    parts.join(" · ")
}

// ---- views ----

#[component]
pub fn ConnectDialog() -> impl IntoView {
    let state = expect_context::<AppState>();
    let backend = StoredValue::new(expect_context::<BackendHandle>());
    let form = state.ssh.form;
    let failure = state.ssh.failure;
    let connecting = RwSignal::new(false);
    let advanced = RwSignal::new(false);
    let recent = RwSignal::new(Vec::<SshRequest>::new());
    let password_ref = NodeRef::<html::Input>::new();
    let passphrase_ref = NodeRef::<html::Input>::new();
    let host_ref = NodeRef::<html::Input>::new();

    // On open: fresh recent list, the last device filled in. On close: forget the error.
    Effect::new(move |_| {
        if !state.ssh.open.get() {
            failure.set(None);
            return;
        }
        let list = load_recent();
        if form.with_untracked(|f| f.target.is_empty())
            && let Some(last) = list.first()
        {
            form.set(last.clone());
        }
        advanced.set(form.with_untracked(|f| !f.options.is_empty() || !f.tree_path.is_empty()));
        recent.set(list);
        request_animation_frame(move || {
            if let Some(el) = host_ref.get_untracked() {
                let _ = el.focus();
            }
        });
    });

    let go = move |request: SshRequest| {
        if request.target.trim().is_empty() {
            failure.set(Some(SshFailure {
                kind: "options".into(),
                message: "Enter a host to connect to".into(),
                host_key: None,
                key_file: None,
            }));
            return;
        }
        connecting.set(true);
        failure.set(None);
        state
            .busy
            .set(Some(format!("Connecting to {}…", request.target.trim())));
        leptos::task::spawn_local(async move {
            let result = connect(state, request.clone()).await;
            connecting.set(false);
            match result {
                Ok(()) => {
                    recent.set(remember(&request));
                    form.update(|f| {
                        f.password = None;
                        f.passphrase = None;
                    });
                    state.ssh.open.set(false);
                }
                Err(f) => {
                    state.busy.set(None);
                    let focus = match f.kind.as_str() {
                        "need_password" => Some(password_ref),
                        "need_passphrase" => Some(passphrase_ref),
                        _ => None,
                    };
                    failure.set(Some(f));
                    if let Some(r) = focus {
                        request_animation_frame(move || {
                            if let Some(el) = r.get_untracked() {
                                let _ = el.focus();
                                el.select();
                            }
                        });
                    }
                }
            }
        });
    };

    let trust = move |key: HostKey| {
        leptos::task::spawn_local(async move {
            match tauri::ssh_trust_host(&key).await {
                Ok(()) => go(form.get_untracked()),
                Err(e) => failure.set(Some(SshFailure {
                    kind: "connect".into(),
                    message: format!("Couldn't save the host key: {e}"),
                    host_key: None,
                    key_file: None,
                })),
            }
        });
    };

    let browse_key = move |_| {
        let backend = backend.get_value();
        leptos::task::spawn_local(async move {
            if let Ok(Some(file)) = backend.0.open_config().await
                && let Some(path) = file.path
            {
                form.update(|f| {
                    f.key_file = path;
                    f.auth = "key".into();
                });
            }
        });
    };

    let close = move || {
        if !connecting.get_untracked() {
            state.ssh.open.set(false);
        }
    };
    let auth = move || form.with(|f| f.auth.clone());
    let needs = move |kind: &str| failure.with(|f| f.as_ref().is_some_and(|f| f.kind == kind));
    let text_field = move |value: fn(&SshRequest) -> String, set: fn(&mut SshRequest, String)| {
        (
            move || form.with(value),
            move |ev| form.update(|f| set(f, event_target_value(&ev))),
        )
    };
    let (target, set_target) = text_field(|f| f.target.clone(), |f, v| f.target = v);
    let (user, set_user) = text_field(|f| f.user.clone(), |f, v| f.user = v);
    let (key_file, set_key_file) = text_field(|f| f.key_file.clone(), |f, v| f.key_file = v);
    let (options, set_options) = text_field(|f| f.options.clone(), |f, v| f.options = v);
    let (tree_path, set_tree_path) = text_field(|f| f.tree_path.clone(), |f, v| f.tree_path = v);

    let host_key_panel = move || {
        let f = failure.get()?;
        let key = *f.host_key.clone()?;
        let changed = f.kind == "host_key_changed";
        let where_ = if key.port == 22 {
            key.host.clone()
        } else {
            format!("{}:{}", key.host, key.port)
        };
        let trust_key = key.clone();
        Some(view! {
            <div class="hostkey" class:changed=changed role="alert">
                {if changed {
                    view! {
                        <p><strong>{format!("The host key for {where_} has changed.")}</strong></p>
                        <p class="small">
                            "Expected after reflashing the device. Otherwise, someone may be intercepting the connection."
                        </p>
                        <dl class="fingerprints">
                            <dt>"Was"</dt>
                            <dd class="mono">{key.old_fingerprint.clone().unwrap_or_default()}</dd>
                            <dt>"Now"</dt>
                            <dd class="mono">{format!("{} {}", key.algorithm, key.fingerprint)}</dd>
                        </dl>
                    }
                        .into_any()
                } else {
                    view! {
                        <p><strong>{format!("First connection to {where_}.")}</strong></p>
                        <p class="small">"Check that this is the device's key before trusting it:"</p>
                        <dl class="fingerprints">
                            <dt>{key.algorithm.clone()}</dt>
                            <dd class="mono">{key.fingerprint.clone()}</dd>
                        </dl>
                    }
                        .into_any()
                }}
                <div class="row">
                    <button
                        class="primary"
                        disabled=move || connecting.get()
                        on:click=move |_| trust(trust_key.clone())
                    >
                        {if changed { "Device was reflashed: trust the new key" } else { "Trust and connect" }}
                    </button>
                    <button on:click=move |_| failure.set(None)>"Cancel"</button>
                </div>
            </div>
        })
    };

    let error_line = move || {
        failure.with(|f| {
            f.as_ref()
                .filter(|f| f.host_key.is_none())
                .map(|f| view! { <p class="form-error" role="alert">{f.message.clone()}</p> })
        })
    };

    let recent_list = move || {
        let list = recent.get();
        (!list.is_empty()).then(|| {
            view! {
                <section>
                    <h4>"Recent devices"</h4>
                    <ul class="plain recent">
                        {list
                            .into_iter()
                            .map(|r| {
                                let pick = r.clone();
                                let forget = r.clone();
                                let detail = recent_detail(&r);
                                view! {
                                    <li class="recent-row">
                                        <button
                                            class="ghost recent-pick"
                                            disabled=move || connecting.get()
                                            title="Connect"
                                            on:click=move |_| {
                                                form.set(pick.clone());
                                                go(pick.clone());
                                            }
                                        >
                                            <span class="mono">{recent_label(&r)}</span>
                                            {(!detail.is_empty()).then(|| view! { <span class="muted small">{detail}</span> })}
                                        </button>
                                        <button
                                            class="ghost small"
                                            title="Forget this device"
                                            aria-label="Forget this device"
                                            on:click=move |_| {
                                                let mut list = load_recent();
                                                list.retain(|x| !same_device(x, &forget));
                                                save_recent(&list);
                                                recent.set(list);
                                            }
                                        >
                                            "✕"
                                        </button>
                                    </li>
                                }
                            })
                            .collect_view()}
                    </ul>
                </section>
            }
        })
    };

    view! {
        <Show when=move || state.ssh.open.get()>
            <div class="modal-backdrop" on:click=move |_| close()>
                <div class="modal wide ssh" role="dialog" aria-label="Connect to a device" on:click=|ev| ev.stop_propagation()>
                    <header class="modal-head">
                        <h3>"Connect to a device"</h3>
                        <button class="ghost" aria-label="Close" on:click=move |_| close()>"✕"</button>
                    </header>
                    <p class="muted small">"Reads the running device tree over SSH (/proc/device-tree)."</p>
                    {recent_list}
                    <form
                        class="ssh-form"
                        on:submit=move |ev| {
                            ev.prevent_default();
                            go(form.get_untracked());
                        }
                    >
                        <label class="span2">
                            "Host"
                            <input
                                type="text"
                                node_ref=host_ref
                                class="mono"
                                placeholder="root@192.168.1.50, or an ~/.ssh/config alias"
                                autocomplete="off"
                                spellcheck="false"
                                prop:value=target
                                on:input=set_target
                            />
                        </label>
                        <label>
                            "User"
                            <input type="text" class="mono" placeholder="from the host or ~/.ssh/config" autocomplete="off" spellcheck="false" prop:value=user on:input=set_user />
                        </label>
                        <label>
                            "Port"
                            <input
                                type="text"
                                class="mono"
                                inputmode="numeric"
                                placeholder="22"
                                prop:value=move || form.with(|f| f.port.map(|p| p.to_string()).unwrap_or_default())
                                on:input=move |ev| {
                                    let v = event_target_value(&ev);
                                    form.update(|f| f.port = v.trim().parse().ok());
                                }
                            />
                        </label>
                        <div class="span2">
                            <span class="label">"Log in with"</span>
                            <div class="segmented" role="radiogroup" aria-label="Log in with">
                                {[("auto", "Automatic"), ("password", "Password"), ("key", "Key file")]
                                    .into_iter()
                                    .map(|(k, label)| view! {
                                        <button
                                            type="button"
                                            role="radio"
                                            class:on=move || auth() == k
                                            aria-checked=move || (auth() == k).to_string()
                                            on:click=move |_| form.update(|f| f.auth = k.into())
                                        >
                                            {label}
                                        </button>
                                    })
                                    .collect_view()}
                            </div>
                            <p class="muted small">
                                {move || match auth().as_str() {
                                    "password" => "Password, or keyboard-interactive with the password.",
                                    "key" => "Only this private key.",
                                    _ => "Tries your SSH agent, your keys, then the password (an empty one if left blank).",
                                }}
                            </p>
                        </div>
                        <Show when=move || auth() != "key">
                            <label class="span2" class:needed=move || needs("need_password")>
                                "Password"
                                <input
                                    type="password"
                                    node_ref=password_ref
                                    autocomplete="off"
                                    placeholder=move || if auth() == "auto" { "Only if the device needs one" } else { "" }
                                    prop:value=move || form.with(|f| f.password.clone().unwrap_or_default())
                                    on:input=move |ev| form.update(|f| f.password = Some(event_target_value(&ev)))
                                />
                            </label>
                        </Show>
                        <Show when=move || auth() == "key">
                            <label class="span2">
                                "Key file"
                                <span class="row tight">
                                    <input type="text" class="mono" placeholder="~/.ssh/id_ed25519" spellcheck="false" prop:value=key_file on:input=set_key_file />
                                    <button type="button" on:click=browse_key>"Browse…"</button>
                                </span>
                            </label>
                        </Show>
                        <Show when=move || auth() == "key" || needs("need_passphrase")>
                            <label class="span2" class:needed=move || needs("need_passphrase")>
                                {move || match failure.with(|f| f.as_ref().and_then(|f| f.key_file.clone())) {
                                    Some(k) => format!("Passphrase for {}", k.rsplit(['/', '\\']).next().unwrap_or(&k)),
                                    None => "Key passphrase".to_string(),
                                }}
                                <input
                                    type="password"
                                    node_ref=passphrase_ref
                                    autocomplete="off"
                                    placeholder="If the key has one"
                                    prop:value=move || form.with(|f| f.passphrase.clone().unwrap_or_default())
                                    on:input=move |ev| form.update(|f| f.passphrase = Some(event_target_value(&ev)))
                                />
                            </label>
                        </Show>
                        <div class="span2">
                            <button type="button" class="link" aria-expanded=move || advanced.get().to_string() on:click=move |_| advanced.update(|a| *a = !*a)>
                                {move || if advanced.get() { "▾ Advanced" } else { "▸ Advanced" }}
                            </button>
                        </div>
                        <Show when=move || advanced.get()>
                            <label class="span2">
                                "Extra options"
                                <input type="text" class="mono" placeholder="-J user@jumphost   -o ProxyCommand=…" spellcheck="false" prop:value=options on:input=set_options />
                                <span class="muted small">
                                    "OpenSSH style: " <code>"-J"</code> " / " <code>"ProxyJump"</code> " (runs your system ssh), "
                                    <code>"ProxyCommand"</code> ", " <code>"-p"</code> ", " <code>"-l"</code> ", " <code>"-i"</code> ", "
                                    <code>"-o ConnectTimeout=…"</code>
                                </span>
                            </label>
                            <label class="span2">
                                "Device tree path"
                                <input type="text" class="mono" placeholder="/proc/device-tree" spellcheck="false" prop:value=tree_path on:input=set_tree_path />
                            </label>
                        </Show>
                        <div class="span2">{error_line}{host_key_panel}</div>
                        <footer class="modal-foot span2">
                            <span class="muted small">"Passwords are never saved."</span>
                            <span class="row tight">
                                <button type="button" on:click=move |_| close()>"Cancel"</button>
                                <button type="submit" class="primary" disabled=move || connecting.get()>
                                    {move || if connecting.get() { "Connecting…" } else { "Connect" }}
                                </button>
                            </span>
                        </footer>
                    </form>
                </div>
            </div>
        </Show>
    }
}

/// The live session's controls, in the toolbar.
#[component]
pub fn LiveBar() -> impl IntoView {
    let state = expect_context::<AppState>();
    let status_text = move || match state.ssh.status.get() {
        LiveStatus::Read(at) => format!("Read at {at}"),
        LiveStatus::Refreshing => "Reading the device…".to_string(),
        LiveStatus::Lost(e) => format!("Couldn't read the device: {e}"),
    };
    view! {
        {move || state.ssh.live.get().map(|live| {
            let title = format!("{} — {} on the device. ", live.label, live.path);
            view! {
                <div class="live-bar" role="group" aria-label="Live device">
                    <span
                        class="live-dot"
                        class:lost=move || matches!(state.ssh.status.get(), LiveStatus::Lost(_))
                        class:busy=move || state.ssh.status.get() == LiveStatus::Refreshing
                        title=status_text
                    ></span>
                    <span class="live-label mono" title=move || format!("{title}{}", status_text())>{live.label.clone()}</span>
                    <button
                        class="ghost icon"
                        title="Refresh: re-read the device"
                        aria-label="Refresh"
                        disabled=move || state.ssh.status.get() == LiveStatus::Refreshing
                        on:click=move |_| refresh(state, true)
                    >
                        <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true">
                            <path d="M13 8a5 5 0 1 1-1.5-3.55" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" />
                            <path d="M12 1.8v3h-3" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" />
                        </svg>
                    </button>
                    <select
                        title="Auto-refresh"
                        aria-label="Auto-refresh"
                        on:change=move |ev| {
                            state.ssh.auto.set(event_target_value(&ev).parse().unwrap_or(0));
                            schedule(state);
                        }
                    >
                        {[(0, "Auto: off"), (5, "Every 5 s"), (10, "Every 10 s"), (30, "Every 30 s"), (60, "Every 60 s")]
                            .into_iter()
                            .map(|(s, label)| view! {
                                <option value=s.to_string() selected=move || state.ssh.auto.get() == s>{label}</option>
                            })
                            .collect_view()}
                    </select>
                    <button class="ghost icon" title="Disconnect (the tree stays shown)" aria-label="Disconnect" on:click=move |_| disconnect(state)>
                        "✕"
                    </button>
                </div>
            }
        })}
    }
}

/// "Use this device's kernel config?" (D-030: always ask).
#[component]
pub fn ConfigOffer() -> impl IntoView {
    let state = expect_context::<AppState>();
    view! {
        <Show when=move || state.ssh.config_offer.get()>
            <div class="offer" role="dialog" aria-label="Device kernel config">
                <p>
                    {move || state.ssh.live.with(|l| l.as_ref().map(|l| l.host.clone()).unwrap_or_default())}
                    " has " <code>"/proc/config.gz"</code> ". Use it as the kernel config?"
                </p>
                <p class="muted small">"It decides which boards in your sources are built."</p>
                <div class="row">
                    <button class="primary" on:click=move |_| use_device_config(state)>"Use it"</button>
                    <button on:click=move |_| state.ssh.config_offer.set(false)>"Not now"</button>
                </div>
            </div>
        </Show>
    }
}
