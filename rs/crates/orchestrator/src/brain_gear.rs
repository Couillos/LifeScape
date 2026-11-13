#![forbid(unsafe_code)]
//! Equipment ladder in the brain (user rule 2026-11-13; pure rules in
//! `lifescape_planner::gear_ladder`). Once per `GEAR_EVERY_SECS` for an
//! idle, non-Iron bot of a melee (or non-combat) goal:
//! - a dead ladder step (abandoned) leaves the graph on EVERY pass, any
//!   goal style (`withdraw_dead`, before `graph_state`: never a Stuck goal);
//! - ceilings share the FREE wallet (`free_coins`: coins minus what the
//!   bot's other resting buys commit); a goal leaving melee cancels its
//!   ladder bids;
//! - with no step open, the ladder names the next piece (one tier above
//!   what the bot owns, weapon first, by its live levels) and:
//!   - an owned piece not worn yet → an `equip_best` step;
//!   - a seller on the market, or a wallet too thin to bid → an
//!     `obtain_items` (market only) + `equip_best` step, launched before
//!     the goal's next node: a covered ask is bought at once, a dearer one
//!     blocks on the wallet whose share covers it (`have_coins` on the
//!     coin-dropping spots);
//!   - no seller → a standing bid (D-B) at the reference walking up to
//!     `GEAR_SHARE_PCT` of the wallet: it waits for a producer while the
//!     bot keeps working, its ceiling following the wallet.
//! - Bids on other ladder pieces (an older step, an old best-gear wish)
//!   are cancelled: one gear budget at a time.
//!
//! Ranged and magic goals keep the wishlist (`brain_wishlist`): the
//! ladder is the melee metal set.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use lifescape_json::Value;
use lifescape_planner::gear_ladder::{self, GearMove, LADDER};
use lifescape_planner::Life;

use crate::brain::push_history;
use crate::jobs::Job;

/// Wall seconds between two ladder passes for one account.
pub const GEAR_EVERY_SECS: f64 = 60.0;
/// Wall seconds before an `equip_best` step for the same piece is filed
/// again (a wear that did not take never loops).
pub const EQUIP_RETRY_SECS: f64 = 3600.0;

fn wall_now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs_f64()).unwrap_or(0.0)
}

fn last_pass() -> &'static Mutex<HashMap<String, f64>> {
    static TABLE: OnceLock<Mutex<HashMap<String, f64>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn last_equip() -> &'static Mutex<HashMap<(String, String), f64>> {
    static TABLE: OnceLock<Mutex<HashMap<(String, String), f64>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// True when the goal fights melee or does not fight (producers fight
/// melee too, `goal_combat_style`).
pub fn ladder_goal(life: &Life) -> bool {
    lifescape_planner::goal_combat_style(&life.goal) == "melee"
}

pub(crate) fn obj_id(item: &str) -> Option<i64> {
    lifescape_gamedata::objs_by_name(item).into_iter().next().map(|obj| obj.obj_id as i64)
}

/// Owned debug names (pack, bank, worn) and worn names.
fn holdings(job: &Job) -> (Vec<String>, Vec<String>) {
    let worn = crate::brain_plan::worn_names(job);
    let mut owned: Vec<String> = crate::brain_plan::live_stacks(job).into_iter().map(|(name, _)| name).collect();
    owned.extend(worn.iter().cloned());
    (owned, worn)
}

/// Ladder pieces the sales pass must never list: the best piece per slot
/// the bot may wield (and higher tiers), plus the next step's object.
pub fn ladder_keep(job: &Job, life: &Life, live: &Value) -> Vec<String> {
    if life.profile.ironman || !ladder_goal(life) {
        return Vec::new();
    }
    let levels_owned = crate::brain_market::live_levels(live);
    let levels: Vec<(&str, i64)> = levels_owned.iter().map(|(name, level)| (name.as_str(), i64::from(*level))).collect();
    let (owned, _) = holdings(job);
    let mut keep = gear_ladder::kept_pieces(&levels, &owned);
    keep.extend(gear_ladder::next_gear_step(&levels, &owned).map(|step| step.item));
    keep
}


/// A buy still resting on the book: open, or partly matched with a
/// remainder (review 2026-11-13: `Matched` rows escaped the cancels).
pub(crate) fn cancellable(row: &lifescape_store::Order) -> bool {
    row.side == lifescape_store::OrderSide::Buy
        && matches!(row.status, lifescape_store::OrderStatus::Open | lifescape_store::OrderStatus::Matched)
        && row.remaining > 0
}

/// Coins the resting buys commit (price × remainder), except those on
/// `except` (a bid never counts against its own ceiling).
pub(crate) fn committed_from(rows: &[lifescape_store::Order], except: Option<i64>) -> i64 {
    rows.iter()
        .filter(|row| cancellable(row) && Some(row.item_id) != except)
        .map(|row| row.price.max(0).saturating_mul(row.remaining))
        .sum()
}

