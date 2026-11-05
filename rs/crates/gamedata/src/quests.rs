//! Quests scraped from quest scripts. Mirrors `gamedata/quests.py`.

use crate::{num, opt_flag, opt_str, str_list, table};

/// One `stat_base` gate inside a quest script.
#[derive(Debug, Clone)]
pub struct SkillReq {
    pub skill: String,
    pub op: String,
    pub level: i64,
}

/// One inventory/worn item check.
#[derive(Debug, Clone)]
pub struct ItemCheck {
    pub container: String,
    pub obj: String,
}

/// One XP reward.
#[derive(Debug, Clone)]
pub struct XpReward {
    pub skill: String,
    pub xp: i64,
}

/// One quest row. `members` stays null: the server never says.
#[derive(Debug, Clone)]
pub struct Quest {
    pub name: String,
    pub members: Option<bool>,
    pub files: i64,
    pub varps: Vec<String>,
    pub skill_reqs: Vec<SkillReq>,
    pub item_checks: Vec<ItemCheck>,
    pub qp_gates: Vec<(String, i64)>,
    pub xp_rewards: Vec<XpReward>,
    pub complete_raw: Vec<String>,
}

fn quest_from_row(row: &crate::Json) -> Quest {
    let skills = match row.get("skill_reqs") {
        Some(crate::Json::Arr(items)) => items
            .iter()
            .map(|item| SkillReq {
                skill: opt_str(item, "skill").unwrap_or_default(),
                op: opt_str(item, "op").unwrap_or_default(),
                level: num(item, "level", 0),
            })
            .collect(),
        _ => Vec::new(),
    };
    let checks = match row.get("item_checks") {
        Some(crate::Json::Arr(items)) => items
            .iter()
            .map(|item| ItemCheck {
                container: opt_str(item, "container").unwrap_or_default(),
                obj: opt_str(item, "obj").unwrap_or_default(),
            })
            .collect(),
        _ => Vec::new(),
    };
    let gates = match row.get("qp_gates") {
        Some(crate::Json::Arr(items)) => {
            items.iter().map(|item| (opt_str(item, "op").unwrap_or_default(), num(item, "value", 0))).collect()
        }
        _ => Vec::new(),
    };
    let rewards = match row.get("xp_rewards") {
        Some(crate::Json::Arr(items)) => items
            .iter()
            .map(|item| XpReward { skill: opt_str(item, "skill").unwrap_or_default(), xp: num(item, "xp", 0) })
            .collect(),
        _ => Vec::new(),
    };
    Quest {
        name: opt_str(row, "quest").unwrap_or_default(),
        members: opt_flag(row, "members"),
        files: num(row, "files", 0),
        varps: str_list(row, "varps"),
        skill_reqs: skills,
        item_checks: checks,
        qp_gates: gates,
        xp_rewards: rewards,
        complete_raw: str_list(row, "complete_raw"),
    }
}

/// Every quest row.
pub fn all_quests() -> Vec<Quest> {
    table("quests").iter().map(quest_from_row).collect()
}

/// Quests with this exact (case-insensitive) debugname.
pub fn by_name(name: &str) -> Vec<Quest> {
    let wanted = name.to_lowercase();
    all_quests().into_iter().filter(|entry| entry.name.to_lowercase() == wanted).collect()
}

#[cfg(test)]
mod tests {
    use super::{all_quests, by_name};

    #[test]
    fn quest_rows() {
        // Python: 64 quests; first row is quest_arena (members null,
        // 14 files); quest_cook resolves by exact name.
        let all = all_quests();
        assert_eq!(all.len(), 64);
        assert_eq!(all[0].name, "quest_arena");
        assert_eq!(all[0].members, None);
        assert_eq!(all[0].files, 14);
        assert_eq!(by_name("quest_cook").len(), 1);
        assert!(by_name("no such quest").is_empty());
    }
}
