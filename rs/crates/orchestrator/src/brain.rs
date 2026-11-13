//! Brain: the daemon-side autopilot. Life files live in
//! `run/lives/<compte>.json` (schema v1, atomic write); the single
//! supervisor calls `on_supervise` every pass, which folds pushed task
//! reports into the graph (replanning) and launches the next ready node
//! when the worker idles. No-ops without a life file, so manual bots are
//! untouched (R1-R7 parity).
//!
//! Planning inputs are public data only (market references, published
//! yields, trend scores): never another bot's state (D22).

#![forbid(unsafe_code)]

use std::sync::Arc;

use lifescape_json::Value;
use lifescape_planner::{Archetype, GoalKind, HistoryEntry, Life, Node, NodeKind, NodeStatus, Profile, expand_goal, life_from_value, life_to_value};

use crate::brain_fold::handle_report;
use crate::daemon::Orchestrator;
use crate::jobs::Job;
pub use crate::brain_plan::{TRIP_TICKS, choose_money};

/// Merchant share of `get_rich` draws (D18, calibrated in L5).
pub const MERCHANT_SHARE_PCT: u64 = 10;
/// Iron cap (D8): at most 20 Iron accounts, enforced at creation.
pub const MAX_IRON: usize = 20;

/// Iron slots left at creation: `existing` counts current Iron lives.
pub fn iron_allowed(existing: usize) -> bool {
    existing < MAX_IRON
}
pub fn lives_dir() -> std::path::PathBuf {
    crate::paths::run_dir().join("lives")
}

pub fn life_path(account: &str) -> std::path::PathBuf {
    lives_dir().join(format!("{account}.json"))
}

/// Load one life file; `None` means no autopilot for this account (not
/// an error: manual bots have no file).
pub fn load_life(account: &str) -> Option<Life> {
    let text = std::fs::read_to_string(life_path(account)).ok()?;
    let value = lifescape_json::parse(&text).ok()?;
    life_from_value(&value)
}

pub fn save_life(life: &Life) -> Result<(), String> {
    crate::paths::write_json(&life_path(&life.account), &life_to_value(life))
        .map_err(|error| format!("brain save {}: {error}", life.account))
}

/// Accepted `--goal` shapes: `max:a,b:40`, `obtain:feather:2000`,
/// `produce:bronze_platebody:20` (made, never bought),
/// `money`, `quest:cook`, `equip:melee`, `area:karamja`, `rich[:merchant
/// |:producer]`. Bare `rich` draws the archetype (10% merchant).
pub fn parse_goal(text: &str) -> Result<GoalKind, String> {
    let text = text.trim();
    if text.is_empty() || text == "money" {
        return Ok(GoalKind::MakeMoney);
    }
    if let Some(rest) = text.strip_prefix("max:") {
        let (skills, level) = rest.rsplit_once(':').ok_or_else(|| "goal max:<skills>:<level>".to_string())?;
        let level: u8 = level.parse().map_err(|_| "goal level must be 2-99".to_string())?;
        if !(2..=99).contains(&level) {
            return Err("goal level must be 2-99".to_string());
        }
        let skills: Vec<String> = skills.split(',').map(str::trim).filter(|skill| !skill.is_empty()).map(str::to_string).collect();
        if skills.is_empty() {
            return Err("goal max: needs at least one skill".to_string());
        }
        return Ok(GoalKind::MaxSkills { skills, level });
    }
    if let Some(rest) = text.strip_prefix("obtain:") {
        let (item, count) = rest.rsplit_once(':').ok_or_else(|| "goal obtain:<item>:<count>".to_string())?;
        let count: i64 = count.parse().map_err(|_| "goal count must be a number".to_string())?;
        return Ok(GoalKind::Obtain { item: item.to_string(), count: count.max(1) });
    }
    if let Some(rest) = text.strip_prefix("produce:") {
        let (item, count) = rest.rsplit_once(':').ok_or_else(|| "goal produce:<item>:<count>".to_string())?;
        let count: i64 = count.parse().map_err(|_| "goal count must be a number".to_string())?;
        if item.is_empty() {
            return Err("goal produce:<item>:<count>".to_string());
        }
        return Ok(GoalKind::Produce { item: item.to_string(), count: count.max(1) });
    }
    if let Some(quest) = text.strip_prefix("quest:") {
        if quest.is_empty() {
            return Err("goal quest:<key>".to_string());
        }
        return Ok(GoalKind::CompleteQuest { quest: quest.to_string() });
    }
    if let Some(style) = text.strip_prefix("equip:") {
        return Ok(GoalKind::EquipBest { style: if style.is_empty() { "melee".to_string() } else { style.to_string() } });
    }
    if let Some(area) = text.strip_prefix("area:") {
        return Ok(GoalKind::AccessArea { area: area.to_string() });
    }
    if text == "rich" {
        return Ok(GoalKind::GetRich { merchant: None });
    }
    if let Some(arch) = text.strip_prefix("rich:") {
        return match arch {
            "merchant" => Ok(GoalKind::GetRich { merchant: Some(true) }),
            "producer" => Ok(GoalKind::GetRich { merchant: Some(false) }),
            _ => Err("goal rich[:merchant|:producer]".to_string()),
        };
    }
    Err(format!("unknown goal {text:?} (max:|obtain:|produce:|money|quest:|equip:|area:|rich)"))
}

