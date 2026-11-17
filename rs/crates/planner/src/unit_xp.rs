//! XP of the XP-bearing units (plan rendements §7, R3.1): XP is never
//! measured, it is derived from counted units times the XP one unit pays
//! (dump tenths / 10, `method_way::action_xp`; combat `combat.rs2`
//! `give_combat_experience`). One carrier per family, so nothing counts
//! twice:
//! - P (product made): gathers, cooking, smithing, crafting stages,
//!   enchant, superheat (Magic + Smithing), telegrab;
//! - C (input eaten): firemaking (log), prayer (bones), runecraft
//!   (essence), alchemy (the object);
//! - S (cast): teleports, curses, bones to bananas, a combat spell's base;
//! - K (damage): a fight, by its style. Per damage point (tenths over
//!   10): accurate/aggressive/defensive 4 to the trained skill,
//!   controlled 1.33 to each of attack, strength and defence, ranged 4,
//!   magic 2, hitpoints 1.33 on every style (`scale(133, 100, base)`).

use crate::method_way::{action_xp, method_way};

/// XP per damage point to the skill a melee style or ranged trains.
pub const XP_PER_DAMAGE: f64 = 4.0;
/// XP per damage point to magic (on top of the spell's base per cast).
pub const MAGIC_XP_PER_DAMAGE: f64 = 2.0;
/// XP per damage point to hitpoints, every style.
pub const HITPOINTS_XP_PER_DAMAGE: f64 = 1.33;

/// The unit kinds that pay XP for `method` run with `style` (the
/// report's `combat_style`: `melee:<skill>`, `ranged`, `magic`).
pub fn carrier_kinds(method: &str, style: &str) -> &'static [&'static str] {
    if !style.is_empty() || method.starts_with("fight:") {
        let magic = style == "magic" || method.starts_with("fight:magic/");
        return if magic { &["damage", "cast"] } else { &["damage"] };
    }
    match method.split([':', '/']).next().unwrap_or("") {
        "fire" | "bury" | "runecraft" | "alch" => &["input"],
        "teleport" | "curse" | "bananas" => &["cast"],
        _ => &["product"],
    }
}

fn spell_xp(spell: &str) -> Option<f64> {
    lifescape_gamedata::spells::all_spells().into_iter().find(|row| row.spell == spell).map(|row| row.xp as f64 / 10.0).filter(|xp| *xp > 0.0)
}

fn skill_of(method: &str) -> Option<String> {
    method_way(method).and_then(|(_, way)| way.skill.map(|(skill, _)| skill))
}

/// Tier-table XP of a gathered product (fish, ore, log), if any.
fn gathered_xp(product: &str) -> Option<(String, f64)> {
    let fish = lifescape_gamedata::fishing::all_fish_tiers().into_iter().find_map(|tier| {
        let at = tier.products.iter().position(|name| *name == product)?;
        tier.exp.get(at).map(|tenths| ("fishing".to_string(), *tenths as f64 / 10.0))
    });
    fish.or_else(|| lifescape_gamedata::rocks::all_rock_tiers().into_iter().find(|tier| tier.product == product).map(|tier| ("mining".to_string(), tier.exp as f64 / 10.0)))
        .or_else(|| lifescape_gamedata::trees::all_tiers().into_iter().find(|tier| tier.product == product).map(|tier| ("woodcutting".to_string(), tier.exp as f64 / 10.0)))
}

/// XP one product unit of `method` pays (`product`: debug name).
fn product_xp(method: &str, product: &str) -> Vec<(String, f64)> {
    let made = method_way(method);
    if let Some((own, way)) = &made {
        if own == product {
            let mut out = Vec::new();
            if let (Some((skill, _)), Some(xp)) = (&way.skill, action_xp(method)) {
                out.push((skill.clone(), xp));
            }
            // Superheat pays the smelt's Smithing beside its Magic.
            if method.starts_with("superheat:") {
                out.extend(action_xp(&format!("smelt:{product}")).map(|xp| ("smithing".to_string(), xp)));
            }
            return out;
        }
    }
    // Another product of the same work: the second fish of a spot, the
    // other pottery stage, a gather tier.
    if let Some(found) = gathered_xp(product) {
        return vec![found];
    }
    let family = method.split(':').next().unwrap_or("");
    let sibling = format!("{family}:{product}");
    match (skill_of(&sibling), action_xp(&sibling)) {
        (Some(skill), Some(xp)) if method_way(&sibling).is_some_and(|(own, _)| own == product) => vec![(skill, xp)],
        _ => Vec::new(),
    }
}

