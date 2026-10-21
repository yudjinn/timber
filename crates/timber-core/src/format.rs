//! Human-readable property values (close to `dtc -O dts`), phandle reference decoding,
//! and whole-tree DTS output.

use crate::{NodeId, Property, Tree};

/// A piece of a formatted value. References carry the node they point at so the UI can
/// render them as links.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Seg {
    Text(String),
    Ref { text: String, target: NodeId },
}

/// How a property refers to other nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RefSpec {
    /// A list of bare phandles.
    Plain,
    /// Each phandle is followed by as many cells as the target's named `#…-cells` says.
    Cells(&'static str),
}

fn ref_spec(name: &str) -> Option<RefSpec> {
    use RefSpec::*;
    Some(match name {
        "interrupt-parent"
        | "memory-region"
        | "operating-points-v2"
        | "remote-endpoint"
        | "cpu"
        | "cpus"
        | "nvmem-cells"
        | "next-level-cache"
        | "cpu-idle-states"
        | "trip"
        | "sram"
        | "phy-handle"
        | "pcs-handle"
        | "iommu-map-mask" => Plain,
        "clocks" | "assigned-clocks" | "assigned-clock-parents" => Cells("#clock-cells"),
        "resets" => Cells("#reset-cells"),
        "power-domains" => Cells("#power-domain-cells"),
        "phys" => Cells("#phy-cells"),
        "dmas" => Cells("#dma-cells"),
        "iommus" => Cells("#iommu-cells"),
        "mboxes" => Cells("#mbox-cells"),
        "pwms" => Cells("#pwm-cells"),
        "io-channels" => Cells("#io-channel-cells"),
        "interconnects" => Cells("#interconnect-cells"),
        "thermal-sensors" => Cells("#thermal-sensor-cells"),
        "sound-dai" => Cells("#sound-dai-cells"),
        "interrupts-extended" => Cells("#interrupt-cells"),
        "cooling-device" => Cells("#cooling-cells"),
        "msi-parent" => Cells("#msi-cells"),
        "hwlocks" => Cells("#hwlock-cells"),
        "nr-gpios" | "ngpios" | "gpio-line-names" | "gpio-ranges" | "gpio-reserved-ranges" => {
            return None;
        }
        n if n == "gpios" || n == "gpio" || n.ends_with("-gpios") || n.ends_with("-gpio") => {
            Cells("#gpio-cells")
        }
        n if n.ends_with("-supply") || n.ends_with("-handle") => Plain,
        n if n
            .strip_prefix("pinctrl-")
            .is_some_and(|i| !i.is_empty() && i.bytes().all(|c| c.is_ascii_digit())) =>
        {
            Plain
        }
        _ => return None,
    })
}

fn to_cells(v: &[u8]) -> Option<Vec<u32>> {
    let (cells, rest) = v.as_chunks::<4>();
    rest.is_empty()
        .then(|| cells.iter().map(|c| u32::from_be_bytes(*c)).collect())
}

/// Decodes a phandle-list property into `(target, args)` entries; `None` if the bytes
/// don't fit the expected shape (unknown phandle, missing `#…-cells`, wrong length).
fn decode_refs(
    tree: &Tree,
    value: &[u8],
    spec: RefSpec,
) -> Option<Vec<(Option<NodeId>, Vec<u32>)>> {
    let cells = to_cells(value)?;
    if cells.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < cells.len() {
        let ph = cells[i];
        i += 1;
        if ph == 0 && matches!(spec, RefSpec::Cells(_)) {
            out.push((None, Vec::new())); // an intentional hole, e.g. in gpio lists
            continue;
        }
        let target = tree.by_phandle(ph)?;
        let n = match spec {
            RefSpec::Plain => 0,
            RefSpec::Cells(prop) => tree.prop_u32(target, prop)? as usize,
        };
        let args = cells.get(i..i + n)?.to_vec();
        i += n;
        out.push((Some(target), args));
    }
    Some(out)
}

/// How a reference to `id` is written: `&label`, or `&{/path}` for unlabeled nodes.
pub fn ref_text(tree: &Tree, id: NodeId) -> String {
    match tree.node(id).labels.first() {
        Some(l) => format!("&{l}"),
        None => format!("&{{{}}}", tree.path(id)),
    }
}

/// Nodes a property refers to (by phandle, or by path for `/aliases`, `/chosen`, `__symbols__`).
pub fn prop_references(tree: &Tree, node: NodeId, prop: &Property) -> Vec<NodeId> {
    if let Some(spec) = ref_spec(&prop.name)
        && let Some(entries) = decode_refs(tree, &prop.value, spec)
    {
        return entries.into_iter().filter_map(|(t, _)| t).collect();
    }
    path_target(tree, node, prop).into_iter().collect()
}

