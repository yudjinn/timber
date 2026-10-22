//! Where to connect and how: the dialog's fields, its "Extra options" (OpenSSH style,
//! D-029), and `~/.ssh/config`, resolved into one [`Resolved`].
//!
//! Precedence follows OpenSSH: the dialog and extra options (the "command line") win over
//! `~/.ssh/config`, and within the config the first value found wins.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// How to reach the device when it isn't a direct TCP connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Proxy {
    /// `ProxyCommand`, run through the shell, already %-expanded.
    Command(String),
    /// `ProxyJump`: run the system `ssh` with these arguments (`-W host:port …`).
    Jump(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// What was typed (an alias, or the host itself): names the device in the UI.
    pub alias: String,
    pub hostname: String,
    pub port: u16,
    pub user: String,
    /// `IdentityFile`s from `-i`, `-o` or the config, `~` expanded.
    pub identity_files: Vec<PathBuf>,
    pub proxy: Option<Proxy>,
    pub connect_timeout: Duration,
}

/// The settings that can come from the command line or the config, unset as `None`.
#[derive(Debug, Default, Clone)]
struct Settings {
    hostname: Option<String>,
    port: Option<u16>,
    user: Option<String>,
    identity_files: Vec<String>,
    proxy_jump: Option<String>,
    proxy_command: Option<String>,
    connect_timeout: Option<u64>,
}

impl Settings {
    /// Sets `key` unless already set (first value wins). Unknown keys are an error when
    /// `strict` (extra options); in the config they're ignored.
    fn set(&mut self, key: &str, value: &str, strict: bool) -> Result<(), String> {
        let value = value.trim();
        match key.to_ascii_lowercase().as_str() {
            "hostname" => {
                self.hostname.get_or_insert_with(|| value.to_string());
            }
            "port" => {
                let port = value
                    .parse()
                    .map_err(|_| format!("Port {value:?} isn't a number"))?;
                self.port.get_or_insert(port);
            }
            "user" => {
                self.user.get_or_insert_with(|| value.to_string());
            }
            "identityfile" => self.identity_files.push(value.to_string()),
            "proxyjump" => {
                self.proxy_jump.get_or_insert_with(|| value.to_string());
            }
            "proxycommand" => {
                self.proxy_command.get_or_insert_with(|| value.to_string());
            }
            "connecttimeout" => {
                let secs = value
                    .parse()
                    .map_err(|_| format!("ConnectTimeout {value:?} isn't a number of seconds"))?;
                self.connect_timeout.get_or_insert(secs);
            }
            _ if strict => {
                return Err(format!(
                    "Option {key} isn't supported. Timber understands HostName, Port, User, \
                     IdentityFile, ProxyJump, ProxyCommand and ConnectTimeout."
                ));
            }
            _ => {}
        }
        Ok(())
    }
}

/// Splits `user@host:port`, `[v6addr]:port` or `ssh://user@host:port`.
pub fn parse_target(target: &str) -> (Option<String>, String, Option<u16>) {
    let t = target.trim();
    let t = t.strip_prefix("ssh://").unwrap_or(t);
    let (user, rest) = match t.rsplit_once('@') {
        Some((u, r)) => (Some(u.to_string()), r),
        None => (None, t),
    };
    if let Some(v6) = rest.strip_prefix('[')
        && let Some((host, after)) = v6.split_once(']')
    {
        let port = after.strip_prefix(':').and_then(|p| p.parse().ok());
        return (user, host.to_string(), port);
    }
    match rest.split_once(':') {
        // More than one colon: a bare IPv6 address.
        Some((host, port)) if !port.contains(':') => match port.parse() {
            Ok(p) => (user, host.to_string(), Some(p)),
            Err(_) => (user, rest.to_string(), None),
        },
        _ => (user, rest.to_string(), None),
    }
}

/// Splits a line into words the way a shell would (quotes and backslashes).
pub fn words(line: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = line.chars();
    let mut quote: Option<char> = None;
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') => cur.extend(chars.next()),
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                in_word = true;
            }
            (None, '\\') => {
                cur.extend(chars.next());
                in_word = true;
            }
            (None, c) if c.is_whitespace() => {
                if in_word {
                    out.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            (None, c) => {
                cur.push(c);
                in_word = true;
            }
        }
    }
    if quote.is_some() {
        return Err("Unclosed quote in the extra options".into());
    }
    if in_word {
        out.push(cur);
    }
    Ok(out)
}

