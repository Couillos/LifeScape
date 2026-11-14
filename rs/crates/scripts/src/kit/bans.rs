//! Bans: per-tile ban clocks shared by the gather loops, the pools
//! and the melee hunt. Pure except the clock read; the pool picks and
//! the load driver own the policy. Absorbed from the old `skill_gather`
//! layer plus the ban durations of `skill_common` (D61).

#![forbid(unsafe_code)]

use std::collections::HashMap;

pub const MAX_FLEES_PER_LOAD: u32 = 2;
pub const FLEE_BAN_TICKS: u64 = 500;
pub const GONE_BAN_TICKS: u64 = 250;
/// Unreachable stands (static NoRoute, never a transient crowd): the spot
/// stays banned ~2 min at dev tick (60 ms, ~18 min at official 600 ms)
/// instead of re-burning a full plan on every repick. A planner proof
/// goes stale fast (doors open, overrides change), so the ban is a
/// re-probe delay, not a near-session exile. Live "can't reach" from a
/// passing blockage rides along: skipping one tree for a while is
/// cheaper than the plan.
pub const UNREACHABLE_BAN_TICKS: u64 = 1_800;
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

/// Toll-fence side test on tiles (x, z): the Al Kharid side is a region
/// derived from the map (`gamedata::toll::behind`), never a line (the
/// fence zigzags, the Al Kharid shore reaches x 3259; live bd23dqpq
/// 2026-11-14). Same flank hunts together, never across.
pub fn same_side(first: (i32, i32), second: (i32, i32)) -> bool {
    lifescape_gamedata::toll::same_side((i64::from(first.0), i64::from(first.1)), (i64::from(second.0), i64::from(second.1)))
}

/// True when the spot sits across the toll fence from the bot; `fence`
/// is the nearby gate (`toll_gate_x`), `None` = no gate in reach (or the
/// run paid the toll): no filtering.
pub fn across_toll(bot: (i32, i32), spot: (i32, i32), fence: Option<i32>) -> bool {
    fence.is_some() && !same_side(spot, bot)
}

#[cfg(test)]
mod tests {
    use super::{across_toll, same_side, starved_oldest, BanSet};


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


    /// Fence side: the region splits flanks; `None` means no filtering.
    /// The south coast spot (3267,3148) sits west of x = 3268 yet behind
    /// the toll (live bd23dqpq 2026-11-14).
    #[test]
    fn fence_side_follows_the_region() {
        assert!(same_side((3262, 3220), (3255, 3275)), "west flank together");
        assert!(same_side((3270, 3220), (3300, 3290)), "east flank together");
        assert!(!same_side((3262, 3220), (3270, 3220)), "across never targets");
        assert!(same_side((3269, 3167), (3267, 3148)), "the coast spot is on the Al Kharid flank");
        assert!(across_toll((3222, 3218), (3267, 3148), Some(3268)), "coast spot across from Lumbridge");
        assert!(!across_toll((3222, 3218), (3255, 3275), Some(3268)), "Lumbridge cows same side");
        assert!(!across_toll((3222, 3218), (3267, 3148), None), "no fence means no filtering");
    }
}
