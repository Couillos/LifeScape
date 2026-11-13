use super::*;
use crate::expand::{resolve_ready, ReadyCtx};
use crate::lifecycle::{insert_gear_step, normalize, withdraw_gear_step};
use crate::model::{Archetype, GoalKind, HistoryEntry, Life, Node, NodeKind, NodeStatus, Profile};

fn owned(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| name.to_string()).collect()
}

fn step_item(levels: &[(&str, i64)], have: &[&str]) -> Option<String> {
    next_gear_step(levels, &owned(have)).map(|step| step.item)
}

/// One tier at a time, weapon first; a level gate hands over to the
/// next slot; never the absolute best piece.
#[test]
fn ladder_climbs_one_tier_per_levels() {
    let fresh = [("attack", 1), ("defence", 1)];
    assert_eq!(step_item(&fresh, &[]).as_deref(), Some("bronze_scimitar"), "weapon first, bronze first");
    assert_eq!(step_item(&fresh, &["bronze_scimitar"]).as_deref(), Some("iron_scimitar"), "next tier: iron");
    assert_eq!(
        step_item(&fresh, &["iron_scimitar"]).as_deref(),
        Some("bronze_platebody"),
        "steel needs attack 5: the body comes next"
    );
    assert_eq!(step_item(&[("attack", 5), ("defence", 1)], &["iron_scimitar"]).as_deref(), Some("steel_scimitar"));
    assert_eq!(step_item(&[("attack", 19), ("defence", 1)], &["steel_scimitar", "iron_platebody"]).as_deref(), Some("bronze_platelegs"));
    assert_eq!(step_item(&[("attack", 20), ("defence", 1)], &["steel_scimitar"]).as_deref(), Some("mithril_scimitar"));
    assert_eq!(step_item(&[("attack", 40), ("defence", 40)], &["bronze_scimitar"]).as_deref(), Some("iron_scimitar"), "never skips a tier");
    assert_eq!(step_item(&[("attack", 20), ("defence", 1)], &["black_scimitar"]).as_deref(), Some("mithril_scimitar"), "black ranks with steel");
    // Black is never a step (no smithing recipe: D-A merchant item).
    for levels in [[("attack", 10), ("defence", 10)], [("attack", 1), ("defence", 1)]] {
        let mut have: Vec<String> = Vec::new();
        while let Some(step) = next_gear_step(&levels, &have) {
            assert!(!step.item.starts_with("black_"), "{}", step.item);
            assert!(
                !lifescape_gamedata::obtain::merchant_buy_allowed(&step.item),
                "D-A: {} is producible, never a shop purchase",
                step.item
            );
            have.push(step.item);
        }
        assert!(have.len() >= 5, "every slot climbs: {have:?}");
    }
    // A quest-gated piece (rune platebody: Dragon Slayer) falls to the
    // next family of its slot.
    let rune = step_item(&[("attack", 1), ("defence", 40)], &["iron_scimitar", "adamant_platebody"]);
    assert_ne!(rune.as_deref(), Some("rune_platebody"));
}

#[test]
fn wallet_share_caps_and_rises_with_coins() {
    assert_eq!(wallet_ceiling(0, GEAR_SHARE_PCT), 0);
    assert_eq!(wallet_ceiling(15, GEAR_SHARE_PCT), 7);
    assert_eq!(wallet_ceiling(300, GEAR_SHARE_PCT), 150, "more coins, higher ceiling");
    assert_eq!(wallet_ceiling(100, TOOL_SHARE_PCT), 25);
    assert_eq!(coins_for(30, GEAR_SHARE_PCT), 60, "a 30-coin ask needs a 60-coin wallet");
    assert_eq!(coins_for(31, GEAR_SHARE_PCT), 62);
    assert_eq!(coins_for(7, TOOL_SHARE_PCT), 28);
}

#[test]
fn moves_follow_the_book_and_the_wallet() {
    let step = GearStep { slot: "weapon", item: "iron_scimitar".to_string(), rank: 1 };
    assert_eq!(gear_move(Some(&step), Some("bronze_scimitar"), 100, None, 1), GearMove::Equip { item: "bronze_scimitar".to_string() });
    assert_eq!(gear_move(None, None, 100, None, 1), GearMove::Wait);
    assert_eq!(gear_move(Some(&step), None, 100, Some(40), 1), GearMove::Obtain { item: "iron_scimitar".to_string() }, "a seller: buy or earn");
    assert_eq!(
        gear_move(Some(&step), None, 100, None, 8),
        GearMove::StandingBid { item: "iron_scimitar".to_string(), price: 8, ceiling: 50 },
        "no seller: standing bid at the reference, walking to half the wallet"
    );
    assert_eq!(gear_move(Some(&step), None, 1, None, 1), GearMove::Obtain { item: "iron_scimitar".to_string() }, "too poor to bid: earn first");
}

