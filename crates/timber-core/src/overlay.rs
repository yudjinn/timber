//! Applying device tree overlays to a base tree, following libfdt's `fdt_overlay_apply`
//! (what U-Boot and `fdtoverlay` do), so results match byte for byte.

use crate::{NodeId, Property, Tree};

/// The result of applying an overlay.
pub struct Applied {
    pub tree: Tree,
    /// What the overlay changed: `(node in the result, property or None for a node it
    /// created or merged into, the overlay node it came from)`.
    pub touched: Vec<(NodeId, Option<String>, NodeId)>,
}

fn be32(v: &[u8], off: usize) -> Option<u32> {
    v.get(off..off + 4)
        .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

fn set_be32(v: &mut [u8], off: usize, x: u32) -> bool {
    match v.get_mut(off..off + 4) {
        Some(slot) => {
            slot.copy_from_slice(&x.to_be_bytes());
            true
        }
        None => false,
    }
}

fn max_phandle(tree: &Tree) -> u32 {
    (0..tree.len())
        .filter_map(|id| tree.phandle(id))
        .filter(|&p| p != u32::MAX)
        .max()
        .unwrap_or(0)
}

fn child(tree: &Tree, parent: NodeId, name: &str) -> Option<NodeId> {
    tree.node(parent)
        .children
        .iter()
        .copied()
        .find(|&c| tree.node(c).name == name)
}

/// libfdt's subnode lookup: an exact match, or for a name without a unit address, the first
/// node with that base name (`spi` finds `spi@fe300000`).
fn subnode(tree: &Tree, parent: NodeId, name: &str) -> Option<NodeId> {
    tree.node(parent).children.iter().copied().find(|&c| {
        let n = &tree.node(c).name;
        n == name
            || (!name.contains('@') && n.split_once('@').is_some_and(|(base, _)| base == name))
    })
}

/// Paths of `__local_fixups__` entries: (overlay node, property, cell offsets).
fn local_fixups(ov: &Tree) -> Vec<(NodeId, String, Vec<usize>)> {
    fn walk(ov: &Tree, lf: NodeId, node: NodeId, out: &mut Vec<(NodeId, String, Vec<usize>)>) {
        for p in &ov.node(lf).props {
            let offs = p
                .value
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| u32::from_be_bytes(*c) as usize)
                .collect();
            out.push((node, p.name.clone(), offs));
        }
        for &c in &ov.node(lf).children {
            if let Some(target) = child(ov, node, &ov.node(c).name) {
                walk(ov, c, target, out);
            }
        }
    }
    let mut out = Vec::new();
    if let Some(lf) = child(ov, Tree::ROOT, "__local_fixups__") {
        walk(ov, lf, Tree::ROOT, &mut out);
    }
    out
}

