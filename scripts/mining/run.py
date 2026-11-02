"""Miner: mines the best F2P rock for the player's level, banks full
loads, sells ore to fund pickaxe upgrades, reattaches lost pickaxe
heads instead of rebuying.

Adapts to any start position: bank, rocks, general store and pickaxe
shop are all picked by proximity from gamedata (best rock for the
level, safe for the combat level, nearest usable bank). Banks across
the toll gate are a last resort (10 coins per crossing). Stays on one
level.

Usage: ../lostcity/.pypy/bin/pypy3 scripts/mining/run.py <username> [loads]
  loads = full bank trips before stopping, 0 means forever (default 0).
"""

import sys


from scripts.mining.common import *
from scripts.mining.pack import *
from scripts.mining.walk import *
from scripts.mining.spots import *
from scripts.mining.gear import *
from scripts.mining.shop import *
from scripts.mining.bank import *

def work_spot(bot, rock_x, rock_z, rock_loc, ore_ids, had_ore):
    """Chop one spot until it goes quiet twice. Clicks once, then only
    re-clicks after a quiet window (the server chains attempts itself)."""
    randomevents.greet(bot)
    if max(abs(bot.world.x - rock_x), abs(bot.world.z - rock_z)) > 4:
        try:
            reached = walk_checked(bot, rock_x, rock_z, timeout=120)
        except (Unreachable, TimeoutError):
            return "unreachable", 0
        if not reached:
            return "unreachable", 0
    if max(abs(bot.world.x - rock_x), abs(bot.world.z - rock_z)) > 1:
        neighbors = sorted(
            [(rock_x + dx, rock_z + dz) for dx in (-1, 0, 1) for dz in (-1, 0, 1) if dx or dz],
            key=lambda tile: abs(tile[0] - bot.world.x) + abs(tile[1] - bot.world.z),
        )
        walked_at = time.time()
        for next_x, next_z in neighbors:
            try:
                bot.walk_to(next_x, next_z, bot.world.level, timeout=2, open_doors=False)
                print("adjacent in %.1fs" % (time.time() - walked_at), flush=True)
                break
            except (Unreachable, TimeoutError) as walk_error:
                print("walk adjacent %s failed: %s" % ((next_x, next_z), walk_error), flush=True)
                if max(abs(bot.world.x - rock_x), abs(bot.world.z - rock_z)) > 4:
                    print("wandered off from %s, giving up" % ((rock_x, rock_z),), flush=True)
                    return "unreachable", 0
                continue
        else:
            return "unreachable", 0
    gained = 0
    silences = 0
    need_click = True
    seen_msgs = len(bot.world.messages)
    while True:
        if need_click:
            print("click rock at %s" % ((rock_x, rock_z),), flush=True)
            bot.intend("mining a rock at (%d,%d)" % (rock_x, rock_z))
            bot.op_loc(1, rock_x, rock_z, rock_loc)
            seen_msgs = len(bot.world.messages)
            need_click = False
        target = bot.world.ticks + ROUND_IDLE_TICKS
        quiet = True
        while bot.world.ticks < target:
            bot.pump(2)
            if hazards.avoid(bot):
                return "threat", gained
            if bot.world.modal is not None:
                if not dismiss_levelup(bot):
                    texts = [text for _com, text in bot.chat_texts()][-4:]
                    print("modal %s texts=%s" % (bot.world.modal, [text[:60] for text in texts]), flush=True)
                    randomevents.greet(bot, timeout=20)
                    bot.pump(10)
                    if bot.world.modal is not None and not dismiss_levelup(bot):
                        return "blocked", gained
            for message in bot.world.messages[seen_msgs:]:
                lowered = message.lower()
                if "can't reach that" in lowered:
                    return "unreachable", gained
                if "mining level of" in lowered:
                    return "level", gained
                if "do not have a pickaxe" in lowered:
                    return "nopick", gained
                if "no ore currently available" in lowered or "no minerals currently available" in lowered:
                    return "depleted", gained
            now_have = sum(pack_total(bot, oid) for oid in ore_ids.values())
            if now_have > had_ore + gained:
                gained = now_have - had_ore
                quiet = False
                break
        if not quiet:
            continue
        silences += 1
        if silences >= 2:
            return "idle", gained
        need_click = True



