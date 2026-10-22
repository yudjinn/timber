//! The tree being shown plus the indexes the UI needs.

use timber_core::dts::Compiled;
use timber_core::source_map::SourceMap;
use timber_core::{Diagnostic, NodeId, Severity, Tree, bsp, dtb, format};

use crate::kind::{Kind, classify};
use crate::search::SearchIndex;
use crate::sources::SourceLink;

#[derive(Debug, Clone)]
pub struct InputFile {
    pub name: String,
    pub data: Vec<u8>,
    /// Where the file is on disk, when known (desktop app).
    pub path: Option<String>,
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
    /// How each node compares with the linked source (blobs only).
    pub diff: Option<DiffInfo>,
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
            diff: None,
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
        let diff = link.as_ref().map(|l| DiffInfo::compute(&self.tree, l));
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
            diff,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffMark {
    Same,
    /// This many properties differ, are missing, or are extra compared with the source.
    Differs(usize),
    /// The node has no counterpart in the source.
    NotInSource,
}

/// Per-node comparison of a blob with its linked source.
#[derive(Debug, Clone, PartialEq)]
pub struct DiffInfo {
    pub marks: Vec<DiffMark>,
    /// Marked nodes strictly below each node.
    pub below: Vec<usize>,
    pub differing: usize,
    pub not_in_source: usize,
}

impl DiffInfo {
    pub fn compute(blob: &Tree, link: &SourceLink) -> Self {
        let marks: Vec<DiffMark> = (0..blob.len())
            .map(|id| {
                if bsp::is_generated(blob, id) {
                    return DiffMark::Same;
                }
                match link.map[id] {
                    None => DiffMark::NotInSource,
                    Some(sid) => {
                        let c = bsp::compare_node(blob, id, &link.tree, sid);
                        match c.differs.len() + c.only_a.len() + c.only_b.len() {
                            0 => DiffMark::Same,
                            n => DiffMark::Differs(n),
                        }
                    }
                }
            })
            .collect();
        // Blobs are parsed depth-first, so children have larger ids than their parents.
        let mut below = vec![0usize; blob.len()];
        for id in (1..blob.len()).rev() {
            if let Some(p) = blob.node(id).parent {
                below[p] += below[id] + usize::from(marks[id] != DiffMark::Same);
            }
        }
        let differing = marks
            .iter()
            .filter(|m| matches!(m, DiffMark::Differs(_)))
            .count();
        let not_in_source = marks
            .iter()
            .filter(|m| **m == DiffMark::NotInSource)
            .count();
        DiffInfo {
            marks,
            below,
            differing,
            not_in_source,
        }
    }

    pub fn is_marked(&self, id: NodeId) -> bool {
        self.marks.get(id).is_some_and(|m| *m != DiffMark::Same)
    }

    /// Marked nodes at or below `id`.
    pub fn in_subtree(&self, id: NodeId) -> usize {
        usize::from(self.is_marked(id)) + self.below.get(id).copied().unwrap_or(0)
    }
}

/// Opened files split into blobs (to show) and sources (to add to the Sources set).
pub struct Opened {
    pub blobs: Vec<InputFile>,
    pub sources: Vec<InputFile>,
    /// Kernel configs and Yocto confs.
    pub configs: Vec<InputFile>,
    pub ignored: Vec<String>,
}

pub fn split_opened(files: Vec<InputFile>) -> Opened {
    let mut out = Opened {
        blobs: Vec::new(),
        sources: Vec::new(),
        configs: Vec::new(),
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
        } else if crate::sources::is_config_file(&f.name) {
            out.configs.push(f);
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
    fn diff_marks_and_subtree_counts() {
        use crate::sources::{EntryKind, SourceSet, annotate};
        let src =
            "/dts-v1/;\n/ { compatible = \"acme,b\"; soc { a { x = <1>; }; b { y = <2>; }; }; };\n";
        let (set, _) = SourceSet::default().with_files(
            EntryKind::Folder,
            vec![("bsp/b.dts".into(), src.as_bytes().to_vec())],
        );
        let mut tree = set.compile("bsp/b.dts").tree;
        let a = tree.find("/soc/a").unwrap();
        tree.node_mut(a).set_prop("x", vec![0, 0, 0, 9]);
        let soc = tree.find("/soc").unwrap();
        tree.add_child(soc, "extra");
        let doc = Document::from_blob("b.dtb", &dtb::write(&tree)).unwrap();
        let link = annotate(&doc.tree, "b.dtb", &set, None);
        let doc = doc.relinked(link);
        let diff = doc.diff.as_ref().unwrap();
        let t = &doc.tree;
        assert_eq!(diff.marks[t.find("/soc/a").unwrap()], DiffMark::Differs(1));
        assert_eq!(
            diff.marks[t.find("/soc/extra").unwrap()],
            DiffMark::NotInSource
        );
        assert_eq!(diff.marks[t.find("/soc/b").unwrap()], DiffMark::Same);
        assert_eq!((diff.differing, diff.not_in_source), (1, 1));
        assert_eq!(diff.below[Tree::ROOT], 2);
        assert_eq!(diff.in_subtree(t.find("/soc").unwrap()), 2);
    }

    #[test]
    fn splits_and_loads() {
        let blob = dtb::write(
            &timber_core::dts::compile_str("t.dts", "/dts-v1/;\n/ { model = \"x\"; };\n").tree,
        );
        let files = vec![
            InputFile {
                name: "board.dtb".into(),
                data: blob.clone(),
                path: None,
            },
            InputFile {
                name: "board.dts".into(),
                data: b"/dts-v1/;\n/ {};\n".to_vec(),
                path: None,
            },
            InputFile {
                name: "notes.txt".into(),
                data: b"hi".to_vec(),
                path: None,
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
