//! Goal expansion (HTN squelette): a goal becomes a full node chain at
//! creation; leaf methods resolve au plus tard, when the node turns
//! ready, because prices and yields move. One task = one script + one
//! resource + one objective (D21): the brain fixes the resource, the
//! script only finds where to exploit it.

use crate::model::{Alternative, GoalKind, Node, NodeKind, NodeStatus, Profile};
pub use crate::tools::{gear_ok, method_feasible, required_tool, tool_family, ToolNeed};
pub use crate::tranches::{reach_method, tranche_resource, tranches};


/// Public numbers the brain hands over at resolve time (market
/// reference, published yields, trend): never another bot's state.
#[derive(Debug, Clone)]
pub struct ReadyCtx {
    pub market_affinity: f64,
    pub ironman: bool,
    pub coins_per_tick: f64,
    pub trip_ticks: u64,
    pub price: i64,
    pub shop_price: i64,
    /// Units the F2P keepers sell of the item in one window (base stock
    /// + restock, `gamedata::shop_buy::purchase_steps`); `None` when no
    /// ladder is known. A need over it never takes the keeper first (live
    /// 2026-11-18: 31 bots sent for 70 uncut sapphires to two gem shops
    /// stocking 1 each, restocked every 25 000 ticks).
    pub shop_window_units: Option<i64>,
    pub yield_per_6000: f64,
    pub xp_per_6000: f64,
    /// Carried + banked coins of the ordering bot. A market leg it
    /// cannot cover is dropped before the choice (same coverage rule
    /// as the carnet, planner-side).
    pub coins: i64,
    /// Carried item debug names (live inventory the brain already
    /// reads): a gather method whose tool is missing is dropped
    /// before assignment unless the pack can buy it.
    pub tools: Vec<String>,
    /// False on `--no-shop` runs: no tool is buyable, only carried.
    pub allow_shop: bool,
    /// False until the bank was seen this session: a tool may sit there,
    /// so the gate never concludes "missing" (the script restores banked
    /// tools or raises a typed `Blocked` itself).
    pub bank_known: bool,
    /// Bot's combat level and position: the site pick reads them (126 /
    /// none in offline contexts: no danger constraint, no distance).
    pub combat: u8,
    pub position: Option<(i64, i64, i64)>,
    /// Best resting ask on the market for the node's object (PRD3: a
    /// covered ask is taken before any production).
    pub best_ask: Option<i64>,
    /// Live base levels by skill name (`smithing`); empty offline.
    pub levels: Vec<(String, u8)>,
    /// Published profit per 6000 ticks per money method (rates).
    pub method_profit: Vec<(String, f64)>,
    /// Units of the node's object the bot already holds (pack, bank,
    /// worn): an obtain buys or makes only what is still missing.
    pub held: i64,
    /// Share of the wallet (percent) a purchase may spend, when the
    /// brain caps it (equipment steps: `gear_ladder::GEAR_SHARE_PCT`).
    /// `None`: the whole wallet, capped at the fair unit price.
    pub coin_share_pct: Option<i64>,
    /// F2P quests the bot finished (keys: `runemysteries`, `prince`):
    /// engine save varps plus the life's done quest nodes. A method whose
    /// quest is missing never launches (`quests::method_quest`); Prince
    /// Ali Rescue makes the toll free.
    pub quests_done: Vec<String>,
    /// The brain's priced pick for a smithing or magic tranche
    /// (`training::best_pick`, market and holdings at resolve): locked
    /// before the static tranche method. `None`: the static method.
    pub training: Option<crate::training::TrainPick>,
    /// Sites the rest of the fleet works right now, with how many bots
    /// each (`combat::pick_site_crowded`): a crowded site reads farther.
    pub crowd: Vec<((i64, i64, i64), u32)>,
    /// Methods the method health registry quarantined (brain,
    /// `method_health`: under 10 % success over 20+ launches, until a
    /// live `check-method` proves them again): never resolved, the
    /// quarantine logged, no silent substitute.
    pub quarantined: Vec<String>,
    /// Method and site keys this bot cooled after an abandonment or a
    /// contended site (`cooldown`, plan P1/D4): a cooled method is
    /// unworkable like a quarantined one, a cooled site is skipped.
    pub cooled: Vec<String>,
    /// A market maker's post by the Varrock fountain (plan Q6,
    /// `market_maker::maker_post`): the site of its `market:make` node.
    pub maker_post: Option<(i64, i64, i64)>,
    /// 100 % market offers this bot may work (plan rendements R5,
    /// `trade_craft`): the money guide's market rows the brain read from
    /// the precomputed guides, with the cycle's asks and the units held.
    /// Empty for an Ironman.
    pub trades: Vec<crate::trade_craft::TradeOffer>,
    /// Free wallet a trade lot may tie up (coins minus what the bot's
    /// resting buys and deals commit).
    pub trade_wallet: i64,
    /// The money guide's lines this bot may take (plan rendements R7,
    /// the brain's `brain_money`): a money or coin node draws among them
    /// (`money_pick`). Empty: no guide published, or no earning line
    /// within reach: the static money methods rank as before R7 (logged
    /// by the brain).
    pub money_guide: Vec<crate::guide_pick::GuideLine>,
    /// Seed of this resolve's draw (`guide_pick::seed_for`: the bot's
    /// seed, the node, its draw counter).
    pub draw_seed: u64,
    /// What this resolve drew from a guide (money here, training in the
    /// brain), read back by the brain to log and record it.
    pub drawn: std::cell::RefCell<Option<crate::guide_pick::Choice>>,
}

