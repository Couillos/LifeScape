"""Every 30 minutes, a point of every account's holdings.

The orchestrator reads the engine save files directly (inventory, worn and
bank), so it never calls the game server. Accounts that are currently
supervised are read from their live bot snapshot instead (inventory, worn
and the last streamed bank), and their stale save is skipped.

History: run/items/holdings.json, one point per 30 minutes, 24h kept.
"""

import json
import os
import sqlite3
import sys
import threading
import time

from lifescape import save

from .naming import humanize
from .paths import ROOT, RUN_DIR, read_json

sys.path.insert(0, os.path.join(ROOT, "gamedata"))

import gamedata as knowledge

SAVE_DIR = save.default_save_dir()
DB_PATH = os.path.join(RUN_DIR, "items", "holdings.sqlite")
LEGACY_JSON = os.path.join(RUN_DIR, "items", "holdings.json")
INTERVAL_SECONDS = 30 * 60
MAX_POINTS = 48
COINS = 995
LIVE_FRESH_SECONDS = 600
# Staff/private accounts: never counted in the public holdings totals.
EXCLUDED_ACCOUNTS = {"admin", "bite"}

_lock = threading.RLock()  # current_holdings() is called under sample_now's lock


def parse_save(path):
    """Player save state; holdings use only its inv/worn/bank keys."""
    return save.parse(path)


def _live_holdings():
    """{account: holdings} from recent bot snapshots (id-keyed inv/worn/bank)."""
    found = {}
    users_dir = os.path.join(RUN_DIR, "users")
    try:
        names = os.listdir(users_dir)
    except OSError:
        return found
    now = time.time()
    for name in names:
        if not name.endswith(".bot.json"):
            continue
        state = read_json(os.path.join(users_dir, name), {}) or {}
        if now - (state.get("updated") or 0) > LIVE_FRESH_SECONDS:
            continue
        account = (state.get("account") or name[: -len(".bot.json")]).lower()
        if account in EXCLUDED_ACCOUNTS:
            continue
        holdings = {"inv": {}, "worn": {}, "bank": {}}
        for key in ("inv", "worn", "bank"):
            for item in state.get(key) or []:
                obj_id = item.get("id")
                count = item.get("count") or 0
                if obj_id is None or count <= 0:
                    continue
                holdings[key][obj_id] = holdings[key].get(obj_id, 0) + count
        found[account] = holdings
    return found


def current_holdings():
    """{obj_id: count} over live supervised accounts + every other save."""
    with _lock:
        live = _live_holdings()
        totals = {}

        def add(bucket):
            for obj_id, count in bucket.items():
                totals[obj_id] = totals.get(obj_id, 0) + count

        for holdings in live.values():
            add(holdings["inv"])
            add(holdings["worn"])
            add(holdings["bank"])
        try:
            saves = [name for name in os.listdir(SAVE_DIR) if name.endswith(".sav")]
        except OSError:
            saves = []
        for name in saves:
            account = name[: -len(".sav")].lower()
            if account in EXCLUDED_ACCOUNTS:
                continue
            saved = parse_save(os.path.join(SAVE_DIR, name))
            if account in live:
                # stale save: only its bank helps until the bot opens the bank
                if not live[account]["bank"]:
                    add(saved["bank"])
                continue
            add(saved["inv"])
            add(saved["worn"])
            add(saved["bank"])
        return totals


def _connect():
    os.makedirs(os.path.dirname(DB_PATH), exist_ok=True)
    connection = sqlite3.connect(DB_PATH, timeout=10)
    connection.execute("PRAGMA journal_mode=WAL").fetchall()
    connection.execute(
        "CREATE TABLE IF NOT EXISTS samples ("
        "id INTEGER PRIMARY KEY AUTOINCREMENT, "
        "taken_at INTEGER NOT NULL)"
    )
    connection.execute(
        "CREATE TABLE IF NOT EXISTS holdings ("
        "sample_id INTEGER NOT NULL, "
        "obj_id INTEGER NOT NULL, "
        "count INTEGER NOT NULL, "
        "PRIMARY KEY (sample_id, obj_id))"
    )
    connection.execute("CREATE INDEX IF NOT EXISTS holdings_obj ON holdings(obj_id)")
    connection.commit()
    return connection


