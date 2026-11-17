//! User lever 2026-11-17 (« entraînement marché plus fréquent »): a climb
//! whose training pick is a MARKET line buys its inputs at the tranche's
//! volume through the market (a need bid capped by the wallet), never
//! farms them; its tools keep their usual legs. An autonomous pick keeps
//! every leg (production first).

use crate::expand::{resolve_ready, ReadyCtx};
use crate::model::{Alternative, Node, NodeKind, NodeStatus};
use crate::replan::{buys_by_market, need_count_from, replan_node_from, ReplanCause};
use crate::training::TrainPick;

const METHOD: &str = "smith:steel_med_helm";

/// A smithing 40->45 climb resolved with the brain's pick (`buys`: a
/// market line), blocked on the hammer and the bars it lacks.
fn blocked_climb(buys: bool) -> Node {
    let mut ctx = ReadyCtx::blind(0.8);
    ctx.training = Some(TrainPick { method: METHOD.to_string(), batch: 0, missing: vec!["steel_bar".to_string()], buys_inputs: buys, cost_per_xp: 0.6, xp_per_tick: 0.7, profit_per_tick: -0.4 });
    let mut node = Node::new("n7", NodeKind::ReachLevel { skill: "smithing".to_string(), level: 45 }, Vec::new(), 45);
    node.alternatives.push(Alternative { method: crate::tranches::reach_method("smithing", 40), est_ticks: 0, est_coins: 0 });
    node.status = NodeStatus::Ready;
    resolve_ready(&mut node, &ctx);
    assert_eq!(node.method.as_deref(), Some(METHOD));
    assert_eq!(node.status, NodeStatus::Blocked { reason: "missing:hammer,steel_bar".to_string() });
    assert_eq!(node.buys_inputs, buys, "the pick's supply rides on the node");
    node
}

/// The bar obtain resolved for a bot of `coins` with the best ask
/// `ask` (`None`: no resting ask), every level unknown (not gated: it
/// could smelt).
fn resolved(sub: &Node, coins: i64, ask: Option<i64>) -> Node {
    let mut ctx = ReadyCtx::blind(0.8);
    ctx.coins = coins;
    ctx.price = 500;
    ctx.best_ask = ask;
    let mut node = sub.clone();
    node.status = NodeStatus::Ready;
    resolve_ready(&mut node, &ctx);
    node
}

/// Profile marché 0.8, smithing 40->45 in market mode (user test): the
/// bars come by a `market:buy` obtain sized at the tranche (every bar of
/// 40->45), never smelted; resting as a need bid when no ask covers, for
/// the whole tranche when the wallet pays it, for what it pays else; the
/// hammer keeps its usual legs.
#[test]
fn a_market_climb_bids_its_bars_at_the_tranche_volume() {
    let mut node = blocked_climb(true);
    let tranche = crate::sizing::tranche_need(METHOD, 40, 45, "steel_bar").expect("bars of the tranche");
    assert!(tranche > 28, "a tranche is more than one pack: {tranche}");
    assert_eq!(need_count_from(&node, "steel_bar", Some(40)), tranche);
    let fresh = replan_node_from(&mut node, &ReplanCause::Blocked, &["hammer".to_string(), "steel_bar".to_string()], &mut 7, Some(40));
    let bars = fresh.iter().find(|sub| matches!(&sub.kind, NodeKind::ObtainItems { item, .. } if item == "steel_bar")).expect("bar obtain");
    assert!(matches!(&bars.kind, NodeKind::ObtainItems { count, .. } if *count == tranche), "{:?}", bars.kind);
    let legs: Vec<&str> = bars.alternatives.iter().map(|alt| alt.method.as_str()).collect();
    assert_eq!(legs, vec!["market:buy"], "market only");
    let hammer = fresh.iter().find(|sub| matches!(&sub.kind, NodeKind::ObtainItems { item, .. } if item == "hammer")).expect("hammer obtain");
    assert!(hammer.alternatives.len() > 1 || hammer.alternatives.iter().any(|alt| alt.method != "market:buy"), "a tool keeps its legs: {:?}", hammer.alternatives);
    // No ask, a rich wallet: a resting need bid for the whole tranche.
    let rich = resolved(bars, 1_000_000, None);
    assert_eq!(rich.method.as_deref(), Some("market:buy"));
    let mut ctx = ReadyCtx::blind(0.8);
    ctx.coins = 1_000_000;
    ctx.price = 500;
    assert_eq!(crate::resolve_obtain::buy_quantity(&ctx, tranche), tranche, "the tranche's volume");
    // A covered ask: bought, never smelted beside it.
    assert_eq!(resolved(bars, 1_000_000, Some(450)).method.as_deref(), Some("market:buy"));
    // A thin wallet: the bid covers what it pays (the climb buys the rest
    // after its next block); none at all: the coins first.
    ctx.coins = 5_000;
    assert_eq!(crate::resolve_obtain::buy_quantity(&ctx, tranche), 10);
    let broke = resolved(bars, 0, None);
    assert!(matches!(&broke.status, NodeStatus::Blocked { reason } if reason.starts_with("missing:coins:")), "{:?}", broke.status);
}

/// Profil autarcique (autonomous pick): the bar obtain keeps every leg and
/// produces (smelting) when no ask covers it, as before.
#[test]
fn an_autonomous_climb_keeps_its_production_legs() {
    let mut node = blocked_climb(false);
    let fresh = replan_node_from(&mut node, &ReplanCause::Blocked, &["steel_bar".to_string()], &mut 7, Some(40));
    let bars = &fresh[0];
    assert!(bars.alternatives.iter().any(|alt| alt.method.starts_with("smelt:")), "{:?}", bars.alternatives);
    assert_eq!(resolved(bars, 1_000_000, None).method.as_deref(), Some("smelt:steel_bar"), "produced, a standing bid beside");
}

/// What a market climb buys: what the market trades, never a carried
/// tool of its method; an autonomous climb buys nothing this way.
#[test]
fn a_market_climb_buys_inputs_never_its_tools() {
    let mut node = Node::new("n7", NodeKind::ReachLevel { skill: "smithing".to_string(), level: 45 }, Vec::new(), 45);
    node.method = Some(METHOD.to_string());
    assert!(!buys_by_market(&node, "steel_bar"), "an autonomous pick");
    node.buys_inputs = true;
    assert!(buys_by_market(&node, "steel_bar"));
    assert!(!buys_by_market(&node, "hammer"), "the method's tool");
    node.method = Some("fire:yew_logs".to_string());
    assert!(buys_by_market(&node, "yew_logs") && !buys_by_market(&node, "tinderbox"));
}
