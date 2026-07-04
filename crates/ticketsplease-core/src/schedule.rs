//! Scheduling: the dependency-satisfied `ready` queue, conflict-free parallel
//! `tracks`, and the scored `next` recommendation.
//!
//! The dependency graph is built and validated (dangling deps, cycles) up front.
//! All ordering is deterministic — sorted by `(priority, id)` and stable greedy
//! colouring — so identical inputs yield byte-identical output (R13).

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::error::{Error, Result};
use crate::lint::Diagnostic;
use crate::ticket::{Priority, Status, Ticket};

/// Validated view over a set of tickets.
struct Graph<'a> {
    by_id: BTreeMap<&'a str, &'a Ticket>,
    /// Reverse edges: id -> tickets that depend on it (sorted, deduped).
    dependents: BTreeMap<&'a str, Vec<&'a str>>,
}

impl<'a> Graph<'a> {
    fn build(tickets: &'a [Ticket]) -> Result<Self> {
        let mut by_id: BTreeMap<&str, &Ticket> = BTreeMap::new();
        for t in tickets {
            if by_id.insert(t.id.as_str(), t).is_some() {
                return Err(Error::Invalid(format!("duplicate ticket id `{}`", t.id)));
            }
        }
        for t in tickets {
            for d in &t.dependencies {
                if !by_id.contains_key(d.as_str()) {
                    return Err(Error::Invalid(format!(
                        "ticket `{}` depends on missing ticket `{d}`",
                        t.id
                    )));
                }
            }
        }
        if let Some(cycle) = find_cycle(&by_id) {
            return Err(Error::Cycle(cycle.join(" -> ")));
        }
        // Reverse edges for `next` scoring: id -> unfinished tickets that depend on it.
        // Terminal dependents (done or closed) are excluded so "downstream work this
        // unblocks" counts only work that is actually still waiting.
        let mut dependents: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for t in tickets {
            dependents.entry(t.id.as_str()).or_default();
        }
        for t in tickets {
            if t.status.is_terminal() {
                continue;
            }
            for d in &t.dependencies {
                dependents
                    .entry(d.as_str())
                    .or_default()
                    .push(t.id.as_str());
            }
        }
        for v in dependents.values_mut() {
            v.sort_unstable();
            v.dedup();
        }
        Ok(Self { by_id, dependents })
    }

    /// Dispatchable = open for dispatch (todo/ready) with every dependency satisfied
    /// (each prerequisite `done`). A `closed` (abandoned) dependency does *not* satisfy,
    /// so the dependent is held out of dispatch — surfaced as orphaned, never silently
    /// scheduled onto dropped work.
    fn is_dispatchable(&self, t: &Ticket) -> bool {
        t.status.is_dispatchable()
            && t.dependencies.iter().all(|d| {
                self.by_id
                    .get(d.as_str())
                    .is_some_and(|dep| dep.status.completes_dependencies())
            })
    }

    fn dispatchable(&self, tickets: &'a [Ticket]) -> Vec<&'a Ticket> {
        let mut out: Vec<&Ticket> = tickets.iter().filter(|t| self.is_dispatchable(t)).collect();
        out.sort_by(|a, b| (a.priority, &a.id).cmp(&(b.priority, &b.id)));
        out
    }
}

/// The dependency-satisfied, priority-ordered queue (R5). Cycles are an error.
pub fn ready(tickets: &[Ticket]) -> Result<Vec<&Ticket>> {
    let graph = Graph::build(tickets)?;
    Ok(graph.dispatchable(tickets))
}

/// Partition the ready set into parallel batches (R6): no two tickets in a batch
/// conflict beyond `max_overlap`. With the default `max_overlap = 0` a batch is
/// strictly conflict-free (two tickets that conflict never share a batch); a higher
/// per-pair budget lets cheaply-overlapping tickets share a batch. Dependency
/// *ordering* needs no handling here — only dispatchable tickets (every dependency
/// already done) are batched, so by construction none depend on each other; the sole
/// hazard is scope overlap. Deterministic greedy (Welsh–Powell) colouring.
pub fn tracks<'a>(
    tickets: &'a [Ticket],
    max_overlap: i64,
    weights: &BTreeMap<String, i64>,
) -> Result<Vec<Vec<&'a Ticket>>> {
    let graph = Graph::build(tickets)?;
    let nodes = graph.dispatchable(tickets);
    let n = nodes.len();
    if n == 0 {
        return Ok(Vec::new());
    }

    let mut adj: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); n];
    for i in 0..n {
        for j in (i + 1)..n {
            // An edge (must-separate) only when the pair's conflict cost exceeds the
            // tolerated budget; pairs within budget may share a batch.
            if conflict_cost(nodes[i], nodes[j], weights) > max_overlap {
                adj[i].insert(j);
                adj[j].insert(i);
            }
        }
    }

    // Welsh–Powell: colour by descending degree, tie-break (priority, id).
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&x, &y| {
        adj[y]
            .len()
            .cmp(&adj[x].len())
            .then_with(|| (nodes[x].priority, &nodes[x].id).cmp(&(nodes[y].priority, &nodes[y].id)))
    });

    let mut colour = vec![usize::MAX; n];
    for &i in &order {
        let used: BTreeSet<usize> = adj[i]
            .iter()
            .filter_map(|&j| (colour[j] != usize::MAX).then_some(colour[j]))
            .collect();
        let mut c = 0;
        while used.contains(&c) {
            c += 1;
        }
        colour[i] = c;
    }

    let batch_count = colour.iter().copied().max().map_or(0, |m| m + 1);
    let mut batches: Vec<Vec<&Ticket>> = vec![Vec::new(); batch_count];
    for (i, &c) in colour.iter().enumerate() {
        batches[c].push(nodes[i]);
    }
    for batch in &mut batches {
        batch.sort_by(|a, b| (a.priority, &a.id).cmp(&(b.priority, &b.id)));
    }
    Ok(batches)
}