/// Prayer XP of burying `subject` (`bury` rows of the dump), if a bone.
fn buried_xp(subject: &str) -> Option<f64> {
    lifescape_gamedata::extra_recipes::by_kind("bury").into_iter().find(|recipe| recipe.inputs.iter().any(|(name, _)| name == subject)).map(|recipe| recipe.xp as f64 / 10.0).filter(|xp| *xp > 0.0)
}

/// XP one unit `(kind, subject)` of `method` pays at `level`, per skill.
/// Kinds outside the method's carriers pay nothing (counted once), but a
/// bone buried pays its Prayer in any run (bones picked up then buried
/// during a fight: §7, Prayer C "souvent DANS un run de combat").
/// Subjects: items by debug name, spells by spell name, NPCs by name.
pub fn unit_xp(method: &str, style: &str, kind: &str, subject: &str, level: u8) -> Vec<(String, f64)> {
    // Static game data: once per (method, style, kind, subject), the
    // level kept only where it counts (alchemy's spell); the guides ask
    // it for every unit of every row (~10^5 times a set).
    static MEMO: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<(String, String, String, String, u8), Vec<(String, f64)>>>> = std::sync::OnceLock::new();
    let level_key = if method.starts_with("alch:") { level } else { 0 };
    crate::guide_methods::memo(&MEMO, (method.to_string(), style.to_string(), kind.to_string(), subject.to_string(), level_key), || compute_unit_xp(method, style, kind, subject, level))
}

fn compute_unit_xp(method: &str, style: &str, kind: &str, subject: &str, level: u8) -> Vec<(String, f64)> {
    if kind == "input" {
        if let Some(xp) = buried_xp(subject) {
            return vec![("prayer".to_string(), xp)];
        }
    }
    if !carrier_kinds(method, style).contains(&kind) {
        return Vec::new();
    }
    match kind {
        "damage" => {
            let trained = if style == "magic" || method.starts_with("fight:magic/") {
                ("magic".to_string(), MAGIC_XP_PER_DAMAGE)
            } else if style == "ranged" || method.starts_with("fight:ranged/") {
                ("ranged".to_string(), XP_PER_DAMAGE)
            } else if style == "melee:controlled" {
                // Controlled: 1.33 per damage to each melee skill.
                return ["attack", "strength", "defence", "hitpoints"].iter().map(|skill| (skill.to_string(), HITPOINTS_XP_PER_DAMAGE)).collect();
            } else {
                let skill = style.strip_prefix("melee:").filter(|skill| !skill.is_empty()).unwrap_or("attack");
                (skill.to_string(), XP_PER_DAMAGE)
            };
            vec![trained, ("hitpoints".to_string(), HITPOINTS_XP_PER_DAMAGE)]
        }
        "cast" => spell_xp(subject).map(|xp| vec![("magic".to_string(), xp)]).unwrap_or_default(),
        "input" if method.starts_with("alch:") => crate::alchemy::alch_spell(level).map(|spell| vec![("magic".to_string(), spell.xp)]).unwrap_or_default(),
        "input" => match (skill_of(method), action_xp(method)) {
            (Some(skill), Some(xp)) => vec![(skill, xp)],
            _ => Vec::new(),
        },
        "product" => product_xp(method, subject),
        _ => Vec::new(),
    }
}

/// XP per skill of `units` (kind, subject, count) for `method`.
pub fn derived_xp(method: &str, style: &str, units: &[(String, String, f64)], level: u8) -> Vec<(String, f64)> {
    let mut out: Vec<(String, f64)> = Vec::new();
    for (kind, subject, count) in units {
        for (skill, xp) in unit_xp(method, style, kind, subject, level) {
            match out.iter_mut().find(|(known, _)| *known == skill) {
                Some(row) => row.1 += xp * count,
                None => out.push((skill, xp * count)),
            }
        }
    }
    out.sort_by(|left, right| left.0.cmp(&right.0));
    out
}

#[cfg(test)]
#[path = "unit_xp_tests.rs"]
mod tests;
