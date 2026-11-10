//! Long-term planner (lot cerveau): HTN goal expansion, tick costs,
//! engagement and replanning. Pure: caller-supplied numbers in, node
//! decisions out. No network, no files, no cross-bot state (D22): market
//! prices, trends and published yields arrive as plain arguments from the
//! brain, which reads only public market data.

#![forbid(unsafe_code)]

pub mod cost;
pub mod expand;
pub mod mapping;
pub mod model;
pub mod replan;

pub use cost::{choose_market_or_farm, cost_craft, cost_farm, cost_market_buy, rank_money_method};
pub use expand::{expand_goal, resolve_ready};
pub use mapping::map_method;
pub use model::{
    Alternative, Archetype, Engagement, GoalKind, HistoryEntry, Life, Node, NodeKind, NodeStatus,
    Profile, life_from_value, life_to_value,
};
pub use replan::{ReplanCause, draw_engagement_ticks, needs_replan, replan_node};
