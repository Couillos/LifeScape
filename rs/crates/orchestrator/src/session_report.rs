//! Task reports (contract 2, plan rendements R1): one measure, the
//! signed balance of a run. Every change of what the bot carries comes
//! from the SDK ledgers (`item_ledger`, `coin_ledger`) with the action
//! behind it; `flow_causes` names its cause with the game data (product,
//! input, ammo, rune, bait, supply, food, discarded, loot, fee, death,
//! shop, trade). The XP-bearing units (§7) ride along: products and
//! inputs counted gross (made then eaten in the same run included),
//! confirmed casts, kills and damage per NPC type (`run_events`).
//!
//! The net diff of holdings (pack, worn and bank, before/after) is only
//! a coherence check (D4, D19): an object whose net differs from the sum
//! of its flows raises a `residual` alert, like every `unattributed`
//! flow. Alerts are logged, never yields.
//!
//! The legacy `items` lines (frozen D45 provenances, one per name and
//! provenance) are derived from the causes so the current rates
//! aggregation reads the same contract until R2 replaces it. Market runs
//! keep `traded` on every non-food line (D44, D45).

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, HashMap};

use lifescape_sdk::client::run_events::RunEvents;
use lifescape_sdk::Bot;

use crate::flow_causes::{self, cause, BalanceLine, RawFlow, RunCtx};
use crate::report_meta::RunMeta;

/// Holdings across pack, bank and worn (one combined map: internal
/// transfers cancel, only boundary flows stay), plus XP, clocks and the
/// ledger positions. Snapshots hold the VISIBLE pack/worn only; the bank
/// rides alongside (first-open baseline at run start, last-seen at run
/// end), because the bank streams only while open.
pub struct Holdings {
    pub items: HashMap<u16, u32>,
    pub stats: HashMap<u8, (u8, i32)>,
    pub tick: u64,
    pub pause_ticks: u64,
    pub seen_messages: usize,
    /// Session bank-open counter (`Bot::bank_opens`) at the snapshot.
    pub bank_opens: u64,
    /// Coin ledger length at the snapshot (`Bot::coin_ledger`): the run's
    /// coin flows are the ones after it.
    pub coin_flows: usize,
    /// Item ledger length at the snapshot (`Bot::item_ledger`).
    pub item_flows: usize,
    /// Casts, kills and damage counted so far (`Bot::run_events`).
    pub events: RunEvents,
}

