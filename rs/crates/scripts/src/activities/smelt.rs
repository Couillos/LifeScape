//! Smelt: ores (+ coal) into bars at a furnace (catalogue §5.7, P1).
//! Ore on a furnace smelts one bar with no interface (the 274 `use`
//! path chains nothing), so the run goes through the shared produce
//! brick with a direct click per ore. The make-x grid is throughput
//! only, never correctness. Superheat stays out: `superheat` owns it.
//!
//! Process: poste fourneau · action fondre · interface directe ·
//! intrants minerais (+ charbon) · produits barres.

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::kit::bank::{bank_trip, bank_withdraw_items, nearest_bank};
use crate::kit::cycle::{resolve_keep, run, CycleState, StepControl, WantGoal};
use crate::kit::pack::{banked_usable, free_slots, obj_id, pack_total};
use crate::kit::produce::{self, Produce, ProduceJob, ProduceSite, ProduceUi};
use crate::kit::survival::{check_events, is_death_abort, learn_level, wait_pack_streamed};
use crate::{ScriptError, ScriptResult};

pub const SMITHING_STAT: u8 = 13;
/// Furnace locs in preference order (big Smelt furnace first).
pub const FURNACES: [&str; 2] = ["furnace1", "furnace2"];
/// Highest F2P bar at each floor (catalogue §5.7): bar and smelt level.
pub const BARS: [(&str, i64); 8] = [("bronze_bar", 1), ("iron_bar", 15), ("silver_bar", 20), ("steel_bar", 30), ("gold_bar", 40), ("mithril_bar", 50), ("adamantite_bar", 70), ("runite_bar", 85)];

const SMELT: Produce<'static> =
    Produce { verb: "smelting", site: ProduceSite::Loc(&FURNACES), ui: ProduceUi::Direct };

/// F2P smelt recipe making `bar` (members excluded).
pub fn smelt_recipe(bar: &str) -> Option<knowledge::recipes::Recipe> {
    knowledge::recipes::making(bar, Some("smelt")).into_iter().find(|entry| entry.members == Some(false))
}

/// F2P smelt recipe eating `ore` (coal names steel, members excluded).
pub fn ore_recipe(ore: &str) -> Option<knowledge::recipes::Recipe> {
    knowledge::recipes::using(ore, Some("smelt")).into_iter().find(|entry| entry.members == Some(false))
}

/// Inputs of `recipe_inputs` the pack holds less of than one bar needs,
/// in recipe order. The server smelts only a full set (`smelting.rs2`
/// `validate_first_smelt`: primary ore AND secondary count, else a
/// failure box): copper alone makes nothing.
pub fn short_inputs(recipe_inputs: &[(String, i64)], carried: &dyn Fn(&str) -> i64) -> Vec<String> {
    recipe_inputs.iter().filter(|(name, each)| carried(name) < *each).map(|(name, _)| name.clone()).collect()
}

/// Furnace post for a run starting at (x, z, level): the nearest F2P
/// furnace on that level (Manhattan), the Wilderness one included (user
/// 2026-11-16), never Tutorial Island's. `None` when the level has none.
pub fn furnace_post(x: i64, z: i64, level: i64) -> Option<knowledge::stations::Station> {
    knowledge::stations::stations_of("furnace")
        .into_iter()
        .filter(|post| post.level == level && knowledge::obtain::station_open(&post.loc, post.x, post.z))
        .min_by_key(|post| ((post.x - x).abs() + (post.z - z).abs(), post.x, post.z))
}

/// Sets one load takes: the pack room (`free` slots plus the inputs
/// already carried, over the slots of one set), the scarcest input in
/// pack + bank (no half-load of copper without its tin), and the order's
/// remaining sets. Never negative.
pub fn load_sets(room_sets: i64, stock_sets: i64, wanted_sets: i64) -> i64 {
    room_sets.min(stock_sets).min(wanted_sets).max(0)
}

/// Sets the order still asks for: the bars left to `--want` over the
/// recipe's odds (iron fails half its smelts: two ores a bar),
/// `i64::MAX` without a want on this bar.
pub fn wanted_sets(goal: Option<WantGoal>, held_bars: u32, bar: u16, odds: f64) -> i64 {
    let Some(goal) = goal.filter(|goal| goal.obj == bar) else { return i64::MAX };
    let left = goal.held.saturating_sub(held_bars);
    (f64::from(left) / odds.clamp(0.01, 1.0)).ceil() as i64
}

