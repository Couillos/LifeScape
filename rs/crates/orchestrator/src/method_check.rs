//! `orchestrator check-method <staff> <method> [count]` and
//! `check-method <staff> --list <file>`: one planner method run live,
//! exactly as the brain launches it, on a staff account (user
//! 2026-11-15: "absolument tout doit être testé en live et
//! fonctionnel"; audit: 0 % families nobody noticed).
//!
//! For each method the planner itself answers what the brain would
//! send (`mapping::map_method` on an obtain node of the product), what
//! the way consumes and needs (`method_way`, `tools::required_tool`).
//! The staff account is staged with cheats that stay on staff (AGENTS.md
//! rule 13): the skill at the way's level, the inputs for `count`
//! actions, the tools, travel coins. The run is watched to its end and
//! judged on the bank + pack + worn count of the product, never the
//! exit code alone. One JSON line per check lands in
//! `run/method_checks.jsonl` (the method health registry reads it).

#![forbid(unsafe_code)]

use std::time::Duration;

use lifescape_json::{self as json, object, str_value, Value};

use crate::await_cmd::{wait_for_end, Baseline, DaemonSource, StateSource, Verdict, WaitEnd, POLL_MS};

/// Wall seconds one check may take.
const CHECK_TIMEOUT_S: f64 = 900.0;
/// Wallet the site pick budgets fares against (`Fare::wallet`); the
/// stage gives only the fares the picked site needs (`MethodPlan::fare`).
const TRAVEL_COINS: i64 = 2_000;
/// Bait or feathers staged per wanted fish (`stage`).
const FISHING_BAIT_FACTOR: i64 = 10;
/// Budget of a drop-farm check (ticks): hunts, loots and banks for this
/// long, the drop counted only when it lands.
const DROP_FARM_TICKS: u64 = 1_500;

/// What the planner says a method needs and launches.
#[derive(Debug, Clone, PartialEq)]
pub struct MethodPlan {
    pub method: String,
    /// A drop farm (no recipe): judged on a clean bounded run, the drop
    /// itself may be rare (an air talisman: 1 goblin in 128).
    pub drop_farm: bool,
    /// F2P quest directory the way needs (`quest_runemysteries`).
    pub quest: Option<String>,
    pub script: String,
    pub args: Vec<String>,
    pub product: String,
    pub skill: Option<(String, u8)>,
    pub inputs: Vec<(String, i64)>,
    pub tools: Vec<String>,
    pub coins: i64,
    /// Travel coins the picked site needs (toll, boat), 0 for a free one.
    pub fare: i64,
}