fn path_target(tree: &Tree, node: NodeId, prop: &Property) -> Option<NodeId> {
    let parent = tree.node(node).parent?;
    let name = tree.node(node).name.as_str();
    let is_path_prop = parent == Tree::ROOT
        && (matches!(name, "aliases" | "__symbols__")
            || (name == "chosen"
                && matches!(prop.name.as_str(), "stdout-path" | "linux,stdout-path")));
    if !is_path_prop {
        return None;
    }
    let s = std::str::from_utf8(prop.value.strip_suffix(&[0])?).ok()?;
    let s = s.split(':').next()?; // stdout-path = "serial0:115200n8"
    tree.find(s)
}

/// Formats a property value. An empty result means a boolean (valueless) property.
pub fn format_prop(tree: &Tree, node: NodeId, prop: &Property) -> Vec<Seg> {
    let v = &prop.value;
    if v.is_empty() {
        return Vec::new();
    }
    if let Some(spec) = ref_spec(&prop.name)
        && let Some(entries) = decode_refs(tree, v, spec)
    {
        let mut segs = Vec::new();
        for (n, (target, args)) in entries.into_iter().enumerate() {
            if n > 0 {
                segs.push(Seg::Text(", ".into()));
            }
            match target {
                Some(t) => {
                    segs.push(Seg::Text("<".into()));
                    segs.push(Seg::Ref {
                        text: ref_text(tree, t),
                        target: t,
                    });
                    let rest: String = args.iter().map(|a| format!(" {}", hex(*a))).collect();
                    segs.push(Seg::Text(format!("{rest}>")));
                }
                None => segs.push(Seg::Text("<0x00>".into())),
            }
        }
        return segs;
    }
    if let Some(target) = path_target(tree, node, prop) {
        let text = format_bytes(&prop.name, v);
        return vec![Seg::Ref { text, target }];
    }
    if let Some(parent) = tree.node(node).parent
        && let Some(cells) = to_cells(v)
    {
        let group = match prop.name.as_str() {
            "reg" => Some((tree.address_cells(parent) + tree.size_cells(parent)) as usize),
            "ranges" | "dma-ranges" => Some(
                (tree.address_cells(node) + tree.address_cells(parent) + tree.size_cells(node))
                    as usize,
            ),
            _ => None,
        };
        if let Some(g) = group.filter(|&g| g > 0 && cells.len() % g == 0 && cells.len() > g) {
            let parts: Vec<String> = cells.chunks(g).map(|c| format_cells(c, false)).collect();
            return vec![Seg::Text(parts.join(", "))];
        }
    }
    vec![Seg::Text(format_bytes(&prop.name, v))]
}

/// Formats a value as a plain string (references as `&label`).
pub fn format_prop_text(tree: &Tree, node: NodeId, prop: &Property) -> String {
    format_prop(tree, node, prop)
        .into_iter()
        .map(|s| match s {
            Seg::Text(t) | Seg::Ref { text: t, .. } => t,
        })
        .collect()
}

fn hex(v: u32) -> String {
    format!("{v:#04x}")
}

fn format_cells(cells: &[u32], decimal: bool) -> String {
    let inner: Vec<String> = cells
        .iter()
        .map(|&c| if decimal { c.to_string() } else { hex(c) })
        .collect();
    format!("<{}>", inner.join(" "))
}

/// Generic formatting by shape: string list, cells, or bytes.
fn format_bytes(name: &str, v: &[u8]) -> String {
    if let Some(strings) = as_string_list(v) {
        return strings
            .iter()
            .map(|s| format!("\"{}\"", escape(s)))
            .collect::<Vec<_>>()
            .join(", ");
    }
    if let Some(cells) = to_cells(v) {
        let decimal = name.starts_with('#') && name.ends_with("-cells");
        return format_cells(&cells, decimal);
    }
    let bytes: Vec<String> = v.iter().map(|b| format!("{b:02x}")).collect();
    format!("[{}]", bytes.join(" "))
}

/// Splits a value into strings if it looks like a NUL-terminated string list.
pub fn as_string_list(v: &[u8]) -> Option<Vec<&str>> {
    let body = v.strip_suffix(&[0])?;
    if body.is_empty() {
        return None;
    }
    let parts: Vec<&str> = body
        .split(|&b| b == 0)
        .map(|s| std::str::from_utf8(s).ok().filter(|s| !s.is_empty()))
        .collect::<Option<_>>()?;
    let printable = parts.iter().all(|s| {
        s.chars()
            .all(|c| !c.is_control() || matches!(c, '\t' | '\n' | '\r'))
    });
    printable.then_some(parts)
}

fn escape(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out
}

/// For each node, the `(source node, property)` pairs that refer to it.
pub fn reverse_references(tree: &Tree) -> Vec<Vec<(NodeId, String)>> {
    let mut rev = vec![Vec::new(); tree.len()];
    for id in 0..tree.len() {
        for prop in &tree.node(id).props {
            for target in prop_references(tree, id, prop) {
                if !rev[target]
                    .iter()
                    .any(|(s, p): &(NodeId, String)| *s == id && *p == prop.name)
                {
                    rev[target].push((id, prop.name.clone()));
                }
            }
        }
    }
    rev
}

