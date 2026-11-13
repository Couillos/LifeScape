//! Combat prerequisites for the brain (cerveau combat): how strong a
//! bot must be before a task sends it somewhere, and what to do when it
//! is not. Pure: caller-supplied levels in, decisions out. No network,
//! no files, no JSON (the gate rides in planner types, never in a file).
//!
//! Server truth: `Player.ts:1371` (combat level from base levels) and
//! `Npc.ts:1014` (outside the Wilderness a hunter skips players above
//! twice its level, so a spot is safe at `2 * vislevel + 1`).
//!
//! Spot matrix (dump-verified, see tests): Draynor willows 15 (young
//! dark wizard 7), Varrock-East men 13 (mugger 6), coal 2701,3331 43
//! (brown bear 21), gold 2733,3225 41 (poison scorpion 20), volcano
//! entrance 29 (scorpion 14, transit to the lesser demons), anvil
//! 2991,3689 5 (giant spider 2). Anything else is safe at 3.
//! shortcut: the matrix pins each method to its script's primary spot;
//! if a script reroutes elsewhere, the pin goes stale (bans + hurt-flee
//! stay the live safety net).

/// Base combat levels (server `baseLevels`: trained levels, never the
/// boosted current HP the client shows in a fight).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BaseLevels {
    pub attack: u8,
    pub strength: u8,
    pub defence: u8,
    pub hitpoints: u8,
    pub ranged: u8,
    pub prayer: u8,
    pub magic: u8,
}

impl BaseLevels {
    /// A fresh account: everything 1, hitpoints 10 (server start).
    pub fn fresh() -> Self {
        BaseLevels {
            attack: 1,
            strength: 1,
            defence: 1,
            hitpoints: 10,
            ranged: 1,
            prayer: 1,
            magic: 1,
        }
    }
}

/// Combat level from base levels (`Player.ts:1371`): quarter of
/// defence + hitpoints + floored half prayer, plus the best of the
/// melee / ranged / magic parts, floored. Fresh accounts read 3.
pub fn combat_level(levels: &BaseLevels) -> u8 {
    let base = 0.25 * f64::from(levels.defence + levels.hitpoints + levels.prayer / 2);
    let melee = 0.325 * f64::from(levels.attack + levels.strength);
    let ranged = 0.325 * f64::from(levels.ranged / 2 + levels.ranged);
    let magic = 0.325 * f64::from(levels.magic / 2 + levels.magic);
    (base + melee.max(ranged).max(magic)) as u8
}

/// Site family and resource a method works (`None`: no site in the data,
/// e.g. smithing at any anvil).
fn method_sites(method: &str) -> Option<(&'static str, String)> {
    let (kind, resource) = if let Some(fish) = method.strip_prefix("gather:fishing/") {
        ("fishing", fish)
    } else if let Some(ore) = method.strip_prefix("gather:mining/") {
        ("mining", ore)
    } else if let Some(logs) = method.strip_prefix("gather:woodcut/") {
        ("woodcut", logs)
    } else if let Some(rest) = method.strip_prefix("fight:") {
        ("fight", rest.rsplit('/').next().unwrap_or(rest))
    } else if let Some(item) = method.strip_prefix("farm:") {
        if crate::mapping::is_fish(item) {
            ("fishing", item)
        } else if item.ends_with("_ore") || item == "coal" || item == "clay" {
            ("mining", item)
        } else if item.ends_with("logs") {
            ("woodcut", item)
        } else if let Some(spot) = crate::mapping::spot_for_drop(item) {
            // A drop farm hunts its spot: the spot's spawns are its sites.
            return Some(("fight", spot));
        } else {
            return None;
        }
    } else {
        return None;
    };
    Some((kind, resource.to_string()))
}

/// Minimum combat level to work one planner method without being hunted
/// on the way in, computed from the game data (no zone written by hand):
/// the least demanding site of the method, a site counting its bank
/// (`gamedata::danger::method_requirement`). `NEVER` when no site is
/// workable at any level; 3 for methods without sites.
pub fn min_combat_for_method(method: &str) -> u8 {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, u8>>> = std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    if let Some(known) = cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(method) {
        return *known;
    }
    let computed = match method_site_table(method) {
        // The least demanding site no gate makes impassable (Karamja water
        // without a boat edge, dungeons, the Wilderness: never).
        Some(table) => table.iter().filter(|row| row.open_ever).map(|row| row.required).min().unwrap_or(lifescape_gamedata::danger::NEVER),
        None => 3,
    };
    cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(method.to_string(), computed);
    computed
}

