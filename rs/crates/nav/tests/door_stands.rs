//! Stands multi-cotes d'une porte mur droit (dossier porte 1530) :
//! `door_stands` rend [tuile porte, exterieur, lateraux avec bord ouvert].
//! Pour 1530 en (3076,3427,0) (mur droit angle SUD) : la tuile, le sud
//! exterieur (3076,3426), l'est (3077,3427, bord ouvert prouve live) ;
//! l'ouest (3075,3427) est exclu (bord ferme). Le peintre est correct
//! (pas d'override custom_edges : voir le test reach ci-dessous).

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

fn door_1530() -> (Tile, u8, u8) {
    (Tile { x: 3076, z: 3427, level: 0 }, 0, 3)
}

/// La liste ordonnee : tuile, exterieur sud, est ouvert ; l'ouest
/// bloque (bord ferme) est exclu.
#[test]
fn door_stands_1530_are_tile_exterior_and_open_east() {
    let (world, _) = setup();
    let (at, shape, angle) = door_1530();
    assert_eq!(
        plan::door_stands(&world, shape, angle, at),
        vec![
            Tile { x: 3076, z: 3427, level: 0 },
            Tile { x: 3076, z: 3426, level: 0 },
            Tile { x: 3077, z: 3427, level: 0 },
        ],
        "tuile, exterieur sud, est ouvert ; ouest exclu",
    );
}

/// Le peintre est correct sur le stand est : le bord EST est ouvert en
/// vue fermee et la regle serveur passe. Aucun override custom_edges :
/// la condition "si et seulement si le peintre ne peut pas etre corrige"
/// est fausse, le peintre n'a rien a corriger.
#[test]
fn east_stand_passes_the_server_reach_rule() {
    let (world, _) = setup();
    let flags = world.get(3077, 3427, 0);
    assert!(lifescape_nav::collision::tile_walkable(flags), "le stand est marchable, flags={flags:#x}");
    assert_eq!(flags & lifescape_nav::collision::BLOCK_EAST, 0, "bord EST ouvert en vue fermee, flags={flags:#x}");
    assert!(
        lifescape_nav::reach::reached(
            &|_, _, _| flags,
            0,
            3077,
            3427,
            3076,
            3427,
            1,
            1,
            3,
            0,
            0,
        ),
        "reachWall1 angle SUD accepte l'est, flags={flags:#x}",
    );
}

/// Non-regression peintre : la route vers le stand est depuis
/// l'interieur marche (sans elle, un futur flag fantome rescellerait la
/// sortie sans qu'aucun or ne le voie).
#[test]
fn route_to_east_stand_from_inside_walks() {
    let (world, tables) = setup();
    let plan = plan::plan(
        &world,
        &tables,
        Tile { x: 3076, z: 3428, level: 0 },
        Tile { x: 3077, z: 3427, level: 0 },
        &NavOptions::default(),
    )
    .expect("le stand est reste atteignable depuis l'interieur");
    assert!(
        plan.segments.iter().all(|segment| !matches!(segment, Segment::OpenDoor { .. })),
        "pure marche, aucune ouverture : {plan:?}",
    );
}

/// La sortie clique depuis l'est, jamais depuis la tuile : le handler
/// op n'a pas le bras src==dest du dispatcher (clic tuile refuse live),
/// seul le stand est ouvre (silencieux, scripts doors.rs2 commentes).
#[test]
fn peksa_exit_clicks_from_the_east_stand() {
    let (world, tables) = setup();
    let plan = plan::plan(
        &world,
        &tables,
        Tile { x: 3076, z: 3428, level: 0 },
        Tile { x: 3076, z: 3425, level: 0 },
        &NavOptions::default(),
    )
    .expect("la sortie Peksa planifie par le stand est");
    let (door, stand, cross_to) = plan
        .segments
        .iter()
        .find_map(|segment| match segment {
            Segment::OpenDoor { door, stand, cross_to } if door.loc == 1530 && (door.x, door.z) == (3076, 3427) => {
                Some((door, stand, cross_to))
            }
            _ => None,
        })
        .expect("la sortie ouvre la porte 1530 de la boutique");
    assert_eq!((door.x, door.z, door.level), (3076, 3427, 0));
    assert_eq!(
        (*stand, *cross_to),
        (Tile { x: 3077, z: 3427, level: 0 }, Tile { x: 3076, z: 3426, level: 0 }),
        "clic est puis traversee vers le sud, jamais src==dest",
    );
}
