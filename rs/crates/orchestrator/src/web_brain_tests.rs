//! Brain view shape (with and without the ambition contract) and the
//! version rule, from fixture life files.

use super::{brain_payload, brain_response_in, brain_version, filed_subtasks, goal_entry_kind, BrainSources};
use lifescape_json::{self as json, object, str_value, Value};
use lifescape_planner::{
    life_to_value, Alternative, Archetype, GoalKind, HistoryEntry, Life, Node, NodeKind, NodeStatus, Profile,
};

fn obj_id(name: &str) -> i64 {
    lifescape_gamedata::objs_by_name(name).first().map(|obj| obj.obj_id as i64).expect("fixture object exists")
}

fn fixture_life() -> Life {
    let profile = Profile::new(false, 0.6, 0.4, 42, Archetype::Producer);
    let mut reach10 = Node::new("n1", NodeKind::ReachLevel { skill: "fishing".to_string(), level: 10 }, Vec::new(), 10);
    reach10.status = NodeStatus::Done;
    reach10.progress_current = 10;
    let mut reach20 =
        Node::new("n2", NodeKind::ReachLevel { skill: "fishing".to_string(), level: 20 }, vec!["n1".to_string(), "n3".to_string()], 20);
    reach20.status = NodeStatus::Pending;
    reach20.blocks = 1;
    reach20.last_error = "Blocked: missing net".to_string();
    reach20.method = Some("gather:fishing/raw_sardine".to_string());
    reach20.alternatives = vec![
        Alternative { method: "gather:fishing/raw_sardine".to_string(), est_ticks: 9000, est_coins: 0 },
        Alternative { method: "gather:fishing/raw_shrimp".to_string(), est_ticks: 12000, est_coins: 0 },
    ];
    let mut obtain = Node::new("n3", NodeKind::ObtainItems { item: "net".to_string(), count: 1 }, Vec::new(), 1);
    obtain.status = NodeStatus::Blocked { reason: "missing:coins:5".to_string() };
    obtain.method = Some("market:buy".to_string());
    obtain.alternatives = vec![Alternative { method: "market:buy".to_string(), est_ticks: 400, est_coins: 5 }];
    obtain.failures = 2;
    Life {
        schema: 1,
        account: "t42".to_string(),
        created_at: 1_000,
        seed: 42,
        profile,
        goal: GoalKind::MaxSkills { skills: vec!["fishing".to_string()], level: 20 },
        nodes: vec![reach10, reach20, obtain],
        engagement: None,
        active: None,
        history: vec![
            HistoryEntry { node: "goal".to_string(), started: 1_000, ended: 1_000, outcome: "stuck:mining@6000:3 failures (node n7, Sdk: x)".to_string() },
            HistoryEntry { node: "goal".to_string(), started: 1_001, ended: 1_001, outcome: "next:fishing 20".to_string() },
            HistoryEntry { node: "n1".to_string(), started: 1_002, ended: 1_003, outcome: "done".to_string() },
        ],
        ambition: None,
        goal_origin: None,
    }
}

fn ambition() -> Value {
    json::parse(
        r#"{"kind":"skiller","label":"Master fisher","groups":["gathering"],"why":"seed draw",
            "milestones":[
              {"id":"m1","label":"fishing 10","kind":"level","target":10,"status":"done","progress":[10,10],"blocked_count":0},
              {"id":"m2","label":"fishing 20","kind":"level","target":20,"status":"active","progress":[10,20],"blocked_count":1}],
            "drawn_at_tick":120,"revisions":[]}"#,
    )
    .expect("ambition fixture parses")
}

fn with_ambition(life: &Life) -> Value {
    let mut value = life_to_value(life);
    if let Value::Obj(map) = &mut value {
        map.insert("ambition".to_string(), ambition());
        map.insert("goal_origin".to_string(), str_value("m2"));
    }
    value
}

