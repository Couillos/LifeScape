//! F2P quests, batch B: blackknight, demon, squire, hunt, prince,
//! priest, dragon, blackarmgang, vampire. Each `solve_*` follows its quest
//! script step for step.

use lifescape_gamedata::quest_requirements;
use lifescape_sdk::{Bot, Error};
use lifescape_sdk::waits::wait_ticks;

use super::common as solver;
use crate::{ScriptError, ScriptResult};

// shortcut: the Rust SDK has no portal table (no ride_transition_at), so
// floor changes click the nearest ladder/stair loc and wait for the level
// to flip instead of routing through the portal they answer.
fn go_level(bot: &mut Bot, level: i32) -> Result<(), ScriptError> {
    for _ in 0..6 {
        if bot.world.level == level {
            return Ok(());
        }
        let names: &[&str] = if bot.world.level > level { &["laddertop", "spiralstairstop", "stairstop"] } else { &["ladder", "spiralstairs", "stairs"] };
        let wants: Vec<String> = names.iter().map(|name| name.to_lowercase()).collect();
        // One static batch up front (static ladders never move under the
        // pump loop); the loop below only waits for the live stream.
        let static_rows = bot.static_locs_near(bot.world.x, bot.world.z, bot.world.level, 150)?;
        let mut cands: Vec<(i32, i32, i32, u16)> = Vec::new();
        let target = bot.world.ticks + 20;
        while bot.world.ticks < target && cands.is_empty() {
            bot.pump(2)?;
            cands.clear();
            let level = bot.world.level;
            let mut consider = |x: i32, z: i32, tile_level: i32, loc_id: u16| {
                if tile_level != level {
                    return;
                }
                let label = lifescape_gamedata::loc_by_id(loc_id as usize).and_then(|entry| entry.name).unwrap_or_default().to_lowercase();
                if wants.iter().any(|want| label.contains(want)) {
                    cands.push((x, z, tile_level, loc_id));
                }
            };
            for row in &static_rows {
                consider(row.x, row.z, row.level, row.loc);
            }
            for (tile, loc) in &bot.world.locs {
                let Some((loc_id, _, _)) = loc else { continue };
                consider(tile.0, tile.1, tile.2, *loc_id);
            }
        }
        cands.sort_by_key(|cand| (cand.0 - bot.world.x).abs() + (cand.1 - bot.world.z).abs());
        let mut climbed = false;
        for (x, z, lvl, loc) in cands.iter().take(4) {
            if bot.approach(*x, *z, *lvl, 2, 1500).is_err() {
                continue;
            }
            bot.op_loc(1, *x, *z, *loc)?;
            let inner = bot.world.ticks + 30;
            while bot.world.ticks < inner {
                bot.pump(2)?;
                if bot.world.level == level {
                    climbed = true;
                    break;
                }
            }
            if climbed {
                break;
            }
        }
        if !climbed {
            return Err(ScriptError::Sdk(Error::Unreachable(format!("no ladder answers from {:?}", bot.position()))));
        }
    }
    Err(ScriptError::Sdk(Error::Unreachable(format!("cannot reach level {level} from {:?}", bot.position()))))
}

fn solve_blackknight(bot: &mut Bot) -> Result<(), ScriptError> {
    const STARTED: i32 = 1;
    const SABOTAGED: i32 = 3;
    const COMPLETE: i32 = 4;
    if solver::varp(bot, "spy") == COMPLETE {
        return Ok(());
    }
    let chainbody = solver::obj_id("iron_chainbody")?;
    let helm = solver::obj_id("bronze_med_helm")?;
    if solver::count(bot, chainbody) + solver::worn_count(bot, chainbody) < 1 || solver::count(bot, helm) + solver::worn_count(bot, helm) < 1 {
        return Err(ScriptError::Sdk(Error::State(format!("no disguise (chainbody + med helm) at {:?}", bot.position()))));
    }
    if solver::varp(bot, "spy") < STARTED {
        solver::talk_like(bot, "sir_amik_varze", &["quest", "laugh", "give it a go"], None, 400)?;
    }
    if solver::varp(bot, "spy") < SABOTAGED {
        if solver::count(bot, solver::obj_id("cabbage")?) < 1 {
            return Err(ScriptError::Sdk(Error::State(format!("no cabbage for the hole at {:?}", bot.position()))));
        }
        bot.intend("quest blackknight: infiltrating the fortress");
        if (bot.world.x - 3016).abs() + (bot.world.z - 3515).abs() > 20 || bot.world.level != 0 {
            bot.walk_to(3016, 3515, 0, 9000)?;
        }
        solver::wear(bot, "iron_chainbody")?;
        solver::wear(bot, "bronze_med_helm")?;
        // South door, then the banquet door past the guard's chat.
        solver::operate_loc(bot, &["bkfortressdoor1"], 1, Some((3016, 3513, 0)))?;
        solver::operate_loc(bot, &["bkfortressdoor2"], 1, Some((3019, 3515, 0)))?;
        solver::finish_dialog_like(bot, &["going in", "don't care"], 200)?;
        let _ = bot.walk_steps(&[(3020, 3515), (3021, 3515)], 30);
        // Grill listen from the kitchen side, then upstairs east.
        if solver::varp(bot, "spy") < 2 {
            bot.walk_to(3019, 3515, 0, 9000)?;
            solver::operate_loc(bot, &["witchgrill"], 1, Some((3025, 3507, 0)))?;
            solver::finish_dialog_like(bot, &["cabbage", "potion", "draynor"], 200)?;
        }
        go_level(bot, 1)?;
        solver::operate_loc(bot, &["bkfortressdoor3"], 1, Some((3025, 3512, 1)))?;
        solver::use_item_on_loc(bot, "cabbage", &["blackknighthole"])?;
        solver::finish_dialog_like(bot, &["sabotaged", "successfully"], 200)?;
        go_level(bot, 0)?;
    }
    solver::talk_plain(bot, "sir_amik_varze", &[], None, 400)
}