impl ReadyCtx {
    pub fn blind(affinity: f64) -> Self {
        ReadyCtx {
            market_affinity: affinity,
            ironman: false,
            coins_per_tick: 0.5,
            trip_ticks: 800,
            price: 10,
            shop_price: 10,
            shop_window_units: None,
            yield_per_6000: 200.0,
            xp_per_6000: 4000.0,
            coins: 0,
            tools: Vec::new(),
            allow_shop: true,
            bank_known: true,
            combat: 126,
            position: None,
            best_ask: None,
            levels: Vec::new(),
            method_profit: Vec::new(),
            held: 0,
            coin_share_pct: None,
            quests_done: Vec::new(),
            training: None,
            crowd: Vec::new(),
            quarantined: Vec::new(),
            cooled: Vec::new(),
            maker_post: None,
            trades: Vec::new(),
            trade_wallet: 0,
            money_guide: Vec::new(),
            draw_seed: 0,
            drawn: std::cell::RefCell::new(None),
        }
    }

    /// Why `method` never launches for this bot now: `quarantined`
    /// (fleet, `method_health`) or `cooled` (this bot, `cooldown`).
    pub fn sick(&self, method: &str) -> Option<&'static str> {
        if self.quarantined.iter().any(|sick| sick == method) {
            Some("quarantined")
        } else if crate::cooldown::method_cooled(&self.cooled, method) {
            Some("cooled")
        } else {
            None
        }
    }
}

/// Which static sources an item has: farmable (woodcut, mine, fish,
/// NPC drops — the socle's activities) vs shopable (NPC shop stock).
/// Ground spawns, smithing recipes and banknotes need activities that
/// branch off the socle later (§5.12): they do not count as farm legs
/// here. Tools (shop-only, e.g. a net) never get a `farm:` leg, so the
/// brain never hunts chickens for tackle.
pub(crate) fn source_flags(item: &str) -> (bool, bool) {
    use lifescape_gamedata::item_sources::ItemSource;
    let mut farm = false;
    for source in lifescape_gamedata::item_sources::sources_for_name(item) {
        match source {
            ItemSource::Woodcut { .. }
            | ItemSource::Mine { .. }
            | ItemSource::Fish { .. }
            | ItemSource::Drop { .. } => farm = true,
            _ => {}
        }
    }
    // An NPC shop leg only where an F2P keeper stocks the object
    // (`obtain::merchant_route_for`, the one source of truth): the old
    // index listed black_full_helm in a shop no F2P world opens (audit
    // 2026-11-15: 1 646 doomed `shop:buy` launches). A keeper behind the
    // Champions' Guild door counts (user 2026-11-16, Q7): the resolver
    // takes it with the bot's quest points (`shop_gate`).
    (farm, lifescape_gamedata::obtain::merchant_route_for(item, i64::MAX))
}

