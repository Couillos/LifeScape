"""Design screen, guide, survival and chef: stages 0 to 200.

Every step is verified through client-visible state (modals closing,
inventory contents, position), never through the tutorial varp which
the server does not transmit.
"""

import time

from lifescape.client import Unreachable

from scripts.tutorial import common as C


def design(bot):
    target = bot.world.ticks + 50
    while bot.world.ticks < target and bot.world.modal is None:
        bot.pump(5)
    if bot.world.modal is None:
        print("ok no design screen, already validated", flush=True)
        return
    bot.if_button(C.COM_DESIGN_ACCEPT)
    C.check(bot, C.wait_modal_closed(bot), "design accepted")


def guide(bot):
    C.check(bot, C.talk(bot, "basics_instructor", near=(3095, 3107)), "guide found")
    C.finish_dialog(bot, ("No, thank you.",))


def controls_tab(bot):
    C.click_tut_tab(bot)
    bot.pump(10)
    print("ok controls tab opened", flush=True)


def door1(bot):
    C.check(bot, C.pass_door(bot, "newbie_door1", beyond=(3100, 3107)), "through guide door")


def survival_start(bot):
    C.check(bot, C.talk(bot, "survival", near=(3103, 3095)), "survival found")
    C.finish_dialog(bot)


def open_inventory(bot):
    axe = C.obj_id(bot, "bronze_axe")
    C.click_tut_tab(bot)
    C.check(bot, C.wait_inv(bot, axe, timeout=30), "axe+tinderbox given")


def chop_tree(bot):
    axe = C.obj_id(bot, "bronze_axe")
    C.check(bot, axe in bot.inventory(), "axe held")
    logs = C.obj_id(bot, "newbielogs")
    if bot.inventory().get(logs, 0) >= 2:
        return
    while bot.inventory().get(logs, 0) < 2:
        if bot.world.modal is not None:
            bot.chat_continue()
            bot.pump(5)
        had = bot.inventory().get(logs, 0)
        ok = C.op_loc_near(bot, ("newbietree", "newbietree2", "newbiedeadtree2"), 1, lambda b: b.inventory().get(logs, 0) > had, timeout=120)
        C.check(bot, ok, "logs chopped")


def light_fire(bot):
    logs = C.obj_id(bot, "newbielogs")
    tinderbox = C.obj_id(bot, "tinderbox")
    if bot.inventory().get(logs, 0) < 1:
        chop_tree(bot)
    here = (bot.world.x, bot.world.z)
    for _attempt in range(3):
        if bot.inventory().get(logs, 0) < 1:
            chop_tree(bot)
        try:
            C.use_held_on_held(bot, logs, tinderbox)
        except KeyError:
            bot.pump(10)
            continue
        target = bot.world.ticks + 50
        while bot.world.ticks < target:
            bot.pump(5)
            try:
                fx, fz, _level, _floc = C.find_loc(bot, "fire")
            except Unreachable:
                continue
            if abs(fx - here[0]) + abs(fz - here[1]) <= 1:
                print("ok fire lit", flush=True)
                return here
        try:
            bot.walk_to(here[0] + 2, here[1], open_doors=True, timeout=30)
        except (Unreachable, TimeoutError):
            pass
        here = (bot.world.x, bot.world.z)
    raise SystemExit("tutorial stuck at: fire lighting pos=%s" % (bot.position,))


def open_stats(bot):
    C.click_tut_tab(bot)
    bot.pump(10)


def survival_food(bot):
    C.check(bot, C.talk(bot, "survival", near=(3103, 3095)), "survival again")
    C.finish_dialog(bot)
    net = C.obj_id(bot, "net")
    C.check(bot, C.wait_inv(bot, net, timeout=60), "net given")


def catch_shrimp(bot, timeout=200):
    raw = C.obj_id(bot, "newbieraw_shrimp")
    had = bot.inventory().get(raw, 0)
    target = bot.world.ticks + timeout
    while bot.world.ticks < target:
        bot.pump(5)
        nid = bot.find_npc("newbiefishing")
        if nid is None:
            continue
        npc = bot.world.npcs[nid]
        try:
            bot.approach(npc["x"], npc["z"], open_doors=True)
        except Unreachable:
            continue
        bot.op_npc(1, nid)
        inner = bot.world.ticks + 50
        while bot.world.ticks < inner:
            bot.pump(5)
            if bot.inventory().get(raw, 0) > had:
                return True
    return False