/// Typed failure of the combat brain: only unknown training styles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CombatError {
    /// No training method for this goal style.
    UnknownStyle(String),
}

impl std::fmt::Display for CombatError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CombatError::UnknownStyle(style) => write!(formatter, "no combat training for style {style}"),
        }
    }
}

/// Priority training method for `reach_combat`: the goal's own style
/// when it fights (ranged/magic), else melee — always on chickens,
/// the weakest CLEAN camp (combat spot `safe`, minimum 1).
pub fn training_method(style: &str) -> Result<&'static str, CombatError> {
    match style {
        "ranged" => Ok("fight:ranged/chickens"),
        "magic" => Ok("fight:magic/chickens"),
        "melee" | "attack" | "strength" | "defence" | "hitpoints" | "prayer" => Ok("fight:melee/chickens"),
        _ => Err(CombatError::UnknownStyle(style.to_string())),
    }
}

/// Combat style the goal trains under: its own style when it fights,
/// else melee (prayer buries where melee fights, producers fight
/// melee). Feeds `training_method` for the priority `reach_combat`.
pub fn goal_combat_style(goal: &crate::model::GoalKind) -> &str {
    match goal {
        crate::model::GoalKind::EquipBest { style } if style == "ranged" || style == "magic" => style,
        crate::model::GoalKind::MaxSkills { skills, .. } => {
            if skills.iter().any(|skill| skill == "ranged") {
                "ranged"
            } else if skills.iter().any(|skill| skill == "magic") {
                "magic"
            } else {
                "melee"
            }
        }
        _ => "melee",
    }
}

/// Where the brain sends one method for a bot: a workable site, the
/// combat level the least demanding site needs (a sub-task to train),
/// or nothing to choose (the method works anywhere).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SitePick {
    /// No site family for this method (smithing, quests...).
    Anywhere,
    /// The nearest site this bot works safely (x, z, level).
    At((i64, i64, i64)),
    /// Only across the Al Kharid toll fence: the fare is budgeted (or
    /// free), the launch carries `toll`.
    Tolled((i64, i64, i64)),
    /// Only behind the Karamja boat (and, `toll`, across the Al Kharid
    /// fence on the way): the round trip is budgeted, the launch carries
    /// `boat` (and `toll`).
    Boat { site: (i64, i64, i64), toll: bool },
    /// Only across the toll fence or behind the boat and the wallet lacks
    /// the fares: these coins are the need (`missing:coins:N`).
    Fare(i64),
    /// No site is safe at this combat level: train to this level first
    /// (`NEVER`: no level ever makes one safe, the method is unworkable).
    Needs(u8),
    /// The only safe sites sit behind a gate the bot does not pass yet:
    /// the dependency that opens it (`access:crafting_guild` — skill and
    /// uniform —, or the key object to obtain).
    Access(&'static str, Option<&'static str>),
}

/// Station families whose server way places them (furnace, anvil, range,
/// wheel, altar...): the brain picks the site, the script works there.
const STATION_FAMILIES: [&str; 14] =
    ["smelt:", "smith:", "cook/", "cook:", "bake:", "spin:", "pottery:", "jewelry:", "runecraft:", "ground:", "telegrab:", "dye:", "tan:", "mill:"];

/// One site of a method: where, the combat level its site and bank need,
/// its gates, and whether some bot can ever pass them.
#[derive(Debug, Clone)]
pub struct SiteRow {
    pub site: (i64, i64, i64),
    pub required: u8,
    pub gates: Vec<lifescape_gamedata::access::Gate>,
    pub open_ever: bool,
}

/// What gated sites the bot passes (user rule 2026-11-13, guilds and
/// keys): its live levels and the objects it holds (pack, bank, worn).
/// `Pass::default()` passes no gate (offline checks, side goals).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pass {
    pub levels: Vec<(String, u8)>,
    pub held: Vec<String>,
}

