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

/// True for a method that hunts a combat spot: a `fight:` or a drop
/// farm (`farm:naturerune` at the goblins).
pub fn is_fight_method(method: &str) -> bool {
    fight_spot(method).is_some()
}

/// Combat spot a fight hunts (`fight:melee/men` → `men`, a drop farm's
/// spot); `None` for anything else (curses cast, never fight back).
pub fn fight_spot(method: &str) -> Option<String> {
    match method_sites(method) {
        Some(("fight", spot)) if !method.starts_with("curse:") => Some(spot),
        _ => None,
    }
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
    } else if let Some(rest) = method.strip_prefix("fight:").or_else(|| method.strip_prefix("curse:")) {
        // A curse casts on a combat camp's NPCs (`curse:<spell>/<spot>`).
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
        // without a boat edge, dungeons: never; the Wilderness is open).
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

/// Combat style the bot fights NOW (user 2026-11-20: « pourquoi ils ne
/// cherchent pas à obtenir du matériel qui augmente les bonus de magie,
/// comme les staffs ? »; live: fiona_swift, a mage whose bid goal
/// (amulet of power) trains magic for its enchant, cast earth strike
/// with an adamant battleaxe: a production goal reads melee). The
/// current goal's open fight wins: the running node, else a ready one
/// (`fight:<style>/..` method, or a magic / ranged level to reach);
/// none, the goal's own style (`goal_combat_style`).
pub fn fight_style(life: &crate::model::Life) -> &str {
    use crate::model::{NodeKind, NodeStatus};
    let floor = life.memory.floor.unwrap_or(0);
    let style_of = |node: &crate::model::Node| -> Option<&'static str> {
        let method = node.method.as_deref().unwrap_or("");
        for style in ["magic", "ranged", "melee"] {
            if method.strip_prefix("fight:").is_some_and(|rest| rest.starts_with(style) && rest[style.len()..].starts_with('/')) {
                return Some(style);
            }
        }
        match &node.kind {
            NodeKind::ReachLevel { skill, .. } if method.is_empty() && skill == "magic" => Some("magic"),
            NodeKind::ReachLevel { skill, .. } if method.is_empty() && skill == "ranged" => Some("ranged"),
            _ => None,
        }
    };
    let current = |node: &&crate::model::Node| crate::lifecycle_retire::node_number(&node.id).is_some_and(|number| number >= floor);
    let open = |status: &NodeStatus| life.nodes.iter().filter(current).filter(|node| &node.status == status).find_map(style_of);
    open(&NodeStatus::Active).or_else(|| open(&NodeStatus::Ready)).unwrap_or_else(|| goal_combat_style(&life.goal))
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
    /// Every site this bot works safely is cooled for it (`cooldown`,
    /// P1/D4): the method waits out the cooldown, another one serves.
    Cooled,
    /// Every open site is full (the fleet works every node, tour 2 P2-3b,
    /// D6): the method steps aside like a cooled one, the node's next
    /// method serves; never a forced full site.
    Full,
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
/// keys): its live levels, the objects it holds (pack, bank, worn) and
/// its quest points (the Champions' Guild door, user 2026-11-16 Q7).
/// `Pass::default()` passes no gate (offline checks, side goals).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pass {
    pub levels: Vec<(String, u8)>,
    pub held: Vec<String>,
    /// Quest points of the finished quests (`quest_points_of`).
    pub quest_points: i64,
}

impl Pass {
    /// The ready context's levels, held objects and quest points.
    pub fn of(ctx: &crate::expand::ReadyCtx) -> Pass {
        Pass { levels: ctx.levels.clone(), held: ctx.tools.clone(), quest_points: lifescape_gamedata::quest_requirements::quest_points_of(&ctx.quests_done) }
    }

