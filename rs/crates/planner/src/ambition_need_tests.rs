//! Decision D3 (user 2026-11-19, docs/plan-equilibre.txt step 6): an
//! ambition follows the NEED alone (the scarcity of what the kind
//! produces: open bids it serves, the prices its products fetch), never
//! the bot's levels; the level-led draw stays for `LEVEL_LED_ACCOUNTS`
//! and every market maker.

use crate::ambition_catalog::{kind_weights, DemandItem, KindDemand};
use crate::ambition_fleet::{draw_kind, level_led, FleetBot, LEVEL_LED_ACCOUNTS};

/// The audit's case: a strong woodcutter (70) with some crafting (30).
fn woodcutter_crafter(account: &str, seed: u64) -> FleetBot {
    FleetBot {
        account: account.to_string(),
        levels: vec![("woodcutting".to_string(), 70), ("crafting".to_string(), 30)],
        ironman: false,
        market_affinity: 0.5,
        seed,
        kind: None,
        market_maker: false,
        operator: false,
    }
}

/// One open bid that pays, as the market reads it.
fn bid_for(item: &str) -> Vec<KindDemand> {
    kind_weights(&[DemandItem { asked: false, item: item.to_string(), weight: 4000.0, unmet: 20, margin: Some(15.0) }], &[])
}

/// Kinds a demand list pulls (weight above the neutral 1).
fn pulled(demand: &[KindDemand]) -> Vec<String> {
    demand.iter().filter(|row| row.weight > 1.0 + 1e-9).map(|row| row.kind.clone()).collect()
}

/// Kinds drawn over 200 seeds (no gap, nothing excluded).
fn drawn(account: &str, demand: &[KindDemand]) -> std::collections::BTreeMap<String, usize> {
    let mut counts = std::collections::BTreeMap::new();
    for seed in 0..200u64 {
        let (kind, _) = draw_kind(&woodcutter_crafter(account, seed * 7_919 + 1), &[], demand, &[]);
        *counts.entry(kind).or_insert(0usize) += 1;
    }
    counts
}

/// Crafting demand only: the woodcutter 70 / crafter 30 draws the
/// crafter on every seed (its woodcutting lead counts for nothing).
#[test]
fn crafting_need_draws_the_crafter_whatever_the_levels() {
    let demand = bid_for("leather_gloves");
    assert_eq!(pulled(&demand), vec!["crafter".to_string()], "precondition: only the crafter serves it: {demand:?}");
    let counts = drawn("frank_lane", &demand);
    assert_eq!(counts.get("crafter").copied(), Some(200), "need alone: {counts:?}");
    let (_, why) = draw_kind(&woodcutter_crafter("frank_lane", 1), &[], &demand, &[]);
    assert_eq!(why, "need:leather_gloves");
}

/// Woodcutting demand only: the same bot draws the woodcutter, because
/// logs are wanted, not because it leads woodcutting.
#[test]
fn woodcutting_need_draws_the_woodcutter() {
    let demand = bid_for("willow_logs");
    assert_eq!(pulled(&demand), vec!["woodcutter".to_string()], "precondition: only the woodcutter serves it: {demand:?}");
    let counts = drawn("frank_lane", &demand);
    assert_eq!(counts.get("woodcutter").copied(), Some(200), "need alone: {counts:?}");
}

/// The exempt accounts keep today's level-led draw: with the same
/// crafting demand, Zezima's woodcutting lead still wins draws.
#[test]
fn exempt_accounts_keep_the_level_led_draw() {
    let demand = bid_for("leather_gloves");
    for account in ["Zezima", "N0valyfe", "Yogosun"] {
        let counts = drawn(account, &demand);
        let woodcutter = counts.get("woodcutter").copied().unwrap_or(0);
        assert!(woodcutter > 0 && counts.len() >= 3, "{account}: level-led draw, got {counts:?}");
        assert!(counts.values().all(|count| *count <= woodcutter), "{account}: the strong group stays the most likely: {counts:?}");
    }
}

