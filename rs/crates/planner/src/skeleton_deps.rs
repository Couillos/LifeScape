//! Skeleton dependencies of reach tranches (audit 2026-11-13 item A10:
//! "the brain computes every dependency recursively"). At expansion, each
//! tranche files, before it runs:
//! - one `obtain` per carried tool its method needs (`Way.tools`: the
//!   net, the pickaxe, the hammer, the mould, the talisman), shared by the
//!   whole chain;
//! - one `obtain` per object the tranche burns, sized by the XP it must
//!   earn (`sizing`: raw fish to cook, ores to smelt, bait, feathers,
//!   arrows, the spell's runes);
//! - the quest its method needs (`quests::method_quest`, expanded);
//! - the guild it must enter when every workable site of its method sits
//!   behind a guild door (`access:<guild>`, expanded).
//! The obtains of tranche k wait on tranche k−1 (just in time: a fisher at
//! 1 does not buy the feathers of 20→30 first). A dependency the bot
//! already holds closes at launch (`complete_node`, holdings proof).

use crate::model::{Archetype, Node, NodeKind, Profile};

fn next_id(counter: &mut usize) -> String {
    *counter += 1;
    format!("n{}", *counter)
}

/// Obtain node with every leg (Iron filters the market at resolve time,
/// like `replan::replan_node`).
fn obtain(item: &str, count: i64, deps: Vec<String>, counter: &mut usize) -> Node {
    let id = next_id(counter);
    let mut node = Node::new(&id, NodeKind::ObtainItems { item: item.to_string(), count }, deps, count);
    node.alternatives = crate::obtain_legs::obtain_alternatives(item, false);
    node
}

/// Profile for nested quest and guild expansions (only `ironman` reads
/// there, and resolution filters it again).
fn neutral_profile() -> Profile {
    Profile::new(false, 0.5, 0.0, 0, Archetype::Producer)
}

/// Guild every workable site of `method` sits behind (all gated, one
/// guild), if any.
fn guild_only(method: &str) -> Option<&'static str> {
    let table = crate::combat::method_site_table(method)?;
    let workable: Vec<&crate::combat::SiteRow> = table.iter().filter(|row| row.open_ever).collect();
    let mut guild: Option<&'static str> = None;
    for row in &workable {
        let gated = row.gates.iter().find_map(|gate| match gate {
            lifescape_gamedata::access::Gate::Guild { area, .. } => Some(*area),
            _ => None,
        });
        match (gated, guild) {
            (None, _) => return None,
            (Some(area), None) => guild = Some(area),
            (Some(area), Some(known)) if area != known => return None,
            _ => {}
        }
    }
    guild
}

/// Reach tranches from `from` to `to` with every dependency each tranche
/// needs filed in front of it (module doc). `depth` bounds nested quest
/// and guild expansions (a quest's own climbs file their deps once).
pub fn reach_chain_planned(skill: &str, from: u8, to: u8, counter: &mut usize, after: &mut String, depth: u8) -> Vec<Node> {
    let tranches = crate::expand::reach_chain(skill, from, to, counter, after);
    let mut out: Vec<Node> = Vec::new();
    let mut tools_filed: Vec<String> = Vec::new();
    let mut previous: Option<String> = None;
    let mut start = from.max(1);
    for mut tranche in tranches {
        let level = match &tranche.kind {
            NodeKind::ReachLevel { level, .. } => *level,
            _ => start,
        };
        let method = tranche.alternatives.first().map(|alt| alt.method.clone()).unwrap_or_default();
        let wait: Vec<String> = previous.iter().cloned().collect();
        let mut extra: Vec<String> = Vec::new();
        if let Some(need) = crate::tools::required_tool(&method) {
            // One obtain per tool for the whole chain: the first tranche
            // that needs it waits on it, later ones follow that tranche.
            for tool in &need.tools {
                if tools_filed.contains(tool) {
                    continue;
                }
                let node = obtain(tool, 1, wait.clone(), counter);
                tools_filed.push(tool.clone());
                extra.push(node.id.clone());
                out.push(node);
            }
        }
        for (item, units) in crate::sizing::tranche_consumption(&method, start, level) {
            if units <= 0 {
                continue;
            }
            let node = obtain(&item, units, wait.clone(), counter);
            extra.push(node.id.clone());
            out.push(node);
        }
        if depth > 0 {
            if let Some(quest) = crate::quests::method_quest(&method, None) {
                let chain = crate::expand::expand_quest_planned(&quest, &neutral_profile(), counter, &|_| 1, depth - 1);
                if let Some(last) = chain.last() {
                    extra.push(last.id.clone());
                }
                out.extend(chain);
            }
            if let Some(guild) = guild_only(&method) {
                let chain = crate::expand::expand_access_planned(guild, &neutral_profile(), counter, &|_| 1, depth - 1);
                if let Some(last) = chain.last() {
                    extra.push(last.id.clone());
                }
                out.extend(chain);
            }
        }
        for id in extra {
            if !tranche.deps.contains(&id) {
                tranche.deps.push(id);
            }
        }
        previous = Some(tranche.id.clone());
        out.push(tranche);
        start = level;
    }
    out
}

#[cfg(test)]
#[path = "skeleton_deps_tests.rs"]
mod tests;
