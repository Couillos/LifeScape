//! Engagement draws and replanning triggers (§5.2). A draw is uniform
//! in [6000, 18000] at the start of the activity script itself;
//! dependencies never count. Replanning fires on blocked/failed/death,
//! lost stock, price drift over threshold, or engagement end.

use crate::model::{Alternative, Node, NodeKind, NodeStatus};

/// Engagement bounds (D1 ticks; 6 min–18 min in dev, 1 h–3 h in prod).
pub const ENGAGE_MIN: u64 = 6000;
pub const ENGAGE_MAX: u64 = 18000;
/// Price drift that forces a re-resolve of a ready node.
pub const PRICE_DRIFT_PCT: f64 = 0.25;

/// Deterministic xorshift (no RNG dependency): same seed and counter
/// always draw the same engagement, so profiles replay.
pub fn draw_engagement_ticks(seed: u64, counter: u64) -> u64 {
    let mut word = seed.wrapping_add(counter.wrapping_mul(0x9E37_79B9_7F4A_7C15)).max(1);
    word ^= word << 13;
    word ^= word >> 7;
    word ^= word << 17;
    ENGAGE_MIN + word % (ENGAGE_MAX - ENGAGE_MIN + 1)
}

/// Why the brain replans a node.
#[derive(Debug, Clone, PartialEq)]
pub enum ReplanCause {
    Blocked,
    Failed,
    Death,
    StockLost,
    PriceDrift,
    EngagementEnd,
}

/// Decide from a pushed report: the stop string plus typed details the
/// session already reports (`missing`, death markers). `None` means the
/// node continues (goal not reached, budget not exhausted).
pub fn needs_replan(stop: &str, missing: &[String], died: bool, stock_lost: bool, price_drift: bool) -> Option<ReplanCause> {
    if died {
        return Some(ReplanCause::Death);
    }
    match stop {
        "blocked" => Some(ReplanCause::Blocked),
        "failed" => Some(ReplanCause::Failed),
        // P1.6: an expired rescue (typed `RescueBudget` in last_error,
        // dossier on disk) replans like a failure: the node retries with
        // a fresh method instead of stranding.
        "rescue_budget" => Some(ReplanCause::Failed),
        _ => {
            if stock_lost {
                Some(ReplanCause::StockLost)
            } else if price_drift {
                Some(ReplanCause::PriceDrift)
            } else if stop == "budget_exhausted" {
                Some(ReplanCause::EngagementEnd)
            } else {
                let _ = missing;
                None
            }
        }
    }
}

/// One inventory load of a consumed input (the last slot holds the tool:
/// tinderbox, hammer, knife).
pub const INPUT_BATCH: i64 = 27;

/// Food a fight blocked on food asks for (the ladder's `MAX_FOOD` load).
pub const FOOD_BATCH: i64 = lifescape_gamedata::melee_ladder::MAX_FOOD as i64;

/// Object a skill method works on: the tail of `cook/raw_shrimp`,
/// `fire:logs`, `smelt:bronze_bar`, `spin:ball_of_wool`.
fn method_subject(method: &str) -> Option<&str> {
    let (_, tail) = method.split_once(['/', ':'])?;
    Some(tail.rsplit('/').next().unwrap_or(tail))
}

/// How many of `need` an obtain sub-task asks for. A consumed input of
/// the node's own work comes by the inventory load: a cook missing raw
/// shrimps asks 27, a smelter 13 copper and 13 tin (live 2026-11-12: one
/// unit per block bought one fish, and three blocks abandoned the
/// node). An obtain node's input scales with its count (5 bronze bars =
/// 5 copper + 5 tin). Tools and anything else come one at a time.
pub fn need_count(node: &Node, need: &str) -> i64 {
    need_count_from(node, need, None)
}

