//! Task reports (contract v2): the session diffs holdings and XP
//! before/after a run and labels every net flow by provenance. Positive
//! flows: shop-coupled coins mark buys and sells; an activity product
//! landing with skill XP is `produced`; melee gains are `loot`; the rest
//! is `ground_spawn`. Negative flows: shop-coupled items are `shop_sold`;
//! food, bait and buried bones are `consumed`; the rest is `lost`.
//! shortcut: net-diff heuristic, exact for bounded skill runs; upgrade to
//! a per-flow SDK ledger when shop and trade flows need exactness too
//! (market lot).

#![forbid(unsafe_code)]

use std::collections::HashMap;

use lifescape_sdk::Bot;

use lifescape_json::{Value, object, str_value};

/// Holdings across pack, bank and worn (one combined map: internal
/// transfers cancel, only boundary flows stay), plus XP and clocks.
/// Snapshots hold the VISIBLE pack/worn only; the bank rides alongside
/// (first-open baseline at run start, last-seen at run end), because the
/// bank streams only while open. Pre-stock cancels instead of popping as
/// phantom gains.
pub struct Holdings {
    pub items: HashMap<u16, u32>,
    pub stats: HashMap<u8, (u8, i32)>,
    pub tick: u64,
    pub pause_ticks: u64,
    pub seen_messages: usize,
}

/// Split the live inventories into visible pack/worn plus the live bank
/// com (when the bank streams right now).
fn split_bank(bot: &Bot) -> (HashMap<u16, u32>, HashMap<u16, u32>) {
    let bank_com = (bot.tables.com_id)("bank_main:inv");
    let mut bank_live: HashMap<u16, u32> = HashMap::new();
    if let Some(com) = bank_com {
        if let Some(slots) = bot.world.inv.get(&com) {
            for (held, count) in slots.values() {
                *bank_live.entry(*held).or_insert(0) = bank_live.get(held).copied().unwrap_or(0).saturating_add(*count);
            }
        }
    }
    let mut visible: HashMap<u16, u32> = HashMap::new();
    for (com, slots) in &bot.world.inv {
        if Some(*com) == bank_com {
            continue;
        }
        for (held, count) in slots.values() {
            *visible.entry(*held).or_insert(0) = visible.get(held).copied().unwrap_or(0).saturating_add(*count);
        }
    }
    (visible, bank_live)
}

/// Snapshot the visible pack/worn once at run start; the end snapshot
/// diffs against it. Bank contents never enter here (see `split_bank`).
pub fn snapshot_holdings(bot: &Bot) -> Holdings {
    let (visible, _) = split_bank(bot);
    Holdings {
        items: visible,
        stats: bot.world.stats.clone(),
        tick: bot.world.ticks,
        pause_ticks: bot.pause_ticks,
        seen_messages: bot.world.messages.len(),
    }
}

/// Refresh the bank cache from the live bank com when it streams (bank
/// open, even emptied), returning the live bank when seen. Call on every
/// heartbeat: flows only happen while open, so the cache is the true
/// bank whenever it is closed.
pub fn refresh_bank_cache(bot: &Bot, cache: &mut HashMap<u16, u32>) -> Option<HashMap<u16, u32>> {
    let bank_com = (bot.tables.com_id)("bank_main:inv");
    if bank_com.is_some_and(|com| bot.world.inv.contains_key(&com)) {
        let (_, bank_live) = split_bank(bot);
        *cache = bank_live.clone();
        Some(bank_live)
    } else {
        None
    }
}

/// Context the session owns: what ran, how it stopped and which graph
/// node ordered it.
pub struct ReportCtx {
    pub activity: String,
    pub node_id: String,
    pub stop: String,
    pub missing: Vec<String>,
}

/// One reported item flow, in display names.
pub struct FlowLine {
    pub name: String,
    pub provenance: &'static str,
    pub qty: i64,
}

/// The pushed report: stop, ticks, XP and item deltas, levels around it.
/// `ticks` is wall-clock (work plus pauses); `market_pause_ticks` is
/// counted apart so rates divide by work ticks.
pub struct TaskReport {
    pub activity: String,
    pub node_id: String,
    pub stop: String,
    pub missing: Vec<String>,
    pub ticks: u64,
    pub market_pause_ticks: u64,
    pub xp: Vec<(u8, String, i64)>,
    pub items: Vec<FlowLine>,
    pub levels_before: Vec<(u8, u8)>,
    pub levels_after: Vec<(u8, u8)>,
}

