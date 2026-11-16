//! Activity key of a measure (plan rendements R2.1): what a run DID,
//! never the object the planner wanted from it.
//!
//! A planner method often names its goal object (`farm:feather`,
//! `farm:bronze_arrow`, `gather:fishing/raw_salmon`), while the work is
//! the same activity whatever the object wanted: killing the chickens of
//! one spot in one style, fishing one spot option. The yield estimator
//! keys its buckets by the activity, so every run of the same work feeds
//! the same bucket and a drop farm's whole loot is measured once.
//!
//! Derivation, mirroring the script the method launches (`mapping`):
//! - `farm:<drop>` runs `melee <spot>`: `fight:melee/<spot>` (the spot
//!   the report's target names, else the spot the planner sends a drop
//!   farm to, `mapping::spot_for_drop`); a ranged or magic style keys
//!   `fight:ranged/<spot>` or `fight:magic/<spell>/<spot>` (the spell
//!   the run cast);
//! - `farm:<fish>` and `gather:fishing/<fish>` fish one spot option:
//!   `gather:fishing/<first catch of the tier>` (trout and salmon share
//!   the lure option, shrimps and anchovies the net);
//! - `farm:<logs>`, `farm:<ore>` run the gatherer on that tree or rock:
//!   `gather:woodcut/<logs>`, `gather:mining/<ore>`;
//! - `farm:blankrune` runs Aubury's essence trips: `essence:blankrune`;
//! - `cook/<raw>` cooks the same dish as `cook:<dish>`;
//! - every other method already names its activity (a recipe IS its
//!   product, a pickup its spawn, a fight its spot, a curse its camp).
//!
//! A drop farm whose spot is unknown (no target, no drop spot) keeps its
//! planner method: nothing is guessed.

use lifescape_gamedata::combat_spots;

/// Fishing activity of a catch: the first catch of its tier (`None`
/// when no F2P tier catches it).
pub fn fishing_activity(fish: &str) -> Option<String> {
    lifescape_gamedata::fishing::all_fish_tiers()
        .into_iter()
        .find(|tier| !tier.product_members && tier.products.contains(&fish))
        .and_then(|tier| tier.products.first().map(|first| format!("gather:fishing/{first}")))
}

/// Spot a fight run worked: the report's target when it names a combat
/// spot, else the planner's drop spot of `item`.
fn fight_spot(target: &str, item: &str) -> Option<String> {
    if combat_spots::by_key(target).is_some() {
        return Some(target.to_string());
    }
    crate::mapping::spot_for_drop(item)
}

/// Spell a magic run cast (its `cast` units), the first one named.
fn cast_spell(units: &[(String, String, i64)]) -> Option<String> {
    units.iter().find(|(kind, _, qty)| kind == "cast" && *qty > 0).map(|(_, subject, _)| subject.clone())
}

/// Activity key of a run of `method` with `style` (the report's combat
/// style), `target` (its first positional, the spot or resource) and its
/// XP-bearing `units` (kind, subject, quantity).
pub fn activity_of_run(method: &str, style: &str, target: &str, units: &[(String, String, i64)]) -> String {
    if let Some(item) = method.strip_prefix("farm:") {
        if !style.is_empty() {
            let Some(spot) = fight_spot(target, item) else { return method.to_string() };
            return match style {
                "ranged" => format!("fight:ranged/{spot}"),
                "magic" => match cast_spell(units) {
                    Some(spell) => format!("fight:magic/{spell}/{spot}"),
                    None => method.to_string(),
                },
                _ => format!("fight:melee/{spot}"),
            };
        }
        return farm_activity(item).unwrap_or_else(|| method.to_string());
    }
    if let Some(fish) = method.strip_prefix("gather:fishing/") {
        return fishing_activity(fish).unwrap_or_else(|| method.to_string());
    }
    // `cook/<raw>` (a training tranche) cooks what `cook:<dish>` cooks.
    if let Some(raw) = method.strip_prefix("cook/") {
        return crate::method_way::cooked_of(raw).map(|dish| format!("cook:{dish}")).unwrap_or_else(|| method.to_string());
    }
    method.to_string()
}

/// Activity a planner method works when no run tells more (the brain's
/// reads, the guides): the same derivation, no target, no units.
pub fn activity_of_method(method: &str) -> String {
    activity_of_run(method, "", "", &[])
}

/// Activity of `farm:<item>` without a fight style: the gatherer, the
/// essence trips, or the drop farm's melee spot.
fn farm_activity(item: &str) -> Option<String> {
    if let Some(fishing) = fishing_activity(item) {
        return Some(fishing);
    }
    if item == "blankrune" {
        return Some("essence:blankrune".to_string());
    }
    if item == "logs" || item.ends_with("_logs") {
        return Some(format!("gather:woodcut/{item}"));
    }
    if item.ends_with("_ore") || item == "coal" || item == "clay" {
        return Some(format!("gather:mining/{item}"));
    }
    crate::mapping::spot_for_drop(item).map(|spot| format!("fight:melee/{spot}"))
}

/// Skill whose band keys an activity: the trained melee skill of the
/// style (attack for controlled), `ranged`, `magic`, else the skill of
/// the method's way. `None` for work no skill gates (milk, pick, tan).
pub fn band_skill(method: &str, style: &str) -> Option<String> {
    // Controlled trains the three melee skills: it bands on attack.
    if style == "melee:controlled" {
        return Some("attack".to_string());
    }
    if let Some(skill) = style.strip_prefix("melee:").filter(|skill| !skill.is_empty()) {
        return Some(skill.to_string());
    }
    if style == "ranged" || style == "magic" {
        return Some(style.to_string());
    }
    if method.starts_with("fight:ranged/") {
        return Some("ranged".to_string());
    }
    if method.starts_with("fight:magic/") || method.starts_with("alch:") {
        return Some("magic".to_string());
    }
    if method.starts_with("essence:") {
        return Some("mining".to_string());
    }
    crate::method_way::method_way(method)
        .or_else(|| crate::spell_methods::spell_way(method))
        .and_then(|(_, way)| way.skill.map(|(skill, _)| skill))
}

#[cfg(test)]
#[path = "activity_tests.rs"]
mod tests;