/// The quest-point door that keeps every shop of `item` shut to this bot
/// (`champions_guild`), `None` when a keeper it reaches sells it or no
/// keeper does: the bot's quest points (`ctx.quests_done`) against the
/// fewest any shop asks (`obtain::shop_quest_points`).
pub(crate) fn shop_gate(item: &str, ctx: &ReadyCtx) -> Option<&'static str> {
    let needed = lifescape_gamedata::obtain::shop_quest_points(item)?;
    if needed <= lifescape_gamedata::quest_requirements::quest_points_of(&ctx.quests_done) {
        return None;
    }
    lifescape_gamedata::obtain::ways(item)
        .iter()
        .filter(|way| way.kind == lifescape_gamedata::obtain::WayKind::Shop)
        .flat_map(|way| way.sites.iter().flat_map(|(x, z, level)| lifescape_gamedata::access::gates_at(*x, *z, *level)))
        .find_map(|gate| matches!(gate, lifescape_gamedata::access::Gate::QuestPoints { .. }).then(|| gate.area()))
}


/// Nested quest/guild expansions a skeleton files (a quest's climb may
/// need a guild, whose climb may need a quest: bounded).
pub const NESTED_DEPTH: u8 = 2;

fn next_id(counter: &mut usize) -> String {
    *counter += 1;
    format!("n{}", *counter)
}

/// Reach tranches from the live level `from` (1 when unknown) to `to`:
/// never a tranche at or below `from` (a level already held is never
/// replayed), empty when the bot already holds `to`.
pub(crate) fn reach_chain(skill: &str, from: u8, to: u8, counter: &mut usize, after: &mut String, rate: crate::work_rate::RateReader) -> Vec<Node> {
    let mut out = Vec::new();
    if to <= from.max(1) {
        return out;
    }
    // Each tranche works the tier open at its START level (1→5 fishes
    // shrimp, 5→10 sardines): the tier unlocked at the target is out of
    // reach for the whole tranche (lobster needs 40 to train 35→40).
    let mut start = from.max(1);
    for level in tranches(skill, from, to) {
        let id = next_id(counter);
        let mut node = Node::new(
            &id,
            NodeKind::ReachLevel { skill: skill.to_string(), level },
            if after.is_empty() { Vec::new() } else { vec![after.clone()] },
            i64::from(level),
        );
        // Its duration from the method's yield level by level (R3.3,
        // `sizing::tranche_ticks`); none without a rate (melee: the
        // skill rides in the node), the resolver's window then.
        let method = reach_method(skill, start);
        let est_ticks = crate::sizing::tranche_ticks_with(rate, &method, start, level).map(|ticks| ticks.ceil() as u64).unwrap_or(0);
        node.alternatives.push(Alternative { method, est_ticks, est_coins: 0 });
        *after = id.clone();
        out.push(node);
        start = level;
    }
    out
}

fn obtain_node(item: &str, count: i64, deps: Vec<String>, counter: &mut usize, ironman: bool) -> Node {
    let id = next_id(counter);
    let mut node = Node::new(&id, NodeKind::ObtainItems { item: item.to_string(), count }, deps, count);
    // PRD3: market first, every production way, the shop only under D-A.
    node.alternatives = crate::obtain_legs::obtain_alternatives(item, ironman);
    node
}

/// Guild key to its gated skill (services GUILDS order, from the 274
/// scripts): the mining entry carries no uniform, only the level.
fn guild_skill(guild: &str) -> Option<&'static str> {
    match guild {
        "cooks_guild" => Some("cooking"),
        "crafting_guild" => Some("crafting"),
        "mining_guild" => Some("mining"),
        _ => None,
    }
}

/// `expand_goal_from` for a fresh bot (every level 1): creation, where
/// no live snapshot exists yet, and offline tests. A goal drawn for a
/// running bot always goes through `expand_goal_from`.
pub fn expand_goal(goal: &GoalKind, profile: &Profile) -> Vec<Node> {
    expand_goal_from(goal, profile, &|_| 1)
}

/// Full skeleton: every step present, leaf methods resolved later
/// (alternatives carry est 0 until `resolve_ready` prices them). Climbs
/// start at the live level (`level_of`, 1 when unknown): a tranche at or
/// below what the bot holds is never generated (live 2026-11-13: 40 % of
/// melee climb ticks replayed reached tranches).
pub fn expand_goal_from(goal: &GoalKind, profile: &Profile, level_of: &dyn Fn(&str) -> u8) -> Vec<Node> {
    expand_goal_with(goal, profile, level_of, &crate::plan_read::PlanRead::prior())
}

