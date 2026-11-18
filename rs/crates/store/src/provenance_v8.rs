//! Migration 8 (user 2026-11-18, Item holdings "Provenance / destination"):
//! every stored report's object provenances are rebuilt from its causes
//! (`task_flows`, the ledger's own attribution) with the current rule
//! (`orchestrator::flow_causes::provenance_of`):
//! - an NPC shop leg or a fee of a merchant run keeps `shop_bought`,
//!   `shop_sold`, `fee` (the janitor's NPC sales read as player trades);
//! - the object an alchemy run cast away reads `alch_high` / `alch_low`
//!   (its confirmed spell, `task_units` cast rows), its runes `consumed`.
//!
//! XP rows (`item_name = ''`) and reports without flows (before the
//! causes were stored) stay as they are. One SQL pass inside the run's
//! transaction: a frozen copy of the rule at this version, never read
//! again after the bump.

#![forbid(unsafe_code)]

use crate::Error;

/// Reports whose object rows are rebuilt: every report with causes.
const WITH_FLOWS: &str = "SELECT DISTINCT report_id FROM task_flows";

/// The rule of `provenance_of` at schema 8, over one flow row `f` of a
/// report `r` (trading = a merchant script or method).
const REBUILT: &str = "
INSERT INTO task_yields(report_id, item_name, provenance, quantity, skill, xp)
SELECT report_id, item_name, provenance, SUM(quantity), '', 0 FROM (
  SELECT f.report_id, f.item_name, f.quantity,
    CASE
      WHEN f.cause = 'input' AND f.quantity < 0 AND EXISTS (SELECT 1 FROM task_units u WHERE u.report_id = f.report_id AND u.unit = 'cast' AND u.subject = 'highlvl_alchemy') THEN 'alch_high'
      WHEN f.cause = 'input' AND f.quantity < 0 AND EXISTS (SELECT 1 FROM task_units u WHERE u.report_id = f.report_id AND u.unit = 'cast' AND u.subject = 'lowlvl_alchemy') THEN 'alch_low'
      WHEN (r.script IN ('market', 'shop') OR r.script LIKE 'market:%' OR r.script LIKE 'shop:%')
        AND f.cause NOT IN ('food', 'shop_bought', 'shop_sold', 'fee') THEN 'traded'
      WHEN f.cause IN ('product', 'byproduct') THEN 'produced'
      WHEN f.cause IN ('input', 'ammo', 'rune', 'bait', 'supply', 'food') THEN 'consumed'
      WHEN f.cause = 'discarded' THEN 'lost'
      WHEN f.cause IN ('loot', 'ground_spawn', 'fee', 'death', 'shop_bought', 'shop_sold', 'traded') THEN f.cause
      WHEN f.quantity > 0 THEN 'unattributed'
      ELSE 'lost'
    END AS provenance
  FROM task_flows f JOIN task_reports r ON r.report_id = f.report_id
)
GROUP BY report_id, item_name, provenance
HAVING SUM(quantity) != 0";

pub(crate) fn apply_provenance_v8(connection: &rusqlite::Connection) -> Result<(), Error> {
    let step = |sql: &str| connection.execute_batch(sql).map_err(|source| Error::Sql(format!("migration 8: {source}")));
    step(&format!("DELETE FROM task_yields WHERE item_name != '' AND report_id IN ({WITH_FLOWS})"))?;
    step(REBUILT)?;
    step("PRAGMA user_version = 8")
}

#[cfg(test)]
mod tests {
    use crate::{temp_database_path, Error, Store};

    fn yields(store: &Store, report: i64) -> Result<Vec<(String, String, i64)>, Error> {
        let mut statement = store.connection.prepare("SELECT item_name, provenance, quantity FROM task_yields WHERE report_id = ?1 AND item_name != '' ORDER BY item_name, provenance")?;
        let rows = statement.query_map([report], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// A janitor's market run sold 28 iron platebodies to an NPC and a
    /// player trade took 4: shop_sold and traded apart. An alchemy run
    /// cast high alchemy on 250: alch_high, its natures consumed. XP rows
    /// and a report without causes stay.
    #[test]
    fn stored_provenances_rebuild_from_causes() -> Result<(), Error> {
        let path = temp_database_path("provenance-v8");
        let store = Store::open(&path)?;
        store.connection.execute_batch(
            "INSERT INTO task_reports(report_id, account, node_id, method, script, ticks, stop, created_clock) VALUES (1, 'nathan_t', 'n1', '', 'market', 100, 'done', 10), (2, 'ryan_mason', 'n2', '', 'alchemy', 100, 'done', 10), (3, 'old', 'n3', '', 'melee', 100, 'done', 10);
             INSERT INTO task_flows(report_id, item_name, cause, quantity) VALUES
               (1, 'Iron platebody', 'shop_sold', -28), (1, 'Iron platebody', 'traded', -4), (1, 'Coins', 'shop_sold', 9000),
               (2, 'Iron platebody', 'input', -250), (2, 'Nature rune', 'rune', -250), (2, 'Coins', 'product', 84000);
             INSERT INTO task_units(report_id, unit, subject, quantity) VALUES (2, 'cast', 'highlvl_alchemy', 250);
             INSERT INTO task_yields(report_id, item_name, provenance, quantity, skill, xp) VALUES
               (1, 'Iron platebody', 'traded', -32, '', 0), (2, 'Iron platebody', 'consumed', -250, '', 0), (2, '', 'produced', 0, 'magic', 16250), (3, 'Bones', 'loot', 5, '', 0);
             PRAGMA user_version = 7;",
        )?;
        super::apply_provenance_v8(&store.connection)?;
        assert_eq!(yields(&store, 1)?, vec![("Coins".into(), "shop_sold".into(), 9000), ("Iron platebody".into(), "shop_sold".into(), -28), ("Iron platebody".into(), "traded".into(), -4)]);
        assert_eq!(yields(&store, 2)?, vec![("Coins".into(), "produced".into(), 84000), ("Iron platebody".into(), "alch_high".into(), -250), ("Nature rune".into(), "consumed".into(), -250)]);
        assert_eq!(yields(&store, 3)?, vec![("Bones".into(), "loot".into(), 5)], "no causes: kept");
        let xp: i64 = store.connection.query_row("SELECT xp FROM task_yields WHERE report_id = 2 AND skill = 'magic'", [], |row| row.get(0))?;
        assert_eq!(xp, 16250, "XP rows kept");
        drop(store);
        let _ = std::fs::remove_file(&path);
        Ok(())
    }
}
