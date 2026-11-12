//! Best gear a bot can use (PRD3 lot 5): pure, levels in, picks out.
//!
//! Per combat style, the best F2P wearable per slot that the bot meets
//! the server wield gates for (`levelrequire` labels, quest gates), that
//! some F2P way yields (`obtain`), scored for that style. Arrows follow
//! the server ammo rule (`player_ranged.rs2`: an arrow above the bow's
//! tier is refused). Tools: the best pickaxe and axe of the bot's tier.

use crate::equipment::{all_wearables, Wearable};

/// Combat style of a gear set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    Melee,
    Ranged,
    Magic,
}

impl Style {
    pub fn parse(text: &str) -> Style {
        match text {
            "ranged" | "range" => Style::Ranged,
            "magic" | "mage" => Style::Magic,
            _ => Style::Melee,
        }
    }
}

/// Server wearpos slots.
pub const HEAD: i64 = 0;
pub const CAPE: i64 = 1;
pub const NECK: i64 = 2;
pub const WEAPON: i64 = 3;
pub const BODY: i64 = 4;
pub const SHIELD: i64 = 5;
pub const LEGS: i64 = 7;
pub const HANDS: i64 = 9;
pub const FEET: i64 = 10;
pub const RING: i64 = 12;
pub const AMMO: i64 = 13;

/// One pick: slot, object, style score.
#[derive(Debug, Clone, PartialEq)]
pub struct Pick {
    pub slot: i64,
    pub item: String,
    pub score: i64,
}

fn bonus(pairs: &[(String, i64)], key: &str) -> i64 {
    pairs.iter().find(|(name, _)| name == key).map(|(_, value)| *value).unwrap_or(0)
}

fn level(levels: &[(&str, i64)], skill: &str) -> i64 {
    levels.iter().find(|(name, _)| *name == skill).map(|(_, value)| *value).unwrap_or(1)
}

fn is_bow(entry: &Wearable) -> bool {
    entry.category.as_deref() == Some("weapon_bow") || entry.name.ends_with("shortbow") || entry.name.ends_with("longbow")
}

fn is_staff(entry: &Wearable) -> bool {
    entry.category.as_deref() == Some("weapon_staff") || entry.name.contains("staff")
}

fn is_tool(entry: &Wearable) -> bool {
    entry.name.ends_with("_pickaxe") || entry.name.ends_with("_axe") && !entry.name.contains("battleaxe")
}

/// Style score of one wearable (higher is better; < 0 = never wear it
/// for this style).
pub fn score(entry: &Wearable, style: Style) -> i64 {
    let attack = |key: &str| bonus(&entry.attack, key);
    let defence = |key: &str| bonus(&entry.defence, key);
    let melee_def = defence("stab") + defence("slash") + defence("crush");
    if entry.wearpos == WEAPON {
        return match style {
            Style::Melee if !is_bow(entry) && !is_staff(entry) => attack("stab").max(attack("slash")).max(attack("crush")) + 2 * entry.strength,
            Style::Ranged if is_bow(entry) => attack("range"),
            Style::Magic if is_staff(entry) => attack("magic") + 1,
            _ => -1,
        };
    }
    if entry.wearpos == AMMO {
        return if style == Style::Ranged && entry.rangebonus > 0 { entry.rangebonus } else { -1 };
    }
    match style {
        Style::Melee => melee_def + 2 * entry.strength + entry.prayer + attack("stab").max(attack("slash")).max(attack("crush")).max(0),
        // Ranged armour: range attack first, then the defences (d'hide,
        // leather, coif); melee plate drains range attack (negative).
        Style::Ranged => 3 * attack("range") + defence("range") + melee_def / 3,
        // Robes and wizard hats: magic attack first, then magic defence.
        Style::Magic => 3 * attack("magic") + defence("magic") + melee_def / 6,
    }
}

/// True when the bot may wield `entry`: every level gate met, any quest
/// gate among `quests_done` (quest directory keys, `quest_dragon`).
pub fn wieldable(entry: &Wearable, levels: &[(&str, i64)], quests_done: &[&str]) -> bool {
    let gates_met = entry.requirements.iter().all(|(skill, needed)| level(levels, skill) >= *needed);
    let quest_met = entry.quest.as_deref().is_none_or(|quest| quests_done.contains(&quest));
    gates_met && quest_met
}