/// Applies the dialog's "Extra options".
fn apply_extra(settings: &mut Settings, extra: &str) -> Result<(), String> {
    let args = words(extra)?;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let (flag, attached) = match arg.strip_prefix('-') {
            Some(rest) if !rest.is_empty() => rest.split_at(1),
            _ => return Err(format!("Unexpected {arg:?} in the extra options")),
        };
        let mut value = || -> Result<String, String> {
            if !attached.is_empty() {
                return Ok(attached.to_string());
            }
            it.next().cloned().ok_or(format!("-{flag} needs a value"))
        };
        match flag {
            "J" => settings.set("ProxyJump", &value()?, true)?,
            "p" => settings.set("Port", &value()?, true)?,
            "l" => settings.set("User", &value()?, true)?,
            "i" => settings.set("IdentityFile", &value()?, true)?,
            "o" => {
                let v = value()?;
                let (k, val) = v
                    .split_once('=')
                    .or_else(|| v.trim().split_once(char::is_whitespace))
                    .ok_or(format!("-o {v:?} needs a value (Key=value)"))?;
                settings.set(k.trim(), val, true)?;
            }
            _ => {
                return Err(format!(
                    "-{flag} isn't supported. Timber understands -J, -p, -l, -i and -o."
                ));
            }
        }
    }
    Ok(())
}

/// `Host` patterns: `*`, `?`, and `!negation`. Matches if any positive pattern matches and
/// no negated one does.
fn host_matches(patterns: &[String], host: &str) -> bool {
    let mut matched = false;
    for p in patterns {
        if let Some(neg) = p.strip_prefix('!') {
            if glob(neg, host) {
                return false;
            }
        } else if glob(p, host) {
            matched = true;
        }
    }
    matched
}

fn glob(pattern: &str, text: &str) -> bool {
    fn go(p: &[char], t: &[char]) -> bool {
        match p.split_first() {
            None => t.is_empty(),
            Some(('*', rest)) => (0..=t.len()).any(|i| go(rest, &t[i..])),
            Some(('?', rest)) => !t.is_empty() && go(rest, &t[1..]),
            Some((c, rest)) => {
                t.first().is_some_and(|tc| tc.eq_ignore_ascii_case(c)) && go(rest, &t[1..])
            }
        }
    }
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    go(&p, &t)
}

/// Reads `~/.ssh/config` (and its `Include`s) into `settings` for `host`. `Match` blocks
/// are skipped.
fn apply_config(settings: &mut Settings, path: &Path, ssh_dir: &Path, host: &str, depth: u8) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let mut active = true;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = match line.split_once(|c: char| c == '=' || c.is_whitespace()) {
            Some((k, v)) => (k.trim(), v.trim().trim_start_matches('=').trim()),
            None => continue,
        };
        let value_words = || words(value).unwrap_or_default();
        match key.to_ascii_lowercase().as_str() {
            "host" => active = host_matches(&value_words(), host),
            "match" => active = false,
            "include" if active && depth < 8 => {
                for pattern in value_words() {
                    for file in expand_include(&pattern, ssh_dir) {
                        apply_config(settings, &file, ssh_dir, host, depth + 1);
                    }
                }
            }
            _ if active => {
                let v = value_words();
                let v = if key.eq_ignore_ascii_case("proxycommand") {
                    value.to_string()
                } else {
                    v.first().cloned().unwrap_or_default()
                };
                let _ = settings.set(key, &v, false);
            }
            _ => {}
        }
    }
}

/// `Include` paths: relative to `~/.ssh`, with `*`/`?` in the file name.
fn expand_include(pattern: &str, ssh_dir: &Path) -> Vec<PathBuf> {
    let path = expand_home(pattern);
    let path = if path.is_absolute() {
        path
    } else {
        ssh_dir.join(path)
    };
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
    match (path.parent(), name) {
        (Some(dir), Some(name)) if name.contains(['*', '?']) => {
            let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
                .into_iter()
                .flatten()
                .flatten()
                .filter(|e| glob(&name, &e.file_name().to_string_lossy()))
                .map(|e| e.path())
                .collect();
            found.sort();
            found
        }
        _ => vec![path],
    }
}