/// Every market maker keeps the level-led draw too (by archetype or by
/// its `market_maker` ambition), whatever its account name.
#[test]
fn market_makers_keep_the_level_led_draw() {
    let demand = bid_for("leather_gloves");
    let mut maker = woodcutter_crafter("hill_eric", 1);
    assert!(!level_led(&maker), "a plain account draws by need");
    maker.market_maker = true;
    assert!(level_led(&maker));
    let mut by_kind = woodcutter_crafter("hill_eric", 1);
    by_kind.kind = Some(crate::market_maker::MARKET_MAKER_KIND.to_string());
    assert!(level_led(&by_kind));
    let mut woodcutters = 0;
    for seed in 0..200u64 {
        maker.seed = seed * 7_919 + 1;
        if draw_kind(&maker, &[], &demand, &[]).0 == "woodcutter" {
            woodcutters += 1;
        }
    }
    assert!(woodcutters > 50, "the woodcutting lead still weighs: {woodcutters}/200");
    assert!(LEVEL_LED_ACCOUNTS.iter().all(|name| level_led(&woodcutter_crafter(&name.to_lowercase(), 1))), "names match without case");
}

/// An open gap goes to the kind with need, never to the bot's strong
/// group; with no need anywhere the draw is even (no level weighs).
#[test]
fn gaps_and_empty_demand_ignore_the_levels() {
    let demand = bid_for("leather_gloves");
    let gaps = vec!["woodcutting".to_string(), "crafting".to_string()];
    for seed in 0..50u64 {
        let (kind, why) = draw_kind(&woodcutter_crafter("frank_lane", seed), &gaps, &demand, &[]);
        assert_eq!((kind.as_str(), why.as_str()), ("crafter", "coverage:crafting"));
    }
    let counts = drawn("frank_lane", &[]);
    let woodcutter = counts.get("woodcutter").copied().unwrap_or(0);
    assert!(woodcutter < 40 && counts.len() >= 10, "even draw without need: {counts:?}");
}

/// One bid as the market reads it (`asked`: a resting sell offer exists).
fn wanted(item: &str, weight: f64, asked: bool) -> DemandItem {
    DemandItem { item: item.to_string(), weight, unmet: 10, margin: Some(5.0), asked }
}

fn need_of(demand: &[KindDemand], kind: &str) -> f64 {
    demand.iter().find(|row| row.kind == kind).map(|row| row.need).unwrap_or(0.0)
}

/// Review 2026-11-19 (live: one amulet_of_power wish at 1.3 M a unit was
/// 72 % of all offer value): a 78 M bid among 30 ordinary bids never
/// gives its kind more than ~1.5/n of the offer share.
#[test]
fn one_outlier_bid_never_owns_the_need() {
    let ordinary = ["willow_logs", "oak_logs", "logs", "raw_shrimp", "raw_sardine", "raw_trout", "copper_ore", "tin_ore", "iron_ore", "coal"];
    let mut items = vec![wanted("amulet_of_power", 78_000_000.0 * 60.0, true)];
    for round in 0..3 {
        for (index, item) in ordinary.iter().enumerate() {
            items.push(wanted(item, 750.0 + 650.0 * (index + round) as f64, true));
        }
    }
    let demand = kind_weights(&items, &[]);
    assert!(ordinary.iter().all(|item| crate::ambition_catalog::serves("mage", item) == 0.0), "precondition: only the outlier calls for mages");
    let share = need_of(&demand, "mage");
    let bound = 1.5 / items.len() as f64;
    assert!(share > 0.0 && share <= bound, "mage need {share:.4} > {bound:.4}: {demand:?}");
    assert!(need_of(&demand, "woodcutter") > share, "nine log bids outweigh one outlier: {demand:?}");
}