fn solve_demon(bot: &mut Bot) -> Result<(), ScriptError> {
    const COMPLETE: i32 = 30;
    const INCANTATION: &str = "Carlem Aber Camerinthum Purchai Gabindo";
    if solver::varp(bot, "demonstart") == COMPLETE {
        return Ok(());
    }
    bot.intend("quest demon: hunting the keys");
    let coins = solver::obj_id("coins")?;
    if solver::varp(bot, "demonstart") < 1 && solver::count(bot, coins) < 1 {
        return Err(ScriptError::Sdk(Error::State(format!("no coin for Aris at {:?}", bot.position()))));
    }
    // Traiborn eats 25 bones for his key; without it past stage 28 the key
    // was lost and he resets the hunt.
    if solver::count(bot, solver::obj_id("silverlight_key_1")?) < 1 && solver::varp(bot, "demonstart") < 29 {
        let bones = solver::obj_id("bones")?;
        let want = if solver::varp(bot, "demonstart") >= 28 || solver::varp(bot, "demonstart") < 3 { 25 } else { (28 - solver::varp(bot, "demonstart")).max(1) as u32 };
        if solver::count(bot, bones) < want {
            return Err(ScriptError::Sdk(Error::State(format!("need {want} bones for Traiborn at {:?}", bot.position()))));
        }
        solver::talk_like(bot, "traiborn", &["knocking", "bones", "key"], None, 400)?;
        solver::talk_like(bot, "traiborn", &["bones", "give"], None, 400)?;
    }
    solver::talk_like(bot, "aris", &["here you go", "delrith", "silverlight", "incantation", "best"], None, 400)?;
    solver::talk_like(bot, "sir_prysin", &["aris", "silverlight", "crystal", "keys", "hunting", "key", "rovin", "wizard"], None, 400)?;
    if solver::count(bot, solver::obj_id("silverlight_key_2")?) < 1 && solver::varp(bot, "demonstart") < 29 {
        solver::talk_like(bot, "captain_rovin", &["important", "demon", "invade", "silverlight", "key"], None, 400)?;
    }
    if solver::count(bot, solver::obj_id("silverlight_key_3")?) < 1 && solver::varp(bot, "demonstart") < 29 {
        // Prysin's key down the kitchen drain: wash it into the Varrock
        // sewer, climb down the manhole and grab it before it despawns.
        if solver::count(bot, solver::obj_id("bucket_water")?) < 1 {
            return Err(ScriptError::Sdk(Error::State(format!("need a bucket of water for the drain at {:?}", bot.position()))));
        }
        let (drain_x, drain_z, drain_level, drain_loc) = solver::find_loc(bot, &["questdrain"], 150)?;
        bot.walk_to(drain_x, drain_z, drain_level, 6000)?;
        bot.op_loc(1, drain_x, drain_z, drain_loc)?;
        wait_ticks(bot, 5)?;
        solver::drain_dialog(bot, 150)?;
        let water = solver::obj_id("bucket_water")?;
        let (use_com, use_slot) = solver::inv_slot(bot, water)?;
        bot.op_loc_u(drain_x, drain_z, drain_loc, water, u16::from(use_slot), use_com)?;
        wait_ticks(bot, 10)?;
        solver::drain_dialog(bot, 150)?;
        bot.approach(3237, 3458, 0, 4, 600)?;
        // Manhole covers never stream: click the portal-table loc ids
        // blind (881 up-cover, 882 down-hole).
        bot.op_loc(1, 3237, 3458, 881)?;
        wait_ticks(bot, 5)?;
        solver::drain_dialog(bot, 150)?;
        if bot.world.z > 9000 {
            bot.op_loc(1, 3237, 3458, 882)?;
            wait_ticks(bot, 5)?;
            solver::drain_dialog(bot, 150)?;
        }
        bot.approach(3225, 9897, 0, 10, 15000)?;
        solver::take_near(bot, "silverlight_key_3", 60)?;
        solver::operate_loc(bot, &["ladder_from_cellar"], 1, None)?;
    }
    if solver::varp(bot, "demonstart") < 29 {
        solver::talk_like(bot, "sir_prysin", &["aris", "silverlight", "crystal", "keys", "hunting", "key", "rovin", "wizard"], None, 400)?;
    }
    solver::wear(bot, "silverlight")?;
    solver::go_to_npc(bot, "delrith", None)?;
    solver::attack(bot, &["delrith"], 180)?;
    solver::finish_dialog_like(bot, &[INCANTATION], 300)?;
    if !solver::wait_complete(bot, 300)? {
        return Err(Error::Timeout(format!("demon slayer never completed at {:?}", bot.position())).into());
    }
    Ok(())
}