/// Level a reach tranche starts from: the live level when known, else
/// the cut below its target (`tranches`).
fn tranche_start(skill: &str, target: u8, live: Option<u8>) -> u8 {
    if let Some(level) = live.filter(|level| *level >= 1) {
        return level.min(target);
    }
    crate::tranches::tranches(skill, 1, target).into_iter().filter(|cut| *cut < target).max().unwrap_or(1)
}

/// `need_count` for a node whose skill stands at `live` (the brain's live
/// level): a reach tranche asks what the rest of the tranche burns — XP
/// still to earn divided by the XP one action pays, times the units each
/// action consumes (`sizing`: 180 feathers for trout 20→30, every shot an
/// arrow, the spell's runes per cast) —, never one unit nor one load.
pub fn need_count_from(node: &Node, need: &str, live: Option<u8>) -> i64 {
    need_count_with(node, need, live, &crate::work_rate::prior_rate)
}

/// `need_count_from` reading the yields from `rate` (R7: the brain's
/// measured reader, `plan_read`).
pub fn need_count_with(node: &Node, need: &str, live: Option<u8>, rate: crate::work_rate::RateReader) -> i64 {
    if let NodeKind::ReachLevel { skill, level } = &node.kind {
        let method = node.method.clone().or_else(|| node.alternatives.first().map(|alt| alt.method.clone())).unwrap_or_default();
        let from = tranche_start(skill, *level, live);
        if let Some(units) = crate::sizing::tranche_need_with(rate, &method, from, *level, need) {
            // A batch launch (alchemy casts the stock covers) asks only what
            // the batch burns: the climb re-prices after it.
            if node.batch > 0 {
                let per_action = crate::sizing::per_action_need_with(rate, &method, from, need).unwrap_or(1);
                return units.min(node.batch.saturating_mul(per_action.max(1))).max(1);
            }
            // A random recipe (iron smelts) runs dry before the climb's
            // end at the expected burn: the margin of `input_margin`.
            let odds = crate::input_margin::recipe_success(&method);
            let per_action = crate::sizing::per_action_need_with(rate, &method, from, need).unwrap_or(1);
            // One work session at a time: the market is asked what one
            // 6000-tick window burns, the next block buys the next.
            let sized = crate::input_margin::sized_inputs(units as f64, per_action, odds).max(units).max(1);
            return crate::sizing::session_need_with(rate, &method, from, need).map_or(sized, |session| sized.min(session)).max(1);
        }
        // A climb whose subject is made from intermediate stages (pie
        // dishes from soft clay): the tranche's actions times the base
        // components per unit (clay, water vessels capped).
        if let (Some(actions), Some(subject)) = (crate::sizing::tranche_actions_with(rate, &method, from, *level), method_subject(&method)) {
            if let Some(per_unit) = nested_per_unit(subject, need, NESTED_STAGES) {
                let actions = crate::sizing::session_actions_with(rate, &method, from).map_or(actions, |session| actions.min(session));
                if let Some(cap) = refilled_vessels(node, need) {
                    return actions.min(cap).max(1);
                }
                return ((actions as f64) * per_unit).ceil().max(1.0) as i64;
            }
        }
    }
    // Food a fight blocked on, whatever the node: a drop farm is an
    // `obtain_items` of its drop (live 2026-11-22 wolfe_carl: `farm:
    // uncut_sapphire` blocked on swordfish asked 1, sized on the sapphires,
    // and 34 sibling nodes launched and blocked on the same fish in a row).
    if let Some(method) = node.method.as_deref() {
        if crate::combat::is_fight_method(method) && lifescape_gamedata::food::heal(need) > 0 {
            return FOOD_BATCH;
        }
    }
    // A 100 % market lot (R5) buys what its lot burns (`node.batch`
    // products, the recipe's failures counted), never one load.
    if let Some(units) = node.method.as_deref().and_then(|method| crate::trade_craft::input_count(method, need, node.batch)) {
        return units;
    }
    // A quest blocked on a brought item obtains the quest's count (20
    // balls of wool for Sheep Shearer, never one).
    if let NodeKind::CompleteQuest { quest } = &node.kind {
        if let Some(item) = lifescape_gamedata::quest_requirements::by_key(quest).and_then(|required| required.items.into_iter().find(|item| item.item == need)) {
            return item.count.max(1);
        }
    }
    let (product, units) = match &node.kind {
        NodeKind::ObtainItems { item, count } => (item.clone(), Some((*count).max(1))),
        _ => {
            let method = node.method.clone().or_else(|| node.alternatives.first().map(|alt| alt.method.clone())).unwrap_or_default();
            // A melee hunt blocked on food (`Blocked: shrimp`, the ladder
            // never fights a biting spot foodless) asks a full load: one
            // shrimp per block would block again after the first bite.
            if crate::combat::is_fight_method(&method) && lifescape_gamedata::food::heal(need) > 0 {
                return FOOD_BATCH;
            }
            match method_subject(&method) {
                Some(subject) => (subject.to_string(), None),
                None => return 1,
            }
        }
    };
    // `cook/raw_shrimp`, `fire:logs`, `tan:cow_hide`: the subject is the
    // consumed input itself.
    if units.is_none() && product == need {
        return INPUT_BATCH;
    }
    // The node's own method first (a baked meal eats its flattened base
    // components, a superheat its runes), then every production way.
    let locked = node.method.as_deref().and_then(crate::method_way::method_way).map(|(_, way)| way);
    for way in locked.into_iter().chain(lifescape_gamedata::obtain::ways(&product)) {
        if !way.is_production() {
            continue;
        }
        if way.tools.iter().any(|tool| tool == need) {
            return 1;
        }
        let Some((_, per_action)) = way.inputs.iter().find(|(input, _)| input == need) else { continue };
        let per_action = (*per_action).max(1);
        if let Some(count) = units {
            if let Some(cap) = refilled_vessels(node, need) {
                return count.min(cap).max(1);
            }
        }
        return match units {
            // Inputs per unit made: an iron smelt fails half the time
            // (`Way::expected_out` 0.5), so a bar costs two ores.
            Some(count) => match way.expected_out() {
                out if out > 0.0 => crate::input_margin::sized_inputs(count as f64 * per_action as f64 / out, per_action, way.success),
                _ => count.saturating_mul(per_action),
            },
            None => {
                let load: i64 = way.inputs.iter().map(|(_, qty)| (*qty).max(1)).sum();
                (INPUT_BATCH / load.max(1)).max(1) * per_action
            }
        };
    }
    // An input of an intermediate stage (clay and water of the soft clay
    // a pie dish is shaped from): the scaled count, never one unit (live
    // 2026-11-14, `obtain piedish_unfired 16` blocked on clay asked 1).
    if let Some(count) = units {
        if let Some(cap) = refilled_vessels(node, need) {
            return count.min(cap).max(1);
        }
        if let Some(per_unit) = nested_per_unit(&product, need, NESTED_STAGES) {
            return ((count as f64) * per_unit).ceil().max(1.0) as i64;
        }
    }
    1
}

