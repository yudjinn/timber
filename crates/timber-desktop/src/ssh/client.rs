//! Connecting (directly or through a proxy), authenticating, and reading the device's tree.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::client::{self, Handle, KeyboardInteractiveAuthResponse};
use russh::keys::agent::AgentIdentity;
use russh::keys::{PrivateKeyWithHashAlg, PublicKey};
use russh::{ChannelMsg, MethodKind, MethodSet};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;

use super::hostkeys::{self, Status};
use super::options::{self, Inputs, Proxy, Resolved};

/// Where Timber's own files and the user's OpenSSH files are.
#[derive(Debug, Clone)]
pub struct Paths {
    pub known_hosts: PathBuf,
    pub user_known_hosts: Option<PathBuf>,
    pub ssh_config: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthMode {
    #[default]
    Auto,
    Password,
    Key,
}

/// The connect dialog's fields.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Request {
    pub target: String,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub auth: AuthMode,
    pub password: Option<String>,
    pub key_file: Option<String>,
    pub passphrase: Option<String>,
    /// "Extra options" (D-029).
    pub options: String,
    /// Where the device's tree is; `/proc/device-tree` if unset.
    pub tree_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct HostKeyInfo {
    pub host: String,
    pub port: u16,
    pub algorithm: String,
    pub fingerprint: String,
    /// The key recorded before (a changed key).
    pub old_fingerprint: Option<String>,
    /// The new key, OpenSSH format, handed back to `ssh_trust_host`.
    pub key: String,
}

/// What the dialog needs to know when a connection doesn't work out.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SshError {
    /// `host_key_unknown`, `host_key_changed`, `need_password`, `need_passphrase`,
    /// `auth`, `connect`, `options`, `fetch`, `closed`.
    pub kind: &'static str,
    pub message: String,
    pub host_key: Option<Box<HostKeyInfo>>,
    /// The key file that needs a passphrase.
    pub key_file: Option<String>,
}

impl SshError {
    pub fn new(kind: &'static str, message: impl Into<String>) -> Self {
        SshError {
            kind,
            message: message.into(),
            host_key: None,
            key_file: None,
        }
    }
}

pub struct Client {
    host: String,
    port: u16,
    paths: Paths,
    /// Set when the host key isn't trusted, to report after the connection fails.
    issue: Arc<Mutex<Option<SshError>>>,
}

impl client::Handler for Client {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        key: &russh::keys::PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let key: PublicKey = match key {
            russh::keys::PublicKeyOrCertificate::PublicKey { key, .. } => key.clone(),
            russh::keys::PublicKeyOrCertificate::Certificate(cert) => {
                PublicKey::from(cert.public_key().clone())
            }
        };
        let status = hostkeys::check(
            &self.paths.known_hosts,
            self.paths.user_known_hosts.as_deref(),
            &self.host,
            self.port,
            &key,
        );
        let (kind, old) = match status {
            Status::Known => return Ok(true),
            Status::Unknown => ("host_key_unknown", None),
            Status::Changed { old } => ("host_key_changed", Some(hostkeys::fingerprint(&old))),
        };
        let message = if old.is_some() {
            format!("The host key for {} has changed", self.host)
        } else {
            format!("{} hasn't been seen before", self.host)
        };
        *self.issue.lock().unwrap() = Some(SshError {
            kind,
            message,
            host_key: Some(Box::new(HostKeyInfo {
                host: self.host.clone(),
                port: self.port,
                algorithm: key.algorithm().to_string(),
                fingerprint: hostkeys::fingerprint(&key),
                old_fingerprint: old,
                key: key.to_openssh().unwrap_or_default(),
            })),
            key_file: None,
        });
        Ok(false)
    }
}

/// A connected, authenticated device.
pub struct Session {
    pub handle: Handle<Client>,
    pub resolved: Resolved,
    /// The proxy process, for `ProxyJump`/`ProxyCommand`; killed when dropped.
    _proxy: Option<tokio::process::Child>,
}

impl Session {
    /// `user@host`, for the UI.
    pub fn label(&self) -> String {
        format!("{}@{}", self.resolved.user, self.resolved.alias)
    }
}

