//! Tutorial stage detection: exact server box texts, never fragments.
//!
//! Matcher table audited against the live corpus
//! (`lostcity/content/scripts/tutorial/scripts/tut_chatbox_steps.rs2`,
//! 74 boxes, `~tutorialstep(title, body)`): every reachable box matches
//! exactly one row. Corpus test below locks that.
//!
//! Past collisions (all fixed here, all covered by the corpus test):
//! - the prospect `please wait` box also contains "prospect": a negative
//!   fragment keeps it on `wait` so waits pump instead of re-prospecting.
//! - "Sit back and watch." contains "attack the rat": `melee` requires
//!   its own title ("attacking.") so the watch box maps to `melee_watch`.
//! - the post-prospect boxes ("It's tin/copper. Talk to the mining
//!   instructor...") matched nothing: new `prospect_done` row.
//!
//! Live signals the fallback deliberately does NOT use: the tutorial varp
//! (id 281, `transmit=false`, the live client never receives it).

use lifescape_sdk::Bot;

use crate::ScriptError;
use crate::quests::common as kit;

pub(crate) fn obj(name: &str) -> Result<u16, ScriptError> {
    kit::obj_id(name)
}

pub(crate) fn has(bot: &Bot, name: &str, count: u32) -> bool {
    obj(name).ok().is_some_and(|id| bot.inventory().get(&id).copied().unwrap_or(0) >= count)
}

// (step key, instruction fragments) in tutorial order.
pub(crate) const STAGE_MATCHERS: &[(&str, &[&str])] = &[
    ("guide", &["getting started"]), ("controls_tab", &["spanner icon"]), ("door1", &["interacting with scenery"]),
    ("survival_start", &["moving around", "survival expert"]), ("open_inventory", &["flashing backpack icon"]),
    ("chop", &["cut down a tree"]), ("wait", &["please wait"]), ("light_fire", &["building a fire"]),
    ("open_stats", &["bar graph icon"]), ("survival_food", &["these are your stats"]),
    ("catch", &["catch some shrimp", "fishing spot"]), ("cook_burnt", &["cooking your shrimp"]),
    ("cook_fresh", &["burning your shrimp"]), ("survival_gate", &["just cooked your first"]),
    ("chef_path", &["mini-map"]), ("chef", &["talk to the chef"]), ("make_dough", &["making dough"]),
    ("bake", &["cooking dough"]), ("music_tab", &["first loaf of bread"]),
    ("chef_door", &["the music player", "next door"]), ("run_controls", &["running man"]),
    ("run_button", &["click on the run button"]), ("quest_door", &["run to the next guide"]),
    ("quest_talk", &["talk with the quest guide"]), ("quest_tab", &["open the quest journal"]),
    ("quest_recap", &["your quest journal"]), ("ladder_down", &["enter some caves"]),
    ("mining_intro", &["mining and smithing"]), ("dig_tin_first", &["mine one tin ore"]),
    ("dig_copper", &["need some copper"]), ("dig_tin", &["need some tin"]), ("prospect", &["prospect"]),
    ("prospect_done", &["talk to the mining instructor"]), ("smelt", &["smelting"]),
    ("smith_talk", &["made a bronze bar"]), ("smith_anvil", &["smithing a dagger"]),
    ("mine_exit", &["finished this area"]), ("combat_intro", &["find out about combat"]),
    ("wield_tab", &["wielding weapons"]), ("wield", &["select wield from the menu"]),
    ("sword_talk", &["now holding your dagger"]), ("sword_equip", &["wield the sword and shield"]),
    ("combat_tab", &["crossed swords"]), ("rat_cage", &["tools needed for battle"]),
    ("melee", &["attacking.", "to attack the rat"]), ("melee_watch", &["sit back and watch"]),
    ("ranged_prep", &["made your first kill"]), ("ranged_kill", &["rat ranging"]),
    ("ladder_bank", &["completed the tasks here"]), ("bank_booth", &["bank of runescape"]),
    ("bank_door", &["this is your bank box"]), ("advisor_talk", &["financial advice"]),
    ("advisor_door", &["continue through the next door"]), ("chapel_path", &["follow the path to the chapel"]),
    ("prayer_tab", &["prayer menu", "flashing icon"]), ("chapel_recap", &["tell you about prayers"]),
    ("friends_tab", &["flashing smily"]), ("ignore_tab", &["this is your friends list"]),
    ("chapel_final", &["this is your ignore list"]), ("chapel_door", &["almost finished", "pass through the"]),
    ("terrova_talk", &["wizards house"]), ("magic_tab", &["open up your final menu"]),
    ("oops_runes", &["unhandled tutorial step"]), ("wind", &["several tries"]),
    ("wind_retry", &["didn't have any effect"]), ("final", &["almost completed the tutorial"]),
];

