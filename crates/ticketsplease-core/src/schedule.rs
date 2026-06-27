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
        // Reverse edges for `next` scoring: id -> NOT-done tickets that depend on it.
        // Done dependents are excluded so "downstream work this unblocks" counts only
        // work that is actually still waiting.
        let mut dependents: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for t in tickets {
            dependents.entry(t.id.as_str()).or_default();
        }
        for t in tickets {
            if t.status == Status::Done {
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

    /// Dispatchable = open for dispatch (todo/ready) with every dependency done.
    fn is_dispatchable(&self, t: &Ticket) -> bool {
        t.status.is_dispatchable()
            && t.dependencies.iter().all(|d| {
                self.by_id
                    .get(d.as_str())
                    .is_some_and(|dep| dep.status == Status::Done)
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

/// Partition the ready set into conflict-free parallel batches (R6): two tickets
/// that share a scope never share a batch. Dependency *ordering* needs no handling
/// here — only dispatchable tickets (every dependency already done) are batched, so
/// by construction none of them depend on each other; the sole parallel hazard left
/// is file overlap, i.e. a shared scope. Deterministic greedy (Welsh–Powell) colouring.
pub fn tracks(tickets: &[Ticket]) -> Result<Vec<Vec<&Ticket>>> {
    let graph = Graph::build(tickets)?;
    let nodes = graph.dispatchable(tickets);
    let n = nodes.len();
    if n == 0 {
        return Ok(Vec::new());
    }

    let mut adj: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); n];
    for i in 0..n {
        for j in (i + 1)..n {
            if conflicts(nodes[i], nodes[j]) {
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

/// A scope overlap between two returned picks (surfaced under `allow_overlap`).
#[derive(Debug, Clone, Serialize)]
pub struct PickConflict {
    /// The other pick's ticket id.
    pub ticket: String,
    /// Scopes shared with that pick.
    pub scopes: Vec<String>,
}

/// Recommend the next ticket(s): score by priority, downstream critical-path
/// length, and remaining-downstream unblock count. With `parallel > 1`, return that
/// many picks, highest-scored first (R7). By default the picks are scope-disjoint
/// (conflict-minimizing); with `allow_overlap` the top-scored picks are returned
/// even when their scopes overlap, each annotated with the overlap so the caller
/// can decide whether the shared-crate work is tolerable.
pub fn next(tickets: &[Ticket], parallel: usize, allow_overlap: bool) -> Result<Vec<Pick<'_>>> {
    let graph = Graph::build(tickets)?;
    let mut nodes = graph.dispatchable(tickets);
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

    // Highest-scored first. Default: skip a candidate that shares a scope with an
    // already-chosen pick. allow_overlap: take the top N regardless of overlap.
    let want = parallel.max(1);
    let mut chosen: Vec<&Ticket> = Vec::new();
    for t in nodes {
        if chosen.len() >= want {
            break;
        }
        if allow_overlap || chosen.iter().all(|&c| !conflicts(c, t)) {
            chosen.push(t);
        }
    }

    // Surface, per pick, the scopes it shares with the other picks in the set.
    let picks = chosen
        .iter()
        .map(|&t| {
            let conflicts_with = chosen
                .iter()
                .filter(|&&o| o.id != t.id)
                .filter_map(|&o| {
                    let scopes = shared_scopes(t, o);
                    (!scopes.is_empty()).then(|| PickConflict {
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
            if !by_id.contains_key(d.as_str()) {
                out.push(Diagnostic {
                    file: format!("{}.md", t.id),
                    id: Some(t.id.clone()),
                    message: format!("depends on missing ticket `{d}`"),
                });
            }
        }
    }
    if let Some(cycle) = find_cycle(&by_id) {
        out.push(Diagnostic {
            file: "(dependency graph)".to_string(),
            id: None,
            message: format!("dependency cycle: {}", cycle.join(" -> ")),
        });
    }
    out
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
    /// Scopes both tickets declare (a shared-scope conflict).
    pub shared_scopes: Vec<String>,
    /// Whether one transitively depends on the other (a hard ordering constraint).
    pub dependency_ordered: bool,
    /// True if either criterion holds — they cannot run in parallel.
    pub conflict: bool,
}

/// Explain the scheduling relationship between two tickets.
pub fn why(tickets: &[Ticket], a_id: &str, b_id: &str) -> Result<Why> {
    let by_id: BTreeMap<&str, &Ticket> = tickets.iter().map(|t| (t.id.as_str(), t)).collect();
    let a = by_id
        .get(a_id)
        .copied()
        .ok_or_else(|| Error::NotFound(a_id.to_string()))?;
    let b = by_id
        .get(b_id)
        .copied()
        .ok_or_else(|| Error::NotFound(b_id.to_string()))?;

    let shared = shared_scopes(a, b);
    let dependency_ordered =
        a_id != b_id && (depends_on(&by_id, a_id, b_id) || depends_on(&by_id, b_id, a_id));

    let conflict = !shared.is_empty() || dependency_ordered;
    Ok(Why {
        a: a_id.to_string(),
        b: b_id.to_string(),
        shared_scopes: shared,
        dependency_ordered,
        conflict,
    })
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

fn shares_scope(a: &Ticket, b: &Ticket) -> bool {
    let set: BTreeSet<&str> = a.scopes.iter().map(String::as_str).collect();
    b.scopes.iter().any(|s| set.contains(s.as_str()))
}

/// The scopes both tickets declare (sorted, deduped).
fn shared_scopes(a: &Ticket, b: &Ticket) -> Vec<String> {
    let a_set: BTreeSet<&str> = a.scopes.iter().map(String::as_str).collect();
    let b_set: BTreeSet<&str> = b.scopes.iter().map(String::as_str).collect();
    a_set
        .intersection(&b_set)
        .map(|s| (*s).to_string())
        .collect()
}

/// Two dispatchable tickets conflict (cannot share a batch) iff they share a scope
/// — the only parallel hazard once dependency ordering is satisfied by the
/// dispatchable filter.
fn conflicts(a: &Ticket, b: &Ticket) -> bool {
    shares_scope(a, b)
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
            &sc,
            &[],
            &[],
            "",
        )
        .unwrap()
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
    fn tracks_separates_shared_scope() {
        let tickets = vec![
            t("a", "todo", "p1", &[], &["core"]),
            t("b", "todo", "p1", &[], &["core"]), // shares scope with a
            t("c", "todo", "p1", &[], &["io"]),   // disjoint
        ];
        let batches = tracks(&tickets).unwrap();
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
        let picks = next(&tickets, 1, false).unwrap();
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
        let batches = tracks(&tickets).unwrap();
        assert_eq!(
            batches.len(),
            1,
            "a and b should share one batch; got {batches:?}"
        );
        // And `why` should not order them: neither depends on the other.
        assert!(!why(&tickets, "a", "b").unwrap().dependency_ordered);
    }

    #[test]
    fn next_parallel_picks_are_disjoint() {
        let tickets = vec![
            t("a", "todo", "p0", &[], &["core"]),
            t("b", "todo", "p0", &[], &["core"]), // conflicts with a
            t("c", "todo", "p1", &[], &["io"]),
        ];
        let picks = next(&tickets, 2, false).unwrap();
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
        let picks = next(&tickets, 2, true).unwrap();
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
