//! Walking-cost shortlist ranking, shared by target selection (combat foes,
//! woodcut spots): Manhattan ranks the pool, pure costs reorder only a
//! bounded shortlist, unplannable sinks behind planned. Pure (a cost
//! closure, no world): callers precompute one `Bot::walk_costs` remote
//! batch (service L1, never a thread-side Dijkstra) and the closure reads
//! the batch back, so unwired and all-disconnected keep the Manhattan
//! order with no caller branching.

use std::cmp::Ordering;

/// Reorder `cands` (Manhattan order in) by pure walking cost: only the
/// first `shortlist` entries are costed, planned-first by ascending cost
/// (stable, so ties and the all-`None` case keep the Manhattan order),
/// the tail rides along untouched. Returns the costs with the candidates
/// so callers log the winning walk without re-planning (tail entries
/// were never costed, so they carry `None`). Fewer than two
/// candidates skip costing (no order can change).
pub fn rank_walk_shortlist<T>(
    cands: Vec<T>,
    shortlist: usize,
    cost_of: impl Fn(&T) -> Option<u32>,
) -> Vec<(Option<u32>, T)> {
    if cands.len() < 2 {
        return cands.into_iter().map(|cand| (None, cand)).collect();
    }
    let mut head: Vec<(Option<u32>, T)> = Vec::new();
    let mut tail: Vec<T> = Vec::new();
    for (index, cand) in cands.into_iter().enumerate() {
        if index < shortlist {
            head.push((cost_of(&cand), cand));
        } else {
            tail.push(cand);
        }
    }
    // Stable: unplannable keeps its Manhattan rank behind the planned.
    head.sort_by(|a, b| match (a.0, b.0) {
        (Some(first), Some(second)) => first.cmp(&second),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    });
    head.into_iter().chain(tail.into_iter().map(|cand| (None, cand))).collect()
}

#[cfg(test)]
mod tests {
    use super::rank_walk_shortlist;
    use std::cell::Cell;

    /// A walk-cheap candidate behind a Manhattan lure wins.
    #[test]
    fn planned_beats_manhattan_lure() {
        let ranked = rank_walk_shortlist(vec!["near", "far"], 2, |cand| match *cand {
            "near" => Some(50),
            _ => Some(36),
        });
        let order: Vec<_> = ranked.into_iter().map(|(_, cand)| cand).collect();
        assert_eq!(order, vec!["far", "near"]);
    }

    /// No planner (all `None`): the Manhattan order stands.
    #[test]
    fn unwired_keeps_manhattan() {
        let ranked = rank_walk_shortlist(vec!["near", "far"], 2, |_| None);
        let order: Vec<_> = ranked.into_iter().map(|(_, cand)| cand).collect();
        assert_eq!(order, vec!["near", "far"]);
    }

    /// Unplannable sinks behind planned, keeping its rank among the lost.
    #[test]
    fn unplannable_sinks_behind_planned() {
        let ranked = rank_walk_shortlist(vec!["lost", "found"], 2, |cand| match *cand {
            "found" => Some(9),
            _ => None,
        });
        assert_eq!(ranked, vec![(Some(9), "found"), (None, "lost")]);
    }

    /// Equal walks keep the Manhattan order (stable, no flip-flop).
    #[test]
    fn ties_keep_manhattan() {
        let ranked = rank_walk_shortlist(vec!["first", "second"], 2, |_| Some(10));
        let order: Vec<_> = ranked.into_iter().map(|(_, cand)| cand).collect();
        assert_eq!(order, vec!["first", "second"]);
    }

    /// Only the shortlist head is costed; the tail rides along in order.
    #[test]
    fn shortlist_bounds_plans_and_tail_stays() {
        let calls = Cell::new(0u32);
        let ranked = rank_walk_shortlist(vec!["a", "b", "c", "d", "e"], 2, |cand| {
            calls.set(calls.get() + 1);
            match *cand {
                "b" => Some(1),
                _ => Some(100),
            }
        });
        assert_eq!(calls.get(), 2, "never more plans than the shortlist");
        let order: Vec<_> = ranked.into_iter().map(|(_, cand)| cand).collect();
        assert_eq!(order, vec!["b", "a", "c", "d", "e"], "head reorders, tail untouched");
    }
}