impl Pass {
    /// The ready context's levels and held objects.
    pub fn of(ctx: &crate::expand::ReadyCtx) -> Pass {
        Pass { levels: ctx.levels.clone(), held: ctx.tools.clone() }
    }

    /// True when the bot passes `gate` now (guild level and uniform held,
    /// key held); impassable gates never open.
    pub fn opens(&self, gate: &lifescape_gamedata::access::Gate) -> bool {
        use lifescape_gamedata::access::Gate;
        match gate {
            Gate::Guild { skill, level, uniform, .. } => {
                self.levels.iter().any(|(name, have)| name == skill && i64::from(*have) >= *level)
                    && uniform.is_none_or(|worn| self.held.iter().any(|item| item == worn))
            }
            Gate::Key { item, .. } => self.held.iter().any(|held| held == item),
            _ => false,
        }
    }
}

/// Sites of a method: its gather, fight or station sites (`None`: the
/// method works anywhere — quests, purchases, item-on-item recipes).
fn method_site_list(method: &str) -> Option<Vec<(i64, i64, i64)>> {
    if let Some((kind, resource)) = method_sites(method) {
        return Some(lifescape_gamedata::danger::sites(kind, &resource));
    }
    if STATION_FAMILIES.iter().any(|family| method.starts_with(family)) {
        let (_, way) = crate::method_way::method_way(method)?;
        return (!way.sites.is_empty()).then_some(way.sites);
    }
    None
}

/// Rows of one method, computed once from the data: each site's combat
/// requirement (read at its surface tile for an underground guild) and
/// its gates.
pub fn method_site_table(method: &str) -> Option<std::sync::Arc<Vec<SiteRow>>> {
    type Table = std::collections::HashMap<String, Option<std::sync::Arc<Vec<SiteRow>>>>;
    static CACHE: std::sync::OnceLock<std::sync::Mutex<Table>> = std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    if let Some(known) = cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(method) {
        return known.clone();
    }
    let table = method_site_list(method).map(|sites| {
        std::sync::Arc::new(
            sites
                .into_iter()
                .map(|(x, z, level)| {
                    let (surface_x, surface_z, surface_level) = lifescape_gamedata::access::surface_of(x, z, level);
                    let gates = lifescape_gamedata::access::gates_at(x, z, level);
                    // A Karamja site only for the scripts that sail.
                    let sails = boat_capable(method) || !lifescape_gamedata::access::behind_boat(x, z, level);
                    let open_ever = sails && !gates.iter().any(lifescape_gamedata::access::impassable);
                    SiteRow { site: (x, z, level), required: lifescape_gamedata::danger::site_requirement(surface_x, surface_z, surface_level), gates, open_ever }
                })
                .collect::<Vec<_>>(),
        )
    });
    let table = table.filter(|rows| !rows.is_empty());
    cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(method.to_string(), table.clone());
    table
}

/// Methods whose scripts arm the boat fare (`kit::toll::arm_fares`):
/// the gatherers and the fights. Any other method never takes a Karamja
/// site (its script would not sail).
pub fn boat_capable(method: &str) -> bool {
    ["gather:fishing/", "gather:mining/", "gather:woodcut/", "fight:", "farm:"].iter().any(|family| method.starts_with(family))
}

/// Coins a trip to `site` takes from the pack: the Al Kharid toll when
/// it crosses the fence (free after Prince Ali Rescue), the boat's round
/// trip when the site is on Karamja.
fn fare_need(site: (i64, i64, i64), position: Option<(i64, i64, i64)>, fare: Fare) -> i64 {
    let across = position.is_some_and(|from| lifescape_gamedata::toll::crossing(from, site));
    let toll = if across && !fare.free_pass { lifescape_gamedata::toll::FARE } else { 0 };
    let boat = if lifescape_gamedata::access::behind_boat(site.0, site.1, site.2) { 2 * lifescape_gamedata::services::BOAT_FARE_GP } else { 0 };
    toll + boat
}

/// What the bot can spend on the Al Kharid toll: its wallet (pack and
/// bank) and whether Prince Ali Rescue made the crossing free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fare {
    pub wallet: i64,
    pub free_pass: bool,
}