/// `expand_goal_from` reading `read` (R7: the measured yields size the
/// tranches and their obtains; a tranche the training guide will choose
/// files nothing of its static method ahead, `plan_read`).
pub fn expand_goal_with(goal: &GoalKind, profile: &Profile, level_of: &dyn Fn(&str) -> u8, read: &crate::plan_read::PlanRead) -> Vec<Node> {
    let mut counter = 0usize;
    let mut nodes = Vec::new();
    match goal {
        GoalKind::MaxSkills { skills, level } => {
            for skill in skills {
                let mut after = String::new();
                nodes.extend(crate::skeleton_deps::reach_chain_with(skill, level_of(skill), *level, &mut counter, &mut after, NESTED_DEPTH, read));
            }
        }
        GoalKind::Obtain { item, count } => {
            nodes.push(obtain_node(item, *count, Vec::new(), &mut counter, profile.ironman));
        }
        GoalKind::MakeMoney => {
            let id = next_id(&mut counter);
            let mut node = Node::new(&id, NodeKind::MakeMoney, Vec::new(), 0);
            // Salmon: the best mainland catch (Karamja lobsters: paid boat).
            node.alternatives.push(Alternative { method: "gather:fishing/raw_salmon".to_string(), est_ticks: 0, est_coins: 0 });
            node.alternatives.push(Alternative { method: "gather:woodcut/yew_logs".to_string(), est_ticks: 0, est_coins: 0 });
            nodes.push(node);
        }
        GoalKind::CompleteQuest { quest } => {
            nodes.extend(expand_quest_planned(quest, profile, &mut counter, level_of, NESTED_DEPTH, read));
        }
        GoalKind::EquipBest { style } => {
            // PRD3 lot 5: the resolver's set for a fresh bot (levels and
            // quests unknown at creation: the level-1 set), one obtain
            // per piece, then the equip step. Upgrades ride the brain's
            // wishlist as levels rise.
            // Melee (2026-11-13): the equipment ladder (`brain_gear`) buys
            // one tier at a time off the graph; the goal only wears the
            // best piece owned (live: level-1 set obtains sat on 1-coin
            // bids, amulet_of_power, iron_platebody).
            let picks = if style == "melee" {
                Vec::new()
            } else {
                lifescape_gamedata::gear::best_gear(lifescape_gamedata::gear::Style::parse(style), &[], &[])
            };
            let mut deps = Vec::new();
            for pick in picks {
                // Only pieces some executable leg yields for this profile
                // (a highwayman-only cape is no target for an Iron).
                if crate::obtain_legs::obtain_alternatives(&pick.item, profile.ironman).is_empty() {
                    continue;
                }
                let obtain = obtain_node(&pick.item, if pick.slot == lifescape_gamedata::gear::AMMO { 100 } else { 1 }, Vec::new(), &mut counter, profile.ironman);
                deps.push(obtain.id.clone());
                nodes.push(obtain);
            }
            let id = next_id(&mut counter);
            let mut node = Node::new(&id, NodeKind::EquipBest { style: style.clone() }, deps, 1);
            node.alternatives.push(Alternative { method: format!("equip:{style}"), est_ticks: 400, est_coins: 0 });
            nodes.push(node);
        }
        GoalKind::AccessArea { area } => {
            nodes.extend(expand_access_planned(area, profile, &mut counter, level_of, NESTED_DEPTH, read));
        }
        GoalKind::Produce { item, count } => {
            // A production quota (ambition milestone): production legs
            // only, the product is never bought (inputs may be, through
            // the `obtain` sub-tasks a missing input files). The default
            // selling lists the result once the node is done.
            let id = next_id(&mut counter);
            let mut node = Node::new(&id, NodeKind::ObtainItems { item: item.clone(), count: *count }, Vec::new(), *count);
            node.alternatives = crate::obtain_legs::produce_alternatives(item);
            nodes.push(node);
        }
        GoalKind::GetRich => {
            let id = next_id(&mut counter);
            let mut node = Node::new(&id, NodeKind::MakeMoney, Vec::new(), 0);
            // Salmon: the best mainland catch (Karamja lobsters: paid boat).
            node.alternatives.push(Alternative { method: "gather:fishing/raw_salmon".to_string(), est_ticks: 0, est_coins: 0 });
            node.alternatives.push(Alternative { method: "gather:mining/coal".to_string(), est_ticks: 0, est_coins: 0 });
            nodes.push(node);
        }
        GoalKind::MakeMarket => {
            // The maker's post (Q6): one leg, never ranked against money
            // methods (`resolve_money`); Iron never trades.
            let id = next_id(&mut counter);
            let mut node = Node::new(&id, NodeKind::MakeMoney, Vec::new(), 0);
            if !profile.ironman {
                node.alternatives.push(Alternative { method: crate::market_maker::MAKE_METHOD.to_string(), est_ticks: 6000, est_coins: 0 });
            }
            nodes.push(node);
        }
    }
    nodes
}