/// The planner's view of `method` for `count` units (pure). A spell
/// method yielding no object (teleport, curse, magic fight: D18) is
/// launched as a magic climb of `count` casts and judged on its XP.
pub fn plan(method: &str, count: i64) -> Result<MethodPlan, String> {
    // `leg@product`: the object the leg obtains (`all_methods` writes it:
    // a drop farm or a purchase names no recipe the planner can read).
    let (leg, named) = match method.split_once('@') {
        Some((leg, product)) => (leg, Some(product.to_string())),
        None => (method, None),
    };
    let way = lifescape_planner::method_way::method_way(leg).or_else(|| lifescape_planner::spell_methods::cast_method_way(leg));
    let product = match (&named, &way) {
        (Some(product), _) => product.clone(),
        (None, Some((product, _))) => product.clone(),
        (None, None) => return Err(format!("{method}: no way known to the planner (name the object: {method}@<object>)")),
    };
    let mut node = if product.is_empty() {
        let skill = way.as_ref().and_then(|(_, found)| found.skill.clone()).map(|(name, _)| name).unwrap_or_else(|| "magic".to_string());
        let mut climb = lifescape_planner::Node::new("check", lifescape_planner::NodeKind::ReachLevel { skill, level: 99 }, Vec::new(), 99);
        climb.batch = count;
        climb
    } else {
        lifescape_planner::Node::new("check", lifescape_planner::NodeKind::ObtainItems { item: product.clone(), count }, Vec::new(), count)
    };
    // The site the brain would send a staff checker to (combat 70, the
    // recipe's level, fares in hand): a mithril rock, never the nearest
    // iron (live 2026-11-15: the check mined iron for `farm:mithril_ore`).
    let skill_level = way.as_ref().and_then(|(_, found)| found.skill.clone());
    let mut levels: Vec<(String, u8)> = ["attack", "strength", "defence", "hitpoints"].iter().map(|stat| (stat.to_string(), STAFF_COMBAT as u8)).collect();
    if let Some((skill, level)) = &skill_level {
        levels.push((skill.clone(), u8::try_from((*level).clamp(1, 99)).unwrap_or(99)));
    }
    let pass = lifescape_planner::combat::Pass { levels, held: Vec::new(), quest_points: 0 };
    let fare = lifescape_planner::combat::Fare { wallet: TRAVEL_COINS, free_pass: false };
    match lifescape_planner::combat::pick_site_gated(leg, STAFF_COMBAT as u8, Some(STAFF_HOME), fare, &pass) {
        lifescape_planner::combat::SitePick::At(site) => node.site = Some(site),
        lifescape_planner::combat::SitePick::Tolled(site) => {
            node.site = Some(site);
            node.toll = true;
        }
        lifescape_planner::combat::SitePick::Boat { site, toll } => {
            node.site = Some(site);
            node.boat = true;
            node.toll = toll;
        }
        // The brain never launches a sited method without its site (the
        // check ran blurite and runite from home, live 2026-11-16).
        lifescape_planner::combat::SitePick::Needs(level) => return Err(format!("{method}: no site a staff bot works safely (combat {level} needed)")),
        lifescape_planner::combat::SitePick::Access(area, key) => return Err(format!("{method}: every site is behind access:{area} (key {key:?})")),
        lifescape_planner::combat::SitePick::Fare(coins) => return Err(format!("{method}: the fares need {coins} coins")),
        lifescape_planner::combat::SitePick::Anywhere => {}
    }
    // A purchase: the brain's shop (`shop_site::pick_shop`: quests, gates,
    // combat, the fare), never the script's own nearest pick.
    if leg == "shop:buy" {
        let mut ctx = lifescape_planner::expand::ReadyCtx::blind(0.5);
        ctx.combat = STAFF_COMBAT as u8;
        ctx.position = Some(STAFF_HOME);
        ctx.coins = TRAVEL_COINS;
        ctx.levels = pass.levels.clone();
        let (_, site) = lifescape_planner::shop_site::pick_shop(&product, &ctx).ok_or_else(|| format!("{method}: no shop a combat-70 bot without quests reaches"))?;
        node.site = Some(site);
        node.toll = lifescape_gamedata::toll::crossing(STAFF_HOME, site);
        node.boat = lifescape_gamedata::access::behind_boat(site.0, site.1, site.2);
    }
    let (script, args) = lifescape_planner::mapping::map_method(leg, &node, None).ok_or_else(|| format!("{method}: no script maps it"))?;
    let mut tools: Vec<String> = way.as_ref().map(|(_, found)| found.tools.clone()).unwrap_or_default();
    if let Some(need) = lifescape_planner::tools::required_tool(leg) {
        for name in need.names() {
            if !tools.contains(&name) {
                tools.push(name);
            }
        }
    }
    let skill = way.as_ref().and_then(|(_, found)| found.skill.clone()).map(|(name, level)| (name, u8::try_from(level.clamp(1, 99)).unwrap_or(99)));
    let inputs = way.as_ref().map(|(_, found)| found.inputs.clone()).unwrap_or_default();
    let coins = match &way {
        Some((_, found)) => found.coins,
        None => lifescape_gamedata::obtain::merchant_price(&product).unwrap_or(0),
    };
    let quest = way.as_ref().and_then(|(_, found)| found.quest.clone());
    let drop_farm = leg.starts_with("farm:") && script == "melee";
    let mut args = args;
    if drop_farm {
        args.extend(["--budget-min".to_string(), "0".to_string(), "--budget-max".to_string(), DROP_FARM_TICKS.to_string()]);
    }
    // The fine method rides to the task report like a brain launch.
    args.extend([crate::report_meta::METHOD_FLAG.to_string(), leg.to_string()]);
    // Travel coins only for a trip that costs money (user 2026-11-17):
    // the gate one way, the boat out and back, nothing for a free site.
    let fare = if node.toll { lifescape_gamedata::toll::FARE } else { 0 } + if node.boat { 2 * lifescape_gamedata::services::BOAT_FARE_GP } else { 0 };
    Ok(MethodPlan { method: method.to_string(), drop_farm, quest, script, args, product, skill, inputs, tools, coins, fare })
}