/// D3 scarcity: an object wanted with NO resting sell offer weighs more
/// than one the asks already supply.
#[test]
fn an_object_without_asks_weighs_more() {
    let demand = kind_weights(&[wanted("willow_logs", 2000.0, false), wanted("raw_shrimp", 2000.0, true)], &[]);
    let (woodcutter, fisher) = (need_of(&demand, "woodcutter"), need_of(&demand, "fisher"));
    assert!(fisher > 0.0 && woodcutter >= 1.9 * fisher, "missing asks: woodcutter {woodcutter:.3} vs fisher {fisher:.3}");
}

/// Saturation: ten crafters lower the crafter need (÷ 1 + holders); the
/// level-led weight is untouched.
#[test]
fn held_kinds_saturate_their_need() {
    let demand = kind_weights(&[wanted("leather_gloves", 2000.0, true), wanted("willow_logs", 2000.0, true)], &[]);
    let held = vec!["crafter"; 10];
    let saturated = crate::ambition_fleet::saturate(&demand, &held);
    assert!((need_of(&saturated, "crafter") - need_of(&demand, "crafter") / 11.0).abs() < 1e-9, "{saturated:?}");
    assert!((need_of(&saturated, "woodcutter") - need_of(&demand, "woodcutter")).abs() < 1e-12);
    let weight = |rows: &[KindDemand]| rows.iter().find(|row| row.kind == "crafter").map(|row| row.weight);
    assert_eq!(weight(&saturated), weight(&demand), "level-led weight kept");
    let crafters = drawn("frank_lane", &saturated).get("crafter").copied().unwrap_or(0);
    assert!(crafters < 40, "ten crafters already: {crafters}/200");
}

/// The `need:<item>` label names the object carrying at least
/// `NEED_LABEL_FLOOR` of THE KIND's need, even among many bids of other
/// kinds; a kind spread over many equal bids names none.
#[test]
fn the_need_label_names_only_a_real_driver() {
    let mut items: Vec<DemandItem> = (0..60).map(|_| wanted("raw_shrimp", 1000.0, true)).collect();
    items.extend((0..30).map(|_| wanted("oak_logs", 1000.0, true)));
    items.push(wanted("willow_logs", 1000.0, false));
    items.extend((0..25).map(|_| wanted("leather_gloves", 1000.0, true)));
    let demand = kind_weights(&items, &[]);
    let woodcutter = demand.iter().find(|row| row.kind == "woodcutter").expect("woodcutter");
    assert_eq!(woodcutter.need_item.as_deref(), Some("willow_logs"), "6 % of the woodcutter's need: {woodcutter:?}");
    let crafter = demand.iter().find(|row| row.kind == "crafter").expect("crafter");
    assert!(crafter.need > 0.0 && crafter.need_item.is_none(), "25 equal bids, none ≥ 5 %: {crafter:?}");
}

/// User 2026-11-19 (« pourtant ça coûte rien de faire fondre aux fours »):
/// superheat makes a bar from the furnace's ore and coal PLUS a nature
/// rune, so the mage never answers the bar demand; the smith still does
/// in full, the miner at half (ores and coal), the runecrafter not at all
/// (the nature rune only feeds the dominated leg).
#[test]
fn a_dominated_leg_serves_nothing() {
    use crate::ambition_catalog::serves;
    for bar in ["iron_bar", "steel_bar", "mithril_bar"] {
        assert_eq!(serves("mage", bar), 0.0, "{bar}: superheat is dominated by the furnace");
        assert_eq!(serves("smith", bar), 1.0, "{bar}");
        assert_eq!(serves("miner", bar), 0.5, "{bar}");
        assert_eq!(serves("runecrafter", bar), 0.0, "{bar}: no nature rune needed");
    }
}

