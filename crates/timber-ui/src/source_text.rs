//! Pure helpers for the source viewer: DTS syntax highlighting and finding the extent of the
//! node or property a source location points at.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tok {
    Text,
    Comment,
    Str,
    Number,
    /// `#include`, `#define`, `/dts-v1/`, `/delete-node/`, …
    Directive,
    /// `label:`
    Label,
    /// `&label` or `&{/path}`
    Ref,
    NodeName,
    PropName,
}

/// Splits each line into highlighted tokens. Block comments may span lines.
pub fn highlight_lines(text: &str) -> Vec<Vec<(Tok, String)>> {
    let mut in_comment = false;
    text.lines()
        .map(|line| highlight_line(line, &mut in_comment))
        .collect()
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, ',' | '.' | '_' | '+' | '#' | '?' | '@' | '-')
}

fn highlight_line(line: &str, in_comment: &mut bool) -> Vec<(Tok, String)> {
    let chars: Vec<char> = line.chars().collect();
    let mut out: Vec<(Tok, String)> = Vec::new();
    let mut push = |tok: Tok, s: String| match out.last_mut() {
        Some((t, prev)) if *t == tok => prev.push_str(&s),
        _ => out.push((tok, s)),
    };
    let mut i = 0;
    // A preprocessor directive takes the whole line (except trailing comments).
    let trimmed = line.trim_start();
    let directive_line = !*in_comment
        && trimmed.starts_with('#')
        && trimmed[1..]
            .trim_start()
            .split(|c: char| !c.is_ascii_alphabetic())
            .next()
            .is_some_and(|w| {
                matches!(
                    w,
                    "include"
                        | "define"
                        | "undef"
                        | "if"
                        | "ifdef"
                        | "ifndef"
                        | "elif"
                        | "else"
                        | "endif"
                        | "error"
                )
            });
    while i < chars.len() {
        let c = chars[i];
        let rest: String = chars[i..].iter().collect();
        if *in_comment {
            match rest.find("*/") {
                Some(end) => {
                    let n = rest[..end + 2].chars().count();
                    push(Tok::Comment, chars[i..i + n].iter().collect());
                    i += n;
                    *in_comment = false;
                }
                None => {
                    push(Tok::Comment, rest);
                    break;
                }
            }
            continue;
        }
        if rest.starts_with("//") {
            push(Tok::Comment, rest);
            break;
        }
        if rest.starts_with("/*") {
            *in_comment = true;
            push(Tok::Comment, "/*".into());
            i += 2;
            continue;
        }
        if c == '"' {
            let mut j = i + 1;
            while j < chars.len() && chars[j] != '"' {
                j += if chars[j] == '\\' { 2 } else { 1 };
            }
            let end = (j + 1).min(chars.len());
            push(Tok::Str, chars[i..end].iter().collect());
            i = end;
            continue;
        }
        if directive_line {
            // Everything up to a string or comment is the directive itself.
            let mut j = i;
            while j < chars.len()
                && chars[j] != '"'
                && !(chars[j] == '/' && matches!(chars.get(j + 1), Some('/' | '*')))
            {
                j += 1;
            }
            push(Tok::Directive, chars[i..j].iter().collect());
            i = j;
            continue;
        }
        if c == '/'
            && let Some(end) = rest[1..].find('/')
        {
            let word = &rest[1..end + 1];
            if !word.is_empty() && word.chars().all(|c| c.is_ascii_lowercase() || c == '-') {
                push(Tok::Directive, rest[..end + 2].to_string());
                i += end + 2;
                continue;
            }
        }
        if c == '&' {
            let mut j = i + 1;
            if chars.get(j) == Some(&'{') {
                while j < chars.len() && chars[j] != '}' {
                    j += 1;
                }
                j = (j + 1).min(chars.len());
            } else {
                while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '_') {
                    j += 1;
                }
            }
            if j > i + 1 {
                push(Tok::Ref, chars[i..j].iter().collect());
                i = j;
                continue;
            }
        }
        if c.is_ascii_digit() && (i == 0 || !is_name_char(chars[i - 1])) {
            let mut j = i;
            while j < chars.len() && (chars[j].is_ascii_alphanumeric()) {
                j += 1;
            }
            // Inside a name (e.g. `i2c@fe5a0000`) it's part of the name, not a number.
            if j >= chars.len() || !is_name_char(chars[j]) || chars[j] == ',' {
                push(Tok::Number, chars[i..j].iter().collect());
                i = j;
                continue;
            }
        }
        if is_name_char(c) && (i == 0 || !is_name_char(chars[i - 1])) {
            let mut j = i;
            while j < chars.len() && is_name_char(chars[j]) {
                j += 1;
            }
            let word: String = chars[i..j].iter().collect();
            let next = chars[j..].iter().find(|c| !c.is_whitespace()).copied();
            let tok = match next {
                Some(':') if j < chars.len() && chars[j] == ':' => Tok::Label,
                Some('{') => Tok::NodeName,
                Some('=') | Some(';') => Tok::PropName,
                _ => Tok::Text,
            };
            push(tok, word);
            i = j;
            if tok == Tok::Label {
                push(Tok::Label, ":".into());
                i += 1;
            }
            continue;
        }
        push(Tok::Text, c.to_string());
        i += 1;
    }
    out
}

