//! Goal expansion (HTN squelette): a goal becomes a full node chain at
//! creation; leaf methods resolve au plus tard, when the node turns
//! ready, because prices and yields move. One task = one script + one
//! resource + one objective (D21): the brain fixes the resource, the
//! script only finds where to exploit it.

use crate::cost::{choose_market_or_farm, cost_farm, cost_market_buy};
use crate::model::{Alternative, GoalKind, Node, NodeKind, Profile};

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
    (farm, shop)
}

fn tradeable_f2p(item: &str) -> bool {
    lifescape_gamedata::objs_by_name(item)
        .into_iter()
        .next()
        .is_some_and(|entry| entry.tradeable && entry.members != Some(true))
}

fn next_id(counter: &mut usize) -> String {
    *counter += 1;
    format!("n{}", *counter)
}

/// D21 level tranches per skill: the brain fixes one resource per
/// tranche, the script never changes tier mid-task. P1 tiers prefer the
/// classic method (§8); one tier per 10 levels where the dump offers no
/// finer step.
pub fn tranches(skill: &str, from: u8, to: u8) -> Vec<u8> {
    let cuts: &[u8] = match skill {
        "fishing" => &[5, 10, 20, 25, 35, 40],
        "woodcutting" => &[15, 30, 45, 60, 75],
        "mining" => &[10, 15, 20, 30, 40, 55],
        "cooking" => &[10, 20, 30, 45],
        "firemaking" => &[15, 30, 60],
        "crafting" => &[5, 7, 8, 16, 20, 27, 34, 43, 70],
        "smithing" => &[15, 20, 30, 40, 50, 70, 85],
        "runecraft" => &[2, 5, 9, 14, 20],
        "attack" | "strength" | "defence" | "hitpoints" => &[10, 20, 30, 40],
        "magic" | "ranged" => &[10, 20, 30, 40],
        // Prayer buries where it fights (no standalone bury script):
        // one tranche, burial in the combat loop.
        "prayer" => &[],
        _ => &[10, 20, 30],
    };
    let mut out: Vec<u8> = cuts.iter().copied().filter(|cut| *cut > from && *cut < to).collect();
    out.push(to);
    out
}

/// Resource the brain fixes for one reach tranche (product of the tier
/// worked during the tranche).
pub fn tranche_resource(skill: &str, level: u8) -> String {
    match skill {
        "fishing" => {
            if level >= 40 {
                "raw_lobster"
            } else if level >= 35 {
                "raw_tuna"
            } else if level >= 25 {
                "raw_pike"
            } else if level >= 20 {
                "raw_trout"
            } else if level >= 5 {
                "raw_sardine"
            } else {
                "raw_shrimp"
            }
            .to_string()
        }
        "woodcutting" => {
            if level >= 60 {
                "yew_logs"
            } else if level >= 45 {
                "maple_logs"
            } else if level >= 30 {
                "willow_logs"
            } else if level >= 15 {
                "oak_logs"
            } else {
                "logs"
            }
            .to_string()
        }
        "mining" => {
            if level >= 55 {
                "mithril_ore"
            } else if level >= 40 {
                "gold_ore"
            } else if level >= 30 {
                "coal"
            } else if level >= 15 {
                "iron_ore"
            } else {
                "copper_ore"
            }
            .to_string()
        }
        "cooking" => {
            if level >= 30 {
                "raw_lobster"
            } else if level >= 15 {
                "raw_trout"
            } else {
                "raw_shrimp"
            }
            .to_string()
        }
        "firemaking" => {
            if level >= 60 {
                "yew_logs"
            } else if level >= 30 {
                "willow_logs"
            } else if level >= 15 {
                "oak_logs"
            } else {
                "logs"
            }
            .to_string()
        }
        "smithing" => {
            if level >= 85 {
                "rune_bar"
            } else if level >= 70 {
                "adamantite_bar"
            } else if level >= 50 {
                "mithril_bar"
            } else if level >= 40 {
                "gold_bar"
            } else if level >= 30 {
                "steel_bar"
            } else if level >= 20 {
                "silver_bar"
            } else if level >= 15 {
                "iron_bar"
            } else {
                "bronze_bar"
            }
            .to_string()
        }
        "crafting" => {
            if level >= 70 {
                "diamond_amulet"
            } else if level >= 43 {
                "diamond_ring"
            } else if level >= 34 {
                "ruby_ring"
            } else if level >= 27 {
                "emerald_ring"
            } else if level >= 20 {
                "sapphire_ring"
            } else if level >= 16 {
                "holy_symbol"
            } else if level >= 8 {
                "gold_amulet"
            } else if level >= 7 {
                "pie_dish"
            } else if level >= 5 {
                "gold_ring"
            } else {
                "ball_of_wool"
            }
            .to_string()
        }
        "runecraft" => {
            if level >= 20 {
                "bodyrune"
            } else if level >= 14 {
                "firerune"
            } else if level >= 9 {
                "earthrune"
            } else if level >= 5 {
                "waterrune"
            } else if level >= 2 {
                "mindrune"
            } else {
                "airrune"
            }
            .to_string()
        }
        "prayer" => "bones".to_string(),
        "attack" | "strength" | "defence" | "hitpoints" | "ranged" | "magic" => {
            if level >= 20 {
                "cow_lumbridge"
            } else {
                "chicken_lumbridge"
            }
            .to_string()
        }
        _ => "chicken_lumbridge".to_string(),
    }
}