fn leg(method: &str, skill: &str, inputs: &[(&str, i64)]) -> crate::obtain_legs::ProductionLeg {
    crate::obtain_legs::ProductionLeg {
        method: method.to_string(),
        skill: Some((skill.to_string(), 1)),
        inputs: inputs.iter().map(|(item, count)| (item.to_string(), *count)).collect(),
        tools: Vec::new(),
        quest: None,
        sites: Vec::new(),
    }
}

/// Dominance on the inputs alone: a leg that only ADDS an input or a
/// quantity to another way is dominated; a different recipe, or the
/// same inputs, is not (both stay counted).
#[test]
fn only_a_superset_of_inputs_is_dominated() {
    use crate::ambition_catalog::dominated;
    let furnace = leg("smelt:iron_bar", "smithing", &[("iron_ore", 1)]);
    let superheat = leg("superheat:iron_bar", "magic", &[("iron_ore", 1), ("naturerune", 1)]);
    let double = leg("smelt2:iron_bar", "smithing", &[("iron_ore", 2)]);
    let other_recipe = leg("craft:iron_bar", "crafting", &[("scrap", 3)]);
    let twin = leg("smelt_twin:iron_bar", "smithing", &[("iron_ore", 1)]);
    let legs = vec![furnace.clone(), superheat.clone(), double.clone(), other_recipe.clone(), twin.clone()];
    assert!(dominated(&superheat, &legs), "an added rune");
    assert!(dominated(&double, &legs), "a larger quantity");
    assert!(!dominated(&furnace, &legs), "the cheapest way");
    assert!(!dominated(&twin, &legs), "equal inputs never dominate each other");
    assert!(!dominated(&other_recipe, &legs), "another recipe is an alternative");
    assert!(!dominated(&superheat, &[superheat.clone()]), "alone, it is the way");
    let mut ground = leg("ground:iron_bar", "none", &[]);
    ground.skill = None;
    assert!(!dominated(&furnace, &[furnace.clone(), ground]), "a ground spawn or a drop farm makes nothing");
}

/// Review 2026-11-19 (« besoin aplati »): once the guide's gain is bounded
/// by what can really be bought (`brain_d17::kind_profits`), it is read
/// ONCE, linearly (share of the fleet's summed gain), and it only TILTS
/// the need of a kind that buyers wait for or whose inputs pile up
/// (× (1 + gain share)): a kind nobody calls for never comes out level
/// with a demanded one, whatever it earns; the level-led `weight` of the
/// D3 exceptions is unchanged.
#[test]
fn the_gain_tilts_the_need_but_never_creates_it() {
    let profits = [("mage".to_string(), 268_593.0), ("smith".to_string(), 10_419.0), ("crafter".to_string(), 6_535.0), ("woodcutter".to_string(), 1270.0)];
    let gain_only = kind_weights(&[], &profits);
    assert!(gain_only.iter().all(|row| row.need == 0.0), "a gain alone is no need: {gain_only:?}");
    // Crafting is wanted (a bid), the mage only earns: the crafter's need
    // stands, the mage's stays 0.
    let demanded = kind_weights(&[wanted("leather_gloves", 2000.0, true)], &profits);
    assert!(need_of(&demanded, "crafter") > 0.0 && need_of(&demanded, "mage") == 0.0, "{demanded:?}");
    // Two kinds called for alike: the one that earns more leans ahead,
    // by its linear share of the gain (not flattened).
    let piles = [("logs".to_string(), 5000.0), ("bones".to_string(), 5000.0)];
    let both = crate::ambition_catalog::kind_weights_with(&[], &profits, &piles);
    let (firemaker, priest) = (need_of(&both, "firemaker"), need_of(&both, "priest"));
    assert!(firemaker > 0.0 && priest > 0.0, "no gain for either: their outlets alone: {both:?}");
    let earning = [("priest".to_string(), 9000.0), ("firemaker".to_string(), 1000.0)];
    let tilted = crate::ambition_catalog::kind_weights_with(&[], &earning, &piles);
    let ratio = (need_of(&tilted, "priest") / need_of(&tilted, "firemaker")) / (priest / firemaker);
    assert!((ratio - 1.9 / 1.1).abs() < 1e-9, "× (1 + 0.9) against × (1 + 0.1): {ratio}");
    let weight = |kind: &str| demanded.iter().find(|row| row.kind == kind).map(|row| row.weight).unwrap_or(0.0);
    assert!((weight("mage") - 2.0).abs() < 1e-9 && (weight("smith") - (1.0 + 10_419.0 / 268_593.0)).abs() < 1e-9, "level-led weight unchanged: {demanded:?}");
}

