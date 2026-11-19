//! Live 2026-11-19: 51 bots each bid for one amulet of power (3 557 up to
//! 1 042 900), its inputs rest at a few coins, and nobody answered: the
//! fleet's best crafter (wolfe_jack, crafting 53, magic 25) was 17 levels
//! under the diamond amulet (70) when a bid trained 15 at most, magic
//! (57 for the enchant) was never trained for a bid, and the lot of 26
//! amulets wanted 26 diamonds when 6 uncut ones rested on the market.
//! User 2026-11-19: magic trains for a bid, no cap on the levels trained
//! (the training cost decides), the lot shrinks to the inputs available.

use super::{explain, opportunity, Producer};
use crate::market_fit::BookView;

fn asks(price: i64, qty: i64) -> Option<BookView> {
    Some(BookView { bids: Vec::new(), asks: vec![(price, qty)], volume_24h: 0, ..BookView::default() })
}

/// The 51 live amulet of power bids (one unit each).
fn power_bids() -> BookView {
    let prices = [
        1_042_900, 320_103, 269_511, 206_433, 185_596, 180_980, 170_740, 162_353, 160_174, 156_645, 155_175, 154_906, 138_071, 119_242, 118_777, 116_957, 112_489, 104_773,
        103_618, 101_610, 97_563, 84_946, 83_120, 82_456, 80_463, 75_153, 74_801, 74_438, 72_301, 57_894, 55_766, 50_494, 47_324, 37_850, 35_881, 31_164, 31_066, 30_178,
        26_422, 26_131, 23_750, 23_323, 19_690, 17_374, 13_319, 12_580, 9_187, 8_402, 4_206, 3_688, 3_557,
    ];
    BookView { bids: prices.iter().map(|price| (*price, 1)).collect(), asks: Vec::new(), volume_24h: 0, ..BookView::default() }
}

/// The live asks of every input (2026-11-19): 6 uncut diamonds, no cut
/// one, plenty of everything else.
fn live_market(item: &str) -> Option<BookView> {
    match item {
        "amulet_of_power" => Some(power_bids()),
        "uncut_diamond" => Some(BookView { bids: vec![(65, 473)], asks: vec![(1, 1), (140, 1), (146, 1), (170, 3)], volume_24h: 0, ..BookView::default() }),
        "gold_bar" => asks(35, 369),
        "ball_of_wool" => asks(3, 229),
        // The live rune supply (2026-11-19): chaos and water runes are
        // short of a water bolt climb (7 548 and 15 096 needed).
        "cosmicrune" => asks(7, 1075),
        "earthrune" => asks(1, 42_241),
        "airrune" => asks(2, 37_208),
        "waterrune" => asks(1, 6711),
        "firerune" => asks(1, 31_430),
        "mindrune" => asks(1, 77_655),
        "chaosrune" => asks(1, 343),
        "bodyrune" => asks(1, 59_867),
        "gold_ore" => asks(20, 2000),
        "sapphire" | "uncut_sapphire" => asks(40, 500),
        "emerald" | "uncut_emerald" => asks(80, 500),
        "ruby" | "uncut_ruby" => asks(150, 500),
        _ => None,
    }
}

fn wolfe_jack_levels(skill: &str) -> u8 {
    match skill {
        "crafting" => 53,
        "magic" => 25,
        "smithing" => 55,
        "mining" => 71,
        _ => 40,
    }
}

/// Moulds, chisel and the standing kit held.
fn crafter_tools(item: &str) -> i64 {
    i64::from(matches!(item, "amulet_mould" | "ring_mould" | "necklace_mould" | "chisel" | "hammer" | "tinderbox"))
}

fn crafter<'a>(level_of: &'a dyn Fn(&str) -> u8, held: &'a dyn Fn(&str) -> i64, avoid: &'a [String]) -> Producer<'a> {
    Producer {
        level_of,
        held,
        avoid,
        quests_done: &[],
        coin_rate: 11.57,
        // As live (`brain_bids::units_per_6000_at`): a method read for no
        // object (a fight trains magic, it makes nothing) has no rate.
        units_per_6000: &|method: &str, item: &str| {
            if item.is_empty() {
                f64::NAN
            } else if method.starts_with("farm:") || method.starts_with("gather:") {
                200.0
            } else {
                600.0
            }
        },
        ironman: false,
        seed: 1,
        tick: 0,
        combat: 70,
        position: None,
        free_pass: false,
        planned_level: &super::no_plan,
        alch_net: &super::no_alch,
        rate: &crate::work_rate::prior_rate,
    }
}

/// wolfe_jack weighs the amulet of power bids: crafting 53 → 70 (17
/// levels, past the old cap) and magic 25 → 57 (now a trainable skill)
/// are priced, and the lot is as many amulets as the 6 uncut diamonds on
/// sale allow, never 26 it has no diamonds for.
#[test]
fn the_best_crafter_trains_crafting_and_magic_for_the_amulet_of_power_bids() {
    let avoid: Vec<String> = Vec::new();
    let bot = crafter(&wolfe_jack_levels, &crafter_tools, &avoid);
    let found = opportunity("amulet_of_power", &power_bids(), &live_market, &bot);
    let why = explain("amulet_of_power", &power_bids(), &live_market, &bot).join("\n");
    let found = found.unwrap_or_else(|| panic!("an opportunity:\n{why}"));
    assert!(found.unit.train.contains(&("crafting".to_string(), 53, 70)), "{:?}\n{why}", found.unit.train);
    assert!(found.unit.train.contains(&("magic".to_string(), 25, 57)), "{:?}\n{why}", found.unit.train);
    assert_eq!(found.count, 6, "one amulet per uncut diamond on sale:\n{why}");
    // The training is priced (crafting by a gem or gold recipe on sale,
    // never 7 071 diamond rings; magic by its fight's ticks per XP):
    // whether it pays is the scoring rule's call, not a cap.
    assert!(why.contains("train crafting to 70 by") && why.contains("train magic to 57 by fight:magic/"), "{why}");
    assert!(!why.contains("by jewelry:diamond_ring"), "{why}");
    assert!(found.ticks > 0.0 && found.spent > 0.0, "{found:?}");
}

/// The lot shrinks to the inputs, never to nothing: 2 uncut diamonds on
/// sale make 2 amulets; none and the bot cannot answer at all.
#[test]
fn the_lot_shrinks_to_the_inputs_on_sale() {
    let avoid: Vec<String> = Vec::new();
    let bot = crafter(&wolfe_jack_levels, &crafter_tools, &avoid);
    let two = |item: &str| if item == "uncut_diamond" { asks(150, 2) } else { live_market(item) };
    assert_eq!(opportunity("amulet_of_power", &power_bids(), &two, &bot).map(|found| found.count), Some(2));
    let none = |item: &str| if item == "uncut_diamond" { None } else { live_market(item) };
    assert_eq!(opportunity("amulet_of_power", &power_bids(), &none, &bot), None, "no diamond anywhere");
}

