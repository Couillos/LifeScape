use super::*;

fn has(index: SupplyIndex, name: &str) -> bool {
    members(index).iter().any(|member| member == name)
}

fn assert_in(index: SupplyIndex, names: &[&str]) {
    for name in names {
        assert!(has(index, name), "{name} belongs to {}", index.key());
    }
}

fn assert_out(index: SupplyIndex, names: &[&str]) {
    for name in names {
        assert!(!has(index, name), "{name} must stay out of {}", index.key());
    }
}

fn exact(index: SupplyIndex, expected: &[&str]) {
    let got: Vec<&str> = members(index).iter().map(String::as_str).collect();
    assert_eq!(got, expected, "{} composition changed: review every object before re-pinning", index.key());
}

#[test]
fn keys_round_trip_and_stay_in_display_order() {
    let keys: Vec<&str> = SupplyIndex::ALL.iter().map(|index| index.key()).collect();
    assert_eq!(
        keys,
        vec![
            "cooking",
            "food",
            "melee_equipment",
            "range_equipment",
            "range_supply",
            "magic_equipment",
            "magic_supply",
            "logs",
            "tools",
            "mining_supply",
            "smithing_supply"
        ]
    );
    for index in SupplyIndex::ALL {
        assert_eq!(SupplyIndex::parse(index.key()), Some(index));
        assert!(!members(index).is_empty(), "{} is empty", index.key());
    }
    assert_eq!(SupplyIndex::parse("nope"), None);
}

/// Raw fish, raw meat and the prep chain (flour, doughs, uncooked
/// dishes, stew links); never the cooked dish, never a vessel, never
/// a members fish.
#[test]
fn cooking_is_what_goes_on_the_range() {
    exact(
        SupplyIndex::Cooking,
        &[
            "bread_dough",
            "bucket_milk",
            "cooking_apple",
            "egg",
            "grain",
            "incomplete_pizza",
            "pastry_dough",
            "pie_shell",
            "pizza_base",
            "pot_flour",
            "potato",
            "raw_anchovies",
            "raw_bear_meat",
            "raw_beef",
            "raw_chicken",
            "raw_herring",
            "raw_lobster",
            "raw_pike",
            "raw_rat_meat",
            "raw_salmon",
            "raw_sardine",
            "raw_shrimp",
            "raw_swordfish",
            "raw_trout",
            "raw_tuna",
            "redberries",
            "stew1",
            "stew2",
            "uncooked_apple_pie",
            "uncooked_cake",
            "uncooked_meat_pie",
            "uncooked_pizza",
            "uncooked_redberry_pie",
            "uncooked_stew",
        ],
    );
    assert_out(SupplyIndex::Cooking, &["shrimp", "cooked_meat", "tomato", "cheese", "jug_water", "bowl_water", "pot_empty", "piedish", "cake_tin", "raw_shark", "raw_cod", "burnt_meat"]);
}

/// Edible food from `foods.json`: cooked fish, bread, pies, pizzas,
/// cakes, cooked meat; never raw, never an alcoholic drink, never a
/// members dish.
#[test]
fn food_is_what_a_bot_eats() {
    assert_in(SupplyIndex::Food, &["shrimp", "bread", "plain_pizza", "meat_pie", "cake", "cooked_meat", "cooked_chicken", "lobster", "swordfish", "stew"]);
    assert_out(SupplyIndex::Food, &["raw_shrimp", "bread_dough", "beer", "jug_wine", "shark", "curry", "burnt_meat", "uncooked_cake"]);
    assert_eq!(members(SupplyIndex::Food).len(), 42);
}

/// Melee weapons and armour; harvest tools, staves, bows and the
/// stat-less broken-tool props are not melee equipment.
#[test]
fn melee_equipment_is_swung_or_plated() {
    assert_in(
        SupplyIndex::MeleeEquipment,
        &["bronze_scimitar", "rune_2h_sword", "iron_battleaxe", "steel_platebody", "rune_kiteshield", "adamant_full_helm_trim", "wooden_shield", "amulet_of_strength", "amulet_of_power", "amulet_of_defence"],
    );
    assert_out(
        SupplyIndex::MeleeEquipment,
        &["bronze_pickaxe", "bronze_axe", "rune_axe", "staff_of_fire", "shortbow", "leather_armour", "bronze_arrow", "macro_broken_bronze_pickaxe", "red_cape", "gold_ring", "wizards_robe", "amulet_of_magic"],
    );
    assert_eq!(members(SupplyIndex::MeleeEquipment).len(), 164);
}

#[test]
fn range_equipment_is_bows_and_leather() {
    exact(
        SupplyIndex::RangeEquipment,
        &[
            "coif",
            "crossbow",
            "dragon_vambraces",
            "dragonhide_body",
            "dragonhide_chaps",
            "hardleather_body",
            "leather_armour",
            "leather_boots",
            "leather_chaps",
            "leather_cowl",
            "leather_gloves",
            "leather_vambraces",
            "longbow",
            "oak_longbow",
            "oak_shortbow",
            "phoenix_crossbow",
            "shortbow",
            "studded_body",
            "studded_chaps",
        ],
    );
    assert_out(SupplyIndex::RangeEquipment, &["bronze_arrow", "bolt"]);
}

