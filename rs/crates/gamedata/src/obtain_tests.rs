//! Tests of `obtain` (split out, rule 4): the user's two examples, the
//! D-A merchant rule, and the frozen list of unobtainable objects.

use super::{merchant_buy_allowed, obtainable_f2p, producible_f2p, unobtainable_reason, ways, WayKind};

#[test]
fn nature_rune_only_drops_in_f2p() {
    // No F2P altar (nature is members) and no F2P shop stocks it: the
    // way is killing a monster that drops it, at a known rate.
    let found = ways("naturerune");
    assert!(!found.is_empty(), "nature runes are obtainable");
    // Monster drops plus two Wilderness ground spawns; no shop, no make.
    let kinds: Vec<WayKind> = found.iter().map(|way| way.kind).collect();
    assert!(kinds.iter().all(|kind| matches!(kind, WayKind::Drop | WayKind::Ground)), "drops and spawns only, got {:?}", found.iter().map(|way| way.describe()).collect::<Vec<_>>());
    let wizard = found.iter().find(|way| way.kind == WayKind::Drop && way.detail == "wizard").expect("wizards drop nature runes");
    assert!(wizard.rate.is_some_and(|rate| rate > 0.0) && !wizard.sites.is_empty(), "rated, with F2P sites");
    assert!(found.iter().any(|way| way.combat.is_some()), "monster level known");
    assert!(!producible_f2p("naturerune"));
    assert!(!merchant_buy_allowed("naturerune"), "a combat spot drops it: market or farm, never a merchant");
}

#[test]
fn bronze_pickaxe_has_every_kind_of_way() {
    // Rev 274 has no pickaxe smithing (added later): the pickaxe is
    // bought, picked up from its ground spawns or dropped by dwarves.
    let found = ways("bronze_pickaxe");
    let kinds: Vec<WayKind> = found.iter().map(|way| way.kind).collect();
    assert!(kinds.contains(&WayKind::Shop), "sold: {kinds:?}");
    assert!(kinds.contains(&WayKind::Ground), "ground spawns: {kinds:?}");
    assert!(kinds.contains(&WayKind::Drop), "dwarf drop: {kinds:?}");
    assert!(!kinds.contains(&WayKind::Make), "no smithing in 274: {kinds:?}");
    let shop = found.iter().find(|way| way.kind == WayKind::Shop).expect("shop way");
    assert!(!shop.sites.is_empty(), "shopkeeper stands in F2P");
    assert!(!producible_f2p("bronze_pickaxe"));
    assert!(merchant_buy_allowed("bronze_pickaxe"), "D-A: not producible, merchant allowed");
    // A bronze sword is smithed: never from a merchant.
    let sword = ways("bronze_sword");
    let smith = sword.iter().find(|way| way.kind == WayKind::Make).expect("smith way");
    assert_eq!(smith.skill.as_ref().map(|(skill, _)| skill.as_str()), Some("smithing"));
    assert!(smith.inputs.iter().any(|(input, _)| input == "bronze_bar"), "from a bronze bar: {}", smith.describe());
    assert!(smith.tools.contains(&"hammer".to_string()));
    assert!(!smith.sites.is_empty(), "F2P anvils");
    assert!(!merchant_buy_allowed("bronze_sword"));
}

#[test]
fn merchant_rule_follows_d_a() {
    for producible in ["wool", "ball_of_wool", "logs", "copper_ore", "bronze_bar", "raw_shrimp", "leather", "bucket_water", "amulet_of_strength", "greendye", "airrune", "uncooked_stew"] {
        assert!(producible_f2p(producible), "{producible} is made or gathered in F2P");
        assert!(!merchant_buy_allowed(producible), "{producible}: no merchant purchase");
    }
    // Farmable drops left the merchant list (user rule 2026-11-13).
    for farmed in ["feather", "naturerune", "air_talisman"] {
        assert!(!producible_f2p(farmed), "{farmed} has no F2P production");
        assert!(!merchant_buy_allowed(farmed), "{farmed}: a combat spot drops it, never a merchant");
        assert!(obtainable_f2p(farmed), "{farmed} obtainable");
    }
    for bought in ["shears", "net", "chisel", "needle", "bucket_empty"] {
        assert!(!producible_f2p(bought), "{bought} has no F2P production");
        assert!(merchant_buy_allowed(bought), "{bought}: merchant purchase allowed");
        assert!(obtainable_f2p(bought), "{bought} obtainable");
    }
    assert!(!merchant_buy_allowed("rune_scimitar") || !producible_f2p("rune_scimitar"), "rule is the negation");
    assert!(!merchant_buy_allowed("abyssal_whip"), "members objects never");
}

