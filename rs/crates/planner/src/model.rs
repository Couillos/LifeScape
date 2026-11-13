//! Planner data model: goals, profiles, nodes, life file values.
//!
//! JSON conversion uses `lifescape-json` values only (no serde): the
//! brain persists `Life` through the atomic writer, tests round-trip it
//! without touching disk.

use lifescape_json::{Value, object, str_value};

/// Long-term goal of one bot (file `goal` section).
#[derive(Debug, Clone, PartialEq)]
pub enum GoalKind {
    MaxSkills { skills: Vec<String>, level: u8 },
    Obtain { item: String, count: i64 },
    MakeMoney,
    CompleteQuest { quest: String },
    EquipBest { style: String },
    AccessArea { area: String },
    GetRich { merchant: Option<bool> },
    /// Make `count` of `item` by production only (an ambition milestone:
    /// the product is never bought; inputs may be). Once the obtain node
    /// is done, the brain's default selling lists the surplus.
    Produce { item: String, count: i64 },
}

/// Producer or merchant for the `get_rich` goal (D18). Drawn once at
/// creation (`merchant_share` 10%), never Iron.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Archetype {
    Producer,
    Merchant,
}

/// Stable per-bot profile, drawn once from the seed (D15, D8 aside).
#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    pub ironman: bool,
    pub market_affinity: f64,
    pub price_patience: f64,
    pub margin: f64,
    pub seed: u64,
    pub archetype: Archetype,
}

impl Profile {
    pub fn new(ironman: bool, market_affinity: f64, price_patience: f64, seed: u64, archetype: Archetype) -> Self {
        Profile {
            ironman,
            market_affinity: market_affinity.clamp(0.0, 1.0),
            price_patience: price_patience.clamp(0.0, 1.0),
            margin: 0.05 + price_patience * 0.15,
            seed,
            archetype,
        }
    }
}

/// One task node kind (PRD §5.2).
#[derive(Debug, Clone, PartialEq)]
pub enum NodeKind {
    ReachLevel { skill: String, level: u8 },
    /// Priority combat training (cerveau gate): train to this combat
    /// level on the goal's style before any trip past it.
    ReachCombat { level: u8 },
    ObtainItems { item: String, count: i64 },
    HaveCoins { amount: i64 },
    MakeMoney,
    CompleteQuest { quest: String },
    EquipBest { style: String },
    AccessArea { area: String },
    SpaceCleanup,
}

/// Node lifecycle: pending (deps unmet) → ready → active → done, or
/// blocked / failed (replanned) / abandoned (method replaced).
#[derive(Debug, Clone, PartialEq)]
pub enum NodeStatus {
    Pending,
    Ready,
    Active,
    Done,
    Blocked { reason: String },
    Failed { error: String },
    Abandoned,
}

impl NodeStatus {
    pub fn as_str(&self) -> String {
        match self {
            NodeStatus::Pending => "pending".to_string(),
            NodeStatus::Ready => "ready".to_string(),
            NodeStatus::Active => "active".to_string(),
            NodeStatus::Done => "done".to_string(),
            NodeStatus::Blocked { .. } => "blocked".to_string(),
            NodeStatus::Failed { .. } => "failed".to_string(),
            NodeStatus::Abandoned => "abandoned".to_string(),
        }
    }

    pub fn detail(&self) -> String {
        match self {
            NodeStatus::Blocked { reason } => reason.clone(),
            NodeStatus::Failed { error } => error.clone(),
            _ => String::new(),
        }
    }

    pub fn from_str(status: &str, detail: &str) -> Self {
        match status {
            "ready" => NodeStatus::Ready,
            "active" => NodeStatus::Active,
            "done" => NodeStatus::Done,
            "blocked" => NodeStatus::Blocked { reason: detail.to_string() },
            "failed" => NodeStatus::Failed { error: detail.to_string() },
            "abandoned" => NodeStatus::Abandoned,
            _ => NodeStatus::Pending,
        }
    }
}