fn proxy_command(proxy: &Proxy) -> tokio::process::Command {
    let mut cmd = match proxy {
        Proxy::Jump(args) => {
            let mut c = tokio::process::Command::new("ssh");
            c.args(args);
            c
        }
        #[cfg(windows)]
        Proxy::Command(line) => {
            let mut c = tokio::process::Command::new("cmd");
            c.arg("/C").arg(line);
            c
        }
        #[cfg(not(windows))]
        Proxy::Command(line) => {
            let mut c = tokio::process::Command::new("sh");
            c.arg("-c").arg(line);
            c
        }
    };
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    cmd
}

fn describe(e: &russh::Error, r: &Resolved) -> String {
    let at = format!("{}:{}", r.hostname, r.port);
    match e {
        russh::Error::IO(io) => match io.kind() {
            std::io::ErrorKind::ConnectionRefused => {
                format!("{at} refused the connection (is sshd running?)")
            }
            std::io::ErrorKind::UnexpectedEof => {
                format!("{at} closed the connection")
            }
            _ => format!("Couldn't connect to {at}: {io}"),
        },
        other => format!("Couldn't connect to {at}: {other}"),
    }
}

pub async fn connect(req: &Request, paths: &Paths) -> Result<Session, SshError> {
    let resolved = options::resolve(&Inputs {
        target: &req.target,
        user: req.user.as_deref(),
        port: req.port,
        extra: &req.options,
        config: paths.ssh_config.as_deref(),
    })
    .map_err(|e| SshError::new("options", e))?;
    let config = Arc::new(client::Config {
        keepalive_interval: Some(Duration::from_secs(15)),
        keepalive_max: 3,
        nodelay: true,
        ..Default::default()
    });
    let issue = Arc::new(Mutex::new(None));
    let handler = Client {
        host: resolved.hostname.clone(),
        port: resolved.port,
        paths: paths.clone(),
        issue: issue.clone(),
    };
    let proxy_stderr = Arc::new(Mutex::new(String::new()));
    let connecting = async {
        match &resolved.proxy {
            None => client::connect(config, (resolved.hostname.as_str(), resolved.port), handler)
                .await
                .map(|h| (h, None)),
            Some(proxy) => {
                let mut child = proxy_command(proxy).spawn().map_err(|e| {
                    let what = match proxy {
                        Proxy::Jump(_) => "ssh (for ProxyJump)",
                        Proxy::Command(_) => "the ProxyCommand",
                    };
                    russh::Error::IO(std::io::Error::other(format!("couldn't run {what}: {e}")))
                })?;
                if let Some(mut err) = child.stderr.take() {
                    let sink = proxy_stderr.clone();
                    tokio::spawn(async move {
                        let mut buf = Vec::new();
                        let _ = err.read_to_end(&mut buf).await;
                        sink.lock()
                            .unwrap()
                            .push_str(&String::from_utf8_lossy(&buf));
                    });
                }
                let (Some(out), Some(inp)) = (child.stdout.take(), child.stdin.take()) else {
                    return Err(russh::Error::Disconnect);
                };
                client::connect_stream(config, tokio::io::join(out, inp), handler)
                    .await
                    .map(|h| (h, Some(child)))
            }
        }
    };
    let (mut handle, proxy) = match tokio::time::timeout(resolved.connect_timeout, connecting).await
    {
        Err(_) => {
            return Err(SshError::new(
                "connect",
                format!(
                    "Timed out connecting to {}:{} after {} s",
                    resolved.hostname,
                    resolved.port,
                    resolved.connect_timeout.as_secs()
                ),
            ));
        }
        Ok(Err(e)) => {
            if let Some(issue) = issue.lock().unwrap().take() {
                return Err(issue);
            }
            let mut message = describe(&e, &resolved);
            // Give the proxy a moment to explain itself ("Could not resolve hostname …").
            tokio::time::sleep(Duration::from_millis(100)).await;
            let stderr = proxy_stderr.lock().unwrap().trim().to_string();
            if !stderr.is_empty() {
                message = format!("{message}. The proxy said: {stderr}");
            }
            return Err(SshError::new("connect", message));
        }
        Ok(Ok(ok)) => ok,
    };
    let auth = tokio::time::timeout(
        Duration::from_secs(60),
        authenticate(&mut handle, &resolved, req),
    );
    match auth.await {
        Err(_) => Err(SshError::new("auth", "Timed out while logging in")),
        Ok(Err(e)) => Err(e),
        Ok(Ok(())) => Ok(Session {
            handle,
            resolved,
            _proxy: proxy,
        }),
    }
}