fn solve_squire(bot: &mut Bot) -> Result<(), ScriptError> {
    const COMPLETE: i32 = 7;
    if solver::varp(bot, "squire") == COMPLETE {
        return Ok(());
    }
    bot.intend("quest squire: the knight's sword");
    // Squire -> Reldo -> Thurgo (pie) -> Squire -> portrait -> Thurgo.
    solver::talk_like(bot, "squire", &["life as a squire", "make a new sword", "dwarves make another", "give it a go"], None, 400)?;
    solver::talk_like(bot, "reldo", &["imcando"], None, 400)?;
    if solver::count(bot, solver::obj_id("redberry_pie")?) < 1 && solver::varp(bot, "squire") < 3 {
        return Err(ScriptError::Sdk(Error::State(format!("no redberry pie for Thurgo at {:?}", bot.position()))));
    }
    solver::talk_like(bot, "thurgo", &["redberry pie"], None, 400)?;
    solver::talk_like(bot, "thurgo", &["special sword"], None, 400)?;
    solver::talk_like(bot, "squire", &["picture", "portrait", "sword"], None, 400)?;
    // Vyvin's cupboard: only answer from the south stand, wait him out.
    if solver::count(bot, solver::obj_id("knights_portrait")?) < 1 && solver::varp(bot, "squire") < 6 {
        bot.intend("climbing to Vyvin's room for the portrait");
        bot.approach(2983, 3335, 2, 4, 600)?;
        let target = bot.world.ticks + 1800;
        while bot.world.ticks < target && solver::count(bot, solver::obj_id("knights_portrait")?) == 0 {
            let vyvin_close = bot.npcs_named(&["sir_vyvin"]).into_iter().any(|(_, x, z)| (x - 2984).abs() <= 1 && (z - 3336).abs() <= 1 || (x - bot.world.x).abs() <= 1 && (z - bot.world.z).abs() <= 1);
            if vyvin_close {
                wait_ticks(bot, 10)?;
                continue;
            }
            let _ = solver::operate_loc(bot, &["vyvincupboardshut"], 1, Some((2984, 3335, 2)));
            let _ = solver::operate_loc(bot, &["vyvincupboardopen"], 1, Some((2984, 3335, 2)));
            wait_ticks(bot, 5)?;
        }
        if solver::count(bot, solver::obj_id("knights_portrait")?) == 0 {
            return Err(ScriptError::Sdk(Error::Unreachable(format!("could not take the portrait at {:?}", bot.position()))));
        }
    }
    solver::talk_like(bot, "thurgo", &["sword", "picture"], None, 400)?;
    if solver::count(bot, solver::obj_id("faladian_sword")?) < 1 {
        if solver::count(bot, solver::obj_id("blurite_ore")?) < 1 || solver::count(bot, solver::obj_id("iron_bar")?) < 2 {
            return Err(ScriptError::Sdk(Error::State(format!("missing sword materials at {:?}", bot.position()))));
        }
        solver::talk_like(bot, "thurgo", &["sword", "materials", "here"], None, 400)?;
    }
    solver::talk_like(bot, "squire", &["sword", "thank", "hurry"], None, 400)?;
    if !solver::wait_complete(bot, 300)? {
        return Err(Error::Timeout(format!("knight's sword never completed at {:?}", bot.position())).into());
    }
    Ok(())
}

fn sail(bot: &mut Bot, sailor: &str, keywords: &[&str]) -> Result<(), ScriptError> {
    let before = (bot.world.x, bot.world.z);
    solver::talk_like(bot, sailor, keywords, None, 400)?;
    let target = bot.world.ticks + 200;
    while bot.world.ticks < target {
        bot.pump(5)?;
        if (bot.world.x, bot.world.z) != before {
            wait_ticks(bot, 10)?;
            return Ok(());
        }
    }
    Err(ScriptError::Sdk(Error::Timeout(format!("ship did not sail with {sailor} at {:?}", bot.position()))))
}

fn solve_hunt(bot: &mut Bot) -> Result<(), ScriptError> {
    bot.intend("quest hunt: pirate's treasure");
    let rum = solver::obj_id("karamja_rum")?;
    let key = solver::obj_id("chest_key")?;
    let message = solver::obj_id("piratemessage")?;
    if solver::count(bot, solver::obj_id("coins")?) < 90 {
        return Err(ScriptError::Sdk(Error::State(format!("need at least 90 coins for the boat fares at {:?}", bot.position()))));
    }
    if solver::varp(bot, "hunt") < 2 && solver::count(bot, key) == 0 && solver::count(bot, message) == 0 {
        if solver::count(bot, rum) < 1 {
            solver::talk_like(bot, "redbeard_frank", &["treasure"], None, 400)?;
            sail(bot, "seaman_lorris", &["yes"])?;
            solver::go_to_npc(bot, "zambo", None)?;
            buy_from_shop(bot, "karamja_rum")?;
        }
        solver::talk_like(bot, "luthas", &["employment"], None, 400)?;
        // Stash the rum, then fill the crate with 10 bananas.
        solver::use_item_on_loc(bot, "karamja_rum", &["bananacrate"])?;
        let banana = solver::obj_id("banana")?;
        let target = bot.world.ticks + 1200;
        while solver::varp(bot, "crate_bananas") < 10 && bot.world.ticks < target {
            if solver::count(bot, banana) < 1 {
                let before = solver::count(bot, banana);
                let inner = bot.world.ticks + 300;
                while solver::count(bot, banana) == before && bot.world.ticks < inner {
                    let _ = solver::operate_loc(bot, &["bananatree"], 1, None);
                }
                continue;
            }
            solver::use_item_on_loc(bot, "banana", &["bananacrate"])?;
        }
        solver::talk_like(bot, "luthas", &["thank", "another"], None, 400)?;
        sail(bot, "customs_officer", &["journey", "search away", "ok"])?;
        solver::talk_like(bot, "wydin", &["job"], None, 400)?;
        solver::wear(bot, "white_apron")?;
        let (crate_x, crate_z, crate_level, crate_loc) = solver::find_loc(bot, &["grocerycrate"], 150)?;
        bot.approach(crate_x, crate_z, crate_level, 2, 15000)?;
        bot.op_loc(1, crate_x, crate_z, crate_loc)?;
        wait_ticks(bot, 10)?;
        if solver::count(bot, rum) < 1 {
            return Err(ScriptError::Sdk(Error::State(format!("no rum in Wydin's back room crate at {:?}", bot.position()))));
        }
    }
    if solver::varp(bot, "hunt") == 1 && solver::count(bot, rum) > 0 && solver::count(bot, key) == 0 {
        solver::talk_like(bot, "redbeard_frank", &["ok", "thanks", "here"], None, 400)?;
    }
    if bot.world.level != 1 {
        bot.walk_to(3230, 3382, 1, 9000)?;
    }
    // The chest only answers from its east side.
    if solver::count(bot, message) < 1 {
        let (chest_x, chest_z, chest_level, chest_loc) = solver::find_loc(bot, &["piratechest"], 150)?;
        bot.walk_to(chest_x + 1, chest_z, chest_level, 6000)?;
        let (use_com, use_slot) = solver::inv_slot(bot, key)?;
        bot.op_loc_u(chest_x, chest_z, chest_loc, key, u16::from(use_slot), use_com)?;
        let target = bot.world.ticks + 300;
        while bot.world.ticks < target && solver::count(bot, message) < 1 {
            bot.pump(5)?;
        }
        if solver::count(bot, message) < 1 {
            return Err(ScriptError::Sdk(Error::State(format!("chest did not yield the pirate message at {:?}", bot.position()))));
        }
    }
    let (com, slot) = solver::inv_slot(bot, message)?;
    bot.op_held(1, message, slot, com)?;
    wait_ticks(bot, 10)?;
    bot.close_modal()?;
    bot.pump(5)?;
    solver::operate_loc(bot, &["woodenstairstop"], 1, None)?;
    bot.approach(2999, 3383, 0, 4, 600)?;
    solver::attack(bot, &["falador_gardener"], 300)?;
    // Dig the Falador park spot with the spade.
    let spade = solver::obj_id("spade")?;
    let (com, slots) = bot.backpack();
    let com = com.ok_or_else(|| ScriptError::Sdk(Error::State("backpack not streamed".to_string())))?;
    let slot = slots.into_iter().find(|(_, (held, _))| *held == spade).map(|(slot, _)| slot)
        .ok_or_else(|| ScriptError::Sdk(Error::State(format!("no spade for the dig at {:?}", bot.position()))))?;
    for (tile_x, tile_z) in [(2999, 3383), (2998, 3383), (3000, 3383), (2999, 3382), (2999, 3384)] {
        if bot.approach(tile_x, tile_z, 0, 1, 6000).is_err() {
            continue;
        }
        bot.op_held(1, spade, slot, com)?;
        wait_ticks(bot, 10)?;
        if solver::has_message(bot, "dig a hole") {
            return Ok(());
        }
    }
    Err(ScriptError::Sdk(Error::Unreachable(format!("could not dig at the Falador park spot at {:?}", bot.position()))))
}

