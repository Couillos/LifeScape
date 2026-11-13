//! Equipment ladder (user rule 2026-11-13): equipment is a real
//! sub-goal, climbed one metal tier at a time, never the absolute best
//! piece at once (audit 2026-11-13: black/mithril platebody wishes with
//! 7–105 coin ceilings never filled, never became nodes).
//!
//! - Tiers: bronze → iron → steel → mithril → adamant → rune (F2P,
//!   smithable). Black counts as steel when owned and is never a step
//!   (no smithing recipe: a merchant item under D-A).
//! - Slots in order: weapon (scimitar, else longsword), body (platebody,
//!   else chainbody), legs (platelegs, else plateskirt), helm (full helm,
//!   else med helm), shield (kiteshield, else square shield).
//! - The step is the NEXT tier of the first slot the bot's levels open
//!   (server wield gates: attack for weapons, defence for armour; a quest
//!   gate such as the rune platebody's falls to the next family).
//! - Purchase cap: `GEAR_SHARE_PCT` of the wallet (pack + bank), so the
//!   ceiling rises as coins come in. Bought on the market only (never a
//!   shop: every step is producible, D-A).
//!
//! Pure: levels, holdings and market numbers come in as arguments.

use crate::expand::ReadyCtx;
use crate::model::{Life, Node, NodeKind, NodeStatus};

/// Wallet share (percent) an equipment step may bid: half the coins the
/// bot holds. One step at a time, so this is the whole gear budget; the
/// other half stays for inputs, tools and the graph's own purchases.
pub const GEAR_SHARE_PCT: i64 = 50;
/// Wallet share (percent) of one tool wish (pickaxe, axe).
pub const TOOL_SHARE_PCT: i64 = 25;
/// Marker alternative of a ladder `equip_best` node (no script: the
/// equip leg is `equip:<style>`), so the brain tells a ladder step from
/// an `equip_best` goal.
pub const LADDER_MARKER: &str = "gear:ladder";

/// Metal prefixes and their tier rank (black ranks with steel).
const METAL_RANKS: [(&str, u8); 7] = [("bronze", 0), ("iron", 1), ("steel", 2), ("black", 2), ("mithril", 3), ("adamant", 4), ("rune", 5)];
/// Metals a step may name, lowest first.
const STEP_METALS: [&str; 6] = ["bronze", "iron", "steel", "mithril", "adamant", "rune"];

/// One ladder slot: its name and the piece families, preferred first.
#[derive(Debug, Clone, Copy)]
pub struct LadderSlot {
    pub slot: &'static str,
    pub families: &'static [&'static str],
}

/// Slots in purchase order: weapon first, then body, legs, helm, shield.
pub const LADDER: [LadderSlot; 5] = [
    LadderSlot { slot: "weapon", families: &["scimitar", "longsword"] },
    LadderSlot { slot: "body", families: &["platebody", "chainbody"] },
    LadderSlot { slot: "legs", families: &["platelegs", "plateskirt"] },
    LadderSlot { slot: "helm", families: &["full_helm", "med_helm"] },
    LadderSlot { slot: "shield", families: &["kiteshield", "sq_shield"] },
];

/// The next equipment purchase.
#[derive(Debug, Clone, PartialEq)]
pub struct GearStep {
    pub slot: &'static str,
    pub item: String,
    pub rank: u8,
}

/// Tier rank of `name` when it is a piece of `slot` (`mithril_scimitar`
/// in the weapon slot: 3).
pub fn piece_rank(slot: &LadderSlot, name: &str) -> Option<u8> {
    let (metal, family) = name.split_once('_')?;
    if !slot.families.contains(&family) {
        return None;
    }
    METAL_RANKS.iter().find(|(known, _)| *known == metal).map(|(_, rank)| *rank)
}

/// Highest tier the bot owns in `slot` (`owned`: pack, bank and worn
/// debug names), `None` when it owns no piece of it.
pub fn owned_rank(slot: &LadderSlot, owned: &[String]) -> Option<u8> {
    owned.iter().filter_map(|name| piece_rank(slot, name)).max()
}

fn wieldable_now(name: &str, levels: &[(&str, i64)]) -> bool {
    lifescape_gamedata::equipment::by_name(name).is_some_and(|entry| lifescape_gamedata::gear::wieldable(&entry, levels, &[]))
}

