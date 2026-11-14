//! Water : récipients d'eau portés comme des outils. Un mélange à l'eau
//! (`pottery.rs2` `make_softclay` : argile + seau/cruche/bol d'eau →
//! argile molle + récipient vide) rend le récipient vide ; la brique le
//! remplit à la source d'eau la plus proche (fontaine, évier ; puits pour
//! les seaux, `water_sources.rs2`) puis mélange de nouveau, jusqu'à ce
//! que l'intrant soit épuisé. Une poignée de récipients sert donc toute
//! une charge (le planner n'en demande que `POTTERY_VESSELS`).
//!
//! Live 2026-11-14 (harry_page, will_field, sara_miller) : `pottery`
//! retirait l'argile, ne trouvait pas d'eau, ne mélangeait rien et
//! redéposait l'argile ~1400 fois en 30 min. Sans aucun récipient (sac +
//! banque connue), l'activité lève `Blocked{bucket_water}` : le cerveau
//! obtient l'eau (seaux : ils se remplissent aussi aux puits, celui
//! d'Edgeville est à 10 cases de la banque ; les fontaines de la
//! cruche les plus proches d'Edgeville sont au monastère, que la route
//! rejoint par le niveau 1 de la Wilderness), jamais une boucle muette.

#![forbid(unsafe_code)]

use lifescape_sdk::Bot;

use crate::activities::fill_water::water_stations;
use crate::kit::pack::{banked_usable, obj_id, pack_total};
use crate::kit::produce::{self, Produce, ProduceJob, ProduceSite, ProduceUi};
use crate::quests::common::inv_slot;
use crate::ScriptError;

/// Vides et pleins mélangeables, du plus utile au moins utile (le seau se
/// remplit aussi aux puits).
pub const VESSELS: [(&str, &str); 3] = [("bucket_empty", "bucket_water"), ("jug_empty", "jug_water"), ("bowl_empty", "bowl_water")];
/// Nom que porte un `Blocked` sans aucun récipient (le seau : puits,
/// fontaines et éviers le remplissent).
pub const MISSING_WATER: &str = "bucket_water";
/// Ticks d'attente d'un envoi de mélange (le serveur répond en 2 ticks),
/// envois au plus (un envoi pendant un `p_delay` est jeté sans un mot).
const MIX_TICKS: u64 = 6;
const MIX_SENDS: u32 = 5;

/// Ids (vide, plein) des récipients connus des tables.
pub fn vessel_ids(bot: &Bot) -> Result<Vec<(u16, u16)>, ScriptError> {
    VESSELS.iter().map(|(empty, full)| Ok((obj_id(bot, empty)?, obj_id(bot, full)?))).collect()
}

/// Récipients portés (vides + pleins).
pub fn carried(bot: &Bot, vessels: &[(u16, u16)]) -> u32 {
    vessels.iter().map(|(empty, full)| pack_total(bot, *empty) + pack_total(bot, *full)).sum()
}

/// Récipients pleins portés.
pub fn carried_water(bot: &Bot, vessels: &[(u16, u16)]) -> u32 {
    vessels.iter().map(|(_, full)| pack_total(bot, *full)).sum()
}

/// Ids à retirer de la banque pour compléter `wanted` récipients : pleins
/// d'abord, puis vides, dans l'ordre de `VESSELS`, bornés par la banque
/// connue.
pub fn withdraw_plan(bot: &Bot, vessels: &[(u16, u16)], wanted: u32) -> Vec<(u16, u32)> {
    let mut short = wanted.saturating_sub(carried(bot, vessels));
    let mut plan: Vec<(u16, u32)> = Vec::new();
    for id in vessels.iter().map(|(_, full)| *full).chain(vessels.iter().map(|(empty, _)| *empty)) {
        let take = banked_usable(bot, id).min(short);
        if take > 0 {
            plan.push((id, take));
            short -= take;
        }
    }
    plan
}

/// True quand aucun récipient n'est ni porté ni en banque connue.
pub fn none_held(bot: &Bot, vessels: &[(u16, u16)]) -> bool {
    carried(bot, vessels) == 0 && vessels.iter().all(|(empty, full)| banked_usable(bot, *empty) == 0 && banked_usable(bot, *full) == 0)
}

