//! Temporary probe: bisect the ~11s server kill. Modes:
//!   idle  - login, pump only (keepalives), 40s
//!   one   - login, one short walk, then pump 30s
//!   far   - login, one far walk (chained 24-step legs like woodcut), pump
//! Prints tick + wall time of death. Delete after diagnosis.
use lifescape_sdk::Bot;
use lifescape_scripts::tables_live::attach_live_tables;
use std::time::Instant;

fn pack_and_key() -> (String, String) {
    let engine = std::env::var("LIFESCAPE_ENGINE_DIR")
        .unwrap_or_else(|_| "/home/josh/dev/lostcity/engine".to_string());
    (format!("{engine}/data/pack"), format!("{engine}/data/config/private.pem"))
}

fn main() {
    let account = std::env::args().nth(1).unwrap_or_else(|| "admin".to_string());
    let mode = std::env::args().nth(2).unwrap_or_else(|| "idle".to_string());
    let host = std::env::var("LIFESCAPE_SERVER_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port: u16 = std::env::var("LIFESCAPE_SERVER_PORT").ok().and_then(|raw| raw.parse().ok()).unwrap_or(43594);
    let (pack_dir, key_path) = pack_and_key();
    let mut bot = Bot::new(&host, port, &pack_dir, &key_path);
    attach_live_tables(&mut bot);
    let password = if account == "admin" { Some("admin") } else { None };
    bot.login(&account, password).expect("login");
    let start = Instant::now();
    println!("login ok pos={:?} t=0", bot.position());
    if mode == "script" {
        bot.intend("woodcut probe");
        let args = vec!["1".to_string()];
        match lifescape_scripts::run("woodcut", &mut bot, &args) {
            Ok(_) => println!("script returned t={:.1}s", start.elapsed().as_secs_f32()),
            Err(err) => println!("SCRIPT DIED t={:.1}s: {err}", start.elapsed().as_secs_f32()),
        }
        bot.close();
        return;
    }
    if mode == "one" || mode == "far" || mode == "two" {
        let (level, x, z) = bot.position();
        let (tx, tz) = if mode == "one" { (x + 2, z) } else { (x + 30, z + 10) };
        println!("walk_to ({tx},{tz})");
        match bot.walk_to(tx, tz, level, 1500) {
            Ok(_) => println!("walk done pos={:?} t={:.1}s", bot.position(), start.elapsed().as_secs_f32()),
            Err(err) => println!("walk ended t={:.1}s: {err}", start.elapsed().as_secs_f32()),
        }
    }
    if mode == "two" {
        // Two back-to-back MOVEs like the dying sequence: walk one step,
        // then immediately replan from (almost) the same start.
        let (level, x, z) = bot.position();
        println!("two-move burst from ({x},{z})");
        match bot.walk_to(x + 1, z, level, 60) {
            Ok(_) => println!("leg1 ok pos={:?}", bot.position()),
            Err(err) => println!("leg1: {err}"),
        }
        match bot.walk_to(x + 24, z + 9, level, 1500) {
            Ok(_) => println!("leg2 ok pos={:?}", bot.position()),
            Err(err) => println!("leg2 t={:.1}s: {err}", start.elapsed().as_secs_f32()),
        }
    }
    if mode == "long" {
        // Script-like activity without any worker: shuttle walks + pumps
        // for ~90 s. If this survives, the killer lives in the worker.
        let (level, home_x, home_z) = bot.position();
        let t0 = Instant::now();
        let mut rounds = 0;
        while t0.elapsed().as_secs() < 85 {
            let (.., x, z) = (0, bot.world.x, bot.world.z);
            let (tx, tz) = if (x - home_x).abs() + (z - home_z).abs() < 5 {
                (home_x + 12, home_z + 6)
            } else {
                (home_x, home_z)
            };
            match bot.walk_to(tx, tz, level, 400) {
                Ok(_) => {}
                Err(err) => println!("round {rounds} walk: {err}"),
            }
            rounds += 1;
            match bot.pump(30) {
                Ok(_) => {}
                Err(err) => {
                    println!("PUMP DIED round={rounds} t={:.1}s: {err}", start.elapsed().as_secs_f32());
                    bot.close();
                    return;
                }
            }
        }
        println!("LONG SURVIVED rounds={rounds} t={:.1}s", start.elapsed().as_secs_f32());
    }
    if mode == "moveonce" {
        // One hunt-style MOVE east, then pure timestamped listening 20 s:
        // does the server go silent right after this exact packet?
        bot.pump(30).expect("settle");
        let (level, x, z) = bot.position();
        println!("moveonce from ({x},{z})");
        let _ = bot.walk_to(x + 23, z + 9, level, 1500);
        let t0 = Instant::now();
        let mut last_seen = Instant::now();
        let mut worst = 0.0;
        while t0.elapsed().as_secs() < 20 {
            match bot.pump(1) {
                Ok(_) => {
                    let gap = last_seen.elapsed().as_secs_f32();
                    if gap > worst {
                        worst = gap;
                        println!("gap {gap:.1}s ends t={:.1}s", t0.elapsed().as_secs_f32());
                    }
                    last_seen = Instant::now();
                }
                Err(err) => {
                    println!("PUMP DIED worst_gap={worst:.1}s t={:.1}s: {err}", start.elapsed().as_secs_f32());
                    bot.close();
                    return;
                }
            }
        }
        let elapsed = start.elapsed().as_secs_f32();
        println!("MOVEONCE SURVIVED worst_gap={worst:.1}s t={elapsed:.1}s");
    }
    let budget = if mode == "idle" { 400 } else { 300 };
    for _ in 0..budget {
        match bot.pump(1) {
            Ok(_) => {}
            Err(err) => {
                println!("PUMP DIED tick={} t={:.1}s: {err}", bot.world.ticks, start.elapsed().as_secs_f32());
                bot.close();
                return;
            }
        }
    }
    println!("SURVIVED ticks={} t={:.1}s", bot.world.ticks, start.elapsed().as_secs_f32());
    bot.close();
}