/// Units of `item` the state holds: pack, bank and worn.
pub fn held(state: &Value, item: &str) -> i64 {
    let ids: Vec<i64> = lifescape_gamedata::objs_by_name(item).into_iter().map(|obj| obj.obj_id as i64).collect();
    ["inv", "bank", "worn"]
        .iter()
        .flat_map(|key| state.get(key).as_arr().cloned().unwrap_or_default())
        .filter(|row| row.get("id").as_i64().is_some_and(|id| ids.contains(&id)))
        .map(|row| row.get("count").as_i64().unwrap_or(0))
        .sum()
}

fn level_of(state: &Value, skill: &str) -> i64 {
    i64::from(crate::brain_succession::live_level(state, skill))
}

/// Display XP of `skill` in a session state (`skills` rows).
pub fn skill_xp(state: &Value, skill: &str) -> i64 {
    let Some(stat) = lifescape_sdk::provenance::skill_id(skill) else { return 0 };
    state.get("skills").as_arr().cloned().unwrap_or_default().iter().find(|row| row.get("id").as_i64() == Some(i64::from(stat))).and_then(|row| row.get("xp").as_i64()).unwrap_or(0)
}

/// What a check counts: the product held, or the XP of its skill for a
/// method yielding no object (teleports, curses, magic fights).
fn measure(state: &Value, plan: &MethodPlan) -> i64 {
    if plan.product.is_empty() {
        return skill_xp(state, plan.skill.as_ref().map(|(skill, _)| skill.as_str()).unwrap_or("magic"));
    }
    held(state, &plan.product)
}

fn send(request: Value, timeout: Duration) -> Result<Value, String> {
    let reply = crate::cli::send_command(&request, timeout).map_err(|error| format!("daemon unreachable: {error}"))?;
    if reply.get("ok").as_bool() == Some(true) {
        Ok(reply)
    } else {
        Err(json::stringify(reply.get("error")))
    }
}

fn cheat(account: &str, command: &str) -> Result<(), String> {
    send(
        object(vec![
            ("cmd", str_value("call")),
            ("account", str_value(account)),
            ("method", str_value("cheat")),
            ("args", Value::Arr(vec![str_value(command)])),
        ]),
        Duration::from_secs(crate::daemon::WORKER_TIMEOUT),
    )
    .map(|_| ())
    .map_err(|error| format!("cheat {command:?}: {error}"))
}

fn show(account: &str) -> Result<Value, String> {
    DaemonSource { account: account.to_string() }.show().map_err(|error| format!("show {account}: {error:?}"))
}

/// Tools every staff checker carries between checks (kept by
/// `check_dispose`): the permanent kit of every trade, so one check
/// never fails on another's missing tool.
pub const STAFF_KIT: [&str; 15] = [
    "bronze_pickaxe", "bronze_axe", "net", "fishing_rod", "fly_fishing_rod", "harpoon", "lobster_pot", "hammer", "tinderbox", "chisel", "needle", "shears", "ring_mould",
    "amulet_mould", "necklace_mould",
];
/// Where a staff checker stands between checks (Draynor bank).
const STAFF_HOME: (i64, i64, i64) = (3092, 3245, 0);
/// Bank types past which the checker empties its bank first (F2P: 60).
const BANK_DISPOSE_AT: usize = 40;
/// Combat stats a staff checker fights at (drop farms, guarded spots).
const STAFF_COMBAT: i64 = 70;