/// Split the live inventories into what the bot carries (the pack and
/// the worn equipment only) plus the live bank com (when the bank
/// streams right now). Interface mirrors of the pack (`bank_side:inv`,
/// `shop_template_side:inv`, `tradeside:inv`...) are never holdings: a
/// run that started with the bank open counted its pack twice and the
/// half that vanished at the close read `lost` (the coins "lost" of the
/// 2026-11-16 audit, live admin: carried 803408 -> 401704).
pub(crate) fn split_bank(bot: &Bot) -> (HashMap<u16, u32>, HashMap<u16, u32>) {
    let bank_com = (bot.tables.com_id)("bank_main:inv");
    let carried: Vec<u16> = ["inventory:inv", "wornitems:wear"].iter().filter_map(|name| (bot.tables.com_id)(name)).collect();
    let mut bank_live: HashMap<u16, u32> = HashMap::new();
    let mut visible: HashMap<u16, u32> = HashMap::new();
    for (com, slots) in &bot.world.inv {
        let target = if Some(*com) == bank_com {
            &mut bank_live
        } else if carried.contains(com) {
            &mut visible
        } else {
            continue;
        };
        for (held, count) in slots.values() {
            *target.entry(*held).or_insert(0) = target.get(held).copied().unwrap_or(0).saturating_add(*count);
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
        bank_opens: bot.bank_opens,
        coin_flows: bot.coin_ledger.flows.len(),
        item_flows: bot.item_ledger.flows.len(),
        events: bot.run_events.clone(),
    }
}

/// Refresh the bank cache from the live bank com when it streams (bank
/// open, even emptied), returning the live bank when seen. Closed, the
/// cache takes the bank as it stood at its stop-transmit (`World::
/// inv_closed`): one pump can drain a withdrawal's bank update together
/// with the close, and a cache refreshed only while streaming kept the
/// bank before that withdrawal (live 2026-11-16: residual +10 salmon,
/// +10 shrimps, +1 and +10 coins, -1 wooden shield, -1 jug of water).
/// Call on every heartbeat and before the end snapshot.
pub fn refresh_bank_cache(bot: &Bot, cache: &mut HashMap<u16, u32>) -> Option<HashMap<u16, u32>> {
    let bank_com = (bot.tables.com_id)("bank_main:inv");
    if bank_com.is_some_and(|com| bot.world.inv.contains_key(&com)) {
        let (_, bank_live) = split_bank(bot);
        *cache = bank_live.clone();
        Some(bank_live)
    } else {
        if let Some(closed) = bank_com.and_then(|com| bot.world.inv_closed.get(&com)) {
            let mut bank: HashMap<u16, u32> = HashMap::new();
            for (held, count) in closed.values() {
                *bank.entry(*held).or_insert(0) = bank.get(held).copied().unwrap_or(0).saturating_add(*count);
            }
            *cache = bank;
        }
        None
    }
}

/// Context the session owns: what ran, how it stopped, which graph node
/// ordered it and the run context frozen at its start.
pub struct ReportCtx {
    pub activity: String,
    pub node_id: String,
    pub stop: String,
    pub missing: Vec<String>,
    /// Typed cause of a non-success stop: (`Usage`|`Failed`|`Sdk`|
    /// `Blocked`|`exit`, message). The brain classifies on this, never on
    /// a session-wide `last_error` that may belong to an older run.
    pub error: Option<(String, String)>,
    pub meta: RunMeta,
}

/// One legacy item line (frozen D45 provenance), in display names.
pub struct FlowLine {
    pub name: String,
    pub provenance: &'static str,
    pub qty: i64,
}

/// XP-bearing units of a run (§7): products (P) and inputs (C) by
/// display name, confirmed casts (S) by spell, kills and damage (K) by
/// NPC name.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Units {
    pub products: BTreeMap<String, i64>,
    pub inputs: BTreeMap<String, i64>,
    pub casts: BTreeMap<String, i64>,
    pub kills: BTreeMap<String, i64>,
    pub damage: BTreeMap<String, i64>,
}

/// One alert: an `unattributed` flow or a coherence `residual` (net diff
/// minus the flows of that object).
#[derive(Debug, Clone, PartialEq)]
pub struct Alert {
    pub kind: &'static str,
    pub name: String,
    pub qty: i64,
    /// Residuals: the holdings behind the net (pack and bank, start ->
    /// end) and the ledger's sum, for the diagnosis; empty otherwise.
    pub detail: String,
}

/// The pushed report. `ticks` is wall-clock (work plus pauses);
/// `market_pause_ticks` the pause slice, `work_ticks` the rest.
pub struct TaskReport {
    pub activity: String,
    pub node_id: String,
    pub stop: String,
    pub missing: Vec<String>,
    pub ticks: u64,
    pub market_pause_ticks: u64,
    pub xp: Vec<(u8, String, i64)>,
    pub items: Vec<FlowLine>,
    pub flows: Vec<BalanceLine>,
    pub units: Units,
    pub alerts: Vec<Alert>,
    pub levels_before: Vec<(u8, u8)>,
    pub levels_after: Vec<(u8, u8)>,
    pub error: Option<(String, String)>,
    /// Bank interfaces opened during the run (silent-loop detector,
    /// `report_progress`).
    pub bank_opens: u64,
    pub meta: RunMeta,
}

/// Run start snapshot: what the session needs to close the report at
/// the end (budget for the hook, node for the push, holdings for the
/// diff, run context). Set on `run` of a skill script, taken at run end.
pub struct TaskStart {
    pub script: String,
    pub budget_max: u64,
    pub node_id: String,
    pub holdings: Holdings,
    pub pause_ticks: u64,
    pub meta: RunMeta,
}

