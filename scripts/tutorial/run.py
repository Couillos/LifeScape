"""Tutorial island driver: detect the stage from what the server shows,
then do what that stage asks. Works from any point mid-quest.

Stage signal (in order of trust):
  1. the tutorial instruction box (title + lines, coms 6180-6184),
  2. the flashing side-tab arrow (TUT_FLASH) and the hint arrow,
  3. modal dialogs and game messages.
Never the tutorial varp (the server does not transmit it) and never
the inventory (items only prove what the player holds, and each step
function checks its own required items before acting).
"""

import sys
import time


from lifescape.client import Bot

from scripts import randomevents
from scripts.tutorial import combat, common, mine, north, south


STEP_ACTIONS = {
    "guide": north.guide,
    "controls_tab": north.controls_tab,
    "door1": north.door1,
    "survival_start": north.survival_start,
    "open_inventory": north.open_inventory,
    "chop": north.chop_tree,
    "light_fire": north.light_fire,
    "open_stats": north.open_stats,
    "survival_food": north.survival_food,
    "catch": north.catch,
    "cook_burnt": north.cook_burnt,
    "cook_fresh": north.cook_fresh,
    "survival_gate": north.survival_gate,
    "chef_path": north.chef_path,
    "chef": north.chef,
    "make_dough": north.make_dough,
    "bake": north.bake_bread,
    "music_tab": north.music_tab,
    "chef_recap": north.chef_recap,
    "chef_door": north.chef_door,
    "run_controls": north.enable_run,
    "run_button": north.enable_run,
    "quest_door": mine.quest_door,
    "quest_talk": mine.quest_talk,
    "quest_tab": mine.quest_tab,
    "quest_recap": mine.quest_recap,
    "ladder_down": mine.ladder_down,
    "mining_intro": mine.mining_intro,
    "prospect": mine.prospect,
    "dig_tin_first": mine.dig_tin_first,
    "dig_copper": mine.dig_copper,
    "dig_tin": mine.dig_tin,
    "smelt": mine.smelt,
    "smith_talk": mine.smith_talk,
    "smith_anvil": mine.smith_anvil,
    "mine_exit": mine.mine_exit,
    "combat_intro": combat.intro,
    "wield_tab": combat.wield_tab,
    "wield": combat.wield_dagger,
    "sword_talk": combat.sword_talk,
    "sword_equip": combat.sword_equip,
    "combat_tab": combat.combat_tab,
    "rat_cage": combat.rat_cage,
    "melee": combat.melee,
    "melee_watch": combat.melee,
    "ranged_prep": combat.ranged_prep,
    "ranged_kill": combat.ranged_kill,
    "ladder_bank": combat.ladder_to_bank,
    "bank_booth": south.bank_booth,
    "bank_door": south.bank_door,
    "advisor_talk": south.advisor_talk,
    "advisor_door": south.advisor_door,
    "chapel_path": south.chapel_path,
    "prayer_tab": south.prayer_tab,
    "chapel_recap": south.chapel_recap,
    "friends_tab": south.friends_tab,
    "ignore_tab": south.ignore_tab,
    "chapel_final": south.chapel_final,
    "chapel_door": south.chapel_door,
    "terrova_talk": south.terrova_talk,
    "magic_tab": south.magic_tab,
    "oops_runes": south.terrova_runes,
    "wind": south.wind_strike,
    "wind_retry": south.wind_strike,
    "final": south.final,
}


def main():
    username = sys.argv[1] if len(sys.argv) > 1 else "t24"
    bot = Bot()
    try:
        bot.login(username)
        print("start %s" % (bot.position,), flush=True)
        north.design(bot)
        bot.close_modal()
        target = bot.world.ticks + 50
        while bot.world.ticks < target and not bot.inventory():
            bot.pump(5)
        # Tutorial texts can lag login by several ticks; wait for them
        # before declaring the instruction missing.
        target = bot.world.ticks + 50
        last_greet = bot.world.ticks
        while bot.world.ticks < target and not common.instruction(bot):
            bot.pump(5)
            if bot.world.ticks - last_greet > 30:
                last_greet = bot.world.ticks
                randomevents.greet(bot)
        if not common.refresh_instruction(bot):
            raise SystemExit("tutorial stuck: no instruction at %s" % (bot.position,))
        started = bot.world.ticks
        last_key = None
        repeats = 0
        blanks = 0
        used_cache = False
        while True:
            # Greet event hosts on sight: an ignored old man/genie fails
            # the run into a teleport tower.
            randomevents.greet(bot)
            if bot.world.x > 3200 and bot.world.level == 0:
                print("done %s" % (bot.position,), flush=True)
                return
            if bot.world.ticks - started > 3000:
                raise SystemExit("tutorial stuck: 3000 ticks elapsed at %s" % (bot.position,))
            stage_key = common.detect_stage(bot)
            if stage_key is not None:
                bot.intend("tutorial: " + str(stage_key))
            if stage_key is None:
                # Grace: the text update can lag the stage change by a tick.
                bot.pump(10)
                stage_key = common.detect_stage(bot)
                if stage_key is not None:
                    blanks = 0
                    continue
                print("no stage match for instruction: %r flash=%s hint=%s" % (common.instruction(bot)[:200], bot.world.side_flash, bot.world.hint), flush=True)
                if common.instruction(bot).strip():
                    raise SystemExit("tutorial stuck: unmapped instruction at %s" % (bot.position,))
                if not used_cache:
                    used_cache = True
                    cached = common.load_stage(username)
                    if cached in STEP_ACTIONS:
                        print("resume cached stage %s" % cached, flush=True)
                        STEP_ACTIONS[cached](bot)
                        continue
                if not common.refresh_instruction(bot):
                    blanks += 1
                else:
                    blanks = 0
                if blanks > 3:
                    raise SystemExit("tutorial stuck: no instruction at %s" % (bot.position,))
                bot.pump(5)
                continue
            blanks = 0
            if stage_key == "wait":
                bot.pump(5)
                continue
            action = STEP_ACTIONS.get(stage_key)
            if action is None:
                raise SystemExit("tutorial stuck: unknown stage %r at %s" % (stage_key, bot.position))
            if stage_key == last_key:
                repeats += 1
                if repeats >= 5:
                    raise SystemExit("tutorial stuck: stage %r not progressing at %s" % (stage_key, bot.position))
            else:
                last_key = stage_key
                repeats = 0
                common.save_stage(username, stage_key)
                print("stage %s hint=%s flash=%s" % (stage_key, bot.world.hint, bot.world.side_flash), flush=True)
            action(bot)
            # Settle: let the server's stage update land before re-detecting,
            # otherwise we act twice on a stale instruction.
            bot.pump(5)
    finally:
        bot.close()


if __name__ == "__main__":
    main()