fn buy_from_shop(bot: &mut Bot, obj_name: &str) -> Result<(), ScriptError> {
    let wanted = solver::obj_id(obj_name)?;
    if solver::count(bot, wanted) > 0 {
        return Ok(());
    }
    let main_shop = lifescape_gamedata::com_id("shop_template:inv")
        .ok_or_else(|| ScriptError::Sdk(Error::State("shop_template:inv missing".to_string())))?;
    let mut cands: Vec<(i32, String, i32, i32, i32)> = Vec::new();
    for shop in lifescape_gamedata::shops::all_shops() {
        if !shop.stocks.iter().any(|stock| stock.obj == obj_name && stock.count > 0) {
            continue;
        }
        for keeper in &shop.keepers {
            for spawn in lifescape_gamedata::spawns::all_npc_spawns(Some(keeper.npc_id as usize)) {
                if spawn.level as i32 != bot.world.level {
                    continue;
                }
                cands.push((((spawn.x as i32 - bot.world.x).abs() + (spawn.z as i32 - bot.world.z).abs()), keeper.npc.clone(), spawn.x as i32, spawn.z as i32, spawn.level as i32));
            }
        }
    }
    cands.sort_by_key(|cand| cand.0);
    for (_, name, x, z, level) in cands {
        bot.approach(x, z, level, 2, 6000)?;
        if bot.trade_request(&name, 60).unwrap_or(false) {
            bot.pump(10)?;
            let before = solver::count(bot, wanted);
            if let Some(slot) = bot.world.inv.get(&main_shop).and_then(|slots| slots.iter().find(|(_, (held, _))| *held == wanted).map(|(slot, _)| *slot)) {
                bot.inv_button(2, wanted, slot, main_shop)?;
                let target = bot.world.ticks + 60;
                while bot.world.ticks < target && solver::count(bot, wanted) <= before {
                    bot.pump(2)?;
                }
            }
            bot.close_modal()?;
            bot.pump(5)?;
            if solver::count(bot, wanted) > 0 {
                return Ok(());
            }
        }
    }
    Err(ScriptError::Sdk(Error::Unreachable(format!("no shop sold {obj_name} near {:?}", bot.position()))))
}