/// A worker-lane plan: ≤ `parallel` lanes, each an ordered queue for one worker, plus
/// the round-by-round merge order.
pub struct LanePlan<'a> {
    /// One ordered queue per worker (non-empty lanes only).
    pub lanes: Vec<Vec<&'a Ticket>>,
    /// The recommended merge order: complete an earlier round everywhere before the
    /// next round's heads start (each later round conflicts with an earlier one).
    pub merge_order: Vec<&'a Ticket>,
}

/// Plan up to `parallel` worker lanes for the ready set. Unlike `tracks` — which emits
/// conflict-free *rounds* that wait for a recompute — this assigns work to fixed
/// worker queues so conflicting tickets are *sequenced onto one lane* (later rebases
/// on earlier) instead of dropped. Built by capping each `tracks` round to `parallel`
/// concurrent tickets and transposing the rounds across lanes; `max_overlap` tolerates
/// cheap overlaps within a round just like `tracks`.
pub fn lanes<'a>(
    tickets: &'a [Ticket],
    parallel: usize,
    max_overlap: i64,
    weights: &BTreeMap<String, i64>,
) -> Result<LanePlan<'a>> {
    let n = parallel.max(1);
    let batches = tracks(tickets, max_overlap, weights)?;
    // Each conflict-free (within budget) batch, capped to n, becomes one or more rounds
    // of ≤ n concurrently-runnable tickets.
    let rounds: Vec<Vec<&Ticket>> = batches
        .iter()
        .flat_map(|b| b.chunks(n).map(<[&Ticket]>::to_vec))
        .collect();
    // Transpose rounds into lanes: lane i gets the i-th ticket of each round, so a
    // worker's queue runs sequentially while round r stays concurrent across lanes.
    let mut lanes: Vec<Vec<&Ticket>> = vec![Vec::new(); n];
    for round in &rounds {
        for (i, &t) in round.iter().enumerate() {
            lanes[i].push(t);
        }
    }
    lanes.retain(|l| !l.is_empty());
    let merge_order: Vec<&Ticket> = rounds.iter().flatten().copied().collect();
    Ok(LanePlan { lanes, merge_order })
}

/// A scored next-pick.
pub struct Pick<'a> {
    /// The recommended ticket.
    pub ticket: &'a Ticket,
    /// Total score (higher is better).
    pub score: i64,
    /// Other picks in the returned set that share a scope with this one. Empty
    /// unless `allow_overlap` let an overlapping pick through — it lets the
    /// implementor judge whether the shared-crate overlap is tolerable.
    pub conflicts_with: Vec<PickConflict>,
}

/// A scope overlap between two returned picks (surfaced when an overlap budget let an
/// overlapping pick through).
#[derive(Debug, Clone, Serialize)]
pub struct PickConflict {
    /// The other pick's ticket id.
    pub ticket: String,
    /// Conflicting scopes shared with that pick (claimed, not shared-by-both).
    pub scopes: Vec<String>,
    /// The conflict cost with that pick (currently the number of conflicting scopes).
    pub cost: i64,
}

/// Recommend the next ticket(s): score by priority, downstream critical-path
/// length, and remaining-downstream unblock count. With `parallel > 1`, return that
/// many picks, highest-scored first (R7). Picks are filled in two passes: first the
/// highest-scored mutually-compatible (conflict-cost 0) picks, then — when a positive
/// `max_overlap` budget leaves slots — the lowest-cost overlaps within that per-pair
/// budget, so the caller fills its N workers least-riskily instead of idling them.
/// `max_overlap` is a per-pair cost cap (`0` = compatible only, `i64::MAX` = unbounded).
/// Each overlapping pick is annotated with the conflict so the caller can judge it.
/// `running` is the in-flight set: candidates that conflict with any of them beyond the
/// budget are dropped, so a freed worker only gets work compatible with what is live.
pub fn next<'a>(
    tickets: &'a [Ticket],
    parallel: usize,
    max_overlap: i64,
    weights: &BTreeMap<String, i64>,
    running: &[&Ticket],
) -> Result<Vec<Pick<'a>>> {
    let graph = Graph::build(tickets)?;
    let mut nodes = graph.dispatchable(tickets);
    // Drop candidates that conflict (beyond the budget) with an already-in-flight
    // ticket, so a freed worker is offered work compatible with what's still running.
    if !running.is_empty() {
        nodes.retain(|t| {
            running
                .iter()
                .all(|r| conflict_cost(t, r, weights) <= max_overlap)
        });
    }
    if nodes.is_empty() {
        return Ok(Vec::new());
    }

    let mut memo: BTreeMap<&str, i64> = BTreeMap::new();
    let mut scores: BTreeMap<&str, i64> = BTreeMap::new();
    for &t in &nodes {
        let id = t.id.as_str();
        let s = 1000 * priority_value(t.priority)
            + 10 * critical_path(id, &graph.dependents, &mut memo)
            + downstream_count(id, &graph.dependents);
        scores.insert(id, s);
    }

    nodes.sort_by(|a, b| {
        scores[b.id.as_str()]
            .cmp(&scores[a.id.as_str()])
            .then_with(|| a.id.cmp(&b.id))
    });

    let want = parallel.max(1);
    let mut chosen: Vec<&Ticket> = Vec::new();
    let mut taken = vec![false; nodes.len()];
    // Pass 1: highest-scored picks that are compatible (cost 0) with the chosen set.
    for (i, &t) in nodes.iter().enumerate() {
        if chosen.len() >= want {
            break;
        }
        if chosen.iter().all(|&c| conflict_cost(c, t, weights) == 0) {
            chosen.push(t);
            taken[i] = true;
        }
    }
    // Pass 2: fill remaining slots with the lowest per-pair cost overlaps within budget.
    while chosen.len() < want && max_overlap > 0 {
        let mut best: Option<(i64, usize)> = None;
        for (i, &t) in nodes.iter().enumerate() {
            if taken[i] {
                continue;
            }
            let marginal = chosen
                .iter()
                .map(|&c| conflict_cost(c, t, weights))
                .max()
                .unwrap_or(0);
            if marginal <= max_overlap && best.map_or(true, |(bc, _)| marginal < bc) {
                best = Some((marginal, i));
            }
        }
        match best {
            Some((_, i)) => {
                chosen.push(nodes[i]);
                taken[i] = true;
            }
            None => break,
        }
    }

    // Surface, per pick, the conflicting scopes (and weighted cost) with other picks.
    let picks = chosen
        .iter()
        .map(|&t| {
            let conflicts_with = chosen
                .iter()
                .filter(|&&o| o.id != t.id)
                .filter_map(|&o| {
                    let scopes = conflicting_scopes(t, o);
                    (!scopes.is_empty()).then(|| PickConflict {
                        cost: scopes
                            .iter()
                            .map(|s| weights.get(s).copied().unwrap_or(1))
                            .sum(),
                        ticket: o.id.clone(),
                        scopes,
                    })
                })
                .collect();
            Pick {
                ticket: t,
                score: scores[t.id.as_str()],
                conflicts_with,
            }
        })
        .collect();
    Ok(picks)
}