fn allows(methods: &Option<MethodSet>, kind: MethodKind) -> bool {
    methods.as_ref().is_none_or(|m| m.contains(&kind))
}

fn default_keys() -> Vec<PathBuf> {
    let Some(home) = options::home() else {
        return Vec::new();
    };
    ["id_ed25519", "id_ecdsa", "id_rsa"]
        .iter()
        .map(|k| home.join(".ssh").join(k))
        .filter(|p| p.exists())
        .collect()
}

fn display_path(p: &Path) -> String {
    if let Some(home) = options::home()
        && let Ok(rest) = p.strip_prefix(&home)
    {
        return format!("~/{}", rest.to_string_lossy());
    }
    p.to_string_lossy().into_owned()
}

/// Tries the methods in order (DESIGN §6) until one works.
async fn authenticate(
    handle: &mut Handle<Client>,
    r: &Resolved,
    req: &Request,
) -> Result<(), SshError> {
    let user = r.user.as_str();
    let net = |e: russh::Error| {
        SshError::new("connect", format!("Connection lost while logging in: {e}"))
    };
    let mut tried: Vec<String> = Vec::new();
    let mut needs_passphrase: Option<PathBuf> = None;

    // `none` first: it tells us which methods the server takes, and dev boards sometimes
    // need nothing at all.
    let mut methods: Option<MethodSet> = match handle.authenticate_none(user).await.map_err(net)? {
        client::AuthResult::Success => return Ok(()),
        client::AuthResult::Failure {
            remaining_methods, ..
        } => Some(remaining_methods),
    };
    let update = |res: client::AuthResult, methods: &mut Option<MethodSet>| -> bool {
        match res {
            client::AuthResult::Success => true,
            client::AuthResult::Failure {
                remaining_methods, ..
            } => {
                *methods = Some(remaining_methods);
                false
            }
        }
    };

    let use_keys = req.auth != AuthMode::Password;
    let mut agent_keys: Vec<PublicKey> = Vec::new();
    if req.auth == AuthMode::Auto
        && allows(&methods, MethodKind::PublicKey)
        && let Some(mut agent) = agent().await
    {
        {
            let identities = agent.request_identities().await.unwrap_or_default();
            for identity in identities {
                let AgentIdentity::PublicKey { key, comment } = identity else {
                    continue;
                };
                let hash = handle
                    .best_supported_rsa_hash()
                    .await
                    .ok()
                    .flatten()
                    .flatten();
                let res = handle
                    .authenticate_publickey_with(user, key.clone(), hash, &mut agent)
                    .await;
                tried.push(if comment.is_empty() {
                    "agent key".to_string()
                } else {
                    format!("agent key {comment}")
                });
                match res {
                    Ok(res) => {
                        if update(res, &mut methods) {
                            return Ok(());
                        }
                    }
                    Err(_) => break,
                }
                agent_keys.push(key);
                if !allows(&methods, MethodKind::PublicKey) {
                    break;
                }
            }
        }
    }

    if use_keys {
        let files: Vec<PathBuf> = match (req.auth, &req.key_file) {
            (AuthMode::Key, Some(k)) if !k.is_empty() => vec![PathBuf::from(k)],
            (AuthMode::Key, _) => return Err(SshError::new("options", "Choose a key file")),
            _ if !r.identity_files.is_empty() => r.identity_files.clone(),
            _ => default_keys(),
        };
        for file in files {
            if !allows(&methods, MethodKind::PublicKey) {
                break;
            }
            let shown = display_path(&file);
            let text = match std::fs::read_to_string(&file) {
                Ok(t) => t,
                Err(e) if req.auth == AuthMode::Key => {
                    return Err(SshError::new(
                        "options",
                        format!("Couldn't read {shown}: {e}"),
                    ));
                }
                Err(_) => continue,
            };
            // Skip keys the agent already offered.
            if let Ok(public) = std::fs::read_to_string(file.with_extension("pub"))
                .map_err(|_| ())
                .and_then(|t| PublicKey::from_openssh(&t).map_err(|_| ()))
                && agent_keys.iter().any(|k| k.key_data() == public.key_data())
            {
                continue;
            }
            let passphrase = req.passphrase.as_deref().filter(|p| !p.is_empty());
            let key = match russh::keys::decode_secret_key(&text, passphrase) {
                Ok(k) => k,
                Err(russh::keys::Error::KeyIsEncrypted) => {
                    needs_passphrase.get_or_insert(file.clone());
                    continue;
                }
                Err(e) if passphrase.is_some() && is_decrypt_error(&e) => {
                    return Err(SshError {
                        key_file: Some(file.to_string_lossy().into_owned()),
                        ..SshError::new("need_passphrase", format!("Wrong passphrase for {shown}"))
                    });
                }
                Err(e) => {
                    if req.auth == AuthMode::Key {
                        return Err(SshError::new(
                            "options",
                            format!("Couldn't load {shown}: {e}"),
                        ));
                    }
                    continue;
                }
            };
            let hash = handle
                .best_supported_rsa_hash()
                .await
                .ok()
                .flatten()
                .flatten();
            let res = handle
                .authenticate_publickey(user, PrivateKeyWithHashAlg::new(Arc::new(key), hash))
                .await
                .map_err(net)?;
            tried.push(format!("key {shown}"));
            if update(res, &mut methods) {
                return Ok(());
            }
        }
    }

    let use_password = req.auth != AuthMode::Key;
    let password = req.password.clone();
    if use_password {
        // No password given: an empty one (common for root on dev boards).
        let pw = password.clone().unwrap_or_default();
        if allows(&methods, MethodKind::Password) {
            let res = handle
                .authenticate_password(user, pw.clone())
                .await
                .map_err(net)?;
            tried.push(if pw.is_empty() {
                "empty password".into()
            } else {
                "password".into()
            });
            if update(res, &mut methods) {
                return Ok(());
            }
        }
        if allows(&methods, MethodKind::KeyboardInteractive) {
            if keyboard_interactive(handle, user, &pw).await.map_err(net)? {
                return Ok(());
            }
            tried.push(if pw.is_empty() {
                "keyboard-interactive (empty)".into()
            } else {
                "keyboard-interactive".into()
            });
        }
    }

    let server_takes_passwords =
        allows(&methods, MethodKind::Password) || allows(&methods, MethodKind::KeyboardInteractive);
    if let Some(file) = needs_passphrase {
        let shown = display_path(&file);
        return Err(SshError {
            key_file: Some(file.to_string_lossy().into_owned()),
            ..SshError::new("need_passphrase", format!("{shown} needs its passphrase"))
        });
    }
    if use_password && password.is_none() && server_takes_passwords {
        return Err(SshError::new(
            "need_password",
            format!("{}@{} needs a password", r.user, r.alias),
        ));
    }
    let tried = if tried.is_empty() {
        "nothing the server accepts".to_string()
    } else {
        tried.join(", ")
    };
    Err(SshError::new(
        "auth",
        format!(
            "Permission denied for {}@{} (tried: {tried})",
            r.user, r.alias
        ),
    ))
}

