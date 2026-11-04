"""Live-test les marches que les bots font tout le temps.

Teleport au depart (staff), puis vraie marche avec le pathfinder normal :
villes, banques, spots woodcut/mining, approche de booth. Le portail a
peage est detecte en live (loc dynamique) et valide le diagnostic.
Resultats dans run/routes_test.json.

Dev tool, pas un bot.

Usage: scripts/orchestrator.py run <account> check_routes [--only NAME] [--fresh] [--limit N] [--skip N]
"""

import argparse
import json
import os
import time

from lifescape.client.core import Unreachable
from lifescape.connection import default_paths

ROOT = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
RESULTS = os.path.join(ROOT, "run", "routes_test.json")

# Coeurs de ville, niveau 0 : que du terrain ouvert entre eux.
BACKBONE = (
    ("lumbridge-varrock", (0, 3222, 3218), (0, 3205, 3429)),
    ("varrock-lumbridge", (0, 3205, 3429), (0, 3222, 3218)),
    ("varrock-falador", (0, 3205, 3429), (0, 2965, 3378)),
    ("falador-port-sarim", (0, 2965, 3378), (0, 3030, 3220)),
    ("port-sarim-draynor", (0, 3030, 3220), (0, 3092, 3242)),
)

TREE_NAMES = ("tree", "oaktree", "willowtree")
ROCK_NAMES = ("clayrock1", "clayrock2", "copperrock1", "copperrock2",
              "tinrock1", "tinrock2", "ironrock1", "ironrock2")


def build_legs(knowledge):
    """Troncons statiques : villes + banque->spot depuis gamedata."""
    legs = [
        {"name": name, "start": start, "dest": dest, "expect": "reach"}
        for name, start, dest in BACKBONE
    ]
    banks = [bank for bank in knowledge.all_banks()
             if not bank.members and bank.level == 0]
    seen_trees, seen_rocks, seen_booths = 0, 0, 0
    for bank in sorted(banks, key=lambda entry: (entry.x, entry.z)):
        if seen_trees < 2:
            for tree in TREE_NAMES:
                spots = knowledge.loc_spawns_near(tree, bank.x, bank.z, 0, radius=60)
                if spots:
                    spot = spots[0]
                    legs.append({"name": "bank-tree-%d-%d" % (bank.x, bank.z),
                                 "start": (0, bank.x, bank.z),
                                 "dest": (0, spot.x, spot.z), "expect": "reach"})
                    seen_trees += 1
                    break
        if seen_rocks < 2:
            for rock in ROCK_NAMES:
                spots = knowledge.loc_spawns_near(rock, bank.x, bank.z, 0, radius=60)
                if spots:
                    spot = spots[0]
                    legs.append({"name": "bank-rock-%d-%d" % (bank.x, bank.z),
                                 "start": (0, bank.x, bank.z),
                                 "dest": (0, spot.x, spot.z), "expect": "reach"})
                    seen_rocks += 1
                    break
        if seen_booths < 2:
            legs.append({"name": "bank-booth-%d-%d" % (bank.x, bank.z),
                         "start": (0, bank.x + 6, bank.z),
                         "dest": (0, bank.x, bank.z), "expect": "reach"})
            seen_booths += 1
        if seen_trees >= 2 and seen_rocks >= 2 and seen_booths >= 2:
            break
    return legs


def find_toll_gate(bot):
    """Portail a peage : loc dynamique, cherche en live dans static_locs."""
    for (x, z, level), (loc_id, shape, angle) in bot.world.static_locs.items():
        if level != 0:
            continue
        try:
            entry = bot.loctypes["loc"][loc_id]
        except (IndexError, TypeError, KeyError):
            continue
        if entry and "toll" in (entry.get("name") or "").lower():
            return (x, z, level, shape, angle)
    return None


def teleport(bot, level, x, z):
    body = "tele %d,%d,%d,%d,%d" % (level, x >> 6, z >> 6, x & 63, z & 63)
    bot.cheat(body)
    target = bot.world.ticks + 250
    while bot.world.ticks < target:
        bot.pump(2)
        if (bot.world.level, bot.world.x, bot.world.z) == (level, x, z):
            return True
    return False


