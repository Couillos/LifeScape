//! Negligible drops of a measured activity (plan rendements R2.1): one
//! "other drops" line per activity instead of a row per rare pickup.
//!
//! A combat or a gather picks up stray objects (a single rune, a bronze
//! med helm, a pot of flour) whose measured rate rests on one or two
//! pickups over every run of the bucket. Published one by one they drown
//! the table (live 2026-11-16: 2 828 of the 4 082 loot rows under one
//! unit per 6000 ticks) and none of them alone tells anything; their
//! summed VALUE does, so they fold into one line valued at the current
//! price (D12) and the profit keeps every coin of them.
//!
//! Threshold `NEGLIGIBLE_DROP_PER_6000` = one unit per 6000 work ticks
//! (one prod hour): under it, a drop shows up less than once in the
//! window the guides rank by, i.e. not a yield a player plans for. Only
//! pickups (`loot`, `ground_spawn`) fold; the activity's own product,
//! every input, consumable, fee and loss always keep their line.

/// Units per 6000 work ticks under which a pickup folds into "other
/// drops" (one per window: rarer than once an hour in prod).
pub const NEGLIGIBLE_DROP_PER_6000: f64 = 1.0;
/// Causes that are pickups (the only ones that may fold).
pub const DROP_CAUSES: [&str; 2] = ["loot", "ground_spawn"];
/// Name of the folded line.
pub const OTHER_DROPS: &str = "other drops";

/// The folded pickups of one activity: how many objects, their summed
/// units and value per 6000 ticks at the current price.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OtherDrops {
    pub items: usize,
    pub units_per_6000: f64,
    pub value_per_6000: f64,
}

/// True for a pickup line under the threshold that is not the activity's
/// own product (`own`: the object the activity exists for).
pub fn negligible(item: &str, cause: &str, per_6000: f64, own: &dyn Fn(&str) -> bool) -> bool {
    DROP_CAUSES.contains(&cause) && per_6000.abs() < NEGLIGIBLE_DROP_PER_6000 && !own(item)
}

/// Splits `flows` (item, cause, per 6000) into the lines kept and the
/// folded "other drops", valued with `price` (coins per unit).
pub fn fold_negligible_drops(flows: &[(String, String, f64)], own: &dyn Fn(&str) -> bool, price: &dyn Fn(&str) -> f64) -> (Vec<(String, String, f64)>, OtherDrops) {
    let mut kept = Vec::new();
    let mut other = OtherDrops::default();
    for (item, cause, rate) in flows {
        if negligible(item, cause, *rate, own) {
            other.items += 1;
            other.units_per_6000 += rate;
            other.value_per_6000 += rate * price(item);
        } else {
            kept.push((item.clone(), cause.clone(), *rate));
        }
    }
    (kept, other)
}

#[cfg(test)]
mod tests {
    use super::{fold_negligible_drops, OTHER_DROPS};

    fn line(item: &str, cause: &str, rate: f64) -> (String, String, f64) {
        (item.to_string(), cause.to_string(), rate)
    }

    /// Rare pickups fold into one valued line; the product, the inputs
    /// and frequent drops keep theirs; no coin is lost.
    #[test]
    fn rare_pickups_fold_into_one_valued_line() {
        let flows = vec![line("bones", "loot", 120.0), line("naturerune", "loot", 0.4), line("bronze_med_helm", "loot", 0.2), line("airrune", "ground_spawn", 0.5), line("shrimp", "food", -0.3), line("airrune_target", "ground_spawn", 0.1)];
        let price = |item: &str| match item {
            "naturerune" => 100.0,
            "bronze_med_helm" => 20.0,
            "airrune" => 4.0,
            _ => 1.0,
        };
        let (kept, other) = fold_negligible_drops(&flows, &|item| item == "airrune_target", &price);
        let names: Vec<&str> = kept.iter().map(|(item, _, _)| item.as_str()).collect();
        assert_eq!(names, vec!["bones", "shrimp", "airrune_target"], "food and the own product never fold");
        assert_eq!(other.items, 3);
        assert!((other.value_per_6000 - (40.0 + 4.0 + 2.0)).abs() < 1e-9, "{other:?}");
        assert_eq!(OTHER_DROPS, "other drops");
    }
}