    /// True when the bot passes `gate` now (guild level and uniform held,
    /// key held, quest points held); impassable gates never open.
    pub fn opens(&self, gate: &lifescape_gamedata::access::Gate) -> bool {
        use lifescape_gamedata::access::Gate;
        match gate {
            Gate::Guild { skill, level, uniform, .. } => {
                self.levels.iter().any(|(name, have)| name == skill && i64::from(*have) >= *level)
                    && uniform.is_none_or(|worn| self.held.iter().any(|item| item == worn))
            }
            Gate::Key { item, .. } => self.held.iter().any(|held| held == item),
            // `champions_guild.rs2`: `%qp < 32` keeps the door shut.
            Gate::QuestPoints { points, .. } => self.quest_points >= *points,
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

/// Loads a banking site is weighed over (user 2026-11-20: « les bots qui
/// smelt utilisent la bank de Draynor et smelt à Lumbridge, ça fait
/// beaucoup de route ; ils doivent utiliser le spot le plus rapide »):
/// one run banks this many times, so the bank round trip counts this
/// many times against the one walk to the site.
pub const BANK_LOADS: i64 = 20;

/// Tiles the bank walks of `site` weigh for `method` over `loads` loads
/// (at most `BANK_LOADS`, the first one carried there): the stations (furnace, range,
/// anvil...) and the gathering that banks its load; a fight, or a site
/// with no bank found, weighs nothing.
pub fn bank_weight(method: &str, site: (i64, i64, i64), loads: i64) -> i64 {
    let inner = crate::trade_craft::inner(method);
    let station = STATION_FAMILIES.iter().any(|family| inner.starts_with(family));
    let gathers = method_sites(inner).is_some_and(|(kind, _)| kind != "fight");
    if !(station || gathers) {
        return 0;
    }
    // The first load rides in the pack: only the next ones walk back.
    crate::method_rate_trip::bank_walk_tiles(site).map_or(0, |tiles| (loads.min(BANK_LOADS) - 1).max(0) * 2 * tiles)
}

/// Rows of one method, computed once from the data: each site's combat
/// requirement (read at its surface tile for an underground guild) and
/// its gates.
pub fn method_site_table(method: &str) -> Option<std::sync::Arc<Vec<SiteRow>>> {
    // A 100 % market lot works at its recipe's stations (`trade_craft`).
    let method = crate::trade_craft::inner(method);
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
                    // Karamja is part of the map for every method (user
                    // 2026-11-16): the boat is a fare like the toll, the
                    // nav and the script walker pay it from the pack.
                    let open_ever = !gates.iter().any(lifescape_gamedata::access::impassable);
                    SiteRow { site: (x, z, level), required: lifescape_gamedata::danger::site_requirement(surface_x, surface_z, surface_level), gates, open_ever }
                })
                .collect::<Vec<_>>(),
        )
    });
    let table = table.filter(|rows| !rows.is_empty());
    cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(method.to_string(), table.clone());
    table
}

/// Coins a trip to `site` takes from the pack: the Al Kharid toll when
/// it crosses the fence (free after Prince Ali Rescue), the boat's round
/// trip when the site is on Karamja.
pub(crate) fn fare_need(site: (i64, i64, i64), position: Option<(i64, i64, i64)>, fare: Fare) -> i64 {
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
    pick_site_crowded(method, combat, position, fare, pass, &[])
}

/// Tiles each other bot working a site adds to its distance: twelve
/// miners on the five Barbarian Village coal rocks (2026-11-15) read
/// every rock empty and failed "no coal mined" 98 times in two hours
/// while the Dwarven Mine and Lumbridge swamp coal sat idle.
pub const CROWD_TILES: i64 = 40;
/// Radius (tiles, same level) within which a fleet site counts as the
/// same working area as a candidate.
pub const CROWD_RADIUS: i64 = 15;

/// Fleet bots of `crowd` working within `CROWD_RADIUS` of `site`.
pub fn crowd_bots(site: (i64, i64, i64), crowd: &[((i64, i64, i64), u32)]) -> i64 {
    crowd
        .iter()
        .filter(|(other, _)| other.2 == site.2 && (other.0 - site.0).abs().max((other.1 - site.1).abs()) <= CROWD_RADIUS)
        .map(|(_, count)| i64::from(*count))
        .sum()
}

/// Crowd penalty of `site`: the bots of `crowd` working within
/// `CROWD_RADIUS`, each worth `CROWD_TILES`.
pub fn crowd_penalty(site: (i64, i64, i64), crowd: &[((i64, i64, i64), u32)]) -> i64 {
    crowd_bots(site, crowd) * CROWD_TILES
}