/// Link-level diagnostics (dangling dependencies, cycles) for `lint`.
pub fn link_diagnostics(tickets: &[Ticket]) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let by_id: BTreeMap<&str, &Ticket> = tickets.iter().map(|t| (t.id.as_str(), t)).collect();
    for t in tickets {
        for d in &t.dependencies {
            match by_id.get(d.as_str()) {
                None => out.push(Diagnostic {
                    file: format!("{}.md", t.id),
                    id: Some(t.id.clone()),
                    code: "missing-dep",
                    message: format!("depends on missing ticket `{d}`"),
                }),
                // A live ticket whose prerequisite was `closed` (terminal but not
                // completing) is orphaned: it will never be dispatched and never
                // deadlock-visibly — surface it like a dead dependency so it is
                // re-pointed, waived, or closed rather than silently stuck.
                Some(dep)
                    if !t.status.is_terminal()
                        && dep.status.is_terminal()
                        && !dep.status.completes_dependencies() =>
                {
                    out.push(Diagnostic {
                        file: format!("{}.md", t.id),
                        id: Some(t.id.clone()),
                        code: "orphaned-by-closed-dep",
                        message: format!(
                            "depends on `{d}` which was closed without completing \
                             (re-point, waive, or close this ticket)"
                        ),
                    });
                }
                Some(_) => {}
            }
        }
        // Related links are non-blocking (no ordering), so a dangling one is a typo
        // to surface but never a cycle — only existence is checked.
        for r in &t.related {
            if !by_id.contains_key(r.as_str()) {
                out.push(Diagnostic {
                    file: format!("{}.md", t.id),
                    id: Some(t.id.clone()),
                    code: "missing-related",
                    message: format!("related to missing ticket `{r}`"),
                });
            }
        }
    }
    if let Some(cycle) = find_cycle(&by_id) {
        out.push(Diagnostic {
            file: "(dependency graph)".to_string(),
            id: None,
            code: "cycle",
            message: format!("dependency cycle: {}", cycle.join(" -> ")),
        });
    }
    out
}

/// Error with [`Error::Cycle`] if the dependency graph contains a cycle. Lets
/// `link` reject a cycle-forming edge at write time (exit 5) instead of letting the
/// corrupt graph surface later in `ready`/`tracks`/`next`. Dangling dependency
/// targets are ignored here (they are not a cycle and `lint` reports them).
pub fn ensure_acyclic(tickets: &[Ticket]) -> Result<()> {
    let by_id: BTreeMap<&str, &Ticket> = tickets.iter().map(|t| (t.id.as_str(), t)).collect();
    match find_cycle(&by_id) {
        Some(cycle) => Err(Error::Cycle(cycle.join(" -> "))),
        None => Ok(()),
    }
}

/// Why two tickets can (or cannot) run in parallel, surfaced for explainability
/// (`tkt why`). Two tickets cannot run in parallel if they share a scope (file
/// overlap) or one transitively depends on the other (ordering). Note this is
/// broader than what `tracks` gates on: `tracks` only batches dispatchable tickets,
/// among which no dependency relationship can exist, so it gates on scope alone.
#[derive(Debug, Clone, Serialize)]
pub struct Why {
    /// First ticket id.
    pub a: String,
    /// Second ticket id.
    pub b: String,
    /// Scopes both tickets claim where at least one is exclusive — the scopes that
    /// block them from running in parallel (a shared-by-both claim is compatible and
    /// excluded). Field name kept for back-compat.
    pub shared_scopes: Vec<String>,
    /// Whether one transitively depends on the other (a hard ordering constraint).
    pub dependency_ordered: bool,
    /// True if either criterion holds — they cannot run in parallel.
    pub conflict: bool,
}

/// Explain the scheduling relationship between two tickets.
pub fn why(tickets: &[Ticket], a_id: &str, b_id: &str) -> Result<Why> {
    // A ticket trivially shares every scope with itself; comparing one to itself is
    // a usage mistake, not a real conflict.
    if a_id == b_id {
        return Err(Error::Invalid(format!(
            "`why` compares two different tickets (got `{a_id}` twice)"
        )));
    }
    let by_id: BTreeMap<&str, &Ticket> = tickets.iter().map(|t| (t.id.as_str(), t)).collect();
    let a = by_id
        .get(a_id)
        .copied()
        .ok_or_else(|| Error::NotFound(a_id.to_string()))?;
    let b = by_id
        .get(b_id)
        .copied()
        .ok_or_else(|| Error::NotFound(b_id.to_string()))?;

    let conflicting = conflicting_scopes(a, b);
    // a != b is guaranteed by the early return above.
    let dependency_ordered = depends_on(&by_id, a_id, b_id) || depends_on(&by_id, b_id, a_id);

    let conflict = !conflicting.is_empty() || dependency_ordered;
    Ok(Why {
        a: a_id.to_string(),
        b: b_id.to_string(),
        shared_scopes: conflicting,
        dependency_ordered,
        conflict,
    })
}