/// Script-side method for one reach tranche.
pub fn reach_method(skill: &str, level: u8) -> String {
    match skill {
        "fishing" => format!("gather:fishing/{}", tranche_resource(skill, level)),
        "woodcutting" => format!("gather:woodcut/{}", tranche_resource(skill, level)),
        "mining" => format!("gather:mining/{}", tranche_resource(skill, level)),
        "cooking" => format!("cook/{}", tranche_resource(skill, level)),
        "firemaking" => format!("fire:{}", tranche_resource(skill, level)),
        // Smelting trains smithing 1→99 (bronze 1, quest squire 1→29);
        // plates run through the money chains, never the reach tiers.
        "smithing" => format!("smelt:{}", tranche_resource(skill, level)),
        // One P1-first family per tier (D64): wheel, gold, pie dish,
        // gold amulet, silver symbol, then the gem ladder.
        "crafting" => {
            let resource = tranche_resource(skill, level);
            if resource == "ball_of_wool" {
                format!("spin:{resource}")
            } else if resource == "pie_dish" || resource == "pot" || resource == "bowl" {
                format!("pottery:{resource}")
            } else {
                format!("jewelry:{resource}")
            }
        }
        "runecraft" => format!("runecraft:{}", tranche_resource(skill, level)),
        // No standalone bury script (in-loop helper only): prayer fights
        // where the bones drop and buries each kill.
        "prayer" => "fight:melee/cows".to_string(),
        "ranged" => format!("fight:ranged/{}", tranche_resource(skill, level)),
        "magic" => format!("fight:magic/{}", tranche_resource(skill, level)),
        "attack" | "strength" | "defence" | "hitpoints" => {
            format!("fight:melee/{}", tranche_resource(skill, level))
        }
        _ => format!("fight:melee/{}", tranche_resource(skill, level)),
    }
}

fn reach_chain(skill: &str, from: u8, to: u8, counter: &mut usize, after: &mut String) -> Vec<Node> {
    let mut out = Vec::new();
    for level in tranches(skill, from, to) {
        let id = next_id(counter);
        let mut node = Node::new(
            &id,
            NodeKind::ReachLevel { skill: skill.to_string(), level },
            if after.is_empty() { Vec::new() } else { vec![after.clone()] },
            i64::from(level),
        );
        node.alternatives.push(Alternative {
            method: reach_method(skill, level),
            est_ticks: 0,
            est_coins: 0,
        });
        *after = id.clone();
        out.push(node);
    }
    out
}

