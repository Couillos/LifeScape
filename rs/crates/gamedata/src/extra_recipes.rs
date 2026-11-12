//! Missing F2P recipes (M10b §2): spinning, pottery, jewellery,
//! amulet stringing, cook prep, runecraft, firemaking, burying and
//! utility magic. XP are raw server tenths from the 274 scripts
//! (`spinning.struct`, `pottery.struct`, `jewellery.struct`,
//! `gem.dbrow`, `runecraft.dbrow`, `firemaking.obj`, `bones.obj`,
//! `spells.json` for utilities); levels match the catalogue §5.
//! Every input, tool and product is `members == Some(false)`.

/// One extra recipe row (static, mirrored into [`ExtraRecipe`]).
pub(crate) struct StaticExtra {
    pub kind: &'static str,
    pub level: i64,
    pub xp: i64,
    pub inputs: &'static [(&'static str, i64)],
    pub outputs: &'static [(&'static str, i64)],
    pub station: Option<&'static str>,
    pub source: &'static str,
}

/// One missing recipe.
#[derive(Debug, Clone)]
pub struct ExtraRecipe {
    pub kind: String,
    pub level: i64,
    pub xp: i64,
    pub inputs: Vec<(String, i64)>,
    pub outputs: Vec<(String, i64)>,
    pub station: Option<String>,
    pub source: String,
}

fn from_static(row: &StaticExtra) -> ExtraRecipe {
    ExtraRecipe {
        kind: row.kind.to_string(),
        level: row.level,
        xp: row.xp,
        inputs: row.inputs.iter().map(|(name, count)| ((*name).to_string(), *count)).collect(),
        outputs: row.outputs.iter().map(|(name, count)| ((*name).to_string(), *count)).collect(),
        station: row.station.map(str::to_string),
        source: row.source.to_string(),
    }
}

