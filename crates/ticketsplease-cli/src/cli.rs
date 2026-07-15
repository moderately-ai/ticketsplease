//! Command-line interface definition and top-level dispatch.

use std::path::PathBuf;

use clap::{ArgGroup, Args, Parser, Subcommand, ValueEnum};
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
    /// Auto-apply a detected drift repair (`migrate`) after this command instead of
    /// only nudging — interactive human sessions only (never in JSON / CI / non-TTY).
    /// A per-invocation form of `[maintenance] auto_migrate = true`.
    #[arg(long = "auto-doctor", global = true)]
    pub auto_doctor: bool,
    #[command(subcommand)]
    pub command: Command,
}

/// Top-level subcommands.
// The variants wrap clap arg structs of uneven size; the enum is parsed once per
// process, so the size spread is irrelevant and boxing would only fight the derive.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
pub enum Command {
    /// Initialize ticketsplease in a repository.
    Init(InitArgs),
    /// Create a new ticket.
    Create(CreateArgs),
    /// Update a ticket's fields.
    Set(SetArgs),
    /// Close a ticket as terminated-without-completion (won't-do, duplicate, obsolete,
    /// …) — terminal like `done`, but does not satisfy its dependents.
    Close(CloseArgs),
    /// Reopen a closed (or done) ticket into an active status, clearing its resolution.
    Reopen(ReopenArgs),
    /// Add or remove a dependency link between tickets.
    Link(LinkArgs),
    /// Show a single ticket.
    Show(ShowArgs),
    /// List tickets.
    List(ListArgs),
    /// Manage saved filter views (named `--where` expressions).
    View(ViewArgs),
    /// Roll up an initiative (a tag/filter): status & priority counts, % done, the
    /// ready frontier, and the blocked set.
    Rollup(RollupArgs),
    /// Export the dependency graph (JSON, or Graphviz DOT with `--dot`).
    Graph(GraphArgs),
    /// Print the critical prerequisite path (longest dependency chain) to a ticket.
    Path(PathArgs),
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
    /// Plan worker lanes: ordered per-worker queues that sequence conflicting work
    /// instead of dropping it, with a merge order.
    Lanes(LanesArgs),
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
    /// List the configured workflow states with their engine category and roles.
    States,
    /// Print a short conceptual guide (scopes, tracks, scoring, guard, claims).
    Guide,
    /// Explain why two tickets can or cannot run in parallel.
    Why(WhyArgs),
    /// Run a named recipe: a typed, parameterized procedure over these commands.
    Run(RunArgs),
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
    /// Do not install the bundled skill.
    #[arg(long)]
    pub no_skill: bool,
    /// Which agent harness to install the skill for (selects the directory convention):
    /// claude | codex | opencode | pi-agent.
    #[arg(long, value_enum, default_value = "claude")]
    pub harness: Harness,
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
    /// Batch-create from a JSON array of ticket specs, or a TOML `[[ticket]]`
    /// document (chosen by `.json`/`.toml` extension; `-` reads stdin as JSON). Each
    /// spec: `{title, id?, status?, priority?, depends_on?, related?, scopes?, paths?,
    /// tags?, body?}`.
    #[arg(long)]
    pub from: Option<String>,
    /// Explicit id (slug); defaults to a slug of the title.
    #[arg(long)]
    pub id: Option<String>,
    /// Status: todo | ready | in-progress | blocked | review | done | closed.
    #[arg(long, default_value = "todo")]
    pub status: String,
    /// Priority: p0 | p1 | p2 | p3.
    #[arg(long, default_value = "p2")]
    pub priority: String,
    /// Dependency ticket ids (repeatable or comma-separated).
    #[arg(long = "depends-on", value_delimiter = ',')]
    pub depends_on: Vec<String>,
    /// Non-blocking related ticket ids (repeatable or comma-separated).
    #[arg(long = "related", value_delimiter = ',')]
    pub related: Vec<String>,
    /// Exclusively-claimed scope names (repeatable or comma-separated).
    #[arg(long = "scope", value_delimiter = ',')]
    pub scopes: Vec<String>,
    /// Shared (additive) scope claims — areas this ticket only appends to, safe to
    /// co-edit with other shared claimants (repeatable or comma-separated).
    #[arg(long = "shared-scope", value_delimiter = ',')]
    pub shared_scopes: Vec<String>,
    /// Explicit path globs (repeatable or comma-separated).
    #[arg(long = "path", value_delimiter = ',')]
    pub paths: Vec<String>,
    /// Tags (repeatable or comma-separated).
    #[arg(long = "tag", value_delimiter = ',')]
    pub tags: Vec<String>,
    /// Markdown body.
    #[arg(long, default_value = "")]
    pub body: String,
    /// Scaffold the body from `.ticketsplease/templates/<name>.md` ({{title}}/{{id}}
    /// substituted). Ignored if --body is non-empty.
    #[arg(long)]
    pub template: Option<String>,
    /// Preview what would be created without writing anything.
    #[arg(long)]
    pub dry_run: bool,
    /// Skip write-time validation (undefined scopes, dangling related/dependency ids).
    /// Use for a forward reference to a ticket or scope you will add next.
    #[arg(long)]
    pub no_validate: bool,
}

