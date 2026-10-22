//! Source folders and files: the combined VFS, the board index, and annotating a blob
//! with the board it was built from (DESIGN §7a).

use std::sync::Arc;

use timber_core::bsp::{self, BoardInfo, Candidate};

use crate::overlays::{OverlayMark, OverlaySpec, apply_all};
use timber_core::dts::{self, Compiled};
pub use timber_core::folder::folder_filter;
use timber_core::kbuild::{self, Config, DtbTarget, Kbuild, kernel_devicetree};
use timber_core::source_map::SourceMap;
use timber_core::vfs::{IncludeResolver, MemVfs, Vfs};
use timber_core::{Diagnostic, NodeId, Tree};

/// Candidates compiled and scored when matching a blob to a board.
const MAX_SCORED: usize = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Folder,
    Files,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// Mount point in the combined VFS (and display name).
    pub name: String,
    pub kind: EntryKind,
    pub files: usize,
    pub boards: usize,
    /// Kernel architectures found (`arch/<arch>/boot/dts`).
    pub arches: Vec<String>,
}

/// Everything added in the Sources panel. Immutable; changes build a new set.
#[derive(Default)]
pub struct SourceSet {
    pub entries: Vec<Entry>,
    pub vfs: MemVfs,
    pub boards: Vec<BoardInfo>,
    /// `NAME=VALUE` preprocessor defines applied to every compile.
    pub defines: Vec<(String, String)>,
    /// A kernel config or Yocto `KERNEL_DEVICETREE`, deciding which boards are built.
    pub config: Option<ConfigSource>,
    /// What the dts Makefiles build (evaluated against `config` when there is one).
    pub kbuild: Kbuild,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConfigSource {
    Kernel { name: String, config: Config },
    Yocto { name: String, list: Vec<String> },
}

impl ConfigSource {
    /// A `.config`/defconfig, or a Yocto file that sets `KERNEL_DEVICETREE`.
    pub fn parse(name: &str, text: &str) -> Result<Self, String> {
        if let Some(list) = kernel_devicetree(text) {
            return Ok(ConfigSource::Yocto {
                name: name.to_string(),
                list,
            });
        }
        let config = Config::parse(text);
        if config.is_empty() {
            return Err(format!(
                "{name} has no CONFIG_ symbols or KERNEL_DEVICETREE"
            ));
        }
        Ok(ConfigSource::Kernel {
            name: name.to_string(),
            config,
        })
    }
}

/// Recognises kernel config and Yocto conf files among opened files.
pub fn is_config_file(name: &str) -> bool {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    base == ".config"
        || base.ends_with("defconfig")
        || base.starts_with("config-")
        || base.ends_with(".conf")
}

fn evaluate_kbuild(vfs: &MemVfs, config: Option<&ConfigSource>) -> Kbuild {
    match config {
        Some(ConfigSource::Kernel { config, .. }) => kbuild::evaluate(vfs, Some(config), None),
        Some(ConfigSource::Yocto { list, .. }) => kbuild::evaluate(vfs, None, Some(list)),
        None => kbuild::evaluate(vfs, None, None),
    }
}

fn is_board(path: &str) -> bool {
    path.ends_with(".dts") || path.ends_with(".dtso")
}

/// Parses `A=1, B, C=x` into defines (a bare name defines it as `1`).
pub fn parse_defines(text: &str) -> Vec<(String, String)> {
    text.split([',', ' ', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|d| match d.split_once('=') {
            Some((n, v)) => (n.trim().to_string(), v.trim().to_string()),
            None => (d.to_string(), "1".to_string()),
        })
        .filter(|(n, _)| !n.is_empty())
        .collect()
}

pub fn format_defines(defines: &[(String, String)]) -> String {
    defines
        .iter()
        .map(|(n, v)| {
            if v == "1" {
                n.clone()
            } else {
                format!("{n}={v}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

impl SourceSet {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn rebuild(
        vfs: MemVfs,
        entries: Vec<(String, EntryKind)>,
        defines: Vec<(String, String)>,
        config: Option<ConfigSource>,
    ) -> Self {
        let kbuild = evaluate_kbuild(&vfs, config.as_ref());
        let resolver = IncludeResolver::new(&vfs);
        let boards: Vec<BoardInfo> = vfs
            .paths()
            .filter(|p| is_board(p))
            .map(|p| bsp::scan_board_in(&vfs, &resolver, p))
            .collect();
        let entries = entries
            .into_iter()
            .map(|(name, kind)| {
                let prefix = format!("{name}/");
                let paths: Vec<&str> = vfs.paths().filter(|p| p.starts_with(&prefix)).collect();
                let mut arches: Vec<String> = paths
                    .iter()
                    .filter_map(|p| {
                        let i = p.find("arch/")?;
                        let (arch, tail) = p[i + 5..].split_once('/')?;
                        tail.starts_with("boot/dts/").then(|| arch.to_string())
                    })
                    .collect();
                arches.sort();
                arches.dedup();
                Entry {
                    files: paths.len(),
                    boards: paths.iter().filter(|p| is_board(p)).count(),
                    name,
                    kind,
                    arches,
                }
            })
            .collect();
        SourceSet {
            entries,
            vfs,
            boards,
            defines,
            config,
            kbuild,
        }
    }

    fn entry_list(&self) -> Vec<(String, EntryKind)> {
        self.entries
            .iter()
            .map(|e| (e.name.clone(), e.kind))
            .collect()
    }

    /// Adds a folder (paths relative to the folder's parent, so they start with its name) or
    /// loose files (mounted under `files/`). Returns the new set and the mount name.
    pub fn with_files(&self, kind: EntryKind, files: Vec<(String, Vec<u8>)>) -> (Self, String) {
        let mut entries = self.entry_list();
        let mut vfs = self.vfs.clone();
        let name = match kind {
            EntryKind::Files => "files".to_string(),
            EntryKind::Folder => {
                let base = files
                    .first()
                    .and_then(|(p, _)| p.split('/').next())
                    .unwrap_or("folder")
                    .to_string();
                let mut name = base.clone();
                let mut n = 2;
                while entries.iter().any(|(e, _)| *e == name) || name == "files" {
                    name = format!("{base}-{n}");
                    n += 1;
                }
                name
            }
        };
        for (path, data) in files {
            let rel = match kind {
                // Re-root under the (possibly de-duplicated) mount name.
                EntryKind::Folder => path
                    .split_once('/')
                    .map_or(path.as_str(), |(_, r)| r)
                    .to_string(),
                EntryKind::Files => path.rsplit('/').next().unwrap_or(&path).to_string(),
            };
            vfs.insert(&format!("{name}/{rel}"), data);
        }
        if !entries.iter().any(|(e, _)| *e == name) {
            entries.push((name.clone(), kind));
        }
        (
            Self::rebuild(vfs, entries, self.defines.clone(), self.config.clone()),
            name,
        )
    }

    /// Replaces the files of an existing folder entry (a reload), keeping its mount name
    /// and position.
    pub fn with_folder_reloaded(&self, name: &str, files: Vec<(String, Vec<u8>)>) -> Self {
        let mut vfs = self.vfs.clone();
        let prefix = format!("{name}/");
        vfs.retain(|p| !p.starts_with(&prefix));
        for (path, data) in files {
            let rel = path.split_once('/').map_or(path.as_str(), |(_, r)| r);
            vfs.insert(&format!("{name}/{rel}"), data);
        }
        Self::rebuild(
            vfs,
            self.entry_list(),
            self.defines.clone(),
            self.config.clone(),
        )
    }

    pub fn without(&self, name: &str) -> Self {
        let mut vfs = self.vfs.clone();
        let prefix = format!("{name}/");
        vfs.retain(|p| !p.starts_with(&prefix));
        let entries = self
            .entry_list()
            .into_iter()
            .filter(|(e, _)| e != name)
            .collect();
        Self::rebuild(vfs, entries, self.defines.clone(), self.config.clone())
    }

    pub fn with_defines(&self, defines: Vec<(String, String)>) -> Self {
        SourceSet {
            entries: self.entries.clone(),
            vfs: self.vfs.clone(),
            boards: self.boards.clone(),
            defines,
            config: self.config.clone(),
            kbuild: self.kbuild.clone(),
        }
    }

    pub fn with_config(&self, config: Option<ConfigSource>) -> Self {
        SourceSet {
            entries: self.entries.clone(),
            vfs: self.vfs.clone(),
            boards: self.boards.clone(),
            defines: self.defines.clone(),
            kbuild: evaluate_kbuild(&self.vfs, config.as_ref()),
            config,
        }
    }

    /// The Makefile target that builds this board source, if any.
    pub fn target(&self, board: &str) -> Option<&DtbTarget> {
        self.kbuild.for_source(board)
    }

    /// Whether every source a Makefile target needs is present (a composite's base and
    /// overlays included).
    pub fn buildable(&self, target: &DtbTarget) -> bool {
        let source_of = |p: &str| match p.strip_suffix(".dtbo") {
            Some(b) => format!("{b}.dtso"),
            None => format!("{}.dts", p.trim_end_matches(".dtb")),
        };
        match &target.composite {
            Some(parts) => parts.iter().all(|p| self.vfs.exists(&source_of(p))),
            None => self.vfs.exists(&target.source),
        }
    }

    /// Whether the config builds this board: `Some(true/false)` with a config, else `None`.
    pub fn built(&self, board: &str) -> Option<bool> {
        self.target(board)
            .and_then(|t| t.built)
            .or(self.config.as_ref().map(|_| false))
    }

    /// Compiles a board; `symbols` mirrors dtc's `-@` (for blobs that carry `__symbols__`).
    pub fn compile_as(&self, path: &str, symbols: bool) -> Compiled {
        let defines: Vec<(&str, &str)> = self
            .defines
            .iter()
            .map(|(n, v)| (n.as_str(), v.as_str()))
            .collect();
        dts::compile_with(
            &self.vfs,
            path,
            &dts::Options {
                defines: &defines,
                symbols,
            },
        )
    }

    pub fn board(&self, path: &str) -> Option<&BoardInfo> {
        self.boards.iter().find(|b| b.path == path)
    }
}

/// A blob's link to the source board it was (most likely) built from.
pub struct SourceLink {
    pub board: String,
    /// Why it was picked, e.g. "file name + root compatible identical".
    pub why: String,
    /// Content score, 0–1.
    pub confidence: f64,
    pub manual: bool,
    pub tree: Tree,
    pub source_map: SourceMap,
    /// Blob node → source node at the same path.
    pub map: Vec<Option<NodeId>>,
    pub diagnostics: Vec<Diagnostic>,
    /// What the "applied at boot" overlays set on the source board.
    pub marks: Vec<OverlayMark>,
}

/// Finds and compiles the source board for `blob`. `forced` is a board path chosen by the
/// user, which skips matching.
/// `boot` overlays are applied to the source board before comparing (D-027).
pub fn annotate(
    blob: &Tree,
    blob_name: &str,
    set: &SourceSet,
    forced: Option<&str>,
    boot: &[OverlaySpec],
) -> Option<SourceLink> {
    let candidates: Vec<Candidate> = match forced {
        Some(path) => {
            let index = set.boards.iter().position(|b| b.path == path)?;
            vec![Candidate {
                index,
                file_name: false,
                reason: None,
            }]
        }
        None => {
            let compatible = blob.prop_strings(Tree::ROOT, "compatible");
            let model = blob.prop_str(Tree::ROOT, "model");
            let mut list = bsp::shortlist(&set.boards, blob_name, &compatible, model);
            // Among equally good matches, prefer boards the config builds.
            list.sort_by_key(|c| {
                (
                    !c.file_name,
                    std::cmp::Reverse(c.reason),
                    set.built(&set.boards[c.index].path) != Some(true),
                )
            });
            // A file-name match that also agrees on compatible needs no runner-up.
            let certain = list.first().is_some_and(|c| {
                c.file_name
                    && matches!(
                        c.reason,
                        Some(bsp::MatchReason::Exact | bsp::MatchReason::First)
                    )
            });
            list.truncate(if certain { 1 } else { MAX_SCORED });
            list
        }
    };
    // A blob built with dtc -@ keeps labelled /omit-if-no-ref/ nodes; compile the same way.
    // Overlays need the base's __symbols__ too.
    let symbols = blob.child_by_name(Tree::ROOT, "__symbols__").is_some() || !boot.is_empty();
    let (candidate, compiled, confidence) = candidates
        .into_iter()
        .map(|c| {
            let mut compiled = set.compile_as(&set.boards[c.index].path, symbols);
            let mut marks = Vec::new();
            if !boot.is_empty() {
                let overlaid = apply_all(std::mem::take(&mut compiled.tree), set, boot);
                compiled.tree = overlaid.tree;
                compiled.diagnostics.extend(overlaid.diagnostics);
                marks = overlaid.marks;
            }
            let score = bsp::content_score(blob, &compiled.tree);
            (c, (compiled, marks), score)
        })
        .max_by(|a, b| a.2.total_cmp(&b.2))?;
    let (compiled, marks) = compiled;
    let map = bsp::map_by_path(blob, &compiled.tree);
    Some(SourceLink {
        board: set.boards[candidate.index].path.clone(),
        why: candidate.describe(),
        confidence,
        manual: forced.is_some(),
        map,
        tree: compiled.tree,
        source_map: compiled.source_map,
        diagnostics: compiled.diagnostics,
        marks,
    })
}

pub type SharedSources = Arc<SourceSet>;

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(files: &[(&str, &str)]) -> Vec<(String, Vec<u8>)> {
        files
            .iter()
            .map(|(p, t)| (p.to_string(), t.as_bytes().to_vec()))
            .collect()
    }

    #[test]
    fn defines_round_trip() {
        let d = parse_defines("BOARD_REV=2, FEATURE_X  Y=a");
        assert_eq!(
            d,
            [
                ("BOARD_REV".into(), "2".into()),
                ("FEATURE_X".into(), "1".into()),
                ("Y".into(), "a".into())
            ]
        );
        assert_eq!(format_defines(&d), "BOARD_REV=2, FEATURE_X, Y=a");
    }

    #[test]
    fn folders_mount_boards_and_annotate_a_blob() {
        let soc =
            "/ { compatible = \"acme,soc\"; soc { uart: serial { status = \"disabled\"; }; }; };\n";
        let board = "/dts-v1/;\n#include \"soc.dtsi\"\n#if REV > 1\n/ { rev2; };\n#endif\n\
                     / { model = \"Acme GW\"; compatible = \"acme,gw\", \"acme,soc\"; };\n&uart { status = \"okay\"; };\n";
        let set = SourceSet::default();
        let (set, name) = set.with_files(
            EntryKind::Folder,
            folder(&[("bsp/soc.dtsi", soc), ("bsp/acme-gw.dts", board)]),
        );
        assert_eq!(name, "bsp");
        assert_eq!(set.entries[0].boards, 1);
        assert_eq!(set.boards[0].compatible, ["acme,gw", "acme,soc"]);

        // Same folder name again is mounted separately.
        let (set2, name2) = set.with_files(EntryKind::Folder, folder(&[("bsp/x.dtsi", "")]));
        assert_eq!(name2, "bsp-2");
        assert_eq!(set2.without("bsp-2").entries.len(), 1);

        // A blob built with REV=2 and patched at boot.
        let set = set.with_defines(parse_defines("REV=2"));
        let mut blob = set.compile_as("bsp/acme-gw.dts", false).tree;
        assert!(blob.prop(Tree::ROOT, "rev2").is_some());
        let uart = blob.find("/soc/serial").unwrap();
        blob.node_mut(uart)
            .set_prop("status", b"disabled\0".to_vec());

        let link = annotate(&blob, "acme-gw.dtb", &set, None, &[]).expect("matched");
        assert_eq!(link.board, "bsp/acme-gw.dts");
        assert!(link.why.starts_with("file name"));
        assert!(link.confidence > 0.8 && link.confidence < 1.0);
        let src = link.map[uart].unwrap();
        let cmp = bsp::compare_node(&blob, uart, &link.tree, src);
        assert_eq!(cmp.differs, ["status"]);
        let locs = link.source_map.prop(src, "status");
        assert_eq!(
            locs.len(),
            2,
            "defined in soc.dtsi, overridden in the board"
        );
    }
}