impl Fare {
    /// The fare a ready node can spend: its wallet, and the free pass
    /// when the bot finished Prince Ali Rescue.
    pub fn of(ctx: &crate::expand::ReadyCtx) -> Fare {
        Fare { wallet: ctx.coins, free_pass: ctx.quests_done.iter().any(|quest| quest == lifescape_gamedata::toll::FREE_PASS_QUEST) }
    }
}

/// `pick_site_paid` for a broke bot without the free pass: across the
/// toll fence only as a fare need (side goals, offline checks).
pub fn pick_site(method: &str, combat: u8, position: Option<(i64, i64, i64)>) -> SitePick {
    pick_site_paid(method, combat, position, Fare { wallet: 0, free_pass: false })
}

/// The brain's site choice for one method: among the sites this combat
/// level works safely (site and bank), the nearest on the bot's side of
/// the toll fence; else the nearest across it when the fare is covered
/// (`Tolled`), else the fare as the need (`Fare`); with no safe site at
/// all, the level the least demanding site needs.
pub fn pick_site_paid(method: &str, combat: u8, position: Option<(i64, i64, i64)>, fare: Fare) -> SitePick {
    pick_site_gated(method, combat, position, fare, &Pass::default())
}

/// `pick_site_paid` with the site gates (`gamedata::access`): a gated
/// site counts only when the bot passes its gate (`Pass::opens`); an
/// impassable one never (Karamja without a boat edge, dungeons, the
/// Wilderness). When only closed gates stand between the bot and every
/// safe site, the gate's dependency is the need (`Access`).
pub fn pick_site_gated(method: &str, combat: u8, position: Option<(i64, i64, i64)>, fare: Fare, pass: &Pass) -> SitePick {
    let Some(table) = method_site_table(method) else {
        return SitePick::Anywhere;
    };
    let rows: Vec<&SiteRow> = table.iter().filter(|row| row.open_ever).collect();
    let distance = |site: &(i64, i64, i64)| match position {
        Some((x, z, level)) => (site.0 - x).abs() + (site.1 - z).abs() + if site.2 == level { 0 } else { 1000 },
        None => 0,
    };
    let across = |site: &(i64, i64, i64)| position.is_some_and(|from| lifescape_gamedata::toll::crossing(from, *site));
    let safe: Vec<&SiteRow> = rows.iter().copied().filter(|row| row.required <= combat).collect();
    // The Karamja boat opens with its fare (budgeted below, like the toll).
    let passes = |gate: &lifescape_gamedata::access::Gate| pass.opens(gate) || matches!(gate, lifescape_gamedata::access::Gate::Boat { area: "karamja", .. });
    let open: Vec<(i64, i64, i64)> = safe.iter().filter(|row| row.gates.iter().all(passes)).map(|row| row.site).collect();
    let boat = |site: &(i64, i64, i64)| lifescape_gamedata::access::behind_boat(site.0, site.1, site.2);
    if let Some(site) = open.iter().filter(|site| !across(site) && !boat(site)).min_by_key(|site| distance(site)) {
        return SitePick::At(*site);
    }
    // Paid sites: the cheapest fares first, then the nearest.
    if let Some(site) = open.iter().min_by_key(|site| (fare_need(**site, position, fare), distance(site))) {
        let need = fare_need(*site, position, fare);
        if fare.wallet < need {
            return SitePick::Fare(need);
        }
        return if boat(site) { SitePick::Boat { site: *site, toll: across(site) } } else { SitePick::Tolled(*site) };
    }
    // Only gated safe sites left: the nearest one's closed gate.
    let gated = safe.iter().filter(|row| !row.gates.is_empty()).min_by_key(|row| distance(&row.site));
    if let Some(gate) = gated.and_then(|row| row.gates.iter().find(|gate| !pass.opens(gate))) {
        use lifescape_gamedata::access::Gate;
        return match gate {
            Gate::Key { area, item } => SitePick::Access(area, Some(*item)),
            other => SitePick::Access(other.area(), None),
        };
    }
    SitePick::Needs(rows.iter().map(|row| row.required).min().unwrap_or(lifescape_gamedata::danger::NEVER))
}

