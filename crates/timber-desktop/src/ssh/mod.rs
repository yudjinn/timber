//! SSH to a device and read its running tree (M5, DESIGN §6). Sessions stay open while the
//! tree is shown so Refresh is quick; a session whose connection dropped (the board
//! rebooted) reconnects with the same settings on the next fetch.

mod client;
mod hostkeys;
mod options;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use serde::Serialize;
use tauri::ipc::Response;
use tauri::{AppHandle, Manager, State};

use client::{Paths, Session};
pub use client::{Request, SshError};

struct Live {
    session: Session,
    /// Kept (in memory only) to reconnect after the device reboots.
    request: Request,
}

#[derive(Default)]
pub struct Sessions {
    next: AtomicU32,
    live: Mutex<HashMap<u32, Arc<tokio::sync::Mutex<Live>>>>,
}

#[derive(Debug, Serialize)]
pub struct Connected {
    id: u32,
    /// `user@host`.
    label: String,
    /// The host as typed (an alias, or the address).
    host: String,
}

fn paths(app: &AppHandle) -> Paths {
    let data = app
        .path()
        .app_data_dir()
        .unwrap_or_else(|_| std::env::temp_dir().join("timber"));
    let ssh = options::home().map(|h| h.join(".ssh"));
    Paths {
        known_hosts: data.join("known_hosts"),
        user_known_hosts: ssh.as_ref().map(|s| s.join("known_hosts")),
        ssh_config: ssh.map(|s| s.join("config")),
    }
}

fn tree_path(request: &Request) -> String {
    request
        .tree_path
        .clone()
        .filter(|p| !p.trim().is_empty())
        .unwrap_or_else(|| client::DEFAULT_TREE_PATH.to_string())
}

#[tauri::command]
pub async fn ssh_connect(
    app: AppHandle,
    sessions: State<'_, Sessions>,
    request: Request,
) -> Result<Connected, SshError> {
    let session = client::connect(&request, &paths(&app)).await?;
    let id = sessions.next.fetch_add(1, Ordering::Relaxed) + 1;
    let connected = Connected {
        id,
        label: session.label(),
        host: session.resolved.alias.clone(),
    };
    sessions.live.lock().unwrap().insert(
        id,
        Arc::new(tokio::sync::Mutex::new(Live { session, request })),
    );
    Ok(connected)
}

fn get(sessions: &Sessions, id: u32) -> Result<Arc<tokio::sync::Mutex<Live>>, SshError> {
    sessions
        .live
        .lock()
        .unwrap()
        .get(&id)
        .cloned()
        .ok_or_else(|| SshError::new("closed", "Not connected to the device"))
}

/// The session's connection, reconnecting first if it dropped.
async fn ready(paths: &Paths, live: &mut Live) -> Result<(), SshError> {
    if live.session.handle.is_closed() {
        live.session = client::connect(&live.request, paths).await.map_err(|e| {
            SshError::new(
                "closed",
                format!(
                    "Lost the connection, and reconnecting failed: {}",
                    e.message
                ),
            )
        })?;
    }
    Ok(())
}

/// Reads the device's tree: `TMBL`, how (0 tar, 1 od, 2 boot FDT), whether `/proc/config.gz`
/// is readable, the path read (u32 length + UTF-8), then the DTB.
#[tauri::command]
pub async fn ssh_fetch(
    app: AppHandle,
    sessions: State<'_, Sessions>,
    id: u32,
) -> Result<Response, SshError> {
    let live = get(&sessions, id)?;
    let mut live = live.lock().await;
    ready(&paths(&app), &mut live).await?;
    let path = tree_path(&live.request);
    let fetched = match client::fetch(&live.session.handle, &path).await {
        // The connection went away mid-fetch: one reconnect and retry.
        Err(e) if e.kind == "closed" => {
            ready(&paths(&app), &mut live).await?;
            client::fetch(&live.session.handle, &path).await?
        }
        other => other?,
    };
    let mut out = b"TMBL".to_vec();
    out.push(match fetched.method {
        "tar" | "tar.gz" => 0,
        "od" => 1,
        _ => 2,
    });
    out.push(fetched.has_config as u8);
    out.extend((fetched.path.len() as u32).to_le_bytes());
    out.extend(fetched.path.as_bytes());
    out.extend(fetched.dtb);
    Ok(Response::new(out))
}

/// The device's kernel config (`/proc/config.gz`, unpacked).
#[tauri::command]
pub async fn ssh_device_config(
    app: AppHandle,
    sessions: State<'_, Sessions>,
    id: u32,
) -> Result<Response, SshError> {
    let live = get(&sessions, id)?;
    let mut live = live.lock().await;
    ready(&paths(&app), &mut live).await?;
    Ok(Response::new(
        client::device_config(&live.session.handle).await?,
    ))
}

#[tauri::command]
pub fn ssh_disconnect(sessions: State<'_, Sessions>, id: u32) {
    let removed = sessions.live.lock().unwrap().remove(&id);
    if let Some(live) = removed {
        tauri::async_runtime::spawn(async move {
            let live = live.lock().await;
            let _ = live
                .session
                .handle
                .disconnect(russh::Disconnect::ByApplication, "", "en")
                .await;
        });
    }
}

/// Trusts a host key the user accepted (new, or "Device was reflashed", D-028).
#[tauri::command]
pub fn ssh_trust_host(app: AppHandle, host: String, port: u16, key: String) -> Result<(), String> {
    ssh_trust_paths(&paths(&app), &host, port, &key)
}

fn ssh_trust_paths(paths: &Paths, host: &str, port: u16, key: &str) -> Result<(), String> {
    let key = russh::keys::PublicKey::from_openssh(key).map_err(|e| e.to_string())?;
    hostkeys::trust(&paths.known_hosts, host, port, &key)
}
