//! Melee style: which combat button trains which skill, and the
//! rotation between attack/strength/defence. Moved out of the melee
//! script: `kit::fight` presses through `set_style`, `kit::gear` scores
//! through `fight_weapon_style` + `style_damage_key`.

#![forbid(unsafe_code)]

use lifescape_sdk::Bot;

use crate::kit::gear::current_weapon_category;
use crate::ScriptError;

/// Controlled style button per weapon category, when this revision offers
/// one: slash/spiked/claws index 2, spear 0-2, polearm 0. Controlled trains
/// attack + strength + defence at once (combat.rs2:151-154, each 133/100 of
/// base xp). Unarmed has NO controlled entry (unarmed_table), so bare fists
/// rotate instead of sitting on accurate.
/// Source: content/scripts/skill_combat/configs/combat.dbrow (slash_table,
/// spear_table, polearm_table, spiked_table, claws_table) crossed with
/// content/scripts/skill_combat/scripts/player/player_attackstyles.rs2:37-76
/// (button suffix -> style index).
pub(crate) fn controlled_button(category: &str) -> Option<&'static str> {
    match category {
        "weapon_slash" => Some("combat_hacksword:hack2"),
        "weapon_spiked" => Some("combat_spiked:spiked2"),
        "weapon_claws" => Some("combat_claw:claw2"),
        "weapon_spear" => Some("combat_spear:spear0"),
        "weapon_polearm" => Some("combat_polearm:polearm0"),
        _ => None,
    }
}

/// Combat xp across the four fighting stats (attack 0, defence 1, strength 2,
/// hitpoints 3): any landed hit moves it, so a flat line means no combat.
pub(crate) fn combat_xp(bot: &Bot) -> i64 {
    // Melee, hitpoints, ranged (4) and magic (6): any style's hit counts.
    [0u8, 1, 2, 3, 4, 6].iter().filter_map(|index| bot.world.stats.get(index)).map(|(_, xp)| i64::from(*xp)).sum()
}

/// Level of one melee skill by stat id (0 attack, 1 defence, 2 strength).
pub(crate) fn melee_stat(bot: &Bot, skill: &str) -> i64 {
    let index = match skill {
        "attack" => 0u8,
        "defence" => 1u8,
        _ => 2u8,
    };
    bot.world.stats.get(&index).map(|(level, _)| i64::from(*level)).unwrap_or(1)
}

/// Style to switch to, if any: controlled never rotates, otherwise the
/// weakest skill when it sits a full level below the current one (ties and
/// balanced skills hold, no click spam).
pub(crate) fn next_style(bot: &Bot, current: &str) -> Option<&'static str> {
    if current == "controlled" {
        return None;
    }
    let want = weakest_melee(bot);
    (want != current && melee_stat(bot, want) < melee_stat(bot, current)).then_some(want)
}

/// Weakest of attack/strength/defence right now.
pub(crate) fn weakest_melee(bot: &Bot) -> &'static str {

    let (attack, strength, defence) =
        (melee_stat(bot, "attack"), melee_stat(bot, "strength"), melee_stat(bot, "defence"));
    if attack <= strength && attack <= defence {
        "attack"
    } else if strength <= defence {
        "strength"
    } else {
        "defence"
    }
}

/// The style `fight` presses for this category: controlled when the
/// revision offers it, else the weakest skill right now. Shared with the
/// gear planner so the scored bonus is the trained bonus.
pub(crate) fn fight_weapon_style(bot: &Bot, category: &str) -> &'static str {
    if controlled_button(category).is_some() { "controlled" } else { weakest_melee(bot) }
}

