//! Cold-start yield of a planner method computed from the game (plan
//! rendements R2): what one action takes in server ticks, its odds at a
//! level, what it yields and burns, and the bank walks a load costs.
//!
//! `actions / 6000 = 6000 / (ticks per action + bank walk per action)`
//! where an action is one attempt (a roll, a cast, a press); a success
//! yields the product, the inputs burn per attempt or per success as the
//! server does. Every timing reads the server scripts (rev 274):
//! - p_op loops re-arm `%action_delay = clock + N` one tick after a
//!   roll: a roll every N + 1 ticks (mining `mining_rate` + 1, woodcut
//!   3 + 1, fishing 4 + 1 or 5 + 1 per spot option);
//! - one-shot actions ending in `p_delay(n)` repeat every n + 2 ticks
//!   (cooking 3, bury 2, low alchemy 3, high alchemy 5, superheat and
//!   enchant 3, bones to bananas 4, teleports 3, filling 2);
//! - make loops queued with `weakqueue`: smelting and anvil 5, gold and
//!   silver jewellery 3, spinning 3, pottery wheel 3, oven 7 (crack odds
//!   `stat_random(crafting, 180, 800)`), leather 3;
//! - instant uses (gem cutting, stringing, milking, dyes, tanning) take
//!   the one client op a tick allows;
//! - firemaking rolls every 4 ticks at `stat_random(firemaking, 64, 512)`;
//! - runecraft crafts the whole pack in one action then `p_delay(3)`;
//! - combat spells and curses 5 ticks; melee from `melee_math`.
//!
//! A method without such a timing (the mill, a prep combine) is a typed
//! `PriorError`, never a default figure. Bank walks: the round trip from
//! the site to the nearest F2P bank at one tile a tick, shared by the
//! actions one pack holds, plus the visit's client ops (open, deposit,
//! one withdrawal per input kind); a way doable anywhere works at the
//! bank.

use std::fmt;

use lifescape_gamedata::action_odds::{cooking, cooking_of, fish_chance, pickaxe_rate, rock_chance, runes_per_essence, stat_odds, tree_chance};
use lifescape_gamedata::obtain::{ways, Way, WayKind};

/// Ticks of the published window.
pub const RATE_WINDOW_TICKS: f64 = 6000.0;
/// Pack slots of a player.
pub const PACK_SLOTS: f64 = 28.0;
/// Tiles a walking player covers per tick (server walk step).
pub const WALK_TILES_PER_TICK: f64 = 1.0;
/// Ticks of one instant use: one client op per tick.
pub const INSTANT_USE_TICKS: f64 = 1.0;
/// Client ops of a bank visit before the withdrawals: open the booth,
/// deposit the pack (one op a tick, one withdrawal per input kind).
pub const BANK_SESSION_OPS: f64 = 2.0;
/// Odds a sheep stays still for the shears (`shear_sheep.rs2`:
/// `random(4) = 0` escapes).
pub const SHEAR_ODDS: f64 = 0.75;
/// Pottery oven crack roll (`pottery.rs2`).
pub const POTTERY_FIRE_ODDS: (i64, i64) = (180, 800);
/// Firemaking light roll, every log (`firemaking.rs2`).
pub const FIREMAKING_ODDS: (i64, i64) = (64, 512);
/// Thread one leather item wears (`leather.rs2`: one reel per five).
pub const THREAD_PER_ITEM: f64 = 0.2;

/// Why no prior can be computed (never a default figure instead).
#[derive(Debug, Clone, PartialEq)]
pub enum PriorError {
    /// The method names no game way (unknown or non-production method).
    NoWay(String),
    /// The server timing of this family is not modelled.
    NoTiming(String),
    /// The success odds of the target are missing from the dump.
    NoOdds(String),
    /// The tool the method needs is not one the server rates.
    NoTool(String),
    /// The fight's NPC has no combat block.
    NoTarget(String),
}

impl fmt::Display for PriorError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PriorError::NoWay(method) => write!(out, "no game way for {method}"),
            PriorError::NoTiming(method) => write!(out, "no server action timing for {method}"),
            PriorError::NoOdds(what) => write!(out, "no success odds for {what}"),
            PriorError::NoTool(tool) => write!(out, "tool {tool} has no server rate"),
            PriorError::NoTarget(npc) => write!(out, "no combat block for {npc}"),
        }
    }
}

