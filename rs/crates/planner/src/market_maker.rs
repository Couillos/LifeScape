//! Market-maker role in the life file (plan marché Q6, wave 3). A maker
//! account is born like any bot (`create-user --role market_maker`:
//! tutorial, then the playing phase, its `market_maker` ambition earning
//! coins with the producer money methods) until its CAPITAL TARGET (what
//! quoting a slice costs, one common target for every maker,
//! `lifescape_market::common_capital_target`); then it
//! switches for good to quoting: every other node abandoned, goal
//! `MakeMarket`, one `market:make` window after another at its post by
//! the Varrock fountain. The quotes themselves live daemon-side
//! (`orchestrator::market_maker`); this module only carries the role and
//! its one-way switch. Status changes go through `lifecycle`.
//!
//! Also here: the load-time migration of the removed merchant role (D18,
//! removed in wave 3): its legs and its `merchant` ambition leave the
//! file, the bot goes on as a producer.

use lifescape_json::{object, str_value, Value};

use crate::model::{Archetype, GoalKind, Life};

/// The maker's only method: stand at its post, the daemon quotes.
pub const MAKE_METHOD: &str = "market:make";
/// Market makers in use (user 2026-11-20: « abandonne les teneurs, garde
/// le mécanisme dans le code mais ne l'utilise plus »): false, no maker
/// is created, a maker life is retired to the neediest trade
/// (`orchestrator::market_maker::retire`). The mechanism stays intact.
pub const MAKERS_ACTIVE: bool = false;
/// `last_error` of the nodes a maker's retirement abandons.
pub const MAKER_RETIRED_CAUSE: &str = "role: market maker retired (makers unused)";
/// The maker's ambition kind (operator-only, never drawn by the fleet):
/// a coin ladder for the playing phase.
pub const MARKET_MAKER_KIND: &str = "market_maker";
/// `last_error` of the nodes a role switch abandons.
pub const MAKER_ROLE_CAUSE: &str = "role: market maker (capital target reached)";
/// Maker posts: the static handover ring around the Varrock fountain
/// (`orchestrator::market_service::RING`, the tiles deals meet on), one
/// per slice in turn: a maker stands where it delivers.
pub const MAKER_POSTS: [(i64, i64, i64); 8] =
    [(3211, 3427, 0), (3212, 3427, 0), (3214, 3427, 0), (3215, 3427, 0), (3211, 3431, 0), (3212, 3431, 0), (3214, 3431, 0), (3215, 3431, 0)];

/// The post of a maker's slice.
pub fn maker_post(role: &MakerRole) -> (i64, i64, i64) {
    MAKER_POSTS[role.slice as usize % MAKER_POSTS.len()]
}

/// The removed merchant method (wave 3): only read here, to strip it
/// from files written before.
const RETIRED_MERCHANT_METHOD: &str = "market:flip";
/// The removed merchant ambition kind.
const RETIRED_MERCHANT_KIND: &str = "merchant";

/// Playing first, then quoting for good.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MakerPhase {
    Earning,
    Quoting,
}

/// One maker's slice of the universe and its capital (life profile).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MakerRole {
    /// Slice index in `0..slices` (`lifescape_market::partition`).
    pub slice: u32,
    pub slices: u32,
    /// Coins that switch the role: the common target of every maker
    /// (`lifescape_market::common_capital_target`).
    pub capital_target: i64,
    pub phase: MakerPhase,
    /// Coins held at the switch (P&L baseline) and its unix second.
    pub capital_at_switch: i64,
    pub switched_at: i64,
}

impl MakerRole {
    pub fn new(slice: u32, slices: u32, capital_target: i64) -> MakerRole {
        MakerRole { slice, slices, capital_target, phase: MakerPhase::Earning, capital_at_switch: 0, switched_at: 0 }
    }
}

/// The life's maker role, if it is one.
pub fn maker_role(life: &Life) -> Option<MakerRole> {
    match life.profile.archetype {
        Archetype::MarketMaker(role) => Some(role),
        Archetype::Producer => None,
    }
}

/// True once the maker quotes (no other activity).
pub fn quoting(life: &Life) -> bool {
    maker_role(life).is_some_and(|role| role.phase == MakerPhase::Quoting)
}

/// True when a playing maker holds its capital target.
pub fn switch_due(role: &MakerRole, coins: i64) -> bool {
    role.phase == MakerPhase::Earning && coins >= role.capital_target
}

