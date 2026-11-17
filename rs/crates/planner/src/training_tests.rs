use std::collections::HashMap;

use super::{best_pick, default_smith_bar, default_smith_product, priced_at_resolve, rank_magic, rank_smithing, smith_products, smithing_cuts, Acquired, AlchSupply, TrainPrices};

/// A bot's numbers for one test: held units, sale values, acquisition
/// prices (bought, `made_by` when the bot makes it), product values.
#[derive(Default)]
struct Book {
    held: HashMap<&'static str, i64>,
    sale: HashMap<&'static str, f64>,
    buy: HashMap<&'static str, (f64, Option<&'static str>)>,
    value: HashMap<&'static str, f64>,
}

fn with_prices<R>(book: &Book, coin_rate: f64, coins: i64, run: impl FnOnce(&TrainPrices) -> R) -> R {
    let held = |item: &str| book.held.get(item).copied().unwrap_or(0);
    let sale = |item: &str| book.sale.get(item).copied().unwrap_or(0.0);
    let acquire = |item: &str, _units: i64| book.buy.get(item).map(|(coins_each, made_by)| Acquired { coins_each: *coins_each, made_by: made_by.map(str::to_string) });
    let value = |item: &str, _units: i64| book.value.get(item).copied().unwrap_or(0.0);
    let prices = TrainPrices { coin_rate, coins, held: &held, sale_value: &sale, acquire: &acquire, product_value: &value, rate: &crate::work_rate::prior_rate };
    run(&prices)
}

/// The anvil table comes from the dump: the four F2P columns of every
/// metal, bars per press from the recipes, no wire, nails or members cell.
#[test]
fn anvil_products_come_from_the_dump() {
    let products = smith_products();
    let find = |name: &str| products.iter().find(|row| row.product == name).unwrap_or_else(|| panic!("{name} is an anvil product"));
    assert_eq!((find("bronze_dagger").bars, find("bronze_dagger").level), (1, 1));
    assert_eq!((find("bronze_scimitar").bars, find("bronze_scimitar").level), (2, 5));
    assert_eq!((find("bronze_platebody").bars, find("bronze_platebody").level), (5, 18));
    assert_eq!((find("steel_platebody").bars, find("steel_platebody").level), (5, 48));
    assert_eq!(find("rune_platebody").bar, "runite_bar");
    assert!(products.iter().all(|row| !row.product.contains("nails") && !row.product.contains("wire")));
    assert!(products.len() >= 90, "six metals × the four columns, got {}", products.len());
}

/// Static tranche product: best metal open, most bars per press, first
/// unlocked on a tie (bronze daggers, scimitars at 5, warhammers at 9,
/// iron daggers at 15, steel platebodies at 48).
#[test]
fn static_product_follows_metal_and_bars() {
    assert_eq!(default_smith_product(1), "bronze_dagger");
    assert_eq!(default_smith_product(5), "bronze_scimitar");
    assert_eq!(default_smith_product(9), "bronze_warhammer");
    assert_eq!(default_smith_product(15), "iron_dagger");
    assert_eq!(default_smith_product(18), "iron_dagger");
    assert_eq!(default_smith_product(20), "iron_scimitar");
    assert_eq!(default_smith_product(48), "steel_platebody");
    assert_eq!(default_smith_bar(14), "bronze_bar");
    assert_eq!(default_smith_bar(85), "runite_bar");
    for level in 1..=99u8 {
        let product = default_smith_product(level);
        let row = smith_products().iter().find(|row| row.product == product).expect("static product is an anvil product");
        assert!(row.level <= level, "{product} opens at {} > {level}", row.level);
    }
}

/// Cuts: each better press of a metal, each metal, each bar to smelt
/// (gold 40), so every tranche re-prices where the choice can change.
#[test]
fn smithing_cuts_mark_every_unlock_that_matters() {
    let cuts = smithing_cuts();
    for level in [5, 9, 15, 18, 20, 24, 30, 33, 40, 48, 50, 68, 85] {
        assert!(cuts.contains(&level), "cut at {level}: {cuts:?}");
    }
    assert!(cuts.iter().all(|cut| *cut > 1 && *cut < 99));
    for level in 2..99u8 {
        if default_smith_product(level) != default_smith_product(level - 1) {
            assert!(cuts.contains(&level), "the static product changes at {level}");
        }
    }
    assert_eq!(crate::tranches::reach_method("smithing", 1), "smith:bronze_dagger");
    assert_eq!(crate::tranches::reach_method("smithing", 48), "smith:steel_platebody");
}

#[test]
fn priced_tranches_are_smithing_and_magic_from_confuse() {
    assert!(priced_at_resolve("smithing", 1));
    assert!(!priced_at_resolve("magic", 2), "wind strike alone below confuse");
    assert!(priced_at_resolve("magic", 3));
    assert!(priced_at_resolve("magic", 21));
    assert!(!priced_at_resolve("fishing", 50));
}

