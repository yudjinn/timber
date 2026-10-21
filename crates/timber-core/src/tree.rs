use std::collections::HashMap;

pub type NodeId = usize;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Property {
    pub name: String,
    pub value: Vec<u8>,
}

#[derive(Clone, Debug, Default)]
pub struct Node {
    /// Full node name including unit address; empty for the root.
    pub name: String,
    pub labels: Vec<String>,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
    pub props: Vec<Property>,
}

impl Node {
    pub fn base_name(&self) -> &str {
        self.name.split('@').next().unwrap_or("")
    }

    pub fn unit_address(&self) -> Option<&str> {
        self.name.split_once('@').map(|(_, addr)| addr)
    }

    pub fn prop(&self, name: &str) -> Option<&[u8]> {
        self.props
            .iter()
            .find(|p| p.name == name)
            .map(|p| p.value.as_slice())
    }

    pub fn has_prop(&self, name: &str) -> bool {
        self.props.iter().any(|p| p.name == name)
    }

    pub fn set_prop(&mut self, name: &str, value: Vec<u8>) {
        match self.props.iter_mut().find(|p| p.name == name) {
            Some(p) => p.value = value,
            None => self.props.push(Property {
                name: name.to_string(),
                value,
            }),
        }
    }
}

/// An arena-allocated device tree. Node 0 is always the root.
#[derive(Clone, Debug)]
pub struct Tree {
    pub nodes: Vec<Node>,
    pub mem_reserve: Vec<(u64, u64)>,
    pub boot_cpuid: u32,
    phandles: HashMap<u32, NodeId>,
}

impl Default for Tree {
    fn default() -> Self {
        Self::new()
    }
}

impl Tree {
    pub const ROOT: NodeId = 0;

    pub fn new() -> Self {
        Tree {
            nodes: vec![Node::default()],
            mem_reserve: Vec::new(),
            boot_cpuid: 0,
            phandles: HashMap::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        false
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id]
    }

    pub fn node_mut(&mut self, id: NodeId) -> &mut Node {
        &mut self.nodes[id]
    }

    pub fn add_child(&mut self, parent: NodeId, name: impl Into<String>) -> NodeId {
        let id = self.nodes.len();
        self.nodes.push(Node {
            name: name.into(),
            parent: Some(parent),
            ..Node::default()
        });
        self.nodes[parent].children.push(id);
        id
    }

    /// Finds a child by exact name, or by base name when `name` has no unit address and
    /// the match is unambiguous.
    pub fn child_by_name(&self, parent: NodeId, name: &str) -> Option<NodeId> {
        let children = &self.nodes[parent].children;
        if let Some(&id) = children.iter().find(|&&c| self.nodes[c].name == name) {
            return Some(id);
        }
        if name.contains('@') {
            return None;
        }
        let mut matches = children
            .iter()
            .filter(|&&c| self.nodes[c].base_name() == name);
        match (matches.next(), matches.next()) {
            (Some(&id), None) => Some(id),
            _ => None,
        }
    }

    pub fn path(&self, id: NodeId) -> String {
        if id == Self::ROOT {
            return "/".to_string();
        }
        let mut path = String::new();
        for a in self.ancestors(id).into_iter().skip(1) {
            path.push('/');
            path.push_str(&self.nodes[a].name);
        }
        path
    }

    /// The chain from the root down to and including `id`.
    pub fn ancestors(&self, id: NodeId) -> Vec<NodeId> {
        let mut chain = vec![id];
        let mut cur = id;
        while let Some(p) = self.nodes[cur].parent {
            chain.push(p);
            cur = p;
        }
        chain.reverse();
        chain
    }

    pub fn depth(&self, id: NodeId) -> usize {
        self.ancestors(id).len() - 1
    }

    pub fn is_ancestor(&self, ancestor: NodeId, id: NodeId) -> bool {
        let mut cur = Some(id);
        while let Some(c) = cur {
            if c == ancestor {
                return true;
            }
            cur = self.nodes[c].parent;
        }
        false
    }

    /// Resolves an absolute path, or a path starting with an alias from `/aliases`.
    pub fn find(&self, path: &str) -> Option<NodeId> {
        let (mut cur, rest) = if let Some(rest) = path.strip_prefix('/') {
            (Self::ROOT, rest)
        } else {
            let (alias, rest) = path.split_once('/').unwrap_or((path, ""));
            let aliases = self.child_by_name(Self::ROOT, "aliases")?;
            let target = self.prop_str(aliases, alias)?;
            if !target.starts_with('/') {
                return None;
            }
            (self.find(target)?, rest)
        };
        for part in rest.split('/').filter(|p| !p.is_empty()) {
            cur = self.child_by_name(cur, part)?;
        }
        Some(cur)
    }

