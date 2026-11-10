//! Production-chain extension of the task contract v2 (PRD 2, D58).
//!
//! The brain reasons in whole chains (ore → bar → plate, fish → cooked,
//! wool → ball → strung amulet) and compares chains, never isolated links
//! (§4.3). `ChainSpec` is that chain on top of a `TaskSpec`: the ordered
//! links, the total tick cost (harvest, transforms, trips, bank), the XP
//! per skill crossed, and the end value (market or shop). It rides as an
//! optional field, so every existing CLI and report parses exactly as
//! before — `None` means a single-link order (contract v2 unchanged).

#![forbid(unsafe_code)]

use lifescape_json::{object, str_value, Value};

use super::cycle::TaskSpec;

/// One chain link: one activity script on one exact resource, with its
/// estimated tick cost (work plus its own trips, never the whole chain).
#[derive(Debug, Clone, PartialEq)]
pub struct ChainLink {
    pub activity: String,
    pub resource: String,
    pub ticks: u64,
}

/// A whole production chain (D58): links in order, total ticks, XP gained
/// in each skill crossed, end value in coins (market or shop floor).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChainSpec {
    pub links: Vec<ChainLink>,
    pub total_ticks: u64,
    pub xp_by_skill: Vec<(String, i64)>,
    pub end_value: i64,
}

impl ChainSpec {
    pub fn to_value(&self) -> Value {
        object(vec![
            (
                "links",
                Value::Arr(
                    self.links
                        .iter()
                        .map(|link| {
                            object(vec![
                                ("activity", str_value(&link.activity)),
                                ("resource", str_value(&link.resource)),
                                ("ticks", Value::Int(link.ticks as i64)),
                            ])
                        })
                        .collect(),
                ),
            ),
            ("total_ticks", Value::Int(self.total_ticks as i64)),
            (
                "xp_by_skill",
                Value::Arr(
                    self.xp_by_skill
                        .iter()
                        .map(|(skill, xp)| object(vec![("skill", str_value(skill)), ("xp", Value::Int(*xp))]))
                        .collect(),
                ),
            ),
            ("end_value", Value::Int(self.end_value)),
        ])
    }

    pub fn from_value(value: &Value) -> Option<ChainSpec> {
        let links = value.get("links").as_arr().map(|rows| {
            rows.iter()
                .map(|row| ChainLink {
                    activity: row.get("activity").as_str().unwrap_or("").to_string(),
                    resource: row.get("resource").as_str().unwrap_or("").to_string(),
                    ticks: row.get("ticks").as_i64().unwrap_or(0).max(0) as u64,
                })
                .collect::<Vec<_>>()
        })?;
        let xp_by_skill = value
            .get("xp_by_skill")
            .as_arr()
            .map(|rows| {
                rows.iter()
                    .map(|row| {
                        (
                            row.get("skill").as_str().unwrap_or("").to_string(),
                            row.get("xp").as_i64().unwrap_or(0),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        Some(ChainSpec {
            links,
            total_ticks: value.get("total_ticks").as_i64().unwrap_or(0).max(0) as u64,
            xp_by_skill,
            end_value: value.get("end_value").as_i64().unwrap_or(0),
        })
    }
}

impl TaskSpec {
    /// Attach a production chain to this order; the v2 fields stay as-is.
    pub fn with_chain(mut self, chain: ChainSpec) -> TaskSpec {
        self.chain = Some(chain);
        self
    }

    /// The attached chain, if the brain ordered a multi-link task.
    pub fn chain(&self) -> Option<&ChainSpec> {
        self.chain.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::{ChainLink, ChainSpec};
    use crate::kit::cycle::TaskSpec;

    fn iron_chain() -> ChainSpec {
        ChainSpec {
            links: vec![
                ChainLink { activity: "mining".to_string(), resource: "iron".to_string(), ticks: 4000 },
                ChainLink { activity: "smelt".to_string(), resource: "iron_bar".to_string(), ticks: 1500 },
            ],
            total_ticks: 6500,
            xp_by_skill: vec![("mining".to_string(), 1400), ("smithing".to_string(), 900)],
            end_value: 3200,
        }
    }

    /// Contract v2 is untouched: a parsed spec carries no chain.
    #[test]
    fn bare_spec_carries_no_chain() {
        let spec = TaskSpec::named("woodcut");
        assert!(spec.chain().is_none(), "no chain unless the brain attaches one");
    }

    /// The chain rides along without touching the v2 fields.
    #[test]
    fn chain_attaches_beside_v2_fields() {
        let spec = TaskSpec::named("mining").with_chain(iron_chain());
        let chain = spec.chain().expect("attached chain reads back");
        assert_eq!(chain.links.len(), 2);
        assert_eq!(chain.total_ticks, 6500);
        assert_eq!(chain.end_value, 3200);
        assert_eq!(spec.activity, "mining", "v2 activity untouched");
    }

    /// Chain JSON round-trips: links, ticks, XP per skill, end value.
    #[test]
    fn chain_json_round_trips() {
        let back = ChainSpec::from_value(&iron_chain().to_value()).expect("chain parses back");
        assert_eq!(back, iron_chain());
    }
}