/// Workers a gathering site holds (plan P7a): its resource nodes (trees,
/// rocks, fishing spots of the method) within `CROWD_RADIUS`; a rock
/// field the miners its respawns feed (`method_rate_trip::miners_fed`,
/// lot 3: four 100-tick coal rocks feed one miner, never four). `None`
/// for a fight camp or a station (no node count: the crowd penalty
/// alone).
pub fn site_capacity(method: &str, site: (i64, i64, i64)) -> Option<i64> {
    let (kind, resource) = method_sites(method)?;
    if kind == "fight" {
        return None;
    }
    let table = method_site_table(method)?;
    let nodes = table.iter().filter(|row| row.site.2 == site.2 && (row.site.0 - site.0).abs().max((row.site.1 - site.1).abs()) <= CROWD_RADIUS).count();
    let nodes = nodes.max(1) as i64;
    if kind == "mining" {
        if let Some(fed) = crate::method_rate_trip::miners_fed(&resource, nodes) {
            return Some(fed);
        }
    }
    Some(nodes)
}

/// True when the fleet already works every node of the site (P7a, D2:
/// the Mining Guild's 5 mithril rocks held 23 miners, 2026-11-17).
/// `crowd`: the bots on the method's resource only (`same_resource_seats`).
pub fn site_full(method: &str, site: (i64, i64, i64), crowd: &[((i64, i64, i64), u32)]) -> bool {
    site_capacity(method, site).is_some_and(|capacity| crowd_bots(site, crowd) >= capacity)
}

/// `pick_site_gated` reading the fleet's sites: among equally open
/// sites, a site with a free node first (`site_full`), then distance
/// (from the surface tile of an underground site) plus the crowd
/// penalty. A guild is a site like any other (user decision D2,
/// 2026-11-17: no guild priority).
pub fn pick_site_crowded(method: &str, combat: u8, position: Option<(i64, i64, i64)>, fare: Fare, pass: &Pass, crowd: &[((i64, i64, i64), u32)]) -> SitePick {
    pick_site_avoiding(method, combat, position, fare, pass, crowd, &[])
}

/// The ready node's site pick: `pick_site_crowded` with the bot's fare,
/// gates and fleet crowd, never on a site this bot cooled for `method`
/// (`cooldown`, plan P1/D4).
pub fn pick_site_for(method: &str, ctx: &crate::expand::ReadyCtx) -> SitePick {
    pick_site_for_loads(method, ctx, BANK_LOADS)
}

/// `pick_site_for` knowing the node's bank loads (`bank_weight`): a
/// small order carried in one load never walks to a far bank's site.
pub fn pick_site_for_loads(method: &str, ctx: &crate::expand::ReadyCtx, loads: i64) -> SitePick {
    let avoid = crate::cooldown::cooled_sites(&ctx.cooled, method);
    let seats = same_resource_seats(method, &ctx.seats);
    pick_site_weighed(method, ctx.combat, ctx.position, Fare::of(ctx), &Pass::of(ctx), &ctx.crowd, &seats, &avoid, loads)
}

/// True when `left` and `right` work the same resource (`method_sites`:
/// kind and resource, `farm:coal` as `gather:mining/coal`).
pub fn same_resource(left: &str, right: &str) -> bool {
    match (method_sites(left), method_sites(right)) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}

/// The bots of `seats` (site, method, count) working `method`'s
/// resource: the crowd a site's capacity is compared to.
pub fn same_resource_seats(method: &str, seats: &[((i64, i64, i64), String, u32)]) -> Vec<((i64, i64, i64), u32)> {
    seats.iter().filter(|(_, other, _)| same_resource(method, other)).map(|(site, _, count)| (*site, *count)).collect()
}

/// `pick_site_crowded` without the `avoid` sites: when they cover every
/// site this combat level works, `Cooled` (never a combat need). Every
/// bot of `crowd` counts as working `method` (`pick_site_seated`).
pub fn pick_site_avoiding(
    method: &str,
    combat: u8,
    position: Option<(i64, i64, i64)>,
    fare: Fare,
    pass: &Pass,
    crowd: &[((i64, i64, i64), u32)],
    avoid: &[(i64, i64, i64)],
) -> SitePick {
    pick_site_seated(method, combat, position, fare, pass, crowd, crowd, avoid)
}

