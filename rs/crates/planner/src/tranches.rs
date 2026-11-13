//! D21 reach tranches: level cuts per skill, the resource fixed for
//! each tranche and the script-side method. Split out of `expand`
//! (rule 4); API re-exported there.

/// Spot keyword of the melee script's training ladder (`melee ladder`,
/// `scripts::kit::ladder::LADDER_KEY`): no fixed spot.
pub const MELEE_LADDER: &str = "ladder";

/// D21 level tranches per skill: the brain fixes one resource per
/// tranche, the script never changes tier mid-task. P1 tiers prefer the
/// classic method (§8); one tier per 10 levels where the dump offers no
/// finer step.
pub fn tranches(skill: &str, from: u8, to: u8) -> Vec<u8> {
    let cuts: &[u8] = match skill {
        "fishing" => &[5, 10, 20, 25, 35, 40],
        "woodcutting" => &[15, 30, 45, 60, 75],
        "mining" => &[10, 15, 20, 30, 40, 55],
        "cooking" => &[10, 20, 30, 45],
        "firemaking" => &[15, 30, 60],
        "crafting" => &[5, 7, 8, 16, 20, 27, 34, 43, 70],
        "smithing" => &[15, 20, 30, 40, 50, 70, 85],
        "runecraft" => &[2, 5, 9, 14, 20],
        "attack" | "strength" | "defence" | "hitpoints" => &[10, 20, 30, 40],
        "magic" | "ranged" => &[10, 20, 30, 40],
        // Prayer buries where it fights (no standalone bury script):
        // one tranche, burial in the combat loop.
        "prayer" => &[],
        _ => &[10, 20, 30],
    };
    let mut out: Vec<u8> = cuts.iter().copied().filter(|cut| *cut > from && *cut < to).collect();
    out.push(to);
    out
}

/// Resource the brain fixes for one reach tranche (product of the tier
/// worked during the tranche).
pub fn tranche_resource(skill: &str, level: u8) -> String {
    match skill {
        "fishing" => {
            if level >= 40 {
                "raw_lobster"
            } else if level >= 35 {
                "raw_tuna"
            } else if level >= 25 {
                "raw_pike"
            } else if level >= 20 {
                "raw_trout"
            } else if level >= 5 {
                "raw_sardine"
            } else {
                "raw_shrimp"
            }
            .to_string()
        }
        "woodcutting" => {
            if level >= 60 {
                "yew_logs"
            } else if level >= 45 {
                "maple_logs"
            } else if level >= 30 {
                "willow_logs"
            } else if level >= 15 {
                "oak_logs"
            } else {
                "logs"
            }
            .to_string()
        }
        "mining" => {
            if level >= 55 {
                "mithril_ore"
            } else if level >= 40 {
                "gold_ore"
            } else if level >= 30 {
                "coal"
            } else if level >= 15 {
                "iron_ore"
            } else {
                "copper_ore"
            }
            .to_string()
        }
        "cooking" => {
            if level >= 30 {
                "raw_lobster"
            } else if level >= 15 {
                "raw_trout"
            } else {
                "raw_shrimp"
            }
            .to_string()
        }
        "firemaking" => {
            if level >= 60 {
                "yew_logs"
            } else if level >= 30 {
                "willow_logs"
            } else if level >= 15 {
                "oak_logs"
            } else {
                "logs"
            }
            .to_string()
        }
        "smithing" => {
            if level >= 85 {
                "rune_bar"
            } else if level >= 70 {
                "adamantite_bar"
            } else if level >= 50 {
                "mithril_bar"
            } else if level >= 40 {
                "gold_bar"
            } else if level >= 30 {
                "steel_bar"
            } else if level >= 20 {
                "silver_bar"
            } else if level >= 15 {
                "iron_bar"
            } else {
                "bronze_bar"
            }
            .to_string()
        }
        "crafting" => {
            if level >= 70 {
                "diamond_amulet"
            } else if level >= 43 {
                "diamond_ring"
            } else if level >= 34 {
                "ruby_ring"
            } else if level >= 27 {
                "emerald_ring"
            } else if level >= 20 {
                "sapphire_ring"
            } else if level >= 16 {
                "holy_symbol"
            } else if level >= 8 {
                "gold_amulet"
            } else if level >= 7 {
                "pie_dish"
            } else if level >= 5 {
                "gold_ring"
            } else {
                "ball_of_wool"
            }
            .to_string()
        }
        "runecraft" => {
            if level >= 20 {
                "bodyrune"
            } else if level >= 14 {
                "firerune"
            } else if level >= 9 {
                "earthrune"
            } else if level >= 5 {
                "waterrune"
            } else if level >= 2 {
                "mindrune"
            } else {
                "airrune"
            }
            .to_string()
        }
        "prayer" => "bones".to_string(),
        // Melee from 10 trains on the ladder (`melee ladder`: the script
        // picks the F2P spot from the live levels, gear, hitpoints and
        // food; audit 2026-11-13: combat 50-59 bots still on cows). The
        // fresh tranche stays on chickens.
        "attack" | "strength" | "defence" | "hitpoints" if level >= 10 => MELEE_LADDER.to_string(),
        // Real `combat_spots` keys (the old `chicken_lumbridge` named no
        // spot: melee silently auto-picked goblins and cows and died).
        "attack" | "strength" | "defence" | "hitpoints" | "ranged" | "magic" => {
            if level >= 10 {
                "cows"
            } else {
                "chickens"
            }
            .to_string()
        }
        _ => "chickens".to_string(),
    }
}

/// Script-side method for one reach tranche.
pub fn reach_method(skill: &str, level: u8) -> String {
    match skill {
        "fishing" => format!("gather:fishing/{}", tranche_resource(skill, level)),
        "woodcutting" => format!("gather:woodcut/{}", tranche_resource(skill, level)),
        "mining" => format!("gather:mining/{}", tranche_resource(skill, level)),
        "cooking" => format!("cook/{}", tranche_resource(skill, level)),
        "firemaking" => format!("fire:{}", tranche_resource(skill, level)),
        // Smelting trains smithing 1→99 (bronze 1, quest squire 1→29);
        // plates run through the money chains, never the reach tiers.
        "smithing" => format!("smelt:{}", tranche_resource(skill, level)),
        // One P1-first family per tier (D64): wheel, gold, pie dish,
        // gold amulet, silver symbol, then the gem ladder.
        "crafting" => {
            let resource = tranche_resource(skill, level);
            if resource == "ball_of_wool" {
                format!("spin:{resource}")
            } else if resource == "pie_dish" || resource == "pot" || resource == "bowl" {
                format!("pottery:{resource}")
            } else {
                format!("jewelry:{resource}")
            }
        }
        "runecraft" => format!("runecraft:{}", tranche_resource(skill, level)),
        // No standalone bury script (in-loop helper only): prayer fights
        // where the bones drop and buries each kill.
        "prayer" => "fight:melee/cows".to_string(),
        "ranged" => format!("fight:ranged/{}", tranche_resource(skill, level)),
        "magic" => format!("fight:magic/{}", tranche_resource(skill, level)),
        "attack" | "strength" | "defence" | "hitpoints" => {
            format!("fight:melee/{}", tranche_resource(skill, level))
        }
        _ => format!("fight:melee/{}", tranche_resource(skill, level)),
    }
}