fn send_simple(account: &str, cmd: &str) -> Result<Value, String> {
    send(object(vec![("cmd", str_value(cmd)), ("account", str_value(account))]), Duration::from_secs(crate::daemon::WORKER_TIMEOUT))
}

fn running(state: &Value) -> bool {
    state.get("phase").as_str() == Some("running")
}

/// Opens the staff session when the daemon lost it (a restart restores
/// the fleet's jobs, not the checker's), then waits for its state.
fn ensure_open(account: &str) -> Result<(), String> {
    if show(account).is_ok() {
        return Ok(());
    }
    let opened = send_simple(account, "open");
    if let Err(error) = &opened {
        if !error.contains("already open") {
            return Err(format!("open {account}: {error}"));
        }
    }
    for _ in 0..60 {
        std::thread::sleep(Duration::from_secs(2));
        if show(account).is_ok_and(|state| state.get("inv").as_arr().is_some()) {
            return Ok(());
        }
    }
    Err(format!("{account}: no state after open"))
}

/// Stops a script still running (a timed-out check), waits for idle.
fn ensure_idle(account: &str) -> Result<(), String> {
    ensure_open(account)?;
    if !running(&show(account)?) {
        return Ok(());
    }
    send_simple(account, "unload")?;
    for _ in 0..30 {
        std::thread::sleep(Duration::from_secs(1));
        if !running(&show(account)?) {
            return Ok(());
        }
    }
    Err(format!("{account} still running after unload"))
}

/// Runs a staff script to its end (dispose), judged on its verdict.
fn run_to_end(account: &str, script: &str, args: &[String]) -> Result<(), String> {
    let first = show(account)?;
    let start_wall = crate::jobs::now_secs();
    send(
        object(vec![
            ("cmd", str_value("run")),
            ("account", str_value(account)),
            ("script", str_value(script)),
            ("args", Value::Arr(args.iter().map(|arg| str_value(arg)).collect())),
        ]),
        Duration::from_secs(crate::daemon::WORKER_TIMEOUT),
    )?;
    let mut source = DaemonSource { account: account.to_string() };
    let advance = || std::thread::sleep(Duration::from_millis(POLL_MS));
    match wait_for_end(&mut source, &Baseline::capture(&first), start_wall, CHECK_TIMEOUT_S, true, &crate::jobs::now_secs, &advance).0 {
        WaitEnd::Done(Verdict::Ok, _) => Ok(()),
        WaitEnd::Done(other, detail) => Err(format!("{script}: {other:?} {detail}")),
        WaitEnd::WallTimeout => Err(format!("{script}: no end in {CHECK_TIMEOUT_S}s")),
        WaitEnd::NoSession => Err(format!("{script}: no session")),
    }
}

/// The staff `tele` cheat to (x, z, level): `tele level,mx,mz,lx,lz`.
fn tele_command(x: i64, z: i64, level: i64) -> String {
    format!("tele {level},{},{},{},{}", x / 64, z / 64, x % 64, z % 64)
}