/// The site pick: `crowd` (every fleet bot) lengthens a site's distance,
/// `seats` (the bots on `method`'s resource) fill its capacity
/// (`site_full`; review lot 3: a fighter beside the coal rocks never
/// fills them).
#[allow(clippy::too_many_arguments)]
pub fn pick_site_seated(
    method: &str,
    combat: u8,
    position: Option<(i64, i64, i64)>,
    fare: Fare,
    pass: &Pass,
    crowd: &[((i64, i64, i64), u32)],
    seats: &[((i64, i64, i64), u32)],
    avoid: &[(i64, i64, i64)],
) -> SitePick {
    pick_site_weighed(method, combat, position, fare, pass, crowd, seats, avoid, BANK_LOADS)
}

/// `pick_site_seated` over `loads` bank loads (`bank_weight`).
#[allow(clippy::too_many_arguments)]
pub fn pick_site_weighed(
    method: &str,
    combat: u8,
    position: Option<(i64, i64, i64)>,
    fare: Fare,
    pass: &Pass,
    crowd: &[((i64, i64, i64), u32)],
    seats: &[((i64, i64, i64), u32)],
    avoid: &[(i64, i64, i64)],
    loads: i64,
) -> SitePick {
    let Some(table) = method_site_table(method) else {
        return SitePick::Anywhere;
    };
    let ever: Vec<&SiteRow> = table.iter().filter(|row| row.open_ever).collect();
    // A cooled site covers its whole working area (P2-3a).
    let rows: Vec<&SiteRow> = ever.iter().copied().filter(|row| !avoid.iter().any(|cool| crate::cooldown::same_area(*cool, row.site))).collect();
    if !avoid.is_empty() && ever.iter().any(|row| row.required <= combat) && !rows.iter().any(|row| row.required <= combat) {
        return SitePick::Cooled;
    }
    let distance = |site: &(i64, i64, i64)| {
        // Underground sites (the Mining Guild, the Dwarven Mine) read
        // from their surface entrance, like the bot's own position.
        let (site_x, site_z, site_level) = lifescape_gamedata::access::surface_of(site.0, site.1, site.2);
        let walk = match position {
            Some((x, z, level)) => {
                let (from_x, from_z, from_level) = lifescape_gamedata::access::surface_of(x, z, level);
                (site_x - from_x).abs() + (site_z - from_z).abs() + if site_level == from_level { 0 } else { 1000 }
            }
            None => 0,
        };
        walk + crowd_penalty(*site, crowd) + bank_weight(method, *site, loads)
    };
    let full = |site: &(i64, i64, i64)| site_full(method, *site, seats);
    let across = |site: &(i64, i64, i64)| position.is_some_and(|from| lifescape_gamedata::toll::crossing(from, *site));
    let safe: Vec<&SiteRow> = rows.iter().copied().filter(|row| row.required <= combat).collect();
    // The Karamja boat opens with its fare (budgeted below, like the toll).
    let passes = |gate: &lifescape_gamedata::access::Gate| pass.opens(gate) || matches!(gate, lifescape_gamedata::access::Gate::Boat { area: "karamja", .. });
    let open: Vec<(i64, i64, i64)> = safe.iter().filter(|row| row.gates.iter().all(passes)).map(|row| row.site).collect();
    let boat = |site: &(i64, i64, i64)| lifescape_gamedata::access::behind_boat(site.0, site.1, site.2);
    // Every open site full (P2-3b, D6): the method steps aside.
    if !open.is_empty() && open.iter().all(full) {
        return SitePick::Full;
    }
    // A site with a free node, the nearest first (D2: no guild priority,
    // the crowd lowers a guild like any rock field); a full free site
    // never beats a free paid one.
    if let Some(site) = open.iter().filter(|site| !across(site) && !boat(site) && !full(site)).min_by_key(|site| distance(site)) {
        return SitePick::At(*site);
    }
    // Paid sites: the cheapest fares first, then the nearest.
    if let Some(site) = open.iter().min_by_key(|site| (full(site), fare_need(**site, position, fare), distance(site))) {
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

    /// Live 2026-11-20: fiona_swift (mage, bid goal amulet of power)
    /// cast earth strike with an adamant battleaxe; kevin_ellis (ranger,
    /// equip best ranged) cast in leather. The open fight sets the gear.
    #[test]
    fn the_open_fight_sets_the_gear_style_before_the_goal() {
        use crate::model::{Archetype, Life, Node, NodeKind, NodeStatus, Profile};
        let node = |id: &str, kind: NodeKind, method: Option<&str>, status: NodeStatus| {
            let mut node = Node::new(id, kind, Vec::new(), 1);
            node.method = method.map(str::to_string);
            node.status = status;
            node
        };
        let level = |skill: &str| NodeKind::ReachLevel { skill: skill.to_string(), level: 50 };
        let mut fiona = Life {
            schema: 1,
            account: "style_fiona".to_string(),
            created_at: 0,
            seed: 1,
            profile: Profile::new(false, 0.5, 0.5, 1, Archetype::Producer),
            goal: GoalKind::Produce { item: "amulet_of_power".to_string(), count: 1 },
            nodes: vec![node("n1995", level("magic"), Some("fight:magic/earth_strike/cows"), NodeStatus::Active)],
            engagement: None,
            active: None,
            history: Vec::new(),
            ambition: None,
            goal_origin: None,
            memory: Default::default(),
        };
        fiona.memory.floor = Some(1990);
        assert_eq!(super::fight_style(&fiona), "magic", "the running cast");
        // Between two runs: the next magic level, its method not chosen yet.
        fiona.nodes = vec![node("n1995", level("magic"), Some("fight:magic/earth_strike/cows"), NodeStatus::Done), node("n1996", level("magic"), None, NodeStatus::Ready)];
        assert_eq!(super::fight_style(&fiona), "magic", "the ready level");
        // A ranged goal fighting magic now wears for magic.
        fiona.goal = GoalKind::EquipBest { style: "ranged".to_string() };
        assert_eq!(super::fight_style(&fiona), "magic");
        // A melee fight within a magic goal: melee.
        fiona.goal = GoalKind::MaxSkills { skills: vec!["magic".to_string()], level: 70 };
        fiona.nodes = vec![node("n1997", level("attack"), Some("fight:melee/cows"), NodeStatus::Ready)];
        assert_eq!(super::fight_style(&fiona), "melee");
        // No fight open, or only an earlier goal's: the goal's style.
        fiona.nodes = vec![node("n1500", level("magic"), Some("fight:magic/earth_strike/cows"), NodeStatus::Ready)];
        assert_eq!(super::fight_style(&fiona), "magic", "the goal's own (magic)");
        fiona.goal = GoalKind::Produce { item: "amulet_of_power".to_string(), count: 1 };
        assert_eq!(super::fight_style(&fiona), "melee", "an earlier goal's node is no fight of this one");
    }

    /// Live 2026-11-20: smelters banked at Draynor and smelted at the
    /// Lumbridge furnace (no bank beside it); Al Kharid's furnace stands
    /// by its bank. Weighed over its bank loads, the fastest furnace wins
    /// even from Draynor, the one walk there paid once.
    #[test]
    fn a_smelter_takes_the_furnace_beside_a_bank() {
        use super::{bank_weight, pick_site_paid, Fare, SitePick, BANK_LOADS};
        let tiles = |site| lifescape_gamedata::stations::stations_of("furnace").into_iter().find(|post| (post.x, post.z, post.level) == site).map(|_| bank_weight("smelt:iron_bar", site, BANK_LOADS));
        let lumbridge = tiles((3225, 3256, 0)).expect("lumbridge furnace");
        let al_kharid = tiles((3272, 3185, 0)).expect("al kharid furnace");
        assert!(al_kharid < lumbridge, "al kharid {al_kharid} vs lumbridge {lumbridge}");
        let draynor_bank = Some((3092, 3245, 0));
        let pick = pick_site_paid("smelt:iron_bar", 60, draynor_bank, Fare { wallet: 10_000, free_pass: false });
        let site = match pick {
            SitePick::At(site) | SitePick::Tolled(site) => site,
            other => panic!("a furnace: {other:?}"),
        };
        assert!((site.0 - 3272).abs() <= 2 && (site.1 - 3185).abs() <= 2, "al kharid's furnace, got {site:?}");
        assert_eq!(bank_weight("fight:melee/cows", (3255, 3270, 0), BANK_LOADS), 0, "a fight weighs no bank");
        assert_eq!(bank_weight("smelt:iron_bar", (3225, 3256, 0), 1), 0, "one load rides in the pack");
    }
}