/// Run start snapshot: what the session needs to close the report at
/// the end (budget for the hook, node for the push, holdings for the
/// diff). Set on `run` of a skill script, taken at run end.
pub struct TaskStart {
    pub script: String,
    pub budget_max: u64,
    pub node_id: String,
    pub holdings: Holdings,
    pub pause_ticks: u64,
}

/// Start carrier for the contract scripts; anything else runs the
/// legacy minimal path (no budget hook, no structured diff). Cook rides
/// the cycle parser; market flip carries its budget/node flags by hand
/// (its CLI is orders, not cycle targets).
pub fn task_start_for(bot: &Bot, script: &str, args: &[String]) -> Option<TaskStart> {
    let activity: &'static str = match script {
        "fishing" => "fishing",
        "mining" => "mining",
        "woodcut" => "woodcut",
        "melee" => "melee",
        "cook" => "cook",
        "market" => return market_start_for(bot, args),
        _ => return None,
    };
    let spec = lifescape_scripts::kit::cycle::parse_cli(activity, args).ok()?;
    Some(TaskStart {
        script: script.to_string(),
        budget_max: spec.budget_max_ticks,
        node_id: spec.node_id.clone(),
        holdings: snapshot_holdings(bot),
        pause_ticks: bot.pause_ticks,
    })
}

/// Flip engagements are time-boxed orders; buy/sell/shopbuy legs carry
/// the same budget/node flags for the session report. Their CLI is
/// orders, not cycle targets, so the flags are read straight off the
/// market CLI.
fn market_start_for(bot: &Bot, args: &[String]) -> Option<TaskStart> {
    match args.first().map(String::as_str) {
        Some("flip") | Some("buy") | Some("sell") | Some("shopbuy") | Some("cleanup") => {}
        _ => return None,
    }
    let mut budget_max = 0u64;
    let mut node_id = String::new();
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--budget-max" => {
                budget_max = args.get(index + 1).and_then(|raw| raw.parse().ok())?;
                index += 2;
            }
            "--node" => {
                node_id = args.get(index + 1).cloned()?;
                index += 2;
            }
            _ => {
                index += 1;
            }
        }
    }
    Some(TaskStart {
        script: "market".to_string(),
        budget_max,
        node_id,
        holdings: snapshot_holdings(bot),
        pause_ticks: bot.pause_ticks,
    })
}

/// Skill trained by one activity, if any (melee trains four at once).
pub fn activity_skill(activity: &str) -> Option<u8> {
    match activity {
        "woodcut" => Some(8),
        "mining" => Some(14),
        "fishing" => Some(10),
        _ => None,
    }
}

/// Product object ids of one activity (empty for melee and quests:
/// their gains are drops and rewards, never skill products).
pub fn activity_products(bot: &Bot, activity: &str) -> Vec<u16> {
    let names: Vec<String> = match activity {
        "woodcut" => lifescape_gamedata::trees::all_tiers().iter().map(|tier| tier.product.clone()).collect(),
        "mining" => lifescape_gamedata::rocks::all_rock_tiers().iter().map(|tier| tier.product.clone()).collect(),
        "fishing" => lifescape_gamedata::fishing::all_fish_tiers()
            .iter()
            .flat_map(|tier| tier.products.iter().map(|product| product.to_string()))
            .collect(),
        _ => Vec::new(),
    };
    names.into_iter().filter_map(|name| (bot.tables.find_obj_by_name)(&name)).collect()
}

fn obj_id(bot: &Bot, name: &str) -> Option<u16> {
    (bot.tables.find_obj_by_name)(name)
}

fn display_name(bot: &Bot, id: u16) -> String {
    (bot.tables.obj_name)(id).unwrap_or_else(|| format!("obj{id}"))
}

/// Bait ids (consumed, never produced): the fish tiers declare them.
fn bait_ids(bot: &Bot) -> Vec<u16> {
    lifescape_gamedata::fishing::all_fish_tiers()
        .iter()
        .filter_map(|tier| tier.bait)
        .filter_map(|name| obj_id(bot, name))
        .collect()
}

