//! Fill water (T): buckets, jugs and bowls at F2P water points.
//! Declaration on the kit: the container is the `produce` input, the water
//! point the station, the interface is direct (no dialogue). Wells only take
//! buckets (`water_sources.rs2`); fountains and sinks take every vessel.
//! No level, no XP: 16 fountains, 13 sinks, 7 wells (stations P2.1,
//! catalogue §6, P2, epoch P2).

#![forbid(unsafe_code)]

use lifescape_sdk::Bot;

use crate::kit::bank::{bank_trip, bank_trip_resilient, bank_withdraw_items, nearest_bank};
use crate::kit::cycle::{resolve_keep, run, StepControl};
use crate::kit::pack::{obj_id, pack_total};
use crate::kit::produce::{self, Produce, ProduceJob, ProduceSite, ProduceUi};
use crate::kit::survival::{check_events, is_death_abort, wait_pack_streamed};
use crate::{ScriptError, ScriptResult};

/// Catalogue §6 epoch and archetype (P2 water point harvest, T).
pub const EPOCH: &str = "P2";
pub const ARCH: &str = "T";
/// Empty vessel to filled water (the dump `next_obj_stage` chain).
pub const PAIRS: [(&str, &str); 4] = [
    ("bucket_empty", "bucket_water"),
    ("jug_empty", "jug_water"),
    ("bowl_empty", "bowl_water"),
    ("vial_empty", "vial_water"),
];
/// Every vessel fills at fountains and sinks; wells are buckets only.
pub const STATIONS_VESSEL: [&str; 3] = ["fountain", "sink", "sink2"];
pub const STATIONS_BUCKET: [&str; 4] = ["fountain", "sink", "sink2", "well"];

/// Filled water for an empty vessel name (bare names alias the empties).
pub fn empty_to_full(name: &str) -> Option<&'static str> {
    let empty = normalize_container(name)?;
    PAIRS.iter().find(|(vessel, _)| *vessel == empty).map(|(_, full)| *full)
}

/// Canonical empty name for a vessel (`bucket` aliases `bucket_empty`).
pub fn normalize_container(name: &str) -> Option<&'static str> {
    PAIRS.iter().find(|(vessel, _)| *vessel == name).map(|(vessel, _)| *vessel).or_else(|| {
        PAIRS.iter().find(|(vessel, _)| vessel.strip_suffix("_empty") == Some(name)).map(|(vessel, _)| *vessel)
    })
}

/// Station debugnames for one empty vessel (wells refuse jugs and bowls).
pub fn water_stations(empty: &str) -> &'static [&'static str] {
    if empty == "bucket_empty" { &STATIONS_BUCKET } else { &STATIONS_VESSEL }
}

/// Fill banked loads in the open session: `fill_water [vessel] [count]`
/// (buckets when omitted, 0 = forever).
pub fn run_fill_water(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let (resource, rest) = crate::kit::cycle::split_resource(args);
    let spec = crate::kit::cycle::parse_cli("fill_water", &rest)?;
    if spec.allow_paid {
        bot.allow_paid = true;
        bot.log("toll gates allowed for this run");
    }
    bot.allow_shop = spec.allow_shop;
    let task_keep = resolve_keep(bot, &spec)?;
    wait_pack_streamed(bot)?;
    check_events(bot)?;
    let empty_name = resource.as_deref().map(normalize_container).unwrap_or(Some("bucket_empty"))
        .ok_or_else(|| ScriptError::Usage(format!("no F2P water vessel named {:?} at {:?}", resource, bot.position())))?;
    let full_name = empty_to_full(empty_name).expect("pair covers every vessel");
    let empty = obj_id(bot, empty_name)?;
    let full = obj_id(bot, full_name)?;
    let fill = Produce { verb: "filling", site: ProduceSite::Loc(water_stations(empty_name)), ui: ProduceUi::Direct };
    let mut bank = nearest_bank(bot)?;
    bot.log(&format!("bank at ({},{})", bank.x, bank.z));
    let mut keep_empty: Vec<u16> = vec![empty];
    keep_empty.extend(task_keep.iter().copied());
    bank_trip(bot, &bank, &keep_empty, 0)?;
    fill_empties(bot, &bank, empty)?;
    if pack_total(bot, empty) == 0 {
        return Err(ScriptError::Blocked { missing: vec![empty_name.to_string()] });
    }
    let outcome = run(bot, &spec, |step_bot, state| {
        check_events(step_bot)?;
        if pack_total(step_bot, empty) == 0 {
            bank_trip(step_bot, &bank, &keep_empty, 0)?;
            fill_empties(step_bot, &bank, empty)?;
            if pack_total(step_bot, empty) == 0 {
                return Err(ScriptError::Blocked { missing: vec![empty_name.to_string()] });
            }
        }
        let job = ProduceJob { spec: &fill, input: empty, input_name: empty_name, want: 0, products: vec![full], near: (bank.x as i32, bank.z as i32) };
        match produce::run_job(step_bot, &job) {
            Err(error) if is_death_abort(&error) => {
                step_bot.log("died filling, restocking to retry");
                return Ok(StepControl::Continue);
            }
            Err(error) => return Err(error),
            Ok(batch) => step_bot.log(&format!("ok filled {} ({} made)", batch.consumed, batch.made)),
        }
        bank = nearest_bank(step_bot)?;
        match bank_trip_resilient(step_bot, &mut bank, &task_keep, 0, &[full]) {
            Ok(_) => {}
            Err(error) if is_death_abort(&error) => {
                step_bot.log("died on the bank trip, filling again");
                return Ok(StepControl::Continue);
            }
            Err(error) => return Err(error),
        }
        step_bot.log(&format!("ok load {} filled", state.units() + 1));
        Ok(StepControl::UnitDone)
    });
    outcome
}

