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
    // Smithing cuts at every anvil unlock that changes what pays (a new
    // metal, more bars per press, a new bar to smelt): each tranche re-
    // picks its product at resolve (`training`).
    let smithing = crate::training::smithing_cuts();
    let cuts: &[u8] = match skill {
        "fishing" => &[5, 10, 20, 25, 35, 40],
        "woodcutting" => &[15, 30, 45, 60, 75],
        "mining" => &[10, 15, 20, 30, 40, 55],
        "cooking" => &[10, 20, 30, 45],
        "firemaking" => &[15, 30, 60],
        "crafting" => &[5, 7, 8, 16, 20, 27, 34, 43, 70],
        "smithing" => smithing,
        "runecraft" => &[2, 5, 9, 14, 20],
        "attack" | "strength" | "defence" | "hitpoints" => &[10, 20, 30, 40],
        // Magic: combat spells below 21; from low alchemy (21) and high
        // alchemy (55) the method is priced at resolve, a tranche every
        // five levels so the choice follows the market.
        "magic" => &[10, 20, 21, 25, 30, 35, 40, 45, 50, 55, 60, 65, 70, 75, 80, 85, 90, 95],
        "ranged" => &[10, 20, 30, 40],
        // Prayer buries banked bones (`bury_loot`), bones then big bones.
        "prayer" => &[10, 20, 30, 40],
        _ => &[10, 20, 30],
    };
    let mut out: Vec<u8> = cuts.iter().copied().filter(|cut| *cut > from && *cut < to).collect();
    out.push(to);
    out
}

/// F2P runes by level gate, best first (server `runecraft.dbrow`).
const RUNECRAFT_TIERS: [(u8, &str); 6] =
    [(20, "bodyrune"), (14, "firerune"), (9, "earthrune"), (5, "waterrune"), (2, "mindrune"), (1, "airrune")];

/// True when the brain has a way to a talisman: the quest that hands it
/// over (`quests::QUEST_GRANTS`) or a production leg a script runs (a
/// drop a combat rung farms).
/// Static data: computed once per process (the brain asks every pass).
pub fn talisman_obtainable(talisman: &str) -> bool {
    static OBTAINABLE: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    OBTAINABLE
        .get_or_init(|| {
            RUNECRAFT_TIERS
                .iter()
                .filter_map(|(_, rune)| crate::tools::rune_talisman(rune))
                .filter(|name| {
                    crate::quests::QUEST_GRANTS.iter().any(|(_, granted)| granted == name) || !crate::obtain_legs::production_legs(name).is_empty()
                })
                .collect()
        })
        .iter()
        .any(|known| known == talisman)
}

/// Resource the brain fixes for one reach tranche (product of the tier
/// worked during the tranche).
pub fn tranche_resource(skill: &str, level: u8) -> String {
    match skill {
        // Lobster (40) and tuna (35) are Karamja water (paid boat, 60
        // coins a round trip): the tiers stay on the mainland, salmon (30,
        // fly rod and feathers) the best catch past 30.
        "fishing" => {
            if level >= 30 {
                "raw_salmon"
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
            } else if level >= 30 {
                // Maples have no F2P site: willows until yews.
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
        // Each tier at its server level: salmon 25, trout 15. Raw lobster,
        // tuna and swordfish come only from Karamja water (paid boat):
        // salmon stays the top mainland tier.
        "cooking" => {
            if level >= 25 {
                "raw_salmon"
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
        // The bar the tranche's anvil product eats (best metal open).
        "smithing" => crate::training::default_smith_bar(level).to_string(),
        "crafting" => {
            if level >= 70 {
                "unstrung_diamond_amulet"
            } else if level >= 43 {
                "diamond_ring"
            } else if level >= 34 {
                "ruby_ring"
            } else if level >= 27 {
                "emerald_ring"
            } else if level >= 20 {
                "sapphire_ring"
            } else if level >= 16 {
                "nostringstar"
            } else if level >= 8 {
                "unstrung_gold_amulet"
            } else if level >= 7 {
                "piedish"
            } else if level >= 5 {
                "gold_ring"
            } else {
                "ball_of_wool"
            }
            .to_string()
        }
        // The best rune open at the level whose talisman the brain can
        // obtain (`talisman_obtainable`): air to 9, earth to 20, body
        // past it (mind, water and fire talismans drop only from NPCs no
        // combat rung hunts).
        "runecraft" => RUNECRAFT_TIERS
            .iter()
            .find(|(gate, rune)| level >= *gate && crate::tools::rune_talisman(rune).is_some_and(|talisman| talisman_obtainable(&talisman)))
            .map(|(_, rune)| rune.to_string())
            .unwrap_or_else(|| "airrune".to_string()),
        // Bones (4.5 XP, every chicken and cow) to 20, big bones (15 XP)
        // after: the market or a giant spot feeds the climb.
        "prayer" => if level >= 20 { "big_bones" } else { "bones" }.to_string(),
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
        // A player smelts bars then hammers them (2026-11-14): the static
        // tranche method is the anvil product of the best metal open at
        // the start (most bars per press, `training::default_smith_
        // product`); the brain re-picks product or bar with the market at
        // resolve (`training::rank_smithing`).
        "smithing" => format!("smith:{}", crate::training::default_smith_product(level)),
        // One P1-first family per tier (D64): wheel, gold, pie dish,
        // gold amulet, silver symbol, then the gem ladder.
        "crafting" => {
            let resource = tranche_resource(skill, level);
            if resource == "ball_of_wool" {
                format!("spin:{resource}")
            } else if resource == "piedish" || resource == "pot_empty" || resource == "bowl_empty" {
                format!("pottery:{resource}")
            } else {
                format!("jewelry:{resource}")
            }
        }
        "runecraft" => format!("runecraft:{}", tranche_resource(skill, level)),
        // Prayer buries banked bones (`bury_loot <bones>`): the bones are
        // the tranche's consumption, sized by its XP.
        "prayer" => format!("bury:{}", tranche_resource(skill, level)),
        "ranged" => format!("fight:ranged/{}", tranche_resource(skill, level)),
        // The best combat spell open at the tranche's start: its runes are
        // sized per cast (`sizing`) and the script casts it (`--spell`).
        "magic" => {
            let spot = tranche_resource(skill, level);
            format!("fight:magic/{}/{spot}", crate::method_way::best_combat_spell_at(level, &spot))
        }
        "attack" | "strength" | "defence" | "hitpoints" => {
            format!("fight:melee/{}", tranche_resource(skill, level))
        }
        _ => format!("fight:melee/{}", tranche_resource(skill, level)),
    }
}

