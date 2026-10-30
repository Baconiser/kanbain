//! The orchestrator's state machine (Phase 5, D22). Deterministic Rust, not
//! an LLM: given an approved plan and where each subtask stands, it says
//! which subtasks may start now and whether the ticket is done, blocked or
//! stopped. Starting runs and merging branches is the runtime's job
//! (`local-deployment`'s orchestrator); this module only decides.
//!
//! Ordering rules:
//! - A subtask waits for everything in its `depends_on`.
//! - Subtasks whose file lists overlap run one after the other, in
//!   dependency order (plan order breaks ties). Disjoint ones run in
//!   parallel; the sandbox's slot limit caps how many actually run at once.
//! - A subtask without a file list overlaps everything: we can't tell what
//!   it touches, so it runs alone rather than gambling on a conflict.

use std::collections::BTreeSet;

use db::models::{orchestration::SubtaskStatus, plan::PlanDocument};

/// What the ticket as a whole is doing after a scheduling step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Something is running or can start.
    Continue,
    /// Every subtask is merged.
    Done,
    /// A subtask failed and nothing else can move.
    Blocked,
    /// Stopped subtasks (or ones waiting on them) leave nothing to run.
    Stopped,
}

/// The plan's subtasks as a DAG, by index into `plan.subtasks`.
#[derive(Debug, Clone)]
pub struct Schedule {
    /// `before[i]`: subtasks that must be merged before `i` starts.
    before: Vec<BTreeSet<usize>>,
    /// A topological order (plan order breaks ties).
    order: Vec<usize>,
}

/// `./src/a.rs` and `src/a.rs/` are the same path.
fn normalize(path: &str) -> &str {
    let path = path.trim();
    let path = path.strip_prefix("./").unwrap_or(path);
    path.trim_end_matches('/')
}

/// The literal part of a path pattern, up to its first glob character.
fn literal_prefix(path: &str) -> (&str, bool) {
    match path.find(['*', '?', '[', '{']) {
        Some(i) => (&path[..i], true),
        None => (path, false),
    }
}

/// Whether two file entries may touch the same file: equal, one a
/// directory containing the other, or a glob whose literal prefix covers
/// the other. Errs towards "yes": a false overlap costs parallelism, a
/// missed one costs a conflict re-run.
pub fn paths_overlap(a: &str, b: &str) -> bool {
    let (a, b) = (normalize(a), normalize(b));
    if a.is_empty() || b.is_empty() {
        return true;
    }
    let (pa, glob_a) = literal_prefix(a);
    let (pb, glob_b) = literal_prefix(b);
    let contains = |dir: &str, path: &str, glob: bool| {
        if glob {
            path.starts_with(dir) || dir.starts_with(path)
        } else {
            path == dir || path.starts_with(&format!("{dir}/"))
        }
    };
    contains(pa, b, glob_a) || contains(pb, a, glob_b) || (glob_a && glob_b && pa == pb)
}

/// Whether two subtasks' file lists overlap; an empty list overlaps all.
pub fn files_overlap(a: &[String], b: &[String]) -> bool {
    if a.is_empty() || b.is_empty() {
        return true;
    }
    a.iter().any(|x| b.iter().any(|y| paths_overlap(x, y)))
}

impl Schedule {
    /// Builds the DAG. The plan is validated (no unknown or cyclic
    /// dependencies) before it can be approved; unknown ids are ignored
    /// here anyway, and a cycle degrades to plan order.
    pub fn new(plan: &PlanDocument) -> Self {
        let n = plan.subtasks.len();
        let index = |id: &str| plan.subtasks.iter().position(|s| s.id == id);
        let mut before: Vec<BTreeSet<usize>> = plan
            .subtasks
            .iter()
            .enumerate()
            .map(|(i, s)| {
                s.depends_on
                    .iter()
                    .filter_map(|d| index(d))
                    .filter(|&d| d != i)
                    .collect()
            })
            .collect();

        // Kahn's algorithm over `depends_on`, lowest plan index first.
        let mut order = Vec::with_capacity(n);
        let mut placed = vec![false; n];
        while order.len() < n {
            let next = (0..n)
                .find(|&i| !placed[i] && before[i].iter().all(|&d| placed[d]))
                // A cycle (only possible in an unvalidated plan): take the
                // first unplaced subtask so the order stays total.
                .or_else(|| (0..n).find(|&i| !placed[i]))
                .expect("an unplaced subtask exists");
            placed[next] = true;
            order.push(next);
        }

        // Overlapping subtasks run in that order: the earlier one first.
        for (pos, &i) in order.iter().enumerate() {
            for &j in &order[pos + 1..] {
                if files_overlap(&plan.subtasks[i].files, &plan.subtasks[j].files) {
                    before[j].insert(i);
                }
            }
        }
        // Drop edges a cycle would have created against the order.
        let rank: Vec<usize> = {
            let mut rank = vec![0; n];
            for (pos, &i) in order.iter().enumerate() {
                rank[i] = pos;
            }
            rank
        };
        for (j, deps) in before.iter_mut().enumerate() {
            deps.retain(|&i| rank[i] < rank[j]);
        }
        Self { before, order }
    }