/// Applies `overlay` to `base`. Errors name the fragment or label that failed.
pub fn apply(base: &Tree, overlay: &Tree) -> Result<Applied, String> {
    let mut tree = base.clone();
    let mut ov = overlay.clone();

    // 1. Renumber the overlay's own phandles above the base's, and the cells that use them.
    let delta = max_phandle(&tree);
    for id in 0..ov.len() {
        for name in ["phandle", "linux,phandle"] {
            if let Some(p) = ov.node_mut(id).props.iter_mut().find(|p| p.name == name)
                && let Some(v) = be32(&p.value, 0)
            {
                set_be32(&mut p.value, 0, v.wrapping_add(delta));
            }
        }
    }
    let fixups_local = local_fixups(&ov);
    for (node, prop, offs) in &fixups_local {
        if let Some(p) = ov
            .node_mut(*node)
            .props
            .iter_mut()
            .find(|p| &p.name == prop)
        {
            for &off in offs {
                if let Some(v) = be32(&p.value, off) {
                    set_be32(&mut p.value, off, v.wrapping_add(delta));
                }
            }
        }
    }

    // 2. Resolve references to the base's labels.
    if let Some(fx) = child(&ov, Tree::ROOT, "__fixups__") {
        let symbols = child(&tree, Tree::ROOT, "__symbols__");
        let entries: Vec<(String, Vec<u8>)> = ov
            .node(fx)
            .props
            .iter()
            .map(|p| (p.name.clone(), p.value.clone()))
            .collect();
        for (label, value) in entries {
            let target = symbols
                .and_then(|s| tree.prop_str(s, &label).map(str::to_string))
                .ok_or_else(|| format!("label `{label}` isn't in the base's __symbols__ (build the base with dtc -@)"))?;
            let node = tree
                .find(&target)
                .ok_or_else(|| format!("`{label}` points at {target}, which isn't in the base"))?;
            let phandle = tree
                .phandle(node)
                .ok_or_else(|| format!("{target} (`{label}`) has no phandle"))?;
            for entry in value.split(|&b| b == 0).filter(|e| !e.is_empty()) {
                let entry = String::from_utf8_lossy(entry);
                let mut parts = entry.rsplitn(3, ':');
                let (off, prop, path) = (parts.next(), parts.next(), parts.next());
                let (Some(off), Some(prop), Some(path)) =
                    (off.and_then(|o| o.parse::<usize>().ok()), prop, path)
                else {
                    return Err(format!("malformed __fixups__ entry `{entry}`"));
                };
                let at = ov
                    .find(path)
                    .ok_or_else(|| format!("__fixups__ entry `{entry}`: no node {path}"))?;
                let p = ov.node_mut(at).props.iter_mut().find(|p| p.name == prop);
                if !p.is_some_and(|p| set_be32(&mut p.value, off, phandle)) {
                    return Err(format!("__fixups__ entry `{entry}` is out of range"));
                }
            }
        }
    }

    // 3. Merge each fragment into its target. Like libfdt, phandle conflicts are settled
    // for every fragment first, so references in any fragment see the final values.
    tree.finalize();
    let fragments: Vec<(NodeId, NodeId)> = ov
        .node(Tree::ROOT)
        .children
        .iter()
        .filter_map(|&frag| child(&ov, frag, "__overlay__").map(|over| (frag, over)))
        .collect();
    for &(frag, over) in &fragments {
        let target = fragment_target(&tree, &ov, frag)?;
        prevent_phandle_overwrite(&tree, target, &mut ov, over, &fixups_local);
    }
    let mut targets: Vec<(NodeId, NodeId)> = Vec::new(); // (fragment, target)
    let mut touched = Vec::new();
    for &(frag, over) in &fragments {
        let target = fragment_target(&tree, &ov, frag)?;
        targets.push((frag, target));
        merge(&mut tree, target, &ov, over, &mut touched);
        tree.finalize();
    }

    // 4. Labels the overlay defines become the base's, at their new paths.
    if let Some(sym) = child(&ov, Tree::ROOT, "__symbols__") {
        let entries: Vec<(String, String)> = ov
            .node(sym)
            .props
            .iter()
            .filter_map(|p| Some((p.name.clone(), ov.prop_str(sym, &p.name)?.to_string())))
            .collect();
        for (label, path) in entries {
            let Some(rest) = path.strip_prefix('/') else {
                continue;
            };
            let (frag_name, after) = rest.split_once('/').unwrap_or((rest, ""));
            let Some(rel) = after.strip_prefix("__overlay__") else {
                continue;
            };
            let Some(&(_, target)) = targets.iter().find(|(f, _)| ov.node(*f).name == frag_name)
            else {
                continue;
            };
            let base_path = tree.path(target);
            let new_path = if rel.is_empty() {
                base_path
            } else if base_path == "/" {
                rel.to_string()
            } else {
                format!("{base_path}{rel}")
            };
            let symbols = match child(&tree, Tree::ROOT, "__symbols__") {
                Some(s) => s,
                None => insert_child_first(&mut tree, Tree::ROOT, "__symbols__"),
            };
            let mut value = new_path.into_bytes();
            value.push(0);
            set_prop_libfdt(&mut tree, symbols, &label, value);
        }
    }
    tree.finalize();
    Ok(Applied { tree, touched })
}

/// The base node a fragment applies to: its `target` phandle or `target-path`.
fn fragment_target(tree: &Tree, ov: &Tree, frag: NodeId) -> Result<NodeId, String> {
    let name = &ov.node(frag).name;
    if let Some(ph) = ov.prop_u32(frag, "target") {
        tree.by_phandle(ph)
            .ok_or_else(|| format!("{name}: target phandle {ph:#x} isn't in the base"))
    } else if let Some(path) = ov.prop_str(frag, "target-path") {
        tree.find(path)
            .ok_or_else(|| format!("{name}: target-path {path} isn't in the base"))
    } else {
        Err(format!("{name} has no target or target-path"))
    }
}

/// libfdt adds new properties before existing ones and replaces existing ones in place.
fn set_prop_libfdt(tree: &mut Tree, node: NodeId, name: &str, value: Vec<u8>) {
    let props = &mut tree.node_mut(node).props;
    match props.iter_mut().find(|p| p.name == name) {
        Some(p) => p.value = value,
        None => props.insert(
            0,
            Property {
                name: name.to_string(),
                value,
            },
        ),
    }
}

/// libfdt adds a new subnode before the existing ones.
fn insert_child_first(tree: &mut Tree, parent: NodeId, name: &str) -> NodeId {
    let id = tree.add_child(parent, name);
    let children = &mut tree.node_mut(parent).children;
    children.pop();
    children.insert(0, id);
    id
}