/// Rows that must NOT match even when their fragments do: the prospect
/// `please wait` box mentions prospecting while the character is busy.
const STAGE_NEGATIVES: &[(&str, &[&str])] = &[("prospect", &["please wait"])];

/// Every row matching the (lowercased) instruction text, in table order.
pub(crate) fn all_matches(lowered: &str) -> Vec<&'static str> {
    STAGE_MATCHERS
        .iter()
        .filter(|(key, fragments)| {
            fragments.iter().all(|fragment| lowered.contains(fragment))
                && !STAGE_NEGATIVES
                    .iter()
                    .filter(|(negative_key, _)| negative_key == key)
                    .flat_map(|(_, negatives)| negatives.iter())
                    .any(|negative| lowered.contains(negative))
        })
        .map(|(key, _)| *key)
        .collect()
}

/// Test seam: first table row matching the raw instruction text.
#[cfg(test)]
pub(crate) fn match_stage(text: &str) -> Option<&'static str> {
    all_matches(&text.to_lowercase()).into_iter().next()
}

/// Re-entry stage when the box is empty or delayed (login, death, resume):
/// geography plus kit only. Every target handler is self-healing (it
/// re-grants, re-mines or re-talks before failing), so a stale pick
/// self-corrects on the next loop once the box reappears.
pub(crate) fn fallback_stage_for(x: i32, z: i32, has: &dyn Fn(&str) -> bool) -> Option<&'static str> {
    if z > 9000 {
        // Same mine/surface split as tutorial::is_mine_region.
        if x >= 3105 && z >= 9515 {
            return Some(if has("shortbow") { "ranged_kill" } else { "melee" });
        }
        if x >= 3105 {
            return Some("combat_intro");
        }
        if has("bronze_bar") {
            return Some("smith_anvil");
        }
        if has("copper_ore") && has("tin_ore") {
            return Some("smelt");
        }
        if has("bronze_pickaxe") {
            return Some("dig_tin_first");
        }
        return Some("mining_intro");
    }
    if x > 3200 {
        return None;
    }
    if x >= 3133 {
        return Some(if has("airrune") && has("mindrune") { "wind" } else { "terrova_talk" });
    }
    if (3115..=3133).contains(&x) && (3095..=3115).contains(&z) {
        return Some("chapel_path");
    }
    if z >= 3108 {
        if x >= 3124 {
            return Some("advisor_talk");
        }
        if x >= 3100 {
            return Some("bank_booth");
        }
        if x >= 3083 {
            return Some("quest_talk");
        }
        return Some("quest_door");
    }
    if z >= 3078 && z < 3096 && x < 3092 {
        if has("bread") {
            return Some("music_tab");
        }
        if has("bread_dough") {
            return Some("bake");
        }
        if has("newbie_pot_flour") {
            return Some("make_dough");
        }
        return Some("chef");
    }
    if z >= 3088 {
        if z >= 3100 && x >= 3090 {
            return Some("guide");
        }
        if x < 3089 {
            return Some("chef_path");
        }
        if has("net") {
            return Some("catch");
        }
        if has("bronze_axe") {
            return Some("chop");
        }
        return Some("survival_start");
    }
    Some("guide")
}

pub(crate) fn fallback_stage(bot: &Bot) -> Option<&'static str> {
    let (x, z) = (bot.world.x, bot.world.z);
    fallback_stage_for(x, z, &|name| has(bot, name, 1))
}

