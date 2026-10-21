//! Working with board sources: cheap board scans, matching a blob to the board it was built
//! from, and comparing a tree against its source.

use crate::format::{Seg, format_prop};
use crate::{NodeId, Tree};

/// What a board `.dts` declares about itself, found without compiling it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardInfo {
    pub path: String,
    /// The root node's `compatible`, as written in the file itself.
    pub compatible: Vec<String>,
    pub model: Option<String>,
    /// The file is an overlay (`/plugin/`).
    pub overlay: bool,
}

/// Scans a board source for the root node's `compatible` and `model` and whether it's an
/// overlay. Only the file's own text is read (no includes, no macro expansion).
pub fn scan_board(path: &str, text: &str) -> BoardInfo {
    let b = text.as_bytes();
    let mut info = BoardInfo {
        path: path.to_string(),
        compatible: Vec::new(),
        model: None,
        overlay: false,
    };
    let mut depth = 0usize;
    let mut in_root = false;
    let mut root_pending = false;
    let mut i = 0;
    let is_name = |c: u8| {
        c.is_ascii_alphanumeric() || matches!(c, b',' | b'.' | b'_' | b'+' | b'#' | b'@' | b'-')
    };

    while i < b.len() {
        let c = b[i];
        match c {
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
                i += 2;
                continue;
            }
            b'"' => {
                i = skip_string(b, i);
                continue;
            }
            b'/' if depth == 0 => {
                if text[i..].starts_with("/plugin/") {
                    info.overlay = true;
                    i += 8;
                    continue;
                }
                // `/ {` opens a root block; other `/word/` directives don't.
                let mut j = i + 1;
                while j < b.len() && b[j].is_ascii_whitespace() {
                    j += 1;
                }
                root_pending = b.get(j) == Some(&b'{');
            }
            b'{' => {
                if depth == 0 {
                    in_root = root_pending;
                    root_pending = false;
                }
                depth += 1;
            }
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    in_root = false;
                }
            }
            c if depth == 1 && in_root && is_name(c) && (i == 0 || !is_name(b[i - 1])) => {
                let start = i;
                while i < b.len() && is_name(b[i]) {
                    i += 1;
                }
                let name = &text[start..i];
                if matches!(name, "compatible" | "model") {
                    let (strings, end) = read_strings(text, i);
                    i = end;
                    if !strings.is_empty() {
                        if name == "model" {
                            info.model = strings.into_iter().next();
                        } else {
                            info.compatible = strings;
                        }
                    }
                }
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    info
}

/// Like [`scan_board`], but when the file itself declares no root `compatible` (a variant
/// that includes another board), follows its quoted `#include`s to find one.
pub fn scan_board_in(
    vfs: &dyn crate::vfs::Vfs,
    resolver: &crate::vfs::IncludeResolver,
    path: &str,
) -> BoardInfo {
    fn inherited(
        vfs: &dyn crate::vfs::Vfs,
        resolver: &crate::vfs::IncludeResolver,
        path: &str,
        depth: usize,
    ) -> Option<BoardInfo> {
        let text = String::from_utf8_lossy(&vfs.read(path)?).into_owned();
        let info = scan_board(path, &text);
        if !info.compatible.is_empty() || depth == 0 {
            return Some(info);
        }
        // The last include that declares one wins, as later definitions override earlier ones.
        let found = text
            .lines()
            .filter_map(|l| l.trim_start().strip_prefix("#include").map(str::trim))
            .filter_map(|l| l.strip_prefix('"').and_then(|l| l.split('"').next()))
            .filter_map(|name| resolver.resolve(name, path, true))
            .filter_map(|inc| inherited(vfs, resolver, &inc, depth - 1))
            .filter(|i| !i.compatible.is_empty())
            .last();
        Some(BoardInfo {
            compatible: found
                .as_ref()
                .map(|f| f.compatible.clone())
                .unwrap_or_default(),
            model: info.model.or(found.and_then(|f| f.model)),
            ..info
        })
    }
    inherited(vfs, resolver, path, 4).unwrap_or(BoardInfo {
        path: path.to_string(),
        compatible: Vec::new(),
        model: None,
        overlay: false,
    })
}