/// Combat style buttons per weapon category (player_attackstyles data).
pub(crate) fn style_buttons(category: &str, skill: &str) -> Option<&'static str> {
    match (category, skill) {
        ("weapon_slash", "attack") => Some("combat_hacksword:hack0"),
        ("weapon_slash", "strength") => Some("combat_hacksword:hack1"),
        ("weapon_slash", "defence") => Some("combat_hacksword:hack3"),
        ("weapon_stab", "attack") => Some("combat_stabsword:stab0"),
        ("weapon_stab", "strength") => Some("combat_stabsword:stab1"),
        ("weapon_stab", "defence") => Some("combat_stabsword:stab3"),
        ("weapon_blunt", "attack") => Some("combat_blunt:blunt0"),
        ("weapon_blunt", "strength") => Some("combat_blunt:blunt1"),
        ("weapon_blunt", "defence") => Some("combat_blunt:blunt2"),
        ("weapon_axe", "attack") => Some("combat_axe:axe0"),
        ("weapon_axe", "strength") => Some("combat_axe:axe1"),
        ("weapon_axe", "defence") => Some("combat_axe:axe3"),
        ("weapon_2h_sword", "attack") => Some("combat_heavysword:heavy0"),
        ("weapon_2h_sword", "strength") => Some("combat_heavysword:heavy1"),
        ("weapon_2h_sword", "defence") => Some("combat_heavysword:heavy3"),
        ("weapon_pickaxe", "attack") => Some("combat_pickaxe:pickaxe0"),
        ("weapon_pickaxe", "strength") => Some("combat_pickaxe:pickaxe1"),
        ("weapon_pickaxe", "defence") => Some("combat_pickaxe:pickaxe3"),
        ("weapon_scythe", "attack") => Some("combat_scythe:scythe0"),
        ("weapon_scythe", "strength") => Some("combat_scythe:scythe1"),
        ("weapon_scythe", "defence") => Some("combat_scythe:scythe3"),
        ("weapon_spear", "attack" | "strength") => Some("combat_spear:spear0"),
        ("weapon_spear", "defence") => Some("combat_spear:spear3"),
        ("unarmed", "attack") => Some("combat_unarmed:unarmed0"),
        ("unarmed", "strength") => Some("combat_unarmed:unarmed1"),
        ("unarmed", "defence") => Some("combat_unarmed:unarmed2"),
        _ => None,
    }
}

/// Server attack-style varp (`gamedata/data/varps.json` id 43,
/// transmit=true): the ground truth presses verify against. A sent
/// `if_button` is silently dropped when its component is not visible
/// (engine `IfButtonHandler.ts`) or the player is delayed (normal queue,
/// see player_attackstyles.rs2:1-4), so open-loop tracking locked
/// rotation forever on one dropped press (t02 live: tracker said
/// defence, server stayed aggressive, str 5->12 while def sat at 4).
pub(crate) const COM_MODE_VARP: u16 = 43;

/// Live `%com_mode` index, when the server has transmitted it yet.
pub(crate) fn live_style_index(bot: &Bot) -> Option<i32> {
    bot.world.varps.get(&COM_MODE_VARP).copied()
}

/// com_mode index the button for `skill` should produce: the trailing
/// number of the dotted component (`stab3` -> 3). Single source stays
/// `style_buttons`/`controlled_button`.
pub(crate) fn style_button_index(dotted: &str) -> Option<i32> {
    let tail = dotted.rsplit(':').next()?;
    let digits: String =
        tail.chars().skip_while(|c| !c.is_ascii_digit()).take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// Skill the live `%com_mode` index trains with the wielded category
/// (damagestyle rows of combat.dbrow in button order): blunt, unarmed,
/// staff and polearm tables hold 3 rows, the rest 4; stab/axe/2h
/// sword/pickaxe/scythe index 2 is a second aggressive.
pub(crate) fn live_skill(category: &str, index: i32) -> Option<&'static str> {
    match category {
        "weapon_spear" => match index {
            0 | 1 | 2 => Some("controlled"),
            3 => Some("defence"),
            _ => None,
        },
        "weapon_polearm" => match index {
            0 => Some("controlled"),
            1 => Some("strength"),
            2 => Some("defence"),
            _ => None,
        },
        "weapon_slash" | "weapon_spiked" | "weapon_claws" => match index {
            0 => Some("attack"),
            1 => Some("strength"),
            2 => Some("controlled"),
            3 => Some("defence"),
            _ => None,
        },
        "weapon_blunt" | "unarmed" | "weapon_staff" => match index {
            0 => Some("attack"),
            1 => Some("strength"),
            2 => Some("defence"),
            _ => None,
        },
        _ => match index {
            0 => Some("attack"),
            1 | 2 => Some("strength"),
            3 => Some("defence"),
            _ => None,
        },
    }
}