/// Area access: guild gates (services GUILDS, from the 274 scripts) climb
/// the skill and obtain the uniform first, the Champions' Guild waits on
/// its quest rung; the `access` node depends on all of them. Shared by
/// `access` goals and `access:<area>` blocks (`record_block_from`).
pub fn expand_access(area: &str, profile: &Profile, counter: &mut usize, level_of: &dyn Fn(&str) -> u8) -> Vec<Node> {
    expand_access_planned(area, profile, counter, level_of, NESTED_DEPTH, &crate::plan_read::PlanRead::prior())
}

/// `expand_access` whose climbs file their skeleton dependencies down to
/// `depth` nested quest/guild expansions.
pub fn expand_access_planned(area: &str, profile: &Profile, counter: &mut usize, level_of: &dyn Fn(&str) -> u8, depth: u8, read: &crate::plan_read::PlanRead) -> Vec<Node> {
    let mut nodes = Vec::new();
    let mut deps = Vec::new();
    if let Some((_, level, uniform)) = lifescape_gamedata::services::GUILDS.iter().find(|entry| entry.0 == area) {
        if let Some(skill) = guild_skill(area) {
            let mut after = String::new();
            let chain = crate::skeleton_deps::reach_chain_with(skill, level_of(skill), (*level).min(99) as u8, counter, &mut after, depth, read);
            if let Some(last) = chain.last() {
                deps.push(last.id.clone());
            }
            nodes.extend(chain);
        }
        if !uniform.is_empty() {
            let node = obtain_node(uniform, 1, Vec::new(), counter, profile.ironman);
            deps.push(node.id.clone());
            nodes.push(node);
        }
    } else if area == "champions_guild" {
        // The door turns away anyone under 32 quest points
        // (`champions_guild.rs2`): the quest-point ladder.
        let (rungs, last) = quest_point_rungs(32, "", profile, counter, level_of, depth, read);
        nodes.extend(rungs);
        deps.extend(last);
    }
    let id = next_id(counter);
    let mut node = Node::new(&id, NodeKind::AccessArea { area: area.to_string() }, deps, 1);
    node.alternatives.push(Alternative { method: format!("access:{area}"), est_ticks: 1200, est_coins: 0 });
    nodes.push(node);
    nodes
}

/// Quest expansion from the typed requirements: skill gates become
/// `reach_level` chains, item checks become `obtain_items`; the quest
/// node itself depends on all of them.
pub fn expand_quest(quest: &str, profile: &Profile, counter: &mut usize) -> Vec<Node> {
    expand_quest_from(quest, profile, counter, &|_| 1)
}

/// `expand_quest` with skill gates climbing from the live levels.
pub fn expand_quest_from(quest: &str, profile: &Profile, counter: &mut usize, level_of: &dyn Fn(&str) -> u8) -> Vec<Node> {
    expand_quest_planned(quest, profile, counter, level_of, NESTED_DEPTH, &crate::plan_read::PlanRead::prior())
}

/// `expand_quest_from` whose climbs file their skeleton dependencies down
/// to `depth` nested quest/guild expansions.
pub fn expand_quest_planned(quest: &str, profile: &Profile, counter: &mut usize, level_of: &dyn Fn(&str) -> u8, depth: u8, read: &crate::plan_read::PlanRead) -> Vec<Node> {
    expand_quest_inner(quest, profile, counter, level_of, depth, true, read)
}

/// Quest-point rungs (`quests::quest_point_ladder`) chained in order, each
/// rung's own requirements expanded (never its own ladder: the order
/// already reaches its gate). Returns the nodes and the last rung's id.
fn quest_point_rungs(points: i64, target: &str, profile: &Profile, counter: &mut usize, level_of: &dyn Fn(&str) -> u8, depth: u8, read: &crate::plan_read::PlanRead) -> (Vec<Node>, Option<String>) {
    let mut nodes: Vec<Node> = Vec::new();
    let mut previous: Option<String> = None;
    for rung in crate::quests::quest_point_ladder(points, target, &[]) {
        let mut chain = expand_quest_inner(&rung, profile, counter, level_of, depth.saturating_sub(1), false, read);
        if let (Some(before), Some(last)) = (previous.as_ref(), chain.last_mut()) {
            last.deps.push(before.clone());
        }
        previous = chain.last().map(|node| node.id.clone());
        nodes.extend(chain);
    }
    (nodes, previous)
}