/// Build the report from two visible snapshots plus the bank baseline
/// (first bank-open in-run) and the last-seen bank. Visible deltas plus
/// bank deltas are the true flows; pre-stock cancels instead of popping
/// as phantom gains. Pure over the snapshots plus the bot's tables.
pub fn build_report(
    bot: &Bot,
    before: &Holdings,
    bank_baseline: &HashMap<u16, u32>,
    after: &Holdings,
    bank_end: &HashMap<u16, u32>,
    ctx: &ReportCtx,
) -> TaskReport {
    let products = activity_products(bot, &ctx.activity);
    let skill = activity_skill(&ctx.activity);
    let coins = obj_id(bot, "coins");
    let baits = bait_ids(bot);
    let mut ids: Vec<u16> = before
        .items
        .keys()
        .chain(after.items.keys())
        .chain(bank_baseline.keys())
        .chain(bank_end.keys())
        .copied()
        .collect();
    ids.sort_unstable();
    ids.dedup();
    let net = |held: u16, map: &HashMap<u16, u32>| map.get(&held).copied().unwrap_or(0) as i64;
    let mut deltas: Vec<(u16, i64)> = Vec::new();
    for id in ids {
        let delta = (net(id, &after.items) - net(id, &before.items)) + (net(id, bank_end) - net(id, bank_baseline));
        if delta != 0 {
            deltas.push((id, delta));
        }
    }
    let coins_delta = coins.and_then(|id| deltas.iter().find(|(held, _)| *held == id).map(|(_, delta)| *delta)).unwrap_or(0);
    let positives = deltas.iter().any(|(id, delta)| *delta > 0 && Some(*id) != coins);
    let negatives = deltas.iter().any(|(id, delta)| *delta < 0 && Some(*id) != coins);
    // A buy in the same run means a shop visit: unsold-looking negatives
    // are sales, not losses (eating still wins: food never sells).
    let shop_visit = positives && coins_delta < 0;
    let prayer_gain = xp_gain(before, after, 5);
    let mut items: Vec<FlowLine> = Vec::new();
    for (id, delta) in deltas {
        let name = display_name(bot, id);
        let low = name.to_lowercase();
        let provenance = if delta > 0 {
            if Some(id) == coins {
                if negatives {
                    lifescape_sdk::provenance::Provenance::ShopSold.as_str()
                } else if ctx.activity == "melee" {
                    lifescape_sdk::provenance::Provenance::Loot.as_str()
                } else {
                    lifescape_sdk::provenance::Provenance::GroundSpawn.as_str()
                }
            } else if coins_delta < 0 {
                lifescape_sdk::provenance::Provenance::ShopBought.as_str()
            } else if skill.is_some_and(|stat| products.contains(&id) && xp_gain(before, after, stat) > 0) {
                lifescape_sdk::provenance::Provenance::Produced.as_str()
            } else if ctx.activity == "melee" {
                lifescape_sdk::provenance::Provenance::Loot.as_str()
            } else {
                lifescape_sdk::provenance::Provenance::GroundSpawn.as_str()
            }
        } else if Some(id) == coins {
            if positives {
                lifescape_sdk::provenance::Provenance::ShopBought.as_str()
            } else {
                lifescape_sdk::provenance::Provenance::Lost.as_str()
            }
        } else if coins_delta > 0 {
            lifescape_sdk::provenance::Provenance::ShopSold.as_str()
        } else if lifescape_gamedata::food::heal(&low) > 0 || baits.contains(&id) || (low.contains("bones") && prayer_gain > 0) {
            lifescape_sdk::provenance::Provenance::Consumed.as_str()
        } else if shop_visit {
            lifescape_sdk::provenance::Provenance::ShopSold.as_str()
        } else {
            lifescape_sdk::provenance::Provenance::Lost.as_str()
        };
        items.push(FlowLine { name, provenance, qty: delta });
    }
    items.sort_by(|left, right| left.name.cmp(&right.name));
    let mut xp: Vec<(u8, String, i64)> = Vec::new();
    let mut stat_ids: Vec<u8> = before.stats.keys().chain(after.stats.keys()).copied().collect();
    stat_ids.sort_unstable();
    stat_ids.dedup();
    for stat in stat_ids.iter().copied() {
        let gain = xp_gain(before, after, stat);
        if gain != 0 {
            xp.push((stat, lifescape_sdk::provenance::skill_name(stat).to_string(), gain));
        }
    }
    let levels = |held: &Holdings| {
        let mut levels: Vec<(u8, u8)> = stat_ids.iter().map(|stat| (*stat, held.stats.get(stat).map(|(level, _)| *level).unwrap_or(1))).collect();
        levels.sort_by_key(|(stat, _)| *stat);
        levels
    };
    TaskReport {
        activity: ctx.activity.clone(),
        node_id: ctx.node_id.clone(),
        stop: ctx.stop.clone(),
        missing: ctx.missing.clone(),
        ticks: after.tick.saturating_sub(before.tick),
        market_pause_ticks: after.pause_ticks.saturating_sub(before.pause_ticks),
        xp,
        items,
        levels_before: levels(before),
        levels_after: levels(after),
    }
}