fn xorshift(seed: u64, stream: u64) -> u64 {
    let mut word = seed.wrapping_add(stream.wrapping_mul(0x9E37_79B9_7F4A_7C15)).max(1);
    word ^= word << 13;
    word ^= word >> 7;
    word ^= word << 17;
    word
}

/// Natural player-like name (`olivia_king`, `IronOlivia`; 12 max, RS2
/// rule), see `names`.
pub fn generate_pseudo(iron: bool, seed: u64) -> String {
    crate::names::natural_name(iron, seed)
}

/// Profile draw from the seed: affinity and patience uniform in [0,1],
/// merchant share 10% (never Iron, enforced by the caller too).
pub fn draw_profile(seed: u64, iron: bool) -> Profile {
    let affinity = (xorshift(seed, 101) % 1000) as f64 / 999.0;
    let patience = (xorshift(seed, 102) % 1000) as f64 / 999.0;
    let archetype = if !iron && xorshift(seed, 103) % 100 < MERCHANT_SHARE_PCT {
        Archetype::Merchant
    } else {
        Archetype::Producer
    };
    Profile::new(iron, affinity, patience, seed, archetype)
}

fn iron_count() -> usize {
    let dir = lives_dir();
    let entries = std::fs::read_dir(&dir).map(|entries| entries.filter_map(|entry| entry.ok()).count()).unwrap_or(0);
    let _ = entries;
    // Count files whose profile says ironman (cheap: only at creation).
    let mut count = 0;
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            if let Ok(text) = std::fs::read_to_string(&path) {
                if let Ok(value) = lifescape_json::parse(&text) {
                    if value.get("profile").get("ironman").as_bool() == Some(true) {
                        count += 1;
                    }
                }
            }
        }
    }
    count
}

/// Create a life file (and its account name when omitted). Existing
/// files load untouched: creation never rewrites a running brain.
pub fn ensure_life(account: &str, iron: bool, goal_text: &str) -> Result<Life, String> {
    if let Some(life) = load_life(account) {
        return Ok(life);
    }
    if iron && !iron_allowed(iron_count()) {
        return Err(format!("iron cap reached ({MAX_IRON}): no new Ironman"));
    }
    let goal = parse_goal(goal_text)?;
    if iron {
        if let GoalKind::GetRich { merchant: Some(true) } = goal {
            return Err("iron never merchants".to_string());
        }
    }
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|span| span.as_nanos() as u64)
        .unwrap_or(1)
        ^ (std::process::id() as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let profile = draw_profile(seed, iron);
    let nodes = expand_goal(&goal, &profile);
    let life = Life {
        schema: lifescape_planner::model::LIFE_SCHEMA,
        account: account.to_string(),
        created_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|span| span.as_secs() as i64)
            .unwrap_or(0),
        seed,
        profile,
        goal,
        nodes,
        engagement: None,
        active: None,
        history: Vec::new(),
        ambition: None,
        goal_origin: None,
    };
    save_life(&life)?;
    Ok(life)
}