fn expand_quest_inner(quest: &str, profile: &Profile, counter: &mut usize, level_of: &dyn Fn(&str) -> u8, depth: u8, with_ladder: bool, read: &crate::plan_read::PlanRead) -> Vec<Node> {
    let mut nodes = Vec::new();
    let mut deps = Vec::new();
    if let Some(required) = lifescape_gamedata::quest_requirements::by_key(quest) {
        for gate in &required.skills {
            let mut after = String::new();
            let chain = crate::skeleton_deps::reach_chain_with(&gate.skill, level_of(&gate.skill), gate.level.min(99) as u8, counter, &mut after, depth, read);
            if let Some(last) = chain.last() {
                deps.push(last.id.clone());
            }
            nodes.extend(chain);
        }
        for check in &required.items {
            // shortcut: worn-only gear still needs obtaining first; the
            // equip step itself is the quest script's job, not a node.
            let node = obtain_node(&check.item, check.count, Vec::new(), counter, profile.ironman);
            deps.push(node.id.clone());
            nodes.push(node);
        }
        // Quest-point gate (Black Knights' Fortress 12, Dragon Slayer 32):
        // the real ladder, cheapest points first, each rung expanded.
        if required.quest_points > 0 && with_ladder {
            let (rungs, last) = quest_point_rungs(required.quest_points, quest, profile, counter, level_of, depth, read);
            nodes.extend(rungs);
            deps.extend(last);
        }
    }
    let id = next_id(counter);
    let mut node = Node::new(&id, NodeKind::CompleteQuest { quest: quest.to_string() }, deps, 1);
    node.alternatives.push(Alternative { method: format!("quest:{quest}"), est_ticks: 6000, est_coins: 0 });
    nodes.push(node);
    nodes
}

/// Leaf resolution at `ready` time: price the alternatives with fresh
/// public numbers and lock the cheapest (trait-weighted, D15). Reach
/// and quest leaves keep their single method; only the estimate fills.
pub fn resolve_ready(node: &mut Node, ctx: &ReadyCtx) {
    node.batch = 0;
    node.buys_inputs = false;
    // A method a dependency cycle went through never resolves again
    // (`lifecycle_cycle`).
    let excluded = node.excluded.clone();
    node.alternatives.retain(|alt| !excluded.contains(&alt.method));
    resolve_method(node, ctx);
    place_on_site(node, ctx);
    crate::resolve_obtain::prefer_purchase_over_combat(node, ctx);
}