/// The 1-based line range of the statement starting at `line`/`col`: a node's whole
/// `{ … };` block, or a property up to its `;`.
pub fn statement_lines(text: &str, line: u32, col: u32) -> (u32, u32) {
    let line_starts: Vec<usize> = std::iter::once(0)
        .chain(text.match_indices('\n').map(|(i, _)| i + 1))
        .collect();
    let Some(&start) = line_starts.get(line.saturating_sub(1) as usize) else {
        return (line, line);
    };
    let start = (start + col.saturating_sub(1) as usize).min(text.len());
    let b = text.as_bytes();
    let (mut i, mut depth) = (start, 0usize);
    let mut end = start;
    while i < b.len() {
        match b[i] {
            b'"' => {
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i = text[i + 2..].find("*/").map_or(b.len(), |p| i + 2 + p + 1);
            }
            b'/' if b.get(i + 1) == Some(&b'/') => {
                i = text[i..].find('\n').map_or(b.len(), |p| i + p);
            }
            b'{' => depth += 1,
            b'}' => depth = depth.saturating_sub(1),
            b';' if depth == 0 => {
                end = i;
                break;
            }
            _ => {}
        }
        end = i;
        i += 1;
    }
    let line_of = |pos: usize| line_starts.partition_point(|&s| s <= pos) as u32;
    (line, line_of(end).max(line))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(line: &str) -> Vec<(Tok, &str)> {
        let mut c = false;
        let v = highlight_line(line, &mut c);
        // Leak for easy comparison in tests.
        v.into_iter()
            .map(|(t, s)| (t, &*Box::leak(s.into_boxed_str())))
            .filter(|(t, _)| *t != Tok::Text)
            .collect()
    }

    #[test]
    fn highlights_dts() {
        assert_eq!(
            toks(r#"	uart0: serial@fe650000 { // console"#),
            [
                (Tok::Label, "uart0:"),
                (Tok::NodeName, "serial@fe650000"),
                (Tok::Comment, "// console")
            ]
        );
        assert_eq!(
            toks(r#"	clocks = <&cru 12>, <&{/soc/x} 0x1>;"#),
            [
                (Tok::PropName, "clocks"),
                (Tok::Ref, "&cru"),
                (Tok::Number, "12"),
                (Tok::Ref, "&{/soc/x}"),
                (Tok::Number, "0x1")
            ]
        );
        assert_eq!(
            toks(r#"#include "rk3588.dtsi""#),
            [(Tok::Directive, "#include "), (Tok::Str, "\"rk3588.dtsi\"")]
        );
        assert_eq!(
            toks("/delete-node/ &foo;"),
            [(Tok::Directive, "/delete-node/"), (Tok::Ref, "&foo")]
        );
        assert_eq!(
            toks("#address-cells = <2>;"),
            [(Tok::PropName, "#address-cells"), (Tok::Number, "2")]
        );
        let lines = highlight_lines("a /* x\n y */ b;\n");
        assert_eq!(lines[1][0], (Tok::Comment, " y */".to_string()));
    }

    #[test]
    fn finds_statement_extent() {
        let text = "/ {\n\tn: node@1 {\n\t\tp = \"}\";\n\t\tc { };\n\t};\n\tq = <1\n\t\t2>;\n};\n";
        assert_eq!(statement_lines(text, 2, 2), (2, 5));
        assert_eq!(statement_lines(text, 3, 3), (3, 3));
        assert_eq!(statement_lines(text, 6, 2), (6, 7));
    }
}
