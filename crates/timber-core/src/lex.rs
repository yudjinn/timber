//! Byte cursor shared by the preprocessor and the DTS parser, including C-style integer
//! expression evaluation and source location tracking via cpp line markers.

use std::cell::Cell;

/// A parse error at a byte offset; turned into a [`crate::Diagnostic`] by the caller.
#[derive(Debug, Clone)]
pub struct PError {
    pub pos: usize,
    pub msg: String,
}

pub type PResult<T> = Result<T, PError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loc {
    pub file: String,
    pub line: u32,
    pub col: u32,
}

pub struct Src<'a> {
    pub b: &'a [u8],
    pub pos: usize,
    /// Line markers (`# 12 "file"`) seen so far: (offset of the next line, file, line).
    marks: Vec<(usize, String, u32)>,
    /// (mark index, byte offset, line at that offset) of the last location lookup, so
    /// sequential lookups don't rescan from the marker each time.
    line_cache: Cell<(usize, usize, u32)>,
}

impl<'a> Src<'a> {
    pub fn new(text: &'a str) -> Self {
        Src {
            b: text.as_bytes(),
            pos: 0,
            marks: Vec::new(),
            line_cache: Cell::new((usize::MAX, 0, 0)),
        }
    }

    pub fn peek(&self) -> Option<u8> {
        self.b.get(self.pos).copied()
    }

    pub fn peek_at(&self, n: usize) -> Option<u8> {
        self.b.get(self.pos + n).copied()
    }

    pub fn bump(&mut self) -> Option<u8> {
        let c = self.peek()?;
        self.pos += 1;
        Some(c)
    }

    pub fn eof(&self) -> bool {
        self.pos >= self.b.len()
    }

    pub fn eat(&mut self, s: &str) -> bool {
        if self.b[self.pos.min(self.b.len())..].starts_with(s.as_bytes()) {
            self.pos += s.len();
            true
        } else {
            false
        }
    }

    pub fn err(&self, msg: impl Into<String>) -> PError {
        PError {
            pos: self.pos,
            msg: msg.into(),
        }
    }

    pub fn expect(&mut self, c: u8) -> PResult<()> {
        self.skip_ws();
        if self.peek() == Some(c) {
            self.pos += 1;
            Ok(())
        } else {
            Err(self.err(format!("expected '{}'", c as char)))
        }
    }

    /// Skips whitespace, comments, and preprocessor line markers.
    pub fn skip_ws(&mut self) {
        loop {
            match self.peek() {
                Some(c) if c.is_ascii_whitespace() => self.pos += 1,
                Some(b'/') if self.peek_at(1) == Some(b'/') => {
                    while self.peek().is_some_and(|c| c != b'\n') {
                        self.pos += 1;
                    }
                }
                Some(b'/') if self.peek_at(1) == Some(b'*') => {
                    self.pos += 2;
                    while !self.eof() && !self.eat("*/") {
                        self.pos += 1;
                    }
                }
                Some(b'#') if self.at_line_marker() => self.line_marker(),
                _ => return,
            }
        }
    }

    fn at_line_marker(&self) -> bool {
        (self.pos == 0 || self.b[self.pos - 1] == b'\n')
            && self.peek_at(1) == Some(b' ')
            && self.peek_at(2).is_some_and(|c| c.is_ascii_digit())
    }

    fn line_marker(&mut self) {
        let start = self.pos;
        while self.peek().is_some_and(|c| c != b'\n') {
            self.pos += 1;
        }
        let line = String::from_utf8_lossy(&self.b[start + 1..self.pos]);
        let (num, file) = line.trim().split_once(' ').unwrap_or((line.trim(), ""));
        let num = num.parse().unwrap_or(1);
        let file = file.trim().trim_matches('"').to_string();
        self.bump();
        if self.marks.last().is_none_or(|m| m.0 < self.pos) {
            self.marks.push((self.pos, file, num));
        }
    }

    pub fn loc_at(&self, pos: usize) -> Loc {
        let pos = pos.min(self.b.len());
        let idx = self.marks.partition_point(|m| m.0 <= pos);
        let (mark_idx, start, file, base) = match idx.checked_sub(1) {
            Some(i) => (
                i,
                self.marks[i].0,
                self.marks[i].1.as_str(),
                self.marks[i].2,
            ),
            None => (usize::MAX - 1, 0, "<input>", 1),
        };
        let (c_idx, c_pos, c_line) = self.line_cache.get();
        let (from, mut line) = if c_idx == mark_idx && c_pos <= pos && c_pos >= start {
            (c_pos, c_line)
        } else {
            (start, base)
        };
        line += self.b[from..pos].iter().filter(|&&c| c == b'\n').count() as u32;
        self.line_cache.set((mark_idx, pos, line));
        let line_start = self.b[..pos]
            .iter()
            .rposition(|&c| c == b'\n')
            .map_or(0, |p| p + 1);
        Loc {
            file: file.to_string(),
            line,
            col: (pos - line_start + 1) as u32,
        }
    }

    #[cfg(test)]
    pub fn loc(&self) -> Loc {
        self.loc_at(self.pos)
    }