/// Once per campaign and whenever the bank fills: combat stats, the
/// staff kit, an emptied bank; every check starts from `STAFF_HOME` (a
/// check that strands its checker never fails the next ones: live
/// admin2 2026-11-15, 200 checks failed from (3228,3504)).
fn prepare(account: &str) -> Result<(), String> {
    ensure_idle(account)?;
    let here = show(account)?.get("position").as_arr().cloned().unwrap_or_default();
    let at = |index: usize| here.get(index).and_then(|value| value.as_i64()).unwrap_or(0);
    let (level, x, z) = (at(0), at(1), at(2));
    let (home_x, home_z, home_level) = STAFF_HOME;
    if level != home_level || (x - home_x).abs() + (z - home_z).abs() > 30 {
        cheat(account, &tele_command(home_x, home_z, home_level))?;
        std::thread::sleep(Duration::from_secs(3));
    }
    // An empty pack (kit aside) before staging: a full pack swallows the
    // staged inputs (`give` adds what fits, the rest vanishes; live admin
    // 2026-11-15: superheat and fly fishing "missing" what was given).
    if show(account)?.get("inv").as_arr().is_some_and(|inv| !inv.is_empty()) {
        let keep: Vec<String> = STAFF_KIT.iter().map(|name| name.to_string()).chain(std::iter::once("coins".to_string())).collect();
        let _ = run_to_end(account, "bank_deposit", &keep);
    }
    let state = show(account)?;
    for stat in ["attack", "strength", "defence", "hitpoints"] {
        if level_of(&state, stat) < STAFF_COMBAT {
            cheat(account, &format!("setstat {stat} {STAFF_COMBAT}"))?;
        }
    }
    let bank_types = state.get("bank").as_arr().map(Vec::len).unwrap_or(0);
    if bank_types >= BANK_DISPOSE_AT || state.get("inv").as_arr().map(Vec::len).unwrap_or(0) >= 20 {
        let keep: Vec<String> = STAFF_KIT.iter().map(|name| name.to_string()).collect();
        run_to_end(account, "check_dispose", &keep)?;
    }
    let state = show(account)?;
    for tool in STAFF_KIT {
        if held(&state, tool) == 0 && !lifescape_gamedata::objs_by_name(tool).is_empty() {
            cheat(account, &format!("give {tool} 1"))?;
        }
    }
    Ok(())
}

/// Actions whose inputs fit the pack beside the kit and coins carried:
/// unstacked inputs take a slot per unit, a stack one slot (live admin
/// 2026-11-15: 24 coal and 3 runite ore filled the pack, the nature
/// runes `give` added after vanished).
pub fn pack_fit(inputs: &[(String, i64)], count: i64, free: i64) -> i64 {
    let stacked = |name: &str| lifescape_gamedata::objs_by_name(name).into_iter().next().is_some_and(|obj| obj.stackable);
    let slots = |actions: i64| -> i64 { inputs.iter().map(|(name, per)| if stacked(name) { 1 } else { per.max(&1) * actions }).sum() };
    (1..=count.max(1)).rev().find(|actions| slots(*actions) <= free).unwrap_or(1)
}

/// Stage the staff account for `count` actions (cheats stay on staff).
fn stage(account: &str, plan: &MethodPlan, count: i64) -> Result<(), String> {
    prepare(account)?;
    let state = show(account)?;
    let carried = state.get("inv").as_arr().map(Vec::len).unwrap_or(0) as i64;
    let count = pack_fit(&plan.inputs, count, 28 - carried - plan.tools.len() as i64);
    if let Some((skill, level)) = &plan.skill {
        if level_of(&state, skill) < i64::from(*level) {
            cheat(account, &format!("setstat {skill} {level}"))?;
        }
    }
    // A quest gate: the staff account's quest varps set done (staff only).
    if let Some(dir) = &plan.quest {
        let key = dir.strip_prefix("quest_").unwrap_or(dir);
        if let Some(quest) = lifescape_gamedata::quest_requirements::by_key(key) {
            for (varp, value) in &quest.completion_varps {
                if let Some(name) = lifescape_gamedata::varps::all_varps().into_iter().find(|row| row.varp_id == *varp).map(|row| row.name) {
                    cheat(account, &format!("setvar {name} {value}"))?;
                }
            }
        }
    }
    for tool in &plan.tools {
        if held(&state, tool) == 0 {
            cheat(account, &format!("give {tool} 1"))?;
        }
    }
    // A fishing spot lands its lower catch too (sardines beside herring,
    // trout beside salmon), each eating bait or a feather: stacked bait
    // for ten casts per wanted fish (live admin 2026-11-16, salmon and
    // herring blocked on bait after three trout or sardines).
    let per_action = if plan.script == "fishing" { FISHING_BAIT_FACTOR } else { 1 };
    for (input, per) in &plan.inputs {
        let want = per.max(&1) * count * per_action;
        let have = held(&state, input);
        if have < want {
            cheat(account, &format!("give {input} {}", want - have))?;
        }
    }
    let coins = (plan.coins * count).max(0) + plan.fare;
    if held(&state, "coins") < coins {
        cheat(account, &format!("give coins {coins}"))?;
    }
    // `give` answers before the pack shows it: settle before the run.
    std::thread::sleep(Duration::from_secs(3));
    Ok(())
}

