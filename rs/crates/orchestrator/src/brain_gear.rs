#![forbid(unsafe_code)]
//! Equipment ladder in the brain (user rule 2026-11-13; pure rules in
//! `lifescape_planner::gear_ladder`). Once per `GEAR_EVERY_SECS` for an
//! idle, non-Iron bot of a melee (or non-combat) goal:
//! - a dead ladder step (abandoned) leaves the graph (`withdraw`);
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

fn obj_id(item: &str) -> Option<i64> {
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

/// Cancels the account's open buys on wearables other than `step`: an
/// older ladder step, or a best-gear wish of the old wishlist (black
/// platebody, amulet of power: live 2026-11-13). Spared: what the
/// graph's own obtains want, and tools (the wishlist keeps those).
fn cancel_stale_bids(job: &Job, life: &Life, step: Option<&str>) {
    let Some(service) = crate::market_service::shared() else { return };
    let wanted = crate::brain_wishlist::wanted_names(life);
    let rows = service.lock_store().account_orders(&job.account).unwrap_or_default();
    for row in rows {
        if row.side != lifescape_store::OrderSide::Buy || row.status != lifescape_store::OrderStatus::Open {
            continue;
        }
        let Some(name) = lifescape_gamedata::obj_by_id(row.item_id as usize).and_then(|obj| obj.name).map(str::to_string) else { continue };
        let ladder_piece = LADDER.iter().any(|slot| gear_ladder::piece_rank(slot, &name).is_some());
        let tool = name.ends_with("_pickaxe") || (name.ends_with("_axe") && !name.contains("battleaxe"));
        let gear = ladder_piece || (lifescape_gamedata::equipment::by_name(&name).is_some() && !tool);
        if gear && Some(name.as_str()) != step && !wanted.contains(&name) {
            let closed = service.cancel_open_for(&job.account, row.item_id, lifescape_store::OrderSide::Buy);
            crate::logln!("brain {}: gear bid on {name} cancelled ({closed}): not the ladder step", job.account);
        }
    }
}

/// One ladder pass. Returns true when the life changed (save it).
pub(crate) fn gear_pass(job: &Job, life: &mut Life, live: &Value) -> bool {
    if life.profile.ironman || !ladder_goal(life) {
        return false;
    }
    let now = wall_now();
    {
        let mut table = last_pass().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if table.get(&job.account).is_some_and(|last| now - last < GEAR_EVERY_SECS) {
            return false;
        }
        table.insert(job.account.clone(), now);
    }
    let mut changed = false;
    // A dead step never strands the goal's graph.
    while let Some(dead) = life
        .nodes
        .iter()
        .position(|node| gear_ladder::is_ladder_equip(node) && node.status == lifescape_planner::NodeStatus::Abandoned)
    {
        let cause = life.nodes[dead].last_error.clone();
        let removed = lifescape_planner::withdraw_gear_step(life, dead);
        if removed.is_empty() {
            break;
        }
        crate::logln!("brain {}: gear step {} died ({cause}), withdrawn", job.account, removed.join(", "));
        for id in &removed {
            push_history(life, id, "gear_withdrawn");
        }
        lifescape_planner::normalize(life);
        changed = true;
    }
    if gear_ladder::step_open(life) {
        return changed;
    }
    let levels_owned = crate::brain_market::live_levels(live);
    if levels_owned.is_empty() {
        return changed;
    }
    let levels: Vec<(&str, i64)> = levels_owned.iter().map(|(name, level)| (name.as_str(), i64::from(*level))).collect();
    let (owned, worn) = holdings(job);
    let step = gear_ladder::next_gear_step(&levels, &owned);
    let unworn = gear_ladder::unworn_upgrade(&levels, &owned, &worn);
    let coins = crate::brain_plan::holdings_count(job, "coins");
    let service = crate::market_service::shared();
    let (best_ask, reference) = match (&service, step.as_ref().and_then(|step| obj_id(&step.item))) {
        (Some(service), Some(id)) => {
            let ask = service.lock_engine().books.get(&id).and_then(|book| book.best().1);
            (ask, service.reference(id).price)
        }
        _ => (None, 1),
    };
    let decision = gear_ladder::gear_move(step.as_ref(), unworn.as_deref(), coins, best_ask, reference);
    cancel_stale_bids(job, life, step.as_ref().map(|step| step.item.as_str()));
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
