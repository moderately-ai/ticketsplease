//! Command-line interface definition and top-level dispatch.

use std::path::PathBuf;

use clap::{ArgGroup, Args, Parser, Subcommand};
use ticketsplease_core::Result;

use crate::commands;
use crate::format::Format;

/// Top-level CLI.
#[derive(Parser)]
#[command(
    name = "ticketsplease",
    version,
    about = "git-native parallel-work ticketing",
    after_help = "New here? Run `tkt guide` for the conceptual model (scopes, tracks, \
                  scoring, guard, claims), or read the bundled skill installed by `tkt init`."
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
    /// Report ticket status; with `--all-branches`, scan `tkt/*` branches.
    Status(StatusArgs),
    /// Cross-check ticket status against `tkt/*` branches and worktrees (drift report).
    Reconcile(ReconcileArgs),
    /// Block until a ticket reaches a target status.
    Watch(WatchArgs),
    /// Add or list a ticket's comments (append-only, conflict-free).
    Comment(CommentArgs),
    /// Tail the cross-branch activity event log (comments, status, claims).
    Events(EventsArgs),
    /// List dependency-satisfied, dispatchable tickets.
    Ready(ReadyArgs),
    /// Partition ready tickets into conflict-free parallel batches.
    Tracks(TracksArgs),
    /// Recommend the next ticket(s) to work on.
    ///
    /// Picks are ranked by a score that prioritises urgent, unblocking work:
    /// 1000 x priority (p0=3..p3=0) + 10 x critical-path length + count of
    /// not-done tickets this one unblocks. Higher is more impactful to do next.
    Next(NextArgs),
    /// Atomically claim a ticket for an agent (race-safe, lease-based).
    Claim(ClaimArgs),
    /// Release a claimed ticket back to the ready pool.
    Release(ReleaseArgs),
    /// Show current claims (assignee, lease expiry, live/expired).
    Claims(ClaimsArgs),
    /// Delete a ticket (removes its file and comments).
    Delete(DeleteArgs),
    /// Rename a ticket's id: move the file, rewrite the id, repoint dependents.
    Rename(RenameArgs),
    /// Check repository setup (config, git, scope globs, base ref).
    Doctor,
    /// Print a short conceptual guide (scopes, tracks, scoring, guard, claims).
    Guide,
    /// Explain why two tickets can or cannot run in parallel.
    Why(WhyArgs),
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
    /// Do not install the bundled Claude skill.
    #[arg(long)]
    pub no_skill: bool,
}

/// `create` arguments.
#[derive(Args)]
#[command(group = ArgGroup::new("create_input").required(true).multiple(false).args(["title", "from"]))]
#[command(after_help = "Examples:\n  \
    tkt create --title \"Add auth\" --scope api --priority p1\n  \
    tkt create --from backlog.json   # batch from a JSON array; - reads stdin")]
pub struct CreateArgs {
    /// Ticket title (single create). Mutually exclusive with --from.
    #[arg(long)]
    pub title: Option<String>,
    /// Batch-create from a JSON array of ticket specs; `-` reads stdin. Each element:
    /// `{title, id?, status?, priority?, depends_on?, scopes?, paths?, tags?, body?}`.
    #[arg(long)]
    pub from: Option<String>,
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
    /// Preview what would be created without writing anything.
    #[arg(long)]
    pub dry_run: bool,
}

