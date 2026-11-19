#![forbid(unsafe_code)]
//! The record of the bots' choices from the guides (plan rendements R7):
//! each choice a resolve made (`ReadyCtx::drawn`: money from
//! `planner::money_pick`, training from `brain_train`) is logged with its
//! line, score, class, band and candidates and whether the exploration
//! slice chose it, then kept in the life's memory (`LifeMemory::picks`,
//! bounded; the draw counter moves on: the next draw of the bot is
//! another one). `orchestrator economy` reads them back (section « choix
//! des bots »: methods chosen, exploration share), staff out (rule 11).

use lifescape_planner::expand::ReadyCtx;
use lifescape_planner::life_memory::PickRecord;
use lifescape_planner::{Life, NodeKind};

/// Records the choice the resolve of `life.nodes[position]` made, when
/// the node kept it (a ban steering may move a money node off it: logged,
/// not recorded). `now`: Unix seconds.
pub(crate) fn record(life: &mut Life, position: usize, ctx: &ReadyCtx, now: i64) {
    let node = life.nodes[position].clone();
    record_choice(life, &node, ctx, now, false);
}

/// `record` for a filler window's coin node (outside the graph, plan
/// équilibre step 3): the same log and draw counter, kept under the kind
/// `filler` in its own pool (`LifeMemory::note_pick`): never a node's
/// pick pushed out.
pub(crate) fn record_filler(life: &mut Life, node: &lifescape_planner::Node, ctx: &ReadyCtx, now: i64) {
    record_choice(life, node, ctx, now, true);
}

fn record_choice(life: &mut Life, node: &lifescape_planner::Node, ctx: &ReadyCtx, now: i64, filler: bool) {
    let Some(mut choice) = ctx.drawn.borrow_mut().take() else { return };
    if filler {
        choice.kind = lifescape_planner::life_memory::FILLER_PICK_KIND;
    }
    let skill = match &node.kind {
        NodeKind::ReachLevel { skill, .. } => skill.clone(),
        _ => String::new(),
    };
    if node.method.as_deref() != Some(choice.method.as_str()) {
        crate::logln!("brain {}: {} {} pick {} not kept (now {:?})", life.account, node.id, choice.kind, choice.method, node.method);
        return;
    }
    crate::logln!(
        "brain {}: {} {} pick {}{} (score {:.3}, class {}, {} in band of {}{}{})",
        life.account,
        node.id,
        choice.kind,
        choice.method,
        if skill.is_empty() { String::new() } else { format!(" for {skill}") },
        choice.score,
        choice.class,
        choice.drawn.band,
        choice.drawn.candidates,
        if choice.drawn.explored { ", exploration" } else { "" },
        if choice.sticky { ", previous pick again" } else { "" }
    );
    let record = PickRecord {
        at: now,
        node: node.id.clone(),
        kind: choice.kind.to_string(),
        skill,
        method: choice.method.clone(),
        score: choice.score,
        class: choice.class,
        band: choice.drawn.band,
        candidates: choice.drawn.candidates,
        explored: choice.drawn.explored,
        sticky: choice.sticky,
    };
    life.memory.note_pick(record);
}

/// One kind's tally over the recorded picks: picks, explorations, sticky
/// picks, mean candidates, and (method, count) most chosen first.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PickTally {
    pub picks: usize,
    pub explored: usize,
    pub sticky: usize,
    pub mean_candidates: f64,
    pub methods: Vec<(String, usize)>,
}

/// The tally of the picks of `kind` at or after `since` (module doc).
pub fn tally(picks: &[PickRecord], kind: &str, since: i64) -> PickTally {
    let chosen: Vec<&PickRecord> = picks.iter().filter(|pick| pick.kind == kind && pick.at >= since).collect();
    let mut methods: Vec<(String, usize)> = Vec::new();
    for pick in &chosen {
        match methods.iter_mut().find(|(method, _)| *method == pick.method) {
            Some(known) => known.1 += 1,
            None => methods.push((pick.method.clone(), 1)),
        }
    }
    methods.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    let candidates: usize = chosen.iter().map(|pick| pick.candidates).sum();
    PickTally {
        picks: chosen.len(),
        explored: chosen.iter().filter(|pick| pick.explored).count(),
        sticky: chosen.iter().filter(|pick| pick.sticky).count(),
        mean_candidates: if chosen.is_empty() { 0.0 } else { candidates as f64 / chosen.len() as f64 },
        methods,
    }
}

