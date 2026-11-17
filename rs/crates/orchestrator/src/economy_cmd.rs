#![forbid(unsafe_code)]
//! `orchestrator economy [hours]`: the world's money at a glance (audit
//! 2026-11-15, chantier 10). Money supply now and its change per day
//! (`holdings_hourly` coins, staff and the persisted exclusions left out
//! at capture), the coin sources and sinks of the window by provenance
//! (task report coin lines, labelled by the SDK coin ledger), and a
//! Laspeyres price index of the window against the one before it, on the
//! fills of player trades. The market section prints the live criteria of
//! plan marché §6 (`Store::market_criteria`, lineages of schema 6), each
//! beside its 2026-11-16 baseline and target. The market makers section
//! (plan marché Q6) lists each maker: slice, phase, capital, P&L, mean
//! spread, book share, stock (`market_maker_report`). The bot choices
//! section (plan rendements R7, `brain_picks`) tallies the choices the
//! bots drew from the guides: methods chosen, exploration share.

use lifescape_store::{laspeyres, Store};

const COINS_ID: i64 = 995;

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs() as i64).unwrap_or(0)
}

/// Coins in the economy at or before `clock` (holdings snapshot).
fn supply_at(store: &Store, clock: i64) -> Option<i64> {
    let (_, items) = store.holdings_at_or_before(clock).ok()??;
    items.iter().find(|(item, _)| *item == COINS_ID).map(|(_, count)| *count)
}

pub fn economy_cli(args: &[String]) -> i32 {
    let hours: i64 = args.first().and_then(|raw| raw.parse().ok()).unwrap_or(24).max(1);
    // `economy [hours] item <name>`: where one object came from and went.
    if let Some(at) = args.iter().position(|arg| arg == "item") {
        return item_cli(hours, &args[at + 1..].join(" "));
    }
    let store = match Store::open(&crate::market_service::db_path()) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("economy: store: {error}");
            return 1;
        }
    };
    let now = now_secs();
    let span = hours * 3600;
    let since = now - span;
    let excluded: Vec<String> = crate::economy_guard::excluded_accounts().into_iter().collect();
    println!("== economy over the last {hours} h (staff and exclusions out) ==");
    match (supply_at(&store, now), supply_at(&store, since)) {
        (Some(current), Some(before)) => {
            let delta = current - before;
            println!("money supply: {current} coins (was {before}): {delta:+} over {hours} h, {:+.0} per day", delta as f64 * 24.0 / hours as f64);
        }
        (Some(current), None) => println!("money supply: {current} coins (no snapshot {hours} h ago)"),
        _ => println!("money supply: no holdings snapshot yet"),
    }
    match store.coin_flows_between(since, 0, &excluded) {
        Ok(flows) => {
            println!("coin flows by provenance (traded = between players, a transfer):");
            let mut created = 0i64;
            for flow in &flows {
                println!("  {:<14} {:>12} coins in {:>6} lines", flow.provenance, flow.coins, flow.lines);
                if flow.provenance != "traded" {
                    created += flow.coins;
                }
            }
            println!("  net created or destroyed outside trades: {created:+}");
            // Q3 (plan marché, 2026-11-16): coins the NPC shops paid for
            // the objects the market paid less for (D-A exception).
            let (npc_coins, npc_lines) = npc_sales(&flows);
            println!("npc shop sales (Q3, coins created): {npc_coins} coins in {npc_lines} lines");
        }
        Err(error) => println!("coin flows: {error}"),
    }
    match (store.fills_between(since - span, since), store.fills_between(since, 0)) {
        (Ok(base), Ok(current)) => {
            let ones = current.iter().filter(|fills| fills.mean_price <= 1.0).count();
            let volume: i64 = current.iter().map(|fills| fills.quantity).sum();
            match laspeyres(&base, &current) {
                Some(index) => println!("price index (window vs the one before): {:.3} ({:+.1} %)", index, (index - 1.0) * 100.0),
                None => println!("price index: no item traded in both windows"),
            }
            println!("fills: {} items, {volume} units; {ones} items trade at 1 coin", current.len());
        }
        (Err(error), _) | (_, Err(error)) => println!("fills: {error}"),
    }
    match store.market_criteria(since, 0, &excluded) {
        Ok(criteria) => print_market(&criteria, hours),
        Err(error) => println!("market criteria: {error}"),
    }
    crate::market_maker_report::print_rows(&crate::market_maker_report::maker_rows(&store, &crate::brain::lives_dir()));
    crate::funding_cmd::print_section();
    crate::trade_report::print_rows(&crate::trade_report::trade_rows(&store, &crate::brain::lives_dir()));
    crate::brain_picks::print_section(&crate::brain_picks::fleet_picks(&crate::brain::lives_dir()), since, hours);
    0
}

/// Coins created by NPC shop sales (`shop_sold` coin lines of the coin
/// ledger, plan marché Q3) and their line count.
pub fn npc_sales(flows: &[lifescape_store::CoinFlowTotal]) -> (i64, i64) {
    flows.iter().filter(|flow| flow.provenance == "shop_sold").fold((0, 0), |(coins, lines), flow| (coins + flow.coins.max(0), lines + flow.lines))
}

