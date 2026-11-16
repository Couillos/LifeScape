//! Underground links (dump `transitions.json`): the ladders, stairs and
//! trapdoors joining an underground area (z ≥ 6400, the map's
//! underground offset) to the surface, for a walk estimate that never
//! crosses the 6400-tile gap in a straight line.

use std::sync::OnceLock;

const RAW: &str = include_str!("../../../../gamedata/data/transitions.json");
/// Z offset of the underground map above its surface.
pub const UNDERGROUND_Z: i64 = 6400;

/// One link: its underground end and its surface end, `(x, z, level)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Link {
    pub below: (i64, i64, i64),
    pub above: (i64, i64, i64),
}

/// True for an underground tile.
pub fn is_underground(z: i64) -> bool {
    z >= UNDERGROUND_Z
}

fn end(value: &lifescape_json::Value) -> Option<(i64, i64, i64)> {
    let parts = value.as_arr()?;
    Some((parts.get(1)?.as_i64()?, parts.get(2)?.as_i64()?, parts.first()?.as_i64()?))
}

/// Every enabled link between an underground area and the surface.
pub fn links() -> &'static [Link] {
    static LINKS: OnceLock<Vec<Link>> = OnceLock::new();
    LINKS.get_or_init(|| {
        let Ok(doc) = lifescape_json::parse(RAW) else { return Vec::new() };
        let mut out = Vec::new();
        for entry in doc.as_arr().cloned().unwrap_or_default() {
            if entry.get("disabled").as_str().is_some() {
                continue;
            }
            let (Some(from), Some(to)) = (end(entry.get("from")), end(entry.get("to"))) else { continue };
            match (is_underground(from.1), is_underground(to.1)) {
                (true, false) => out.push(Link { below: from, above: to }),
                (false, true) => out.push(Link { below: to, above: from }),
                _ => {}
            }
        }
        out
    })
}

#[cfg(test)]
mod tests {
    use super::links;

    /// The Mining Guild ladder joins Falador to the guild's mine.
    #[test]
    fn mining_guild_ladder_is_a_link() {
        assert!(links().iter().any(|link| link.above.0 == 3020 && link.above.1 == 3339 && link.below.1 == 9739));
        assert!(links().iter().all(|link| link.below.1 >= 6400 && link.above.1 < 6400));
    }
}
