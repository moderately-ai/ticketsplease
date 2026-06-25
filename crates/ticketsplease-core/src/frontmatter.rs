//! Round-trip-safe frontmatter editing — the one non-negotiable (R3).
//!
//! A ticket file is `---\n<frontmatter>\n---\n<body>`. Typed *reads* are handled
//! by a real YAML parser (see [`crate::ticket`]); *writes* are line-surgical here:
//! we locate the target key's line(s) and replace only those bytes, leaving
//! unknown keys, key order, comments, blank lines, and the entire body
//! byte-for-byte intact. When a managed key uses YAML we can't safely rewrite
//! (block scalars, anchors/aliases), we error rather than risk corruption.

use crate::error::{Error, Result};

/// A parsed ticket document split into byte-preserving parts.
#[derive(Debug, Clone)]
pub struct Document {
    /// The opening fence line, e.g. `"---\n"`.
    leading: String,
    /// The frontmatter text between the fences (each line keeps its ending).
    fm: String,
    /// The closing fence line plus the entire body, verbatim.
    trailing: String,
}

impl Document {
    /// Split a raw ticket file into (opening fence, frontmatter, closing fence + body).
    pub fn parse(raw: &str) -> Result<Self> {
        let lines: Vec<&str> = raw.split_inclusive('\n').collect();
        if lines.first().map(|l| l.trim_end()) != Some("---") {
            return Err(Error::Invalid(
                "missing YAML frontmatter (file must start with `---`)".into(),
            ));
        }
        let close = (1..lines.len())
            .find(|&i| lines[i].trim_end() == "---")
            .ok_or_else(|| {
                Error::Invalid("unterminated YAML frontmatter (missing closing `---`)".into())
            })?;
        Ok(Self {
            leading: lines[0].to_string(),
            fm: lines[1..close].concat(),
            trailing: lines[close..].concat(),
        })
    }

    /// Reassemble the document exactly.
    #[must_use]
    pub fn render(&self) -> String {
        let mut s = String::with_capacity(self.leading.len() + self.fm.len() + self.trailing.len());
        s.push_str(&self.leading);
        s.push_str(&self.fm);
        s.push_str(&self.trailing);
        s
    }

    /// The frontmatter text (between the fences).
    #[must_use]
    pub fn fm(&self) -> &str {
        &self.fm
    }

    /// The markdown body (everything after the closing fence line).
    #[must_use]
    pub fn body(&self) -> &str {
        match self.trailing.find('\n') {
            Some(i) => &self.trailing[i + 1..],
            None => "",
        }
    }

    /// Replace the markdown body, leaving the frontmatter (and closing fence)
    /// byte-for-byte intact. A non-empty body is normalised to end with a newline.
    pub fn set_body(&mut self, body: &str) {
        let fence_end = self
            .trailing
            .find('\n')
            .map_or(self.trailing.len(), |i| i + 1);
        let mut out = self.trailing[..fence_end].to_string();
        if !out.ends_with('\n') {
            out.push('\n'); // keep the closing fence on its own line
        }
        out.push_str(body);
        if !body.is_empty() && !body.ends_with('\n') {
            out.push('\n');
        }
        self.trailing = out;
    }

    /// Append text to the end of the markdown body (on its own line).
    pub fn append_body(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if !self.trailing.is_empty() && !self.trailing.ends_with('\n') {
            self.trailing.push('\n');
        }
        self.trailing.push_str(text);
        if !text.ends_with('\n') {
            self.trailing.push('\n');
        }
    }

    /// Whether the frontmatter has a top-level key `key`.
    #[must_use]
    pub fn has_key(&self, key: &str) -> bool {
        self.fm.split_inclusive('\n').any(|l| is_key_line(l, key))
    }

    /// Append `key: []` when the key is absent (used by migrations to back-fill).
    pub fn ensure_empty_list(&mut self, key: &str) {
        if self.has_key(key) {
            return;
        }
        let ending = self.default_ending();
        if !self.fm.is_empty() && !self.fm.ends_with('\n') {
            self.fm.push_str(ending);
        }
        self.fm.push_str(key);
        self.fm.push_str(": []");
        self.fm.push_str(ending);
    }