fn solve_prince(bot: &mut Bot) -> Result<(), ScriptError> {
    const COMPLETE: i32 = 110;
    if solver::varp(bot, "princequest") == COMPLETE {
        return Ok(());
    }
    bot.intend("quest prince: freeing prince ali");
    if solver::varp(bot, "princequest") < 10 {
        solver::talk_like(bot, "hassan", &["help", "chancellor", "osman"], None, 400)?;
    }
    if solver::varp(bot, "princequest") < 20 {
        solver::talk_like(bot, "osman", &["better", "okay", "luck", "leela", "first", "second"], None, 400)?;
    }
    if solver::varp(bot, "princequest") < 30 {
        for name in ["princeskey", "blondwig", "pink_skirt", "skinpaste"] {
            if solver::count(bot, solver::obj_id(name)?) < 1 {
                return Err(ScriptError::Sdk(Error::State(format!("need {name} for Leela at {:?}", bot.position()))));
            }
        }
        solver::talk_like(bot, "leela", &["help", "free", "prince", "equipment", "guard"], None, 400)?;
    }
    if solver::varp(bot, "princequest") < 40 {
        if solver::count(bot, solver::obj_id("beer")?) < 3 {
            return Err(ScriptError::Sdk(Error::State(format!("need 3 beers for Joe at {:?}", bot.position()))));
        }
        solver::talk_like(bot, "joe", &["beer", "fancy", "throat", "thirsty"], None, 400)?;
        solver::talk_like(bot, "joe", &["beer", "fancy", "throat", "thirsty"], None, 400)?;
        if solver::varp(bot, "princequest") < 40 {
            return Err(ScriptError::Sdk(Error::State(format!("Joe did not drink the beers at {:?}", bot.position()))));
        }
    }
    if solver::varp(bot, "princequest") < 100 {
        let (door_x, door_z, door_level, door_loc) = solver::find_loc(bot, &["alidoor"], 150)?;
        let target = bot.world.ticks + 150;
        while bot.world.ticks < target && bot.npcs_named(&["lady_keli"]).is_empty() {
            bot.pump(5)?;
        }
        let key = solver::obj_id("princeskey")?;
        if solver::count(bot, key) < 1 {
            return Err(ScriptError::Sdk(Error::State(format!("need princeskey for the cell at {:?}", bot.position()))));
        }
        let rope = solver::obj_id("rope")?;
        let mut unlocked = false;
        for _ in 0..4 {
            if !bot.npcs_named(&["lady_keli"]).is_empty() {
                if solver::count(bot, rope) < 1 {
                    return Err(ScriptError::Sdk(Error::State(format!("need rope for Keli at {:?}", bot.position()))));
                }
                tie_keli(bot)?;
            }
            let seen = bot.world.messages.len();
            // The door only answers from the south stand.
            let _ = bot.walk_to(door_x, door_z + 1, door_level, 750);
            let (use_com, use_slot) = solver::inv_slot(bot, key)?;
            bot.op_loc_u(door_x, door_z, door_loc, key, u16::from(use_slot), use_com)?;
            let inner = bot.world.ticks + 100;
            while bot.world.ticks < inner && !unlocked {
                bot.pump(5)?;
                for text in bot.world.messages[seen..].iter() {
                    if text.contains("unlock the door") {
                        unlocked = true;
                    } else if text.contains("Keli") {
                        break;
                    }
                }
                if bot.world.messages[seen..].iter().any(|text| text.contains("Keli")) {
                    break;
                }
            }
            if unlocked {
                break;
            }
        }
        if !unlocked {
            return Err(ScriptError::Sdk(Error::Unreachable(format!("cell door refused at {:?}", bot.position()))));
        }
        solver::talk_like(bot, "prince_ali_prison", &["rescue", "disguise", "key", "leela"], Some((door_x, door_z + 1)), 400)?;
    }
    solver::talk_like(bot, "hassan", &["reward", "payment", "rescued", "emir"], None, 400)?;
    if !solver::wait_complete(bot, 300)? {
        return Err(Error::Timeout(format!("prince ali never completed at {:?}", bot.position())).into());
    }
    Ok(())
}

fn tie_keli(bot: &mut Bot) -> Result<(), ScriptError> {
    let rope = solver::obj_id("rope")?;
    solver::go_to_npc(bot, "lady_keli", None)?;
    let target = bot.world.ticks + 1800;
    while bot.world.ticks < target {
        if bot.npcs_named(&["lady_keli"]).is_empty() {
            return Ok(());
        }
        let Some(slot) = bot.find_npc("lady_keli") else {
            bot.pump(5)?;
            continue;
        };
        let Some(npc) = bot.world.npcs.get(&slot) else {
            bot.pump(5)?;
            continue;
        };
        let (npc_x, npc_z) = (npc.x, npc.z);
        let _ = bot.approach(npc_x, npc_z, bot.world.level, 1, 1500);
        let slot = bot.find_npc("lady_keli").unwrap_or(slot);
        let Ok((use_com, use_slot)) = solver::inv_slot(bot, rope) else {
            return Err(ScriptError::Sdk(Error::State(format!("no rope for Keli at {:?}", bot.position()))));
        };
        bot.op_npc_u(slot, rope, u16::from(use_slot), use_com)?;
        wait_ticks(bot, 10)?;
        solver::drain_dialog(bot, 150)?;
        bot.pump(5)?;
    }
    Err(ScriptError::Sdk(Error::Unreachable(format!("could not tie Keli at {:?}", bot.position()))))
}

