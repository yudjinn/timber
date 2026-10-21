//! Virtual filesystem the compiler reads sources through, plus kernel-style include
//! resolution.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Read-only file access. Paths are `/`-separated and relative to the opened folder.
pub trait Vfs {
    fn read(&self, path: &str) -> Option<Cow<'_, [u8]>>;

    fn exists(&self, path: &str) -> bool {
        self.read(path).is_some()
    }

    fn paths(&self) -> Box<dyn Iterator<Item = &str> + '_>;
}

/// An in-memory [`Vfs`].
#[derive(Debug, Clone, Default)]
pub struct MemVfs {
    files: BTreeMap<String, Vec<u8>>,
}

impl MemVfs {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, path: &str, data: impl Into<Vec<u8>>) {
        self.files.insert(normalize(path), data.into());
    }

    pub fn with(mut self, path: &str, data: impl Into<Vec<u8>>) -> Self {
        self.insert(path, data);
        self
    }
}

impl Vfs for MemVfs {
    fn read(&self, path: &str) -> Option<Cow<'_, [u8]>> {
        self.files.get(path).map(|d| Cow::Borrowed(d.as_slice()))
    }

    fn exists(&self, path: &str) -> bool {
        self.files.contains_key(path)
    }

    fn paths(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        Box::new(self.files.keys().map(String::as_str))
    }
}

/// Normalizes a path: `\` → `/`, drops `.` and empty segments, resolves `..`.
pub fn normalize(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for seg in path.split(['/', '\\']) {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

pub fn dirname(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(d, _)| d)
}

pub fn basename(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(_, b)| b)
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        normalize(name)
    } else {
        normalize(&format!("{dir}/{name}"))
    }
}

/// Resolves `#include` names the way the kernel's dtc invocation does (`-I` paths and
/// `scripts/dtc/include-prefixes`), falling back to suffix and basename matches for
/// non-kernel BSP layouts.
pub struct IncludeResolver<'a> {
    vfs: &'a dyn Vfs,
    /// Kernel source roots found in the folder (prefix before `arch/`), with their arches.
    roots: Vec<(String, Vec<String>)>,
    by_basename: HashMap<&'a str, Vec<&'a str>>,
}

impl<'a> IncludeResolver<'a> {
    pub fn new(vfs: &'a dyn Vfs) -> Self {
        let mut roots: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut by_basename: HashMap<&str, Vec<&str>> = HashMap::new();
        for path in vfs.paths() {
            by_basename.entry(basename(path)).or_default().push(path);
            // Look for `<root>arch/<arch>/boot/dts/...`.
            let mut search = 0;
            while let Some(i) = path[search..].find("arch/") {
                let at = search + i;
                if at == 0 || path.as_bytes()[at - 1] == b'/' {
                    let rest = &path[at + 5..];
                    if let Some((arch, tail)) = rest.split_once('/')
                        && tail.starts_with("boot/dts/")
                    {
                        roots
                            .entry(path[..at].to_string())
                            .or_default()
                            .insert(arch.to_string());
                        break;
                    }
                }
                search = at + 5;
            }
        }
        let roots = roots
            .into_iter()
            .map(|(r, a)| (r, a.into_iter().collect()))
            .collect();
        IncludeResolver {
            vfs,
            roots,
            by_basename,
        }
    }

    /// Resolves `name` included from `from`. `quoted` is true for `"..."` includes, which
    /// also search the including file's directory first.
    pub fn resolve(&self, name: &str, from: &str, quoted: bool) -> Option<String> {
        let name = name.replace('\\', "/");
        let try_path = |p: String| self.vfs.exists(&p).then_some(p);

        if quoted && let Some(p) = try_path(join(dirname(from), &name)) {
            return Some(p);
        }
        for (root, arches) in &self.roots {
            if let Some(p) = try_path(join(root, &format!("include/{name}"))) {
                return Some(p);
            }
            for arch in arches {
                if let Some(p) = try_path(join(root, &format!("arch/{arch}/boot/dts/{name}"))) {
                    return Some(p);
                }
                // include-prefixes: `<arch>/...` → arch/<arch>/boot/dts/...
                if let Some(rest) = name.strip_prefix(&format!("{arch}/"))
                    && let Some(p) = try_path(join(root, &format!("arch/{arch}/boot/dts/{rest}")))
                {
                    return Some(p);
                }
            }
            if let Some(p) = try_path(join(root, &format!("scripts/dtc/include-prefixes/{name}"))) {
                return Some(p);
            }
        }
        if let Some(p) = try_path(normalize(&name)) {
            return Some(p);
        }

        // Fallbacks for arbitrary layouts: a path ending in the include name, preferring
        // the candidate that shares the longest directory prefix with the includer.
        let norm = normalize(&name);
        let candidates: Vec<&str> = self
            .by_basename
            .get(basename(&norm))
            .map(|v| {
                v.iter()
                    .copied()
                    .filter(|p| *p == norm || p.ends_with(&format!("/{norm}")))
                    .collect()
            })
            .unwrap_or_default();
        let shared = |p: &str| {
            p.split('/')
                .zip(from.split('/'))
                .take_while(|(a, b)| a == b)
                .count()
        };
        candidates
            .into_iter()
            .max_by_key(|p| (shared(p), std::cmp::Reverse(p.len())))
            .map(str::to_string)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kernel() -> MemVfs {
        MemVfs::new()
            .with("linux/arch/arm64/boot/dts/rockchip/rk3568.dtsi", "")
            .with("linux/arch/arm64/boot/dts/rockchip/rk3568-evb.dts", "")
            .with("linux/arch/arm/boot/dts/rockchip/rk3288.dtsi", "")
            .with("linux/include/dt-bindings/gpio/gpio.h", "")
            .with("linux/drivers/gpio/gpio.h", "")
    }

    #[test]
    fn kernel_include_paths() {
        let vfs = kernel();
        let r = IncludeResolver::new(&vfs);
        let from = "linux/arch/arm64/boot/dts/rockchip/rk3568-evb.dts";
        assert_eq!(
            r.resolve("rk3568.dtsi", from, true).as_deref(),
            Some("linux/arch/arm64/boot/dts/rockchip/rk3568.dtsi")
        );
        assert_eq!(
            r.resolve("dt-bindings/gpio/gpio.h", from, false).as_deref(),
            Some("linux/include/dt-bindings/gpio/gpio.h")
        );
        assert_eq!(
            r.resolve("arm/rockchip/rk3288.dtsi", from, false)
                .as_deref(),
            Some("linux/arch/arm/boot/dts/rockchip/rk3288.dtsi")
        );
        assert_eq!(r.resolve("missing.dtsi", from, true), None);
    }

    #[test]
    fn suffix_fallback_for_bsp_layouts() {
        let vfs = MemVfs::new()
            .with("bsp/dts/board.dts", "")
            .with("bsp/dts/include/dt-bindings/gpio/gpio.h", "")
            .with("other/dt-bindings/gpio/gpio.h", "");
        let r = IncludeResolver::new(&vfs);
        assert_eq!(
            r.resolve("dt-bindings/gpio/gpio.h", "bsp/dts/board.dts", false)
                .as_deref(),
            Some("bsp/dts/include/dt-bindings/gpio/gpio.h")
        );
    }

    #[test]
    fn normalize_paths() {
        assert_eq!(normalize("./a//b/../c"), "a/c");
        assert_eq!(normalize("a\\b"), "a/b");
    }
}
