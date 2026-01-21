// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Tree structure for organizing runs by parent-child relationships.
//!
//! This module provides a tree structure that groups runs based on their
//! `parent_run_id` relationships. It supports:
//!
//! - Building trees from a flat list of runs
//! - Virtual nodes for pruned (missing) parents
//! - Compressed display for linear chains (single-child descendants)
//!
//! # Display rules
//!
//! 1. **Roots (depth 0):** No tree characters, just the base indent.
//! 2. **Children with siblings:** `├─` (not last) or `└─` (last).
//! 3. **Only children (linear chain):** No branch character, just continuation/space.
//! 4. **Continuation lines:** `│ ` when ancestor has more siblings, `  ` otherwise.
//! 5. **Virtual parents:** Displayed as `???  (pruned parent)` with children below.
//! 6. **Sorting:** Children by own `started_at` descending; roots by max subtree
//!    `started_at` descending (tree with most recently added run comes first).

use chrono::{DateTime, FixedOffset};
use quick_junit::ReportUuid;
use std::collections::{HashMap, HashSet};

/// Information extracted from a run for tree building.
#[derive(Clone, Debug)]
pub(super) struct RunInfo {
    /// The unique identifier for this run.
    pub run_id: ReportUuid,
    /// The parent run ID, if this is a rerun.
    pub parent_run_id: Option<ReportUuid>,
    /// When the run started (used for sorting).
    pub started_at: DateTime<FixedOffset>,
}

/// A tree structure organizing runs by parent-child relationships.
///
/// The tree supports multiple roots (independent runs or runs with pruned
/// parents).
#[derive(Debug)]
pub(super) struct RunTree {
    /// Pre-computed traversal order for display.
    items: Vec<TreeIterItem>,
}

/// A root entry in the tree.
#[derive(Debug, Clone)]
enum RootEntry {
    /// A real run that exists in the store and has no parent (or parent is pruned).
    Real(ReportUuid),
    /// A virtual parent that doesn't exist in the store.
    /// Contains the ID of the missing parent.
    Virtual(ReportUuid),
}

impl RunTree {
    /// Builds a tree from a slice of run information.
    pub(super) fn build(runs: &[RunInfo]) -> Self {
        if runs.is_empty() {
            return Self { items: Vec::new() };
        }

        let run_map: HashMap<ReportUuid, &RunInfo> = runs.iter().map(|r| (r.run_id, r)).collect();

        // Build parent -> children map, tracking which runs are children and which
        // parents are virtual (pruned).
        let mut children: HashMap<ReportUuid, Vec<ReportUuid>> = HashMap::new();
        let mut is_child: HashSet<ReportUuid> = HashSet::new();
        let mut virtual_parents: HashSet<ReportUuid> = HashSet::new();

        for run in runs {
            if let Some(parent_id) = run.parent_run_id {
                is_child.insert(run.run_id);
                children.entry(parent_id).or_default().push(run.run_id);
                if !run_map.contains_key(&parent_id) {
                    virtual_parents.insert(parent_id);
                }
            }
        }

        for children_list in children.values_mut() {
            children_list.sort_by(|a, b| {
                let a_time = run_map.get(a).map(|r| r.started_at);
                let b_time = run_map.get(b).map(|r| r.started_at);
                b_time.cmp(&a_time) // Descending.
            });
        }

        // Build roots with their max subtree times, tracking reachability.
        let mut reachable: HashSet<ReportUuid> = HashSet::new();
        let mut roots: Vec<_> = runs
            .iter()
            .filter(|run| !is_child.contains(&run.run_id))
            .map(|run| RootEntry::Real(run.run_id))
            .chain(virtual_parents.iter().map(|&id| RootEntry::Virtual(id)))
            .map(|root| {
                let max_time = Self::max_subtree_time(&root, &children, &run_map, &mut reachable);
                (root, max_time)
            })
            .collect();

        // Add roots for any unreachable runs (disconnected cycles).
        // This handles both pure cycles (no natural roots) and disconnected
        // cycles that exist alongside normal trees.
        for run in runs {
            if !reachable.contains(&run.run_id) {
                let root = RootEntry::Real(run.run_id);
                let max_time = Self::max_subtree_time(&root, &children, &run_map, &mut reachable);
                roots.push((root, max_time));
            }
        }

        // Sort descending so the tree with the most recently added run comes
        // first.
        roots.sort_by(|(_, a_max), (_, b_max)| b_max.cmp(a_max));

        let items = Self::build_traversal(&roots, &children);

        Self { items }
    }

