//! Client stands of the F2P bank booths (`gamedata/data/bank_stands.json`).
//!
//! Each booth sits in a counter: of the stands the server accepts the
//! booth op from, some are on the bankers' side, a sealed pocket no walk
//! reaches. Aiming one cost an exhaustive NoRoute search on the plan
//! service (3-11 s under load, ~1000 failure dossiers a day). The table
//! keeps, per booth, only the stands planned in from outside the bank;
//! `rs/crates/nav/tests/bank_stands.rs` recomputes it from the map.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::OnceLock;

const RAW: &str = include_str!("../../../../gamedata/data/bank_stands.json");

type BoothKey = (i64, i64, i64);

fn pairs(value: &lifescape_json::Value) -> Vec<(i64, i64)> {
    value
        .as_arr()
        .map(|rows| {
            rows.iter()
                .filter_map(|pair| {
                    let coords = pair.as_arr()?;
                    Some((coords.first()?.as_i64()?, coords.get(1)?.as_i64()?))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Client stands per booth (x, z, level). Empty when the dump does not
/// parse: `tests::every_f2p_booth_is_tabled` guards the shipped file.
fn table() -> &'static HashMap<BoothKey, Vec<(i64, i64)>> {
    static TABLE: OnceLock<HashMap<BoothKey, Vec<(i64, i64)>>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut out = HashMap::new();
        let Ok(doc) = lifescape_json::parse(RAW) else { return out };
        for bank in doc.get("banks").as_arr().map(Vec::as_slice).unwrap_or(&[]) {
            for booth in bank.get("booths").as_arr().map(Vec::as_slice).unwrap_or(&[]) {
                let (Some(x), Some(z), Some(level)) =
                    (booth.get("x").as_i64(), booth.get("z").as_i64(), booth.get("level").as_i64())
                else {
                    continue;
                };
                out.insert((x, z, level), pairs(booth.get("client")));
            }
        }
        out
    })
}

/// Stands on the client side of the booth at (x, z, level), or None when
/// the booth is not tabled (members banks, unknown spawn).
pub fn client_stands(x: i64, z: i64, level: i64) -> Option<&'static [(i64, i64)]> {
    table().get(&(x, z, level)).map(Vec::as_slice)
}

#[cfg(test)]
mod tests {
    use super::client_stands;
    use crate::banks::all_banks;

    #[test]
    fn every_f2p_booth_is_tabled() {
        let free: Vec<_> = all_banks().into_iter().filter(|bank| !bank.members).collect();
        assert_eq!(free.len(), 32, "F2P bankbooth spawns in rev 274");
        for bank in free {
            let stands = client_stands(bank.x, bank.z, bank.level).unwrap_or_else(|| panic!("booth ({},{}) untabled", bank.x, bank.z));
            assert!(!stands.is_empty(), "booth ({},{}) has no client stand", bank.x, bank.z);
        }
        assert!(client_stands(2721, 3494, 0).is_none(), "members booth (Catherby side) is not tabled");
    }

    /// The bankers' side never appears: Varrock west east stand (3187,3436)
    /// (859 NoRoute dossiers), Varrock east south (3252,3418), Falador
    /// west south (2945,3366) (2038 dossiers).
    #[test]
    fn teller_side_stands_are_never_listed() {
        assert_eq!(client_stands(3186, 3436, 0), Some(&[(3185, 3436)][..]));
        assert_eq!(client_stands(3252, 3419, 0), Some(&[(3252, 3420)][..]));
        assert_eq!(client_stands(2945, 3367, 0), Some(&[(2945, 3368)][..]));
        assert_eq!(client_stands(3091, 3245, 0), Some(&[(3092, 3245)][..]));
    }
}