/// Market-free bot at smithing 18: bars cost what making them costs. A
/// bronze platebody (5 bars a press) beats iron daggers when bronze bars
/// are cheaper per XP; held bars cost what they sell for (nothing without
/// buyers), so a stock of iron bars turns the pick to iron.
#[test]
fn smithing_pick_weighs_bars_per_xp_and_presses() {
    let mut book = Book::default();
    book.buy.insert("bronze_bar", (10.0, None));
    book.buy.insert("iron_bar", (40.0, None));
    let pick = with_prices(&book, 0.5, 1000, |prices| rank_smithing(18, 20, prices));
    assert_eq!(pick[0].method, "smith:bronze_platebody", "{pick:?}");
    assert_eq!(pick[0].missing, vec!["bronze_bar".to_string()], "no bar held: obtain first");
    book.held.insert("iron_bar", 200);
    let pick = with_prices(&book, 0.5, 1000, |prices| rank_smithing(18, 20, prices));
    assert!(pick[0].method.starts_with("smith:iron_"), "{:?}", pick[0]);
    assert!(pick[0].missing.is_empty());
}

/// The product's value counts: scimitar bids (live 2026-11-14, 2961 down
/// to 700) pull a smith off the platebody; bars it smelts itself add the
/// furnace XP.
#[test]
fn product_value_and_own_smelting_count() {
    let mut book = Book::default();
    book.buy.insert("bronze_bar", (10.0, Some("smelt:bronze_bar")));
    book.value.insert("bronze_scimitar", 900.0);
    let ranked = with_prices(&book, 0.5, 1000, |prices| rank_smithing(18, 20, prices));
    assert_eq!(ranked[0].method, "smith:bronze_scimitar");
    assert!(ranked[0].cost_per_xp < 0.0, "a paying product makes the XP free");
    book.value.clear();
    let made = with_prices(&book, 0.5, 1000, |prices| rank_smithing(18, 20, prices));
    let platebody = made.iter().find(|pick| pick.method == "smith:bronze_platebody").expect("platebody ranked");
    book.buy.insert("bronze_bar", (10.0, None));
    let bought = with_prices(&book, 0.5, 1000, |prices| rank_smithing(18, 20, prices));
    let bought_platebody = bought.iter().find(|pick| pick.method == "smith:bronze_platebody").expect("platebody ranked");
    assert!(platebody.cost_per_xp < bought_platebody.cost_per_xp, "smelted bars add 6.2 XP each");
}

/// Gold bars at 40: when every anvil product sells for nothing and gold
/// bars sell well, the furnace wins (the classic F2P gold smelt).
#[test]
fn smelting_gold_wins_when_it_pays() {
    let mut book = Book::default();
    book.buy.insert("steel_bar", (60.0, None));
    book.buy.insert("iron_bar", (40.0, None));
    book.buy.insert("bronze_bar", (10.0, None));
    book.buy.insert("gold_ore", (30.0, None));
    book.value.insert("gold_bar", 150.0);
    let pick = with_prices(&book, 0.5, 5000, |prices| rank_smithing(40, 45, prices));
    assert_eq!(pick[0].method, "smelt:gold_bar", "{:?}", &pick[..3]);
    assert_eq!(pick[0].missing, vec!["gold_ore".to_string()]);
}

/// Magic 25 (simulated bot, the fleet is at ≤ 10): water bolt costs its
/// runes; 300 own iron daggers alch at a small loss for 31 XP a cast and
/// win. The batch is the stock (casts the climb needs, at most), nature
/// and fire runes not held are obtained first.
#[test]
fn magic_25_alchs_its_stock_when_it_beats_bolts() {
    let mut book = Book::default();
    for (rune, price) in [("airrune", 5.0), ("waterrune", 5.0), ("chaosrune", 90.0), ("mindrune", 4.0), ("naturerune", 60.0), ("firerune", 5.0)] {
        book.buy.insert(rune, (price, None));
    }
    let supply = vec![AlchSupply { item: "iron_dagger".to_string(), units: 300, unit_cost: 10.0, own: true }];
    let ranked = with_prices(&book, 0.5, 50_000, |prices| rank_magic(25, 35, prices, &supply));
    let best = &ranked[0];
    assert_eq!(best.method, "alch:iron_dagger", "{ranked:?}");
    assert_eq!(best.batch, 300);
    assert_eq!(best.missing, vec!["naturerune".to_string(), "firerune".to_string()]);
    assert!(ranked.iter().any(|pick| pick.method == "fight:magic/water_bolt/cows"), "the bolt stays ranked");
    // No object to alch: the combat spell.
    let fight = with_prices(&book, 0.5, 50_000, |prices| best_pick("magic", 25, 30, prices, &[])).expect("a pick");
    assert_eq!(fight.method, "fight:magic/water_bolt/cows");
    assert_eq!(fight.missing, vec!["airrune".to_string(), "chaosrune".to_string(), "waterrune".to_string()]);
}