/// True when the bot's live melee stats survive the melee ladder rung
/// `spot` (`gamedata::melee_ladder::rate`, naked: the planner does not
/// read the worn bonuses, so the gate errs on the safe side; food and
/// gates assumed obtained — they are dependencies, not dangers). No live
/// levels (offline replays, tests): no stat gate. A spot the ladder does
/// not rate (dark wizards cast: no melee exchange) never fits.
pub fn rung_fit(spot: &str, ctx: &crate::expand::ReadyCtx) -> bool {
    use lifescape_gamedata::melee_ladder::{rate, rung_by_key, Holdings, DEFAULT_HEAL, MAX_FOOD};
    if rung_by_key(spot).is_none() {
        return false;
    }
    let level = |skill: &str| ctx.levels.iter().find(|(name, _)| name == skill).map(|(_, level)| i64::from(*level));
    let (Some(attack), Some(strength), Some(defence)) = (level("attack"), level("strength"), level("defence")) else { return true };
    let fighter = lifescape_gamedata::melee_math::Fighter::naked(attack, strength, defence, level("hitpoints").unwrap_or(10).max(10), i64::from(ctx.combat));
    let holdings = Holdings { food_count: MAX_FOOD, best_heal: DEFAULT_HEAL, coins: u32::MAX, brass_key: true, east_of_toll: true };
    rate(&fighter, &holdings).into_iter().find(|rated| rated.rung.key == spot).is_some_and(|rated| rated.refused.is_none())
}

/// A `farm:<drop>` leg hunts a spot the bot survives (`rung_fit` of
/// `mapping::spot_for_drop`); every other method passes. Live 2026-11-13
/// (bd23dqpq, `melee dark_wizards --want cosmicrune 1`): a drop farm the
/// bot cannot survive dies on the first trip, the market serves instead.
pub fn drop_farm_fit(method: &str, ctx: &crate::expand::ReadyCtx) -> bool {
    let Some(item) = method.strip_prefix("farm:") else { return true };
    match method_sites(method) {
        Some(("fight", spot)) => rung_fit(&spot, ctx),
        Some(_) => true,
        // Dropped only where the melee executor never hunts (dark wizards).
        None => lifescape_gamedata::combat_spots::drop_spots(item).is_empty(),
    }
}

#[cfg(test)]
#[path = "combat_toll_tests.rs"]
mod toll_tests;

#[cfg(test)]
#[path = "combat_access_tests.rs"]
mod access_tests;

#[cfg(test)]
mod tests {
    use super::{combat_level, goal_combat_style, min_combat_for_method, pick_site, training_method, BaseLevels, CombatError, SitePick};
    use crate::model::GoalKind;

    fn levels(attack: u8, strength: u8, defence: u8, hitpoints: u8, ranged: u8, prayer: u8, magic: u8) -> BaseLevels {
        BaseLevels { attack, strength, defence, hitpoints, ranged, prayer, magic }
    }

    #[test]
    fn fresh_accounts_read_three() {
        assert_eq!(combat_level(&BaseLevels::fresh()), 3, "neuf = 3");
    }

    #[test]
    fn formula_matches_known_builds() {
        // Melee 40s: base 20 + melee 26 = 46.
        assert_eq!(combat_level(&levels(40, 40, 40, 40, 1, 1, 1)), 46);
        // Ranger 40: base 2.75 + range 19.5 = 22.25 -> 22.
        assert_eq!(combat_level(&levels(1, 1, 1, 10, 40, 1, 1)), 22);
        // 60/60/60 + 43 prayer (classic 74): base 35.25 + melee 39.
        assert_eq!(combat_level(&levels(60, 60, 60, 60, 1, 43, 1)), 74);
        // Maxed: base 61.75 + melee 64.35 -> 126.
        assert_eq!(combat_level(&levels(99, 99, 99, 99, 99, 99, 99)), 126);
    }

    /// Requirements are computed from the data (no zone in the code): the
    /// least demanding site of each method, its bank included.
    #[test]
    fn requirements_come_from_the_sites() {
        assert_eq!(min_combat_for_method("gather:fishing/raw_shrimp"), 15, "only Draynor water, under the level-7 wizards");
        assert_eq!(min_combat_for_method("farm:raw_shrimp"), 15, "the obtain leg fishes the same water");
        assert_eq!(min_combat_for_method("gather:woodcut/willow_logs"), 3, "some willows sit outside every hunter");
        assert_eq!(min_combat_for_method("fight:melee/chickens"), 3);
        // Karamja water banks from the Port Sarim dock its boat lands on:
        // Draynor's bank, under the same wizards as its shrimp.
        assert_eq!(min_combat_for_method("gather:fishing/raw_lobster"), 15, "Karamja water banks at Draynor via the boat");
        // Anvils are station sites now (args contract 2026-11-13): the
        // least demanding anvil with its bank, no longer a pinned 5.
        assert_eq!(min_combat_for_method("smith:steel_platebody"), 3);
        assert_eq!(min_combat_for_method("quest:cook"), 3);
    }