/// One costed way to satisfy a node.
#[derive(Debug, Clone, PartialEq)]
pub struct Alternative {
    pub method: String,
    pub est_ticks: u64,
    pub est_coins: i64,
}

/// One graph node: condition plus deps, status, chosen method and its
/// costed alternatives (kept for the dashboard, §5.2 explicability).
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub id: String,
    pub kind: NodeKind,
    pub deps: Vec<String>,
    pub status: NodeStatus,
    pub method: Option<String>,
    pub alternatives: Vec<Alternative>,
    pub progress_current: i64,
    pub progress_target: i64,
    pub spent_ticks: u64,
    /// Runtime failures counted against this node (`lifecycle`): bounded
    /// by `MAX_FAILURES`, never reset by a replan (no ghost retries).
    pub failures: u32,
    /// Blocked stops (missing tool/input) counted the same way.
    pub blocks: u32,
    /// Cooldown revivals of this node once abandoned (`revive_stuck`):
    /// bounded by `MAX_REVIVES`, past it the abandonment is final.
    pub revives: u32,
    /// Last typed error seen on this node (`Type: message`), shown on the
    /// dashboard and in the dossier when the node is abandoned.
    pub last_error: String,
    /// Site the brain sends the bot to (x, z, level), chosen at resolve
    /// from the game data for the bot's combat level (`pick_site`).
    pub site: Option<(i64, i64, i64)>,
}

impl Node {
    pub fn new(id: &str, kind: NodeKind, deps: Vec<String>, target: i64) -> Self {
        Node {
            id: id.to_string(),
            kind,
            deps,
            status: NodeStatus::Pending,
            method: None,
            alternatives: Vec::new(),
            progress_current: 0,
            progress_target: target,
            spent_ticks: 0,
            failures: 0,
            blocks: 0,
            revives: 0,
            last_error: String::new(),
            site: None,
        }
    }

    pub fn ready(&self, done: &[String]) -> bool {
        self.deps.iter().all(|dep| done.contains(dep))
    }
}

/// Engagement bounds of a `make_money` node (D1 ticks, D5/D8 rules live
/// in the brain: suspension and no cap are execution, not planning).
#[derive(Debug, Clone, PartialEq)]
pub struct Engagement {
    pub node: String,
    pub min_ticks: u64,
    pub max_ticks: u64,
    pub chosen: u64,
}

/// One history line of the life file.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryEntry {
    pub node: String,
    pub started: i64,
    pub ended: i64,
    pub outcome: String,
}

/// Whole long-term state of one account (file `run/lives/<compte>.json`,
/// schema v1, atomic write daemon-side).
#[derive(Debug, Clone, PartialEq)]
pub struct Life {
    pub schema: i64,
    pub account: String,
    pub created_at: i64,
    pub seed: u64,
    pub profile: Profile,
    pub goal: GoalKind,
    pub nodes: Vec<Node>,
    pub engagement: Option<Engagement>,
    pub active: Option<String>,
    pub history: Vec<HistoryEntry>,
    /// Long-term ambition driving the goal draws (`ambition.rs`); `None`
    /// only in files written before ambitions (the brain assigns one).
    pub ambition: Option<crate::ambition::Ambition>,
    /// Milestone id the current goal was drawn from (`m3`), if any.
    pub goal_origin: Option<String>,
}

pub const LIFE_SCHEMA: i64 = 1;

