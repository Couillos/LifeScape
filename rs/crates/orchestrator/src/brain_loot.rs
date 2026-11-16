#![forbid(unsafe_code)]
//! Pickup filter by value (plan marché M5, §3.7 "offre à la source"): a
//! fighter never picks up a drop worth less than the ticks of the pickup
//! at its own coin rate (a player leaves feathers at 1 coin lying). The
//! brain lists, per fight launch, the drops of the spot's monsters whose
//! expected stack is worth less than `PICKUP_TICKS` ticks of the bot's
//! rate (`brain_reserve::OwnCosts::coins_per_tick`); the fight script
//! leaves them lying (`--skip-loot NAME`). The value of a drop is public:
//! its current traded price or what an F2P shop pays, the richer one.
//! Coins and buryable bones are never listed (bones are buried on the
//! spot: prayer training).

/// Ticks one pickup costs: the walk and the click (loot.rs: 98.9 % of
/// the takes land within 20 ticks).
pub const PICKUP_TICKS: f64 = 20.0;

/// The spot key of a fight method (`fight:melee/<spot>`,
/// `fight:ranged/<spot>`, `fight:magic/<spell>/<spot>`).
pub fn fight_spot(method: &str) -> Option<&str> {
    let tail = method.strip_prefix("fight:")?;
    tail.rsplit('/').next().filter(|spot| !spot.is_empty() && *spot != tail)
}

/// Drops of `spot`'s monsters not worth their pickup (pure): `value_of`
/// gives the public value of one unit, `coins_per_tick` the bot's rate.
pub fn loot_skip(spot: &str, value_of: &dyn Fn(&str) -> i64, coins_per_tick: f64) -> Vec<String> {
    let Some(entry) = lifescape_gamedata::combat_spots::by_key(spot).or_else(|| lifescape_gamedata::combat_spots::catalogue_spots().into_iter().find(|candidate| candidate.key == spot)) else {
        return Vec::new();
    };
    let pickup_worth = PICKUP_TICKS * coins_per_tick.max(0.0);
    let mut skip: Vec<String> = Vec::new();
    for npc in entry.npc_names.iter().flat_map(|name| lifescape_gamedata::npcs_by_name(name)) {
        for drop in lifescape_gamedata::drops::drops_for_npc(npc.npc_id) {
            if drop.obj == "coins" || drop.obj.ends_with("bones") || drop.rate <= 0.0 {
                continue;
            }
            let mean = if drop.qty_min >= 1 && drop.qty_max >= drop.qty_min { (drop.qty_min + drop.qty_max) as f64 / 2.0 } else { 1.0 };
            if (value_of(&drop.obj) as f64) * mean < pickup_worth {
                skip.push(drop.obj.clone());
            }
        }
    }
    skip.sort();
    skip.dedup();
    skip
}

/// Launch arguments of the filter.
pub fn skip_args(skip: &[String]) -> Vec<String> {
    skip.iter().flat_map(|name| ["--skip-loot".to_string(), name.clone()]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fight_methods_name_their_spot() {
        assert_eq!(fight_spot("fight:melee/chickens"), Some("chickens"));
        assert_eq!(fight_spot("fight:ranged/cows"), Some("cows"));
        assert_eq!(fight_spot("fight:magic/wind_strike/cows"), Some("cows"));
        assert_eq!(fight_spot("gather:tree/logs"), None);
    }

    /// Chickens: feathers at 1 coin are not worth 20 ticks of a bot
    /// earning 2 coins a tick; raw chicken worth 60 is. A poor bot (0.05
    /// coin a tick) takes everything.
    #[test]
    fn cheap_drops_are_left_lying_for_a_rich_bot() {
        let value = |item: &str| match item {
            "feather" => 1,
            "raw_chicken" => 60,
            _ => 100,
        };
        let skip = loot_skip("chickens", &value, 2.0);
        assert!(skip.contains(&"feather".to_string()), "{skip:?}");
        assert!(!skip.contains(&"raw_chicken".to_string()), "{skip:?}");
        assert!(!skip.iter().any(|name| name == "coins" || name.ends_with("bones")));
        assert!(loot_skip("chickens", &value, 0.05).is_empty(), "a poor bot takes every drop");
        assert_eq!(skip_args(&["feather".to_string()]), vec!["--skip-loot", "feather"]);
    }
}
