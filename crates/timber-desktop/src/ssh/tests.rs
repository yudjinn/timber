//! Against a throwaway `sshd` on a free port, run as the current user with temporary keys
//! and a fake `/proc/device-tree` (the tree path is configurable for exactly this). Skipped
//! if `sshd`/`ssh-keygen` aren't installed. Password and keyboard-interactive logins can't
//! be tested this way: an unprivileged `sshd` can't check passwords.

use std::collections::BTreeMap;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use timber_core::vfs::MemVfs;
use timber_core::{NodeId, Tree, dtb, dts};

use super::client::{self, AuthMode, Paths, Request};
use super::{Live, hostkeys, ready};

const SSHD: &str = "/usr/bin/sshd";

fn have_tools() -> bool {
    Path::new(SSHD).exists()
        && Command::new("ssh-keygen").arg("-?").output().is_ok()
        && std::env::var("USER").is_ok()
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("timber-ssh-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn keygen(path: &Path, passphrase: &str) {
    let ok = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", passphrase, "-f"])
        .arg(path)
        .status()
        .unwrap()
        .success();
    assert!(ok);
}

fn fixture_tree() -> Tree {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../timber-core/tests/fixtures");
    let mut vfs = MemVfs::default();
    for f in ["board.dts", "soc.dtsi", "include/dt-bindings/soc.h"] {
        vfs.insert(f, std::fs::read(dir.join(f)).unwrap());
    }
    dts::compile(&vfs, "board.dts").tree
}

/// Lays `tree` out like `/proc/device-tree`, with the kernel's `name` properties.
fn write_dir(tree: &Tree, id: NodeId, dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    let node = tree.node(id);
    let name = if id == Tree::ROOT {
        vec![0]
    } else {
        [node.base_name().as_bytes(), &[0]].concat()
    };
    std::fs::write(dir.join("name"), name).unwrap();
    for p in &node.props {
        std::fs::write(dir.join(&p.name), &p.value).unwrap();
    }
    for &c in &node.children {
        write_dir(tree, c, &dir.join(&tree.node(c).name));
    }
}

fn flat(tree: &Tree) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    for id in 0..tree.len() {
        let path = tree.path(id);
        out.insert(format!("{path}/"), Vec::new());
        for p in &tree.node(id).props {
            out.insert(format!("{path}:{}", p.name), p.value.clone());
        }
    }
    out
}

/// A device: an sshd with its own host key, plus keys a client can log in with.
struct Device {
    dir: Scratch,
    port: u16,
    sshd: Option<Child>,
}

impl Device {
    fn new(name: &str) -> Self {
        let dir = Scratch::new(name);
        let d = &dir.0;
        keygen(&d.join("host_key"), "");
        keygen(&d.join("client"), "");
        keygen(&d.join("locked"), "sesame");
        let auth = [
            std::fs::read_to_string(d.join("client.pub")).unwrap(),
            std::fs::read_to_string(d.join("locked.pub")).unwrap(),
        ]
        .concat();
        std::fs::write(d.join("authorized_keys"), auth).unwrap();
        write_dir(&fixture_tree(), Tree::ROOT, &d.join("device-tree"));
        // PATHs without `tar` (the `od` fallback) and without `gzip` (plain tar).
        for (dir, tools) in [
            ("notar", &["sh", "find", "od", "cat", "gzip"]),
            ("nogzip", &["sh", "find", "od", "cat", "tar"]),
        ] {
            std::fs::create_dir_all(d.join(dir)).unwrap();
            for tool in tools {
                std::os::unix::fs::symlink(which(tool), d.join(dir).join(tool)).unwrap();
            }
        }
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let mut dev = Device {
            dir,
            port,
            sshd: None,
        };
        dev.start();
        dev
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.0.join(name)
    }

