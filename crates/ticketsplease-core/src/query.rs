//! A small boolean query language for filtering tickets (`--where`).
//!
//! Grammar (AND binds tighter than OR; `NOT` is prefix; parentheses group):
//!
//! ```text
//! or   := and ("OR" and)*
//! and  := not ("AND" not)*
//! not  := "NOT" not | atom
//! atom := "(" or ")" | field ":" value
//! ```
//!
//! `AND` / `OR` / `NOT` are case-insensitive keywords. A value is a bareword
//! (`[A-Za-z0-9_./-]`, so `query/planner`, `p0`, and slug ids work unquoted) or a
//! single/double-quoted string for values with spaces. `status:`/`priority:` values
//! are validated at parse time, so a typo fails loudly (exit 3) rather than silently
//! matching nothing. The evaluator is pure — no I/O — so it is cheap to reuse across
//! `list`, `set --where`, `rollup`, and saved views.

use std::str::FromStr;

use crate::error::{Error, Result};
use crate::ticket::{Priority, Status, Ticket};

/// A parsed `--where` expression, evaluated against a ticket with [`Predicate::matches`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Predicate {
    /// Both operands must match.
    And(Box<Predicate>, Box<Predicate>),
    /// Either operand may match.
    Or(Box<Predicate>, Box<Predicate>),
    /// The operand must not match.
    Not(Box<Predicate>),
    /// A `field:value` comparison.
    Term {
        /// The ticket field being compared.
        field: Field,
        /// The value to compare against (already unquoted).
        value: String,
    },
}

/// The ticket fields a `--where` term can match on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    /// `status:<value>` — exact lifecycle status.
    Status,
    /// `priority:<value>` — exact priority (p0..p3).
    Priority,
    /// `tag:<value>` — membership in `tags`.
    Tag,
    /// `scope:<value>` — membership in `scopes`.
    Scope,
    /// `assignee:<value>` — exact current assignee.
    Assignee,
    /// `id:<value>` — exact ticket id.
    Id,
    /// `dep:<value>` — membership in `dependencies`.
    Dep,
    /// `related:<value>` — membership in `related`.
    Related,
}

/// The queryable field names, for error messages and help text.
const VALID_FIELDS: &str = "status, priority, tag, scope, assignee, id, dep, related";

impl Field {
    fn parse(name: &str) -> Result<Self> {
        Ok(match name {
            "status" => Field::Status,
            "priority" => Field::Priority,
            "tag" => Field::Tag,
            "scope" => Field::Scope,
            "assignee" => Field::Assignee,
            "id" => Field::Id,
            "dep" | "dependency" => Field::Dep,
            "related" => Field::Related,
            other => {
                return Err(Error::Invalid(format!(
                    "unknown query field `{other}` (valid: {VALID_FIELDS})"
                )))
            }
        })
    }
}

impl Predicate {
    /// Whether `ticket` satisfies this predicate.
    #[must_use]
    pub fn matches(&self, ticket: &Ticket) -> bool {
        match self {
            Predicate::And(a, b) => a.matches(ticket) && b.matches(ticket),
            Predicate::Or(a, b) => a.matches(ticket) || b.matches(ticket),
            Predicate::Not(a) => !a.matches(ticket),
            Predicate::Term { field, value } => match field {
                // Validated at parse time, so the re-parse here cannot fail.
                Field::Status => Status::from_str(value).is_ok_and(|s| ticket.status == s),
                Field::Priority => Priority::from_str(value).is_ok_and(|p| ticket.priority == p),
                Field::Tag => ticket.tags.iter().any(|x| x == value),
                Field::Scope => ticket.scopes.iter().any(|x| x == value),
                Field::Assignee => ticket.assignee.as_deref() == Some(value.as_str()),
                Field::Id => ticket.id == *value,
                Field::Dep => ticket.dependencies.iter().any(|x| x == value),
                Field::Related => ticket.related.iter().any(|x| x == value),
            },
        }
    }
}