fn obtain_node(item: &str, count: i64, deps: Vec<String>, counter: &mut usize, ironman: bool) -> Node {
    let id = next_id(counter);
    let mut node = Node::new(&id, NodeKind::ObtainItems { item: item.to_string(), count }, deps, count);
    let (farmable, shopable) = source_flags(item);
    // No executable `shop:buy` for unsourced items: the market leg stays
    // as the generic fallback (the carnet may still fill it).
    if farmable {
        node.alternatives.push(Alternative { method: format!("farm:{item}"), est_ticks: 0, est_coins: 0 });
    }
    if shopable {
        node.alternatives.push(Alternative { method: "shop:buy".to_string(), est_ticks: 0, est_coins: 0 });
    }
    if !ironman && (tradeable_f2p(item) || node.alternatives.is_empty()) {
        node.alternatives.push(Alternative { method: "market:buy".to_string(), est_ticks: 0, est_coins: 0 });
    }
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

/// Full skeleton at creation: every step present, leaf methods resolved
/// later (alternatives carry est 0 until `resolve_ready` prices them).
pub fn expand_goal(goal: &GoalKind, profile: &Profile) -> Vec<Node> {
    let mut counter = 0usize;
    let mut nodes = Vec::new();
    match goal {
        GoalKind::MaxSkills { skills, level } => {
            for skill in skills {
                let mut after = String::new();
                nodes.extend(reach_chain(skill, 1, *level, &mut counter, &mut after));
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
            nodes.extend(expand_quest(quest, profile, &mut counter));
        }
        GoalKind::EquipBest { style } => {
            let weapon = match style.as_str() {
                "ranged" => "oak_shortbow",
                "magic" => "staff_of_air",
                _ => "iron_sword",
            };
            let obtain = obtain_node(weapon, 1, Vec::new(), &mut counter, profile.ironman);
            let obtain_id = obtain.id.clone();
            nodes.push(obtain);
            let id = next_id(&mut counter);
            let mut node = Node::new(
                &id,
                NodeKind::EquipBest { style: style.clone() },
                vec![obtain_id],
                1,
            );
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
                    let chain = reach_chain(skill, 1, (*level).min(99) as u8, &mut counter, &mut after);
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
                let quest_nodes = expand_quest("cook", profile, &mut counter);
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
    let mut nodes = Vec::new();
    let mut deps = Vec::new();
    if let Some(required) = lifescape_gamedata::quest_requirements::by_key(quest) {
        for gate in &required.skills {
            let mut after = String::new();
            let chain = reach_chain(&gate.skill, 1, gate.level.min(99) as u8, counter, &mut after);
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
    match &node.kind {
        NodeKind::ObtainItems { item, count } => {
            let mut priced = Vec::new();
            let (farmable, shopable) = source_flags(item);
            if farmable {
                let farm = cost_farm(*count, ctx.yield_per_6000.max(1.0), ctx.trip_ticks);
                priced.push(Alternative { method: format!("farm:{item}"), est_ticks: farm, est_coins: 0 });
            }
            if shopable {
                let shop = cost_market_buy(ctx.trip_ticks, ctx.shop_price.max(1), *count, ctx.coins_per_tick.max(0.01));
                let cover = ctx.shop_price.max(0) * (*count).max(0);
                if cover <= ctx.coins {
                    priced.push(Alternative { method: "shop:buy".to_string(), est_ticks: shop, est_coins: cover });
                }
            }
            if !ctx.ironman && (tradeable_f2p(item) || priced.is_empty()) {
                let market = cost_market_buy(ctx.trip_ticks, ctx.price.max(1), *count, ctx.coins_per_tick.max(0.01));
                let cover = ctx.price.max(0) * (*count).max(0);
                // Uncovered market legs never list: the carnet would
                // refuse them, so the planner does not offer them.
                if cover <= ctx.coins || priced.is_empty() {
                    priced.push(Alternative { method: "market:buy".to_string(), est_ticks: market, est_coins: cover });
                }
            }
            priced.sort_by_key(|alt| alt.est_ticks);
            let pick = if ctx.ironman {
                priced.first().cloned()
            } else {
                let market = priced.iter().find(|alt| alt.method == "market:buy").cloned();
                let best_other = priced.iter().filter(|alt| alt.method != "market:buy").min_by_key(|alt| alt.est_ticks).cloned();
                match (market, best_other) {
                    (Some(buy), Some(other)) => {
                        if choose_market_or_farm(buy.est_ticks, other.est_ticks, ctx.market_affinity) {
                            Some(buy)
                        } else {
                            Some(other)
                        }
                    }
                    (Some(buy), None) => Some(buy),
                    (None, Some(other)) => Some(other),
                    (None, None) => None,
                }
            };
            node.alternatives = priced;
            node.method = pick.map(|alt| alt.method);
        }
        NodeKind::MakeMoney => {
            for alt in &mut node.alternatives {
                if alt.method == "market:flip" {
                    alt.est_ticks = 6000;
                } else {
                    alt.est_ticks = cost_farm(200, ctx.yield_per_6000.max(1.0), ctx.trip_ticks);
                }
            }
            node.alternatives.sort_by_key(|alt| alt.est_ticks);
            node.method = node.alternatives.first().map(|alt| alt.method.clone());
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
                node.method = node.alternatives.first().map(|alt| alt.method.clone());
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
mod tests {
    use super::{expand_goal, reach_method, resolve_ready, tranches, ReadyCtx};
    use crate::model::{Archetype, GoalKind, Node, NodeKind, Profile};

    fn profile() -> Profile {
        Profile::new(false, 0.72, 0.4, 7, Archetype::Producer)
    }

    #[test]
    fn max_skills_expands_tranches_in_order() {
        let nodes = expand_goal(&GoalKind::MaxSkills { skills: vec!["fishing".to_string()], level: 40 }, &profile());
        let levels: Vec<u8> = nodes
            .iter()
            .filter_map(|node| match &node.kind {
                NodeKind::ReachLevel { level, .. } => Some(*level),
                _ => None,
            })
            .collect();
        assert_eq!(levels, vec![5, 10, 20, 25, 35, 40], "D21 tranches up to the goal");
        for pair in nodes.windows(2) {
            assert_eq!(pair[1].deps, vec![pair[0].id.clone()], "linear chain");
        }
        assert!(nodes.iter().all(|node| node.method.is_none()), "methods resolve at ready, not at creation");
    }

    #[test]
    fn three_goals_expand() {
        let quest = expand_goal(&GoalKind::CompleteQuest { quest: "cook".to_string() }, &profile());
        assert!(quest.iter().any(|node| matches!(&node.kind, NodeKind::ObtainItems { item, .. } if item == "bucket_milk")), "milk sub-task");
        assert!(quest.iter().any(|node| matches!(&node.kind, NodeKind::ObtainItems { item, .. } if item == "pot_flour")), "flour sub-task");
        assert!(quest.iter().any(|node| matches!(&node.kind, NodeKind::CompleteQuest { quest } if quest == "cook")), "quest leaf");
        let leaf = quest.last().expect("leaf");
        assert_eq!(leaf.deps.len(), 3, "milk + egg + flour gate the quest");

        let rich = expand_goal(&GoalKind::GetRich { merchant: Some(true) }, &profile());
        assert_eq!(rich.len(), 1);
        assert!(rich[0].alternatives.iter().any(|alt| alt.method == "market:flip"), "merchant flips");

        let equip = expand_goal(&GoalKind::EquipBest { style: "melee".to_string() }, &profile());
        assert_eq!(equip.len(), 2, "obtain then equip");
        assert!(matches!(equip[1].kind, NodeKind::EquipBest { .. }));
    }

    #[test]
    fn tranche_resources_are_fixed() {
        assert_eq!(reach_method("fishing", 20), "gather:fishing/raw_trout");
        assert_eq!(reach_method("woodcutting", 30), "gather:woodcut/willow_logs");
        assert_eq!(reach_method("mining", 30), "gather:mining/coal");
        assert_eq!(tranches("fishing", 1, 40), vec![5, 10, 20, 25, 35, 40]);
    }

    /// P3.6: every skill owns its reach tiers and family methods (D21,
    /// catalogue §8). Tiers never move without a catalogue reason.
    #[test]
    fn every_skill_owns_its_reach_tiers() {
        assert_eq!(tranches("firemaking", 1, 99), vec![15, 30, 60, 99]);
        assert_eq!(tranches("smithing", 1, 99), vec![15, 20, 30, 40, 50, 70, 85, 99]);
        assert_eq!(tranches("crafting", 1, 99), vec![5, 7, 8, 16, 20, 27, 34, 43, 70, 99]);
        assert_eq!(tranches("runecraft", 1, 99), vec![2, 5, 9, 14, 20, 99]);
        assert_eq!(tranches("prayer", 1, 99), vec![99], "one tranche, burial in-loop");
        assert_eq!(tranches("magic", 1, 99), vec![10, 20, 30, 40, 99]);
        assert_eq!(reach_method("firemaking", 30), "fire:willow_logs");
        assert_eq!(reach_method("smithing", 30), "smelt:steel_bar");
        assert_eq!(reach_method("smithing", 1), "smelt:bronze_bar");
        assert_eq!(reach_method("crafting", 1), "spin:ball_of_wool");
        assert_eq!(reach_method("crafting", 7), "pottery:pie_dish");
        assert_eq!(reach_method("crafting", 8), "jewelry:gold_amulet");
        assert_eq!(reach_method("crafting", 70), "jewelry:diamond_amulet");
        assert_eq!(reach_method("runecraft", 9), "runecraft:earthrune");
        assert_eq!(reach_method("prayer", 99), "fight:melee/cows");
        assert_eq!(reach_method("ranged", 20), "fight:ranged/cow_lumbridge");
        assert_eq!(reach_method("magic", 20), "fight:magic/cow_lumbridge");
    }

    #[test]
    fn iron_never_lists_market() {
        let iron = Profile::new(true, 0.9, 0.4, 7, Archetype::Producer);
        let nodes = expand_goal(&GoalKind::Obtain { item: "feather".to_string(), count: 100 }, &iron);
        assert!(nodes[0].alternatives.iter().all(|alt| alt.method != "market:buy"), "no market for Iron");
        let rich = expand_goal(&GoalKind::GetRich { merchant: Some(true) }, &iron);
        assert!(rich[0].alternatives.iter().all(|alt| alt.method != "market:flip"), "Iron never merchants");
    }

    #[test]
    fn tools_never_farm_shops_instead() {
        // A net comes from shops, never from the field: the farm leg
        // must not exist, or the brain would hunt chickens for tackle.
        let nodes = expand_goal(&GoalKind::Obtain { item: "net".to_string(), count: 1 }, &profile());
        let methods: Vec<&str> = nodes[0].alternatives.iter().map(|alt| alt.method.as_str()).collect();
        assert!(!methods.iter().any(|method| method.starts_with("farm:")), "nets never farm, got {methods:?}");
        assert!(methods.contains(&"shop:buy"), "nets come from shops, got {methods:?}");
        // Feathers drop from chickens: the farm leg stays.
        let feather = expand_goal(&GoalKind::Obtain { item: "feather".to_string(), count: 100 }, &profile());
        assert!(feather[0].alternatives.iter().any(|alt| alt.method == "farm:feather"), "feathers farm");
    }

    #[test]
    fn access_area_derives_guild_deps() {
        // Cooks: cooking 32 chain plus the toque to obtain, gating the
        // access node; mining: level only, no uniform; champions: the
        // cheapest QP rung; unknown zones stay bare (no known gate).
        let cooks = expand_goal(&GoalKind::AccessArea { area: "cooks_guild".to_string() }, &profile());
        let leaf = cooks.last().expect("access leaf");
        assert!(matches!(leaf.kind, NodeKind::AccessArea { .. }), "leaf accesses, got {:?}", leaf.kind);
        assert!(!leaf.deps.is_empty(), "guild gate leaves no depless node");
        assert!(cooks.iter().any(|node| matches!(&node.kind, NodeKind::ReachLevel { skill, level } if skill == "cooking" && *level == 32)));
        assert!(cooks.iter().any(|node| matches!(&node.kind, NodeKind::ObtainItems { item, .. } if item == "chefs_hat")));
        let mining = expand_goal(&GoalKind::AccessArea { area: "mining_guild".to_string() }, &profile());
        let leaf = mining.last().expect("access leaf");
        assert!(!leaf.deps.is_empty(), "level gate leaves no depless node");
        assert!(mining.iter().any(|node| matches!(&node.kind, NodeKind::ReachLevel { skill, level } if skill == "mining" && *level == 60)));
        assert!(!mining.iter().any(|node| matches!(node.kind, NodeKind::ObtainItems { .. })), "no uniform on the mining guild");
        let champions = expand_goal(&GoalKind::AccessArea { area: "champions_guild".to_string() }, &profile());
        let leaf = champions.last().expect("access leaf");
        assert!(!leaf.deps.is_empty(), "qp gate leaves no depless node");
        assert!(champions.iter().any(|node| matches!(&node.kind, NodeKind::CompleteQuest { quest } if quest == "cook")));
        let bare = expand_goal(&GoalKind::AccessArea { area: "karamja".to_string() }, &profile());
        assert_eq!(bare.len(), 1, "unknown zone stays bare, got {bare:?}");
    }

    #[test]
    fn space_cleanup_locks_its_method() {
        let mut node = Node::new("n5", NodeKind::SpaceCleanup, Vec::new(), 0);
        resolve_ready(&mut node, &ReadyCtx::blind(0.5));
        assert_eq!(node.method.as_deref(), Some("space:cleanup"), "cleanup resolves directly");
    }

    #[test]
    fn ready_resolution_prices_and_picks() {
        let mut nodes = expand_goal(&GoalKind::Obtain { item: "feather".to_string(), count: 2000 }, &profile());
        let mut ctx = ReadyCtx::blind(0.72);
        ctx.coins_per_tick = 2.0;
        ctx.price = 3;
        ctx.shop_price = 5;
        ctx.yield_per_6000 = 3500.0;
        ctx.coins = 100000;
        resolve_ready(&mut nodes[0], &ctx);
        assert!(nodes[0].method.is_some(), "a method locks at ready");
        assert!(nodes[0].alternatives.iter().any(|alt| alt.est_ticks > 0), "estimates fill au plus tard");
        // Cheap market + market-friendly trait buys; same numbers with a
        // market-hostile trait farms instead.
        let bought = nodes[0].method.clone().unwrap();
        let mut hostile = ctx.clone();
        hostile.market_affinity = 0.0;
        let mut second = nodes[0].clone();
        second.method = None;
        resolve_ready(&mut second, &hostile);
        assert_eq!(bought, "market:buy", "friendly trait buys at these prices");
        assert_ne!(second.method.as_deref(), Some("market:buy"), "hostile trait farms at these prices");
        // Broke bots cannot cover a market leg: same numbers, empty
        // wallet farms instead of placing doomed orders.
        let mut broke = ctx.clone();
        broke.coins = 0;
        let mut skint = nodes[0].clone();
        skint.method = None;
        resolve_ready(&mut skint, &broke);
        assert_ne!(skint.method.as_deref(), Some("market:buy"), "broke bots farm");
        // A tool resolves to shop or market, never to the field.
        let mut net = expand_goal(&GoalKind::Obtain { item: "net".to_string(), count: 1 }, &profile());
        resolve_ready(&mut net[0], &ctx);
        assert!(matches!(net[0].method.as_deref(), Some("shop:buy") | Some("market:buy")), "nets never farm, got {:?}", net[0].method);
    }
}
