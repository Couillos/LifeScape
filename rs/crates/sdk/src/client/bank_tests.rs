use super::{bank_button, deposit_presses, transfer_refused, BankButton};

/// Every amount from 1 to 120 against stacks of 1 to 120: exactly one
/// button, it moves exactly what is asked (All only when the whole stack
/// goes), and 1/5/10 win only when they fall exactly.
#[test]
fn one_button_moves_exactly_the_amount() {
    for stack in 1..=120u32 {
        for want in 1..=120u32 {
            let button = bank_button(want, stack).expect("a positive transfer has a button");
            let moved = match button {
                BankButton::One => 1,
                BankButton::Five => 5,
                BankButton::Ten => 10,
                BankButton::All => stack,
                BankButton::Amount(amount) => amount,
            };
            assert_eq!(moved, want.min(stack), "want {want} of {stack}: {button:?}");
            if want >= stack {
                assert_eq!(button, BankButton::All, "the whole stack goes in one Deposit/Withdraw-All");
            } else if matches!(want, 1 | 5 | 10) {
                assert_ne!(button.op(), 5, "1/5/10 never open the count dialog");
            } else {
                assert_eq!(button, BankButton::Amount(want), "any other part of a stack is one X");
            }
        }
    }
}

/// The user's scenarios: coins (arbitrary amount → X), a pile of ores
/// (all → All), 5 and 10 units, nothing to move.
#[test]
fn bank_scenarios_pick_the_player_button() {
    assert_eq!(bank_button(60, 2726), Some(BankButton::Amount(60)), "60 coins out of 2726: Withdraw-X 60");
    assert_eq!(bank_button(27, 27), Some(BankButton::All), "27 ores carried: Deposit-All");
    assert_eq!(bank_button(u32::MAX, 14), Some(BankButton::All), "deposit everything carried");
    assert_eq!(bank_button(5, 300), Some(BankButton::Five));
    assert_eq!(bank_button(10, 300), Some(BankButton::Ten));
    assert_eq!(bank_button(1, 300), Some(BankButton::One));
    assert_eq!(bank_button(28, 14), Some(BankButton::All), "more wanted than banked: the whole stack");
    assert_eq!(bank_button(0, 14), None);
    assert_eq!(bank_button(3, 0), None);
    assert_eq!([BankButton::One, BankButton::Five, BankButton::Ten, BankButton::All, BankButton::Amount(7)].map(BankButton::op), [1, 2, 3, 4, 5]);
}

/// Live 2026-11-16 (oscar_cole): 25 raw trout/salmon slots took 25
/// Deposit-All presses. One press per object type, its lowest slot;
/// kept types never pressed.
#[test]
fn deposit_all_presses_once_per_type() {
    const TROUT: u16 = 335;
    const SALMON: u16 = 331;
    const ROD: u16 = 309;
    let mut slots: Vec<(u8, (u16, u32))> = vec![(0, (ROD, 1))];
    for slot in 1..28u8 {
        slots.push((slot, (if slot % 3 == 0 { SALMON } else { TROUT }, 1)));
    }
    assert_eq!(deposit_presses(&slots, &[ROD]), vec![(1, TROUT), (3, SALMON)], "two presses for 27 slots");
    assert!(deposit_presses(&[(4, (ROD, 1))], &[ROD]).is_empty(), "only kept items: nothing to press");
}

/// Refusals stop the wait at once; older lines never replay.
#[test]
fn refusals_end_the_wait() {
    let lines = vec![
        "You don't have enough inventory space to withdraw that many.".to_string(),
        "You need to subscribe for a members account if you want to".to_string(),
    ];
    assert!(transfer_refused(&lines, 0));
    assert!(transfer_refused(&lines, 1), "bank type cap");
    assert!(!transfer_refused(&lines, 2), "nothing new");
    assert!(!transfer_refused(&["Welcome to RuneScape.".to_string()], 0));
}