/// One check's outcome.
#[derive(Debug, Clone)]
pub struct Checked {
    pub drop_farm: bool,
    pub verdict: String,
    pub detail: String,
    pub before: i64,
    pub after: i64,
    pub wall_s: f64,
}

impl Checked {
    /// Worked: the product count rose (a `blocked` end once the staged
    /// inputs ran out is the normal end of a production run).
    pub fn worked(&self) -> bool {
        self.after > self.before || (self.drop_farm && self.verdict == "ok")
    }
}

/// Run `plan` on `account` and judge it on the product count.
pub fn check(account: &str, plan: &MethodPlan, count: i64) -> Result<Checked, String> {
    if !crate::economy_guard::is_staff(account) {
        return Err(format!("{account} is no staff account: check-method stages with cheats (admin, admin2 only)"));
    }
    stage(account, plan, count)?;
    let first = show(account)?;
    let before = measure(&first, plan);
    let start_wall = crate::jobs::now_secs();
    let reply = send(
        object(vec![
            ("cmd", str_value("run")),
            ("account", str_value(account)),
            ("script", str_value(&plan.script)),
            ("args", Value::Arr(plan.args.iter().map(|arg| str_value(arg)).collect())),
        ]),
        Duration::from_secs(crate::daemon::WORKER_TIMEOUT),
    )?;
    let _ = reply;
    let mut source = DaemonSource { account: account.to_string() };
    let advance = || std::thread::sleep(Duration::from_millis(POLL_MS));
    let (end, last) = wait_for_end(&mut source, &Baseline::capture(&first), start_wall, CHECK_TIMEOUT_S, true, &crate::jobs::now_secs, &advance);
    let (verdict, detail) = match end {
        WaitEnd::Done(Verdict::Ok, detail) => ("ok".to_string(), detail),
        WaitEnd::Done(other, detail) => (format!("{other:?}").to_lowercase(), detail),
        WaitEnd::WallTimeout => ("timeout".to_string(), format!("no end in {CHECK_TIMEOUT_S}s")),
        WaitEnd::NoSession => ("no_session".to_string(), String::new()),
    };
    std::thread::sleep(Duration::from_secs(2));
    let after_state = show(account).unwrap_or(last);
    Ok(Checked { drop_farm: plan.drop_farm, verdict, detail, before, after: measure(&after_state, plan), wall_s: crate::jobs::now_secs() - start_wall })
}

fn record(account: &str, plan: Option<&MethodPlan>, method: &str, outcome: &Result<Checked, String>) -> Value {
    let mut fields = vec![
        ("at", Value::Int(crate::jobs::now_secs() as i64)),
        ("account", str_value(account)),
        ("method", str_value(method)),
    ];
    if let Some(plan) = plan {
        fields.push(("script", str_value(&plan.script)));
        fields.push(("args", Value::Arr(plan.args.iter().map(|arg| str_value(arg)).collect())));
        fields.push(("product", str_value(&plan.product)));
    }
    match outcome {
        Ok(done) => {
            fields.push(("worked", Value::Bool(done.worked())));
            fields.push(("verdict", str_value(&done.verdict)));
            fields.push(("detail", str_value(&done.detail)));
            fields.push(("made", Value::Int(done.after - done.before)));
            fields.push(("wall_s", Value::Int(done.wall_s as i64)));
        }
        Err(error) => {
            fields.push(("worked", Value::Bool(false)));
            fields.push(("verdict", str_value("setup")));
            fields.push(("detail", str_value(error)));
        }
    }
    object(fields)
}

/// Path of the check registry (one JSON line per check).
pub fn registry_path() -> std::path::PathBuf {
    crate::paths::run_dir().join("method_checks.jsonl")
}

