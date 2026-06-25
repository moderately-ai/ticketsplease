//! ticketsplease — git-native parallel-work ticketing CLI.

mod cli;
mod commands;
mod format;
mod skill;
mod update;

use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    let cli = cli::Cli::parse();
    match cli::run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            // exit_code() is the stable CLI contract (R12).
            ExitCode::from(err.exit_code() as u8)
        }
    }
}