    /// Computes the maximum started_at time across a root and all its
    /// transitive descendants, and marks all visited runs in `reachable`.
    fn max_subtree_time(
        root: &RootEntry,
        children: &HashMap<ReportUuid, Vec<ReportUuid>>,
        run_map: &HashMap<ReportUuid, &RunInfo>,
        reachable: &mut HashSet<ReportUuid>,
    ) -> Option<DateTime<FixedOffset>> {
        let mut max_time: Option<DateTime<FixedOffset>> = None;

        let mut stack: Vec<ReportUuid> = match root {
            RootEntry::Real(run_id) => vec![*run_id],
            RootEntry::Virtual(parent_id) => {
                // Virtual parent has no time; start with its children.
                children
                    .get(parent_id)
                    .map(|c| c.to_vec())
                    .unwrap_or_default()
            }
        };

        while let Some(run_id) = stack.pop() {
            // Guard against cycles in parent_run_id relationships.
            if !reachable.insert(run_id) {
                continue;
            }
            if let Some(time) = run_map.get(&run_id).map(|r| r.started_at) {
                max_time = Some(max_time.map_or(time, |m| m.max(time)));
            }
            if let Some(child_ids) = children.get(&run_id) {
                stack.extend(child_ids.iter().copied());
            }
        }

        max_time
    }

    /// Builds the traversal order using iterative DFS.
    fn build_traversal(
        roots: &[(RootEntry, Option<DateTime<FixedOffset>>)],
        children: &HashMap<ReportUuid, Vec<ReportUuid>>,
    ) -> Vec<TreeIterItem> {
        struct NodeState {
            run_id: Option<ReportUuid>,
            children_key: ReportUuid,
            depth: usize,
            is_last: bool,
            is_only_child: bool,
            continuation_flags: Vec<bool>,
        }

        let mut items = Vec::new();
        let mut stack: Vec<NodeState> = Vec::new();
        let mut visited: HashSet<ReportUuid> = HashSet::new();

        for (root, _) in roots.iter().rev() {
            let (run_id, children_key) = match root {
                RootEntry::Real(id) => (Some(*id), *id),
                RootEntry::Virtual(id) => (None, *id),
            };
            stack.push(NodeState {
                run_id,
                children_key,
                depth: 0,
                is_last: true,
                is_only_child: false,
                continuation_flags: Vec::new(),
            });
        }

        while let Some(state) = stack.pop() {
            // Guard against cycles in parent_run_id relationships.
            if !visited.insert(state.children_key) {
                continue;
            }

            items.push(TreeIterItem {
                run_id: state.run_id,
                depth: state.depth,
                is_last: state.is_last,
                is_only_child: state.is_only_child,
                continuation_flags: state.continuation_flags.clone(),
            });

            let Some(child_ids) = children.get(&state.children_key) else {
                continue;
            };

            let child_count = child_ids.len();
            for (i, child_id) in child_ids.iter().enumerate().rev() {
                let child_is_last = i == child_count - 1;
                let child_is_only_child = child_count == 1;
                let child_depth = state.depth + 1;

                // Compute continuation flags for the child:
                //
                // - Depth 1: no continuation flags (root's children).
                // - Parent is compressed (only-child): inherit parent's flags
                //   without adding a new one, since the parent has no visual
                //   column to itself.
                // - Otherwise, add a continuation flag for the parent's column.
                let child_continuation = if child_depth == 1 {
                    Vec::new()
                } else if state.is_only_child {
                    state.continuation_flags.clone()
                } else {
                    let mut flags = state.continuation_flags.clone();
                    flags.push(!state.is_last);
                    flags
                };

                stack.push(NodeState {
                    run_id: Some(*child_id),
                    children_key: *child_id,
                    depth: child_depth,
                    is_last: child_is_last,
                    is_only_child: child_is_only_child && child_depth > 1,
                    continuation_flags: child_continuation,
                });
            }
        }

        items
    }