fn append(line: &Value) {
    use std::io::Write;
    let path = registry_path();
    match std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        Ok(mut file) => {
            if writeln!(file, "{}", json::stringify(line)).is_err() {
                eprintln!("check-method: cannot write {}", path.display());
            }
        }
        Err(error) => eprintln!("check-method: cannot open {}: {error}", path.display()),
    }
}

fn check_one(account: &str, method: &str, count: i64) -> bool {
    let planned = plan(method, count);
    let outcome = match &planned {
        Ok(found) => check(account, found, count),
        Err(error) => Err(error.clone()),
    };
    let line = record(account, planned.as_ref().ok(), method, &outcome);
    append(&line);
    println!("{}", json::stringify(&line));
    outcome.map(|done| done.worked()).unwrap_or(false)
}

/// Every method the planner may launch to obtain an F2P object (its
/// obtain legs, the market aside: a purchase is the market's test),
/// sorted, deduplicated.
pub fn all_methods() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for obj in lifescape_gamedata::all_objs() {
        let Some(name) = obj.name else { continue };
        if obj.members != Some(false) || name.starts_with("cert_") {
            continue;
        }
        for alt in lifescape_planner::obtain_legs::obtain_alternatives(name, false) {
            if alt.method.starts_with("market:") {
                continue;
            }
            // Every leg carries its object (`leg@object`): a drop farm or a
            // purchase names no recipe.
            let method = format!("{}@{name}", alt.method);
            if !out.contains(&method) {
                out.push(method);
            }
        }
    }
    // D18: the spell methods no obtain leg names (teleports, curses,
    // crumble undead; bananas too), judged on their XP or product.
    for method in lifescape_planner::spell_methods::all_spell_methods() {
        if !out.contains(&method) {
            out.push(method);
        }
    }
    out.sort();
    out
}

