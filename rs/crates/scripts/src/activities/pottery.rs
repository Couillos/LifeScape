//! Pottery: clay and water into soft clay (inventory), shaped on a
//! wheel then fired in an oven (catalogue §5.6, P2). Wheel = the
//! `skill_multi3` grid (`makex_a/b/c` = pot, pie dish, bowl, all carried),
//! firing is Direct (one unfired per click, cracks lose the pot).
//!
//! Charge (2026-11-14, boucle muette harry_page/will_field/sara_miller :
//! argile retirée puis redéposée ~1400 fois, aucune eau, « ok load » sans
//! un plat) : banque du `--site` (sinon la plus proche), dépôt hors
//! garde, retrait de `VESSELS_KEPT` récipients d'eau (pleins d'abord) puis
//! de l'argile pour le reste du sac ; aucun récipient ou aucune argile ⇒
//! `Blocked` qui nomme les deux à la fois. Mélange à la source d'eau la
//! plus proche de la banque (`kit::water`, récipients remplis tant qu'il
//! reste de l'argile), tour puis four du site ; un ordre sur la pièce
//! crue (`piedish_unfired`) s'arrête à la tour. « ok load » seulement
//! quand la charge a fait quelque chose ; une charge vide est une erreur.
//!
//! Process: poste tour (6) + four (2) + source d'eau · action mélanger/
//! façonner/cuire · interface directe + grille + directe · intrants
//! argile + eau · produits cru + cuit.

#![forbid(unsafe_code)]

use lifescape_gamedata::extra_recipes::ExtraRecipe;
use lifescape_sdk::Bot;

use crate::kit::bank::{bank_trip, nearest_bank};
use crate::kit::cycle::{resolve_keep, run, StepControl};
use crate::kit::pack::{banked_usable, free_slots, obj_id, pack_total};
use crate::kit::produce::{self, Produce, ProduceJob, ProduceSite, ProduceUi};
use crate::kit::survival::{check_events, is_death_abort, learn_level, wait_pack_streamed};
use crate::kit::water::{self, MISSING_WATER};
use crate::{ScriptError, ScriptResult};

pub const CRAFTING_STAT: u8 = 12;
pub const CLAY: &str = "clay";
pub const WATER: &str = "jug_water";
pub const SOFT: &str = "softclay";
pub const UNFIRED: &str = "pot_unfired";
pub const PRODUCT: &str = "pot_empty";
pub const WHEELS: [&str; 1] = ["potterywheel"];
pub const OVENS: [&str; 1] = ["potteryoven"];
/// Water vessels a load carries (refilled between mixes, `kit::water`):
/// the rest of the pack is clay. Same count as the planner's
/// `replan::POTTERY_VESSELS`.
pub const VESSELS_KEPT: u32 = 7;

/// Brain products: fired name, unfired name, wheel button (`pottery.rs2`
/// `~skill_multi3` order: pot, pie dish, bowl).
pub const SHAPES: [(&str, &str, &str); 3] = [
    ("pot_empty", "pot_unfired", "skill_multi3:makex_a"),
    ("piedish", "piedish_unfired", "skill_multi3:makex_b"),
    ("bowl_empty", "bowl_unfired", "skill_multi3:makex_c"),
];

/// Shape row by fired or unfired product name.
pub fn shape_row(name: &str) -> Option<(&'static str, &'static str, &'static str)> {
    SHAPES.iter().find(|(fired, unfired, _)| fired.eq_ignore_ascii_case(name) || unfired.eq_ignore_ascii_case(name)).copied()
}

/// True when the order wants the fired piece (an unfired name stops at
/// the wheel: firing it would eat the very item the `--want` counts).
pub fn fires(resource: Option<&str>, unfired: &str) -> bool {
    resource.is_none_or(|name| !name.eq_ignore_ascii_case(unfired))
}
const FIRE: Produce<'static> = Produce { verb: "firing", site: ProduceSite::Loc(&OVENS), ui: ProduceUi::Direct };

/// Extra row whose outputs contain `name` for this `kind`.
fn extra_row(kind: &str, name: &str) -> Option<ExtraRecipe> {
    lifescape_gamedata::extra_recipes::by_kind(kind).into_iter().find(|entry| entry.outputs.iter().any(|(output, _)| output == name))
}