/// Decompiles the whole tree to DTS source.
pub fn to_dts(tree: &Tree) -> String {
    let mut out = String::from("/dts-v1/;\n\n");
    for (addr, size) in &tree.mem_reserve {
        out.push_str(&format!("/memreserve/ {addr:#018x} {size:#018x};\n"));
    }
    if !tree.mem_reserve.is_empty() {
        out.push('\n');
    }
    write_node(tree, Tree::ROOT, 0, &mut out);
    out
}

fn write_node(tree: &Tree, id: NodeId, depth: usize, out: &mut String) {
    let node = tree.node(id);
    let indent = "\t".repeat(depth);
    out.push_str(&indent);
    for l in &node.labels {
        out.push_str(&format!("{l}: "));
    }
    out.push_str(if id == Tree::ROOT { "/" } else { &node.name });
    out.push_str(" {\n");
    for prop in &node.props {
        let value = format_prop_text(tree, id, prop);
        if value.is_empty() {
            out.push_str(&format!("{indent}\t{};\n", prop.name));
        } else {
            out.push_str(&format!("{indent}\t{} = {value};\n", prop.name));
        }
    }
    for &child in &node.children {
        out.push('\n');
        write_node(tree, child, depth + 1, out);
    }
    out.push_str(&format!("{indent}}};\n"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dts::compile_str;

    fn tree() -> Tree {
        let c = compile_str(
            "t.dts",
            r#"/dts-v1/;
            / {
                #address-cells = <2>;
                #size-cells = <1>;
                aliases { serial0 = &uart; };
                chosen { stdout-path = "serial0:115200n8"; };
                cru: clock-controller { #clock-cells = <1>; };
                gpio0: gpio { #gpio-cells = <2>; gpio-controller; };
                vcc: regulator {};
                uart: serial@fe650000 {
                    compatible = "rockchip,rk3568-uart", "snps,dw-apb-uart";
                    reg = <0x0 0xfe650000 0x100>, <0x0 0xfe660000 0x100>;
                    clocks = <&cru 12>, <&cru 40>;
                    reset-gpios = <&gpio0 3 1>, <0>, <&gpio0 4 0>;
                    vcc-supply = <&vcc>;
                    status = "okay";
                    dma-coherent;
                    mac = [00 11 22];
                    odd = <&vcc 5>;
                };
            };"#,
        );
        assert!(!c.has_errors(), "{:?}", c.diagnostics);
        c.tree
    }

    fn text(t: &Tree, path: &str, prop: &str) -> String {
        let id = t.find(path).unwrap();
        let p = t.node(id).props.iter().find(|p| p.name == prop).unwrap();
        format_prop_text(t, id, p)
    }

    #[test]
    fn formats_like_dtc() {
        let t = tree();
        let u = "/serial@fe650000";
        assert_eq!(
            text(&t, u, "compatible"),
            r#""rockchip,rk3568-uart", "snps,dw-apb-uart""#
        );
        assert_eq!(
            text(&t, u, "reg"),
            "<0x00 0xfe650000 0x100>, <0x00 0xfe660000 0x100>"
        );
        assert_eq!(text(&t, u, "clocks"), "<&cru 0x0c>, <&cru 0x28>");
        assert_eq!(
            text(&t, u, "reset-gpios"),
            "<&gpio0 0x03 0x01>, <0x00>, <&gpio0 0x04 0x00>"
        );
        assert_eq!(text(&t, u, "vcc-supply"), "<&vcc>");
        assert_eq!(text(&t, u, "dma-coherent"), "");
        assert_eq!(text(&t, u, "mac"), "[00 11 22]");
        assert_eq!(text(&t, "/", "#address-cells"), "<2>");
        // `odd` isn't a known reference property, so it stays numeric.
        assert!(text(&t, u, "odd").starts_with("<0x"));
    }

    #[test]
    fn reference_segments_and_reverse_index() {
        let t = tree();
        let uart = t.find("/serial@fe650000").unwrap();
        let cru = t.find("/clock-controller").unwrap();
        let p = t
            .node(uart)
            .props
            .iter()
            .find(|p| p.name == "clocks")
            .unwrap();
        assert!(format_prop(&t, uart, p).contains(&Seg::Ref {
            text: "&cru".into(),
            target: cru
        }));

        let rev = reverse_references(&t);
        assert_eq!(rev[cru], [(uart, "clocks".to_string())]);
        let aliases = t.find("/aliases").unwrap();
        let chosen = t.find("/chosen").unwrap();
        assert!(rev[uart].contains(&(aliases, "serial0".to_string())));
        assert!(rev[uart].contains(&(chosen, "stdout-path".to_string())));
    }

    #[test]
    fn decompiled_dts_recompiles_to_the_same_tree() {
        let t = tree();
        let dts = to_dts(&t);
        let again = compile_str("again.dts", &dts);
        assert!(!again.has_errors(), "{dts}\n{:?}", again.diagnostics);
        assert_eq!(
            crate::dtb::write(&again.tree),
            crate::dtb::write(&t),
            "{dts}"
        );
    }
}