/// Parse a `--where` expression. Errors (exit 3) on an empty expression, an unknown
/// field, an invalid `status`/`priority` value, or a syntax error (unbalanced
/// parens, a dangling operator, trailing tokens).
pub fn parse(input: &str) -> Result<Predicate> {
    let tokens = tokenize(input)?;
    if tokens.is_empty() {
        return Err(Error::Invalid("empty query expression".into()));
    }
    let mut p = Parser { tokens, pos: 0 };
    let expr = p.parse_or()?;
    if p.pos != p.tokens.len() {
        return Err(Error::Invalid(format!(
            "unexpected trailing input in query near token {}",
            p.pos + 1
        )));
    }
    Ok(expr)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    LParen,
    RParen,
    /// A bare word: either a keyword (`and`/`or`/`not`, no colon) or a `field:value`.
    Word(String),
}

/// Split the input into parens and words, stripping quotes (a quoted span keeps its
/// inner spaces and parens literally). Quotes may appear anywhere in a word so both
/// `"a b":x` and `tag:"a b"` work.
fn tokenize(input: &str) -> Result<Vec<Tok>> {
    let mut toks = Vec::new();
    let mut chars = input.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            c if c.is_whitespace() => {
                chars.next();
            }
            '(' => {
                chars.next();
                toks.push(Tok::LParen);
            }
            ')' => {
                chars.next();
                toks.push(Tok::RParen);
            }
            _ => {
                let mut word = String::new();
                while let Some(&c) = chars.peek() {
                    match c {
                        c if c.is_whitespace() => break,
                        '(' | ')' => break,
                        '"' | '\'' => {
                            let quote = c;
                            chars.next(); // opening quote
                            let mut closed = false;
                            for q in chars.by_ref() {
                                if q == quote {
                                    closed = true;
                                    break;
                                }
                                word.push(q);
                            }
                            if !closed {
                                return Err(Error::Invalid(
                                    "unterminated quoted value in query".into(),
                                ));
                            }
                        }
                        _ => word.push(chars.next().unwrap()),
                    }
                }
                toks.push(Tok::Word(word));
            }
        }
    }
    Ok(toks)
}

/// Keyword classification: a word is a keyword only if it has no `:` (so `tag:and`
/// is a term, not the AND keyword) and matches a reserved word case-insensitively.
fn keyword(word: &str) -> Option<&'static str> {
    if word.contains(':') {
        return None;
    }
    match word.to_ascii_lowercase().as_str() {
        "and" => Some("AND"),
        "or" => Some("OR"),
        "not" => Some("NOT"),
        _ => None,
    }
}

struct Parser {
    tokens: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek_keyword(&self, kw: &str) -> bool {
        matches!(self.tokens.get(self.pos), Some(Tok::Word(w)) if keyword(w) == Some(kw))
    }