fn goal_to_value(goal: &GoalKind) -> Value {
    match goal {
        GoalKind::MaxSkills { skills, level } => object(vec![
            ("kind", str_value("max_skills")),
            ("skills", Value::Arr(skills.iter().map(|skill| str_value(skill)).collect())),
            ("level", Value::Int(i64::from(*level))),
        ]),
        GoalKind::Obtain { item, count } => object(vec![
            ("kind", str_value("obtain")),
            ("item", str_value(item)),
            ("count", Value::Int(*count)),
        ]),
        GoalKind::MakeMoney => object(vec![("kind", str_value("make_money"))]),
        GoalKind::CompleteQuest { quest } => object(vec![
            ("kind", str_value("complete_quest")),
            ("quest", str_value(quest)),
        ]),
        GoalKind::EquipBest { style } => object(vec![
            ("kind", str_value("equip_best")),
            ("style", str_value(style)),
        ]),
        GoalKind::AccessArea { area } => object(vec![
            ("kind", str_value("access_area")),
            ("area", str_value(area)),
        ]),
        GoalKind::GetRich { merchant } => object(vec![
            ("kind", str_value("get_rich")),
            (
                "merchant",
                match merchant {
                    Some(true) => Value::Bool(true),
                    Some(false) => Value::Bool(false),
                    None => Value::Null,
                },
            ),
        ]),
        GoalKind::Produce { item, count } => object(vec![
            ("kind", str_value("produce")),
            ("item", str_value(item)),
            ("count", Value::Int(*count)),
        ]),
    }
}

fn goal_from_value(value: &Value) -> Option<GoalKind> {
    let strings = |key: &str| {
        value.get(key).as_arr().map(|rows| {
            rows.iter().filter_map(|row| row.as_str().map(str::to_string)).collect::<Vec<_>>()
        })
    };
    match value.get("kind").as_str().unwrap_or("") {
        "max_skills" => Some(GoalKind::MaxSkills {
            skills: strings("skills").unwrap_or_default(),
            level: value.get("level").as_i64().unwrap_or(1).clamp(2, 99) as u8,
        }),
        "obtain" => Some(GoalKind::Obtain {
            item: value.get("item").as_str().unwrap_or("").to_string(),
            count: value.get("count").as_i64().unwrap_or(1).max(1),
        }),
        "make_money" => Some(GoalKind::MakeMoney),
        "complete_quest" => Some(GoalKind::CompleteQuest {
            quest: value.get("quest").as_str().unwrap_or("").to_string(),
        }),
        "equip_best" => Some(GoalKind::EquipBest {
            style: value.get("style").as_str().unwrap_or("melee").to_string(),
        }),
        "access_area" => Some(GoalKind::AccessArea {
            area: value.get("area").as_str().unwrap_or("").to_string(),
        }),
        "produce" => Some(GoalKind::Produce {
            item: value.get("item").as_str().unwrap_or("").to_string(),
            count: value.get("count").as_i64().unwrap_or(1).max(1),
        }),
        "get_rich" => Some(GoalKind::GetRich {
            merchant: match &value.get("merchant") {
                Value::Bool(flag) => Some(*flag),
                _ => None,
            },
        }),
        _ => None,
    }
}

fn kind_to_parts(kind: &NodeKind) -> (String, Value) {
    match kind {
        NodeKind::ReachLevel { skill, level } => (
            "reach_level".to_string(),
            object(vec![("skill", str_value(skill)), ("level", Value::Int(i64::from(*level)))]),
        ),
        NodeKind::ReachCombat { level } => (
            "reach_combat".to_string(),
            object(vec![("level", Value::Int(i64::from(*level)))]),
        ),
        NodeKind::ObtainItems { item, count } => (
            "obtain_items".to_string(),
            object(vec![("item", str_value(item)), ("count", Value::Int(*count))]),
        ),
        NodeKind::HaveCoins { amount } => (
            "have_coins".to_string(),
            object(vec![("amount", Value::Int(*amount))]),
        ),
        NodeKind::MakeMoney => ("make_money".to_string(), object(vec![])),
        NodeKind::CompleteQuest { quest } => (
            "complete_quest".to_string(),
            object(vec![("quest", str_value(quest))]),
        ),
        NodeKind::EquipBest { style } => (
            "equip_best".to_string(),
            object(vec![("style", str_value(style))]),
        ),
        NodeKind::AccessArea { area } => (
            "access_area".to_string(),
            object(vec![("area", str_value(area))]),
        ),
        NodeKind::SpaceCleanup => ("space_cleanup".to_string(), object(vec![])),
    }
}