#[test]
fn new_sources_reach_the_brain() {
    let kebab = ways("kebab");
    assert!(kebab.iter().any(|way| way.kind == WayKind::Dialogue && way.coins == 1), "Karim: {kebab:?}");
    let shield = ways("antidragonbreathshield");
    assert!(shield.iter().any(|way| way.kind == WayKind::Script), "Duke Horacio: {shield:?}");
    let strength = ways("amulet_of_strength");
    assert!(strength.iter().any(|way| way.kind == WayKind::Spell && way.skill == Some(("magic".to_string(), 49))), "ruby enchant at 49: {strength:?}");
    let staff = ways("staff_of_air");
    assert!(staff.iter().any(|way| way.kind == WayKind::Shop && way.detail == "staffshop"), "Zaff: {staff:?}");
    let bones = ways("bones");
    assert!(bones.iter().any(|way| way.kind == WayKind::Drop && way.rate == Some(1.0)), "bones every kill");
}

/// Every F2P tradeable object either has a way or a typed reason; the
/// "no F2P source" bucket is pinned by name so it never grows silently.
#[test]
fn unobtainables_are_named() {
    let mut no_source = Vec::new();
    for obj in crate::f2p::f2p_tradeable_objs() {
        let name = obj.name.unwrap_or("");
        if let Some(reason) = unobtainable_reason(name) {
            if reason.starts_with("no F2P source") {
                no_source.push(name.to_string());
            }
        }
    }
    no_source.sort();
    let expected: Vec<String> = NO_F2P_SOURCE.iter().map(|name| name.to_string()).collect();
    assert_eq!(no_source, expected, "the no-source bucket moved: update NO_F2P_SOURCE only with a server reason");
}

/// F2P-flagged tradeable objects with no way on an F2P world in rev 274
/// (members-only shops, drops or recipes): never a planning target.
const NO_F2P_SOURCE: &[&str] = &[
    "anchovie_pizza",
    "black_2h_sword",
    "black_axe",
    "black_scimitar",
    "black_warhammer",
    "blackrobebottom",
    "blackrobetop",
    "charcoal",
    "cooking_pot",
    "halberd",
    "half_full_wine_jug",
    "javelin",
    "meat_pizza",
    "obj_1459",
    "rock",
    "warhammer",
];

/// Every F2P bar's smelt way carries the server recipe (smelting.struct):
/// bronze eats copper AND tin, steel iron + 2 coal, mithril/adamantite/
/// runite 4/6/8 coal; a furnace site, no tool. Iron is the only uncertain
/// one: half its ores make nothing, so a bar costs two ores.
#[test]
fn smelt_ways_carry_every_input_and_the_iron_odds() {
    let cases: [(&str, &[(&str, i64)], f64); 8] = [
        ("bronze_bar", &[("copper_ore", 1), ("tin_ore", 1)], 1.0),
        ("iron_bar", &[("iron_ore", 1)], 0.5),
        ("silver_bar", &[("silver_ore", 1)], 1.0),
        ("steel_bar", &[("coal", 2), ("iron_ore", 1)], 1.0),
        ("gold_bar", &[("gold_ore", 1)], 1.0),
        ("mithril_bar", &[("coal", 4), ("mithril_ore", 1)], 1.0),
        ("adamantite_bar", &[("adamantite_ore", 1), ("coal", 6)], 1.0),
        ("runite_bar", &[("coal", 8), ("runite_ore", 1)], 1.0),
    ];
    for (bar, inputs, odds) in cases {
        let found = ways(bar);
        let smelt = found
            .iter()
            .find(|way| way.kind == WayKind::Make && way.detail.starts_with("smelt "))
            .unwrap_or_else(|| panic!("{bar} smelts"));
        let mut got = smelt.inputs.clone();
        got.sort();
        let want: Vec<(String, i64)> = inputs.iter().map(|(name, count)| (name.to_string(), *count)).collect();
        assert_eq!(got, want, "{bar}: {}", smelt.describe());
        assert!(smelt.tools.is_empty(), "{bar} needs no tool");
        assert!(!smelt.sites.is_empty(), "{bar} has furnace sites");
        assert_eq!(smelt.success, odds, "{bar} odds");
        assert_eq!(smelt.expected_out(), odds, "{bar} bars per action");
    }
    let smith = ways("bronze_platebody").into_iter().find(|way| way.detail.starts_with("smith ")).expect("platebody smiths");
    assert_eq!(smith.inputs, vec![("bronze_bar".to_string(), 5)], "5 bars per platebody: {}", smith.describe());
    assert_eq!(smith.tools, vec!["hammer".to_string()], "hammer carried, not consumed");
}

