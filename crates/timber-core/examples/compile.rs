//! Dev tool: compile a DTS within a folder and report timing and diagnostics.
//!
//! `cargo run --release -p timber-core --example compile -- <folder> <path/in/folder.dts> [--dts]`

use std::path::Path;
use std::time::Instant;

use timber_core::{dts, format, vfs::MemVfs};

fn load(dir: &Path, root: &Path, vfs: &mut MemVfs) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        let is_link = entry.file_type().is_ok_and(|t| t.is_symlink());
        if path.is_dir() {
            if !is_link {
                load(&path, root, vfs);
            }
        } else if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("dts" | "dtsi" | "dtso" | "h")
        ) {
            let rel = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            vfs.insert(&rel, std::fs::read(&path).unwrap());
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let [_, folder, main, rest @ ..] = args.as_slice() else {
        eprintln!("usage: compile <folder> <path/in/folder.dts> [--dts]");
        std::process::exit(2);
    };
    let root = Path::new(folder);
    let t = Instant::now();
    let mut vfs = MemVfs::new();
    // A kernel tree: just the parts dts builds read. Anything else: the whole folder.
    if root.join("arch").is_dir() && root.join("include").is_dir() {
        load(&root.join("arch"), root, &mut vfs);
        load(&root.join("include"), root, &mut vfs);
    } else {
        load(root, root, &mut vfs);
    }
    let loaded = t.elapsed();
    let t = Instant::now();
    let c = dts::compile(&vfs, main);
    let compiled = t.elapsed();
    for d in &c.diagnostics {
        eprintln!("{d}");
    }
    eprintln!(
        "load {loaded:?}, compile {compiled:?}: {} nodes, {} input files",
        c.tree.len(),
        c.inputs.len()
    );
    if rest.iter().any(|a| a == "--dts") {
        print!("{}", format::to_dts(&c.tree));
    }
}