fn xp_gain(before: &Holdings, after: &Holdings, stat: u8) -> i64 {
    let was = before.stats.get(&stat).map(|(_, xp)| *xp).unwrap_or(0);
    let now = after.stats.get(&stat).map(|(_, xp)| *xp).unwrap_or(was);
    (now.saturating_sub(was)) as i64
}

/// Report JSON for the daemon push and the session log line.
pub fn report_value(account: &str, report: &TaskReport) -> Value {
    let xp: Vec<Value> = report
        .xp
        .iter()
        .map(|(stat, skill, gain)| {
            object(vec![
                ("stat", Value::Int(i64::from(*stat))),
                ("skill", str_value(skill)),
                ("xp", Value::Int(*gain)),
            ])
        })
        .collect();
    let items: Vec<Value> = report
        .items
        .iter()
        .map(|flow| {
            object(vec![
                ("name", str_value(&flow.name)),
                ("provenance", str_value(flow.provenance)),
                ("qty", Value::Int(flow.qty)),
            ])
        })
        .collect();
    let levels = |held: &[(u8, u8)]| {
        Value::Arr(
            held.iter()
                .map(|(stat, level)| {
                    object(vec![("stat", Value::Int(i64::from(*stat))), ("level", Value::Int(i64::from(*level)))])
                })
                .collect(),
        )
    };
    object(vec![
        ("account", str_value(account)),
        ("activity", str_value(&report.activity)),
        ("node_id", str_value(&report.node_id)),
        ("stop", str_value(&report.stop)),
        ("missing", Value::Arr(report.missing.iter().map(|name| str_value(name)).collect())),
        ("ticks", Value::Int(report.ticks as i64)),
        ("market_pause_ticks", Value::Int(report.market_pause_ticks as i64)),
        ("xp", Value::Arr(xp)),
        ("items", Value::Arr(items)),
        ("levels_before", levels(&report.levels_before)),
        ("levels_after", levels(&report.levels_after)),
    ])
}

#[cfg(test)]
mod tests {
    use super::{activity_products, activity_skill, build_report, report_value, snapshot_holdings, Holdings, ReportCtx};
    use lifescape_sdk::Bot;
    use std::collections::HashMap;

    fn live_bot() -> Bot {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        lifescape_scripts::attach_live_tables(&mut bot);
        bot
    }

    fn empty_bank() -> HashMap<u16, u32> {
        HashMap::new()
    }

    fn stocked(bot: &mut Bot, items: &[(u16, u32)], stats: &[(u8, (u8, i32))]) {
        let com = (bot.tables.com_id)("inventory:inv").expect("inventory:inv in dumps");
        bot.world.inv.insert(com, items.iter().enumerate().map(|(slot, (held, count))| (slot as u8, (*held, *count))).collect());
        for (stat, value) in stats {
            bot.world.stats.insert(*stat, *value);
        }
    }

    fn holdings(items: &[(u16, u32)], stats: &[(u8, (u8, i32))], tick: u64) -> Holdings {
        Holdings {
            items: items.iter().map(|(id, count)| (*id, *count)).collect(),
            stats: stats.iter().map(|(stat, value)| (*stat, *value)).collect(),
            tick,
            pause_ticks: 0,
            seen_messages: 0,
        }
    }