def mine_load(bot, tiers, bank, coins_id, mining_level, side_bank, main_bank, main_shop):
    ore_ids = {}
    for tier in tiers:
        try:
            ore_ids[tier.product] = obj_id(bot, tier.product)
        except KeyError:
            continue
    ban_until = {}
    idle_since = bot.world.ticks
    last_hitpoints = hitpoints(bot)
    flees = 0
    spot = None
    tier = None
    while True:
        if not randomevents.check(bot):
            raise SystemExit("random event unresolved at %s" % (bot.position,))
        coins = pack_total(bot, coins_id)
        if free_pack_slots(bot) <= (0 if coins > 0 else 1):
            return True, bank, mining_level
        if best_pickaxe(bot) is None:
            if not recover_pickaxe(bot, side_bank, main_bank, main_shop, coins_id):
                print("pickaxe lost, banking load and rebuying, pack=%s" % (sorted(pack_counts(bot).values()),), flush=True)
                return True, bank, mining_level
        mining_level = learn_level(bot, mining_level)
        best = pick_tier(tiers, mining_level)
        if spot is not None and tier is not None and best is not None and best.level > tier.level:
            print("level up, switching tier", flush=True)
            spot = None
        hitpoints_seen = hitpoints(bot)
        if hitpoints_seen <= 0:
            print("died, respawning", flush=True)
            bot.intend("died, respawning")
            hazards.wait_respawn(bot)
            last_hitpoints = hitpoints(bot)
            spot = None
            continue
        hurt_now = hitpoints_seen < last_hitpoints
        last_hitpoints = hitpoints_seen
        if hurt_now:
            flees += 1
            print("hurt, fleeing", flush=True)
            bot.intend("fleeing a hunter")
            attack_at = (bot.world.x, bot.world.z)
            if spot is not None:
                ban_until[(spot.x, spot.z)] = bot.world.ticks + FLEE_BAN_TICKS
            ban_area(ban_until, attack_at[0], attack_at[1], ATTACK_BAN_RADIUS, bot.world.ticks + FLEE_BAN_TICKS)
            if not compass_escape(bot):
                retreat_along_trail(bot)
            if flees > MAX_FLEES_PER_LOAD:
                print("too many attacks, aborting load", flush=True)
                return False, bank, mining_level
            wait_hitpoints(bot, hitpoints(bot) + 1, timeout=200)
            spot = None
            continue
        now = bot.world.ticks
        banned = set(key for key, until in ban_until.items() if until > now)
        if spot is None or (spot.x, spot.z) in banned:
            for _wait in range(16):
                tier, spot, use_bank = pick_rock_spot(tiers, bank, mining_level, banned, bot.combat_level(), bot.world.x, bot.world.z, bot=bot, relaxed=_wait >= 8)
                if tier is not None and spot is not None:
                    break
                print("no rock spot, waiting for respawns", flush=True)
                bot.intend("waiting for a safe rock spot")
                wait_ticks(bot, 15)
            if tier is None or spot is None:
                print("no rock spot left", flush=True)
                return False, bank, mining_level
            bank = use_bank
            had_ore = sum(pack_total(bot, oid) for oid in ore_ids.values())
            print("mining %s at %s (%s)" % (tier.product, (spot.x, spot.z), bot.position), flush=True)
            bot.intend("mining %s at (%d,%d)" % (tier.product, spot.x, spot.z))
        elif not is_rock_still_there(bot, spot, tier):
            live_here = bot.world.locs.get((spot.x, spot.z, bot.world.level))
            print("rock gone at %s live=%s" % ((spot.x, spot.z), live_here), flush=True)
            ban_until[(spot.x, spot.z)] = now + DEPLETE_BAN_TICKS
            spot = None
            continue
        result, gained = work_spot(bot, spot.x, spot.z, obj_loc_id(bot, spot, tier), ore_ids, had_ore)
        had_ore += gained
        if gained > 0:
            idle_since = bot.world.ticks
        if result in ("idle", "depleted"):
            print("rock quiet, next one", flush=True)
            ban_until[(spot.x, spot.z)] = now + DEPLETE_BAN_TICKS
            spot = None
        elif result == "nopick":
            spot = None
        elif result in ("unreachable", "level"):
            ban_until[(spot.x, spot.z)] = now + 500
            spot = None
        elif result == "threat":
            print("hunter close, re-picking a spot", flush=True)
            if spot is not None:
                ban_until[(spot.x, spot.z)] = now + DEPLETE_BAN_TICKS
            spot = None
        elif result == "blocked":
            print("unknown dialog blocking mine, aborting load", flush=True)
            return False, bank, mining_level
        if bot.world.ticks - idle_since > STALL_TICKS:
            print("stall: no ore for %d ticks" % STALL_TICKS, flush=True)
            return False, bank, mining_level