    /// The brain sends a fresh bot to a site it works safely, and asks for
    /// training when no site is safe yet.
    #[test]
    fn site_pick_orients_or_requires_training() {
        let lumbridge = Some((3222, 3218, 0));
        match pick_site("gather:woodcut/willow_logs", 3, lumbridge) {
            SitePick::At((x, z, level)) => {
                assert!(lifescape_gamedata::danger::site_requirement(x, z, level) <= 3, "picked a site a CB3 cannot work: ({x},{z})");
            }
            other => panic!("willows are open to CB3, got {other:?}"),
        }
        assert_eq!(pick_site("gather:fishing/raw_shrimp", 3, lumbridge), SitePick::Needs(15));
        assert!(matches!(pick_site("gather:fishing/raw_shrimp", 15, lumbridge), SitePick::At(_)));
        assert_eq!(pick_site("smith:bronze_bar", 3, lumbridge), SitePick::Anywhere);
    }

    #[test]
    fn priority_training_follows_the_goal_style_on_clean_camps() {        assert_eq!(training_method("ranged"), Ok("fight:ranged/chickens"));
        assert_eq!(training_method("magic"), Ok("fight:magic/chickens"));
        for style in ["melee", "attack", "strength", "prayer"] {
            assert_eq!(training_method(style), Ok("fight:melee/chickens"), "{style} trains melee");
        }
        assert_eq!(
            training_method("sailing"),
            Err(CombatError::UnknownStyle("sailing".to_string())),
            "unknown styles fail typed"
        );
        // Every training method executes (mapping knows it) on the
        // weakest CLEAN camp (chickens: safe, minimum 1).
        let chickens = lifescape_gamedata::combat_spots::by_key("chickens").expect("chickens");
        assert_eq!(chickens.danger, "safe");
        assert_eq!(chickens.min_level, 1);
        let probe = crate::model::Node::new(
            "n9",
            crate::model::NodeKind::ObtainItems { item: "feather".to_string(), count: 1 },
            Vec::new(),
            1,
        );
        for style in ["melee", "ranged", "magic"] {
            let method = training_method(style).expect("style trains");
            assert!(crate::mapping::map_method(method, &probe, None).is_some(), "{method} executes");
        }
    }

    #[test]
    fn goal_style_feeds_the_priority_training() {
        assert_eq!(goal_combat_style(&GoalKind::EquipBest { style: "ranged".to_string() }), "ranged");
        assert_eq!(goal_combat_style(&GoalKind::EquipBest { style: "magic".to_string() }), "magic");
        assert_eq!(goal_combat_style(&GoalKind::EquipBest { style: "melee".to_string() }), "melee");
        assert_eq!(
            goal_combat_style(&GoalKind::MaxSkills { skills: vec!["attack".to_string(), "ranged".to_string()], level: 40 }),
            "ranged",
            "ranged wins the mix"
        );
        assert_eq!(
            goal_combat_style(&GoalKind::MaxSkills { skills: vec!["magic".to_string()], level: 40 }),
            "magic"
        );
        assert_eq!(goal_combat_style(&GoalKind::MaxSkills { skills: vec!["fishing".to_string()], level: 40 }), "melee");
        assert_eq!(goal_combat_style(&GoalKind::MakeMoney), "melee", "producers fight melee");
        // Every goal style trains (jamais d'envoi vers la mort).
        for goal in [
            GoalKind::MakeMoney,
            GoalKind::MaxSkills { skills: vec!["ranged".to_string()], level: 40 },
            GoalKind::EquipBest { style: "magic".to_string() },
        ] {
            assert!(training_method(goal_combat_style(&goal)).is_ok(), "{goal:?} trains");
        }
    }

}
