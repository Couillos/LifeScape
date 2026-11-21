//! Station pick for `kit::produce`: the tiles of a station debugname
//! near the anchor (live stream, served static map, station dump), minus
//! the sealed ones and those behind a gate the bot does not pass now
//! (`kit::gates`), ranked by walking cost. Split from `produce`
//! (file-size rule).

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

/// Manhattan radius of the station search around the anchor.
const STATION_RADIUS: i32 = 150;

/// Manhattan radius of the brain's site: the station it names (a site
/// is a station tile; a range or furnace spans a few tiles).
const SITE_STATION_RADIUS: i32 = 8;

/// Lowercase debugname of a loc id, empty when the dump does not know it.
pub(crate) fn station_debugname(loc: u16) -> String {
    knowledge::loc_by_id(usize::from(loc)).and_then(|entry| entry.name).unwrap_or_default().to_lowercase()
}

/// Tiles with exactly this station debugname, same level, within the
/// station radius of `near`: live stream first, one static batch second
/// (the worker paints nothing). Nearest first; exact match only, so
/// orange furniture never reads as a cooking range.
/// Stations ranked by Manhattan distance and then re-ranked by walking cost.
const STATION_SHORTLIST: usize = 8;

/// The station to work: the nearest of `site_tiles` that a route
/// reaches (one `walk_costs` batch, `skill_walk::refine_walking`;
/// routed first). Never the closest by Manhattan distance alone (live
/// 2026-11-17: 9 cooks from Edgeville sent to the Black Knights' banquet
/// fireplace (3030,3514), sealed from outside: door loops, then
/// "consumed nothing"). Nav unwired: Manhattan order, as before.
///
/// With a brain site (`--site`), only the station of that site: the
/// brain weighed its bank walks (`planner::combat::site_loads`), the
/// nearest station to the bot would undo it (user 2026-11-21: cooks
/// banked at Draynor and walked to the Lumbridge range). None reached
/// there is `None`, the caller's typed failure.
pub(crate) fn reachable_station(bot: &mut Bot, name: &str, near: (i32, i32), level: i32) -> Option<(i32, i32, i32, u16)> {
    let mut cands = site_tiles(bot, name, near, level);
    if bot.assigned_site.is_some() {
        cands.retain(|(x, z, _, _)| (x - near.0).abs() + (z - near.1).abs() <= SITE_STATION_RADIUS);
    }
    let ranked = crate::skill_walk::refine_walking(bot, cands, STATION_SHORTLIST, name, |tile| lifescape_nav::plan::Tile { x: tile.0, z: tile.1, level: tile.2 });
    ranked.into_iter().next().map(|(_, tile)| tile)
}

/// Stations on `level`: the brain's site level (an upstairs range), the
/// bot's own without a site.
pub(crate) fn site_tiles(bot: &mut Bot, name: &str, near: (i32, i32), level: i32) -> Vec<(i32, i32, i32, u16)> {
    let mut cands: Vec<(i64, (i32, i32, i32, u16))> = Vec::new();
    let mut consider = |x: i32, z: i32, tile_level: i32, loc: u16| {
        // A station the map proves sealed (nav `site_reach` dump) never
        // runs: the Black Knights' banquet fireplace (3030,3514).
        let sealed = knowledge::site_reach::unreachable(i64::from(x), i64::from(z), i64::from(tile_level)).is_some();
        if tile_level != level || sealed || station_debugname(loc) != name {
            return;
        }
        let dist = i64::from((x - near.0).abs() + (z - near.1).abs());
        if dist <= i64::from(STATION_RADIUS) {
            cands.push((dist, (x, z, tile_level, loc)));
        }
    };
    for (tile, slot) in &bot.world.locs {
        if let Some((loc, _, _)) = slot {
            consider(tile.0, tile.1, tile.2, *loc);
        }
    }
    match bot.static_locs_near(near.0, near.1, level, STATION_RADIUS) {
        Ok(rows) => {
            for row in rows {
                consider(row.x, row.z, row.level, row.loc);
            }
        }
        Err(error) => {
            bot.log(&format!("station search: static locs unavailable ({error}), live only"));
        }
    }
    // The served static map keeps one loc per tile: a wall on the same
    // tile hides the station (live 2026-11-14, Barbarian Village wheels
    // (3087,3409)/(3087,3410) share their tiles with the hut walls, "no
    // produce station potterywheel"). The station dump names them all.
    for station in knowledge::stations::stations_of(name) {
        if let Ok(loc) = u16::try_from(station.loc_id) {
            consider(station.x as i32, station.z as i32, station.level as i32, loc);
        }
    }
    // A station behind a gate the bot does not pass now (guild door:
    // level and WORN uniform, `kit::gates`) is never a candidate: the
    // walk-cost ranking would price it at its closest reachable tile.
    cands.retain(|(_, (x, z, tile_level, _))| crate::kit::gates::tile_open(bot, *x, *z, *tile_level));
    cands.sort();
    cands.dedup();
    cands.into_iter().map(|(_, tile)| tile).collect()
}

