//! Products chosen by the fleet's supply (user 2026-11-20): the piles
//! (feathers, raw chicken and beef, cow hides, essence) pick the product
//! of every consuming skill's quota; a talisman nobody can get is never
//! aimed; the persisted milestones follow the rule.

use super::{pick_supplied, product_workable, revise_supplied, sink_pressure};
use crate::ambition::{new_ambition, new_ambition_supplied, refresh_supplied, Ambition, BotNow, Milestone, MilestoneStatus, Stamp, Target};
use crate::ambition_catalog::DemandItem;

const STAMP: Stamp = Stamp { tick: 0, secs: 0 };

/// Fleet stock beyond the bids, live 2026-11-20 (`holdings_hourly`).
fn live_piles() -> Vec<(String, f64)> {
    [
        ("feather", 1_048_638.0),
        ("raw_chicken", 192_216.0),
        ("raw_beef", 192_881.0),
        ("cow_hide", 161_258.0),
        ("leather", 23_457.0),
        ("cooked_chicken", 10_370.0),
        ("cooked_meat", 2_212.0),
        ("blankrune", 126_052.0),
        ("air_talisman", 728.0),
        ("earth_talisman", 5_779.0),
        ("body_talisman", 2_767.0),
        ("airrune", 22_852.0),
        ("mindrune", 77_535.0),
        ("waterrune", 6_781.0),
        ("earthrune", 66_100.0),
        ("firerune", 108_187.0),
        ("bodyrune", 148_877.0),
    ]
    .into_iter()
    .map(|(item, units)| (item.to_string(), units))
    .collect()
}

fn produced(ambition: &Ambition) -> Vec<String> {
    ambition
        .milestones
        .iter()
        .filter(|stone| stone.status != MilestoneStatus::Done)
        .filter_map(|stone| match &stone.target {
            Target::Produce { item, .. } => Some(item.clone()),
            _ => None,
        })
        .collect()
}

fn drawn(kind: &str, skill: &'static str, level: u8, surplus: &[(String, f64)], held: &dyn Fn(&str) -> i64) -> Ambition {
    let level_of = move |name: &str| if name == skill { level } else { 1 };
    let now = BotNow { level_of: &level_of, coins: 0, held };
    new_ambition_supplied(kind, "random", &now, 7, (&[], surplus), STAMP).expect("an ambition")
}

/// « raw chicken, raw beef s'accumulent … même si c'est pour train le
/// cooking »: a cook at 29 (rungs 20 and 30 offer pies, pike, trout,
/// tuna, salmon) cooks the piled meat.
#[test]
fn a_cook_cooks_the_raw_meat_the_fleet_piles_up() {
    let cook = drawn("cook", "cooking", 29, &live_piles(), &|_| 0);
    let items = produced(&cook);
    assert!(!items.is_empty() && items.iter().all(|item| item == "cooked_chicken" || item == "cooked_meat"), "{items:?}");
    assert!(sink_pressure("cooked_meat", "cooking", &live_piles()) > 190_000.0 - 1.0, "beef piled, cooked meat scarce");
}

/// « ou pêcher des truites car ça rapporte bcp d'XP »: a fisher at 45
/// (rungs 40 and 50 offer lobster, tuna, swordfish) fly-fishes the
/// feathers.
#[test]
fn a_fisher_fly_fishes_the_piled_feathers() {
    let fisher = drawn("fisher", "fishing", 45, &live_piles(), &|_| 0);
    let items = produced(&fisher);
    assert!(!items.is_empty() && items.iter().all(|item| item == "raw_trout" || item == "raw_salmon"), "{items:?}");
}

/// Cow hides are tanned (a skill-less leg) into the leather a crafter
/// sews: the hides call for leather goods, never jewellery.
#[test]
fn a_crafter_sews_the_hides_through_tanning() {
    let hides: Vec<(String, f64)> = vec![("cow_hide".to_string(), 161_258.0)];
    let crafter = drawn("crafter", "crafting", 25, &hides, &|_| 0);
    let items = produced(&crafter);
    let leather = |item: &String| crate::obtain_legs::production_legs(item).iter().any(|leg| leg.inputs.iter().any(|(input, _)| input == "leather" || input == "hard_leather"));
    assert!(!items.is_empty() && items.iter().all(leather), "{items:?}");
}

