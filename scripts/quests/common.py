"""Shared runner for the F2P quest requirement checks.

The script never reads a save file: skills, combat level, quest points,
inventory and worn items come from the live session. The bank is read by
walking to a booth with pathfinding and opening it; a save file stays the
orchestrator's business. The typed report goes to the orchestrator as a
log event, which fills quest completion from the save.
"""

import json


from lifescape.client import Unreachable
from lifescape.quests import NavigationBlocked, PlayerState, QuestReport, SKILL_INDEX, check
from lifescape.waits import wait_backpack

import gamedata as knowledge
from scripts.quests import solver
from scripts.tutorial import common as tutorial_common

BACKPACK_COM = knowledge.com_id("inventory:inv")
WORN_COM = knowledge.com_id("wornitems:wear")
BANK_COM = knowledge.com_id("bank_main:inv")
QUEST_POINTS_VARP = knowledge.varp_id_by_name("qp")
START_TICKS = 200
BANK_ATTEMPTS = 6


def _drop_junk(bot):
    solver.drop_junk(bot)


def _wait_for_state(bot):
    deadline = bot.world.ticks + START_TICKS
    while bot.world.ticks < deadline:
        if len(bot.world.stats) >= 21:
            break
        bot.pump(5)
    wait_backpack(bot, BACKPACK_COM)


def _counts(slots):
    items = {}
    for held, count in slots.values():
        items[held] = items.get(held, 0) + count
    return items


def _carried(bot):
    backpack, slots = bot.backpack()
    carried = _counts(slots if backpack is not None else {})
    for held, count in _counts(bot.world.inv.get(WORN_COM, {})).items():
        carried[held] = carried.get(held, 0) + count
    return carried


def _all_carried(bot, requirements):
    carried = _carried(bot)
    return all(carried.get(item.obj_id, 0) >= item.count for item in requirements.items)


def open_nearest_bank(bot):
    if BANK_COM in bot.world.inv:
        return bot.bank_items()

    banks = sorted(
        (bank for bank in knowledge.all_banks() if not bank.members and bank.level == bot.world.level),
        key=lambda bank: abs(bank.x - bot.world.x) + abs(bank.z - bot.world.z),
    )
    banks = [bank for bank in banks if bot.can_reach(bank.x, bank.z, bank.level)]
    if not banks:
        raise Unreachable("no reachable F2P bank on level %d" % bot.world.level)

    last_error = None
    for bank in banks[:BANK_ATTEMPTS]:
        try:
            bot.intend("walking to bank %d,%d" % (bank.x, bank.z))
            bot.approach(bank.x, bank.z, radius=2, timeout=9000, open_doors=True)
            booth = tutorial_common.find_loc(bot, "bankbooth")
            if booth is not None:
                booth_x, booth_z, _booth_level, booth_loc = booth
                bot.open_bank(booth_x, booth_z, booth_loc)
            else:
                bot.open_bank(bank.x, bank.z, bank.loc_id)
            bot.intend("reading the bank")
            return bot.bank_items()
        except (Unreachable, TimeoutError) as error:
            last_error = error
            print("bank %d,%d unusable: %s" % (bank.x, bank.z, error), flush=True)
    raise Unreachable("no reachable bank, last error: %s" % (last_error,))


def snapshot(bot, bank_items):
    backpack, slots = bot.backpack()
    if backpack is None:
        slots = {}
    return PlayerState(
        levels={name: bot.world.stats[index][0] for name, index in SKILL_INDEX.items() if index in bot.world.stats},
        inventory=_counts(slots),
        worn=_counts(bot.world.inv.get(WORN_COM, {})),
        bank=bank_items,
        varps=dict(bot.world.varps),
        quest_points=bot.world.varps.get(QUEST_POINTS_VARP, 0),
        combat_level=bot.combat_level(),
    )


def report(result, account):
    payload = result.as_dict()
    payload["kind"] = "quest_requirements"
    payload["account"] = account
    print("@@EVENT " + json.dumps(payload, sort_keys=True), flush=True)


