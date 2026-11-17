//! Coins a planned route takes from the pack (user rule 2026-11-17: a
//! bot carries travel coins only when its trip will really cost money,
//! and only that amount). The route decides, never a flat reserve:
//! - a toll door segment (`DoorRef::toll`: the Al Kharid border gate,
//!   `border_gate.rs2` refuses under 10 coins in the pack);
//! - a custom fare edge (`CustomEdgeRef::fare`: the Port Sarim sailors
//!   and the Karamja customs officer, 30 coins each, `sailors.rs2`,
//!   `customs_officer.rs2`; gangplanks are free).
//! Walks, ordinary doors and rides cost nothing. Pure: no network, no
//! live state.

use crate::plan::{Plan, Segment};

/// Coins the Al Kharid toll gate wants in the pack (`border_gate.rs2`:
/// `inv_total(inv, coins) < 10` keeps the gate shut).
pub const TOLL_FARE: u32 = 10;

/// Coins one planned route takes from the pack: every toll door and
/// every fare edge it crosses, 0 for a route without any.
pub fn route_fare(plan: &Plan) -> u32 {
    plan.segments
        .iter()
        .map(|segment| match segment {
            Segment::OpenDoor { door, .. } if door.toll => TOLL_FARE,
            Segment::Custom { edge, .. } => edge.fare,
            _ => 0,
        })
        .fold(0u32, u32::saturating_add)
}

/// Coins a trip out and back takes: the out route's fares plus the way
/// back's (a one-way trip passes `None`).
pub fn trip_fare(out: &Plan, back: Option<&Plan>) -> u32 {
    route_fare(out).saturating_add(back.map_or(0, route_fare))
}

#[cfg(test)]
mod tests {
    use super::{route_fare, trip_fare, TOLL_FARE};
    use crate::custom_edge::CustomEdgeRef;
    use crate::plan::{DoorRef, Plan, Segment, Tile};

    fn tile(x: i32, z: i32) -> Tile {
        Tile { x, z, level: 0 }
    }

    fn plan(segments: Vec<Segment>) -> Plan {
        Plan { segments, cost: 0, adjusted_dest: None, dest_blocker: None }
    }

    fn walk() -> Segment {
        Segment::Walk { tiles: vec![tile(3222, 3218), tile(3223, 3218)] }
    }

    fn door(loc: u16, toll: bool) -> Segment {
        Segment::OpenDoor {
            door: DoorRef { loc, x: 3268, z: 3227, level: 0, open_op: Some(1), toll },
            stand: tile(3267, 3227),
            cross_to: tile(3268, 3227),
        }
    }

    fn custom(name: &str, fare: u32) -> Segment {
        let edge = CustomEdgeRef { name: name.to_string(), kind: "npc_fare".to_string(), fare, ..CustomEdgeRef::default() };
        Segment::Custom { edge, stand: tile(3027, 3218), expect: tile(2956, 3143) }
    }

    /// No toll door, no fare edge: the trip carries nothing.
    #[test]
    fn a_route_without_paid_transitions_costs_nothing() {
        let free = plan(vec![walk(), door(1530, false), walk()]);
        assert_eq!(route_fare(&free), 0);
        assert_eq!(trip_fare(&free, Some(&free)), 0);
        assert_eq!(route_fare(&plan(Vec::new())), 0, "the bot's own tile");
    }

    /// Lumbridge to Al Kharid and back: the gate both ways, 2 x 10.
    #[test]
    fn al_kharid_out_and_back_pays_the_gate_twice() {
        let out = plan(vec![walk(), door(2882, true), walk()]);
        let back = plan(vec![walk(), door(2883, true), walk()]);
        assert_eq!(route_fare(&out), TOLL_FARE);
        assert_eq!(trip_fare(&out, Some(&back)), 2 * TOLL_FARE);
        assert_eq!(trip_fare(&out, None), TOLL_FARE, "one way: one toll");
    }

    /// The Karamja boat: 30 out (sailor), 30 back (customs officer), the
    /// gangplanks free.
    #[test]
    fn the_karamja_boat_costs_its_fare_each_way() {
        let out = plan(vec![walk(), custom("boat_port_sarim_to_karamja", 30), custom("gangplank_karamja_dock", 0), walk()]);
        let back = plan(vec![walk(), custom("boat_karamja_to_port_sarim", 30), custom("gangplank_port_sarim_dock", 0), walk()]);
        assert_eq!(route_fare(&out), 30);
        assert_eq!(trip_fare(&out, Some(&back)), 60);
    }

    /// Several paid transitions on one trip: Al Kharid to Karamja crosses
    /// the gate and the boat, both ways.
    #[test]
    fn several_paid_transitions_add_up() {
        let out = plan(vec![walk(), door(2882, true), walk(), custom("boat_port_sarim_to_karamja", 30), walk()]);
        let back = plan(vec![custom("boat_karamja_to_port_sarim", 30), walk(), door(2883, true), walk()]);
        assert_eq!(route_fare(&out), 40);
        assert_eq!(trip_fare(&out, Some(&back)), 80);
    }
}