fn skip_string(b: &[u8], start: usize) -> usize {
    let mut i = start + 1;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            b'"' => return i + 1,
            _ => i += 1,
        }
    }
    b.len()
}

/// Reads `= "a", "b";` after a property name; returns the strings and where it stopped.
fn read_strings(text: &str, mut i: usize) -> (Vec<String>, usize) {
    let b = text.as_bytes();
    let skip_ws = |mut i: usize| {
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        i
    };
    i = skip_ws(i);
    if b.get(i) != Some(&b'=') {
        return (Vec::new(), i);
    }
    i += 1;
    let mut out = Vec::new();
    loop {
        i = skip_ws(i);
        if b.get(i) != Some(&b'"') {
            break;
        }
        let end = skip_string(b, i);
        out.push(text[i + 1..end.saturating_sub(1).max(i + 1)].replace("\\\"", "\""));
        i = skip_ws(end);
        if b.get(i) == Some(&b',') {
            i += 1;
        } else {
            break;
        }
    }
    (out, i)
}

/// Why a board was considered a match for a blob.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MatchReason {
    /// Only `model` matched.
    Model,
    /// Some compatible strings are shared.
    Shared(usize),
    /// The most specific (first) compatible string is the same.
    First,
    /// The whole compatible list is identical.
    Exact,
}

impl MatchReason {
    pub fn describe(self) -> String {
        match self {
            MatchReason::Exact => "root compatible identical".into(),
            MatchReason::First => "same board compatible".into(),
            MatchReason::Shared(n) => format!(
                "{n} compatible string{} shared",
                if n == 1 { "" } else { "s" }
            ),
            MatchReason::Model => "same model".into(),
        }
    }
}

/// Candidate boards for a blob with the given root `compatible`/`model`, best first.
pub fn rank_boards(
    boards: &[BoardInfo],
    compatible: &[&str],
    model: Option<&str>,
) -> Vec<(usize, MatchReason)> {
    let mut ranked: Vec<(usize, MatchReason, bool)> = boards
        .iter()
        .enumerate()
        .filter(|(_, b)| !b.overlay)
        .filter_map(|(i, b)| {
            let same_model = model.is_some() && b.model.as_deref() == model;
            let shared = b
                .compatible
                .iter()
                .filter(|c| compatible.contains(&c.as_str()))
                .count();
            let reason = if !b.compatible.is_empty()
                && b.compatible
                    .iter()
                    .map(String::as_str)
                    .eq(compatible.iter().copied())
            {
                MatchReason::Exact
            } else if b.compatible.first().map(String::as_str) == compatible.first().copied()
                && shared > 0
            {
                MatchReason::First
            } else if shared > 0 {
                MatchReason::Shared(shared)
            } else if same_model {
                MatchReason::Model
            } else {
                return None;
            };
            Some((i, reason, same_model))
        })
        .collect();
    ranked.sort_by(|a, b| {
        (b.1, b.2)
            .cmp(&(a.1, a.2))
            .then_with(|| boards[a.0].path.cmp(&boards[b.0].path))
    });
    ranked.into_iter().map(|(i, r, _)| (i, r)).collect()
}

fn file_stem(path: &str) -> &str {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    name.rsplit_once('.').map_or(name, |(stem, _)| stem)
}

/// A shortlist candidate for a blob, with the signals that put it there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub index: usize,
    /// The board's file name matches the blob's (`acme-gw.dtb` ↔ `acme-gw.dts`).
    pub file_name: bool,
    pub reason: Option<MatchReason>,
}