/// `set` arguments.
#[derive(Args)]
#[command(group = ArgGroup::new("body_op").multiple(false).args(["body", "body_file", "append_body", "append_body_file"]))]
#[command(
    after_help = "Single: pass an id. Bulk: pass --where/--view to edit every matching\n\
    ticket at once (--title and body edits are single-target only and rejected in bulk)."
)]
pub struct SetArgs {
    /// Ticket id (single-ticket edit). Omit when using --where/--view for a bulk edit.
    pub id: Option<String>,
    /// Bulk edit: apply the mutations to every ticket matching this `--where`
    /// expression (see `tkt list` for the grammar).
    #[arg(long = "where")]
    pub where_: Option<String>,
    /// Bulk edit: apply to every ticket matching a saved view (ANDs with --where).
    #[arg(long)]
    pub view: Option<String>,
    /// New title.
    #[arg(long)]
    pub title: Option<String>,
    /// New status.
    #[arg(long)]
    pub status: Option<String>,
    /// Close reason — only valid alongside `--status closed`: duplicate | wontdo |
    /// obsolete | superseded | cancelled. Cleared automatically when the ticket
    /// later leaves `closed`.
    #[arg(long)]
    pub reason: Option<String>,
    /// One-line note explaining a close — only valid alongside `--status closed`.
    #[arg(long, allow_hyphen_values = true)]
    pub note: Option<String>,
    /// New priority.
    #[arg(long)]
    pub priority: Option<String>,
    /// Scopes to add (repeatable or comma-separated).
    #[arg(long = "add-scope", value_delimiter = ',')]
    pub add_scope: Vec<String>,
    /// Scopes to remove (repeatable or comma-separated).
    #[arg(long = "remove-scope", value_delimiter = ',')]
    pub remove_scope: Vec<String>,
    /// Shared (additive) scope claims to add (repeatable or comma-separated).
    #[arg(long = "add-shared-scope", value_delimiter = ',')]
    pub add_shared_scope: Vec<String>,
    /// Shared scope claims to remove (repeatable or comma-separated).
    #[arg(long = "remove-shared-scope", value_delimiter = ',')]
    pub remove_shared_scope: Vec<String>,
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
    /// Non-blocking related links to add (repeatable or comma-separated).
    #[arg(long = "add-related", value_delimiter = ',')]
    pub add_related: Vec<String>,
    /// Non-blocking related links to remove (repeatable or comma-separated).
    #[arg(long = "remove-related", value_delimiter = ',')]
    pub remove_related: Vec<String>,
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
    /// Bypass workflow transition enforcement for this change (records `forced` in the
    /// emitted status event). No effect unless `[workflow] enforce_transitions` is on.
    #[arg(long)]
    pub force: bool,
    /// Preview the change without writing anything.
    #[arg(long)]
    pub dry_run: bool,
    /// Skip write-time validation of added scopes/related/dependency ids.
    #[arg(long)]
    pub no_validate: bool,
}