    /// Surgically set a scalar key's value, preserving everything else. Appends
    /// the key at the end of the frontmatter if absent.
    pub fn set_scalar(&mut self, key: &str, value: &str) -> Result<()> {
        let ending = self.default_ending();
        let rendered = render_yaml_scalar(value);
        let mut out = String::with_capacity(self.fm.len() + rendered.len() + key.len() + 4);
        let mut done = false;
        for line in self.fm.split_inclusive('\n') {
            if !done && is_key_line(line, key) {
                guard_simple_scalar(line, key)?;
                out.push_str(key);
                out.push_str(": ");
                out.push_str(&rendered);
                out.push_str(line_ending(line));
                done = true;
            } else {
                out.push_str(line);
            }
        }
        if !done {
            if !out.is_empty() && !out.ends_with('\n') {
                out.push_str(ending);
            }
            out.push_str(key);
            out.push_str(": ");
            out.push_str(&rendered);
            out.push_str(ending);
        }
        self.fm = out;
        Ok(())
    }

    /// Add `item` to list `key` (idempotent). Returns whether the document changed.
    pub fn add_list_item(&mut self, key: &str, item: &str) -> Result<bool> {
        self.edit_list(key, |items| {
            if items.iter().any(|x| x == item) {
                false
            } else {
                items.push(item.to_string());
                true
            }
        })
    }

    /// Remove `item` from list `key` (idempotent). Returns whether it changed.
    pub fn remove_list_item(&mut self, key: &str, item: &str) -> Result<bool> {
        self.edit_list(key, |items| {
            let before = items.len();
            items.retain(|x| x != item);
            items.len() != before
        })
    }

    /// The line ending used in the frontmatter (defaults to `"\n"`).
    fn default_ending(&self) -> &'static str {
        self.fm.split_inclusive('\n').next().map_or("\n", |l| {
            let e = line_ending(l);
            if e.is_empty() {
                "\n"
            } else {
                e
            }
        })
    }

    /// Locate list `key`, apply `op` to its items, and re-render the list span in
    /// place — preserving inline-vs-block style. If the key is absent, `op` runs
    /// against an empty list and (if it adds anything) a new block list is appended.
    fn edit_list(&mut self, key: &str, op: impl FnOnce(&mut Vec<String>) -> bool) -> Result<bool> {
        let lines: Vec<&str> = self.fm.split_inclusive('\n').collect();
        let Some(key_idx) = lines.iter().position(|l| is_key_line(l, key)) else {
            let mut items = Vec::new();
            if !op(&mut items) {
                return Ok(false);
            }
            let ending = self.default_ending();
            let mut out = self.fm.clone();
            if !out.is_empty() && !out.ends_with('\n') {
                out.push_str(ending);
            }
            out.push_str(&render_list(
                key,
                &items,
                &ListStyle::Block {
                    indent: "  ".into(),
                },
                ending,
            ));
            self.fm = out;
            return Ok(true);
        };

        let key_line = lines[key_idx];
        let ending = {
            let e = line_ending(key_line);
            if e.is_empty() {
                self.default_ending()
            } else {
                e
            }
        };
        let after = value_after_key(key_line, key);

        let (mut items, style, span_end) = if let Some(rest) = after.strip_prefix('[') {
            let inner = rest.strip_suffix(']').ok_or_else(|| {
                Error::Invalid(format!(
                    "cannot edit `{key}`: multi-line flow sequences are unsupported; edit manually"
                ))
            })?;
            (parse_inline_items(inner), ListStyle::Inline, key_idx + 1)
        } else if after.is_empty() {
            let mut end = key_idx + 1;
            let mut indent = String::from("  ");
            let mut items = Vec::new();
            let mut first = true;
            while end < lines.len() {
                let raw = lines[end].trim_end();
                let trimmed = raw.trim_start();
                if let Some(rest) = trimmed.strip_prefix("- ") {
                    if first {
                        indent = raw[..raw.len() - trimmed.len()].to_string();
                        first = false;
                    }
                    items.push(unquote(rest.trim()));
                    end += 1;
                } else {
                    break;
                }
            }
            (items, ListStyle::Block { indent }, end)
        } else {
            return Err(Error::Invalid(format!(
                "cannot edit `{key}` as a list: it holds a scalar value; edit manually"
            )));
        };

        if !op(&mut items) {
            return Ok(false);
        }

        let new_block = render_list(key, &items, &style, ending);
        let mut out = String::with_capacity(self.fm.len() + new_block.len());
        for l in &lines[..key_idx] {
            out.push_str(l);
        }
        out.push_str(&new_block);
        for l in &lines[span_end..] {
            out.push_str(l);
        }
        self.fm = out;
        Ok(true)
    }
}

/// Render a scalar value for direct frontmatter templating (quotes when needed).
#[must_use]
pub fn render_scalar(value: &str) -> String {
    render_yaml_scalar(value)
}

