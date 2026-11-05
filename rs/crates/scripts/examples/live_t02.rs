//! Live check on an idle test account (default t02): login, ticks,
//! tables, read-only debug, one short walk, clean logout. Prints each
//! step; exits nonzero on the first failure. Not run by `cargo test`.

use lifescape_sdk::{Bot, Tables, NOTABLES};
use lifescape_scripts::tables_live::{LIVETABLES, attach_live_tables};

fn pack_and_key() -> (String, String) {
    let engine = std::env::var("LIFESCAPE_ENGINE_DIR")
        .unwrap_or_else(|_| "/home/josh/dev/lostcity/engine".to_string());
    (format!("{engine}/data/pack"), format!("{engine}/data/config/private.pem"))
}

fn main() {
    let account = std::env::args().nth(1).unwrap_or_else(|| "t02".to_string());
    let host = std::env::var("LIFESCAPE_SERVER_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port: u16 = std::env::var("LIFESCAPE_SERVER_PORT").ok().and_then(|raw| raw.parse().ok()).unwrap_or(43594);
    let (pack_dir, key_path) = pack_and_key();

    let tables: Tables = LIVETABLES;
    assert!((tables.com_id)("bank_main:inv").is_some(), "live tables miss bank_main:inv");
    assert!((tables.find_obj_by_name)("coins").is_some(), "live tables miss coins");
    assert!((NOTABLES.com_id)("bank_main:inv").is_none(), "NOTABLES must miss");

    let mut bot = Bot::new(&host, port, &pack_dir, &key_path);
    attach_live_tables(&mut bot);
    bot.login(&account, None).expect("login t02");
    println!("login ok: pos={:?} ticks={} hp?", bot.position(), bot.world.ticks);

    let before = bot.world.ticks;
    let mut pumped = 0;
    while bot.world.ticks <= before && pumped < 400 {
        bot.pump(1).expect("pump 1");
        pumped += 1;
    }
    println!("pump ok: ticks {} -> {} ({} frames)", before, bot.world.ticks, pumped);
    assert!(bot.world.ticks > before, "ticks did not advance");

    let (level, x, z) = bot.position();
    let report = bot.debug_walk(x, z, level);
    println!("debug_walk lines={}", report.lines.len());
    assert!(!report.lines.is_empty(), "debug_walk empty");

    // One short walk: try +1 east, then +1 north; first reachable wins.
    let mut walked = false;
    for (tx, tz) in [(x + 1, z), (x, z + 1), (x - 1, z), (x, z - 1)] {
        match bot.walk_to(tx, tz, level, 600) {
            Ok(_) => {
                println!("walk ok: {:?} -> {:?}", (x, z), bot.position());
                walked = bot.position() == (level, tx, tz);
                break;
            }
            Err(err) => println!("walk to ({tx},{tz}) refused: {err}"),
        }
    }
    assert!(walked, "no adjacent tile reachable");

    bot.close();
    println!("LIVE OK account={account}");
}