/// `close` arguments.
#[derive(Args)]
pub struct CloseArgs {
    /// Ticket id to close.
    pub id: String,
    /// Why it is being closed: duplicate | wontdo | obsolete | superseded | cancelled.
    #[arg(long)]
    pub reason: Option<String>,
    /// One-line note explaining the close.
    #[arg(long, allow_hyphen_values = true)]
    pub note: Option<String>,
    /// Bypass workflow transition enforcement for this close.
    #[arg(long)]
    pub force: bool,
    /// Preview the change without writing anything.
    #[arg(long)]
    pub dry_run: bool,
}

/// `reopen` arguments.
#[derive(Args)]
pub struct ReopenArgs {
    /// Ticket id to reopen (must currently be terminal: closed or done).
    pub id: String,
    /// Active status to reopen into. Defaults to `todo`.
    #[arg(long, default_value = "todo")]
    pub status: String,
    /// Bypass workflow transition enforcement for this reopen.
    #[arg(long)]
    pub force: bool,
    /// Preview the change without writing anything.
    #[arg(long)]
    pub dry_run: bool,
}

/// `link` arguments.
#[derive(Args)]
#[command(group = ArgGroup::new("link_target").required(true).multiple(false).args(["depends_on", "related"]))]
pub struct LinkArgs {
    /// Ticket that gains (or loses) a link.
    pub id: String,
    /// Add/remove a hard dependency (blocks scheduling; cycle-checked).
    #[arg(long = "depends-on")]
    pub depends_on: Option<String>,
    /// Add/remove a soft, non-blocking related link (ignored by scheduling; never
    /// cycle-checked).
    #[arg(long)]
    pub related: Option<String>,
    /// Remove the link instead of adding it.
    #[arg(long)]
    pub remove: bool,
    /// Skip write-time validation that the link target exists (for a forward reference).
    #[arg(long)]
    pub no_validate: bool,
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
#[command(after_help = "Filter expression (--where) grammar:\n  \
    field:value combined with AND / OR / NOT and parentheses.\n  \
    fields: status priority tag scope assignee id dep related\n  \
    e.g. --where 'tag:dialect AND NOT status:done'\n  \
         --where '(priority:p0 OR priority:p1) AND scope:core'")]
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
    /// Boolean filter expression: `field:value` joined by AND/OR/NOT and parens
    /// (fields: status priority tag scope assignee id dep related). Composes (AND)
    /// with the single-axis flags above.
    #[arg(long = "where")]
    pub where_: Option<String>,
    /// Apply a saved view's expression (see `tkt view`); composes (AND) with `--where`.
    #[arg(long)]
    pub view: Option<String>,
    /// Hide completed (done) tickets.
    #[arg(long)]
    pub hide_done: bool,
}

/// `view` subcommand group.
#[derive(Args)]
pub struct ViewArgs {
    #[command(subcommand)]
    pub command: ViewCommand,
}

/// Subcommands under `view`.
#[derive(Subcommand)]
pub enum ViewCommand {
    /// Save (or overwrite) a named filter expression.
    Save(ViewSaveArgs),
    /// List saved views.
    List,
    /// Print a single view's expression.
    Show(ViewShowArgs),
    /// Delete a saved view.
    Delete(ViewShowArgs),
}

/// `rollup` arguments. With no selector, rolls up the whole board.
#[derive(Args)]
pub struct RollupArgs {
    /// Restrict to tickets carrying this tag (the usual initiative key).
    #[arg(long)]
    pub tag: Option<String>,
    /// Restrict with a `--where` expression (ANDs with --tag/--view).
    #[arg(long = "where")]
    pub where_: Option<String>,
    /// Restrict with a saved view (ANDs with --tag/--where).
    #[arg(long)]
    pub view: Option<String>,
}

