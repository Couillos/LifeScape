//! Static cache against a real service: round trips counted by the
//! service's own per-op counters (the `nav-stats` numbers).

use lifescape_json::{object, str_value, Value};
use lifescape_nav::service::{PlanClient, Service, Snapshot, test_socket};

use crate::client::travel::test_nav_context;
use crate::client::Bot;

fn served_bot(socket: &std::path::Path, version: &str) -> Bot {
    let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
    bot.world.x = 3240;
    bot.world.z = 3227;
    bot.world.level = 0;
    bot.last_keepalive = Some(std::time::Instant::now());
    bot.last_send_tick = bot.world.ticks;
    bot.plan_client = Some(PlanClient::new(socket, "static-cache"));
    bot.plan_data_version = Some(version.to_string());
    bot
}

fn op_counts(client: &PlanClient) -> Vec<(String, i64)> {
    let reply = client.request(&object(vec![("op", str_value("stats"))])).expect("stats");
    ["toll_fence", "tiles_flags", "door_defs", "in_f2p", "static_locs_near", "walk_costs"]
        .iter()
        .map(|op| (op.to_string(), reply.get(op).as_i64().unwrap_or(-1)))
        .collect()
}

/// A picker loop (60 picks, the bot drifting one tile per pick around a
/// fight spot): before this cache every call was one round trip (60 per
/// op, 360 total). Now each op pays its first trip only, with answers
/// identical to the uncached service reads.
#[test]
fn picker_loop_pays_one_round_trip_per_op() {
    let (world, tables) = test_nav_context();
    let version = "static-v1";
    let service = Service::start(Snapshot::new(world, tables, version.to_string()), &test_socket("sdk-static"), 1)
        .expect("service starts");
    let client = PlanClient::new(service.socket_path(), "probe");
    let mut bot = served_bot(service.socket_path(), version);
    let mut reference = served_bot(service.socket_path(), version);
    let ring = [(3221, 3218, 0), (3223, 3218, 0), (3222, 3217, 0), (3222, 3219, 0)];
    let spots = [(3200, 3240, 0), (3204, 3243, 0), (3210, 3250, 0)];
    let before = op_counts(&client);
    let calls = 60;
    let mut gates = Vec::new();
    for pick in 0..calls {
        bot.world.x = 3240 + (pick % 7);
        bot.world.z = 3227 + (pick % 5);
        let (x, z) = (bot.world.x, bot.world.z);
        gates.push(((x, z), bot.toll_fence(x, z, 0, 150).expect("toll")));
        bot.tiles_flags(&ring).expect("ring");
        bot.door_defs(&[1530]).expect("defs");
        bot.in_f2p_batch(&spots).expect("f2p");
        bot.static_locs_near(3208, 3220, 0, 64).expect("booths");
        bot.static_locs_near(3208 + (pick % 3), 3220, 0, 2).expect("covered small query");
    }
    let after = op_counts(&client);
    let paid: Vec<(String, i64)> = before.iter().zip(&after).map(|((op, was), (_, now))| (op.clone(), now - was)).collect();
    eprintln!("COUNTS {calls} picks, round trips per op (before: {calls} each): {paid:?}");
    for (op, trips) in &paid {
        if op != "walk_costs" {
            assert!(*trips <= 1, "{op} paid {trips} round trips for {calls} picks");
        }
    }
    // Same answers as the uncached reads (raw ops, a fresh unpinned bot).
    for ((x, z), gate) in gates {
        let raw = lifescape_nav::service_queries::decode_toll_fence_reply(&raw_toll(&client, version, x, z)).expect("raw");
        assert_eq!(gate, raw, "toll at ({x},{z})");
    }
    reference.plan_data_version = None;
    assert_eq!(bot.tiles_flags(&ring).expect("ring"), reference.tiles_flags(&ring).expect("ring"));
    assert_eq!(bot.door_defs(&[1530]).expect("defs"), reference.door_defs(&[1530]).expect("defs"));
    assert_eq!(bot.in_f2p_batch(&spots).expect("f2p"), reference.in_f2p_batch(&spots).expect("f2p"));
    assert_eq!(bot.static_locs_near(3209, 3221, 0, 3).expect("locs"), reference.static_locs_near(3209, 3221, 0, 3).expect("locs"));
}

fn raw_toll(client: &PlanClient, version: &str, x: i32, z: i32) -> Value {
    client
        .request(&object(vec![
            ("op", str_value("toll_fence")),
            ("version", str_value(version)),
            ("x", Value::Int(i64::from(x))),
            ("z", Value::Int(i64::from(z))),
            ("level", Value::Int(0)),
            ("radius", Value::Int(150)),
        ]))
        .expect("raw toll")
}

/// reload-nav: the first miss learns the new version (stale refusal,
/// adopt, retry) and the cache drops every old answer; the old tile then
/// goes remote again under the new version.
#[test]
fn reload_drops_cached_answers_once_the_version_is_learned() {
    let (world, tables) = test_nav_context();
    let service = Service::start(Snapshot::new(world.clone(), tables.clone(), "old-v".to_string()), &test_socket("sdk-static-reload"), 1)
        .expect("service starts");
    let client = PlanClient::new(service.socket_path(), "probe");
    let mut bot = served_bot(service.socket_path(), "old-v");
    bot.tiles_flags(&[(3222, 3218, 0)]).expect("first read");
    let counted = |client: &PlanClient| op_counts(client).into_iter().find(|(op, _)| op == "tiles_flags").map(|(_, count)| count).unwrap_or(-1);
    let first = counted(&client);
    bot.tiles_flags(&[(3222, 3218, 0)]).expect("cached read");
    assert_eq!(counted(&client), first, "repeat answered locally");
    service.reload(Snapshot::new(world, tables, "new-v".to_string()));
    bot.tiles_flags(&[(3100, 3200, 0)]).expect("miss learns the new version");
    assert_eq!(bot.plan_data_version.as_deref(), Some("new-v"), "version adopted");
    let learned = counted(&client);
    bot.tiles_flags(&[(3222, 3218, 0)]).expect("old tile under the new version");
    assert_eq!(counted(&client), learned + 1, "old answer dropped, fetched again under new-v");
}