/// The one-way switch: every open node abandoned (`lifecycle`), the role
/// quoting with its capital baseline, goal `MakeMarket` appended.
/// Returns the abandoned node ids; nothing when the life is no playing
/// maker.
pub fn become_quoting(life: &mut Life, coins: i64, now_secs: i64) -> Vec<String> {
    let Archetype::MarketMaker(mut role) = life.profile.archetype else { return Vec::new() };
    if role.phase == MakerPhase::Quoting {
        return Vec::new();
    }
    let abandoned = crate::lifecycle::abandon_for_role(life, MAKER_ROLE_CAUSE);
    role.phase = MakerPhase::Quoting;
    role.capital_at_switch = coins;
    role.switched_at = now_secs;
    life.profile.archetype = Archetype::MarketMaker(role);
    let profile = life.profile.clone();
    crate::goals::append_goal(life, GoalKind::MakeMarket, &profile, &|_| 1, &|_| 0);
    life.goal_origin = None;
    crate::lifecycle::normalize(life);
    abandoned
}

/// `profile.maker` of the life file (`null` for a producer).
pub fn role_to_value(archetype: &Archetype) -> Value {
    let Archetype::MarketMaker(role) = archetype else { return Value::Null };
    object(vec![
        ("slice", Value::Int(i64::from(role.slice))),
        ("slices", Value::Int(i64::from(role.slices))),
        ("capital_target", Value::Int(role.capital_target)),
        ("phase", str_value(if role.phase == MakerPhase::Quoting { "quoting" } else { "earning" })),
        ("capital_at_switch", Value::Int(role.capital_at_switch)),
        ("switched_at", Value::Int(role.switched_at)),
    ])
}

/// Archetype of a file's `profile`: `market_maker` with its role, else a
/// producer (the removed `merchant` included: wave-3 migration).
pub fn archetype_from_value(profile: &Value) -> Archetype {
    let maker = profile.get("maker");
    if profile.get("archetype").as_str() != Some("market_maker") || maker.is_null() {
        return Archetype::Producer;
    }
    let number = |key: &str| maker.get(key).as_i64().unwrap_or(0);
    Archetype::MarketMaker(MakerRole {
        slice: u32::try_from(number("slice").max(0)).unwrap_or(0),
        slices: u32::try_from(number("slices").max(1)).unwrap_or(1),
        capital_target: number("capital_target").max(1),
        phase: if maker.get("phase").as_str() == Some("quoting") { MakerPhase::Quoting } else { MakerPhase::Earning },
        capital_at_switch: number("capital_at_switch"),
        switched_at: number("switched_at"),
    })
}

