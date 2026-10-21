//! Device tree source compiler: preprocesses, parses, merges, and resolves references
//! following dtc's semantics, recording where every node and property came from.

use std::collections::{HashMap, HashSet};

use crate::lex::{PError, PResult, Src, is_ident_char, is_ident_start};
use crate::preprocess::preprocess;
use crate::source_map::{LocKind, NodeSources, SourceLoc, SourceMap};
use crate::vfs::{MemVfs, Vfs};
use crate::{Diagnostic, NodeId, Property, Severity, Tree};

const MAX_ERRORS: usize = 100;

pub struct Compiled {
    pub tree: Tree,
    pub source_map: SourceMap,
    /// Preprocessor and compiler messages. The tree is best-effort when errors exist.
    pub diagnostics: Vec<Diagnostic>,
    /// The source declared `/plugin/` (an overlay).
    pub plugin: bool,
    /// Every file the preprocessor read, in first-include order (what to watch for changes).
    pub inputs: Vec<String>,
}

impl Compiled {
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error)
    }

    pub fn errors(&self) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
    }
}

/// Compiles the DTS at `main` (a path in `vfs`); includes resolve against the same `vfs`.
pub fn compile(vfs: &dyn Vfs, main: &str) -> Compiled {
    compile_with_defines(vfs, main, &[])
}

/// Like [`compile`], with preprocessor defines (`-D NAME=VALUE`) as the build passes them.
pub fn compile_with_defines(vfs: &dyn Vfs, main: &str, defines: &[(&str, &str)]) -> Compiled {
    let pre = preprocess(vfs, main, defines);
    let mut compiled = compile_preprocessed(&pre.text);
    let mut diagnostics = pre.diagnostics;
    diagnostics.append(&mut compiled.diagnostics);
    compiled.diagnostics = diagnostics;
    compiled.inputs = pre.files;
    compiled
}

/// Compiles a single self-contained DTS text.
pub fn compile_str(name: &str, text: &str) -> Compiled {
    compile(
        &MemVfs::new().with(name, text),
        &crate::vfs::normalize(name),
    )
}

/// Compiles already-preprocessed text (which may contain `# line "file"` markers).
pub fn compile_preprocessed(text: &str) -> Compiled {
    let mut p = Parser {
        s: Src::new(text),
        root: DNode::default(),
        plugin: false,
        fragments: 0,
        mem_reserve: Vec::new(),
        diags: Vec::new(),
        files: Vec::new(),
        file_ids: HashMap::new(),
        deleted: Vec::new(),
    };
    p.top();
    p.finish()
}

// ---- intermediate tree ----

#[derive(Debug, Clone)]
enum Ref {
    Label(String),
    Path(String),
}

impl Ref {
    fn display(&self) -> String {
        match self {
            Ref::Label(l) => format!("&{l}"),
            Ref::Path(p) => format!("&{{{p}}}"),
        }
    }
}

#[derive(Debug, Clone)]
enum Chunk {
    Bytes(Vec<u8>),
    /// A 32-bit phandle cell referring to a node; the offset is where it was written.
    Phandle(Ref, usize),
    /// A NUL-terminated path string referring to a node.
    Path(Ref, usize),
}

#[derive(Debug, Clone)]
struct DProp {
    name: String,
    chunks: Vec<Chunk>,
    locs: Vec<SourceLoc>,
    /// Deleted entries stay in place so a later re-add keeps the original position, as
    /// in dtc.
    deleted: bool,
}

#[derive(Debug, Clone, Default)]
struct DNode {
    name: String,
    labels: Vec<String>,
    props: Vec<DProp>,
    children: Vec<DNode>,
    locs: Vec<SourceLoc>,
    omit_if_no_ref: bool,
    /// See [`DProp::deleted`].
    deleted: bool,
}

impl DNode {
    /// dtc's `delete_node`: marks the node and everything under it deleted and drops labels.
    fn delete(&mut self) {
        self.deleted = true;
        self.labels.clear();
        self.props.iter_mut().for_each(|p| p.deleted = true);
        self.children.iter_mut().for_each(DNode::delete);
    }
}

enum Item {
    Prop(DProp),
    Node(NodeDef),
    DelNode(String, SourceLoc),
    DelProp(String, SourceLoc),
}

struct NodeDef {
    name: String,
    labels: Vec<String>,
    items: Vec<Item>,
    loc: SourceLoc,
    omit_if_no_ref: bool,
}

struct Parser<'a> {
    s: Src<'a>,
    root: DNode,
    plugin: bool,
    fragments: u32,
    mem_reserve: Vec<(u64, u64)>,
    diags: Vec<Diagnostic>,
    files: Vec<String>,
    file_ids: HashMap<String, u32>,
    deleted: Vec<(String, SourceLoc)>,
}

fn is_name_char(c: u8) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(
            c,
            b',' | b'.' | b'_' | b'+' | b'*' | b'#' | b'?' | b'@' | b'-'
        )
}

