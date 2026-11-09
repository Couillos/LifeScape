//! Quests scraped from quest scripts.

/// One static quest row (build-time data, mirrored into [`Quest`] per call).
pub(crate) struct StaticQuest {
    pub name: &'static str,
    pub files: i64,
    pub varps: &'static [&'static str],
    pub skill_reqs: &'static [(&'static str, &'static str, i64)],
    pub item_checks: &'static [(&'static str, &'static str)],
    pub qp_gates: &'static [(&'static str, i64)],
    pub xp_rewards: &'static [(&'static str, i64)],
    pub complete_raw: &'static [&'static str],
    pub members: Option<bool>,
}

mod generated {
    include!(concat!(env!("OUT_DIR"), "/quests_generated.rs"));
}

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

fn quest_from_static(row: &StaticQuest) -> Quest {
    Quest {
        name: row.name.to_string(),
        members: row.members,
        files: row.files,
        varps: row.varps.iter().map(|entry| (*entry).to_string()).collect(),
        skill_reqs: row
            .skill_reqs
            .iter()
            .map(|(skill, op, level)| SkillReq { skill: (*skill).to_string(), op: (*op).to_string(), level: *level })
            .collect(),
        item_checks: row
            .item_checks
            .iter()
            .map(|(container, obj)| ItemCheck { container: (*container).to_string(), obj: (*obj).to_string() })
            .collect(),
        qp_gates: row.qp_gates.iter().map(|(op, value)| ((*op).to_string(), *value)).collect(),
        xp_rewards: row
            .xp_rewards
            .iter()
            .map(|(skill, xp)| XpReward { skill: (*skill).to_string(), xp: *xp })
            .collect(),
        complete_raw: row.complete_raw.iter().map(|entry| (*entry).to_string()).collect(),
    }
}

/// Every quest row.
pub fn all_quests() -> Vec<Quest> {
    generated::ALL.iter().map(quest_from_static).collect()
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
        // 64 quests; first row is quest_arena (members null,
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
