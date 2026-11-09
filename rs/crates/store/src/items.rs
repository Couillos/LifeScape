#![forbid(unsafe_code)]
//! Tradeable-item catalogue rows (id, shop price, alchemy value).

use crate::{Error, Store};

/// One row of the `items` table.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub item_id: i64,
    pub name: String,
    pub shop_price: i64,
    pub alch_value: i64,
}

fn map_item(row: &rusqlite::Row<'_>) -> Result<Item, rusqlite::Error> {
    Ok(Item {
        item_id: row.get(0)?,
        name: row.get(1)?,
        shop_price: row.get(2)?,
        alch_value: row.get(3)?,
    })
}

impl Store {
    /// Inserts the item or replaces its catalogue fields on id conflict.
    pub fn upsert_item(&self, item: &Item) -> Result<(), Error> {
        if item.item_id <= 0 {
            return Err(Error::Invalid(format!("bad item_id {}", item.item_id)));
        }
        if item.name.is_empty() {
            return Err(Error::Invalid("item name is empty".to_string()));
        }
        self.connection.execute(
            "INSERT INTO items(item_id, name, shop_price, alch_value) VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(item_id) DO UPDATE SET name=excluded.name, \
             shop_price=excluded.shop_price, alch_value=excluded.alch_value",
            rusqlite::params![item.item_id, item.name, item.shop_price, item.alch_value],
        )?;
        Ok(())
    }

    /// Fetches one item by id, `None` when unknown.
    pub fn get_item(&self, item_id: i64) -> Result<Option<Item>, Error> {
        let mut statement = self.connection.prepare(
            "SELECT item_id, name, shop_price, alch_value FROM items WHERE item_id = ?1",
        )?;
        let mut found_rows = statement.query_map(rusqlite::params![item_id], map_item)?;
        match found_rows.next() {
            None => Ok(None),
            Some(found) => Ok(Some(found?)),
        }
    }

    /// Whole catalogue ordered by id.
    pub fn list_items(&self) -> Result<Vec<Item>, Error> {
        let mut statement = self.connection.prepare(
            "SELECT item_id, name, shop_price, alch_value FROM items ORDER BY item_id",
        )?;
        let mapped = statement.query_map([], map_item)?;
        let mut catalogue = Vec::new();
        for mapped_row in mapped {
            catalogue.push(mapped_row?);
        }
        Ok(catalogue)
    }
}

#[cfg(test)]
mod items_tests {
    use super::*;

    #[test]
    fn upsert_and_get_roundtrip() -> Result<(), Error> {
        let store = Store::open_in_memory()?;
        assert_eq!(store.get_item(995)?, None);
        store.upsert_item(&Item {
            item_id: 995,
            name: "Coins".to_string(),
            shop_price: 0,
            alch_value: 0,
        })?;
        store.upsert_item(&Item {
            item_id: 314,
            name: "Feather".to_string(),
            shop_price: 2,
            alch_value: 1,
        })?;
        assert_eq!(
            store.get_item(314)?,
            Some(Item {
                item_id: 314,
                name: "Feather".to_string(),
                shop_price: 2,
                alch_value: 1,
            })
        );
        assert_eq!(store.list_items()?.len(), 2);
        Ok(())
    }
}