/// `set` arguments.
#[derive(Args)]
#[command(group = ArgGroup::new("body_op").multiple(false).args(["body", "body_file", "append_body", "append_body_file"]))]
pub struct SetArgs {
    /// Ticket id.
    pub id: String,
    /// New title.
    #[arg(long)]
    pub title: Option<String>,
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
    /// Tags to remove (repeatable or comma-separated).
    #[arg(long = "remove-tag", value_delimiter = ',')]
    pub remove_tag: Vec<String>,
    /// Explicit path globs to add (repeatable or comma-separated).
    #[arg(long = "add-path", value_delimiter = ',')]
    pub add_path: Vec<String>,
    /// Explicit path globs to remove (repeatable or comma-separated).
    #[arg(long = "remove-path", value_delimiter = ',')]
    pub remove_path: Vec<String>,
    /// Dependencies to add (repeatable or comma-separated); rejected if it would
    /// create a cycle, like `link`.
    #[arg(long = "add-dependency", value_delimiter = ',')]
    pub add_dependency: Vec<String>,
    /// Dependencies to remove (repeatable or comma-separated).
    #[arg(long = "remove-dependency", value_delimiter = ',')]
    pub remove_dependency: Vec<String>,
    /// Replace the body with this text (markdown bullets are fine).
    #[arg(long, allow_hyphen_values = true)]
    pub body: Option<String>,
    /// Replace the body from a file; `-` reads stdin (safe for rich markdown
    /// containing backticks, `$(...)`, etc. that a shell would otherwise mangle).
    #[arg(long = "body-file")]
    pub body_file: Option<String>,
    /// Append this text to the body.
    #[arg(long = "append-body", allow_hyphen_values = true)]
    pub append_body: Option<String>,
    /// Append the body from a file; `-` reads stdin.
    #[arg(long = "append-body-file")]
    pub append_body_file: Option<String>,
    /// Preview the change without writing anything.
    #[arg(long)]
    pub dry_run: bool,
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
    /// Read the ticket as committed on this git ref (e.g. a `tkt/<id>` branch)
    /// instead of the working tree.
    #[arg(long)]
    pub r#ref: Option<String>,
}

/// `list` arguments.
#[derive(Args)]
pub struct ListArgs {
    /// Filter by status.
    #[arg(long)]
    pub status: Option<String>,
    /// Filter to tickets declaring this scope.
    #[arg(long)]
    pub scope: Option<String>,
    /// Filter to tickets carrying this tag.
    #[arg(long)]
    pub tag: Option<String>,
    /// Filter by priority (p0..p3).
    #[arg(long)]
    pub priority: Option<String>,
    /// Hide completed (done) tickets.
    #[arg(long)]
    pub hide_done: bool,
}

/// `status` arguments.
#[derive(Args)]
pub struct StatusArgs {
    /// Scan `refs/heads/<prefix>*` branches and report each ticket's tip status,
    /// instead of the working tree.
    #[arg(long)]
    pub all_branches: bool,
    /// Branch namespace to scan with `--all-branches`; a ticket id is the branch
    /// name minus this prefix.
    #[arg(long, default_value = "tkt/")]
    pub prefix: String,
}

/// `watch` arguments.
#[derive(Args)]
pub struct WatchArgs {
    /// Ticket id.
    pub id: String,
    /// Target status to wait for (e.g. `review`). Also returns on `done`.
    #[arg(long)]
    pub until: String,
    /// Poll the ticket on this git ref instead of auto-resolving `<prefix><id>`
    /// (falling back to the working tree).
    #[arg(long)]
    pub r#ref: Option<String>,
    /// Branch namespace used to auto-resolve the ref when `--ref` is omitted.
    #[arg(long, default_value = "tkt/")]
    pub prefix: String,
    /// Seconds between polls.
    #[arg(long, default_value_t = 5)]
    pub interval: u64,
    /// Give up after this many seconds (exit 7). Omit to wait indefinitely.
    #[arg(long)]
    pub timeout: Option<u64>,
}

/// `comment` subcommand group.
#[derive(Args)]
pub struct CommentArgs {
    #[command(subcommand)]
    pub command: CommentCommand,
}

/// Subcommands under `comment`.
#[derive(Subcommand)]
pub enum CommentCommand {
    /// Append a comment to a ticket.
    Add(CommentAddArgs),
    /// List a ticket's comments.
    List(CommentListArgs),
}

/// `comment add` arguments.
#[derive(Args)]
#[command(group = ArgGroup::new("comment_body").required(true).multiple(false).args(["body", "body_file"]))]
pub struct CommentAddArgs {
    /// Ticket id.
    pub id: String,
    /// Comment author, recorded as `by` (e.g. the worker name).
    #[arg(long = "as")]
    pub as_: Option<String>,
    /// Reply to an existing comment id (threading).
    #[arg(long = "reply-to")]
    pub reply_to: Option<String>,
    /// Comment text (markdown).
    #[arg(long)]
    pub body: Option<String>,
    /// Comment text from a file; `-` reads stdin (shell-safe for rich markdown).
    #[arg(long = "body-file")]
    pub body_file: Option<String>,
}