impl Parser<'_> {
    // ---- diagnostics and locations ----

    fn source_loc(&mut self, pos: usize, kind: LocKind) -> SourceLoc {
        let loc = self.s.loc_at(pos);
        let file = match self.file_ids.get(&loc.file) {
            Some(&id) => id,
            None => {
                let id = self.files.len() as u32;
                self.files.push(loc.file.clone());
                self.file_ids.insert(loc.file, id);
                id
            }
        };
        SourceLoc {
            file,
            line: loc.line,
            col: loc.col,
            kind,
        }
    }

    fn report(&mut self, severity: Severity, pos: usize, message: String) {
        if self.diags.len() >= MAX_ERRORS {
            return;
        }
        let loc = self.s.loc_at(pos);
        let snippet = self.s.line_text(pos);
        self.diags.push(Diagnostic {
            severity,
            file: loc.file,
            line: loc.line,
            col: loc.col,
            message,
            snippet,
        });
    }

    fn error(&mut self, e: PError) {
        self.report(Severity::Error, e.pos, e.msg);
    }

    fn too_many_errors(&self) -> bool {
        self.diags.len() >= MAX_ERRORS
    }

    /// Skips to the end of the current statement: past the next `;` at this nesting level,
    /// or up to (not past) a `}` that closes the enclosing block.
    fn recover(&mut self) {
        let mut depth = 0usize;
        while let Some(c) = self.s.peek() {
            match c {
                b'"' => {
                    self.s.pos += 1;
                    while let Some(c) = self.s.bump() {
                        match c {
                            b'\\' => {
                                self.s.bump();
                            }
                            b'"' | b'\n' => break,
                            _ => {}
                        }
                    }
                    continue;
                }
                b'{' => depth += 1,
                b'}' if depth == 0 => return,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        self.s.pos += 1;
                        self.s.skip_ws();
                        if self.s.peek() == Some(b';') {
                            self.s.pos += 1;
                        }
                        return;
                    }
                }
                b';' if depth == 0 => {
                    self.s.pos += 1;
                    return;
                }
                _ => {}
            }
            self.s.pos += 1;
        }
    }

    // ---- grammar ----

    fn top(&mut self) {
        loop {
            self.s.skip_ws();
            if self.s.eof() || self.too_many_errors() {
                return;
            }
            let start = self.s.pos;
            if let Err(e) = self.top_item() {
                self.error(e);
                if self.s.pos == start {
                    self.s.pos += 1;
                }
                self.recover();
            }
        }
    }

    fn top_item(&mut self) -> PResult<()> {
        if self.s.eat("/dts-v1/") {
            return self.s.expect(b';');
        }
        if self.s.eat("/plugin/") {
            self.plugin = true;
            return self.s.expect(b';');
        }
        let labels = self.labels();
        self.s.skip_ws();
        let start = self.s.pos;
        if self.s.eat("/memreserve/") {
            let addr = self.s.primary()?;
            let size = self.s.primary()?;
            self.mem_reserve.push((addr, size));
            return self.s.expect(b';');
        }
        if self.s.eat("/delete-node/") {
            self.s.skip_ws();
            let r = self.reference()?;
            self.s.expect(b';')?;
            let loc = self.source_loc(start, LocKind::Deleted);
            match self.find_ref(&r) {
                Some(idx) if !idx.is_empty() => {
                    let path = path_of(&self.root, &idx);
                    node_at_mut(&mut self.root, &idx).delete();
                    self.deleted.push((path, loc));
                }
                Some(_) => {
                    // The statement is fully parsed; report without skipping the next one.
                    self.report(Severity::Error, start, "cannot delete the root node".into());
                    return Ok(());
                }
                None => {
                    // The statement is fully parsed; report without skipping the next one.
                    self.report(
                        Severity::Error,
                        start,
                        format!("/delete-node/: no node {}", r.display()),
                    );
                    return Ok(());
                }
            }
            return Ok(());
        }
        if self.s.eat("/omit-if-no-ref/") {
            self.s.skip_ws();
            let r = self.reference()?;
            self.s.expect(b';')?;
            match self.find_ref(&r) {
                Some(idx) => node_at_mut(&mut self.root, &idx).omit_if_no_ref = true,
                None => {
                    // The statement is fully parsed; report without skipping the next one.
                    self.report(
                        Severity::Error,
                        start,
                        format!("/omit-if-no-ref/: no node {}", r.display()),
                    );
                    return Ok(());
                }
            }
            return Ok(());
        }
        match self.s.peek() {
            Some(b'/') => {
                self.s.pos += 1;
                self.s.expect(b'{')?;
                let items = self.body()?;
                self.s.expect(b';')?;
                let loc = self.source_loc(start, LocKind::Defined);
                let mut deleted = std::mem::take(&mut self.deleted);
                let root = &mut self.root;
                let kind = if root.locs.is_empty() {
                    LocKind::Defined
                } else {
                    LocKind::Overridden
                };
                root.locs.push(SourceLoc { kind, ..loc });
                union_labels(&mut root.labels, labels);
                apply(root, items, &mut deleted, "");
                self.deleted = deleted;
                Ok(())
            }
            Some(b'&') => {
                let r = self.reference()?;
                self.s.expect(b'{')?;
                let items = self.body()?;
                self.s.expect(b';')?;
                let loc = self.source_loc(start, LocKind::Overridden);
                // In an overlay, `&{/path}` always becomes a fragment; `&label` merges into
                // the overlay's own node if an earlier fragment defined it (as dtc does).
                let found = match r {
                    Ref::Path(_) if self.plugin => None,
                    _ => self.find_ref(&r),
                };
                if found.is_none() && self.plugin {
                    self.fragment(r, items, loc, start);
                    return Ok(());
                }
                let Some(idx) = found else {
                    // The statement is fully parsed; report without skipping the next one.
                    self.report(
                        Severity::Error,
                        start,
                        format!("label or path {} not found", r.display()),
                    );
                    return Ok(());
                };
                let path = path_of(&self.root, &idx);
                let mut deleted = std::mem::take(&mut self.deleted);
                let node = node_at_mut(&mut self.root, &idx);
                node.locs.push(loc);
                union_labels(&mut node.labels, labels);
                apply(node, items, &mut deleted, &path);
                self.deleted = deleted;
                Ok(())
            }
            _ => Err(self.s.err("expected '/ {', '&label {', or a directive")),
        }
    }

    /// In `/plugin/` mode, `&target { ... }` becomes `fragment@N { target...; __overlay__ { ... } }`.
    fn fragment(&mut self, r: Ref, items: Vec<Item>, loc: SourceLoc, pos: usize) {
        let n = self.fragments;
        self.fragments += 1;
        let target = match r {
            Ref::Label(_) => DProp {
                name: "target".into(),
                chunks: vec![Chunk::Phandle(r, pos)],
                locs: vec![loc],
                deleted: false,
            },
            Ref::Path(p) => {
                let mut v = p.into_bytes();
                v.push(0);
                DProp {
                    name: "target-path".into(),
                    chunks: vec![Chunk::Bytes(v)],
                    locs: vec![loc],
                    deleted: false,
                }
            }
        };
        let mut overlay = DNode {
            name: "__overlay__".into(),
            locs: vec![loc],
            ..DNode::default()
        };
        let mut deleted = std::mem::take(&mut self.deleted);
        apply(&mut overlay, items, &mut deleted, "");
        self.deleted = deleted;
        self.root.children.push(DNode {
            name: format!("fragment@{n}"),
            props: vec![target],
            children: vec![overlay],
            locs: vec![loc],
            ..DNode::default()
        });
    }

    /// Parses node contents after `{`, through the closing `}`.
    fn body(&mut self) -> PResult<Vec<Item>> {
        let mut items = Vec::new();
        loop {
            self.s.skip_ws();
            match self.s.peek() {
                Some(b'}') => {
                    self.s.pos += 1;
                    return Ok(items);
                }
                None => return Err(self.s.err("unexpected end of input: missing '}'")),
                _ => {}
            }
            let start = self.s.pos;
            match self.body_item() {
                Ok(item) => items.push(item),
                Err(e) => {
                    self.error(e);
                    if self.too_many_errors() {
                        return Err(self.s.err("too many errors"));
                    }
                    if self.s.pos == start {
                        self.s.pos += 1;
                    }
                    self.recover();
                }
            }
        }
    }

    fn body_item(&mut self) -> PResult<Item> {
        let start = self.s.pos;
        if self.s.eat("/delete-node/") {
            self.s.skip_ws();
            let name = self.name()?;
            self.s.expect(b';')?;
            return Ok(Item::DelNode(
                name,
                self.source_loc(start, LocKind::Deleted),
            ));
        }
        if self.s.eat("/delete-property/") {
            self.s.skip_ws();
            let name = self.name()?;
            self.s.expect(b';')?;
            return Ok(Item::DelProp(
                name,
                self.source_loc(start, LocKind::Deleted),
            ));
        }
        let omit = self.s.eat("/omit-if-no-ref/");
        let labels = self.labels();
        self.s.skip_ws();
        let name_pos = self.s.pos;
        let name = self.name()?;
        self.s.skip_ws();
        match self.s.peek() {
            Some(b'{') => {
                self.s.pos += 1;
                let items = self.body()?;
                self.s.expect(b';')?;
                let loc = self.source_loc(name_pos, LocKind::Defined);
                Ok(Item::Node(NodeDef {
                    name,
                    labels,
                    items,
                    loc,
                    omit_if_no_ref: omit,
                }))
            }
            Some(b'=') => {
                self.s.pos += 1;
                let chunks = self.values()?;
                let loc = self.source_loc(name_pos, LocKind::Defined);
                Ok(Item::Prop(DProp {
                    name,
                    chunks,
                    locs: vec![loc],
                    deleted: false,
                }))
            }
            Some(b';') => {
                self.s.pos += 1;
                let loc = self.source_loc(name_pos, LocKind::Defined);
                Ok(Item::Prop(DProp {
                    name,
                    chunks: Vec::new(),
                    locs: vec![loc],
                    deleted: false,
                }))
            }
            _ => Err(self
                .s
                .err(format!("expected '{{', '=', or ';' after `{name}`"))),
        }
    }

    /// Reads any `label:` prefixes.
    fn labels(&mut self) -> Vec<String> {
        let mut labels = Vec::new();
        loop {
            self.s.skip_ws();
            let save = self.s.pos;
            if let Some(id) = self.s.ident()
                && self.s.peek() == Some(b':')
            {
                self.s.pos += 1;
                labels.push(id.to_string());
                continue;
            }
            self.s.pos = save;
            return labels;
        }
    }

    fn name(&mut self) -> PResult<String> {
        let start = self.s.pos;
        while self.s.peek().is_some_and(is_name_char) {
            self.s.pos += 1;
        }
        if self.s.pos == start {
            return Err(self.s.err("expected a node or property name"));
        }
        Ok(String::from_utf8_lossy(&self.s.b[start..self.s.pos]).into_owned())
    }

    /// Parses `&label` or `&{/path}`.
    fn reference(&mut self) -> PResult<Ref> {
        if self.s.peek() != Some(b'&') {
            return Err(self.s.err("expected '&label' or '&{/path}'"));
        }
        self.s.pos += 1;
        if self.s.peek() == Some(b'{') {
            let start = self.s.pos + 1;
            let end = self.s.b[start..]
                .iter()
                .position(|&c| c == b'}' || c == b'\n')
                .map(|p| start + p)
                .filter(|&e| self.s.b[e] == b'}')
                .ok_or_else(|| self.s.err("unterminated &{path}"))?;
            self.s.pos = end + 1;
            return Ok(Ref::Path(
                String::from_utf8_lossy(&self.s.b[start..end]).into_owned(),
            ));
        }
        let start = self.s.pos;
        while self.s.peek().is_some_and(is_ident_char) {
            self.s.pos += 1;
        }
        if self.s.pos == start {
            return Err(self.s.err("expected a label after '&'"));
        }
        Ok(Ref::Label(
            String::from_utf8_lossy(&self.s.b[start..self.s.pos]).into_owned(),
        ))
    }

    /// Parses a property value list after `=`, through the terminating `;`.
    fn values(&mut self) -> PResult<Vec<Chunk>> {
        let mut chunks = Vec::new();
        loop {
            self.labels();
            self.s.skip_ws();
            match self.s.peek() {
                Some(b'"') => chunks.push(Chunk::Bytes(self.string()?)),
                Some(b'<') => {
                    self.s.pos += 1;
                    self.cells(32, &mut chunks)?;
                }
                Some(b'[') => {
                    self.s.pos += 1;
                    chunks.push(Chunk::Bytes(self.bytestring()?));
                }
                Some(b'&') => {
                    let pos = self.s.pos;
                    let r = self.reference()?;
                    chunks.push(Chunk::Path(r, pos));
                }
                Some(b'/') if self.s.eat("/bits/") => {
                    self.s.skip_ws();
                    let pos = self.s.pos;
                    let bits = self.s.primary()?;
                    if !matches!(bits, 8 | 16 | 32 | 64) {
                        return Err(PError {
                            pos,
                            msg: "/bits/ must be 8, 16, 32, or 64".into(),
                        });
                    }
                    self.s.skip_ws();
                    if self.s.peek() != Some(b'<') {
                        return Err(self.s.err("expected '<' after /bits/"));
                    }
                    self.s.pos += 1;
                    self.cells(bits as u32, &mut chunks)?;
                }
                _ => return Err(self.s.err("expected a property value")),
            }
            self.labels();
            self.s.skip_ws();
            match self.s.peek() {
                Some(b',') => self.s.pos += 1,
                Some(b';') => {
                    self.s.pos += 1;
                    return Ok(chunks);
                }
                _ => return Err(self.s.err("expected ',' or ';' after property value")),
            }
        }
    }

    /// Parses cells after `<`, through `>`.
    fn cells(&mut self, bits: u32, chunks: &mut Vec<Chunk>) -> PResult<()> {
        let mut buf = Vec::new();
        loop {
            self.s.skip_ws();
            let pos = self.s.pos;
            let value = match self.s.peek() {
                Some(b'>') => {
                    self.s.pos += 1;
                    break;
                }
                Some(b'&') => {
                    if bits != 32 {
                        return Err(self
                            .s
                            .err("phandle references are only allowed in 32-bit cells"));
                    }
                    let r = self.reference()?;
                    if !buf.is_empty() {
                        chunks.push(Chunk::Bytes(std::mem::take(&mut buf)));
                    }
                    chunks.push(Chunk::Phandle(r, pos));
                    continue;
                }
                Some(b'(') => self.s.primary()?,
                Some(b'\'') => self.s.char_literal()?,
                Some(b'-' | b'~' | b'!') => self.s.unary()?,
                Some(c) if c.is_ascii_digit() => self.s.number()?,
                Some(c) if is_ident_start(c) => {
                    let id = self.s.ident().unwrap_or_default();
                    if self.s.peek() == Some(b':') {
                        self.s.pos += 1; // a label inside cells
                        continue;
                    }
                    return Err(PError {
                        pos,
                        msg: crate::lex::unknown_ident(id),
                    });
                }
                None => return Err(self.s.err("unexpected end of input: missing '>'")),
                Some(_) => return Err(self.s.err("expected a cell value or '>'")),
            };
            let mask = if bits == 64 {
                u64::MAX
            } else {
                (1u64 << bits) - 1
            };
            let fits_signed =
                bits == 64 || (value as i64) >= -(1i64 << (bits - 1)) && (value as i64) < 0;
            if value & !mask != 0 && !fits_signed {
                self.report(
                    Severity::Warning,
                    pos,
                    format!("value {value:#x} truncated to {bits} bits"),
                );
            }
            let bytes = (value & mask).to_be_bytes();
            buf.extend(&bytes[8 - bits as usize / 8..]);
        }
        if !buf.is_empty() {
            chunks.push(Chunk::Bytes(buf));
        }
        Ok(())
    }

    fn string(&mut self) -> PResult<Vec<u8>> {
        let start = self.s.pos;
        self.s.pos += 1;
        let mut out = Vec::new();
        loop {
            match self.s.bump() {
                Some(b'"') => break,
                Some(b'\\') => out.push(self.s.escape()?),
                Some(b'\n') | None => {
                    return Err(PError {
                        pos: start,
                        msg: "unterminated string".into(),
                    });
                }
                Some(c) => out.push(c),
            }
        }
        out.push(0);
        Ok(out)
    }

    fn bytestring(&mut self) -> PResult<Vec<u8>> {
        let mut out = Vec::new();
        loop {
            self.labels();
            self.s.skip_ws();
            match self.s.peek() {
                Some(b']') => {
                    self.s.pos += 1;
                    return Ok(out);
                }
                Some(c) if c.is_ascii_hexdigit() => {
                    let hi = (c as char).to_digit(16).unwrap();
                    let lo = self.s.peek_at(1).and_then(|c| (c as char).to_digit(16));
                    let Some(lo) = lo else {
                        return Err(self.s.err("byte strings need two hex digits per byte"));
                    };
                    out.push((hi * 16 + lo) as u8);
                    self.s.pos += 2;
                }
                _ => return Err(self.s.err("expected hex bytes or ']'")),
            }
        }
    }

    // ---- lookups on the intermediate tree ----

    fn find_ref(&self, r: &Ref) -> Option<Vec<usize>> {
        match r {
            Ref::Label(l) => find_label(&self.root, l),
            Ref::Path(p) => find_path(&self.root, p),
        }
    }

    // ---- resolution ----

    fn finish(mut self) -> Compiled {
        let root = std::mem::take(&mut self.root);
        let mut tree = Tree::new();
        tree.mem_reserve = std::mem::take(&mut self.mem_reserve);
        let mut sources = vec![NodeSources::default()];
        let mut pending = Vec::new();
        let mut omit = Vec::new();
        build(
            &mut tree,
            &mut sources,
            &mut pending,
            &mut omit,
            Tree::ROOT,
            root,
        );

        // Labels → nodes, reporting duplicates.
        let mut labels: HashMap<String, NodeId> = HashMap::new();
        for (id, node_sources) in sources.iter().enumerate() {
            for l in tree.node(id).labels.clone() {
                if let Some(&other) = labels.get(&l)
                    && other != id
                {
                    let loc = node_sources.node.first().copied();
                    let (file, line, col) = loc.map_or((String::new(), 0, 0), |l| {
                        (self.files[l.file as usize].clone(), l.line, l.col)
                    });
                    self.diags.push(Diagnostic {
                        severity: Severity::Error,
                        file,
                        line,
                        col,
                        message: format!(
                            "duplicate label `{l}` on {} and {}",
                            tree.path(other),
                            tree.path(id)
                        ),
                        snippet: String::new(),
                    });
                } else {
                    labels.insert(l, id);
                }
            }
        }

        // Like dtc, new phandles take the lowest unused number.
        let mut used: HashSet<u32> = (0..tree.len()).filter_map(|id| tree.phandle(id)).collect();
        let mut next_phandle = 1;
        let mut fixups: Vec<(String, NodeId, String, usize)> = Vec::new(); // (label, node, prop, offset)
        let mut local_fixups: Vec<(NodeId, String, u32)> = Vec::new();
        let mut referenced = HashSet::new();

        for (id, prop_idx, chunks) in pending {
            let mut value = Vec::new();
            let prop_name = tree.node(id).props[prop_idx].name.clone();
            for chunk in chunks {
                match chunk {
                    Chunk::Bytes(b) => value.extend(b),
                    Chunk::Phandle(r, pos) => {
                        let target = resolve(&tree, &labels, &r);
                        let ph = match target {
                            Some(t) => {
                                referenced.insert(t);
                                if self.plugin {
                                    local_fixups.push((id, prop_name.clone(), value.len() as u32));
                                }
                                match tree.phandle(t) {
                                    Some(ph) => ph,
                                    None => {
                                        while used.contains(&next_phandle) {
                                            next_phandle += 1;
                                        }
                                        let ph = next_phandle;
                                        used.insert(ph);
                                        tree.node_mut(t).props.push(Property {
                                            name: "phandle".into(),
                                            value: ph.to_be_bytes().to_vec(),
                                        });
                                        ph
                                    }
                                }
                            }
                            None if self.plugin && matches!(r, Ref::Label(_)) => {
                                let Ref::Label(l) = &r else { unreachable!() };
                                fixups.push((l.clone(), id, prop_name.clone(), value.len()));
                                u32::MAX
                            }
                            None => {
                                self.report(
                                    Severity::Error,
                                    pos,
                                    format!(
                                        "reference to non-existent node or label {}",
                                        r.display()
                                    ),
                                );
                                0
                            }
                        };
                        value.extend(ph.to_be_bytes());
                    }
                    Chunk::Path(r, pos) => match resolve(&tree, &labels, &r) {
                        Some(t) => {
                            referenced.insert(t);
                            value.extend(tree.path(t).as_bytes());
                            value.push(0);
                        }
                        None => {
                            self.report(
                                Severity::Error,
                                pos,
                                format!("reference to non-existent node or label {}", r.display()),
                            );
                        }
                    },
                }
            }
            tree.node_mut(id).props[prop_idx].value = value;
        }

        // dtc resolves references before dropping unreferenced `/omit-if-no-ref/` nodes, so
        // references from dropped nodes still count and still allocate phandles.
        let removed: HashSet<NodeId> = omit
            .into_iter()
            .filter(|id| !referenced.contains(id))
            .collect();
        if !removed.is_empty() {
            let map = tree.remove_subtrees(&removed);
            let mut kept = vec![NodeSources::default(); tree.len()];
            for (old, new) in map.iter().enumerate() {
                if let Some(new) = new {
                    kept[*new] = std::mem::take(&mut sources[old]);
                }
            }
            sources = kept;
            fixups = fixups
                .into_iter()
                .filter_map(|(l, id, p, o)| Some((l, map[id]?, p, o)))
                .collect();
            local_fixups = local_fixups
                .into_iter()
                .filter_map(|(id, p, o)| Some((map[id]?, p, o)))
                .collect();
        }

        // Like dtc, drop `name` properties that just repeat the node's base name.
        for (id, node_sources) in sources.iter_mut().enumerate() {
            let node = tree.node(id);
            if node
                .prop("name")
                .is_some_and(|v| v.strip_suffix(&[0]) == Some(node.base_name().as_bytes()))
            {
                tree.node_mut(id).props.retain(|p| p.name != "name");
                node_sources.props.retain(|(n, _)| n != "name");
            }
        }

        if !fixups.is_empty() {
            let node = tree.add_child(Tree::ROOT, "__fixups__");
            sources.push(NodeSources::default());
            for (label, id, prop, offset) in fixups {
                let entry = format!("{}:{prop}:{offset}", tree.path(id));
                let n = tree.node_mut(node);
                let mut value = n.prop(&label).map(<[u8]>::to_vec).unwrap_or_default();
                value.extend(entry.as_bytes());
                value.push(0);
                n.set_prop(&label, value);
            }
        }
        if !local_fixups.is_empty() {
            let base = tree.add_child(Tree::ROOT, "__local_fixups__");
            sources.push(NodeSources::default());
            for (id, prop, offset) in local_fixups {
                let mut cur = base;
                for a in tree.ancestors(id).into_iter().skip(1) {
                    let name = tree.node(a).name.clone();
                    cur = match tree
                        .node(cur)
                        .children
                        .iter()
                        .copied()
                        .find(|&c| tree.node(c).name == name)
                    {
                        Some(c) => c,
                        None => {
                            sources.push(NodeSources::default());
                            tree.add_child(cur, name)
                        }
                    };
                }
                let n = tree.node_mut(cur);
                let mut value = n.prop(&prop).map(<[u8]>::to_vec).unwrap_or_default();
                value.extend(offset.to_be_bytes());
                n.set_prop(&prop, value);
            }
        }

        tree.finalize();
        Compiled {
            tree,
            source_map: SourceMap {
                files: self.files,
                nodes: sources,
                deleted: self.deleted,
            },
            diagnostics: self.diags,
            plugin: self.plugin,
            inputs: Vec::new(),
        }
    }
}

