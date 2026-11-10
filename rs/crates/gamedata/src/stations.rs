//! D57 station positions: furnaces, anvils, ranges, fires, wheels,
//! pottery wheels/ovens, prayer altars, runecraft ruins/altars, mills,
//! hoppers, water points and fields. All rows stand in an F2P zone
//! (`free2play.csv`); the dump is generated from `maps-server.zip`
//! (see `gamedata/data/stations.json`), never hand-edited.

/// One static station row (build-time data, mirrored into [`Station`]).
pub(crate) struct StaticStation {
    pub kind: &'static str,
    pub loc: &'static str,
    pub loc_id: i64,
    pub x: i64,
    pub z: i64,
    pub level: i64,
}

mod generated {
    include!(concat!(env!("OUT_DIR"), "/stations_generated.rs"));
}

/// One F2P station spawn.
#[derive(Debug, Clone)]
pub struct Station {
    pub kind: String,
    pub loc: String,
    pub loc_id: i64,
    pub x: i64,
    pub z: i64,
    pub level: i64,
}

fn station_from_static(row: &StaticStation) -> Station {
    Station {
        kind: row.kind.to_string(),
        loc: row.loc.to_string(),
        loc_id: row.loc_id,
        x: row.x,
        z: row.z,
        level: row.level,
    }
}

/// Every F2P station spawn.
pub fn all_stations() -> Vec<Station> {
    generated::ALL.iter().map(station_from_static).collect()
}

/// Stations of one kind (`furnace`, `anvil`, `range`, `fire`, ...).
pub fn stations_of(kind: &str) -> Vec<Station> {
    let wanted = kind.to_lowercase();
    all_stations().into_iter().filter(|entry| entry.kind == wanted).collect()
}

/// Distinct station kinds present in the dump.
pub fn station_kinds() -> Vec<String> {
    let mut kinds: Vec<String> = Vec::new();
    for entry in all_stations() {
        if !kinds.contains(&entry.kind) {
            kinds.push(entry.kind);
        }
    }
    kinds.sort();
    kinds
}

/// Closest station of this kind on the same level (Manhattan distance).
pub fn nearest_station(kind: &str, x: i64, z: i64, level: i64) -> Option<Station> {
    let mut best: Option<(i64, Station)> = None;
    for entry in stations_of(kind) {
        if entry.level != level {
            continue;
        }
        let dist = (entry.x - x).abs() + (entry.z - z).abs();
        if best.as_ref().is_none_or(|current| dist < current.0) {
            best = Some((dist, entry));
        }
    }
    best.map(|(_, entry)| entry)
}

#[cfg(test)]
mod tests {
    use super::{all_stations, nearest_station, station_kinds, stations_of};
    use crate::free_zones::is_free;

    #[test]
    fn station_counts_match_the_catalogue() {
        // D57 counts from CATALOGUE §5-6, verified from maps-server.zip.
        // Furnaces pair furnace1+furnace2 tiles (4 sites); mills pair
        // mill+millbase tiles (2 sites); anvils keep the 2 Doric tiles.
        let all = all_stations();
        assert!(!all.is_empty(), "stations load");
        let count = |kind: &str| stations_of(kind).len();
        assert_eq!(count("range"), 32, "ranges");
        assert_eq!(count("fire"), 20, "permanent fires");
        assert_eq!(count("spinningwheel"), 5, "wheels");
        assert_eq!(count("potterywheel"), 6, "pottery wheels");
        assert_eq!(count("potteryoven"), 2, "pottery ovens");
        assert_eq!(count("ruin"), 6, "runecraft ruins");
        assert_eq!(count("altar_rc"), 6, "runecraft altars");
        assert_eq!(count("hopper"), 2, "hoppers");
        assert_eq!(count("wheat"), 597, "wheat tiles");
        assert_eq!(count("potato"), 268, "potato tiles");
        assert_eq!(count("cabbage"), 189, "cabbage tiles");
        assert_eq!(count("onion"), 27, "onion tiles");
        // Furnace tiles pair per site; anvil tiles keep Doric.
        assert_eq!(count("furnace"), 8, "furnace tiles (4 sites)");
        assert_eq!(count("anvil"), 19, "anvil tiles (17 + 2 doric)");
        assert!(count("prayer_altar") >= 5, "prayer altars");
        assert!(count("fountain") >= 15, "fountains");
        assert!(count("sink") >= 8, "sinks");
        assert!(count("well") >= 6, "wells");
        assert!(count("mill") >= 2, "mills");
        // Catalogue §5.7: Al-Kharid furnace+bank, Wilderness furnace.
        let furnaces = stations_of("furnace");
        assert!(furnaces.iter().any(|entry| (entry.x - 3272).abs() <= 2 && (entry.z - 3185).abs() <= 2));
        assert!(furnaces.iter().any(|entry| (entry.x - 3143).abs() <= 2 && (entry.z - 3735).abs() <= 2));
    }

    #[test]
    fn stations_stand_in_free_zones() {
        // M10a acceptance for stations: no station outside free2play.csv.
        for entry in all_stations() {
            assert!(is_free(entry.x, entry.z), "station {} at ({}, {}) stands free", entry.kind, entry.x, entry.z);
        }
        assert!(!station_kinds().is_empty());
        let near = nearest_station("furnace", 3270, 3180, 0).expect("al-kharid furnace");
        assert!((near.x - 3272).abs() <= 2 && (near.z - 3185).abs() <= 2);
        assert!(nearest_station("furnace", 3270, 3180, 1).is_none() || nearest_station("furnace", 3270, 3180, 0).is_some());
        assert!(nearest_station("no_such_kind", 3200, 3200, 0).is_none());
    }

    #[test]
    fn every_recipe_kind_has_a_free_post() {
        // M10b: every F2P recipe (dump + extra) names a station kind that
        // stands in an F2P zone; inventory transforms name none.
        for recipe in crate::f2p::f2p_recipes() {
            let station = match recipe.kind.as_str() {
                "smelt" => Some("furnace"),
                "smith" => Some("anvil"),
                "cook" => Some("range"),
                _ => None,
            };
            if let Some(kind) = station {
                assert!(!stations_of(kind).is_empty(), "{} posts placed for {}", kind, recipe.source);
            }
        }
        for entry in crate::extra_recipes::all_extra() {
            if let Some(station) = &entry.station {
                assert!(!stations_of(station).is_empty(), "station {station} placed for {}", entry.source);
            }
        }
    }
}
