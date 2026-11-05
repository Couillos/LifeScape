//! Shops: stock, restock rates, and keeper NPCs. Mirrors `gamedata/shops.py`.

use crate::{Json, num, opt_num, opt_str, table_value};

/// One stocked item.
#[derive(Debug, Clone)]
pub struct Stock {
    pub obj: String,
    pub count: i64,
    pub rate: i64,
}

/// One shop keeper (NPC id plus buy/sell multipliers).
#[derive(Debug, Clone)]
pub struct Keeper {
    pub npc: String,
    pub npc_id: i64,
    pub shop: String,
    pub sell_multiplier: Option<i64>,
    pub buy_multiplier: Option<i64>,
    pub title: Option<String>,
}

/// One shop with its stocks and keepers.
#[derive(Debug, Clone)]
pub struct Shop {
    pub name: String,
    pub scope: Option<String>,
    pub size: i64,
    pub restock: bool,
    pub stocks: Vec<Stock>,
    pub keepers: Vec<Keeper>,
    pub members: Option<bool>,
}

fn keeper_from_row(row: &Json) -> Keeper {
    Keeper {
        npc: opt_str(row, "npc").unwrap_or_default(),
        npc_id: num(row, "npc_id", -1),
        shop: opt_str(row, "shop").unwrap_or_default(),
        sell_multiplier: opt_num(row, "sell_multiplier"),
        buy_multiplier: opt_num(row, "buy_multiplier"),
        title: opt_str(row, "title"),
    }
}

fn int_or_string(row: &Json, key: &str) -> i64 {
    match row.get(key) {
        Some(Json::Int(value)) => *value,
        Some(Json::Float(value)) => *value as i64,
        Some(Json::Str(text)) => text.parse::<i64>().unwrap_or(0),
        _ => 0,
    }
}

fn shop_rows() -> (Vec<Json>, Vec<Json>) {
    match table_value("shops") {
        Some(Json::Obj(fields)) => {
            let shops = fields
                .iter()
                .find(|(name, _)| name == "shops")
                .and_then(|(_, value)| match value {
                    Json::Arr(rows) => Some(rows.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            let keepers = fields
                .iter()
                .find(|(name, _)| name == "keepers")
                .and_then(|(_, value)| match value {
                    Json::Arr(rows) => Some(rows.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            (shops, keepers)
        }
        _ => (Vec::new(), Vec::new()),
    }
}

/// Every shop, each with its keepers attached.
pub fn all_shops() -> Vec<Shop> {
    let (shops, keepers) = shop_rows();
    shops
        .iter()
        .map(|row| {
            let name = opt_str(row, "name").unwrap_or_default();
            let stocks = match row.get("stocks") {
                Some(Json::Arr(items)) => items
                    .iter()
                    .map(|item| Stock {
                        obj: opt_str(item, "obj").unwrap_or_default(),
                        count: num(item, "count", 1),
                        rate: num(item, "rate", 0),
                    })
                    .collect(),
                _ => Vec::new(),
            };
            let mine: Vec<Keeper> = keepers
                .iter()
                .filter(|keeper| keeper.get("shop").and_then(Json::as_str) == Some(name.as_str()))
                .map(keeper_from_row)
                .collect();
            Shop {
                name,
                scope: opt_str(row, "scope"),
                size: int_or_string(row, "size"),
                restock: row.get("restock").and_then(Json::as_str).is_some_and(|text| {
                    matches!(text.to_lowercase().as_str(), "yes" | "true" | "1")
                }),
                stocks,
                keepers: mine,
                // Python never fills this in either: no server flag.
                members: None,
            }
        })
        .collect()
}

/// Shops with this exact (case-insensitive) name.
pub fn by_name(name: &str) -> Vec<Shop> {
    let wanted = name.to_lowercase();
    all_shops().into_iter().filter(|entry| entry.name.to_lowercase() == wanted).collect()
}

/// Raw keeper rows for one NPC debugname (case-insensitive).
pub fn keeper_for_npc(npc_name: &str) -> Vec<Keeper> {
    let wanted = npc_name.to_lowercase();
    let (_, keepers) = shop_rows();
    keepers
        .iter()
        .filter(|keeper| keeper.get("npc").and_then(Json::as_str).is_some_and(|npc| npc.to_lowercase() == wanted))
        .map(keeper_from_row)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{all_shops, by_name, keeper_for_npc};

    #[test]
    fn shop_vectors() {
        // Python: 203 shops; scimitarshop is shared/size 40/restocking,
        // first stock is 5x bronze_scimitar, keeper is Zeke (541).
        assert_eq!(all_shops().len(), 203);
        let scim = by_name("scimitarshop");
        assert_eq!(scim.len(), 1);
        assert_eq!(scim[0].scope.as_deref(), Some("shared"));
        assert_eq!(scim[0].size, 40);
        assert!(scim[0].restock);
        assert_eq!((scim[0].stocks[0].obj.as_str(), scim[0].stocks[0].count), ("bronze_scimitar", 5));
        assert_eq!(scim[0].keepers[0].npc_id, 541);
        assert_eq!(scim[0].keepers[0].sell_multiplier, Some(1000));
        let zeke = keeper_for_npc("zeke");
        assert_eq!(zeke.len(), 1);
        assert_eq!(zeke[0].shop, "scimitarshop");
        assert!(by_name("no such shop").is_empty());
    }
}