fn sources() -> BrainSources {
    let net = obj_id("net");
    let pickaxe = obj_id("iron_pickaxe");
    BrainSources {
        live: object(vec![
            ("skills", Value::Arr(vec![object(vec![("id", Value::Int(10)), ("xp", Value::Int(1_154)), ("level", Value::Int(10))])])),
            ("combat", Value::Int(7)),
            ("ticks", Value::Int(50_000)),
            ("inv", Value::Arr(vec![object(vec![("id", Value::Int(obj_id("coins"))), ("name", str_value("Coins")), ("count", Value::Int(250))])])),
            ("bank", Value::Arr(vec![object(vec![("id", Value::Int(obj_id("coins"))), ("name", str_value("Coins")), ("count", Value::Int(50))])])),
            ("worn", Value::Arr(vec![object(vec![("id", Value::Int(obj_id("bronze_sword"))), ("count", Value::Int(1))])])),
        ]),
        orders: vec![
            object(vec![
                ("order", Value::Int(7)),
                ("side", str_value("buy")),
                ("item", Value::Int(net)),
                ("qty", Value::Int(1)),
                ("left", Value::Int(1)),
                ("price", Value::Int(5)),
                ("status", str_value("open")),
            ]),
            object(vec![
                ("order", Value::Int(8)),
                ("side", str_value("buy")),
                ("item", Value::Int(pickaxe)),
                ("qty", Value::Int(1)),
                ("left", Value::Int(1)),
                ("price", Value::Int(90)),
                ("status", str_value("open")),
            ]),
        ],
        deals: vec![object(vec![
            ("deal", Value::Int(3)),
            ("buyer", str_value("t42")),
            ("seller", str_value("t07")),
            ("item", Value::Int(net)),
            ("qty", Value::Int(1)),
            ("price", Value::Int(5)),
            ("status", str_value("convened")),
        ])],
        wishes: vec![object(vec![("item", str_value("iron_pickaxe")), ("slot", str_value("pickaxe")), ("gain", Value::Int(10))])],
        ceilings: vec![(8, 140)],
        errors: Vec::new(),
        life_mtime_ms: 1_700_000_000_000,
    }
}

fn ticks_100(_stamp: i64) -> u64 {
    100
}

fn node<'a>(payload: &'a Value, id: &str) -> &'a Value {
    payload.get("nodes").as_arr().and_then(|rows| rows.iter().find(|row| row.get("id").as_str() == Some(id))).expect("node present")
}