/// CLI entry: `check-method <staff> <method> [count]` or
/// `check-method <staff> --list <file> [count]` (one method per line,
/// `#` comments). Exit 0 when every check worked.
pub fn check_method_cli(args: &[String]) -> i32 {
    let usage = "usage: check-method <staff> <method> [count] | check-method <staff> --list <file> [count] | check-method --print-all";
    if args.first().map(String::as_str) == Some("--print-all") {
        for method in all_methods() {
            println!("{method}");
        }
        return 0;
    }
    if args.first().map(String::as_str) == Some("--plan") {
        for method in args.iter().skip(1) {
            match plan(method, 3) {
                Ok(found) => println!("{method}: {} {:?} product={} skill={:?} inputs={:?} tools={:?} coins={}", found.script, found.args, found.product, found.skill, found.inputs, found.tools, found.coins),
                Err(error) => println!("{method}: {error}"),
            }
        }
        return 0;
    }
    let (Some(account), Some(target)) = (args.first(), args.get(1)) else {
        eprintln!("{usage}");
        return 2;
    };
    let (methods, count_at) = if target == "--list" {
        let Some(file) = args.get(2) else {
            eprintln!("{usage}");
            return 2;
        };
        let text = match std::fs::read_to_string(file) {
            Ok(text) => text,
            Err(error) => {
                eprintln!("check-method: {file}: {error}");
                return 2;
            }
        };
        (text.lines().map(|line| line.split('#').next().unwrap_or("").trim().to_string()).filter(|line| !line.is_empty()).collect::<Vec<_>>(), 3)
    } else {
        (vec![target.clone()], 2)
    };
    let count = args.get(count_at).and_then(|raw| raw.parse::<i64>().ok()).unwrap_or(10).max(1);
    let mut all = true;
    for method in &methods {
        all &= check_one(account, method, count);
    }
    if all {
        0
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::{held, plan};
    use lifescape_json::{object, Value};

    #[test]
    fn the_planner_names_script_inputs_and_tools() {
        let ring = plan("jewelry:gold_ring", 5).expect("gold ring");
        assert_eq!(ring.product, "gold_ring");
        assert!(ring.inputs.iter().any(|(name, _)| name == "gold_bar"), "{ring:?}");
        let needs: Vec<&String> = ring.inputs.iter().map(|(name, _)| name).chain(ring.tools.iter()).collect();
        assert!(needs.iter().any(|name| *name == "ring_mould"), "the mould is staged: {needs:?}");
        let strung = plan("string:unstrung_gold_amulet", 5).expect("stringing");
        assert_eq!(strung.script, "stringing");
        assert!(plan("no_such:method", 1).is_err());
    }

    /// D18 spell methods: each plans its script, its runes per cast and
    /// magic at its level; a cast without product is judged on XP.
    #[test]
    fn spell_methods_plan_their_casts() {
        let teleport = plan("teleport:varrock_teleport", 5).expect("teleport plans");
        assert_eq!((teleport.script.as_str(), teleport.product.as_str()), ("enchant_teleport", ""));
        assert_eq!(&teleport.args[..2], &["varrock_teleport".to_string(), "5".to_string()]);
        assert_eq!(teleport.skill, Some(("magic".to_string(), 25)));
        assert!(teleport.inputs.contains(&("lawrune".to_string(), 1)));
        let curse = plan("curse:confuse/chickens", 5).expect("curse plans");
        assert_eq!(curse.script, "curse");
        assert_eq!(&curse.args[..3], &["chickens".to_string(), "--spell".to_string(), "confuse".to_string()]);
        assert!(curse.args.windows(2).any(|pair| pair[0] == "--site"), "{:?}", curse.args);
        assert!(curse.inputs.contains(&("bodyrune".to_string(), 1)));
        let bananas = plan("bananas:bones", 2).expect("bananas plan");
        assert_eq!((bananas.script.as_str(), bananas.product.as_str()), ("enchant_teleport", "banana"));
        let crumble = plan("fight:magic/crumble_undead/wild_skeletons", 5).expect("crumble plans");
        assert_eq!((crumble.script.as_str(), crumble.product.as_str()), ("magic", ""));
        assert_eq!(crumble.skill, Some(("magic".to_string(), 39)));
        assert!(crumble.inputs.contains(&("chaosrune".to_string(), 1)));
        let all = super::all_methods();
        for method in ["teleport:lumbridge_teleport", "curse:bind/cows", "bananas:bones", "fight:magic/crumble_undead/wild_skeletons"] {
            assert!(all.contains(&method.to_string()), "{method} listed");
        }
        let state = object(vec![("skills", Value::Arr(vec![object(vec![("id", Value::Int(6)), ("xp", Value::Int(1234))])]))]);
        assert_eq!(super::measure(&state, &teleport), 1234, "judged on magic XP");
    }

    #[test]
    fn staged_inputs_fit_the_pack() {
        let superheat = vec![("coal".to_string(), 8), ("runite_ore".to_string(), 1), ("firerune".to_string(), 4), ("naturerune".to_string(), 1)];
        assert_eq!(super::pack_fit(&superheat, 3, 26), 2, "two casts: 18 ore and coal, two rune stacks");
        let shrimp = vec![("raw_shrimp".to_string(), 1)];
        assert_eq!(super::pack_fit(&shrimp, 3, 26), 3);
        assert_eq!(super::pack_fit(&superheat, 3, 5), 1, "at least one action");
    }

    #[test]
    fn the_tele_cheat_names_the_map_square() {
        assert_eq!(super::tele_command(3092, 3245, 0), "tele 0,48,50,20,45");
        assert_eq!(super::tele_command(3225, 3256, 0), "tele 0,50,50,25,56");
    }

    #[test]
    fn held_counts_pack_bank_and_worn() {
        let row = |id: i64, count: i64| object(vec![("id", Value::Int(id)), ("count", Value::Int(count))]);
        let state = object(vec![("inv", Value::Arr(vec![row(2357, 3)])), ("bank", Value::Arr(vec![row(2357, 10), row(995, 50)])), ("worn", Value::Arr(Vec::new()))]);
        assert_eq!(held(&state, "gold_bar"), 13);
        assert_eq!(held(&state, "coins"), 50);
    }
}