impl std::error::Error for PriorError {}

/// What the prior is asked for (the D2 key of a yield bucket).
#[derive(Debug, Clone, Default)]
pub struct PriorQuery {
    pub method: String,
    /// Level in the method's skill (fights: every combat stat).
    pub level: u8,
    /// Tool tier of the key (`rune_pickaxe`), empty: the base tool.
    pub tool: String,
    /// Site of the key, `None`: the way's site nearest a bank.
    pub site: Option<(i64, i64, i64)>,
}

/// One action of a method: ticks, odds and per-attempt flows.
#[derive(Debug, Clone, PartialEq)]
pub struct Action {
    pub ticks: f64,
    pub success: f64,
    /// (debug item, cause, signed quantity per ATTEMPT).
    pub flows: Vec<(String, &'static str, f64)>,
    /// (unit kind, subject, quantity per attempt).
    pub units: Vec<(&'static str, String, f64)>,
    /// Pack slots one attempt fills (inputs carried or products made).
    pub slots: f64,
    /// Sites the work happens at (empty: anywhere, at the bank).
    pub sites: Vec<(i64, i64, i64)>,
    /// False for work that never banks (fights, casting on the spot).
    pub banks: bool,
}

/// The computed prior of one key, per 6000 ticks of work.
#[derive(Debug, Clone, PartialEq)]
pub struct RatePrior {
    pub actions_per_6000: f64,
    pub ticks_per_action: f64,
    pub bank_ticks_per_action: f64,
    pub success: f64,
    /// (debug item, cause, signed per 6000).
    pub flows: Vec<(String, &'static str, f64)>,
    /// (unit kind, subject, per 6000).
    pub units: Vec<(&'static str, String, f64)>,
}

fn stackable(item: &str) -> bool {
    lifescape_gamedata::objs_by_name(item).first().is_some_and(|obj| obj.stackable)
}

fn method_way_of(method: &str) -> Result<(String, Way), PriorError> {
    crate::method_way::method_way(method).ok_or_else(|| PriorError::NoWay(method.to_string()))
}

/// Pack slots of `items` (stackables fill none per action).
fn slots_of(items: &[(String, f64)]) -> f64 {
    items.iter().filter(|(item, _)| !stackable(item)).map(|(_, qty)| qty.max(0.0)).sum()
}

fn recipe_action(way: &Way, product: &str, ticks: f64, success: f64) -> Action {
    let inputs: Vec<(String, f64)> = way.inputs.iter().map(|(item, qty)| (item.clone(), (*qty).max(1) as f64)).collect();
    let mut flows: Vec<(String, &'static str, f64)> = inputs.iter().map(|(item, qty)| (item.clone(), crate::method_rate_units::input_cause(item), -qty)).collect();
    flows.push((product.to_string(), "product", success));
    let made = [(product.to_string(), 1.0)];
    Action { ticks, success, flows, units: vec![("product", product.to_string(), success)], slots: slots_of(&inputs).max(slots_of(&made)), sites: way.sites.clone(), banks: true }
}

/// The action one attempt of `query.method` is, from the game.
pub fn action_of(query: &PriorQuery) -> Result<Action, PriorError> {
    let method = query.method.as_str();
    let level = i64::from(query.level.max(1));
    if method.starts_with("fight:") {
        return crate::method_rate_fight::fight_action(query);
    }
    if let Some(item) = method.strip_prefix("alch:") {
        return crate::method_rate_units::alch_action(item, query.level);
    }
    if let Some(found) = crate::method_rate_units::spell_action(method)? {
        return Ok(found);
    }
    if method.starts_with("essence:") {
        let pickaxe = tool_or_base("essence", &query.tool, "bronze_pickaxe");
        let rate = pickaxe_rate(&pickaxe).ok_or_else(|| PriorError::NoTool(pickaxe.clone()))?;
        let sites = ways("blankrune").into_iter().flat_map(|way| way.sites).collect();
        return Ok(Action { ticks: (rate + 1) as f64, success: 1.0, flows: vec![("blankrune".to_string(), "product", 1.0)], units: vec![("product", "blankrune".to_string(), 1.0)], slots: 1.0, sites, banks: true });
    }
    if let Some(field) = method.strip_prefix("pick:") {
        // A field leg (`pick:wheat` gives grain): one pick, `p_delay(0)`.
        let product = if field == "wheat" { "grain" } else { field };
        let way = ways(product).into_iter().find(|way| way.kind == WayKind::Harvest && way.detail.starts_with("pick ")).ok_or_else(|| PriorError::NoWay(method.to_string()))?;
        return Ok(harvest(product, &way, 2.0));
    }
    let (product, way) = match method_way_of(method) {
        Ok(found) => found,
        Err(error) if method.starts_with("farm:") => return crate::method_rate_fight::drop_action(method, query).map_err(|_| error),
        Err(error) => return Err(error),
    };
    let family = method.split([':', '/']).next().unwrap_or("");
    let detail_kind = way.detail.split_whitespace().next().unwrap_or("");
    let skill = way.skill.as_ref().map(|(name, _)| name.as_str()).unwrap_or("");
    if way.kind == WayKind::Gather {
        return gather_action(&product, &way, skill, level, &query.tool);
    }
    let action = match (family, detail_kind) {
        ("cook", _) | ("bake", _) | ("craft", "cook") => {
            let (raw, odds) = cooking_of(&product).or_else(|| way.inputs.first().and_then(|(raw, _)| cooking(raw).map(|odds| (raw.clone(), odds)))).ok_or_else(|| PriorError::NoOdds(product.clone()))?;
            let (low, high) = odds.range.filter(|_| !way.sites.is_empty()).unwrap_or(odds.fire);
            let success = stat_odds(level, low, high);
            let mut action = recipe_action(&way, &product, 3.0, success);
            if let Some(burnt) = odds.burnt {
                action.flows.push((burnt, "byproduct", 1.0 - success));
            }
            let _ = raw;
            action
        }
        ("smelt", _) => recipe_action(&way, &product, 5.0, if way.success > 0.0 { way.success } else { 1.0 }),
        ("smith", _) => recipe_action(&way, &product, 5.0, 1.0),
        ("jewelry", _) | ("spin", _) => recipe_action(&way, &product, 3.0, 1.0),
        ("pottery", "pottery_shape") => recipe_action(&way, &product, 3.0, 1.0),
        ("pottery", "pottery_fire") => recipe_action(&way, &product, 7.0, stat_odds(level, POTTERY_FIRE_ODDS.0, POTTERY_FIRE_ODDS.1)),
        ("craft", "leather") => {
            let mut action = recipe_action(&way, &product, 3.0, 1.0);
            action.flows.push(("thread".to_string(), "supply", -THREAD_PER_ITEM));
            action
        }
        ("cut", _) | ("string", _) => recipe_action(&way, &product, INSTANT_USE_TICKS, 1.0),
        ("fire", _) => {
            // The log burns on a lit roll only; the ashes stay on the
            // ground (`firemaking.rs2`), never a flow.
            let success = stat_odds(level, FIREMAKING_ODDS.0, FIREMAKING_ODDS.1);
            let log = way.inputs.first().map(|(log, _)| log.clone()).ok_or_else(|| PriorError::NoWay(method.to_string()))?;
            Action { ticks: 4.0, success, flows: vec![(log.clone(), "input", -success)], units: vec![("input", log, success)], slots: 1.0, sites: Vec::new(), banks: true }
        }
        ("bury", _) => {
            let bones = way.inputs.first().map(|(bones, _)| bones.clone()).ok_or_else(|| PriorError::NoWay(method.to_string()))?;
            Action { ticks: 2.0, success: 1.0, flows: vec![(bones.clone(), "input", -1.0)], units: vec![("input", bones, 1.0)], slots: 1.0, sites: Vec::new(), banks: true }
        }
        ("runecraft", _) => runecraft_action(&product, &way, level)?,
        ("superheat", _) => recipe_action(&way, &product, 3.0, 1.0),
        ("enchant", _) => recipe_action(&way, &product, 3.0, 1.0),
        ("telegrab", _) => {
            let spawns = way.sites.len().max(1) as f64;
            let respawn = lifescape_gamedata::objs_by_name(&product).first().map(|obj| obj.respawn_ticks as f64).ok_or_else(|| PriorError::NoOdds(product.clone()))?;
            let mut action = recipe_action(&way, &product, (respawn / spawns).max(6.0), 1.0);
            action.slots = 1.0;
            action
        }
        ("ground", _) => {
            let spawns = way.sites.len().max(1) as f64;
            let respawn = lifescape_gamedata::objs_by_name(&product).first().map(|obj| obj.respawn_ticks as f64).ok_or_else(|| PriorError::NoOdds(product.clone()))?;
            Action { ticks: (respawn / spawns).max(INSTANT_USE_TICKS), success: 1.0, flows: vec![(product.clone(), "ground_spawn", 1.0)], units: Vec::new(), slots: 1.0, sites: way.sites.clone(), banks: true }
        }
        ("pick", _) | ("gather", _) if way.kind == WayKind::Harvest && way.detail.starts_with("shear") => {
            // A failed grab costs one tick, a success its two p_delay(0).
            let ticks = SHEAR_ODDS * 3.0 + (1.0 - SHEAR_ODDS) * INSTANT_USE_TICKS;
            Action { ticks, success: SHEAR_ODDS, flows: vec![(product.clone(), "product", SHEAR_ODDS)], units: Vec::new(), slots: 1.0, sites: way.sites.clone(), banks: true }
        }
        ("pick", _) | ("gather", _) => harvest(&product, &way, 2.0),
        ("fill", _) => harvest(&product, &way, 2.0),
        ("milk", _) | ("dye", _) | ("tan", _) => harvest(&product, &way, INSTANT_USE_TICKS),
        _ => return Err(PriorError::NoTiming(method.to_string())),
    };
    Ok(action)
}

/// A harvest: inputs (the empty vessel, dye ingredients, the hide) and
/// the way's fee in coins per action.
fn harvest(product: &str, way: &Way, ticks: f64) -> Action {
    let mut action = recipe_action(way, product, ticks, 1.0);
    action.units.clear();
    if way.coins > 0 {
        action.flows.push(("coins".to_string(), "fee", -(way.coins as f64)));
    }
    action
}

fn tool_or_base(_family: &str, tool: &str, base: &str) -> String {
    let named = tool.split('+').next().unwrap_or("").trim();
    if named.is_empty() { base.to_string() } else { named.to_string() }
}

fn base_tool(way: &Way) -> String {
    way.tools.first().cloned().unwrap_or_default()
}

fn gather_action(product: &str, way: &Way, skill: &str, level: i64, tool: &str) -> Result<Action, PriorError> {
    let base = base_tool(way);
    let (ticks, success, extra): (f64, f64, Vec<(String, &'static str, f64)>) = match skill {
        "mining" => {
            let pickaxe = tool_or_base("mining", tool, &base);
            let rate = pickaxe_rate(&pickaxe).ok_or_else(|| PriorError::NoTool(pickaxe.clone()))?;
            let (low, high) = rock_chance(product).ok_or_else(|| PriorError::NoOdds(product.to_string()))?;
            // One roll in 256 finds a gem instead (`get_ore_normal`).
            let odds = if (low, high) == (256, 256) { 1.0 } else { stat_odds(level, low, high) * 255.0 / 256.0 };
            ((rate + 1) as f64, odds, Vec::new())
        }
        "woodcutting" => {
            let axe = tool_or_base("woodcut", tool, &base);
            let (low, high) = tree_chance(product, &axe).ok_or_else(|| PriorError::NoOdds(format!("{product} with {axe}")))?;
            (4.0, stat_odds(level, low, high), Vec::new())
        }
        "fishing" => return fish_action(product, way, level),
        _ => return Err(PriorError::NoTiming(format!("gather {product}"))),
    };
    let mut flows = vec![(product.to_string(), "product", success)];
    flows.extend(extra);
    Ok(Action { ticks, success, flows, units: vec![("product", product.to_string(), success)], slots: success.max(1.0), sites: way.sites.clone(), banks: true })
}

/// Ticks between two rolls of a fishing spot option (`%action_delay =
/// clock + N`, a roll every N + 1): saltfish net 5, bait 4; freshfish
/// lure and bait 4; rarefish cage and harpoon 5 (`fishing_spots/*.rs2`).
fn fish_roll_ticks(action: &str) -> Option<f64> {
    match action {
        "Net" | "Cage" | "Harpoon" => Some(6.0),
        "Bait" | "Lure" => Some(5.0),
        _ => None,
    }
}

fn fish_action(product: &str, way: &Way, level: i64) -> Result<Action, PriorError> {
    let tier = lifescape_gamedata::fishing::all_fish_tiers().into_iter().find(|tier| tier.products.contains(&product)).ok_or_else(|| PriorError::NoOdds(product.to_string()))?;
    let ticks = fish_roll_ticks(tier.action).ok_or_else(|| PriorError::NoTiming(format!("fishing {}", tier.action)))?;
    // `fish_roll`: the second fish rolls first once its level is met.
    let first = tier.products[0];
    let second = tier.products.get(1).filter(|_| tier.second_level > 0 && level >= tier.second_level);
    let odds_of = |fish: &str| fish_chance(fish).map(|(low, high)| stat_odds(level, low, high)).ok_or_else(|| PriorError::NoOdds(fish.to_string()));
    let mut caught: Vec<(String, f64)> = Vec::new();
    let mut miss = 1.0;
    if let Some(second) = second {
        let odds = odds_of(second)?;
        caught.push((second.to_string(), odds));
        miss *= 1.0 - odds;
    }
    caught.push((first.to_string(), miss * odds_of(first)?));
    let success: f64 = caught.iter().map(|(_, odds)| odds).sum();
    let mut flows: Vec<(String, &'static str, f64)> = caught.iter().map(|(fish, odds)| (fish.clone(), "product", *odds)).collect();
    if let Some(bait) = tier.bait {
        flows.push((bait.to_string(), "bait", -success));
    }
    let units = caught.iter().map(|(fish, odds)| ("product", fish.clone(), *odds)).collect();
    Ok(Action { ticks, success, flows, units, slots: success, sites: way.sites.clone(), banks: true })
}

fn runecraft_action(rune: &str, way: &Way, level: i64) -> Result<Action, PriorError> {
    let per_essence = runes_per_essence(rune, level).ok_or_else(|| PriorError::NoOdds(rune.to_string()))? as f64;
    let essence = way.inputs.first().map(|(item, _)| item.clone()).unwrap_or_else(|| "blankrune".to_string());
    // The whole pack (27 essences beside the talisman) crafts in one
    // action, then `p_delay(3)`: four ticks shared by the load.
    let load = PACK_SLOTS - 1.0;
    Ok(Action {
        ticks: 4.0 / load,
        success: 1.0,
        flows: vec![(essence.clone(), "input", -1.0), (rune.to_string(), "product", per_essence)],
        units: vec![("input", essence, 1.0)],
        slots: 1.0,
        sites: way.sites.clone(),
        banks: true,
    })
}

/// Shortest bank round trip among a method's sites (0 without one),
/// memoised per method: the sites are static game data (a tree way
/// lists thousands of tiles).
fn nearest_trip(method: &str, sites: &[(i64, i64, i64)]) -> f64 {
    static MEMO: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, f64>>> = std::sync::OnceLock::new();
    let memo = MEMO.get_or_init(Default::default);
    if let Some(known) = memo.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(method) {
        return *known;
    }
    let trip = sites.iter().filter_map(|site| bank_round_trip(*site)).fold(None, |best: Option<f64>, trip| Some(best.map_or(trip, |known| known.min(trip)))).unwrap_or(0.0);
    memo.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(method.to_string(), trip);
    trip
}

/// Round-trip walk from `site` to the nearest F2P bank, in ticks.
pub fn bank_round_trip(site: (i64, i64, i64)) -> Option<f64> {
    let bank = lifescape_gamedata::banks::nearest_free_bank(site.0, site.1, site.2)?;
    let tiles = (bank.x - site.0).abs().max((bank.z - site.1).abs()) as f64;
    Some(2.0 * tiles / WALK_TILES_PER_TICK)
}

/// The prior of one key: the action, its bank walks, per 6000 ticks.
pub fn method_prior(query: &PriorQuery) -> Result<RatePrior, PriorError> {
    let action = action_of(query)?;
    if action.ticks <= 0.0 {
        return Err(PriorError::NoTiming(query.method.clone()));
    }
    let bank_ticks_per_action = if action.banks {
        let tools = crate::tools::required_tool(&query.method).map(|need| need.tools.iter().filter(|tool| !stackable(tool)).count()).unwrap_or(0) as f64;
        let per_load = ((PACK_SLOTS - tools) / action.slots.max(1e-9)).max(1.0);
        let trip = match query.site {
            Some(site) => bank_round_trip(site).unwrap_or(0.0),
            None => nearest_trip(&query.method, &action.sites),
        };
        let kinds = action.flows.iter().filter(|(item, cause, qty)| *qty < 0.0 && *cause != "fee" && !stackable(item)).count() as f64;
        (trip + (BANK_SESSION_OPS + kinds) * INSTANT_USE_TICKS) / per_load
    } else {
        0.0
    };
    let actions_per_6000 = RATE_WINDOW_TICKS / (action.ticks + bank_ticks_per_action);
    Ok(RatePrior {
        actions_per_6000,
        ticks_per_action: action.ticks,
        bank_ticks_per_action,
        success: action.success,
        flows: action.flows.iter().map(|(item, cause, qty)| (item.clone(), *cause, qty * actions_per_6000)).collect(),
        units: action.units.iter().map(|(kind, subject, qty)| (*kind, subject.clone(), qty * actions_per_6000)).collect(),
    })
}

/// The guides' prior (R4): `method_prior` plus, for a fight, the
/// expected loot of its kills from the NPC's drop table (`loot` per
/// 6000). The brain keeps `method_prior` (no decision change before
/// R7); the guides price what a fight brings home.
pub fn guide_prior(query: &PriorQuery) -> Result<RatePrior, PriorError> {
    let mut prior = method_prior(query)?;
    if query.method.starts_with("fight:") {
        for (item, per_kill) in crate::method_rate_fight::fight_drops_per_kill(&query.method) {
            prior.flows.push((item, "loot", per_kill * prior.actions_per_6000));
        }
    }
    Ok(prior)
}

/// Site of a way nearest a bank (the site a prior without one walks
/// from); `None` for work done anywhere or on the spot (fights).
pub fn best_site(query: &PriorQuery) -> Option<(i64, i64, i64)> {
    static MEMO: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, Option<(i64, i64, i64)>>>> = std::sync::OnceLock::new();
    let memo = MEMO.get_or_init(Default::default);
    if let Some(known) = memo.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(&query.method) {
        return *known;
    }
    let found = compute_best_site(query);
    memo.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(query.method.clone(), found);
    found
}

fn compute_best_site(query: &PriorQuery) -> Option<(i64, i64, i64)> {
    let action = action_of(query).ok()?;
    if !action.banks {
        return None;
    }
    action.sites.iter().filter_map(|site| bank_round_trip(*site).map(|trip| (trip, *site))).min_by(|left, right| left.0.total_cmp(&right.0)).map(|(_, site)| site)
}

/// Physical ceiling of a method: the most attempts one work tick holds
/// (server action ticks, certain success, no bank), with the per-attempt
/// units it can carry. A run above it is impossible: rejected.
pub fn units_ceiling_per_tick(query: &PriorQuery) -> Result<Vec<(&'static str, String, f64)>, PriorError> {
    let action = action_of(query)?;
    let ticks = action.ticks.max(1e-9);
    // Certain success: each unit at its per-success quantity.
    let scale = if action.success > 0.0 { 1.0 / action.success } else { 1.0 };
    Ok(action.units.iter().map(|(kind, subject, qty)| (*kind, subject.clone(), qty * scale / ticks)).collect())
}

#[cfg(test)]
#[path = "method_rate_tests.rs"]
mod tests;