// Station names match `stations::stations_of` kinds; `None` is inventory.
static ALL: &[StaticExtra] = &[
    // Wool spinning (spinning.struct: wool 25; flax is members, excluded).
    StaticExtra { kind: "spin", level: 1, xp: 25, inputs: &[("wool", 1)], outputs: &[("ball_of_wool", 1)], station: Some("spinningwheel"), source: "spin_wool" },
    // Soft clay (no XP in the 274 script) then shaping on the wheel.
    StaticExtra { kind: "pottery_shape", level: 1, xp: 0, inputs: &[("clay", 1), ("jug_water", 1)], outputs: &[("softclay", 1)], station: None, source: "mix_softclay" },
    StaticExtra { kind: "pottery_shape", level: 1, xp: 63, inputs: &[("softclay", 1)], outputs: &[("pot_unfired", 1)], station: Some("potterywheel"), source: "shape_pot" },
    StaticExtra { kind: "pottery_shape", level: 7, xp: 150, inputs: &[("softclay", 1)], outputs: &[("piedish_unfired", 1)], station: Some("potterywheel"), source: "shape_piedish" },
    StaticExtra { kind: "pottery_shape", level: 8, xp: 180, inputs: &[("softclay", 1)], outputs: &[("bowl_unfired", 1)], station: Some("potterywheel"), source: "shape_bowl" },
    // Firing in the pottery oven.
    StaticExtra { kind: "pottery_fire", level: 1, xp: 63, inputs: &[("pot_unfired", 1)], outputs: &[("pot_empty", 1)], station: Some("potteryoven"), source: "fire_pot" },
    StaticExtra { kind: "pottery_fire", level: 7, xp: 100, inputs: &[("piedish_unfired", 1)], outputs: &[("piedish", 1)], station: Some("potteryoven"), source: "fire_piedish" },
    StaticExtra { kind: "pottery_fire", level: 8, xp: 150, inputs: &[("bowl_unfired", 1)], outputs: &[("bowl_empty", 1)], station: Some("potteryoven"), source: "fire_bowl" },
    // Gold and silver jewellery (jewellery.struct, F2P only).
    StaticExtra { kind: "jewellery", level: 5, xp: 150, inputs: &[("gold_bar", 1), ("ring_mould", 1)], outputs: &[("gold_ring", 1)], station: Some("furnace"), source: "gold_ring" },
    StaticExtra { kind: "jewellery", level: 6, xp: 200, inputs: &[("gold_bar", 1), ("necklace_mould", 1)], outputs: &[("gold_necklace", 1)], station: Some("furnace"), source: "gold_necklace" },
    StaticExtra { kind: "jewellery", level: 8, xp: 300, inputs: &[("gold_bar", 1), ("amulet_mould", 1)], outputs: &[("unstrung_gold_amulet", 1)], station: Some("furnace"), source: "gold_amulet" },
    StaticExtra { kind: "jewellery", level: 16, xp: 500, inputs: &[("silver_bar", 1), ("holy_symbol_mould", 1)], outputs: &[("nostringstar", 1)], station: Some("furnace"), source: "holy_symbol" },
    // Gem jewellery (cut gems, F2P stones only; dragonstone is members).
    StaticExtra { kind: "jewellery", level: 20, xp: 400, inputs: &[("gold_bar", 1), ("sapphire", 1), ("ring_mould", 1)], outputs: &[("sapphire_ring", 1)], station: Some("furnace"), source: "sapphire_ring" },
    StaticExtra { kind: "jewellery", level: 27, xp: 550, inputs: &[("gold_bar", 1), ("emerald", 1), ("ring_mould", 1)], outputs: &[("emerald_ring", 1)], station: Some("furnace"), source: "emerald_ring" },
    StaticExtra { kind: "jewellery", level: 34, xp: 700, inputs: &[("gold_bar", 1), ("ruby", 1), ("ring_mould", 1)], outputs: &[("ruby_ring", 1)], station: Some("furnace"), source: "ruby_ring" },
    StaticExtra { kind: "jewellery", level: 43, xp: 850, inputs: &[("gold_bar", 1), ("diamond", 1), ("ring_mould", 1)], outputs: &[("diamond_ring", 1)], station: Some("furnace"), source: "diamond_ring" },
    StaticExtra { kind: "jewellery", level: 20, xp: 550, inputs: &[("gold_bar", 1), ("sapphire", 1), ("necklace_mould", 1)], outputs: &[("sapphire_necklace", 1)], station: Some("furnace"), source: "sapphire_necklace" },
    StaticExtra { kind: "jewellery", level: 29, xp: 600, inputs: &[("gold_bar", 1), ("emerald", 1), ("necklace_mould", 1)], outputs: &[("emerald_necklace", 1)], station: Some("furnace"), source: "emerald_necklace" },
    StaticExtra { kind: "jewellery", level: 40, xp: 750, inputs: &[("gold_bar", 1), ("ruby", 1), ("necklace_mould", 1)], outputs: &[("ruby_necklace", 1)], station: Some("furnace"), source: "ruby_necklace" },
    StaticExtra { kind: "jewellery", level: 56, xp: 900, inputs: &[("gold_bar", 1), ("diamond", 1), ("necklace_mould", 1)], outputs: &[("diamond_necklace", 1)], station: Some("furnace"), source: "diamond_necklace" },
    StaticExtra { kind: "jewellery", level: 24, xp: 650, inputs: &[("gold_bar", 1), ("sapphire", 1), ("amulet_mould", 1)], outputs: &[("unstrung_sapphire_amulet", 1)], station: Some("furnace"), source: "sapphire_amulet" },
    StaticExtra { kind: "jewellery", level: 31, xp: 700, inputs: &[("gold_bar", 1), ("emerald", 1), ("amulet_mould", 1)], outputs: &[("unstrung_emerald_amulet", 1)], station: Some("furnace"), source: "emerald_amulet" },
    StaticExtra { kind: "jewellery", level: 50, xp: 850, inputs: &[("gold_bar", 1), ("ruby", 1), ("amulet_mould", 1)], outputs: &[("unstrung_ruby_amulet", 1)], station: Some("furnace"), source: "ruby_amulet" },
    StaticExtra { kind: "jewellery", level: 70, xp: 1000, inputs: &[("gold_bar", 1), ("diamond", 1), ("amulet_mould", 1)], outputs: &[("unstrung_diamond_amulet", 1)], station: Some("furnace"), source: "diamond_amulet" },
    // Amulet stringing (stringing.rs2: 40 XP, inventory).
    StaticExtra { kind: "string", level: 1, xp: 40, inputs: &[("ball_of_wool", 1), ("unstrung_gold_amulet", 1)], outputs: &[("strung_gold_amulet", 1)], station: None, source: "string_gold_amulet" },
    StaticExtra { kind: "string", level: 1, xp: 40, inputs: &[("ball_of_wool", 1), ("unstrung_sapphire_amulet", 1)], outputs: &[("strung_sapphire_amulet", 1)], station: None, source: "string_sapphire_amulet" },
    StaticExtra { kind: "string", level: 1, xp: 40, inputs: &[("ball_of_wool", 1), ("unstrung_emerald_amulet", 1)], outputs: &[("strung_emerald_amulet", 1)], station: None, source: "string_emerald_amulet" },
    StaticExtra { kind: "string", level: 1, xp: 40, inputs: &[("ball_of_wool", 1), ("unstrung_ruby_amulet", 1)], outputs: &[("strung_ruby_amulet", 1)], station: None, source: "string_ruby_amulet" },
    StaticExtra { kind: "string", level: 1, xp: 40, inputs: &[("ball_of_wool", 1), ("unstrung_diamond_amulet", 1)], outputs: &[("strung_diamond_amulet", 1)], station: None, source: "string_diamond_amulet" },
    StaticExtra { kind: "string", level: 1, xp: 40, inputs: &[("ball_of_wool", 1), ("nostringstar", 1)], outputs: &[("stringstar", 1)], station: None, source: "string_holy_symbol" },
    // Windmill (general_use/scripts/windmills.rs2): grain in the hopper
    // (`hopper_fill`), lever, then an empty pot on the flour bin
    // (`take_flour_bin`). No skill, no XP.
    StaticExtra { kind: "mill", level: 1, xp: 0, inputs: &[("grain", 1), ("pot_empty", 1)], outputs: &[("pot_flour", 1)], station: Some("hopper"), source: "mill_flour" },
    // Cook prep (inventory, no XP; the bake pays in `recipes` cook rows).
    StaticExtra { kind: "cook_prep", level: 1, xp: 0, inputs: &[("pot_flour", 1), ("jug_water", 1)], outputs: &[("bread_dough", 1)], station: None, source: "mix_bread_dough" },
    StaticExtra { kind: "cook_prep", level: 1, xp: 0, inputs: &[("pot_flour", 1), ("jug_water", 1)], outputs: &[("pastry_dough", 1)], station: None, source: "mix_pastry_dough" },
    StaticExtra { kind: "cook_prep", level: 1, xp: 0, inputs: &[("pot_flour", 1), ("jug_water", 1)], outputs: &[("pizza_base", 1)], station: None, source: "mix_pizza_base" },
    StaticExtra { kind: "cook_prep", level: 1, xp: 0, inputs: &[("pastry_dough", 1), ("piedish", 1)], outputs: &[("pie_shell", 1)], station: None, source: "make_pie_shell" },
    StaticExtra { kind: "cook_prep", level: 10, xp: 0, inputs: &[("pie_shell", 1), ("redberries", 1)], outputs: &[("uncooked_redberry_pie", 1)], station: None, source: "fill_redberry_pie" },
    StaticExtra { kind: "cook_prep", level: 20, xp: 0, inputs: &[("pie_shell", 1), ("cooked_meat", 1)], outputs: &[("uncooked_meat_pie", 1)], station: None, source: "fill_meat_pie" },
    StaticExtra { kind: "cook_prep", level: 30, xp: 0, inputs: &[("pie_shell", 1), ("cooking_apple", 1)], outputs: &[("uncooked_apple_pie", 1)], station: None, source: "fill_apple_pie" },
    StaticExtra { kind: "cook_prep", level: 35, xp: 0, inputs: &[("pizza_base", 1), ("tomato", 1), ("cheese", 1)], outputs: &[("uncooked_pizza", 1)], station: None, source: "make_plain_pizza" },
    StaticExtra { kind: "cook_prep", level: 40, xp: 300, inputs: &[("pot_flour", 1), ("egg", 1), ("bucket_milk", 1)], outputs: &[("uncooked_cake", 1)], station: None, source: "mix_cake" },
    StaticExtra { kind: "cook_prep", level: 35, xp: 1100, inputs: &[("grapes", 1), ("jug_water", 1)], outputs: &[("jug_unfermented_wine", 1)], station: None, source: "ferment_wine" },
    // Runecraft (runecraft.dbrow: 50-75 XP per essence, F2P six).
    StaticExtra { kind: "runecraft", level: 1, xp: 50, inputs: &[("blankrune", 1), ("air_talisman", 1)], outputs: &[("airrune", 1)], station: Some("ruin"), source: "craft_air" },
    StaticExtra { kind: "runecraft", level: 2, xp: 55, inputs: &[("blankrune", 1), ("mind_talisman", 1)], outputs: &[("mindrune", 1)], station: Some("ruin"), source: "craft_mind" },
    StaticExtra { kind: "runecraft", level: 5, xp: 60, inputs: &[("blankrune", 1), ("water_talisman", 1)], outputs: &[("waterrune", 1)], station: Some("ruin"), source: "craft_water" },
    StaticExtra { kind: "runecraft", level: 9, xp: 65, inputs: &[("blankrune", 1), ("earth_talisman", 1)], outputs: &[("earthrune", 1)], station: Some("ruin"), source: "craft_earth" },
    StaticExtra { kind: "runecraft", level: 14, xp: 70, inputs: &[("blankrune", 1), ("fire_talisman", 1)], outputs: &[("firerune", 1)], station: Some("ruin"), source: "craft_fire" },
    StaticExtra { kind: "runecraft", level: 20, xp: 75, inputs: &[("blankrune", 1), ("body_talisman", 1)], outputs: &[("bodyrune", 1)], station: Some("ruin"), source: "craft_body" },
    // Firemaking (firemaking.obj productexp, F2P logs only; maple is members).
    StaticExtra { kind: "firemaking", level: 1, xp: 400, inputs: &[("logs", 1), ("tinderbox", 1)], outputs: &[("ashes", 1)], station: None, source: "burn_logs" },
    StaticExtra { kind: "firemaking", level: 15, xp: 600, inputs: &[("oak_logs", 1), ("tinderbox", 1)], outputs: &[("ashes", 1)], station: None, source: "burn_oak" },
    StaticExtra { kind: "firemaking", level: 30, xp: 900, inputs: &[("willow_logs", 1), ("tinderbox", 1)], outputs: &[("ashes", 1)], station: None, source: "burn_willow" },
    StaticExtra { kind: "firemaking", level: 60, xp: 2025, inputs: &[("yew_logs", 1), ("tinderbox", 1)], outputs: &[("ashes", 1)], station: None, source: "burn_yew" },
    // Prayer burying (bones.obj bone_exp, F2P two).
    StaticExtra { kind: "bury", level: 1, xp: 45, inputs: &[("bones", 1)], outputs: &[], station: None, source: "bury_bones" },
    StaticExtra { kind: "bury", level: 1, xp: 150, inputs: &[("big_bones", 1)], outputs: &[], station: None, source: "bury_big_bones" },
];