/// User 2026-11-19 (« la prière, l'allumage de feux, la magie, le tir à
/// distance font disparaître des objets, c'est important pour
/// l'économie »): what a kind's training consumes is read from the
/// catalogue's training methods (their prior flows), never a hand list.
#[test]
fn every_sink_kind_reads_what_its_training_consumes() {
    let consumed = |kind: &str| crate::ambition_catalog::consumed_by(kind);
    assert!(consumed("firemaker").iter().any(|item| item == "logs"), "{:?}", consumed("firemaker"));
    assert!(consumed("priest").iter().any(|item| item == "bones"), "{:?}", consumed("priest"));
    assert!(consumed("ranger").iter().any(|item| item.ends_with("_arrow")), "{:?}", consumed("ranger"));
    assert!(consumed("mage").iter().any(|item| item.ends_with("rune")), "{:?}", consumed("mage"));
    assert!(consumed("cook").iter().any(|item| item.starts_with("raw_")), "{:?}", consumed("cook"));
    assert!(consumed("runecrafter").iter().any(|item| item == "blankrune"), "{:?}", consumed("runecrafter"));
    assert!(consumed("smith").iter().any(|item| item.ends_with("_ore") || item.ends_with("_bar")), "{:?}", consumed("smith"));
    assert!(!consumed("crafter").is_empty(), "leather, gems, clay, wool…");
    assert!(!consumed("firemaker").iter().any(|item| item == "coins"));
}

/// The need counts the OUTLET a kind offers: a massive surplus of logs
/// on the asks calls for firemakers, a surplus of bones for priests,
/// even with no bid and a 36 M guide outlier elsewhere (the three
/// signals, bids, surplus and gain, each weigh 1 over the fleet: the gain
/// never crushes the shortfalls nor the surpluses).
#[test]
fn a_surplus_calls_for_the_kind_that_consumes_it() {
    let profits = [("mage".to_string(), 36_214_820.0), ("smith".to_string(), 4_596_917.0), ("crafter".to_string(), 978_728.0), ("woodcutter".to_string(), 1270.0)];
    let logs = crate::ambition_catalog::kind_weights_with(&[], &profits, &[("logs".to_string(), 120_000.0)]);
    let firemaker = need_of(&logs, "firemaker");
    assert!(firemaker > 0.0, "logs piling up: firemakers wanted: {logs:?}");
    for kind in ["mage", "smith", "crafter", "woodcutter"] {
        assert!(firemaker > need_of(&logs, kind), "the surplus is not crushed by the gain of {kind}: {logs:?}");
    }
    assert_eq!(need_of(&logs, "warrior"), 0.0, "a melee fight consumes nothing");
    let bones = crate::ambition_catalog::kind_weights_with(&[], &[], &[("bones".to_string(), 20_000.0)]);
    assert!(need_of(&bones, "priest") > 0.0, "{bones:?}");
    assert_eq!(need_of(&bones, "firemaker"), 0.0, "nobody else burns bones");
    let both = crate::ambition_catalog::kind_weights_with(&[], &[], &[("bones".to_string(), 20.0), ("logs".to_string(), 200_000.0)]);
    assert!(need_of(&both, "firemaker") > need_of(&both, "priest"), "more surplus, more need: {both:?}");
}