/// « pourquoi personne ne fait de runecrafting »: water and mind
/// talismans are held by nobody, sold by no shop, made by no skill (rare
/// drops): never aimed. The essence and the held talismans pick the
/// runes the fleet lacks most (air, earth), never body runes it piles.
#[test]
fn runes_need_a_talisman_someone_can_get() {
    let piles = live_piles();
    let runecrafter = drawn("runecrafter", "runecraft", 12, &piles, &|_| 0);
    let items = produced(&runecrafter);
    assert!(!items.is_empty(), "{:?}", runecrafter.milestones);
    assert!(items.iter().all(|item| item == "airrune" || item == "earthrune"), "{items:?}");
    assert!(!product_workable("waterrune", "runecraft", &|_| 0, &piles));
    assert!(product_workable("waterrune", "runecraft", &|item| i64::from(item == "water_talisman"), &piles), "a held talisman works");
    assert!(product_workable("bodyrune", "runecraft", &|_| 0, &piles), "the fleet holds body talismans");
    // Nothing anybody can get: no rune quota at all, only the levels.
    let bare = vec![("logs".to_string(), 5.0)];
    let stuck = drawn("runecrafter", "runecraft", 12, &bare, &|_| 0);
    assert!(produced(&stuck).is_empty(), "{:?}", stuck.milestones);
}

/// No surplus data (market down): the ladder picks as before.
#[test]
fn without_surplus_data_the_ladder_picks_as_before() {
    let level_of = |name: &str| if name == "cooking" { 29 } else { 1 };
    let now = BotNow { level_of: &level_of, coins: 0, held: &|_| 0 };
    let before = new_ambition("cook", "random", &now, 7, &[], STAMP).expect("an ambition");
    let supplied = new_ambition_supplied("cook", "random", &now, 7, (&[], &[]), STAMP).expect("an ambition");
    assert_eq!(before, supplied);
    let pick = pick_supplied(&["trout", "pike"], 40, "cooking", 20, 3, &[], &[], &|_| 0).expect("a pick");
    assert!(!pick.sink && pick.count == 40);
}

/// A paying bid for water runes makes no demand milestone when nobody
/// can get the talisman; air runes still do.
#[test]
fn a_bid_needing_an_unobtainable_talisman_makes_no_milestone() {
    let bid = |item: &str| DemandItem { item: item.to_string(), weight: 50.0, unmet: 100, margin: Some(5.0), asked: false };
    let level_of = |name: &str| if name == "runecraft" { 12 } else { 1 };
    let now = BotNow { level_of: &level_of, coins: 0, held: &|_| 0 };
    let piles = live_piles();
    let mut water = new_ambition("runecrafter", "random", &now, 7, &[], STAMP).expect("an ambition");
    refresh_supplied(&mut water, &now, 7, (&[bid("waterrune")], &piles));
    assert!(!water.milestones.iter().any(|stone| stone.origin == "demand:waterrune"), "{:?}", water.milestones);
    let mut air = new_ambition("runecrafter", "random", &now, 7, &[], STAMP).expect("an ambition");
    refresh_supplied(&mut air, &now, 7, (&[bid("airrune")], &piles));
    assert!(air.milestones.iter().any(|stone| stone.origin == "demand:airrune"), "{:?}", air.milestones);
}

fn stone(id: &str, item: &str, count: i64, skill: &str, origin: &str, made: i64) -> Milestone {
    Milestone {
        id: id.to_string(),
        label: String::new(),
        target: Target::Produce { item: item.to_string(), count, skill: skill.to_string() },
        status: MilestoneStatus::Pending,
        progress: (made, count),
        blocked_count: 0,
        origin: origin.to_string(),
    }
}

