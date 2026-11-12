#![forbid(unsafe_code)]
//! `orchestrator obtain`: offline view of the gamedata obtain index
//! (PRD3 lot 1). No daemon, no network.
//!
//! - `obtain <item>...`: every F2P way to get each object (make, gather,
//!   drop with rate and monster level, shop, dialogue, quest, spell…),
//!   plus the D-A verdicts (producible in F2P, NPC purchase allowed).
//! - `obtain --shops`: the F2P NPC-shop inventory, one line per stocked
//!   object with its shops and the purchase verdict.
//! - `obtain --unobtainable`: F2P tradeable objects with no F2P way, and why.

use lifescape_gamedata::obtain;

pub fn obtain_cmd(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        None => {
            eprintln!("usage: orchestrator obtain <item>... | --shops | --unobtainable");
            2
        }
        Some("--shops") => {
            print!("{}", shops_report());
            0
        }
        Some("--unobtainable") => {
            for obj in lifescape_gamedata::f2p::f2p_tradeable_objs() {
                let name = obj.name.unwrap_or("");
                if let Some(reason) = obtain::unobtainable_reason(name) {
                    println!("{name}\t{reason}");
                }
            }
            0
        }
        Some(_) => {
            let mut code = 0;
            for item in args {
                if lifescape_gamedata::objs_by_name(item).is_empty() {
                    eprintln!("obtain: unknown object {item} (debug names: bronze_pickaxe, naturerune…)");
                    code = 1;
                    continue;
                }
                print!("{}", item_report(item));
            }
            code
        }
    }
}

/// One object's ways and verdicts, as printed by `obtain <item>`.
pub fn item_report(item: &str) -> String {
    let mut out = String::new();
    let producible = obtain::producible_f2p(item);
    let merchant = obtain::merchant_buy_allowed(item);
    out.push_str(&format!(
        "{item}: obtainable={} producible={producible} merchant_buy={}\n",
        obtain::obtainable_f2p(item),
        if merchant { "allowed" } else { "forbidden" }
    ));
    let ways = obtain::ways(item);
    if ways.is_empty() {
        out.push_str(&format!("  (no F2P way: {})\n", obtain::unobtainable_reason(item).unwrap_or("unknown")));
    }
    for way in ways {
        out.push_str(&format!("  {}\n", way.describe()));
    }
    out
}

/// The F2P shop inventory with the D-A verdict per stocked object.
pub fn shops_report() -> String {
    let mut stocked: std::collections::BTreeMap<String, Vec<String>> = std::collections::BTreeMap::new();
    for shop in lifescape_gamedata::f2p::f2p_shops() {
        for stock in lifescape_gamedata::f2p::f2p_stock(&shop) {
            stocked.entry(stock.obj.clone()).or_default().push(shop.name.clone());
        }
    }
    let mut out = String::from("object\tverdict\tshops\n");
    let mut allowed = 0;
    for (item, shops) in &stocked {
        let verdict = if obtain::merchant_buy_allowed(item) {
            allowed += 1;
            "allowed"
        } else {
            "forbidden (producible in F2P)"
        };
        out.push_str(&format!("{item}\t{verdict}\t{}\n", shops.join(",")));
    }
    out.push_str(&format!("# {} objects stocked by F2P shops, {allowed} buyable from merchants\n", stocked.len()));
    out
}
