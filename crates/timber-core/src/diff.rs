//! Comparing two device trees (DESIGN §7): node by node by path, with references compared
//! by the path they point to, so trees that numbered phandles differently still match.
//!
//! The result is A's tree with B's extra nodes grafted on as *ghosts*. They're appended
//! after A's nodes, so A's node ids stay valid. Ghosts carry no properties (B's phandles
//! mean nothing in A); look them up in B through [`TreeDiff::map`].

use std::collections::HashSet;

use crate::bsp::{compare_node, is_generated, map_by_path};
use crate::format::{Seg, format_prop};
use crate::{NodeId, Property, Tree};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    Same,
    /// This many properties differ, are only in A, or are only in B.
    Changed(usize),
    /// The node isn't in B.
    OnlyA,
    /// A ghost: the node is only in B.
    OnlyB,
}

#[derive(Debug, Clone)]
pub struct TreeDiff {
    /// A's tree plus ghosts.
    pub tree: Tree,
    /// Nodes below this id are A's; the rest are ghosts.
    pub a_len: usize,
    /// For every node (A's and ghosts), its counterpart in B.
    pub map: Vec<Option<NodeId>>,
    pub marks: Vec<Mark>,
    /// Marked nodes strictly below each node.
    pub below: Vec<usize>,
    pub changed: usize,
    pub only_a: usize,
    pub only_b: usize,
}

impl TreeDiff {
    pub fn is_ghost(&self, id: NodeId) -> bool {
        id >= self.a_len
    }

    pub fn is_marked(&self, id: NodeId) -> bool {
        self.marks.get(id).is_some_and(|m| *m != Mark::Same)
    }

    /// Marked nodes at or below `id`.
    pub fn in_subtree(&self, id: NodeId) -> usize {
        usize::from(self.is_marked(id)) + self.below.get(id).copied().unwrap_or(0)
    }

    pub fn total(&self) -> usize {
        self.changed + self.only_a + self.only_b
    }

    /// The marked nodes whose parent isn't marked the same way: a whole new or removed
    /// subtree counts once. `tree` is the compared tree (A plus ghosts; `self.tree` unless
    /// it was moved out).
    pub fn roots(&self, tree: &Tree, mark: fn(&Mark) -> bool) -> Vec<NodeId> {
        (0..tree.len().min(self.marks.len()))
            .filter(|&id| mark(&self.marks[id]))
            .filter(|&id| {
                tree.node(id).parent.is_none_or(|p| {
                    !mark(&self.marks[p]) || matches!(self.marks[id], Mark::Changed(_))
                })
            })
            .collect()
    }
}

/// Compares `a` with `b`. `map` (A → B) is computed by path when not given.
pub fn compare(a: &Tree, b: &Tree, map: Option<Vec<Option<NodeId>>>) -> TreeDiff {
    let mut map = map.unwrap_or_else(|| map_by_path(a, b));
    map.resize(a.len(), None);
    let a_len = a.len();
    let mut tree = a.clone();

    // Graft B's unmatched nodes under their parent's counterpart, in B's order.
    let matched: HashSet<NodeId> = map.iter().flatten().copied().collect();
    let mut b_to_union: Vec<Option<NodeId>> = vec![None; b.len()];
    for (aid, bid) in map.iter().enumerate() {
        if let Some(bid) = bid {
            b_to_union[*bid] = Some(aid);
        }
    }
    fn graft(
        b: &Tree,
        bid: NodeId,
        tree: &mut Tree,
        b_to_union: &mut [Option<NodeId>],
        matched: &HashSet<NodeId>,
        map: &mut Vec<Option<NodeId>>,
    ) {
        for &child in &b.node(bid).children {
            if !matched.contains(&child) && !is_generated(b, child) {
                let Some(parent) = b_to_union[bid] else {
                    continue;
                };
                let id = tree.add_child(parent, b.node(child).name.clone());
                tree.node_mut(id).labels = b.node(child).labels.clone();
                b_to_union[child] = Some(id);
                map.push(Some(child));
            }
            graft(b, child, tree, b_to_union, matched, map);
        }
    }
    graft(
        b,
        Tree::ROOT,
        &mut tree,
        &mut b_to_union,
        &matched,
        &mut map,
    );
    tree.finalize();

    let marks: Vec<Mark> = (0..tree.len())
        .map(|id| {
            if id >= a_len {
                return Mark::OnlyB;
            }
            if is_generated(a, id) {
                return Mark::Same;
            }
            match map[id] {
                None => Mark::OnlyA,
                Some(bid) => {
                    let c = compare_node(a, id, b, bid);
                    match c.differs.len() + c.only_a.len() + c.only_b.len() {
                        0 => Mark::Same,
                        n => Mark::Changed(n),
                    }
                }
            }
        })
        .collect();

    let mut below = vec![0usize; tree.len()];
    fn count(tree: &Tree, id: NodeId, marks: &[Mark], below: &mut [usize]) -> usize {
        let mut n = 0;
        for &c in &tree.node(id).children {
            n += count(tree, c, marks, below) + usize::from(marks[c] != Mark::Same);
        }
        below[id] = n;
        n
    }
    count(&tree, Tree::ROOT, &marks, &mut below);

    let tally = |f: fn(&Mark) -> bool| marks.iter().filter(|m| f(m)).count();
    TreeDiff {
        changed: tally(|m| matches!(m, Mark::Changed(_))),
        only_a: tally(|m| *m == Mark::OnlyA),
        only_b: tally(|m| *m == Mark::OnlyB),
        tree,
        a_len,
        map,
        marks,
        below,
    }
}