    fn ctx(activity: &str, stop: &str) -> ReportCtx {
        ReportCtx { activity: activity.to_string(), node_id: "n9".to_string(), stop: stop.to_string(), missing: Vec::new() }
    }

    /// Woodcut gains with WC XP land as produced; levels ride along.
    #[test]
    fn logs_with_xp_are_produced() {
        let mut bot = live_bot();
        let logs = (bot.tables.find_obj_by_name)("logs").expect("logs in dumps");
        let before = holdings(&[], &[(8, (2, 100))], 1000);
        let after = holdings(&[(logs, 25)], &[(8, (2, 350))], 1600);
        let report = build_report(&bot, &before, &empty_bank(), &after, &empty_bank(), &ctx("woodcut", "goal_reached"));
        assert_eq!(report.ticks, 600);
        assert_eq!(report.xp, vec![(8, "woodcutting".to_string(), 250)]);
        assert_eq!(report.items.len(), 1);
        assert_eq!((report.items[0].provenance, report.items[0].qty), ("produced", 25));
        assert_eq!(report.levels_before, vec![(8, 2)]);
        assert_eq!(report.levels_after, vec![(8, 2)]);
    }

    /// Melee gains are loot, even with combat XP; eaten food is consumed.
    #[test]
    fn melee_drops_are_loot_food_is_consumed() {
        let mut bot = live_bot();
        let bones = (bot.tables.find_obj_by_name)("bones").expect("bones in dumps");
        let shrimp = (bot.tables.find_obj_by_name)("shrimp").expect("shrimp in dumps");
        let before = holdings(&[(shrimp, 3)], &[(0, (23, 6900))], 1000);
        let after = holdings(&[(bones, 2), (shrimp, 2)], &[(0, (23, 7100))], 2000);
        let report = build_report(&bot, &before, &empty_bank(), &after, &empty_bank(), &ctx("melee", "budget_exhausted"));
        let by_name: HashMap<&str, &str> =
            report.items.iter().map(|flow| (flow.name.as_str(), flow.provenance)).collect();
        let bones_name = (bot.tables.obj_name)(bones).expect("bones display");
        let shrimp_name = (bot.tables.obj_name)(shrimp).expect("shrimp display");
        assert_eq!(by_name[bones_name.as_str()], "loot");
        assert_eq!(by_name[shrimp_name.as_str()], "consumed");
    }

    /// Coins coupling marks the shop flows: sold logs out, bought tool
    /// in; coins alone without a counterpart are found, not earned.
    #[test]
    fn coins_coupling_marks_shop_flows() {
        let mut bot = live_bot();
        let logs = (bot.tables.find_obj_by_name)("logs").expect("logs in dumps");
        let axe = (bot.tables.find_obj_by_name)("bronze_axe").expect("bronze_axe in dumps");
        let coins = (bot.tables.find_obj_by_name)("coins").expect("coins in dumps");
        let before = holdings(&[(logs, 20), (coins, 5)], &[], 100);
        let after = holdings(&[(coins, 3), (axe, 1)], &[], 200);
        let report = build_report(&bot, &before, &empty_bank(), &after, &empty_bank(), &ctx("woodcut", "goal_reached"));
        let by_name: HashMap<&str, (&str, i64)> =
            report.items.iter().map(|flow| (flow.name.as_str(), (flow.provenance, flow.qty))).collect();
        let logs_name = (bot.tables.obj_name)(logs).expect("logs display");
        let axe_name = (bot.tables.obj_name)(axe).expect("axe display");
        assert_eq!(by_name[logs_name.as_str()].0, "shop_sold");
        assert_eq!(by_name[axe_name.as_str()].0, "shop_bought");
    }

    /// Unexplained losses are lost; a lone coin drain with no counterpart
    /// is a fee, not a buy.
    #[test]
    fn unexplained_losses_are_lost() {
        let bot = live_bot();
        let coins = (bot.tables.find_obj_by_name)("coins").expect("coins in dumps");
        let before = holdings(&[(coins, 30)], &[], 100);
        let after = holdings(&[(coins, 20)], &[], 200);
        let report = build_report(&bot, &before, &empty_bank(), &after, &empty_bank(), &ctx("woodcut", "failed"));
        assert_eq!(report.items.len(), 1);
        assert_eq!((report.items[0].provenance, report.items[0].qty), ("lost", -10));
    }