#[cfg(test)]
mod tests {
    use super::site_tiles;
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    fn offline_bot() -> Bot {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot
    }

    /// Live 2026-11-21: the brain's Edgeville range (3081,3508,1) lost the
    /// walk-cost pick to the Cooks' Guild range (3143,3452,1) for cooks
    /// with no chef's hat worn (door 2712, `cooking_guild.rs2`): the walk
    /// cost of a blocked station is its closest reachable tile (73 for
    /// the guild range without the hat, a floor elsewhere). The gamedata
    /// gate drops it before the ranking; cooking 32 and the hat worn keep
    /// it, and its shorter walk wins.
    #[test]
    fn the_guild_range_needs_the_worn_hat() {
        let mut bot = offline_bot();
        crate::wire_nav(&mut bot);
        let world = bot.nav_world.clone().expect("wired");
        let tables = bot.nav_tables.clone().expect("wired");
        let service = lifescape_nav::service::Service::start(
            lifescape_nav::service::Snapshot::new(world, tables, "test-guild-range".to_string()),
            &lifescape_nav::service::test_socket("guild-range"),
            1,
        )
        .expect("service starts");
        bot.plan_client = Some(lifescape_nav::service::PlanClient::new(service.socket_path(), "guild-range"));
        bot.plan_data_version = Some("test-guild-range".to_string());
        bot.last_keepalive = Some(std::time::Instant::now());
        (bot.world.x, bot.world.z, bot.world.level) = (3185, 3442, 0);
        bot.world.stats.insert(crate::activities::cook::COOK_STAT, (52, 130_000));
        let guild = |tile: Option<(i32, i32, i32, u16)>| tile.is_some_and(|(x, z, _, _)| (3138..=3148).contains(&x) && (3445..=3453).contains(&z));
        let bare = super::reachable_station(&mut bot, "range", (3081, 3508), 1);
        assert!(bare.is_some() && !guild(bare), "no hat worn: never the guild range, got {bare:?}");
        let hat = crate::kit::pack::obj_id(&bot, "chefs_hat").expect("chef's hat in the dumps");
        let worn_com = (bot.tables.com_id)(crate::kit::gear::WORN_COM_NAME).expect("worn com in the dumps");
        bot.world.inv.insert(worn_com, [(0u8, (hat, 1u32))].into_iter().collect());
        bot.last_keepalive = Some(std::time::Instant::now());
        let hatted = super::reachable_station(&mut bot, "range", (3081, 3508), 1);
        assert!(guild(hatted), "cooking 52 and the hat worn: the nearer guild range, got {hatted:?}");
    }

    /// User 2026-11-21: the brain's site names the station. Hat worn, the
    /// guild range is the nearer walk from Varrock, yet the brain's
    /// Edgeville range (3081,3508,1) is the one worked; without a site the
    /// nearest walk still wins.
    #[test]
    fn the_brain_site_station_beats_a_nearer_one() {
        let mut bot = offline_bot();
        crate::wire_nav(&mut bot);
        let world = bot.nav_world.clone().expect("wired");
        let tables = bot.nav_tables.clone().expect("wired");
        let service = lifescape_nav::service::Service::start(
            lifescape_nav::service::Snapshot::new(world, tables, "test-site-range".to_string()),
            &lifescape_nav::service::test_socket("site-range"),
            1,
        )
        .expect("service starts");
        bot.plan_client = Some(lifescape_nav::service::PlanClient::new(service.socket_path(), "site-range"));
        bot.plan_data_version = Some("test-site-range".to_string());
        bot.last_keepalive = Some(std::time::Instant::now());
        (bot.world.x, bot.world.z, bot.world.level) = (3185, 3442, 0);
        bot.world.stats.insert(crate::activities::cook::COOK_STAT, (52, 130_000));
        let hat = crate::kit::pack::obj_id(&bot, "chefs_hat").expect("chef's hat in the dumps");
        let worn_com = (bot.tables.com_id)(crate::kit::gear::WORN_COM_NAME).expect("worn com in the dumps");
        bot.world.inv.insert(worn_com, [(0u8, (hat, 1u32))].into_iter().collect());
        let free = super::reachable_station(&mut bot, "range", (3081, 3508), 1);
        assert!(free.is_some_and(|(x, z, _, _)| (x - 3081).abs() + (z - 3508).abs() > 8), "no site: the nearest walk, got {free:?}");
        bot.assigned_site = Some((3081, 3508, 1));
        bot.last_keepalive = Some(std::time::Instant::now());
        let sited = super::reachable_station(&mut bot, "range", (3081, 3508), 1);
        assert!(sited.is_some_and(|(x, z, _, _)| (x - 3081).abs() + (z - 3508).abs() <= 8), "the brain's range, got {sited:?}");
    }

