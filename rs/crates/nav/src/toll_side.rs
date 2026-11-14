#![forbid(unsafe_code)]
//! Côtés du péage d'Al Kharid par connexité (2026-11-14) : remplissage
//! de marche sur la collision statique d'un niveau, portes ordinaires
//! ouvertes, portes à péage (`StaticWorld::is_toll`) fermées, borné à
//! une fenêtre. Deux remplissages (une graine de chaque côté de la
//! porte) donnent les deux côtés ; leur intersection vide prouve que la
//! clôture sépare la fenêtre. Le côté Al Kharid, sérialisé en bandes par
//! rangée, devient `gamedata/data/toll_region.json` (test d'or
//! `tests/toll_region.rs`), lu par `gamedata::toll` : planificateur et
//! scripts testent la même région, jamais une droite x = 3268.
//! Pur : monde statique et tables en entrée, ensembles de cases en sortie.

use std::collections::{HashSet, VecDeque};

use crate::bfs::can_step;
use crate::collision::CollisionMap;
use crate::plan::Tile;
use crate::static_world::StaticWorld;
use crate::tables::Tables;

/// Fenêtre fermée (bornes incluses) d'un remplissage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub min_x: i32,
    pub min_z: i32,
    pub max_x: i32,
    pub max_z: i32,
}

impl Window {
    pub fn contains(&self, tile_x: i32, tile_z: i32) -> bool {
        (self.min_x..=self.max_x).contains(&tile_x) && (self.min_z..=self.max_z).contains(&tile_z)
    }
}

/// Drapeaux d'une case, portes ordinaires du 3x3 ouvertes (même retrait
/// exact de mur que le planificateur), portes à péage et portes bloquées
/// à la main (`Tables::is_blocked`) laissées fermées. Les exigences
/// (clés, quêtes) sont ignorées : un remplissage optimiste qui reste
/// séparé prouve la séparation.
fn open_flags(world: &StaticWorld, tables: &Tables, tile_x: i32, tile_z: i32, level: i32) -> u32 {
    let mut doors = Vec::new();
    for delta_x in -1..=1 {
        for delta_z in -1..=1 {
            let (near_x, near_z) = (tile_x + delta_x, tile_z + delta_z);
            let Some((loc, shape, angle)) = world.door_at(near_x, near_z, level) else { continue };
            if world.is_toll(near_x, near_z, level) || tables.is_blocked(loc, Tile { x: near_x, z: near_z, level }) {
                continue;
            }
            if world.loc_def(loc as usize).is_some_and(|def| def.open_op.is_some()) {
                doors.push((near_x, near_z, loc, shape, angle));
            }
        }
    }
    if doors.is_empty() {
        return world.get(tile_x, tile_z, level);
    }
    let mut scratch = CollisionMap::new();
    for delta_x in -1..=1 {
        for delta_z in -1..=1 {
            scratch.set(tile_x + delta_x, tile_z + delta_z, level, world.get(tile_x + delta_x, tile_z + delta_z, level));
        }
    }
    for (near_x, near_z, loc, shape, angle) in doors {
        world.unpaint_into(&mut scratch, loc, shape, angle, near_x, near_z, level);
    }
    scratch.get(tile_x, tile_z, level)
}

/// Cases atteignables à pied depuis `seed` sur `level` sans sortir de
/// `window` ni franchir une porte à péage (pas du serveur : cardinaux
/// sur la case d'arrivée, diagonales sur les deux intermédiaires aussi).
pub fn flood(world: &StaticWorld, tables: &Tables, seed: (i32, i32), level: i32, window: Window) -> HashSet<(i32, i32)> {
    let mut seen: HashSet<(i32, i32)> = HashSet::new();
    let mut queue = VecDeque::new();
    if !window.contains(seed.0, seed.1) {
        return seen;
    }
    seen.insert(seed);
    queue.push_back(seed);
    let mut flags_cache: std::collections::HashMap<(i32, i32), u32> = std::collections::HashMap::new();
    while let Some((at_x, at_z)) = queue.pop_front() {
        for (step_x, step_z) in [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)] {
            let (next_x, next_z) = (at_x + step_x, at_z + step_z);
            if !window.contains(next_x, next_z) || seen.contains(&(next_x, next_z)) {
                continue;
            }
            let mut read = |tile_x: i32, tile_z: i32| *flags_cache.entry((tile_x, tile_z)).or_insert_with(|| open_flags(world, tables, tile_x, tile_z, level));
            if can_step(&mut read, at_x, at_z, next_x, next_z) {
                seen.insert((next_x, next_z));
                queue.push_back((next_x, next_z));
            }
        }
    }
    seen
}

