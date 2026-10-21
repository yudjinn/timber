//! The tree being shown plus the indexes the UI needs.

use timber_core::dts::Compiled;
use timber_core::source_map::SourceMap;
use timber_core::{Diagnostic, NodeId, Severity, Tree, dtb, format};

use crate::kind::{Kind, classify};
use crate::search::SearchIndex;
use crate::sources::SourceLink;

#[derive(Debug, Clone)]
pub struct InputFile {
    pub name: String,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// A DTB/DTBO file; `name` is the file name.
    Blob,
    /// A board compiled from the sources; `name` is its path in the source VFS.
    Board,
}

pub struct Document {
    pub name: String,
    pub origin: Origin,
    pub tree: Tree,
    /// Where nodes were defined, for trees compiled from source.
    pub source_map: Option<SourceMap>,
    /// For blobs: the source board they were matched to.
    pub link: Option<SourceLink>,
    /// Nodes whose labels were borrowed from the linked source (blobs built without `-@`).
    pub borrowed_labels: Vec<bool>,
    pub diagnostics: Vec<Diagnostic>,
    /// The tree is an overlay (`/plugin/` source or `__fixups__`/`fragment@` blob).
    pub overlay: bool,
    pub kinds: Vec<Kind>,
    /// For each node, the `(node, property)` pairs that refer to it.
    pub referenced_by: Vec<Vec<(NodeId, String)>>,
    pub search: SearchIndex,
}

impl Document {
    fn new(
        name: String,
        origin: Origin,
        tree: Tree,
        source_map: Option<SourceMap>,
        diagnostics: Vec<Diagnostic>,
        overlay: bool,
    ) -> Self {
        let kinds = (0..tree.len()).map(|id| classify(&tree, id)).collect();
        let referenced_by = format::reverse_references(&tree);
        let search = SearchIndex::new(&tree);
        let borrowed_labels = vec![false; tree.len()];
        Document {
            name,
            origin,
            tree,
            source_map,
            link: None,
            borrowed_labels,
            diagnostics,
            overlay,
            kinds,
            referenced_by,
            search,
        }
    }

    pub fn from_blob(name: &str, data: &[u8]) -> Result<Self, String> {
        let tree = dtb::parse(data).map_err(|e| format!("{name}: {e}"))?;
        let overlay = tree.child_by_name(Tree::ROOT, "__fixups__").is_some()
            || tree
                .node(Tree::ROOT)
                .children
                .iter()
                .any(|&c| tree.node(c).name.starts_with("fragment@"));
        Ok(Self::new(
            name.to_string(),
            Origin::Blob,
            tree,
            None,
            Vec::new(),
            overlay,
        ))
    }

    pub fn from_compiled(path: &str, compiled: Compiled) -> Self {
        Self::new(
            path.to_string(),
            Origin::Board,
            compiled.tree,
            Some(compiled.source_map),
            compiled.diagnostics,
            compiled.plugin,
        )
    }

    /// The same blob with a (new) source link; its compile messages become this document's.
    /// Nodes without labels borrow them from the source, so references read `&cru` rather
    /// than `&{/clock-controller@fd7c0000}` and labels are searchable.
    pub fn relinked(&self, link: Option<SourceLink>) -> Self {
        let diagnostics = link
            .as_ref()
            .map(|l| l.diagnostics.clone())
            .unwrap_or_default();
        let mut tree = self.tree.clone();
        for (id, &borrowed) in self.borrowed_labels.iter().enumerate() {
            if borrowed {
                tree.node_mut(id).labels.clear();
            }
        }
        let mut borrowed_labels = vec![false; tree.len()];
        if let Some(l) = &link {
            for (id, borrowed) in borrowed_labels.iter_mut().enumerate() {
                let Some(sid) = l.map[id] else { continue };
                if tree.node(id).labels.is_empty() && !l.tree.node(sid).labels.is_empty() {
                    tree.node_mut(id).labels = l.tree.node(sid).labels.clone();
                    *borrowed = true;
                }
            }
        }
        Document {
            name: self.name.clone(),
            origin: self.origin,
            tree,
            source_map: self.source_map.clone(),
            link,
            borrowed_labels,
            diagnostics,
            overlay: self.overlay,
            kinds: self.kinds.clone(),
            referenced_by: self.referenced_by.clone(),
            search: self.search.clone(),
        }
    }

    /// Where node `id` comes from: the source map and the node's id within it.
    pub fn source_for(&self, id: NodeId) -> Option<(&SourceMap, NodeId)> {
        if let Some(sm) = &self.source_map {
            return Some((sm, id));
        }
        let link = self.link.as_ref()?;
        Some((&link.source_map, link.map.get(id).copied().flatten()?))
    }

    /// Short display name: the file name without directories.
    pub fn short_name(&self) -> &str {
        self.name.rsplit('/').next().unwrap_or(&self.name)
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

/// Opened files split into blobs (to show) and sources (to add to the Sources set).
pub struct Opened {
    pub blobs: Vec<InputFile>,
    pub sources: Vec<InputFile>,
    pub ignored: Vec<String>,
}

pub fn split_opened(files: Vec<InputFile>) -> Opened {
    let mut out = Opened {
        blobs: Vec::new(),
        sources: Vec::new(),
        ignored: Vec::new(),
    };
    for f in files {
        if dtb::is_dtb(&f.data) {
            out.blobs.push(f);
        } else if matches!(
            f.name.rsplit_once('.').map(|(_, e)| e),
            Some("dts" | "dtsi" | "dtso" | "h")
        ) {
            out.sources.push(f);
        } else {
            out.ignored.push(f.name);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_and_loads() {
        let blob = dtb::write(
            &timber_core::dts::compile_str("t.dts", "/dts-v1/;\n/ { model = \"x\"; };\n").tree,
        );
        let files = vec![
            InputFile {
                name: "board.dtb".into(),
                data: blob.clone(),
            },
            InputFile {
                name: "board.dts".into(),
                data: b"/dts-v1/;\n/ {};\n".to_vec(),
            },
            InputFile {
                name: "notes.txt".into(),
                data: b"hi".to_vec(),
            },
        ];
        let opened = split_opened(files);
        assert_eq!((opened.blobs.len(), opened.sources.len()), (1, 1));
        assert_eq!(opened.ignored, ["notes.txt"]);
        let doc = Document::from_blob("board.dtb", &blob).unwrap();
        assert_eq!(doc.tree.prop_str(Tree::ROOT, "model"), Some("x"));
        assert!(doc.source_for(Tree::ROOT).is_none());
        assert!(Document::from_blob("bad.dtb", b"nope").is_err());
    }
}
