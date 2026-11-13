//! Mining activity: the brain's ore (`mining gold_ore --site x,z,level`)
//! at the brain's site, banked full loads. A thin executor: the ordered
//! ore only (best tier by level only on manual runs without one), the
//! bank nearest the site, a pickaxe from the pack or the bank — never a
//! shop buy nor a toll crossing of its own (`blocked` instead, the brain
//! obtains and routes). Everything else lives in the kit (`kit::mine`
//! driver, `kit::spots` pool, `kit::tools` rank).

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::kit::cycle::{resolve_keep, run, StepControl};
use crate::kit::bank::{bank_trip, nearest_bank};
use crate::kit::pack::obj_id;
use crate::kit::survival::{check_events, learn_level};
use crate::{ScriptError, ScriptResult};

/// Rock tiers the run may mine: the ordered ore's tier alone, or every
/// usable tier on a manual run. An unknown ore is a Usage error; an ore
/// above the live level is `blocked{level:mining:N}`.
pub fn ordered_rock_tiers(resource: Option<&str>, level: i64) -> Result<Vec<knowledge::rocks::RockTier>, ScriptError> {
    let tiers = crate::kit::spots::usable_rock_tiers();
    let Some(ore) = resource else { return Ok(tiers) };
    let ordered: Vec<knowledge::rocks::RockTier> = tiers.into_iter().filter(|tier| tier.product.eq_ignore_ascii_case(ore)).collect();
    let Some(need) = ordered.iter().map(|tier| tier.level).min() else {
        return Err(ScriptError::Usage(format!("no F2P rock yields {ore:?}")));
    };
    if level < need {
        return Err(crate::kit::cycle::blocked_level("mining", need));
    }
    Ok(ordered)
}

/// Mine banked loads in the open session (0 = forever).
pub fn run_mining(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let spec = crate::kit::cycle::parse_cli("mining", args)?;
    crate::kit::toll::arm(bot, spec.allow_paid)?;
    bot.allow_shop = spec.allow_shop;
    bot.assigned_site = spec.site;
    let task_keep = resolve_keep(bot, &spec)?;
    crate::kit::survival::wait_pack_streamed(bot)?;
    check_events(bot)?;
    let coins = obj_id(bot, "coins")?;
    let mut level = learn_level(bot, crate::kit::mine::MINING_STAT, 1);
    let tiers = ordered_rock_tiers(spec.resource.first().map(String::as_str), level)?;
    // The bank serving the brain's site, not the one nearest the bot
    // (Lumbridge → Draynor for an Al Kharid mine order).
    let mut bank = match crate::kit::site::site_bank(bot) {
        Some(bank) => bank,
        None => nearest_bank(bot)?,
    };
    bot.log(&format!("bank at ({},{})", bank.x, bank.z));
    let pick = &crate::kit::tools::PICK_SPEC;
    let (handle, head) = (crate::kit::tools::PICK_HANDLE, crate::kit::tools::PICK_HEAD_SUFFIX);
    if !crate::kit::tools::reclaim_free(bot, &bank, pick, handle, head)? {
        return Err(crate::kit::tools::blocked_tool("bronze_pickaxe"));
    }
    let mut toolless = 0u32;
    run(bot, &spec, |step_bot, state| {
        check_events(step_bot)?;
        let (ok, new_level) = crate::kit::mine::mine_load(step_bot, &tiers, &mut bank, coins, level)?;
        level = new_level;
        if !ok {
            return Err(ScriptError::Failed(format!("mine failed at {:?}", step_bot.position())));
        }
        let mut keep: Vec<u16> = pick.known_ids(step_bot).into_iter().collect();
        keep.extend(task_keep.iter().copied());
        bank_trip(step_bot, &bank, &keep, 0)?;
        toolless = if pick.best_held(step_bot).is_none() { toolless + 1 } else { 0 };
        if toolless >= 2 {
            return Err(crate::kit::tools::blocked_tool("bronze_pickaxe"));
        }
        step_bot.log(&format!("ok load {} banked (mining={level})", state.units() + 1));
        Ok(StepControl::UnitDone)
    })
}

#[cfg(test)]
mod tests {
    use super::ordered_rock_tiers;

    /// The brain's ore is the only tier mined (farm:tin_ore at level 20
    /// used to mine iron); above the level it blocks typed.
    #[test]
    fn ordered_ore_only_and_level_gap_blocks() {
        let tin = ordered_rock_tiers(Some("tin_ore"), 20).expect("tin at 20");
        assert!(!tin.is_empty() && tin.iter().all(|tier| tier.product == "tin_ore"), "tin only");
        match ordered_rock_tiers(Some("gold_ore"), 20) {
            Err(crate::ScriptError::Blocked { missing }) => assert_eq!(missing, vec!["level:mining:40".to_string()]),
            other => panic!("gold at 20 must block on level, got {other:?}"),
        }
        assert!(matches!(ordered_rock_tiers(Some("dragon_ore"), 99), Err(crate::ScriptError::Usage(_))));
        assert!(ordered_rock_tiers(None, 1).expect("manual run").len() > 3, "manual runs keep every tier");
    }
}
