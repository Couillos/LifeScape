//! Computed ambition ladders (user 2026-11-15, audit chantier 6: "les
//! paliers et quotas calculés au lieu des listes écrites à la main").
//! A ladder is derived from the game data the planner already reads:
//! - a producer kind (woodcutter, fisher, miner, cook, crafter, smith,
//!   runecrafter) climbs its skill by rungs of `RUNG_STEP` levels to 99;
//!   each rung offers the products the skill opened in the last
//!   `FRESH_WINDOW` levels (else the best ones it makes), every one a
//!   production leg of that skill (`obtain_legs::production_legs`), and a
//!   quota sized by XP: the actions that climb `QUOTA_LEVELS` levels at
//!   the rung (`sizing::xp_between` over `method_way::action_xp`);
//! - a consuming kind (firemaker, priest) climbs levels only: the priced
//!   training picks what to buy, cut or burn (`training_production`);
//! - a fighting kind (warrior, ranger, mage) climbs its stats, its
//!   equipment tier every other rung;
//! - the merchant doubles its coin target.
//! No product list is written by hand: a level the server gates (herring
//! 10, not 5) reaches the ladder through the data (the old fisher rung 10
//! asked anchovies at 15).

use crate::ambition_catalog::{Rung, Then};

/// Levels between two rungs.
pub const RUNG_STEP: u8 = 10;
/// Levels a rung's products opened within.
pub const FRESH_WINDOW: u8 = 15;
/// Products offered per rung at most.
pub const MAX_PRODUCTS: usize = 4;
/// Levels a quota's XP climbs at its rung.
pub const QUOTA_LEVELS: u8 = 5;
/// Quota bounds.
pub const QUOTA_MIN: i64 = 10;
pub const QUOTA_MAX: i64 = 200;

/// How a kind's ladder is built.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Rule {
    /// Produces with `skill`.
    Producer(&'static str),
    /// Levels of `skill`, the `style` equipment every other rung (none:
    /// `""`).
    Climber(&'static str, &'static str),
    /// Melee: attack and strength levels, defence with the equipment.
    Melee,
    /// Coin targets rising from 5 000 to 1 000 000.
    Coins,
    /// No ladder (generalist, overall).
    Empty,
}

/// One product of a skill: the object, the level its leg opens at, XP
/// per action.
#[derive(Debug, Clone, PartialEq)]
pub struct Product {
    pub item: String,
    pub level: u8,
    pub xp: f64,
}

/// Every F2P product `skill` makes (its production legs), lowest level
/// first, then name (computed once per skill).
pub fn products_of(skill: &str) -> Vec<Product> {
    type Table = std::collections::HashMap<String, Vec<Product>>;
    static MEMO: std::sync::OnceLock<std::sync::Mutex<Table>> = std::sync::OnceLock::new();
    let memo = MEMO.get_or_init(Default::default);
    if let Some(known) = memo.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(skill) {
        return known.clone();
    }
    let found = compute_products(skill);
    memo.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(skill.to_string(), found.clone());
    found
}

fn compute_products(skill: &str) -> Vec<Product> {
    let mut out: Vec<Product> = Vec::new();
    for obj in lifescape_gamedata::all_objs() {
        let Some(name) = obj.name else { continue };
        if obj.members != Some(false) || name.starts_with("cert_") {
            continue;
        }
        let best = crate::obtain_legs::production_legs(name)
            .into_iter()
            // A quota every bot of the kind can work: no pickup, no quest
            // gate (blurite: The Knight's Sword), a site some combat level
            // makes safe, and a harvest with real F2P sites (no F2P maple).
            .filter(|leg| !leg.method.starts_with("ground:") && leg.quest.is_none() && crate::combat::min_combat_for_method(&leg.method) < lifescape_gamedata::danger::NEVER)
            .filter(|leg| !leg.method.starts_with("farm:") || !leg.sites.is_empty())
            .filter_map(|leg| {
                let (trained, level) = leg.skill.clone()?;
                let xp = crate::method_way::action_xp(&leg.method)?;
                (trained == skill && xp > 0.0).then(|| (u8::try_from(level.clamp(1, 99)).unwrap_or(99), xp))
            })
            .min_by_key(|(level, _)| *level);
        if let Some((level, xp)) = best {
            out.push(Product { item: name.to_string(), level, xp });
        }
    }
    out.sort_by(|left, right| left.level.cmp(&right.level).then_with(|| left.item.cmp(&right.item)));
    out
}

/// Products of a rung at `level`: the ones opened in the last
/// `FRESH_WINDOW` levels, highest first, else the best made below.
pub fn rung_products(products: &[Product], level: u8) -> Vec<Product> {
    let open: Vec<&Product> = products.iter().filter(|product| product.level <= level).collect();
    let fresh: Vec<&Product> = open.iter().copied().filter(|product| product.level + FRESH_WINDOW > level).collect();
    let mut chosen: Vec<Product> = if fresh.is_empty() { open.clone() } else { fresh }.into_iter().cloned().collect();
    chosen.sort_by(|left, right| right.level.cmp(&left.level).then_with(|| left.item.cmp(&right.item)));
    chosen.truncate(MAX_PRODUCTS);
    chosen
}