/// Skill the server is actually training: its `%com_mode` when known,
/// else the last style we asked for. Every kill re-derives this, so a
/// dropped press self-corrects on the next kill instead of pinning the
/// tracker on a style the server never took.
pub(crate) fn current_skill<'a>(bot: &Bot, assumed: &'a str) -> &'a str {
    let category = current_weapon_category(bot);
    live_style_index(bot).and_then(|index| live_skill(&category, index)).unwrap_or(assumed)
}

/// A scimitar's speed beats a 2h sword's raw bonus: one-handed slashers win
/// ties (strategy, not game data).
pub(crate) fn weapon_weight(category: &str) -> f64 {
    match category {
        "weapon_slash" => 1.0,
        "weapon_stab" => 0.9,
        "weapon_axe" | "weapon_blunt" => 0.85,
        "weapon_pickaxe" => 0.7,
        "weapon_2h_sword" | "weapon_spear" => 0.6,
        _ => 0.5,
    }
}

/// Attack bonus the bot's next button actually trains with, per weapon
/// category: the revision ties each combat button to one damagetype
/// (slash/stab/crush), and the bot presses the button for the skill it
/// trains (style_buttons) or the controlled button when the category
/// offers one. Unknown pairs fall back to the best bonus.
/// Sources (read-only): content/scripts/skill_combat/configs/combat.dbrow
/// (weapon_*_table damagestyle/damagetype rows: slash_table is
/// acc/agg/con/def on slash/slash/stab/slash, stab_table acc/agg/agg/def
/// on stab/stab/slash/stab, blunt_table all crush, axe_table
/// acc/agg/agg/def on slash/slash/crush/slash, pickaxe_table on
/// stab/stab/crush/stab, spear_table all controlled on
/// stab/slash/crush/stab, spiked_table acc/agg/con/def on
/// crush/crush/stab/crush, claws_table like slash, polearm_table
/// con/agg/def on stab/slash/stab, unarmed/staff all crush) crossed with
/// content/.../player/player_attackstyles.rs2 (button suffix -> style).
pub(crate) fn style_damage_key(category: &str, style: &str) -> Option<&'static str> {
    match (category, style) {
        ("weapon_slash" | "weapon_claws", "controlled") => Some("stab"),
        ("weapon_slash" | "weapon_claws", _) => Some("slash"),
        ("weapon_stab", _) => Some("stab"),
        ("weapon_blunt", _) => Some("crush"),
        ("weapon_axe" | "weapon_2h_sword", _) => Some("slash"),
        ("weapon_scythe", "strength") => Some("stab"),
        ("weapon_scythe", _) => Some("slash"),
        ("weapon_pickaxe", _) => Some("stab"),
        ("weapon_spear", _) => Some("stab"),
        ("weapon_spiked", "controlled") => Some("stab"),
        ("weapon_polearm", "controlled") => Some("stab"),
        ("weapon_polearm", "defence") => Some("stab"),
        ("weapon_polearm", _) => Some("slash"),
        ("unarmed", _) => Some("crush"),
        _ => None,
    }
}

