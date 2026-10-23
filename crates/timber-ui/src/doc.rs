//! The tree being shown plus the indexes the UI needs.

use timber_core::diff::{self, Mark, TreeDiff};
use timber_core::dts::Compiled;
use timber_core::source_map::SourceMap;
use timber_core::{Diagnostic, NodeId, Severity, Tree, dtb, format};

use crate::kind::{Kind, classify};
use crate::overlays::{Overlaid, OverlayMark};
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
    /// How the tree compares with another (D-032): its linked source by default. When
    /// set, `tree` also holds ghosts for nodes only in the other tree (D-033).
    pub diff: Option<Comparison>,
    /// What the overlays applied to this tree set or added.
    pub overlay_marks: Vec<OverlayMark>,
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
            overlay_marks: Vec::new(),
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

    /// Replaces the tree with one that has overlays applied (before any source link).
    pub fn with_overlays(self, overlaid: Overlaid) -> Self {
        let mut diagnostics = self.diagnostics;
        diagnostics.extend(overlaid.diagnostics);
        let mut doc = Self::new(
            self.name,
            self.origin,
            overlaid.tree,
            self.source_map,
            diagnostics,
            self.overlay,
        );
        doc.overlay_marks = overlaid.marks;
        doc
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
        // Overlay messages stay; the previous link's compile messages are replaced.
        let mut diagnostics: Vec<Diagnostic> = self
            .diagnostics
            .iter()
            .filter(|d| d.line == 0 && d.col == 0)
            .cloned()
            .collect();
        diagnostics.extend(
            link.as_ref()
                .map(|l| l.diagnostics.clone())
                .unwrap_or_default(),
        );
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
            diff: None,
            overlay_marks: self.overlay_marks.clone(),
            diagnostics,
            overlay: self.overlay,
            kinds: self.kinds.clone(),
            referenced_by: self.referenced_by.clone(),
            search: self.search.clone(),
        }
    }

    /// Compares the tree with `b` (`map`: this tree's nodes → `b`'s, by path if `None`),
    /// grafting ghosts for `b`'s extra nodes.
    pub fn with_comparison(
        mut self,
        against: &str,
        from_source: bool,
        b: Tree,
        map: Option<Vec<Option<NodeId>>>,
    ) -> Self {
        let mut d = diff::compare(&self.tree, &b, map);
        self.tree = std::mem::replace(&mut d.tree, Tree::new());
        for bid in &d.map[d.a_len..] {
            self.kinds
                .push(bid.map_or(Kind::Device, |bid| classify(&b, bid)));
        }
        self.borrowed_labels.resize(self.tree.len(), false);
        self.referenced_by.resize(self.tree.len(), Vec::new());
        self.search = SearchIndex::new(&self.tree);
        self.diff = Some(Comparison {
            against: against.to_string(),
            from_source,
            b,
            d,
        });
        self
    }

    /// Nodes in the tree itself, not counting ghosts.
    pub fn node_count(&self) -> usize {
        self.diff.as_ref().map_or(self.tree.len(), |d| d.a_len)
    }

    pub fn is_ghost(&self, id: NodeId) -> bool {
        self.diff.as_ref().is_some_and(|d| d.is_ghost(id))
    }

    /// The tree without ghosts, for export.
    pub fn own_tree(&self) -> Tree {
        let Some(d) = &self.diff else {
            return self.tree.clone();
        };
        let ghosts = (d.a_len..self.tree.len()).collect();
        let mut tree = self.tree.clone();
        tree.remove_subtrees(&ghosts);
        tree.finalize();
        tree
    }

    /// Where node `id` comes from: the source map and the node's id within it.
    pub fn source_for(&self, id: NodeId) -> Option<(&SourceMap, NodeId)> {
        if self.is_ghost(id) {
            return None;
        }
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

/// The tree shown compared with another (A with B).
#[derive(Debug, Clone)]
pub struct Comparison {
    /// B's name, for messages ("rk3588-rock-5b.dts", "rock5b · snapshot 12:01:33").
    pub against: String,
    /// B is the linked source board (the default comparison).
    pub from_source: bool,
    pub b: Tree,
    /// Marks, counts and the node map; its `tree` was moved into the document.
    pub d: TreeDiff,
}

impl std::ops::Deref for Comparison {
    type Target = TreeDiff;
    fn deref(&self) -> &TreeDiff {
        &self.d
    }
}

impl Comparison {
    /// How B is named in the UI: "source" or its name.
    pub fn b_name(&self) -> &str {
        if self.from_source {
            "source"
        } else {
            &self.against
        }
    }

    /// "3 changed · 1 new · 2 gone", or "no differences".
    pub fn summary(&self) -> String {
        if self.total() == 0 {
            return "no differences".into();
        }
        let mut parts = Vec::new();
        for (n, what) in [
            (self.changed, "changed"),
            (self.only_a, "new"),
            (self.only_b, "gone"),
        ] {
            if n > 0 {
                parts.push(format!("{n} {what}"));
            }
        }
        parts.join(" · ")
    }

    /// The counterpart of `id` in B.
    pub fn b_node(&self, id: NodeId) -> Option<NodeId> {
        self.map.get(id).copied().flatten()
    }

    /// A short marker for the popup and web: `Δ2`, `new`, `gone`.
    pub fn pill(&self, id: NodeId) -> Option<String> {
        match self.marks.get(id)? {
            Mark::Changed(k) => Some(format!("Δ{k}")),
            Mark::OnlyA => Some("new".into()),
            Mark::OnlyB => Some("gone".into()),
            Mark::Same => None,
        }
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
    fn compared_with_source_and_ghosts() {
        use crate::sources::{EntryKind, SourceSet, annotate};
        let src = "/dts-v1/;\n/ { compatible = \"acme,b\"; soc { a { x = <1>; }; b { y = <2>; }; c { }; }; };\n";
        let (set, _) = SourceSet::default().with_files(
            EntryKind::Folder,
            vec![("bsp/b.dts".into(), src.as_bytes().to_vec())],
        );
        let mut tree = set.compile_as("bsp/b.dts", false).tree;
        let a = tree.find("/soc/a").unwrap();
        tree.node_mut(a).set_prop("x", vec![0, 0, 0, 9]);
        let soc = tree.find("/soc").unwrap();
        tree.add_child(soc, "extra");
        let c = tree.find("/soc/c").unwrap();
        let mut tree = {
            let mut t = tree;
            t.remove_subtrees(&[c].into_iter().collect());
            t
        };
        tree.finalize();
        let doc = Document::from_blob("b.dtb", &dtb::write(&tree)).unwrap();
        let own = doc.tree.len();
        let link = annotate(&doc.tree, "b.dtb", &set, None, &[]).unwrap();
        let (b, map) = (link.tree.clone(), link.map.clone());
        let doc = doc
            .relinked(Some(link))
            .with_comparison("b.dts", true, b, Some(map));
        let diff = doc.diff.as_ref().unwrap();
        let t = &doc.tree;
        assert_eq!(diff.marks[t.find("/soc/a").unwrap()], Mark::Changed(1));
        assert_eq!(diff.marks[t.find("/soc/extra").unwrap()], Mark::OnlyA);
        assert_eq!(diff.marks[t.find("/soc/b").unwrap()], Mark::Same);
        let ghost = t.find("/soc/c").unwrap();
        assert!(doc.is_ghost(ghost) && diff.pill(ghost).as_deref() == Some("gone"));
        assert_eq!((diff.changed, diff.only_a, diff.only_b), (1, 1, 1));
        assert_eq!(diff.in_subtree(t.find("/soc").unwrap()), 3);
        assert_eq!(doc.node_count(), own);
        assert_eq!(doc.kinds.len(), t.len());
        assert!(doc.source_for(ghost).is_none());
        assert!(doc.own_tree().find("/soc/c").is_none());
        assert_eq!(diff.b_name(), "source");
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