    /// Ground gains with no XP and no shop coupling read as ground
    /// spawns, never as skill products.
    #[test]
    fn uncoupled_gains_are_ground_spawns() {
        let mut bot = live_bot();
        let feather = (bot.tables.find_obj_by_name)("feather").expect("feather in dumps");
        let before = holdings(&[], &[(8, (2, 100))], 100);
        let after = holdings(&[(feather, 5)], &[(8, (2, 100))], 200);
        let report = build_report(&bot, &before, &empty_bank(), &after, &empty_bank(), &ctx("woodcut", "goal_reached"));
        assert_eq!(report.items[0].provenance, "ground_spawn");
    }

    /// Activity tables: the three gatherers expose their products, melee
    /// exposes none (its gains are drops).
    #[test]
    fn activity_tables_cover_gatherers() {
        let bot = live_bot();
        assert_eq!(activity_skill("woodcut"), Some(8));
        assert_eq!(activity_skill("mining"), Some(14));
        assert_eq!(activity_skill("melee"), None);
        assert!(!activity_products(&bot, "woodcut").is_empty());
        assert!(!activity_products(&bot, "mining").is_empty());
        assert!(!activity_products(&bot, "fishing").is_empty());
        assert!(activity_products(&bot, "melee").is_empty());
    }

    /// Start carrier: the contract scripts parse their CLI into a
    /// budget and a node; anything else runs the minimal legacy path.
    #[test]
    fn task_start_parses_budget_and_node() {
        use super::task_start_for;
        let bot = live_bot();
        let start = task_start_for(
            &bot,
            "woodcut",
            &["--budget-max".to_string(), "6000".to_string(), "--node".to_string(), "n9".to_string()],
        )
        .expect("contract script carries a start");
        assert_eq!((start.budget_max, start.node_id.as_str()), (6000, "n9"));
        assert!(task_start_for(&bot, "quest_cook", &[]).is_none(), "quests stay on the minimal path");
        assert!(task_start_for(&bot, "woodcut", &["--frobnicate".to_string()]).is_none(), "junk CLI carries nothing");
        let cook = task_start_for(
            &bot,
            "cook",
            &["--budget-max".to_string(), "6000".to_string(), "--node".to_string(), "n9".to_string()],
        )
        .expect("cook carries a start");
        assert_eq!((cook.budget_max, cook.node_id.as_str()), (6000, "n9"));
        let flip = task_start_for(
            &bot,
            "market",
            &["flip".to_string(), "--budget-max".to_string(), "9000".to_string(), "--node".to_string(), "n3".to_string()],
        )
        .expect("flip carries a start");
        assert_eq!((flip.budget_max, flip.node_id.as_str()), (9000, "n3"));
        let cleanup = task_start_for(
            &bot,
            "market",
            &["cleanup".to_string(), "--node".to_string(), "n2".to_string(), "--budget-min".to_string(), "0".to_string(), "--budget-max".to_string(), "6000".to_string()],
        )
        .expect("cleanup carries a start");
        assert_eq!((cleanup.budget_max, cleanup.node_id.as_str()), (6000, "n2"), "janitor reports to its node");
        assert!(task_start_for(&bot, "market", &["deliver".to_string(), "1".to_string(), "2".to_string(), "3=1,1".to_string()]).is_none(), "deliver legs stay minimal");
    }

    /// Live snapshot reads the combined inventories (pack, bank, worn all
    /// count: internal trips cancel).
    #[test]
    fn live_snapshot_aggregates_everything_open() {
        let mut bot = live_bot();
        let logs = (bot.tables.find_obj_by_name)("logs").expect("logs in dumps");
        stocked(&mut bot, &[(logs, 4)], &[(8, (2, 100))]);
        let held = snapshot_holdings(&bot);
        assert_eq!(held.items.get(&logs), Some(&4));
        assert_eq!(held.stats.get(&8), Some(&(2, 100)));
    }

