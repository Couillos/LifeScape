//! Unit tests of `expand` (split out, rule 4).

use super::{expand_goal, reach_method, resolve_ready, tranches, ReadyCtx};
use crate::model::{Archetype, GoalKind, Node, NodeKind, Profile};

fn profile() -> Profile {
    Profile::new(false, 0.72, 0.4, 7, Archetype::Producer)
}

#[test]
fn max_skills_expands_tranches_in_order() {
    let nodes = expand_goal(&GoalKind::MaxSkills { skills: vec!["fishing".to_string()], level: 40 }, &profile());
    let levels: Vec<u8> = nodes
        .iter()
        .filter_map(|node| match &node.kind {
            NodeKind::ReachLevel { level, .. } => Some(*level),
            _ => None,
        })
        .collect();
    assert_eq!(levels, vec![5, 10, 20, 25, 35, 40], "D21 tranches up to the goal");
    for pair in nodes.windows(2) {
        assert_eq!(pair[1].deps, vec![pair[0].id.clone()], "linear chain");
    }
    assert!(nodes.iter().all(|node| node.method.is_none()), "methods resolve at ready, not at creation");
}

#[test]
fn three_goals_expand() {
    let quest = expand_goal(&GoalKind::CompleteQuest { quest: "cook".to_string() }, &profile());
    assert!(quest.iter().any(|node| matches!(&node.kind, NodeKind::ObtainItems { item, .. } if item == "bucket_milk")), "milk sub-task");
    assert!(quest.iter().any(|node| matches!(&node.kind, NodeKind::ObtainItems { item, .. } if item == "pot_flour")), "flour sub-task");
    assert!(quest.iter().any(|node| matches!(&node.kind, NodeKind::CompleteQuest { quest } if quest == "cook")), "quest leaf");
    let leaf = quest.last().expect("leaf");
    assert_eq!(leaf.deps.len(), 3, "milk + egg + flour gate the quest");

    let rich = expand_goal(&GoalKind::GetRich { merchant: Some(true) }, &profile());
    assert_eq!(rich.len(), 1);
    assert!(rich[0].alternatives.iter().any(|alt| alt.method == "market:flip"), "merchant flips");

    let equip = expand_goal(&GoalKind::EquipBest { style: "melee".to_string() }, &profile());
    assert!(equip.len() >= 2, "obtains then equip");
    let leaf = equip.last().expect("equip leaf");
    assert!(matches!(leaf.kind, NodeKind::EquipBest { .. }));
    assert_eq!(leaf.deps.len(), equip.len() - 1, "equip waits on every piece");
    let ranged = expand_goal(&GoalKind::EquipBest { style: "ranged".to_string() }, &profile());
    assert!(ranged.iter().any(|node| matches!(&node.kind, NodeKind::ObtainItems { item, .. } if item.contains("bow"))), "a bow");
    assert!(ranged.iter().any(|node| matches!(&node.kind, NodeKind::ObtainItems { item, count } if item.contains("arrow") && *count == 100)), "arrows");
}

#[test]
fn tranche_resources_are_fixed() {
    assert_eq!(reach_method("fishing", 20), "gather:fishing/raw_trout");
    assert_eq!(reach_method("woodcutting", 30), "gather:woodcut/willow_logs");
    assert_eq!(reach_method("mining", 30), "gather:mining/coal");
    assert_eq!(tranches("fishing", 1, 40), vec![5, 10, 20, 25, 35, 40]);
}