pub fn home() -> Option<PathBuf> {
    #[allow(deprecated)] // fine on every platform Timber targets since Rust 1.85
    std::env::home_dir()
}

fn expand_home(p: &str) -> PathBuf {
    match (p.strip_prefix("~/").or(p.strip_prefix("~\\")), home()) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(p),
    }
}

fn local_user() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "root".into())
}

/// `%h`, `%p`, `%r`, `%n`, `%d`, `%%` in `ProxyCommand` and `IdentityFile`.
fn percent(s: &str, r: &Resolved) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('h') => out.push_str(&r.hostname),
            Some('p') => out.push_str(&r.port.to_string()),
            Some('r') => out.push_str(&r.user),
            Some('n') => out.push_str(&r.alias),
            Some('d') => out.push_str(&home().unwrap_or_default().to_string_lossy()),
            Some('%') => out.push('%'),
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    out
}

/// The connection's inputs (the dialog's fields).
pub struct Inputs<'a> {
    pub target: &'a str,
    pub user: Option<&'a str>,
    pub port: Option<u16>,
    pub extra: &'a str,
    /// `~/.ssh/config`, if any.
    pub config: Option<&'a Path>,
}

pub fn resolve(inputs: &Inputs) -> Result<Resolved, String> {
    let (t_user, alias, t_port) = parse_target(inputs.target);
    if alias.is_empty() {
        return Err("Enter a host to connect to".into());
    }
    let mut s = Settings {
        user: inputs
            .user
            .filter(|u| !u.is_empty())
            .map(str::to_string)
            .or(t_user),
        port: inputs.port.or(t_port),
        ..Settings::default()
    };
    apply_extra(&mut s, inputs.extra)?;
    if let Some(config) = inputs.config {
        let ssh_dir = config.parent().unwrap_or(Path::new("."));
        apply_config(&mut s, config, ssh_dir, &alias, 0);
    }
    let mut r = Resolved {
        hostname: alias.clone(),
        alias,
        port: s.port.unwrap_or(22),
        user: s.user.clone().unwrap_or_else(local_user),
        identity_files: Vec::new(),
        proxy: None,
        connect_timeout: Duration::from_secs(s.connect_timeout.unwrap_or(10).max(1)),
    };
    if let Some(hostname) = &s.hostname {
        r.hostname = percent(hostname, &r);
    }
    r.identity_files = s
        .identity_files
        .iter()
        .map(|f| expand_home(&percent(f, &r)))
        .collect();
    let jump = s.proxy_jump.filter(|j| !j.eq_ignore_ascii_case("none"));
    let command = s.proxy_command.filter(|c| !c.eq_ignore_ascii_case("none"));
    // Like OpenSSH, whichever came first wins; the command line is applied first, so
    // ProxyJump given there beats a ProxyCommand from the config and vice versa.
    r.proxy = match (jump, command) {
        (Some(j), _) => Some(Proxy::Jump(jump_args(&j, &r)?)),
        (None, Some(c)) => Some(Proxy::Command(percent(&c, &r))),
        (None, None) => None,
    };
    Ok(r)
}

