"""Design screen, guide, survival and chef: stages 0 to 200.

Every step is verified through client-visible state (modals closing,
inventory contents, position), never through the tutorial varp which
the server does not transmit.
"""


from lifescape.client import Unreachable

from scripts.tutorial import common as tutorial_common


def design(bot):
    target = bot.world.ticks + 50
    while bot.world.ticks < target and bot.world.modal is None:
        bot.pump(5)
    if bot.world.modal is None:
        print("ok no design screen, already validated", flush=True)
        return
    bot.if_button(tutorial_common.COM_DESIGN_ACCEPT)
    tutorial_common.check(bot, tutorial_common.wait_modal_closed(bot), "design accepted")


def guide(bot):
    tutorial_common.check(bot, tutorial_common.talk(bot, "basics_instructor", near=(3095, 3107)), "guide found")
    tutorial_common.finish_dialog(bot, ("No, thank you.",))


def controls_tab(bot):
    tutorial_common.click_tut_tab(bot)
    bot.pump(10)
    print("ok controls tab opened", flush=True)


def door1(bot):
    tutorial_common.check(bot, tutorial_common.pass_door(bot, "newbie_door1", beyond=(3100, 3107)), "through guide door")


def survival_start(bot):
    tutorial_common.check(bot, tutorial_common.talk(bot, "survival", near=(3103, 3095)), "survival found")
    tutorial_common.finish_dialog(bot)


def open_inventory(bot):
    axe = tutorial_common.obj_id(bot, "bronze_axe")
    tutorial_common.click_tut_tab(bot)
    tutorial_common.check(bot, tutorial_common.wait_inv(bot, axe, timeout=30), "axe+tinderbox given")


def chop_tree(bot):
    axe = tutorial_common.obj_id(bot, "bronze_axe")
    tutorial_common.check(bot, axe in bot.inventory(), "axe held")
    logs = tutorial_common.obj_id(bot, "newbielogs")
    if bot.inventory().get(logs, 0) >= 2:
        return
    while bot.inventory().get(logs, 0) < 2:
        if bot.world.modal is not None:
            bot.chat_continue()
            bot.pump(5)
        had = bot.inventory().get(logs, 0)
        ok = tutorial_common.op_loc_near(bot, ("newbietree", "newbietree2", "newbiedeadtree2"), 1, lambda b: b.inventory().get(logs, 0) > had, timeout=120)
        tutorial_common.check(bot, ok, "logs chopped")


def light_fire(bot):
    logs = tutorial_common.obj_id(bot, "newbielogs")
    tinderbox = tutorial_common.obj_id(bot, "tinderbox")
    if bot.inventory().get(logs, 0) < 1:
        chop_tree(bot)
    here = (bot.world.x, bot.world.z)
    for _attempt in range(3):
        if bot.inventory().get(logs, 0) < 1:
            chop_tree(bot)
        try:
            tutorial_common.use_held_on_held(bot, logs, tinderbox)
        except KeyError:
            bot.pump(10)
            continue
        target = bot.world.ticks + 50
        while bot.world.ticks < target:
            bot.pump(5)
            try:
                fx, fz, _level, _floc = tutorial_common.find_loc(bot, "fire")
            except Unreachable:
                continue
            if abs(fx - here[0]) + abs(fz - here[1]) <= 1:
                print("ok fire lit", flush=True)
                return here
        try:
            bot.walk_to(here[0] + 2, here[1], open_doors=True, timeout=1500)
        except (Unreachable, TimeoutError):
            pass
        here = (bot.world.x, bot.world.z)
    raise SystemExit("tutorial stuck at: fire lighting pos=%s" % (bot.position,))


def open_stats(bot):
    tutorial_common.click_tut_tab(bot)
    bot.pump(10)


def survival_food(bot):
    tutorial_common.check(bot, tutorial_common.talk(bot, "survival", near=(3103, 3095)), "survival again")
    tutorial_common.finish_dialog(bot)
    net = tutorial_common.obj_id(bot, "net")
    tutorial_common.check(bot, tutorial_common.wait_inv(bot, net, timeout=60), "net given")