/// A node in the exported dependency graph, carrying the same scoring components
/// `next` ranks by so a visualizer can size/colour nodes by impact.
#[derive(Debug, Clone, Serialize)]
pub struct GraphNode {
    /// Ticket id.
    pub id: String,
    /// Ticket title.
    pub title: String,
    /// Lifecycle status.
    pub status: Status,
    /// Priority.
    pub priority: Priority,
    /// `next` score: `1000*priority + 10*critical_path + downstream_count`.
    pub score: i64,
    /// Longest remaining downstream chain length (in nodes).
    pub critical_path: i64,
    /// Count of not-done tickets this one would unblock.
    pub downstream_count: i64,
}

/// A dependency edge: `from` depends on `to`.
#[derive(Debug, Clone, Serialize)]
pub struct GraphEdge {
    /// The dependent ticket.
    pub from: String,
    /// The prerequisite ticket.
    pub to: String,
}

/// The dependency DAG with per-node scoring metrics, for `graph` export. Edges are
/// every declared dependency (regardless of status); metrics use the remaining-work
/// dependents map (done tickets excluded), matching `next`'s scoring.
#[derive(Debug, Clone, Serialize)]
pub struct GraphExport {
    /// Nodes in input (id-sorted) order.
    pub nodes: Vec<GraphNode>,
    /// Dependency edges (`from` depends on `to`).
    pub edges: Vec<GraphEdge>,
}

/// Build the dependency-graph export (validates the graph first).
pub fn graph_export(tickets: &[Ticket]) -> Result<GraphExport> {
    let graph = Graph::build(tickets)?;
    let mut memo: BTreeMap<&str, i64> = BTreeMap::new();
    let nodes = tickets
        .iter()
        .map(|t| {
            let id = t.id.as_str();
            let critical_path = critical_path(id, &graph.dependents, &mut memo);
            let downstream_count = downstream_count(id, &graph.dependents);
            GraphNode {
                id: t.id.clone(),
                title: t.title.clone(),
                status: t.status,
                priority: t.priority,
                score: 1000 * priority_value(t.priority) + 10 * critical_path + downstream_count,
                critical_path,
                downstream_count,
            }
        })
        .collect();
    let edges = tickets
        .iter()
        .flat_map(|t| {
            t.dependencies.iter().map(|d| GraphEdge {
                from: t.id.clone(),
                to: d.clone(),
            })
        })
        .collect();
    Ok(GraphExport { nodes, edges })
}

/// The longest chain of dependencies ending at `id` — its critical prerequisite path
/// — returned root-first (deepest prerequisite … → `id`). A ticket with no
/// dependencies yields `[id]`. Validates the graph (acyclic) first.
pub fn longest_prerequisite_path(tickets: &[Ticket], id: &str) -> Result<Vec<String>> {
    let graph = Graph::build(tickets)?;
    if !graph.by_id.contains_key(id) {
        return Err(Error::NotFound(id.to_string()));
    }
    let mut memo: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut chain = longest_dep_chain(id, &graph.by_id, &mut memo);
    chain.reverse(); // node-first (id → deepest) becomes root-first (deepest → id)
    Ok(chain.iter().map(|s| (*s).to_string()).collect())
}

/// Longest chain starting at `node` and descending dependency edges, `node` first.
/// Memoized; safe because `Graph::build` already proved the graph acyclic.
fn longest_dep_chain<'a>(
    node: &'a str,
    by_id: &BTreeMap<&'a str, &'a Ticket>,
    memo: &mut BTreeMap<&'a str, Vec<&'a str>>,
) -> Vec<&'a str> {
    if let Some(cached) = memo.get(node) {
        return cached.clone();
    }
    let mut best: Vec<&str> = Vec::new();
    if let Some(t) = by_id.get(node) {
        for d in &t.dependencies {
            let sub = longest_dep_chain(d.as_str(), by_id, memo);
            if sub.len() > best.len() {
                best = sub;
            }
        }
    }
    let mut chain = vec![node];
    chain.extend(best);
    memo.insert(node, chain.clone());
    chain
}

/// The safe parallel width: the largest set of *dispatchable* tickets that can run at
/// once with no pair exceeding `max_overlap` (an orchestrator's "how many workers can
/// I usefully spin up right now"). Validates the graph first.
pub fn parallel_width(
    tickets: &[Ticket],
    max_overlap: i64,
    weights: &BTreeMap<String, i64>,
) -> Result<usize> {
    let graph = Graph::build(tickets)?;
    let nodes = graph.dispatchable(tickets);
    Ok(max_compatible_among(&nodes, max_overlap, weights))
}

/// The largest mutually-compatible subset of `tickets` (every pair's conflict cost
/// ≤ `max_overlap`) — a maximum independent set in the conflict graph. Exact for a
/// frontier of ≤ 22 tickets (the normal case); beyond that it falls back to a greedy
/// lower bound to stay fast.
#[must_use]
pub fn max_compatible_among(
    tickets: &[&Ticket],
    max_overlap: i64,
    weights: &BTreeMap<String, i64>,
) -> usize {
    let n = tickets.len();
    if n == 0 {
        return 0;
    }
    let mut adj: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); n];
    for i in 0..n {
        for j in (i + 1)..n {
            if conflict_cost(tickets[i], tickets[j], weights) > max_overlap {
                adj[i].insert(j);
                adj[j].insert(i);
            }
        }
    }
    if n > 22 {
        return greedy_independent_set(&adj);
    }
    let all: Vec<usize> = (0..n).collect();
    max_independent_set(&adj, &all)
}

