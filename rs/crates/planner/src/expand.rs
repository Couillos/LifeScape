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
    let mut shop = false;
    for source in lifescape_gamedata::item_sources::sources_for_name(item) {
        match source {
            ItemSource::Woodcut { .. }
            | ItemSource::Mine { .. }
            | ItemSource::Fish { .. }
            | ItemSource::Drop { .. } => farm = true,
            ItemSource::Shop { .. } => shop = true,
            _ => {}
        }
    }
    // D-A (PRD3): an NPC shop leg only for objects nobody makes or
    // gathers in F2P; producible objects come from the market or work.
    (farm, shop && lifescape_gamedata::obtain::merchant_buy_allowed(item))
}


fn next_id(counter: &mut usize) -> String {
    *counter += 1;
    format!("n{}", *counter)
}

/// Reach tranches from the live level `from` (1 when unknown) to `to`:
/// never a tranche at or below `from` (a level already held is never
/// replayed), empty when the bot already holds `to`.
pub(crate) fn reach_chain(skill: &str, from: u8, to: u8, counter: &mut usize, after: &mut String) -> Vec<Node> {
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
        node.alternatives.push(Alternative {
            method: reach_method(skill, start),
            est_ticks: 0,
            est_coins: 0,
        });
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
    let mut counter = 0usize;
    let mut nodes = Vec::new();
    match goal {
        GoalKind::MaxSkills { skills, level } => {
            for skill in skills {
                let mut after = String::new();
                nodes.extend(reach_chain(skill, level_of(skill), *level, &mut counter, &mut after));
            }
        }
        GoalKind::Obtain { item, count } => {
            nodes.push(obtain_node(item, *count, Vec::new(), &mut counter, profile.ironman));
        }
        GoalKind::MakeMoney => {
            let id = next_id(&mut counter);
            let mut node = Node::new(&id, NodeKind::MakeMoney, Vec::new(), 0);
            node.alternatives.push(Alternative { method: "gather:fishing/raw_lobster".to_string(), est_ticks: 0, est_coins: 0 });
            node.alternatives.push(Alternative { method: "gather:woodcut/yew_logs".to_string(), est_ticks: 0, est_coins: 0 });
            if !profile.ironman {
                node.alternatives.push(Alternative { method: "market:flip".to_string(), est_ticks: 0, est_coins: 0 });
            }
            nodes.push(node);
        }
        GoalKind::CompleteQuest { quest } => {
            nodes.extend(expand_quest_from(quest, profile, &mut counter, level_of));
        }
        GoalKind::EquipBest { style } => {
            // PRD3 lot 5: the resolver's set for a fresh bot (levels and
            // quests unknown at creation: the level-1 set), one obtain
            // per piece, then the equip step. Upgrades ride the brain's
            // wishlist as levels rise.
            let picks = lifescape_gamedata::gear::best_gear(lifescape_gamedata::gear::Style::parse(style), &[], &[]);
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
            // Guild gates (services GUILDS, from the 274 scripts): the
            // skill chain first, then the uniform to obtain — the access
            // node never stands depless when a requirement exists.
            let mut deps = Vec::new();
            if let Some((_, level, uniform)) = lifescape_gamedata::services::GUILDS.iter().find(|entry| entry.0 == area) {
                if let Some(skill) = guild_skill(area) {
                    let mut after = String::new();
                    let chain = reach_chain(skill, level_of(skill), (*level).min(99) as u8, &mut counter, &mut after);
                    if let Some(last) = chain.last() {
                        deps.push(last.id.clone());
                    }
                    nodes.extend(chain);
                }
                if !uniform.is_empty() {
                    let node = obtain_node(uniform, 1, Vec::new(), &mut counter, profile.ironman);
                    deps.push(node.id.clone());
                    nodes.push(node);
                }
            } else if area == "champions_guild" {
                // shortcut: qp:32 needs the 32-QP ladder, but quest rewards
                // are not in the dumps, so no minimal quest set derives —
                // the cheapest QP rung (Cook's Assistant) gates the node
                // until the ladder lands.
                let quest_nodes = expand_quest_from("cook", profile, &mut counter, level_of);
                if let Some(last) = quest_nodes.last() {
                    deps.push(last.id.clone());
                }
                nodes.extend(quest_nodes);
            }
            let id = next_id(&mut counter);
            let mut node = Node::new(&id, NodeKind::AccessArea { area: area.clone() }, deps, 1);
            node.alternatives.push(Alternative { method: format!("access:{area}"), est_ticks: 1200, est_coins: 0 });
            nodes.push(node);
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
        GoalKind::GetRich { merchant } => {
            let is_merchant = merchant.unwrap_or(profile.archetype == crate::model::Archetype::Merchant);
            let id = next_id(&mut counter);
            let mut node = Node::new(&id, NodeKind::MakeMoney, Vec::new(), 0);
            if is_merchant && !profile.ironman {
                node.alternatives.push(Alternative { method: "market:flip".to_string(), est_ticks: 0, est_coins: 0 });
            } else {
                node.alternatives.push(Alternative { method: "gather:fishing/raw_lobster".to_string(), est_ticks: 0, est_coins: 0 });
                node.alternatives.push(Alternative { method: "gather:mining/coal".to_string(), est_ticks: 0, est_coins: 0 });
            }
            nodes.push(node);
        }
    }
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
    let mut nodes = Vec::new();
    let mut deps = Vec::new();
    if let Some(required) = lifescape_gamedata::quest_requirements::by_key(quest) {
        for gate in &required.skills {
            let mut after = String::new();
            let chain = reach_chain(&gate.skill, level_of(&gate.skill), gate.level.min(99) as u8, counter, &mut after);
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
        // shortcut: quest-point gates collapse onto Cook's Assistant (1
        // QP, no requirements): the cheapest QP in F2P. A full QP ladder
        // (ordering all 18 quests by gate) is deferred; L7 goals run one
        // low-QP quest, where this never triggers.
        if required.quest_points > 0 && quest != "cook" {
            let id = next_id(counter);
            let dep = Node::new(&id, NodeKind::CompleteQuest { quest: "cook".to_string() }, Vec::new(), 1);
            deps.push(dep.id.clone());
            nodes.push(dep);
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
    resolve_method(node, ctx);
    place_on_site(node, ctx);
}

/// The brain's site choice for the locked method (game data, bot's
/// combat level): the node records where the bot goes; no safe site
/// yet blocks typed on the combat level to train (`missing:combat:N`),
/// a method no level makes safe blocks as unworkable.
fn place_on_site(node: &mut Node, ctx: &ReadyCtx) {
    node.site = None;
    if matches!(node.status, NodeStatus::Blocked { .. }) {
        return;
    }
    let Some(locked) = node.method.clone() else { return };
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
            .filter(|method| *method != locked && !method.starts_with("market:") && !method.starts_with("shop:") && method_feasible(method, ctx)),
    );
    let mut needed: Option<u8> = None;
    for method in &candidates {
        match crate::combat::pick_site(method, ctx.combat, ctx.position) {
            crate::combat::SitePick::Anywhere => {
                node.method = Some(method.clone());
                return;
            }
            crate::combat::SitePick::At(site) => {
                node.method = Some(method.clone());
                node.site = Some(site);
                return;
            }
            crate::combat::SitePick::Needs(level) => needed = Some(needed.map_or(level, |known| known.min(level))),
        }
    }
    node.method = None;
    node.status = match needed {
        Some(level) if level < lifescape_gamedata::danger::NEVER => NodeStatus::Blocked { reason: format!("missing:combat:{level}") },
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
            if node.method.is_none() {
                // The tranche method may name gear the pack lacks: only a
                // workable method locks; otherwise block typed on the
                // tool so the obtain chain funds it (never a substitute
                // method, never a doomed dispatch).
                let workable = node.alternatives.iter().find(|alt| method_feasible(&alt.method, ctx)).map(|alt| alt.method.clone());
                match workable {
                    Some(method) => node.method = Some(method),
                    None => {
                        if let Some(first) = node.alternatives.first() {
                            let missing = required_tool(&first.method).map(|need| {
                                if gear_ok(need.tool, ctx) { need.bait.unwrap_or(need.tool) } else { need.tool }
                            });
                            node.method = None;
                            node.status = match missing {
                                Some(tool) => NodeStatus::Blocked { reason: format!("missing:{tool}") },
                                None => NodeStatus::Blocked { reason: format!("unworkable:{}", first.method) },
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

#[cfg(test)]
#[path = "expand_tests.rs"]
mod tests;
