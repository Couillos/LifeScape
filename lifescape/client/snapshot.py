"""Shared snapshot reads: one source for the live Bot and the worker.

Both the SDK (world dicts) and the session worker (heartbeat snapshot)
answer the same read-only calls from the same plain data: inventories,
chat texts, stats and positions. Keeping the logic here avoids the two
sides drifting apart.
"""

from .. import spots as spots_mod


def aggregate(inv):
    items = {}
    for _com, slots in inv.items():
        for _slot, (held, count) in slots.items():
            items[held] = items.get(held, 0) + count
    return items


def backpack_com(inv, inv_cap):
    for com, slots in inv.items():
        if inv_cap.get(com) == 28:
            return com
    for com, slots in inv.items():
        if len(slots) <= 28:
            return com
    return None


def backpack_view(inv, inv_cap):
    com = backpack_com(inv, inv_cap)
    if com is None:
        return {"com": None, "items": []}
    slots = inv.get(com, {})
    return {
        "com": com,
        "items": [{"slot": slot, "id": held, "count": count} for slot, (held, count) in sorted(slots.items())],
    }


def chat_options(texts):
    items = sorted(texts.items())
    headers = [index for index, (_com, text) in enumerate(items) if "select an option" in text.lower()]
    if not headers:
        return []
    header = headers[-1]
    base = items[header][0]
    return [[com, text] for com, text in items[header + 1:header + 6] if base < com <= base + 5]


def bank_items(inv, com_ids):
    com = (com_ids or {}).get("bank_main:inv")
    return aggregate({com: inv.get(com, {})} if com is not None else {})


def combat_level(stats):
    levels = {index: level for index, (level, _xp) in stats.items()}
    return spots_mod.combat_level(levels)
