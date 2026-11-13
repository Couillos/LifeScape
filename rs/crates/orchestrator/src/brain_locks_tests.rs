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