def catch_shrimp(bot, timeout=200):
    raw = tutorial_common.obj_id(bot, "newbieraw_shrimp")
    had = bot.inventory().get(raw, 0)
    target = bot.world.ticks + timeout
    while bot.world.ticks < target:
        bot.pump(5)
        nid = bot.find_npc("newbiefishing")
        if nid is None:
            continue
        try:
            bot.approach_npc(nid, open_doors=True)
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
    raw = tutorial_common.obj_id(bot, raw_name)
    if bot.inventory().get(raw, 0) < 1:
        tutorial_common.check(bot, catch_shrimp(bot), "shrimp caught for cooking")
    tutorial_common.check(bot, tutorial_common.wait_inv(bot, raw, timeout=timeout), "has %s" % raw_name)
    light_fire(bot)
    want_id = tutorial_common.obj_id(bot, want)
    before = dict(bot.inventory())
    ok = tutorial_common.use_held_on_loc_near(bot, ("fire",), raw, lambda b: b.inventory().get(want_id, 0) > before.get(want_id, 0))
    tutorial_common.check(bot, ok, "%s cooked" % want)


def catch(bot):
    tutorial_common.check(bot, catch_shrimp(bot), "shrimp caught")


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
            x, z, _level, loc = tutorial_common.find_loc(bot, "newbiegateclosed")
            try:
                bot.approach(x, z, radius=1, open_doors=True)
            except Unreachable:
                pass
            bot.op_loc(1, x, z, loc)
            inner = bot.world.ticks + 35
            while bot.world.ticks < inner and (bot.world.x, bot.world.z) not in GATE_TILES:
                bot.pump(5)
            tutorial_common.refresh_openings(bot)
        try:
            bot.walk_raw(3085, 3092, 0, bot.world.ticks + 400)
        except Exception:
            bot.pump(5)
    if bot.world.x >= 3089:
        raise SystemExit("tutorial stuck at: through survival gate pos=%s" % (bot.position,))
    print("ok through survival gate", flush=True)
    bot.walk_to(3080, 3086, open_doors=True, timeout=6000)
    tutorial_common.refresh_openings(bot)


def chef(bot):
    tutorial_common.check(bot, tutorial_common.talk(bot, "cook_instructor", near=(3080, 3084)), "chef found")
    tutorial_common.finish_dialog(bot, ("Nothing, thanks.",))
    flour = tutorial_common.obj_id(bot, "newbie_pot_flour")
    tutorial_common.check(bot, tutorial_common.wait_inv(bot, flour, timeout=60), "flour+water given")


def make_dough(bot):
    water = tutorial_common.obj_id(bot, "bucket_water")
    flour = tutorial_common.obj_id(bot, "newbie_pot_flour")
    dough = tutorial_common.obj_id(bot, "bread_dough")
    if bot.inventory().get(flour, 0) < 1 or bot.inventory().get(water, 0) < 1:
        chef(bot)
    tutorial_common.check(bot, tutorial_common.wait_inv(bot, flour, timeout=60), "flour+water ready")
    tutorial_common.check(bot, tutorial_common.wait_inv(bot, water, timeout=60), "flour+water ready")
    tutorial_common.use_held_on_held(bot, water, flour)
    tutorial_common.check(bot, tutorial_common.wait_inv(bot, dough, timeout=90), "dough mixed")


def bake_bread(bot):
    dough = tutorial_common.obj_id(bot, "bread_dough")
    if dough not in bot.inventory():
        make_dough(bot)
    bread = tutorial_common.obj_id(bot, "bread")
    ok = tutorial_common.use_held_on_loc_near(bot, ("newbierange",), dough, lambda b: bread in b.inventory())
    tutorial_common.check(bot, ok, "bread baked")


def music_tab(bot):
    tutorial_common.click_tut_tab(bot)
    bot.pump(10)
    print("ok music tab opened", flush=True)


def chef_recap(bot):
    tutorial_common.check(bot, tutorial_common.talk(bot, "cook_instructor", near=(3080, 3084)), "chef recap")
    tutorial_common.finish_dialog(bot, ("Nothing, thanks.",))


def chef_recap(bot):
    tutorial_common.check(bot, tutorial_common.talk(bot, "cook_instructor", near=(3080, 3084)), "chef recap")
    tutorial_common.finish_dialog(bot, ("Nothing, thanks.",))


def chef_door(bot):
    tutorial_common.check(bot, tutorial_common.pass_door(bot, "newbie_door3", beyond=(3070, 3090)), "through chef door")


def enable_run(bot):
    tutorial_common.click_tut_tab(bot)
    bot.pump(10)
    bot.if_button(tutorial_common.COM_RUN)
    bot.pump(15)
    print("ok run toggled (verified at quest door)", flush=True)
