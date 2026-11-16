use super::{kind, ItemLedger, Seen, ACTION_TICKS};
use std::collections::HashMap;

fn seen(pack: &[(u16, u32)], tick: u64, xp: &[(u8, i32)], context: Option<(&'static str, u64)>, bank_open: bool) -> Seen<'static> {
    Seen { pack: pack.iter().copied().collect::<HashMap<_, _>>(), bank_open, tick, xp: xp.to_vec(), dead: false, context }
}

#[test]
fn every_change_names_its_cause() {
    let mut ledger = ItemLedger::default();
    ledger.observe(seen(&[(440, 5)], 1, &[(13, 100)], None, false));
    // A smelt: ore out, bar in, XP up.
    ledger.observe(seen(&[(440, 4), (2349, 1)], 10, &[(13, 106)], None, false));
    // A shop buy: hammer in under the shop context.
    ledger.observe(seen(&[(440, 4), (2349, 1), (2347, 1)], 30, &[(13, 106)], Some(("shop", 40)), false));
    // A drop, scoped to the bar.
    ledger.note_action(kind::DROPPED, Some(2349), -1, 60 + ACTION_TICKS);
    ledger.observe(seen(&[(440, 4), (2347, 1)], 60, &[(13, 106)], None, false));
    // Banked: a transfer, no flow.
    ledger.observe(seen(&[], 90, &[(13, 106)], None, true));
    let totals = ledger.totals_since(0);
    assert!(totals.contains(&(440, kind::SKILL, -1)), "{totals:?}");
    assert!(totals.contains(&(2349, kind::SKILL, 1)));
    assert!(totals.contains(&(2347, "shop", 1)));
    assert!(totals.contains(&(2349, kind::DROPPED, -1)));
    assert_eq!(totals.len(), 4, "the bank transfer adds nothing: {totals:?}");
    let smelt = ledger.flows.iter().find(|flow| flow.obj == 2349 && flow.delta == 1).expect("bar flow");
    assert_eq!(smelt.skills, 1 << 13, "the smithing bit rides the flow");
}

/// A deposit whose inventory update lands just after the local bank
/// close stays a transfer (the coins "lost" of the 2026-11-16 audit).
#[test]
fn bank_close_grace_keeps_late_deposits_transfers() {
    let mut ledger = ItemLedger::default();
    ledger.observe(seen(&[(995, 9_000)], 1, &[], None, false));
    ledger.observe(seen(&[(995, 9_000)], 5, &[], None, true));
    ledger.observe(seen(&[], 6, &[], None, false));
    assert!(ledger.flows.is_empty(), "late deposit is a transfer: {:?}", ledger.flows);
    ledger.observe(seen(&[(995, 3)], 20, &[], None, false));
    assert_eq!(ledger.totals_since(0), vec![(995, "unattributed", 3)]);
}

/// Scoped actions explain only their object and sign: arrows fired just
/// after a pickup are never the pickup's (ground_spawn negative, audit
/// §8); eating while a loot pickup is in force stays eating.
#[test]
fn scoped_actions_match_object_and_sign() {
    let mut ledger = ItemLedger::default();
    ledger.observe(seen(&[(882, 50), (315, 3)], 1, &[(4, 0)], None, false));
    ledger.note_action(kind::PICKUP, Some(526), 1, 40);
    ledger.observe(seen(&[(882, 49), (315, 3), (526, 1)], 3, &[(4, 0)], None, false));
    ledger.note_action(kind::EAT, Some(315), -1, 4 + ACTION_TICKS);
    ledger.observe(seen(&[(882, 48), (315, 2), (526, 1)], 5, &[(4, 40)], None, false));
    let totals = ledger.totals_since(0);
    assert!(totals.contains(&(526, kind::PICKUP, 1)), "{totals:?}");
    assert!(totals.contains(&(315, kind::EAT, -1)), "{totals:?}");
    assert!(totals.contains(&(882, "unattributed", -1)), "first arrow before any XP: {totals:?}");
    assert!(totals.contains(&(882, kind::SKILL, -1)), "second arrow inside the ranged XP window: {totals:?}");
}

/// Wielding moves nothing out of what the bot carries (pack plus worn
/// observed together); a death takes the losses that follow it.
#[test]
fn deaths_take_the_losses_after_them() {
    let mut ledger = ItemLedger::default();
    ledger.observe(seen(&[(1277, 1), (315, 5)], 1, &[], None, false));
    let mut dead = seen(&[(1277, 1), (315, 5)], 2, &[], None, false);
    dead.dead = true;
    ledger.observe(dead);
    ledger.observe(seen(&[], 12, &[], None, false));
    assert_eq!(ledger.totals_since(0), vec![(315, kind::DEATH, -5), (1277, kind::DEATH, -1)]);
}

/// Casts explain runes out and the product in, both signs.
#[test]
fn a_cast_explains_both_sides() {
    let mut ledger = ItemLedger::default();
    ledger.observe(seen(&[(561, 5), (1637, 1)], 1, &[(6, 0)], None, false));
    ledger.note_action(kind::CAST, None, 0, 2 + ACTION_TICKS);
    ledger.observe(seen(&[(561, 4), (1639, 1)], 3, &[(6, 0)], None, false));
    let totals = ledger.totals_since(0);
    assert_eq!(totals.len(), 3);
    assert!(totals.iter().all(|(_, kind, _)| *kind == kind::CAST), "{totals:?}");
}