fn kind_from_parts(kind: &str, params: &Value) -> Option<NodeKind> {
    match kind {
        "reach_level" => Some(NodeKind::ReachLevel {
            skill: params.get("skill").as_str().unwrap_or("").to_string(),
            level: params.get("level").as_i64().unwrap_or(2).clamp(2, 99) as u8,
        }),
        "reach_combat" => Some(NodeKind::ReachCombat {
            level: params.get("level").as_i64().unwrap_or(3).clamp(3, 126) as u8,
        }),
        "obtain_items" => Some(NodeKind::ObtainItems {
            item: params.get("item").as_str().unwrap_or("").to_string(),
            count: params.get("count").as_i64().unwrap_or(1),
        }),
        "have_coins" => Some(NodeKind::HaveCoins { amount: params.get("amount").as_i64().unwrap_or(0) }),
        "make_money" => Some(NodeKind::MakeMoney),
        "complete_quest" => Some(NodeKind::CompleteQuest {
            quest: params.get("quest").as_str().unwrap_or("").to_string(),
        }),
        "equip_best" => Some(NodeKind::EquipBest {
            style: params.get("style").as_str().unwrap_or("melee").to_string(),
        }),
        "access_area" => Some(NodeKind::AccessArea {
            area: params.get("area").as_str().unwrap_or("").to_string(),
        }),
        "space_cleanup" => Some(NodeKind::SpaceCleanup),
        _ => None,
    }
}

