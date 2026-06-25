//! Output formatting: human-readable (default) versus the stable JSON contract.

use clap::ValueEnum;
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

/// Print a JSON value as deterministic pretty text. `serde_json`'s default map is
/// a `BTreeMap`, so object keys are emitted in sorted order (R13).
pub fn print_json(value: &serde_json::Value) -> Result<()> {
    let s = serde_json::to_string_pretty(value)
        .map_err(|e| Error::Internal(format!("serializing json: {e}")))?;
    println!("{s}");
    Ok(())
}