    /// Returns an iterator over the tree items in display order.
    pub(super) fn iter(&self) -> impl Iterator<Item = &TreeIterItem> {
        self.items.iter()
    }
}

/// An item in the tree traversal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TreeIterItem {
    /// The run ID, or None for a virtual (pruned) parent.
    pub(super) run_id: Option<ReportUuid>,
    /// Depth in the tree (0 for roots).
    pub(super) depth: usize,
    /// Whether this is the last child of its parent.
    pub(super) is_last: bool,
    /// Whether this is an only child (for compressed chain display).
    /// When true, branch characters (`├─`, `└─`) should be omitted.
    pub(super) is_only_child: bool,
    /// For each ancestor level (index 0 = depth 1, etc.), whether to draw
    /// a continuation line (`│`). False means draw space.
    pub(super) continuation_flags: Vec<bool>,
}

impl TreeIterItem {
    /// Width of tree prefix characters (excluding base indent), in units of
    /// 2-char segments.
    ///
    /// This accounts for compressed chains where only-children don't add visual
    /// width. Used to calculate padding for column alignment.
    pub(super) fn tree_prefix_width(&self) -> usize {
        if self.depth == 0 {
            0
        } else {
            self.continuation_flags.len() + if self.is_only_child { 0 } else { 1 }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use proptest::prelude::*;
    use rand::seq::SliceRandom;
    use test_strategy::proptest;

    fn make_run(id_suffix: u32, parent_suffix: Option<u32>, hours_ago: i64) -> RunInfo {
        let run_id = format!("50000000-0000-0000-0000-{:012}", id_suffix)
            .parse()
            .expect("valid UUID");
        let parent_run_id = parent_suffix.map(|p| {
            format!("50000000-0000-0000-0000-{:012}", p)
                .parse()
                .expect("valid UUID")
        });

        RunInfo {
            run_id,
            parent_run_id,
            started_at: chrono::FixedOffset::east_opt(0)
                .unwrap()
                .with_ymd_and_hms(2024, 6, 15, (12 - hours_ago).try_into().unwrap_or(0), 0, 0)
                .unwrap(),
        }
    }

    #[test]
    fn test_empty() {
        let tree = RunTree::build(&[]);
        assert_eq!(tree.iter().count(), 0);
    }

    #[test]
    fn test_cycle_self_parent() {
        // A run claims to be its own parent. This is a corrupt state -- just
        // ensure we don't loop.
        let runs = vec![make_run(1, Some(1), 0)];
        let tree = RunTree::build(&runs);
        let items: Vec<_> = tree.iter().cloned().collect();

        // The run should appear exactly once.
        assert_eq!(items.len(), 1, "self-cycle should not cause infinite loop");
        assert_eq!(items[0].run_id, Some(runs[0].run_id));
    }

    #[test]
    fn test_cycle_disconnected_from_root() {
        // A has no parent (root), B -> C -> B (disconnected cycle). This is a
        // corrupt state -- ensure we don't loop, and that all three appear
        // exactly once.
        let runs = vec![
            make_run(1, None, 2),    // A: root.
            make_run(2, Some(3), 1), // B: parent is C.
            make_run(3, Some(2), 0), // C: parent is B.
        ];
        let tree = RunTree::build(&runs);
        let items: Vec<_> = tree.iter().cloned().collect();

        assert_eq!(items.len(), 3, "all nodes should appear once");
        let run_ids: HashSet<_> = items.iter().filter_map(|item| item.run_id).collect();
        assert!(run_ids.contains(&runs[0].run_id));
        assert!(run_ids.contains(&runs[1].run_id));
        assert!(run_ids.contains(&runs[2].run_id));
    }

    #[test]
    fn test_single_root() {
        let runs = vec![make_run(1, None, 0)];
        let tree = RunTree::build(&runs);
        let items: Vec<_> = tree.iter().cloned().collect();

        assert_eq!(
            items,
            vec![TreeIterItem {
                run_id: Some(runs[0].run_id),
                depth: 0,
                is_last: true,
                is_only_child: false,
                continuation_flags: vec![],
            }]
        );
    }

    #[test]
    fn test_linear_chain() {
        // parent -> child -> grandchild
        let runs = vec![
            make_run(1, None, 3),    // Root, oldest.
            make_run(2, Some(1), 2), // Child.
            make_run(3, Some(2), 1), // Grandchild.
        ];
        let tree = RunTree::build(&runs);
        let items: Vec<_> = tree.iter().cloned().collect();

        assert_eq!(
            items,
            vec![
                // Root (parent).
                TreeIterItem {
                    run_id: Some(runs[0].run_id),
                    depth: 0,
                    is_last: true,
                    is_only_child: false,
                    continuation_flags: vec![],
                },
                // Child - only child of parent, but depth 1 so is_only_child is
                // false.
                TreeIterItem {
                    run_id: Some(runs[1].run_id),
                    depth: 1,
                    is_last: true,
                    is_only_child: false,
                    continuation_flags: vec![],
                },
                // Grandchild - only child of child.
                // continuation_flags is [false] because child (depth 1) is last.
                TreeIterItem {
                    run_id: Some(runs[2].run_id),
                    depth: 2,
                    is_last: true,
                    is_only_child: true,
                    continuation_flags: vec![false],
                },
            ]
        );
    }

    #[test]
    fn test_branching() {
        // parent has two children: child1 and child2
        let runs = vec![
            make_run(1, None, 3),    // Root.
            make_run(2, Some(1), 2), // child1 (older).
            make_run(3, Some(1), 1), // child2 (newer).
        ];
        let tree = RunTree::build(&runs);
        let items: Vec<_> = tree.iter().cloned().collect();

        assert_eq!(
            items,
            vec![
                // Root.
                TreeIterItem {
                    run_id: Some(runs[0].run_id),
                    depth: 0,
                    is_last: true,
                    is_only_child: false,
                    continuation_flags: vec![],
                },
                // child2 (newer, comes first due to descending sort).
                TreeIterItem {
                    run_id: Some(runs[2].run_id),
                    depth: 1,
                    is_last: false,
                    is_only_child: false,
                    continuation_flags: vec![],
                },
                // child1 (older, comes second).
                TreeIterItem {
                    run_id: Some(runs[1].run_id),
                    depth: 1,
                    is_last: true,
                    is_only_child: false,
                    continuation_flags: vec![],
                },
            ]
        );
    }

    #[test]
    fn test_pruned_parent() {
        // Run whose parent doesn't exist.
        let runs = vec![make_run(2, Some(1), 0)];
        let tree = RunTree::build(&runs);
        let items: Vec<_> = tree.iter().cloned().collect();

        assert_eq!(
            items,
            vec![
                // Virtual parent (run_id=None indicates pruned).
                TreeIterItem {
                    run_id: None,
                    depth: 0,
                    is_last: true,
                    is_only_child: false,
                    continuation_flags: vec![],
                },
                // Real child - is_only_child=false (depth 1 never only_child)
                // and empty continuation_flags (matching real parent behavior).
                TreeIterItem {
                    run_id: Some(runs[0].run_id),
                    depth: 1,
                    is_last: true,
                    is_only_child: false,
                    continuation_flags: vec![],
                },
            ]
        );
    }

    #[test]
    fn test_pruned_parent_with_multiple_children() {
        // Virtual parent with two direct children.
        // Both should have branch characters (not marked as only_child).
        let runs = vec![
            make_run(2, Some(1), 2), // Older child of pruned parent.
            make_run(3, Some(1), 1), // Newer child of same pruned parent.
        ];
        let tree = RunTree::build(&runs);
        let items: Vec<_> = tree.iter().cloned().collect();

        assert_eq!(
            items,
            vec![
                // Virtual parent.
                TreeIterItem {
                    run_id: None,
                    depth: 0,
                    is_last: true,
                    is_only_child: false,
                    continuation_flags: vec![],
                },
                // Newer child (sorted first by started_at descending).
                TreeIterItem {
                    run_id: Some(runs[1].run_id),
                    depth: 1,
                    is_last: false,
                    is_only_child: false,
                    continuation_flags: vec![],
                },
                // Older child (sorted second).
                TreeIterItem {
                    run_id: Some(runs[0].run_id),
                    depth: 1,
                    is_last: true,
                    is_only_child: false,
                    continuation_flags: vec![],
                },
            ]
        );
    }

    #[test]
    fn test_multiple_trees() {
        // Two independent runs (no parent relationship).
        // Sorted by max subtree time descending (most recent first).
        let runs = vec![
            make_run(1, None, 2), // Older.
            make_run(2, None, 1), // Newer.
        ];
        let tree = RunTree::build(&runs);
        let items: Vec<_> = tree.iter().cloned().collect();

        assert_eq!(
            items,
            vec![
                // First tree (newer run, sorted first by max subtree time).
                TreeIterItem {
                    run_id: Some(runs[1].run_id), // run 2 (newer)
                    depth: 0,
                    is_last: true,
                    is_only_child: false,
                    continuation_flags: vec![],
                },
                // Second tree (older run).
                TreeIterItem {
                    run_id: Some(runs[0].run_id), // run 1 (older)
                    depth: 0,
                    is_last: true,
                    is_only_child: false,
                    continuation_flags: vec![],
                },
            ]
        );
    }

    #[test]
    fn test_branching_with_chains() {
        // parent -> child1 -> grandchild1
        // parent -> child2
        let runs = vec![
            make_run(1, None, 4),    // Root.
            make_run(2, Some(1), 3), // child1 (older).
            make_run(3, Some(2), 2), // grandchild1.
            make_run(4, Some(1), 1), // child2 (newer).
        ];
        let tree = RunTree::build(&runs);
        let items: Vec<_> = tree.iter().cloned().collect();

        // Order: parent, child2 (newer), child1 (older), grandchild1.
        // Children sorted by started_at descending, so child2 comes before child1.
        assert_eq!(
            items,
            vec![
                // parent.
                TreeIterItem {
                    run_id: Some(runs[0].run_id),
                    depth: 0,
                    is_last: true,
                    is_only_child: false,
                    continuation_flags: vec![],
                },
                // child2 (newer, first).
                TreeIterItem {
                    run_id: Some(runs[3].run_id),
                    depth: 1,
                    is_last: false,
                    is_only_child: false,
                    continuation_flags: vec![],
                },
                // child1 (older, second).
                TreeIterItem {
                    run_id: Some(runs[1].run_id),
                    depth: 1,
                    is_last: true,
                    is_only_child: false,
                    continuation_flags: vec![],
                },
                // grandchild1 (only child of child1).
                // continuation_flags: child1 is last, so [false].
                TreeIterItem {
                    run_id: Some(runs[2].run_id),
                    depth: 2,
                    is_last: true,
                    is_only_child: true,
                    continuation_flags: vec![false],
                },
            ]
        );
    }

    #[test]
    fn test_continuation_flags_with_siblings() {
        // parent -> child1 -> grandchild1
        // parent -> child2
        let runs = vec![
            make_run(1, None, 4),    // Root.
            make_run(2, Some(1), 3), // child1 (older).
            make_run(3, Some(2), 2), // grandchild1.
            make_run(4, Some(1), 1), // child2 (newer).
        ];
        let tree = RunTree::build(&runs);
        let items: Vec<_> = tree.iter().cloned().collect();

        // Order: parent, child2 (newer), child1 (older), grandchild1.
        // child2 comes first (newer), child1 is last.
        // grandchild1's continuation_flags should be [false] because child1 is
        // the last child.
        assert_eq!(
            items,
            vec![
                TreeIterItem {
                    run_id: Some(runs[0].run_id),
                    depth: 0,
                    is_last: true,
                    is_only_child: false,
                    continuation_flags: vec![],
                },
                TreeIterItem {
                    run_id: Some(runs[3].run_id), // child2 (newer)
                    depth: 1,
                    is_last: false,
                    is_only_child: false,
                    continuation_flags: vec![],
                },
                TreeIterItem {
                    run_id: Some(runs[1].run_id), // child1 (older)
                    depth: 1,
                    is_last: true,
                    is_only_child: false,
                    continuation_flags: vec![],
                },
                TreeIterItem {
                    run_id: Some(runs[2].run_id), // grandchild1
                    depth: 2,
                    is_last: true,
                    is_only_child: true,
                    continuation_flags: vec![false],
                },
            ]
        );
    }

    #[test]
    fn test_continuation_flags_not_last() {
        // parent -> child1 -> grandchild1 (child1 is not last)
        // parent -> child2
        // But with different timestamps so child1 comes before child2.
        let runs = vec![
            make_run(1, None, 4),    // Root.
            make_run(2, Some(1), 1), // child1 (newer).
            make_run(3, Some(2), 0), // grandchild1.
            make_run(4, Some(1), 3), // child2 (older).
        ];
        let tree = RunTree::build(&runs);
        let items: Vec<_> = tree.iter().cloned().collect();

        // Order: parent, child1 (newer), grandchild1, child2 (older).
        // child1 comes first (newer), child1 is NOT last.
        // grandchild1's continuation_flags should be [true] because child1 is NOT the last child.
        assert_eq!(
            items,
            vec![
                TreeIterItem {
                    run_id: Some(runs[0].run_id), // parent
                    depth: 0,
                    is_last: true,
                    is_only_child: false,
                    continuation_flags: vec![],
                },
                TreeIterItem {
                    run_id: Some(runs[1].run_id), // child1 (newer)
                    depth: 1,
                    is_last: false,
                    is_only_child: false,
                    continuation_flags: vec![],
                },
                TreeIterItem {
                    run_id: Some(runs[2].run_id), // grandchild1
                    depth: 2,
                    is_last: true,
                    is_only_child: true,
                    continuation_flags: vec![true], // child1 is not last, so draw │
                },
                TreeIterItem {
                    run_id: Some(runs[3].run_id), // child2 (older)
                    depth: 1,
                    is_last: true,
                    is_only_child: false,
                    continuation_flags: vec![],
                },
            ]
        );
    }

    #[test]
    fn test_deep_chain_with_branching() {
        // root -> child1 -> gc1 -> ggc1
        // root -> child2
        // child1 comes first (newer), so gc1 and ggc1 should have continuation [true].
        let runs = vec![
            make_run(1, None, 5),    // Root.
            make_run(2, Some(1), 1), // child1 (newer).
            make_run(3, Some(2), 2), // gc1.
            make_run(4, Some(3), 3), // ggc1.
            make_run(5, Some(1), 4), // child2 (older).
        ];
        let tree = RunTree::build(&runs);
        let items: Vec<_> = tree.iter().cloned().collect();

        // Order: root, child1, gc1, ggc1, child2.
        // ggc1 inherits gc1's continuation_flags because gc1 is compressed.
        assert_eq!(
            items,
            vec![
                TreeIterItem {
                    run_id: Some(runs[0].run_id), // root
                    depth: 0,
                    is_last: true,
                    is_only_child: false,
                    continuation_flags: vec![],
                },
                TreeIterItem {
                    run_id: Some(runs[1].run_id), // child1 (newer)
                    depth: 1,
                    is_last: false,
                    is_only_child: false,
                    continuation_flags: vec![],
                },
                TreeIterItem {
                    run_id: Some(runs[2].run_id), // gc1
                    depth: 2,
                    is_last: true,
                    is_only_child: true,
                    continuation_flags: vec![true],
                },
                TreeIterItem {
                    run_id: Some(runs[3].run_id), // ggc1
                    depth: 3,
                    is_last: true,
                    is_only_child: true,
                    continuation_flags: vec![true],
                },
                TreeIterItem {
                    run_id: Some(runs[4].run_id), // child2 (older)
                    depth: 1,
                    is_last: true,
                    is_only_child: false,
                    continuation_flags: vec![],
                },
            ]
        );
    }

    #[test]
    fn test_compressed_grandparent_with_branching_children() {
        // Tests the case where a compressed (only-child) node has multiple
        // children, and one of those children also has children.
        //
        // root -> A (only child, compressed)
        //         ├─ B (not only child, has sibling C)
        //         │  └─ D (only child, compressed)
        //         └─ C
        //
        // Key test: D should have continuation flags [false, true]:
        // - [0]=false: A is last child of root (no continuation)
        // - [1]=true: B is not last child of A (continuation needed for C)
        let runs = vec![
            make_run(1, None, 5),    // root
            make_run(2, Some(1), 4), // A (only child of root)
            make_run(3, Some(2), 1), // B (newer, will be first among siblings)
            make_run(4, Some(3), 0), // D (only child of B)
            make_run(5, Some(2), 3), // C (older, will be last among siblings)
        ];
        let tree = RunTree::build(&runs);
        let items: Vec<_> = tree.iter().cloned().collect();

        // Order: root, A, B, D, C
        assert_eq!(
            items,
            vec![
                TreeIterItem {
                    run_id: Some(runs[0].run_id), // root
                    depth: 0,
                    is_last: true,
                    is_only_child: false,
                    continuation_flags: vec![],
                },
                TreeIterItem {
                    run_id: Some(runs[1].run_id), // A (only child, compressed)
                    depth: 1,
                    is_last: true,
                    is_only_child: false, // depth 1 is never only_child
                    continuation_flags: vec![],
                },
                TreeIterItem {
                    run_id: Some(runs[2].run_id), // B (not last, has sibling C)
                    depth: 2,
                    is_last: false,
                    is_only_child: false,
                    continuation_flags: vec![false], // A is last
                },
                TreeIterItem {
                    run_id: Some(runs[3].run_id), // D (only child of B, compressed)
                    depth: 3,
                    is_last: true,
                    is_only_child: true,
                    continuation_flags: vec![false, true], // A is last, B is not last
                },
                TreeIterItem {
                    run_id: Some(runs[4].run_id), // C (last sibling)
                    depth: 2,
                    is_last: true,
                    is_only_child: false,
                    continuation_flags: vec![false], // A is last
                },
            ]
        );
    }

    #[test]
    fn test_roots_sorted_by_max_subtree_time() {
        // Two independent trees:
        // - Tree A: run 1 (older root) -> run 3 (newest overall)
        // - Tree B: run 2 (newer root, but no children)
        // Tree A should come first because it contains the most recent run (run 3).
        let runs = vec![
            make_run(1, None, 3),    // Older root.
            make_run(2, None, 2),    // Newer root (but no children).
            make_run(3, Some(1), 0), // Newest run, child of run 1.
        ];
        let tree = RunTree::build(&runs);
        let items: Vec<_> = tree.iter().cloned().collect();

        // Tree A (run 1) comes first because max(run 1, run 3) > run 2.
        assert_eq!(
            items,
            vec![
                TreeIterItem {
                    run_id: Some(runs[0].run_id), // run 1 (root of tree A)
                    depth: 0,
                    is_last: true,
                    is_only_child: false,
                    continuation_flags: vec![],
                },
                TreeIterItem {
                    run_id: Some(runs[2].run_id), // run 3 (child of tree A)
                    depth: 1,
                    is_last: true,
                    is_only_child: false,
                    continuation_flags: vec![],
                },
                TreeIterItem {
                    run_id: Some(runs[1].run_id), // run 2 (tree B)
                    depth: 0,
                    is_last: true,
                    is_only_child: false,
                    continuation_flags: vec![],
                },
            ]
        );
    }

    #[proptest]
    fn tree_output_invariant_under_shuffle(
        #[strategy(arb_forest_with_shuffle())] runs_pair: (Vec<RunInfo>, Vec<RunInfo>),
    ) {
        let (runs, shuffled_runs) = runs_pair;

        // Build tree with original order.
        let original_tree = RunTree::build(&runs);
        let original_items: Vec<_> = original_tree.iter().cloned().collect();

        // Build tree with shuffled order.
        let shuffled_tree = RunTree::build(&shuffled_runs);
        let shuffled_items: Vec<_> = shuffled_tree.iter().cloned().collect();

        // The output should be identical regardless of input order.
        prop_assert_eq!(
            original_items,
            shuffled_items,
            "Tree structure should be invariant under input shuffle.\n\
             Original runs: {:?}\n\
             Shuffled runs: {:?}",
            runs.iter().map(|r| r.run_id).collect::<Vec<_>>(),
            shuffled_runs.iter().map(|r| r.run_id).collect::<Vec<_>>(),
        );
    }

    /// A tree node for generating arbitrary tree structures.
    #[derive(Debug, Clone)]
    struct TreeNode {
        id: u32,
        children: Vec<TreeNode>,
    }

    impl TreeNode {
        /// Flattens the tree into a list of RunInfo with parent relationships.
        fn flatten(&self, parent_id: Option<u32>, base_time: i64, runs: &mut Vec<RunInfo>) {
            let time_offset = runs.len() as i64;
            runs.push(RunInfo {
                run_id: format!("50000000-0000-0000-0000-{:012}", self.id)
                    .parse()
                    .expect("valid UUID"),
                parent_run_id: parent_id.map(|p| {
                    format!("50000000-0000-0000-0000-{:012}", p)
                        .parse()
                        .expect("valid UUID")
                }),
                started_at: chrono::FixedOffset::east_opt(0)
                    .unwrap()
                    .with_ymd_and_hms(2024, 6, 15, 12, 0, 0)
                    .unwrap()
                    + chrono::Duration::hours(base_time + time_offset),
            });

            for child in &self.children {
                child.flatten(Some(self.id), base_time, runs);
            }
        }
    }

    /// Strategy to generate a tree node with limited depth and branching.
    fn arb_tree_node(max_depth: usize, id: u32) -> BoxedStrategy<TreeNode> {
        if max_depth == 0 {
            Just(TreeNode {
                id,
                children: vec![],
            })
            .boxed()
        } else {
            (0..=2u32)
                .prop_flat_map(move |num_children| {
                    let child_strategies: Vec<BoxedStrategy<TreeNode>> = (0..num_children)
                        .map(|i| {
                            let child_id = id * 10 + i + 1;
                            arb_tree_node(max_depth - 1, child_id)
                        })
                        .collect();

                    child_strategies
                        .into_iter()
                        .collect::<Vec<BoxedStrategy<TreeNode>>>()
                        .prop_map(move |children| TreeNode { id, children })
                })
                .boxed()
        }
    }

    /// Strategy to generate a forest (multiple independent trees).
    fn arb_forest() -> impl Strategy<Value = Vec<RunInfo>> {
        // Generate 1-3 independent trees, each with depth 0-3.
        (1..=3usize, 0..=3usize).prop_flat_map(|(num_trees, max_depth)| {
            let tree_strategies: Vec<BoxedStrategy<TreeNode>> = (0..num_trees)
                .map(|tree_idx| {
                    let root_id = (tree_idx as u32 + 1) * 1000;
                    arb_tree_node(max_depth, root_id)
                })
                .collect();

            tree_strategies
                .into_iter()
                .collect::<Vec<BoxedStrategy<TreeNode>>>()
                .prop_map(move |trees: Vec<TreeNode>| {
                    let mut runs = Vec::new();
                    for (i, tree) in trees.iter().enumerate() {
                        tree.flatten(None, (i * 100) as i64, &mut runs);
                    }
                    runs
                })
        })
    }

    /// Strategy to generate a forest and a shuffled version of it.
    fn arb_forest_with_shuffle() -> impl Strategy<Value = (Vec<RunInfo>, Vec<RunInfo>)> {
        arb_forest().prop_perturb(|original, mut rng| {
            let mut shuffled = original.clone();
            shuffled.shuffle(&mut rng);
            (original, shuffled)
        })
    }
}