/// The picks recorded in the life files (staff and excluded accounts
/// out), every account together.
pub fn fleet_picks(lives_dir: &std::path::Path) -> Vec<PickRecord> {
    let excluded = crate::economy_guard::excluded_accounts();
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(lives_dir) else { return out };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Some(account) = path.file_stem().and_then(|stem| stem.to_str()).map(str::to_string) else { continue };
        if excluded.contains(&account) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let Ok(value) = lifescape_json::parse(&text) else { continue };
        out.extend(lifescape_planner::life_memory::picks_from_value(&value.get("memory")));
    }
    out
}

/// The « choix des bots » section of `orchestrator economy` over the
/// last `hours`: per kind, the picks, the exploration share (D14 aims at
/// ~5 % while lines are unmeasured), the mean candidates and the
/// methods most chosen with their share.
pub fn print_section(picks: &[PickRecord], since: i64, hours: i64) {
    println!("bot choices (plan rendements R7, last {hours} h, staff out; recorded picks, the last {} per life):", lifescape_planner::life_memory::PICKS_CAP);
    for kind in ["money", "training", lifescape_planner::life_memory::FILLER_PICK_KIND] {
        let found = tally(picks, kind, since);
        if found.picks == 0 {
            println!("  {kind:<8} no pick recorded");
            continue;
        }
        println!(
            "  {kind:<8} {} picks, exploration {} ({:.1} %, target ~{:.0} %), previous pick again {}, {:.1} candidates on average",
            found.picks,
            found.explored,
            found.explored as f64 * 100.0 / found.picks as f64,
            lifescape_planner::guide_pick::EXPLORE_SHARE * 100.0,
            found.sticky,
            found.mean_candidates
        );
        for (method, count) in found.methods.iter().take(12) {
            println!("    {method:<44} {count:>5}  {:>5.1} %", *count as f64 * 100.0 / found.picks as f64);
        }
        if found.methods.len() > 12 {
            println!("    ... {} more methods", found.methods.len() - 12);
        }
    }
}

#[cfg(test)]
#[path = "brain_choice_bench_tests.rs"]
mod bench;

#[cfg(test)]
mod tests {
    use super::tally;
    use lifescape_planner::life_memory::PickRecord;

    fn pick(kind: &str, method: &str, at: i64, explored: bool) -> PickRecord {
        PickRecord { at, node: "n1".to_string(), kind: kind.to_string(), skill: String::new(), method: method.to_string(), score: 1.0, class: 0, band: 2, candidates: 4, explored, sticky: false }
    }

    /// The economy section's numbers: methods most chosen first, the
    /// exploration share, the window.
    #[test]
    fn the_tally_counts_methods_and_exploration_in_the_window() {
        let picks = vec![
            pick("money", "fight:melee/cows", 100, false),
            pick("money", "gather:woodcut/yew_logs", 110, false),
            pick("money", "fight:melee/cows", 120, true),
            pick("money", "fight:melee/cows", 10, false),
            pick("training", "fire:yew_logs", 120, false),
        ];
        let money = tally(&picks, "money", 50);
        assert_eq!((money.picks, money.explored), (3, 1));
        assert_eq!(money.methods, vec![("fight:melee/cows".to_string(), 2), ("gather:woodcut/yew_logs".to_string(), 1)]);
        assert!((money.mean_candidates - 4.0).abs() < 1e-9);
        assert_eq!(tally(&picks, "training", 50).picks, 1);
        assert_eq!(tally(&picks, "money", 1000).picks, 0);
    }
}