/// `graph` arguments. Selectors restrict the exported subgraph (metrics stay
/// board-global). No selector = the whole graph.
#[derive(Args)]
pub struct GraphArgs {
    /// Restrict to tickets carrying this tag.
    #[arg(long)]
    pub tag: Option<String>,
    /// Restrict with a `--where` expression (ANDs with --tag/--view).
    #[arg(long = "where")]
    pub where_: Option<String>,
    /// Restrict with a saved view (ANDs with --tag/--where).
    #[arg(long)]
    pub view: Option<String>,
    /// Emit Graphviz DOT (dependencies solid, related dashed) instead of JSON/human.
    #[arg(long)]
    pub dot: bool,
}

/// `path` arguments.
#[derive(Args)]
pub struct PathArgs {
    /// Ticket id to trace the critical prerequisite path to.
    pub id: String,
}

/// `view save` arguments.
#[derive(Args)]
pub struct ViewSaveArgs {
    /// View name.
    pub name: String,
    /// The `--where` expression to store (validated before saving).
    pub expr: String,
}

/// `view show` / `view delete` arguments.
#[derive(Args)]
pub struct ViewShowArgs {
    /// View name.
    pub name: String,
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
    /// pick is annotated with the scopes it shares with the others. Alias for
    /// `--max-overlap any`.
    #[arg(long)]
    pub allow_overlap: bool,
    /// Per-pair overlap budget for filling N picks: `0` (default) = compatible only;
    /// `K` = also admit the cheapest overlaps costing ≤ K per pair; `any` = unbounded.
    #[arg(long = "max-overlap", default_value = "0")]
    pub max_overlap: String,
    /// In-flight ticket ids to stay compatible with — picks conflicting with these
    /// (beyond the budget) are dropped. Omit to default to every in-progress ticket
    /// with a live claim, so a dispatch loop needs no args.
    #[arg(long = "running", visible_alias = "avoid", value_delimiter = ',')]
    pub running: Vec<String>,
    /// Treat every scope claim as shared (additive) — collapse conflicts and pack
    /// picks; you reconcile at merge.
    #[arg(long, conflicts_with = "strict")]
    pub assume_shared: bool,
    /// Treat every scope claim as exclusive — ignore `shared_scopes` and scope weights
    /// (the conservative view, on demand).
    #[arg(long)]
    pub strict: bool,
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
    /// Gate on a declared-area overlap with an open sibling too (exit 6), not just an
    /// under-declaration — restoring the pre-warn-default behaviour. Overrides
    /// `[guard] gate_collisions`.
    #[arg(long)]
    pub strict: bool,
    /// Downgrade a declared-area overlap to a non-failing WARN (exit 0) even when
    /// `[guard] gate_collisions` is on. Overrides the config.
    #[arg(long, conflicts_with = "strict")]
    pub warn_collisions: bool,
    /// Show which changed files hit each affected, under-declared, and colliding scope.
    #[arg(long)]
    pub explain: bool,
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

empty_args!(ReadyArgs, LintArgs,);

/// `migrate` arguments.
#[derive(Args)]
pub struct MigrateArgs {
    /// Rewrite tickets stuck in a since-renamed/removed state to a current one:
    /// `--remap old=new` (repeatable). `new` must be a defined workflow state. Use when a
    /// config change renames or drops a state that live tickets still occupy.
    #[arg(long = "remap", value_name = "OLD=NEW")]
    pub remap: Vec<String>,
    /// Preview the migration without writing: report what *would* change (tickets
    /// backfilled/remapped, and whether the skill link would be repaired) and leave
    /// every file untouched.
    #[arg(long = "dry-run")]
    pub dry_run: bool,
}

/// `tracks` arguments.
#[derive(Args)]
pub struct TracksArgs {
    /// Cap each batch to at most N tickets, splitting larger batches — so an
    /// orchestrator with N workers gets worker-sized fronts.
    #[arg(long)]
    pub parallel: Option<usize>,
    /// Per-pair overlap budget: `0` (default) = strictly conflict-free batches; `K` =
    /// let tickets that conflict by ≤ K per pair share a batch; `any` = unbounded.
    #[arg(long = "max-overlap", default_value = "0")]
    pub max_overlap: String,
    /// Print only the safe parallel width (largest set runnable at once within the
    /// budget) — how many workers you can usefully spin up right now.
    #[arg(long)]
    pub width: bool,
    /// Emit the conflict matrix (every ready pair with its conflicting scopes and cost)
    /// instead of batches, so you can do your own assignment.
    #[arg(long)]
    pub overlap_matrix: bool,
    /// Treat every scope claim as shared (additive) — one batch; reconcile at merge.
    #[arg(long, conflicts_with = "strict")]
    pub assume_shared: bool,
    /// Treat every scope claim as exclusive — ignore `shared_scopes` and scope weights.
    #[arg(long)]
    pub strict: bool,
}

/// `lanes` arguments.
#[derive(Args)]
pub struct LanesArgs {
    /// Number of worker lanes (default: the safe parallel width).
    #[arg(long)]
    pub parallel: Option<usize>,
    /// Per-pair overlap budget tolerated within a concurrent round (see `tracks`).
    #[arg(long = "max-overlap", default_value = "0")]
    pub max_overlap: String,
    /// Treat every scope claim as shared (additive) — collapse conflicts.
    #[arg(long, conflicts_with = "strict")]
    pub assume_shared: bool,
    /// Treat every scope claim as exclusive — ignore `shared_scopes` and scope weights.
    #[arg(long)]
    pub strict: bool,
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
    /// Link this project's `.claude/skills/ticketsplease` to the canonical skill copy
    /// (or, with `--copy`, write a committable real copy).
    Install(SkillInstallArgs),
    /// Refresh the canonical skill copy from this binary (run by the installer; no repo
    /// needed). Every linked project then sees the update.
    Sync,
}

/// The agent harness to install the skill for. Every harness consumes the identical
/// `SKILL.md` + `references/` layout — only the discovery directory differs — so this
/// selects the install path, reusing the same canonical-copy + symlink machinery.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Harness {
    /// Claude Code — `.claude/skills` (project), `~/.claude/skills` (global).
    Claude,
    /// OpenAI Codex — `.agents/skills`, the cross-tool Agent Skills standard directory
    /// (also read by opencode and Pi); `~/.agents/skills` (global).
    Codex,
    /// opencode — `.opencode/skills` (project), `~/.config/opencode/skills` (global).
    Opencode,
    /// Pi coding agent — `.pi/skills` (project), `~/.pi/agent/skills` (global).
    #[value(name = "pi-agent", alias = "pi")]
    PiAgent,
}

