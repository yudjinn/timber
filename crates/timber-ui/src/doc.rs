//! A loaded device tree plus the indexes the UI needs.

use timber_core::source_map::SourceMap;
use timber_core::vfs::MemVfs;
use timber_core::{Diagnostic, NodeId, Severity, Tree, dtb, dts, format};

use crate::kind::{Kind, classify};
use crate::search::SearchIndex;

#[derive(Debug, Clone)]
pub struct InputFile {
    pub name: String,
    pub data: Vec<u8>,
}

pub struct Document {
    /// What was opened, for display (a file name).
    pub name: String,
    pub tree: Tree,
    pub source_map: Option<SourceMap>,
    pub diagnostics: Vec<Diagnostic>,
    /// The tree is an overlay (`/plugin/` source or `__fixups__`/`fragment@` blob).
    pub overlay: bool,
    pub kinds: Vec<Kind>,
    /// For each node, the `(node, property)` pairs that refer to it.
    pub referenced_by: Vec<Vec<(NodeId, String)>>,
    pub search: SearchIndex,
}

impl Document {
    pub fn new(
        name: String,
        tree: Tree,
        source_map: Option<SourceMap>,
        diagnostics: Vec<Diagnostic>,
        overlay: bool,
    ) -> Self {
        let kinds = (0..tree.len()).map(|id| classify(&tree, id)).collect();
        let referenced_by = format::reverse_references(&tree);
        let search = SearchIndex::new(&tree);
        Document {
            name,
            tree,
            source_map,
            diagnostics,
            overlay,
            kinds,
            referenced_by,
            search,
        }
    }

    pub fn error_count(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count()
    }

    pub fn warning_count(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Warning)
            .count()
    }
}

pub enum Loaded {
    Doc(Box<Document>),
    /// Several files could be the main one; ask which.
    Choose(Vec<String>),
}

fn extension(name: &str) -> &str {
    name.rsplit_once('.').map_or("", |(_, e)| e)
}

/// Loads opened files: a DTB, or a DTS compiled with the other files available as includes.
/// `main` picks the file when several qualify.
pub fn load(files: &[InputFile], main: Option<&str>) -> Result<Loaded, String> {
    let is_blob = |f: &InputFile| dtb::is_dtb(&f.data);
    let is_source =
        |f: &InputFile| matches!(extension(&f.name), "dts" | "dtso") && !dtb::is_dtb(&f.data);
    let candidates: Vec<&InputFile> = files
        .iter()
        .filter(|f| is_blob(f) || is_source(f))
        .collect();

    let chosen = match main {
        Some(name) => files
            .iter()
            .find(|f| f.name == name)
            .ok_or_else(|| format!("{name} was not opened"))?,
        None => match candidates.as_slice() {
            [one] => one,
            [] => match files {
                // A lone .dtsi still compiles on its own.
                [one] if extension(&one.name) == "dtsi" => one,
                _ => return Err("Open a .dtb/.dtbo blob or a .dts/.dtso source.".into()),
            },
            many => {
                return Ok(Loaded::Choose(
                    many.iter().map(|f| f.name.clone()).collect(),
                ));
            }
        },
    };

    if is_blob(chosen) {
        let tree = dtb::parse(&chosen.data).map_err(|e| format!("{}: {e}", chosen.name))?;
        let overlay = tree.child_by_name(Tree::ROOT, "__fixups__").is_some()
            || tree
                .node(Tree::ROOT)
                .children
                .iter()
                .any(|&c| tree.node(c).name.starts_with("fragment@"));
        let doc = Document::new(chosen.name.clone(), tree, None, Vec::new(), overlay);
        return Ok(Loaded::Doc(Box::new(doc)));
    }

    let mut vfs = MemVfs::new();
    for f in files.iter().filter(|f| !dtb::is_dtb(&f.data)) {
        vfs.insert(&f.name, f.data.clone());
    }
    let main_path = timber_core::vfs::normalize(&chosen.name);
    let compiled = dts::compile(&vfs, &main_path);
    let doc = Document::new(
        chosen.name.clone(),
        compiled.tree,
        Some(compiled.source_map),
        compiled.diagnostics,
        compiled.plugin,
    );
    Ok(Loaded::Doc(Box::new(doc)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, data: &[u8]) -> InputFile {
        InputFile {
            name: name.into(),
            data: data.to_vec(),
        }
    }

    #[test]
    fn picks_the_main_file() {
        let board = file(
            "board.dts",
            b"/dts-v1/;\n#include \"soc.dtsi\"\n/ { model = \"x\"; };\n",
        );
        let soc = file("soc.dtsi", b"/ { soc {}; };\n");
        let Ok(Loaded::Doc(doc)) = load(&[board.clone(), soc.clone()], None) else {
            panic!()
        };
        assert!(doc.tree.find("/soc").is_some());
        assert_eq!(doc.error_count(), 0);

        let other = file("other.dts", b"/dts-v1/;\n/ {};\n");
        let Ok(Loaded::Choose(names)) = load(&[board.clone(), soc, other], None) else {
            panic!()
        };
        assert_eq!(names, ["board.dts", "other.dts"]);

        let blob = dtb::write(&doc.tree);
        let Ok(Loaded::Doc(doc)) = load(&[file("board.dtb", &blob)], None) else {
            panic!()
        };
        assert!(doc.source_map.is_none());
        assert!(load(&[file("readme.txt", b"hi")], None).is_err());
    }
}
