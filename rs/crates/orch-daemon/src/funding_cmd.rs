#![forbid(unsafe_code)]
//! `orchestrator fund-makers [--dry-run] [--force]` (user 2026-11-17): the richest
//! fleet accounts give coins to the market makers still earning, so they
//! reach the common capital target and switch to quoting at their next
//! launch pass (`market_maker::maybe_switch`).
//!
//! The daemon reads every connected account's live purse the way the
//! switch reads it (`live_coins`: pack, bank and worn coins of the live
//! snapshot, bank streamed), the makers' roles from their lives (offline
//! makers listed, never funded), each donor's reserve (resting bids and
//! buyer deals, next gear and kit purchase, margin) and the gifts in
//! flight; `funding_plan::plan` splits the shortfalls; without
//! `--dry-run` each planned gift becomes a gift deal
//! (`funding_gifts`), delivered by the market's physical handover.
//! One-shot and idempotent: run it again, the gifts in flight count on
//! both sides and nothing is planned twice.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use lifescape_json::{object, str_value, Value};

use crate::daemon::Orchestrator;
use crate::funding_plan::{DonorPurse, MakerNeed};
use crate::jobs::Job;
use crate::market_service::MarketService;

/// Coins of a live snapshot as the maker switch reads them
/// (`brain_plan::holdings_count(job, "coins")`: pack, bank, worn);
/// `None` while the bank is not streamed (its coins unread).
pub fn live_coins(live: &Value) -> Option<i64> {
    (live.get("bank_streamed").as_bool() == Some(true)).then(|| crate::live_items::live_units(live, "coins"))
}

fn sum_for(flying: &[(String, String, i64)], wanted: &dyn Fn(&(String, String, i64)) -> bool) -> i64 {
    flying.iter().filter(|gift| wanted(gift)).map(|gift| gift.2).sum()
}

/// The plan's inputs from the connected jobs, the lives and the gifts in
/// flight, plus the accounts left out before the plan (no life file).
fn gather(orch: &Orchestrator, service: &MarketService) -> (Vec<MakerNeed>, Vec<DonorPurse>, Vec<String>) {
    let mut jobs: Vec<Arc<Job>> = orch.jobs.lock().map(|jobs| jobs.values().cloned().collect()).unwrap_or_default();
    jobs.sort_by(|left, right| left.account.cmp(&right.account));
    let flying = service.gifts_in_flight();
    let excluded: BTreeSet<String> = crate::economy_guard::excluded_accounts();
    let deps = lifescape_orch_brain::brain_deps::BrainDeps::live();
    let (mut makers, mut donors, mut notes) = (Vec::new(), Vec::new(), Vec::new());
    let mut online: BTreeSet<String> = BTreeSet::new();
    for job in &jobs {
        let account = job.account.clone();
        online.insert(account.clone());
        let live = job.inner.try_lock().ok().and_then(|inner| inner.live_shared());
        let coins = live.as_deref().and_then(live_coins);
        let pending_in = sum_for(&flying, &|gift| gift.1 == account);
        let pending_out = sum_for(&flying, &|gift| gift.0 == account);
        let is_excluded = excluded.contains(&account.to_lowercase());
        let life = if is_excluded { None } else { crate::brain::load_life(&account) };
        if let Some(role) = life.as_ref().and_then(lifescape_planner::maker_role) {
            let role = crate::market_maker::current_role(role);
            makers.push(MakerNeed { account, capital: coins, target: role.capital_target, quoting: role.phase == lifescape_planner::MakerPhase::Quoting, pending_in });
            continue;
        }
        if is_excluded {
            donors.push(DonorPurse { account, coins, excluded: true, ..DonorPurse::default() });
            continue;
        }
        let Some(life) = life else {
            notes.push(format!("  skipped {account}: no life file, not a fleet bot"));
            continue;
        };
        let readable = coins.is_some() && !life.profile.ironman;
        let next_purchase = match (&live, readable) {
            (Some(live), true) => crate::brain_gear::next_gear_cost(job, &life, live, &deps) + crate::brain_tool_shop::next_kit_price(job, &life, live, &deps),
            _ => 0,
        };
        // `committed_coins` counts the gifts it owes: named apart (`pending_out`).
        let committed = if readable { (crate::brain_gear::committed_coins(&account, None, &deps) - pending_out).max(0) } else { 0 };
        donors.push(DonorPurse { account, coins, committed, next_purchase, pending_out, ironman: life.profile.ironman, excluded: false, maker: false });
    }
    for (account, role) in crate::market_maker_report::maker_lives(&crate::paths::lives_dir()) {
        if online.contains(&account) {
            continue;
        }
        let pending_in = sum_for(&flying, &|gift| gift.1 == account);
        makers.push(MakerNeed { account, capital: None, target: role.capital_target, quoting: role.phase == lifescape_planner::MakerPhase::Quoting, pending_in });
    }
    (makers, donors, notes)
}