/// P3.6: every skill owns its reach tiers and family methods (D21,
/// catalogue §8). Tiers never move without a catalogue reason.
#[test]
fn every_skill_owns_its_reach_tiers() {
    assert_eq!(tranches("firemaking", 1, 99), vec![15, 30, 60, 99]);
    assert_eq!(tranches("smithing", 1, 99), vec![15, 20, 30, 40, 50, 70, 85, 99]);
    assert_eq!(tranches("crafting", 1, 99), vec![5, 7, 8, 16, 20, 27, 34, 43, 70, 99]);
    assert_eq!(tranches("runecraft", 1, 99), vec![2, 5, 9, 14, 20, 99]);
    assert_eq!(tranches("prayer", 1, 99), vec![99], "one tranche, burial in-loop");
    assert_eq!(tranches("magic", 1, 99), vec![10, 20, 30, 40, 99]);
    assert_eq!(reach_method("firemaking", 30), "fire:willow_logs");
    assert_eq!(reach_method("smithing", 30), "smelt:steel_bar");
    assert_eq!(reach_method("smithing", 1), "smelt:bronze_bar");
    assert_eq!(reach_method("crafting", 1), "spin:ball_of_wool");
    assert_eq!(reach_method("crafting", 7), "pottery:pie_dish");
    assert_eq!(reach_method("crafting", 8), "jewelry:gold_amulet");
    assert_eq!(reach_method("crafting", 70), "jewelry:diamond_amulet");
    assert_eq!(reach_method("runecraft", 9), "runecraft:earthrune");
    assert_eq!(reach_method("prayer", 99), "fight:melee/cows");
    assert_eq!(reach_method("ranged", 20), "fight:ranged/cows");
    assert_eq!(reach_method("magic", 20), "fight:magic/cows");
}

#[test]
fn iron_never_lists_market() {
    let iron = Profile::new(true, 0.9, 0.4, 7, Archetype::Producer);
    let nodes = expand_goal(&GoalKind::Obtain { item: "feather".to_string(), count: 100 }, &iron);
    assert!(nodes[0].alternatives.iter().all(|alt| alt.method != "market:buy"), "no market for Iron");
    let rich = expand_goal(&GoalKind::GetRich { merchant: Some(true) }, &iron);
    assert!(rich[0].alternatives.iter().all(|alt| alt.method != "market:flip"), "Iron never merchants");
}

#[test]
fn tools_never_farm_shops_instead() {
    // A net comes from shops, never from the field: the farm leg
    // must not exist, or the brain would hunt chickens for tackle.
    let nodes = expand_goal(&GoalKind::Obtain { item: "net".to_string(), count: 1 }, &profile());
    let methods: Vec<&str> = nodes[0].alternatives.iter().map(|alt| alt.method.as_str()).collect();
    assert!(!methods.iter().any(|method| method.starts_with("farm:")), "nets never farm, got {methods:?}");
    assert!(methods.contains(&"shop:buy"), "nets come from shops, got {methods:?}");
    // Feathers drop from chickens: the farm leg stays.
    let feather = expand_goal(&GoalKind::Obtain { item: "feather".to_string(), count: 100 }, &profile());
    assert!(feather[0].alternatives.iter().any(|alt| alt.method == "farm:feather"), "feathers farm");
}

#[test]
fn access_area_derives_guild_deps() {
    // Cooks: cooking 32 chain plus the toque to obtain, gating the
    // access node; mining: level only, no uniform; champions: the
    // cheapest QP rung; unknown zones stay bare (no known gate).
    let cooks = expand_goal(&GoalKind::AccessArea { area: "cooks_guild".to_string() }, &profile());
    let leaf = cooks.last().expect("access leaf");
    assert!(matches!(leaf.kind, NodeKind::AccessArea { .. }), "leaf accesses, got {:?}", leaf.kind);
    assert!(!leaf.deps.is_empty(), "guild gate leaves no depless node");
    assert!(cooks.iter().any(|node| matches!(&node.kind, NodeKind::ReachLevel { skill, level } if skill == "cooking" && *level == 32)));
    assert!(cooks.iter().any(|node| matches!(&node.kind, NodeKind::ObtainItems { item, .. } if item == "chefs_hat")));
    let mining = expand_goal(&GoalKind::AccessArea { area: "mining_guild".to_string() }, &profile());
    let leaf = mining.last().expect("access leaf");
    assert!(!leaf.deps.is_empty(), "level gate leaves no depless node");
    assert!(mining.iter().any(|node| matches!(&node.kind, NodeKind::ReachLevel { skill, level } if skill == "mining" && *level == 60)));
    assert!(!mining.iter().any(|node| matches!(node.kind, NodeKind::ObtainItems { .. })), "no uniform on the mining guild");
    let champions = expand_goal(&GoalKind::AccessArea { area: "champions_guild".to_string() }, &profile());
    let leaf = champions.last().expect("access leaf");
    assert!(!leaf.deps.is_empty(), "qp gate leaves no depless node");
    assert!(champions.iter().any(|node| matches!(&node.kind, NodeKind::CompleteQuest { quest } if quest == "cook")));
    let bare = expand_goal(&GoalKind::AccessArea { area: "karamja".to_string() }, &profile());
    assert_eq!(bare.len(), 1, "unknown zone stays bare, got {bare:?}");
}

