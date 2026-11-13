//! Long-term planner (lot cerveau): HTN goal expansion, tick costs,
//! engagement and replanning. Pure: caller-supplied numbers in, node
//! decisions out. No network, no files, no cross-bot state (D22): market
//! prices, trends and published yields arrive as plain arguments from the
//! brain, which reads only public market data.

#![forbid(unsafe_code)]

pub mod ambition;
pub mod ambition_catalog;
pub mod ambition_fleet;
pub mod bans;
pub mod bans_scope;
pub mod catalog;
pub mod chains;
pub mod combat;
pub mod cost;
pub mod dialogue_site;
pub mod expand;
pub mod gear_ladder;
pub mod goals;
pub mod lifecycle;
pub mod mapping;
pub mod method_way;
pub mod model;
pub mod obtain_legs;
pub mod profit;
pub mod quests;
pub mod replan;
pub mod resolve_obtain;
pub mod shop_site;
pub mod side_goal;
pub mod skeleton_deps;
pub mod sizing;
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
pub use combat::{combat_level, goal_combat_style, min_combat_for_method, pick_site, pick_site_gated, pick_site_paid, training_method, BaseLevels, CombatError, Fare, Pass, SitePick};
pub use cost::{choose_market_or_farm, cost_craft, cost_farm, cost_market_buy, rank_money_method};
pub use expand::{expand_goal, expand_goal_from, method_feasible, required_tool, resolve_ready};
pub use lifecycle::{complete_node, end_engagement, graph_state, insert_gear_step, insert_ready, park_for_fill, withdraw_gear_step, mark_launched, reached_live, next_id, normalize, record_block, record_block_from, record_contract_error, retry_uncounted, record_failure, record_progress, retarget_market, revive_stuck, FailureFold, GraphState, MAX_BLOCKS, MAX_FAILURES, MAX_REVIVES, REVIVE_COOLDOWN_TICKS};
pub use bans_scope::{banned_scope, market_route, rescue_candidates};
pub use bans::{ban_entry, ban_states, banned_skills, stuck_cause, unban_entry, BanState, StuckCause, SKILL_BAN_TICKS, UNBAN_TAG};
pub use gear_ladder::{gear_move, next_gear_step, next_ready, next_ready_except, withdraw_dead_steps, withdraw_melee_set, GearMove, GearStep, GEAR_SHARE_PCT, TOOL_SHARE_PCT};
pub use profit::{chain_profit, ChainProfit, ProfitError};
pub use quests::{method_quest, missing_quest, quests_done_from, quests_done_in_graph};
pub use side_goal::{method_banned, side_goal, steer_off_bans, SIDE_PREFIX};
pub use goals::{append_goal, draw_goal, draw_goal_origin, goal_closed, goal_root_done, goal_skills, next_goal, NoGoal, DONE_TAG, STUCK_TAG, TRAINABLE};
pub use ambition::{
    achieved, ambition_from_value, describe as describe_ambition, ambition_to_value, extend as extend_ambition, goal_for_milestone, mark_drawn, may_change, new_ambition,
    new_ambition_extended, next_milestone, open_count, AmbitionError, BotNow,
    on_goal_closed, refresh, revise, Ambition, CloseOutcome, Milestone, MilestoneStatus, Revision, Stamp, Target,
    AMBITION_CHANGE_TICKS, GENERALIST, MILESTONE_MAX_BLOCKS,
};
pub use ambition_catalog::{kind_weights, DemandItem, KindDemand, GROUPS, MERCHANT};
pub use ambition_fleet::{assign_fleet, coverage_gaps, coverage_repair, draw_kind, optional_change, Assignment, FleetBot};
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