/// Render items as an inline flow sequence (`[a, b]`, or `[]` when empty).
#[must_use]
pub fn render_inline_list(items: &[String]) -> String {
    if items.is_empty() {
        return "[]".to_string();
    }
    let rendered: Vec<String> = items.iter().map(|i| render_yaml_scalar(i)).collect();
    format!("[{}]", rendered.join(", "))
}

/// Inline flow (`[a, b]`) versus block (`- a`) sequence style.
enum ListStyle {
    Inline,
    Block { indent: String },
}

fn render_list(key: &str, items: &[String], style: &ListStyle, ending: &str) -> String {
    if items.is_empty() {
        return format!("{key}: []{ending}");
    }
    match style {
        ListStyle::Inline => {
            let rendered: Vec<String> = items.iter().map(|i| render_yaml_scalar(i)).collect();
            format!("{key}: [{}]{ending}", rendered.join(", "))
        }
        ListStyle::Block { indent } => {
            let mut s = format!("{key}:{ending}");
            for i in items {
                s.push_str(indent);
                s.push_str("- ");
                s.push_str(&render_yaml_scalar(i));
                s.push_str(ending);
            }
            s
        }
    }
}

/// Whether `line` is a top-level `key:` line for exactly `key` (no indentation).
fn is_key_line(line: &str, key: &str) -> bool {
    let t = line.trim_end();
    t.strip_prefix(key).is_some_and(|r| r.starts_with(':'))
}

/// The trimmed value text following `key:` on a key line.
fn value_after_key<'a>(line: &'a str, key: &str) -> &'a str {
    let t = line.trim_end();
    let rest = &t[key.len()..];
    rest.strip_prefix(':').unwrap_or(rest).trim()
}

/// Reject surgical edits of YAML nodes we can't safely rewrite line-by-line.
fn guard_simple_scalar(line: &str, key: &str) -> Result<()> {
    if let Some(first) = value_after_key(line, key).chars().next() {
        if matches!(first, '|' | '>' | '&' | '*') {
            return Err(Error::Invalid(format!(
                "cannot surgically edit `{key}`: unsupported YAML node; edit manually"
            )));
        }
    }
    Ok(())
}

fn line_ending(line: &str) -> &'static str {
    if line.ends_with("\r\n") {
        "\r\n"
    } else if line.ends_with('\n') {
        "\n"
    } else {
        ""
    }
}

fn parse_inline_items(inner: &str) -> Vec<String> {
    inner
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(unquote)
        .collect()
}

/// Strip surrounding YAML quotes (single or double) and minimally unescape.
fn unquote(s: &str) -> String {
    let b = s.as_bytes();
    if b.len() >= 2 && b[0] == b'"' && b[b.len() - 1] == b'"' {
        let inner = &s[1..s.len() - 1];
        let mut out = String::with_capacity(inner.len());
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            if c == '\\' {
                match chars.next() {
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some(other) => out.push(other),
                    None => {}
                }
            } else {
                out.push(c);
            }
        }
        out
    } else if b.len() >= 2 && b[0] == b'\'' && b[b.len() - 1] == b'\'' {
        s[1..s.len() - 1].replace("''", "'")
    } else {
        s.to_string()
    }
}

/// Render a scalar as plain YAML when safe, otherwise as a double-quoted string.
fn render_yaml_scalar(v: &str) -> String {
    if is_plain_safe(v) {
        v.to_string()
    } else {
        quote_double(v)
    }
}

fn is_plain_safe(v: &str) -> bool {
    if v.is_empty() || v.trim() != v {
        return false;
    }
    let lower = v.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "true" | "false" | "null" | "yes" | "no" | "on" | "off" | "~"
    ) {
        return false;
    }
    if v.parse::<f64>().is_ok() {
        return false;
    }
    let first = v.as_bytes()[0];
    if b"-?:,[]{}#&*!|>'\"%@`".contains(&first) {
        return false;
    }
    if v.contains(": ") || v.contains(" #") || v.ends_with(':') {
        return false;
    }
    !v.chars().any(char::is_control)
}