/// The user ruled (2026-11-19, « ça coûte rien de faire fondre aux
/// fours ») that superheat is dominated by the furnace: a dominated
/// method's inputs are no outlet of its kind (general rule, `dominated`).
/// Ore piling up calls for smiths, never for mages.
#[test]
fn ore_in_surplus_calls_for_smiths_never_mages() {
    let mage = crate::ambition_catalog::consumed_by("mage");
    assert!(!mage.iter().any(|item| item.ends_with("_ore") || item == "coal"), "superheat is dominated: {mage:?}");
    let ore = crate::ambition_catalog::kind_weights_with(&[], &[], &[("iron_ore".to_string(), 42_000.0)]);
    assert!(need_of(&ore, "smith") > 0.0, "{ore:?}");
    assert_eq!(need_of(&ore, "mage"), 0.0, "{ore:?}");
    assert!(crate::ambition_catalog::consumed_by("smith").iter().any(|item| item == "iron_ore"), "the furnace keeps its ore");
}

/// Relecture tour 2 (2026-11-19): 1 256 bones read 0.094 and 639 000
/// feathers 0.120, one compression of the RAW unit sum (plates and
/// feathers added together). The outlet counts each object in TRAINING
/// WINDOWS (units on the asks ÷ what the kind's fastest method burns per
/// 6000 ticks, the same prior flows as `consumed_by`), summed per kind,
/// then ln(1 + windows): 1 000 bones are under one window of burying,
/// 600 000 feathers hundreds of windows of fly fishing.
#[test]
fn the_outlet_counts_training_windows_not_raw_units() {
    let rate = |kind: &str, item: &str| {
        crate::ambition_catalog::consumption_of(kind).into_iter().find(|(known, _)| known == item).map(|(_, per_6000)| per_6000).expect("a consumption rate")
    };
    let (bones_rate, feather_rate) = (rate("priest", "bones"), rate("fisher", "feather"));
    assert!(bones_rate > 0.0 && feather_rate > 0.0);
    let piles = [("bones".to_string(), 1000.0), ("feather".to_string(), 600_000.0)];
    let demand = crate::ambition_catalog::kind_weights_with(&[], &[], &piles);
    let (priest, fisher) = (need_of(&demand, "priest"), need_of(&demand, "fisher"));
    let windows = (1.0 + 1000.0 / bones_rate).ln() / (1.0 + 600_000.0 / feather_rate).ln();
    assert!((priest / fisher - windows).abs() < 1e-9, "shares follow the windows: {} vs {windows}", priest / fisher);
    let raw = crate::ambition_catalog::compress(1000.0) / crate::ambition_catalog::compress(600_000.0);
    assert!(priest / fisher < raw * 0.5, "never the raw-unit ratio {raw}: {}", priest / fisher);
}

/// User 2026-11-20 (« abandonne les teneurs … trouve les rôles idéaux
/// pour ces 11 comptes »): a retired market maker takes the neediest
/// kind, no roll; held kinds saturate it, so the next one spreads.
#[test]
fn a_retired_maker_takes_the_neediest_trade_and_the_next_spreads() {
    let need = |kind: &str, need: f64| KindDemand { kind: kind.to_string(), weight: 1.0, need, top_item: None, need_item: None };
    let demand = vec![need("cook", 3.0), need("fisher", 2.5), need("crafter", 1.0)];
    let bot = woodcutter_crafter("retired_maker", 7);
    let first = crate::ambition_fleet::neediest_kind(&bot, &demand, &[]).map(|(kind, _)| kind);
    assert_eq!(first.as_deref(), Some("cook"));
    // One cook held: its need halves (saturate), the fisher comes next.
    let saturated = crate::ambition_fleet::saturate(&demand, &["cook"]);
    let second = crate::ambition_fleet::neediest_kind(&bot, &saturated, &[]).map(|(kind, _)| kind);
    assert_eq!(second.as_deref(), Some("fisher"));
}
