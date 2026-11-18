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

    /// Lowest item id whose name is exactly `name` (same pick as the
    /// first match of `list_items`), `None` when unknown. One indexed-size
    /// scan, never the whole catalogue materialised (CPU 2026-11-14).
    pub fn item_id_by_name(&self, name: &str) -> Result<Option<i64>, Error> {
        let mut statement = self.connection.prepare_cached(
            "SELECT item_id FROM items WHERE name = ?1 ORDER BY item_id LIMIT 1",
        )?;
        let mut found_rows = statement.query(rusqlite::params![name])?;
        match found_rows.next()? {
            Some(row) => Ok(Some(row.get(0)?)),
            None => Ok(None),
        }
    }

    /// Quotation lot of one item (plan prix lot D): its prices are coins
    /// per `lot` units; 1 when unknown.
    pub fn item_lot(&self, item_id: i64) -> Result<i64, Error> {
        let mut statement = self.connection.prepare_cached("SELECT lot FROM items WHERE item_id = ?1")?;
        let mut rows = statement.query(rusqlite::params![item_id])?;
        Ok(match rows.next()? {
            Some(row) => row.get::<_, i64>(0)?.max(1),
            None => 1,
        })
    }

    /// Every item's quotation lot over 1 (`item_id`, `lot`).
    pub fn item_lots(&self) -> Result<Vec<(i64, i64)>, Error> {
        let mut statement = self.connection.prepare("SELECT item_id, lot FROM items WHERE lot > 1 ORDER BY item_id")?;
        let mapped = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        let mut out = Vec::new();
        for row in mapped {
            out.push(row?);
        }
        Ok(out)
    }

    /// Sets an item's quotation lot (plan prix lot D). A changed lot is a
    /// migration of the item's own history: every stored price of it
    /// (orders, deals, executions, candles) is rescaled from the old lot
    /// to the new one in the same savepoint, so a book never mixes units.
    /// Returns true when the lot changed.
    pub fn set_item_lot(&self, item_id: i64, lot: i64) -> Result<bool, Error> {
        let lot = lot.max(1);
        let old = self.item_lot(item_id)?;
        if old == lot {
            return Ok(false);
        }
        let scale = |column: &str| format!("{column} = ({column} * {lot} + {half}) / {old}", half = old / 2);
        self.connection.execute_batch("SAVEPOINT relot")?;
        let statements = [
            format!("UPDATE orders SET {} WHERE item_id = {item_id}", scale("price")),
            format!("UPDATE deals SET {} WHERE item_id = {item_id}", scale("price")),
            format!("UPDATE executions SET {} WHERE item_id = {item_id}", scale("price")),
            format!(
                "UPDATE candles SET {}, {}, {}, {} WHERE item_id = {item_id}",
                scale("open_price"),
                scale("high_price"),
                scale("low_price"),
                scale("close_price")
            ),
            format!("UPDATE items SET lot = {lot} WHERE item_id = {item_id}"),
            // A resting order that is no whole number of lots leaves the
            // book (its owner lists again in lots).
            format!("UPDATE orders SET status = 'cancelled', cancel_reason = 'lot' WHERE item_id = {item_id} AND status = 'open' AND remaining % {lot} != 0"),
        ];
        for statement in &statements {
            if let Err(error) = self.connection.execute_batch(statement) {
                let _ = self.connection.execute_batch("ROLLBACK TO relot; RELEASE relot");
                return Err(error.into());
            }
        }
        self.connection.execute_batch("RELEASE relot")?;
        Ok(true)
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
    fn item_id_by_name_takes_the_first_exact_match() -> Result<(), Error> {
        let store = Store::open_in_memory()?;
        for (item_id, name) in [(7, "Logs"), (3, "Logs"), (9, "logs")] {
            store.upsert_item(&Item { item_id, name: name.to_string(), shop_price: 0, alch_value: 0 })?;
        }
        assert_eq!(store.item_id_by_name("Logs")?, Some(3));
        assert_eq!(store.item_id_by_name("logs")?, Some(9), "case-sensitive like the catalogue scan");
        assert_eq!(store.item_id_by_name("Oak logs")?, None);
        Ok(())
    }

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

#[cfg(test)]
mod lot_tests {
    use crate::{Error, Item, Store};

    /// Plan prix lot D: air runes quoted per 100 rescale their stored
    /// prices (×100) once; setting the same lot again changes nothing;
    /// other items stay.
    #[test]
    fn a_new_lot_rescales_the_items_prices_once() -> Result<(), Error> {
        let store = Store::open_in_memory()?;
        store.upsert_item(&Item { item_id: 556, name: "Air rune".to_string(), shop_price: 4, alch_value: 1 })?;
        store.upsert_item(&Item { item_id: 2353, name: "Steel bar".to_string(), shop_price: 100, alch_value: 60 })?;
        store.connection.execute_batch(
            "INSERT INTO orders(account, side, item_id, quantity, remaining, price, status, dormant, created_clock, updated_clock) VALUES ('a', 'sell', 556, 300, 300, 2, 'open', 0, 1, 1), ('b', 'sell', 2353, 5, 5, 99, 'open', 0, 1, 1);
             INSERT INTO executions(item_id, quantity, price, clock, server_tick) VALUES (556, 100, 1, 5, 5);",
        )?;
        assert_eq!(store.item_lot(556)?, 1);
        assert!(store.set_item_lot(556, 100)?);
        assert!(!store.set_item_lot(556, 100)?, "idempotent");
        let price = |item: i64| -> Result<i64, Error> { Ok(store.connection.query_row("SELECT price FROM orders WHERE item_id = ?1", [item], |row| row.get(0))?) };
        assert_eq!(price(556)?, 200, "2 per unit = 200 per 100");
        assert_eq!(price(2353)?, 99, "a unit-quoted item stays");
        let print: i64 = store.connection.query_row("SELECT price FROM executions WHERE item_id = 556", [], |row| row.get(0))?;
        assert_eq!(print, 100);
        assert_eq!(store.item_lots()?, vec![(556, 100)]);
        assert!(store.set_item_lot(556, 10)?, "100 -> 10");
        assert_eq!(price(556)?, 20);
        Ok(())
    }
}