/// Start carrier for the contract scripts; anything else runs the
/// legacy minimal path (no budget hook, no structured diff). `args` are
/// the script args (session flags stripped), `method` the planner method
/// the launch carried.
pub fn task_start_for(bot: &Bot, script: &str, args: &[String], method: Option<&str>) -> Option<TaskStart> {
    let meta = crate::report_meta::run_meta(bot, script, args, method);
    let activity: &'static str = match script {
        "fishing" => "fishing",
        "mining" => "mining",
        "woodcut" => "woodcut",
        "melee" => "melee",
        "cook" => "cook",
        "market" => return market_start_for(bot, args, meta),
        _ => return node_start_for(bot, script, args, meta),
    };
    // A CLI the cycle parser refuses still reports when the brain sent
    // it (`--node`): the script fails `Usage` and the brain must see it.
    let Ok(spec) = lifescape_scripts::kit::cycle::parse_cli(activity, args) else {
        return node_start_for(bot, script, args, meta);
    };
    Some(TaskStart {
        script: script.to_string(),
        budget_max: spec.budget_max_ticks,
        node_id: spec.node_id.clone(),
        holdings: snapshot_holdings(bot),
        pause_ticks: bot.pause_ticks,
        meta,
    })
}

/// Any script launched for a graph node (`--node ID`) reports its end,
/// so no node is left without an outcome (tutorial, walkto, quests,
/// equip...). No budget: those scripts end on their own.
fn node_start_for(bot: &Bot, script: &str, args: &[String], meta: RunMeta) -> Option<TaskStart> {
    let at = args.iter().position(|arg| arg == "--node")?;
    let node_id = args.get(at + 1).filter(|id| !id.is_empty() && !id.starts_with("--"))?.clone();
    Some(TaskStart { script: script.to_string(), budget_max: 0, node_id, holdings: snapshot_holdings(bot), pause_ticks: bot.pause_ticks, meta })
}

/// Flip engagements are time-boxed orders; buy/sell/shopbuy legs carry
/// the same budget/node flags for the session report. Their CLI is
/// orders, not cycle targets, so the flags are read straight off the
/// market CLI.
fn market_start_for(bot: &Bot, args: &[String], meta: RunMeta) -> Option<TaskStart> {
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
    Some(TaskStart { script: "market".to_string(), budget_max, node_id, holdings: snapshot_holdings(bot), pause_ticks: bot.pause_ticks, meta })
}

/// Base object of a carried id: a banknote (`cert_<name>`) counts for the
/// object it notes. The bank holds only base objects, a market lot rides
/// noted: kept apart, 281 noted iron ore bought then banked read as
/// residual +281 (base) and -281 (note) in one report (live 2026-11-16,
/// amber_moore fight:melee/men; 22 such pairs in the first 101 reports).
fn base_id(id: u16) -> u16 {
    u16::try_from(crate::market_audit::base_of(i64::from(id))).unwrap_or(id)
}

/// Holdings per base object (`base_id`), signed for the diff.
fn fold_notes(map: &HashMap<u16, u32>) -> HashMap<u16, i64> {
    let mut folded: HashMap<u16, i64> = HashMap::new();
    for (id, count) in map {
        *folded.entry(base_id(*id)).or_insert(0) += i64::from(*count);
    }
    folded
}

fn debug_name(id: u16) -> String {
    lifescape_gamedata::obj_by_id(usize::from(id)).and_then(|entry| entry.name).map(str::to_string).unwrap_or_else(|| format!("obj{id}"))
}

fn display_name(bot: &Bot, id: u16) -> String {
    (bot.tables.obj_name)(id).unwrap_or_else(|| format!("obj{id}"))
}

fn mask_skills(mask: u32) -> Vec<String> {
    (0u8..32).filter(|stat| mask & (1 << stat) != 0).map(|stat| lifescape_sdk::provenance::skill_name(stat).to_string()).collect()
}

/// Spell debug name of a spellbook button component, if it is one.
fn spell_of_com(com: u16) -> Option<String> {
    lifescape_gamedata::spells::all_spells()
        .into_iter()
        .find(|spell| lifescape_gamedata::com_id(&format!("magic:{}", spell.spell)) == Some(com))
        .map(|spell| spell.spell)
}