def _migrate_legacy_json(connection):
    """Import the old JSON history once, then forget it."""
    if not os.path.exists(LEGACY_JSON):
        return
    if connection.execute("SELECT COUNT(*) FROM samples").fetchall()[0][0] > 0:
        os.remove(LEGACY_JSON)
        return
    raw = read_json(LEGACY_JSON, {}) or {}
    with connection:
        for sample in raw.get("samples") or []:
            cursor = connection.execute("INSERT INTO samples (taken_at) VALUES (?)", (int(sample.get("time") or 0),))
            connection.executemany(
                "INSERT OR REPLACE INTO holdings (sample_id, obj_id, count) VALUES (?, ?, ?)",
                [(cursor.lastrowid, int(obj_id), int(count)) for obj_id, count in (sample.get("items") or {}).items()],
            )
    os.remove(LEGACY_JSON)


def sample_now():
    """The 30-minute job: write one timestamped sample of every holding."""
    with _lock:
        holdings = current_holdings()
        if not holdings:
            return False
        connection = _connect()
        try:
            _migrate_legacy_json(connection)
            with connection:
                cursor = connection.execute("INSERT INTO samples (taken_at) VALUES (?)", (int(time.time()),))
                sample_id = cursor.lastrowid
                connection.executemany(
                    "INSERT OR REPLACE INTO holdings (sample_id, obj_id, count) VALUES (?, ?, ?)",
                    [(sample_id, obj_id, count) for obj_id, count in holdings.items()],
                )
                connection.execute(
                    "DELETE FROM holdings WHERE sample_id IN ("
                    "SELECT id FROM samples ORDER BY id DESC LIMIT -1 OFFSET ?)",
                    (MAX_POINTS,),
                )
                connection.execute(
                    "DELETE FROM samples WHERE id NOT IN ("
                    "SELECT id FROM samples ORDER BY id DESC LIMIT ?)",
                    (MAX_POINTS,),
                )
        finally:
            connection.close()
    print("holdings: %d item kinds across all accounts" % len(holdings), flush=True)
    return True


def maybe_sample():
    connection = _connect()
    try:
        _migrate_legacy_json(connection)
        rows = connection.execute("SELECT taken_at FROM samples ORDER BY id DESC LIMIT 1").fetchall()
        row = rows[0] if rows else None
    finally:
        connection.close()
    if row and time.time() - row[0] < INTERVAL_SECONDS:
        return False
    return sample_now()


def snapshot(top_items=500):
    connection = _connect()
    try:
        _migrate_legacy_json(connection)
        rows = connection.execute("SELECT id, taken_at FROM samples ORDER BY id DESC LIMIT ?", (MAX_POINTS,)).fetchall()
        rows.reverse()
        sample_ids = [row[0] for row in rows]
        times = [row[1] for row in rows]
        by_item = {}
        if sample_ids:
            placeholders = ",".join("?" * len(sample_ids))
            for sample_id, obj_id, count in connection.execute(
                "SELECT sample_id, obj_id, count FROM holdings WHERE sample_id IN (%s)" % placeholders,
                sample_ids,
            ).fetchall():
                by_item.setdefault(obj_id, {})[sample_id] = count
    finally:
        connection.close()

    live = current_holdings()
    live_total = sum(live.values())

    ids = set(live.keys())
    ids.add(COINS)
    ids.update(by_item.keys())

    series = {obj_id: [by_item.get(obj_id, {}).get(sample_id, 0) for sample_id in sample_ids] for obj_id in ids}
    total = [sum(by_item.get(obj_id, {}).get(sample_id, 0) for obj_id in by_item) for sample_id in sample_ids]

    ranked = sorted(ids, key=lambda obj_id: (-live.get(obj_id, 0), obj_id))[:top_items]
    items = [
        {
            "id": obj_id,
            "name": knowledge.objs.display_name_by_id(obj_id) or humanize(knowledge.obj_name_by_id(obj_id)) or ("obj %d" % obj_id),
            "count": live.get(obj_id, 0),
            "series": series[obj_id],
        }
        for obj_id in ranked
    ]

    return {
        "intervalMinutes": INTERVAL_SECONDS // 60,
        "samples": times,
        "total": total,
        "liveTotal": live_total,
        "items": items,
    }


def start_scheduler():
    def loop():
        while True:
            try:
                maybe_sample()
            except Exception as error:
                print("holdings error: %r" % (error,), flush=True)
            time.sleep(60)

    threading.Thread(target=loop, daemon=True).start()