/// The piece of `metal` for `slot` the bot may buy and wield now: F2P,
/// tradeable, producible (the market can be supplied), wieldable at
/// `levels`; the first family that qualifies.
fn piece_for(metal: &str, slot: &LadderSlot, levels: &[(&str, i64)]) -> Option<String> {
    slot.families.iter().map(|family| format!("{metal}_{family}")).find(|name| {
        let f2p_tradeable =
            lifescape_gamedata::objs_by_name(name).into_iter().next().is_some_and(|obj| obj.tradeable && obj.members == Some(false));
        f2p_tradeable && lifescape_gamedata::obtain::producible_f2p(name) && wieldable_now(name, levels)
    })
}

/// The next step: in slot order, the lowest tier above what the bot
/// owns there (bronze when it owns nothing), when the bot's levels let it
/// wield that piece. A gated slot gives way to the next slot; `None` once
/// every slot holds the best tier its levels open.
pub fn next_gear_step(levels: &[(&str, i64)], owned: &[String]) -> Option<GearStep> {
    for slot in &LADDER {
        let owned = owned_rank(slot, owned);
        let Some(metal) = STEP_METALS.iter().find(|metal| {
            let rank = METAL_RANKS.iter().find(|(known, _)| known == *metal).map(|(_, rank)| *rank).unwrap_or(0);
            owned.is_none_or(|have| rank > have)
        }) else {
            continue;
        };
        if let Some(item) = piece_for(metal, slot, levels) {
            let rank = METAL_RANKS.iter().find(|(known, _)| known == metal).map(|(_, rank)| *rank).unwrap_or(0);
            return Some(GearStep { slot: slot.slot, item, rank });
        }
    }
    None
}

/// Ladder pieces the bot must keep (never listed for sale): per slot,
/// the best piece it may wield now and anything of a higher tier (worn
/// later). Lower tiers are surplus once a better one is owned.
pub fn kept_pieces(levels: &[(&str, i64)], owned: &[String]) -> Vec<String> {
    let mut keep = Vec::new();
    for slot in &LADDER {
        let best_wieldable =
            owned.iter().filter(|name| wieldable_now(name, levels)).filter_map(|name| piece_rank(slot, name)).max();
        for name in owned {
            if let Some(rank) = piece_rank(slot, name) {
                if best_wieldable.is_none_or(|best| rank >= best) {
                    keep.push(name.clone());
                }
            }
        }
    }
    keep.sort();
    keep.dedup();
    keep
}

fn melee_score(name: &str) -> Option<(i64, i64)> {
    lifescape_gamedata::equipment::by_name(name).map(|entry| (entry.wearpos, lifescape_gamedata::gear::score(&entry, lifescape_gamedata::gear::Style::Melee)))
}

/// An owned ladder piece the bot may wield and that beats what it wears
/// in that wear slot (melee score): bought or delivered, not worn yet.
pub fn unworn_upgrade(levels: &[(&str, i64)], owned: &[String], worn: &[String]) -> Option<String> {
    for slot in &LADDER {
        let best = owned
            .iter()
            .filter(|name| !worn.contains(name) && wieldable_now(name, levels))
            .filter_map(|name| piece_rank(slot, name).map(|rank| (rank, name)))
            .max_by_key(|(rank, _)| *rank)
            .map(|(_, name)| name.clone());
        let Some(best) = best else { continue };
        let Some((wearpos, score)) = melee_score(&best) else { continue };
        let worn_score = worn.iter().filter_map(|name| melee_score(name)).filter(|(pos, _)| *pos == wearpos).map(|(_, score)| score).max();
        if worn_score.is_none_or(|current| score > current) {
            return Some(best);
        }
    }
    None
}

/// Per-unit cap of a purchase limited to `share_pct` of `coins`.
pub fn wallet_ceiling(coins: i64, share_pct: i64) -> i64 {
    coins.max(0).saturating_mul(share_pct.clamp(1, 100)) / 100
}

/// Wallet whose `share_pct` covers `cost` (rounded up): the `have_coins`
/// target of a capped purchase short of coins.
pub fn coins_for(cost: i64, share_pct: i64) -> i64 {
    let share = share_pct.clamp(1, 100);
    (cost.max(1).saturating_mul(100) + share - 1) / share
}