/// Build the report from the run's ledger flows, with the two visible
/// snapshots plus the bank baseline (first bank-open in-run) and the
/// last-seen bank for the coherence check.
pub fn build_report(
    bot: &Bot,
    before: &Holdings,
    bank_baseline: &HashMap<u16, u32>,
    after: &Holdings,
    bank_end: &HashMap<u16, u32>,
    ctx: &ReportCtx,
) -> TaskReport {
    let coins = (bot.tables.find_obj_by_name)("coins");
    let trading = crate::rates_service::is_trading_method(&ctx.activity);
    let mut stat_ids: Vec<u8> = before.stats.keys().chain(after.stats.keys()).copied().collect();
    stat_ids.sort_unstable();
    stat_ids.dedup();
    let xp: Vec<(u8, String, i64)> = stat_ids
        .iter()
        .map(|stat| (*stat, lifescape_sdk::provenance::skill_name(*stat).to_string(), xp_gain(before, after, *stat)))
        .filter(|(_, _, gain)| *gain != 0)
        .collect();
    let events = after.events.since(&before.events);
    let mut run = RunCtx {
        activity: ctx.activity.clone(),
        planner_method: ctx.meta.planner_method.clone(),
        run_skills: xp.iter().filter(|(_, _, gain)| *gain > 0).map(|(_, skill, _)| skill.clone()).collect(),
        kills: events.kills.values().sum(),
        made: Vec::new(),
    };
    // Item flows grouped per (object, ledger kind, XP window, sign):
    // the game data reads one group, not every tick.
    let mut groups: BTreeMap<(u16, &'static str, u32, bool), i64> = BTreeMap::new();
    for flow in bot.item_ledger.flows.iter().skip(before.item_flows) {
        if Some(flow.obj) == coins {
            continue;
        }
        *groups.entry((base_id(flow.obj), flow.kind, flow.skills, flow.delta > 0)).or_insert(0) += flow.delta;
    }
    let raw: Vec<(u16, RawFlow)> = groups
        .into_iter()
        .map(|((obj, kind, skills, _), delta)| {
            (obj, RawFlow { name: debug_name(obj), display: display_name(bot, obj), kind: kind.to_string(), delta, skills: mask_skills(skills) })
        })
        .collect();
    let raw_flows: Vec<RawFlow> = raw.iter().map(|(_, flow)| flow.clone()).collect();
    run.made = flow_causes::made_products(&raw_flows, &run);
    let mut lines: Vec<BalanceLine> = raw.iter().map(|(_, flow)| (flow.display.clone(), flow_causes::cause_of(flow, &run), flow.delta)).collect();
    let mut ledger_net: HashMap<u16, i64> = HashMap::new();
    for (obj, flow) in &raw {
        *ledger_net.entry(*obj).or_insert(0) += flow.delta;
    }
    if let Some(coin_id) = coins {
        let name = display_name(bot, coin_id);
        for (kind, net) in bot.coin_ledger.totals_since(before.coin_flows) {
            lines.push((name.clone(), flow_causes::coin_cause(kind, net, &run), net));
            *ledger_net.entry(coin_id).or_insert(0) += net;
        }
    }
    let flows = flow_causes::balance(&lines);
    let mut alerts: Vec<Alert> =
        flows.iter().filter(|(_, cause_name, _)| *cause_name == cause::UNATTRIBUTED).map(|(name, _, qty)| Alert { kind: "unattributed", name: name.clone(), qty: *qty, detail: String::new() }).collect();
    // Coherence (D4, D19): net holdings diff against the ledger, per base
    // object (notes folded: the bank holds only base objects).
    let (carried_start, carried_end) = (fold_notes(&before.items), fold_notes(&after.items));
    let (bank_start, bank_last) = (fold_notes(bank_baseline), fold_notes(bank_end));
    let mut ids: Vec<u16> = carried_start.keys().chain(carried_end.keys()).chain(bank_start.keys()).chain(bank_last.keys()).chain(ledger_net.keys()).copied().collect();
    ids.sort_unstable();
    ids.dedup();
    let held = |id: u16, map: &HashMap<u16, i64>| map.get(&id).copied().unwrap_or(0);
    for id in ids {
        let net = (held(id, &carried_end) - held(id, &carried_start)) + (held(id, &bank_last) - held(id, &bank_start));
        let flowed = ledger_net.get(&id).copied().unwrap_or(0);
        let residual = net - flowed;
        if residual != 0 {
            let detail = format!(
                "carried {}->{} bank {}->{} flows {flowed}",
                held(id, &carried_start),
                held(id, &carried_end),
                held(id, &bank_start),
                held(id, &bank_last)
            );
            alerts.push(Alert { kind: "residual", name: display_name(bot, id), qty: residual, detail });
        }
    }
    let mut legacy: BTreeMap<(String, &'static str), i64> = BTreeMap::new();
    for (name, cause_name, qty) in &flows {
        *legacy.entry((name.clone(), flow_causes::legacy_provenance(cause_name, *qty, trading))).or_insert(0) += qty;
    }
    let items: Vec<FlowLine> = legacy.into_iter().filter(|(_, qty)| *qty != 0).map(|((name, provenance), qty)| FlowLine { name, provenance, qty }).collect();
    let (products, inputs) = flow_causes::object_units(&flows);
    let npc = |npc_type: &u16| (bot.tables.npc_name)(*npc_type).unwrap_or_else(|| format!("npc{npc_type}"));
    let mut units = Units { products, inputs, ..Units::default() };
    // A confirmed press that is no spellbook button (a dialogue click
    // while the magic XP of a fight landed) names no spell: not a cast.
    for (com, count) in &events.casts {
        if let Some(spell) = spell_of_com(*com) {
            *units.casts.entry(spell).or_insert(0) += *count as i64;
        }
    }
    for (npc_type, count) in &events.kills {
        *units.kills.entry(npc(npc_type)).or_insert(0) += *count as i64;
    }
    for (npc_type, damage) in &events.damage {
        *units.damage.entry(npc(npc_type)).or_insert(0) += *damage as i64;
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
        error: ctx.error.clone(),
        ticks: after.tick.saturating_sub(before.tick),
        market_pause_ticks: after.pause_ticks.saturating_sub(before.pause_ticks),
        xp,
        items,
        flows,
        units,
        alerts,
        levels_before: levels(before),
        levels_after: levels(after),
        bank_opens: after.bank_opens.saturating_sub(before.bank_opens),
        meta: ctx.meta.clone(),
    }
}

fn xp_gain(before: &Holdings, after: &Holdings, stat: u8) -> i64 {
    let was = before.stats.get(&stat).map(|(_, xp)| *xp).unwrap_or(0);
    let now = after.stats.get(&stat).map(|(_, xp)| *xp).unwrap_or(was);
    (now.saturating_sub(was)) as i64
}

pub use crate::report_contract::report_value;

/// Closes the run's report: diffs the start snapshot, pushes the report
/// to the daemon, logs one JSON line and one line per alert
/// (`unattributed` flows, coherence `residual`s: objective zero).
pub(crate) fn finish_task_report(shared: &std::sync::Arc<crate::session::Shared>, stop: &str, missing: &[String], error: Option<&lifescape_json::Value>) {
    let Some(start) = shared.task_start.lock().unwrap().take() else { return };
    let mut bot = shared.bot.lock().unwrap();
    // The ledgers observe on tick boundaries: close the current tick so
    // they and the end snapshot see the same state, and take the bank
    // as it stands now (open or at its last stop-transmit).
    bot.observe_ledgers();
    refresh_bank_cache(&bot, &mut shared.last_bank.lock().unwrap());
    let end = snapshot_holdings(&bot);
    // The end cache counts only when the bank opened in-run (baseline
    // set): a run that never banks sees no bank moves, so pre-stock stays
    // invisible instead of popping as gains.
    let baseline = shared.bank_baseline.lock().unwrap().take();
    let bank_end = if baseline.is_some() { shared.last_bank.lock().unwrap().clone() } else { HashMap::new() };
    let baseline = baseline.unwrap_or_default();
    let ctx = ReportCtx {
        activity: start.script.clone(),
        node_id: start.node_id.clone(),
        stop: stop.to_string(),
        missing: missing.to_vec(),
        error: error.map(|error| (error.get("type").as_str().unwrap_or("?").to_string(), error.get("message").as_str().unwrap_or("").to_string())),
        meta: RunMeta {
            tool: crate::report_meta::end_fight_tool(&bot, &start.script, &start.meta.tool),
            combat_style: crate::report_meta::end_combat_style(&bot, &start.script, &start.meta.combat_style),
            ..start.meta.clone()
        },
    };
    let report = build_report(&bot, &start.holdings, &baseline, &end, &bank_end, &ctx);
    drop(bot);
    let value = report_value(&shared.account, &report);
    crate::session_push::push_task_report(&shared.account, &value);
    shared.log(&format!("task_report {}", lifescape_json::stringify(&value)));
    for alert in &report.alerts {
        shared.log(&format!("task_report alert {} {} {} [{}] ({} {})", alert.kind, alert.name, alert.qty, alert.detail, report.activity, report.meta.planner_method));
    }
}

#[cfg(test)]
#[path = "session_report_tests.rs"]
mod tests;
