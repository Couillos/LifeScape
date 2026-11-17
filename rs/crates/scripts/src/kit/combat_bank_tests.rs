//! Tests du voyage banque de combat (relevés live 2026-11-14).

use std::collections::HashMap;

use lifescape_sdk::Bot;

use super::{combat_keep_ids, food_load, kept_food_surplus, kit_to_withdraw, pack_surplus, planned_kit, trip_verdict, GearPlan, HUNT_MIN_FREE};
use crate::kit::gear::{gear_plan, owned_gear, WORN_COM_NAME};
use crate::kit::pack::obj_id;
use crate::kit::style::{fight_weapon_style, style_damage_key};
use crate::tables_live::attach_live_tables;

/// Bot hors ligne : niveaux (attaque, défense, force, distance), style.
fn fighter(attack: u8, defence: u8, strength: u8, ranged: u8, style: &str) -> Bot {
    let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
    attach_live_tables(&mut bot);
    bot.world.stats.insert(0, (attack, 0));
    bot.world.stats.insert(1, (defence, 0));
    bot.world.stats.insert(2, (strength, 0));
    bot.world.stats.insert(4, (ranged, 0));
    bot.combat_style = style.to_string();
    bot
}

fn id(bot: &Bot, name: &str) -> u16 {
    obj_id(bot, name).unwrap_or_else(|_| panic!("{name} resolves"))
}

fn stackable(name: &str) -> bool {
    lifescape_gamedata::objs_by_name(name).first().is_some_and(|obj| obj.stackable)
}

/// Sac : une case par unité d'un objet non empilable.
fn fund_pack(bot: &mut Bot, items: &[(&str, u32)]) {
    let com = (bot.tables.com_id)("inventory:inv").expect("pack com");
    let mut slots: HashMap<u8, (u16, u32)> = HashMap::new();
    for (name, units) in items {
        let obj = id(bot, name);
        if stackable(name) {
            slots.insert(slots.len() as u8, (obj, *units));
        } else {
            for _ in 0..*units {
                slots.insert(slots.len() as u8, (obj, 1));
            }
        }
    }
    bot.world.inv.insert(com, slots);
    bot.world.inv_cap.insert(com, 28);
}

fn wear(bot: &mut Bot, worn: &[(i64, &str, u32)]) {
    let com = (bot.tables.com_id)(WORN_COM_NAME).expect("worn com");
    let slots: HashMap<u8, (u16, u32)> = worn.iter().map(|(slot, name, count)| (*slot as u8, (id(bot, name), *count))).collect();
    bot.world.inv.insert(com, slots);
}

/// Banque ouverte (`bank_main:inv` dans `world.inv`).
fn open_bank(bot: &mut Bot, items: &[(&str, u32)]) {
    let com = (bot.tables.com_id)("bank_main:inv").expect("bank com");
    let slots: HashMap<u8, (u16, u32)> = items.iter().enumerate().map(|(slot, (name, count))| (slot as u8, (id(bot, name), *count))).collect();
    bot.world.inv.insert(com, slots);
}

fn plan_of(bot: &Bot) -> GearPlan {
    gear_plan(bot, &owned_gear(bot), fight_weapon_style, style_damage_key)
}

fn names(bot: &Bot, ids: &[u16]) -> Vec<String> {
    let mut out: Vec<String> = ids.iter().map(|obj| lifescape_gamedata::obj_by_id(usize::from(*obj)).and_then(|row| row.name).unwrap_or("?").to_string()).collect();
    out.sort();
    let _ = bot;
    out
}