    fn parse_or(&mut self) -> Result<Predicate> {
        let mut left = self.parse_and()?;
        while self.peek_keyword("OR") {
            self.pos += 1;
            let right = self.parse_and()?;
            left = Predicate::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Predicate> {
        let mut left = self.parse_not()?;
        while self.peek_keyword("AND") {
            self.pos += 1;
            let right = self.parse_not()?;
            left = Predicate::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_not(&mut self) -> Result<Predicate> {
        if self.peek_keyword("NOT") {
            self.pos += 1;
            return Ok(Predicate::Not(Box::new(self.parse_not()?)));
        }
        self.parse_atom()
    }

    fn parse_atom(&mut self) -> Result<Predicate> {
        match self.tokens.get(self.pos) {
            Some(Tok::LParen) => {
                self.pos += 1;
                let inner = self.parse_or()?;
                match self.tokens.get(self.pos) {
                    Some(Tok::RParen) => {
                        self.pos += 1;
                        Ok(inner)
                    }
                    _ => Err(Error::Invalid("unbalanced parentheses in query".into())),
                }
            }
            Some(Tok::Word(w)) => {
                if let Some(kw) = keyword(w) {
                    return Err(Error::Invalid(format!(
                        "expected a `field:value` term, found operator `{kw}`"
                    )));
                }
                let term = parse_term(w)?;
                self.pos += 1;
                Ok(term)
            }
            Some(Tok::RParen) => Err(Error::Invalid("unexpected `)` in query".into())),
            None => Err(Error::Invalid("unexpected end of query expression".into())),
        }
    }
}

/// Parse a single `field:value` word into a [`Predicate::Term`], validating
/// `status`/`priority` values eagerly so a typo fails at parse time.
fn parse_term(word: &str) -> Result<Predicate> {
    let (name, value) = word.split_once(':').ok_or_else(|| {
        Error::Invalid(format!(
            "expected a `field:value` term (valid fields: {VALID_FIELDS}), found `{word}`"
        ))
    })?;
    if name.is_empty() {
        return Err(Error::Invalid(format!("missing field name in `{word}`")));
    }
    if value.is_empty() {
        return Err(Error::Invalid(format!("missing value for `{name}:`")));
    }
    let field = Field::parse(name)?;
    // Fail loud on an invalid enum value rather than silently matching nothing.
    match field {
        Field::Status => {
            Status::from_str(value)?;
        }
        Field::Priority => {
            Priority::from_str(value)?;
        }
        _ => {}
    }
    Ok(Predicate::Term {
        field,
        value: value.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ticket(raw: &str) -> Ticket {
        Ticket::parse(raw).unwrap()
    }

    fn matches(expr: &str, raw: &str) -> bool {
        parse(expr).unwrap().matches(&ticket(raw))
    }

    const T: &str = "---\nid: foo\ntitle: T\nstatus: in-progress\npriority: p1\ndependencies: [base]\nrelated: [see-me]\nscopes: [core, query/planner]\ntags: [ux, bug]\nassignee: worker-1\n---\n";

    #[test]
    fn single_terms_match_each_field() {
        assert!(matches("status:in-progress", T));
        assert!(matches("priority:p1", T));
        assert!(matches("tag:ux", T));
        assert!(matches("scope:query/planner", T));
        assert!(matches("assignee:worker-1", T));
        assert!(matches("id:foo", T));
        assert!(matches("dep:base", T));
        assert!(matches("related:see-me", T));
        assert!(!matches("tag:missing", T));
        assert!(!matches("status:done", T));
    }

    #[test]
    fn boolean_combinators_and_precedence() {
        assert!(matches("tag:ux AND status:in-progress", T));
        assert!(!matches("tag:ux AND status:done", T));
        assert!(matches("tag:ux OR status:done", T));
        assert!(matches("NOT status:done", T));
        assert!(!matches("NOT tag:ux", T));
        // AND binds tighter than OR: `done AND x` is false, OR `tag:ux` is true.
        assert!(matches("status:done AND tag:bug OR tag:ux", T));
        // Parentheses override precedence: (done OR ux) AND missing -> false.
        assert!(!matches("(status:done OR tag:ux) AND tag:missing", T));
        assert!(matches("(status:done OR tag:ux) AND priority:p1", T));
    }

    #[test]
    fn quoted_values_keep_spaces() {
        let raw = "---\nid: foo\ntitle: T\ntags: [\"needs review\"]\n---\n";
        assert!(matches("tag:\"needs review\"", raw));
        assert!(!matches("tag:\"needs\"", raw));
    }

    #[test]
    fn invalid_field_and_value_fail_loudly() {
        assert!(parse("bogus:x").is_err());
        assert!(parse("status:doing").is_err()); // invalid enum value
        assert!(parse("priority:p9").is_err());
        assert!(parse("").is_err());
        assert!(parse("   ").is_err());
        assert!(parse("tag:").is_err());
    }

    #[test]
    fn syntax_errors_are_rejected() {
        assert!(parse("(tag:x").is_err()); // unbalanced
        assert!(parse("tag:x)").is_err()); // trailing
        assert!(parse("tag:x AND").is_err()); // dangling operator
        assert!(parse("AND tag:x").is_err()); // leading operator
        assert!(parse("tag:x tag:y").is_err()); // missing operator -> trailing input
    }

    #[test]
    fn keyword_lookalikes_as_values_are_terms() {
        let raw = "---\nid: and\ntitle: T\ntags: [or]\n---\n";
        assert!(matches("id:and", raw));
        assert!(matches("tag:or", raw));
        assert!(matches("id:and AND tag:or", raw));
    }
}