/// A piece of a formatted property value, for showing two values side by side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub text: String,
    /// Not matched in the other value.
    pub changed: bool,
}

/// Splits a formatted value into tokens: strings, cells, bytes and punctuation. Each comes
/// with a key to compare by (references as their target's path).
fn tokens(tree: &Tree, node: NodeId, prop: &Property) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for seg in format_prop(tree, node, prop) {
        match seg {
            Seg::Ref { text, target } => out.push((text, format!("&{{{}}}", tree.path(target)))),
            Seg::Text(t) => {
                let mut chars = t.chars().peekable();
                while let Some(&c) = chars.peek() {
                    if c.is_whitespace() {
                        chars.next();
                    } else if "<>[],".contains(c) {
                        chars.next();
                        out.push((c.to_string(), c.to_string()));
                    } else if c == '"' {
                        let mut s = String::from(chars.next().unwrap());
                        while let Some(c) = chars.next() {
                            s.push(c);
                            if c == '\\' {
                                s.extend(chars.next());
                            } else if c == '"' {
                                break;
                            }
                        }
                        out.push((s.clone(), s));
                    } else {
                        let mut s = String::new();
                        while let Some(&c) = chars.peek() {
                            if c.is_whitespace() || "<>[],\"".contains(c) {
                                break;
                            }
                            s.push(c);
                            chars.next();
                        }
                        out.push((s.clone(), s));
                    }
                }
            }
        }
    }
    out
}

/// Whether to put a space between two tokens when showing them (`<0x1 0x2>, "a"`).
pub fn space_between(prev: &str, next: &str) -> bool {
    !(matches!(prev, "<" | "[") || matches!(next, ">" | "]" | ","))
}

/// Property `pa` of A's node and `pb` of B's, tokenised with the parts that don't match the
/// other side flagged. A missing property gives no tokens.
pub fn value_diff(
    a: &Tree,
    ai: NodeId,
    pa: Option<&Property>,
    b: &Tree,
    bi: NodeId,
    pb: Option<&Property>,
) -> (Vec<Token>, Vec<Token>) {
    let ta = pa.map(|p| tokens(a, ai, p)).unwrap_or_default();
    let tb = pb.map(|p| tokens(b, bi, p)).unwrap_or_default();
    let (ka, kb) = lcs_matches(&ta, &tb);
    let side = |t: Vec<(String, String)>, keep: Vec<bool>| {
        t.into_iter()
            .zip(keep)
            .map(|((text, _), kept)| Token {
                text,
                changed: !kept,
            })
            .collect()
    };
    (side(ta, ka), side(tb, kb))
}