fn quote_double(v: &str) -> String {
    let mut s = String::with_capacity(v.len() + 2);
    s.push('"');
    for c in v.chars() {
        match c {
            '"' => s.push_str("\\\""),
            '\\' => s.push_str("\\\\"),
            '\n' => s.push_str("\\n"),
            '\t' => s.push_str("\\t"),
            _ => s.push(c),
        }
    }
    s.push('"');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "---\nid: foo\ntitle: A title\nstatus: todo\npriority: p2\ndependencies: [a, b]\nscopes:\n  - one\n  - two\ntags: []\ncustom: keep me\n---\n# Body\n\nText here.\n";

    #[test]
    fn round_trip_identity() {
        let doc = Document::parse(SAMPLE).unwrap();
        assert_eq!(doc.render(), SAMPLE);
    }

    #[test]
    fn body_extraction() {
        let doc = Document::parse(SAMPLE).unwrap();
        assert_eq!(doc.body(), "# Body\n\nText here.\n");
    }

    #[test]
    fn set_body_replaces_only_the_body() {
        let mut doc = Document::parse(SAMPLE).unwrap();
        let fm_before = doc.fm().to_string();
        doc.set_body("# New\n\nfresh.\n");
        assert_eq!(doc.body(), "# New\n\nfresh.\n");
        assert_eq!(doc.fm(), fm_before, "frontmatter must be untouched");
        // The result still round-trips through the parser.
        assert_eq!(
            Document::parse(&doc.render()).unwrap().body(),
            "# New\n\nfresh.\n"
        );
    }

    #[test]
    fn set_body_normalises_trailing_newline() {
        let mut doc = Document::parse(SAMPLE).unwrap();
        doc.set_body("no newline");
        assert_eq!(doc.body(), "no newline\n");
    }

    #[test]
    fn append_body_adds_after_existing_body() {
        let mut doc = Document::parse(SAMPLE).unwrap();
        doc.append_body("- appended");
        assert!(doc.body().starts_with("# Body\n\nText here.\n"));
        assert!(doc.body().ends_with("- appended\n"));
    }

    #[test]
    fn set_scalar_changes_only_that_line() {
        let mut doc = Document::parse(SAMPLE).unwrap();
        doc.set_scalar("status", "in-progress").unwrap();
        let out = doc.render();
        assert!(out.contains("status: in-progress\n"));
        assert!(out.contains("custom: keep me\n"));
        assert!(out.contains("scopes:\n  - one\n  - two\n"));
        assert!(out.contains("# Body\n\nText here.\n"));
        let diff = SAMPLE
            .lines()
            .zip(out.lines())
            .filter(|(a, b)| a != b)
            .count();
        assert_eq!(diff, 1, "exactly one line should change");
    }

    #[test]
    fn add_block_list_item_idempotent() {
        let mut doc = Document::parse(SAMPLE).unwrap();
        assert!(doc.add_list_item("scopes", "three").unwrap());
        assert!(doc
            .render()
            .contains("scopes:\n  - one\n  - two\n  - three\n"));
        let mut again = Document::parse(&doc.render()).unwrap();
        assert!(!again.add_list_item("scopes", "three").unwrap());
    }

    #[test]
    fn add_inline_list_item() {
        let mut doc = Document::parse(SAMPLE).unwrap();
        assert!(doc.add_list_item("dependencies", "c").unwrap());
        assert!(doc.render().contains("dependencies: [a, b, c]\n"));
    }

    #[test]
    fn remove_to_empty_collapses() {
        let mut doc = Document::parse(SAMPLE).unwrap();
        doc.remove_list_item("dependencies", "a").unwrap();
        doc.remove_list_item("dependencies", "b").unwrap();
        assert!(doc.render().contains("dependencies: []\n"));
    }

    #[test]
    fn add_missing_key_appends_block_within_frontmatter() {
        let mut doc = Document::parse(SAMPLE).unwrap();
        assert!(doc.add_list_item("paths", "src/**").unwrap());
        let out = doc.render();
        assert!(out.contains("paths:\n  - src/**\n"));
        assert!(out.contains("\n---\n# Body\n"), "body/fence preserved");
    }

    #[test]
    fn set_missing_scalar_appends() {
        let mut doc = Document::parse(SAMPLE).unwrap();
        doc.set_scalar("branch", "tkt/foo").unwrap();
        assert!(doc.render().contains("branch: tkt/foo\n"));
    }

    #[test]
    fn parse_requires_frontmatter() {
        assert!(Document::parse("no frontmatter here\n").is_err());
        assert!(Document::parse("---\nid: x\n").is_err());
    }

    #[test]
    fn scalar_quoting_when_needed() {
        let mut doc = Document::parse(SAMPLE).unwrap();
        doc.set_scalar("title", "Add: a thing # hmm").unwrap();
        assert!(doc.render().contains("title: \"Add: a thing # hmm\"\n"));
    }

    #[test]
    fn block_scalar_is_rejected() {
        let raw = "---\nid: x\ntitle: |\n  multi\n  line\n---\nbody\n";
        let mut doc = Document::parse(raw).unwrap();
        assert!(doc.set_scalar("title", "new").is_err());
    }
}
