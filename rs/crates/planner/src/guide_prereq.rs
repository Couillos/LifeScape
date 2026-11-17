//! Prerequisites of a guide row (plan rendements D9, §2): levels, combat
//! (the Wilderness is a combat prerequisite, never a refusal: open world),
//! quests, carried tools, capital of a lot; and whether a bot of the
//! filter's profile (every skill at one level, its quests done) passes
//! them.

use crate::guide_model::{GuideFilter, Prereqs, ShopNeed};

/// The prerequisites of `method` (with `style`) at the band starting at
/// `band_level`, its tool tier `tool`.
pub fn prereqs_of(method: &str, style: &str, band_level: u8, tool: &str) -> Prereqs {
    static MEMO: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<(String, String, u8, String), Prereqs>>> = std::sync::OnceLock::new();
    crate::guide_methods::memo(&MEMO, (method.to_string(), style.to_string(), band_level, tool.to_string()), || compute_prereqs(method, style, band_level, tool))
}

fn compute_prereqs(method: &str, style: &str, band_level: u8, tool: &str) -> Prereqs {
    let mut levels: Vec<(String, u8)> = Vec::new();
    if let Some(skill) = crate::activity::band_skill(method, style) {
        let gate = crate::guide_methods::gate_level(method).max(band_level);
        levels.push((skill, gate));
    }
    // Superheat pays Smithing beside Magic: the smelt's own gate.
    if let Some(bar) = method.strip_prefix("superheat:") {
        let smelt = format!("smelt:{bar}");
        if let Some((_, way)) = crate::method_way::method_way(&smelt) {
            if let Some((skill, level)) = way.skill {
                levels.push((skill, level.clamp(1, 99) as u8));
            }
        }
    }
    let mut quests: Vec<String> = crate::quests::method_quest(method, None).into_iter().collect();
    if let Some((_, way)) = crate::method_way::method_way(method) {
        if let Some(quest) = way.quest {
            let key = quest.strip_prefix("quest_").unwrap_or(&quest).to_string();
            if !quests.contains(&key) {
                quests.push(key);
            }
        }
    }
    let mut tools: Vec<String> = crate::tools::required_tool(method).map(|need| need.tools).unwrap_or_default();
    for worn in tool.split('+').filter(|name| !name.is_empty()) {
        // The row's tier stands for its family's base tool.
        tools.retain(|base| !crate::tools::tool_family(base).iter().any(|name| name == worn));
        if !tools.iter().any(|known| known == worn) {
            tools.push(worn.to_string());
        }
    }
    let combat = crate::combat::min_combat_for_method(method);
    Prereqs { levels, combat, quests, quest_points: 0, tools, capital: 0.0, ironman: true, ..Prereqs::default() }
}

/// Adds the condition of a shop a row trades with (Q7, user
/// 2026-11-16): its quest, its door's quest points.
pub fn require_shop(into: &mut Prereqs, need: ShopNeed) {
    if let Some(quest) = need.quest {
        if !into.quests.iter().any(|known| known == quest) {
            into.quests.push(quest.to_string());
        }
    }
    into.quest_points = into.quest_points.max(need.quest_points);
}

/// Union of two prerequisite sets (a chain's stages): the highest level
/// per skill, the highest combat and quest points, every quest and
/// tool, every purchase and source gap (D22), capitals added.
pub fn merge(into: &mut Prereqs, other: &Prereqs) {
    into.quest_points = into.quest_points.max(other.quest_points);
    for (skill, level) in &other.levels {
        match into.levels.iter_mut().find(|(known, _)| known == skill) {
            Some(known) => known.1 = known.1.max(*level),
            None => into.levels.push((skill.clone(), *level)),
        }
    }
    into.combat = into.combat.max(other.combat);
    for quest in &other.quests {
        if !into.quests.contains(quest) {
            into.quests.push(quest.clone());
        }
    }
    for tool in &other.tools {
        if !into.tools.contains(tool) {
            into.tools.push(tool.clone());
        }
    }
    into.capital += other.capital;
    into.ironman &= other.ironman;
    into.market.extend(other.market.iter().cloned());
    into.sources.extend(other.sources.iter().cloned());
}

/// Combat level of a bot with every skill at `level` (hitpoints at
/// least 10, the server's start).
pub fn combat_at(level: u8) -> u8 {
    let levels = crate::combat::BaseLevels { attack: level, strength: level, defence: level, hitpoints: level.max(10), ranged: level, prayer: level, magic: level };
    crate::combat::combat_level(&levels)
}

/// True when the filter's bot passes `prereqs`: every level at or under
/// its level, the combat its levels make, every quest done, the quest
/// points its quests grant (`quest_points_of`). Without a level the row
/// is read at its own gate: levels pass, quests still ask.
pub fn passes(prereqs: &Prereqs, filter: &GuideFilter) -> bool {
    let points = prereqs.quest_points <= 0 || prereqs.quest_points <= lifescape_gamedata::quest_requirements::quest_points_of(&filter.quests_done);
    let quests = points && prereqs.quests.iter().all(|quest| filter.quests_done.contains(quest));
    match filter.level {
        Some(level) => quests && prereqs.levels.iter().all(|(_, needed)| *needed <= level) && prereqs.combat <= combat_at(level),
        None => quests,
    }
}

#[cfg(test)]
mod tests {
    use super::{passes, prereqs_of};
    use crate::guide_model::GuideFilter;

    /// Levels, quests and tools come from the game data; a quest gates
    /// the row until the bot has done it.
    #[test]
    fn prerequisites_come_from_the_data() {
        let essence = prereqs_of("essence:blankrune", "", 1, "rune_pickaxe");
        assert!(essence.quests.contains(&"runemysteries".to_string()), "{essence:?}");
        assert!(essence.tools.contains(&"rune_pickaxe".to_string()), "{essence:?}");
        let mut filter = GuideFilter { level: Some(41), ..GuideFilter::default() };
        assert!(!passes(&essence, &filter), "quest not done");
        filter.quests_done.push("runemysteries".to_string());
        assert!(passes(&essence, &filter));
        let steel = prereqs_of("smelt:steel_bar", "", 31, "");
        assert_eq!(steel.levels, vec![("smithing".to_string(), 31)]);
        assert!(!passes(&steel, &GuideFilter { level: Some(29), ..GuideFilter::default() }));
        let heat = prereqs_of("superheat:steel_bar", "", 43, "");
        assert!(heat.levels.contains(&("smithing".to_string(), 30)), "{heat:?}");
    }
}