/// Exact maximum independent set over `remaining` (include/exclude branch-and-bound;
/// including a vertex drops its neighbours). Fine for the small frontiers it gates.
fn max_independent_set(adj: &[BTreeSet<usize>], remaining: &[usize]) -> usize {
    match remaining.split_first() {
        None => 0,
        Some((&v, rest)) => {
            let exclude = max_independent_set(adj, rest);
            let kept: Vec<usize> = rest
                .iter()
                .copied()
                .filter(|u| !adj[v].contains(u))
                .collect();
            let include = 1 + max_independent_set(adj, &kept);
            include.max(exclude)
        }
    }
}

/// Greedy independent-set lower bound (smallest-degree-first), for large frontiers.
fn greedy_independent_set(adj: &[BTreeSet<usize>]) -> usize {
    let mut order: Vec<usize> = (0..adj.len()).collect();
    order.sort_by_key(|&i| adj[i].len());
    let mut chosen: Vec<usize> = Vec::new();
    for v in order {
        if chosen.iter().all(|&c| !adj[v].contains(&c)) {
            chosen.push(v);
        }
    }
    chosen.len()
}

// --- internal graph helpers -------------------------------------------------

fn priority_value(p: Priority) -> i64 {
    match p {
        Priority::P0 => 3,
        Priority::P1 => 2,
        Priority::P2 => 1,
        Priority::P3 => 0,
    }
}

/// Every scope a ticket claims, in either mode (exclusive `scopes` ∪ `shared_scopes`).
fn claimed_scopes(t: &Ticket) -> BTreeSet<&str> {
    t.scopes
        .iter()
        .chain(t.shared_scopes.iter())
        .map(String::as_str)
        .collect()
}

/// Scopes both tickets claim where at least one claims it *exclusively* — the scopes
/// that prevent them running in parallel. Two *shared* (additive) claims on the same
/// scope are compatible and excluded. Sorted, deduped. Exposed so a caller can build
/// the conflict matrix for its own assignment.
#[must_use]
pub fn conflicting_scopes(a: &Ticket, b: &Ticket) -> Vec<String> {
    let a_claims = claimed_scopes(a);
    let b_claims = claimed_scopes(b);
    let a_shared: BTreeSet<&str> = a.shared_scopes.iter().map(String::as_str).collect();
    let b_shared: BTreeSet<&str> = b.shared_scopes.iter().map(String::as_str).collect();
    a_claims
        .intersection(&b_claims)
        .copied()
        .filter(|s| !(a_shared.contains(*s) && b_shared.contains(*s)))
        .map(str::to_string)
        .collect()
}

/// The cost of co-scheduling two tickets: the summed `weights` of their conflicting
/// scopes (a scope absent from `weights` costs 1; pass an empty map for unit costs).
/// `0` means compatible — safe to run in parallel. `tracks` and `next` gate on this
/// against a per-pair overlap budget; callers can use it to report a chosen set's
/// residual overlap cost.
#[must_use]
pub fn conflict_cost(a: &Ticket, b: &Ticket, weights: &BTreeMap<String, i64>) -> i64 {
    conflicting_scopes(a, b)
        .iter()
        .map(|s| weights.get(s).copied().unwrap_or(1))
        .sum()
}

/// Whether `from` transitively depends on `to` (directed reachability over dep
/// edges). The `seen` set also keeps it terminating on a (lint-flagged) cycle.
fn depends_on(by_id: &BTreeMap<&str, &Ticket>, from: &str, to: &str) -> bool {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut stack: Vec<&str> = vec![from];
    while let Some(n) = stack.pop() {
        let Some(t) = by_id.get(n) else { continue };
        for d in &t.dependencies {
            if d == to {
                return true;
            }
            if seen.insert(d.as_str()) {
                stack.push(d.as_str());
            }
        }
    }
    false
}

/// Longest downstream chain length (in nodes) starting at `node`.
fn critical_path<'a>(
    node: &'a str,
    dependents: &BTreeMap<&'a str, Vec<&'a str>>,
    memo: &mut BTreeMap<&'a str, i64>,
) -> i64 {
    if let Some(&v) = memo.get(node) {
        return v;
    }
    let mut best = 1;
    if let Some(deps) = dependents.get(node) {
        for &d in deps {
            best = best.max(1 + critical_path(d, dependents, memo));
        }
    }
    memo.insert(node, best);
    best
}

/// Count of transitively-dependent tickets `node` would unblock.
fn downstream_count(node: &str, dependents: &BTreeMap<&str, Vec<&str>>) -> i64 {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        if let Some(ds) = dependents.get(n) {
            for &d in ds {
                if seen.insert(d) {
                    stack.push(d);
                }
            }
        }
    }
    seen.len() as i64
}

fn find_cycle<'a>(by_id: &BTreeMap<&'a str, &'a Ticket>) -> Option<Vec<String>> {
    let mut ids: Vec<&str> = by_id.keys().copied().collect();
    ids.sort_unstable();
    let mut state: BTreeMap<&str, u8> = ids.iter().map(|&k| (k, 0u8)).collect();
    let mut path: Vec<&str> = Vec::new();
    for &start in &ids {
        if state[start] == 0 {
            if let Some(cycle) = dfs_cycle(start, by_id, &mut state, &mut path) {
                return Some(cycle);
            }
        }
    }
    None
}