def main():
    username = sys.argv[1] if len(sys.argv) > 1 else "karl_lee"
    loads = int(sys.argv[2]) if len(sys.argv) > 2 else 0
    bot = Bot()
    try:
        bot.login(username, username)
        start_ticks = bot.world.ticks
        last_greet = start_ticks
        while not bot.world.inv.get(BACKPACK_COM) and bot.world.ticks - start_ticks < 150:
            bot.pump(5)
            if bot.world.ticks - last_greet > 30:
                last_greet = bot.world.ticks
                randomevents.greet(bot)
        if not randomevents.check(bot):
            raise SystemExit("random event unresolved at %s" % (bot.position,))
        side_bank = knowledge.comps.com_id("bank_side:inv")
        main_bank = knowledge.comps.com_id("bank_main:inv")
        side_shop = knowledge.comps.com_id("shop_template_side:inv")
        main_shop = knowledge.comps.com_id("shop_template:inv")
        coins_id = obj_id(bot, COINS)
        tiers = usable_tiers()
        mining_level = learn_level(bot, 1)
        bank = pick_home_bank(bot, tiers, mining_level)
        if bank is None:
            raise SystemExit("no reachable bank from %s" % (bot.position,))
        print("bank at (%d,%d)" % (bank.x, bank.z), flush=True)
        current_pick = best_pickaxe(bot)
        junk = [held for (held, _count) in pack_counts(bot).values() if held not in known_pick_ids(bot) and held != coins_id]
        bank_coins_known = 0
        if current_pick is None or junk:
            if current_pick is None and reattach(bot):
                print("ok pickaxe reattached from pack, no bootstrap needed", flush=True)
                current_pick = best_pickaxe(bot)
                junk = [held for (held, _count) in pack_counts(bot).values() if held not in known_pick_ids(bot) and held != coins_id]
            keep = {pick_id(bot, current_pick)} if current_pick else set()
            bronze_price = obj_price(bot, "bronze_pickaxe")
            withdraw = max(0, bronze_price - pack_total(bot, coins_id)) if current_pick is None else 0
            ok, bank_coins_known = bank_trip(bot, bank, side_bank, main_bank, keep, withdraw)
            if not ok:
                raise SystemExit("cleanup bank trip failed")
            if best_pickaxe(bot) is None:
                if restore_from_bank(bot, bank, side_bank, main_bank):
                    print("ok bootstrapped pickaxe from bank", flush=True)
                    current_pick = best_pickaxe(bot)
                elif pack_total(bot, coins_id) >= bronze_price and buy_pickaxe(bot, main_shop, "bronze_pickaxe"):
                    print("ok bootstrapped bronze pickaxe", flush=True)
                else:
                    raise SystemExit("cannot bootstrap: no pickaxe, %d coins" % pack_total(bot, coins_id))
        done_loads = 0
        toolless_loads = 0
        while loads == 0 or done_loads < loads:
            try:
                ok, bank, mining_level = mine_load(bot, tiers, bank, coins_id, mining_level, side_bank, main_bank, main_shop)
            except ConnectionError:
                print("disconnected, relogging", flush=True)
                bot.relogin()
                wait_backpack(bot, BACKPACK_COM)
                continue
            if not ok:
                raise SystemExit("mine failed at %s" % (bot.position,))
            candidates, cheapest = cheapest_upgrade(bot, mining_level)
            if candidates:
                sell_for_target(bot, side_shop, coins_id, max(0, cheapest - bank_coins_known))
            current = best_pickaxe(bot)
            keep = {pick_id(bot, current)} if current else set()
            # Pack is emptied into the bank during this same trip, so the
            # withdraw must cover the full price, not price minus the
            # about-to-be-deposited pack coins.
            withdraw = cheapest if candidates else 0
            ok, bank_coins_known = bank_trip(bot, bank, side_bank, main_bank, keep, withdraw)
            if not ok:
                raise SystemExit("bank trip failed")
            if candidates:
                for candidate in candidates:
                    if mining_level >= PICK_REQS.get(candidate, 99) and buy_pickaxe(bot, main_shop, candidate):
                        print("ok upgraded to %s" % candidate, flush=True)
                        break
            if best_pickaxe(bot) is None:
                toolless_loads += 1
            else:
                toolless_loads = 0
            if toolless_loads >= 2:
                raise SystemExit("no pickaxe and cannot rebuy, aborting at %s" % (bot.position,))
            done_loads += 1
            print("ok load %d banked (mining=%s)" % (done_loads, mining_level), flush=True)
        return 0
    finally:
        bot.close()


if __name__ == "__main__":
    sys.exit(main())

if __name__ == "__main__":
    sys.exit(main())