/// Run one produce job: `Ok(false)` after a death (restock and retry).
fn work_job(bot: &mut Bot, job: &ProduceJob<'_>, what: &str) -> Result<bool, ScriptError> {
    match produce::run_job(bot, job) {
        Err(error) if is_death_abort(&error) => {
            bot.log(&format!("died {what}, restocking to retry"));
            Ok(false)
        }
        Err(error) => Err(error),
        Ok(_) => Ok(true),
    }
}

/// Pottery loads in the open session: `pottery [product] [count]` (pot
/// when omitted; `piedish`, `bowl_empty`, or an unfired name to stop at
/// the wheel; 0 = forever), `--site` = the wheel and oven.
pub fn run_pottery(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let (resource, rest) = crate::kit::cycle::split_resource(args);
    let spec = crate::kit::cycle::parse_cli("pottery", &rest)?;
    // Soft clay itself (`pottery:softclay`, water on clay in the pack):
    // the mix alone, banked loads (live admin 2026-11-15: "no F2P
    // pottery product softclay").
    if resource.as_deref() == Some(SOFT) {
        return run_softclay(bot, &spec);
    }
    let (product_name, unfired_name, button) = shape_row(resource.as_deref().unwrap_or(PRODUCT)).ok_or_else(|| ScriptError::Usage(format!("no F2P pottery product {resource:?}")))?;
    let firing = fires(resource.as_deref(), unfired_name);
    let made_name = if firing { product_name } else { unfired_name };
    let shape_spec = Produce { verb: "shaping", site: ProduceSite::Loc(&WHEELS), ui: ProduceUi::Grid { button } };
    if spec.allow_paid {
        bot.allow_paid = true;
        bot.log("toll gates allowed for this run");
    }
    bot.allow_shop = spec.allow_shop;
    bot.assigned_site = spec.site;
    let task_keep = resolve_keep(bot, &spec)?;
    wait_pack_streamed(bot)?;
    check_events(bot)?;
    let level = learn_level(bot, CRAFTING_STAT, 1);
    let shape = extra_row("pottery_shape", unfired_name).expect("shape row placed");
    let fire = extra_row("pottery_fire", product_name).expect("fire row placed");
    let needed = if firing { shape.level.max(fire.level) } else { shape.level };
    if level < needed {
        return Err(crate::kit::cycle::blocked_level("crafting", i64::from(needed)));
    }
    let (clay, soft, unfired, fired) = (obj_id(bot, CLAY)?, obj_id(bot, SOFT)?, obj_id(bot, unfired_name)?, obj_id(bot, product_name)?);
    let made_id = if firing { fired } else { unfired };
    let vessels = water::vessel_ids(bot)?;
    let bank = match crate::kit::site::site_bank(bot) {
        Some(site_bank) => site_bank,
        None => nearest_bank(bot)?,
    };
    bot.log(&format!("bank at ({},{}), making {made_name}", bank.x, bank.z));
    let mut keep: Vec<u16> = vessels.iter().flat_map(|(empty, full)| [*empty, *full]).collect();
    keep.extend([clay, soft]);
    if firing {
        keep.push(unfired);
    }
    keep.extend(task_keep.iter().copied());
    // Banked pieces past the mix finish first (no water, no wheel trip
    // for the unfired ones of a fired order).
    let ready: Vec<u16> = if firing { vec![unfired, soft] } else { vec![soft] };
    run(bot, &spec, |step_bot, state| {
        check_events(step_bot)?;
        if pack_total(step_bot, clay) + pack_total(step_bot, soft) + pack_total(step_bot, unfired) == 0 {
            load(step_bot, &bank, &keep, clay, &ready, &vessels)?;
        }
        let near = (bank.x as i32, bank.z as i32);
        if pack_total(step_bot, clay) > 0 {
            let mixed = water::mix_all(step_bot, clay, &vessels, soft, near)?;
            step_bot.log(&format!("mixed {mixed} {SOFT}"));
        }
        let before = pack_total(step_bot, made_id);
        if pack_total(step_bot, soft) > 0 {
            let job = ProduceJob { spec: &shape_spec, input: soft, input_name: SOFT, want: 0, products: vec![unfired], near };
            if !work_job(step_bot, &job, "shaping")? {
                return Ok(StepControl::Continue);
            }
        }
        if firing && pack_total(step_bot, unfired) > 0 {
            let job = ProduceJob { spec: &FIRE, input: unfired, input_name: unfired_name, want: 0, products: vec![fired], near };
            if !work_job(step_bot, &job, "firing")? {
                return Ok(StepControl::Continue);
            }
        }
        let made = pack_total(step_bot, made_id).saturating_sub(before);
        if made == 0 {
            return Err(ScriptError::Failed(format!("pottery load made no {made_name} at {:?}", step_bot.position())));
        }
        bank_trip(step_bot, &bank, &keep, 0)?;
        Ok(state.load_done(step_bot, &format!("made {made} {made_name} (crafting={level})")))
    })
}