fn union_labels(into: &mut Vec<String>, labels: Vec<String>) {
    for l in labels {
        if !into.contains(&l) {
            into.push(l);
        }
    }
}

/// Merges parsed items into a node with dtc semantics.
fn apply(target: &mut DNode, items: Vec<Item>, deleted: &mut Vec<(String, SourceLoc)>, path: &str) {
    for item in items {
        match item {
            Item::Prop(mut p) => match target.props.iter_mut().find(|q| q.name == p.name) {
                Some(existing) => {
                    existing.deleted = false;
                    existing.chunks = p.chunks;
                    existing.locs.extend(p.locs.into_iter().map(|l| SourceLoc {
                        kind: LocKind::Overridden,
                        ..l
                    }));
                }
                None => {
                    p.locs.iter_mut().for_each(|l| l.kind = LocKind::Defined);
                    target.props.push(p);
                }
            },
            Item::Node(def) => {
                let idx = match target.children.iter().position(|c| c.name == def.name) {
                    Some(i) => {
                        let child = &mut target.children[i];
                        let kind = if child.deleted {
                            LocKind::Defined
                        } else {
                            LocKind::Overridden
                        };
                        child.deleted = false;
                        child.locs.push(SourceLoc { kind, ..def.loc });
                        i
                    }
                    None => {
                        target.children.push(DNode {
                            name: def.name.clone(),
                            locs: vec![SourceLoc {
                                kind: LocKind::Defined,
                                ..def.loc
                            }],
                            ..DNode::default()
                        });
                        target.children.len() - 1
                    }
                };
                let child = &mut target.children[idx];
                child.omit_if_no_ref |= def.omit_if_no_ref;
                union_labels(&mut child.labels, def.labels);
                let child_path = format!("{path}/{}", def.name);
                apply(child, def.items, deleted, &child_path);
            }
            Item::DelNode(name, loc) => {
                if let Some(child) = target
                    .children
                    .iter_mut()
                    .find(|c| c.name == name && !c.deleted)
                {
                    child.delete();
                    deleted.push((format!("{path}/{name}"), loc));
                }
            }
            Item::DelProp(name, loc) => {
                if let Some(prop) = target
                    .props
                    .iter_mut()
                    .find(|p| p.name == name && !p.deleted)
                {
                    prop.deleted = true;
                    let node_path = if path.is_empty() { "/" } else { path };
                    deleted.push((format!("{node_path}:{name}"), loc));
                }
            }
        }
    }
}