/// The ladder obtain is market only (never a shop, D-A), buys a covered
/// ask at once and blocks on the wallet whose share covers a dearer one.
#[test]
fn ladder_obtain_buys_within_its_share_or_earns() {
    let mut life = life_with(Vec::new());
    let ids = insert_gear_step(&mut life, Some("iron_scimitar"), "melee");
    assert_eq!(ids.len(), 2);
    let obtain = life.nodes.iter().position(|node| node.id == ids[0]).expect("obtain");
    let methods: Vec<&str> = life.nodes[obtain].alternatives.iter().map(|alt| alt.method.as_str()).collect();
    assert_eq!(methods, vec!["market:buy"], "market only, never shop:buy");
    assert!(is_gear_obtain(&life, obtain));
    let mut ctx = ReadyCtx::blind(0.5);
    ctx.coin_share_pct = Some(GEAR_SHARE_PCT);
    ctx.coins = 100;
    ctx.price = 1;
    ctx.best_ask = Some(40);
    let mut node = life.nodes[obtain].clone();
    resolve_ready(&mut node, &ctx);
    assert_eq!(node.method.as_deref(), Some("market:buy"), "40 <= half of 100");
    assert_eq!(crate::resolve_obtain::bid_ceiling(&ctx, 1), 50);
    ctx.best_ask = Some(70);
    let mut node = life.nodes[obtain].clone();
    resolve_ready(&mut node, &ctx);
    assert_eq!(node.status, NodeStatus::Blocked { reason: "missing:coins:140".to_string() }, "a 70-coin ask needs 140 in the wallet");
    ctx.best_ask = None;
    assert!(withdraw_due(&ctx), "no seller and a wallet to bid: the standing bid takes over");
    ctx.coins = 1;
    assert!(!withdraw_due(&ctx), "too poor to bid: the obtain earns first");
}

fn life_with(nodes: Vec<Node>) -> Life {
    Life {
        schema: 1,
        account: "t_gear".to_string(),
        created_at: 0,
        seed: 7,
        profile: Profile::new(false, 0.5, 0.5, 7, Archetype::Producer),
        goal: GoalKind::MaxSkills { skills: vec!["woodcutting".to_string()], level: 30 },
        nodes,
        engagement: None,
        active: None,
        history: Vec::new(),
        ambition: None,
        goal_origin: None,
    }
}

/// The gear sub-goal launches before the goal's next tranche, and a
/// withdrawn step leaves the graph without its ids ever coming back.
#[test]
fn gear_steps_launch_first_and_withdraw_cleanly() {
    let mut tranche = Node::new("n1", NodeKind::ReachLevel { skill: "woodcutting".to_string(), level: 20 }, Vec::new(), 20);
    tranche.status = NodeStatus::Ready;
    let mut life = life_with(vec![tranche]);
    assert_eq!(next_ready(&life), Some(0));
    assert!(!step_open(&life));
    let ids = insert_gear_step(&mut life, Some("iron_scimitar"), "melee");
    normalize(&mut life);
    assert!(step_open(&life));
    assert_eq!(life.nodes[next_ready(&life).expect("ready")].id, ids[0], "equipment first");
    assert_eq!(gear_subtree(&life), {
        let mut both = ids.clone();
        both.sort();
        both
    });
    let obtain = life.nodes.iter().position(|node| node.id == ids[0]).expect("obtain");
    let removed = withdraw_gear_step(&mut life, obtain);
    assert_eq!(removed.len(), 2, "{removed:?}");
    assert!(!step_open(&life));
    assert_eq!(life.nodes.len(), 1, "the goal's tranche stays");
    for id in &removed {
        life.history.push(HistoryEntry { node: id.clone(), started: 0, ended: 0, outcome: "gear_withdrawn".to_string() });
    }
    let again = insert_gear_step(&mut life, None, "melee");
    assert!(!removed.contains(&again[0]), "ids never reused: {again:?} after {removed:?}");
    assert_eq!(life.nodes.last().map(|node| node.status.clone()), Some(NodeStatus::Ready), "equip alone runs at once");
}

#[test]
fn owned_pieces_kept_and_worn() {
    let levels = [("attack", 1), ("defence", 1)];
    let have = owned(&["bronze_scimitar", "iron_scimitar", "bronze_platebody"]);
    let keep = kept_pieces(&levels, &have);
    assert!(keep.contains(&"iron_scimitar".to_string()) && keep.contains(&"bronze_platebody".to_string()), "{keep:?}");
    assert!(!keep.contains(&"bronze_scimitar".to_string()), "the lower tier is surplus: {keep:?}");
    assert_eq!(unworn_upgrade(&levels, &have, &owned(&["bronze_scimitar"])).as_deref(), Some("iron_scimitar"));
    assert_eq!(unworn_upgrade(&levels, &have, &owned(&["iron_scimitar", "bronze_platebody"])), None);
}