    /// Closed bank: the cache stands in while open, and the first-open
    /// baseline cancels pre-stock: only in-run bank moves report.
    #[test]
    fn closed_bank_reads_from_cache() {
        use super::{refresh_bank_cache, split_bank};
        let mut bot = live_bot();
        let logs = (bot.tables.find_obj_by_name)("logs").expect("logs in dumps");
        let bank_com = (bot.tables.com_id)("bank_main:inv").expect("bank_main in dumps");
        let pack_com = (bot.tables.com_id)("inventory:inv").expect("inventory:inv in dumps");
        bot.world.inv.insert(bank_com, [(0u8, (logs, 100u32))].into_iter().collect());
        bot.world.inv.insert(pack_com, [(0u8, (logs, 3u32))].into_iter().collect());
        let mut cache = HashMap::new();
        assert!(refresh_bank_cache(&bot, &mut cache).is_some(), "open bank warms the cache");
        assert_eq!(cache.get(&logs), Some(&100), "open bank warms the cache");
        let (visible, live) = split_bank(&bot);
        assert_eq!(visible.get(&logs), Some(&3), "visible excludes the bank com");
        assert_eq!(live.get(&logs), Some(&100));
        // Bank closes: the cache keeps the stock, refresh reports unseen.
        bot.world.inv.remove(&bank_com);
        let mut cache_closed = cache.clone();
        assert!(refresh_bank_cache(&bot, &mut cache_closed).is_none(), "closed bank refreshes nothing");
        assert_eq!(cache_closed.get(&logs), Some(&100), "closed bank keeps the cache");
        let held = snapshot_holdings(&bot);
        assert_eq!(held.items.get(&logs), Some(&3), "snapshot holds visible only");
        // Opened but emptied: the cache follows to empty, never stale.
        bot.world.inv.insert(bank_com, HashMap::new());
        assert!(refresh_bank_cache(&bot, &mut cache_closed).is_some(), "emptied bank still streams");
        assert!(cache_closed.is_empty(), "emptied bank clears the cache");
    }

    /// Pre-stock cancels: a bank holding 100 logs at first-open plus 25
    /// deposited in-run reports +25 produced (with XP), never +125.
    #[test]
    fn prestock_cancels_against_the_baseline() {
        let mut bot = live_bot();
        let logs = (bot.tables.find_obj_by_name)("logs").expect("logs in dumps");
        let before = holdings(&[(logs, 3)], &[(8, (2, 100))], 100);
        let after = holdings(&[(logs, 3)], &[(8, (2, 350))], 200);
        let baseline: HashMap<u16, u32> = [(logs, 100)].into_iter().collect();
        let end: HashMap<u16, u32> = [(logs, 125)].into_iter().collect();
        let report = build_report(&bot, &before, &baseline, &after, &end, &ctx("woodcut", "goal_reached"));
        assert_eq!(report.items.len(), 1);
        assert_eq!((report.items[0].provenance, report.items[0].qty), ("produced", 25));
        // Bank untouched in-run (never opened): pre-stock on both sides
        // is no flow at all, even when the cache knows it.
        let report = build_report(&bot, &before, &baseline, &after, &baseline, &ctx("woodcut", "goal_reached"));
        assert!(report.items.is_empty(), "unmoved pre-stock reports nothing, got {:?}", report.items.iter().map(|flow| (&flow.name, flow.qty)).collect::<Vec<_>>());
    }

    /// Report JSON carries every contract field for the daemon push.
    #[test]
    fn report_json_carries_the_contract() {
        let mut bot = live_bot();
        let logs = (bot.tables.find_obj_by_name)("logs").expect("logs in dumps");
        let before = holdings(&[], &[(8, (2, 100))], 100);
        let after = holdings(&[(logs, 25)], &[(8, (2, 350))], 700);
        let report = build_report(&bot, &before, &empty_bank(), &after, &empty_bank(), &ctx("woodcut", "budget_exhausted"));
        let value = report_value("t01", &report);
        assert_eq!(value.get("account").as_str(), Some("t01"));
        assert_eq!(value.get("activity").as_str(), Some("woodcut"));
        assert_eq!(value.get("stop").as_str(), Some("budget_exhausted"));
        assert_eq!(value.get("ticks").as_i64(), Some(600));
        assert_eq!(value.get("market_pause_ticks").as_i64(), Some(0));
        assert_eq!(value.get("xp").as_arr().map(|rows| rows.len()), Some(1));
        assert_eq!(value.get("items").as_arr().map(|rows| rows.len()), Some(1));
        assert!(!value.get("levels_before").is_null() && !value.get("levels_after").is_null());
    }
}
