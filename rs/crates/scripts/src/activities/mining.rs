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

use crate::kit::cycle::{resolve_keep, run};
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
/// Hurt on the site (D3): bank (the pickaxe and the task's objects kept),
/// heal by the meal rules, then the next load walks back.
fn heal_hurt(bot: &mut Bot, bank: &knowledge::banks::Bank, pick: &crate::kit::tools::ToolSpec, task_keep: &[u16]) -> Result<(), ScriptError> {
    let mut keep: Vec<u16> = pick.known_ids(bot).into_iter().collect();
    keep.extend(task_keep.iter().copied());
    bank_trip(bot, bank, &keep, 0)?;
    crate::kit::bank::heal_at_bank(bot)?;
    crate::kit::meals::eat_to_full(bot)?;
    Ok(())
}

/// Ores of the run's tiers in the pack.
fn ores_held(bot: &Bot, tiers: &[lifescape_gamedata::rocks::RockTier]) -> u32 {
    tiers.iter().filter_map(|tier| obj_id(bot, &tier.product).ok()).map(|id| crate::kit::pack::pack_total(bot, id)).sum()
}

/// Pure: a load that ended without filling (rocks dry, stall bound) is
/// banked as a load when it mined something and the bot is not hurt
/// (the hurt rules own a hurt load); nothing mined stays a failure.
pub fn stalled_load_banks(mined: u32, hurt: bool) -> bool {
    mined > 0 && !hurt
}

/// True when the bot is under its heal line now (the hurt rules own it).
fn hurt_now(bot: &Bot) -> bool {
    let (hp, max_hp) = (crate::kit::survival::hitpoints(bot), crate::kit::survival::max_hitpoints(bot));
    crate::kit::survival::hurt_return(hp, max_hp, 0) != crate::kit::survival::HurtReturn::NotHurt
}

pub fn run_mining(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let spec = crate::kit::cycle::parse_cli("mining", args)?;
    crate::kit::toll::arm_fares(bot, spec.allow_paid, spec.allow_boat, spec.site)?;
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
    let mut hurt_streak = 0u32;
    run(bot, &spec, |step_bot, state| {
        check_events(step_bot)?;
        // Under the eat line before the walk out (tour 3 point 8, live
        // tim_wolfe 12:54 on the Wilderness iron: runs relaunched at hp
        // 34, then 22, before the hurt rule saw 12): heal first, by the
        // same rule as a hurt load; still hurt past `HURT_RETURNS`, typed.
        let (hp, max_hp) = (crate::kit::survival::hitpoints(step_bot), crate::kit::survival::max_hitpoints(step_bot));
        if crate::kit::survival::hurt_return(hp, max_hp, 0) != crate::kit::survival::HurtReturn::NotHurt {
            if crate::kit::survival::hurt_return(hp, max_hp, hurt_streak) == crate::kit::survival::HurtReturn::GiveUp {
                return Err(ScriptError::Failed(format!("hurt on the site: hp {hp}/{max_hp} after {hurt_streak} heals at {:?}, too dangerous now", step_bot.position())));
            }
            hurt_streak += 1;
            step_bot.log(&format!("hurt before the load (hp {hp}/{max_hp}): banking to heal first ({hurt_streak}/{})", crate::kit::survival::HURT_RETURNS));
            heal_hurt(step_bot, &bank, pick, &task_keep)?;
            return Ok(state.load_done(step_bot, &format!("healed before a load (mining={level})")));
        }
        let until = state.want_goal(step_bot, &spec);
        let ores_before = ores_held(step_bot, &tiers);
        let (ok, new_level) = crate::kit::mine::mine_load(step_bot, &tiers, &mut bank, coins, level, until)?;
        level = new_level;
        // A load that mined ore before its rocks ran dry is a load, never
        // a failure (live 2026-11-17, 223 « no adamantite_ore mined » on
        // the two Al Kharid adamantite rocks: 800-tick respawn, longer
        // than the repick window, while "You manage to mine some
        // adamantite"). The ore is banked; the measured yield prices the
        // slow spot (R2), the guides steer away from it.
        let mined = ores_held(step_bot, &tiers).saturating_sub(ores_before);
        if !ok && stalled_load_banks(mined, hurt_now(step_bot)) {
            step_bot.log(&format!("rocks dry before a full load: banking the {mined} ore(s) mined"));
        } else if !ok {
            // Hurt on the site (D3): bank, heal by the meal rules, come
            // back; past `HURT_RETURNS` hurt loads the failure below
            // stands (the brain cools the site for this bot).
            let hp = crate::kit::survival::hitpoints(step_bot);
            let max_hp = crate::kit::survival::max_hitpoints(step_bot);
            if crate::kit::survival::hurt_return(hp, max_hp, hurt_streak) == crate::kit::survival::HurtReturn::HealAndReturn {
                hurt_streak += 1;
                step_bot.log(&format!("hurt on the site (hp {hp}/{max_hp}): banking to heal, then back ({hurt_streak}/{})", crate::kit::survival::HURT_RETURNS));
                heal_hurt(step_bot, &bank, pick, &task_keep)?;
                return Ok(state.load_done(step_bot, &format!("healed after a hurt load (mining={level})")));
            }
            // Other players on the rocks: contention, never a method
            // failure (D4: the brain moves the bot to another site).
            if let Some(contended) = crate::kit::site::contended_error(step_bot) {
                return Err(contended);
            }
            // Typed context (rule 6, live 2026-11-15: 272 bare "mine
            // failed" over 34 bots could not be classed).
            let ores: Vec<&str> = tiers.iter().map(|tier| tier.product.as_str()).collect();
            let last = step_bot.world.messages.last().cloned().unwrap_or_else(|| "no game message".to_string());
            return Err(ScriptError::Failed(format!(
                "mine failed at {:?}: no {} mined (load stalled or hurt), hp {}, {} free slots; last game: {last}",
                step_bot.position(),
                ores.join("/"),
                crate::kit::survival::hitpoints(step_bot),
                crate::kit::pack::free_slots(step_bot)
            )));
        }
        hurt_streak = 0;
        let mut keep: Vec<u16> = pick.known_ids(step_bot).into_iter().collect();
        keep.extend(task_keep.iter().copied());
        bank_trip(step_bot, &bank, &keep, 0)?;
        toolless = if pick.best_held(step_bot).is_none() { toolless + 1 } else { 0 };
        if toolless >= 2 {
            return Err(crate::kit::tools::blocked_tool("bronze_pickaxe"));
        }
        Ok(state.load_done(step_bot, &format!("banked (mining={level})")))
    })
}

#[cfg(test)]
mod tests {
    use super::ordered_rock_tiers;

    /// Two ores mined before the rocks ran dry: banked, no failure; none
    /// mined, or hurt: the failure and hurt rules stand.
    #[test]
    fn a_dry_load_with_ore_is_banked() {
        assert!(super::stalled_load_banks(2, false));
        assert!(!super::stalled_load_banks(0, false));
        assert!(!super::stalled_load_banks(2, true));
    }

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
