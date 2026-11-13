use super::*;
use lifescape_json::{self as json, object, str_value, Value};

fn no_zone(_pos_x: i64, _pos_z: i64) -> String {
    String::new()
}

fn town(pos_x: i64, _pos_z: i64) -> String {
    if pos_x >= 3200 { "Varrock".to_string() } else { "Lumbridge".to_string() }
}

#[test]
fn signature_strips_numbers_coords_lists_and_chat() {
    assert_eq!(signature("failed: mine failed at (0, 3092, 3242)", "t01"), "failed: mine failed at (#,#,#)");
    assert_eq!(signature("failed: mine failed at (0,3092,3242)", "t01"), "failed: mine failed at (#,#,#)");
    assert_eq!(
        signature("travel: no route (3237,3282,0) -> (3252,3418,0): Disconnected closest=(3251,3420,0) blocked=[] blockers=[door 'loc_1530' loc=1530]", "x"),
        "travel: no route (#,#,#) -> (#,#,#): Disconnected closest=(#,#,#) blocked=[…] blockers=[…]"
    );
    assert_eq!(
        signature("step refused: leg to (3200, 3201) stalled; last game: You bury the bones.", "t01"),
        "step refused: leg to (#,#) stalled"
    );
    assert_eq!(signature("withdraw of -5 timed out (12 ticks)", "t01"), "withdraw of # timed out (# ticks)");
    assert_eq!(signature("bot fiona_park lost", "fiona_park"), "bot <acct> lost");
    assert_eq!(signature("flags=0x200   and\n more", ""), "flags=# and more");
    assert_eq!(signature("flags=0x2a0/0xc0", ""), "flags=#/#");
    // Two errors that differ only by numbers share a signature.
    assert_eq!(signature("item 1351 is not in the backpack at (0, 1, 2)", "t01"), signature("item 590 is not in the backpack at (1, 3200, 3300)", "t01"));
    let long = "x".repeat(400);
    assert!(signature(&long, "t01").chars().count() <= SIGNATURE_CAP + 1);
}

#[test]
fn clip_respects_char_boundaries() {
    assert_eq!(clip("abc", 5), "abc");
    assert_eq!(clip("ééé", 3), "é…");
}

#[test]
fn script_kind_and_node_from_args() {
    assert_eq!(script_kind("failed: x"), "Failed");
    assert_eq!(script_kind("usage: x"), "Usage");
    assert_eq!(script_kind("blocked, missing: hammer"), "Blocked");
    assert_eq!(script_kind("sdk: travel: nav-down"), "Sdk");
    let args: Vec<String> = ["cows", "--node", "n36", "--budget-max", "1"].iter().map(|arg| arg.to_string()).collect();
    assert_eq!(node_of_args(&args), "n36");
    assert_eq!(node_of_args(&[]), "");
}

#[test]
fn event_row_keeps_task_and_place() {
    let frame = object(vec![
        ("kind", str_value("script_error")),
        ("script", str_value("melee")),
        ("args", Value::Arr(vec![str_value("cows"), str_value("--node"), str_value("n36")])),
        ("message", str_value("failed: no cow near (0, 3250, 3270)")),
        ("at", Value::Arr(vec![Value::Int(0), Value::Int(3250), Value::Int(3270)])),
    ]);
    let row = from_event("t01", &frame, 1_000, &town).expect("script error row");
    assert_eq!(row.source, "task");
    assert_eq!(row.kind, "Failed");
    assert_eq!(row.node_id, "n36");
    assert_eq!(row.args, "cows --node n36");
    assert_eq!(row.position, Some((3250, 3270, 0)));
    assert_eq!(row.zone, "Varrock");
    assert_eq!(row.signature, "failed: no cow near (#,#,#)");
    let random = object(vec![("kind", str_value("random_event"))]);
    assert!(from_event("t01", &random, 1_000, &town).is_none());
}