/// Rayon (Chebyshev) dans lequel une case non marchable (eau d'un banc
/// de pêche, rocher, arbre) prend le côté de ses voisines marchables.
pub const BLOCKED_REACH: i32 = 3;

/// Région « derrière le péage » dérivée de la connexité.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TollRegion {
    /// Fenêtre retenue : son `max_z` est la plus haute rangée où les deux
    /// côtés restent séparés (au-delà, le bout nord de la clôture les
    /// relie à pied, sans porte).
    pub window: Window,
    /// Cases du côté de la graine est (marchables, plus les cases
    /// bloquées dont les voisines marchables les plus proches sont de ce
    /// côté), en bandes par rangée.
    pub rows: Vec<(i32, Vec<(i32, i32)>)>,
}

/// Côté Al Kharid de la porte à péage `gate` (sa case est la graine est,
/// la case à l'ouest la graine ouest) : la fenêtre `base` monte rangée
/// par rangée jusqu'à `base.max_z` tant que les deux remplissages restent
/// disjoints ; la dernière fenêtre disjointe donne la région. Erreur
/// quand la porte ne sépare rien dès sa propre rangée.
pub fn derive_region(world: &StaticWorld, tables: &Tables, gate: (i32, i32, i32), base: Window) -> Result<TollRegion, String> {
    let (gate_x, gate_z, level) = gate;
    let east_seed = (gate_x, gate_z);
    let west_seed = (gate_x - 1, gate_z);
    let separated = |max_z: i32| {
        let window = Window { max_z, ..base };
        let east = flood(world, tables, east_seed, level, window);
        (!east.contains(&west_seed)).then_some(east)
    };
    if separated(gate_z).is_none() {
        return Err(format!("toll gate {gate:?} separates nothing in {base:?}"));
    }
    // Monotone : une fois relié, plus haut reste relié (dichotomie).
    let (mut low, mut high) = (gate_z, base.max_z);
    while low < high {
        let middle = low + (high - low + 1) / 2;
        if separated(middle).is_some() {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    let window = Window { max_z: low, ..base };
    let east = flood(world, tables, east_seed, level, window);
    let west = flood(world, tables, west_seed, level, window);
    let mut side = east.clone();
    for tile_z in window.min_z..=window.max_z {
        for tile_x in window.min_x..=window.max_x {
            if east.contains(&(tile_x, tile_z)) || west.contains(&(tile_x, tile_z)) {
                continue;
            }
            if nearest_side(&east, &west, tile_x, tile_z) == Some(true) {
                side.insert((tile_x, tile_z));
            }
        }
    }
    Ok(TollRegion { window, rows: row_spans(&side) })
}

/// Côté d'une case non marchable : le premier anneau (Chebyshev, jusqu'à
/// `BLOCKED_REACH`) qui touche un remplissage décide à la majorité
/// (`Some(true)` = est) ; égalité ou rien à portée : `None`.
fn nearest_side(east: &HashSet<(i32, i32)>, west: &HashSet<(i32, i32)>, tile_x: i32, tile_z: i32) -> Option<bool> {
    for radius in 1..=BLOCKED_REACH {
        let (mut east_count, mut west_count) = (0, 0);
        for delta_x in -radius..=radius {
            for delta_z in -radius..=radius {
                if delta_x.abs().max(delta_z.abs()) != radius {
                    continue;
                }
                let near = (tile_x + delta_x, tile_z + delta_z);
                east_count += i32::from(east.contains(&near));
                west_count += i32::from(west.contains(&near));
            }
        }
        if east_count + west_count > 0 {
            return match east_count.cmp(&west_count) {
                std::cmp::Ordering::Greater => Some(true),
                std::cmp::Ordering::Less => Some(false),
                std::cmp::Ordering::Equal => None,
            };
        }
    }
    None
}

/// Forme fichier (`gamedata/data/toll_region.json`) : porte, fenêtre
/// retenue, puis une rangée par ligne `[z, x0, x1, x0, x1, …]`.
pub fn region_json(region: &TollRegion, gate: (i32, i32, i32)) -> String {
    let window = region.window;
    let mut text = format!(
        "{{\n  \"gate\": [{}, {}, {}],\n  \"window\": [{}, {}, {}, {}],\n  \"rows\": [\n",
        gate.0, gate.1, gate.2, window.min_x, window.min_z, window.max_x, window.max_z
    );
    let lines: Vec<String> = region
        .rows
        .iter()
        .map(|(tile_z, spans)| {
            let cells: Vec<String> = std::iter::once(tile_z.to_string()).chain(spans.iter().flat_map(|(start, end)| [start.to_string(), end.to_string()])).collect();
            format!("    [{}]", cells.join(", "))
        })
        .collect();
    text.push_str(&lines.join(",\n"));
    text.push_str("\n  ]\n}\n");
    text
}

/// Bandes `[x_debut, x_fin]` (incluses) d'un ensemble de cases, rangée
/// par rangée (z croissant, x croissant) : la forme sérialisée.
pub fn row_spans(tiles: &HashSet<(i32, i32)>) -> Vec<(i32, Vec<(i32, i32)>)> {
    let mut sorted: Vec<(i32, i32)> = tiles.iter().map(|(tile_x, tile_z)| (*tile_z, *tile_x)).collect();
    sorted.sort_unstable();
    let mut rows: Vec<(i32, Vec<(i32, i32)>)> = Vec::new();
    for (tile_z, tile_x) in sorted {
        match rows.last_mut() {
            Some((row_z, spans)) if *row_z == tile_z => match spans.last_mut() {
                Some((_, end)) if *end + 1 == tile_x => *end = tile_x,
                _ => spans.push((tile_x, tile_x)),
            },
            _ => rows.push((tile_z, vec![(tile_x, tile_x)])),
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::{nearest_side, row_spans, Window};

    /// Une case d'eau prend le côté de ses rives les plus proches ; à
    /// égalité ou hors de portée, aucun côté.
    #[test]
    fn blocked_tiles_follow_their_nearest_banks() {
        let east = [(10, 11), (11, 11)].into_iter().collect();
        let west = [(5, 10)].into_iter().collect();
        assert_eq!(nearest_side(&east, &west, 10, 10), Some(true), "the bank just north is east");
        assert_eq!(nearest_side(&east, &west, 6, 10), Some(false), "next to the west bank");
        let tied_east = [(8, 10)].into_iter().collect();
        let tied_west = [(6, 10)].into_iter().collect();
        assert_eq!(nearest_side(&tied_east, &tied_west, 7, 10), None, "one tile from each side: no side");
        assert_eq!(nearest_side(&east, &west, 40, 40), None, "nothing within reach");
    }

    #[test]
    fn spans_merge_runs_per_row() {
        let tiles = [(1, 5), (2, 5), (3, 5), (7, 5), (4, 6)].into_iter().collect();
        assert_eq!(row_spans(&tiles), vec![(5, vec![(1, 3), (7, 7)]), (6, vec![(4, 4)])]);
        let window = Window { min_x: 0, min_z: 0, max_x: 3, max_z: 3 };
        assert!(window.contains(3, 0) && !window.contains(4, 0), "bounds inclusive");
    }
}