fn is_decrypt_error(e: &russh::keys::Error) -> bool {
    let text = e.to_string().to_ascii_lowercase();
    text.contains("decrypt")
        || text.contains("crypto")
        || text.contains("passphrase")
        || text.contains("password")
}

/// Answers every prompt with the password, for a few rounds.
async fn keyboard_interactive(
    handle: &mut Handle<Client>,
    user: &str,
    password: &str,
) -> Result<bool, russh::Error> {
    let mut res = handle
        .authenticate_keyboard_interactive_start(user, None)
        .await?;
    for _ in 0..4 {
        match res {
            KeyboardInteractiveAuthResponse::Success => return Ok(true),
            KeyboardInteractiveAuthResponse::Failure { .. } => return Ok(false),
            KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. } => {
                let answers = prompts.iter().map(|_| password.to_string()).collect();
                res = handle
                    .authenticate_keyboard_interactive_respond(answers)
                    .await?;
            }
        }
    }
    Ok(false)
}

#[cfg(unix)]
async fn agent() -> Option<russh::keys::agent::client::AgentClient<tokio::net::UnixStream>> {
    russh::keys::agent::client::AgentClient::connect_env()
        .await
        .ok()
}

#[cfg(windows)]
async fn agent()
-> Option<russh::keys::agent::client::AgentClient<tokio::net::windows::named_pipe::NamedPipeClient>>
{
    russh::keys::agent::client::AgentClient::connect_named_pipe(r"\\.\pipe\openssh-ssh-agent")
        .await
        .ok()
}

