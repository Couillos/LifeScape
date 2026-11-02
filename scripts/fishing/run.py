"""Fisher: fishes the best spot for the player's level, banks full
loads, sells fish to fund tool and bait upgrades.

Adapts to any start position: bank, spots, general store and tool shop
are all picked by proximity from gamedata (best spot for the level,
safe for the combat level, nearest usable bank). Banks across the toll
gate are a last resort. Stays on one level."""

import sys
import time


from scripts.fishing.common import *
from scripts.fishing.pack import *
from scripts.fishing.walk import *
from scripts.fishing.spots import *
from scripts.fishing.gear import *
from scripts.fishing.shop import *
from scripts.fishing.bank import *

def work_spot(bot, tier, product_ids, had_fish):
    found = nearest_live(bot, tier, bot.world.x, bot.world.z)
    if found[0] is None:
        return "gone", 0
    _slot, spot_x, spot_z = found
    if max(abs(bot.world.x - spot_x), abs(bot.world.z - spot_z)) > 4:
        try:
            reached = walk_checked(bot, spot_x, spot_z, timeout=120)
        except (Unreachable, TimeoutError):
            return "unreachable", 0
        if not reached:
            return "unreachable", 0
        found = nearest_live(bot, tier, bot.world.x, bot.world.z)
        if found[0] is None:
            print("no live spot, waiting for respawns", flush=True)
            for _wait in range(14):
                wait_ticks(bot, 60)
                found = nearest_live(bot, tier, bot.world.x, bot.world.z)
                if found[0] is not None:
                    break
            if found[0] is None:
                return "gone", 0
        _slot, spot_x, spot_z = found
    if max(abs(bot.world.x - spot_x), abs(bot.world.z - spot_z)) > 2:
        neighbors = sorted(
            [(spot_x + dx, spot_z + dz) for dx in (-1, 0, 1) for dz in (-1, 0, 1) if dx or dz],
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
                if max(abs(bot.world.x - spot_x), abs(bot.world.z - spot_z)) > 4:
                    print("wandered off from %s, giving up" % ((spot_x, spot_z),), flush=True)
                    return "unreachable", 0
                continue
        else:
            return "unreachable", 0
    gained = 0
    seen_msgs = len(bot.world.messages)
    last_x, last_z = spot_x, spot_z
    slot, live_x, live_z = nearest_live(bot, tier, last_x, last_z)
    if slot is None:
        return "gone", 0
    # Single op: the server chains attempts itself while in range. Re-clicking
    # restarts the chain (stopAction), so never spam it; re-pick instead.
    print("click %s at %s" % (tier.action, (live_x, live_z)), flush=True)
    bot.intend("fishing %s" % tier.products[0])
    bot.op_npc(tier.op, slot)
    seen_msgs = len(bot.world.messages)
    idle_since = bot.world.ticks
    while True:
        bot.pump(2)
        if hazards.avoid(bot):
            return "threat", gained
        if bot.world.modal is not None:
            if not dismiss_modal(bot):
                texts = [text for _com, text in bot.chat_texts()][-4:]
                print("modal %s texts=%s" % (bot.world.modal, [text[:60] for text in texts]), flush=True)
                randomevents.greet(bot, timeout=20)
                bot.pump(10)
                if bot.world.modal is not None and not dismiss_modal(bot):
                    return "blocked", gained
            for message in bot.world.messages[seen_msgs:]:
                lowered = message.lower()
                if "can't reach that" in lowered:
                    return "unreachable", gained
                if "need at least" in lowered and "fishing" in lowered:
                    return "level", gained
                if "need a " in lowered and ("to catch" in lowered or "to bait" in lowered or "to lure" in lowered):
                    return "notool", gained
                if "don't have any" in lowered and "left" in lowered:
                    return "nobait", gained
                if "can't carry any more" in lowered:
                    return "full", gained
        slot, live_x, live_z = nearest_live(bot, tier, last_x, last_z)
        if slot is None:
            return "gone", gained
        if max(abs(live_x - last_x), abs(live_z - last_z)) > 6:
            return "moved", gained
        now_have = sum(pack_total(bot, product_id) for product_id in product_ids)
        if now_have > had_fish + gained:
            gained = now_have - had_fish
            idle_since = bot.world.ticks
        if bot.world.ticks - idle_since > 60:
            return "gone", gained



def fish_load(bot, tiers, bank, coins_id, fishing_level, side_bank, main_bank, side_shop, main_shop):
    product_ids = {}
    for tier in tiers:
        for product in tier.products:
            try:
                product_ids[product] = obj_id(bot, product)
            except KeyError:
                continue
    ban_until = {}
    walk_hurts = 0
    bait_ban_until = 0
    bait_ids = {}
    for tier in tiers:
        if tier.bait:
            try: bait_ids[tier.bait] = obj_id(bot, tier.bait)
            except KeyError: pass
    idle_since = bot.world.ticks
    hitpoints_seen = hitpoints(bot)
    last_hitpoints = hitpoints_seen
    flees = 0
    spawn = None
    tier = None
    while True:
        if not randomevents.check(bot):
            raise SystemExit("random event unresolved at %s" % (bot.position,))
        coins = pack_total(bot, coins_id)
        if free_pack_slots(bot) <= (0 if coins > 0 else 1):
            return True, bank, fishing_level
        fishing_level = learn_level(bot, fishing_level)
        best_tier = knowledge.best_fish_tier(fishing_level)
        if spawn is not None and tier is not None and best_tier is not None and tier.level < best_tier.level:
            print("level up, switching tier", flush=True)
            spawn = None
        hitpoints_seen = hitpoints(bot)
        if hitpoints_seen <= 0:
            print("died, respawning", flush=True)
            bot.intend("died, respawning")
            hazards.wait_respawn(bot)
            last_hitpoints = hitpoints(bot)
            spawn = None
            tier = None
            continue
        hurt_now = hitpoints_seen < last_hitpoints
        last_hitpoints = hitpoints_seen
        if hurt_now:
            flees += 1
            print("hurt, fleeing", flush=True)
            bot.intend("fleeing a hunter")
            if spawn is not None:
                ban_until[(spawn.x, spawn.z)] = bot.world.ticks + FLEE_BAN_TICKS
            if flees > MAX_FLEES_PER_LOAD:
                print("too many attacks, aborting load", flush=True)
                return False, bank, fishing_level
            if not compass_escape(bot):
                retreat_along_trail(bot)
            wait_hitpoints(bot, hitpoints(bot) + 1, timeout=200)
            spawn = None
            tier = None
            continue
        now = bot.world.ticks
        banned = set(key for key, until in ban_until.items() if until > now)
        usable = [tier for tier in tiers if tier.bait is None or now >= bait_ban_until or pack_total(bot, bait_ids.get(tier.bait, -1)) > 0]
        if spawn is None:
            if abs(bot.world.x - bank.x) + abs(bot.world.z - bank.z) > 60:
                try:
                    walk_checked(bot, bank.x, bank.z, timeout=300)
                except (Unreachable, TimeoutError):
                    pass
            for _wait in range(16):
                tier, spawn, use_bank = pick_fish_spot(
                    usable, fishing_level, banned, bot.combat_level(), bot.world.x, bot.world.z, bot.world.level,
                    pinned_bank=bank, bot=bot, relaxed=_wait >= 8,
                )
                if tier is not None and spawn is not None:
                    break
                print("no fish spot, waiting for respawns", flush=True)
                bot.intend("waiting for a safe fishing spot")
                wait_ticks(bot, 15)
            if tier is None or spawn is None:
                print("no fish spot left", flush=True)
                return False, bank, fishing_level
            bank = use_bank
            gear = ensure_gear(bot, tier, side_shop, main_shop, coins_id, side_bank, main_bank, bank)
            if gear == "nobait":
                print("bait broke, dropping to net tier", flush=True)
                bait_ban_until = bot.world.ticks + 1000
                spawn = None
                tier = None
                continue
            if not gear:
                print("no gear for %s pack=%s at %s" % (tier.tool, sorted(pack_counts(bot).values()), bot.position), flush=True)
                return False, bank, fishing_level
            print("fishing %s at %s (%s) pack=%s" % (tier.products[0], (spawn.x, spawn.z), bot.position, sorted(pack_counts(bot).values())), flush=True)
            bot.intend("walking to the fishing spot at (%d,%d)" % (spawn.x, spawn.z))
            try:
                if not walk_checked(bot, spawn.x, spawn.z, timeout=300):
                    wait_hitpoints(bot, hitpoints(bot) + 1, timeout=200)
                    walk_hurts += 1
                    if walk_hurts >= 3:
                        ban_until[(spawn.x, spawn.z)] = bot.world.ticks + 500
                        walk_hurts = 0
                    spawn = None
                    tier = None
                    continue
            except (Unreachable, TimeoutError) as walk_error:
                print("spot walk failed: %s" % (walk_error,), flush=True)
                ban_until[(spawn.x, spawn.z)] = bot.world.ticks + SPOT_GONE_BAN_TICKS
                spawn = None
                tier = None
                continue
        wanted = [product_ids[product] for product in tier.products if product in product_ids]
        had_fish = sum(pack_total(bot, product_id) for product_id in wanted)
        result, gained = work_spot(bot, tier, wanted, had_fish)
        if gained > 0: idle_since, walk_hurts = bot.world.ticks, 0
        if result == "full":
            return True, bank, fishing_level
        if result == "moved":
            print("spot moved, re-scanning live", flush=True)
            spawn = None
            tier = None
            continue
        if result in ("idle", "gone"):
            print("spot quiet, next one", flush=True)
            if spawn is not None:
                ban_until[(spawn.x, spawn.z)] = now + SPOT_GONE_BAN_TICKS
            spawn = None
            tier = None
        elif result == "notool":
            spawn = None
            tier = None
        elif result == "threat":
            print("hunter close, re-picking a spot", flush=True)
            if spawn is not None:
                ban_until[(spawn.x, spawn.z)] = now + SPOT_GONE_BAN_TICKS
            spawn = None
            tier = None
        elif result == "nobait":
            if restock_bait(bot, tier, side_shop, main_shop, coins_id, side_bank, main_bank, bank):
                print("bait restocked, resuming", flush=True)
                continue
            print("bait out, dropping to net tier", flush=True)
            bait_ban_until = bot.world.ticks + 1000
            spawn = None
            tier = None
            if free_pack_slots(bot) <= 0:
                return True, bank, fishing_level
            continue
        elif result in ("unreachable", "level"):
            if spawn is not None:
                ban_until[(spawn.x, spawn.z)] = now + 500
            spawn = None
            tier = None
        elif result == "blocked":
            print("unknown dialog blocking fish, aborting load", flush=True)
            return False, bank, fishing_level
        if bot.world.ticks - idle_since > STALL_TICKS:
            print("stall: no fish for %d ticks" % STALL_TICKS, flush=True)
            return False, bank, fishing_level



def main():
    username = sys.argv[1] if len(sys.argv) > 1 else "t46"
    loads = int(sys.argv[2]) if len(sys.argv) > 2 else 0
    bot = Bot()
    try:
        bot.login(username, username)
        start_ticks = bot.world.ticks
        last_greet = bot.world.ticks
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
        tiers = knowledge.all_fish_tiers()
        fishing_level = learn_level(bot, 1)
        bank = nearest_free_bank(bot)
        if bank is None:
            raise SystemExit("no reachable bank from %s" % (bot.position,))
        print("bank at (%d,%d)" % (bank.x, bank.z), flush=True)
        current_tool = best_tool(bot)
        junk = [held for (held, _count) in pack_counts(bot).values() if held not in known_tool_ids(bot) and held != coins_id]
        bank_coins_known = 0
        bait_held = [tier for tier in tiers if tier.bait and pack_total(bot, obj_id(bot, tier.bait)) > 0]
        if current_tool is None or junk:
            keep = set()
            net_price = obj_price(bot, "net")
            if current_tool:
                try:
                    keep.add(obj_id(bot, current_tool))
                except KeyError:
                    pass
            for tier in bait_held:
                try:
                    keep.add(obj_id(bot, tier.bait))
                except KeyError:
                    continue
            withdraw = max(0, net_price - pack_total(bot, coins_id)) if current_tool is None else 0
            if current_tool is None and withdraw > 0 and junk:
                # No coins for the net: sell expendable junk first.
                sell_junk(bot, side_shop, coins_id, keep | known_tool_ids(bot), net_price)
                withdraw = max(0, net_price - pack_total(bot, coins_id))
            ok, bank_coins_known = bank_trip(bot, bank, side_bank, main_bank, keep, withdraw)
            if not ok:
                raise SystemExit("cleanup bank trip failed")
            if best_tool(bot) is None:
                if pack_total(bot, coins_id) >= net_price and buy_tool(bot, main_shop, "net"):
                    print("ok bootstrapped net", flush=True)
                else:
                    raise SystemExit("cannot bootstrap: no tool, %d coins" % pack_total(bot, coins_id))
        done_loads = 0
        while loads == 0 or done_loads < loads:
            ensure_not_mazed(bot)
            try:
                ok, bank, fishing_level = fish_load(bot, tiers, bank, coins_id, fishing_level, side_bank, main_bank, side_shop, main_shop)
            except ConnectionError:
                print("disconnected, relogging", flush=True)
                bot.relogin()
                wait_backpack(bot, BACKPACK_COM)
                continue
            if not ok:
                raise SystemExit("fish failed at %s" % (bot.position,))
            candidates, cheapest = cheapest_upgrade(bot, fishing_level)
            if candidates:
                sell_for_target(bot, side_shop, coins_id, max(0, cheapest - bank_coins_known))
            keep = {held for (held, _count) in pack_counts(bot).values() if held in known_tool_ids(bot)}
            for tier in tiers:
                if not tier.bait:
                    continue
                try:
                    if pack_total(bot, obj_id(bot, tier.bait)) > 0:
                        keep.add(obj_id(bot, tier.bait))
                except KeyError:
                    continue
            withdraw = cheapest if candidates else 0
            ok, bank_coins_known = bank_trip(bot, bank, side_bank, main_bank, keep, withdraw)
            if not ok:
                raise SystemExit("bank trip failed")
            if candidates:
                for candidate in candidates:
                    if buy_tool(bot, main_shop, candidate):
                        print("ok upgraded to %s" % candidate, flush=True)
                        break
            done_loads += 1
            print("ok load %d banked (fishing=%s)" % (done_loads, fishing_level), flush=True)
        return 0
    finally:
        bot.close()


if __name__ == "__main__":
    sys.exit(main())

if __name__ == "__main__":
    sys.exit(main())