/// The brain's site choice for the locked method (game data, bot's
/// combat level): the node records where the bot goes; no safe site
/// yet blocks typed on the combat level to train (`missing:combat:N`),
/// a method no level makes safe blocks as unworkable.
pub(crate) fn place_on_site(node: &mut Node, ctx: &ReadyCtx) {
    node.site = None;
    node.toll = false;
    node.boat = false;
    if matches!(node.status, NodeStatus::Blocked { .. }) {
        return;
    }
    let Some(locked) = node.method.clone() else { return };
    // A market maker's post is its slice's tile by the fountain (Q6).
    if locked == crate::market_maker::MAKE_METHOD {
        node.site = ctx.maker_post;
        return;
    }
    // The locked method first, then every tool-feasible alternative: the
    // first with a site this bot works now wins (a money node falls back
    // from Karamja lobsters to what is reachable).
    let mut candidates = vec![locked.clone()];
    // Only field methods fall back: a purchase leg (`market:`, `shop:`,
    // the `market:bid` marker) was already judged by the resolver (coins,
    // D-A), and a site never revives one it refused.
    candidates.extend(
        node.alternatives
            .iter()
            .map(|alt| alt.method.clone())
            // A trade lot (R5) is never a fallback: its lot was sized on
            // its own lock.
            .filter(|method| *method != locked && !method.starts_with("market:") && !method.starts_with("shop:") && !method.starts_with("dialogue:") && !crate::trade_craft::is_trade(method) && method_feasible(method, ctx)),
    );
    let mut needed: Option<u8> = None;
    let mut fare_short: Option<i64> = None;
    let mut gate_need: Option<String> = None;
    let mut cooled = false;
    let mut full_everywhere = false;
    // A shop purchase walks to the nearest shop that stocks the object
    // (the planner's shop, `--shop`), never a gated one.
    if locked == "shop:buy" {
        if let NodeKind::ObtainItems { item, .. } = &node.kind {
            if crate::shop_site::every_shop_cooled(item, ctx) {
                node.method = None;
                node.site = None;
                node.status = NodeStatus::Blocked { reason: format!("cooled:{locked}") };
                return;
            }
            node.site = crate::shop_site::pick_shop(item, ctx).map(|(_, site)| site);
            // `pick_shop` keeps a keeper across the Al Kharid fence or on
            // Karamja only with the fares covered: the launch then carries
            // `toll` / `boat`.
            node.toll = node.site.is_some_and(|site| ctx.position.is_some_and(|from| lifescape_gamedata::toll::crossing(from, site)));
            node.boat = node.site.is_some_and(|(x, z, level)| lifescape_gamedata::access::behind_boat(x, z, level));
        }
        return;
    }
    // A dialogue seller (Karim): its spawn, the toll budgeted on top of
    // the purchase when the walk crosses the fence.
    if locked == "dialogue:buy" {
        if let NodeKind::ObtainItems { item, count } = &node.kind {
            let units = (*count - ctx.held).max(1);
            match crate::dialogue_site::pick_seller(item, units, ctx) {
                crate::dialogue_site::DialoguePick::At { site, toll } => {
                    node.site = Some(site);
                    node.toll = toll;
                }
                crate::dialogue_site::DialoguePick::Coins(coins) => {
                    node.method = None;
                    node.status = NodeStatus::Blocked { reason: format!("missing:coins:{coins}") };
                }
                crate::dialogue_site::DialoguePick::None => {
                    node.method = None;
                    node.status = NodeStatus::Blocked { reason: format!("unworkable:dialogue:buy (no safe F2P seller of {item})") };
                }
            }
        }
        return;
    }
    for method in &candidates {
        match crate::combat::pick_site_for(method, ctx) {
            crate::combat::SitePick::Anywhere => {
                node.method = Some(method.clone());
                return;
            }
            crate::combat::SitePick::At(site) => {
                node.method = Some(method.clone());
                node.site = Some(site);
                return;
            }
            // Across the Al Kharid fence with the fare budgeted: the
            // launch carries `toll` (user decision 2026-11-13).
            crate::combat::SitePick::Tolled(site) => {
                node.method = Some(method.clone());
                node.site = Some(site);
                node.toll = true;
                return;
            }
            // Karamja with the round trip budgeted: the launch carries
            // `boat` (and `toll` when the trip crosses the fence too).
            crate::combat::SitePick::Boat { site, toll } => {
                node.method = Some(method.clone());
                node.site = Some(site);
                node.toll = toll;
                node.boat = true;
                return;
            }
            crate::combat::SitePick::Fare(coins) => fare_short = Some(coins),
            // A guild door (skill + uniform) or a key: the dependency that
            // opens it, filed by `record_block_from` (`access:<guild>`
            // expands the climb and the uniform, a key is an obtain).
            crate::combat::SitePick::Access(area, key) => {
                gate_need.get_or_insert_with(|| key.map(str::to_string).unwrap_or_else(|| format!("access:{area}")));
            }
            crate::combat::SitePick::Needs(level) => needed = Some(needed.map_or(level, |known| known.min(level))),
            crate::combat::SitePick::Cooled => cooled = true,
            crate::combat::SitePick::Full => full_everywhere = true,
        }
    }
    node.method = None;
    node.status = match (fare_short, needed) {
        // Every site this bot works was cooled for it (P1/D4): the
        // method waits out the cooldown, never a combat need.
        (None, _) if cooled && gate_need.is_none() => NodeStatus::Blocked { reason: format!("cooled:{locked}") },
        // Every site of every candidate is full (D6): waits like a cooled
        // method, the crowd thins out by itself.
        (None, _) if full_everywhere && gate_need.is_none() => NodeStatus::Blocked { reason: format!("full:{locked}") },
        // The fare is the cheapest unlock: a `have_coins` dependency
        // earns it, then the node relaunches with `toll`.
        (Some(coins), _) => NodeStatus::Blocked { reason: format!("missing:coins:{coins}") },
        (None, _) if gate_need.is_some() => NodeStatus::Blocked { reason: format!("missing:{}", gate_need.unwrap_or_default()) },
        (None, Some(level)) if level < lifescape_gamedata::danger::NEVER => NodeStatus::Blocked { reason: format!("missing:combat:{level}") },
        _ => NodeStatus::Blocked { reason: format!("unworkable:{locked} (no site safe at any level with a bank in reach)") },
    };
}

