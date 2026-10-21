//! Flattened device tree (DTB) reader and writer.

use std::collections::HashMap;

use crate::{Error, Result, Tree, bail};

pub const MAGIC: u32 = 0xd00d_feed;

const FDT_BEGIN_NODE: u32 = 1;
const FDT_END_NODE: u32 = 2;
const FDT_PROP: u32 = 3;
const FDT_NOP: u32 = 4;
const FDT_END: u32 = 9;

pub fn is_dtb(data: &[u8]) -> bool {
    data.len() >= 4 && u32::from_be_bytes([data[0], data[1], data[2], data[3]]) == MAGIC
}

fn be32(data: &[u8], off: usize) -> Result<u32> {
    data.get(off..off + 4)
        .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or_else(|| Error(format!("truncated DTB (reading offset {off:#x})")))
}

fn be64(data: &[u8], off: usize) -> Result<u64> {
    Ok(((be32(data, off)? as u64) << 32) | be32(data, off + 4)? as u64)
}

fn cstr(data: &[u8], off: usize) -> Result<&str> {
    let tail = data.get(off..).ok_or_else(|| Error(format!("string offset {off:#x} out of range")))?;
    let end = tail.iter().position(|&b| b == 0).ok_or_else(|| Error("unterminated string in DTB".into()))?;
    std::str::from_utf8(&tail[..end]).map_err(|_| Error(format!("invalid UTF-8 in DTB name at {off:#x}")))
}

fn align4(n: usize) -> usize {
    (n + 3) & !3
}

pub fn parse(data: &[u8]) -> Result<Tree> {
    if data.len() < 40 {
        bail!("file is too small to be a DTB");
    }
    if be32(data, 0)? != MAGIC {
        bail!("not a DTB (bad magic)");
    }
    let total = (be32(data, 4)? as usize).min(data.len());
    let data = &data[..total];
    let off_struct = be32(data, 8)? as usize;
    let off_strings = be32(data, 12)? as usize;
    let off_rsv = be32(data, 16)? as usize;
    let version = be32(data, 20)?;
    let boot_cpuid = be32(data, 28)?;
    let size_strings = if version >= 3 { be32(data, 32)? as usize } else { total.saturating_sub(off_strings) };
    let size_struct = if version >= 17 { be32(data, 36)? as usize } else { total.saturating_sub(off_struct) };

    let strings = data
        .get(off_strings..off_strings.saturating_add(size_strings).min(total))
        .ok_or_else(|| Error("strings block out of range".into()))?;
    let st = data
        .get(off_struct..off_struct.saturating_add(size_struct).min(total))
        .ok_or_else(|| Error("struct block out of range".into()))?;

    let mut tree = Tree::new();
    tree.boot_cpuid = boot_cpuid;

    let mut p = off_rsv;
    loop {
        let (addr, size) = (be64(data, p)?, be64(data, p + 8)?);
        p += 16;
        if addr == 0 && size == 0 {
            break;
        }
        tree.mem_reserve.push((addr, size));
    }

    let mut stack = Vec::new();
    let mut seen_root = false;
    let mut pos = 0;
    loop {
        let token = be32(st, pos)?;
        pos += 4;
        match token {
            FDT_BEGIN_NODE => {
                let name = cstr(st, pos)?;
                pos = align4(pos + name.len() + 1);
                // Version < 16 stores full paths; keep only the last component.
                let name = name.rsplit('/').next().unwrap_or(name);
                match stack.last() {
                    None if seen_root => bail!("DTB has more than one root node"),
                    None => {
                        seen_root = true;
                        stack.push(Tree::ROOT);
                    }
                    Some(&parent) => {
                        let id = tree.add_child(parent, name);
                        stack.push(id);
                    }
                }
            }
            FDT_END_NODE => {
                stack.pop().ok_or_else(|| Error("unbalanced END_NODE in DTB".into()))?;
            }
            FDT_PROP => {
                let len = be32(st, pos)? as usize;
                let name_off = be32(st, pos + 4)? as usize;
                pos += 8;
                let value = st.get(pos..pos + len).ok_or_else(|| Error("property value out of range".into()))?;
                pos = align4(pos + len);
                let name = cstr(strings, name_off)?;
                let &node = stack.last().ok_or_else(|| Error("property outside of any node".into()))?;
                tree.node_mut(node).props.push(crate::Property { name: name.to_string(), value: value.to_vec() });
            }
            FDT_NOP => {}
            FDT_END => break,
            t => bail!("bad DTB token {t:#x} at struct offset {:#x}", pos - 4),
        }
    }
    if !stack.is_empty() {
        bail!("DTB ended with unclosed nodes");
    }
    tree.finalize();
    Ok(tree)
}

pub fn write(tree: &Tree) -> Vec<u8> {
    struct Out {
        st: Vec<u8>,
        strings: Vec<u8>,
        offsets: HashMap<String, u32>,
    }

    fn pad(buf: &mut Vec<u8>) {
        while !buf.len().is_multiple_of(4) {
            buf.push(0);
        }
    }

    fn emit(tree: &Tree, id: usize, out: &mut Out) {
        let node = tree.node(id);
        out.st.extend(FDT_BEGIN_NODE.to_be_bytes());
        out.st.extend(node.name.as_bytes());
        out.st.push(0);
        pad(&mut out.st);
        for prop in &node.props {
            let off = *out.offsets.entry(prop.name.clone()).or_insert_with(|| {
                let off = out.strings.len() as u32;
                out.strings.extend(prop.name.as_bytes());
                out.strings.push(0);
                off
            });
            out.st.extend(FDT_PROP.to_be_bytes());
            out.st.extend((prop.value.len() as u32).to_be_bytes());
            out.st.extend(off.to_be_bytes());
            out.st.extend(&prop.value);
            pad(&mut out.st);
        }
        for &child in &node.children {
            emit(tree, child, out);
        }
        out.st.extend(FDT_END_NODE.to_be_bytes());
    }

    let mut out = Out { st: Vec::new(), strings: Vec::new(), offsets: HashMap::new() };
    emit(tree, Tree::ROOT, &mut out);
    out.st.extend(FDT_END.to_be_bytes());

    let mut rsv = Vec::new();
    for &(addr, size) in tree.mem_reserve.iter().chain(std::iter::once(&(0, 0))) {
        rsv.extend(addr.to_be_bytes());
        rsv.extend(size.to_be_bytes());
    }

    let off_rsv = 40u32;
    let off_struct = off_rsv + rsv.len() as u32;
    let off_strings = off_struct + out.st.len() as u32;
    let total = off_strings + out.strings.len() as u32;

    let mut buf = Vec::with_capacity(total as usize);
    for field in [
        MAGIC,
        total,
        off_struct,
        off_strings,
        off_rsv,
        17,
        16,
        tree.boot_cpuid,
        out.strings.len() as u32,
        out.st.len() as u32,
    ] {
        buf.extend(field.to_be_bytes());
    }
    buf.extend(rsv);
    buf.extend(out.st);
    buf.extend(out.strings);
    buf
}