impl Harness {
    /// Project-scoped skills base directory, relative to the repo root.
    #[must_use]
    pub fn project_base_dir(self) -> &'static str {
        match self {
            Harness::Claude => ".claude/skills",
            Harness::Codex => ".agents/skills",
            Harness::Opencode => ".opencode/skills",
            Harness::PiAgent => ".pi/skills",
        }
    }

    /// User-global skills directory, relative to `$HOME`, where the harness
    /// auto-discovers skills across every project.
    #[must_use]
    pub fn global_base_dir(self) -> &'static str {
        match self {
            Harness::Claude => ".claude/skills",
            Harness::Codex => ".agents/skills",
            Harness::Opencode => ".config/opencode/skills",
            Harness::PiAgent => ".pi/agent/skills",
        }
    }

    /// Human label for install output.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Harness::Claude => "Claude Code",
            Harness::Codex => "OpenAI Codex",
            Harness::Opencode => "opencode",
            Harness::PiAgent => "Pi",
        }
    }

    /// Canonical machine name (matches the `--harness` value), for JSON output.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Harness::Claude => "claude",
            Harness::Codex => "codex",
            Harness::Opencode => "opencode",
            Harness::PiAgent => "pi-agent",
        }
    }
}

/// `skill install` arguments.
#[derive(Args)]
pub struct SkillInstallArgs {
    /// Which agent harness to install the skill for (selects the directory convention):
    /// claude | codex | opencode | pi-agent.
    #[arg(long, value_enum, default_value = "claude")]
    pub harness: Harness,
    /// Install into the harness's user-global skills directory (available in every
    /// project) instead of this repo. Not valid with --dir.
    #[arg(long)]
    pub global: bool,
    /// Explicit project base-directory override (advanced); defaults to the harness's
    /// convention. Ignored with --global.
    #[arg(long)]
    pub dir: Option<String>,
    /// Write a committable real copy instead of a symlink to the canonical copy.
    #[arg(long)]
    pub copy: bool,
}

