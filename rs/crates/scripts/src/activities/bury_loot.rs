//! Bury + arrow sweep: bones from our own kills never sit in the pack
//! (slots and prayer in one click each), and spent arrows come back the
//! same way coins do. Anywhere goes: no spot, no bank, no queue — the
//! combat loops, or call `bury_now` after each kill and `loot_arrows` with the
//! coin sweep. Certificates (`cert_*`) are bank paper, never buried.

#![forbid(unsafe_code)]

use lifescape_sdk::Bot;

use crate::ScriptResult;

/// Buryable F2P drops from our own kills (dump debugnames).
pub const BURYABLE: [&str; 2] = ["bones", "big_bones"];

/// True for a bone that goes in the ground, never in the bank.
pub fn is_bones(name: &str) -> bool {
    BURYABLE.contains(&name.to_lowercase().as_str())
}

/// True for a spent arrow worth picking up (plain or poisoned).
pub fn is_arrow_pickup(name: &str) -> bool {
    let lowered = name.to_lowercase();
    let base: &str = lowered.strip_suffix("_p").unwrap_or(&lowered);
    crate::activities::ranged_style::ARROW_TIERS.contains(&base)
}

/// Bury every buryable bone in the pack. One pass, no walk, no bank.
pub fn bury_now(bot: &mut Bot) -> ScriptResult {
    crate::kit::loot::bury_bones(bot)?;
    Ok(0)
}

/// Prayer from banked bones: `bury_loot [bones|big_bones] [count]
/// [--want ...] [--level prayer N]`. Each load withdraws a pack of the
/// named bones and buries them; none banked or carried blocks by name.
pub fn run_bury_loot(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let resource = args.first().filter(|first| first.parse::<u32>().is_err() && !first.starts_with("--")).cloned();
    let rest: Vec<String> = if resource.is_some() { args[1..].to_vec() } else { args.to_vec() };
    let spec = crate::kit::cycle::parse_cli("bury_loot", &rest)?;
    let bones_name = resource.unwrap_or_else(|| BURYABLE[0].to_string());
    if !is_bones(&bones_name) {
        return Err(crate::ScriptError::Usage(format!("{bones_name:?} is no buryable F2P bone")));
    }
    let keep = crate::kit::cycle::resolve_keep(bot, &spec)?;
    crate::kit::survival::wait_pack_streamed(bot)?;
    crate::kit::survival::check_events(bot)?;
    let bones = crate::kit::pack::obj_id(bot, &bones_name)?;
    let bank = crate::kit::bank::nearest_bank(bot)?;
    crate::kit::cycle::run(bot, &spec, |step_bot, state| {
        crate::kit::survival::check_events(step_bot)?;
        if crate::kit::pack::pack_total(step_bot, bones) == 0 {
            crate::kit::bank::bank_trip(step_bot, &bank, &keep, 0)?;
            let room = crate::kit::pack::free_slots(step_bot) as u32;
            crate::kit::withdraw::bank_withdraw_items(step_bot, &bank, &[bones], room)?;
            if crate::kit::pack::pack_total(step_bot, bones) == 0 {
                return Err(crate::ScriptError::Blocked { missing: vec![bones_name.clone()] });
            }
        }
        let buried = crate::kit::loot::bury_bones(step_bot)?;
        step_bot.log(&format!("ok load {} buried {buried} {bones_name}", state.units() + 1));
        Ok(crate::kit::cycle::StepControl::UnitDone)
    })
}

/// Pick up spent arrows around the killer spot, nearest first, then bury.
/// Dead sockets fail typed; refused takes only log.
pub fn loot_arrows(bot: &mut Bot) -> Result<u32, crate::ScriptError> {
    let mut taken = 0u32;
    let mut items: Vec<((i32, i32, i32), u16)> = bot
        .world
        .ground_items(bot.world.x, bot.world.z, bot.world.level, crate::kit::loot::LOOT_RADIUS, None)
        .into_iter()
        .map(|((tile, oid), _)| (tile, oid))
        .collect();
    items.sort_by_key(|(tile, _)| (tile.0 - bot.world.x).abs() + (tile.1 - bot.world.z).abs());
    for (_, oid) in items {
        if crate::kit::pack::free_slots(bot) == 0 {
            break;
        }
        let name = (bot.tables.obj_name)(oid).unwrap_or_default();
        if !is_arrow_pickup(&name) {
            continue;
        }
        match bot.take(Some(oid), None, crate::kit::loot::TAKE_TICKS) {
            Ok(true) => taken += 1,
            Ok(false) => bot.log(&format!("arrow {name} gave nothing at {:?}", bot.position())),
            Err(error)
                if matches!(
                    error,
                    lifescape_sdk::Error::Interrupted(_) | lifescape_sdk::Error::Io(_) | lifescape_sdk::Error::Login(_)
                ) =>
            {
                return Err(error.into())
            }
            Err(error) => bot.log(&format!("arrow {name} failed at {:?}: {error}", bot.position())),
        }
    }
    crate::kit::loot::bury_bones(bot)?;
    Ok(taken)
}

#[cfg(test)]
mod tests {
    use super::{is_arrow_pickup, is_bones, BURYABLE};

    #[test]
    fn bones_go_down_paper_does_not() {
        assert!(is_bones("bones"), "our own drops bury");
        assert!(is_bones("big_bones"), "big bones bury");
        assert!(is_bones("BONES"), "case never matters");
        assert!(!is_bones("cert_bones"), "bank paper never buries");
        assert!(!is_bones("bronze_arrow"), "ammo never buries");
        assert!(!is_bones("coins"), "coins never bury");
        assert_eq!(BURYABLE.len(), 2, "bones and big bones only");
    }

    #[test]
    fn arrow_sweep_takes_shot_arrows_only() {
        assert!(is_arrow_pickup("bronze_arrow"), "spent bronze comes back");
        assert!(is_arrow_pickup("steel_arrow_p"), "poisoned steel comes back");
        assert!(!is_arrow_pickup("shortbow"), "the bow stays worn");
        assert!(!is_arrow_pickup("arrow_shaft"), "shafts are fletching, not loot");
        assert!(!is_arrow_pickup("bones"), "bones bury, never sweep");
        assert!(!is_arrow_pickup("coins"), "coins keep their own sweep");
    }
}
