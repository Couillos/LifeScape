"""Verification loop: static cross-checks + quick live spot checks.

Run until everything is green, fix the dump or the code, repeat:
    ../../lostcity/.pypy/bin/pypy3 scripts/verify.py
"""

import json
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))

from gamedata import base

DATA_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "data")

EXPECTED_TABLES = (
    "objs",
    "npcs",
    "locs",
    "comps",
    "shops",
    "spells",
    "recipes",
    "quests",
    "varps",
    "spawns_obj",
    "trees",
    "rocks",
)

_failures = []


def check(condition, what):
    print(("ok %s" % what) if condition else ("FAIL %s" % what), flush=True)
    if not condition:
        _failures.append(what)
    return condition


def check_static():
    for table_name in EXPECTED_TABLES:
        path = os.path.join(DATA_DIR, table_name + ".json")
        if not check(os.path.exists(path), "table present: %s" % table_name):
            continue
        try:
            with open(path, encoding="utf-8") as handle:
                payload = json.load(handle)
        except ValueError:
            check(False, "table parses: %s" % table_name)
            continue
        check(isinstance(payload, (list, dict)) and len(payload) > 0, "table non-empty: %s" % table_name)


def check_cross_refs():
    sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
    import gamedata as knowledge

    obj_names = {entry.name for entry in knowledge.all_objs() if entry.name}
    npc_count = len(knowledge.all_npcs())
    missing_objs = set()
    for shop in knowledge.all_shops():
        for stock in shop.stocks:
            if stock.obj not in obj_names:
                missing_objs.add(stock.obj)
    check(not missing_objs, "shop stocks resolve: %s" % sorted(missing_objs)[:5])

    bad_spells = [spell.spell for spell in knowledge.all_spells() if not spell.com or knowledge.com_id(spell.com) is None]
    check(not bad_spells, "spell comps resolve: %s" % bad_spells[:5])

    bad_recipes = 0
    # Fletching 'metal' is a material class (resolved with arrowheads at
    # runtime), not an obj debugname.
    materials = {"bronze", "iron", "steel", "mithril", "adamant", "rune"}
    for recipe in knowledge.all_recipes():
        for name in list(recipe.inputs) + list(recipe.outputs):
            if isinstance(name, str) and name not in obj_names and name not in ("null", "") and name not in materials:
                bad_recipes += 1
    check(bad_recipes == 0, "recipe objs resolve (bad=%d)" % bad_recipes)

    npc_names = {entry.name for entry in knowledge.all_npcs() if entry.name}
    bad_keepers = []
    for shop in knowledge.all_shops():
        for keeper in shop.keepers:
            if keeper.npc not in npc_names:
                bad_keepers.append(keeper.npc)
    check(not bad_keepers, "keeper npcs resolve: %s" % bad_keepers[:5])

    bad_spawns = [spawn.npc_id for spawn in knowledge.all_npc_spawns() if spawn.npc_id < 0 or spawn.npc_id >= npc_count]
    check(not bad_spawns, "npc spawn ids in range: %s" % bad_spawns[:5])
    total_spawns = sum(len(entry.spawns) for entry in knowledge.all_npcs() if entry is not None)
    check(total_spawns > 7000, "npc spawn points embedded in npcs.json: %d" % total_spawns)
    no_members_npc = [1 for entry in knowledge.all_npcs() if entry is not None and not isinstance(entry.members, bool)]
    check(not no_members_npc, "npcs carry members flag")
    no_hunt = [entry.name for entry in knowledge.all_npcs() if entry is not None and not isinstance(entry.huntrange, int)]
    check(not no_hunt, "npcs carry huntrange")
    no_hunts_flag = [entry.name for entry in knowledge.all_npcs() if entry is not None and not isinstance(entry.hunts_players, bool)]
    check(not no_hunts_flag, "npcs carry hunts_players flag")
    no_members_obj = [1 for entry in knowledge.all_objs() if entry is not None and not isinstance(entry.members, bool)]
    check(not no_members_obj, "objs carry members flag")

    survival = [spawn for spawn in knowledge.all_npc_spawns(943) if (spawn.x, spawn.z) == (3103, 3095)]
    check(len(survival) == 1, "survival instructor static spawn at pond")

    banks = knowledge.all_banks()
    check(len(banks) > 50, "bank booth spawns: %d" % len(banks))
    check(all(isinstance(bank.members, bool) for bank in banks), "banks carry location members flag")
    check(any(bank.members for bank in banks) and any(not bank.members for bank in banks), "banks span both zones")
    draynor = [bank for bank in banks if abs(bank.x - 3091) + abs(bank.z - 3243) <= 5]
    check(len(draynor) >= 2, "draynor bank booths")
    near_lumby = knowledge.nearest_bank(3222, 3222, 0)
    check(near_lumby is not None, "nearest bank to lumbridge exists")

    tiers = knowledge.all_tiers()
    check(len(tiers) == 6, "woodcutting tiers: %d" % len(tiers))
    oak_tier = next((tier for tier in tiers if tier.product == "oak_logs"), None)
    check(oak_tier is not None and oak_tier.level == 15 and oak_tier.exp == 375, "oak tier facts")
    fish_tiers = knowledge.all_fish_tiers()
    check(len(fish_tiers) == 6, "fishing tiers: %d" % len(fish_tiers))
    shrimp_tier = next((tier for tier in fish_tiers if tier.products[0] == "raw_shrimp"), None)
    check(shrimp_tier is not None and shrimp_tier.level == 1 and shrimp_tier.tool == "net", "shrimp tier facts")
    check(knowledge.best_fish_tier(25).products[0] == "raw_pike", "pike best at 25")
    fish_products = {product for tier in fish_tiers for product in tier.products}
    check(not (fish_products - obj_names), "fish products resolve: %s" % sorted(fish_products - obj_names)[:5])
    salt_ids = knowledge.spot_npc_ids("saltfish")
    check(len(salt_ids) > 5, "saltfish spot npcs: %d" % len(salt_ids))
    check(all(knowledge.npc_by_id(npc_id) is not None for npc_id in salt_ids), "spot npc ids in range")
    tier_names = set()
    for tier in tiers:
        tier_names.update(tier.trees)
    missing_trees = [name for name in tier_names if not knowledge.loc_by_name(name)]
    check(not missing_trees, "tree locs resolve: %s" % missing_trees[:5])
    oak_spots = knowledge.loc_spawns_near(("oaktree",), 3200, 3220, 0, radius=30)
    check(len(oak_spots) > 0, "oak spots near lumbridge: %d" % len(oak_spots))

    rock_tiers = knowledge.all_rock_tiers()
    check(len(rock_tiers) == 17, "mining rock tiers: %d" % len(rock_tiers))
    copper_tier = next((tier for tier in rock_tiers if tier.product == "copper_ore"), None)
    check(copper_tier is not None and copper_tier.level == 1 and copper_tier.exp == 175, "copper tier facts")
    iron_tier = next((tier for tier in rock_tiers if tier.product == "iron_ore"), None)
    check(iron_tier is not None and iron_tier.level == 15, "iron tier facts")
    check(all(tier.product_members is False for tier in rock_tiers if tier.product in ("copper_ore", "tin_ore", "iron_ore", "coal")), "f2p ore flags")
    rock_locs = set()
    for tier in rock_tiers:
        rock_locs.update(name for name in tier.rocks if not name.startswith("macro_"))
    missing_rocks = [name for name in rock_locs if not knowledge.loc_by_name(name)]
    check(not missing_rocks, "rock locs resolve: %s" % missing_rocks[:5])
    copper_spots = knowledge.loc_spawns_near(("copperrock1", "copperrock2"), 3299, 3015, 0, radius=30)
    check(len(copper_spots) > 0, "copper spots dumped: %d" % len(copper_spots))

    wind = knowledge.spell_by_name("wind_strike")
    check(len(wind) == 1 and wind[0].runes == {"mindrune": 1, "airrune": 1} and wind[0].level == 1, "wind strike facts")

    dagger = [recipe for recipe in knowledge.recipes_making("bronze_dagger", "smith")]
    check(len(dagger) == 1 and dagger[0].inputs == {"bronze_bar": 1}, "bronze dagger recipe")


def main():
    check_static()
    check_cross_refs()
    print("done, failures: %d" % len(_failures), flush=True)
    if _failures:
        raise SystemExit("verify FAILED: %s" % "; ".join(_failures))
    print("verify OK", flush=True)


if __name__ == "__main__":
    main()