/// Intermediate stages `nested_per_unit` looks through (pie dish ← soft
/// clay ← clay: two).
const NESTED_STAGES: u8 = 2;

/// Water vessels a pottery run carries: the script refills the emptied
/// jugs at the nearest fountain between mixes (`activities/pottery.rs`),
/// so a handful serves any count.
pub const POTTERY_VESSELS: i64 = 7;

/// Vessel cap when `need` is a water vessel the node's method refills
/// itself (pottery), `None` otherwise.
fn refilled_vessels(node: &Node, need: &str) -> Option<i64> {
    let method = node.method.as_deref().or_else(|| node.alternatives.first().map(|alt| alt.method.as_str()))?;
    let water = matches!(need, "jug_water" | "bucket_water" | "bowl_water");
    (water && method.starts_with("pottery:")).then_some(POTTERY_VESSELS)
}

/// Units of `need` one unit of `product` burns through up to `depth`
/// intermediate production stages (inputs per action ÷ expected output
/// at every stage). `None` when no production way leads to it.
pub fn nested_per_unit(product: &str, need: &str, depth: u8) -> Option<f64> {
    for way in lifescape_gamedata::obtain::ways(product) {
        if !way.is_production() {
            continue;
        }
        let out = if way.expected_out() > 0.0 { way.expected_out() } else { 1.0 };
        for (input, per_action) in &way.inputs {
            let per_unit = (*per_action).max(1) as f64 / out;
            if input == need {
                return Some(per_unit);
            }
            if depth > 0 && input != product {
                if let Some(inner) = nested_per_unit(input, need, depth - 1) {
                    return Some(per_unit * inner);
                }
            }
        }
    }
    None
}