/// `create-user [pseudo] [--iron] [--goal kind]`: pseudo generated when
/// omitted, life file written, then the CLI opens the session and runs
/// the tutorial (tutorial first, brain takes over whenever idle after).
pub fn create_user(account_opt: Option<String>, iron: bool, goal: Option<String>) -> Result<String, String> {
    let account = match account_opt {
        Some(name) if !name.is_empty() => name,
        _ => {
            let seed = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|span| span.as_nanos() as u64)
                .unwrap_or(1);
            // A name already used by a known account draws again.
            let taken = |name: &str| lives_dir().join(format!("{name}.json")).exists();
            (0..64u64).map(|attempt| generate_pseudo(iron, seed.wrapping_add(attempt))).find(|name| !taken(name)).ok_or("no free account name after 64 draws")?
        }
    };
    ensure_life(&account, iron, goal.as_deref().unwrap_or("max:fishing:40"))?;
    Ok(account)
}

/// Keep-list for a cleanup order: unfinished graph obtain items plus the
/// item checks of unfinished quests, deduplicated and sorted. Coins, food
/// and best tools are added worker-side (planner triage, kit specs).
pub fn cleanup_keep(life: &Life) -> Vec<String> {
    let mut keep = Vec::new();
    for node in &life.nodes {
        if matches!(node.status, NodeStatus::Done | NodeStatus::Abandoned) {
            continue;
        }
        match &node.kind {
            NodeKind::ObtainItems { item, .. } => keep.push(item.clone()),
            NodeKind::CompleteQuest { quest } => {
                if let Some(required) = lifescape_gamedata::quest_requirements::by_key(quest) {
                    for check in &required.items {
                        keep.push(check.item.clone());
                    }
                }
            }
            _ => {}
        }
    }
    keep.sort();
    keep.dedup();
    keep
}

/// Space-pressure insert (L8) for one life: terminal graphs included, a
/// full bank needs its janitor even when everything else is done.
/// Returns true when a node was inserted.
pub fn maybe_insert_cleanup(life: &mut Life, bank_type_count: usize) -> bool {
    let open = life.nodes.iter().any(|node| {
        matches!(node.kind, NodeKind::SpaceCleanup)
            && !matches!(node.status, NodeStatus::Done | NodeStatus::Abandoned)
    });
    // Done breaks a failure streak; two abandoned janitors in a row mean
    // the pressure never drains, so inserts stop instead of looping.
    let stalled = lifescape_planner::cleanup_stalled(&life.nodes);
    if !lifescape_planner::cleanup_needed(open, stalled, bank_type_count) {
        return false;
    }
    let mut counter = life.nodes.len();
    for node in &life.nodes {
        if let Some(number) = node.id.strip_prefix('n').and_then(|raw| raw.parse::<usize>().ok()) {
            counter = counter.max(number);
        }
    }
    lifescape_planner::insert_ready(life, Node::new(&format!("n{}", counter + 1), NodeKind::SpaceCleanup, Vec::new(), 0));
    true
}

pub(crate) fn push_history(life: &mut Life, node: &str, outcome: &str) {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs() as i64).unwrap_or(0);
    life.history.push(HistoryEntry { node: node.to_string(), started: now, ended: now, outcome: outcome.to_string() });
}

/// Brain inbox: the supervisor never waits on files, the store or a
/// session socket. It only try-sends here; one brain thread owns every
/// blocking brain step (life files, market/store reads, worker RPC).
/// A wedged worker or store stalls the brain, never the supervision or
/// the CLI dispatch. A full queue drops loudly (the next pass re-queues).
fn inbox() -> &'static std::sync::Mutex<
    Option<std::sync::mpsc::SyncSender<(String, Option<Value>)>>,