/// `comment list` arguments.
#[derive(Args)]
pub struct CommentListArgs {
    /// Ticket id.
    pub id: String,
    /// Read comments as committed on this git ref instead of the working tree.
    #[arg(long)]
    pub r#ref: Option<String>,
}

/// `events` arguments.
#[derive(Args)]
pub struct EventsArgs {
    /// Only events newer than this event id (a cursor for resumable tailing).
    #[arg(long)]
    pub since: Option<String>,
    /// Only events for this ticket.
    #[arg(long)]
    pub ticket: Option<String>,
    /// Only events of this kind (e.g. `comment`, `status`, `claim`).
    #[arg(long = "type")]
    pub kind: Option<String>,
    /// Block until at least one matching event appears (wake-on-event); pair with
    /// `--since <last-id>` and loop to consume the stream without missing any.
    #[arg(long)]
    pub watch: bool,
    /// Poll interval in seconds while `--watch`ing.
    #[arg(long, default_value_t = 2)]
    pub interval: u64,
    /// With `--watch`, give up after this many seconds (exit 7).
    #[arg(long)]
    pub timeout: Option<u64>,
}

/// `next` arguments.
#[derive(Args)]
pub struct NextArgs {
    /// Return up to N picks (scope-disjoint by default).
    #[arg(long, default_value_t = 1)]
    pub parallel: usize,
    /// Allow picks whose scopes overlap (you resolve the shared-crate work); each
    /// pick is annotated with the scopes it shares with the others.
    #[arg(long)]
    pub allow_overlap: bool,
    /// Atomically claim the first pick that is still free (race-safe dispatch).
    /// Requires `--as`. Tries picks in order so a lost race falls through to the next.
    #[arg(long)]
    pub claim: bool,
    /// Identity to claim as (with `--claim`).
    #[arg(long = "as")]
    pub agent: Option<String>,
    /// Lease length in seconds for `--claim`.
    #[arg(long, default_value_t = ticketsplease_core::claim::DEFAULT_TTL_SECS)]
    pub ttl: u64,
}

/// `why` arguments.
#[derive(Args)]
pub struct WhyArgs {
    /// First ticket id.
    pub a: String,
    /// Second ticket id.
    pub b: String,
}

/// `claim` arguments.
#[derive(Args)]
#[command(after_help = "Examples:\n  \
    tkt claim my-ticket --as worker-1\n  \
    tkt next --claim --as worker-1   # atomically claim the best ready pick")]
pub struct ClaimArgs {
    /// Ticket id to claim.
    pub id: String,
    /// Identity of the claiming agent (recorded as the assignee).
    #[arg(long = "as")]
    pub agent: String,
    /// Lease length in seconds; once it expires the claim is reclaimable by others.
    #[arg(long, default_value_t = ticketsplease_core::claim::DEFAULT_TTL_SECS)]
    pub ttl: u64,
    /// Steal the claim even if another agent holds a live lease.
    #[arg(long)]
    pub force: bool,
}

/// `release` arguments.
#[derive(Args)]
pub struct ReleaseArgs {
    /// Ticket id to release.
    pub id: String,
    /// Identity releasing the claim; only the holder may release without --force.
    #[arg(long = "as")]
    pub agent: Option<String>,
    /// Release even if the claim is held by another agent.
    #[arg(long)]
    pub force: bool,
}

/// `delete` arguments.
#[derive(Args)]
pub struct DeleteArgs {
    /// Ticket id to delete.
    pub id: String,
}

/// `rename` arguments.
#[derive(Args)]
pub struct RenameArgs {
    /// Current ticket id.
    pub old: String,
    /// New ticket id (slug).
    pub new: String,
}

/// `reconcile` arguments.
#[derive(Args)]
pub struct ReconcileArgs {
    /// Branch namespace that marks per-ticket work branches (id = branch minus this).
    #[arg(long, default_value = "tkt/")]
    pub prefix: String,
}