fn find_label(node: &DNode, label: &str) -> Option<Vec<usize>> {
    if node.labels.iter().any(|l| l == label) {
        return Some(Vec::new());
    }
    for (i, child) in node.children.iter().enumerate() {
        if let Some(mut path) = find_label(child, label) {
            path.insert(0, i);
            return Some(path);
        }
    }
    None
}

/// Resolves `/abs/path` or `label/sub/path` (dtc's `&{label/path}` form).
fn find_path(root: &DNode, path: &str) -> Option<Vec<usize>> {
    let (mut idx, rest) = match path.strip_prefix('/') {
        Some(rest) => (Vec::new(), rest),
        None => {
            let (label, rest) = path.split_once('/').unwrap_or((path, ""));
            (find_label(root, label)?, rest)
        }
    };
    let mut cur = node_at(root, &idx);
    for part in rest.split('/').filter(|p| !p.is_empty()) {
        let i = cur
            .children
            .iter()
            .position(|c| c.name == part && !c.deleted)
            .or_else(|| {
                if part.contains('@') {
                    return None;
                }
                let mut m = cur
                    .children
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| !c.deleted && c.name.split('@').next() == Some(part));
                match (m.next(), m.next()) {
                    (Some((i, _)), None) => Some(i),
                    _ => None,
                }
            })?;
        idx.push(i);
        cur = &cur.children[i];
    }
    Some(idx)
}