fn usable(entry: &Wearable, levels: &[(&str, i64)], quests_done: &[&str]) -> bool {
    let f2p = crate::objs_by_name(&entry.name).into_iter().next().is_some_and(|obj| obj.members == Some(false));
    if !f2p || !wieldable(entry, levels, quests_done) || !crate::obtain::obtainable_f2p(&entry.name) {
        return false;
    }
    // A quest reward (Silverlight, the anti-dragon shield) counts once
    // its quest is done: some way must need no quest, or only quests the
    // bot finished (scripted ways carry their file's quest hints).
    crate::obtain::ways(&entry.name).iter().any(|way| {
        let mut needs: Vec<String> = way.quest.iter().cloned().collect();
        needs.extend(way.quest_hints());
        needs.iter().all(|quest| quests_done.contains(&quest.as_str()))
    })
}

/// Best gear for `style` at `levels` (stat name to base level, missing =
/// 1). One pick per slot; a two-handed weapon (bows, 2h swords) empties
/// the shield slot when it beats the best one-hander plus shield.
pub fn best_gear(style: Style, levels: &[(&str, i64)], quests_done: &[&str]) -> Vec<Pick> {
    let candidates: Vec<Wearable> = all_wearables().into_iter().filter(|entry| !is_tool(entry) && usable(entry, levels, quests_done)).collect();
    let best_in = |slot: i64, filter: &dyn Fn(&Wearable) -> bool| -> Option<(Wearable, i64)> {
        candidates
            .iter()
            .filter(|entry| entry.wearpos == slot && filter(entry))
            .map(|entry| (entry.clone(), score(entry, style)))
            .filter(|(_, value)| *value >= 0)
            .max_by(|left, right| left.1.cmp(&right.1).then_with(|| right.0.name.cmp(&left.0.name)))
    };
    let mut picks = Vec::new();
    for slot in [HEAD, CAPE, NECK, BODY, LEGS, HANDS, FEET, RING] {
        if let Some((entry, value)) = best_in(slot, &|entry: &Wearable| entry.wearpos2 != SHIELD || slot != WEAPON) {
            if value > 0 {
                picks.push(Pick { slot, item: entry.name, score: value });
            }
        }
    }
    let one_hand = best_in(WEAPON, &|entry: &Wearable| entry.wearpos2 != SHIELD);
    let two_hand = best_in(WEAPON, &|entry: &Wearable| entry.wearpos2 == SHIELD);
    let shield = best_in(SHIELD, &|_| true);
    let one_total = one_hand.as_ref().map(|(_, value)| *value).unwrap_or(-1) + shield.as_ref().map(|(_, value)| (*value).max(0) / 2).unwrap_or(0);
    let two_total = two_hand.as_ref().map(|(_, value)| *value).unwrap_or(-1);
    // Bows are always two-handed: ranged never takes the one-hander path.
    let take_two = two_hand.is_some() && (style == Style::Ranged || two_total > one_total);
    if take_two {
        let (weapon, value) = two_hand.clone().expect("checked above");
        picks.push(Pick { slot: WEAPON, item: weapon.name.clone(), score: value });
        if style == Style::Ranged {
            let bow_tier = weapon.tier.max(1);
            let arrows = candidates
                .iter()
                .filter(|entry| entry.wearpos == AMMO && entry.rangebonus > 0 && entry.tier <= bow_tier)
                .max_by_key(|entry| entry.rangebonus);
            if let Some(arrow) = arrows {
                picks.push(Pick { slot: AMMO, item: arrow.name.clone(), score: arrow.rangebonus });
            }
        }
    } else if let Some((weapon, value)) = one_hand {
        picks.push(Pick { slot: WEAPON, item: weapon.name, score: value });
        if let Some((guard, value)) = shield {
            if value > 0 {
                picks.push(Pick { slot: SHIELD, item: guard.name, score: value });
            }
        }
    }
    picks.sort_by_key(|pick| pick.slot);
    picks
}