#[cfg(test)]
mod tests {
    use super::{all_matches, fallback_stage_for, match_stage};

    /// Live com join: title, then split lines glued with spaces. The server
    /// splits bodies on `|` (see `tutorialstep_page`), so `|` becomes a space.
    fn box_text(title: &str, body: &str) -> String {
        format!("{title}\n{}", body.replace('|', " "))
    }

    // Exact (title, body) pairs from tut_chatbox_steps.rs2. Two boxes stay
    // unmapped on purpose: the legacy `%tutorial == 2` left-click box (named
    // constants start at 0/1, no path ever sets 2) and the low-memory music
    // box (the SDK never sends the lowmem command, the server stays hi-mem).
    const CORPUS: &[(Option<&str>, &str, &str)] = &[
        (Some("guide"), r#"Getting started"#, r#"To start the tutorial use your left mouse-button to click|on the 'RuneScape Guide' in this room. He is indicated|by a flashing yellow arrow above his head. If you can't|see him, use your keyboard's arrow keys to rotate the view."#),
        (Some("controls_tab"), r#""#, r#"Player controls|Please click on the flashing spanner icon found at the bottom right of your screen. This will display your player controls."#),
        (None, r#"Player controls"#, r#"To start the tutorial use your left-mouse button to click on the|'RuneScape Guide' in this room. He is indicated by a flashing|yellow arrow above his head. If you can't see him, use your keyboard's arrow keys to rotate the view."#),
        (Some("door1"), r#"Interacting with scenery"#, r#"You can interact with many items of scenery by simply clicking on them. Right clicking will also give more options. Feel free to try it with the things in this room, then click on the door indicated with the yellow arrow to go through to the next instructor."#),
        (Some("survival_start"), r#"Moving around"#, r#"Follow the path to find the next instructor.|Clicking on the ground will walk you to that point.|Talk to the survival expert by the pond to continue the tutorial.|Remember you can rotate the view by pressing the arrow keys."#),
        (Some("open_inventory"), r#"Viewing the items that you were given."#, r#"Click on the flashing backpack icon to the|right side of the main window to view your inventory.|Your inventory is a list of everything you have in your backpack."#),
        (Some("chop"), r#"Cut down a tree"#, r#"You can click on the backpack icon at any time|to view the items that you currently have in your inventory.|You will see that you now have an axe in your inventory.|Use this to get some logs by clicking on the indicated tree."#),
        (Some("wait"), r#"Please wait..."#, r#"|Your character is now attempting to cut down the tree.|Sit back for a moment whilst <text_gender("he", "she")> does all the hard work."#),
        (Some("light_fire"), r#"Building a fire"#, r#"Well done - you managed to cut some logs from the tree!|Next use the tinderbox in your inventory to light the logs.|a) First click on the tinderbox to 'use it'.|b) Then click on the logs in your inventory to light them."#),
        (Some("wait"), r#"Please wait..."#, r#"|Your character is now attempting to light the fire.|This should only take a few seconds."#),
        (Some("open_stats"), r#"You gained some experience..."#, r#"Click on the flashing bar graph icon|near the inventory button|to see your skill abilities."#),
        (Some("survival_food"), r#"These are your stats."#, r#"Here you will see how good your skills are. Moving your|mouse over the icons will show you the exact amount of|experience for that skill in the bottom panel.|Speak to the instructor to continue."#),
        (Some("catch"), r#"Catch some Shrimp."#, r#"Click on the fishing spot, indicated by the flashing arrow.|Remember you can check your inventory by clicking|the backpack icon."#),
        (Some("wait"), r#"Please wait..."#, r#"This should only take a few seconds.|As you gain fishing experience you'll find that|there are many types of fish and|many ways to catch them."#),
        (Some("cook_burnt"), r#"Cooking your shrimp."#, r#"Now right click on the shrimp and select the use option.|Next left click on the fire you just lit.|If while doing this you look in the top left of the screen,|you will see the instruction that you're giving your character."#),
        (Some("cook_fresh"), r#"Burning your shrimp."#, r#"You have just burnt your first shrimp. This is normal.|As you get more experience in cooking you will burn stuff less.|Let's try cooking without burning it this time.|First catch some shrimp, then use them on a fire."#),
        (Some("survival_gate"), r#"Well done, you've just cooked your first RuneScape meal."#, r#"If you'd like a recap on anything you've learned so far speak|to Brynna. You can now move on to the next instructor.|Click to the gate shown and follow the path.|Remember you can move the camera with the arrow keys."#),
        (Some("chef_path"), r#"Find your next instructor."#, r#"Follow the path until you get to the door with the yellow arrow|above it. Click on the door to open it. Notice the mini-map in|the top right, this shows a top down view of the area around|you. This can also be used for navigation."#),
        (Some("chef"), r#"Find your next instructor."#, r#"Talk to the chef indicated. He will teach you the more advanced|aspects of cooking such as combining ingredients. He will also|teach you about your music player menu as well."#),
        (Some("make_dough"), r#"Making dough."#, r#"This is the base for many of the meals. To make dough we must|mix flour and water. So first right click the bucket of water and|select use then left click on the pot of flour."#),
        (Some("bake"), r#""#, r#"Cooking dough. Now you have dough made you can cook it. To|cook the dough use it with the range shown by the arrow. If you|lose your dough talk to Lev, he will give you more ingredients."#),
        (Some("music_tab"), r#"Well done, your first loaf of bread. As you gain experience in"#, r#"cooking you will be able to make other things like pies, cakes|and even kebabs. Now you've got the hang of cooking let's|move on. Click on the flashing icon in the bottom right to see|the jukebox."#),
        (None, r#"The Music Player."#, r#"As you are playing in low memery mode you will not be able to|listen to the music of RuneScape, but this is where you would|find the jukebox controls. Pass through the door to continue. If you need a recap on anything covered here talk to Lev."#),
        (Some("chef_door"), r#"The Music Player."#, r#"From this interface you can control the music that is played.|As you explore the world more of the tunes will become unlocked.|Once you've examined this menu use the next door to continue.|If you need a recap on anything covered here talk to Lev."#),
        (Some("run_controls"), r#"It's only a short distance to the next guide."#, r#"|Why not try running there. Start by opening the player|controls, that's the flashing icon of a running man."#),
        (Some("run_button"), r#"Running."#, r#"In this menu you will see many options from waving to walking.|At the top of the panel there are two buttons. One is walk the|other one is run. Click on the run button."#),
        (Some("quest_door"), r#"Run to the next guide."#, r#"Now that you have run turned on follow the path,|until you come to the end. You may notice that your|energy left goes down. If this reaches zero you'll stop running.|Click on the door to pass through it."#),
        (Some("quest_talk"), r#""#, r#"Talk with the Quest Guide Instructor.||He'll tell you all about quests."#),
        (Some("quest_tab"), r#""#, r#"Open the Quest journal.||Click on the flashing icon next to your inventory."#),
        (Some("quest_recap"), r#""#, r#"Your Quest journal.|This is your quest journal, a list of all the quests in the game.|Talk to the instructor again for an explanation."#),
        (Some("ladder_down"), r#""#, r#"Moving on.|It's time to enter some caves. Click on the ladder to go down to|the next area."#),
        (Some("mining_intro"), r#"Mining and smithing."#, r#"Next let's get you a weapon, or more to the point,|you can make your first weapon yourself.|Don't panic, the mining instructor will help you.|Talk to him and he'll tell you all about it."#),
        (Some("wait"), r#"Please wait..."#, r#"|Your character is now attempting to prospect the rock. This should take only a few seconds."#),
        (Some("prospect"), r#"Prospecting"#, r#"To prospect a mineable rock just right click it and select the|prospect rock option. This will tell you the type of ore you can|mine from it. Try it now on one of the rocks indicated."#),
        (Some("prospect"), r#"It's copper."#, r#"|So you know there's copper in the brown rocks, try prospecting|the grey ones next."#),
        (Some("prospect"), r#"It's tin."#, r#"|So you know there's tin in the grey rocks, try prospecting|the brown ones next."#),
        (Some("prospect_done"), r#"It's tin."#, r#"Talk to the mining instructor to find out about these types of|ore and how you can mine them. He'll give you the needed tools|too."#),
        (Some("prospect_done"), r#"It's copper."#, r#"Talk to the mining instructor to find out about these types of|ore and how you can mine them. He'll give you the needed tools|too."#),
        (Some("wait"), r#"Please wait..."#, r#"|Your character is now attempting to mine the rock. This should take only a few seconds."#),
        (Some("dig_tin_first"), r#"Mining."#, r#"It's quite simple really, all you need to do is right click on the rock and select mine. You can only mine when you have a pickaxe. So give it a try, first mine one tin ore."#),
        (Some("dig_tin"), r#"Mining."#, r#"Now you have some copper ore you just need some tin ore, then you'll have all you need to create a bronze bar. As you did|before right click on the tin rock and select 'mine'."#),
        (Some("dig_copper"), r#"Mining."#, r#"Now you have some tin ore you just need some copper ore, then you'll have all you need to create a bronze bar. As you did|before right click on the copper rock and select 'mine'."#),
        (Some("smelt"), r#"Smelting."#, r#"You should now have both copper and tin ore. So let's smelt|them to make a bronze bar. To do this right click on either tin or|copper ore and select use then left click on the furnace. Try it now."#),
        (Some("smith_talk"), r#"You've made a bronze bar!"#, r#"|Speak to Dezzick and he'll show you how to make it into a weapon."#),
        (Some("smith_anvil"), r#"Smithing a dagger."#, r#"To smith you'll need a hammer like the one you were given by Dezzick, access to an anvil like the one with the arrow over it|and enough metal bars to make what you are trying to smith.|To start the process use the bar on one of the anvils."#),
        (Some("mine_exit"), r#"You've finished this area."#, r#"So let's move on. Go through the gates shown by the arrow. Remember you may need to move the camera to see your surroundings. Speak to the guide for a recap at any time."#),
        (Some("combat_intro"), r#"Combat."#, r#"In this area you will find out about combat,|both melee and ranged.|Speak to the guide and he'll tell you all about it."#),
        (Some("wield_tab"), r#""#, r#"Wielding weapons.|You now have access to a new interface. Click on the flashing icon of a man, the one to the right of your backpack icon."#),
        (Some("wield"), r#"This is your worn inventory."#, r#"From here you can see what items you have equipped.|Let's get one of those slots filled, go back to your inventory|and right click your dagger, select wield from the menu."#),
        (Some("sword_talk"), r#"You're now holding your dagger."#, r#"If you check your worn inventory you will see it in your right hand. Clothes, armour, weapons and many other items are equipped like this. You can unequip items by clicking on the |worn inventory. Now go and speak to Vannaka to continue."#),
        (Some("sword_equip"), r#"Unequipping items."#, r#"In your worn inventory panel right click on the dagger and|select the remove option from the drop down list. After you've unequipped the dagger wield the sword and shield. As you pass the mouse over an item you will see its name appear at the top left."#),
        (Some("combat_tab"), r#"Combat interface."#, r#"|Click on the flashing crossed swords icon to see the combat interface."#),
        (Some("rat_cage"), r#"This is your Combat interface."#, r#"From this interface you can select the type of attack your| character will use. Different monsters have different|weaknesses. Now you have all the tools needed for battle why not slay some rats. Click on the gates indicated to continue."#),
        (Some("melee"), r#"Attacking."#, r#"|To attack the rat right click it and select the attack option. You will then walk over to it and start hitting it."#),
        (Some("melee_watch"), r#"Sit back and watch."#, r#"While you are fighting you will see a bar over your head.|The bar shows how much health you have left. Your opponent|will have one too. You will continue to|attack the rat until it's dead or you do something else."#),
        (Some("ranged_prep"), r#"Well done, you've made your first kill!"#, r#"|Pass through the gate and talk to Vannaka, he will give you your next task."#),
        (Some("ranged_kill"), r#"Rat ranging."#, r#"Now you have a bow and some arrows. Before you can use|them you'll need to equip them. Once equipped with the|ranging gear try killing another rat. Remember, to attack right click on the monster and select attack."#),
        (Some("ladder_bank"), r#"Moving on."#, r#"You have completed the tasks here, to move on click on the|ladder shown. If you need to go over any of what you learned|here just talk to Vannaka and he'll tell you what he can."#),
        (Some("bank_booth"), r#"Banking."#, r#"Follow the path and you will come to the front of the building. This is the 'Bank of RuneScape' where you can store all your most valued items. To open your bank box just right click on an open booth indicated and select use."#),
        (Some("bank_door"), r#"This is your bank box."#, r#"You can store stuff here for safe keeping. If you die,|anything in your bank will be saved. To deposit something right|click it and select store. Once you've had a good look close the|window and move on through the door indicated."#),
        (Some("advisor_talk"), r#"Financial advice."#, r#"|The guide here will tell you all about making cash. Just click on him to hear what he's going to say."#),
        (Some("advisor_door"), r#""#, r#"|Continue through the next door."#),
        (Some("chapel_path"), r#""#, r#"Follow the path to the chapel and enter it.|Once inside talk to the monk.|He'll tell you all about prayer."#),
        (Some("prayer_tab"), r#""#, r#"Your prayer menu.||Click on the flashing icon to open the prayer menu."#),
        (Some("chapel_recap"), r#""#, r#"Your prayer menu.||Talk with Brother Brace and he'll tell you about prayers."#),
        (Some("friends_tab"), r#""#, r#"You should now see another new icon. Click on flashing smily to open your friends list."#),
        (Some("ignore_tab"), r#"This is your friends list."#, r#"|This will be explained by Brother Brace shortly, but first click|on the other flashing icon next to the friends list one."#),
        (Some("chapel_final"), r#"This is your ignore list."#, r#"The two lists; friends and ignore, can be very helpful for|keeping track of when your friends are online or for blocking messages from people you simply don't like. Speak with|Brother Brace and he will tell you more."#),
        (Some("chapel_door"), r#""#, r#"Your final instructor!|You're almost finished on tutorial island. Pass through the|door to find the path leading to your last instructor."#),
        (Some("terrova_talk"), r#"Your final instructor!"#, r#"Just follow the path to the wizards house,|where you will be shown how to cast spells.|Just talk with the mage indicated to find out more."#),
        (Some("magic_tab"), r#"Open up your final menu."#, r#"|Open up the Magic menu by clicking on the flashing icon next to the Prayer button you just learned about."#),
        (Some("wind"), r#"Cast Wind Strike at a chicken."#, r#"Now you have runes you should see the Wind Strike icon at the top left corner of the Magic interface. Walk over to the caged chickens, click the Wind Strike icon and then select one of the chickens to cast it on. It may take several tries."#),
        (Some("wind_retry"), r#"Cast Wind Strike at a chicken."#, r#"That's it, you cast a spell! Sadly it didn't have any effect this time, but the more you practice, the better you'll get. Repeat this process until you successfully cast the spell. Click the Wind Strike icon again and then select one of the chickens."#),
        (Some("final"), r#"You have almost completed the tutorial!"#, r#"|All you need to do now is move on to the mainland.|Just speak with Terrova and he'll teleport you to Castle Lumbridge.|"#),
    ];

    #[test]
    fn every_live_box_maps_to_exactly_one_stage() {
        assert_eq!(CORPUS.len(), 74, "one row per live ~tutorialstep box");
        for (want, title, body) in CORPUS {
            let text = box_text(title, body);
            assert_eq!(&match_stage(&text), want, "box {title:?}");
            let hits = all_matches(&text.to_lowercase());
            let want_count = if want.is_some() { 1 } else { 0 };
            assert_eq!(hits.len(), want_count, "box {title:?} candidates {hits:?}");
        }
    }

    #[test]
    fn prospect_wait_pumps_instead_of_reprospecting() {
        let text = box_text("Please wait...", "|Your character is now attempting to prospect the rock. This should take only a few seconds.");
        assert_eq!(match_stage(&text), Some("wait"));
    }

    #[test]
    fn sit_back_watch_is_not_plain_melee() {
        let watch = box_text(
            "Sit back and watch.",
            "While you are fighting you will see a bar over your head.|The bar shows how much health you have left. Your opponent|will have one too. You will continue to|attack the rat until it's dead or you do something else.",
        );
        assert_eq!(match_stage(&watch), Some("melee_watch"));
        let attack = box_text(
            "Attacking.",
            "|To attack the rat right click it and select the attack option. You will then walk over to it and start hitting it.",
        );
        assert_eq!(match_stage(&attack), Some("melee"));
    }

    #[test]
    fn first_kill_box_is_unambiguous_ranged_prep() {
        let text = box_text(
            "Well done, you've made your first kill!",
            "|Pass through the gate and talk to Vannaka, he will give you your next task.",
        );
        assert_eq!(match_stage(&text), Some("ranged_prep"));
        assert_eq!(all_matches(&text.to_lowercase()), vec!["ranged_prep"]);
    }

    #[test]
    fn empty_text_matches_nothing() {
        assert_eq!(match_stage(""), None);
        assert_eq!(match_stage("   \n  "), None);
    }

    #[test]
    fn iron_surface_case_falls_back_to_quest_door() {
        // Live constat: (3072, 3133), empty box, stale `ranged_prep`
        // intention. With no text the geography fallback must pick the
        // quest chain, never a mine stage.
        let none = |_: &str| false;
        let stage = fallback_stage_for(3072, 3133, &none);
        assert_eq!(stage, Some("quest_door"));
        assert_ne!(stage, Some("ranged_prep"));
    }

    #[test]
    fn fallback_covers_spawn_mine_and_surface() {
        let none = |_: &str| false;
        assert_eq!(fallback_stage_for(3095, 3107, &none), Some("guide"));
        assert_eq!(fallback_stage_for(3103, 3095, &|item| item == "bronze_axe"), Some("chop"));
        assert_eq!(fallback_stage_for(3103, 3095, &|item| item == "net"), Some("catch"));
        assert_eq!(fallback_stage_for(3080, 3086, &|item| item == "newbie_pot_flour"), Some("make_dough"));
        assert_eq!(fallback_stage_for(3080, 3086, &|item| item == "bread_dough"), Some("bake"));
        assert_eq!(fallback_stage_for(3080, 3086, &|item| item == "bread"), Some("music_tab"));
        assert_eq!(fallback_stage_for(3086, 3121, &none), Some("quest_talk"));
        assert_eq!(fallback_stage_for(3079, 9505, &none), Some("mining_intro"));
        assert_eq!(fallback_stage_for(3079, 9505, &|item| item == "bronze_pickaxe"), Some("dig_tin_first"));
        let ores = |item: &str| item == "copper_ore" || item == "tin_ore";
        assert_eq!(fallback_stage_for(3079, 9505, &ores), Some("smelt"));
        assert_eq!(fallback_stage_for(3079, 9505, &|item| item == "bronze_bar"), Some("smith_anvil"));
        assert_eq!(fallback_stage_for(3107, 9510, &none), Some("combat_intro"));
        assert_eq!(fallback_stage_for(3110, 9518, &none), Some("melee"));
        assert_eq!(fallback_stage_for(3110, 9518, &|item| item == "shortbow"), Some("ranged_kill"));
        assert_eq!(fallback_stage_for(3110, 3120, &none), Some("bank_booth"));
        assert_eq!(fallback_stage_for(3128, 3124, &none), Some("advisor_talk"));
        assert_eq!(fallback_stage_for(3125, 3106, &none), Some("chapel_path"));
        assert_eq!(fallback_stage_for(3141, 3090, &none), Some("terrova_talk"));
        let runes = |item: &str| item == "airrune" || item == "mindrune";
        assert_eq!(fallback_stage_for(3141, 3090, &runes), Some("wind"));
        assert_eq!(fallback_stage_for(3210, 3220, &none), None);
    }
}
