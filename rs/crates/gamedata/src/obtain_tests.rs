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
    assert!(!super::merchant_route("naturerune"), "no F2P keeper stocks it: market or farm");
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
    assert!(merchant_buy_allowed("bronze_sword"), "user 2026-11-15: every F2P object may be bought");
}

/// User rule 2026-11-15 (replaces D-A): every F2P object may be bought
/// from a keeper that stocks it, made, gathered or dropped alike; a shop
/// route still needs that keeper. Members objects never.
#[test]
fn merchants_sell_every_f2p_object_they_stock() {
    for item in ["wool", "logs", "bronze_bar", "raw_shrimp", "feather", "naturerune", "air_talisman", "shears", "net", "amulet_mould"] {
        assert!(merchant_buy_allowed(item), "{item}: a merchant purchase is allowed");
        assert!(obtainable_f2p(item), "{item} obtainable");
    }
    for stocked in ["feather", "fishing_bait", "amulet_mould", "shears", "net", "chisel", "bucket_empty", "hammer"] {
        assert!(super::merchant_route(stocked), "{stocked}: an F2P keeper stocks it");
    }
    assert!(!super::merchant_route("naturerune"), "no F2P keeper stocks nature runes");
    assert!(!merchant_buy_allowed("abyssal_whip"), "members objects never");
    assert!(producible_f2p("bronze_bar") && merchant_buy_allowed("bronze_bar"), "producible and buyable");
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

/// The second catch of a tier opens at its own level (`fishing.rs2`):
/// herring 10 behind sardine 5, salmon 30 behind trout 20, tuna 35.
#[test]
fn a_second_catch_opens_at_its_own_level() {
    let level = |item: &str| ways(item).into_iter().find(|way| way.kind == WayKind::Gather).and_then(|way| way.skill).map(|(_, level)| level);
    assert_eq!(level("raw_sardine"), Some(5));
    assert_eq!(level("raw_herring"), Some(10));
    assert_eq!(level("raw_trout"), Some(20));
    assert_eq!(level("raw_salmon"), Some(30));
    assert_eq!(level("raw_tuna"), Some(35));
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

/// Furnace and anvil sites the planner may pick: Lumbridge, Al Kharid,
/// Falador (+ the Wilderness furnace, gated by danger) and the plain
/// anvils; never Varrock for a furnace (it has none: live "no produce
/// station furnace1/furnace2 near (3186,3436)"), never Tutorial Island's
/// mine (3081,9496), never Doric's quest-gated anvils (2950,3451).
#[test]
fn station_sites_are_reachable_f2p_posts() {
    let smelt = ways("steel_bar").into_iter().find(|way| way.detail.starts_with("smelt ")).expect("steel smelts");
    let near = |sites: &[(i64, i64, i64)], x: i64, z: i64| sites.iter().any(|site| (site.0 - x).abs() <= 2 && (site.1 - z).abs() <= 2);
    for (x, z, town) in [(3226, 3255, "Lumbridge"), (3272, 3185, "Al Kharid"), (2975, 3368, "Falador")] {
        assert!(near(&smelt.sites, x, z), "{town} furnace offered: {:?}", smelt.sites);
    }
    assert!(!smelt.sites.iter().any(|site| (3150..=3290).contains(&site.0) && (3380..=3500).contains(&site.1)), "no furnace in Varrock: {:?}", smelt.sites);
    assert!(!smelt.sites.iter().any(|site| site.1 > 9000 && (3072..=3118).contains(&site.0)), "no Tutorial Island furnace: {:?}", smelt.sites);
    let smith = ways("bronze_dagger").into_iter().find(|way| way.detail.starts_with("smith ")).expect("dagger smiths");
    assert!(near(&smith.sites, 3188, 3421) && near(&smith.sites, 3248, 3404), "Varrock anvils offered: {:?}", smith.sites);
    assert!(!near(&smith.sites, 2950, 3451), "Doric's anvils need his quest: {:?}", smith.sites);
    assert!(!smith.sites.iter().any(|site| site.1 > 9000 && (3072..=3118).contains(&site.0)), "no Tutorial Island anvil: {:?}", smith.sites);
    assert!(super::station_open("anvil", 3188, 3421) && !super::station_open("doric_anvil", 2950, 3451) && !super::station_open("furnace2", 3081, 9496));
}

/// User 2026-11-14 ("personne n'a mieux qu'une bronze pickaxe"): no F2P
/// recipe smiths a pickaxe in rev 274 (`smithing.inv` lists axes only)
/// and no F2P monster drops one, so every upgrade pickaxe is a merchant
/// purchase (Nurmof's `pickaxeshop`, Dwarven Mine) at the server price
/// (`calc_shop_value`: cost × sell multiplier 1000 / 1000). Woodcutting
/// axes are smithable: they stay player trade.
#[test]
fn upgrade_pickaxes_come_from_nurmof_axes_from_players() {
    use super::{merchant_price, merchant_route};
    for pickaxe in ["iron_pickaxe", "steel_pickaxe", "mithril_pickaxe", "adamant_pickaxe", "rune_pickaxe"] {
        assert!(merchant_route(pickaxe), "{pickaxe}: shop only");
        let cost = crate::objs_by_name(pickaxe)[0].price;
        assert_eq!(merchant_price(pickaxe), Some(cost.max(1)), "{pickaxe}: Nurmof sells at cost");
    }
    // User 2026-11-15: smithable axes sell at Bob's too; mithril and
    // better have no F2P keeper.
    for axe in ["iron_axe", "steel_axe"] {
        assert!(merchant_route(axe), "{axe}: Bob sells it");
    }
    for axe in ["mithril_axe", "adamant_axe", "rune_axe"] {
        assert!(!merchant_route(axe), "{axe}: no F2P keeper");
    }
    assert_eq!(merchant_price("dragon_platebody"), None, "no F2P keeper stocks it");
}

/// User 2026-11-14 (permanent tool kit): Bob's Brilliant Axes sells the
/// bronze, iron and steel axes; the kit buys them there when no player
/// sells. Mithril and better are smithed only: no shop route. Graph
/// obtains keep D-A (no merchant purchase of a producible axe).
#[test]
fn kit_axes_have_a_shop_route_up_to_steel() {
    use super::{kit_axe, kit_shop_route, merchant_buy_allowed, merchant_price, shop_buy_allowed};
    for axe in ["bronze_axe", "iron_axe", "steel_axe"] {
        assert!(kit_shop_route(axe) && shop_buy_allowed(axe), "{axe}: Bob sells it");
        assert!(merchant_buy_allowed(axe), "{axe}: graph obtains buy it too (user 2026-11-15)");
    }
    // Bob sells at cost (16, 200); the Wilderness bandit camp for less.
    assert!(merchant_price("bronze_axe").is_some_and(|price| price <= 16));
    assert!(merchant_price("steel_axe").is_some_and(|price| price <= 200));
    for axe in ["mithril_axe", "adamant_axe", "rune_axe", "black_axe"] {
        assert!(!kit_shop_route(axe), "{axe}: no F2P shop, players only");
    }
    assert!(!kit_axe("bronze_battleaxe"), "a battleaxe is no woodcutting axe");
    for tool in ["hammer", "tinderbox", "bronze_pickaxe", "rune_pickaxe"] {
        assert!(kit_shop_route(tool), "{tool}: merchant route");
    }
    assert!(kit_shop_route("bronze_sword"), "every stocked object has a merchant route (user 2026-11-15)");
}

/// User 2026-11-16 (Q7): rare-shop purchases are open to every bot. The
/// Champions' Guild keepers stand upstairs behind the 32 quest-point
/// door (`champions_guild.rs2` `%qp < 32`): their stock is a shop way
/// carrying the door's points, a route only for a bot that holds them
/// (live 2026-11-15: 394 `shopbuy black_full_helm: no reachable shop`
/// when every bot was sent there). Scavvo (`runiteshop`) sells the rune
/// pieces, Valaine (`championshop`) the adamant platebody and black.
#[test]
fn champions_guild_sells_behind_its_quest_points() {
    let rows = [
        ("adamant_platebody", "championshop", (3193, 3361, 1)),
        ("black_full_helm", "championshop", (3193, 3361, 1)),
        ("rune_longsword", "runiteshop", (3191, 3351, 1)),
        ("rune_sword", "runiteshop", (3191, 3351, 1)),
        ("rune_chainbody", "runiteshop", (3191, 3351, 1)),
        ("rune_platelegs", "runiteshop", (3191, 3351, 1)),
        ("rune_plateskirt", "runiteshop", (3191, 3351, 1)),
        ("rune_mace", "runiteshop", (3191, 3351, 1)),
        ("coif", "runiteshop", (3191, 3351, 1)),
    ];
    for (item, shop, site) in rows {
        let way = super::ways(item).into_iter().find(|way| way.kind == super::WayKind::Shop && way.detail == shop).unwrap_or_else(|| panic!("{item}: no {shop} way"));
        assert_eq!(way.quest_points, 32, "{item}: the guild door");
        assert_eq!(way.sites, vec![site], "{item}: the keeper upstairs");
        assert!(way.quest.is_none(), "{item}: no quest of its own");
        assert!(!super::merchant_route(item), "{item}: not open to a bot without the points");
        assert!(!super::merchant_route_for(item, 31), "{item}: 31 points stay outside");
        assert!(super::merchant_route_for(item, 32), "{item}: 32 points buy");
        assert_eq!(super::shop_quest_points(item), Some(32));
        assert!(super::merchant_price(item).is_some_and(|price| price > 0), "{item}: a stocked keeper prices it");
        assert!(super::unobtainable_reason(item).is_none(), "{item}");
    }
    assert_eq!(super::shop_quest_points("bronze_pickaxe"), Some(0), "Nurmof's floor is open");
    assert_eq!(super::shop_quest_points("black_platelegs"), Some(0), "Louie sells them in Al Kharid too");
    assert!(super::merchant_route("black_platelegs"));
    assert!(super::merchant_route("bronze_pickaxe"));
    assert_eq!(super::shop_quest_points("rune_full_helm"), None, "no keeper sells it");
}

/// Lundail (`magearena_runeshop`, the only F2P-flagged nature rune
/// keeper) stands in the Mage Arena cellar (2534,4719), reached by the
/// ladder north of the arena (~(3083,3963), `mage_arena.rs2` telejump
/// -549/+756). Neither square is in `maps/free2play.csv`: an F2P world
/// never spawns him (`GameMap.loadNpcs` skips members squares) and
/// refuses the telejump (`ServerOps` p_telejump `isFreeToPlay`). No shop
/// way, whatever the Wilderness rule.
#[test]
fn lundail_is_not_on_the_free_map() {
    assert!(!crate::free_zones::is_free(2534, 4719), "the cellar");
    assert!(!crate::free_zones::is_free(3083, 3963), "the ladder north of the arena");
    assert!(crate::free_zones::is_free(3083, 3895), "the F2P Wilderness stops below");
    let shops: Vec<String> = super::ways("naturerune").iter().filter(|way| way.kind == super::WayKind::Shop).map(|way| way.describe()).collect();
    assert!(shops.is_empty(), "{shops:?}");
    assert!(!super::merchant_route_for("naturerune", i64::MAX));
    assert!(super::obtainable_f2p("naturerune"), "drops still give it");
}

/// Davon's amulet store (Brimhaven, `amuletshop`) stocks every amulet at
/// 0: what players sell him only, never a purchase (amulet of power).
#[test]
fn davon_sells_nothing() {
    assert!(!super::ways("amulet_of_power").iter().any(|way| way.kind == super::WayKind::Shop));
    assert!(!super::merchant_route_for("amulet_of_power", i64::MAX));
    assert_eq!(super::merchant_price("amulet_of_power"), None);
}

/// Only the server's pickables pick (`pickables.rs2`): no pick way for
/// cooking apples, redberries, grapes or tomatoes.
#[test]
fn only_server_pickables_pick() {
    for item in ["cooking_apple", "redberries", "grapes", "tomato"] {
        assert!(!super::ways(item).iter().any(|way| way.detail.contains("pick") || way.detail.contains("field")), "{item}: {:?}", super::ways(item));
    }
    assert!(!super::ways("cabbage").is_empty());
}

/// A stock line at 0 (Grum's jewellery, Drogo's ores and bars, Gerrant's
/// raw fish) only holds what players sell the keeper, and no bot sells
/// to an NPC: never a purchase route, never a shop price. Live staff
/// checks 2026-11-16: "trade gave no diamond_ring" at Grum's, every time.
#[test]
fn empty_stock_lines_are_no_merchant_route() {
    for item in ["diamond_ring", "gold_ring", "strung_gold_amulet", "copper_ore", "iron_bar", "gold_bar"] {
        assert!(!super::merchant_route(item), "{item} has no stocked keeper");
        assert_eq!(super::merchant_price(item), None, "{item}");
    }
    for item in ["hammer", "bronze_pickaxe", "bronze_scimitar"] {
        assert!(super::merchant_route(item), "{item} is stocked");
    }
}

/// User 2026-11-19: after a market delivery the pack is banked but its
/// tools (the paused task resumes from the square): every object some
/// F2P way carries as a tool, not only axes, pickaxes and nets (the gem
/// cutters' chisel was banked and the task blocked on it).
#[test]
fn carried_tools_cover_every_recipe_tool() {
    for tool in ["chisel", "hammer", "tinderbox", "needle", "ring_mould", "amulet_mould", "bronze_pickaxe", "bronze_axe", "net"] {
        assert!(super::is_carried_tool(tool), "{tool}");
    }
    for product in ["yew_logs", "coins", "raw_shrimp", "iron_ore", "sapphire"] {
        assert!(!super::is_carried_tool(product), "{product}");
    }
}
