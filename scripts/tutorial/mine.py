"""Quest guide and mine: quest dialogs, descent, prospecting, mining,
smelting and smithing. Verified through inventory and movement.
"""

from scripts.tutorial import common as tutorial_common


def quest_door(bot):
    tutorial_common.check(bot, tutorial_common.pass_door(bot, "newbie_door4", beyond=(3086, 3128)), "through quest door")


def quest_talk(bot):
    tutorial_common.check(bot, tutorial_common.talk(bot, "quest_instructor", near=(3086, 3121)), "quest guide found")
    tutorial_common.finish_dialog(bot, ("Nope, I'm ready to move on!",))


def quest_tab(bot):
    tutorial_common.click_tut_tab(bot)
    bot.pump(10)
    print("ok quest tab opened", flush=True)


def quest_recap(bot):
    tutorial_common.check(bot, tutorial_common.talk(bot, "quest_instructor", near=(3086, 3121)), "quest guide again")
    tutorial_common.finish_dialog(bot, ("Nope, I'm ready to move on!",))


def ladder_down(bot):
    tutorial_common.use_exit(bot, ("newbieladdertop1",), "down the mine ladder")


def mining_intro(bot):
    tutorial_common.check(bot, tutorial_common.talk(bot, "mining_instructor", near=(3079, 9505)), "miner found")
    tutorial_common.finish_dialog(bot, ("Nope, I'm ready to move on!",))


def prospect(bot):
    ok = tutorial_common.op_loc_near(bot, ("newbiecopperrock",), 2, lambda b: tutorial_common.has_message(b, "contains copper"))
    tutorial_common.check(bot, ok, "copper prospected")
    ok = tutorial_common.op_loc_near(bot, ("newbietinrock",), 2, lambda b: tutorial_common.has_message(b, "contains tin"))
    tutorial_common.check(bot, ok, "tin prospected")
    tutorial_common.check(bot, tutorial_common.talk(bot, "mining_instructor", near=(3079, 9505)), "miner again")
    tutorial_common.finish_dialog(bot, ("Nope, I'm ready to move on!",))
    pickaxe = tutorial_common.obj_id(bot, "bronze_pickaxe")
    tutorial_common.check(bot, tutorial_common.wait_inv(bot, pickaxe, timeout=60), "pickaxe given")


def dig(bot, rock_name, want_obj):
    ore = tutorial_common.obj_id(bot, want_obj)
    had = bot.inventory().get(ore, 0)
    ok = tutorial_common.op_loc_near(bot, (rock_name,), 1, lambda b: b.inventory().get(ore, 0) > had)
    tutorial_common.check(bot, ok, "%s mined" % want_obj)


def dig_tin_first(bot):
    dig(bot, "newbietinrock", "tin_ore")


def dig_copper(bot):
    dig(bot, "newbiecopperrock", "copper_ore")


def dig_tin(bot):
    dig(bot, "newbietinrock", "tin_ore")


def smelt(bot):
    copper = tutorial_common.obj_id(bot, "copper_ore")
    tin = tutorial_common.obj_id(bot, "tin_ore")
    if bot.inventory().get(copper, 0) < 1:
        dig(bot, "newbiecopperrock", "copper_ore")
    if bot.inventory().get(tin, 0) < 1:
        dig(bot, "newbietinrock", "tin_ore")
    bar = tutorial_common.obj_id(bot, "bronze_bar")
    ok = tutorial_common.use_held_on_loc_near(bot, ("newbiefurnace",), copper, lambda b: bar in b.inventory())
    tutorial_common.check(bot, ok, "bar smelted")


def smith_talk(bot):
    bar = tutorial_common.obj_id(bot, "bronze_bar")
    tutorial_common.check(bot, tutorial_common.talk(bot, "mining_instructor", near=(3079, 9505)), "miner for hammer")
    tutorial_common.finish_dialog(bot, ("Nope, I'm ready to move on!",))
    hammer = tutorial_common.obj_id(bot, "hammer")
    tutorial_common.check(bot, tutorial_common.wait_inv(bot, hammer, timeout=60), "hammer given")


def smith_anvil(bot):
    bar = tutorial_common.obj_id(bot, "bronze_bar")
    if bar not in bot.inventory():
        smelt(bot)
    dagger = tutorial_common.obj_id(bot, "bronze_dagger")
    opened = tutorial_common.use_held_on_loc_near(bot, ("anvil",), bar, lambda b: b.world.modal is not None)
    tutorial_common.check(bot, opened, "smithing interface")
    bot.inv_button(1, dagger, 0, tutorial_common.COM_SMITH_COL1)
    bot.close_modal()
    bot.pump(5)
    tutorial_common.check(bot, tutorial_common.wait_inv(bot, dagger, timeout=90), "dagger smithed")


def mine_exit(bot):
    tutorial_common.use_exit(bot, ("newbiedoor4",), "out of the mine")