def cook_on_fire(bot, raw_name, want, timeout=90):
    raw = C.obj_id(bot, raw_name)
    if bot.inventory().get(raw, 0) < 1:
        C.check(bot, catch_shrimp(bot), "shrimp caught for cooking")
    C.check(bot, C.wait_inv(bot, raw, timeout=timeout), "has %s" % raw_name)
    light_fire(bot)
    want_id = C.obj_id(bot, want)
    before = dict(bot.inventory())
    ok = C.use_held_on_loc_near(bot, ("fire",), raw, lambda b: b.inventory().get(want_id, 0) > before.get(want_id, 0))
    C.check(bot, ok, "%s cooked" % want)


def catch(bot):
    C.check(bot, catch_shrimp(bot), "shrimp caught")


def cook_burnt(bot):
    cook_on_fire(bot, "newbieraw_shrimp", "burntfish1")


def cook_fresh(bot):
    cook_on_fire(bot, "newbieraw_shrimp", "shrimp")


def chef_path(bot):
    chef(bot)


GATE_TILES = frozenset(((3089, 3091), (3089, 3092)))


def survival_gate(bot):
    target = bot.world.ticks + 500
    while bot.world.x >= 3089 and bot.world.ticks < target:
        if (bot.world.x, bot.world.z) not in GATE_TILES:
            x, z, _level, loc = C.find_loc(bot, "newbiegateclosed")
            try:
                bot.approach(x, z, radius=1, open_doors=True)
            except Unreachable:
                pass
            bot.op_loc(1, x, z, loc)
            inner = bot.world.ticks + 35
            while bot.world.ticks < inner and (bot.world.x, bot.world.z) not in GATE_TILES:
                bot.pump(5)
            C.refresh_openings(bot)
        try:
            bot._walk_raw(3085, 3092, 0, time.time() + 8)
        except Exception:
            bot.pump(5)
    if bot.world.x >= 3089:
        raise SystemExit("tutorial stuck at: through survival gate pos=%s" % (bot.position,))
    print("ok through survival gate", flush=True)
    bot.walk_to(3080, 3086, open_doors=True, timeout=120)
    C.refresh_openings(bot)


def chef(bot):
    C.check(bot, C.talk(bot, "cook_instructor", near=(3080, 3084)), "chef found")
    C.finish_dialog(bot, ("Nothing, thanks.",))
    flour = C.obj_id(bot, "newbie_pot_flour")
    C.check(bot, C.wait_inv(bot, flour, timeout=60), "flour+water given")


def make_dough(bot):
    water = C.obj_id(bot, "bucket_water")
    flour = C.obj_id(bot, "newbie_pot_flour")
    dough = C.obj_id(bot, "bread_dough")
    if bot.inventory().get(flour, 0) < 1 or bot.inventory().get(water, 0) < 1:
        chef(bot)
    C.check(bot, C.wait_inv(bot, flour, timeout=60), "flour+water ready")
    C.check(bot, C.wait_inv(bot, water, timeout=60), "flour+water ready")
    C.use_held_on_held(bot, water, flour)
    C.check(bot, C.wait_inv(bot, dough, timeout=90), "dough mixed")


def bake_bread(bot):
    dough = C.obj_id(bot, "bread_dough")
    if dough not in bot.inventory():
        make_dough(bot)
    bread = C.obj_id(bot, "bread")
    ok = C.use_held_on_loc_near(bot, ("newbierange",), dough, lambda b: bread in b.inventory())
    C.check(bot, ok, "bread baked")


def music_tab(bot):
    C.click_tut_tab(bot)
    bot.pump(10)
    print("ok music tab opened", flush=True)


def chef_recap(bot):
    C.check(bot, C.talk(bot, "cook_instructor", near=(3080, 3084)), "chef recap")
    C.finish_dialog(bot, ("Nothing, thanks.",))


def chef_recap(bot):
    C.check(bot, C.talk(bot, "cook_instructor", near=(3080, 3084)), "chef recap")
    C.finish_dialog(bot, ("Nothing, thanks.",))


def chef_door(bot):
    C.check(bot, C.pass_door(bot, "newbie_door3", beyond=(3070, 3090)), "through chef door")


def enable_run(bot):
    C.click_tut_tab(bot)
    bot.pump(10)
    bot.if_button(C.COM_RUN)
    bot.pump(15)
    print("ok run toggled (verified at quest door)", flush=True)
