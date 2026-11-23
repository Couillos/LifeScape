#![forbid(unsafe_code)]
//! Market catalogue: every F2P tradeable object from the dumps. The demo
//! seeding that used to live here (synthetic `demo-merchant` fills and
//! resting depth) is gone (PRD3 lot 0): a bot crossing a demo quote got a
//! deal no live party ever delivered. Prices now come from real trades
//! only, 1 coin before the first print.

/// One catalogue row (F2P tradeable item from gamedata).
#[derive(Clone, Debug)]
pub struct CatalogueItem {
    pub id: i64,
    pub name: String,
    pub shop_price: i64,
    pub alch_value: i64,
}

/// F2P tradeable catalogue straight from the dumps (members and
/// untradeable rows never reach the market).
pub fn catalogue() -> Vec<CatalogueItem> {
    lifescape_gamedata::f2p::f2p_tradeable_objs()
        .iter()
        .map(|obj| CatalogueItem {
            id: obj.obj_id as i64,
            name: obj.display.or(obj.name).unwrap_or("unknown").to_string(),
            shop_price: obj.price.max(0),
            alch_value: obj.alch_high.max(0),
        })
        .collect()
}
