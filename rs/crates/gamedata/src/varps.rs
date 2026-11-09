//! Player variables (varps): id, name and persistence.

/// One static player variable (build-time data, mirrored into [`Varp`] per call).
pub(crate) struct StaticVarp {
    pub varp_id: i64,
    pub name: &'static str,
    pub scope: i64,
    pub transmit: bool,
}

mod generated {
    include!(concat!(env!("OUT_DIR"), "/varps_generated.rs"));
}

/// One player variable.
#[derive(Debug, Clone)]
pub struct Varp {
    pub varp_id: i64,
    pub name: String,
    pub scope: i64,
    pub transmit: bool,
}

fn varp_from_static(row: &StaticVarp) -> Varp {
    Varp { varp_id: row.varp_id, name: row.name.to_string(), scope: row.scope, transmit: row.transmit }
}

/// Every varp, ordered by id.
pub fn all_varps() -> Vec<Varp> {
    generated::ALL.iter().map(varp_from_static).collect()
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
