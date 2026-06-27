//! ticketsplease — git-native parallel-work ticketing CLI.

mod cli;
mod commands;
mod format;
mod skill;
mod update;

use std::process::ExitCode;

use clap::Parser;

use crate::format::Format;

fn main() -> ExitCode {
    let cli = cli::Cli::parse();
    let fmt = cli.format;
    match cli::run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            // Errors go to stderr so stdout stays a clean result channel. In JSON
            // mode the error is itself a machine-readable envelope; the exit code
            // is the stable contract (R12) either way.
            match fmt {
                Format::Json => eprintln!(
                    "{}",
                    serde_json::json!({
                        "schema_version": 1,
                        "error": { "code": err.code(), "message": err.message() }
                    })
                ),
                Format::Human => eprintln!("error: {err}"),
            }
            ExitCode::from(err.exit_code() as u8)
        }
    }
}
