//! Shared gather/combat helpers: ban clocks, message classes, tool ranks,
//! toll-fence side tests. State reads only (no walk, no send, no pump);
//! network and game-loop policy stay in the calling scripts.
//!
//! Tool recovery (`reattach_tool`) is the one exception: a pack-only op
//! (no walk, no bank modal) shared by the axe and pickaxe loops. Bank
//! restores stay caller-side on top of `best_banked_by_rank`.
//!
//! `skill_common` is frozen (file-size rule), so this is the second shared
//! module, not an addition to it.

#![forbid(unsafe_code)]

use std::collections::{HashMap, HashSet};

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::skill_common::{
    death_message_seen, hitpoints, obj_id, obj_price, pack_ids, pack_slot, pack_total, FLEE_BAN_TICKS, MAX_FLEES_PER_LOAD,
    WorkOutcome,
};
use crate::ScriptError;

/// Per-tile ban clocks shared by the gather loops and the melee hunt:
/// `ban` records, `is_banned` reads against the live tick, `retain` drops
/// the expired, `release_oldest_if_starved` retries the oldest ban when a
/// tier is blacklisted but not depleted (transient storm, never a wall).
#[derive(Debug, Default)]
pub struct BanSet {
    bans: HashMap<(i32, i32), u64>,
}

impl BanSet {
    pub fn new() -> Self {
        Self { bans: HashMap::new() }
    }

    /// Ban until an absolute tick (door proved uncrossable: `u64::MAX`).
    pub fn ban_until(&mut self, pos: (i32, i32), until: u64) {
        self.bans.insert(pos, until);
    }

    /// Ban `duration` ticks from `now` (saturating, so a huge ban sticks).
    pub fn ban_for(&mut self, pos: (i32, i32), now: u64, duration: u64) {
        self.bans.insert(pos, now.saturating_add(duration));
    }

    /// True while the ban clock still runs (expired entries read as free;
    /// `retain` reaps them on the next loop pass).
    pub fn is_banned(&self, pos: (i32, i32), now: u64) -> bool {
        self.bans.get(&pos).is_some_and(|until| *until > now)
    }

    /// Raw membership, ignoring the clock (planner-proven dead tiles).
    pub fn contains(&self, pos: (i32, i32)) -> bool {
        self.bans.contains_key(&pos)
    }

    /// Remaining clock for debug lines, if any.
    pub fn until(&self, pos: (i32, i32)) -> Option<u64> {
        self.bans.get(&pos).copied()
    }

    /// Drop every expired clock.
    pub fn retain(&mut self, now: u64) {
        self.bans.retain(|_, until| *until > now);
    }

    pub fn remove(&mut self, pos: (i32, i32)) {
        self.bans.remove(&pos);
    }

    pub fn len(&self) -> usize {
        self.bans.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bans.is_empty()
    }

    /// Oldest ban to retry when a tier starves: `Some` only when every
    /// in-range spot seen is banned (100% — none stumped, gated or hunted).
    /// Anything else means the tier is genuinely unavailable, not
    /// blacklisted. The entry leaves the map; a still-dead spot re-bans on
    /// the next attempt, one release per repick so no hot loop.
    pub fn release_oldest_if_starved(
        &mut self,
        total_seen: usize,
        banned: &[((i32, i32), u64)],
    ) -> Option<(i32, i32)> {
        let oldest = starved_oldest(total_seen, banned)?;
        self.bans.remove(&oldest);
        Some(oldest)
    }
}

/// Oldest banned spot to retry when a tier starves: `Some` only on a full
/// ban sweep (every spot seen banned). Pure, so the spot pickers and the
/// tests share it.
pub fn starved_oldest(total_seen: usize, banned: &[((i32, i32), u64)]) -> Option<(i32, i32)> {
    if total_seen == 0 || banned.len() != total_seen {
        return None;
    }
    banned.iter().min_by_key(|(_, until)| *until).map(|(pos, _)| *pos)
}