    /// Live station scan: a streamed range tile resolves by exact
    /// debugname; unknown stations find nothing.
    #[test]
    fn live_station_resolves_by_exact_debugname() {
        let mut bot = offline_bot();
        bot.world.level = 0;
        let range_id = (0u16..)
            .find(|id| super::station_debugname(*id) == "range")
            .expect("a range loc exists");
        bot.world.locs.insert((3200, 3200, 0), Some((range_id, 0, 0)));
        let found = site_tiles(&mut bot, "range", (3200, 3200), 0);
        assert!(found.iter().any(|(x, z, _, _)| (*x, *z) == (3200, 3200)), "streamed range resolves, got: {found:?}");
        assert!(site_tiles(&mut bot, "no_such_station", (3200, 3200), 0).is_empty(), "unknown station finds nothing");
    }

    /// Live 2026-11-14 (admin, `pottery --site 3087,3410,0`): the served
    /// static map keeps the hut wall on the wheel tiles, "no produce
    /// station potterywheel". The station dump finds both wheels.
    #[test]
    fn wheels_under_walls_come_from_the_station_dump() {
        let mut bot = offline_bot();
        bot.world.level = 0;
        let found = site_tiles(&mut bot, "potterywheel", (3087, 3410), 0);
        assert!(found.iter().any(|(x, z, _, _)| (*x, *z) == (3087, 3409)), "{found:?}");
        assert!(found.iter().any(|(x, z, _, _)| (*x, *z) == (3087, 3410)), "{found:?}");
        assert!(site_tiles(&mut bot, "potterywheel", (3200, 3200), 0).iter().all(|(x, _, _, _)| *x != 3087), "out of radius");
    }

    /// Live 2026-11-20 (ella_quill and 12 more, `cook --site 3081,3508,1`,
    /// 115 NoRoute in 24 h): the brain names the Edgeville range upstairs
    /// (level 1); the search kept the bot's level 0, missed it, and took
    /// the only level-0 cooking loc in range, the Black Knights' banquet
    /// fireplace (3030,3514), sealed in the fortress: "I can't reach
    /// that!" on every outside stand, then `Disconnected` on the inside
    /// ones. The search reads the site's level.
    #[test]
    fn the_station_search_reads_the_site_level() {
        let mut bot = offline_bot();
        bot.world.level = 0;
        let found = site_tiles(&mut bot, "range", (3081, 3508), 1);
        assert!(found.iter().any(|(x, z, level, _)| (*x, *z, *level) == (3081, 3508, 1)), "the upstairs range, got: {found:?}");
        assert!(found.iter().all(|(_, _, level, _)| *level == 1), "site level only, got: {found:?}");
    }

    /// Same fireplace without a brain site (108 "cooking consumed
    /// nothing", ray_79 20 of them): streamed or served, a station the
    /// map proves sealed (`unreachable_sites.json`, nav `site_reach`) is
    /// never a candidate.
    #[test]
    fn a_sealed_station_is_never_a_candidate() {
        let mut bot = offline_bot();
        bot.world.level = 0;
        let fireplace = (0u16..).find(|id| super::station_debugname(*id) == "grandfireplace_oldbrick").expect("fireplace loc");
        bot.world.locs.insert((3030, 3514, 0), Some((fireplace, 0, 0)));
        assert!(site_tiles(&mut bot, "grandfireplace_oldbrick", (3060, 3500), 0).is_empty(), "the banquet fireplace is sealed");
    }
}