fn node_to_value(node: &Node) -> Value {
    let (kind, params) = kind_to_parts(&node.kind);
    let status = node.status.as_str();
    let detail = node.status.detail();
    object(vec![
        ("id", str_value(&node.id)),
        ("kind", str_value(&kind)),
        ("params", params),
        ("deps", Value::Arr(node.deps.iter().map(|dep| str_value(dep)).collect())),
        ("status", str_value(&status)),
        ("detail", str_value(&detail)),
        ("method", node.method.as_deref().map(str_value).unwrap_or(Value::Null)),
        (
            "alternatives",
            Value::Arr(
                node.alternatives
                    .iter()
                    .map(|alt| {
                        object(vec![
                            ("method", str_value(&alt.method)),
                            ("est_ticks", Value::Int(alt.est_ticks as i64)),
                            ("est_coins", Value::Int(alt.est_coins)),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("progress", object(vec![
            ("current", Value::Int(node.progress_current)),
            ("target", Value::Int(node.progress_target)),
        ])),
        ("spent_ticks", Value::Int(node.spent_ticks as i64)),
        ("failures", Value::Int(i64::from(node.failures))),
        ("blocks", Value::Int(i64::from(node.blocks))),
        ("revives", Value::Int(i64::from(node.revives))),
        ("last_error", str_value(&node.last_error)),
        (
            "site",
            match node.site {
                Some((x, z, level)) => Value::Arr(vec![Value::Int(x), Value::Int(z), Value::Int(level)]),
                None => Value::Null,
            },
        ),
    ])
}

fn node_from_value(value: &Value) -> Option<Node> {
    let kind = kind_from_parts(value.get("kind").as_str().unwrap_or(""), &value.get("params"))?;
    let alternatives = value
        .get("alternatives")
        .as_arr()
        .map(|rows| {
            rows.iter()
                .map(|row| Alternative {
                    method: row.get("method").as_str().unwrap_or("").to_string(),
                    est_ticks: row.get("est_ticks").as_i64().unwrap_or(0).max(0) as u64,
                    est_coins: row.get("est_coins").as_i64().unwrap_or(0),
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Some(Node {
        id: value.get("id").as_str().unwrap_or("").to_string(),
        kind,
        deps: value
            .get("deps")
            .as_arr()
            .map(|rows| rows.iter().filter_map(|row| row.as_str().map(str::to_string)).collect())
            .unwrap_or_default(),
        status: NodeStatus::from_str(
            value.get("status").as_str().unwrap_or("pending"),
            value.get("detail").as_str().unwrap_or(""),
        ),
        method: value.get("method").as_str().map(str::to_string),
        alternatives,
        progress_current: value.get("progress").get("current").as_i64().unwrap_or(0),
        progress_target: value.get("progress").get("target").as_i64().unwrap_or(0),
        spent_ticks: value.get("spent_ticks").as_i64().unwrap_or(0).max(0) as u64,
        // Absent in files written before the lifecycle counters: zero.
        failures: value.get("failures").as_i64().unwrap_or(0).clamp(0, i64::from(u32::MAX)) as u32,
        blocks: value.get("blocks").as_i64().unwrap_or(0).clamp(0, i64::from(u32::MAX)) as u32,
        revives: value.get("revives").as_i64().unwrap_or(0).clamp(0, i64::from(u32::MAX)) as u32,
        last_error: value.get("last_error").as_str().unwrap_or("").to_string(),
        site: value.get("site").as_arr().and_then(|cells| {
            let part = |slot: usize| cells.get(slot).and_then(|cell| cell.as_i64());
            Some((part(0)?, part(1)?, part(2)?))
        }),
    })
}

/// Serialize a life file (schema v1).
pub fn life_to_value(life: &Life) -> Value {
    let engagement = match &life.engagement {
        Some(engaged) => object(vec![
            ("node", str_value(&engaged.node)),
            ("min_ticks", Value::Int(engaged.min_ticks as i64)),
            ("max_ticks", Value::Int(engaged.max_ticks as i64)),
            ("chosen", Value::Int(engaged.chosen as i64)),
        ]),
        None => Value::Null,
    };
    object(vec![
        ("schema", Value::Int(LIFE_SCHEMA)),
        ("account", str_value(&life.account)),
        ("created_at", Value::Int(life.created_at)),
        ("seed", Value::Int(life.seed as i64)),
        (
            "profile",
            object(vec![
                ("ironman", Value::Bool(life.profile.ironman)),
                ("market_affinity", Value::Float(life.profile.market_affinity)),
                ("price_patience", Value::Float(life.profile.price_patience)),
                (
                    "archetype",
                    str_value(match life.profile.archetype {
                        Archetype::Merchant => "merchant",
                        Archetype::Producer => "producer",
                    }),
                ),
            ]),
        ),
        ("goal", goal_to_value(&life.goal)),
        ("nodes", Value::Arr(life.nodes.iter().map(node_to_value).collect())),
        ("engagement", engagement),
        (
            "active",
            life.active.as_deref().map(str_value).unwrap_or(Value::Null),
        ),
        (
            "history",
            Value::Arr(
                life.history
                    .iter()
                    .map(|entry| {
                        object(vec![
                            ("node", str_value(&entry.node)),
                            ("started", Value::Int(entry.started)),
                            ("ended", Value::Int(entry.ended)),
                            ("outcome", str_value(&entry.outcome)),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("ambition", life.ambition.as_ref().map(crate::ambition::ambition_to_value).unwrap_or(Value::Null)),
        ("goal_origin", life.goal_origin.as_deref().map(str_value).unwrap_or(Value::Null)),
    ])
}

/// Parse a life file back; `None` on schema mismatch or missing goal.
pub fn life_from_value(value: &Value) -> Option<Life> {
    if value.get("schema").as_i64() != Some(LIFE_SCHEMA) {
        return None;
    }
    let profile = value.get("profile");
    let mut nodes = Vec::new();
    for row in value.get("nodes").as_arr().cloned().unwrap_or_default() {
        nodes.push(node_from_value(&row)?);
    }
    Some(Life {
        schema: LIFE_SCHEMA,
        account: value.get("account").as_str().unwrap_or("").to_string(),
        created_at: value.get("created_at").as_i64().unwrap_or(0),
        seed: value.get("seed").as_i64().unwrap_or(0) as u64,
        profile: Profile::new(
            profile.get("ironman").as_bool().unwrap_or(false),
            profile.get("market_affinity").as_f64().unwrap_or(0.5),
            profile.get("price_patience").as_f64().unwrap_or(0.5),
            value.get("seed").as_i64().unwrap_or(0) as u64,
            match profile.get("archetype").as_str().unwrap_or("producer") {
                "merchant" => Archetype::Merchant,
                _ => Archetype::Producer,
            },
        ),
        goal: goal_from_value(&value.get("goal"))?,
        nodes,
        engagement: match &value.get("engagement") {
            Value::Null => None,
            engaged => Some(Engagement {
                node: engaged.get("node").as_str().unwrap_or("").to_string(),
                min_ticks: engaged.get("min_ticks").as_i64().unwrap_or(6000).max(0) as u64,
                max_ticks: engaged.get("max_ticks").as_i64().unwrap_or(18000).max(0) as u64,
                chosen: engaged.get("chosen").as_i64().unwrap_or(6000).max(0) as u64,
            }),
        },
        active: value.get("active").as_str().map(str::to_string),
        history: value
            .get("history")
            .as_arr()
            .map(|rows| {
                rows.iter()
                    .map(|row| HistoryEntry {
                        node: row.get("node").as_str().unwrap_or("").to_string(),
                        started: row.get("started").as_i64().unwrap_or(0),
                        ended: row.get("ended").as_i64().unwrap_or(0),
                        outcome: row.get("outcome").as_str().unwrap_or("").to_string(),
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
        // Absent before ambitions: loads fine, the brain assigns one.
        ambition: crate::ambition::ambition_from_value(&value.get("ambition")),
        goal_origin: value.get("goal_origin").as_str().map(str::to_string),
    })
}

#[cfg(test)]
mod tests {
    use super::{life_from_value, life_to_value, Archetype, Engagement, GoalKind, HistoryEntry, Life, Profile};

    fn life() -> Life {
        Life {
            schema: super::LIFE_SCHEMA,
            account: "t42".to_string(),
            created_at: 1762675200,
            seed: 918273645,
            profile: Profile::new(false, 0.72, 0.4, 918273645, Archetype::Producer),
            goal: GoalKind::MaxSkills { skills: vec!["fishing".to_string()], level: 20 },
            nodes: crate::expand_goal(
                &GoalKind::MaxSkills { skills: vec!["fishing".to_string()], level: 20 },
                &Profile::new(false, 0.72, 0.4, 918273645, Archetype::Producer),
            ),
            engagement: Some(Engagement { node: "n2".to_string(), min_ticks: 6000, max_ticks: 18000, chosen: 9000 }),
            active: Some("n2".to_string()),
            history: vec![HistoryEntry {
                node: "n1".to_string(),
                started: 1762680000,
                ended: 1762690000,
                outcome: "goal_reached".to_string(),
            }],
            ambition: None,
            goal_origin: None,
        }
    }

    #[test]
    fn life_file_roundtrips() {
        let value = life_to_value(&life());
        let back = life_from_value(&value).expect("life parses back");
        assert_eq!(back, life(), "aller-retour exact");
        assert_eq!(value.get("schema").as_i64(), Some(1), "schema v1");
    }

    #[test]
    fn bad_schema_refuses() {
        let mut value = life_to_value(&life());
        if let lifescape_json::Value::Obj(map) = &mut value {
            map.insert("schema".to_string(), lifescape_json::Value::Int(99));
        }
        assert!(life_from_value(&value).is_none(), "schema mismatch fails loud");
        let mut no_goal = life_to_value(&life());
        if let lifescape_json::Value::Obj(map) = &mut no_goal {
            map.insert("goal".to_string(), lifescape_json::Value::Null);
        }
        assert!(life_from_value(&no_goal).is_none(), "missing goal fails loud");
    }

    #[test]
    fn reach_combat_roundtrips() {
        let mut trained = life();
        trained.nodes.push(super::Node::new(
            "n9",
            super::NodeKind::ReachCombat { level: 43 },
            Vec::new(),
            43,
        ));
        let back = life_from_value(&life_to_value(&trained)).expect("reach_combat parses back");
        assert_eq!(back.nodes.last().map(|node| &node.kind), Some(&super::NodeKind::ReachCombat { level: 43 }));
    }
}