/// Highest F2P bar smeltable at `level` (steel from 30, P1 rentable).
pub fn bar_for_level(level: i64) -> &'static str {
    BARS.iter().rev().find(|(_, need)| level >= *need).map(|(name, _)| *name).unwrap_or("bronze_bar")
}

/// Smelt banked loads in the open session: `smelt [bar|ore] [count]`
/// (best bar by level when omitted, 0 = forever; superheat excluded).
pub fn run_smelt(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let (resource, rest) = crate::kit::cycle::split_resource(args);
    let spec = crate::kit::cycle::parse_cli("smelt", &rest)?;
    if spec.allow_paid {
        bot.allow_paid = true;
        bot.log("toll gates allowed for this run");
    }
    bot.allow_shop = spec.allow_shop;
    let task_keep = resolve_keep(bot, &spec)?;
    wait_pack_streamed(bot)?;
    check_events(bot)?;
    let level = learn_level(bot, SMITHING_STAT, 1);
    let wanted = resource.as_deref().unwrap_or_else(|| bar_for_level(level));
    let recipe = smelt_recipe(wanted).or_else(|| ore_recipe(wanted)).ok_or_else(|| ScriptError::Usage(format!("no F2P smelt recipe eats or makes {wanted:?} at {:?}", bot.position())))?;
    let Some((bar_name, _)) = recipe.outputs.first() else {
        return Err(ScriptError::Usage(format!("smelt {wanted:?} makes nothing at {:?}", bot.position())));
    };
    if level < recipe.level {
        return Err(crate::kit::cycle::blocked_level("smithing", i64::from(recipe.level)));
    }
    // The ore used on the furnace: the server picks the bar from it
    // (`smelt_ore_single`: copper → bronze, iron + 2 coal → steel), never
    // from coal alone. Every other input rides in the same set.
    let Some((primary_name, _)) = recipe.inputs.iter().find(|(name, _)| *name != "coal") else {
        return Err(ScriptError::Usage(format!("smelt {bar_name} has no ore input at {:?}", bot.position())));
    };
    let mut inputs: Vec<SetInput> = Vec::new();
    for (name, each) in &recipe.inputs {
        inputs.push(SetInput { name: name.clone(), id: obj_id(bot, name)?, each: (*each).max(1) });
    }
    let primary = obj_id(bot, primary_name)?;
    let bar = obj_id(bot, bar_name)?;
    let mut bank = nearest_bank(bot)?;
    // The brain's `--site` furnace, else the nearest one to the bot.
    let (here_x, here_z, here_level) = match spec.site {
        Some((site_x, site_z, site_level)) => (i64::from(site_x), i64::from(site_z), i64::from(site_level)),
        None => (i64::from(bot.world.x), i64::from(bot.world.z), i64::from(bot.world.level)),
    };
    let post = furnace_post(here_x, here_z, here_level)
        .ok_or_else(|| ScriptError::Usage(format!("no furnace on level {here_level} from {:?}", bot.position())))?;
    bot.log(&format!("furnace post at ({},{})", post.x, post.z));
    let odds = knowledge::recipes::success_odds(&recipe.source);
    let still_wanted = |seen: &Bot, goal: Option<WantGoal>| wanted_sets(goal, pack_total(seen, bar).saturating_add(banked_usable(seen, bar)), bar, odds);
    // Ores carried in are banked with the rest and come back out as
    // balanced sets: a pack full of freshly mined copper would leave no
    // slot for the tin.
    let first_goal = CycleState::begin(bot, &spec).want_goal(bot, &spec);
    let first_wanted = still_wanted(bot, first_goal);
    restock(bot, &bank, &task_keep, &inputs, &recipe.inputs, first_wanted)?;
    let outcome = run(bot, &spec, |step_bot, state| {
        check_events(step_bot)?;
        let wanted = still_wanted(step_bot, state.want_goal(step_bot, &spec));
        if full_sets(step_bot, &inputs) == 0 {
            restock(step_bot, &bank, &task_keep, &inputs, &recipe.inputs, wanted)?;
        }
        // One use per full set: a stray ore past the last set would only
        // raise the server's "need tin/coal" box; never past the order.
        let sets = full_sets(step_bot, &inputs).min(wanted).max(0) as u32;
        if sets == 0 {
            // `want: 0` would read "all carried": the order is met.
            return Ok(StepControl::GoalReached);
        }
        let job = ProduceJob { spec: &SMELT, input: primary, input_name: primary_name, want: sets, products: vec![bar], near: (post.x as i32, post.z as i32) };
        match produce::run_job(step_bot, &job) {
            Err(error) if is_death_abort(&error) => {
                step_bot.log("died smelting, restocking to retry");
                return Ok(StepControl::Continue);
            }
            Err(error) => return Err(error),
            Ok(batch) => {
                step_bot.log(&format!("ok smelted {} ({} made)", batch.consumed, batch.made));
            }
        }
        bank = nearest_bank(step_bot)?;
        bank_trip(step_bot, &bank, &task_keep, 0)?;
        Ok(state.load_done(step_bot, &format!("smelted {bar_name} (smithing={level})")))
    });
    outcome
}

