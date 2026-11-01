"""Bank, chapel and magic: bank booth, advisor, prayers, wind strike
and the teleport to the mainland. Verified through modals closing,
inventory and the final teleport.
"""

import time

from scripts.tutorial import common as C


def bank_booth(bot):
    # Talk to the banker himself: clicking the booth only opens his
    # dialog when he happens to stand within 2 tiles, otherwise the
    # bank opens *without* advancing the tutorial (infinite repeat).
    if not C.talk(bot, "banker"):
        x, z, _level, loc = C.find_loc(bot, "newbiebankbooth")
        bot.approach(x, z, open_doors=True)
        bot.op_loc(1, x, z, loc)
        bot.pump(10)
    if bot.world.modal:
        C.finish_dialog(bot, ("Yes.",))
    bot.close_modal()
    bot.pump(5)
    C.check(bot, C.wait_modal_closed(bot, timeout=60), "bank visited")


def bank_door(bot):
    C.check(bot, C.pass_door(bot, "newbie_door6", beyond=(3127, 3124)), "through bank door")


def advisor_talk(bot):
    C.check(bot, C.talk(bot, "account_instructor", near=(3127, 3124)), "advisor found")
    C.finish_dialog(bot, ("No thanks.",))


def advisor_door(bot):
    C.check(bot, C.pass_door(bot, "newbie_door7", beyond=(3132, 3124)), "through advisor door")


def chapel_path(bot):
    C.check(bot, C.talk(bot, "brother_noob", near=(3125, 3106)), "brother found")
    C.finish_dialog(bot, ("Nope, I'm ready to move on!",))


def prayer_tab(bot):
    C.click_tut_tab(bot)
    bot.pump(10)
    print("ok prayer tab opened", flush=True)


def chapel_recap(bot):
    C.check(bot, C.talk(bot, "brother_noob", near=(3125, 3106)), "brother prayer list")
    C.finish_dialog(bot, ("Nope, I'm ready to move on!",))
    try:
        bones = C.obj_id(bot, "newbiebones")
        if bones in bot.inventory():
            com, slot = C.inv_slot(bot, bones)
            bot.op_held(1, bones, slot, com)
            bot.pump(10)
            print("ok bones buried", flush=True)
    except KeyError:
        pass


def friends_tab(bot):
    C.click_tut_tab(bot)
    bot.pump(10)
    print("ok friends tab opened", flush=True)


def ignore_tab(bot):
    C.click_tut_tab(bot)
    bot.pump(10)
    print("ok ignore tab opened", flush=True)


def chapel_final(bot):
    C.check(bot, C.talk(bot, "brother_noob", near=(3125, 3106)), "brother final")
    C.finish_dialog(bot, ("Nope, I'm ready to move on!",))


def chapel_door(bot):
    C.check(bot, C.pass_door(bot, "newbie_door8", beyond=(3121, 3098)), "out of chapel")


def terrova_talk(bot):
    C.check(bot, C.talk(bot, "magic_instructor", near=(3141, 3089)), "terrova found")
    C.finish_dialog(bot, ("Yes.",))


def wind_strike(bot, timeout=250):
    air = C.obj_id(bot, "airrune")
    mind = C.obj_id(bot, "mindrune")
    try:
        bot.walk_to(3141, 3089, bot.world.level, timeout=60, open_doors=True)
    except Exception:
        pass
    target = bot.world.ticks + timeout
    while bot.world.ticks < target:
        if bot.inventory().get(air, 0) < 1 or bot.inventory().get(mind, 0) < 1:
            terrova_runes(bot)
            continue
        nid = None
        inner = bot.world.ticks + 50
        while bot.world.ticks < inner and nid is None:
            bot.pump(5)
            nid = bot.find_npc("newbiechicken")
        if nid is None:
            continue
        npc = bot.world.npcs.get(nid)
        if npc is None:
            continue
        try:
            bot.approach(npc["x"], npc["z"], radius=5, open_doors=True)
        except Exception:
            pass
        seen = len(bot.world.messages)
        C.cast_on_npc(bot, nid)
        inner = bot.world.ticks + 20
        while bot.world.ticks < inner:
            bot.pump(2)
            if nid not in bot.world.npcs:
                print("ok chicken killed with magic", flush=True)
                return True
            for message in bot.world.messages[seen:]:
                if "can't reach that" in message.lower():
                    print("magic blocked at %s, next chicken" % (bot.position,), flush=True)
                    inner = 0
                    break
    return False


def magic_tab(bot):
    C.click_tut_tab(bot)
    bot.pump(10)
    # The tab click lands on a transient stage with no instruction text;
    # talking right away hands out runes and moves to wind strike.
    terrova_runes(bot)
    print("ok magic tab opened", flush=True)


def terrova_runes(bot):
    air = C.obj_id(bot, "airrune")
    mind = C.obj_id(bot, "mindrune")
    if bot.inventory().get(air, 0) >= 1 and bot.inventory().get(mind, 0) >= 1:
        print("ok runes already held", flush=True)
        return
    C.check(bot, C.talk(bot, "magic_instructor", near=(3141, 3089)), "terrova runes")
    C.finish_dialog(bot, ("Yes.",))
    C.check(bot, C.wait_inv(bot, air, timeout=60), "runes held")


def final(bot):
    if bot.world.x > 3200:
        print("ok already on the mainland", flush=True)
        return
    start = (bot.world.x, bot.world.z, bot.world.level)
    C.check(bot, C.talk(bot, "magic_instructor", near=(3141, 3089)), "terrova final")
    C.finish_dialog(bot, ("Yes.",))
    target = bot.world.ticks + 100
    while bot.world.ticks < target:
        bot.pump(5)
        if (bot.world.x, bot.world.z, bot.world.level) != start:
            print("ok teleported to mainland", flush=True)
            return
    C.check(bot, False, "teleported to mainland")