def _walk_leg(bot, leg, timeout, force_run=False):
    """Une marche : (outcome ou None si yank, ticks de marche).

    Dest bloquee (arbre, rocher, booth) : on s'arrete a cote via
    approach, le pathfinding ne marche que sur des cases libres.
    """
    from lifescape import collision as collision_mod

    (slevel, sx, sz), (dlevel, dx, dz) = leg["start"], leg["dest"]
    paid = bool(leg.get("paid", False))
    run = True if force_run else None
    try:
        blocked = not collision_mod.tile_walkable(bot.collision.get(dx, dz, dlevel))
    except (KeyError, IndexError):
        blocked = False
    started = bot.world.ticks
    try:
        if blocked or leg.get("approach"):
            bot.approach(dx, dz, dlevel, radius=2, timeout=timeout, open_doors=True, paid=paid)
        else:
            bot.walk_to(dx, dz, dlevel, timeout=timeout, paid=paid, run=run)
    except Unreachable as error:
        if leg["expect"] == "diagnose" and "toll" in str(error).lower():
            return "ok (diagnosed toll)", bot.world.ticks - started
        if _yanked(bot, leg):
            return None, bot.world.ticks - started
        return "unreachable: %s" % (error,), bot.world.ticks - started
    except TimeoutError as error:
        if _yanked(bot, leg):
            return None, bot.world.ticks - started
        return "timeout: %s" % (error,), bot.world.ticks - started
    spent = bot.world.ticks - started
    if leg["expect"] == "diagnose":
        return "ok (walked through, had coins?)", spent
    return "ok in %d ticks" % spent, spent


def _planner_reaches(bot, leg):
    """Le planner voit-il un chemin ? Pres = non vide (walk retarget
    dessus) ; loin = vraie progression, sinon le flood doit prouver
    qu'on n'est pas enferme (quai cloture, enclos)."""
    (slevel, sx, sz), (dlevel, dx, dz) = leg["start"], leg["dest"]
    if abs(dx - sx) > 60 or abs(dz - sz) > 60:
        return bot._walk_leg_ok(bot.planning_map, leg["start"], leg["dest"],
                                flood_min=60 * 60)
    return bool(bot._find_path(bot.planning_map, slevel, sx, sz, dx, dz,
                               max_waypoints=10000))


def _yanked(bot, leg):
    """Yanke par un event : loin du depart ET de l'arrivee."""
    (slevel, sx, sz), (dlevel, dx, dz) = leg["start"], leg["dest"]
    if bot.world.level != slevel:
        return True
    if abs(bot.world.x - sx) + abs(bot.world.z - sz) > 30:
        return abs(bot.world.x - dx) + abs(bot.world.z - dz) > 30
    return False


TOLL_PRICE = 10


def _pack_coins(bot, knowledge):
    """Pieces dans le pack (le peage se paie comptant)."""
    try:
        coins = knowledge.objs.by_name("coins")
    except (AttributeError, KeyError):
        return 0
    if not coins:
        return 0
    coin_id = coins[0].obj_id
    try:
        _com, slots = bot.backpack()
    except (AttributeError, KeyError):
        return 0
    return sum(count for _slot, (_held, count) in slots.items() if _held == coin_id)


def test_leg(bot, leg, force_run=False):
    from scripts import randomevents

    paid = bool(leg.get("paid", False))
    run = True if force_run else None
    if paid:
        import gamedata as knowledge
        if _pack_coins(bot, knowledge) < TOLL_PRICE:
            return "skipped: no coins for toll"
    (slevel, sx, sz), (dlevel, dx, dz) = leg["start"], leg["dest"]
    greet_ticks = 0
    staged = teleport(bot, slevel, sx, sz)
    if staged:
        before = bot.world.ticks
        randomevents.greet(bot)
        greet_ticks += bot.world.ticks - before
    else:
        # Pas staff (ou tele refuse) : rejoint le depart a pied, c'est
        # aussi un test de marche.
        dist_start = abs(sx - bot.world.x) + abs(sz - bot.world.z)
        try:
            bot.walk_to(sx, sz, slevel, timeout=min(20000, max(3000, dist_start * 30)), paid=paid, run=run)
        except (Unreachable, TimeoutError) as error:
            return "no staging at %s: %s" % (leg["start"], error)
        before = bot.world.ticks
        randomevents.greet(bot)
        greet_ticks += bot.world.ticks - before
    dist = abs(dx - sx) + abs(dz - sz)
    timeout = min(20000, max(3000, dist * 30))
    walk_ticks = 0
    for attempt in (1, 2, 3):
        outcome, spent = _walk_leg(bot, leg, timeout, force_run)
        walk_ticks += spent
        if outcome is not None:
            before = bot.world.ticks
            randomevents.greet(bot)
            greet_ticks += bot.world.ticks - before
            tag = " [run]" if force_run else ""
            if attempt > 1:
                return "restaged x%d: %s%s (walk %d, greet %d)" % (attempt - 1, outcome, tag, walk_ticks, greet_ticks)
            if outcome.startswith("ok in"):
                return "%s%s (walk %d, greet %d)" % (outcome, tag, walk_ticks, greet_ticks)
            return outcome
        # Yank event en cours de patte : re-staging, puis on note.
        print("yanked mid-leg, restaging at %s" % (leg["start"],), flush=True)
        before = bot.world.ticks
        randomevents.greet(bot)
        greet_ticks += bot.world.ticks - before
        if not staged or not teleport(bot, slevel, sx, sz):
            break
    return "unreachable: yanked by random event mid-leg"