/// The daemon command: the plan's lines, and the gifts created unless
/// `dry_run`.
pub fn fund_makers(orch: &Orchestrator, dry_run: bool, force: bool) -> Value {
    let Some(service) = crate::market_service::shared() else {
        return object(vec![("ok", Value::Bool(false)), ("error", str_value("market service down: no gift can be convened"))]);
    };
    let (makers, donors, notes) = gather(orch, &service);
    let plan = crate::funding_plan::plan(&makers, &donors, force);
    let mut lines = vec![if dry_run { "fund-makers --dry-run: plan only, nothing created".to_string() } else { "fund-makers: gifts created below".to_string() }];
    if force {
        lines.push("--force: donors keep only their bid-backed coins and a 5000 margin (next purchase not reserved)".to_string());
    }
    lines.extend(crate::funding_plan::plan_lines(&plan));
    lines.extend(notes);
    if !dry_run {
        if crate::plan_service::stats_json().is_none() {
            lines.push("nav service down: the gifts wait for it to be convened".to_string());
        }
        for gift in &plan.gifts {
            match service.create_gift(&gift.donor, &gift.maker, gift.amount, 1) {
                Ok(id) => lines.push(format!("created gift {id}: {} -> {} : {}", gift.donor, gift.maker, gift.amount)),
                Err(error) => lines.push(format!("gift {} -> {} : {} refused: {error}", gift.donor, gift.maker, gift.amount)),
            }
        }
    }
    object(vec![("ok", Value::Bool(true)), ("lines", Value::Arr(lines.iter().map(|line| str_value(line)).collect()))])
}

/// `orchestrator fund-makers [--dry-run] [--force]`: asks the running daemon,
/// never starts one.
pub fn fund_makers_cli(args: &[String]) -> i32 {
    if let Some(unknown) = args.iter().find(|arg| arg.as_str() != "--dry-run" && arg.as_str() != "--force") {
        eprintln!("usage: fund-makers [--dry-run] [--force] (unknown argument {unknown:?})");
        return 2;
    }
    let dry_run = args.iter().any(|arg| arg == "--dry-run");
    let force = args.iter().any(|arg| arg == "--force");
    let request = object(vec![("cmd", str_value("fund_makers")), ("dry_run", Value::Bool(dry_run)), ("force", Value::Bool(force))]);
    match crate::client::send_command(&request, Duration::from_secs(120)) {
        Ok(reply) if reply.get("ok").as_bool() == Some(true) => {
            for line in reply.get("lines").as_arr().cloned().unwrap_or_default() {
                println!("{}", line.as_str().unwrap_or(""));
            }
            0
        }
        Ok(reply) => {
            eprintln!("fund-makers: {}", reply.get("error").as_str().unwrap_or("refused without a reason"));
            1
        }
        Err(error) => {
            eprintln!("fund-makers: daemon unreachable ({error}); start it with the documented procedure first");
            1
        }
    }
}

/// `orchestrator economy`: the funding section, from the journal.
pub fn print_section() {
    let text = std::fs::read_to_string(crate::funding_gifts::journal_path()).unwrap_or_default();
    for line in crate::funding_gifts::section_lines(&crate::funding_gifts::summarize(&text)) {
        println!("{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::live_coins;
    use lifescape_json::{object, str_value, Value};

    fn coins(count: i64) -> Value {
        object(vec![("id", Value::Int(995)), ("name", str_value("Coins")), ("count", Value::Int(count))])
    }

    fn snapshot(pack: i64, bank: i64, streamed: bool) -> Value {
        object(vec![
            ("inv", Value::Arr(vec![coins(pack)])),
            ("bank", Value::Arr(vec![coins(bank)])),
            ("bank_streamed", Value::Bool(streamed)),
        ])
    }

    /// The switch reads the REAL capital: pack plus bank coins of the
    /// live snapshot (`holdings_count`, what `maybe_switch` reads), the
    /// same number `fund-makers` reads; at the common target the maker
    /// switches, one coin under it keeps playing.
    #[test]
    fn switch_reads_pack_plus_bank() {
        let deps = &crate::brain_deps::BrainDeps::live();
        let job = crate::jobs::Job::new("mm_switch_test");
        job.inner.lock().expect("inner").live = Some(std::sync::Arc::new(snapshot(3_000, 100_000, true)));
        let read = crate::brain_plan::holdings_count(&job, "coins", deps);
        assert_eq!(read, 103_000, "pack 3 000 + bank 100 000");
        assert_eq!(live_coins(&snapshot(3_000, 100_000, true)), Some(read), "fund-makers reads like the switch");
        let role = crate::market_maker::current_role(lifescape_planner::MakerRole::new(0, 11, 1));
        assert_eq!(role.capital_target, 103_000, "the common target");
        assert!(lifescape_planner::switch_due(&role, read), "funded: switches at its next pass");
        assert!(!lifescape_planner::switch_due(&role, read - 1));
        job.inner.lock().expect("inner").live = Some(std::sync::Arc::new(snapshot(3_000, 0, false)));
        assert_eq!(crate::brain_plan::holdings_count(&job, "coins", deps), 3_000, "bank not streamed: under the target, never a false switch");
        assert_eq!(live_coins(&snapshot(3_000, 0, false)), None, "fund-makers never plans on an unread bank");
    }
}