    fn start(&mut self) {
        let d = &self.dir.0;
        let config = format!(
            "Port {port}\nListenAddress 127.0.0.1\nHostKey {d}/host_key\nPidFile {d}/sshd.pid\n\
             AuthorizedKeysFile {d}/authorized_keys\nUsePAM no\nStrictModes no\n\
             PasswordAuthentication no\nKbdInteractiveAuthentication no\nMaxAuthTries 30\n\
             LogLevel ERROR\n",
            port = self.port,
            d = d.display()
        );
        std::fs::write(d.join("sshd_config"), config).unwrap();
        let child = Command::new(SSHD)
            .args(["-D", "-e", "-f"])
            .arg(d.join("sshd_config"))
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        self.sshd = Some(child);
        for _ in 0..100 {
            if std::net::TcpStream::connect(("127.0.0.1", self.port)).is_ok() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("sshd didn't start");
    }

    fn stop(&mut self) {
        if let Some(mut c) = self.sshd.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    fn paths(&self) -> Paths {
        Paths {
            known_hosts: self.path("timber_known_hosts"),
            user_known_hosts: None,
            ssh_config: None,
        }
    }

    fn request(&self, key: &str) -> Request {
        Request {
            target: format!("127.0.0.1:{}", self.port),
            auth: AuthMode::Key,
            key_file: Some(self.path(key).to_string_lossy().into_owned()),
            tree_path: Some(self.path("device-tree").to_string_lossy().into_owned()),
            ..Request::default()
        }
    }

    /// Trusts the device's current host key.
    fn trust(&self) {
        let key = std::fs::read_to_string(self.path("host_key.pub")).unwrap();
        let key = russh::keys::PublicKey::from_openssh(&key).unwrap();
        hostkeys::trust(&self.paths().known_hosts, "127.0.0.1", self.port, &key).unwrap();
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        self.stop();
    }
}

fn which(tool: &str) -> PathBuf {
    let out = Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {tool}"))
        .output()
        .unwrap();
    PathBuf::from(String::from_utf8(out.stdout).unwrap().trim())
}

#[tokio::test(flavor = "multi_thread")]
async fn host_keys_and_fetch() {
    if !have_tools() {
        return eprintln!("skipping: sshd or ssh-keygen not installed");
    }
    let mut dev = Device::new("main");
    let paths = dev.paths();

    // First contact: unknown key, reported with its fingerprint.
    let err = client::connect(&dev.request("client"), &paths)
        .await
        .err()
        .unwrap();
    assert_eq!(err.kind, "host_key_unknown", "{err:?}");
    let info = err.host_key.unwrap();
    let want = russh::keys::PublicKey::from_openssh(
        &std::fs::read_to_string(dev.path("host_key.pub")).unwrap(),
    )
    .unwrap();
    assert_eq!(info.fingerprint, hostkeys::fingerprint(&want));
    super::ssh_trust_paths(&paths, &info.host, info.port, &info.key).unwrap();

    // Trusted: log in and read the tree with tar.
    let session = client::connect(&dev.request("client"), &paths)
        .await
        .unwrap();
    let tree_path = dev.path("device-tree").to_string_lossy().into_owned();
    let fetched = client::fetch(&session.handle, &tree_path).await.unwrap();
    assert_eq!(fetched.method, "tar.gz");
    let got = dtb::parse(&fetched.dtb).unwrap();
    assert_eq!(flat(&got), flat(&fixture_tree()));
    assert_eq!(fetched.has_config, Path::new("/proc/config.gz").exists());
    if fetched.has_config {
        let config = client::device_config(&session.handle).await.unwrap();
        assert!(String::from_utf8_lossy(&config).contains("CONFIG_"));
    }

    // No tree anywhere (this test machine has no /sys/firmware/devicetree): a clear message.
    if !Path::new("/sys/firmware/devicetree/base").exists()
        && !Path::new("/sys/firmware/fdt").exists()
    {
        let err = client::fetch(&session.handle, "/nonexistent")
            .await
            .err()
            .unwrap();
        assert!(
            err.message.contains("No device tree at /nonexistent"),
            "{err:?}"
        );
    }

    // The `od` fallback, with a PATH that has no tar.
    let notar = dev.path("notar").to_string_lossy().into_owned();
    let out = client::run(
        &session.handle,
        &format!("PATH={notar} sh -c 'command -v tar || echo none'"),
        Duration::from_secs(10),
    )
    .await
    .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "none");
    let fetched = client::fetch_with_path(&session.handle, &tree_path, &notar)
        .await
        .unwrap();
    assert_eq!(fetched.method, "od");
    assert_eq!(
        flat(&dtb::parse(&fetched.dtb).unwrap()),
        flat(&fixture_tree())
    );

    let nogzip = dev.path("nogzip").to_string_lossy().into_owned();
    let fetched = client::fetch_with_path(&session.handle, &tree_path, &nogzip)
        .await
        .unwrap();
    assert_eq!(fetched.method, "tar");
    assert_eq!(
        flat(&dtb::parse(&fetched.dtb).unwrap()),
        flat(&fixture_tree())
    );

    // Reconnects after the connection drops (the board rebooted).
    let mut live = Live {
        session,
        request: dev.request("client"),
    };
    live.session
        .handle
        .disconnect(russh::Disconnect::ByApplication, "", "en")
        .await
        .unwrap();
    for _ in 0..50 {
        if live.session.handle.is_closed() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(live.session.handle.is_closed());
    ready(&paths, &mut live).await.unwrap();
    assert!(
        client::fetch(&live.session.handle, &tree_path)
            .await
            .is_ok()
    );

    // Reflashed: a new host key on the same address.
    dev.stop();
    std::fs::remove_file(dev.path("host_key")).unwrap();
    std::fs::remove_file(dev.path("host_key.pub")).unwrap();
    keygen(&dev.path("host_key"), "");
    dev.start();
    let err = client::connect(&dev.request("client"), &paths)
        .await
        .err()
        .unwrap();
    assert_eq!(err.kind, "host_key_changed", "{err:?}");
    let info = err.host_key.unwrap();
    assert_eq!(info.old_fingerprint, Some(hostkeys::fingerprint(&want)));
    assert_ne!(info.fingerprint, hostkeys::fingerprint(&want));
    dev.trust();
    assert!(
        client::connect(&dev.request("client"), &paths)
            .await
            .is_ok()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn key_auth() {
    if !have_tools() {
        return eprintln!("skipping: sshd or ssh-keygen not installed");
    }
    let dev = Device::new("auth");
    dev.trust();
    let paths = dev.paths();

    // An encrypted key: asks for the passphrase, rejects a wrong one, accepts the right one.
    let err = client::connect(&dev.request("locked"), &paths)
        .await
        .err()
        .unwrap();
    assert_eq!(err.kind, "need_passphrase", "{err:?}");
    assert!(err.key_file.unwrap().ends_with("locked"));
    let mut req = dev.request("locked");
    req.passphrase = Some("wrong".into());
    let err = client::connect(&req, &paths).await.err().unwrap();
    assert_eq!(err.kind, "need_passphrase", "{err:?}");
    assert!(err.message.contains("Wrong passphrase"));
    req.passphrase = Some("sesame".into());
    assert!(client::connect(&req, &paths).await.is_ok());

    // Automatic, with the key given as an extra option.
    let req = Request {
        target: format!("127.0.0.1:{}", dev.port),
        options: format!("-i {}", dev.path("client").display()),
        ..Request::default()
    };
    let session = client::connect(&req, &paths).await.unwrap();
    assert_eq!(
        session.label(),
        format!("{}@127.0.0.1", std::env::var("USER").unwrap())
    );

    // A key the server doesn't accept: permission denied, naming what was tried.
    keygen(&dev.path("stranger"), "");
    let err = client::connect(&dev.request("stranger"), &paths)
        .await
        .err()
        .unwrap();
    assert_eq!(err.kind, "auth", "{err:?}");
    assert!(err.message.contains("stranger"), "{}", err.message);

    // Through a ProxyCommand.
    if which("socat").exists() {
        let req = Request {
            target: "device-behind-proxy".into(),
            options: format!(
                "-o ProxyCommand='socat - TCP:127.0.0.1:%p' -p {} -o HostName=127.0.0.1",
                dev.port
            ),
            auth: AuthMode::Key,
            key_file: Some(dev.path("client").to_string_lossy().into_owned()),
            ..Request::default()
        };
        let session = client::connect(&req, &paths).await.unwrap();
        assert!(session.label().ends_with("@device-behind-proxy"));
    }

    // Nothing listening.
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let req = Request {
        target: format!("127.0.0.1:{port}"),
        ..Request::default()
    };
    let err = client::connect(&req, &paths).await.err().unwrap();
    assert_eq!(err.kind, "connect");
    assert!(err.message.contains("refused"), "{}", err.message);
}