/// Quota of a rung: the actions that climb `QUOTA_LEVELS` levels from
/// `level` at the median XP of its products, bounded, rounded to 5.
pub fn quota(products: &[Product], level: u8) -> i64 {
    let mut xps: Vec<f64> = products.iter().map(|product| product.xp).collect();
    xps.sort_by(f64::total_cmp);
    let Some(median) = xps.get(xps.len() / 2).copied().filter(|xp| *xp > 0.0) else { return QUOTA_MIN };
    let actions = crate::sizing::xp_between(level, level.saturating_add(QUOTA_LEVELS).min(99)) / median;
    let rounded = ((actions / 5.0).round() * 5.0) as i64;
    rounded.clamp(QUOTA_MIN, QUOTA_MAX)
}

fn leak_items(products: &[Product]) -> &'static [&'static str] {
    let names: Vec<&'static str> = products.iter().map(|product| &*Box::leak(product.item.clone().into_boxed_str())).collect();
    Box::leak(names.into_boxed_slice())
}

fn rung_levels() -> impl Iterator<Item = u8> {
    (1..=9).map(|step| step * RUNG_STEP)
}

/// The ladder `rule` builds (computed once per kind by the catalogue).
pub fn build(rule: Rule) -> Vec<Rung> {
    match rule {
        Rule::Producer(skill) => {
            let all = products_of(skill);
            rung_levels()
                .map(|level| {
                    let chosen = rung_products(&all, level);
                    if chosen.is_empty() {
                        Rung { skill, level, then: Then::Nothing }
                    } else {
                        Rung { skill, level, then: Then::Produce(leak_items(&chosen), quota(&chosen, level), skill) }
                    }
                })
                .collect()
        }
        Rule::Climber(skill, style) => rung_levels()
            .map(|level| {
                let then = if !style.is_empty() && (level / RUNG_STEP) % 2 == 1 { Then::Equip(style) } else { Then::Nothing };
                Rung { skill, level, then }
            })
            .collect(),
        Rule::Melee => rung_levels()
            .skip(1)
            .flat_map(|level| {
                [
                    Rung { skill: "attack", level, then: Then::Nothing },
                    Rung { skill: "strength", level, then: Then::Nothing },
                    Rung { skill: "defence", level, then: Then::Equip("melee") },
                ]
            })
            .collect(),
        Rule::Coins => [5_000, 20_000, 50_000, 100_000, 250_000, 500_000, 1_000_000].iter().map(|amount| Rung { skill: "", level: 0, then: Then::Coins(*amount) }).collect(),
        Rule::Empty => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::{build, products_of, quota, rung_products, Product, Rule, QUOTA_MAX, QUOTA_MIN};
    use crate::ambition_catalog::Then;

    #[test]
    fn a_producer_rung_offers_what_its_skill_just_opened() {
        let fish = products_of("fishing");
        let at = |level: u8| rung_products(&fish, level).into_iter().map(|product| product.item).collect::<Vec<_>>();
        assert!(at(10).contains(&"raw_shrimp".to_string()) && !at(10).contains(&"raw_anchovies".to_string()), "anchovies open at 15: {:?}", at(10));
        assert!(at(20).iter().any(|item| item == "raw_anchovies" || item == "raw_trout"), "{:?}", at(20));
        assert!(at(40).contains(&"raw_lobster".to_string()), "{:?}", at(40));
        assert!(!at(90).is_empty(), "the top tier stays offered");
    }

    #[test]
    fn quotas_climb_a_few_levels_within_bounds() {
        let cheap = vec![Product { item: "logs".to_string(), level: 1, xp: 25.0 }];
        let rich = vec![Product { item: "steel_platebody".to_string(), level: 48, xp: 187.5 }];
        assert!((QUOTA_MIN..=QUOTA_MAX).contains(&quota(&cheap, 10)));
        assert!(quota(&rich, 20) < quota(&vec![Product { item: "steel_dagger".to_string(), level: 30, xp: 37.5 }], 20), "a richer action needs fewer units");
        assert_eq!(quota(&cheap, 90), QUOTA_MAX, "high levels cap");
    }

    /// Every computed rung's product is made by its kind's skill at or
    /// under the rung level (the anchovies-at-10 bug cannot come back).
    #[test]
    fn producer_ladders_hold_their_levels() {
        for skill in ["woodcutting", "fishing", "mining", "cooking", "crafting", "smithing", "runecraft"] {
            let ladder = build(Rule::Producer(skill));
            assert_eq!(ladder.len(), 9, "{skill}: rungs 10..90");
            let mut produced = 0;
            for rung in &ladder {
                if let Then::Produce(items, count, trained) = rung.then {
                    produced += 1;
                    assert_eq!(trained, skill);
                    assert!(count >= QUOTA_MIN && count <= QUOTA_MAX);
                    for item in items {
                        let ok = crate::obtain_legs::production_legs(item).iter().any(|leg| leg.skill.as_ref().is_some_and(|(name, level)| name == skill && *level <= i64::from(rung.level)));
                        assert!(ok, "{skill} rung {}: {item} not made at that level", rung.level);
                    }
                }
            }
            assert!(produced >= 3, "{skill}: a real production ladder ({produced} rungs)");
        }
    }

    #[test]
    fn climbers_equip_every_other_rung() {
        let mage = build(Rule::Climber("magic", "magic"));
        assert_eq!(mage.iter().filter(|rung| matches!(rung.then, Then::Equip("magic"))).count(), 5);
        assert!(build(Rule::Climber("firemaking", "")).iter().all(|rung| rung.then == Then::Nothing));
        let melee = build(Rule::Melee);
        assert!(melee.iter().any(|rung| rung.skill == "attack") && melee.iter().any(|rung| rung.skill == "defence" && rung.then == Then::Equip("melee")));
    }
}
