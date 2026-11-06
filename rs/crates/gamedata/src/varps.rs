//! Player variables (varps): id, name and persistence.

use crate::{Json, flag, num, opt_str, table};

/// One player variable.
#[derive(Debug, Clone)]
pub struct Varp {
    pub varp_id: i64,
    pub name: String,
    pub scope: i64,
    pub transmit: bool,
}

/// Every varp, ordered by id.
pub fn all_varps() -> Vec<Varp> {
    table("varps")
        .iter()
        .filter_map(|row| match row {
            Json::Obj(_) => Some(Varp {
                varp_id: num(row, "id", -1),
                name: opt_str(row, "name").unwrap_or_default(),
                scope: num(row, "scope", 0),
                transmit: flag(row, "transmit", false),
            }),
            _ => None,
        })
        .collect()
}

/// Varp by name (case-insensitive), or None.
pub fn by_name(name: &str) -> Option<Varp> {
    let wanted = name.to_lowercase();
    all_varps().into_iter().find(|entry| entry.name.to_lowercase() == wanted)
}

/// Varp id by name (case-insensitive), or None.
pub fn id_by_name(name: &str) -> Option<i64> {
    by_name(name).map(|entry| entry.varp_id)
}

#[cfg(test)]
mod tests {
    use super::{all_varps, by_name, id_by_name};

    #[test]
    fn varp_vectors() {
        // 359 varps; cookquest is id 29 (scope 1,
        // not transmitted).
        assert_eq!(all_varps().len(), 359);
        let cook = by_name("cookquest").expect("cookquest");
        assert_eq!((cook.varp_id, cook.scope, cook.transmit), (29, 1, false));
        assert_eq!(id_by_name("cookquest"), Some(29));
        assert_eq!(id_by_name("COOKQUEST"), Some(29));
        assert!(by_name("nope").is_none());
    }
}
