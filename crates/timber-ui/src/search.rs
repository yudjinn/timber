//! Matching and ranking for the global search and the child popup filter.

use timber_core::{NodeId, Tree};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MatchKind {
    Name,
    Label,
    Alias,
    Compatible,
}

impl MatchKind {
    pub fn chip(self) -> &'static str {
        match self {
            MatchKind::Name => "name",
            MatchKind::Label => "label",
            MatchKind::Alias => "alias",
            MatchKind::Compatible => "compatible",
        }
    }
}

/// How well a query matched: lower is better.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Quality {
    Exact,
    Prefix,
    WordStart,
    Substring,
    Fuzzy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub id: NodeId,
    pub kind: MatchKind,
    pub quality: Quality,
    /// The text that matched (node name, label, alias, or compatible string).
    pub text: String,
    /// Character indices in `text` to highlight.
    pub positions: Vec<usize>,
}

/// Scores `text` against a lowercase query.
pub fn score(text: &str, query: &str) -> Option<(Quality, Vec<usize>)> {
    if query.is_empty() {
        return None;
    }
    let lower = text.to_lowercase();
    let qlen = query.chars().count();
    let span = |start_byte: usize| {
        let start = lower[..start_byte].chars().count();
        (start..start + qlen).collect::<Vec<_>>()
    };
    if lower == query {
        return Some((Quality::Exact, span(0)));
    }
    if lower.starts_with(query) {
        return Some((Quality::Prefix, span(0)));
    }
    let mut first_sub = None;
    for (i, _) in lower.match_indices(query) {
        let at_word = lower[..i]
            .chars()
            .last()
            .is_some_and(|c| matches!(c, ',' | '-' | '_' | '@' | '/' | '.'));
        if at_word {
            return Some((Quality::WordStart, span(i)));
        }
        first_sub.get_or_insert(i);
    }
    if let Some(i) = first_sub {
        return Some((Quality::Substring, span(i)));
    }
    // Fuzzy: the query's characters in order.
    let mut positions = Vec::new();
    let mut q = query.chars().peekable();
    for (i, c) in lower.chars().enumerate() {
        if q.peek() == Some(&c) {
            positions.push(i);
            q.next();
        }
    }
    q.peek().is_none().then_some((Quality::Fuzzy, positions))
}

/// Best match of `query` against a node's name, labels, and compatible strings.
pub fn match_node(tree: &Tree, id: NodeId, query: &str) -> Option<Hit> {
    let node = tree.node(id);
    let mut best: Option<Hit> = None;
    let mut consider = |kind: MatchKind, text: &str| {
        if let Some((quality, positions)) = score(text, query) {
            let better = best
                .as_ref()
                .is_none_or(|b| (quality, kind) < (b.quality, b.kind));
            if better {
                best = Some(Hit {
                    id,
                    kind,
                    quality,
                    text: text.to_string(),
                    positions,
                });
            }
        }
    };
    consider(MatchKind::Name, &node.name);
    for l in &node.labels {
        consider(MatchKind::Label, l);
    }
    for c in tree.prop_strings(id, "compatible") {
        consider(MatchKind::Compatible, c);
    }
    best
}

/// Whole-tree search across names, labels, aliases, and compatible strings.
#[derive(Clone)]
pub struct SearchIndex {
    aliases: Vec<(String, NodeId)>,
    depth: Vec<usize>,
}

impl SearchIndex {
    pub fn new(tree: &Tree) -> Self {
        let mut aliases = Vec::new();
        if let Some(a) = tree.child_by_name(Tree::ROOT, "aliases") {
            for p in &tree.node(a).props {
                if let Some(target) = tree.prop_str(a, &p.name).and_then(|path| tree.find(path)) {
                    aliases.push((p.name.clone(), target));
                }
            }
        }
        let depth = (0..tree.len()).map(|id| tree.depth(id)).collect();
        SearchIndex { aliases, depth }
    }

    pub fn search(&self, tree: &Tree, query: &str, limit: usize) -> Vec<Hit> {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return Vec::new();
        }
        let mut hits: Vec<Hit> = (0..tree.len())
            .filter_map(|id| match_node(tree, id, &query))
            .collect();
        for (alias, target) in &self.aliases {
            if let Some((quality, positions)) = score(alias, &query) {
                let hit = Hit {
                    id: *target,
                    kind: MatchKind::Alias,
                    quality,
                    text: alias.clone(),
                    positions,
                };
                match hits.iter_mut().find(|h| h.id == *target) {
                    Some(h) if (quality, MatchKind::Alias) < (h.quality, h.kind) => *h = hit,
                    Some(_) => {}
                    None => hits.push(hit),
                }
            }
        }
        hits.sort_by(|a, b| {
            (a.quality, self.depth[a.id], a.text.len(), a.id).cmp(&(
                b.quality,
                self.depth[b.id],
                b.text.len(),
                b.id,
            ))
        });
        hits.truncate(limit);
        hits
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> Tree {
        let c = timber_core::dts::compile_str(
            "t.dts",
            r#"/dts-v1/;
            / {
                aliases { serial2 = &uart2; mmc0 = &sdhci; };
                soc {
                    uart2: serial@fe660000 { compatible = "rockchip,rk3568-uart", "snps,dw-apb-uart"; };
                    sdhci: mmc@fe310000 { compatible = "rockchip,rk3568-dwcmshc"; };
                    i2c0: i2c@fdd40000 { pmic@20 { compatible = "rockchip,rk809"; }; };
                };
            };"#,
        );
        c.tree
    }

    #[test]
    fn scoring() {
        assert_eq!(score("serial", "serial").unwrap().0, Quality::Exact);
        assert_eq!(
            score("serial@fe660000", "serial").unwrap().0,
            Quality::Prefix
        );
        assert_eq!(
            score("rockchip,rk3568-uart", "rk3568").unwrap(),
            (Quality::WordStart, (9..15).collect())
        );
        assert_eq!(
            score("serial@fe660000", "rial").unwrap().0,
            Quality::Substring
        );
        assert_eq!(
            score("serial@fe660000", "srl").unwrap(),
            (Quality::Fuzzy, vec![0, 2, 5])
        );
        assert!(score("serial", "xyz").is_none());
    }

    #[test]
    fn search_ranks_and_tags_matches() {
        let t = tree();
        let idx = SearchIndex::new(&t);
        let hits = idx.search(&t, "uart2", 10);
        assert_eq!(hits[0].kind, MatchKind::Label);
        assert_eq!(t.path(hits[0].id), "/soc/serial@fe660000");

        let hits = idx.search(&t, "mmc0", 10);
        assert_eq!(
            (hits[0].kind, t.path(hits[0].id).as_str()),
            (MatchKind::Alias, "/soc/mmc@fe310000")
        );

        let hits = idx.search(&t, "rk809", 10);
        assert_eq!(
            (hits[0].kind, t.path(hits[0].id).as_str()),
            (MatchKind::Compatible, "/soc/i2c@fdd40000/pmic@20")
        );

        // Names beat compatibles at equal quality; one hit per node.
        let hits = idx.search(&t, "serial", 10);
        assert_eq!(hits.iter().filter(|h| h.id == hits[0].id).count(), 1);
        assert_eq!(hits[0].kind, MatchKind::Name);
    }
}