fn dfs_cycle<'a>(
    node: &'a str,
    by_id: &BTreeMap<&'a str, &'a Ticket>,
    state: &mut BTreeMap<&'a str, u8>,
    path: &mut Vec<&'a str>,
) -> Option<Vec<String>> {
    state.insert(node, 1);
    path.push(node);
    if let Some(t) = by_id.get(node) {
        let mut deps: Vec<&str> = t.dependencies.iter().map(String::as_str).collect();
        deps.sort_unstable();
        for d in deps {
            match state.get(d).copied().unwrap_or(2) {
                0 => {
                    if let Some(cycle) = dfs_cycle(d, by_id, state, path) {
                        return Some(cycle);
                    }
                }
                1 => {
                    let pos = path.iter().position(|&x| x == d).unwrap_or(0);
                    let mut cycle: Vec<String> =
                        path[pos..].iter().map(|s| (*s).to_string()).collect();
                    cycle.push(d.to_string());
                    return Some(cycle);
                }
                _ => {}
            }
        }
    }
    path.pop();
    state.insert(node, 2);
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(id: &str, status: &str, priority: &str, deps: &[&str], scopes: &[&str]) -> Ticket {
        let d: Vec<String> = deps.iter().map(|s| (*s).to_string()).collect();
        let sc: Vec<String> = scopes.iter().map(|s| (*s).to_string()).collect();
        Ticket::new(
            id,
            id,
            status.parse().unwrap(),
            priority.parse().unwrap(),
            &d,
            &[],
            &sc,
            &[],
            &[],
            &[],
            "",
        )
        .unwrap()
    }

    /// Build a ticket with explicit related links (for the non-blocking tests).
    fn t_rel(id: &str, status: &str, deps: &[&str], related: &[&str]) -> Ticket {
        let d: Vec<String> = deps.iter().map(|s| (*s).to_string()).collect();
        let r: Vec<String> = related.iter().map(|s| (*s).to_string()).collect();
        Ticket::new(
            id,
            id,
            status.parse().unwrap(),
            "p2".parse().unwrap(),
            &d,
            &r,
            &[],
            &[],
            &[],
            &[],
            "",
        )
        .unwrap()
    }

    /// Build a ticket with explicit exclusive + shared scope claims.
    fn t_scoped(id: &str, exclusive: &[&str], shared: &[&str]) -> Ticket {
        let e: Vec<String> = exclusive.iter().map(|s| (*s).to_string()).collect();
        let sh: Vec<String> = shared.iter().map(|s| (*s).to_string()).collect();
        Ticket::new(
            id,
            id,
            "todo".parse().unwrap(),
            "p2".parse().unwrap(),
            &[],
            &[],
            &e,
            &sh,
            &[],
            &[],
            "",
        )
        .unwrap()
    }

    #[test]
    fn shared_scope_claims_are_compatible() {
        let a = t_scoped("a", &[], &["core"]); // additive core
        let b = t_scoped("b", &[], &["core"]); // additive core
        let c = t_scoped("c", &["core"], &[]); // rewrites core
        let d = t_scoped("d", &["core"], &[]); // rewrites core
        let w = BTreeMap::new();
        assert_eq!(
            conflict_cost(&a, &b, &w),
            0,
            "shared x shared is compatible"
        );
        assert!(
            conflict_cost(&a, &c, &w) > 0,
            "shared x exclusive conflicts"
        );
        assert!(
            conflict_cost(&c, &d, &w) > 0,
            "exclusive x exclusive conflicts"
        );
        // The two additive tickets share one tracks batch.
        let additive = [t_scoped("a", &[], &["core"]), t_scoped("b", &[], &["core"])];
        let batches = tracks(&additive, 0, &BTreeMap::new()).unwrap();
        assert_eq!(batches.len(), 1, "additive co-scheduling: {batches:?}");
        // why agrees: no conflict between two additive claims.
        let pair = vec![t_scoped("a", &[], &["core"]), t_scoped("b", &[], &["core"])];
        assert!(!why(&pair, "a", "b").unwrap().conflict);
    }

    #[test]
    fn parallel_width_is_the_max_compatible_set() {
        let w = BTreeMap::new();
        let exclusive = vec![
            t_scoped("a", &["core"], &[]),
            t_scoped("b", &["core"], &[]),
            t_scoped("c", &["core"], &[]),
        ];
        assert_eq!(
            parallel_width(&exclusive, 0, &w).unwrap(),
            1,
            "all conflict"
        );
        assert_eq!(
            parallel_width(&exclusive, 1, &w).unwrap(),
            3,
            "budget frees them"
        );
        let disjoint = vec![
            t_scoped("a", &["x"], &[]),
            t_scoped("b", &["y"], &[]),
            t_scoped("c", &["z"], &[]),
        ];
        assert_eq!(parallel_width(&disjoint, 0, &w).unwrap(), 3, "disjoint");
    }

    #[test]
    fn lanes_sequence_conflicts_onto_one_worker() {
        let w = BTreeMap::new();
        // a,b conflict on core; c is disjoint (io).
        let tickets = vec![
            t_scoped("a", &["core"], &[]),
            t_scoped("b", &["core"], &[]),
            t_scoped("c", &["io"], &[]),
        ];
        let plan = lanes(&tickets, 2, 0, &w).unwrap();
        // a and b cannot run together, so they are sequenced onto one lane (not dropped).
        assert!(
            plan.lanes.iter().any(|l| {
                let ids: Vec<&str> = l.iter().map(|t| t.id.as_str()).collect();
                ids.contains(&"a") && ids.contains(&"b")
            }),
            "a and b share a lane"
        );
        // c runs concurrently on its own lane; every ready ticket is planned once.
        assert_eq!(plan.lanes.len(), 2);
        assert_eq!(plan.merge_order.len(), 3);
    }

    #[test]
    fn max_overlap_dial_fills_workers_least_cost_first() {
        // Three tickets all rewrite `core`: the disjoint width is 1.
        let tickets = vec![
            t_scoped("a", &["core"], &[]),
            t_scoped("b", &["core"], &[]),
            t_scoped("c", &["core"], &[]),
        ];
        let w = BTreeMap::new();
        // Strict (budget 0): only one fits.
        assert_eq!(next(&tickets, 3, 0, &w, &[]).unwrap().len(), 1);
        assert_eq!(tracks(&tickets, 0, &w).unwrap().len(), 3);
        // Budget 1: every pair costs 1, so all three fill / share one batch.
        let picks = next(&tickets, 3, 1, &w, &[]).unwrap();
        assert_eq!(picks.len(), 3);
        assert!(picks.iter().any(|p| !p.conflicts_with.is_empty()));
        assert!(picks
            .iter()
            .flat_map(|p| &p.conflicts_with)
            .all(|c| c.cost == 1));
        assert_eq!(tracks(&tickets, 1, &w).unwrap().len(), 1);
    }

    #[test]
    fn ready_filters_and_orders() {
        let tickets = vec![
            t("a", "todo", "p2", &[], &[]),
            t("b", "todo", "p0", &[], &[]),
            t("c", "todo", "p1", &["d"], &[]), // blocked: d not done
            t("d", "in-progress", "p1", &[], &[]), // not dispatchable
        ];
        let r = ready(&tickets).unwrap();
        let ids: Vec<&str> = r.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, vec!["b", "a"]); // p0 before p2; c blocked, d in-progress
    }

    #[test]
    fn cycle_is_an_error() {
        let tickets = vec![
            t("a", "todo", "p2", &["b"], &[]),
            t("b", "todo", "p2", &["a"], &[]),
        ];
        assert!(matches!(ready(&tickets), Err(Error::Cycle(_))));
    }

    #[test]
    fn dangling_dependency_is_an_error() {
        let tickets = vec![t("a", "todo", "p2", &["ghost"], &[])];
        assert!(ready(&tickets).is_err());
    }

    #[test]
    fn related_links_do_not_block_or_cycle() {
        // `a` relates to `b` (not done) and they relate to each other (a cycle in the
        // related graph). Neither blocks readiness, and no cycle is reported.
        let tickets = vec![
            t_rel("a", "todo", &[], &["b"]),
            t_rel("b", "in-progress", &[], &["a"]),
        ];
        let ids: Vec<&str> = ready(&tickets)
            .unwrap()
            .iter()
            .map(|t| t.id.as_str())
            .collect();
        assert_eq!(
            ids,
            vec!["a"],
            "a is ready despite relating to a non-done ticket"
        );
        assert!(
            !link_diagnostics(&tickets).iter().any(|d| d.code == "cycle"),
            "a related cycle is not a dependency cycle"
        );
    }

    #[test]
    fn link_diagnostics_flags_missing_related() {
        let tickets = vec![t_rel("a", "todo", &[], &["ghost"])];
        let diags = link_diagnostics(&tickets);
        assert!(diags
            .iter()
            .any(|d| d.code == "missing-related" && d.message.contains("ghost")));
    }

    #[test]
    fn link_diagnostics_flags_orphaned_by_closed_dependency() {
        // `a` (live) depends on `base`, which was closed without completing: a is orphaned.
        let orphan = vec![
            t("base", "closed", "p1", &[], &[]),
            t("a", "todo", "p1", &["base"], &[]),
        ];
        assert!(link_diagnostics(&orphan)
            .iter()
            .any(|d| d.code == "orphaned-by-closed-dep" && d.id.as_deref() == Some("a")));
        // A done dependency is satisfied, not an orphan; a terminal dependent is moot.
        let ok = vec![
            t("base", "done", "p1", &[], &[]),
            t("a", "todo", "p1", &["base"], &[]),
            t("b", "closed", "p1", &["base2"], &[]),
            t("base2", "closed", "p1", &[], &[]),
        ];
        assert!(!link_diagnostics(&ok)
            .iter()
            .any(|d| d.code == "orphaned-by-closed-dep"));
    }

    #[test]
    fn graph_export_carries_edges_and_scoring_metrics() {
        let tickets = vec![
            t("base", "todo", "p0", &[], &[]),
            t("mid", "todo", "p2", &["base"], &[]),
            t("leaf", "todo", "p2", &["mid"], &[]),
        ];
        let g = graph_export(&tickets).unwrap();
        assert_eq!(g.nodes.len(), 3);
        assert_eq!(g.edges.len(), 2);
        let base = g.nodes.iter().find(|n| n.id == "base").unwrap();
        assert_eq!(base.downstream_count, 2, "base unblocks mid + leaf");
        assert_eq!(base.critical_path, 3, "base -> mid -> leaf");
        assert!(g.edges.iter().any(|e| e.from == "mid" && e.to == "base"));
    }

    #[test]
    fn longest_prerequisite_path_is_root_first() {
        let tickets = vec![
            t("base", "todo", "p2", &[], &[]),
            t("mid", "todo", "p2", &["base"], &[]),
            t("leaf", "todo", "p2", &["mid", "base"], &[]),
        ];
        assert_eq!(
            longest_prerequisite_path(&tickets, "leaf").unwrap(),
            vec!["base", "mid", "leaf"]
        );
        assert_eq!(
            longest_prerequisite_path(&tickets, "base").unwrap(),
            vec!["base"]
        );
        assert!(longest_prerequisite_path(&tickets, "ghost").is_err());
    }

    #[test]
    fn tracks_separates_shared_scope() {
        let tickets = vec![
            t("a", "todo", "p1", &[], &["core"]),
            t("b", "todo", "p1", &[], &["core"]), // shares scope with a
            t("c", "todo", "p1", &[], &["io"]),   // disjoint
        ];
        let batches = tracks(&tickets, 0, &BTreeMap::new()).unwrap();
        // a and b must be in different batches; no batch has both.
        for batch in &batches {
            let ids: BTreeSet<&str> = batch.iter().map(|t| t.id.as_str()).collect();
            assert!(!(ids.contains("a") && ids.contains("b")));
        }
        // every dispatchable ticket appears exactly once.
        let total: usize = batches.iter().map(Vec::len).sum();
        assert_eq!(total, 3);
    }

    #[test]
    fn next_prefers_priority() {
        let tickets = vec![
            t("a", "todo", "p2", &[], &["x"]),
            t("b", "todo", "p0", &[], &["y"]),
        ];
        let picks = next(&tickets, 1, 0, &BTreeMap::new(), &[]).unwrap();
        assert_eq!(picks[0].ticket.id, "b");
    }

    #[test]
    fn why_explains_shared_scope_and_disjoint() {
        let tickets = vec![
            t("a", "todo", "p1", &[], &["core"]),
            t("b", "todo", "p1", &[], &["core"]),
            t("c", "todo", "p1", &[], &["io"]),
        ];
        let shared = why(&tickets, "a", "b").unwrap();
        assert!(shared.conflict);
        assert_eq!(shared.shared_scopes, vec!["core"]);
        assert!(!why(&tickets, "a", "c").unwrap().conflict);
        assert!(why(&tickets, "a", "ghost").is_err());
    }

    #[test]
    fn why_explains_dependency_component() {
        let tickets = vec![
            t("a", "todo", "p1", &["b"], &["x"]),
            t("b", "todo", "p1", &[], &["y"]),
        ];
        let w = why(&tickets, "a", "b").unwrap();
        assert!(w.conflict);
        assert!(w.dependency_ordered);
        assert!(w.shared_scopes.is_empty());
    }

    #[test]
    fn why_siblings_of_a_shared_dependency_can_run_in_parallel() {
        // a and b both depend on base but not on each other, with disjoint scopes:
        // no ordering between them, so no conflict (the old weak-component said yes).
        let tickets = vec![
            t("base", "todo", "p1", &[], &["base"]),
            t("a", "todo", "p1", &["base"], &["x"]),
            t("b", "todo", "p1", &["base"], &["y"]),
        ];
        let w = why(&tickets, "a", "b").unwrap();
        assert!(!w.dependency_ordered);
        assert!(!w.conflict);
    }

    #[test]
    fn done_dependency_does_not_serialize_independent_dependents() {
        // base is DONE; a and b depend only on it and have disjoint scopes, so once
        // base is merged they are independent and should run in parallel (one batch).
        let tickets = vec![
            t("base", "done", "p1", &[], &["base-scope"]),
            t("a", "todo", "p1", &["base"], &["x"]),
            t("b", "todo", "p1", &["base"], &["y"]),
        ];
        let batches = tracks(&tickets, 0, &BTreeMap::new()).unwrap();
        assert_eq!(
            batches.len(),
            1,
            "a and b should share one batch; got {batches:?}"
        );
        // And `why` should not order them: neither depends on the other.
        assert!(!why(&tickets, "a", "b").unwrap().dependency_ordered);
    }

    #[test]
    fn closed_dependency_does_not_satisfy_dependents() {
        // base is CLOSED (abandoned), not done: `a` must NOT become ready — a closed
        // prerequisite must never silently unblock work built on top of it.
        let orphaned = vec![
            t("base", "closed", "p1", &[], &[]),
            t("a", "todo", "p1", &["base"], &[]),
        ];
        let ids: Vec<&str> = ready(&orphaned)
            .unwrap()
            .iter()
            .map(|t| t.id.as_str())
            .collect();
        assert!(
            ids.is_empty(),
            "a is orphaned by a closed dep, not ready: {ids:?}"
        );
        // Contrast: a done prerequisite does satisfy it.
        let done = vec![
            t("base", "done", "p1", &[], &[]),
            t("a", "todo", "p1", &["base"], &[]),
        ];
        let ids: Vec<&str> = ready(&done)
            .unwrap()
            .iter()
            .map(|t| t.id.as_str())
            .collect();
        assert_eq!(ids, vec!["a"]);
    }

    #[test]
    fn next_parallel_picks_are_disjoint() {
        let tickets = vec![
            t("a", "todo", "p0", &[], &["core"]),
            t("b", "todo", "p0", &[], &["core"]), // conflicts with a
            t("c", "todo", "p1", &[], &["io"]),
        ];
        let picks = next(&tickets, 2, 0, &BTreeMap::new(), &[]).unwrap();
        let ids: BTreeSet<&str> = picks.iter().map(|p| p.ticket.id.as_str()).collect();
        // Cannot pick both a and b together (they share scope `core`).
        assert!(!(ids.contains("a") && ids.contains("b")));
        assert_eq!(picks.len(), 2);
        // Disjoint picks report no conflicts.
        assert!(picks.iter().all(|p| p.conflicts_with.is_empty()));
    }

    #[test]
    fn next_allow_overlap_returns_overlapping_picks_annotated() {
        let tickets = vec![
            t("a", "todo", "p0", &[], &["core"]),
            t("b", "todo", "p0", &[], &["core"]), // shares `core` with a
        ];
        let picks = next(&tickets, 2, i64::MAX, &BTreeMap::new(), &[]).unwrap();
        let ids: BTreeSet<&str> = picks.iter().map(|p| p.ticket.id.as_str()).collect();
        // With --allow-overlap both top-scored picks come back, despite the overlap.
        assert!(ids.contains("a") && ids.contains("b"));
        // ...and each is annotated with the shared scope so the caller can judge it.
        let a = picks.iter().find(|p| p.ticket.id == "a").unwrap();
        assert_eq!(a.conflicts_with.len(), 1);
        assert_eq!(a.conflicts_with[0].ticket, "b");
        assert_eq!(a.conflicts_with[0].scopes, vec!["core"]);
    }
}