> {
    static INBOX: std::sync::OnceLock<
        std::sync::Mutex<Option<std::sync::mpsc::SyncSender<(String, Option<Value>)>>>,
    > = std::sync::OnceLock::new();
    INBOX.get_or_init(|| std::sync::Mutex::new(None))
}

fn ensure_brain(orch: &Arc<Orchestrator>) {
    {
        let slot = inbox().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if slot.is_some() {
            return;
        }
    }
    let (sender, receiver) = std::sync::mpsc::sync_channel::<(String, Option<Value>)>(16);
    {
        let mut slot = inbox().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        *slot = Some(sender);
    }
    let orch = orch.clone();
    std::thread::Builder::new()
        .name("brain-tick".to_string())
        .spawn(move || {
            while let Ok((account, report)) = receiver.recv() {
                let job = orch.jobs.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(&account).cloned();
                let Some(job) = job else {
                    continue;
                };
                if let Some(report) = report {
                    // Rates first (L4 store, raw rows + snapshot buckets),
                    // then the life graph. Both are bounded file/store
                    // writes on the brain thread, never the supervisor.
                    if let Err(error) = crate::jobs::ingest_report_persisted(&report) {
                        crate::logln!("job {account}: rates ingest failed: {error}");
                    }
                    crate::logln!("brain {}: folded report stop={}", account, report.get("stop").as_str().unwrap_or("?"));
                    handle_report(&orch, &job, &report);
                }
                crate::brain_launch::maybe_launch(&orch, &job);
            }
        })
        .expect("brain thread spawns");
}

/// Supervisor entry: enqueue only, never block. The brain thread folds
/// reports and launches; `had_report` mirrors the flag the loop already
/// drained, so the brain never misses a push.
pub fn on_supervise(orch: &Arc<Orchestrator>, job: &Arc<Job>, report: Option<Value>) {
    ensure_brain(orch);
    let had_report = report.is_some();
    let queued = inbox()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref()
        .map(|sender| sender.try_send((job.account.clone(), report)))
        .is_some_and(|sent| sent.is_ok());
    if !queued {
        // The drained report goes back to pending (`last_report` still
        // holds it): dropped here, its node read "no report" later.
        let kept = had_report && job.inner.try_lock().map(|mut inner| inner.report_pending = true).is_ok();
        let note = match (had_report, kept) {
            (true, true) => " (report kept pending)",
            (true, false) => " (report dropped: job contended)",
            _ => "",
        };
        crate::logln!("brain {}: inbox full, tick deferred to next pass{note}", job.account);
    }
}

#[cfg(test)]
mod tests {
    use super::{cleanup_keep, draw_profile, generate_pseudo, iron_allowed, maybe_insert_cleanup, parse_goal, MAX_IRON};
    use lifescape_planner::{GoalKind, Life, Node, NodeKind};

    #[test]
    fn goals_parse() {
        assert!(matches!(parse_goal("money"), Ok(GoalKind::MakeMoney)));
        assert!(matches!(parse_goal(""), Ok(GoalKind::MakeMoney)));
        let max = parse_goal("max:fishing,cooking:40").expect("max parses");
        assert!(matches!(max, GoalKind::MaxSkills { level: 40, .. }));
        assert!(parse_goal("max:fishing").is_err(), "level required");
        assert!(matches!(parse_goal("quest:cook"), Ok(GoalKind::CompleteQuest { .. })));
        assert!(matches!(parse_goal("produce:bronze_platebody:20"), Ok(GoalKind::Produce { count: 20, .. })));
        assert!(matches!(parse_goal("rich:merchant"), Ok(GoalKind::GetRich { merchant: Some(true) })));
        assert!(parse_goal("rich:ever").is_err(), "archetype closed");
        assert!(parse_goal("nope").is_err(), "unknown goals fail loud");
    }

