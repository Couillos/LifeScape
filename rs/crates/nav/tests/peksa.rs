//! Poche Peksa (boutique des casques, dossier `admin-4975`) : la porte
//! 1530 est dans les dumps et franchissable du bon cote, la sortie
//! reste un `Disconnected` fidele au serveur (porte unilaterale angle
//! SUD) qui nomme desormais son joint : loc, nom, porte, tuile, stand.

use std::sync::Arc;

use lifescape_nav::plan::{self, NavOptions, Segment, Tile};
use lifescape_nav::static_world::StaticWorld;
use lifescape_nav::tables::Tables;

fn setup() -> (Arc<StaticWorld>, Tables) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..");
    let zip = root.join("lostcity/engine/data/pack/.cache/maps-server.zip");
    let gametypes = std::fs::read_to_string(root.join("LifeScape/gamedata/data/gametypes.json")).expect("gametypes");
    let overrides = std::fs::read_to_string(root.join("LifeScape/gamedata/data/nav_overrides.json")).expect("overrides");
    let defs = lifescape_nav::loctypes::parse_loctypes(&gametypes, &overrides).expect("loctypes");
    let world = StaticWorld::open(&zip, defs).expect("static world");
    let zones = lifescape_nav::f2p::load_zones(&root.join("lostcity/content/maps/free2play.csv")).expect("f2p");
    world.set_f2p(zones);
    let tables = Tables::load(&root.join("LifeScape/gamedata/data")).expect("tables");
    world.set_sky_floors(tables.sky_floors.clone());
    (world, tables)
}

/// La porte est dans nos dumps : 1530 (Open, mur droit angle SUD) sur
/// le mur nord de la boutique (3076,3427,0).
#[test]
fn peksa_door_is_in_the_dumps() {
    let (world, tables) = setup();
    assert_eq!(world.door_at(3076, 3427, 0), Some((1530, 0, 3)), "porte Peksa absente de la nav");
    assert_eq!(world.loc_at(3076, 3427, 0), Some((1530, 0, 3)));
    let def = world.loc_def(1530).expect("def 1530");
    assert!(def.blockwalk);
    assert_eq!(def.open_op, Some(1), "la porte doit exposer son op Open");
    assert_eq!(tables.loc_name(1530), "loc_1530", "porte anonyme : le loc id reste l'identite");
}

/// L'entree passe la porte depuis le stand exterieur (3076,3426).
#[test]
fn peksa_enter_swings_door_1530() {
    let (world, tables) = setup();
    let plan = plan::plan(
        &world,
        &tables,
        Tile { x: 3076, z: 3424, level: 0 },
        Tile { x: 3075, z: 3429, level: 0 },
        &NavOptions::default(),
    )
    .expect("l'entree Peksa planifie");
    let door = plan.segments.iter().find_map(|segment| match segment {
        Segment::OpenDoor { door, stand, .. } => Some((door, stand)),
        _ => None,
    });
    let (door, stand) = door.expect("l'entree ouvre la porte 1530");
    assert_eq!(door.loc, 1530);
    assert_eq!((door.x, door.z, door.level), (3076, 3427, 0));
    assert_eq!((stand.x, stand.z), (3076, 3426), "stand exterieur, le seul que le moteur accepte");
}

/// Sortie `admin-4975` : `Disconnected` fidele au serveur, mais le
/// joint est nomme — porte 1530, tuile, op et stand exterieur.
#[test]
fn peksa_exit_names_the_seal() {
    let (world, tables) = setup();
    let from = Tile { x: 3077, z: 3429, level: 0 };
    let dest = Tile { x: 3081, z: 3428, level: 0 };
    let mut options = NavOptions::default();
    options.f2p_only = true;
    let no = plan::plan(&world, &tables, from, dest, &options).expect_err("la sortie reste scellee (porte unilaterale)");
    assert_eq!(no.reason, plan::NoRouteReason::Disconnected);
    assert_eq!(no.closest_reachable, from);
    assert!(!no.blockers.is_empty(), "un Disconnected doit dire ce qui scelle");
    let seal = no
        .blockers
        .iter()
        .find(|blocker| blocker.loc == 1530 && (blocker.x, blocker.z) == (3076, 3427))
        .expect("la porte 1530 de la boutique scelle la poche");
    assert_eq!((seal.x, seal.z, seal.level), (3076, 3427, 0), "OÙ");
    assert_eq!(seal.name, "loc_1530", "QUEL objet");
    assert_eq!(seal.open_op, Some(1), "QUELLE porte : op Open");
    assert!(seal.is_door);
    assert_eq!(
        seal.stand,
        Some(Tile { x: 3076, z: 3426, level: 0 }),
        "QUEL stand : le clic ne passe que depuis l'exterieur"
    );
}

/// Un ajustement nomme son occupant : le comptoir (pas une coordonnee nue).
#[test]
fn adjusted_dest_names_the_counter() {
    let (world, tables) = setup();
    let dest = Tile { x: 3091, z: 3242, level: 0 };
    let plan = plan::plan(
        &world,
        &tables,
        Tile { x: 3092, z: 3243, level: 0 },
        dest,
        &NavOptions::default(),
    )
    .expect("le comptoir ajuste planifie");
    assert!(plan.adjusted_dest.is_some(), "destination sur le comptoir : ajustee");
    let blocker = plan.dest_blocker.as_ref().expect("l'ajustement nomme son occupant");
    assert_eq!((blocker.x, blocker.z, blocker.level), (dest.x, dest.z, dest.level));
    let (loc, _, _) = world.loc_at(dest.x, dest.z, dest.level).expect("un loc occupe le comptoir");
    assert_eq!(blocker.loc, loc);
    assert_eq!(blocker.name, tables.loc_name(loc));
    assert!(!blocker.name.starts_with("loc_"), "le comptoir a un vrai nom de dump, got {}", blocker.name);
}