/// Coins this account's resting buys commit, except on `except` (0 while
/// the market service is down: nothing rests then).
pub(crate) fn committed_coins(account: &str, except: Option<i64>) -> i64 {
    let Some(service) = crate::market_service::shared() else { return 0 };
    let rows = service.lock_store().account_orders(account).unwrap_or_default();
    committed_from(&rows, except)
}

/// The wallet a new ceiling may share (review 2026-11-13, finding 6):
/// coins held minus what this bot's other resting buys already commit, so
/// gear (50 %), tools (25 % each) and the graph's buys never promise more
/// than the wallet holds.
pub(crate) fn free_coins(job: &Job, except: Option<i64>) -> i64 {
    (crate::brain_plan::holdings_count(job, "coins") - committed_coins(&job.account, except)).max(0)
}

fn cancel_order(service: &crate::market_service::MarketService, account: &str, order: i64) -> bool {
    let reply = service.handle(&lifescape_json::object(vec![
        ("op", lifescape_json::str_value("cancel")),
        ("account", lifescape_json::str_value(account)),
        ("order", Value::Int(order)),
    ]));
    reply.get("ok").as_bool() == Some(true)
}

/// Cancels the account's resting buys on wearables other than `step`: an
/// older ladder step, or a best-gear wish of the old wishlist (black
/// platebody, amulet of power: live 2026-11-13). Spared: what the
/// graph's own obtains want, and tools (the wishlist keeps those). With
/// `ladder_only`, only ladder pieces go (a goal that left melee: its
/// ranged/magic wishes stay, review 2026-11-13).
fn cancel_stale_bids(job: &Job, life: &Life, step: Option<&str>, ladder_only: bool) {
    let Some(service) = crate::market_service::shared() else { return };
    let mut spared = crate::brain_wishlist::wanted_names(life);
    if ladder_only {
        spared.extend(crate::brain_wishlist::wishlist(job, life).into_iter().map(|wish| wish.item));
    }
    let rows = service.lock_store().account_orders(&job.account).unwrap_or_default();
    for row in rows.iter().filter(|row| cancellable(row)) {
        let Some(name) = lifescape_gamedata::obj_by_id(row.item_id as usize).and_then(|obj| obj.name).map(str::to_string) else { continue };
        let ladder_piece = LADDER.iter().any(|slot| gear_ladder::piece_rank(slot, &name).is_some());
        let tool = name.ends_with("_pickaxe") || (name.ends_with("_axe") && !name.contains("battleaxe"));
        let gear = ladder_piece || (!ladder_only && lifescape_gamedata::equipment::by_name(&name).is_some() && !tool);
        if gear && Some(name.as_str()) != step && !spared.contains(&name) {
            let closed = cancel_order(&service, &job.account, row.order_id);
            let why = if ladder_only { "the goal left the melee ladder" } else { "not the ladder step" };
            crate::logln!("brain {}: gear bid {} on {name} cancelled ({closed}): {why}", job.account, row.order_id);
        }
    }
}

/// Withdraw every dead ladder step (review 2026-11-13, finding 2): run on
/// EVERY launch pass, before `graph_state`, whatever the goal's style, so
/// an abandoned step never leaves the goal Stuck (closed stuck, its skill
/// banned for a gear cause). Returns true when the life changed.
pub(crate) fn withdraw_dead(job: &Job, life: &mut Life) -> bool {
    let causes: Vec<String> = life
        .nodes
        .iter()
        .filter(|node| node.status == lifescape_planner::NodeStatus::Abandoned && !node.last_error.is_empty())
        .map(|node| format!("{}={}", node.id, node.last_error))
        .collect();
    let removed = lifescape_planner::withdraw_dead_steps(life);
    if removed.is_empty() {
        return false;
    }
    let mine: Vec<&String> = causes.iter().filter(|cause| removed.iter().any(|node_id| cause.starts_with(&format!("{node_id}=")))).collect();
    crate::logln!("brain {}: gear step {} died ({}), withdrawn", job.account, removed.join(", "), mine.iter().map(|cause| cause.as_str()).collect::<Vec<_>>().join(" | "));
    for node_id in &removed {
        push_history(life, node_id, "gear_withdrawn");
    }
    true
}

