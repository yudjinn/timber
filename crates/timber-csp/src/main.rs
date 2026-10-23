//! A trunk post-build step (Trunk.toml): writes a Content-Security-Policy `<meta>` into the
//! built `index.html` (D-041). The policy only lets the page talk to its own origin, so the
//! browser itself stops it from sending your files anywhere.
//!
//! Trunk's inline startup script changes with every build (it names the hashed wasm), so it
//! is allowed by its SHA-256 hash, computed here. Dev builds also carry trunk's live-reload
//! script, whose address trunk fills in when serving; those get `'unsafe-inline'` and the
//! local WebSocket instead. In the desktop app (Tauri sets `TAURI_ENV_PLATFORM` for its
//! build commands), the policy also allows Tauri's IPC.

use std::path::PathBuf;

use base64::Engine;
use sha2::{Digest, Sha256};

/// The contents of every inline `<script>` (one without `src`).
fn inline_scripts(html: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(start) = rest.find("<script") {
        let after = &rest[start..];
        let Some(open_end) = after.find('>') else {
            break;
        };
        let tag = &after[..open_end];
        let body_start = open_end + 1;
        let Some(close) = after[body_start..].find("</script>") else {
            break;
        };
        if !tag.contains(" src=") {
            out.push(&after[body_start..body_start + close]);
        }
        rest = &after[body_start + close..];
    }
    out
}

fn sha256(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    format!(
        "'sha256-{}'",
        base64::engine::general_purpose::STANDARD.encode(digest)
    )
}

/// The policy for a page with these inline scripts.
pub fn policy(scripts: &[&str], desktop: bool) -> String {
    // Trunk's live-reload script has its address filled in by `trunk serve`, so it can't be
    // hashed ahead of time.
    let dev = scripts.iter().any(|s| s.contains("{{__TRUNK"));
    let mut script_src = vec!["'self'".to_string(), "'wasm-unsafe-eval'".into()];
    if dev {
        script_src.push("'unsafe-inline'".into());
    } else {
        script_src.extend(scripts.iter().map(|s| sha256(s)));
    }
    let mut connect = vec!["'self'"];
    if desktop {
        // Tauri's IPC: ipc://localhost (Linux, macOS), http://ipc.localhost (Windows).
        connect.extend(["ipc:", "http://ipc.localhost"]);
    }
    if dev {
        connect.extend(["ws://127.0.0.1:*", "ws://localhost:*"]);
    }
    [
        "default-src 'none'".to_string(),
        format!("script-src {}", script_src.join(" ")),
        // Leptos sets `style` attributes; CSS can only reach out through url(), which
        // img-src and font-src limit.
        "style-src 'self' 'unsafe-inline'".into(),
        // Image exports render through blob: and data: URLs.
        "img-src 'self' data: blob:".into(),
        "font-src 'self'".into(),
        format!("connect-src {}", connect.join(" ")),
        "manifest-src 'self'".into(),
        "base-uri 'none'".into(),
        "form-action 'none'".into(),
    ]
    .join("; ")
}

/// `html` with the policy `<meta>` first in `<head>`.
pub fn with_policy(html: &str, desktop: bool) -> Result<String, String> {
    let policy = policy(&inline_scripts(html), desktop);
    let meta = format!("<meta http-equiv=\"Content-Security-Policy\" content=\"{policy}\" />");
    let head = html.find("<head>").ok_or("no <head> in index.html")? + "<head>".len();
    Ok(format!("{}\n    {meta}{}", &html[..head], &html[head..]))
}

fn main() {
    let dir = std::env::var_os("TRUNK_STAGING_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::args_os().nth(1).map(PathBuf::from))
        .expect("run from trunk (TRUNK_STAGING_DIR), or pass the dist directory");
    let path = dir.join("index.html");
    let html = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let desktop = std::env::var_os("TAURI_ENV_PLATFORM").is_some();
    let out = with_policy(&html, desktop).unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(&path, out).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = "<html><head><script type=\"module\">init('/a_bg.wasm');</script>\
        <script src=\"/x.js\"></script></head><body><script>console.log(1)</script></body></html>";

    #[test]
    fn hashes_inline_scripts_only() {
        assert_eq!(
            inline_scripts(PAGE),
            ["init('/a_bg.wasm');", "console.log(1)"]
        );
        let p = policy(&inline_scripts(PAGE), false);
        // echo -n "console.log(1)" | openssl dgst -sha256 -binary | base64
        assert!(
            p.contains("'sha256-CihokcEcBW4atb/CW/XWsvWwbTjqwQlE9nj9ii5ww5M='"),
            "{p}"
        );
        assert!(p.contains("connect-src 'self';"), "{p}");
        assert!(
            !p.contains("unsafe-inline' 'sha")
                && !p.contains("script-src 'self' 'wasm-unsafe-eval' 'unsafe-inline'")
        );
    }

    #[test]
    fn dev_and_desktop() {
        let dev = ["const address = '{{__TRUNK_ADDRESS__}}';"];
        let p = policy(&dev, true);
        assert!(
            p.contains("script-src 'self' 'wasm-unsafe-eval' 'unsafe-inline'"),
            "{p}"
        );
        assert!(
            p.contains(
                "connect-src 'self' ipc: http://ipc.localhost ws://127.0.0.1:* ws://localhost:*"
            ),
            "{p}"
        );
        let html = with_policy(PAGE, false).unwrap();
        assert!(html.starts_with("<html><head>\n    <meta http-equiv=\"Content-Security-Policy\""));
    }
}