/// One recipe input: name, object id, count per bar.
struct SetInput {
    name: String,
    id: u16,
    each: i64,
}

/// Full recipe sets the pack holds (the scarcest input decides).
fn full_sets(bot: &Bot, inputs: &[SetInput]) -> i64 {
    inputs.iter().map(|input| i64::from(pack_total(bot, input.id)) / input.each).min().unwrap_or(0)
}

/// Bank, top the pack up, and stop `blocked` on every input short of one
/// set (copper banked without tin names the tin; the brain files one
/// obtain per name).
fn restock(bot: &mut Bot, bank: &lifescape_gamedata::banks::Bank, keep: &[u16], inputs: &[SetInput], recipe_inputs: &[(String, i64)], wanted: i64) -> Result<(), ScriptError> {
    bank_trip(bot, bank, keep, 0)?;
    fill_sets(bot, bank, inputs, wanted)?;
    let carried = |name: &str| inputs.iter().find(|input| input.name == name).map(|input| i64::from(pack_total(bot, input.id))).unwrap_or(0);
    let missing = short_inputs(recipe_inputs, &carried);
    if !missing.is_empty() {
        return Err(ScriptError::Blocked { missing });
    }
    Ok(())
}

/// Top the pack up to balanced sets: what the pack already carries
/// counts, each input is withdrawn up to `sets × each`, sets bounded by
/// the room, the scarcest input (pack + known bank) and the order
/// (`load_sets`); nothing when the booth is far (the caller reports
/// `blocked`).
fn fill_sets(bot: &mut Bot, bank: &lifescape_gamedata::banks::Bank, inputs: &[SetInput], wanted: i64) -> Result<(), ScriptError> {
    if !crate::kit::withdraw::reach_bank_for(bot, bank, &inputs.iter().map(|input| input.id).collect::<Vec<u16>>())? {
        return Ok(());
    }
    let per_bar: i64 = inputs.iter().map(|input| input.each).sum();
    let carried: i64 = inputs.iter().map(|input| i64::from(pack_total(bot, input.id))).sum();
    let room = (free_slots(bot) as i64 + carried) / per_bar.max(1);
    let stock = inputs.iter().map(|input| i64::from(pack_total(bot, input.id).saturating_add(banked_usable(bot, input.id))) / input.each).min().unwrap_or(0);
    let sets = load_sets(room, stock, wanted);
    for input in inputs {
        let lacking = sets * input.each - i64::from(pack_total(bot, input.id));
        if lacking > 0 {
            bank_withdraw_items(bot, bank, &[input.id], lacking as u32)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{bar_for_level, furnace_post, load_sets, ore_recipe, short_inputs, smelt_recipe, wanted_sets, BARS, FURNACES, SMITHING_STAT};

    /// A load is balanced and bounded: the room, the scarcest input in
    /// pack + bank (20 copper and 5 tin load 5 sets, never 14 copper of
    /// which 9 smelt nothing) and the order's remaining sets.
    #[test]
    fn a_load_is_bounded_by_room_stock_and_order() {
        assert_eq!(load_sets(14, 5, i64::MAX), 5, "tin is the scarce input");
        assert_eq!(load_sets(14, 100, i64::MAX), 14, "the pack is the bound");
        assert_eq!(load_sets(14, 100, 3), 3, "--want 3 bars loads 3 sets");
        assert_eq!(load_sets(9, 0, 5), 0, "nothing banked loads nothing");
    }

    /// `--want` sets over the server odds: iron fails half its smelts, so
    /// 5 iron bars still due take 10 ores; certain bars take one set each;
    /// a want on another object never caps the load.
    #[test]
    fn wanted_sets_follow_the_odds() {
        use crate::kit::cycle::WantGoal;
        let bar = 2351;
        let goal = Some(WantGoal { obj: bar, held: 15 });
        assert_eq!(wanted_sets(goal, 10, bar, 0.5), 10, "5 iron bars due: 10 ores");
        assert_eq!(wanted_sets(goal, 10, bar, 1.0), 5);
        assert_eq!(wanted_sets(goal, 15, bar, 1.0), 0, "met");
        assert_eq!(wanted_sets(goal, 20, bar, 1.0), 0, "past it");
        assert_eq!(wanted_sets(Some(WantGoal { obj: 1, held: 15 }), 0, bar, 1.0), i64::MAX, "want on another object");
        assert_eq!(wanted_sets(None, 0, bar, 0.5), i64::MAX, "no want");
    }

    /// Live 2026-11-13 (paul_rhodes, Falador; bh99tw2f, Al Kharid): 200
    /// copper banked, no tin; the run withdrew copper alone and used it on
    /// the furnace, the server answered its "need tin" box, and the job
    /// failed "consumed nothing" three times. A set is every input of the
    /// recipe; whatever falls short of one set is reported, so the brain
    /// files the tin (and the copper too when both are gone).
    #[test]
    fn a_smelt_set_needs_every_input() {
        let bronze = smelt_recipe("bronze_bar").expect("bronze").inputs;
        let carried = |pairs: &'static [(&'static str, i64)]| move |name: &str| pairs.iter().find(|(held, _)| *held == name).map(|(_, count)| *count).unwrap_or(0);
        assert_eq!(short_inputs(&bronze, &carried(&[("copper_ore", 14)])), vec!["tin_ore".to_string()], "copper alone smelts nothing");
        assert_eq!(short_inputs(&bronze, &carried(&[])), vec!["copper_ore".to_string(), "tin_ore".to_string()], "both ores asked");
        assert!(short_inputs(&bronze, &carried(&[("copper_ore", 1), ("tin_ore", 1)])).is_empty(), "one of each is a set");
        let steel = smelt_recipe("steel_bar").expect("steel").inputs;
        assert_eq!(short_inputs(&steel, &carried(&[("iron_ore", 9), ("coal", 1)])), vec!["coal".to_string()], "steel needs 2 coal per bar");
        assert!(short_inputs(&steel, &carried(&[("iron_ore", 1), ("coal", 2)])).is_empty());
        let iron = smelt_recipe("iron_bar").expect("iron").inputs;
        assert_eq!(short_inputs(&iron, &carried(&[])), vec!["iron_ore".to_string()]);
    }

    /// Live 2026-11-13 (bct5rxpg, eric_smith, b3gybcev at Varrock east,
    /// will_field at Edgeville): the post was picked near the bank but the
    /// job searched 150 tiles around the BANK, so every bot banking far
    /// from a furnace failed "no produce station". The post is the
    /// furnace nearest the bot, the Wilderness one included (user
    /// 2026-11-16: the Wilderness is open to every bot).
    #[test]
    fn furnace_post_is_the_nearest_furnace() {
        let near = |x: i64, z: i64| {
            let post = furnace_post(x, z, 0).expect("a furnace");
            (post.x, post.z)
        };
        let lumbridge = near(3253, 3420);
        assert!((lumbridge.0 - 3226).abs() <= 1 && (lumbridge.1 - 3255).abs() <= 1, "Varrock east smelts at Lumbridge, got {lumbridge:?}");
        let falador = near(2946, 3368);
        assert_eq!(falador, (2975, 3368), "Falador west bank smelts at Falador");
        let al_kharid = near(3269, 3167);
        assert!((al_kharid.0 - 3272).abs() <= 1 && (al_kharid.1 - 3185).abs() <= 1, "Al Kharid smelts at home, got {al_kharid:?}");
        assert_eq!(near(3096, 3493), (2975, 3368), "Edgeville: Falador is still the nearest");
        let north = near(3140, 3600);
        assert!(north.1 >= 3520, "deep north smelts at the Wilderness furnace, got {north:?}");
    }
    use crate::kit::pack::obj_id;
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    fn offline_bot() -> Bot {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot
    }

    /// Catalogue §5.7 ladder comes from the dump (bronze 1 to runite 85,
    /// coal joining iron from steel on): levels, inputs and F2P flags.
    #[test]
    fn smelt_ladder_comes_from_gamedata() {
        assert_eq!(BARS.map(|(_, level)| level), [1, 15, 20, 30, 40, 50, 70, 85]);
        for (bar, level) in BARS {
            let recipe = smelt_recipe(bar).unwrap_or_else(|| panic!("{bar} recipe resolves"));
            assert_eq!(recipe.kind, "smelt");
            assert_eq!(recipe.level, level, "{bar} level");
            assert_eq!(recipe.members, Some(false), "{bar} F2P");
            assert_eq!(recipe.outputs, vec![(bar.to_string(), 1)]);
        }
        let bronze = smelt_recipe("bronze_bar").expect("bronze smelts");
        assert!(bronze.inputs.iter().any(|(name, _)| name == "copper_ore"), "copper in, got {:?}", bronze.inputs);
        assert!(bronze.inputs.iter().any(|(name, _)| name == "tin_ore"), "tin in, got {:?}", bronze.inputs);
        let steel = smelt_recipe("steel_bar").expect("steel smelts");
        assert!(steel.inputs.contains(&("iron_ore".to_string(), 1)) && steel.inputs.contains(&("coal".to_string(), 2)), "steel eats iron+2 coal, got {:?}", steel.inputs);
        let runite = smelt_recipe("runite_bar").expect("runite smelts");
        assert!(runite.inputs.contains(&("runite_ore".to_string(), 1)) && runite.inputs.contains(&("coal".to_string(), 8)), "runite eats ore+8 coal, got {:?}", runite.inputs);
        assert!(smelt_recipe("perfect_gold_bar").is_none(), "members perfect gold excluded");
        assert!(smelt_recipe("STEEL_BAR").is_some(), "case-insensitive");
        assert!(smelt_recipe("logs").is_none(), "logs never smelt");
    }

    /// Ore names resolve too (coal names steel); default pick is the
    /// highest bar at the level, never above.
    #[test]
    fn ore_lookup_and_default_pick() {
        let iron = ore_recipe("iron_ore").expect("iron smelts");
        assert!(iron.outputs.iter().any(|(name, _)| name == "iron_bar"), "iron gives iron_bar, got {:?}", iron.outputs);
        let steel = ore_recipe("coal").expect("coal smelts with iron");
        assert!(steel.outputs.iter().any(|(name, _)| name == "steel_bar"), "coal gives steel, got {:?}", steel.outputs);
        assert!(ore_recipe("logs").is_none(), "logs never smelt");
        assert_eq!(bar_for_level(1), "bronze_bar");
        assert_eq!(bar_for_level(19), "iron_bar");
        assert_eq!(bar_for_level(29), "silver_bar");
        assert_eq!(bar_for_level(30), "steel_bar");
        assert_eq!(bar_for_level(99), "runite_bar");
    }

    /// Furnace posts and inputs resolve: loc debugnames stand in the
    /// furnace dump (4 F2P sites incl. Al-Kharid and Wilderness), ores,
    /// coal and bars through live tables.
    #[test]
    fn furnace_posts_and_inputs_resolve() {
        let furnace_tiles = lifescape_gamedata::stations::stations_of("furnace");
        assert_eq!(furnace_tiles.len(), 8, "furnace tiles (4 sites)");
        for name in FURNACES {
            assert!(furnace_tiles.iter().any(|entry| entry.loc == *name), "furnace loc {name} placed");
        }
        assert!(furnace_tiles.iter().any(|entry| (entry.x - 3272).abs() <= 2 && (entry.z - 3185).abs() <= 2), "Al-Kharid furnace");
        assert!(furnace_tiles.iter().any(|entry| (entry.x - 3143).abs() <= 2 && (entry.z - 3735).abs() <= 2), "Wilderness furnace");
        let near = lifescape_gamedata::stations::nearest_station("furnace", 3270, 3180, 0).expect("Al-Kharid post");
        assert!((near.x - 3272).abs() <= 2 && (near.z - 3185).abs() <= 2);
        let bot = offline_bot();
        for (bar, _) in BARS {
            let recipe = smelt_recipe(bar).expect("recipe");
            for (name, _) in &recipe.inputs {
                assert!(obj_id(&bot, name).is_ok(), "{name} resolves");
            }
            assert!(obj_id(&bot, bar).is_ok(), "{bar} resolves");
        }
        assert_eq!(SMITHING_STAT, 13, "provenance smithing id");
    }
}
