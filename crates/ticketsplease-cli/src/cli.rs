//! Command-line interface definition and top-level dispatch.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use ticketsplease_core::Result;

use crate::format::Format;

/// Top-level CLI.
#[derive(Parser)]
#[command(
    name = "ticketsplease",
    version,
    about = "git-native parallel-work ticketing"
)]
pub struct Cli {
    /// Path to the repository root.
    #[arg(long, global = true, default_value = ".")]
    pub repo: PathBuf,
    /// Output format (human-readable by default; JSON is the stable contract).
    #[arg(long, global = true, value_enum, default_value = "human")]
    pub format: Format,
    #[command(subcommand)]
    pub command: Command,
}

/// Top-level subcommands.
#[derive(Subcommand)]
pub enum Command {
    /// Initialize ticketsplease in a repository.
    Init(InitArgs),
    /// Create a new ticket.
    Create(CreateArgs),
    /// Update a ticket's fields.
    Set(SetArgs),
    /// Add or remove a dependency link between tickets.
    Link(LinkArgs),
    /// Show a single ticket.
    Show(ShowArgs),
    /// List tickets.
    List(ListArgs),
    /// List dependency-satisfied, dispatchable tickets.
    Ready(ReadyArgs),
    /// Partition ready tickets into conflict-free parallel batches.
    Tracks(TracksArgs),
    /// Recommend the next ticket(s) to work on.
    Next(NextArgs),
    /// Guard a branch against scope under-declaration and collisions.
    Guard(GuardArgs),
    /// Lint and validate all tickets.
    Lint(LintArgs),
    /// Migrate ticket frontmatter to the current schema version.
    Migrate(MigrateArgs),
    /// Manage the bundled Claude skill.
    Skill(SkillArgs),
    /// Update the ticketsplease binary in place.
    SelfUpdate(SelfUpdateArgs),
}

// Argument structs are intentionally empty in the scaffold; each is fleshed out
// when its command is implemented in the corresponding milestone.
macro_rules! empty_args {
    ($($name:ident),* $(,)?) => {
        $(
            /// Placeholder arguments (filled in when the command is implemented).
            #[derive(Args)]
            pub struct $name {}
        )*
    };
}

empty_args!(
    InitArgs,
    CreateArgs,
    SetArgs,
    LinkArgs,
    ShowArgs,
    ListArgs,
    ReadyArgs,
    TracksArgs,
    NextArgs,
    GuardArgs,
    LintArgs,
    MigrateArgs,
    SelfUpdateArgs,
);

/// `skill` subcommand group.
#[derive(Args)]
pub struct SkillArgs {
    #[command(subcommand)]
    pub command: SkillCommand,
}

/// Subcommands under `skill`.
#[derive(Subcommand)]
pub enum SkillCommand {
    /// Write the bundled skill into `.claude/skills/ticketsplease/`.
    Install(SkillInstallArgs),
}

/// Placeholder arguments for `skill install`.
#[derive(Args)]
pub struct SkillInstallArgs {}

/// Dispatch a parsed CLI invocation.
pub fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Init(_) => not_implemented("init"),
        Command::Create(_) => not_implemented("create"),
        Command::Set(_) => not_implemented("set"),
        Command::Link(_) => not_implemented("link"),
        Command::Show(_) => not_implemented("show"),
        Command::List(_) => not_implemented("list"),
        Command::Ready(_) => not_implemented("ready"),
        Command::Tracks(_) => not_implemented("tracks"),
        Command::Next(_) => not_implemented("next"),
        Command::Guard(_) => not_implemented("guard"),
        Command::Lint(_) => not_implemented("lint"),
        Command::Migrate(_) => not_implemented("migrate"),
        Command::Skill(args) => match args.command {
            SkillCommand::Install(_) => not_implemented("skill install"),
        },
        Command::SelfUpdate(_) => not_implemented("self-update"),
    }
}

fn not_implemented(name: &str) -> Result<()> {
    eprintln!("ticketsplease {name}: not yet implemented");
    Ok(())
}