/// Wave-3 migration at load: the removed merchant leg leaves every node
/// (a node that ran it resolves afresh, its money node ranks the
/// producer methods), the merchant ambition leaves the life (the brain
/// assigns a fleet one at the next pass). Idempotent.
pub fn migrate_merchant(life: &mut Life) {
    for node in &mut life.nodes {
        node.alternatives.retain(|alt| alt.method != RETIRED_MERCHANT_METHOD);
        if node.method.as_deref() == Some(RETIRED_MERCHANT_METHOD) {
            node.method = None;
        }
    }
    if life.ambition.as_ref().is_some_and(|ambition| ambition.kind == RETIRED_MERCHANT_KIND) {
        life.ambition = None;
        life.goal_origin = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{life_from_value, life_to_value, NodeKind, NodeStatus, Profile};

    /// A merchant life written before wave 3: archetype, goal, an active
    /// flip node, the merchant ambition.
    const MERCHANT_FILE: &str = r#"{"schema":1,"account":"ryan_park","created_at":1,"seed":9,
        "profile":{"ironman":false,"market_affinity":0.5,"price_patience":0.5,"archetype":"merchant"},
        "goal":{"kind":"get_rich","merchant":true},
        "nodes":[{"id":"n1","kind":"make_money","params":{},"deps":[],"status":"active",
          "method":"market:flip","alternatives":[{"method":"market:flip","est_ticks":6000,"est_coins":0}],
          "progress":{"current":0,"target":0},"spent_ticks":100},
          {"id":"n2","kind":"make_money","params":{},"deps":[],"status":"ready",
          "alternatives":[{"method":"gather:fishing/raw_salmon","est_ticks":0,"est_coins":0},{"method":"market:flip","est_ticks":0,"est_coins":0}],
          "progress":{"current":0,"target":0},"spent_ticks":0}],
        "engagement":null,"active":"n1","history":[],
        "ambition":{"kind":"merchant","label":"marchand","groups":["merchant"],"why":"coverage:merchant","milestones":[],"drawn_at_tick":0,"drawn_at":0,"rung":0,"revisions":[]},
        "goal_origin":"m1"}"#;

    #[test]
    fn a_merchant_life_loads_as_a_producer_without_flip() {
        let value = lifescape_json::parse(MERCHANT_FILE).expect("json");
        let life = life_from_value(&value).expect("loads");
        assert_eq!(life.profile.archetype, Archetype::Producer);
        assert_eq!(life.goal, GoalKind::GetRich);
        assert_eq!(life.nodes[0].method, None, "the flip leg is gone: the node resolves afresh");
        assert!(life.nodes.iter().all(|node| node.alternatives.iter().all(|alt| alt.method != RETIRED_MERCHANT_METHOD)));
        assert_eq!(life.nodes[1].alternatives.len(), 1, "the producer leg stays");
        assert!(life.ambition.is_none() && life.goal_origin.is_none(), "the merchant ambition leaves, the brain assigns one");
        let written = lifescape_json::stringify(&life_to_value(&life));
        assert!(!written.contains(RETIRED_MERCHANT_METHOD) && !written.contains("\"merchant\""), "{written}");
        // Idempotent: the written file loads to the same life.
        let again = life_from_value(&lifescape_json::parse(&written).expect("json")).expect("loads");
        assert_eq!(again, life);
    }

    fn maker_life(phase: MakerPhase) -> Life {
        let mut role = MakerRole::new(3, 11, 99_000);
        role.phase = phase;
        let profile = Profile::new(false, 0.5, 0.5, 7, Archetype::MarketMaker(role));
        let goal = GoalKind::GetRich;
        let mut nodes = crate::expand_goal(&GoalKind::MaxSkills { skills: vec!["fishing".to_string()], level: 20 }, &profile);
        nodes[0].status = NodeStatus::Done;
        Life {
            schema: crate::model::LIFE_SCHEMA,
            account: "maker_one".to_string(),
            created_at: 0,
            seed: 7,
            profile,
            goal,
            nodes,
            engagement: None,
            active: None,
            history: Vec::new(),
            ambition: None,
            goal_origin: None,
            memory: Default::default(),
        }
    }

    #[test]
    fn the_role_round_trips_through_the_file() {
        for phase in [MakerPhase::Earning, MakerPhase::Quoting] {
            let life = maker_life(phase);
            let again = life_from_value(&life_to_value(&life)).expect("loads");
            assert_eq!(again.profile.archetype, life.profile.archetype);
            assert_eq!(maker_role(&again).map(|role| (role.slice, role.slices, role.capital_target)), Some((3, 11, 99_000)));
        }
    }

    #[test]
    fn the_capital_target_switches_the_role_for_good() {
        let mut life = maker_life(MakerPhase::Earning);
        let role = maker_role(&life).expect("maker");
        assert!(!switch_due(&role, 98_999), "under the target: keep playing");
        assert!(switch_due(&role, 99_000));
        let open_before: Vec<String> = life.nodes.iter().filter(|node| node.status != NodeStatus::Done).map(|node| node.id.clone()).collect();
        let abandoned = become_quoting(&mut life, 99_500, 1_700_000_000);
        assert_eq!(abandoned, open_before, "every open node leaves");
        assert!(quoting(&life));
        let role = maker_role(&life).expect("maker");
        assert_eq!((role.capital_at_switch, role.switched_at), (99_500, 1_700_000_000));
        assert_eq!(life.goal, GoalKind::MakeMarket);
        let post = life.nodes.iter().find(|node| node.status != NodeStatus::Abandoned && node.status != NodeStatus::Done).expect("the post node");
        assert!(matches!(post.kind, NodeKind::MakeMoney));
        assert_eq!(post.alternatives.iter().map(|alt| alt.method.as_str()).collect::<Vec<_>>(), vec![MAKE_METHOD]);
        assert_eq!(life.nodes[0].status, NodeStatus::Done, "done work stays done");
        assert!(!switch_due(&role, 1_000_000), "one way: never switches again");
        assert!(become_quoting(&mut life, 1, 1).is_empty());
        let producer = Life { profile: Profile::new(false, 0.5, 0.5, 7, Archetype::Producer), ..maker_life(MakerPhase::Earning) };
        let mut untouched = producer.clone();
        assert!(become_quoting(&mut untouched, 1_000_000, 1).is_empty());
        assert_eq!(untouched, producer, "a producer never switches");
    }
}