/// Top the pack up with banked empties (nothing when the bank holds none or
/// the booth is far: the caller reports `blocked`, never an empty walk).
fn fill_empties(bot: &mut Bot, bank: &lifescape_gamedata::banks::Bank, empty: u16) -> Result<(), ScriptError> {
    let free = crate::kit::pack::free_slots(bot) as u32;
    if free == 0 {
        return Ok(());
    }
    if !crate::kit::withdraw::reach_bank_for(bot, bank, &[empty])? {
        return Ok(());
    }
    bank_withdraw_items(bot, bank, &[empty], free)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{empty_to_full, normalize_container, water_stations, ARCH, EPOCH, PAIRS, STATIONS_BUCKET, STATIONS_VESSEL};
    use crate::kit::pack::obj_id;
    use crate::tables_live::attach_live_tables;
    use lifescape_gamedata as knowledge;
    use lifescape_sdk::Bot;

    fn offline_bot() -> Bot {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot
    }

    /// Vessels: empties map to filled water, bare names alias the empties,
    /// junk maps to nothing. Epoch P2, archetype T.
    #[test]
    fn vessels_map_empty_to_full() {
        assert_eq!((EPOCH, ARCH), ("P2", "T"));
        assert_eq!(PAIRS.len(), 4);
        assert_eq!(empty_to_full("bucket_empty"), Some("bucket_water"));
        assert_eq!(empty_to_full("bucket"), Some("bucket_water"), "bare alias");
        assert_eq!(empty_to_full("VIAL"), None, "case-sensitive vessels");
        assert_eq!(normalize_container("jug"), Some("jug_empty"));
        assert_eq!(normalize_container("bowl_empty"), Some("bowl_empty"));
        assert_eq!(normalize_container("coins"), None, "junk never fills");
    }

    /// Wells are buckets only (`water_sources.rs2` `_well` takes just
    /// `bucket_empty`); every other vessel fills at fountains and sinks.
    #[test]
    fn wells_take_buckets_only() {
        assert!(water_stations("bucket_empty").contains(&"well"), "buckets: {STATIONS_BUCKET:?}");
        for vessel in ["jug_empty", "bowl_empty", "vial_empty"] {
            assert!(!water_stations(vessel).contains(&"well"), "{vessel} skips wells: {:?}", water_stations(vessel));
            assert!(water_stations(vessel).contains(&"fountain"), "{vessel} fills at fountains");
        }
        assert_eq!(STATIONS_VESSEL, ["fountain", "sink", "sink2"]);
        assert_eq!(STATIONS_BUCKET, ["fountain", "sink", "sink2", "well"]);
    }

    /// Vessels resolve and are F2P; water point counts match the catalogue
    /// (16 fountains, 13 sinks, 7 wells).
    #[test]
    fn vessels_and_points_are_f2p() {
        let bot = offline_bot();
        for (empty, full) in PAIRS {
            obj_id(&bot, empty).expect("empty in tables");
            obj_id(&bot, full).expect("full in tables");
            for name in [empty, full] {
                let row = knowledge::objs_by_name(name).into_iter().next().expect("dump row");
                assert_eq!(row.members, Some(false), "{name} is F2P, got {:?}", row.members);
            }
        }
        assert_eq!(knowledge::stations::stations_of("fountain").len(), 16, "fountains");
        assert_eq!(knowledge::stations::stations_of("sink").len(), 13, "sinks");
        assert_eq!(knowledge::stations::stations_of("well").len(), 7, "wells");
        for station in ["fountain", "sink", "sink2", "well"] {
            assert!(knowledge::all_locs().iter().any(|entry| entry.name.as_deref() == Some(station)), "{station} loc in dumps");
            assert!(!knowledge::stations::stations_of(station).is_empty() || station == "sink2", "{station} placed");
        }
    }

    /// CLI: a bare number is a fill count (the gather convention), a vessel
    /// name fills the resource the brain would otherwise fix.
    #[test]
    fn bare_count_parses() {
        let spec = crate::kit::cycle::parse_cli("fill_water", &["3".to_string()]).expect("count parses");
        assert_eq!(spec.targets.count, Some(3));
        assert!(spec.resource.is_empty(), "no resource positional");
    }
}