/// Review 2026-11-13 (finding 2): an abandoned ladder step kept a done
/// goal's graph Stuck (the brain withdrew it once a minute only), so the
/// goal closed stuck and its skill was banned for a gear cause. The
/// withdrawal is pure and runs before every `graph_state`.
#[test]
fn a_dead_gear_step_never_strands_the_goal() {
    let mut tranche = Node::new("n1", NodeKind::ReachLevel { skill: "woodcutting".to_string(), level: 20 }, Vec::new(), 20);
    tranche.status = NodeStatus::Done;
    let mut life = life_with(vec![tranche]);
    let ids = insert_gear_step(&mut life, Some("iron_scimitar"), "melee");
    normalize(&mut life);
    let obtain = life.nodes.iter().position(|node| node.id == ids[0]).expect("obtain");
    crate::lifecycle::record_contract_error(&mut life, obtain, "market bid refused");
    normalize(&mut life);
    assert!(matches!(crate::lifecycle::graph_state(&life), crate::lifecycle::GraphState::Stuck { .. }), "a dead step strands a done goal");
    let removed = withdraw_dead_steps(&mut life);
    assert_eq!(removed.len(), 2, "{removed:?}");
    assert_eq!(crate::lifecycle::graph_state(&life), crate::lifecycle::GraphState::Finished, "the goal closes done");
    assert!(withdraw_dead_steps(&mut life).is_empty(), "idempotent");
}

/// The launch pick skips the nodes waiting on a market fill: the goal's
/// next node runs meanwhile.
#[test]
fn the_next_ready_node_skips_market_waits() {
    let mut tranche = Node::new("n1", NodeKind::ReachLevel { skill: "woodcutting".to_string(), level: 20 }, Vec::new(), 20);
    tranche.status = NodeStatus::Ready;
    let mut life = life_with(vec![tranche]);
    let ids = insert_gear_step(&mut life, Some("iron_scimitar"), "melee");
    normalize(&mut life);
    assert_eq!(life.nodes[next_ready(&life).expect("ready")].id, ids[0], "gear first");
    let waiting = vec![ids[0].clone()];
    assert_eq!(next_ready_except(&life, &waiting).map(|index| life.nodes[index].id.clone()), Some("n1".to_string()));
    let both = vec![ids[0].clone(), "n1".to_string()];
    assert_eq!(next_ready_except(&life, &both), None);
}

/// Live 2026-11-13: amulet_of_power, iron_platebody/legs obtains of an
/// `equip melee` goal (the level-1 full set) sat on 1-coin bids. Melee
/// equipment is the ladder's (`brain_gear`): the goal is a lone equip
/// node, and old files lose their open set obtains.
#[test]
fn melee_equip_goals_leave_the_set_to_the_ladder() {
    let profile = Profile::new(false, 0.5, 0.5, 7, Archetype::Producer);
    let melee = crate::expand::expand_goal(&GoalKind::EquipBest { style: "melee".to_string() }, &profile);
    assert_eq!(melee.len(), 1, "{melee:?}");
    assert!(matches!(melee[0].kind, NodeKind::EquipBest { .. }) && melee[0].deps.is_empty());
    let ranged = crate::expand::expand_goal(&GoalKind::EquipBest { style: "ranged".to_string() }, &profile);
    assert!(ranged.len() >= 2, "ranged keeps its set");
    // An old file: obtains then the goal's equip node.
    let mut amulet = Node::new("n1", NodeKind::ObtainItems { item: "amulet_of_power".to_string(), count: 1 }, Vec::new(), 1);
    amulet.status = NodeStatus::Ready;
    amulet.method = Some("market:buy".to_string());
    let mut body = Node::new("n2", NodeKind::ObtainItems { item: "iron_platebody".to_string(), count: 1 }, Vec::new(), 1);
    body.status = NodeStatus::Active;
    let mut sword = Node::new("n3", NodeKind::ObtainItems { item: "iron_scimitar".to_string(), count: 1 }, Vec::new(), 1);
    sword.status = NodeStatus::Done;
    let equip = Node::new("n4", NodeKind::EquipBest { style: "melee".to_string() }, vec!["n1".to_string(), "n2".to_string(), "n3".to_string()], 1);
    let mut life = life_with(vec![amulet, body, sword, equip]);
    life.active = Some("n2".to_string());
    let removed = withdraw_melee_set(&mut life);
    assert_eq!(removed, vec![("n1".to_string(), "amulet_of_power".to_string())], "open, not running");
    let equip = life.nodes.iter().find(|node| node.id == "n4").expect("equip stays");
    assert_eq!(equip.deps, vec!["n2".to_string(), "n3".to_string()]);
    assert!(withdraw_melee_set(&mut life).is_empty(), "idempotent");
}