/// Output of a remote command.
pub struct Output {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub status: Option<u32>,
}

/// Runs `command` and collects its output, giving up after `limit`.
pub async fn run(
    handle: &Handle<Client>,
    command: &str,
    limit: Duration,
) -> Result<Output, SshError> {
    let lost = |e: russh::Error| SshError::new("closed", format!("Lost the connection: {e}"));
    let work = async {
        let mut channel = handle.channel_open_session().await.map_err(lost)?;
        channel.exec(true, command).await.map_err(lost)?;
        let mut out = Output {
            stdout: Vec::new(),
            stderr: Vec::new(),
            status: None,
        };
        while let Some(msg) = channel.wait().await {
            match msg {
                ChannelMsg::Data { data } => out.stdout.extend_from_slice(&data),
                ChannelMsg::ExtendedData { data, .. } => out.stderr.extend_from_slice(&data),
                ChannelMsg::ExitStatus { exit_status } => out.status = Some(exit_status),
                ChannelMsg::Close => break,
                _ => {}
            }
        }
        Ok(out)
    };
    tokio::time::timeout(limit, work).await.map_err(|_| {
        SshError::new(
            "fetch",
            format!("The device didn't answer within {} s", limit.as_secs()),
        )
    })?
}

/// Quotes for a POSIX shell.
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Prints `TIMBER <how> <config.gz readable> <path>`, then the tree (DESIGN §6).
const FETCH_SCRIPT: &str = r#"cfg=0; [ -r /proc/config.gz ] && cfg=1
for d in "$1" /sys/firmware/devicetree/base; do
  [ -d "$d" ] || continue
  if command -v tar >/dev/null 2>&1; then
    # Mostly tiny files padded to 512 bytes: gzip makes it ~50x smaller.
    if command -v gzip >/dev/null 2>&1; then
      echo "TIMBER TGZ $cfg $d"; tar -C "$d" -cf - . | gzip; exit 0
    fi
    echo "TIMBER TAR $cfg $d"; exec tar -C "$d" -cf - .
  fi
  echo "TIMBER OD $cfg $d"
  cd "$d" && find . | while IFS= read -r f; do if [ -d "$f" ]; then echo "D $f"; else echo "F $f"; od -An -v -tx1 "$f"; fi; done
  exit 0
done
if [ -r /sys/firmware/fdt ]; then echo "TIMBER FDT $cfg /sys/firmware/fdt"; exec cat /sys/firmware/fdt; fi
echo "TIMBER NONE $cfg $1"
"#;

pub const DEFAULT_TREE_PATH: &str = "/proc/device-tree";

pub struct Fetched {
    /// How it was read: `tar.gz`, `tar` or `od` (the running kernel's tree), or `fdt` (the blob the
    /// bootloader handed over, `/sys/firmware/fdt`).
    pub method: &'static str,
    /// Where it was read from on the device.
    pub path: String,
    pub has_config: bool,
    pub dtb: Vec<u8>,
}

pub async fn fetch(handle: &Handle<Client>, tree_path: &str) -> Result<Fetched, SshError> {
    let command = format!("sh -c {} timber {}", quote(FETCH_SCRIPT), quote(tree_path));
    let out = run(handle, &command, Duration::from_secs(90)).await?;
    parse_fetch(&out, tree_path)
}

