"""Shops: stock, restock rates, and keeper NPCs with multipliers."""

from dataclasses import dataclass, field

from gamedata import base


@dataclass(frozen=True)
class Stock:
    obj: str
    count: int
    rate: int = 0


@dataclass(frozen=True)
class Keeper:
    npc: str
    npc_id: int
    shop: str
    sell_multiplier: int | None = None
    buy_multiplier: int | None = None
    title: str | None = None


@dataclass(frozen=True)
class Shop:
    name: str
    scope: str | None = None
    size: int = 0
    restock: bool = False
    stocks: tuple = ()
    keepers: tuple = ()
    members: bool | None = None


def _table():
    payload = base.load_table("shops")
    if isinstance(payload, dict):
        return payload.get("shops", []), payload.get("keepers", [])
    return [], []


def all_shops():
    shops, keepers = _table()
    by_shop: dict[str, list] = {}
    for keeper in keepers:
        by_shop.setdefault(keeper.get("shop", ""), []).append(keeper)
    out = []
    for row in shops:
        name = row.get("name", "")
        out.append(Shop(
            name=name,
            scope=row.get("scope"),
            size=row.get("size", 0),
            restock=str(row.get("restock", "")).lower() in ("yes", "true", "1"),
            stocks=tuple(Stock(obj=item.get("obj", ""), count=item.get("count", 1), rate=item.get("rate", 0)) for item in row.get("stocks", [])),
            keepers=tuple(Keeper(
                npc=item.get("npc", ""),
                npc_id=item.get("npc_id", -1),
                shop=item.get("shop", ""),
                sell_multiplier=item.get("sell_multiplier"),
                buy_multiplier=item.get("buy_multiplier"),
                title=item.get("title"),
            ) for item in by_shop.get(name, [])),
        ))
    return out


def by_name(name):
    wanted = name.lower()
    return [entry for entry in all_shops() if entry.name.lower() == wanted]


def keeper_for_npc(npc_name):
    wanted = npc_name.lower()
    _, keepers = _table()
    return [keeper for keeper in keepers if str(keeper.get("npc", "")).lower() == wanted]