/// Toll-fence side test: the fence is a vertical line, spots flip sides
/// with it. Same flank hunts together, never across.
pub fn same_side(line: i32, first: i32, second: i32) -> bool {
    (first >= line) == (second >= line)
}

/// True when the spot sits across the toll fence from the bot.
pub fn across_toll(bot_x: i32, spot_x: i32, line: Option<i32>) -> bool {
    line.is_some_and(|fence| !same_side(fence, spot_x, bot_x))
}

/// Server refusal lines shared by the three gather clicks: direct refusals
/// ("can't reach that"), level gates, missing tools, missing bait, the
/// full-pack lines ("too full to hold any more", "can't carry any more"),
/// and rock depletion ("no ore/minerals currently available" reads Gone:
/// the pick repicks instead of waiting on an empty rock). `None`
/// when the line is chatter or progress (gain and combat engagement stay
/// caller-side; combat keeps its own under-attack parser).
pub fn classify_message(line: &str) -> Option<WorkOutcome> {
    let lowered = line.to_lowercase();
    if lowered.contains("can't reach that") {
        return Some(WorkOutcome::UnreachableSpot);
    }
    if lowered.contains("too full to hold any more") || lowered.contains("can't carry any more") {
        return Some(WorkOutcome::Full);
    }
    if lowered.contains("no ore currently available") || lowered.contains("no minerals currently available") {
        return Some(WorkOutcome::Gone);
    }
    if lowered.contains("level of") || (lowered.contains("need at least") && lowered.contains("fishing")) {
        return Some(WorkOutcome::LevelLocked);
    }
    if lowered.contains("do not have an axe") || lowered.contains("do not have a pickaxe") {
        return Some(WorkOutcome::NoTool);
    }
    if lowered.contains("need a ")
        && (lowered.contains("to catch") || lowered.contains("to bait") || lowered.contains("to lure"))
    {
        return Some(WorkOutcome::NoTool);
    }
    if lowered.contains("don't have any") && lowered.contains("left") {
        return Some(WorkOutcome::NoBait);
    }
    None
}

/// Server start-of-work acks per skill: the action loop runs on the
/// server's side. Own anim can never be the detector: the server only
/// masks OTHER players (self is excluded from its own build area, so
/// no self entry ever carries ANIM), and the client already skips the
/// player 0x2 block. Swing-ack + still-live resource is the "player is
/// working" signal instead: quiet then means a dry streak, not a dead
/// spot — keep waiting, the load-level STALL_TICKS bound stays the
/// backstop. Same helper for woodcut/mining/fishing/smithing (no
/// smithing caller yet: its ack plugs in when the script exists).
pub const WOODCUT_ACKS: [&str; 1] = ["swing your axe"];
pub const MINING_ACKS: [&str; 1] = ["swing your pick"];
pub const FISHING_ACKS: [&str; 2] = ["cast out your line", "attempt to catch"];

/// True when a work-start ack arrived since `seen` (the message count
/// right after the click). Case-insensitive substring, like the
/// refusal vocabulary in `classify_message`.
pub fn work_acked(bot: &Bot, seen: usize, acks: &[&str]) -> bool {
    bot.world.messages.get(seen..).unwrap_or(&[]).iter().any(|line| {
        let lowered = line.to_lowercase();
        acks.iter().any(|ack| lowered.contains(ack))
    })
}

/// Best-first tool order (`AXE_ORDER`, `PICK_ORDER`, fishing tools by max
/// tier level): rank decides, shop stock gates, level gates pass through
/// `usable`. State reads only, never a walk or a buy.
pub struct ToolSpec {
    pub order: &'static [&'static str],
}

impl ToolSpec {
    /// Rank in `order` (0 = best). Unknown tools never rank.
    pub fn rank(&self, name: &str) -> Option<usize> {
        self.order.iter().position(|tool| *tool == name)
    }

