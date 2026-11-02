"""Woodcutter: chops the best affordable trees for the player's level,
banks full loads, sells logs to fund axe upgrades, reattaches
lost axe heads instead of rebuying.

Adapts to any start position: bank, trees, general store and axe shop
are all picked by proximity from gamedata (best spot for the level,
safe for the combat level, nearest usable bank). Banks across the
toll gate are a last resort (10 coins per crossing). Stays on one level.

Usage: ../lostcity/.pypy/bin/pypy3 scripts/woodcut/run.py <username> [loads]
  loads = full bank trips before stopping, 0 means forever (default 0).
"""

import sys
import time


from scripts.woodcut.common import *
from scripts.woodcut.pack import *
from scripts.woodcut.walk import *
from scripts.woodcut.spots import *
from scripts.woodcut.gear import *
from scripts.woodcut.shop import *
from scripts.woodcut.bank import *

def work_spot(bot, tree_x, tree_z, tree_loc, log_ids, had_logs):
    """Chop one spot until it goes quiet twice. Clicks once, then only
    re-clicks after a quiet window (the server chains attempts itself)."""
    if max(abs(bot.world.x - tree_x), abs(bot.world.z - tree_z)) > 4:
        # Far spot (live trees can be far when the area is depleted):
        # trek properly instead of hopping neighbor tiles across the map.
        try:
            reached = walk_checked(bot, tree_x, tree_z, timeout=120)
        except (Unreachable, TimeoutError):
            return "unreachable", 0
        if not reached:
            return "unreachable", 0
    if max(abs(bot.world.x - tree_x), abs(bot.world.z - tree_z)) > 1:
        neighbors = sorted(
            [(tree_x + dx, tree_z + dz) for dx in (-1, 0, 1) for dz in (-1, 0, 1) if dx or dz],
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
                if max(abs(bot.world.x - tree_x), abs(bot.world.z - tree_z)) > 4:
                    print("wandered off from %s, giving up" % ((tree_x, tree_z),), flush=True)
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
            print("click tree at %s" % ((tree_x, tree_z),), flush=True)
            bot.intend("chopping a tree at (%d,%d)" % (tree_x, tree_z))
            bot.op_loc(1, tree_x, tree_z, tree_loc)
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
                if "woodcutting level of" in lowered:
                    return "level", gained
                if "do not have an axe" in lowered:
                    return "noaxe", gained
            now_have = sum(pack_total(bot, oid) for oid in log_ids.values())
            if now_have > had_logs + gained:
                gained = now_have - had_logs
                quiet = False
                break
        if not quiet:
            continue
        randomevents.greet(bot)
        silences += 1
        if silences == 1 and is_stump(bot, tree_x, tree_z):
            return "idle", gained
        if silences >= 2:
            return "idle", gained
        need_click = True



def chop_load(bot, tiers, bank, coins_id, woodcutting_level, side_bank, main_bank, main_shop):
    log_ids = {}
    for tier in tiers:
        try:
            log_ids[tier.product] = obj_id(bot, tier.product)
        except KeyError:
            continue
    ban_until = {}
    idle_since = bot.world.ticks
    last_hitpoints = hitpoints(bot)
    flees = 0
    spot = None
    tier = None
    while True:
        ensure_not_mazed(bot)
        coins = pack_total(bot, coins_id)
        if free_pack_slots(bot) <= (0 if coins > 0 else 1):
            return True, bank, woodcutting_level
        if best_axe(bot) is None:
            if not recover_axe(bot, side_bank, main_bank, main_shop, coins_id):
                print("axe lost, banking load and rebuying, pack=%s" % (sorted(pack_counts(bot).values()),), flush=True)
                return True, bank, woodcutting_level
        woodcutting_level = learn_level(bot, woodcutting_level)
        best_tier = pick_tier(tiers, woodcutting_level)
        if spot is not None and tier is not None and best_tier is not None and tier.level < best_tier.level:
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
            nearby = []
            for _slot, npc in bot.world.npcs.items():
                entry = bot.loctypes["npc"][npc["type"]] if npc["type"] < len(bot.loctypes["npc"]) else None
                label = entry["name"] if entry else npc["type"]
                nearby.append((abs(npc["x"] - bot.world.x) + abs(npc["z"] - bot.world.z), label, npc["x"], npc["z"]))
            nearby.sort()
            print("nearest npcs: %s" % ([(label, x, z, dist) for dist, label, x, z in nearby[:5]],), flush=True)
            attack_at = (bot.world.x, bot.world.z)
            if spot is not None:
                ban_until[(spot.x, spot.z)] = bot.world.ticks + FLEE_BAN_TICKS
            # The hunter may camp the tree (or chase us): ban the area and
            # break contact far away so the re-pick truly relocates.
            ban_area(ban_until, attack_at[0], attack_at[1], ATTACK_BAN_RADIUS, bot.world.ticks + FLEE_BAN_TICKS)
            if not compass_escape(bot):
                retreat_along_trail(bot)
            if flees > MAX_FLEES_PER_LOAD:
                print("too many attacks, aborting load", flush=True)
                return False, bank, woodcutting_level
            wait_hitpoints(bot, hitpoints(bot) + 1, timeout=200)
            spot = None
            continue
        now = bot.world.ticks
        banned = set(key for key, until in ban_until.items() if until > now)
        if spot is None or (spot.x, spot.z) in banned:
            for _wait in range(16):
                tier, spot, use_bank = pick_tree_spot(tiers, bank, woodcutting_level, banned, bot.combat_level(), bot.world.x, bot.world.z, bot=bot, relaxed=_wait >= 8)
                if tier is not None and spot is not None:
                    break
                print("no tree spot, waiting for respawns", flush=True)
                bot.intend("waiting for a safe tree spot")
                wait_ticks(bot, 15)
            if tier is None or spot is None:
                print("no tree spot left", flush=True)
                return False, bank, woodcutting_level
            bank = use_bank
            had_logs = sum(pack_total(bot, oid) for oid in log_ids.values())
            print("chopping %s at %s (%s)" % (tier.product, (spot.x, spot.z), bot.position), flush=True)
            bot.intend("chopping %s at (%d,%d)" % (tier.product, spot.x, spot.z))
        elif not is_tree_still_there(bot, spot, tier.trees):
            live_here = bot.world.locs.get((spot.x, spot.z, bot.world.level))
            print("tree gone at %s live=%s" % ((spot.x, spot.z), live_here), flush=True)
            ban_until[(spot.x, spot.z)] = now + DEPLETE_BAN_TICKS
            spot = None
            continue
        result, gained = work_spot(bot, spot.x, spot.z, obj_loc_id(bot, spot, tier), log_ids, had_logs)
        had_logs += gained
        if gained > 0:
            idle_since = bot.world.ticks
        if result == "idle":
            print("tree quiet, next one", flush=True)
            ban_until[(spot.x, spot.z)] = now + DEPLETE_BAN_TICKS
            spot = None
        elif result == "noaxe":
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
            print("unknown dialog blocking chop, aborting load", flush=True)
            return False, bank, woodcutting_level
        if bot.world.ticks - idle_since > STALL_TICKS:
            print("stall: no logs for %d ticks" % STALL_TICKS, flush=True)
            return False, bank, woodcutting_level



def main():
    username = sys.argv[1] if len(sys.argv) > 1 else "t48"
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
        ensure_not_mazed(bot)
        side_bank = knowledge.comps.com_id("bank_side:inv")
        main_bank = knowledge.comps.com_id("bank_main:inv")
        side_shop = knowledge.comps.com_id("shop_template_side:inv")
        main_shop = knowledge.comps.com_id("shop_template:inv")
        coins_id = obj_id(bot, COINS)
        tiers = knowledge.all_tiers()
        woodcutting_level = learn_level(bot, 1)
        bank = nearest_free_bank(bot)
        if bank is None:
            raise SystemExit("no reachable bank from %s" % (bot.position,))
        print("bank at (%d,%d)" % (bank.x, bank.z), flush=True)
        current_axe = best_axe(bot)
        junk = [held for (held, _count) in pack_counts(bot).values() if held not in known_axe_ids(bot) and held != coins_id]
        log_ids = set()
        for tier in tiers:
            try:
                log_ids.add(obj_id(bot, tier.product))
            except KeyError:
                pass
        junk = [held for held in junk if held not in log_ids]
        bank_coins_known = 0
        if current_axe is None or junk:
            if current_axe is None and reattach(bot):
                print("ok reattached axe from pack, no bootstrap needed", flush=True)
                current_axe = best_axe(bot)
                junk = [held for (held, _count) in pack_counts(bot).values() if held not in known_axe_ids(bot) and held != coins_id]
                junk = [held for held in junk if held not in log_ids]
            keep = set(log_ids)
            if current_axe is not None:
                keep.add(axe_id(bot, current_axe))
            ok, bank_coins_known = bank_trip(bot, bank, side_bank, main_bank, keep, 0)
            if not ok:
                raise SystemExit("cleanup bank trip failed")
            if best_axe(bot) is None:
                if restore_from_bank(bot, bank, side_bank, main_bank):
                    current_axe = best_axe(bot)
                    print("bank at (%d,%d)" % (bank.x, bank.z), flush=True)
                else:
                    ok, bank_coins_known = bank_trip(bot, bank, side_bank, main_bank, keep, withdraw=-1, withdraw_logs=10, log_ids=tuple(log_ids))
                    if not ok:
                        raise SystemExit("funding bank trip failed")
                    need_cross_gate = False
                    gate = nearest_toll_gate(bot)
                    if gate is not None and bot.world.x >= gate[0]:
                        need_cross_gate = True
                    target = 16 + (10 if need_cross_gate else 0)
                    if pack_total(bot, coins_id) < target:
                        sell_for_target(bot, side_shop, coins_id, target - pack_total(bot, coins_id))
                    if need_cross_gate and not cross_toll_gate(bot):
                        raise SystemExit("cannot cross toll gate at %s" % (bot.position,))
                    if pack_total(bot, coins_id) >= 16 and buy_attempt(bot, main_shop, "bronze_axe"):
                        print("ok bootstrapped bronze axe", flush=True)
                        bank = nearest_free_bank(bot) or bank
                        print("bank at (%d,%d)" % (bank.x, bank.z), flush=True)
                    else:
                        raise SystemExit("cannot bootstrap: no axe, %d coins" % pack_total(bot, coins_id))
        done_loads = 0
        toolless_loads = 0
        while loads == 0 or done_loads < loads:
            ensure_not_mazed(bot)
            try:
                ok, bank, woodcutting_level = chop_load(bot, tiers, bank, coins_id, woodcutting_level, side_bank, main_bank, main_shop)
            except ConnectionError:
                print("disconnected, relogging", flush=True)
                bot.relogin()
                wait_backpack(bot, BACKPACK_COM)
                continue
            if not ok:
                raise SystemExit("chop failed at %s" % (bot.position,))
            candidates, cheapest = cheapest_upgrade(bot)
            if candidates:
                sell_for_target(bot, side_shop, coins_id, max(0, cheapest - bank_coins_known - pack_total(bot, coins_id)))
            current = best_axe(bot)
            keep = {axe_id(bot, current)} if current else set()
            # Pack is emptied into the bank during this same trip, so the
            # withdraw must cover the full price, not price minus the
            # about-to-be-deposited pack coins.
            withdraw = cheapest if candidates else 0
            ok, bank_coins_known = bank_trip(bot, bank, side_bank, main_bank, keep, withdraw)
            if not ok:
                raise SystemExit("bank trip failed")
            if candidates:
                for candidate in candidates:
                    if buy_attempt(bot, main_shop, candidate):
                        print("ok upgraded to %s" % candidate, flush=True)
                        break
            if best_axe(bot) is None:
                toolless_loads += 1
            else:
                toolless_loads = 0
            if toolless_loads >= 2:
                raise SystemExit("no axe and cannot rebuy, aborting at %s" % (bot.position,))
            done_loads += 1
            print("ok load %d banked (wc=%s)" % (done_loads, woodcutting_level), flush=True)
        return 0
    finally:
        bot.close()


if __name__ == "__main__":
    sys.exit(main())

if __name__ == "__main__":
    sys.exit(main())