/// frank_lane (melee, ladder) : le sac porte le butin (19 casques
/// médiums, 5 dagues de fer, 140 flèches de bronze) et 3 choux, le
/// corps la hache de bataille noire. Rien de ce butin n'est gardé au
/// dépôt, rien n'est re-retiré, le bouclier rangé sort à l'unité.
#[test]
fn melee_trip_keeps_no_looted_gear() {
    let mut bot = fighter(58, 57, 57, 1, "melee");
    wear(&mut bot, &[(0, "bronze_med_helm", 1), (3, "black_battleaxe", 1), (4, "black_chainbody", 1), (7, "black_platelegs", 1)]);
    fund_pack(&mut bot, &[("cabbage", 3), ("bronze_med_helm", 19), ("bronze_arrow", 140), ("iron_dagger", 5)]);
    open_bank(&mut bot, &[("cooked_meat", 28), ("iron_dagger", 31), ("bronze_med_helm", 52), ("bronze_arrow", 495), ("cabbage", 72), ("wooden_shield", 1)]);
    let plan = plan_of(&bot);
    let keep = combat_keep_ids(&bot, &plan, &[], true);
    assert!(keep.is_empty(), "melee keeps nothing through the deposit: {:?}", names(&bot, &keep));
    assert_eq!(names(&bot, &kit_to_withdraw(&bot, &plan)), vec!["wooden_shield".to_string()], "only the missing shield comes out");
    assert_eq!(
        names(&bot, &pack_surplus(&bot, &keep)),
        vec!["bronze_arrow".to_string(), "bronze_med_helm".to_string(), "iron_dagger".to_string()],
        "looted gear is surplus"
    );
}

/// Un objet de la chasse (clé, péage) et la nourriture portée quand la
/// banque n'en a pas restent ; les runes du sort ordonné aussi en magie.
#[test]
fn hunt_items_food_and_runes_ride_the_deposit() {
    let mut bot = fighter(30, 30, 30, 1, "melee");
    fund_pack(&mut bot, &[("edgevilledungeonkey", 1), ("shrimps", 4), ("bronze_med_helm", 3)]);
    open_bank(&mut bot, &[("coins", 100)]);
    let plan = plan_of(&bot);
    let keep = combat_keep_ids(&bot, &plan, &[("edgevilledungeonkey".to_string(), 1)], false);
    assert_eq!(names(&bot, &keep), vec!["edgevilledungeonkey".to_string(), "shrimp".to_string()]);
    let keep = combat_keep_ids(&bot, &plan, &[], true);
    assert!(keep.is_empty(), "the bank restocks the food: deposited, withdrawn to plan");
    bot.combat_style = "magic".to_string();
    let keep = combat_keep_ids(&bot, &plan_of(&bot), &[], true);
    assert_eq!(names(&bot, &keep), vec!["airrune".to_string(), "mindrune".to_string()]);
}

/// ryan_park (« ranged chickens ») : dague de fer en main, carquois de
/// flèches de fer ; la banque OUVERTE tient 2 arcs courts, un arc long,
/// un corps de cuir et 1379 flèches de bronze. L'ancien test
/// `inventory().contains_key` lisait l'arc banqué comme porté : jamais
/// retiré. L'arc court et le corps de cuir sortent, jamais la dague ;
/// toutes les flèches que l'arc accepte sont gardées.
#[test]
fn ranged_trip_withdraws_the_banked_bow_and_leather() {
    let mut bot = fighter(49, 49, 49, 1, "ranged");
    wear(&mut bot, &[(0, "bronze_med_helm", 1), (3, "iron_dagger", 1), (13, "iron_arrow", 99)]);
    fund_pack(&mut bot, &[("shrimps", 10), ("raw_chicken", 5)]);
    open_bank(&mut bot, &[("shortbow", 2), ("longbow", 1), ("leather_armour", 1), ("bronze_arrow", 1379), ("iron_dagger", 25), ("bronze_med_helm", 31), ("shrimps", 51)]);
    let plan = plan_of(&bot);
    let wanted = names(&bot, &kit_to_withdraw(&bot, &plan));
    assert_eq!(wanted, vec!["leather_armour".to_string(), "shortbow".to_string()]);
    assert!(planned_kit(&plan).iter().all(|(_, name)| name != "iron_dagger"), "no melee weapon in a ranged kit");
    assert!(planned_kit(&plan).iter().all(|(slot, _)| *slot != 5), "the bow is two-handed: no shield");
    let keep = combat_keep_ids(&bot, &plan, &[], true);
    assert_eq!(names(&bot, &keep), vec!["bronze_arrow".to_string(), "iron_arrow".to_string()]);
}