#[test]
fn space_cleanup_locks_its_method() {
    let mut node = Node::new("n5", NodeKind::SpaceCleanup, Vec::new(), 0);
    resolve_ready(&mut node, &ReadyCtx::blind(0.5));
    assert_eq!(node.method.as_deref(), Some("space:cleanup"), "cleanup resolves directly");
}

#[test]
fn ready_resolution_prices_and_picks() {
    // PRD3 lot 4: a covered ask is taken — by a bid at the ask while the
    // bot produces (2026-11-13: never idle on a handover); without one the
    // bot produces with a standing bid beside it (D-B); broke, it still
    // produces; an object it cannot make is bought and waited for.
    let mut nodes = expand_goal(&GoalKind::Obtain { item: "feather".to_string(), count: 200 }, &profile());
    let mut ctx = ReadyCtx::blind(0.72);
    ctx.coins_per_tick = 2.0;
    ctx.price = 3;
    ctx.shop_price = 5;
    ctx.yield_per_6000 = 3500.0;
    ctx.coins = 100000;
    ctx.best_ask = Some(3);
    resolve_ready(&mut nodes[0], &ctx);
    assert_eq!(nodes[0].method.as_deref(), Some("farm:feather"), "covered ask: produce meanwhile");
    assert!(nodes[0].alternatives.iter().any(|alt| alt.method == "market:bid"), "the bid takes the ask beside it");
    assert!(nodes[0].alternatives.iter().any(|alt| alt.est_ticks > 0), "estimates fill au plus tard");

    let mut no_ask = nodes[0].clone();
    no_ask.method = None;
    let mut quiet = ctx.clone();
    quiet.best_ask = None;
    resolve_ready(&mut no_ask, &quiet);
    assert_eq!(no_ask.method.as_deref(), Some("farm:feather"), "no seller: produce it");
    assert!(no_ask.alternatives.iter().any(|alt| alt.method == "market:bid"), "standing bid beside production");

    let mut broke = quiet.clone();
    broke.coins = 0;
    let mut skint = nodes[0].clone();
    skint.method = None;
    resolve_ready(&mut skint, &broke);
    assert_eq!(skint.method.as_deref(), Some("farm:feather"), "broke bots produce");
    assert!(!skint.alternatives.iter().any(|alt| alt.method == "market:bid"), "no coins, no bid");

    // Live 2026-11-12: 340 raw chickens resting at 1 coin, a
    // cook with 25 coins needing 100 hunted chickens instead. A wallet
    // covering part of the need takes that part at the ask.
    let mut thin = ctx.clone();
    thin.coins = 25;
    thin.best_ask = Some(1);
    thin.price = 1;
    let mut partial = nodes[0].clone();
    partial.method = None;
    resolve_ready(&mut partial, &thin);
    assert_eq!(partial.method.as_deref(), Some("farm:feather"), "the bot produces the rest meanwhile");
    assert!(partial.alternatives.iter().any(|alt| alt.method == "market:bid"), "the covered part is bid at the ask");
    assert_eq!(crate::resolve_obtain::buy_quantity(&thin, 200), 25, "the wallet caps the order");
    let mut stocked = thin.clone();
    stocked.held = 190;
    assert_eq!(crate::resolve_obtain::buy_quantity(&stocked, 200), 10, "only what is still missing");
    // An ask far above the item's value is no bargain: produce instead.
    let mut dear = thin.clone();
    dear.best_ask = Some(21);
    dear.coins = 25;
    let mut steep = nodes[0].clone();
    steep.method = None;
    resolve_ready(&mut steep, &dear);
    assert_eq!(steep.method.as_deref(), Some("farm:feather"), "an unfair partial ask is not taken");

    // A net is not producible: the merchant sells it (D-A), never the field.
    let mut net = expand_goal(&GoalKind::Obtain { item: "net".to_string(), count: 1 }, &profile());
    resolve_ready(&mut net[0], &quiet);
    assert!(matches!(net[0].method.as_deref(), Some("shop:buy") | Some("market:buy")), "nets never farm, got {:?}", net[0].method);
}

