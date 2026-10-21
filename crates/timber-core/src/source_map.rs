//! Where each node and property of a compiled tree came from.

use crate::NodeId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocKind {
    /// The first block that created the node or set the property.
    Defined,
    /// A later block (same-name node, `&label { }`) that extended or overrode it.
    Overridden,
    /// A `/delete-node/` or `/delete-property/`.
    Deleted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceLoc {
    /// Index into [`SourceMap::files`].
    pub file: u32,
    pub line: u32,
    pub col: u32,
    pub kind: LocKind,
}

#[derive(Debug, Clone, Default)]
pub struct NodeSources {
    pub node: Vec<SourceLoc>,
    pub props: Vec<(String, Vec<SourceLoc>)>,
}

#[derive(Debug, Clone, Default)]
pub struct SourceMap {
    pub files: Vec<String>,
    /// Indexed by [`NodeId`].
    pub nodes: Vec<NodeSources>,
    /// Paths removed by `/delete-node/` or `/delete-property/` (`node/prop`), with where.
    pub deleted: Vec<(String, SourceLoc)>,
}

impl SourceMap {
    pub fn node(&self, id: NodeId) -> &[SourceLoc] {
        self.nodes.get(id).map_or(&[], |n| n.node.as_slice())
    }

    pub fn prop(&self, id: NodeId, name: &str) -> &[SourceLoc] {
        self.nodes
            .get(id)
            .and_then(|n| n.props.iter().find(|(p, _)| p == name))
            .map_or(&[], |(_, locs)| locs.as_slice())
    }

    pub fn file(&self, loc: &SourceLoc) -> &str {
        self.files.get(loc.file as usize).map_or("", String::as_str)
    }
}