/// True when `need` of a market climb (`Node::buys_inputs`, user lever
/// 2026-11-17: « les profils marché achètent leurs intrants ») comes by
/// the market only: an input or consumable the market trades (bars, logs,
/// raw fish, runes, bones), never a carried tool of the method (the
/// hammer, the tinderbox: their usual legs). The obtain then bids at the
/// tranche's volume (`need_count_with`, a need bid capped by the wallet,
/// `resolve_obtain::buy_quantity`).
pub fn buys_by_market(node: &Node, need: &str) -> bool {
    if !node.buys_inputs || !crate::obtain_legs::tradeable_f2p(need) {
        return false;
    }
    let method = node.method.as_deref().or_else(|| node.alternatives.first().map(|alt| alt.method.as_str())).unwrap_or("");
    !crate::tools::required_tool(method).is_some_and(|tools| tools.tools.iter().any(|tool| tool == need))
}

/// Apply one replan cause: returns fresh sub-task nodes to insert (e.g. an
/// `obtain_items` for each missing tool/input). The node itself moves to
/// `ready` (retry with a new method) or stays `blocked` until its deps
/// land; `failed` with no alternative abandons it.
pub fn replan_node(node: &mut Node, cause: &ReplanCause, missing: &[String], counter: &mut usize) -> Vec<Node> {
    replan_node_from(node, cause, missing, counter, None)
}

/// `replan_node` with the node's skill at its live level (`need_count_from`).
pub fn replan_node_from(node: &mut Node, cause: &ReplanCause, missing: &[String], counter: &mut usize, live: Option<u8>) -> Vec<Node> {
    replan_node_with(node, cause, missing, counter, live, &crate::work_rate::prior_rate)
}