/// PRD3 lot 4: nothing workable, nothing affordable: the block names
/// what unlocks a way (skill level, then coins), never a silent skip.
#[test]
fn obtain_blocks_on_level_then_coins() {
    use crate::model::NodeStatus;
    let mut sword = expand_goal(&GoalKind::Obtain { item: "bronze_sword".to_string(), count: 1 }, &profile());
    let mut ctx = ReadyCtx::blind(0.5);
    ctx.levels = vec![("smithing".to_string(), 1)];
    ctx.coins = 0;
    resolve_ready(&mut sword[0], &ctx);
    assert!(
        matches!(&sword[0].status, NodeStatus::Blocked { reason } if reason == "missing:level:smithing:4"),
        "the bronze sword smiths at 4 (dump), got {:?}",
        sword[0].status
    );
    let mut ready_sword = expand_goal(&GoalKind::Obtain { item: "bronze_sword".to_string(), count: 1 }, &profile());
    ctx.levels = vec![("smithing".to_string(), 4)];
    resolve_ready(&mut ready_sword[0], &ctx);
    assert_eq!(ready_sword[0].method.as_deref(), Some("smith:bronze_sword"), "at 4 it smiths: {:?}", ready_sword[0].status);

    let mut plate = expand_goal(&GoalKind::Obtain { item: "steel_platebody".to_string(), count: 1 }, &profile());
    resolve_ready(&mut plate[0], &ctx);
    assert!(
        matches!(&plate[0].status, NodeStatus::Blocked { reason } if reason.starts_with("missing:level:smithing:")),
        "steel plate needs smithing levels first, got {:?}",
        plate[0].status
    );

    let mut runes = expand_goal(&GoalKind::Obtain { item: "naturerune".to_string(), count: 10 }, &profile());
    let mut weak = ReadyCtx::blind(0.5);
    weak.coins = 0;
    resolve_ready(&mut runes[0], &weak);
    // Nature runes drop from wizards: farmable, so the drop farm runs.
    assert_eq!(runes[0].method.as_deref(), Some("farm:naturerune"), "got {:?}", runes[0].status);
}

/// t04: a net in the pack and no coins, handed sardines, never
/// touches the rod it lacks — the planner drops the tranche for
/// the workable one (or blocks typed when nothing works).
#[test]
fn tool_gate_drops_unworkable_tranches() {
    use super::{method_feasible, required_tool};
    use crate::model::NodeStatus;
    fn sardine_node() -> Node {
        let mut node = Node::new(
            "n1",
            NodeKind::ReachLevel { skill: "fishing".to_string(), level: 5 },
            Vec::new(),
            5,
        );
        node.alternatives.push(crate::model::Alternative {
            method: "gather:fishing/raw_sardine".to_string(),
            est_ticks: 0,
            est_coins: 0,
        });
        node
    }
    // The table names rod plus bait for sardines, net alone for shrimp.
    let need = required_tool("gather:fishing/raw_sardine").expect("sardines need gear");
    assert_eq!((need.tool, need.bait), ("fishing_rod", Some("fishing_bait")));
    assert_eq!(required_tool("gather:fishing/raw_shrimp").expect("shrimp needs gear").tool, "net");
    assert!(required_tool("fight:melee/cows").is_none(), "melee needs no gear");
    let bow = required_tool("fight:ranged/cows").expect("ranged needs a bow");
    assert_eq!((bow.tool, bow.bait), ("shortbow", Some("bronze_arrow")));
    let runes = required_tool("fight:magic/cows").expect("magic needs runes");
    assert_eq!((runes.tool, runes.bait), ("mindrune", Some("airrune")));
    assert_eq!(required_tool("gather:sheep/wool").expect("shears").tool, "shears");

    // Rod-less and broke: sardines unworkable. No substitute method
    // (rule 7): the node blocks typed on the rod, the brain files the
    // obtain sub-task.
    let mut with_net = sardine_node();
    let mut ctx = ReadyCtx::blind(0.5);
    ctx.tools = vec!["net".to_string()];
    assert!(!method_feasible("gather:fishing/raw_sardine", &ctx), "no rod, no bait, no coins");
    assert!(method_feasible("gather:fishing/raw_shrimp", &ctx), "the net works");
    resolve_ready(&mut with_net, &ctx);
    assert_eq!(with_net.method, None, "no substitute, got {:?}", with_net.method);
    assert!(
        matches!(&with_net.status, NodeStatus::Blocked { reason } if reason == "missing:fishing_rod"),
        "blocked on the rod, got {:?}",
        with_net.status
    );
    // Rod carried, bait missing: the bait is what blocks.
    let mut with_rod = sardine_node();
    ctx.tools = vec!["fishing_rod".to_string()];
    resolve_ready(&mut with_rod, &ctx);
    assert!(matches!(&with_rod.status, NodeStatus::Blocked { reason } if reason == "missing:fishing_bait"), "got {:?}", with_rod.status);

    // Same empty pack but funded: the rod and the bait are shop
    // stock, so the tranche stays (the script bootstraps the buy).
    let mut funded = sardine_node();
    let mut rich = ReadyCtx::blind(0.5);
    rich.coins = 100;
    assert!(method_feasible("gather:fishing/raw_sardine", &rich), "rod plus bait fundable");
    resolve_ready(&mut funded, &rich);
    assert_eq!(funded.method.as_deref(), Some("gather:fishing/raw_sardine"), "buyable gear keeps the tranche");

    // Nothing carried, nothing funded, no shop: typed block naming
    // the missing tool (the obtain chain funds it), never Usage,
    // never a silent skip.
    let mut broke = sardine_node();
    let mut dry = ReadyCtx::blind(0.5);
    dry.allow_shop = false;
    resolve_ready(&mut broke, &dry);
    assert!(broke.method.is_none(), "no method locks, got {:?}", broke.method);
    assert!(
        matches!(&broke.status, NodeStatus::Blocked { reason } if reason == "missing:fishing_rod"),
        "blocked names the rod, got {:?}",
        broke.status
    );
}

