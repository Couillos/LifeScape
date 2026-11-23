#![forbid(unsafe_code)]
//! Supply indices of the item holdings view (user 2026-11-18): the fleet
//! counts of each `lifescape_gamedata::supply_indices` family summed
//! into one number (live, its 24 h change) and one curve (hourly and
//! daily points of `holdings_hourly`/`holdings_daily` summed over the
//! family's object ids, banknotes included). Same accounts as the item
//! list: staff, private and Ironman holdings are already out of `live`.

use std::collections::{BTreeMap, HashMap};

use lifescape_gamedata::supply_indices::{self, SupplyIndex};
use lifescape_json::{object, str_value, Value};

/// Object ids (base objects and their notes) counted by an index.
pub fn index_ids(index: SupplyIndex) -> Vec<i64> {
    supply_indices::obj_ids(index).into_iter().map(|obj_id| obj_id as i64).collect()
}

fn summed(counts: &dyn Fn(i64) -> i64, ids: &[i64]) -> i64 {
    ids.iter().map(|obj_id| counts(*obj_id)).sum()
}

/// `[{key,label,count,change24h,ids}]` in display order: `count` is the
/// live total of the index, `change24h` its change against the hourly
/// point 24 h ago in thousandths (`null` without a baseline or from
/// zero), `ids` the object ids the front filters the item list with.
pub(crate) fn indices_value(live: &BTreeMap<i64, i64>, baseline: Option<&HashMap<i64, i64>>) -> Value {
    let rows = SupplyIndex::ALL
        .iter()
        .map(|index| {
            let ids = index_ids(*index);
            let count = summed(&|obj_id| live.get(&obj_id).copied().unwrap_or(0), &ids);
            let before = baseline.map(|counts| summed(&|obj_id| counts.get(&obj_id).copied().unwrap_or(0), &ids));
            let change = match crate::holdings::count_change_thousandths(count, before) {
                Some(change) => Value::Int(change),
                None => Value::Null,
            };
            object(vec![
                ("key", str_value(index.key())),
                ("label", str_value(index.label())),
                ("count", Value::Int(count)),
                ("change24h", change),
                ("ids", Value::Arr(ids.iter().map(|obj_id| Value::Int(*obj_id)).collect())),
            ])
        })
        .collect();
    Value::Arr(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(name: &str) -> i64 {
        lifescape_gamedata::objs_by_name(name)[0].obj_id as i64
    }

    fn row<'a>(rows: &'a Value, key: &str) -> &'a Value {
        rows.as_arr().unwrap().iter().find(|row| row.get("key").as_str() == Some(key)).expect("index row")
    }

    /// Raw shrimps and their notes sum into cooking, cooked shrimps into
    /// food; the 24 h change reads the same baseline as the item list.
    #[test]
    fn indices_sum_live_counts_and_notes() {
        let mut live = BTreeMap::new();
        live.insert(obj("raw_shrimp"), 10);
        live.insert(obj("cert_raw_shrimp"), 30);
        live.insert(obj("shrimp"), 5);
        live.insert(obj("coins"), 1_000);
        let mut before = HashMap::new();
        before.insert(obj("raw_shrimp"), 20);
        let rows = indices_value(&live, Some(&before));
        assert_eq!(rows.as_arr().map(Vec::len), Some(11));
        let cooking = row(&rows, "cooking");
        assert_eq!(cooking.get("label").as_str(), Some("Cooking"));
        assert_eq!(cooking.get("count").as_i64(), Some(40));
        assert_eq!(cooking.get("change24h").as_i64(), Some(1_000), "20 -> 40 is +100 %");
        assert!(cooking.get("ids").as_arr().unwrap().iter().any(|id| id.as_i64() == Some(obj("cert_raw_shrimp"))));
        let food = row(&rows, "food");
        assert_eq!(food.get("count").as_i64(), Some(5));
        assert!(food.get("change24h").is_null(), "from zero: no percentage");
        assert_eq!(row(&rows, "tools").get("count").as_i64(), Some(0));
        let unknown = indices_value(&live, None);
        assert!(row(&unknown, "cooking").get("change24h").is_null(), "no baseline yet");
    }
}
