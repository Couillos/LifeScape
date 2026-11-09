//! Shops: stock, restock rates, and keeper NPCs.

/// One static stock row (build-time data).
pub(crate) struct StaticStock {
    pub obj: &'static str,
    pub count: i64,
    pub rate: i64,
}

/// One static shop row (build-time data, keepers joined per call).
pub(crate) struct StaticShop {
    pub name: &'static str,
    pub scope: Option<&'static str>,
    pub size: i64,
    pub restock: bool,
    pub stocks: &'static [StaticStock],
    pub members: Option<bool>,
}

/// One static keeper row (build-time data).
pub(crate) struct StaticKeeper {
    pub npc: &'static str,
    pub npc_id: i64,
    pub shop: &'static str,
    pub sell: Option<i64>,
    pub buy: Option<i64>,
    pub title: Option<&'static str>,
}

mod generated {
    include!(concat!(env!("OUT_DIR"), "/shops_generated.rs"));
}

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

fn keeper_from_static(row: &StaticKeeper) -> Keeper {
    Keeper {
        npc: row.npc.to_string(),
        npc_id: row.npc_id,
        shop: row.shop.to_string(),
        sell_multiplier: row.sell,
        buy_multiplier: row.buy,
        title: row.title.map(str::to_string),
    }
}

/// Every shop, each with its keepers attached.
pub fn all_shops() -> Vec<Shop> {
    generated::SHOPS
        .iter()
        .map(|row| {
            let mine: Vec<Keeper> =
                generated::KEEPERS.iter().filter(|keeper| keeper.shop == row.name).map(keeper_from_static).collect();
            Shop {
                name: row.name.to_string(),
                scope: row.scope.map(str::to_string),
                size: row.size,
                restock: row.restock,
                stocks: row
                    .stocks
                    .iter()
                    .map(|item| Stock { obj: item.obj.to_string(), count: item.count, rate: item.rate })
                    .collect(),
                keepers: mine,
                // Server-derived flag from the dump (keeper NPCs): null
                // means no keeper info (stalls, interface inventories).
                members: row.members,
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
    generated::KEEPERS
        .iter()
        .filter(|keeper| keeper.npc.to_lowercase() == wanted)
        .map(keeper_from_static)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{all_shops, by_name, keeper_for_npc};

    #[test]
    fn shop_vectors() {
        // 203 shops; scimitarshop is shared/size 40/restocking,
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
        assert_eq!(scim[0].members, Some(false));
        let food = by_name("foodshop");
        assert_eq!(food.len(), 1);
        assert_eq!(food[0].members, None, "stalls carry no keeper flag");
        assert!(by_name("no such shop").is_empty());
    }
}