/// `replan_node_from` sizing its obtains from `rate` (`need_count_with`).
pub fn replan_node_with(node: &mut Node, cause: &ReplanCause, missing: &[String], counter: &mut usize, live: Option<u8>, rate: crate::work_rate::RateReader) -> Vec<Node> {
    match cause {
        ReplanCause::Blocked => {
            let mut fresh = Vec::new();
            for need in missing {
                *counter += 1;
                let count = need_count_with(node, need, live, rate);
                let mut sub = Node::new(
                    &format!("n{}", *counter),
                    NodeKind::ObtainItems { item: need.clone(), count },
                    Vec::new(),
                    count,
                );
                // Same legs as creation (PRD3: market first, every
                // production way, the shop under D-A); Iron is filtered
                // at resolve time, which re-prices them. A 100 % market
                // lot (R5) buys its inputs on the market only (a tool it
                // lacks comes by the usual legs); so does a climb whose
                // training pick is a market line (user lever 2026-11-17).
                sub.alternatives = if node.method.as_deref().is_some_and(|method| crate::trade_craft::buys_input(method, need)) || buys_by_market(node, need) {
                    vec![Alternative { method: "market:buy".to_string(), est_ticks: 0, est_coins: 0 }]
                } else {
                    crate::obtain_legs::obtain_alternatives(need, false)
                };
                fresh.push(sub);
            }
            // The blocked node waits on its new deps (or retries when
            // nothing concrete is missing).
            if fresh.is_empty() {
                node.status = NodeStatus::Ready;
                node.method = None;
            } else {
                let deps: Vec<String> = fresh.iter().map(|sub| sub.id.clone()).collect();
                node.deps.extend(deps);
                node.status = NodeStatus::Pending;
            }
            fresh
        }
        ReplanCause::Failed | ReplanCause::Death | ReplanCause::StockLost => {
            node.status = NodeStatus::Ready;
            node.method = None;
            node.spent_ticks = 0;
            Vec::new()
        }
        ReplanCause::PriceDrift => {
            // Method re-resolves at the next `ready` pass with fresh
            // numbers; the old lock drops so a cheaper way can win.
            node.status = NodeStatus::Ready;
            node.method = None;
            Vec::new()
        }
        ReplanCause::EngagementEnd => {
            match &node.kind {
                NodeKind::MakeMoney => {
                    node.status = NodeStatus::Done;
                    node.progress_current = node.progress_target;
                    Vec::new()
                }
                _ => {
                    node.status = NodeStatus::Ready;
                    node.method = None;
                    Vec::new()
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ENGAGE_MAX, ENGAGE_MIN, ReplanCause, draw_engagement_ticks, needs_replan, replan_node};
    use crate::model::{Node, NodeKind, NodeStatus};

    /// Live 2026-11-14 (harry_page n133, `obtain piedish_unfired 16`,
    /// `pottery:piedish_unfired`): the script blocked on clay and the
    /// sub-task asked ONE clay (the dish is shaped from soft clay, the
    /// clay sits one stage deeper). The count scales through the soft
    /// clay stage; the water vessels the script refills are capped.
    #[test]
    fn nested_inputs_scale_with_the_order() {
        let mut node = Node::new("n133", NodeKind::ObtainItems { item: "piedish_unfired".to_string(), count: 16 }, Vec::new(), 16);
        node.method = Some("pottery:piedish_unfired".to_string());
        assert_eq!(super::need_count(&node, "clay"), 16, "one clay per soft clay per dish");
        assert_eq!(super::need_count(&node, "softclay"), 16, "direct input unchanged");
        assert_eq!(super::need_count(&node, "jug_water"), super::POTTERY_VESSELS, "refilled vessels, a handful");
        assert_eq!(super::nested_per_unit("piedish", "clay", 2), Some(1.0), "dish ← unfired ← soft clay ← clay");
        assert_eq!(super::nested_per_unit("piedish", "iron_ore", 2), None);
        assert_eq!(super::need_count(&node, "bucket_water"), super::POTTERY_VESSELS, "the script blocks on buckets");
        let water = crate::obtain_legs::obtain_alternatives("bucket_water", false);
        assert!(water.iter().any(|alt| alt.method.starts_with("fill:")), "buckets are filled by the brain: {water:?}");
    }

    #[test]
    fn engagement_draw_stays_in_bounds_and_replays() {
        for counter in 0..50 {
            let drawn = draw_engagement_ticks(918273645, counter);
            assert!((ENGAGE_MIN..=ENGAGE_MAX).contains(&drawn), "draw {drawn} in [6000, 18000]");
        }
        assert_eq!(draw_engagement_ticks(42, 3), draw_engagement_ticks(42, 3), "seeded draws replay");
        assert_ne!(draw_engagement_ticks(42, 3), draw_engagement_ticks(42, 4), "counter advances the draw");
    }

    #[test]
    fn triggers_cover_the_prd_list() {
        assert_eq!(needs_replan("blocked", &[], false, false, false), Some(ReplanCause::Blocked));
        assert_eq!(needs_replan("failed", &[], false, false, false), Some(ReplanCause::Failed));
        assert_eq!(needs_replan("rescue_budget", &[], false, false, false), Some(ReplanCause::Failed), "expired rescue replans");
        assert_eq!(needs_replan("goal_reached", &[], true, false, false), Some(ReplanCause::Death), "death wins over the stop");
        assert_eq!(needs_replan("goal_reached", &[], false, true, false), Some(ReplanCause::StockLost));
        assert_eq!(needs_replan("goal_reached", &[], false, false, true), Some(ReplanCause::PriceDrift));
        assert_eq!(needs_replan("budget_exhausted", &[], false, false, false), Some(ReplanCause::EngagementEnd));
        assert_eq!(needs_replan("goal_reached", &[], false, false, false), None, "progress continues");
    }

    #[test]
    fn blocked_spawns_obtain_and_pends() {
        let mut node = Node::new("n9", NodeKind::ObtainItems { item: "logs".to_string(), count: 10 }, Vec::new(), 10);
        let fresh = replan_node(&mut node, &ReplanCause::Blocked, &["bronze_axe".to_string()], &mut 9);
        assert_eq!(fresh.len(), 1);
        assert!(matches!(&fresh[0].kind, NodeKind::ObtainItems { item, .. } if item == "bronze_axe"));
        assert_eq!(node.status, NodeStatus::Pending, "waits on its new dep");
        assert_eq!(node.deps, vec!["n10".to_string()]);
    }

    /// Multi-input recipes (server `smelting.struct`, `smithing.dbrow`):
    /// a smelter blocked on both ores files both obtains, each sized by
    /// the recipe; iron fails half its smelts, so a bar asks two ores;
    /// a smith asks five bars per platebody and one hammer.
    #[test]
    fn multi_input_recipes_file_every_input_at_recipe_size() {
        let obtain = |item: &str, count: i64| Node::new("n9", NodeKind::ObtainItems { item: item.to_string(), count }, Vec::new(), count);
        let mut bronze = obtain("bronze_bar", 14);
        let fresh = replan_node(&mut bronze, &ReplanCause::Blocked, &["copper_ore".to_string(), "tin_ore".to_string()], &mut 20);
        let asked: Vec<(String, i64)> = fresh
            .iter()
            .filter_map(|sub| match &sub.kind {
                NodeKind::ObtainItems { item, count } => Some((item.clone(), *count)),
                _ => None,
            })
            .collect();
        assert_eq!(asked, vec![("copper_ore".to_string(), 14), ("tin_ore".to_string(), 14)], "both ores, one each per bar");
        assert_eq!(bronze.deps, vec!["n21".to_string(), "n22".to_string()], "the smelt waits on both");
        assert_eq!(super::need_count(&obtain("iron_bar", 10), "iron_ore"), 22, "half the iron smelts fail, plus the 10 % margin");
        assert_eq!(super::need_count(&obtain("iron_bar", 5), "iron_ore"), 11);
        assert_eq!(super::need_count(&obtain("steel_bar", 10), "iron_ore"), 10, "steel never fails");
        assert_eq!(super::need_count(&obtain("steel_bar", 10), "coal"), 20, "two coal per steel bar");
        assert_eq!(super::need_count(&obtain("mithril_bar", 3), "coal"), 12);
        assert_eq!(super::need_count(&obtain("bronze_platebody", 2), "bronze_bar"), 10, "five bars per platebody");
        assert_eq!(super::need_count(&obtain("bronze_platebody", 2), "hammer"), 1, "the hammer is a tool");
    }

    /// Live 2026-11-12: a cook missing raw shrimps filed `obtain 1`: one
    /// fish per block, abandoned after three. Inputs now come by the load.
    #[test]
    fn missing_inputs_come_by_the_inventory_load() {
        let reach = |method: &str| {
            let mut node = Node::new("n7", NodeKind::ReachLevel { skill: "cooking".to_string(), level: 20 }, Vec::new(), 20);
            node.method = Some(method.to_string());
            node
        };
        // Audit 2026-11-13 (A9): a reach tranche asks what the rest of the
        // tranche burns (cooking 10→20 shrimps: 30 XP each), never a load.
        let shrimps = crate::sizing::tranche_need("cook/raw_shrimp", 10, 20, "raw_shrimp").expect("shrimp xp");
        assert_eq!(super::need_count(&reach("cook/raw_shrimp"), "raw_shrimp"), shrimps, "the cook's own input, tranche-sized");
        assert_eq!(super::need_count_from(&reach("cook/raw_shrimp"), "raw_shrimp", Some(15)), crate::sizing::tranche_need("cook/raw_shrimp", 15, 20, "raw_shrimp").expect("xp"), "from the live level");
        assert!(super::need_count(&reach("fire:logs"), "logs") > super::INPUT_BATCH);
        assert_eq!(super::need_count(&reach("fire:logs"), "tinderbox"), 1, "a tool comes alone");
        let copper = super::need_count(&reach("smelt:bronze_bar"), "copper_ore");
        assert_eq!(copper, super::need_count(&reach("smelt:bronze_bar"), "tin_ore"), "copper and tin pair up");
        let bars = Node::new("n8", NodeKind::ObtainItems { item: "bronze_bar".to_string(), count: 5 }, Vec::new(), 5);
        assert_eq!(super::need_count(&bars, "tin_ore"), 5, "an obtain scales its inputs");
        // The melee ladder blocks on food: a full load, never one shrimp.
        assert_eq!(super::need_count(&reach("fight:melee/ladder"), "shrimp"), super::FOOD_BATCH);
        assert_eq!(super::FOOD_BATCH, 20);
        // A drop farm hunting a biting spot blocks on food too (ordered
        // spots follow the food rule since 2026-11-17): a full load.
        assert_eq!(super::need_count(&reach("farm:naturerune"), "trout"), super::FOOD_BATCH);
        assert_eq!(super::need_count(&reach("fight:melee/ladder"), "bronze_sword"), 1, "gear still comes alone");
        let mut cook = reach("cook/raw_shrimp");
        let fresh = replan_node(&mut cook, &ReplanCause::Blocked, &["raw_shrimp".to_string()], &mut 7);
        assert!(matches!(&fresh[0].kind, NodeKind::ObtainItems { item, count } if item == "raw_shrimp" && *count == shrimps), "{:?}", fresh[0].kind);
    }

    /// Live 2026-11-22 wolfe_carl: a drop farm (an obtain of its drop)
    /// blocked on food asks a full load, never one fish sized on the drop.
    #[test]
    fn a_drop_farm_obtain_blocked_on_food_asks_a_full_load() {
        let mut farm = Node::new("n9", NodeKind::ObtainItems { item: "uncut_sapphire".to_string(), count: 100 }, Vec::new(), 100);
        farm.method = Some("farm:uncut_sapphire".to_string());
        assert_eq!(super::need_count(&farm, "swordfish"), super::FOOD_BATCH);
        assert_eq!(super::need_count(&farm, "bronze_sword"), 1, "gear still comes alone");
    }

    /// A baked meal blocks on its base components (`cook_meals` assembles
    /// the pie in the pack): sized from the locked method's flattened way.
    #[test]
    fn meals_size_their_base_components() {
        let mut pies = Node::new("n8", NodeKind::ObtainItems { item: "redberry_pie".to_string(), count: 10 }, Vec::new(), 10);
        pies.method = Some("bake:redberry_pie".to_string());
        assert_eq!(super::need_count(&pies, "redberries"), 10);
        assert_eq!(super::need_count(&pies, "pot_flour"), 10);
    }

    #[test]
    fn engagement_end_closes_money_only() {
        let mut money = Node::new("n3", NodeKind::MakeMoney, Vec::new(), 0);
        replan_node(&mut money, &ReplanCause::EngagementEnd, &[], &mut 3);
        assert_eq!(money.status, NodeStatus::Done, "one engagement = one money node");
        let mut level = Node::new("n4", NodeKind::ReachLevel { skill: "fishing".to_string(), level: 20 }, Vec::new(), 20);
        replan_node(&mut level, &ReplanCause::EngagementEnd, &[], &mut 4);
        assert_eq!(level.status, NodeStatus::Ready, "levels continue toward the goal");
    }
}

#[cfg(test)]
#[path = "market_climb_tests.rs"]
mod market_climb_tests;
