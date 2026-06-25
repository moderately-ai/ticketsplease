//! Output formatting: human-readable (default) versus the stable JSON contract.

use clap::ValueEnum;

/// Output format selector. Human-readable is the default; JSON is the contract.
#[derive(Copy, Clone, Debug, Default, ValueEnum)]
pub enum Format {
    /// Human-readable text.
    #[default]
    Human,
    /// Stable, versioned JSON.
    Json,
}