/// Which tokens of each side are part of a longest common subsequence (by key).
fn lcs_matches(a: &[(String, String)], b: &[(String, String)]) -> (Vec<bool>, Vec<bool>) {
    let (n, m) = (a.len(), b.len());
    // Very long values (big pinctrl tables): compare position by position instead.
    if n * m > 4_000_000 {
        let same = |i: usize, other: &[(String, String)], this: &[(String, String)]| {
            other.get(i).is_some_and(|o| o.1 == this[i].1)
        };
        return (
            (0..n).map(|i| same(i, b, a)).collect(),
            (0..m).map(|i| same(i, a, b)).collect(),
        );
    }
    let mut dp = vec![0u32; (n + 1) * (m + 1)];
    let at = |i: usize, j: usize| i * (m + 1) + j;
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[at(i, j)] = if a[i].1 == b[j].1 {
                dp[at(i + 1, j + 1)] + 1
            } else {
                dp[at(i + 1, j)].max(dp[at(i, j + 1)])
            };
        }
    }
    let (mut ka, mut kb) = (vec![false; n], vec![false; m]);
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if a[i].1 == b[j].1 {
            ka[i] = true;
            kb[j] = true;
            i += 1;
            j += 1;
        } else if dp[at(i + 1, j)] >= dp[at(i, j + 1)] {
            i += 1;
        } else {
            j += 1;
        }
    }
    (ka, kb)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dts::compile_str;

    fn tree(src: &str) -> Tree {
        let c = compile_str("t.dts", src);
        assert!(!c.has_errors(), "{:?}", c.diagnostics);
        c.tree
    }

    #[test]
    fn marks_ghosts_and_phandles() {
        // B numbers phandles differently (an extra referenced node first) and lacks /soc/c.
        let a = tree(
            "/dts-v1/;\n/ { soc { clk: clk { #clock-cells = <0>; }; dev { clocks = <&clk>; x = <1>; }; c { }; }; };\n",
        );
        let b = tree(
            "/dts-v1/;\n/ { soc { o: other { }; clk: clk { #clock-cells = <0>; }; dev { clocks = <&clk>; x = <2>; y = <&o>; }; gone { sub { }; }; }; };\n",
        );
        let d = compare(&a, &b, None);
        let t = &d.tree;
        assert_eq!(d.marks[t.find("/soc/clk").unwrap()], Mark::Same);
        // x differs, y is only in B; `clocks` matches despite the different phandle.
        assert_eq!(d.marks[t.find("/soc/dev").unwrap()], Mark::Changed(2));
        assert_eq!(d.marks[t.find("/soc/c").unwrap()], Mark::OnlyA);
        let gone = t.find("/soc/gone").unwrap();
        assert!(d.is_ghost(gone) && d.marks[gone] == Mark::OnlyB);
        assert!(t.find("/soc/gone/sub").is_some_and(|s| d.is_ghost(s)));
        assert!(t.find("/soc/other").is_some_and(|s| d.is_ghost(s)));
        assert_eq!((d.changed, d.only_a, d.only_b), (1, 1, 3));
        assert_eq!(d.in_subtree(t.find("/soc").unwrap()), 5);
        // A's ids are unchanged; ghosts come after.
        assert_eq!(d.a_len, a.len());
        assert_eq!(t.find("/soc/c"), a.find("/soc/c"));
        // Ghosts map to B; a whole removed subtree is one root.
        assert_eq!(d.map[gone], b.find("/soc/gone"));
        assert_eq!(d.roots(&d.tree, |m| *m == Mark::OnlyB).len(), 2);
        assert!(t.node(gone).props.is_empty());
    }

    #[test]
    fn values_side_by_side() {
        let a =
            tree("/dts-v1/;\n/ { n { reg = <0x0 0x1000 0x0 0x100>; s = \"a\", \"b c\"; }; };\n");
        let b = tree("/dts-v1/;\n/ { n { reg = <0x0 0x2000 0x0 0x100>; s = \"a\"; }; };\n");
        let (an, bn) = (a.find("/n").unwrap(), b.find("/n").unwrap());
        let prop = |t: &Tree, id, name| t.node(id).props.iter().find(|p| p.name == name).cloned();
        let (pa, pb) = (prop(&a, an, "reg"), prop(&b, bn, "reg"));
        let (ta, tb) = value_diff(&a, an, pa.as_ref(), &b, bn, pb.as_ref());
        let changed = |t: &[Token]| {
            t.iter()
                .filter(|t| t.changed)
                .map(|t| t.text.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(changed(&ta), ["0x1000"]);
        assert_eq!(changed(&tb), ["0x2000"]);
        assert_eq!(ta.len(), 6);
        let (pa, pb) = (prop(&a, an, "s"), prop(&b, bn, "s"));
        let (ta, tb) = value_diff(&a, an, pa.as_ref(), &b, bn, pb.as_ref());
        assert_eq!(changed(&ta), [",", "\"b c\""]);
        assert!(changed(&tb).is_empty());
        // Missing on one side: everything on the other side is changed.
        let (ta, tb) = value_diff(&a, an, pa.as_ref(), &b, bn, None);
        assert!(tb.is_empty() && ta.iter().all(|t| t.changed));
        assert!(
            !space_between("<", "0x0") && !space_between("0x0", ">") && space_between(",", "\"a\"")
        );
    }
}
