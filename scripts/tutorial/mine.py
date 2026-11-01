"""Quest guide and mine: quest dialogs, descent, prospecting, mining,
smelting and smithing. Verified through inventory and movement.
"""

from scripts.tutorial import common as C


def quest_door(bot):
    C.check(bot, C.pass_door(bot, "newbie_door4", beyond=(3086, 3128)), "through quest door")


def quest_talk(bot):
    C.check(bot, C.talk(bot, "quest_instructor", near=(3086, 3121)), "quest guide found")
    C.finish_dialog(bot, ("Nope, I'm ready to move on!",))


def quest_tab(bot):
    C.click_tut_tab(bot)
    bot.pump(10)
    print("ok quest tab opened", flush=True)


def quest_recap(bot):
    C.check(bot, C.talk(bot, "quest_instructor", near=(3086, 3121)), "quest guide again")
    C.finish_dialog(bot, ("Nope, I'm ready to move on!",))


def ladder_down(bot):
    C.use_exit(bot, ("newbieladdertop1",), "down the mine ladder")


def mining_intro(bot):
    C.check(bot, C.talk(bot, "mining_instructor", near=(3079, 9505)), "miner found")
    C.finish_dialog(bot, ("Nope, I'm ready to move on!",))


def prospect(bot):
    ok = C.op_loc_near(bot, ("newbiecopperrock",), 2, lambda b: C.has_message(b, "contains copper"))
    C.check(bot, ok, "copper prospected")
    ok = C.op_loc_near(bot, ("newbietinrock",), 2, lambda b: C.has_message(b, "contains tin"))
    C.check(bot, ok, "tin prospected")
    C.check(bot, C.talk(bot, "mining_instructor", near=(3079, 9505)), "miner again")
    C.finish_dialog(bot, ("Nope, I'm ready to move on!",))
    pickaxe = C.obj_id(bot, "bronze_pickaxe")
    C.check(bot, C.wait_inv(bot, pickaxe, timeout=60), "pickaxe given")


def dig(bot, rock_name, want_obj):
    ore = C.obj_id(bot, want_obj)
    had = bot.inventory().get(ore, 0)
    ok = C.op_loc_near(bot, (rock_name,), 1, lambda b: b.inventory().get(ore, 0) > had)
    C.check(bot, ok, "%s mined" % want_obj)


def dig_tin_first(bot):
    dig(bot, "newbietinrock", "tin_ore")


def dig_copper(bot):
    dig(bot, "newbiecopperrock", "copper_ore")


def dig_tin(bot):
    dig(bot, "newbietinrock", "tin_ore")


def smelt(bot):
    copper = C.obj_id(bot, "copper_ore")
    tin = C.obj_id(bot, "tin_ore")
    if bot.inventory().get(copper, 0) < 1:
        dig(bot, "newbiecopperrock", "copper_ore")
    if bot.inventory().get(tin, 0) < 1:
        dig(bot, "newbietinrock", "tin_ore")
    bar = C.obj_id(bot, "bronze_bar")
    ok = C.use_held_on_loc_near(bot, ("newbiefurnace",), copper, lambda b: bar in b.inventory())
    C.check(bot, ok, "bar smelted")


def smith_talk(bot):
    bar = C.obj_id(bot, "bronze_bar")
    C.check(bot, C.talk(bot, "mining_instructor", near=(3079, 9505)), "miner for hammer")
    C.finish_dialog(bot, ("Nope, I'm ready to move on!",))
    hammer = C.obj_id(bot, "hammer")
    C.check(bot, C.wait_inv(bot, hammer, timeout=60), "hammer given")


def smith_anvil(bot):
    bar = C.obj_id(bot, "bronze_bar")
    if bar not in bot.inventory():
        smelt(bot)
    dagger = C.obj_id(bot, "bronze_dagger")
    opened = C.use_held_on_loc_near(bot, ("anvil",), bar, lambda b: b.world.modal is not None)
    C.check(bot, opened, "smithing interface")
    bot.inv_button(1, dagger, 0, C.COM_SMITH_COL1)
    bot.close_modal()
    bot.pump(5)
    C.check(bot, C.wait_inv(bot, dagger, timeout=90), "dagger smithed")


def mine_exit(bot):
    C.use_exit(bot, ("newbiedoor4",), "out of the mine")