    /// Best held tool, best-first.
    pub fn best_held(&self, bot: &Bot) -> Option<String> {
        let held = pack_ids(bot);
        self.order
            .iter()
            .find(|name| obj_id(bot, name).is_ok_and(|id| held.contains(&id)))
            .map(|name| name.to_string())
    }

    /// Every order id the tables know (for dirty-pack and sale filters:
    /// a banked tool type still counts as a tool, never as junk).
    pub fn known_ids(&self, bot: &Bot) -> HashSet<u16> {
        self.order.iter().filter_map(|name| obj_id(bot, name).ok()).collect()
    }

    /// Pack ids that survive the bank trip: the single best tool only (the
    /// server works with the best carried tool, so a second is dead weight
    /// that death would drop anyway).
    pub fn keep_ids(&self, bot: &Bot) -> Vec<u16> {
        self.best_held(bot).and_then(|name| obj_id(bot, &name).ok()).into_iter().collect()
    }

    /// Affordable upgrade cands, best-first, with the cheapest price:
    /// better-ranked than held, level-usable by the caller, actually in
    /// shop stock. One pass, never a loop.
    pub fn cheapest_upgrade(&self, bot: &Bot, usable: impl Fn(&str) -> bool) -> (Vec<String>, i64) {
        let current = self.best_held(bot).and_then(|name| self.rank(&name)).unwrap_or(self.order.len());
        let cands: Vec<String> = self.order[..current]
            .iter()
            .filter(|name| usable(name) && crate::skill_common::shop_stock_keeper(bot, name).is_some())
            .map(|name| name.to_string())
            .collect();
        let cheapest = cands.iter().map(|name| obj_price(name)).min().unwrap_or(0);
        (cands, cheapest)
    }
}

/// A gain resets the no-progress stall budget to now.
pub fn note_gain(idle_since: &mut u64, now: u64) {
    *idle_since = now;
}

/// True when no progress for longer than the bound (strictly over, so a
/// gain exactly on the bound still counts).
pub fn stall_expired(now: u64, idle_since: u64, bound: u64) -> bool {
    now.saturating_sub(idle_since) > bound
}

/// Hurt mid-load: ban the current spot for the flee window, count the
/// flee, true when the load must abort (past the flee cap).
pub fn on_hurt(bans: &mut BanSet, spot: Option<(i32, i32)>, flees: &mut u32, now: u64) -> bool {
    if let Some(pos) = spot {
        bans.ban_for(pos, now, FLEE_BAN_TICKS);
    }
    *flees += 1;
    *flees > MAX_FLEES_PER_LOAD
}

/// True when death happened since `seen`: hp flatlined or the server death
/// line arrived. Returns the resynced index (everything observed so far)
/// so the next call neither replays nor skips a line.
pub fn death_since(bot: &Bot, seen: usize) -> (bool, usize) {
    let died = hitpoints(bot) == 0 || death_message_seen(bot, seen);
    (died, bot.world.messages.len())
}

/// True when the fight started since the click: xp flowing, hp taken,
/// or the target animating (its attack anim streams — ours never does,
/// and no positive "you're fighting" message exists server-side, all
/// combat lines are refusals). Pure so the loop stays thin; the caller
/// owns the snapshots and the re-click throttle. Ready for smithing-
/// style loops too: any started-loop-vs-dead-click verdict.
pub fn fight_engaged(
    xp_now: i64,
    xp_at_click: i64,
    hp_now: u8,
    hp_before: u8,
    anim_now: Option<u16>,
    anim_at_click: Option<u16>,
) -> bool {
    xp_now != xp_at_click || hp_now < hp_before || anim_now != anim_at_click
}