    #[test]
    fn pseudos_fit_rs2_and_replay() {
        let iron = generate_pseudo(true, 42);
        assert!(iron.starts_with("Iron") && iron.len() <= 12, "Iron + name, 12 max, got {iron}");
        assert!(crate::names::valid_name(&generate_pseudo(false, 42)), "natural and valid");
        assert_eq!(iron, generate_pseudo(true, 42), "seeded pseudos replay");
        assert_ne!(generate_pseudo(false, 1), generate_pseudo(false, 2), "names vary");
    }

    #[test]
    fn merchant_share_and_iron_rule() {
        let mut merchants = 0;
        for seed in 0..100 {
            if draw_profile(seed, false).archetype == lifescape_planner::Archetype::Merchant {
                merchants += 1;
            }
            assert_eq!(draw_profile(seed, true).archetype, lifescape_planner::Archetype::Producer, "iron never merchants");
        }
        assert!((5..=20).contains(&merchants), "merchant share ~10%, got {merchants}/100");
    }


    #[test]
    fn profile_draw_replays_from_the_seed() {
        let first = draw_profile(918273645, false);
        let second = draw_profile(918273645, false);
        assert_eq!(first, second, "same seed replays the whole profile");
        assert!((0.0..=1.0).contains(&first.market_affinity), "affinity in [0,1]");
        assert!((0.0..=1.0).contains(&first.price_patience), "patience in [0,1]");
        assert!((first.margin - (0.05 + first.price_patience * 0.15)).abs() < 1e-12, "margin derives from patience");
        assert_eq!(first.seed, 918273645, "seed persists for the replay");
        assert_ne!(draw_profile(1, false).market_affinity, draw_profile(2, false).market_affinity, "seeds vary");
    }

    #[test]
    fn iron_cap_holds_at_twenty() {
        assert_eq!(MAX_IRON, 20, "D8 cap pinned");
        assert!(iron_allowed(19), "19 Iron leave one slot");
        assert!(!iron_allowed(20), "20 Iron close creation");
        assert!(!iron_allowed(99), "over cap stays closed");
    }

    #[test]
    fn cleanup_keep_covers_graph_and_quests() {
        let profile = draw_profile(7, false);
        let mut nodes = lifescape_planner::expand_goal(
            &GoalKind::Obtain { item: "feather".to_string(), count: 100 },
            &profile,
        );
        nodes.push(Node::new("n9", NodeKind::CompleteQuest { quest: "cook".to_string() }, Vec::new(), 1));
        let life = Life {
            schema: lifescape_planner::model::LIFE_SCHEMA,
            account: "t99".to_string(),
            created_at: 0,
            seed: 7,
            profile,
            goal: GoalKind::Obtain { item: "feather".to_string(), count: 100 },
            nodes,
            engagement: None,
            active: None,
            history: Vec::new(),
            ambition: None,
            goal_origin: None,
        };
        let keep = cleanup_keep(&life);
        assert!(keep.contains(&"feather".to_string()), "graph obtain survives, got {keep:?}");
        assert!(keep.contains(&"bucket_milk".to_string()), "quest check survives, got {keep:?}");
        assert!(keep.windows(2).all(|pair| pair[0] <= pair[1]), "sorted for stable orders");
    }

    fn terminal_life() -> Life {
        let profile = draw_profile(7, false);
        let mut done = Node::new("n1", NodeKind::MakeMoney, Vec::new(), 0);
        done.status = lifescape_planner::NodeStatus::Done;
        Life {
            schema: lifescape_planner::model::LIFE_SCHEMA,
            account: "t99".to_string(),
            created_at: 0,
            seed: 7,
            profile,
            goal: GoalKind::MakeMoney,
            nodes: vec![done],
            engagement: None,
            active: None,
            history: Vec::new(),
            ambition: None,
            goal_origin: None,
        }
    }