/// Bank contents unseen this session: a tool may be banked, so the
/// gate never blocks on it (the script restores it or blocks typed).
#[test]
fn unseen_bank_never_blocks_on_a_tool() {
    let mut node = Node::new("n1", NodeKind::ReachLevel { skill: "fishing".to_string(), level: 5 }, Vec::new(), 5);
    node.alternatives.push(crate::model::Alternative { method: "gather:fishing/raw_shrimp".to_string(), est_ticks: 0, est_coins: 0 });
    let mut ctx = ReadyCtx::blind(0.5);
    ctx.bank_known = false;
    resolve_ready(&mut node, &ctx);
    assert_eq!(node.method.as_deref(), Some("gather:fishing/raw_shrimp"), "got {:?}", node.status);
    let mut seen = Node::new("n2", NodeKind::ReachLevel { skill: "fishing".to_string(), level: 5 }, Vec::new(), 5);
    seen.alternatives.push(crate::model::Alternative { method: "gather:fishing/raw_shrimp".to_string(), est_ticks: 0, est_coins: 0 });
    ctx.bank_known = true;
    ctx.allow_shop = false;
    resolve_ready(&mut seen, &ctx);
    assert!(matches!(&seen.status, crate::model::NodeStatus::Blocked { reason } if reason == "missing:net"), "seen and empty blocks, got {:?}", seen.status);
}

/// Every combat tranche names a real `combat_spots` key: an unknown
/// key used to send melee to its auto pick (goblins, cows: deaths).
#[test]
fn combat_tranches_name_real_spots() {
    for skill in ["attack", "strength", "defence", "hitpoints", "ranged", "magic"] {
        for level in [1u8, 9, 10, 20, 30, 40] {
            let method = reach_method(skill, level);
            let spot = method.rsplit('/').next().expect("spot part");
            assert!(lifescape_gamedata::combat_spots::by_key(spot).is_some(), "{skill} {level}: {method} names no spot");
        }
    }
}

