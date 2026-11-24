//! One work session at a time (user decision 2026-11-22): the inputs a
//! level climb obtains are sized to what ONE session of its method burns
//! (`sizing::session_need_with`, one 6000-tick window), never the whole
//! tranche. New nodes are sized so at creation (`replan::need_count_with`,
//! `skeleton_deps`); this pass applies the rule to the state already
//! persisted: an open input obtain of a climb whose count passes its
//! session shrinks to it, the next block buys the next session. The bid
//! on the book follows by its own refresh (remainder cut to the need).

use crate::model::{Life, NodeKind, NodeStatus};

/// True when `resize_climb_inputs` has something to look at: an open
/// level climb with a method whose dependencies include an open input
/// obtain (the walk's own conditions, without the yield reads). False means
/// the resize returns no line and changes nothing: the callers skip the
/// readers they would build for it.
pub fn has_open_climb_inputs(life: &Life) -> bool {
    life.nodes.iter().any(|climb| {
        if !matches!(climb.kind, NodeKind::ReachLevel { .. }) || matches!(climb.status, NodeStatus::Done | NodeStatus::Abandoned) {
            return false;
        }
        if climb.method.is_none() && climb.alternatives.is_empty() {
            return false;
        }
        climb.deps.iter().any(|dep| life.nodes.iter().any(|node| node.id == *dep && matches!(node.kind, NodeKind::ObtainItems { .. }) && !matches!(node.status, NodeStatus::Done | NodeStatus::Abandoned)))
    })
}

/// Brings every open input obtain of an open level climb down to one
/// session, reading yields from `rate` and the skill levels from
/// `level_of`. Returns one line per resized node. Idempotent.
pub fn resize_climb_inputs(life: &mut Life, level_of: &dyn Fn(&str) -> u8, rate: crate::work_rate::RateReader) -> Vec<String> {
    let mut resized: Vec<(usize, i64)> = Vec::new();
    for climb in &life.nodes {
        let NodeKind::ReachLevel { skill, .. } = &climb.kind else { continue };
        if matches!(climb.status, NodeStatus::Done | NodeStatus::Abandoned) {
            continue;
        }
        let Some(method) = climb.method.clone().or_else(|| climb.alternatives.first().map(|alt| alt.method.clone())) else { continue };
        let from = level_of(skill).max(1);
        for dep in &climb.deps {
            let Some(position) = life.nodes.iter().position(|node| node.id == *dep) else { continue };
            let node = &life.nodes[position];
            let NodeKind::ObtainItems { item, count } = &node.kind else { continue };
            if matches!(node.status, NodeStatus::Done | NodeStatus::Abandoned) {
                continue;
            }
            if let Some(session) = crate::sizing::session_need_with(rate, &method, from, item).filter(|session| session < count) {
                resized.push((position, session));
            }
        }
    }
    let mut lines = Vec::new();
    for (position, session) in resized {
        let node = &mut life.nodes[position];
        if let NodeKind::ObtainItems { item, count } = &mut node.kind {
            if session < *count {
                lines.push(format!("{} {item} {count} -> {session} (one session)", node.id));
                *count = session;
                node.progress_target = session;
            }
        }
    }
    lines
}

#[cfg(test)]
#[path = "session_resize_tests.rs"]
mod tests;