/// `fetch` with the device's `PATH` replaced (to test the fallbacks; the sshd tests are
/// Unix only).
#[cfg(all(test, unix))]
pub async fn fetch_with_path(
    handle: &Handle<Client>,
    tree_path: &str,
    path: &str,
) -> Result<Fetched, SshError> {
    let command = format!(
        "PATH={} sh -c {} timber {}",
        quote(path),
        quote(FETCH_SCRIPT),
        quote(tree_path)
    );
    let out = run(handle, &command, Duration::from_secs(90)).await?;
    parse_fetch(&out, tree_path)
}

fn parse_fetch(out: &Output, tree_path: &str) -> Result<Fetched, SshError> {
    let fail = |m: String| SshError::new("fetch", m);
    // Find the header even if a login script printed something first.
    let data = &out.stdout;
    let start = data
        .windows(7)
        .position(|w| w == b"TIMBER ")
        .filter(|&i| i == 0 || data[i - 1] == b'\n')
        .ok_or_else(|| {
            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            fail(if err.is_empty() {
                "The device didn't run Timber's fetch script (is `sh` available?)".into()
            } else {
                format!("The fetch script failed on the device: {err}")
            })
        })?;
    let nl = data[start..]
        .iter()
        .position(|&b| b == b'\n')
        .map(|i| start + i)
        .ok_or_else(|| fail("The device's answer was cut short".into()))?;
    let header = String::from_utf8_lossy(&data[start..nl]).into_owned();
    let mut parts = header.splitn(4, ' ').skip(1);
    let how = parts.next().unwrap_or_default();
    let has_config = parts.next() == Some("1");
    let path = parts.next().unwrap_or_default().to_string();
    let body = &data[nl + 1..];
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    let tree = match how {
        "TAR" => timber_core::live::from_tar(body),
        "TGZ" => {
            let mut tar = Vec::new();
            std::io::Read::read_to_end(&mut flate2::read::GzDecoder::new(body), &mut tar)
                .map_err(|e| fail(format!("Couldn't unpack the device's tree: {e}")))?;
            timber_core::live::from_tar(&tar)
        }
        "OD" => timber_core::live::from_od(&String::from_utf8_lossy(body)),
        "FDT" => {
            return if timber_core::dtb::is_dtb(body) {
                Ok(Fetched {
                    method: "fdt",
                    path,
                    has_config,
                    dtb: body.to_vec(),
                })
            } else {
                Err(fail(format!(
                    "/sys/firmware/fdt didn't hold a device tree. {stderr}"
                )))
            };
        }
        _ => {
            let fallback = if tree_path == "/sys/firmware/devicetree/base" {
                String::new()
            } else {
                " or /sys/firmware/devicetree/base".into()
            };
            return Err(fail(format!(
                "No device tree at {tree_path}{fallback}, and /sys/firmware/fdt isn't readable \
                 (it needs root)"
            )));
        }
    };
    let tree = tree.map_err(|e| {
        fail(if stderr.is_empty() {
            format!("Couldn't read the device's tree: {e}")
        } else {
            format!("Couldn't read the device's tree: {e} ({stderr})")
        })
    })?;
    Ok(Fetched {
        method: match how {
            "OD" => "od",
            "TGZ" => "tar.gz",
            _ => "tar",
        },
        path,
        has_config,
        dtb: timber_core::dtb::write(&tree),
    })
}

/// The device's `/proc/config.gz`, unpacked.
pub async fn device_config(handle: &Handle<Client>) -> Result<Vec<u8>, SshError> {
    let out = run(handle, "cat /proc/config.gz", Duration::from_secs(30)).await?;
    if out.status != Some(0) || out.stdout.is_empty() {
        return Err(SshError::new(
            "fetch",
            format!(
                "Couldn't read /proc/config.gz: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        ));
    }
    let mut text = Vec::new();
    std::io::Read::read_to_end(
        &mut flate2::read::GzDecoder::new(&out.stdout[..]),
        &mut text,
    )
    .map_err(|e| SshError::new("fetch", format!("Couldn't unpack /proc/config.gz: {e}")))?;
    Ok(text)
}
