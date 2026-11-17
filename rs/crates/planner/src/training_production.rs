//! Priced training of the consuming skills (user 2026-11-15: "train des
//! skills qui s'entraînent plus vite en farmant de l'argent et en
//! achetant les compo pas cher"): cooking, crafting, firemaking and
//! prayer pick their method at resolve like smithing does — every recipe
//! open at the level, its inputs priced at what the bot holds (sale
//! value), buys or makes (`TrainPrices::acquire`, its time included),
//! its product at what the market absorbs, its time per XP from its
//! yield (R3.3, `training::rated_option`). Same score and ranking as
//! `training` (most profitable first, then the cheapest XP).

use crate::training::{rated_option, TrainPick, TrainPrices};

/// Skills this file prices.
pub const PRODUCTION_SKILLS: [&str; 4] = ["cooking", "crafting", "firemaking", "prayer"];

/// One open recipe of a consuming skill.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub method: String,
    /// What the action makes and the market may buy (`None`: ashes left
    /// on the ground, bones buried).
    pub product: Option<String>,
    pub level: u8,
    pub inputs: Vec<(String, i64)>,
    /// Coins paid per action (tanning fee).
    pub coins: i64,
    pub xp: f64,
    pub success: f64,
}

/// Every F2P recipe of `skill` the dumps know (computed once per skill).
pub fn candidates(skill: &str) -> Vec<Candidate> {
    type Table = std::collections::HashMap<String, Vec<Candidate>>;
    static MEMO: std::sync::OnceLock<std::sync::Mutex<Table>> = std::sync::OnceLock::new();
    let memo = MEMO.get_or_init(Default::default);
    if let Some(known) = memo.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(skill) {
        return known.clone();
    }
    let found = compute_candidates(skill);
    memo.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(skill.to_string(), found.clone());
    found
}

fn compute_candidates(skill: &str) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    let mut push = |method: String, product: Option<String>| {
        if out.iter().any(|known| known.method == method) {
            return;
        }
        let Some((_, way)) = crate::method_way::method_way(&method) else { return };
        let Some((trained, level)) = way.skill.clone() else { return };
        let Some(xp) = crate::method_way::action_xp(&method) else { return };
        if trained != skill || way.inputs.is_empty() || xp <= 0.0 {
            return;
        }
        // Only recipes a script runs as a level climb (`mapping`).
        let probe = crate::model::Node::new("probe", crate::model::NodeKind::ReachLevel { skill: skill.to_string(), level: 99 }, Vec::new(), 0);
        if crate::mapping::map_method(&method, &probe, None).is_none() {
            return;
        }
        let level = u8::try_from(level.clamp(1, 99)).unwrap_or(99);
        out.push(Candidate { method, product, level, inputs: way.inputs.clone(), coins: way.coins, xp, success: if way.success > 0.0 { way.success } else { 1.0 } });
    };
    for obj in lifescape_gamedata::all_objs() {
        let Some(name) = obj.name else { continue };
        if obj.members != Some(false) || name.starts_with("cert_") {
            continue;
        }
        match skill {
            "firemaking" => push(format!("fire:{name}"), None),
            "prayer" => push(format!("bury:{name}"), None),
            _ => {
                for leg in crate::obtain_legs::production_legs(name) {
                    if leg.skill.as_ref().is_some_and(|(trained, _)| trained == skill) {
                        push(leg.method.clone(), Some(name.to_string()));
                    }
                }
            }
        }
    }
    out.sort_by(|left, right| left.level.cmp(&right.level).then_with(|| left.method.cmp(&right.method)));
    out
}

/// Every open recipe of `skill` at `level` toward `target`, priced, best
/// first. A recipe with an input neither held, bought nor made is out.
pub fn rank_production(skill: &str, level: u8, target: u8, prices: &TrainPrices) -> Vec<TrainPick> {
    let xp_needed = crate::sizing::xp_between(level, target.max(level.saturating_add(1)));
    let mut out = Vec::new();
    for row in candidates(skill).into_iter().filter(|row| row.level <= level) {
        out.extend(rated_option(&row.method, skill, level, xp_needed, prices));
    }
    crate::training::sort_picks(&mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::{candidates, rank_production};
    use crate::training::{Acquired, TrainPrices};

    #[test]
    fn each_consuming_skill_has_open_recipes() {
        let methods = |skill: &str| candidates(skill).into_iter().map(|row| row.method).collect::<Vec<_>>();
        assert!(methods("cooking").iter().any(|method| method.contains("trout")), "{:?}", methods("cooking"));
        assert!(methods("firemaking").contains(&"fire:willow_logs".to_string()));
        assert!(methods("prayer").contains(&"bury:big_bones".to_string()));
        assert!(methods("crafting").iter().any(|method| method == "craft:leather_gloves"), "{:?}", methods("crafting"));
    }

    /// Cheap raw fish whose cooked fish sells: the paying recipe wins
    /// over one that loses; with nothing to buy, nothing is priced.
    #[test]
    fn the_paying_recipe_wins() {
        let held = |_: &str| 0;
        let sale = |_: &str| 0.0;
        let acquire = |item: &str, _: i64| match item {
            "raw_trout" => Some(Acquired { coins_each: 5.0, made_by: None }),
            "raw_shrimp" => Some(Acquired { coins_each: 30.0, made_by: None }),
            _ => None,
        };
        let value = |item: &str, _: i64| match item {
            "trout" => 40.0,
            "shrimp" => 5.0,
            _ => 0.0,
        };
        let prices = TrainPrices { coin_rate: 0.5, coins: 10_000, held: &held, sale_value: &sale, acquire: &acquire, product_value: &value, rate: &crate::work_rate::prior_rate };
        let ranked = rank_production("cooking", 20, 25, &prices);
        let first = ranked.first().expect("trout and shrimp are priced");
        assert!(first.method.contains("trout") && first.profit_per_tick > 0.0, "{ranked:?}");
        assert!(ranked.iter().all(|pick| pick.method.contains("trout") || pick.method.contains("shrimp")), "only buyable inputs: {ranked:?}");
    }
}