/// Sortes de sources d'eau, la plus proche de `near` d'abord (le poste
/// prend la première sorte trouvée : sans ce tri, un seau d'Edgeville
/// allait à la fontaine du monastère, 43 cases, au lieu du puits, 21).
pub fn nearest_kinds(kinds: &[&'static str], near: (i32, i32)) -> Vec<&'static str> {
    let mut sorted = kinds.to_vec();
    sorted.sort_by_key(|kind| kind_distance(kind, near));
    sorted
}

/// Distance de Manhattan de `near` à la plus proche source de cette sorte.
fn kind_distance(kind: &str, near: (i32, i32)) -> i64 {
    lifescape_gamedata::stations::stations_of(kind).iter().map(|station| (station.x - i64::from(near.0)).abs() + (station.z - i64::from(near.1)).abs()).min().unwrap_or(i64::MAX)
}

/// Remplit les vides portés d'UNE sorte de récipient : celle dont la
/// source est la plus proche de `near` (jamais autour du site assigné :
/// la source sert la banque). Le tour suivant de `mix_all` revient à la
/// même source : 3 seaux au puits d'Edgeville battent 7 cruches dont la
/// fontaine (monastère) ne se rejoint que par la Wilderness.
pub fn refill(bot: &mut Bot, vessels: &[(u16, u16)], near: (i32, i32)) -> Result<u32, ScriptError> {
    let mut carried_kinds: Vec<(i64, u16, u16, &'static str)> = vessels
        .iter()
        .zip(VESSELS.iter())
        .filter(|((empty, _), _)| pack_total(bot, *empty) > 0)
        .map(|((empty, full), (empty_name, _))| {
            let distance = water_stations(empty_name).iter().map(|kind| kind_distance(kind, near)).min().unwrap_or(i64::MAX);
            (distance, *empty, *full, *empty_name)
        })
        .collect();
    carried_kinds.sort_by_key(|(distance, ..)| *distance);
    let Some((_, empty, full, empty_name)) = carried_kinds.first().copied() else { return Ok(0) };
    let kinds = nearest_kinds(water_stations(empty_name), near);
    let spec = Produce { verb: "filling", site: ProduceSite::Loc(&kinds), ui: ProduceUi::Direct };
    let job = ProduceJob { spec: &spec, input: empty, input_name: empty_name, want: 0, products: vec![full], near };
    let site = bot.assigned_site.take();
    let outcome = produce::run_job(bot, &job);
    bot.assigned_site = site;
    Ok(outcome?.made)
}

/// Utilise `input` sur un récipient plein jusqu'à épuisement de l'un des
/// deux (clic inventaire direct, aucune interface). Rend les `product`
/// faits ; un mélange sans effet est une erreur typée.
pub fn mix(bot: &mut Bot, input: u16, vessels: &[(u16, u16)], product: u16) -> Result<u32, ScriptError> {
    let start = pack_total(bot, product);
    while pack_total(bot, input) > 0 {
        let Some(full) = vessels.iter().map(|(_, full)| *full).find(|full| pack_total(bot, *full) > 0) else { break };
        let had = pack_total(bot, product);
        // `OpHeldUHandler` drops a use sent while the player is delayed
        // (the fill's or the previous mix's `p_delay`), without a word:
        // resend after a short window, bounded (live 2026-11-14 admin, the
        // first mix after filling 7 jugs never landed).
        for _ in 0..MIX_SENDS {
            let (com, slot) = inv_slot(bot, input)?;
            let (use_com, use_slot) = inv_slot(bot, full)?;
            bot.op_held_u(input, slot, com, full, use_slot, use_com)?;
            let deadline = bot.world.ticks + MIX_TICKS;
            while bot.world.ticks < deadline && pack_total(bot, product) == had {
                if bot.world.modal.is_some() || !bot.chat_options().is_empty() || bot.world.count_dialog {
                    return Err(ScriptError::Failed(format!("mixing opened an interface at {:?}", bot.position())));
                }
                bot.pump(1)?;
            }
            if pack_total(bot, product) > had {
                break;
            }
        }
        if pack_total(bot, product) == had {
            return Err(ScriptError::Failed(format!("mixing with water made nothing in {MIX_SENDS} sends at {:?}", bot.position())));
        }
    }
    Ok(pack_total(bot, product).saturating_sub(start))
}

/// Mélange tout l'intrant porté : eau portée d'abord, puis remplissages
/// à la source la plus proche de `near` tant qu'il reste de l'intrant et
/// des récipients. Chaque tour doit faire au moins un produit.
pub fn mix_all(bot: &mut Bot, input: u16, vessels: &[(u16, u16)], product: u16, near: (i32, i32)) -> Result<u32, ScriptError> {
    let mut made = 0;
    loop {
        made += mix(bot, input, vessels, product)?;
        if pack_total(bot, input) == 0 || carried(bot, vessels) == 0 {
            return Ok(made);
        }
        if refill(bot, vessels, near)? == 0 {
            return Err(ScriptError::Failed(format!("no water vessel filled near {near:?} at {:?}", bot.position())));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{none_held, vessel_ids, withdraw_plan, VESSELS};
    use crate::kit::pack::obj_id;
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    fn bot_with_bank(banked: &[(&str, u32)]) -> Bot {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        let com = (bot.tables.com_id)("bank_main:inv").expect("bank com");
        let counts = banked.iter().map(|(name, count)| (obj_id(&bot, name).expect("obj"), *count)).collect();
        bot.world.seed_closed_inv(com, &counts);
        bot
    }

    /// Live 2026-11-14 (admin, Edgeville) : les seaux allaient à la
    /// fontaine du monastère (43 cases) ; le puits d'Edgeville (21) passe
    /// d'abord. Les cruches, refusées au puits, gardent la fontaine.
    #[test]
    fn the_nearest_water_kind_goes_first() {
        use crate::activities::fill_water::water_stations;
        let edgeville = (3095, 3489);
        assert_eq!(super::nearest_kinds(water_stations("bucket_empty"), edgeville)[0], "well");
        assert_eq!(super::nearest_kinds(water_stations("jug_empty"), edgeville)[0], "fountain");
    }

    /// Pleins d'abord (aucun trajet à la source), puis vides, bornés par
    /// la banque ; sans aucun récipient, l'activité bloque sur l'eau.
    #[test]
    fn vessels_come_full_first_then_empty() {
        let bot = bot_with_bank(&[("jug_water", 2), ("bucket_empty", 9)]);
        let vessels = vessel_ids(&bot).expect("vessel names resolve");
        assert_eq!(vessels.len(), VESSELS.len());
        let jug = obj_id(&bot, "jug_water").expect("jug");
        let bucket = obj_id(&bot, "bucket_empty").expect("bucket");
        assert_eq!(withdraw_plan(&bot, &vessels, 7), vec![(jug, 2), (bucket, 5)]);
        assert!(!none_held(&bot, &vessels));
        let dry = bot_with_bank(&[("clay", 20)]);
        assert!(none_held(&dry, &vessels), "no vessel anywhere: blocked on water");
        assert!(withdraw_plan(&dry, &vessels, 7).is_empty());
    }
}