#[test]
fn shape_without_ambition() {
    let payload = brain_payload("t42", &life_to_value(&fixture_life()), &sources(), &ticks_100);
    assert_eq!(payload.get("ok").as_bool(), Some(true), "{}", json::stringify(&payload));
    assert_eq!(payload.get("account").as_str(), Some("t42"));
    assert!(payload.get("ambition").is_null(), "no ambition = null");
    assert_eq!(payload.get("goal").get("label").as_str(), Some("fishing 20"));
    assert!(payload.get("goal").get("origin_milestone").is_null());
    assert!(payload.get("version").as_str().is_some_and(|version| version.starts_with("1700000000000-")));

    let reach = node(&payload, "n2");
    for key in ["id", "kind", "label", "status", "progress", "deps", "method", "alternatives", "block", "last_error", "failures", "blocks", "revives", "filed"] {
        assert!(reach.as_obj().is_some_and(|map| map.contains_key(key)), "node key {key}");
    }
    assert_eq!(reach.get("label").as_str(), Some("fishing 20"));
    assert_eq!(json::stringify(&reach.get("progress")), "[0, 20]");
    assert_eq!(json::stringify(&reach.get("deps")), r#"["n1", "n3"]"#);
    assert_eq!(json::stringify(&reach.get("filed")), r#"["n3"]"#, "the obtain it filed when blocked");
    let chosen: Vec<bool> = reach.get("alternatives").as_arr().unwrap_or(&Vec::new()).iter().filter_map(|alt| alt.get("chosen").as_bool()).collect();
    assert_eq!(chosen, vec![true, false]);
    let obtain = node(&payload, "n3");
    assert_eq!(obtain.get("status").as_str(), Some("blocked"));
    assert_eq!(obtain.get("block").as_str(), Some("missing:coins:5"));
    assert_eq!(obtain.get("item").as_str(), Some("net"));
    assert_eq!(obtain.get("failures").as_i64(), Some(2));
    assert!(node(&payload, "n1").get("block").is_null());

    let orders = payload.get("orders").as_arr().cloned().unwrap_or_default();
    assert_eq!(orders.len(), 2);
    assert_eq!(orders[0].get("item").as_str(), Some("net"), "orders carry the debug name for matching");
    assert_eq!(orders[0].get("remaining").as_i64(), Some(1));
    assert!(orders[0].get("ceiling").is_null(), "fixed-price order");
    assert_eq!(orders[1].get("ceiling").as_i64(), Some(140));
    let deal = &payload.get("deals").as_arr().cloned().unwrap_or_default()[0];
    assert_eq!((deal.get("role").as_str(), deal.get("partner").as_str()), (Some("buyer"), Some("t07")));
    assert_eq!(deal.get("item").as_str(), Some("net"));
    let wish = &payload.get("wishes").as_arr().cloned().unwrap_or_default()[0];
    assert_eq!(wish.get("item").as_str(), Some("iron_pickaxe"));
    assert_eq!(wish.get("bid").get("price").as_i64(), Some(90));
    assert_eq!(wish.get("bid").get("ceiling").as_i64(), Some(140));

    let live = payload.get("live");
    assert_eq!(live.get("levels").get("fishing").as_i64(), Some(10));
    assert_eq!(live.get("combat").as_i64(), Some(7));
    assert_eq!(live.get("coins").as_i64(), Some(300), "pack + bank");
    assert_eq!(json::stringify(&live.get("worn")), r#"["bronze_sword"]"#);

    let kinds: Vec<&str> = payload.get("goal_history").as_arr().map(|rows| rows.iter().filter_map(|row| row.get("kind").as_str()).collect()).unwrap_or_default();
    assert_eq!(kinds, vec!["stuck", "next"], "goal entries only");
    let ban = &payload.get("banned_skills").as_arr().cloned().unwrap_or_default()[0];
    assert_eq!(ban.get("skill").as_str(), Some("mining"));
    assert_eq!(ban.get("active").as_bool(), Some(true));
    assert_eq!(ban.get("ticks_left").as_i64(), Some(5900));
    assert_eq!(ban.get("until_tick").as_i64(), Some(55_900));
    assert_eq!(ban.get("cause").as_str(), Some("3 failures (node n7, Sdk: x)"));
}

#[test]
fn shape_with_ambition_passes_the_contract_through() {
    let payload = brain_payload("t42", &with_ambition(&fixture_life()), &sources(), &ticks_100);
    assert_eq!(payload.get("ok").as_bool(), Some(true));
    assert_eq!(json::stringify(&payload.get("ambition")), json::stringify(&ambition()));
    assert_eq!(payload.get("goal").get("origin_milestone").as_str(), Some("m2"));
    let milestones = payload.get("ambition").get("milestones").as_arr().cloned().unwrap_or_default();
    assert_eq!(milestones.len(), 2);
    let plain = brain_payload("t42", &life_to_value(&fixture_life()), &sources(), &ticks_100);
    assert_ne!(payload.get("version"), plain.get("version"), "an ambition change redraws");
}

#[test]
fn unreadable_life_is_a_loud_error() {
    let mut value = life_to_value(&fixture_life());
    if let Value::Obj(map) = &mut value {
        map.insert("schema".to_string(), Value::Int(99));
    }
    let payload = brain_payload("t42", &value, &sources(), &ticks_100);
    assert_eq!(payload.get("ok").as_bool(), Some(false));
    assert!(payload.get("error").as_str().is_some_and(|error| error.contains("unreadable")));
}

#[test]
fn version_moves_only_with_shown_content() {
    let life = life_to_value(&fixture_life());
    let base = brain_payload("t42", &life, &sources(), &ticks_100);
    let again = brain_payload("t42", &life, &sources(), &ticks_100);
    assert_eq!(base.get("version"), again.get("version"), "same inputs, same version");

    let later = brain_payload("t42", &life, &sources(), &|_stamp| 900);
    assert_eq!(base.get("version"), later.get("version"), "ban countdown alone never redraws");
    let mut ticking = sources();
    if let Value::Obj(map) = &mut ticking.live {
        map.insert("ticks".to_string(), Value::Int(60_000));
    }
    assert_eq!(base.get("version"), brain_payload("t42", &life, &ticking, &ticks_100).get("version"), "bot tick alone never redraws");

    let mut touched = sources();
    touched.life_mtime_ms += 1;
    assert_ne!(base.get("version"), brain_payload("t42", &life, &touched, &ticks_100).get("version"), "life rewritten");
    let mut richer = sources();
    if let Value::Obj(map) = &mut richer.live {
        map.insert("bank".to_string(), Value::Arr(Vec::new()));
    }
    assert_ne!(base.get("version"), brain_payload("t42", &life, &richer, &ticks_100).get("version"), "coins moved");
    let mut moved = fixture_life();
    moved.nodes[1].status = NodeStatus::Active;
    moved.active = Some("n2".to_string());
    assert_ne!(base.get("version"), brain_payload("t42", &life_to_value(&moved), &sources(), &ticks_100).get("version"), "status moved");
    let expired = brain_payload("t42", &life, &sources(), &|_stamp| 7000);
    assert_ne!(base.get("version"), expired.get("version"), "a ban ending redraws");
    assert_eq!(brain_version(5, &base).split('-').next(), Some("5"));
}

#[test]
fn filed_subtasks_are_younger_deps_of_a_blocked_node() {
    let life = fixture_life();
    assert_eq!(filed_subtasks(&life.nodes[1]), vec!["n3".to_string()]);
    assert!(filed_subtasks(&life.nodes[0]).is_empty(), "never blocked");
    let mut skeleton = Node::new("n5", NodeKind::MakeMoney, vec!["n2".to_string()], 0);
    skeleton.blocks = 2;
    assert!(filed_subtasks(&skeleton).is_empty(), "older deps are the skeleton");
    assert_eq!(goal_entry_kind("unban:mining"), "unban");
    assert_eq!(goal_entry_kind("done:fishing"), "done");
}

fn scratch(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("lifescape-brain-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("lives")).expect("scratch lives dir");
    dir
}

#[test]
fn response_reads_the_file_and_the_sources() {
    let dir = scratch("route");
    std::fs::write(dir.join("lives").join("t42.json"), json::stringify(&with_ambition(&fixture_life()))).expect("write fixture");
    let fixture = sources();
    let orders = fixture.orders.clone();
    let dispatch = move |request: Value| -> Value {
        match (request.get("cmd").as_str(), request.get("op").as_str()) {
            (Some("live"), _) => object(vec![("ok", Value::Bool(true)), ("state", sources().live)]),
            (Some("market"), Some("my_orders")) => object(vec![("ok", Value::Bool(true)), ("orders", Value::Arr(orders.clone()))]),
            (Some("market"), Some("my_deals")) => object(vec![("ok", Value::Bool(false)), ("error", str_value("market service down"))]),
            (Some("brain_wishes"), _) => object(vec![("ok", Value::Bool(true)), ("wishes", Value::Arr(Vec::new()))]),
            _ => object(vec![("ok", Value::Bool(false)), ("error", str_value("unexpected"))]),
        }
    };
    let payload = brain_response_in(&dir, "t42", &dispatch, &ticks_100);
    assert_eq!(payload.get("ok").as_bool(), Some(true), "{}", json::stringify(&payload));
    assert_eq!(payload.get("ambition").get("label").as_str(), Some("Master fisher"));
    assert_eq!(payload.get("orders").as_arr().map(Vec::len), Some(2));
    assert_eq!(json::stringify(&payload.get("errors")), r#"["deals: market service down"]"#, "a down source is shown, never silently empty");
    assert_eq!(brain_response_in(&dir, "nobody", &dispatch, &ticks_100).get("ok").as_bool(), Some(false));
    assert_eq!(brain_response_in(&dir, "../x", &dispatch, &ticks_100).get("ok").as_bool(), Some(false));
    let _ = std::fs::remove_dir_all(&dir);
}