fn solve_priest(bot: &mut Bot) -> Result<(), ScriptError> {
    bot.intend("quest priest: the restless ghost");
    if solver::varp(bot, "priest") < 1 {
        solver::talk_like(bot, "father_aereck", &["looking for a quest", "quest"], None, 400)?;
    }
    if solver::varp(bot, "priest") < 2 {
        solver::talk_like(bot, "father_urhney", &["aereck", "ghost", "sent me"], None, 400)?;
    }
    if solver::varp(bot, "priest") < 3 {
        // The ghost only speaks through the ghostspeak amulet: wear it,
        // spawn it from the coffin, and talk fast (it despawns quickly).
        solver::wear(bot, "amulet_of_ghostspeak")?;
        let mut talked = false;
        for _ in 0..6 {
            let _ = solver::operate_loc(bot, &["shutghostcoffin", "restless_ghost_altar"], 1, None);
            let Some(slot) = bot.find_npc("ghost") else {
                bot.pump(10)?;
                continue;
            };
            let Some(npc) = bot.world.npcs.get(&slot) else { continue };
            if bot.approach(npc.x, npc.z, bot.world.level, 2, 1000).is_err() {
                continue;
            }
            bot.talk_to(bot.find_npc("ghost").unwrap_or(slot))?;
            bot.pump(5)?;
            if bot.world.modal.is_none() {
                continue;
            }
            if solver::finish_dialog_like(bot, &["problem", "doomed", "skull", "yep", "yes"], 400).is_ok() {
                talked = true;
                break;
            }
        }
        if !talked {
            return Err(ScriptError::Sdk(Error::Unreachable(format!("ghost never talked at {:?}", bot.position()))));
        }
    }
    // The skull wakes its skeleton; take it and carry it to the altar.
    if solver::count(bot, solver::obj_id("ghostskull")?) == 0 {
        bot.approach(3120, 9567, 0, 6, 600)?;
        solver::take_near(bot, "ghostskull", 300)?;
    }
    bot.approach(3249, 3194, 0, 6, 600)?;
    let _ = solver::operate_loc(bot, &["shutghostcoffin"], 1, None);
    for _ in 0..6 {
        if bot.world.modal.is_none() || !bot.chat_options().is_empty() {
            break;
        }
        bot.chat_continue()?;
        bot.pump(5)?;
    }
    let skull = solver::obj_id("ghostskull")?;
    let altar = solver::find_loc(bot, &["restless_ghost_altar", "shutghostcoffin"], 150)?;
    let mut stands: Vec<(i32, i32)> = (-1..=1).flat_map(|dx| (-1..=1).map(move |dz| (altar.0 + dx, altar.1 + dz))).filter(|tile| *tile != (altar.0, altar.1)).collect();
    stands.sort_by_key(|tile| (tile.0 - bot.world.x).abs() + (tile.1 - bot.world.z).abs());
    for (stand_x, stand_z) in stands {
        if bot.approach(stand_x, stand_z, altar.2, 0, 1500).is_err() {
            continue;
        }
        let (use_com, use_slot) = solver::inv_slot(bot, skull)?;
        bot.op_loc_u(altar.0, altar.1, altar.3, skull, u16::from(use_slot), use_com)?;
        bot.pump(10)?;
        break;
    }
    for _ in 0..12 {
        if bot.world.modal.is_none() || !bot.chat_options().is_empty() || solver::completion_open(bot) {
            break;
        }
        bot.chat_continue()?;
        bot.pump(5)?;
    }
    let _ = solver::wear(bot, "amulet_of_strength");
    if !solver::wait_complete(bot, 300)? {
        return Err(Error::Timeout(format!("restless ghost never completed at {:?}", bot.position())).into());
    }
    Ok(())
}

fn solve_dragon(bot: &mut Bot) -> Result<(), ScriptError> {
    const COMPLETE: i32 = 10;
    if solver::varp(bot, "dragonquest") == COMPLETE {
        return Ok(());
    }
    bot.intend("quest dragon: dragon slayer");
    // One stage per run (logout persists the next): item gates skip
    // finished loot, varp gates skip finished talks.
    if solver::varp(bot, "dragonquest") < 1 {
        solver::talk_like(bot, "guildmaster", &["rune plate", "plate mail", "oziach"], None, 400)?;
        return Ok(());
    }
    if solver::varp(bot, "dragonquest") < 2 {
        if (bot.world.x - 3069).abs() + (bot.world.z - 3517).abs() > 30 {
            let _ = bot.walk_to(3069, 3522, 0, 9000);
        }
        solver::talk_like(bot, "oziach", &["rune plate", "sell", "guildmaster", "prove", "shield", "dragon", "together", "fare"], None, 400)?;
        return Ok(());
    }
    if solver::count(bot, solver::obj_id("antidragonbreathshield")?) == 0 {
        solver::talk_like(bot, "duke_of_lumbridge", &["shield", "dragon", "breath"], None, 400)?;
        return Ok(());
    }
    if solver::varp(bot, "dragon_ned_hired") < 1 {
        solver::talk_like(bot, "ned", &["sailor", "crandor", "take me", "map"], None, 400)?;
        return Ok(());
    }
    if solver::count(bot, solver::obj_id("mappart1")?) == 0 {
        let _ = bot.walk_to(2941, 3252, 0, 9000);
        solver::use_item_on_loc(bot, "melzarkey", &["melzardoor"])?;
        for (npc, key, door) in [("dragonslayer_giantrat", "redkey", "reddoor"), ("dragonslayer_ghost", "orangekey", "orangedoor"), ("dragonslayer_skeleton", "yellowkey", "yellowdoor")] {
            if solver::count(bot, solver::obj_id(key)?) == 0 && solver::count(bot, solver::obj_id("mappart1")?) == 0 {
                solver::attack(bot, &[npc], 600)?;
                solver::take_near(bot, key, 300)?;
            }
            if solver::count(bot, solver::obj_id(key)?) > 0 {
                solver::use_item_on_loc(bot, key, &[door])?;
            }
        }
        let _ = solver::operate_loc(bot, &["funladdertop"], 1, None);
        for (npc, key, door) in [("dragonslayer_zombie", "bluekey", "bluedoor"), ("melzar_the_mad", "magentakey", "magentadoor"), ("dragonslayer_demon", "greenkey", "greendoor")] {
            if solver::count(bot, solver::obj_id(key)?) == 0 && solver::count(bot, solver::obj_id("mappart1")?) == 0 {
                solver::attack(bot, &[npc], 600)?;
                solver::take_near(bot, key, 300)?;
            }
            if solver::count(bot, solver::obj_id(key)?) > 0 {
                solver::use_item_on_loc(bot, key, &[door])?;
            }
        }
        solver::operate_loc(bot, &["funchestshut"], 1, None)?;
        solver::operate_loc(bot, &["funchestopen"], 1, None)?;
        return Ok(());
    }
    if solver::count(bot, solver::obj_id("mappart3")?) == 0 {
        solver::talk_like(bot, "oracle", &["map", "crandor", "piece"], None, 400)?;
        solver::operate_loc(bot, &["dragon_slayer_magic_door"], 1, None)?;
        solver::operate_loc(bot, &["oraclechestshut"], 1, None)?;
        solver::operate_loc(bot, &["oraclechestopen"], 1, None)?;
        return Ok(());
    }
    if solver::count(bot, solver::obj_id("mappart2")?) == 0 {
        solver::talk_like(bot, "wormbrain", &["piece of map", "pay", "alright", "10,000"], None, 400)?;
        return Ok(());
    }
    if solver::varp(bot, "dragonquest") < 3 {
        solver::talk_like(bot, "klarense", &["buy", "yep", "sounds good"], None, 400)?;
        return Ok(());
    }
    let _ = solver::operate_loc(bot, &["dragonshipgangplank_on"], 1, None);
    let _ = solver::operate_loc(bot, &["dragonshipladdertop"], 1, None);
    if solver::varp(bot, "dragonquestvar") < 3 {
        for _ in 0..3 {
            solver::use_item_on_loc(bot, "woodplank", &["shiphole"])?;
        }
    }
    let _ = solver::operate_loc(bot, &["dragonshipladder"], 1, None);
    let _ = solver::operate_loc(bot, &["dragonshipgangplank_off"], 1, None);
    // Join the three map pieces, hand the map to Ned, sail to Crandor.
    if solver::count(bot, solver::obj_id("dragonmap")?) == 0 {
        let first = solver::obj_id("mappart1")?;
        let second = solver::obj_id("mappart2")?;
        for part in ["mappart1", "mappart2", "mappart3"] {
            if solver::count(bot, solver::obj_id(part)?) == 0 {
                return Err(ScriptError::Sdk(Error::State(format!("missing map piece {part} at {:?}", bot.position()))));
            }
        }
        let (com, slot) = solver::inv_slot(bot, first)?;
        let (use_com, use_slot) = solver::inv_slot(bot, second)?;
        bot.op_held_u(first, slot, com, second, use_slot, use_com)?;
        let target = bot.world.ticks + 60;
        while bot.world.ticks < target && solver::count(bot, solver::obj_id("dragonmap")?) == 0 {
            bot.pump(2)?;
        }
    }
    solver::talk_like(bot, "ned", &["crandor island", "take me", "map", "sailor"], None, 400)?;
    solver::operate_loc(bot, &["dragonshipgangplank_on"], 1, None)?;
    solver::operate_loc(bot, &["dragonshipladdertop"], 1, None)?;
    solver::talk_like(bot, "dragonslayer_ned", &["yep", "ready", "go"], None, 400)?;
    // Elvarg with the shield worn: dragon fire is unsurvivable without it.
    solver::wear(bot, "antidragonbreathshield")?;
    solver::operate_loc(bot, &["crandor_rock_opening"], 1, None)?;
    let _ = solver::operate_loc(bot, &["elvarg_gate_right", "elvarg_gate_left"], 1, None);
    solver::attack(bot, &["elvarg"], 1200)?;
    solver::wait_complete(bot, 300)?;
    Ok(())
}

