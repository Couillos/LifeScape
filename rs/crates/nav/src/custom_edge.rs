//! Executor view of a hand-added edge (`nav_overrides.json`
//! `custom_edges`): what a plan carries so a worker without tables can
//! ride it, plus its wire codec. Two kinds: "loc" (click a loc op that
//! teleports: ship gangplanks) and "npc_fare" (talk to an NPC, answer
//! its dialogue, pay coins from the pack: the Port Sarim sailors, the
//! Karamja customs officer).

use lifescape_json::{object, str_value, Value};

use crate::plan::Tile;

/// Which hand-added edge a step takes, with everything the executor
/// needs: the loc to click or the NPCs to talk to, the dialogue answers
/// and the fare. Old plans carry the name only (`kind` empty: no live
/// executor, the ride fails typed).
#[derive(Clone, Debug, Default)]
pub struct CustomEdgeRef {
    pub name: String,
    /// "loc" or "npc_fare" (`tables::CustomEdge::kind`).
    pub kind: String,
    pub loc: u16,
    pub op: u8,
    /// Tile of the loc a "loc" edge clicks.
    pub at: Option<Tile>,
    /// NPC debugnames an "npc_fare" edge talks to (any one answers).
    pub npcs: Vec<String>,
    /// Dialogue options to pick, whichever page asks them.
    pub choices: Vec<String>,
    /// Coins the edge takes from the pack.
    pub fare: u32,
}

impl CustomEdgeRef {
    /// The executor's view of one table edge.
    pub fn of(edge: &crate::tables::CustomEdge) -> CustomEdgeRef {
        CustomEdgeRef {
            name: edge.name.clone(),
            kind: edge.kind.clone(),
            loc: edge.loc,
            op: edge.op,
            at: edge.at,
            npcs: edge.npcs.clone(),
            choices: edge.choices.clone(),
            fare: edge.fare,
        }
    }

    /// Wire fields (appended to the segment object beside `name`).
    pub fn fields(&self) -> Vec<(&'static str, Value)> {
        let strings = |items: &[String]| Value::Arr(items.iter().map(|item| str_value(item)).collect());
        vec![
            ("edge_kind", str_value(&self.kind)),
            ("loc", Value::Int(i64::from(self.loc))),
            ("op", Value::Int(i64::from(self.op))),
            (
                "at",
                self.at.map_or(Value::Null, |tile| {
                    object(vec![
                        ("x", Value::Int(i64::from(tile.x))),
                        ("z", Value::Int(i64::from(tile.z))),
                        ("level", Value::Int(i64::from(tile.level))),
                    ])
                }),
            ),
            ("npcs", strings(&self.npcs)),
            ("choices", strings(&self.choices)),
            ("fare", Value::Int(i64::from(self.fare))),
        ]
    }

    /// Parse a segment object; absent fields (old plans) read empty.
    pub fn parse(value: &Value) -> Result<CustomEdgeRef, String> {
        let strings = |key: &str| -> Vec<String> {
            value.get(key).as_arr().map(|items| items.iter().filter_map(|item| item.as_str().map(str::to_string)).collect()).unwrap_or_default()
        };
        let at = match value.get("at") {
            Value::Null => None,
            tile => Some(Tile {
                x: tile.get("x").as_i64().ok_or("custom.at.x")? as i32,
                z: tile.get("z").as_i64().ok_or("custom.at.z")? as i32,
                level: tile.get("level").as_i64().ok_or("custom.at.level")? as i32,
            }),
        };
        Ok(CustomEdgeRef {
            name: value.get("name").as_str().ok_or("custom.name")?.to_string(),
            kind: value.get("edge_kind").as_str().unwrap_or("").to_string(),
            loc: value.get("loc").as_i64().unwrap_or(0) as u16,
            op: value.get("op").as_i64().unwrap_or(1) as u8,
            at,
            npcs: strings("npcs"),
            choices: strings("choices"),
            fare: value.get("fare").as_i64().unwrap_or(0).max(0) as u32,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::CustomEdgeRef;
    use crate::plan::Tile;

    /// The executor fields survive the wire; a name-only (old) plan
    /// parses with an empty kind.
    #[test]
    fn custom_refs_round_trip() {
        let sail = CustomEdgeRef {
            name: "boat".to_string(),
            kind: "npc_fare".to_string(),
            loc: 0,
            op: 1,
            at: None,
            npcs: vec!["captain_tobias".to_string()],
            choices: vec!["Yes please.".to_string()],
            fare: 30,
        };
        let plank = CustomEdgeRef { name: "plank".to_string(), kind: "loc".to_string(), loc: 2082, op: 1, at: Some(Tile { x: 2956, z: 3144, level: 1 }), ..Default::default() };
        for edge in [sail, plank] {
            let mut fields = vec![("name", lifescape_json::str_value(&edge.name))];
            fields.extend(edge.fields());
            let back = CustomEdgeRef::parse(&lifescape_json::object(fields)).expect("parses");
            assert_eq!(format!("{back:?}"), format!("{edge:?}"));
        }
        let old = CustomEdgeRef::parse(&lifescape_json::object(vec![("name", lifescape_json::str_value("legacy"))])).expect("old plan parses");
        assert_eq!((old.kind.as_str(), old.fare), ("", 0));
    }
}