/// Press the button for `skill` and confirm the server took it: true
/// when live `%com_mode` matches the button's index. A dropped press
/// (invisible tab, delay queue) is retried twice, then logged loudly so
/// the next kill retries instead of training the wrong skill in
/// silence. False when the pair has no button at all (also logged).
pub(crate) fn set_style(bot: &mut Bot, skill: &str) -> Result<bool, ScriptError> {
    let category = current_weapon_category(bot);
    let dotted = if skill == "controlled" {
        controlled_button(&category)
    } else {
        style_buttons(&category, skill)
    };
    let Some(dotted) = dotted else {
        bot.log(&format!("no {skill} button for {category}, holding server style"));
        return Ok(false);
    };
    let Some(com) = (bot.tables.com_id)(dotted) else {
        bot.log(&format!("{dotted} missing from component table, holding server style"));
        return Ok(false);
    };
    let want = style_button_index(dotted);
    for _ in 0..3 {
        bot.if_button(com)?;
        bot.pump(3)?;
        if want.is_none() || live_style_index(bot) == want {
            return Ok(true);
        }
    }
    bot.log(&format!(
        "style {skill} ({dotted}) not confirmed (live={:?} want={want:?}), next kill retries",
        live_style_index(bot)
    ));
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::{controlled_button, current_skill, live_skill, live_style_index, melee_stat, next_style, style_button_index, style_buttons, style_damage_key, weakest_melee, COM_MODE_VARP};
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    /// Offline bot with attack/defence/strength levels set.
    fn stat_bot(attack: u8, defence: u8, strength: u8) -> Bot {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.stats.insert(0, (attack, 0));
        bot.world.stats.insert(1, (defence, 0));
        bot.world.stats.insert(2, (strength, 0));
        bot
    }

    /// t01's live state (atk 10, str/def 1): the laggard trains, never the
    /// leader. Previously the blind 3-kill rotation sat on accurate.
    #[test]
    fn weakest_melee_picks_laggard() {
        let bot = stat_bot(10, 1, 1);
        assert_eq!(weakest_melee(&bot), "strength");
        assert_eq!(melee_stat(&bot, "attack"), 10);
        assert_eq!(next_style(&bot, "attack"), Some("strength"));
        assert_eq!(next_style(&bot, "strength"), None, "already training the weakest: hold");
        assert_eq!(next_style(&bot, "controlled"), None, "controlled never rotates");
    }

    /// Balanced skills hold the current style (no click spam on ties), a
    /// one-level gap rotates to the laggard.
    #[test]
    fn rotation_holds_ties_switches_on_gap() {
        let tied = stat_bot(5, 5, 5);
        assert_eq!(next_style(&tied, "attack"), None);
        let gapped = stat_bot(6, 5, 5);
        assert_eq!(next_style(&gapped, "attack"), Some("strength"), "5 < 6 trains first");
        assert_eq!(next_style(&gapped, "strength"), None, "weakest held");
    }

    /// Controlled exists exactly where the revision's combat.dbrow puts a
    /// ^style_melee_controlled entry (slash/spear/polearm/spiked/claws);
    /// unarmed and plain axes have none.
    #[test]
    fn controlled_only_where_the_revision_has_it() {
        assert_eq!(controlled_button("weapon_slash"), Some("combat_hacksword:hack2"));
        assert_eq!(controlled_button("weapon_spear"), Some("combat_spear:spear0"));
        assert_eq!(controlled_button("weapon_polearm"), Some("combat_polearm:polearm0"));
        assert_eq!(controlled_button("unarmed"), None);
        assert_eq!(controlled_button("weapon_axe"), None);
        assert_eq!(controlled_button("weapon_blunt"), None);
    }

    /// Style map covers the buttons the bot presses (dbrow damagetypes x
    /// player_attackstyles suffixes); unknown categories fall back to max.
    #[test]
    fn style_damage_follows_the_pressed_button() {
        assert_eq!(style_damage_key("weapon_stab", "attack"), Some("stab"));
        assert_eq!(style_damage_key("weapon_blunt", "defence"), Some("crush"));
        assert_eq!(style_damage_key("weapon_axe", "strength"), Some("slash"));
        assert_eq!(style_damage_key("weapon_pickaxe", "attack"), Some("stab"));
        assert_eq!(style_damage_key("weapon_spear", "attack"), Some("stab"));
        assert_eq!(style_damage_key("weapon_polearm", "controlled"), Some("stab"));
        assert_eq!(style_damage_key("unarmed", "defence"), Some("crush"));
        assert_eq!(style_damage_key("weapon_bow", "attack"), None, "ranged: max fallback");
    }

    /// Every pressed button resolves to the com_mode index the server
    /// takes (dbrow row order x player_attackstyles suffixes): the
    /// verifier compares against this, never a second table.
    #[test]
    fn button_index_matches_pressed_button() {
        let cases = [
            ("weapon_slash", "attack", 0),
            ("weapon_slash", "strength", 1),
            ("weapon_slash", "defence", 3),
            ("weapon_stab", "attack", 0),
            ("weapon_stab", "strength", 1),
            ("weapon_stab", "defence", 3),
            ("weapon_blunt", "attack", 0),
            ("weapon_blunt", "strength", 1),
            ("weapon_blunt", "defence", 2),
            ("weapon_axe", "attack", 0),
            ("weapon_axe", "strength", 1),
            ("weapon_axe", "defence", 3),
            ("weapon_2h_sword", "attack", 0),
            ("weapon_2h_sword", "strength", 1),
            ("weapon_2h_sword", "defence", 3),
            ("weapon_pickaxe", "attack", 0),
            ("weapon_pickaxe", "strength", 1),
            ("weapon_pickaxe", "defence", 3),
            ("weapon_scythe", "attack", 0),
            ("weapon_scythe", "strength", 1),
            ("weapon_scythe", "defence", 3),
            ("weapon_spear", "attack", 0),
            ("weapon_spear", "strength", 0),
            ("weapon_spear", "defence", 3),
            ("unarmed", "attack", 0),
            ("unarmed", "strength", 1),
            ("unarmed", "defence", 2),
        ];
        for (category, skill, want) in cases {
            let dotted = style_buttons(category, skill).unwrap_or_else(|| panic!("no button for {category}/{skill}"));
            assert_eq!(style_button_index(dotted), Some(want), "{dotted}");
        }
        assert_eq!(style_button_index("combat_hacksword:hack2"), Some(2), "controlled slash");
        assert_eq!(style_button_index("combat_spear:spear0"), Some(0), "controlled spear");
        assert_eq!(style_button_index("combat_polearm:polearm0"), Some(0), "controlled polearm");
        assert_eq!(style_button_index("combat_x:foo"), None, "no trailing number");
    }


    /// Live index decodes to the trained skill per revision tables:
    /// stab/axe/2h/pickaxe/scythe 2 is a second aggressive, spear is
    /// all-controlled but its defensive, blunt/unarmed/staff stop at 2.
    #[test]
    fn live_skill_follows_dbrow_tables() {
        assert_eq!(live_skill("weapon_stab", 2), Some("strength"), "second aggressive");
        assert_eq!(live_skill("weapon_stab", 3), Some("defence"));
        assert_eq!(live_skill("weapon_spear", 1), Some("controlled"));
        assert_eq!(live_skill("weapon_spear", 3), Some("defence"));
        assert_eq!(live_skill("weapon_polearm", 0), Some("controlled"));
        assert_eq!(live_skill("weapon_polearm", 2), Some("defence"));
        assert_eq!(live_skill("weapon_slash", 2), Some("controlled"));
        assert_eq!(live_skill("weapon_blunt", 2), Some("defence"));
        assert_eq!(live_skill("unarmed", 2), Some("defence"));
        assert_eq!(live_skill("unarmed", 0), Some("attack"));
        assert_eq!(live_skill("weapon_stab", 9), None, "out of table");
    }

    /// t02 live lock (assumed defence, server aggressive, atk5/str12/def4
    /// bare-fisted): trusting the live varp re-arms the rotation to the
    /// real laggard instead of holding a style never taken.
    #[test]
    fn live_varp_unsticks_a_dropped_press() {
        let mut bot = stat_bot(5, 4, 12);
        bot.world.varps.insert(COM_MODE_VARP, 1);
        assert_eq!(live_style_index(&bot), Some(1));
        assert_eq!(current_skill(&bot, "defence"), "strength", "server trains strength, not the assumed defence");
        assert_eq!(next_style(&bot, current_skill(&bot, "defence")), Some("defence"), "real laggard re-armed");
        let mut quiet = stat_bot(5, 5, 5);
        assert_eq!(current_skill(&quiet, "attack"), "attack", "no varp yet: keep assumed");
        quiet.world.varps.insert(COM_MODE_VARP, 3);
        assert_eq!(current_skill(&quiet, "attack"), "attack", "index 3 is out of the unarmed table: keep assumed");
    }
}