/// Every missing recipe.
pub fn all_extra() -> Vec<ExtraRecipe> {
    ALL.iter().map(from_static).collect()
}

/// Missing recipes of one kind (`spin`, `jewellery`, `runecraft`, ...).
pub fn by_kind(kind: &str) -> Vec<ExtraRecipe> {
    all_extra().into_iter().filter(|entry| entry.kind == kind).collect()
}

/// F2P utility spells (already in `spells` with XP): teleports, alchemy,
/// superheat, enchant-amulet, grab and bones-to-bananas (D64 P1/P2 in
/// `methods`). No station: cast from inventory/spellbook.
pub fn utility_spells() -> Vec<crate::spells::Spell> {
    crate::spells::all_spells()
        .into_iter()
        .filter(|entry| {
            !entry.members
                && matches!(
                    entry.spell.as_str(),
                    "varrock_teleport"
                        | "lumbridge_teleport"
                        | "falador_teleport"
                        | "lowlvl_alchemy"
                        | "highlvl_alchemy"
                        | "superheat_item"
                        | "bones_to_bananas"
                        | "telekinetic_grab"
                        | "enchant_lvl1"
                        | "enchant_lvl2"
                        | "enchant_lvl3"
                        | "enchant_lvl4"
                )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{all_extra, by_kind, utility_spells};
    use crate::stations::stations_of;

    fn assert_f2p(name: &str) {
        let found = crate::objs_by_name(name).into_iter().next().unwrap_or_else(|| panic!("{name} in dumps"));
        assert_eq!(found.members, Some(false), "{name} is F2P, got {:?}", found.members);
    }

    #[test]
    fn extra_recipes_use_f2p_objs_and_stations() {
        // Every input/output is F2P; every station kind has F2P posts.
        // No member object (flax, bowstring, dragonstone, maple) appears.
        let all = all_extra();
        assert!(all.len() >= 45, "extra recipes, got {}", all.len());
        for entry in &all {
            for (name, _) in entry.inputs.iter().chain(entry.outputs.iter()) {
                assert_f2p(name);
                assert!(
                    !matches!(
                        name.as_str(),
                        "flax" | "bow_string" | "dragonstone" | "maple_logs" | "magic_logs" | "yew_shortbow"
                    ),
                    "no member obj {} in {}",
                    name,
                    entry.source
                );
            }
            if let Some(station) = &entry.station {
                assert!(!stations_of(station).is_empty(), "station {station} placed for {}", entry.source);
            }
            assert!(entry.xp >= 0, "xp for {}", entry.source);
        }
        // Spot checks from the 274 scripts.
        let spin = by_kind("spin");
        assert_eq!(spin.len(), 1);
        assert_eq!((spin[0].level, spin[0].xp), (1, 25));
        let gold_ring = all_extra().into_iter().find(|entry| entry.source == "gold_ring").expect("gold ring");
        assert_eq!((gold_ring.level, gold_ring.xp), (5, 150));
        let diamond_amulet = all_extra().into_iter().find(|entry| entry.source == "diamond_amulet").expect("diamond amulet");
        assert_eq!((diamond_amulet.level, diamond_amulet.xp), (70, 1000));
        let air = all_extra().into_iter().find(|entry| entry.source == "craft_air").expect("air");
        assert_eq!((air.level, air.xp), (1, 50));
        let yew = all_extra().into_iter().find(|entry| entry.source == "burn_yew").expect("yew");
        assert_eq!((yew.level, yew.xp), (60, 2025));
        let wine = all_extra().into_iter().find(|entry| entry.source == "ferment_wine").expect("wine");
        assert_eq!((wine.level, wine.xp), (35, 1100));
        // Utility spells: 12 F2P (3 teleports, 2 alch, superheat, bananas,
        // grab, 4 amulet enchants); ring enchants stay out (members).
        let utils = utility_spells();
        assert_eq!(utils.len(), 12, "utility spells, got {}", utils.len());
        assert!(utils.iter().any(|entry| entry.spell == "superheat_item" && entry.xp == 530));
        assert!(utils.iter().any(|entry| entry.spell == "telekinetic_grab" && entry.level == 33));
    }
}