/// Ammunition only (mithril arrows and better are members in 274).
#[test]
fn range_supply_is_ammunition() {
    exact(SupplyIndex::RangeSupply, &["bolt", "bronze_arrow", "iron_arrow", "steel_arrow"]);
    assert_out(SupplyIndex::RangeSupply, &["shortbow", "mithril_arrow"]);
}

#[test]
fn magic_equipment_is_staves_and_robes() {
    exact(
        SupplyIndex::MagicEquipment,
        &["amulet_of_magic", "black_robe", "blackwizhat", "bluewizhat", "magic_staff", "plainstaff", "staff_of_air", "staff_of_earth", "staff_of_fire", "staff_of_water", "wizards_robe"],
    );
    assert_out(SupplyIndex::MagicEquipment, &["airrune", "blue_skirt"]);
}

/// The runes F2P spells burn and runecrafting makes; essence is mined.
#[test]
fn magic_supply_is_runes() {
    exact(
        SupplyIndex::MagicSupply,
        &["airrune", "bodyrune", "chaosrune", "cosmicrune", "deathrune", "earthrune", "firerune", "lawrune", "mindrune", "naturerune", "waterrune"],
    );
    assert_out(SupplyIndex::MagicSupply, &["blankrune", "bloodrune", "soulrune", "air_talisman"]);
}

/// Every F2P tree product; maple logs carry no members flag on the
/// server (`firemaking.obj`), magic logs do.
#[test]
fn logs_are_tree_products() {
    exact(SupplyIndex::Logs, &["logs", "maple_logs", "oak_logs", "willow_logs", "yew_logs"]);
    assert_out(SupplyIndex::Logs, &["magic_logs", "ashes"]);
}

#[test]
fn tools_are_carried_never_consumed() {
    exact(
        SupplyIndex::Tools,
        &[
            "adamant_axe",
            "adamant_pickaxe",
            "air_talisman",
            "amulet_mould",
            "black_axe",
            "body_talisman",
            "bronze_axe",
            "bronze_pickaxe",
            "chisel",
            "earth_talisman",
            "fire_talisman",
            "fishing_rod",
            "fly_fishing_rod",
            "hammer",
            "harpoon",
            "holy_symbol_mould",
            "iron_axe",
            "iron_pickaxe",
            "lobster_pot",
            "mind_talisman",
            "mithril_axe",
            "mithril_pickaxe",
            "necklace_mould",
            "needle",
            "net",
            "ring_mould",
            "rune_axe",
            "rune_pickaxe",
            "shears",
            "steel_axe",
            "steel_pickaxe",
            "tinderbox",
            "water_talisman",
        ],
    );
    assert_out(SupplyIndex::Tools, &["thread", "fishing_bait", "feather", "macro_broken_bronze_pickaxe", "bronze_battleaxe", "cake_tin"]);
}

/// Everything a rock yields: ores, coal, clay and rune essence.
#[test]
fn mining_supply_is_rock_products() {
    exact(
        SupplyIndex::MiningSupply,
        &["adamantite_ore", "blankrune", "clay", "coal", "copper_ore", "gold_ore", "iron_ore", "mithril_ore", "runite_ore", "silver_ore", "tin_ore"],
    );
    assert_out(SupplyIndex::MiningSupply, &["iron_bar", "softclay", "uncut_sapphire"]);
}

#[test]
fn smithing_supply_is_bars() {
    exact(SupplyIndex::SmithingSupply, &["adamantite_bar", "bronze_bar", "gold_bar", "iron_bar", "mithril_bar", "runite_bar", "silver_bar", "steel_bar"]);
    assert_out(SupplyIndex::SmithingSupply, &["iron_ore", "coal"]);
}

/// The equipment and supply families never share an object: a curve
/// counts each held object once among them.
#[test]
fn indices_are_disjoint() {
    for (position, left) in SupplyIndex::ALL.iter().enumerate() {
        for right in &SupplyIndex::ALL[position + 1..] {
            for name in members(*left) {
                assert!(!has(*right, name), "{name} in both {} and {}", left.key(), right.key());
            }
        }
    }
}

/// Every member is F2P, tradeable and a base object; its banknote
/// folds on it through `obj_ids` / `indices_of`.
#[test]
fn members_are_f2p_bases_and_notes_fold() {
    for index in SupplyIndex::ALL {
        for name in members(index) {
            assert!(in_universe(name), "{name} ({}) outside the F2P market universe", index.key());
        }
    }
    let raw = crate::objs_by_name("raw_shrimp")[0].obj_id;
    let note = crate::objs_by_name("cert_raw_shrimp")[0].obj_id;
    let ids = obj_ids(SupplyIndex::Cooking);
    assert!(ids.contains(&raw) && ids.contains(&note), "raw shrimp and its note count");
    assert_eq!(indices_of(note), vec![SupplyIndex::Cooking]);
    assert_eq!(indices_of(crate::objs_by_name("coins")[0].obj_id), Vec::<SupplyIndex>::new());
    // A stackable object has no note: only its own id counts.
    let arrows = crate::objs_by_name("bronze_arrow")[0].obj_id;
    assert_eq!(indices_of(arrows), vec![SupplyIndex::RangeSupply]);
}