fn resolve_method(node: &mut Node, ctx: &ReadyCtx) {
    match &node.kind {
        NodeKind::ObtainItems { item, count } => {
            let (item, count) = (item.clone(), *count);
            crate::resolve_obtain::resolve_obtain(node, &item, count, ctx);
        }
        NodeKind::MakeMoney | NodeKind::HaveCoins { .. } => {
            crate::resolve_obtain::resolve_money(node, ctx);
        }
        NodeKind::SpaceCleanup => {
            node.alternatives = vec![Alternative {
                method: "space:cleanup".to_string(),
                est_ticks: 6000,
                est_coins: 0,
            }];
            node.method = Some("space:cleanup".to_string());
        }
        _ => {
            if node.method.is_none() && lock_training_pick(node, ctx) {
                return;
            }
            if node.method.is_none() {
                // The tranche method may name gear the pack lacks: only a
                // workable method locks; otherwise block typed on the
                // tool so the obtain chain funds it (never a substitute
                // method, never a doomed dispatch).
                // Workable now (food in hand where the spot bites) first.
                let workable = node
                    .alternatives
                    .iter()
                    .find(|alt| crate::food_gate::ready_now(&alt.method, ctx))
                    .or_else(|| node.alternatives.iter().find(|alt| method_feasible(&alt.method, ctx)))
                    .map(|alt| alt.method.clone());
                match workable {
                    Some(method) => node.method = Some(method),
                    None => {
                        if let Some(first) = node.alternatives.first() {
                            // Gear held: unworkable for another reason
                            // (quarantine, quest, combat fit), never a held
                            // tool named missing (a buy-held loop).
                            let missing = required_tool(&first.method).and_then(|need| need.missing(ctx));
                            let sick = ctx.sick(&first.method);
                            node.method = None;
                            node.status = match (missing, sick) {
                                (Some(tool), None) => NodeStatus::Blocked { reason: format!("missing:{tool}") },
                                (_, Some(why)) => NodeStatus::Blocked { reason: format!("{why}:{}", first.method) },
                                _ => NodeStatus::Blocked { reason: format!("unworkable:{}", first.method) },
                            };
                        }
                    }
                }
            }
            for alt in &mut node.alternatives {
                if alt.est_ticks == 0 {
                    alt.est_ticks = 6000;
                }
            }
        }
    }
}

/// A priced tranche (`training`), or a combat level's climb read on the
/// training guide (R7 suite): the brain's pick locks with its batch
/// (alchemy casts). The tools it lacks (the hammer) and the inputs the
/// pick found short block typed, all at once, the pick's method kept so
/// the obtain sub-tasks size on it (`replan::need_count_from`). `false`:
/// no pick, or its quest is missing (the static method resolves).
fn lock_training_pick(node: &mut Node, ctx: &ReadyCtx) -> bool {
    if !matches!(node.kind, NodeKind::ReachLevel { .. } | NodeKind::ReachCombat { .. }) {
        return false;
    }
    let Some(pick) = ctx.training.as_ref() else { return false };
    if node.excluded.contains(&pick.method) || ctx.sick(&pick.method).is_some() || crate::quests::missing_quest(&pick.method, None, ctx).is_some() {
        return false;
    }
    let mut missing: Vec<String> = Vec::new();
    if let Some(need) = required_tool(&pick.method) {
        missing.extend(need.tools.iter().filter(|tool| !gear_ok(tool, ctx)).cloned());
    }
    // Bank unseen: the script finds banked inputs or blocks itself.
    if ctx.bank_known {
        for input in &pick.missing {
            if !missing.contains(input) {
                missing.push(input.clone());
            }
        }
    }
    node.method = Some(pick.method.clone());
    node.batch = pick.batch;
    // A market pick buys its inputs (`replan`: market legs only).
    node.buys_inputs = pick.buys_inputs;
    if !missing.is_empty() {
        node.status = NodeStatus::Blocked { reason: format!("missing:{}", missing.join(",")) };
    }
    true
}

#[cfg(test)]
#[path = "expand_fish_tests.rs"]
mod fish_tests;

#[cfg(test)]
#[path = "expand_tests.rs"]
mod tests;