/// What the brain does about equipment this pass.
#[derive(Debug, Clone, PartialEq)]
pub enum GearMove {
    /// Nothing to buy (every slot at its best open tier).
    Wait,
    /// An owned piece is not worn: file an `equip_best` step alone.
    Equip { item: String },
    /// A seller exists, or the wallet is too thin to bid: file the
    /// obtain + equip step (a covered ask is bought at once, otherwise
    /// the obtain blocks on coins and earns them: `have_coins`).
    Obtain { item: String },
    /// No seller yet: a standing bid (D-B) waits for a producer, at the
    /// reference, walking up to the wallet cap; the bot keeps working.
    StandingBid { item: String, price: i64, ceiling: i64 },
}

/// The pass decision for one bot. `step` is `next_gear_step`, `unworn`
/// is `unworn_upgrade`, `coins` the wallet, `best_ask`/`reference` the
/// market numbers of the step's object.
pub fn gear_move(step: Option<&GearStep>, unworn: Option<&str>, coins: i64, best_ask: Option<i64>, reference: i64) -> GearMove {
    if let Some(item) = unworn {
        return GearMove::Equip { item: item.to_string() };
    }
    let Some(step) = step else { return GearMove::Wait };
    let ceiling = wallet_ceiling(coins, GEAR_SHARE_PCT);
    let reference = reference.max(1);
    match best_ask {
        Some(_) => GearMove::Obtain { item: step.item.clone() },
        None if ceiling >= reference => GearMove::StandingBid { item: step.item.clone(), price: reference, ceiling },
        None => GearMove::Obtain { item: step.item.clone() },
    }
}

/// True for an `equip_best` node filed by the ladder (not a goal's).
pub fn is_ladder_equip(node: &Node) -> bool {
    matches!(node.kind, NodeKind::EquipBest { .. }) && node.alternatives.iter().any(|alt| alt.method == LADDER_MARKER)
}

fn open(node: &Node) -> bool {
    !matches!(node.status, NodeStatus::Done | NodeStatus::Abandoned)
}

/// Ids of a ladder step's chain: the `equip_best` node at `equip` and
/// every transitive dependency (obtain, `have_coins`).
pub fn step_chain(life: &Life, equip: usize) -> Vec<String> {
    let mut chain = vec![life.nodes[equip].id.clone()];
    let mut cursor = 0;
    while cursor < chain.len() {
        let id = chain[cursor].clone();
        if let Some(node) = life.nodes.iter().find(|node| node.id == id) {
            for dep in &node.deps {
                if !chain.contains(dep) {
                    chain.push(dep.clone());
                }
            }
        }
        cursor += 1;
    }
    chain
}

/// Open nodes of every open ladder step (the gear sub-goal).
pub fn gear_subtree(life: &Life) -> Vec<String> {
    let mut ids = Vec::new();
    for (index, node) in life.nodes.iter().enumerate() {
        if is_ladder_equip(node) && open(node) {
            ids.extend(step_chain(life, index));
        }
    }
    ids.sort();
    ids.dedup();
    ids
}

/// True while a ladder step is open: one step at a time.
pub fn step_open(life: &Life) -> bool {
    life.nodes.iter().any(|node| is_ladder_equip(node) && open(node))
}

/// True for the obtain node of an open ladder step.
pub fn is_gear_obtain(life: &Life, index: usize) -> bool {
    matches!(life.nodes[index].kind, NodeKind::ObtainItems { .. }) && gear_subtree(life).contains(&life.nodes[index].id)
}

/// A ladder obtain about to resolve with no seller left and a wallet
/// that can bid: the step withdraws (the standing bid waits for a
/// producer, the bot keeps working) instead of resting a market wait
/// that would hold the worker.
pub fn withdraw_due(ctx: &ReadyCtx) -> bool {
    ctx.best_ask.is_none() && wallet_ceiling(ctx.coins, ctx.coin_share_pct.unwrap_or(GEAR_SHARE_PCT)) >= ctx.price.max(1)
}

/// The next node to launch: a ready node of the gear sub-goal first
/// (equipment precedes the goal's next tranche), else the first ready
/// node. The tutorial never starts here (it has its own gate).
pub fn next_ready(life: &Life) -> Option<usize> {
    let launchable = |node: &Node| {
        node.status == NodeStatus::Ready && !matches!(&node.kind, NodeKind::CompleteQuest { quest } if quest == "tutorial")
    };
    let gear = gear_subtree(life);
    life.nodes
        .iter()
        .position(|node| launchable(node) && gear.contains(&node.id))
        .or_else(|| life.nodes.iter().position(launchable))
}

#[cfg(test)]
#[path = "gear_ladder_tests.rs"]
mod tests;