    pub fn prop(&self, id: NodeId, name: &str) -> Option<&[u8]> {
        self.nodes[id].prop(name)
    }

    pub fn prop_u32(&self, id: NodeId, name: &str) -> Option<u32> {
        let v = self.prop(id, name)?;
        (v.len() == 4).then(|| u32::from_be_bytes([v[0], v[1], v[2], v[3]]))
    }

    /// The first string of a string property.
    pub fn prop_str(&self, id: NodeId, name: &str) -> Option<&str> {
        let v = self.prop(id, name)?;
        let end = v.iter().position(|&b| b == 0).unwrap_or(v.len());
        std::str::from_utf8(&v[..end]).ok()
    }

    pub fn prop_strings(&self, id: NodeId, name: &str) -> Vec<&str> {
        let Some(v) = self.prop(id, name) else {
            return Vec::new();
        };
        v.split(|&b| b == 0)
            .filter(|s| !s.is_empty())
            .filter_map(|s| std::str::from_utf8(s).ok())
            .collect()
    }

    /// `#address-cells` that applies to children of `id`.
    pub fn address_cells(&self, id: NodeId) -> u32 {
        self.prop_u32(id, "#address-cells").unwrap_or(2)
    }

    /// `#size-cells` that applies to children of `id`.
    pub fn size_cells(&self, id: NodeId) -> u32 {
        self.prop_u32(id, "#size-cells").unwrap_or(1)
    }

    pub fn is_enabled(&self, id: NodeId) -> bool {
        matches!(self.prop_str(id, "status"), None | Some("okay" | "ok"))
    }

    pub fn phandle(&self, id: NodeId) -> Option<u32> {
        self.prop_u32(id, "phandle")
            .or_else(|| self.prop_u32(id, "linux,phandle"))
    }

    pub fn by_phandle(&self, phandle: u32) -> Option<NodeId> {
        self.phandles.get(&phandle).copied()
    }

    pub fn subtree_size(&self, id: NodeId) -> usize {
        1 + self.nodes[id]
            .children
            .iter()
            .map(|&c| self.subtree_size(c))
            .sum::<usize>()
    }

    /// Removes the given nodes with their subtrees, compacting ids. Returns the mapping
    /// from old to new ids (`None` for removed nodes). Call [`Tree::finalize`] afterwards.
    pub fn remove_subtrees(
        &mut self,
        remove: &std::collections::HashSet<NodeId>,
    ) -> Vec<Option<NodeId>> {
        fn copy(
            old: &mut [Node],
            new: &mut Tree,
            old_id: NodeId,
            new_id: NodeId,
            remove: &std::collections::HashSet<NodeId>,
            map: &mut [Option<NodeId>],
        ) {
            let children = std::mem::take(&mut old[old_id].children);
            for c in children {
                if remove.contains(&c) {
                    continue;
                }
                let id = new.add_child(new_id, std::mem::take(&mut old[c].name));
                new.nodes[id].labels = std::mem::take(&mut old[c].labels);
                new.nodes[id].props = std::mem::take(&mut old[c].props);
                map[c] = Some(id);
                copy(old, new, c, id, remove, map);
            }
        }
        let mut old = std::mem::take(&mut self.nodes);
        let mut map = vec![None; old.len()];
        map[Self::ROOT] = Some(Self::ROOT);
        let mut new = Tree::new();
        new.nodes[Self::ROOT].labels = std::mem::take(&mut old[Self::ROOT].labels);
        new.nodes[Self::ROOT].props = std::mem::take(&mut old[Self::ROOT].props);
        copy(&mut old, &mut new, Self::ROOT, Self::ROOT, remove, &mut map);
        self.nodes = new.nodes;
        map
    }

    /// Rebuilds derived indexes. Call after constructing or mutating a tree.
    pub fn finalize(&mut self) {
        self.phandles.clear();
        for id in 0..self.nodes.len() {
            if let Some(ph) = self.phandle(id)
                && ph != 0
                && ph != u32::MAX
            {
                self.phandles.insert(ph, id);
            }
        }
        // Recover labels from __symbols__ (DTBs built with -@) for nodes that have none.
        if let Some(symbols) = self.child_by_name(Self::ROOT, "__symbols__") {
            let entries: Vec<(String, String)> = self.nodes[symbols]
                .props
                .iter()
                .filter_map(|p| {
                    let end = p
                        .value
                        .iter()
                        .position(|&b| b == 0)
                        .unwrap_or(p.value.len());
                    let path = std::str::from_utf8(&p.value[..end]).ok()?;
                    Some((p.name.clone(), path.to_string()))
                })
                .collect();
            for (label, path) in entries {
                if let Some(id) = self.find(&path)
                    && !self.nodes[id].labels.contains(&label)
                {
                    self.nodes[id].labels.push(label);
                }
            }
        }
    }
}