/// `ssh` arguments that connect through the jump host(s) and forward stdio to the device.
fn jump_args(jumps: &str, r: &Resolved) -> Result<Vec<String>, String> {
    let hops: Vec<&str> = jumps
        .split(',')
        .map(str::trim)
        .filter(|h| !h.is_empty())
        .collect();
    let Some((last, before)) = hops.split_last() else {
        return Err("ProxyJump is empty".into());
    };
    let dest = if r.hostname.contains(':') {
        format!("[{}]:{}", r.hostname, r.port)
    } else {
        format!("{}:{}", r.hostname, r.port)
    };
    let mut args = vec!["-W".to_string(), dest];
    if !before.is_empty() {
        args.push("-J".into());
        args.push(before.join(","));
    }
    let (user, host, port) = parse_target(last);
    if let Some(port) = port {
        args.push("-p".into());
        args.push(port.to_string());
    }
    args.push(match user {
        Some(u) => format!("{u}@{host}"),
        None => host,
    });
    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs<'a>(target: &'a str, extra: &'a str, config: Option<&'a Path>) -> Inputs<'a> {
        Inputs {
            target,
            user: None,
            port: None,
            extra,
            config,
        }
    }

    #[test]
    fn targets() {
        assert_eq!(
            parse_target("root@board:2222"),
            (Some("root".into()), "board".into(), Some(2222))
        );
        assert_eq!(parse_target("board"), (None, "board".into(), None));
        assert_eq!(
            parse_target("[fe80::1]:22"),
            (None, "fe80::1".into(), Some(22))
        );
        assert_eq!(parse_target("fe80::1"), (None, "fe80::1".into(), None));
        assert_eq!(
            parse_target("ssh://u@h:7"),
            (Some("u".into()), "h".into(), Some(7))
        );
    }

    #[test]
    fn extra_options() {
        let r = resolve(&inputs(
            "board",
            "-p 2200 -l root -o ConnectTimeout=3 -oUser=ignored",
            None,
        ))
        .unwrap();
        assert_eq!(
            (r.port, r.user.as_str(), r.connect_timeout.as_secs()),
            (2200, "root", 3)
        );
        let r = resolve(&inputs("10.0.0.5", "-J me@gw:2022", None)).unwrap();
        assert_eq!(
            r.proxy,
            Some(Proxy::Jump(vec![
                "-W".into(),
                "10.0.0.5:22".into(),
                "-p".into(),
                "2022".into(),
                "me@gw".into()
            ]))
        );
        let r = resolve(&inputs("b", "-J a,b2,c", None)).unwrap();
        assert_eq!(
            r.proxy,
            Some(Proxy::Jump(vec![
                "-W".into(),
                "b:22".into(),
                "-J".into(),
                "a,b2".into(),
                "c".into()
            ]))
        );
        let r = resolve(&inputs("b", r#"-o "ProxyCommand nc -x proxy %h %p""#, None)).unwrap();
        assert_eq!(r.proxy, Some(Proxy::Command("nc -x proxy b 22".into())));
        assert!(
            resolve(&inputs("b", "-X", None))
                .unwrap_err()
                .contains("-X isn't supported")
        );
        assert!(
            resolve(&inputs("b", "-o StrictHostKeyChecking=no", None))
                .unwrap_err()
                .contains("isn't supported")
        );
    }

    #[test]
    fn config_file() {
        let dir = std::env::temp_dir().join(format!("timber-sshcfg-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("config.d")).unwrap();
        std::fs::write(
            dir.join("config"),
            "Include config.d/*\n\
             Host rock !rock-old\n  HostName 192.168.1.50\n  User root\n  IdentityFile ~/.ssh/dev_%r\n\
             Host *.lab\n  ProxyJump gw.example.com\n\
             Match host foo\n  User nope\n\
             Host *\n  User fallback\n  Port 2222\n",
        )
        .unwrap();
        std::fs::write(dir.join("config.d/early"), "Host rock\n  Port 22\n").unwrap();
        let config = dir.join("config");
        let r = resolve(&inputs("rock", "", Some(&config))).unwrap();
        assert_eq!(
            (r.hostname.as_str(), r.user.as_str(), r.port),
            ("192.168.1.50", "root", 22)
        );
        assert!(r.identity_files[0].ends_with(".ssh/dev_root"));
        // The command line wins over the config.
        let r = resolve(&inputs("admin@rock:2000", "", Some(&config))).unwrap();
        assert_eq!((r.user.as_str(), r.port), ("admin", 2000));
        let r = resolve(&inputs("x.lab", "", Some(&config))).unwrap();
        assert!(matches!(r.proxy, Some(Proxy::Jump(_))));
        assert_eq!((r.user.as_str(), r.port), ("fallback", 2222));
        let r = resolve(&inputs("rock-old", "", Some(&config))).unwrap();
        assert_eq!(r.hostname, "rock-old");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