    /// The trimmed text of the line containing `pos`, for error snippets.
    pub fn line_text(&self, pos: usize) -> String {
        let pos = pos.min(self.b.len());
        let ls = self.b[..pos]
            .iter()
            .rposition(|&c| c == b'\n')
            .map_or(0, |p| p + 1);
        let le = self.b[pos..]
            .iter()
            .position(|&c| c == b'\n')
            .map_or(self.b.len(), |p| pos + p);
        String::from_utf8_lossy(&self.b[ls..le]).trim().to_string()
    }

    pub fn ident(&mut self) -> Option<&'a str> {
        let start = self.pos;
        if !self
            .peek()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
        {
            return None;
        }
        while self
            .peek()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_')
        {
            self.pos += 1;
        }
        std::str::from_utf8(&self.b[start..self.pos]).ok()
    }

    // ---- expressions ----

    pub fn expr(&mut self) -> PResult<u64> {
        let cond = self.binary(0)?;
        self.skip_ws();
        if self.peek() == Some(b'?') {
            self.pos += 1;
            let a = self.expr()?;
            self.expect(b':')?;
            let b = self.expr()?;
            return Ok(if cond != 0 { a } else { b });
        }
        Ok(cond)
    }

    fn binop(&self) -> Option<(&'static str, u8)> {
        // Two-character operators first so `<<` isn't read as `<`.
        const OPS: [(&str, u8); 18] = [
            ("||", 1),
            ("&&", 2),
            ("==", 6),
            ("!=", 6),
            ("<=", 7),
            (">=", 7),
            ("<<", 8),
            (">>", 8),
            ("|", 3),
            ("^", 4),
            ("&", 5),
            ("<", 7),
            (">", 7),
            ("+", 9),
            ("-", 9),
            ("*", 10),
            ("/", 10),
            ("%", 10),
        ];
        let rest = &self.b[self.pos..];
        OPS.iter()
            .find(|(op, _)| rest.starts_with(op.as_bytes()))
            .copied()
    }

    fn binary(&mut self, min_prec: u8) -> PResult<u64> {
        let mut lhs = self.unary()?;
        loop {
            self.skip_ws();
            let op_pos = self.pos;
            let Some((op, prec)) = self.binop() else {
                break;
            };
            if prec < min_prec {
                break;
            }
            self.pos += op.len();
            let rhs = self.binary(prec + 1)?;
            lhs = match op {
                "||" => (lhs != 0 || rhs != 0) as u64,
                "&&" => (lhs != 0 && rhs != 0) as u64,
                "==" => (lhs == rhs) as u64,
                "!=" => (lhs != rhs) as u64,
                "<=" => (lhs <= rhs) as u64,
                ">=" => (lhs >= rhs) as u64,
                "<" => (lhs < rhs) as u64,
                ">" => (lhs > rhs) as u64,
                "<<" => lhs.checked_shl(rhs as u32).unwrap_or(0),
                ">>" => lhs.checked_shr(rhs as u32).unwrap_or(0),
                "|" => lhs | rhs,
                "^" => lhs ^ rhs,
                "&" => lhs & rhs,
                "+" => lhs.wrapping_add(rhs),
                "-" => lhs.wrapping_sub(rhs),
                "*" => lhs.wrapping_mul(rhs),
                "/" | "%" if rhs == 0 => {
                    return Err(PError {
                        pos: op_pos,
                        msg: "division by zero".into(),
                    });
                }
                "/" => lhs / rhs,
                _ => lhs % rhs,
            };
        }
        Ok(lhs)
    }

    pub fn unary(&mut self) -> PResult<u64> {
        self.skip_ws();
        let op = self.peek();
        if matches!(op, Some(b'-' | b'~' | b'!' | b'+')) {
            self.pos += 1;
            let v = self.unary()?;
            return Ok(match op {
                Some(b'-') => v.wrapping_neg(),
                Some(b'~') => !v,
                Some(b'!') => (v == 0) as u64,
                _ => v,
            });
        }
        self.primary()
    }

    pub fn primary(&mut self) -> PResult<u64> {
        self.skip_ws();
        match self.peek() {
            Some(b'(') => {
                self.pos += 1;
                let v = self.expr()?;
                self.expect(b')')?;
                Ok(v)
            }
            Some(b'\'') => self.char_literal(),
            Some(c) if c.is_ascii_digit() => self.number(),
            Some(c) if c.is_ascii_alphabetic() || c == b'_' => {
                let start = self.pos;
                let id = self.ident().unwrap_or_default();
                Err(PError {
                    pos: start,
                    msg: unknown_ident(id),
                })
            }
            _ => Err(self.err("expected a number")),
        }
    }

    pub fn number(&mut self) -> PResult<u64> {
        let start = self.pos;
        let radix = match (self.peek(), self.peek_at(1)) {
            (Some(b'0'), Some(b'x' | b'X')) => {
                self.pos += 2;
                16
            }
            (Some(b'0'), Some(c)) if c.is_ascii_digit() => 8,
            _ => 10,
        };
        let digits_start = self.pos;
        while self.peek().is_some_and(|c| (c as char).is_digit(radix)) {
            self.pos += 1;
        }
        let digits = std::str::from_utf8(&self.b[digits_start..self.pos]).unwrap_or("");
        if digits.is_empty() {
            return Err(PError {
                pos: start,
                msg: "malformed number".into(),
            });
        }
        let value = u64::from_str_radix(digits, radix).map_err(|_| PError {
            pos: start,
            msg: "integer literal out of range".into(),
        })?;
        while self
            .peek()
            .is_some_and(|c| matches!(c, b'u' | b'U' | b'l' | b'L'))
        {
            self.pos += 1;
        }
        if self
            .peek()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_')
        {
            return Err(PError {
                pos: start,
                msg: "malformed number".into(),
            });
        }
        Ok(value)
    }

    pub fn char_literal(&mut self) -> PResult<u64> {
        let start = self.pos;
        self.pos += 1;
        let v = match self.peek() {
            Some(b'\\') => {
                self.pos += 1;
                self.escape()? as u64
            }
            Some(b'\'') | None => return Err(self.err("empty character literal")),
            Some(_) => {
                let rest = std::str::from_utf8(&self.b[self.pos..]).unwrap_or("");
                let ch = rest.chars().next().unwrap_or('\0');
                self.pos += ch.len_utf8().max(1);
                ch as u64
            }
        };
        if self.bump() != Some(b'\'') {
            return Err(PError {
                pos: start,
                msg: "unterminated character literal".into(),
            });
        }
        Ok(v)
    }

    /// Parses an escape sequence; the cursor is just past the backslash.
    pub fn escape(&mut self) -> PResult<u8> {
        let c = self
            .bump()
            .ok_or_else(|| self.err("unterminated escape sequence"))?;
        Ok(match c {
            b'n' => b'\n',
            b't' => b'\t',
            b'r' => b'\r',
            b'a' => 7,
            b'b' => 8,
            b'v' => 11,
            b'f' => 12,
            b'x' => {
                let mut v = 0u32;
                let mut n = 0;
                while n < 2
                    && let Some(d) = self.peek().and_then(|c| (c as char).to_digit(16))
                {
                    v = v * 16 + d;
                    self.pos += 1;
                    n += 1;
                }
                if n == 0 {
                    return Err(self.err("\\x escape with no hex digits"));
                }
                v as u8
            }
            b'0'..=b'7' => {
                let mut v = (c - b'0') as u32;
                let mut n = 1;
                while n < 3
                    && let Some(d @ b'0'..=b'7') = self.peek()
                {
                    v = v * 8 + (d - b'0') as u32;
                    self.pos += 1;
                    n += 1;
                }
                v as u8
            }
            other => other,
        })
    }
}