/// Bank closed: the last known bank (`inv_closed`) still names the bow
/// for an `equip ranged` step.
#[test]
fn closed_bank_still_counts_as_owned_gear() {
    let mut bot = fighter(49, 49, 49, 1, "ranged");
    wear(&mut bot, &[(3, "iron_dagger", 1)]);
    fund_pack(&mut bot, &[("shrimps", 2)]);
    let com = (bot.tables.com_id)("bank_main:inv").expect("bank com");
    let bow = id(&bot, "shortbow");
    bot.world.inv_closed.insert(com, HashMap::from([(0u8, (bow, 2u32))]));
    assert_eq!(names(&bot, &kit_to_withdraw(&bot, &plan_of(&bot))), vec!["shortbow".to_string()]);
}

/// pia_brown (melee, attaque 36) : hache de bronze en main, épée de
/// bronze et jambières en banque : l'épée sort, la hache reste un outil.
#[test]
fn melee_trip_takes_the_banked_sword_over_the_worn_axe() {
    let mut bot = fighter(36, 36, 36, 1, "melee");
    wear(&mut bot, &[(0, "bronze_med_helm", 1), (3, "bronze_axe", 1), (4, "leather_armour", 1), (5, "wooden_shield", 1)]);
    fund_pack(&mut bot, &[("iron_dagger", 6), ("cabbage", 9)]);
    open_bank(&mut bot, &[("bronze_sword", 1), ("bronze_dagger", 1), ("iron_dagger", 3), ("bronze_platelegs", 1), ("shortbow", 1), ("tinderbox", 1)]);
    let plan = plan_of(&bot);
    let wanted = names(&bot, &kit_to_withdraw(&bot, &plan));
    assert_eq!(wanted, vec!["bronze_platelegs".to_string(), "bronze_sword".to_string()]);
}

/// Un voyage qui laisse le sac sous `HUNT_MIN_FREE` cases libres est une
/// erreur typée qui nomme le contenu, jamais une nouvelle boucle.
#[test]
fn a_trip_that_leaves_the_pack_full_fails_typed() {
    let labels = vec!["19x Bronze med helm".to_string()];
    assert!(trip_verdict(HUNT_MIN_FREE, &labels).is_ok());
    let error = trip_verdict(1, &labels).expect_err("one free slot loops");
    assert!(matches!(&error, crate::ScriptError::Failed(text) if text.contains("Bronze med helm")), "{error}");
    assert!(trip_verdict(0, &labels).is_err());
}

/// La nourriture suit le plan sans jamais remplir le sac.
#[test]
fn food_load_never_fills_the_pack() {
    assert_eq!(food_load(10, 0, 28), 10);
    assert_eq!(food_load(10, 0, 5), 3);
    assert_eq!(food_load(3, 2, 1), 2);
    assert_eq!(food_load(3, 0, 0), 0);
}

/// Plan P14 : sac 1 pièce + 27 choux, banque sans nourriture : la garde
/// ne laisse jamais moins de `HUNT_MIN_FREE` cases (au plus 28 - 1 - 2
/// choux), et rien n'est redéposé quand la place suffit.
#[test]
fn kept_food_is_bounded_by_the_free_slots() {
    let surplus = kept_food_surplus(27, 27, 0);
    assert!(27 - surplus <= 28 - 1 - HUNT_MIN_FREE as u32, "kept {} cabbages", 27 - surplus);
    assert_eq!(kept_food_surplus(27, 10, 0), 17, "the hunt's load only");
    assert_eq!(kept_food_surplus(5, 10, 20), 0, "room left: nothing deposited");
    assert_eq!(kept_food_surplus(0, 10, 0), 0);
}
