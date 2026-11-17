//! Funding plan: greedy split, donor reserves, exclusions, idempotence.

use super::{margin, plan, reserve_of, DonorPurse, Gift, MakerNeed, MIN_GIFT, MIN_MARGIN};

fn maker(account: &str, capital: Option<i64>) -> MakerNeed {
    MakerNeed { account: account.to_string(), capital, target: 103_000, quoting: false, pending_in: 0 }
}

fn donor(account: &str, coins: i64) -> DonorPurse {
    DonorPurse { account: account.to_string(), coins: Some(coins), ..DonorPurse::default() }
}

fn gift(donor: &str, maker: &str, amount: i64) -> Gift {
    Gift { donor: donor.to_string(), maker: maker.to_string(), amount }
}

/// The margin is 10 % of the purse, never under its floor.
#[test]
fn margin_has_a_floor() {
    assert_eq!(margin(0), MIN_MARGIN);
    assert_eq!(margin(20_000), MIN_MARGIN);
    assert_eq!(margin(300_000), 30_000);
}

/// A donor keeps its bids, its next purchase and its margin; what it
/// already owes is counted out too.
#[test]
fn donor_reserve_is_named() {
    let purse = DonorPurse { committed: 12_000, next_purchase: 8_000, pending_out: 20_000, ..donor("rich", 200_000) };
    let reserve = reserve_of(&purse).expect("a donor");
    assert_eq!((reserve.committed, reserve.next_purchase, reserve.margin, reserve.pending_out), (12_000, 8_000, 20_000, 20_000));
    assert_eq!(reserve.free, 200_000 - 12_000 - 8_000 - 20_000 - 20_000);
    let poor = reserve_of(&DonorPurse { committed: 9_000, ..donor("poor", 10_000) }).expect("a donor");
    assert_eq!(poor.free, 0, "never below zero");
}

/// Staff, private, makers, Irons and unread banks never give.
#[test]
fn exclusions_never_give() {
    let donors = vec![
        DonorPurse { excluded: true, ..donor("admin", 5_000_000) },
        DonorPurse { excluded: true, ..donor("bite", 5_000_000) },
        DonorPurse { maker: true, ..donor("mm_other", 5_000_000) },
        DonorPurse { ironman: true, ..donor("iron_ann", 5_000_000) },
        DonorPurse { coins: None, ..donor("unread", 0) },
    ];
    let funded = plan(&[maker("mm_a", Some(0))], &donors);
    assert!(funded.gifts.is_empty(), "{:?}", funded.gifts);
    assert_eq!(funded.uncovered, 103_000);
    let skipped: Vec<&str> = funded.skipped.iter().map(|(account, _)| account.as_str()).collect();
    assert_eq!(skipped, vec!["admin", "bite", "mm_other", "iron_ann", "unread"]);
}

/// Greedy: the largest shortfall first, the richest donor first, the
/// next one when it runs dry; a covered maker gets nothing more.
#[test]
fn greedy_from_the_richest() {
    let makers = vec![maker("mm_a", Some(93_000)), maker("mm_b", Some(3_000)), maker("mm_c", Some(103_500))];
    let donors = vec![
        DonorPurse { committed: 20_000, ..donor("rich", 150_000) },
        donor("richer", 200_000),
        donor("small", 20_000),
    ];
    let funded = plan(&makers, &donors);
    // richer frees 180 000, rich 150 000 - 15 000 - 20 000 = 115 000,
    // small 20 000 - 5 000 = 15 000. mm_b lacks 100 000, mm_a 10 000.
    assert_eq!(funded.gifts, vec![gift("richer", "mm_b", 100_000), gift("rich", "mm_a", 10_000)]);
    assert_eq!(funded.uncovered, 0);
    let planned: Vec<(&str, i64, i64)> = funded.makers.iter().map(|line| (line.account.as_str(), line.shortfall, line.planned)).collect();
    assert_eq!(planned, vec![("mm_a", 10_000, 10_000), ("mm_b", 100_000, 100_000), ("mm_c", 0, 0)]);
}

/// A shortfall larger than one purse is split over several donors.
#[test]
fn one_shortfall_over_several_donors() {
    let donors = vec![donor("first", 70_000), donor("second", 50_000), donor("third", 30_000)];
    let funded = plan(&[maker("mm_a", Some(0))], &donors);
    // Free: first 63 000, second 45 000, third 25 000.
    assert_eq!(funded.gifts, vec![gift("first", "mm_a", 63_000), gift("second", "mm_a", 40_000)]);
    assert_eq!(funded.uncovered, 0);
}

/// Quoting, offline and small shortfalls are left alone.
#[test]
fn quoting_offline_and_small_needs_wait() {
    let makers = vec![
        MakerNeed { quoting: true, ..maker("mm_q", Some(0)) },
        maker("mm_off", None),
        maker("mm_near", Some(103_000 - MIN_GIFT + 1)),
    ];
    let funded = plan(&makers, &[donor("rich", 1_000_000)]);
    assert!(funded.gifts.is_empty(), "{:?}", funded.gifts);
    assert_eq!(funded.uncovered, 0);
    assert_eq!(funded.skipped.iter().map(|(account, _)| account.as_str()).collect::<Vec<_>>(), vec!["mm_q", "mm_off"]);
}

/// Idempotence: once the planned gifts are in flight (pending on both
/// sides), the same purses plan nothing more; once settled (coins moved)
/// neither.
#[test]
fn plan_again_gives_nothing_more() {
    let makers = vec![maker("mm_a", Some(3_000)), maker("mm_b", Some(50_000))];
    let donors = vec![donor("rich", 400_000), donor("other", 90_000)];
    let first = plan(&makers, &donors);
    assert_eq!(first.gifts.iter().map(|gift| gift.amount).sum::<i64>(), 100_000 + 53_000);
    let owed = |account: &str| first.gifts.iter().filter(|gift| gift.donor == account).map(|gift| gift.amount).sum::<i64>();
    let owed_to = |account: &str| first.gifts.iter().filter(|gift| gift.maker == account).map(|gift| gift.amount).sum::<i64>();
    let in_flight_makers: Vec<MakerNeed> = makers.iter().map(|need| MakerNeed { pending_in: owed_to(&need.account), ..need.clone() }).collect();
    let in_flight_donors: Vec<DonorPurse> = donors.iter().map(|purse| DonorPurse { pending_out: owed(&purse.account), ..purse.clone() }).collect();
    assert!(plan(&in_flight_makers, &in_flight_donors).gifts.is_empty(), "gifts in flight count");
    let settled_makers: Vec<MakerNeed> = makers.iter().map(|need| MakerNeed { capital: need.capital.map(|coins| coins + owed_to(&need.account)), ..need.clone() }).collect();
    let settled_donors: Vec<DonorPurse> = donors.iter().map(|purse| DonorPurse { coins: purse.coins.map(|coins| coins - owed(&purse.account)), ..purse.clone() }).collect();
    assert!(plan(&settled_makers, &settled_donors).gifts.is_empty(), "settled gifts are in the purses");
}

/// The dry-run lines name every gift as `donor -> maker : amount`.
#[test]
fn plan_lines_name_each_gift() {
    let funded = plan(&[maker("mm_a", Some(3_000))], &[donor("rich", 400_000)]);
    let lines = super::plan_lines(&funded);
    assert!(lines.iter().any(|line| line.trim() == "rich -> mm_a : 100000"), "{lines:#?}");
    assert!(lines.iter().any(|line| line.contains("margin 40000")), "{lines:#?}");
}