pub fn unknown_ident(id: &str) -> String {
    format!("unknown identifier `{id}`; if it's a macro, the header that defines it is missing")
}

pub fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_'
}

pub fn is_ident_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(s: &str) -> u64 {
        Src::new(s).expr().unwrap()
    }

    #[test]
    fn expressions() {
        assert_eq!(eval("1 + 2 * 3"), 7);
        assert_eq!(eval("(1 + 2) * 3"), 9);
        assert_eq!(eval("0x10 | 1 << 2"), 0x14);
        assert_eq!(eval("1 ? 5 : 6"), 5);
        assert_eq!(eval("0 ? 5 : 1 ? 7 : 8"), 7);
        assert_eq!(eval("-1") as u32, 0xffff_ffff);
        assert_eq!(eval("~0 >> 60"), 0xf);
        assert_eq!(eval("'a'"), 97);
        assert_eq!(eval("'\\n'"), 10);
        assert_eq!(eval("010"), 8);
        assert_eq!(eval("0"), 0);
        assert_eq!(eval("32UL"), 32);
        assert_eq!(eval("2 > 1 && 3 >= 3"), 1);
        assert_eq!(eval("10 - 2 - 3"), 5);
        assert_eq!(eval("!0 + !5"), 1);
    }

    #[test]
    fn expression_errors() {
        assert!(Src::new("1 / 0").expr().is_err());
        assert!(Src::new("0x").expr().is_err());
        assert!(Src::new("12abc").expr().is_err());
        let e = Src::new("GIC_SPI + 1").expr().unwrap_err();
        assert!(e.msg.contains("GIC_SPI"));
    }

    #[test]
    fn locations_follow_line_markers() {
        let text = "# 1 \"a.dts\"\nfoo\n# 10 \"b.dtsi\"\nbar\nbaz\n";
        let mut s = Src::new(text);
        s.skip_ws();
        assert_eq!(
            s.loc(),
            Loc {
                file: "a.dts".into(),
                line: 1,
                col: 1
            }
        );
        s.pos += 3;
        s.skip_ws();
        assert_eq!(
            s.loc(),
            Loc {
                file: "b.dtsi".into(),
                line: 10,
                col: 1
            }
        );
        s.pos += 4;
        assert_eq!(s.loc().line, 11);
    }
}
