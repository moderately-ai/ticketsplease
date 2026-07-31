//! Output formatting: human-readable (default) versus the stable JSON contract.

use clap::ValueEnum;
use ticketsplease_core::config::{CommentMode, CommentSourceMode};
use ticketsplease_core::{Error, Result};

/// Output format selector. Human-readable is the default; JSON is the contract.
#[derive(Copy, Clone, Debug, Default, ValueEnum)]
pub enum Format {
    /// Human-readable text.
    #[default]
    Human,
    /// Stable, versioned JSON.
    Json,
}

/// CLI spelling for repository-configured comment verbosity.
#[derive(Copy, Clone, Debug, ValueEnum)]
pub enum CommentModeArg {
    Auto,
    Full,
    Summary,
    None,
}

impl From<CommentModeArg> for CommentMode {
    fn from(value: CommentModeArg) -> Self {
        match value {
            CommentModeArg::Auto => Self::Auto,
            CommentModeArg::Full => Self::Full,
            CommentModeArg::Summary => Self::Summary,
            CommentModeArg::None => Self::None,
        }
    }
}

/// CLI spelling for comment source selection.
#[derive(Copy, Clone, Debug, ValueEnum)]
pub enum CommentSourceArg {
    All,
    Worktree,
    TicketBranch,
}

impl From<CommentSourceArg> for CommentSourceMode {
    fn from(value: CommentSourceArg) -> Self {
        match value {
            CommentSourceArg::All => Self::All,
            CommentSourceArg::Worktree => Self::Worktree,
            CommentSourceArg::TicketBranch => Self::TicketBranch,
        }
    }
}

/// Global output choices whose absence defers to `[output]` in the repository config.
#[derive(Copy, Clone, Debug, Default)]
pub struct OutputOverrides {
    pub comments: Option<CommentModeArg>,
    pub comment_source: Option<CommentSourceArg>,
}

/// Print a JSON value as deterministic pretty text. `serde_json`'s default map is
/// a `BTreeMap`, so object keys are emitted in sorted order (R13).
pub fn print_json(value: &serde_json::Value) -> Result<()> {
    let s = serde_json::to_string_pretty(value)
        .map_err(|e| Error::Internal(format!("serializing json: {e}")))?;
    println!("{s}");
    Ok(())
}
