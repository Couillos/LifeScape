//! Long-term planner (lot cerveau): HTN goal expansion, tick costs,
//! engagement and replanning. Pure: caller-supplied numbers in, node
//! decisions out. No network, no files, no cross-bot state (D22): market
//! prices, trends and published yields arrive as plain arguments from the
//! brain, which reads only public market data.

#![forbid(unsafe_code)]

pub mod catalog;
pub mod chains;
pub mod combat;
pub mod cost;
pub mod expand;
pub mod goals;
pub mod lifecycle;
pub mod mapping;
pub mod model;
pub mod obtain_legs;
pub mod replan;
pub mod resolve_obtain;
pub mod space;
pub mod tools;
pub mod tranches;

pub use catalog::{
    activity_from_value, activity_to_value, catalog_from_value, catalog_to_value, validate, Activity, Arch,
    Epoch, Prereqs, Stack,
};
pub use chains::{
    best_for_money, best_for_xp, boot_covers, boot_plan, chain_by_id, cheapest_source, eval_chain, quest_shortcut,
    quests_for_goal, synergies, usable_chains, xp_per_tick, BootStep, Chain, ChainEval, ChainPrices, ChainStep,
    QuestShortcut, Source, SourceCtx, CHAINS, WILDERNESS_TAX_TICKS,
};
pub use combat::{combat_level, goal_combat_style, min_combat_for_method, pick_site, training_method, BaseLevels, CombatError, SitePick};
pub use cost::{choose_market_or_farm, cost_craft, cost_farm, cost_market_buy, rank_money_method};
pub use expand::{expand_goal, method_feasible, required_tool, resolve_ready};
pub use lifecycle::{graph_state, mark_launched, next_id, normalize, record_block, record_contract_error, record_failure, record_progress, FailureFold, GraphState, MAX_BLOCKS, MAX_FAILURES};
pub use goals::{append_goal, goal_skills, next_goal, stuck_skills, DONE_TAG, STUCK_TAG, TRAINABLE};
pub use mapping::map_method;
pub use model::{
    Alternative, Archetype, Engagement, GoalKind, HistoryEntry, Life, Node, NodeKind, NodeStatus,
    Profile, life_from_value, life_to_value,
};
pub use replan::{need_count, ReplanCause, draw_engagement_ticks, needs_replan, replan_node, INPUT_BATCH};
pub use space::{
    Fallback, ItemEcon, cleanup_needed, cleanup_stalled, d17_order, fallback_for, iron_may_use, launch_allowed,
    market_leg, needs_cleanup, reserve_floor, triage_bank, BANK_TYPE_LIMIT, FOOD_KEEP_MIN,
};
