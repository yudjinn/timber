//! A running device's tree, read from `/proc/device-tree` (or `/sys/firmware/devicetree/base`)
//! as a `tar` archive or as `od` hex dumps, turned back into a [`Tree`].
//!
//! In that directory, nodes are directories and properties are files holding the raw value.
//! The kernel adds a `name` property to every node; it's dropped where it only repeats the
//! node's name. Directory order there is hash order, so nodes are sorted by unit address and
//! properties into the usual DTS order.

use std::collections::HashMap;

use crate::{NodeId, Result, Tree, bail};

/// Builds a tree from `(path, Some(value))` properties and `(path, None)` directories, with
/// paths relative to the tree's root (`cpus/cpu@0/reg`).
pub fn from_entries<'a>(entries: impl IntoIterator<Item = (&'a str, Option<Vec<u8>>)>) -> Tree {
    let mut tree = Tree::new();
    let mut dirs: HashMap<String, NodeId> = HashMap::new();
    dirs.insert(String::new(), Tree::ROOT);
    fn dir(tree: &mut Tree, dirs: &mut HashMap<String, NodeId>, path: &str) -> NodeId {
        if let Some(&id) = dirs.get(path) {
            return id;
        }
        let (parent, name) = path.rsplit_once('/').unwrap_or(("", path));
        let parent = dir(tree, dirs, parent);
        let id = tree.add_child(parent, name);
        dirs.insert(path.to_string(), id);
        id
    }
    for (path, value) in entries {
        let path = clean(path);
        match value {
            None => {
                dir(&mut tree, &mut dirs, path);
            }
            Some(value) if !path.is_empty() => {
                let (parent, name) = path.rsplit_once('/').unwrap_or(("", path));
                let node = dir(&mut tree, &mut dirs, parent);
                tree.node_mut(node).set_prop(name, value);
            }
            Some(_) => {}
        }
    }
    tidy(&mut tree);
    tree.finalize();
    tree
}

fn clean(path: &str) -> &str {
    let path = path.trim_start_matches("./").trim_matches('/');
    if path == "." { "" } else { path }
}

/// Drops the kernel's `name` properties and puts nodes and properties in a readable order.
fn tidy(tree: &mut Tree) {
    for id in 0..tree.len() {
        let node = tree.node(id);
        let base = node.base_name().to_string();
        let synthetic = node.prop("name").is_some_and(|v| {
            let v = v.strip_suffix(&[0]).unwrap_or(v);
            v == base.as_bytes() || (id == Tree::ROOT && v.is_empty())
        });
        let node = tree.node_mut(id);
        if synthetic {
            node.props.retain(|p| p.name != "name");
        }
        node.props.sort_by_cached_key(|p| prop_rank(&p.name));
    }
    for id in 0..tree.len() {
        let mut children = std::mem::take(&mut tree.node_mut(id).children);
        children.sort_by_cached_key(|&c| child_key(tree.node(c).name.as_str()));
        tree.node_mut(id).children = children;
    }
}

/// DTS convention: identity and addressing first, `status` and phandles last.
fn prop_rank(name: &str) -> (u8, String) {
    const FIRST: &[&str] = &[
        "compatible",
        "model",
        "device_type",
        "reg",
        "reg-names",
        "ranges",
        "#address-cells",
        "#size-cells",
    ];
    let rank = match name {
        "status" => 250,
        "phandle" | "linux,phandle" => 251,
        _ => FIRST
            .iter()
            .position(|f| *f == name)
            .map_or(100, |i| i as u8),
    };
    (rank, name.to_string())
}

/// Nodes without a unit address first, then by address (numerically), then by name.
fn child_key(name: &str) -> (bool, u128, String) {
    match name.split_once('@') {
        Some((base, addr)) => {
            let first = addr.split(',').next().unwrap_or(addr);
            let n = u128::from_str_radix(first, 16).unwrap_or(u128::MAX);
            (true, n, format!("{base}@{addr}"))
        }
        None => (false, 0, name.to_string()),
    }
}

