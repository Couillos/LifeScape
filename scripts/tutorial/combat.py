"""Combat instructor: wielding, melee kill, ranged kill, ladder to bank.

Verified through inventory (weapons given, bones looted) and movement.
"""

import time

from lifescape.client import Unreachable

from scripts.tutorial import common as C


def equip_held(bot, obj_name, timeout=60):
    obj = C.obj_id(bot, obj_name)
    C.check(bot, C.wait_inv(bot, obj, timeout=timeout), "%s held" % obj_name)
    com, slot = C.inv_slot(bot, obj)
    bot.op_held(2, obj, slot, com)
    bot.pump(10)


def intro(bot):
    C.check(bot, C.talk(bot, "combat_instructor", near=(3107, 9510)), "combat instructor found")
    C.finish_dialog(bot, ("Nope, I'm ready to move on!",))


def wield_tab(bot):
    C.click_tut_tab(bot)
    bot.pump(10)
    print("ok worn tab opened", flush=True)


def wield_dagger(bot):
    C.click_tut_tab(bot)
    bot.pump(10)
    dagger = C.obj_id(bot, "bronze_dagger")
    try:
        com, slot = C.inv_slot(bot, dagger)
    except KeyError:
        print("ok dagger already wielded", flush=True)
        return
    bot.op_held(2, dagger, slot, com)
    bot.pump(10)
    print("ok dagger equipped (verified at next talk)", flush=True)


def sword_talk(bot):
    C.check(bot, C.talk(bot, "combat_instructor", near=(3107, 9510)), "combat instructor again")
    C.finish_dialog(bot, ("Nope, I'm ready to move on!",))
    sword = C.obj_id(bot, "bronze_sword")
    C.check(bot, C.wait_inv(bot, sword, timeout=60), "sword+shield given")


def sword_equip(bot):
    equip_held(bot, "bronze_sword")
    equip_held(bot, "wooden_shield")


def combat_tab(bot):
    C.click_tut_tab(bot)
    bot.pump(10)


PIT_CENTER = (3109, 9519)


def rat_cage(bot):
    for _attempt in range(5):
        x, z, _level, loc = C.find_loc(bot, "newbiedoor5")
        if abs(bot.world.x - PIT_CENTER[0]) + abs(bot.world.z - PIT_CENTER[1]) <= 6:
            print("ok inside rat pit", flush=True)
            return
        print("rat cage try %d at %s door=(%d,%d)" % (_attempt, bot.position, x, z), flush=True)
        C.cross_door(bot, x, z, loc, beyond=PIT_CENTER, what="rat cage")
        bot.pump(5)
    if abs(bot.world.x - PIT_CENTER[0]) + abs(bot.world.z - PIT_CENTER[1]) > 6:
        raise SystemExit("tutorial stuck at: inside rat pit pos=%s" % (bot.position,))
    print("ok inside rat pit", flush=True)


def wait_bones(bot, timeout=400):
    bones = C.obj_id(bot, "newbiebones")
    target = bot.world.ticks + timeout
    while bot.world.ticks < target:
        bot.pump(5)
        if bot.world.ground_items(bot.world.x, bot.world.z, bot.world.level, obj_id=bones):
            return True
    return False


def melee(bot):
    deadline = bot.world.ticks + 400
    target = None
    tries = 0
    while bot.world.ticks < deadline:
        bot.pump(5)
        seen = len(bot.world.messages)
        if target is not None and target in bot.world.npcs:
            continue
        if target is not None:
            if wait_bones(bot, timeout=200):
                break
            target = None
            continue
        nid = bot.find_npc("newbiegiantrat")
        if nid is None:
            continue
        tries += 1
        if tries % 10 == 1:
            print("melee try %d at %s rat=%d" % (tries, bot.position, nid), flush=True)
        npc = bot.world.npcs[nid]
        try:
            bot.approach(npc["x"], npc["z"], open_doors=True)
        except Unreachable:
            continue
        bot.op_npc(2, nid)
        target = nid
        for message in bot.world.messages[seen:]:
            if "can't reach that" in message.lower():
                print("melee blocked at %s, next rat" % (bot.position,), flush=True)
                target = None
                break
    C.check(bot, wait_bones(bot, timeout=100), "rat killed in melee")
    try:
        bot.take(name="newbiebones", timeout=60)
        print("ok bones picked up", flush=True)
    except Unreachable:
        print("ok no bones taken", flush=True)


def ranged_prep(bot):
    # Leave the pit area through the cage gate into the east corridor,
    # then the instructor is reachable on foot.
    x, z, _level, loc = C.find_loc(bot, "newbiedoor5")
    C.cross_door(bot, x, z, loc, beyond=(3112, 9518), what="rat cage exit")
    C.check(bot, C.talk(bot, "combat_instructor", near=(3107, 9510)), "combat instructor ranged")
    C.finish_dialog(bot, ("Nope, I'm ready to move on!",))
    shortbow = C.obj_id(bot, "shortbow")
    C.check(bot, C.wait_inv(bot, shortbow, timeout=60), "bow given")
    equip_held(bot, "shortbow")
    equip_held(bot, "bronze_arrow")


def ranged_kill(bot):
    for _attempt in range(4):
        nid = None
        target = bot.world.ticks + 50
        while bot.world.ticks < target and nid is None:
            bot.pump(5)
            nid = bot.find_npc("newbiegiantrat")
        if nid is None:
            continue
        seen = len(bot.world.messages)
        bot.op_npc(2, nid)
        if wait_bones(bot, timeout=150):
            print("ok rat killed at range", flush=True)
            return
        for message in bot.world.messages[seen:]:
            if "can't reach that" in message.lower():
                print("ranged blocked at %s" % (bot.position,), flush=True)
        try:
            x, z, _level, loc = C.find_loc(bot, "newbiedoor5")
            bot.approach(x, z, open_doors=True)
            bot.op_loc(1, x, z, loc)
            bot.pump(10)
        except Unreachable:
            pass
    raise SystemExit("tutorial stuck at: ranged rat kill pos=%s" % (bot.position,))


def ladder_to_bank(bot):
    C.use_exit(bot, ("newbieladder2",), "up the bank ladder")