/// Soft clay loads: clay and water vessels from the bank, mixed in the
/// pack, banked, until the order's count or the clay runs out.
fn run_softclay(bot: &mut Bot, spec: &crate::kit::cycle::TaskSpec) -> ScriptResult {
    bot.allow_shop = spec.allow_shop;
    let task_keep = resolve_keep(bot, spec)?;
    wait_pack_streamed(bot)?;
    check_events(bot)?;
    let (clay, soft) = (obj_id(bot, CLAY)?, obj_id(bot, SOFT)?);
    let vessels = water::vessel_ids(bot)?;
    let bank = nearest_bank(bot)?;
    let mut keep: Vec<u16> = vessels.iter().flat_map(|(empty, full)| [*empty, *full]).collect();
    keep.push(clay);
    keep.extend(task_keep.iter().copied());
    run(bot, spec, |step_bot, state| {
        check_events(step_bot)?;
        if pack_total(step_bot, clay) == 0 {
            load(step_bot, &bank, &keep, clay, &[], &vessels)?;
        }
        let mixed = water::mix_all(step_bot, clay, &vessels, soft, (bank.x as i32, bank.z as i32))?;
        if mixed == 0 {
            return Err(ScriptError::Failed(format!("no {SOFT} mixed at {:?}", step_bot.position())));
        }
        bank_trip(step_bot, &bank, &keep, 0)?;
        Ok(state.load_done(step_bot, &format!("mixed {mixed} {SOFT}")))
    })
}

/// One load at the bank, one visit: deposit all but the keep-list, take
/// the banked pieces past the mix (`ready`), then water vessels (full
/// first) and clay for the rest of the pack. Nothing to work ⇒ `Blocked`
/// naming clay and water together.
fn load(bot: &mut Bot, bank: &lifescape_gamedata::banks::Bank, keep: &[u16], clay: u16, ready: &[u16], vessels: &[(u16, u16)]) -> Result<(), ScriptError> {
    bank_trip(bot, bank, keep, 0)?;
    let mut room = free_slots(bot) as u32;
    let mut wants: Vec<(u16, u32)> = Vec::new();
    for piece in ready {
        let take = banked_usable(bot, *piece).min(room);
        if take > 0 {
            wants.push((*piece, take));
            room -= take;
        }
    }
    let clay_held = pack_total(bot, clay) + banked_usable(bot, clay);
    if wants.is_empty() {
        blocked_unless(bot, clay_held > 0, !water::none_held(bot, vessels))?;
    }
    if room > 1 && clay_held > 0 && !water::none_held(bot, vessels) {
        let plan = water::withdraw_plan(bot, vessels, VESSELS_KEPT.min(room / 2));
        room = room.saturating_sub(plan.iter().map(|(_, count)| *count).sum());
        wants.extend(plan);
        wants.push((clay, room.min(banked_usable(bot, clay))));
    }
    crate::kit::withdraw::bank_withdraw_counts(bot, bank, &wants)?;
    if ready.iter().any(|piece| pack_total(bot, *piece) > 0) {
        return Ok(());
    }
    blocked_unless(bot, pack_total(bot, clay) > 0, water::carried(bot, vessels) > 0)
}

