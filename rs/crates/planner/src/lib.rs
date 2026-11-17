//! Long-term planner (lot cerveau): HTN goal expansion, tick costs,
//! engagement and replanning. Pure: caller-supplied numbers in, node
//! decisions out. No network, no files, no cross-bot state (D22): market
//! prices, trends and published yields arrive as plain arguments from the
//! brain, which reads only public market data.

#![forbid(unsafe_code)]

pub mod activity;
pub mod ambition;
pub mod ambition_catalog;
pub mod ambition_fleet;
pub mod ambition_ladder;
pub mod bans;
pub mod alchemy;
pub mod bid_response;
pub mod bans_scope;
pub mod catalog;
pub mod chains;
pub mod combat;
pub mod cooldown;
pub mod cost;
pub mod dialogue_site;
pub mod expand;
pub mod gear_ladder;
pub mod gear_plan;
pub mod guide_chain;
pub mod guide_engine;
pub mod guide_flows;
pub mod guide_methods;
pub mod guide_model;
pub mod guide_money;
pub mod guide_prereq;
pub mod guide_read;
pub mod guide_training;
pub mod goals;
pub mod lifecycle;
pub mod lifecycle_retire;
pub mod life_memory;
pub mod market_fit;
pub mod market_maker;
pub mod market_value;
pub mod derived_demand;
pub mod mapping;
pub mod method_way;
pub mod method_rate;
pub mod method_rate_fight;
pub mod method_rate_trip;
pub mod method_rate_units;
pub mod unit_xp;
pub mod spell_methods;
pub mod model;
pub mod obtain_legs;
pub mod profit;
pub mod quest_hold;
pub mod quests;
pub mod rare_items;
pub mod replan;
pub mod resolve_obtain;
pub mod shop_site;
pub mod side_goal;
pub mod skeleton_deps;
pub mod sizing;
pub mod space;
pub mod tool_kit;
pub mod tool_upgrade;
pub mod tools;
pub mod training;
pub mod training_production;
pub mod training_spells;
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
pub use lifecycle_retire::{retire_due, retire_finished, Retirement};
pub use life_memory::{LifeMemory, Retired, RECENT_CAP};
pub use lifecycle::{abandon_for_role, max_id_number, complete_node, end_engagement, graph_state, insert_gear_step, insert_ready, park_for_fill, withdraw_gear_step, mark_launched, reached_live, next_id, normalize, record_block, record_block_from, record_contract_error, retry_uncounted, record_failure, record_failure_timed, record_progress, record_xp, retarget_market, retarget_produce, revive_stuck, FailureFold, GraphState, MAX_BLOCKS, MAX_FAILURES, MAX_REVIVES, REVIVE_COOLDOWN_TICKS};
pub use bans_scope::{banned_scope, market_failed, market_route, market_unfilled_error, rescue_candidates, rescued_before, MARKET_RESCUE_TAG, PRODUCE_RESCUE_TAG};
pub use bans::{ban_entry, ban_states, banned_skills, quest_lifted_bans, stuck_cause, unban_entry, BanState, StuckCause, SKILL_BAN_TICKS, UNBAN_TAG};
pub use gear_ladder::{gear_move, next_gear_step, next_ready, next_ready_except, withdraw_dead_steps, withdraw_melee_set, GearMove, GearStep, GEAR_SHARE_PCT, TOOL_SHARE_PCT};
pub use profit::{chain_profit, ChainProfit, ProfitError};
pub use quests::{method_quest, missing_quest, quests_done_from, quests_done_in_graph};
pub use quest_hold::{cooled_quest_nodes, cooled_quests, node_quest, open_quests, quest_ban_key, quest_shortfalls, reserved_quest_objects, withhold_reserved, QuestShortfall, QUEST_BAN_PREFIX};
pub use side_goal::{goal_feasible, method_banned, side_goal, side_goal_with, steer_for_work, steer_off_bans, WorkSteer, SIDE_PREFIX};
pub use goals::{append_goal, draw_goal, draw_goal_origin, goal_closed, goal_root_done, goal_skills, lowest_skill_goal, serves_goal, next_goal, NoGoal, DONE_TAG, OVERALL_SKILLS, STUCK_TAG, TRAINABLE};
pub use ambition::{
    achieved, ambition_from_value, describe as describe_ambition, ambition_to_value, drop_stale_products, extend as extend_ambition, goal_for_milestone, mark_drawn, may_change, new_ambition,
    new_ambition_extended, next_milestone, open_count, pinned, pinned_ambition, release_pin, AmbitionError, BotNow, Pin,
    on_goal_closed, refresh, revise, Ambition, CloseOutcome, Milestone, MilestoneStatus, Revision, Stamp, Target,
    AMBITION_CHANGE_TICKS, GENERALIST, MILESTONE_MAX_BLOCKS, OVERALL,
};
pub use ambition_catalog::{kind_weights, DemandItem, KindDemand, GROUPS};
pub use market_maker::{become_quoting, maker_post, maker_role, quoting, switch_due, MakerPhase, MakerRole, MAKER_POSTS, MAKE_METHOD, MARKET_MAKER_KIND, MAKER_ROLE_CAUSE};
pub use ambition_fleet::{assign_fleet, coverage_gaps, coverage_repair, draw_kind, optional_change, Assignment, FleetBot};
pub use mapping::map_method;
pub use model::{
    Alternative, Archetype, Engagement, GoalKind, HistoryEntry, Life, Node, NodeKind, NodeStatus,
    Profile, life_from_value, life_to_value,
};
pub use replan::{need_count, ReplanCause, draw_engagement_ticks, needs_replan, replan_node, INPUT_BATCH};
pub use space::{
    Fallback, ItemEcon, cleanup_cooling, cleanup_needed, cleanup_stalled, janitor_stalled, d17_order, fallback_for, iron_may_use, launch_allowed,
    market_leg, needs_cleanup, reserve_floor, triage_bank, BANK_TYPE_LIMIT, CLEANUP_RETRY_SECS, CLEANUP_SHORT_TAG, FOOD_KEEP_MIN,
};