def report_result(account, quest, solved, error=None):
    payload = {"kind": "quest_result", "account": account, "quest": quest, "solved": bool(solved)}
    if error is not None:
        payload["error"] = str(error)
    print("@@EVENT " + json.dumps(payload, sort_keys=True), flush=True)




def run(bot, requirements, solve=None):
    """Check requirements and solve in the open session."""
    # Quest stages are gated on max(live varps, saved varps), and the save is
    # only written on logout: in a long-lived session it falls behind, so the
    # solver replays steps it already did. Logging out and back in refreshes
    # it (a fresh worker used to do this implicitly, one process per run).
    bot.relogin()
    from scripts import randomevents

    # A random event during a quest run teleports the bot away and every
    # later step fails. Hand it to the rescue instead of failing the run.
    event = randomevents.event_at(bot)
    if event == "cube":
        # Cubes rain all run long: solve the backlog inline and carry on
        # instead of bouncing to the rescue and back for every box.
        _wait_for_state(bot)
        solver.solve_strange_box(bot)
        event = randomevents.event_at(bot)
    if event is not None:
        randomevents.report(bot, event, False)
        raise RuntimeError("%s random event pending: the rescue must clear it first" % event)
    _wait_for_state(bot)
    _drop_junk(bot)
    # A random-event box replicates until solved and then eats quest
    # item deliveries; clear any backlog before doing anything else.
    solver.solve_strange_box(bot)
    try:
        bank_items = {} if _all_carried(bot, requirements) else open_nearest_bank(bot)
    except (Unreachable, TimeoutError) as error:
        result = QuestReport(
            quest=requirements.key,
            name=requirements.name,
            met=False,
            completed=False,
            reasons=(NavigationBlocked(detail=str(error)),),
        )
    else:
        result = check(requirements, snapshot(bot, bank_items))
    report(result, bot.username)
    print(result.summary(), flush=True)
    try:
        from orchestration import quest_state
        save_completed = quest_state.completion(bot.username, requirements.key)
    except Exception:
        save_completed = None
    if save_completed or result.completed:
        # Save-based and reliable (unlike live varps, which the server
        # only pushes on change): nothing left to do.
        print("%s: already complete" % (requirements.name,), flush=True)
        report_result(bot.username, requirements.key, True)
        return 0
    if result.met and solve is not None:
        # The save the checker reads can lag the live session (a run
        # that completes the quest but never logs out cleanly): the
        # live varps are truth, so skip a finished quest instead of
        # talking to its post-quest dialogue forever.
        if all(bot.world.varps.get(varp_id) == value for varp_id, value in requirements.completion_varps):
            print("%s: already complete" % (requirements.name,), flush=True)
            report_result(bot.username, requirements.key, True)
            return 0
        try:
            solver.withdraw_requirements(bot, requirements)
            solver.close_bank(bot)
            solve(bot)
            solved = solver.wait_complete(bot)
            if not solved:
                # The completion message is easy to miss (it can arrive
                # while a follow-up dialogue is open): the live quest
                # varps are proof enough. Pump once more first: the
                # server often applies the stage on queued ticks after
                # the last dialogue click lands.
                bot.pump(20)
                solved = all(bot.world.varps.get(varp_id) == value for varp_id, value in requirements.completion_varps)
        except (Unreachable, TimeoutError, solver.SolverError) as error:
            # A random event mid-run (teleport or cube rain) breaks every
            # later step the same way: hand it to the rescue like the
            # start-of-run check instead of failing the quest.
            event = randomevents.event_at(bot)
            if event is not None:
                randomevents.report(bot, event, False)
                raise RuntimeError("%s random event pending: the rescue must clear it first" % event)
            print("quest solve failed: %s" % (error,), flush=True)
            report_result(bot.username, requirements.key, False, error)
        else:
            report_result(bot.username, requirements.key, solved)
            print("%s: %s" % (
                requirements.name,
                "completed" if solved else "solver ran but no completion message",
            ), flush=True)
    return 0