//! Command-line interface definition and top-level dispatch.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use ticketsplease_core::Result;

use crate::commands;
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

/// `init` arguments.
#[derive(Args)]
pub struct InitArgs {
    /// Tickets directory to create (relative to the repo root).
    #[arg(long, default_value = "tickets")]
    pub dir: String,
    /// Overwrite an existing config file.
    #[arg(long)]
    pub force: bool,
}

/// `create` arguments.
#[derive(Args)]
pub struct CreateArgs {
    /// Ticket title.
    #[arg(long)]
    pub title: String,
    /// Explicit id (slug); defaults to a slug of the title.
    #[arg(long)]
    pub id: Option<String>,
    /// Status: todo | ready | in-progress | blocked | review | done.
    #[arg(long, default_value = "todo")]
    pub status: String,
    /// Priority: p0 | p1 | p2 | p3.
    #[arg(long, default_value = "p2")]
    pub priority: String,
    /// Dependency ticket ids (repeatable or comma-separated).
    #[arg(long = "depends-on", value_delimiter = ',')]
    pub depends_on: Vec<String>,
    /// Declared scope names (repeatable or comma-separated).
    #[arg(long = "scope", value_delimiter = ',')]
    pub scopes: Vec<String>,
    /// Explicit path globs (repeatable or comma-separated).
    #[arg(long = "path", value_delimiter = ',')]
    pub paths: Vec<String>,
    /// Tags (repeatable or comma-separated).
    #[arg(long = "tag", value_delimiter = ',')]
    pub tags: Vec<String>,
    /// Markdown body.
    #[arg(long, default_value = "")]
    pub body: String,
}

/// `set` arguments.
#[derive(Args)]
pub struct SetArgs {
    /// Ticket id.
    pub id: String,
    /// New status.
    #[arg(long)]
    pub status: Option<String>,
    /// New priority.
    #[arg(long)]
    pub priority: Option<String>,
    /// Scopes to add (repeatable or comma-separated).
    #[arg(long = "add-scope", value_delimiter = ',')]
    pub add_scope: Vec<String>,
    /// Scopes to remove (repeatable or comma-separated).
    #[arg(long = "remove-scope", value_delimiter = ',')]
    pub remove_scope: Vec<String>,
    /// Tags to add (repeatable or comma-separated).
    #[arg(long = "add-tag", value_delimiter = ',')]
    pub add_tag: Vec<String>,
}

/// `link` arguments.
#[derive(Args)]
pub struct LinkArgs {
    /// Ticket that gains (or loses) a dependency.
    pub id: String,
    /// The dependency target id.
    #[arg(long = "depends-on")]
    pub depends_on: String,
    /// Remove the link instead of adding it.
    #[arg(long)]
    pub remove: bool,
}

/// `show` arguments.
#[derive(Args)]
pub struct ShowArgs {
    /// Ticket id.
    pub id: String,
}

/// `list` arguments.
#[derive(Args)]
pub struct ListArgs {
    /// Filter by status.
    #[arg(long)]
    pub status: Option<String>,
}

/// `next` arguments.
#[derive(Args)]
pub struct NextArgs {
    /// Return up to N mutually conflict-free picks.
    #[arg(long, default_value_t = 1)]
    pub parallel: usize,
}

/// `guard` arguments.
#[derive(Args)]
pub struct GuardArgs {
    /// Branch (or ref) to guard.
    pub branch: String,
    /// Base ref to diff against (defaults to the config `default_base`).
    #[arg(long)]
    pub base: Option<String>,
    /// Explicit ticket id (otherwise inferred from the branch name).
    #[arg(long)]
    pub ticket: Option<String>,
}

// Commands implemented in later milestones keep placeholder argument structs.
macro_rules! empty_args {
    ($($name:ident),* $(,)?) => {
        $(
            /// Placeholder arguments (filled in when the command is implemented).
            #[derive(Args)]
            pub struct $name {}
        )*
    };
}

empty_args!(ReadyArgs, TracksArgs, LintArgs, MigrateArgs, SelfUpdateArgs,);

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

/// `skill install` arguments.
#[derive(Args)]
pub struct SkillInstallArgs {}

/// Dispatch a parsed CLI invocation.
pub fn run(cli: Cli) -> Result<()> {
    let repo = cli.repo.as_path();
    let fmt = cli.format;
    match &cli.command {
        Command::Init(a) => commands::init(repo, fmt, a),
        Command::Create(a) => commands::create(repo, fmt, a),
        Command::Set(a) => commands::set(repo, fmt, a),
        Command::Link(a) => commands::link(repo, fmt, a),
        Command::Show(a) => commands::show(repo, fmt, a),
        Command::List(a) => commands::list(repo, fmt, a),
        Command::Lint(_) => commands::lint(repo, fmt),
        Command::Ready(_) => commands::ready(repo, fmt),
        Command::Tracks(_) => commands::tracks(repo, fmt),
        Command::Next(a) => commands::next(repo, fmt, a),
        Command::Guard(a) => commands::guard(repo, fmt, a),
        Command::Migrate(_) => not_implemented("migrate"),
        Command::Skill(a) => match &a.command {
            SkillCommand::Install(_) => not_implemented("skill install"),
        },
        Command::SelfUpdate(_) => not_implemented("self-update"),
    }
}

fn not_implemented(name: &str) -> Result<()> {
    eprintln!("ticketsplease {name}: not yet implemented");
    Ok(())
}