/// The rule applies to the persisted milestones (user rule): live, 12
/// runecrafter quotas aimed water runes. The bid answer leaves, the
/// ladder quotas are re-picked (new ids), once.
#[test]
fn persisted_water_rune_quotas_move_on() {
    let level_of = |name: &str| if name == "runecraft" { 56 } else { 1 };
    let now = BotNow { level_of: &level_of, coins: 0, held: &|_| 0 };
    let mut ambition = new_ambition("runecrafter", "random", &now, 7, &[], STAMP).expect("an ambition");
    ambition.milestones = vec![
        stone("m1", "waterrune", 100, "runecraft", "demand:waterrune", 0),
        stone("m2", "waterrune", 200, "runecraft", "ladder", 0),
        Milestone { target: Target::Level { skill: "runecraft".to_string(), level: 60 }, ..stone("m3", "", 0, "", "ladder", 0) },
        stone("m4", "waterrune", 200, "runecraft", "ladder", 0),
    ];
    let piles = live_piles();
    let gone = revise_supplied(&mut ambition, &now, 7, &piles, Some("m1"));
    assert_eq!(gone, vec!["m1".to_string(), "m2".to_string(), "m4".to_string()]);
    let items = produced(&ambition);
    assert_eq!(items.len(), 2, "{:?}", ambition.milestones);
    assert!(items.iter().all(|item| item == "airrune" || item == "earthrune"), "{items:?}");
    assert!(ambition.milestones.iter().all(|stone| !["m1", "m2", "m4"].contains(&stone.id.as_str())), "fresh ids: {:?}", ambition.milestones);
    assert!(revise_supplied(&mut ambition, &now, 7, &piles, None).is_empty(), "stable");
}

/// An idle ladder quota eating no pile moves to a sink; the running one
/// and one already started keep their product.
#[test]
fn idle_quotas_move_to_the_piles_running_ones_stay() {
    let level_of = |name: &str| if name == "cooking" { 55 } else { 1 };
    let now = BotNow { level_of: &level_of, coins: 0, held: &|_| 0 };
    let mut cook = new_ambition("cook", "random", &now, 7, &[], STAMP).expect("an ambition");
    cook.milestones = vec![
        stone("m1", "swordfish", 200, "cooking", "ladder", 0),
        stone("m2", "swordfish", 200, "cooking", "ladder", 0),
        stone("m3", "lobster", 200, "cooking", "ladder", 40),
        stone("m4", "trout", 50, "cooking", "demand:trout", 0),
    ];
    let piles = live_piles();
    let gone = revise_supplied(&mut cook, &now, 7, &piles, Some("m2"));
    assert_eq!(gone, vec!["m1".to_string()]);
    let items = produced(&cook);
    assert!(items[0] == "cooked_chicken" || items[0] == "cooked_meat", "{items:?}");
    assert_eq!(&items[1..], &["swordfish".to_string(), "lobster".to_string(), "trout".to_string()], "running, started and bid answers stay");
    assert!(revise_supplied(&mut cook, &now, 7, &[], None).is_empty(), "no data, no decision");
}

/// The fleet held 4 fly fishing rods: a trout quota's missing rod files
/// its purchase at the fishing shop (the obtain of the block), so a
/// fisher sent to the feathers buys its rod.
#[test]
fn a_trout_quota_buys_its_fly_fishing_rod() {
    use crate::model::{Archetype, GoalKind, Life, NodeKind, Profile};
    let profile = Profile::new(false, 0.5, 0.5, 7, Archetype::Producer);
    let goal = GoalKind::Produce { item: "raw_trout".to_string(), count: 85 };
    let mut life = Life {
        schema: 1,
        account: "t00".to_string(),
        created_at: 0,
        seed: 7,
        profile: profile.clone(),
        nodes: crate::expand::expand_goal(&goal, &profile),
        goal,
        engagement: None,
        active: None,
        history: Vec::new(),
        ambition: None,
        goal_origin: None,
        memory: Default::default(),
    };
    crate::lifecycle::record_block(&mut life, 0, &["fly_fishing_rod".to_string()]);
    let rod = life.nodes.iter().find(|node| matches!(&node.kind, NodeKind::ObtainItems { item, .. } if item == "fly_fishing_rod")).expect("the rod is planned");
    let legs: Vec<&str> = rod.alternatives.iter().map(|alt| alt.method.as_str()).collect();
    assert!(legs.contains(&"shop:buy"), "bought at the fishing shop: {legs:?}");
}

/// Live 2026-11-20: "produce waterrune" answers bought a water talisman
/// no bot holds for 6.4 M coins. A lot whose every leg needs a tool
/// nobody can come by is not workable; earth runes (talismans held) are.
#[test]
fn a_lot_needing_a_talisman_nobody_holds_is_not_workable() {
    let surplus = vec![("earth_talisman".to_string(), 5_779.0), ("rune_essence".to_string(), 124_000.0)];
    assert!(!super::lot_workable("waterrune", &surplus));
    assert!(super::lot_workable("earthrune", &surplus));
    assert!(super::lot_workable("waterrune", &[]), "no surplus data: no judgement");
}