impl Candidate {
    pub fn describe(&self) -> String {
        match (self.file_name, self.reason) {
            (true, Some(r)) => format!("file name + {}", r.describe()),
            (true, None) => "file name".into(),
            (false, Some(r)) => r.describe(),
            (false, None) => "chosen manually".into(),
        }
    }
}

/// Candidates for a blob named `blob_name`, best first: file name, then compatible/model.
pub fn shortlist(
    boards: &[BoardInfo],
    blob_name: &str,
    compatible: &[&str],
    model: Option<&str>,
) -> Vec<Candidate> {
    let stem = file_stem(blob_name);
    let mut out: Vec<Candidate> = rank_boards(boards, compatible, model)
        .into_iter()
        .map(|(index, reason)| Candidate {
            index,
            file_name: file_stem(&boards[index].path) == stem,
            reason: Some(reason),
        })
        .collect();
    for (index, b) in boards.iter().enumerate() {
        if !b.overlay && file_stem(&b.path) == stem && !out.iter().any(|c| c.index == index) {
            out.push(Candidate {
                index,
                file_name: true,
                reason: None,
            });
        }
    }
    // Stable: keeps compatible ranking within each file-name group.
    out.sort_by_key(|c| !c.file_name);
    out
}

/// How well a compiled source explains a blob, from 0 to 1: half node coverage (blob nodes
/// found at the same path), half property agreement within those nodes.
pub fn content_score(blob: &Tree, source: &Tree) -> f64 {
    let map = map_by_path(blob, source);
    let (mut nodes, mut matched, mut props, mut equal) = (0usize, 0usize, 0usize, 0usize);
    for id in (0..blob.len()).filter(|&id| !is_generated(blob, id)) {
        nodes += 1;
        let Some(sid) = map[id] else { continue };
        matched += 1;
        let cmp = compare_node(blob, id, source, sid);
        let compared = blob
            .node(id)
            .props
            .iter()
            .filter(|p| !ignored(&p.name))
            .count();
        props += compared + cmp.only_b.len();
        equal += compared - cmp.differs.len() - cmp.only_a.len();
    }
    let coverage = if nodes == 0 {
        0.0
    } else {
        matched as f64 / nodes as f64
    };
    let agreement = if props == 0 {
        0.0
    } else {
        equal as f64 / props as f64
    };
    (coverage + agreement) / 2.0
}

/// Whether U-Boot commonly rewrites this property at boot, so a difference from source
/// is probably expected.
pub fn likely_bootloader_patched(path: &str, prop: &str) -> bool {
    let node = path.rsplit('/').next().unwrap_or("");
    path == "/chosen"
        || path.starts_with("/chosen/")
        || path.starts_with("/reserved-memory")
        || (node.split('@').next() == Some("memory") && prop == "reg")
        || matches!(
            prop,
            "mac-address" | "local-mac-address" | "serial-number" | "status"
        )
}

/// For each node of `a`, the node at the same path in `b`.
pub fn map_by_path(a: &Tree, b: &Tree) -> Vec<Option<NodeId>> {
    fn walk(a: &Tree, ai: NodeId, b: &Tree, bi: NodeId, map: &mut [Option<NodeId>]) {
        map[ai] = Some(bi);
        for &child in &a.node(ai).children {
            let name = &a.node(child).name;
            let found = b
                .node(bi)
                .children
                .iter()
                .copied()
                .find(|&c| b.node(c).name.eq_ignore_ascii_case(name));
            if let Some(bc) = found {
                walk(a, child, b, bc, map);
            }
        }
    }
    let mut map = vec![None; a.len()];
    walk(a, Tree::ROOT, b, Tree::ROOT, &mut map);
    map
}

/// Nodes the compiler generates rather than the source defining them.
pub fn is_generated(tree: &Tree, id: NodeId) -> bool {
    tree.ancestors(id).iter().any(|&a| {
        matches!(
            tree.node(a).name.as_str(),
            "__symbols__" | "__fixups__" | "__local_fixups__"
        )
    })
}