fn solve_blackarmgang(bot: &mut Bot, gang: Option<&str>) -> Result<(), ScriptError> {
    bot.intend("quest blackarmgang: shield of arrav");
    if solver::varp(bot, "phoenixgang") == 10 || solver::varp(bot, "blackarmgang") == 4 {
        return Ok(());
    }
    let black = solver::varp(bot, "blackarmgang");
    let phoenix = solver::varp(bot, "phoenixgang");
    // Phoenix is the soloable half; Black Arm needs a Phoenix insider for
    // the two crossbows, so it stops loudly without them.
    if gang == Some("blackarm") || (black >= 1 && phoenix < 9) {
        return join_blackarm(bot);
    }
    join_phoenix(bot)
}

fn join_blackarm(bot: &mut Bot) -> Result<(), ScriptError> {
    match solver::varp(bot, "blackarmgang") {
        0 => solver::talk_like(bot, "tramp", &["alleyway", "join"], None, 400),
        1 => solver::talk_like(bot, "katrine", &["place", "black arm", "tramp", "member", "try", "no problem"], None, 400),
        2 => {
            if solver::count(bot, solver::obj_id("phoenix_crossbow")?) < 2 {
                return Err(ScriptError::Sdk(Error::State(format!("need 2 phoenix crossbows: pair with a Phoenix insider at {:?}", bot.position()))));
            }
            solver::talk_like(bot, "katrine", &["crossbows", "yes"], None, 400)
        }
        _ => loot_and_finish(bot),
    }
}

fn join_phoenix(bot: &mut Bot) -> Result<(), ScriptError> {
    match solver::varp(bot, "phoenixgang") {
        0 => solver::talk_like(bot, "reldo", &["quest", "search"], None, 400),
        1 => {
            solver::operate_loc(bot, &["questbookcase"], 2, None)?;
            let book = solver::obj_id("the_shield_of_arrav")?;
            let (com, slot) = solver::inv_slot(bot, book)?;
            bot.op_held(1, book, slot, com)?;
            wait_ticks(bot, 10)?;
            bot.close_modal()?;
            bot.pump(5)?;
            Ok(())
        }
        2 => solver::talk_like(bot, "reldo", &["phoenix", "read", "shield"], None, 400),
        3 => {
            if solver::count(bot, solver::obj_id("coins")?) < 20 {
                return Err(ScriptError::Sdk(Error::State(format!("need 20 coins for Baraek at {:?}", bot.position()))));
            }
            solver::talk_like(bot, "baraek", &["phoenix", "have 20", "okay"], None, 400)
        }
        4 => {
            straven_goto(bot)?;
            solver::talk_like(bot, "straven", &["know who you are", "offer", "services"], None, 400)
        }
        8 => {
            let report = solver::obj_id("intelligence_report")?;
            if solver::count(bot, report) < 1 {
                solver::attack(bot, &["jonny_the_beard"], 600)?;
                solver::take_near(bot, "intelligence_report", 120)?;
            }
            straven_goto(bot)?;
            solver::talk_like(bot, "straven", &["report", "intelligence", "join"], None, 400)
        }
        9 => loot_and_finish(bot),
        _ => Err(ScriptError::Sdk(Error::State(format!("Phoenix join did not finish at {:?}", bot.position())))),
    }
}

