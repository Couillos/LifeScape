//! Locked stock is not the bot's to use (live 2026-11-13, b3gybcev).

use super::{free_count, free_tool_names};

/// b3gybcev: a hammer listed for sale (order 56648) is locked, the smith
/// script cannot take it out, yet the brain read "holds 1 hammer, done
/// without launch" four times and banned smithing. Held minus locked.
#[test]
fn locked_stock_is_not_held() {
    assert_eq!(free_count(1, 1), 0, "the listed hammer");
    assert_eq!(free_count(5, 2), 3);
    assert_eq!(free_count(1, 3), 0, "never negative");
    let entries = vec![("hammer".to_string(), 1), ("net".to_string(), 1), ("fishing_bait".to_string(), 300)];
    let locked = |name: &str| match name {
        "hammer" => 1,
        "fishing_bait" => 100,
        _ => 0,
    };
    assert_eq!(free_tool_names(&entries, &locked), vec!["net".to_string(), "fishing_bait".to_string()]);
}

/// User 2026-11-19 (point 2): ida_reed had sold 84 big bones to
/// thorn_vera and 1 strung sapphire amulet to rae_tanner, the handovers
/// still ahead; her bank janitor listed them again ("seller holds 0"),
/// marked them unsellable and could drop them on a full bank. Goods sold
/// and not yet handed over ride the janitor's keep list: never relisted,
/// never dropped. Bought goods, settled and failed deals do not.
#[test]
fn sold_goods_awaiting_their_handover_are_kept() {
    use lifescape_store::{Deal, DealStatus};
    let deal = |seller: &str, item_id: i64, status: DealStatus| Deal {
        deal_id: 1,
        buyer: if seller == "ida_reed" { "thorn_vera".to_string() } else { "ida_reed".to_string() },
        seller: seller.to_string(),
        item_id,
        quantity: 84,
        price: 30,
        meeting_x: 0,
        meeting_y: 0,
        meeting_level: 0,
        status,
        fail_reason: String::new(),
        deadline_tick: 0,
        created_clock: 0,
        updated_clock: 0,
        taker_side: None,
    };
    let big_bones = lifescape_gamedata::objs_by_name("big_bones")[0].obj_id as i64;
    let amulet = lifescape_gamedata::objs_by_name("strung_sapphire_amulet")[0].obj_id as i64;
    let deals = vec![
        deal("ida_reed", big_bones, DealStatus::Convened),
        deal("ida_reed", amulet, DealStatus::Matched),
        deal("ida_reed", 1515, DealStatus::Settled),
        deal("rae_tanner", 1511, DealStatus::Convened),
    ];
    assert_eq!(super::promised_names("ida_reed", &deals), vec!["big_bones".to_string(), "strung_sapphire_amulet".to_string()]);
}
