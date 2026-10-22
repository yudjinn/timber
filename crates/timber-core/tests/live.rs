//! Reading a device's tree back from real `tar` and `od` output: the fixture board is written
//! out the way the kernel lays out `/proc/device-tree`, dumped with the system's tools, and
//! read back.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use timber_core::vfs::MemVfs;
use timber_core::{NodeId, Tree, dts, live};

fn fixture_tree() -> Tree {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut vfs = MemVfs::default();
    for f in ["board.dts", "soc.dtsi", "include/dt-bindings/soc.h"] {
        vfs.insert(f, std::fs::read(dir.join(f)).unwrap());
    }
    let compiled = dts::compile(&vfs, "board.dts");
    assert!(!compiled.has_errors(), "{:?}", compiled.diagnostics);
    compiled.tree
}

/// Writes `tree` out like `/proc/device-tree`, including the kernel's `name` properties.
fn write_dir(tree: &Tree, id: NodeId, dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    let node = tree.node(id);
    let mut name = node.base_name().as_bytes().to_vec();
    name.push(0);
    if id == Tree::ROOT {
        name = vec![0];
    }
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
        out.insert(format!("{path} (node)"), Vec::new());
        for p in &tree.node(id).props {
            out.insert(format!("{path}:{}", p.name), p.value.clone());
        }
    }
    out
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("timber-live-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn tar_and_od_round_trip() {
    let tree = fixture_tree();
    let dir = scratch("dt");
    write_dir(&tree, Tree::ROOT, &dir);
    let want = flat(&tree);

    let tar = Command::new("tar")
        .arg("-C")
        .arg(&dir)
        .args(["-cf", "-", "."])
        .output();
    match tar {
        Ok(out) if out.status.success() => {
            let got = live::from_tar(&out.stdout).unwrap();
            assert_eq!(flat(&got), want, "tar");
        }
        _ => eprintln!("skipping tar: not available"),
    }

    // The same script the desktop app runs when the device has no `tar`.
    let od = Command::new("sh")
        .arg("-c")
        .arg(r#"cd "$1" && find . | while IFS= read -r f; do if [ -d "$f" ]; then echo "D $f"; else echo "F $f"; od -An -v -tx1 "$f"; fi; done"#)
        .arg("sh")
        .arg(&dir)
        .output()
        .unwrap();
    assert!(od.status.success());
    let got = live::from_od(&String::from_utf8(od.stdout).unwrap()).unwrap();
    assert_eq!(flat(&got), want, "od");
    let _ = std::fs::remove_dir_all(&dir);
}
