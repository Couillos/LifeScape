//! Temporary probe: dump doors and walkability around candidate areas.

use std::sync::Arc;

use lifescape_nav::static_world::StaticWorld;

fn root() -> std::path::PathBuf {
    if let Ok(dir) = std::env::var("LIFESCAPE_ENGINE_DIR") {
        if !dir.is_empty() {
            return std::path::PathBuf::from(dir).join("../LifeScape");
        }
    }
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..")
}

fn world() -> Arc<StaticWorld> {
    let root = root();
    let zip = root.join("lostcity/engine/data/pack/.cache/maps-server.zip");
    let gametypes = std::fs::read_to_string(root.join("LifeScape/gamedata/data/gametypes.json")).unwrap();
    let overrides = std::fs::read_to_string(root.join("LifeScape/gamedata/data/nav_overrides.json")).unwrap();
    let defs = lifescape_nav::loctypes::parse_loctypes(&gametypes, &overrides).unwrap();
    StaticWorld::open(&zip, defs).unwrap()
}

#[test]
fn probe_doors() {
    let world = world();
    for (label, cx, cz, level, radius) in [
        ("lumbridge-castle-door", 3222, 3218, 0, 8),
        ("lumbridge-throne", 3208, 3220, 0, 8),
        ("bk-door3", 3025, 3511, 1, 6),
        ("temple-outside", 3408, 3491, 0, 6),
        ("temple-inside", 3408, 3485, 0, 6),
        ("toll", 3268, 3227, 0, 4),
        ("lumbridge-cellar", 3209, 9616, 0, 6),
        ("varrock-sewer", 3237, 9858, 0, 6),
        ("entrana", 2820, 3357, 0, 10),
    ] {
        println!("--- {label} ---");
        for dx in -radius..=radius {
            for dz in -radius..=radius {
                let (x, z) = (cx + dx, cz + dz);
                if let Some((id, shape, angle)) = world.door_at(x, z, level) {
                    let toll = world.is_toll(x, z, level);
                    println!("door id={id} at ({x},{z},{level}) shape={shape} angle={angle} toll={toll}");
                }
            }
        }
        for (x, z) in [(cx, cz), (cx + 1, cz), (cx, cz + 1)] {
            let flags = world.get(x, z, level);
            println!(
                "tile ({x},{z},{level}) flags={flags:#x} walkable={} loc={:?}",
                lifescape_nav::collision::tile_walkable(flags),
                world.loc_at(x, z, level)
            );
        }
    }
}