/// Reattach a carried handle + head instead of rebuying (axe and pickaxe
/// loops share it). Pack ops only, no bank modal: resolve the handle, try
/// each carried head with `head_suffix`, verify through `spec` (a plain
/// ref, never a closure over `&Bot`, so no double-borrow). Ok(false) when
/// the handle or every head is missing: nothing sent.
pub fn reattach_tool(bot: &mut Bot, handle_name: &str, head_suffix: &str, spec: &ToolSpec) -> Result<bool, ScriptError> {
    let Ok(handle) = obj_id(bot, handle_name) else {
        return Ok(false);
    };
    if pack_total(bot, handle) == 0 {
        return Ok(false);
    }
    let heads: Vec<u16> = knowledge::all_objs()
        .into_iter()
        .filter(|entry| entry.name.as_deref().unwrap_or("").ends_with(head_suffix))
        .filter_map(|entry| entry.name.clone())
        .filter_map(|name| obj_id(bot, &name).ok())
        .collect();
    for head in heads {
        if pack_total(bot, head) == 0 {
            continue;
        }
        if use_handle_on_head(bot, handle, head, spec)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn use_handle_on_head(bot: &mut Bot, handle: u16, head: u16, spec: &ToolSpec) -> Result<bool, ScriptError> {
    let Some(pack_com) = bot.backpack().0 else {
        return Ok(false);
    };
    let (Ok(handle_slot), Ok(head_slot)) = (pack_slot(bot, handle), pack_slot(bot, head)) else {
        return Ok(false);
    };
    bot.op_held_u(handle, handle_slot, pack_com, head, head_slot, pack_com)?;
    let target = bot.world.ticks + 50;
    while bot.world.ticks < target {
        bot.pump(5)?;
        if spec.best_held(bot).is_some() {
            bot.log("ok tool reattached");
            return Ok(true);
        }
    }
    Ok(false)
}

/// Best-ranked banked tool slot (the pack holds one tool: an upgrade banks
/// the old one, a restore withdraws the best, never a second). Pure: rank
/// decides through a `fn` pointer (no closure, no `&Bot` capture); names
/// come from the dump (`obj_by_id` debug names, not display labels).
pub fn best_banked_by_rank(
    slots: &HashMap<u8, (u16, u32)>,
    known: &HashSet<u16>,
    rank: fn(&str) -> Option<usize>,
) -> Option<(u8, u16)> {
    slots
        .iter()
        .filter(|(_, (held, _))| known.contains(held))
        .min_by_key(|(_, (held, _))| {
            knowledge::obj_by_id(usize::from(*held)).and_then(|entry| entry.name).and_then(|name| rank(&name)).unwrap_or(usize::MAX)
        })
        .map(|(slot, (held, _))| (*slot, *held))
}

#[cfg(test)]
mod tests {
    use super::{across_toll, best_banked_by_rank, classify_message, death_since, fight_engaged, note_gain, on_hurt, reattach_tool, same_side, stall_expired, starved_oldest, work_acked, BanSet, ToolSpec, FISHING_ACKS, MINING_ACKS, WOODCUT_ACKS};
    use crate::skill_common::WorkOutcome;
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    /// Ban clocks: a short ban frees at expiry, a door ban (`u64::MAX`)
    /// survives every retain, and the reap drops only the expired.
    #[test]
    fn ban_expiry_and_door_ban() {
        let mut bans = BanSet::new();
        assert!(bans.is_empty());
        bans.ban_for((1, 2), 100, 200);
        assert!(bans.is_banned((1, 2), 299), "clock still runs");
        assert!(!bans.is_banned((1, 2), 300), "expiry reads as free");
        assert!(bans.contains((1, 2)), "raw membership ignores the clock");
        bans.ban_until((3, 4), u64::MAX);
        bans.retain(1_000_000);
        assert!(!bans.contains((1, 2)), "reap drops the expired");
        assert!(bans.is_banned((3, 4), 1_000_000), "door ban never expires");
        assert_eq!(bans.len(), 1);
    }

    /// Starvation release: a full ban sweep retries the oldest (and it
    /// leaves the map); one stump/gate/hunter note breaks unanimity.
    #[test]
    fn starvation_releases_oldest_only_on_full_sweep() {
        let mut bans = BanSet::new();
        bans.ban_until((1, 1), 50);
        bans.ban_until((2, 2), 30);
        assert_eq!(starved_oldest(0, &[]), None, "nothing seen, nothing released");
        assert_eq!(starved_oldest(2, &[((1, 1), 50)]), None, "mixed stump+ban never releases");
        let got = bans.release_oldest_if_starved(2, &[((1, 1), 50), ((2, 2), 30)]);
        assert_eq!(got, Some((2, 2)), "oldest first, got: {got:?}");
        assert!(!bans.contains((2, 2)), "released entry leaves the map");
        assert_eq!(bans.release_oldest_if_starved(2, &[((1, 1), 50)]), None, "no unanimity, no release");
    }

    /// Message classes: direct refusals, level gates, missing tools/bait,
    /// full packs and rock depletion map; loot chatter stays `None`.
    #[test]
    fn classify_covers_the_gather_refusals() {
        assert_eq!(classify_message("I can't reach that!"), Some(WorkOutcome::UnreachableSpot));
        assert_eq!(classify_message("You need a Woodcutting level of 60."), Some(WorkOutcome::LevelLocked));
        assert_eq!(classify_message("You need a Mining level of 15."), Some(WorkOutcome::LevelLocked));
        assert_eq!(classify_message("You need at least level 5 Fishing."), Some(WorkOutcome::LevelLocked));
        assert_eq!(classify_message("You do not have an axe to cut this."), Some(WorkOutcome::NoTool));
        assert_eq!(classify_message("You do not have a pickaxe to mine this."), Some(WorkOutcome::NoTool));
        assert_eq!(classify_message("You need a rod to catch these."), Some(WorkOutcome::NoTool));
        assert_eq!(classify_message("You don't have any bait left."), Some(WorkOutcome::NoBait));
        assert_eq!(classify_message("Your inventory is too full to hold any more logs."), Some(WorkOutcome::Full));
        assert_eq!(classify_message("Your inventory is too full to hold any more iron ore."), Some(WorkOutcome::Full));
        assert_eq!(classify_message("You can't carry any more fish."), Some(WorkOutcome::Full));
        assert_eq!(classify_message("There is no ore currently available."), Some(WorkOutcome::Gone));
        assert_eq!(classify_message("There are no minerals currently available."), Some(WorkOutcome::Gone));
        assert_eq!(classify_message("You get some oak logs."), None, "progress stays caller-side");
    }

    /// Fence side: the gate line splits flanks; `None` means no filtering.
    #[test]
    fn fence_side_splits_at_the_gate_line() {
        assert!(same_side(3268, 3262, 3255), "west flank together");
        assert!(same_side(3268, 3270, 3300), "east flank together");
        assert!(!same_side(3268, 3262, 3270), "across never targets");
        assert!(across_toll(2900, 3086, Some(2920)), "east spot is across");
        assert!(!across_toll(2900, 2910, Some(2920)), "west spot is same side");
        assert!(!across_toll(2900, 3086, None), "no fence means no filtering");
    }

    /// Tool rank: best held wins, keep holds one tool only, unknown tools
    /// never rank. State reads only, no shop walk needed for the rank.
    #[test]
    fn tool_rank_picks_best_held() {
        const ORDER: [&str; 3] = ["steel_axe", "iron_axe", "bronze_axe"];
        const SPEC: ToolSpec = ToolSpec { order: &ORDER };
        assert_eq!(SPEC.rank("steel_axe"), Some(0));
        assert_eq!(SPEC.rank("no_such_tool"), None, "unknown never ranks");
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        assert_eq!(SPEC.best_held(&bot), None, "empty pack holds nothing");
        let bronze = crate::skill_common::obj_id(&bot, "bronze_axe").expect("bronze_axe in tables");
        let steel = crate::skill_common::obj_id(&bot, "steel_axe").expect("steel_axe in tables");
        let com = (bot.tables.com_id)("inventory:inv").expect("inventory:inv in dumps");
        bot.world.inv.insert(com, [(0u8, (bronze, 1u32)), (1, (steel, 1))].into_iter().collect());
        assert_eq!(SPEC.best_held(&bot), Some("steel_axe".to_string()), "best held wins");
        assert_eq!(SPEC.keep_ids(&bot), vec![steel], "one tool in the pack: the best");
        assert!(!SPEC.keep_ids(&bot).contains(&bronze), "inferior banks, never doubles");
        assert!(SPEC.known_ids(&bot).contains(&bronze), "known covers the banked types too");
    }

    /// Upgrade shortlist: only better-ranked, shop-gated by the caller
    /// predicate (`usable`); the cheapest price is the min of the cands.
    #[test]
    fn upgrade_lists_only_usable_better_tools() {
        const ORDER: [&str; 3] = ["steel_axe", "iron_axe", "bronze_axe"];
        const SPEC: ToolSpec = ToolSpec { order: &ORDER };
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        let bronze = crate::skill_common::obj_id(&bot, "bronze_axe").expect("bronze_axe in tables");
        let com = (bot.tables.com_id)("inventory:inv").expect("inventory:inv in dumps");
        bot.world.inv.insert(com, [(0u8, (bronze, 1u32))].into_iter().collect());
        let (none, cheapest) = SPEC.cheapest_upgrade(&bot, |_| false);
        assert!(none.is_empty() && cheapest == 0, "nothing usable, nothing listed");
        let (cands, _) = SPEC.cheapest_upgrade(&bot, |_| true);
        assert!(!cands.contains(&"bronze_axe".to_string()), "held rank never lists itself");
        assert!(cands.iter().all(|name| SPEC.rank(name) < SPEC.rank("bronze_axe")), "only better ranks, got: {cands:?}");
    }

    /// Stall budget: a gain resets to now, expiry is strictly over the
    /// bound (a gain exactly on the bound still counts).
    #[test]
    fn stall_budget_resets_on_gain() {
        let mut idle_since = 100u64;
        note_gain(&mut idle_since, 150);
        assert_eq!(idle_since, 150);
        assert!(!stall_expired(150 + 1000, 150, 1000), "exactly on the bound still counts");
        assert!(stall_expired(150 + 1001, 150, 1000), "one past the bound expires");
    }

    /// Hurt: bans the spot for the flee window, counts, aborts past the
    /// cap (two flees ride, the third aborts the load).
    #[test]
    fn hurt_bans_and_aborts_past_the_cap() {
        use crate::skill_common::{FLEE_BAN_TICKS, MAX_FLEES_PER_LOAD};
        let mut bans = BanSet::new();
        let mut flees = 0u32;
        assert!(!on_hurt(&mut bans, Some((5, 5)), &mut flees, 100), "first flee rides");
        assert!(bans.is_banned((5, 5), 100 + FLEE_BAN_TICKS - 1), "spot banned for the flee window");
        assert_eq!(flees, 1);
        let mut abort = false;
        for _ in 0..MAX_FLEES_PER_LOAD {
            abort = on_hurt(&mut bans, None, &mut flees, 100);
        }
        assert!(abort, "past {MAX_FLEES_PER_LOAD} flees the load aborts");
    }

    /// Death since an index: message or flatlined hp, resync never replays.
    #[test]
    fn death_since_detects_message_and_flat_hp() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.stats.insert(3, (10, 0));
        let seen = bot.world.messages.len();
        bot.world.messages.push("You get some oak logs.".to_string());
        let (alive, fresh) = death_since(&bot, seen);
        assert!(!alive, "loot chatter is not death");
        bot.world.messages.push("Oh dear you are dead!".to_string());
        let (died, fresh2) = death_since(&bot, fresh);
        assert!(died, "death line detected");
        let (again, _) = death_since(&bot, fresh2);
        assert!(!again, "resync never replays");
        bot.world.stats.insert(3, (0, 0));
        let (flat, _) = death_since(&bot, fresh2);
        assert!(flat, "hp 0 detects death alone");
    }

    /// Engagement evidence: each channel alone proves the fight (xp
    /// only flows on our hits, hp only drops on damage taken, target
    /// anim only changes when it acts); all quiet means the click
    /// never landed.
    #[test]
    fn fight_engaged_reads_any_channel() {
        assert!(!fight_engaged(100, 100, 18, 18, None, None), "silence is not engagement");
        assert!(fight_engaged(120, 100, 18, 18, None, None), "xp flow is engagement");
        assert!(fight_engaged(100, 100, 15, 18, None, None), "damage taken is engagement");
        assert!(fight_engaged(100, 100, 18, 18, Some(7), None), "target animating is engagement");
        assert!(!fight_engaged(100, 100, 19, 18, None, None), "regen/heal is not engagement");
    }

    /// Work-start ack: per-skill vocab, case-insensitive, only lines
    /// after the click count (seen-indexed, never replays old swings).
    #[test]
    fn work_acked_detects_swing_since_click() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.messages.push("You swing your axe at the tree.".to_string());
        let seen = bot.world.messages.len();
        bot.world.messages.push("You get some yew logs.".to_string());
        assert!(!work_acked(&bot, seen, &WOODCUT_ACKS), "loot is not a start ack");
        assert!(!work_acked(&bot, seen, &MINING_ACKS), "axe swing is not a pick swing");
        bot.world.messages.push("YOU SWING YOUR PICK AT THE ROCK.".to_string());
        assert!(work_acked(&bot, seen, &MINING_ACKS), "case-insensitive pick ack");
        assert!(!work_acked(&bot, 0, &FISHING_ACKS), "no cast, no ack");
        bot.world.messages.push("You attempt to catch a fish.".to_string());
        assert!(work_acked(&bot, seen, &FISHING_ACKS), "attempt counts as fishing ack");
    }

    /// Reattach without a handle never sends: unknown handle and empty
    /// pack both return Ok(false) before any `op_held_u` (an offline send
    /// would fail, so Ok(false) proves nothing was attempted).
    #[test]
    fn reattach_without_handle_never_sends() {
        const ORDER: [&str; 2] = ["steel_pickaxe", "bronze_pickaxe"];
        const SPEC: ToolSpec = ToolSpec { order: &ORDER };
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        assert!(!reattach_tool(&mut bot, "no_such_handle", "_pickaxehead", &SPEC).expect("unknown handle never sends"));
        assert!(!reattach_tool(&mut bot, "macro_pickaxehandle", "_pickaxehead", &SPEC).expect("empty pack never sends"));
    }

    /// Bank restore withdraws the best-ranked tool, never a second or a
    /// worse one (rank fn decides, dump debug names, unknown never ranks).
    #[test]
    fn banked_best_rank_prefers_steel() {
        fn rank(name: &str) -> Option<usize> {
            ["steel_pickaxe", "bronze_pickaxe"].iter().position(|tool| *tool == name)
        }
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        let bronze = crate::skill_common::obj_id(&bot, "bronze_pickaxe").expect("bronze_pickaxe in tables");
        let steel = crate::skill_common::obj_id(&bot, "steel_pickaxe").expect("steel_pickaxe in tables");
        let known: std::collections::HashSet<u16> = [bronze, steel].into_iter().collect();
        let slots: std::collections::HashMap<u8, (u16, u32)> =
            [(3u8, (bronze, 1u32)), (7, (steel, 1))].into_iter().collect();
        assert_eq!(best_banked_by_rank(&slots, &known, rank), Some((7, steel)), "best banked tool wins");
        assert_eq!(best_banked_by_rank(&std::collections::HashMap::new(), &known, rank), None, "empty bank restores nothing");
    }
}