    #[test]
    fn space_pressure_inserts_even_on_terminal_graphs() {        let mut life = terminal_life();
        assert!(maybe_insert_cleanup(&mut life, 55), "full bank on a done graph still inserts");
        assert!(matches!(life.nodes.last().map(|node| (&node.kind, &node.status)),
            Some((NodeKind::SpaceCleanup, lifescape_planner::NodeStatus::Ready))), "janitor ready at once");
        let mut roomy = terminal_life();
        assert!(!maybe_insert_cleanup(&mut roomy, 54), "room left inserts nothing");
        assert_eq!(roomy.nodes.len(), 1, "life untouched");
        let mut second = terminal_life();
        second.nodes.push(Node::new("n2", NodeKind::SpaceCleanup, Vec::new(), 0));
        assert!(!maybe_insert_cleanup(&mut second, 59), "an open node blocks doubles");
        let mut stalled = terminal_life();
        for id in ["n2", "n3"] {
            let mut abandoned = Node::new(id, NodeKind::SpaceCleanup, Vec::new(), 0);
            abandoned.status = lifescape_planner::NodeStatus::Abandoned;
            stalled.nodes.push(abandoned);
        }
        assert!(!maybe_insert_cleanup(&mut stalled, 60), "two abandoned in a row stop the loop");
    }

    /// P3.6 integration: every family method the planner emits names a
    /// script the registry dispatches. P3.1-P3.5 all landed (72
    /// scripts), so the pending set is empty: an unmapped script fails
    /// this test, a missing one fails typed at launch.
    #[test]
    fn mapped_scripts_are_registered() {
        let obtain = Node::new("n9", NodeKind::ObtainItems { item: "feather".to_string(), count: 2000 }, Vec::new(), 2000);
        let reach = Node::new("n1", NodeKind::ReachLevel { skill: "smithing".to_string(), level: 30 }, Vec::new(), 30);
        for method in [
            "gather:fishing/raw_trout", "gather:woodcut/willow_logs", "gather:mining/coal",
            "gather:sheep/wool", "gather:wheat/grain",
            "cook/raw_shrimp", "cook:trout", "bake:bread",
            "smelt:steel_bar", "smith:steel_platebody",
            "tan:cow_hide", "craft:leather_gloves",
            "fire:willow_logs", "spin:ball_of_wool", "pottery:pot", "jewelry:gold_ring",
            "string:unstrung_gold_amulet", "alch:gold_ring", "superheat:iron_bar",
            "enchant:strung_sapphire_amulet",
            "runecraft:airrune", "pick:grain", "mill:pot_flour", "fill:jug_water",
            "essence:blankrune",
            "fight:melee/cows", "fight:ranged/cows", "fight:magic/cows",
            "farm:feather", "market:buy", "shop:buy", "market:flip", "space:cleanup",
            "equip:melee", "quest:cook", "access:varrock",
        ] {
            let node = if method.starts_with("gather:") || method.starts_with("smelt:") || method.starts_with("cook/") {
                &reach
            } else {
                &obtain
            };
            let (script, _) = lifescape_planner::map_method(method, node, None)
                .unwrap_or_else(|| panic!("{method} maps"));
            assert!(crate::registry::script_known(&script), "{method} -> {script}: not registered");
        }
    }

    /// P3.6: a method without script is a contract error: abandoned at
    /// once with its cause (relaunching the same method can never work).
    #[test]
    fn no_script_abandons_typed_never_silent() {
        let mut life = terminal_life();
        life.nodes[0].status = lifescape_planner::NodeStatus::Active;
        life.active = Some("n1".to_string());
        crate::brain_launch::fail_no_script(&mut life, 0, "smelt:iron_bar");
        assert_eq!(life.nodes[0].status, lifescape_planner::NodeStatus::Abandoned);
        assert!(life.active.is_none(), "no stranded lock");
        assert_eq!(life.nodes[0].last_error, "no-script: smelt:iron_bar", "the missing method rides in the error");
        assert_eq!(life.history.iter().filter(|entry| entry.outcome == "abandoned").count(), 1, "one typed entry in history");
    }
}