/// Each reach tranche works the tier open at its start level: a
/// level-1 fisher never gets sardines (level 5) or lobsters (40).
#[test]
fn reach_tranches_work_the_tier_open_at_their_start() {
    let mut counter = 0;
    let mut after = String::new();
    let chain = super::reach_chain("fishing", 1, 40, &mut counter, &mut after);
    let methods: Vec<(u8, String)> = chain
        .iter()
        .map(|node| match &node.kind {
            NodeKind::ReachLevel { level, .. } => (*level, node.alternatives[0].method.clone()),
            other => panic!("reach nodes only, got {other:?}"),
        })
        .collect();
    let expected: Vec<(u8, String)> = [
        (5, "raw_shrimp"),
        (10, "raw_sardine"),
        (20, "raw_sardine"),
        (25, "raw_trout"),
        (35, "raw_pike"),
        (40, "raw_tuna"),
    ]
    .iter()
    .map(|(level, fish)| (*level, format!("gather:fishing/{fish}")))
    .collect();
    assert_eq!(methods, expected);
    let mut counter = 0;
    let mut after = String::new();
    let mining = super::reach_chain("mining", 12, 20, &mut counter, &mut after);
    assert_eq!(mining[0].alternatives[0].method, "gather:mining/copper_ore", "12→15 mines copper, iron needs 15");
    assert_eq!(mining[1].alternatives[0].method, "gather:mining/iron_ore", "15→20 mines iron");
}

/// PRD3 D-A: a producible object never gets an NPC shop leg (bronze
/// sword: smithed; air runes: runecrafted); a non-producible one keeps
/// it (shears, nature runes are not sold in F2P shops at all).
#[test]
fn merchant_legs_follow_d_a() {
    for producible in ["bronze_sword", "airrune", "pot_flour"] {
        let nodes = expand_goal(&GoalKind::Obtain { item: producible.to_string(), count: 1 }, &profile());
        assert!(!nodes[0].alternatives.iter().any(|alt| alt.method == "shop:buy"), "{producible}: no shop leg, got {:?}", nodes[0].alternatives);
    }
    let shears = expand_goal(&GoalKind::Obtain { item: "shears".to_string(), count: 1 }, &profile());
    assert!(shears[0].alternatives.iter().any(|alt| alt.method == "shop:buy"), "shears stay buyable");
    // The tool gate never funds a producible tool from a shop.
    let mut ctx = ReadyCtx::blind(0.5);
    ctx.coins = 1000;
    assert!(!super::gear_ok("bronze_axe", &ctx), "bronze axe is smithed: no merchant funding");
    assert!(super::gear_ok("net", &ctx), "nets are merchant goods");
}

/// Live bct5rxpg (2026-11-12): shrimp fishing needs combat 15 (Draynor
/// wizards), so the site pick fell back to `market:buy` the resolver had
/// refused for want of coins. The fallback only ever tries field
/// methods; with none safe, the node blocks on the combat level.
#[test]
fn site_fallback_never_revives_a_refused_purchase() {
    use crate::model::NodeStatus;
    let mut nodes = expand_goal(&GoalKind::Obtain { item: "raw_shrimp".to_string(), count: 1 }, &profile());
    let mut ctx = ReadyCtx::blind(0.5);
    ctx.coins = 0;
    ctx.combat = 3;
    ctx.tools = vec!["net".to_string()];
    ctx.position = Some((3222, 3218, 0));
    resolve_ready(&mut nodes[0], &ctx);
    assert_ne!(nodes[0].method.as_deref(), Some("market:buy"), "no coins: never a purchase, got {:?}", nodes[0].status);
    assert_ne!(nodes[0].method.as_deref(), Some("market:bid"), "the bid marker is no method");
    assert!(matches!(&nodes[0].status, NodeStatus::Blocked { reason } if reason.starts_with("missing:combat:")), "got {:?}", nodes[0].status);
}

/// 2026-11-13: an object the bot cannot make (no workable production)
/// with a covered ask is a pure buy: the brain places it and waits.
#[test]
fn unmakeable_covered_ask_is_a_pure_buy() {
    let mut nodes = expand_goal(&GoalKind::Obtain { item: "net".to_string(), count: 1 }, &profile());
    let mut ctx = ReadyCtx::blind(0.72);
    ctx.coins = 500;
    ctx.price = 5;
    ctx.best_ask = Some(5);
    resolve_ready(&mut nodes[0], &ctx);
    assert_eq!(nodes[0].method.as_deref(), Some("market:buy"), "{:?}", nodes[0].alternatives);
}
