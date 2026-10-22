//! Native file access: reading opened files and source folders, packed into one binary
//! message for the UI (JSON number arrays would be several times larger for kernel trees).

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use timber_core::folder::{folder_filter, is_kernel_root, kernel_dir_wanted};

/// A batch of files for the UI. Layout (little-endian):
/// `"TMBR"`, `u32` version, `u32` handle length, handle (UTF-8 folder path or empty),
/// `u32` file count, then per file `u32` name length, name, `u32` data length, data.
pub fn pack(handle: &str, files: &[(String, Vec<u8>)]) -> Vec<u8> {
    fn put(out: &mut Vec<u8>, bytes: &[u8]) {
        out.extend((bytes.len() as u32).to_le_bytes());
        out.extend(bytes);
    }
    let size: usize = files.iter().map(|(n, d)| 8 + n.len() + d.len()).sum();
    let mut out = Vec::with_capacity(16 + handle.len() + size);
    out.extend(b"TMBR");
    out.extend(1u32.to_le_bytes());
    put(&mut out, handle.as_bytes());
    out.extend((files.len() as u32).to_le_bytes());
    for (name, data) in files {
        put(&mut out, name.as_bytes());
        put(&mut out, data);
    }
    out
}

/// Reads individual files; names are their absolute paths (the UI keeps them so it can open
/// sources in an editor).
pub fn read_files(paths: &[PathBuf]) -> Result<Vec<(String, Vec<u8>)>, String> {
    paths
        .iter()
        .map(|p| {
            let name = std::fs::canonicalize(p)
                .unwrap_or_else(|_| p.clone())
                .to_string_lossy()
                .into_owned();
            fs::read(p)
                .map(|d| (name, d))
                .map_err(|e| format!("{}: {e}", p.display()))
        })
        .collect()
}

/// Lists a folder the way the web build's directory walk does: hidden directories are
/// skipped, and inside a kernel tree only the parts dts builds use are entered. Paths are
/// `/`-separated and start with the folder's own name.
pub fn walk(root: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    let root_name = root.file_name().map_or_else(
        || "folder".to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let mut stack: Vec<(PathBuf, String, Option<String>)> =
        vec![(root.to_path_buf(), root_name, None)];
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut files = Vec::new();
    while let Some((dir, rel, kernel)) = stack.pop() {
        // Directory symlinks are followed, but each real directory is walked once.
        if !seen.insert(fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone())) {
            continue;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let children: Vec<(bool, String, PathBuf)> = entries
            .flatten()
            .filter_map(|e| {
                let path = e.path();
                // `metadata` follows symlinks: linked headers read as files.
                let meta = fs::metadata(&path).ok()?;
                Some((
                    meta.is_dir(),
                    e.file_name().to_string_lossy().into_owned(),
                    path,
                ))
            })
            .collect();
        let listing: Vec<(bool, &str)> =
            children.iter().map(|(d, n, _)| (*d, n.as_str())).collect();
        let kernel = kernel.or_else(|| is_kernel_root(&listing).then(String::new));
        for (is_dir, name, path) in children {
            let child_rel = format!("{rel}/{name}");
            if !is_dir {
                files.push((child_rel, path));
                continue;
            }
            if name.starts_with('.') {
                continue;
            }
            let child_kernel = kernel.as_ref().map(|k| {
                if k.is_empty() {
                    name.clone()
                } else {
                    format!("{k}/{name}")
                }
            });
            if child_kernel
                .as_deref()
                .is_some_and(|k| !kernel_dir_wanted(k))
            {
                continue;
            }
            stack.push((path, child_rel, child_kernel));
        }
    }
    if files.is_empty() && !root.is_dir() {
        return Err(format!("{} is not a folder", root.display()));
    }
    Ok(files)
}

/// Reads the source files of a folder.
pub fn read_folder(root: &Path) -> Result<Vec<(String, Vec<u8>)>, String> {
    let listing = walk(root)?;
    let paths: Vec<String> = listing.iter().map(|(p, _)| p.clone()).collect();
    let keep = folder_filter(&paths);
    listing
        .into_iter()
        .filter(|(p, _)| keep(p))
        .map(|(rel, path)| {
            fs::read(&path)
                .map(|d| (rel, d))
                .map_err(|e| format!("{}: {e}", path.display()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walks_a_kernel_tree_and_packs() {
        let dir = std::env::temp_dir().join(format!("timber-walk-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let k = dir.join("linux");
        for d in [
            "arch/arm64/boot/dts/acme",
            "arch/arm64/kernel",
            "include/dt-bindings/gpio",
            "include/linux",
            "scripts",
            "drivers",
            ".git",
        ] {
            fs::create_dir_all(k.join(d)).unwrap();
        }
        fs::write(
            k.join("arch/arm64/boot/dts/acme/board.dts"),
            "/dts-v1/;\n/ {};\n",
        )
        .unwrap();
        fs::write(k.join("arch/arm64/kernel/x.h"), "").unwrap();
        fs::write(k.join("include/dt-bindings/gpio/gpio.h"), "#define X 1\n").unwrap();
        fs::write(k.join("include/linux/kernel.h"), "").unwrap();
        fs::write(k.join("drivers/d.h"), "").unwrap();
        fs::write(k.join(".git/HEAD"), "").unwrap();

        let mut files = read_folder(&k).unwrap();
        files.sort();
        let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            [
                "linux/arch/arm64/boot/dts/acme/board.dts",
                "linux/include/dt-bindings/gpio/gpio.h"
            ]
        );

        let packed = pack("/x/linux", &files);
        assert_eq!(&packed[..4], b"TMBR");
        assert_eq!(u32::from_le_bytes(packed[8..12].try_into().unwrap()), 8);
        let _ = fs::remove_dir_all(&dir);
    }
}
