use super::{bid_only_items, classify, BotReach, RareClass, RestingOrder};

fn levels(rows: &[(&str, u8)]) -> Vec<(String, u8)> {
    rows.iter().map(|(skill, level)| (skill.to_string(), *level)).collect()
}

/// A mid-level producer without quests and a low-level quester with
/// every F2P quest (39 quest points).
fn fleet() -> Vec<BotReach> {
    let producer = BotReach {
        account: "producer".to_string(),
        levels: levels(&[
            ("attack", 40),
            ("strength", 40),
            ("defence", 40),
            ("hitpoints", 40),
            ("smithing", 52),
            ("mining", 60),
            ("crafting", 40),
            ("fishing", 55),
            ("cooking", 60),
            ("magic", 30),
            ("prayer", 31),
            ("woodcutting", 45),
            ("firemaking", 40),
            ("runecraft", 20),
            ("ranged", 30),
        ]),
        quests_done: Vec::new(),
    };
    let quester = BotReach {
        account: "quester".to_string(),
        levels: levels(&[("attack", 30), ("defence", 30), ("strength", 30), ("hitpoints", 30)]),
        quests_done: lifescape_gamedata::quest_requirements::all_f2p_quests().into_iter().map(|quest| quest.key).collect(),
    };
    vec![producer, quester]
}

fn class_of(item: &str, fleet: &[BotReach]) -> (RareClass, String) {
    let verdict = classify(item, fleet);
    eprintln!("{item}: {} — {}", verdict.class.label(), verdict.detail);
    (verdict.class, verdict.detail)
}

/// Golden classes of the bid-only objects of 2026-11-16 (plan-marche
/// §1.1, §4.4), from the server data and the planner legs only.
#[test]
fn rare_objects_fall_in_their_class() {
    let fleet = fleet();
    // The chain exists, nobody runs it.
    for item in ["stringstar", "leather_vambraces", "swordfish", "raw_swordfish"] {
        assert_eq!(class_of(item, &fleet).0, RareClass::ProducibleNow, "{item}");
    }
    // An F2P keeper sells it: Gerrant's harpoon, Valaine's adamant
    // platebody behind the Champions' Guild door (Q7), passed by the
    // quester only.
    assert_eq!(class_of("harpoon", &fleet).0, RareClass::ShopF2p);
    let (class, detail) = class_of("adamant_platebody", &fleet);
    assert_eq!(class, RareClass::ShopF2p);
    assert!(detail.contains("championshop") && detail.contains("32 quest points: 1/2 bots pass"), "{detail}");
    let (class, detail) = class_of("rune_longsword", &fleet);
    assert_eq!(class, RareClass::ShopF2p, "Scavvo");
    assert!(detail.contains("runiteshop"), "{detail}");
    // A smith could, none is high enough; no keeper.
    let (class, detail) = class_of("rune_full_helm", &fleet);
    assert_eq!(class, RareClass::OutOfReach);
    assert!(detail.contains("smithing 92") && detail.contains("fleet best 52"), "{detail}");
    let (class, detail) = class_of("amulet_of_power", &fleet);
    assert_eq!(class, RareClass::OutOfReach, "Davon's stock is 0: enchanting only");
    assert!(detail.contains("magic 57"), "{detail}");
    // C13: the Holy symbol is `blessedstar`, blessed by Brother Jered
    // upstairs in the Monastery (order members: prayer 31), a service no
    // planner leg runs; `stringstar` is the Unblessed symbol the fleet
    // can make.
    let (class, detail) = class_of("blessedstar", &fleet);
    assert_eq!(class, RareClass::Unfindable);
    assert!(detail.contains("brother_jered") && detail.contains("stringstar"), "{detail}");
}

/// The fleet's levels move the class: the same rune full helm is made
/// by a smithing-92 bot once runite ore is mined (mining 85; smithing
/// alone leaves the bar out of reach), and a fleet without the quester
/// sees the guild door shut to everyone (still a shop: the ladder opens
/// it).
#[test]
fn the_fleet_decides_the_class() {
    let mut fleet = fleet();
    fleet[0].levels.retain(|(skill, _)| skill != "smithing");
    fleet[0].levels.push(("smithing".to_string(), 92));
    let (class, detail) = class_of("rune_full_helm", &fleet);
    assert_eq!(class, RareClass::OutOfReach);
    assert!(detail.contains("runite_bar"), "the bar is the gap now: {detail}");
    fleet[0].levels.retain(|(skill, _)| skill != "mining");
    fleet[0].levels.push(("mining".to_string(), 85));
    assert_eq!(class_of("rune_full_helm", &fleet).0, RareClass::ProducibleNow);
    let alone = vec![fleet[0].clone()];
    let (class, detail) = class_of("rune_longsword", &alone);
    assert_eq!(class, RareClass::ProducibleNow, "smithing 92 makes the longsword (91): {detail}");
    let (class, detail) = class_of("coif", &alone);
    assert_eq!(class, RareClass::ShopF2p);
    assert!(detail.contains("0/1 bots pass"), "{detail}");
    assert_eq!(class_of("blessedstar", &[]).0, RareClass::Unfindable);
}

/// Bid-only objects: an open bid from a counted account and no ask at
/// all (staff out, a staff ask still counts as supply).
#[test]
fn bid_only_items_skip_asked_and_staff() {
    let order = |item: &str, account: &str, buy: bool| RestingOrder { item: item.to_string(), account: account.to_string(), buy };
    let orders = vec![
        order("blessedstar", "fiona_park", true),
        order("blessedstar", "ida_reed", true),
        order("blessedstar", "ida_reed", true),
        order("coal", "ida_reed", true),
        order("coal", "liam_west", false),
        order("rune_axe", "admin", true),
        order("feather", "admin2", false),
        order("feather", "noah_hill", true),
    ];
    let counted = |account: &str| !matches!(account, "admin" | "admin2" | "bite");
    let rows = bid_only_items(&orders, &counted);
    assert_eq!(rows, vec![("blessedstar".to_string(), 3, 2)]);
}