/// Reads a `tar` archive of the device-tree directory (ustar, GNU long names, or pax paths).
pub fn from_tar(data: &[u8]) -> Result<Tree> {
    let mut entries: Vec<(String, Option<Vec<u8>>)> = Vec::new();
    let mut pos = 0;
    let mut long_name: Option<String> = None;
    while pos + 512 <= data.len() {
        let h = &data[pos..pos + 512];
        if h.iter().all(|&b| b == 0) {
            break;
        }
        let size = tar_number(&h[124..136])
            .ok_or_else(|| crate::Error("bad size in tar header".into()))?;
        let body_start = pos + 512;
        let body_end = body_start + size;
        if body_end > data.len() {
            bail!("tar archive is truncated");
        }
        let body = &data[body_start..body_end];
        pos = body_start + size.div_ceil(512) * 512;
        let kind = h[156];
        let name = || {
            let mut name = tar_string(&h[0..100]);
            if &h[257..263] == b"ustar\0" {
                let prefix = tar_string(&h[345..500]);
                if !prefix.is_empty() {
                    name = format!("{prefix}/{name}");
                }
            }
            name
        };
        match kind {
            b'L' => long_name = Some(tar_string(body)),
            b'x' => {
                if let Some(path) = pax_path(body) {
                    long_name = Some(path);
                }
            }
            b'g' => {}
            b'0' | 0 | b'5' => {
                let path = long_name.take().unwrap_or_else(name);
                let value = (kind != b'5' && !path.ends_with('/')).then(|| body.to_vec());
                entries.push((path, value));
            }
            _ => long_name = None,
        }
    }
    if entries.is_empty() {
        bail!("the tar archive is empty");
    }
    Ok(from_entries(
        entries.iter().map(|(p, v)| (p.as_str(), v.clone())),
    ))
}

fn tar_string(field: &[u8]) -> String {
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).into_owned()
}

fn tar_number(field: &[u8]) -> Option<usize> {
    if field[0] & 0x80 != 0 {
        // GNU base-256 for large sizes.
        let mut n: usize = (field[0] & 0x7f) as usize;
        for &b in &field[1..] {
            n = n.checked_mul(256)?.checked_add(b as usize)?;
        }
        return Some(n);
    }
    let text = tar_string(field);
    let text = text.trim_matches(|c: char| c == ' ' || c == '\0');
    if text.is_empty() {
        return Some(0);
    }
    usize::from_str_radix(text, 8).ok()
}

/// The `path` record of a pax extended header (`"<len> path=<value>\n"` records).
fn pax_path(body: &[u8]) -> Option<String> {
    let mut rest = body;
    while !rest.is_empty() {
        let space = rest.iter().position(|&b| b == b' ')?;
        let len: usize = std::str::from_utf8(&rest[..space]).ok()?.parse().ok()?;
        let record = rest.get(space + 1..len)?;
        let record = record.strip_suffix(b"\n").unwrap_or(record);
        if let Some(value) = record.strip_prefix(b"path=") {
            return Some(String::from_utf8_lossy(value).into_owned());
        }
        rest = rest.get(len..)?;
    }
    None
}

