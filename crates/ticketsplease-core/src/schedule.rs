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
use crate::ticket::{Priority, Ticket};

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
        let mut dependents: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for t in tickets {
            dependents.entry(t.id.as_str()).or_default();
        }
        for t in tickets {
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
                    .is_some_and(|dep| dep.status == crate::ticket::Status::Done)
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
/// sharing any scope, or in the same weakly-connected dependency component, never
/// share a batch. Deterministic greedy (Welsh–Powell) colouring.
pub fn tracks(tickets: &[Ticket]) -> Result<Vec<Vec<&Ticket>>> {
    let graph = Graph::build(tickets)?;
    let nodes = graph.dispatchable(tickets);
    let n = nodes.len();
    if n == 0 {
        return Ok(Vec::new());
    }

    let comp = components(tickets);
    let mut adj: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); n];
    for i in 0..n {
        for j in (i + 1)..n {
            if conflicts(nodes[i], nodes[j], &comp) {
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

/// A scored next-pick with its components, for explainability.
pub struct Pick<'a> {
    /// The recommended ticket.
    pub ticket: &'a Ticket,
    /// Total score (higher is better).
    pub score: i64,
}

/// Recommend the next ticket(s): score by priority, downstream critical-path
/// length, and transitive unblock count. With `parallel > 1`, return that many
/// mutually conflict-free picks, highest-scored first (R7).
pub fn next(tickets: &[Ticket], parallel: usize) -> Result<Vec<Pick<'_>>> {
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

    let want = parallel.max(1);
    let comp = components(tickets);
    let mut picks: Vec<Pick> = Vec::new();
    for t in nodes {
        if picks.len() >= want {
            break;
        }
        if picks.iter().all(|p| !conflicts(p.ticket, t, &comp)) {
            picks.push(Pick {
                ticket: t,
                score: scores[t.id.as_str()],
            });
        }
    }
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

/// Why two tickets can (or cannot) share a parallel batch — the same criteria
/// `tracks` uses, surfaced for explainability (`tkt why`).
#[derive(Debug, Clone, Serialize)]
pub struct Why {
    /// First ticket id.
    pub a: String,
    /// Second ticket id.
    pub b: String,
    /// Scopes both tickets declare (a shared-scope conflict).
    pub shared_scopes: Vec<String>,
    /// Whether they sit in the same weakly-connected dependency component.
    pub same_dependency_component: bool,
    /// True if either criterion holds — they cannot run in the same batch.
    pub conflict: bool,
}

/// Explain the scheduling relationship between two tickets.
pub fn why(tickets: &[Ticket], a_id: &str, b_id: &str) -> Result<Why> {
    let by_id: BTreeMap<&str, &Ticket> = tickets.iter().map(|t| (t.id.as_str(), t)).collect();
    let a = by_id
        .get(a_id)
        .ok_or_else(|| Error::NotFound(a_id.to_string()))?;
    let b = by_id
        .get(b_id)
        .ok_or_else(|| Error::NotFound(b_id.to_string()))?;

    let a_scopes: BTreeSet<&str> = a.scopes.iter().map(String::as_str).collect();
    let shared_scopes: Vec<String> = b
        .scopes
        .iter()
        .filter(|s| a_scopes.contains(s.as_str()))
        .cloned()
        .collect();

    let comp = components(tickets);
    let same_dependency_component =
        a_id != b_id && comp.get(a_id).copied() == comp.get(b_id).copied();

    let conflict = !shared_scopes.is_empty() || same_dependency_component;
    Ok(Why {
        a: a_id.to_string(),
        b: b_id.to_string(),
        shared_scopes,
        same_dependency_component,
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

fn conflicts(a: &Ticket, b: &Ticket, comp: &BTreeMap<&str, usize>) -> bool {
    shares_scope(a, b) || comp.get(a.id.as_str()).copied() == comp.get(b.id.as_str()).copied()
}

/// Weakly-connected component id for every ticket (union-find over dep edges).
fn components(tickets: &[Ticket]) -> BTreeMap<&str, usize> {
    let mut ids: Vec<&str> = tickets.iter().map(|t| t.id.as_str()).collect();
    ids.sort_unstable();
    let index: BTreeMap<&str, usize> = ids.iter().enumerate().map(|(i, &id)| (id, i)).collect();
    let mut parent: Vec<usize> = (0..ids.len()).collect();
    for t in tickets {
        let Some(&ti) = index.get(t.id.as_str()) else {
            continue;
        };
        for d in &t.dependencies {
            if let Some(&di) = index.get(d.as_str()) {
                let a = uf_find(&mut parent, ti);
                let b = uf_find(&mut parent, di);
                if a != b {
                    parent[a] = b;
                }
            }
        }
    }
    ids.iter()
        .map(|&id| (id, uf_find(&mut parent, index[id])))
        .collect()
}

fn uf_find(parent: &mut [usize], mut x: usize) -> usize {
    while parent[x] != x {
        parent[x] = parent[parent[x]];
        x = parent[x];
    }
    x
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
        let picks = next(&tickets, 1).unwrap();
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
        assert!(w.same_dependency_component);
        assert!(w.shared_scopes.is_empty());
    }

    #[test]
    fn next_parallel_picks_are_disjoint() {
        let tickets = vec![
            t("a", "todo", "p0", &[], &["core"]),
            t("b", "todo", "p0", &[], &["core"]), // conflicts with a
            t("c", "todo", "p1", &[], &["io"]),
        ];
        let picks = next(&tickets, 2).unwrap();
        let ids: BTreeSet<&str> = picks.iter().map(|p| p.ticket.id.as_str()).collect();
        // Cannot pick both a and b together.
        assert!(!(ids.contains("a") && ids.contains("b")));
        assert_eq!(picks.len(), 2);
    }
}
