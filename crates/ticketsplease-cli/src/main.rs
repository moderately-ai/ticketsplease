//! ticketsplease — git-native parallel-work ticketing CLI.

mod advisory;
mod cli;
mod commands;
mod format;
mod recipe;
mod skill;
mod templates;
mod update;
mod update_check;

use std::process::ExitCode;

use clap::Parser;

use crate::format::Format;

fn main() -> ExitCode {
    let cli = cli::Cli::parse();
    let fmt = cli.format;
    let repo = cli.repo.clone();
    let code = match cli::run(cli) {
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
    };
    // Maintenance advisories run last, after the command's output and exit code are
    // settled — stderr-only, and a no-op outside an interactive human session.
    advisory::run(&repo, fmt);
    code
}