fn merge(
    tree: &mut Tree,
    target: NodeId,
    ov: &Tree,
    node: NodeId,
    touched: &mut Vec<(NodeId, Option<String>, NodeId)>,
) {
    touched.push((target, None, node));
    for p in &ov.node(node).props {
        set_prop_libfdt(tree, target, &p.name, p.value.clone());
        touched.push((target, Some(p.name.clone()), node));
    }
    for &c in &ov.node(node).children {
        let name = ov.node(c).name.clone();
        let sub = match subnode(tree, target, &name) {
            Some(existing) => existing,
            None => insert_child_first(tree, target, &name),
        };
        merge(tree, sub, ov, c, touched);
    }
}

/// libfdt's `overlay_prevent_phandle_overwrite`: when an overlay node lands on a base node
/// that already has a phandle, the overlay's phandle is dropped and references to it are
/// pointed at the existing one.
fn prevent_phandle_overwrite(
    tree: &Tree,
    target: NodeId,
    ov: &mut Tree,
    node: NodeId,
    fixups_local: &[(NodeId, String, Vec<usize>)],
) {
    if let (Some(existing), Some(mine)) = (tree.phandle(target), ov.phandle(node))
        && existing != mine
    {
        ov.node_mut(node)
            .props
            .retain(|p| p.name != "phandle" && p.name != "linux,phandle");
        for (n, prop, offs) in fixups_local {
            if let Some(p) = ov.node_mut(*n).props.iter_mut().find(|p| &p.name == prop) {
                for &off in offs {
                    if be32(&p.value, off) == Some(mine) {
                        set_be32(&mut p.value, off, existing);
                    }
                }
            }
        }
    }
    for c in ov.node(node).children.clone() {
        let name = ov.node(c).name.clone();
        if let Some(t) = subnode(tree, target, &name) {
            prevent_phandle_overwrite(tree, t, ov, c, fixups_local);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dts::{Options, compile_str, compile_with};
    use crate::vfs::MemVfs;

    #[test]
    fn applies_fragments_fixups_and_symbols() {
        let base_src = "/dts-v1/;\n/ { soc { i2c1: i2c@1 { #address-cells = <1>; #size-cells = <0>; status = \"disabled\"; }; clk: clock { #clock-cells = <1>; }; }; };\n";
        let base = compile_with(
            &MemVfs::new().with("b.dts", base_src),
            "b.dts",
            &Options {
                symbols: true,
                ..Options::default()
            },
        )
        .tree;
        let ov_src = "/dts-v1/;\n/plugin/;\n&i2c1 { status = \"okay\"; cam: camera@36 { reg = <0x36>; clocks = <&clk 3>; port { ep: endpoint { remote-endpoint = <&ep>; }; }; }; };\n&{/soc} { extra { x; }; };\n";
        // Overlays are built with -@ (as the kernel does), so their labels can be exported.
        let ov = compile_with(
            &MemVfs::new().with("o.dtso", ov_src),
            "o.dtso",
            &Options {
                symbols: true,
                ..Options::default()
            },
        );
        assert!(!ov.has_errors(), "{:?}", ov.diagnostics);
        let out = apply(&base, &ov.tree).unwrap();
        let t = &out.tree;
        let i2c = t.find("/soc/i2c@1").unwrap();
        assert_eq!(t.prop_str(i2c, "status"), Some("okay"));
        let cam = t.find("/soc/i2c@1/camera@36").unwrap();
        // &clk resolved through the base's __symbols__.
        let clk = t.find("/soc/clock").unwrap();
        assert_eq!(be32(t.prop(cam, "clocks").unwrap(), 0), t.phandle(clk));
        // The overlay's own phandle was renumbered above the base's, and its use followed.
        let ep = t.find("/soc/i2c@1/camera@36/port/endpoint").unwrap();
        assert!(t.phandle(ep).unwrap() > max_phandle(&base));
        assert_eq!(
            be32(t.prop(ep, "remote-endpoint").unwrap(), 0),
            t.phandle(ep)
        );
        assert!(t.find("/soc/extra").is_some());
        // Overlay labels now point into the base.
        let sym = t.find("/__symbols__").unwrap();
        assert_eq!(t.prop_str(sym, "cam"), Some("/soc/i2c@1/camera@36"));
        assert!(t.find("/fragment@0").is_none());
        assert!(
            out.touched
                .iter()
                .any(|(n, p, _)| *n == i2c && p.as_deref() == Some("status"))
        );
    }

    #[test]
    fn missing_symbols_is_a_clear_error() {
        let base = compile_str("b.dts", "/dts-v1/;\n/ { a: n {}; };\n").tree;
        let ov = compile_str("o.dtso", "/dts-v1/;\n/plugin/;\n&a { x; };\n").tree;
        let err = apply(&base, &ov).err().unwrap();
        assert!(err.contains("dtc -@"), "{err}");
    }
}