fn node_at<'n>(root: &'n DNode, idx: &[usize]) -> &'n DNode {
    idx.iter().fold(root, |n, &i| &n.children[i])
}

fn node_at_mut<'n>(root: &'n mut DNode, idx: &[usize]) -> &'n mut DNode {
    idx.iter().fold(root, |n, &i| &mut n.children[i])
}

fn path_of(root: &DNode, idx: &[usize]) -> String {
    let mut path = String::new();
    let mut cur = root;
    for &i in idx {
        cur = &cur.children[i];
        path.push('/');
        path.push_str(&cur.name);
    }
    path
}

fn resolve(tree: &Tree, labels: &HashMap<String, NodeId>, r: &Ref) -> Option<NodeId> {
    match r {
        Ref::Label(l) => labels.get(l).copied(),
        Ref::Path(p) if p.starts_with('/') => tree.find(p),
        Ref::Path(p) => {
            let (label, rest) = p.split_once('/').unwrap_or((p, ""));
            let mut cur = *labels.get(label)?;
            for part in rest.split('/').filter(|p| !p.is_empty()) {
                cur = tree.child_by_name(cur, part)?;
            }
            Some(cur)
        }
    }
}

/// Moves the intermediate tree into the arena; property values containing references are
/// queued in `pending` for resolution.
fn build(
    tree: &mut Tree,
    sources: &mut Vec<NodeSources>,
    pending: &mut Vec<(NodeId, usize, Vec<Chunk>)>,
    omit: &mut Vec<NodeId>,
    id: NodeId,
    node: DNode,
) {
    if node.omit_if_no_ref {
        omit.push(id);
    }
    tree.node_mut(id).labels = node.labels;
    sources[id].node = node.locs;
    for (i, prop) in node.props.into_iter().filter(|p| !p.deleted).enumerate() {
        sources[id].props.push((prop.name.clone(), prop.locs));
        let has_refs = prop.chunks.iter().any(|c| !matches!(c, Chunk::Bytes(_)));
        let value = if has_refs {
            pending.push((id, i, prop.chunks));
            Vec::new()
        } else {
            prop.chunks
                .into_iter()
                .flat_map(|c| {
                    if let Chunk::Bytes(b) = c {
                        b
                    } else {
                        Vec::new()
                    }
                })
                .collect()
        };
        tree.node_mut(id).props.push(Property {
            name: prop.name,
            value,
        });
    }
    for child in node.children.into_iter().filter(|c| !c.deleted) {
        let cid = tree.add_child(id, child.name.clone());
        sources.push(NodeSources::default());
        build(tree, sources, pending, omit, cid, child);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(text: &str) -> Compiled {
        let c = compile_str("t.dts", text);
        assert!(
            !c.has_errors(),
            "{}",
            c.diagnostics
                .iter()
                .map(|d| d.to_string())
                .collect::<Vec<_>>()
                .join("\n")
        );
        c
    }

    fn val(t: &Tree, path: &str, prop: &str) -> Vec<u8> {
        t.prop(t.find(path).unwrap_or_else(|| panic!("no {path}")), prop)
            .unwrap_or_else(|| panic!("no {path}:{prop}"))
            .to_vec()
    }

    fn cells(v: &[u32]) -> Vec<u8> {
        v.iter().flat_map(|c| c.to_be_bytes()).collect()
    }

    #[test]
    fn values() {
        let c = ok(r#"/dts-v1/;
            / {
                b;
                s = "a", "bc\n";
                c = <1 0x20 (3 + 4) 'A'>, <5>;
                n = <(-1)>;
                b8 = /bits/ 8 <1 2 0xff>;
                b16 = /bits/ 16 <0x1234>;
                b64 = /bits/ 64 <1>;
                bytes = [00 11aa BB];
                mixed = "x", <1>, [ff];
            };"#);
        let t = &c.tree;
        assert_eq!(val(t, "/", "b"), b"");
        assert_eq!(val(t, "/", "s"), b"a\0bc\n\0");
        assert_eq!(val(t, "/", "c"), cells(&[1, 0x20, 7, 65, 5]));
        assert_eq!(val(t, "/", "n"), cells(&[0xffff_ffff]));
        assert_eq!(val(t, "/", "b8"), [1, 2, 0xff]);
        assert_eq!(val(t, "/", "b16"), [0x12, 0x34]);
        assert_eq!(val(t, "/", "b64"), [0, 0, 0, 0, 0, 0, 0, 1]);
        assert_eq!(val(t, "/", "bytes"), [0, 0x11, 0xaa, 0xbb]);
        assert_eq!(val(t, "/", "mixed"), [b'x', 0, 0, 0, 0, 1, 0xff]);
    }

    #[test]
    fn merging_overrides_and_deletes() {
        let c = ok(r#"/dts-v1/;
            / {
                soc { uart0: serial@1000 { status = "disabled"; clock-frequency = <1>; }; gone {}; };
            };
            / { soc { extra {}; }; };
            &uart0 { status = "okay"; /delete-property/ clock-frequency; };
            / { soc { /delete-node/ gone; }; };
            "#);
        let t = &c.tree;
        let uart = t.find("/soc/serial@1000").unwrap();
        assert_eq!(t.prop_str(uart, "status"), Some("okay"));
        assert!(t.prop(uart, "clock-frequency").is_none());
        assert!(t.find("/soc/extra").is_some());
        assert!(t.find("/soc/gone").is_none());
        assert_eq!(t.node(uart).labels, ["uart0"]);

        let status = c.source_map.prop(uart, "status");
        assert_eq!(status.len(), 2);
        assert_eq!((status[0].kind, status[0].line), (LocKind::Defined, 3));
        assert_eq!((status[1].kind, status[1].line), (LocKind::Overridden, 6));
        let soc = t.find("/soc").unwrap();
        assert_eq!(c.source_map.node(soc).len(), 3);
        let deleted: Vec<&str> = c
            .source_map
            .deleted
            .iter()
            .map(|(p, _)| p.as_str())
            .collect();
        assert_eq!(deleted, ["/soc/serial@1000:clock-frequency", "/soc/gone"]);
    }

    #[test]
    fn deleted_entries_are_revived_in_place() {
        let c = ok(r#"/dts-v1/;
            / { n { a = <1>; b = <2>; c = <3>; x {}; y {}; }; };
            / { n { /delete-property/ a; /delete-node/ x; }; };
            / { n { z {}; a = <9>; x { again; }; }; };
            "#);
        let t = &c.tree;
        let n = t.find("/n").unwrap();
        let props: Vec<&str> = t.node(n).props.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(props, ["a", "b", "c"]);
        let children: Vec<&str> = t
            .node(n)
            .children
            .iter()
            .map(|&c| t.node(c).name.as_str())
            .collect();
        assert_eq!(children, ["x", "y", "z"]);
        assert!(t.prop(t.find("/n/x").unwrap(), "again").is_some());
    }

    #[test]
    fn label_relative_paths() {
        let c = ok(r#"/dts-v1/;
            / { pins: pinctrl { group { a = <1>; }; }; user { p = <&{pins/group}>; s = &{pins/group}; }; };
            &{pins/group} { b = <2>; };
            /delete-node/ &{pins/group};
            / { pinctrl { group { c; }; }; };
            "#);
        let g = c.tree.find("/pinctrl/group").unwrap();
        assert!(c.tree.prop(g, "c").is_some());
        assert!(c.tree.prop(g, "a").is_none());
        assert_eq!(
            c.tree.prop_str(c.tree.find("/user").unwrap(), "s"),
            Some("/pinctrl/group")
        );
    }

    #[test]
    fn failed_override_does_not_swallow_the_next_statement() {
        let c = compile_str(
            "t.dts",
            "/dts-v1/;\n/ { a {}; };\n&missing { x; };\n&{/a} { y; };\n",
        );
        assert_eq!(c.errors().count(), 1);
        assert!(c.tree.prop(c.tree.find("/a").unwrap(), "y").is_some());
    }

    #[test]
    fn redundant_name_property_is_dropped() {
        let c = ok(
            "/dts-v1/;\n/ { memory@0 { name = \"memory\"; }; other { name = \"different\"; }; };\n",
        );
        assert!(
            c.tree
                .prop(c.tree.find("/memory@0").unwrap(), "name")
                .is_none()
        );
        assert!(
            c.tree
                .prop(c.tree.find("/other").unwrap(), "name")
                .is_some()
        );
    }

    #[test]
    fn references_and_phandles() {
        let c = ok(r#"/dts-v1/;
            / {
                intc: interrupt-controller { phandle = <5>; };
                clk: clock { };
                dev {
                    interrupt-parent = <&intc>;
                    clocks = <&clk 1>, <&{/clock} 2>;
                    target = &clk;
                    target2 = &{/dev};
                };
            };"#);
        let t = &c.tree;
        assert_eq!(val(t, "/dev", "interrupt-parent"), cells(&[5]));
        // Like dtc, a new phandle takes the lowest unused number.
        assert_eq!(t.prop_u32(t.find("/clock").unwrap(), "phandle"), Some(1));
        assert_eq!(val(t, "/dev", "clocks"), cells(&[1, 1, 1, 2]));
        assert_eq!(val(t, "/dev", "target"), b"/clock\0");
        assert_eq!(val(t, "/dev", "target2"), b"/dev\0");
        assert_eq!(t.by_phandle(1), t.find("/clock"));
    }

    #[test]
    fn plugin_fragments_and_fixups() {
        let c = ok(r#"/dts-v1/;
            /plugin/;
            &i2c1 { status = "okay"; local: dev@10 { reg = <0x10>; }; };
            &{/soc} { x = <&local>; };
            "#);
        assert!(c.plugin);
        let t = &c.tree;
        assert_eq!(val(t, "/fragment@0", "target"), cells(&[0xffff_ffff]));
        assert_eq!(val(t, "/fragment@1", "target-path"), b"/soc\0");
        assert_eq!(val(t, "/__fixups__", "i2c1"), b"/fragment@0:target:0\0");
        assert!(t.find("/fragment@0/__overlay__/dev@10").is_some());
        assert_eq!(
            val(t, "/__local_fixups__/fragment@1/__overlay__", "x"),
            cells(&[0])
        );
    }

    #[test]
    fn overlay_merges_into_its_own_labels() {
        let c = ok(r#"/dts-v1/;
            /plugin/;
            &{/} { bl: backlight { status = "disabled"; }; };
            &bl { status = "okay"; };
            &external { x; };
            "#);
        let t = &c.tree;
        let roots: Vec<&str> = t
            .node(Tree::ROOT)
            .children
            .iter()
            .map(|&c| t.node(c).name.as_str())
            .collect();
        assert_eq!(roots, ["fragment@0", "fragment@1", "__fixups__"]);
        assert_eq!(
            t.prop_str(
                t.find("/fragment@0/__overlay__/backlight").unwrap(),
                "status"
            ),
            Some("okay")
        );
    }

    #[test]
    fn omit_if_no_ref() {
        let c = ok(r#"/dts-v1/;
            / {
                /omit-if-no-ref/ a: pins-a {};
                /omit-if-no-ref/ b: pins-b {};
                c: pins-c {};
                user { pinctrl-0 = <&a>; };
            };
            /omit-if-no-ref/ &c;
            "#);
        let t = &c.tree;
        assert!(t.find("/pins-a").is_some());
        assert!(t.find("/pins-b").is_none());
        assert!(t.find("/pins-c").is_none());
    }

    #[test]
    fn errors_have_locations_and_recover() {
        let c = compile_str(
            "t.dts",
            "/dts-v1/;\n/ {\n    a = <GIC_SPI 1>;\n    b = <&nolabel>;\n    good = <1>;\n    c = \"unterminated;\n    d;\n};\n",
        );
        let errs: Vec<_> = c.errors().collect();
        assert_eq!(errs.len(), 3, "{errs:#?}");
        assert_eq!((errs[0].line, errs[0].col), (3, 10));
        assert!(errs[0].message.contains("GIC_SPI"));
        assert_eq!(errs[1].line, 6);
        assert_eq!(errs[2].line, 4);
        assert!(errs[2].message.contains("&nolabel"));
        // Statements between the errors were still parsed.
        assert!(c.tree.prop(Tree::ROOT, "good").is_some());
    }

    #[test]
    fn locations_across_includes() {
        let vfs = MemVfs::new()
            .with(
                "board.dts",
                "/dts-v1/;\n#include \"soc.dtsi\"\n&uart { status = \"okay\"; };\n",
            )
            .with(
                "soc.dtsi",
                "/ {\n\n    uart: serial { status = \"disabled\"; };\n};\n",
            );
        let c = compile(&vfs, "board.dts");
        assert!(!c.has_errors(), "{:?}", c.diagnostics);
        let uart = c.tree.find("/serial").unwrap();
        let locs = c.source_map.prop(uart, "status");
        let at: Vec<(&str, u32)> = locs
            .iter()
            .map(|l| (c.source_map.file(l), l.line))
            .collect();
        assert_eq!(at, [("soc.dtsi", 3), ("board.dts", 3)]);
    }

    #[test]
    fn duplicate_labels_are_errors() {
        let c = compile_str("t.dts", "/dts-v1/;\n/ { x: a {}; x: b {}; };\n");
        assert!(
            c.errors()
                .any(|d| d.message.contains("duplicate label `x`"))
        );
    }
}