#[test]
fn report_rows_only_for_failed_or_blocked() {
    let report = json::parse(
        r#"{"account":"t01","activity":"market","error":{"message":"run returned 1","type":"exit"},"missing":[],"node_id":"n33","stop":"failed"}"#,
    )
    .unwrap();
    let (outcome, row) = from_report("t01", &report, Some((3100, 3200, 0)), 5_000, &town).expect("failed report");
    assert_eq!(outcome.node_id, "n33");
    assert_eq!(outcome.since, 5_000 - FOLD_WINDOW_SECS);
    assert_eq!(row.kind, "exit");
    assert_eq!(row.script, "market");
    assert_eq!(row.method, "market");
    assert_eq!(row.zone, "Lumbridge");
    let blocked = json::parse(r#"{"account":"t01","activity":"smith","error":null,"missing":["hammer"],"node_id":"n15","stop":"blocked"}"#).unwrap();
    let (_, row) = from_report("t01", &blocked, None, 5_000, &no_zone).expect("blocked report");
    assert_eq!(row.kind, "Blocked");
    assert_eq!(row.message, "blocked, missing: hammer");
    assert_eq!(row.detail, "missing hammer");
    assert!(row.position.is_none());
    let done = json::parse(r#"{"account":"t01","activity":"smith","stop":"goal_reached"}"#).unwrap();
    assert!(from_report("t01", &done, None, 5_000, &no_zone).is_none());
}

#[test]
fn dossier_row_has_from_dest_and_ref() {
    let dossier = json::parse(
        r#"{"account":"fiona_park","dest":{"level":0,"x":3252,"z":3418},"from":{"level":0,"x":3237,"z":3282},
            "error":{"kind":"NoRoute","expected":"route","observed":"travel: no route (3237,3282,0) -> (3252,3418,0): Disconnected"}}"#,
    )
    .unwrap();
    let row = from_dossier("fiona_park-39560.json", 7_000, &dossier, &town).expect("dossier row");
    assert_eq!(row.source, "nav");
    assert_eq!(row.kind, "NoRoute");
    assert_eq!(row.position, Some((3237, 3282, 0)));
    assert_eq!(row.detail, "dest (3252,3418,0) Varrock");
    assert_eq!(row.dossier, "fiona_park-39560.json");
    assert_eq!(row.source_ref.as_deref(), Some("nav:fiona_park-39560.json:7000"));
    assert_eq!(row.signature, "travel: no route (#,#,#) -> (#,#,#): Disconnected");
    assert!(from_dossier("x.json", 1, &object(vec![]), &town).is_none(), "no kind, no row");
}

#[test]
fn deal_row_groups_by_reason_and_item() {
    let deal = FailedDeal {
        deal_id: 42,
        buyer: "t01".to_string(),
        seller: "t02".to_string(),
        item_id: 440,
        item_name: "Iron ore".to_string(),
        quantity: 5,
        price: 30,
        meeting: (3213, 3429, 0),
        reason: "daemon restart".to_string(),
        clock: 9_000,
    };
    let row = from_deal(&deal, &town);
    assert_eq!(row.kind, "daemon_restart");
    assert_eq!(row.signature, "deal daemon_restart: Iron ore");
    assert_eq!(row.peer, "t02");
    assert_eq!(row.zone, "Varrock");
    assert_eq!(row.source_ref.as_deref(), Some("deal:42"));
}

#[test]
fn brain_lines_parse_abandon_stuck_and_ban() {
    let abandon = from_brain_line(
        r#"brain bct5rxpg: n33 Failure("exit: run returned 1") -> abandoned (failures 3/3 blocks 0) exit: run returned 1"#,
        10,
    )
    .expect("abandon");
    assert_eq!(abandon.kind, "abandoned");
    assert_eq!(abandon.node_id, "n33");
    assert_eq!(abandon.account, "bct5rxpg");
    assert_eq!(abandon.message, "exit: run returned 1");
    assert_eq!(abandon.signature, "exit: run returned #");
    let stuck = from_brain_line("brain will_field: graph stuck, nothing left to run; abandoned: n1=Blocked: missing bronze_arrow", 10).expect("stuck");
    assert_eq!(stuck.kind, "graph_stuck");
    assert_eq!(stuck.signature, "graph stuck, nothing left to run");
    let ban = from_brain_line("brain t05: skill fishing banned 6000 ticks: failed: no fishing water in range of (0, 3200, 3300) (level 1)", 10).expect("ban");
    assert_eq!(ban.kind, "skill_banned");
    assert_eq!(ban.script, "fishing");
    assert_eq!(ban.signature, "skill fishing banned: failed: no fishing water in range of (#,#,#) (level #)");
    let goal = from_brain_line("brain harry_page: goal stuck -> next goal defence 45", 10).expect("goal");
    assert_eq!(goal.kind, "goal_stuck");
    assert!(from_brain_line("brain t05: n12 Failure(\"x\") -> ready (failures 1/3 blocks 0) x", 10).is_none(), "retries are not abandons");
    assert!(from_brain_line("brain t05: normalize n31 abandoned: dependency n33 abandoned", 10).is_none(), "cascades skipped");
    assert!(from_brain_line("brain t05: folded report stop=failed", 10).is_none());
    assert!(from_brain_line("job t05: event x", 10).is_none());
}

#[test]
fn utc_text_formats_civil_time() {
    assert_eq!(utc_text(0), "1970-01-01 00:00:00");
    assert_eq!(utc_text(1_794_565_628), "2026-11-13 10:27:08");
    assert_eq!(utc_text(951_782_400), "2000-02-29 00:00:00");
}