/// Magic 60 with a staff of fire: high alchemy, fire free, an ask under
/// the alch value bought to alch (the object itself is to obtain); the
/// wallet bounds the batch.
#[test]
fn magic_60_high_alchs_cheap_asks_with_a_fire_staff() {
    let mut book = Book::default();
    book.held.insert("staff_of_fire", 1);
    book.held.insert("naturerune", 500);
    book.buy.insert("naturerune", (60.0, None));
    for (rune, price) in [("airrune", 5.0), ("deathrune", 150.0), ("firerune", 5.0)] {
        book.buy.insert(rune, (price, None));
    }
    let high = crate::alchemy::alch_value("steel_platebody", true).expect("steel platebody alchs") as f64;
    let supply = vec![AlchSupply { item: "steel_platebody".to_string(), units: 400, unit_cost: high - 80.0, own: false }];
    let ranked = with_prices(&book, 0.5, 100_000, |prices| rank_magic(60, 65, prices, &supply));
    assert_eq!(ranked[0].method, "alch:steel_platebody", "{ranked:?}");
    assert_eq!(ranked[0].missing, vec!["steel_platebody".to_string()], "runes held, fire from the staff");
    let poor = with_prices(&book, 0.5, (high as i64) * 20, |prices| rank_magic(60, 65, prices, &supply));
    let batch = poor.iter().find(|pick| pick.method == "alch:steel_platebody").map(|pick| pick.batch).unwrap_or(0);
    assert!((10..=21).contains(&batch), "the wallet pays ~20 platebodies, got {batch}");
}

/// Below confuse, no pricing: the static wind strike runs. Under the
/// alchemy level the stock never alchs.
#[test]
fn magic_under_21_never_alchs() {
    let mut book = Book::default();
    for (rune, price) in [("airrune", 5.0), ("mindrune", 4.0), ("waterrune", 5.0), ("earthrune", 5.0), ("chaosrune", 90.0)] {
        book.buy.insert(rune, (price, None));
    }
    let supply = vec![AlchSupply { item: "iron_dagger".to_string(), units: 300, unit_cost: 10.0, own: true }];
    assert_eq!(with_prices(&book, 0.5, 1000, |prices| best_pick("magic", 2, 3, prices, &supply)), None);
    let pick = with_prices(&book, 0.5, 1000, |prices| best_pick("magic", 20, 21, prices, &supply)).expect("a pick");
    assert!(!pick.method.starts_with("alch:"), "{pick:?}");
}

/// User 2026-11-15: the most profitable method, never a loss when one
/// pays; among losses the smallest coin loss per XP (not per tick).
#[test]
fn a_paying_method_beats_any_loss() {
    let row = |method: &str, profit_per_tick: f64, xp_per_tick: f64| super::TrainPick {
        method: method.to_string(),
        batch: 0,
        missing: Vec::new(),
        buys_inputs: false,
        cost_per_xp: -profit_per_tick / xp_per_tick,
        xp_per_tick,
        profit_per_tick,
    };
    let mut picks = vec![row("slow_small_loss", -0.5, 0.5), row("fast_loss", -2.0, 4.0), row("pays", 0.3, 1.0), row("pays_more", 1.2, 0.8)];
    super::sort_best_first(&mut picks);
    let order: Vec<&str> = picks.iter().map(|pick| pick.method.as_str()).collect();
    assert_eq!(order, vec!["pays_more", "pays", "fast_loss", "slow_small_loss"], "0.5/XP lost beats 1.0/XP lost");
}



/// What a guide-picked climb obtains first (R6, brain side): the
/// consumables of its yield the bot holds less than an action of; an air
/// staff pays the teleport's air runes.
#[test]
fn a_climb_obtains_what_it_lacks_for_one_action() {
    let none = |_: &str| 0;
    assert_eq!(super::missing_inputs("fire:yew_logs", 60, 65, &none), vec!["yew_logs".to_string()]);
    let logs = |item: &str| if item == "yew_logs" { 5 } else { 0 };
    assert!(super::missing_inputs("fire:yew_logs", 60, 65, &logs).is_empty(), "a load in hand");
    let staff = |item: &str| if item == "staff_of_air" { 1 } else { 0 };
    let mut runes = super::missing_inputs("teleport:varrock_teleport", 25, 30, &staff);
    runes.sort();
    assert_eq!(runes, vec!["firerune".to_string(), "lawrune".to_string()], "the staff pays the air");
    assert!(super::missing_inputs("fight:melee/cows", 10, 15, &none).is_empty(), "melee burns nothing");
}