/// User rule 2026-11-13: an NPC shop sells a bot only what F2P offers no
/// other way to get. Feathers drop from chickens, fishing bait from men
/// (farmable spots): market or farm, never the shop. Nets, rods, shears
/// and harpoons have no farmable F2P source: the shop stays.
#[test]
fn farmable_drops_never_come_from_a_merchant() {
    assert!(!merchant_buy_allowed("feather"), "chickens drop feathers");
    assert!(!merchant_buy_allowed("fishing_bait"), "men drop fishing bait");
    assert!(!merchant_buy_allowed("bronze_arrow"), "goblins and men drop bronze arrows");
    for shop_only in ["net", "fishing_rod", "fly_fishing_rod", "harpoon", "lobster_pot", "shears", "tinderbox", "chisel"] {
        assert!(merchant_buy_allowed(shop_only), "{shop_only} has no farmable F2P source");
    }
}

/// Bait is a consumed input of the fishing way (one per catch), and rune
/// essence is mined only after Rune Mysteries (`essence_mine.rs2`).
#[test]
fn fishing_bait_and_essence_quest_are_way_requirements() {
    let trout = ways("raw_trout").into_iter().find(|way| way.kind == WayKind::Gather).expect("trout is fished");
    assert_eq!(trout.inputs, vec![("feather".to_string(), 1)], "{}", trout.describe());
    assert!(trout.tools.contains(&"fly_fishing_rod".to_string()));
    let pike = ways("raw_pike").into_iter().find(|way| way.kind == WayKind::Gather).expect("pike is fished");
    assert_eq!(pike.inputs, vec![("fishing_bait".to_string(), 1)]);
    let shrimp = ways("raw_shrimp").into_iter().find(|way| way.kind == WayKind::Gather).expect("shrimp is fished");
    assert!(shrimp.inputs.is_empty(), "nets burn no bait");
    let essence = ways("blankrune").into_iter().find(|way| way.kind == WayKind::Gather).expect("essence is mined");
    assert_eq!(essence.quest.as_deref(), Some("quest_runemysteries"));
}

/// Moulds and talismans are carried, never consumed (`jewellery.rs2`,
/// `runecraft.rs2`): the way lists them as tools, the inputs keep only
/// what one action burns (audit 2026-11-13: moulds sized as inputs).
#[test]
fn moulds_and_talismans_are_tools() {
    let ring = ways("gold_ring").into_iter().find(|way| way.kind == WayKind::Make).expect("gold ring recipe");
    assert_eq!(ring.inputs, vec![("gold_bar".to_string(), 1)]);
    assert!(ring.tools.contains(&"ring_mould".to_string()), "{:?}", ring.tools);
    let air = ways("airrune").into_iter().find(|way| way.kind == WayKind::Make).expect("air rune recipe");
    assert_eq!(air.inputs, vec![("blankrune".to_string(), 1)]);
    assert_eq!(air.tools, vec!["air_talisman".to_string()]);
    let sapphire = ways("sapphire").into_iter().find(|way| way.kind == WayKind::Make).expect("gem cutting");
    assert_eq!(sapphire.tools, vec!["chisel".to_string()]);
}
