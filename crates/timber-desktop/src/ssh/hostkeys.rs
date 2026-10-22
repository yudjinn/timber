//! Host keys, trust on first use (D-007, D-028): keys the user accepted live in Timber's own
//! `known_hosts`; `~/.ssh/known_hosts` is consulted but never written. Timber's entry wins
//! where they disagree, so a reflashed device can be re-trusted without touching OpenSSH's.

use std::path::Path;

use russh::keys::known_hosts::known_host_keys_path;
use russh::keys::{HashAlg, PublicKey};

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Known,
    Unknown,
    /// A different key of the same type was recorded for this host.
    Changed {
        old: PublicKey,
    },
}

pub fn fingerprint(key: &PublicKey) -> String {
    key.fingerprint(HashAlg::Sha256).to_string()
}

/// How `known_hosts` names a host: `host`, or `[host]:port` off port 22.
fn host_token(host: &str, port: u16) -> String {
    if port == 22 {
        host.to_string()
    } else {
        format!("[{host}]:{port}")
    }
}

fn check_file(path: &Path, host: &str, port: u16, key: &PublicKey) -> Option<Status> {
    let recorded = known_host_keys_path(host, port, path).ok()?;
    let same_type = recorded
        .into_iter()
        .map(|(_, k)| k)
        .filter(|k| k.algorithm() == key.algorithm())
        .collect::<Vec<_>>();
    if same_type.iter().any(|k| k.key_data() == key.key_data()) {
        Some(Status::Known)
    } else {
        same_type
            .into_iter()
            .next()
            .map(|old| Status::Changed { old })
    }
}

pub fn check(timber: &Path, user: Option<&Path>, host: &str, port: u16, key: &PublicKey) -> Status {
    check_file(timber, host, port, key)
        .or_else(|| user.and_then(|u| check_file(u, host, port, key)))
        .unwrap_or(Status::Unknown)
}

/// Records `key` for the host in Timber's file, replacing any key of the same type.
pub fn trust(timber: &Path, host: &str, port: u16, key: &PublicKey) -> Result<(), String> {
    let token = host_token(host, port);
    let line = key.to_openssh().map_err(|e| e.to_string())?;
    let algorithm = key.algorithm().to_string();
    let existing = std::fs::read_to_string(timber).unwrap_or_default();
    let mut out: Vec<&str> = existing
        .lines()
        .filter(|l| {
            let mut parts = l.split_whitespace();
            !(parts.next() == Some(&token) && parts.next() == Some(&algorithm))
        })
        .collect();
    let new = format!(
        "{token} {}",
        line.split_whitespace()
            .take(2)
            .collect::<Vec<_>>()
            .join(" ")
    );
    out.push(&new);
    if let Some(dir) = timber.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(timber, out.join("\n") + "\n").map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh ed25519 public key, from `ssh-keygen`.
    pub(crate) fn key() -> PublicKey {
        let dir = std::env::temp_dir().join(format!(
            "timber-key-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("k");
        let ok = std::process::Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-f"])
            .arg(&path)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        let text = std::fs::read_to_string(path.with_extension("pub")).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        PublicKey::from_openssh(&text).unwrap()
    }

    #[test]
    fn tofu_and_reflash() {
        let dir = std::env::temp_dir().join(format!("timber-hk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let timber = dir.join("known_hosts");
        let user = dir.join("user_known_hosts");
        let (a, b) = (key(), key());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&user, format!("[board]:2222 {}\n", a.to_openssh().unwrap())).unwrap();

        assert_eq!(
            check(&timber, Some(&user), "board", 2222, &a),
            Status::Known
        );
        assert_eq!(
            check(&timber, Some(&user), "other", 2222, &a),
            Status::Unknown
        );
        // Reflashed: OpenSSH's file has the old key.
        let Status::Changed { old } = check(&timber, Some(&user), "board", 2222, &b) else {
            panic!("expected a changed key");
        };
        assert_eq!(old.key_data(), a.key_data());
        trust(&timber, "board", 2222, &b).unwrap();
        assert_eq!(
            check(&timber, Some(&user), "board", 2222, &b),
            Status::Known
        );
        // OpenSSH's file is untouched; reflashed again, the old Timber entry is replaced.
        assert!(
            std::fs::read_to_string(&user)
                .unwrap()
                .contains(&a.to_openssh().unwrap())
        );
        let c = key();
        assert!(matches!(
            check(&timber, Some(&user), "board", 2222, &c),
            Status::Changed { .. }
        ));
        trust(&timber, "board", 2222, &c).unwrap();
        assert_eq!(std::fs::read_to_string(&timber).unwrap().lines().count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