// shortcut: the Phoenix HQ spawns at an unreachable tile (server map bug);
// committed scripts must not cheat, so walk there and fail loudly when
// the map refuses.
fn straven_goto(bot: &mut Bot) -> Result<(), ScriptError> {
    bot.intend("walking to Straven in the Phoenix HQ");
    bot.walk_to(3246, 9780, 0, 9000).map_err(|_| {
        ScriptError::Sdk(Error::Unreachable(format!("Straven tile 3246,9780 unreachable on foot from {:?}: server map bug, needs manual positioning", bot.position())))
    })
}

fn loot_and_finish(bot: &mut Bot) -> Result<(), ScriptError> {
    let half1 = solver::obj_id("arravshield1")?;
    let half2 = solver::obj_id("arravshield2")?;
    if solver::count(bot, half1) < 1 && solver::varp(bot, "phoenixgang") >= 9 {
        solver::operate_loc(bot, &["phoenixdoor"], 1, None)?;
        solver::operate_loc(bot, &["phoenixshutchest"], 1, None)?;
        solver::operate_loc(bot, &["phoenixopenchest"], 1, None)?;
        solver::operate_loc(bot, &["phoenixdoor"], 1, None)?;
    }
    if solver::count(bot, half2) < 1 && solver::varp(bot, "blackarmgang") >= 3 {
        solver::operate_loc(bot, &["blackarmdoor"], 1, None)?;
        solver::operate_loc(bot, &["blackarmcupboardshut"], 1, None)?;
        solver::operate_loc(bot, &["blackarmcupboardopen"], 2, None)?;
        solver::operate_loc(bot, &["blackarmdoor"], 1, None)?;
    }
    if solver::count(bot, half1) < 1 || solver::count(bot, half2) < 1 {
        return Err(ScriptError::Sdk(Error::State(format!("missing a shield half: pair with the other gang at {:?}", bot.position()))));
    }
    solver::talk_like(bot, "curator", &["retrieved", "shield", "reward", "certificate"], None, 400)?;
    if solver::count(bot, solver::obj_id("arravcertificate")?) < 1 {
        return Err(ScriptError::Sdk(Error::State(format!("curator did not issue the certificate at {:?}", bot.position()))));
    }
    solver::talk_like(bot, "king_roald", &["majesty", "reward", "claim", "certificate"], None, 400)?;
    if !solver::wait_complete(bot, 300)? {
        return Err(Error::Timeout(format!("shield of arrav never completed at {:?}", bot.position())).into());
    }
    Ok(())
}

fn solve_vampire(bot: &mut Bot) -> Result<(), ScriptError> {
    bot.intend("quest vampire: vampire slayer");
    if solver::varp(bot, "vampire") < 1 {
        solver::talk_like(bot, "morgan", &["adventure", "yes", "help"], None, 400)?;
    }
    if solver::count(bot, solver::obj_id("beer")?) < 1 {
        let beer = solver::obj_id("beer")?;
        if bot.bank_withdraw(beer, 1, 60).is_err() {
            return Err(ScriptError::Sdk(Error::State(format!("need a beer for Harlow at {:?}", bot.position()))));
        }
    }
    if solver::varp(bot, "vampire") < 2 {
        solver::talk_like(bot, "dr_harlow", &["morgan", "help", "beer"], None, 400)?;
        solver::talk_like(bot, "dr_harlow", &["beer", "ok mate", "here"], None, 400)?;
    }
    solver::operate_loc(bot, &["2614", "vampcoffin"], 1, None)?;
    solver::attack(bot, &["count_draynor"], 600)?;
    solver::wait_complete(bot, 300)?;
    Ok(())
}

/// Run one batch-B quest by key (`blackknight`, `demon`, `squire`, `hunt`,
/// `prince`, `priest`, `dragon`, `blackarmgang`, `vampire`). `args[0]`
/// selects the gang for `blackarmgang` (`phoenix` default).
pub fn run_quest(key: &str, bot: &mut Bot, args: &[String]) -> ScriptResult {
    let Some(requirements) = quest_requirements::by_key(key) else {
        return Err(ScriptError::Usage(format!("unknown batch-B quest {key:?}")));
    };
    match key {
        "blackknight" => solver::run_requirements(bot, &requirements, solve_blackknight),
        "demon" => solver::run_requirements(bot, &requirements, solve_demon),
        "squire" => solver::run_requirements(bot, &requirements, solve_squire),
        "hunt" => solver::run_requirements(bot, &requirements, solve_hunt),
        "prince" => solver::run_requirements(bot, &requirements, solve_prince),
        "priest" => solver::run_requirements(bot, &requirements, solve_priest),
        "dragon" => solver::run_requirements(bot, &requirements, solve_dragon),
        "blackarmgang" => {
            let gang = args.first().map(String::as_str);
            solver::run_requirements(bot, &requirements, |bot| solve_blackarmgang(bot, gang))
        }
        "vampire" => solver::run_requirements(bot, &requirements, solve_vampire),
        _ => Err(ScriptError::Usage(format!("quest {key:?} lives in batch A"))),
    }
}