fn share(value: Option<f64>) -> String {
    value.map_or_else(|| "n/a".to_string(), |ratio| format!("{:.1} %", ratio * 100.0))
}

/// Seconds as ticks at the tick length this process knows
/// (`NODE_TICK_RATE`, printed beside: the server runs 60 ms today,
/// `world.json`), never guessed.
fn as_ticks(seconds: i64) -> String {
    let tick_ms = lifescape_sdk::waits::tick_ms().max(1);
    format!("{} ticks at {tick_ms} ms", (seconds.max(0) as u64).saturating_mul(1000) / tick_ms)
}

/// Plan marché §6 criteria (baseline 2026-11-16 -> target).
fn print_market(criteria: &lifescape_store::MarketCriteria, hours: i64) {
    println!("market (plan marché §6, last {hours} h; baseline 2026-11-16 -> target):");
    println!("  two-sided objects        {}/{} = {:.1} %   (12.7 % -> >= 35 %)", criteria.two_sided_items, criteria.active_items, criteria.two_sided_share() * 100.0);
    let spread = criteria.median_spread.map_or_else(|| "n/a".to_string(), |ratio| format!("x{ratio:.2}"));
    println!("  median ask/bid spread    {spread}   (x1.72 -> <= x1.25)");
    let ratio = criteria.cancel_ratio().map_or_else(|| "n/a".to_string(), |value| format!("{value:.2}"));
    println!("  true bid cancels/settled {}/{} = {ratio}   (6217/6929 -> <= 0.2)", criteria.buy_true_cancels, criteria.buy_settled);
    println!("  gear plan cancels        {}   (~5300/24 h -> <= 200)", criteria.gear_plan_cancels);
    let (lower, same, higher) = criteria.reopened;
    println!("  reopened lower           {} ({lower} lower, {same} same, {higher} higher)   (46 % -> <= 5 %)", share(criteria.reopened_lower_share()));
    let life = criteria.median_unserved_life_secs.map_or_else(|| "n/a".to_string(), |secs| format!("{secs} s = {}", as_ticks(secs)));
    println!("  median unserved buy life {life}   (~1080 s -> >= its horizon)");
    println!("  volume                   {} units, {} coins   (142957 / 3.75 M per 24 h -> +50 %)", criteria.volume_units, criteria.volume_coins);
    println!("  consumed objects at 1    {} {:?}   (>= 6 -> 0)", criteria.consumed_at_one.len(), criteria.consumed_at_one);
    println!("  asks at 1 coin           {} objects, {} orders, {} units   (85 / 353473 u. -> to follow)", criteria.ask_one_items, criteria.ask_one_orders, criteria.ask_one_units);
    println!("  asks-only, never traded  {}   (65 -> <= 20)", criteria.asks_only_never_traded);
    println!("  amends                   {} ({:.1}/min)", criteria.amends, criteria.amends as f64 / (hours as f64 * 60.0));
}

/// Sources and sinks of one object (or every object when `name` is
/// empty) over the last `hours`, by provenance (task report lines, the
/// SDK item ledger's attribution).
fn item_cli(hours: i64, name: &str) -> i32 {
    let store = match Store::open(&crate::market_service::db_path()) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("economy: store: {error}");
            return 1;
        }
    };
    let excluded: Vec<String> = crate::economy_guard::excluded_accounts().into_iter().collect();
    let filter = (!name.is_empty()).then_some(name);
    let flows = match store.item_flows_since(now_secs() - hours * 3600, &excluded, filter) {
        Ok(flows) => flows,
        Err(error) => {
            eprintln!("economy: {error}");
            return 1;
        }
    };
    println!("== item flows over the last {hours} h{} (staff out): + entered a pack, - left it ==", if name.is_empty() { String::new() } else { format!(" for {name}") });
    let mut shown = 0;
    for (item, provenance, quantity, lines) in &flows {
        if name.is_empty() && shown >= 60 {
            break;
        }
        println!("  {item:<24} {provenance:<14} {quantity:>9} in {lines:>5} lines");
        shown += 1;
    }
    if flows.is_empty() {
        println!("  no flow recorded");
    }
    0
}

#[cfg(test)]
mod tests {
    use lifescape_store::CoinFlowTotal;

    /// Q3 tracking: the coins NPC shops paid (`shop_sold` coin lines).
    #[test]
    fn npc_sales_count_the_coins_shops_created() {
        let flow = |provenance: &str, coins: i64, lines: i64| CoinFlowTotal { provenance: provenance.to_string(), coins, lines };
        let flows = vec![flow("traded", 9000, 40), flow("shop_sold", 1250, 12), flow("shop_bought", -800, 5), flow("loot", 300, 9)];
        assert_eq!(super::npc_sales(&flows), (1250, 12));
        assert_eq!(super::npc_sales(&[]), (0, 0));
    }
}