/// `Blocked` naming every missing ingredient at once (clay, water).
fn blocked_unless(bot: &Bot, has_clay: bool, has_water: bool) -> Result<(), ScriptError> {
    let mut missing: Vec<String> = Vec::new();
    if !has_clay {
        missing.push(CLAY.to_string());
    }
    if !has_water {
        missing.push(MISSING_WATER.to_string());
    }
    if missing.is_empty() {
        return Ok(());
    }
    bot.log(&format!("pottery blocked: missing {}", missing.join(", ")));
    Err(ScriptError::Blocked { missing })
}

#[cfg(test)]
mod tests {
    use super::{extra_row, CLAY, OVENS, PRODUCT, SOFT, UNFIRED, WATER, WHEELS};
    use crate::kit::pack::obj_id;
    use crate::kit::produce::grid_button;
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    fn offline_bot() -> Bot {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot
    }

    /// Live 2026-11-14: `pottery piedish_unfired --want piedish_unfired
    /// 16` fired every dish it shaped, so the counted item never grew.
    /// An unfired order stops at the wheel; a fired (or no) name fires.
    #[test]
    fn an_unfired_order_stops_at_the_wheel() {
        assert!(!super::fires(Some("piedish_unfired"), "piedish_unfired"));
        assert!(super::fires(Some("piedish"), "piedish_unfired"));
        assert!(super::fires(None, "pot_unfired"), "bare pottery fires pots");
    }

    /// Catalogue §5.6 pot chain from the dumps: mix (no XP), shape pot 63,
    /// fire pot 63; dishes and bowls sit higher.
    #[test]
    fn pot_chain_comes_from_gamedata() {
        let mix = extra_row("pottery_shape", SOFT).expect("mix row");
        assert_eq!((mix.level, mix.xp), (1, 0), "mix pays nothing");
        assert!(mix.inputs.iter().any(|(name, _)| name == CLAY));
        assert!(mix.inputs.iter().any(|(name, _)| name == WATER));
        let shape = extra_row("pottery_shape", UNFIRED).expect("shape row");
        assert_eq!((shape.level, shape.xp), (1, 63), "shape pot");
        assert_eq!(shape.station.as_deref(), Some("potterywheel"));
        let fire = extra_row("pottery_fire", PRODUCT).expect("fire row");
        assert_eq!((fire.level, fire.xp), (1, 63), "fire pot");
        assert_eq!(fire.station.as_deref(), Some("potteryoven"));
        assert!(extra_row("pottery_shape", "bowl_unfired").is_some(), "bowls wait for later");
    }

    /// The brain's product picks its wheel button and rows (pot a, pie
    /// dish b, bowl c in `pottery.rs2`); every row exists in the dumps.
    #[test]
    fn brain_products_map_to_rows_and_buttons() {
        assert_eq!(super::shape_row("piedish"), Some(("piedish", "piedish_unfired", "skill_multi3:makex_b")));
        assert_eq!(super::shape_row("bowl_empty").map(|row| row.2), Some("skill_multi3:makex_c"));
        assert!(super::shape_row("vase").is_none(), "members pottery is no row");
        let bot = offline_bot();
        for (fired, unfired, button) in super::SHAPES {
            assert!(extra_row("pottery_shape", unfired).is_some() && extra_row("pottery_fire", fired).is_some(), "{fired} rows");
            assert!(grid_button(&bot, button).is_ok(), "{button} resolves");
        }
    }

    /// Six wheels and two ovens stand free; the grid button and every
    /// pot name resolve through live tables.
    #[test]
    fn wheels_ovens_buttons_and_names_resolve() {
        assert_eq!(lifescape_gamedata::stations::stations_of(WHEELS[0]).len(), 6, "wheels");
        assert_eq!(lifescape_gamedata::stations::stations_of(OVENS[0]).len(), 2, "ovens");
        let bot = offline_bot();
        assert!(grid_button(&bot, "skill_multi3:makex_a").is_ok(), "pot make-x resolves");
        for name in [CLAY, WATER, SOFT, UNFIRED, PRODUCT] {
            assert!(obj_id(&bot, name).is_ok(), "{name} resolves");
        }
    }
}