/// One ladder pass. Returns true when the life changed (save it).
pub(crate) fn gear_pass(job: &Job, life: &mut Life, live: &Value) -> bool {
    let changed = withdraw_dead(job, life);
    if life.profile.ironman {
        return changed;
    }
    let now = wall_now();
    {
        let mut table = last_pass().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if table.get(&job.account).is_some_and(|last| now - last < GEAR_EVERY_SECS) {
            return changed;
        }
        table.insert(job.account.clone(), now);
    }
    if !ladder_goal(life) {
        // A goal that left melee (ranged, magic) drops its ladder bids:
        // their coins go back to the wallet its wishes share.
        cancel_stale_bids(job, life, None, true);
        return changed;
    }
    if gear_ladder::step_open(life) {
        return changed;
    }
    let mut changed = changed;
    let levels_owned = crate::brain_market::live_levels(live);
    if levels_owned.is_empty() {
        return changed;
    }
    let levels: Vec<(&str, i64)> = levels_owned.iter().map(|(name, level)| (name.as_str(), i64::from(*level))).collect();
    let (owned, worn) = holdings(job);
    let step = gear_ladder::next_gear_step(&levels, &owned);
    let unworn = gear_ladder::unworn_upgrade(&levels, &owned, &worn);
    let step_id = step.as_ref().and_then(|step| obj_id(&step.item));
    let coins = free_coins(job, step_id);
    let service = crate::market_service::shared();
    let (best_ask, reference) = match (&service, step_id) {
        (Some(service), Some(id)) => {
            let ask = service.lock_engine().books.get(&id).and_then(|book| book.best().1);
            (ask, service.reference(id).price)
        }
        _ => (None, 1),
    };
    let decision = gear_ladder::gear_move(step.as_ref(), unworn.as_deref(), coins, best_ask, reference);
    cancel_stale_bids(job, life, step.as_ref().map(|step| step.item.as_str()), false);
    match decision {
        GearMove::Wait => {}
        GearMove::Equip { item } => {
            let key = (job.account.clone(), item.clone());
            let mut table = last_equip().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if table.get(&key).is_none_or(|last| now - last >= EQUIP_RETRY_SECS) {
                table.insert(key, now);
                let ids = lifescape_planner::insert_gear_step(life, None, "melee");
                crate::logln!("brain {}: gear {item} owned, not worn: equip step {}", job.account, ids.join(", "));
                for id in &ids {
                    push_history(life, id, "gear_step");
                }
                lifescape_planner::normalize(life);
                changed = true;
            }
        }
        GearMove::Obtain { item } => {
            let ids = lifescape_planner::insert_gear_step(life, Some(&item), "melee");
            crate::logln!(
                "brain {}: gear step {item} (ask {best_ask:?}, wallet {coins}, cap {}): {}",
                job.account,
                gear_ladder::wallet_ceiling(coins, gear_ladder::GEAR_SHARE_PCT),
                ids.join(", ")
            );
            for id in &ids {
                push_history(life, id, "gear_step");
            }
            lifescape_planner::normalize(life);
            changed = true;
        }
        GearMove::StandingBid { item, ceiling, .. } => {
            let (Some(service), Some(id)) = (service, obj_id(&item)) else { return changed };
            match crate::market_reprice::keep_bid(&service, &job.account, id, 1, ceiling) {
                Ok(crate::market_reprice::StandingBid::Placed { price, .. }) => {
                    crate::logln!("brain {}: gear {item} standing bid @ {price} (ceiling {ceiling}, wallet {coins}), waiting for a producer", job.account);
                }
                Ok(crate::market_reprice::StandingBid::Kept { .. }) => {}
                Err(error) => crate::logln!("brain {}: gear {item} standing bid refused: {error}", job.account),
            }
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::{cancellable, committed_from};
    use lifescape_store::{Order, OrderSide, OrderStatus};

    fn order(item: i64, side: OrderSide, price: i64, remaining: i64, status: OrderStatus) -> Order {
        Order {
            order_id: item,
            account: "b0".to_string(),
            side,
            item_id: item,
            quantity: remaining.max(1),
            remaining,
            price,
            status,
            dormant: false,
            created_clock: 0,
            updated_clock: 0,
        }
    }

    /// Review 2026-11-13 (finding 6): standing bids (gear 50 %, two tools
    /// 25 % each) and the graph's buys all took shares of the WHOLE wallet.
    /// Each ceiling now reads the wallet minus the coins this bot's other
    /// resting buys commit; a partly matched buy still rests (cancellable).
    #[test]
    fn resting_buys_commit_their_coins() {
        let rows = vec![
            order(1, OrderSide::Buy, 10, 3, OrderStatus::Open),
            order(2, OrderSide::Buy, 5, 4, OrderStatus::Matched),
            order(3, OrderSide::Buy, 7, 9, OrderStatus::Settled),
            order(4, OrderSide::Sell, 100, 9, OrderStatus::Open),
            order(5, OrderSide::Buy, 100, 9, OrderStatus::Cancelled),
            order(6, OrderSide::Buy, 100, 0, OrderStatus::Matched),
        ];
        assert_eq!(committed_from(&rows, None), 50);
        assert_eq!(committed_from(&rows, Some(1)), 20, "a bid never counts against its own ceiling");
        let resting: Vec<i64> = rows.iter().filter(|row| cancellable(row)).map(|row| row.item_id).collect();
        assert_eq!(resting, vec![1, 2], "open and partly matched buys");
    }
}