/// `claims` arguments.
#[derive(Args)]
pub struct ClaimsArgs {
    /// Include claims recorded on `<prefix>*` branch tips, not just the working tree.
    #[arg(long)]
    pub all_branches: bool,
    /// Branch namespace to scan with --all-branches.
    #[arg(long, default_value = "tkt/")]
    pub prefix: String,
}

/// `guard` arguments.
#[derive(Args)]
#[command(after_help = "Examples:\n  \
    tkt guard tkt/my-feature --base main\n  \
    tkt guard tkt/my-feature --direct-only   # skip cargo reverse-dep expansion")]
pub struct GuardArgs {
    /// Branch (or ref) to guard.
    pub branch: String,
    /// Base ref to diff against (defaults to the config `default_base`).
    #[arg(long)]
    pub base: Option<String>,
    /// Explicit ticket id (otherwise inferred from the branch name).
    #[arg(long)]
    pub ticket: Option<String>,
    /// Gate on direct file/crate overlap only; skip cargo reverse-dependency
    /// expansion (and the transitive collisions/under-declarations it adds).
    #[arg(long, visible_alias = "no-reverse-deps")]
    pub direct_only: bool,
    /// Still compute and report transitive collisions (keeping the `cause` triage),
    /// but exit 0 when every conflict is transitive — only a direct overlap or an
    /// under-declaration fails the gate. Unlike `--direct-only`, the reverse-dep
    /// expansion still runs, so the report keeps its transitive collisions for review.
    #[arg(long)]
    pub ignore_transitive: bool,
    /// Ref to read the `[scopes]` config from (defaults to the base). Guards against
    /// a stale/empty config on the checked-out feature branch giving a false all-clear.
    #[arg(long)]
    pub config_ref: Option<String>,
    /// Branch namespace whose tips supply sibling tickets' in-flight status for
    /// collision detection (the branch-per-ticket flow).
    #[arg(long, default_value = "tkt/")]
    pub prefix: String,
}

/// `self-update` arguments.
#[derive(Args)]
pub struct SelfUpdateArgs {
    /// Update to a specific tag (default: the latest release).
    #[arg(long)]
    pub version: Option<String>,
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

empty_args!(ReadyArgs, LintArgs, MigrateArgs,);

/// `tracks` arguments.
#[derive(Args)]
pub struct TracksArgs {
    /// Cap each conflict-free batch to at most N tickets, splitting larger batches —
    /// so an orchestrator with N workers gets worker-sized fronts.
    #[arg(long)]
    pub parallel: Option<usize>,
}

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
pub struct SkillInstallArgs {
    /// Base directory to install the skill under (default `.claude/skills`).
    #[arg(long, default_value = ".claude/skills")]
    pub dir: String,
}

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
        Command::Status(a) => commands::status(repo, fmt, a),
        Command::Reconcile(a) => commands::reconcile(repo, fmt, a),
        Command::Watch(a) => commands::watch(repo, fmt, a),
        Command::Comment(a) => match &a.command {
            CommentCommand::Add(a) => commands::comment_add(repo, fmt, a),
            CommentCommand::List(a) => commands::comment_list(repo, fmt, a),
        },
        Command::Events(a) => commands::events(repo, fmt, a),
        Command::Lint(_) => commands::lint(repo, fmt),
        Command::Ready(_) => commands::ready(repo, fmt),
        Command::Tracks(a) => commands::tracks(repo, fmt, a),
        Command::Next(a) => commands::next(repo, fmt, a),
        Command::Claim(a) => commands::claim(repo, fmt, a),
        Command::Release(a) => commands::release(repo, fmt, a),
        Command::Claims(a) => commands::claims(repo, fmt, a),
        Command::Delete(a) => commands::delete(repo, fmt, a),
        Command::Rename(a) => commands::rename(repo, fmt, a),
        Command::Doctor => commands::doctor(repo, fmt),
        Command::Guide => commands::guide(fmt),
        Command::Why(a) => commands::why(repo, fmt, a),
        Command::Guard(a) => commands::guard(repo, fmt, a),
        Command::Migrate(_) => commands::migrate(repo, fmt),
        Command::Skill(a) => match &a.command {
            SkillCommand::Install(a) => commands::skill_install(repo, fmt, a),
        },
        Command::SelfUpdate(a) => commands::self_update(fmt, a),
    }
}
