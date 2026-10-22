//! Overlays attached to the tree shown or to a DTB's source board (D-027): loading them
//! (`.dtbo` blobs or `.dtso` sources) and applying them in order.

use std::sync::Arc;

use timber_core::{Diagnostic, NodeId, Severity, Tree, dtb, overlay};

use crate::sources::SourceSet;
use crate::state::SourceLocation;

#[derive(Debug, Clone, PartialEq)]
pub enum OverlayFrom {
    /// A compiled `.dtbo`.
    Blob(Arc<Vec<u8>>),
    /// A `.dtso` in the sources VFS, compiled when applied.
    Source(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct OverlaySpec {
    pub name: String,
    pub from: OverlayFrom,
}

impl OverlaySpec {
    pub fn source(path: &str) -> Self {
        OverlaySpec {
            name: path.rsplit('/').next().unwrap_or(path).to_string(),
            from: OverlayFrom::Source(path.to_string()),
        }
    }
}

/// Something an overlay set or added, for the node panel.
#[derive(Debug, Clone, PartialEq)]
pub struct OverlayMark {
    pub node: NodeId,
    /// `None`: the overlay created or merged into the node itself.
    pub prop: Option<String>,
    pub overlay: String,
    /// Where in the overlay's source (for `.dtso` overlays).
    pub loc: Option<SourceLocation>,
    /// The overlay created this node (rather than changing an existing one).
    pub created: bool,
}

pub struct Overlaid {
    pub tree: Tree,
    pub marks: Vec<OverlayMark>,
    pub diagnostics: Vec<Diagnostic>,
}

fn problem(overlay: &str, message: String) -> Diagnostic {
    Diagnostic {
        severity: Severity::Error,
        file: overlay.to_string(),
        line: 0,
        col: 0,
        message,
        snippet: String::new(),
    }
}

/// Applies `specs` to `base` in order. An overlay that fails is reported and skipped; the
/// rest still apply.
pub fn apply_all(base: Tree, set: &SourceSet, specs: &[OverlaySpec]) -> Overlaid {
    let mut out = Overlaid {
        tree: base,
        marks: Vec::new(),
        diagnostics: Vec::new(),
    };
    for spec in specs {
        let (ov, source_map) = match &spec.from {
            OverlayFrom::Blob(data) => match dtb::parse(data) {
                Ok(t) => (t, None),
                Err(e) => {
                    out.diagnostics.push(problem(&spec.name, e.to_string()));
                    continue;
                }
            },
            OverlayFrom::Source(path) => {
                // Overlays are built with -@, as the kernel does, so their labels export.
                let compiled = set.compile_as(path, true);
                let failed = compiled.has_errors();
                out.diagnostics.extend(compiled.diagnostics);
                if failed {
                    out.diagnostics
                        .push(problem(&spec.name, "didn't compile; not applied".into()));
                    continue;
                }
                (compiled.tree, Some(compiled.source_map))
            }
        };
        let before = out.tree.len();
        match overlay::apply(&out.tree, &ov) {
            Ok(applied) => {
                for (node, prop, from) in applied.touched {
                    let loc = source_map.as_ref().and_then(|sm| {
                        let locs = match &prop {
                            Some(p) => sm.prop(from, p),
                            None => sm.node(from),
                        };
                        let l = locs.last()?;
                        Some(SourceLocation {
                            path: sm.file(l).to_string(),
                            line: l.line,
                            col: l.col,
                        })
                    });
                    out.marks.push(OverlayMark {
                        node,
                        created: node >= before,
                        prop,
                        overlay: spec.name.clone(),
                        loc,
                    });
                }
                out.tree = applied.tree;
            }
            Err(e) => out
                .diagnostics
                .push(problem(&spec.name, format!("not applied: {e}"))),
        }
    }
    out
}