/// `run` arguments.
#[derive(Args)]
#[command(after_help = "Examples:\n  \
    tkt run supersede --arg id=auth --arg with=auth-api,auth-ui,auth-db\n  \
    tkt run --list            # list recipes\n  \
    tkt run supersede --describe   # show a recipe's typed inputs/outputs\n  \
    tkt run supersede --arg id=auth --arg with=... --dry-run")]
pub struct RunArgs {
    /// Recipe name to run (omit with --list).
    pub name: Option<String>,
    /// An input value as `key=value` (repeatable).
    #[arg(long = "arg", value_name = "K=V")]
    pub arg: Vec<String>,
    /// Preview the resolved steps without executing anything.
    #[arg(long)]
    pub dry_run: bool,
    /// List the available recipes and exit.
    #[arg(long)]
    pub list: bool,
    /// Print the recipe's typed input/output contract and exit.
    #[arg(long)]
    pub describe: bool,
}

/// Dispatch a parsed CLI invocation.
pub fn run(cli: Cli) -> Result<()> {
    let repo = cli.repo.as_path();
    let fmt = cli.format;
    match &cli.command {
        Command::Init(a) => commands::init(repo, fmt, a),
        Command::Create(a) => commands::create(repo, fmt, a),
        Command::Set(a) => commands::set(repo, fmt, a),
        Command::Close(a) => commands::close(repo, fmt, a),
        Command::Reopen(a) => commands::reopen(repo, fmt, a),
        Command::Link(a) => commands::link(repo, fmt, a),
        Command::Show(a) => commands::show(repo, fmt, a),
        Command::List(a) => commands::list(repo, fmt, a),
        Command::View(a) => match &a.command {
            ViewCommand::Save(a) => commands::view_save(repo, fmt, a),
            ViewCommand::List => commands::view_list(repo, fmt),
            ViewCommand::Show(a) => commands::view_show(repo, fmt, a),
            ViewCommand::Delete(a) => commands::view_delete(repo, fmt, a),
        },
        Command::Rollup(a) => commands::rollup(repo, fmt, a),
        Command::Graph(a) => commands::graph(repo, fmt, a),
        Command::Path(a) => commands::path(repo, fmt, a),
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
        Command::Lanes(a) => commands::lanes(repo, fmt, a),
        Command::Next(a) => commands::next(repo, fmt, a),
        Command::Claim(a) => commands::claim(repo, fmt, a),
        Command::Release(a) => commands::release(repo, fmt, a),
        Command::Claims(a) => commands::claims(repo, fmt, a),
        Command::Delete(a) => commands::delete(repo, fmt, a),
        Command::Rename(a) => commands::rename(repo, fmt, a),
        Command::Doctor => commands::doctor(repo, fmt),
        Command::States => commands::states(repo, fmt),
        Command::Guide => commands::guide(fmt),
        Command::Why(a) => commands::why(repo, fmt, a),
        Command::Run(a) => commands::run(repo, fmt, a),
        Command::Guard(a) => commands::guard(repo, fmt, a),
        Command::Migrate(a) => commands::migrate(repo, fmt, a),
        Command::Skill(a) => match &a.command {
            SkillCommand::Install(a) => commands::skill_install(repo, fmt, a),
            SkillCommand::Sync => commands::skill_sync(fmt),
        },
        Command::SelfUpdate(a) => commands::self_update(fmt, a),
    }
}