/// Reads the fallback dump used when the device has no `tar`: `D <path>` for each
/// directory, and `F <path>` followed by `od -An -v -tx1` lines for each file.
pub fn from_od(text: &str) -> Result<Tree> {
    let mut entries: Vec<(String, Option<Vec<u8>>)> = Vec::new();
    for line in text.lines() {
        if let Some(path) = line.strip_prefix("D ") {
            entries.push((path.to_string(), None));
        } else if let Some(path) = line.strip_prefix("F ") {
            entries.push((path.to_string(), Some(Vec::new())));
        } else if !line.trim().is_empty() {
            let Some((_, Some(value))) = entries.last_mut() else {
                bail!("unexpected line in the device's dump: {line}");
            };
            for byte in line.split_whitespace() {
                match u8::from_str_radix(byte, 16) {
                    Ok(b) => value.push(b),
                    Err(_) => bail!("unexpected line in the device's dump: {line}"),
                }
            }
        }
    }
    if entries.is_empty() {
        bail!("the device's dump is empty");
    }
    Ok(from_entries(
        entries.iter().map(|(p, v)| (p.as_str(), v.clone())),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(name: &str, kind: u8, size: usize) -> Vec<u8> {
        let mut h = vec![0u8; 512];
        h[..name.len()].copy_from_slice(name.as_bytes());
        let size = format!("{size:011o}\0");
        h[124..136].copy_from_slice(size.as_bytes());
        h[156] = kind;
        h[257..263].copy_from_slice(b"ustar\0");
        h
    }

    fn tar(entries: &[(&str, Option<&[u8]>)]) -> Vec<u8> {
        let mut out = Vec::new();
        for (name, value) in entries {
            let push_body = |out: &mut Vec<u8>, body: &[u8]| {
                out.extend_from_slice(body);
                out.resize(out.len().div_ceil(512) * 512, 0);
            };
            if name.len() > 100 {
                out.extend(header("././@LongLink", b'L', name.len() + 1));
                push_body(&mut out, format!("{name}\0").as_bytes());
            }
            let short = &name[..name.len().min(100)];
            match value {
                None => out.extend(header(short, b'5', 0)),
                Some(v) => {
                    out.extend(header(short, b'0', v.len()));
                    push_body(&mut out, v);
                }
            }
        }
        out.extend([0u8; 1024]);
        out
    }

    #[test]
    fn tar_round_trip() {
        let long = format!("./{}@1000/compatible", "a".repeat(120));
        let data = tar(&[
            ("./", None),
            ("./name", Some(b"\0")),
            ("./compatible", Some(b"acme,board\0")),
            ("./soc@0/", None),
            ("./soc@0/uart@200/", None),
            ("./soc@0/uart@200/name", Some(b"uart\0")),
            ("./soc@0/uart@200/status", Some(b"okay\0")),
            ("./soc@0/uart@200/reg", Some(&[0, 0, 2, 0])),
            ("./soc@0/uart@100/", None),
            ("./soc@0/uart@100/name", Some(b"serial\0")),
            (&long, Some(b"x\0")),
        ]);
        let t = from_tar(&data).unwrap();
        assert_eq!(t.prop_str(Tree::ROOT, "compatible"), Some("acme,board"));
        assert!(t.prop(Tree::ROOT, "name").is_none());
        let soc = t.find("/soc@0").unwrap();
        let names: Vec<_> = t
            .node(soc)
            .children
            .iter()
            .map(|&c| t.node(c).name.clone())
            .collect();
        assert_eq!(names, ["uart@100", "uart@200"]);
        let uart = t.find("/soc@0/uart@200").unwrap();
        let props: Vec<_> = t.node(uart).props.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(props, ["reg", "status"]);
        // A `name` that differs from the node name is real.
        assert!(t.prop(t.find("/soc@0/uart@100").unwrap(), "name").is_some());
        assert!(t.find(&format!("/{}@1000", "a".repeat(120))).is_some());
    }

    #[test]
    fn od_dump() {
        let text = "D .\nF ./compatible\n 61 63 6d 65 2c 62 00\nD ./cpus\nF ./cpus/#address-cells\n 00 00 00 01\nF ./cpus/empty\n";
        let t = from_od(text).unwrap();
        assert_eq!(t.prop_str(Tree::ROOT, "compatible"), Some("acme,b"));
        assert_eq!(
            t.prop_u32(t.find("/cpus").unwrap(), "#address-cells"),
            Some(1)
        );
        assert_eq!(t.prop(t.find("/cpus").unwrap(), "empty"), Some(&[][..]));
    }
}