def save(results):
    os.makedirs(os.path.dirname(RESULTS), exist_ok=True)
    with open(RESULTS, "w") as handle:
        json.dump(results, handle, indent=1, sort_keys=True)


def run(bot, *args):
    import gamedata as knowledge

    parser = argparse.ArgumentParser()
    parser.add_argument("--only", default=None)
    parser.add_argument("--fresh", action="store_true")
    parser.add_argument("--limit", type=int, default=0)
    parser.add_argument("--skip", type=int, default=0)
    parser.add_argument("--trace", default=None)
    parser.add_argument("--run", action="store_true",
                        help="force la course (mesure à plat, énergie non gérée)")
    options = parser.parse_args(args)

    bot.paths = default_paths()
    bot.ensure_nav()
    if options.trace:
        bot.connection.trace = open(options.trace, "a")
    legs = build_legs(knowledge)
    gate = find_toll_gate(bot)
    if gate is not None:
        x, z, level, _shape, angle = gate
        if angle in (0, 2):
            start, dest = (level, x - 8, z), (level, x + 8, z)
        else:
            start, dest = (level, x, z - 8), (level, x, z + 8)
        legs.append({"name": "toll-gate", "start": start, "dest": dest, "expect": "diagnose"})
        legs.append({"name": "toll-gate-paid", "start": start, "dest": dest, "expect": "reach", "paid": True})
    else:
        print("no toll gate in static locs, skipping", flush=True)
    if options.only:
        legs = [leg for leg in legs if options.only in leg["name"]]
    results = {}
    if not options.fresh:
        try:
            with open(RESULTS) as handle:
                previous = json.load(handle)
        except (OSError, ValueError):
            previous = {}
        for key, entry in previous.items():
            if entry.get("outcome", "").startswith("ok"):
                results[key] = entry
    legs = [leg for leg in legs if leg["name"] not in results or not results[leg["name"]]["outcome"].startswith("ok")]
    legs = legs[options.skip:]
    if options.limit:
        legs = legs[:options.limit]
    # Le planner (portes ouvertes, F2P) refuse les pattes sans chemin :
    # enclos scelle, quai cloture, portail members. Meme regle que la
    # marche (un chemin non vide suffit, walk_to retarget dessus).
    validated = []
    for leg in legs:
        if leg["start"][0] != leg["dest"][0]:
            validated.append(leg)
        elif _planner_reaches(bot, leg):
            validated.append(leg)
        else:
            width, height, angle, shape, access = bot._describe_target(leg["dest"][1], leg["dest"][2], leg["dest"][0])
            diag = bot._find_path(bot.planning_map, leg["start"][0], leg["start"][1], leg["start"][2],
                                  leg["dest"][1], leg["dest"][2], max_waypoints=10000)
            visited = sum(1 for distance in bot.router.distances if distance < 99999999)
            print("skip   %-22s no planner path (diag path=%d end=%s visited=%d shape=%s compass=%s)" % (
                leg["name"], len(diag), diag[-1][1:] if diag else None, visited, shape,
                "f2p" if getattr(bot, "freemap", None) is not None else "NOFREEMAP"), flush=True)
            results[leg["name"]] = {
                "start": list(leg["start"]),
                "dest": list(leg["dest"]),
                "expect": leg["expect"],
                "outcome": "skipped: no planner path",
                "seconds": 0,
            }
    legs = validated
    print("resume: %d already ok, %d to test" % (len(results), len(legs)), flush=True)
    for index, leg in enumerate(legs):
        started = time.time()
        try:
            outcome = test_leg(bot, leg, force_run=options.run)
        except Exception as error:  # report, keep testing the next one
            outcome = "error: %s: %s" % (type(error).__name__, error)
        results[leg["name"]] = {
            "start": list(leg["start"]),
            "dest": list(leg["dest"]),
            "expect": leg["expect"],
            "outcome": outcome,
            "seconds": round(time.time() - started, 1),
        }
        print("%4d/%d %-22s %s" % (index + 1, len(legs), leg["name"], outcome), flush=True)
        if (index + 1) % 5 == 0:
            save(results)
        if not outcome.startswith("ok"):
            teleport(bot, 0, 3222, 3218)
    save(results)
    skipped = {name: entry["outcome"] for name, entry in results.items() if entry["outcome"].startswith("skipped")}
    bad = {name: entry["outcome"] for name, entry in results.items() if not entry["outcome"].startswith(("ok", "skipped"))}
    print("tested=%d ok=%d failed=%d skipped=%d" % (len(results), len(results) - len(bad) - len(skipped), len(bad), len(skipped)), flush=True)
    for name, outcome in sorted(bad.items()):
        print("  %-22s %s" % (name, outcome))
    for name, outcome in sorted(skipped.items()):
        print("  %-22s %s" % (name, outcome))
    return 0