/// Best tool of a family the bot can use: pickaxes by mining level
/// (`levelrequire` = the server's mining gate), axes by woodcutting tier.
pub fn best_tool(family: &str, levels: &[(&str, i64)]) -> Option<String> {
    let (suffix, skill) = match family {
        "pickaxe" => ("_pickaxe", "mining"),
        "axe" => ("_axe", "woodcutting"),
        _ => return None,
    };
    all_wearables()
        .into_iter()
        .filter(|entry| entry.name.ends_with(suffix) && !entry.name.contains("battleaxe"))
        .filter(|entry| crate::objs_by_name(&entry.name).into_iter().next().is_some_and(|obj| obj.members == Some(false)))
        .filter(|entry| entry.tier.max(1) <= level(levels, skill).max(1) && crate::obtain::obtainable_f2p(&entry.name))
        .max_by_key(|entry| (entry.tier, entry.strength))
        .map(|entry| entry.name)
}

#[cfg(test)]
mod tests {
    use super::{best_gear, best_tool, Style, AMMO, BODY, WEAPON};

    fn pick(picks: &[super::Pick], slot: i64) -> Option<String> {
        picks.iter().find(|pick| pick.slot == slot).map(|pick| pick.item.clone())
    }

    #[test]
    fn melee_tiers_follow_attack_and_defence() {
        let fresh = best_gear(Style::Melee, &[], &[]);
        let weapon = pick(&fresh, WEAPON).expect("a weapon at level 1");
        assert!(weapon.starts_with("bronze_") || weapon.starts_with("iron_"), "level 1 swings bronze/iron, got {weapon}");
        let strong = best_gear(Style::Melee, &[("attack", 40), ("defence", 40), ("strength", 40)], &[]);
        assert!(pick(&strong, WEAPON).expect("weapon").starts_with("rune_"), "attack 40 swings rune: {strong:?}");
        let body = pick(&strong, BODY).expect("body");
        assert_ne!(body, "rune_platebody", "rune plate needs Dragon Slayer");
        let hero = best_gear(Style::Melee, &[("attack", 40), ("defence", 40), ("strength", 40)], &["quest_dragon"]);
        assert_eq!(pick(&hero, BODY).as_deref(), Some("rune_platebody"), "after the quest");
    }

    #[test]
    fn ranged_pairs_bow_and_arrows_by_tier() {
        let fresh = best_gear(Style::Ranged, &[("ranged", 1)], &[]);
        assert!(pick(&fresh, WEAPON).is_some_and(|bow| bow.contains("bow")), "{fresh:?}");
        assert!(pick(&fresh, AMMO).is_some_and(|arrow| arrow.contains("arrow")), "{fresh:?}");
        // Willow and maple bows are members in rev 274: oak is the top
        // F2P bow (ranged 5).
        let archer = best_gear(Style::Ranged, &[("ranged", 30), ("defence", 1)], &[]);
        let bow = pick(&archer, WEAPON).expect("bow");
        assert!(bow.starts_with("oak_"), "the best F2P bow is oak, got {bow}");
        let arrow = pick(&archer, AMMO).expect("arrows");
        let bow_tier = crate::equipment::by_name(&bow).expect("bow row").tier;
        assert!(crate::equipment::by_name(&arrow).expect("arrow row").tier <= bow_tier, "server ammo rule");
    }

    #[test]
    fn magic_wears_a_staff() {
        let mage = best_gear(Style::Magic, &[("magic", 1)], &[]);
        assert!(pick(&mage, WEAPON).is_some_and(|staff| staff.contains("staff")), "{mage:?}");
    }

    #[test]
    fn tools_follow_the_skill_tier() {
        // Iron pickaxes mine at level 1 (levelrequire 0, like bronze).
        assert_eq!(best_tool("pickaxe", &[("mining", 1)]).as_deref(), Some("iron_pickaxe"));
        assert_eq!(best_tool("pickaxe", &[("mining", 41)]).as_deref(), Some("rune_pickaxe"));
        assert_eq!(best_tool("pickaxe", &[("mining", 21)]).as_deref(), Some("mithril_pickaxe"));
        assert!(best_tool("axe", &[("woodcutting", 1)]).is_some());
    }
}