/// A property value with references written as target paths, so two compilations that
/// numbered phandles differently still compare equal.
pub fn normalized_value(tree: &Tree, node: NodeId, prop: &crate::Property) -> String {
    format_prop(tree, node, prop)
        .into_iter()
        .map(|s| match s {
            Seg::Text(t) => t,
            Seg::Ref { target, .. } => format!("&{{{}}}", tree.path(target)),
        })
        .collect()
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NodeComparison {
    /// Properties in both whose values differ.
    pub differs: Vec<String>,
    /// Properties only in `a`.
    pub only_a: Vec<String>,
    /// Properties only in `b`.
    pub only_b: Vec<String>,
}

fn ignored(prop: &str) -> bool {
    matches!(prop, "phandle" | "linux,phandle")
}

/// Compares the properties of `a`'s node `ai` with `b`'s node `bi`.
pub fn compare_node(a: &Tree, ai: NodeId, b: &Tree, bi: NodeId) -> NodeComparison {
    let mut out = NodeComparison::default();
    for p in a.node(ai).props.iter().filter(|p| !ignored(&p.name)) {
        match b.node(bi).props.iter().find(|q| q.name == p.name) {
            None => out.only_a.push(p.name.clone()),
            Some(q) if q.value == p.value => {}
            Some(q) => {
                if normalized_value(a, ai, p) != normalized_value(b, bi, q) {
                    out.differs.push(p.name.clone());
                }
            }
        }
    }
    for q in b.node(bi).props.iter().filter(|q| !ignored(&q.name)) {
        if !a.node(ai).has_prop(&q.name) {
            out.only_b.push(q.name.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dts::compile_str;

    #[test]
    fn scans_root_compatible_and_model() {
        let text = r#"
            // compatible = "not,this";
            /dts-v1/;
            #include "soc.dtsi"
            / {
                model = "Radxa ROCK 5B";
                compatible = "radxa,rock-5b",
                             "rockchip,rk3588";
                child { compatible = "nested,not-root"; };
            };
            &uart { compatible = "override,not-root"; };
        "#;
        let info = scan_board("rock-5b.dts", text);
        assert_eq!(info.compatible, ["radxa,rock-5b", "rockchip,rk3588"]);
        assert_eq!(info.model.as_deref(), Some("Radxa ROCK 5B"));
        assert!(!info.overlay);
        assert!(scan_board("o.dtso", "/dts-v1/;\n/plugin/;\n&{/} { x; };").overlay);
    }

    #[test]
    fn variant_boards_inherit_compatible_from_includes() {
        let vfs = crate::vfs::MemVfs::new()
            .with("base.dts", "/dts-v1/;\n#include \"soc.dtsi\"\n/ { model = \"Base\"; compatible = \"acme,base\", \"acme,soc\"; };\n")
            .with("soc.dtsi", "/ { compatible = \"acme,soc\"; };\n")
            .with("variant.dts", "#include \"base.dts\"\n/ { model = \"Variant\"; };\n");
        let r = crate::vfs::IncludeResolver::new(&vfs);
        let info = scan_board_in(&vfs, &r, "variant.dts");
        assert_eq!(info.compatible, ["acme,base", "acme,soc"]);
        assert_eq!(info.model.as_deref(), Some("Variant"));
    }

    #[test]
    fn shortlist_prefers_file_name_and_content_picks_the_custom_board() {
        // A custom board that kept the EVK's compatible.
        let evk_src =
            "/dts-v1/;\n/ { compatible = \"fsl,imx8mp-evk\", \"fsl,imx8mp\"; leds { }; };\n";
        let custom_src = "/dts-v1/;\n/ { compatible = \"fsl,imx8mp-evk\", \"fsl,imx8mp\"; gw { reg = <1>; }; can { }; };\n";
        let boards = vec![
            scan_board("linux/imx8mp-evk.dts", evk_src),
            scan_board("bsp/acme-gw.dts", custom_src),
        ];
        let blob = compile_str("b.dts", custom_src).tree;

        let list = shortlist(
            &boards,
            "acme-gw.dtb",
            &["fsl,imx8mp-evk", "fsl,imx8mp"],
            None,
        );
        assert_eq!(
            list[0],
            Candidate {
                index: 1,
                file_name: true,
                reason: Some(MatchReason::Exact)
            }
        );
        assert_eq!(list[0].describe(), "file name + root compatible identical");

        // Without the file-name hint, content decides.
        let evk = compile_str("e.dts", evk_src).tree;
        let custom = compile_str("c.dts", custom_src).tree;
        assert!(content_score(&blob, &custom) > content_score(&blob, &evk));
        assert_eq!(content_score(&blob, &custom), 1.0);
    }

    #[test]
    fn bootloader_hints() {
        assert!(likely_bootloader_patched("/chosen", "bootargs"));
        assert!(likely_bootloader_patched("/memory@40000000", "reg"));
        assert!(likely_bootloader_patched(
            "/soc/ethernet@30be0000",
            "local-mac-address"
        ));
        assert!(!likely_bootloader_patched("/soc/ethernet@30be0000", "reg"));
    }

    #[test]
    fn ranks_boards() {
        let board = |path: &str, compat: &[&str], model: Option<&str>| BoardInfo {
            path: path.into(),
            compatible: compat.iter().map(|s| s.to_string()).collect(),
            model: model.map(Into::into),
            overlay: false,
        };
        let boards = [
            board(
                "evb.dts",
                &["rockchip,rk3588-evb1", "rockchip,rk3588"],
                None,
            ),
            board(
                "rock-5b.dts",
                &["radxa,rock-5b", "rockchip,rk3588"],
                Some("Radxa ROCK 5B"),
            ),
            board(
                "rock-5b-plus.dts",
                &["radxa,rock-5b", "radxa,rock-5b-plus", "rockchip,rk3588"],
                None,
            ),
            board("other.dts", &["acme,other"], Some("Radxa ROCK 5B")),
            board("unrelated.dts", &["acme,unrelated"], None),
        ];
        let ranked = rank_boards(
            &boards,
            &["radxa,rock-5b", "rockchip,rk3588"],
            Some("Radxa ROCK 5B"),
        );
        let order: Vec<(&str, MatchReason)> = ranked
            .iter()
            .map(|&(i, r)| (boards[i].path.as_str(), r))
            .collect();
        assert_eq!(
            order,
            [
                ("rock-5b.dts", MatchReason::Exact),
                ("rock-5b-plus.dts", MatchReason::First),
                ("evb.dts", MatchReason::Shared(1)),
                ("other.dts", MatchReason::Model),
            ]
        );
    }

    #[test]
    fn maps_and_compares_by_path_ignoring_phandle_numbers() {
        let source = compile_str(
            "s.dts",
            "/dts-v1/;\n/ { clk: clock { #clock-cells = <1>; }; dev { clocks = <&clk 1>; a = <1>; b = <2>; }; };\n",
        )
        .tree;
        // Same shape, but phandle numbered differently, `a` changed, `b` missing, `c` added,
        // and an extra node.
        let blob = compile_str(
            "b.dts",
            "/dts-v1/;\n/ { other { phandle = <1>; }; clk: clock { #clock-cells = <1>; }; dev { clocks = <&clk 1>; a = <5>; c; }; extra {}; };\n",
        )
        .tree;
        let map = map_by_path(&blob, &source);
        let dev = blob.find("/dev").unwrap();
        assert_eq!(map[dev], source.find("/dev"));
        assert_eq!(map[blob.find("/extra").unwrap()], None);
        let cmp = compare_node(&blob, dev, &source, map[dev].unwrap());
        assert_eq!(
            cmp,
            NodeComparison {
                differs: vec!["a".into()],
                only_a: vec!["c".into()],
                only_b: vec!["b".into()]
            }
        );
    }
}