    /// Subtasks that must be merged before subtask `i` starts.
    pub fn before(&self, i: usize) -> impl Iterator<Item = usize> + '_ {
        self.before[i].iter().copied()
    }

    /// Pending subtasks whose prerequisites are all merged, in order.
    pub fn ready(&self, status: &[SubtaskStatus]) -> Vec<usize> {
        self.order
            .iter()
            .copied()
            .filter(|&i| status[i] == SubtaskStatus::Pending)
            .filter(|&i| {
                self.before[i]
                    .iter()
                    .all(|&d| status[d] == SubtaskStatus::Merged)
            })
            .collect()
    }

    /// Where the ticket stands once [`Self::ready`] subtasks are started.
    pub fn outcome(&self, status: &[SubtaskStatus]) -> Outcome {
        if status.iter().all(|s| *s == SubtaskStatus::Merged) {
            return Outcome::Done;
        }
        if status.contains(&SubtaskStatus::Running) || !self.ready(status).is_empty() {
            return Outcome::Continue;
        }
        if status.contains(&SubtaskStatus::Failed) {
            Outcome::Blocked
        } else {
            Outcome::Stopped
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn plan(subtasks: serde_json::Value) -> PlanDocument {
        PlanDocument::from_value(json!({ "subtasks": subtasks })).unwrap()
    }

    use SubtaskStatus::{Failed, Merged, Pending, Running, Stopped};

    #[test]
    fn overlap_rules() {
        assert!(paths_overlap("src/a.rs", "./src/a.rs"));
        assert!(paths_overlap("src", "src/a.rs"));
        assert!(paths_overlap("src/", "src/deep/a.rs"));
        assert!(!paths_overlap("src/a.rs", "src/ab.rs"));
        assert!(!paths_overlap("src/a", "src/ab.rs"));
        assert!(paths_overlap("src/**/*.rs", "src/x/y.rs"));
        assert!(!paths_overlap("web/*.ts", "src/a.rs"));
        assert!(files_overlap(&[], &["x".into()]));
        assert!(!files_overlap(&["a".into()], &["b".into()]));
    }

    /// The Phase 5 demo shape: two disjoint subtasks in parallel, a third
    /// after both.
    #[test]
    fn disjoint_run_in_parallel_and_dependents_wait() {
        let s = Schedule::new(&plan(json!([
            {"id": "1", "title": "Model", "files": ["shop/src/model.rs"]},
            {"id": "2", "title": "Mail", "files": ["shop/src/mail.rs"]},
            {"id": "3", "title": "Route", "files": ["shop/src/route.rs"],
             "depends_on": ["1", "2"]}
        ])));
        assert_eq!(s.ready(&[Pending, Pending, Pending]), [0, 1]);
        assert_eq!(s.ready(&[Merged, Running, Pending]), Vec::<usize>::new());
        assert_eq!(s.ready(&[Merged, Merged, Pending]), [2]);
        assert_eq!(s.outcome(&[Merged, Running, Pending]), Outcome::Continue);
        assert_eq!(s.outcome(&[Merged, Merged, Merged]), Outcome::Done);
    }

    #[test]
    fn overlapping_files_run_in_sequence() {
        let s = Schedule::new(&plan(json!([
            {"id": "a", "title": "A", "files": ["src/lib.rs", "src/a.rs"]},
            {"id": "b", "title": "B", "files": ["src/b.rs"]},
            {"id": "c", "title": "C", "files": ["src/lib.rs"]}
        ])));
        assert_eq!(s.ready(&[Pending, Pending, Pending]), [0, 1]);
        assert_eq!(s.ready(&[Merged, Running, Pending]), [2]);
        assert_eq!(s.before(2).collect::<Vec<_>>(), [0]);
    }

    /// Overlap edges follow the dependency order, so a dependency that
    /// points "backwards" in plan order can't deadlock with an overlap.
    #[test]
    fn overlap_follows_dependency_order() {
        let s = Schedule::new(&plan(json!([
            {"id": "1", "title": "Uses it", "files": ["x.rs"], "depends_on": ["2"]},
            {"id": "2", "title": "Adds it", "files": ["x.rs"]}
        ])));
        assert_eq!(s.ready(&[Pending, Pending]), [1]);
        assert_eq!(s.ready(&[Pending, Merged]), [0]);
    }

    #[test]
    fn unknown_files_run_alone() {
        let s = Schedule::new(&plan(json!([
            {"id": "1", "title": "A", "files": ["a.rs"]},
            {"id": "2", "title": "Somewhere"},
            {"id": "3", "title": "C", "files": ["c.rs"]}
        ])));
        assert_eq!(s.ready(&[Pending, Pending, Pending]), [0]);
        assert_eq!(s.ready(&[Merged, Pending, Pending]), [1]);
        assert_eq!(s.ready(&[Merged, Merged, Pending]), [2]);
    }

    #[test]
    fn failures_block_and_stops_stop() {
        let s = Schedule::new(&plan(json!([
            {"id": "1", "title": "A", "files": ["a.rs"]},
            {"id": "2", "title": "B", "files": ["b.rs"], "depends_on": ["1"]},
            {"id": "3", "title": "C", "files": ["c.rs"]}
        ])));
        // The independent subtask keeps going while another failed.
        assert_eq!(s.outcome(&[Failed, Pending, Running]), Outcome::Continue);
        assert_eq!(s.outcome(&[Failed, Pending, Merged]), Outcome::Blocked);
        assert_eq!(s.outcome(&[Stopped, Pending, Merged]), Outcome::Stopped);
        assert_eq!(s.outcome(&[Merged, Pending, Stopped]), Outcome::Continue);
    }
}
