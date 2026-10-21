//! Device tree model plus readers/writers for the flattened (DTB) and source (DTS) formats.

pub mod bsp;
pub mod dtb;
pub mod dts;
pub mod format;
mod lex;
mod preprocess;
pub mod source_map;
mod tree;
pub mod vfs;

pub use tree::{Node, NodeId, Property, Tree};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

macro_rules! bail {
    ($($t:tt)*) => { return Err($crate::Error(format!($($t)*))) };
}
pub(crate) use bail;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

/// A compiler message tied to a source location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub file: String,
    pub line: u32,
    pub col: u32,
    pub message: String,
    /// The source line the message refers to, trimmed.
    pub snippet: String,
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        write!(
            f,
            "{}:{}:{}: {kind}: {}",
            self.file, self.line, self.col, self.message
        )?;
        if !self.snippet.is_empty() {
            write!(f, "\n    {}", self.snippet)?;
        }
        Ok(())
    }
}
