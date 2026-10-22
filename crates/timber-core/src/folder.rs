//! Which files of a source folder matter for device tree builds, so folder walks (browser
//! or native) can skip the rest of a kernel tree.

/// Source-like files: everything the DTS compiler can read.
pub fn is_source_file(path: &str) -> bool {
    matches!(
        path.rsplit_once('.').map(|(_, e)| e),
        Some("dts" | "dtsi" | "dtso" | "h")
    )
}

/// Kernel roots in a folder listing: the prefix before `arch/<arch>/boot/dts/`.
fn kernel_roots<'a>(paths: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut roots: Vec<String> = Vec::new();
    for p in paths {
        if let Some(i) = p.find("arch/") {
            let rest = &p[i + 5..];
            let is_dts = rest
                .split_once('/')
                .is_some_and(|(_, tail)| tail.starts_with("boot/dts/"));
            let at_boundary = i == 0 || p.as_bytes()[i - 1] == b'/';
            if is_dts && at_boundary && !roots.iter().any(|r| r == &p[..i]) {
                roots.push(p[..i].to_string());
            }
        }
    }
    roots
}

/// Whether a directory inside a kernel tree (path relative to the kernel root) can hold
/// files dts builds use, so a folder walk can skip everything else.
pub fn kernel_dir_wanted(rel: &str) -> bool {
    let parts: Vec<&str> = rel.split('/').collect();
    matches!(
        parts.as_slice(),
        ["arch"]
            | ["arch", _]
            | ["arch", _, "boot"]
            | ["arch", _, "boot", "dts", ..]
            | ["include"]
            | ["include", "dt-bindings", ..]
            | ["include", "uapi", ..]
    )
}

/// Whether a directory listing (`(is_dir, name)`) looks like the top of a kernel tree.
pub fn is_kernel_root(children: &[(bool, &str)]) -> bool {
    ["arch", "include", "scripts"]
        .iter()
        .all(|d| children.contains(&(true, d)))
}

/// Which files of a folder listing to read. In a kernel tree only the parts dts builds use;
/// elsewhere every source-like file.
pub fn folder_filter(paths: &[String]) -> impl Fn(&str) -> bool + 'static {
    let roots = kernel_roots(paths.iter().map(String::as_str));
    move |path: &str| {
        if !is_source_file(path) || path.contains("/.git/") {
            return false;
        }
        match roots.iter().find(|r| path.starts_with(r.as_str())) {
            Some(root) => {
                let rest = &path[root.len()..];
                (rest.starts_with("arch/") && rest.contains("/boot/dts/"))
                    || rest.starts_with("include/dt-bindings/")
                    || rest.starts_with("include/uapi/")
            }
            None => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernel_folders_read_only_dts_parts() {
        let paths: Vec<String> = [
            "linux/arch/arm64/boot/dts/rockchip/a.dts",
            "linux/arch/arm64/kernel/head.h",
            "linux/include/dt-bindings/gpio/gpio.h",
            "linux/include/linux/kernel.h",
            "linux/drivers/gpio/x.h",
            "bsp/board/custom.dtsi",
            "bsp/README",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let want = folder_filter(&paths);
        let kept: Vec<&str> = paths
            .iter()
            .map(String::as_str)
            .filter(|p| want(p))
            .collect();
        assert_eq!(
            kept,
            [
                "linux/arch/arm64/boot/dts/rockchip/a.dts",
                "linux/include/dt-bindings/gpio/gpio.h",
                "bsp/board/custom.dtsi"
            ]
        );
    }

    #[test]
    fn kernel_walk_pruning() {
        assert!(is_kernel_root(&[
            (true, "arch"),
            (true, "include"),
            (true, "scripts"),
            (false, "Kconfig")
        ]));
        assert!(!is_kernel_root(&[(true, "arch"), (true, "include")]));
        for wanted in [
            "arch",
            "arch/arm64",
            "arch/arm64/boot",
            "arch/arm64/boot/dts/rockchip",
            "include",
            "include/dt-bindings/gpio",
            "include/uapi/linux",
        ] {
            assert!(kernel_dir_wanted(wanted), "{wanted}");
        }
        for skipped in [
            "drivers",
            "arch/arm64/kernel",
            "include/linux",
            "Documentation",
            "arch/arm64/boot/install",
        ] {
            assert!(!kernel_dir_wanted(skipped), "{skipped}");
        }
    }
}
