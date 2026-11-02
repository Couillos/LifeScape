"""Shared kit for the tutorial-island script: ids, lookups and waits.

Why no varp tracking: the tutorial progress varp (281) is not flagged
transmit, so the server never sends it. Every step below is verified
through something the client really sees: inventory, position, a modal
opening or closing, or a game message.
"""

import json
import time

from lifescape import collision as collision_mod
from lifescape import doors as doors_mod
from lifescape.client import Unreachable

COM_DESIGN_MAIN = 3559
COM_DESIGN_ACCEPT = 3651
COM_RUN = 153
COM_SMITH_COL1 = 1119
SPELL_WIND_STRIKE = 1152
TUT_TAB = 3


def has_message(bot, text):
    text = text.lower()
    for _com, line in bot.chat_texts():
        if text in line.lower():
            return True
    for message in bot.world.messages:
        if text in message.lower():
            return True
    return False


def wait_message(bot, text, timeout=100):
    target = bot.world.ticks + timeout
    while bot.world.ticks < target:
        bot.pump(5)
        for _com, line in bot.chat_texts():
            if text.lower() in line.lower():
                if bot.world.modal is not None:
                    bot.chat_continue()
                    bot.pump(3)
                return True
        for message in bot.world.messages:
            if text.lower() in message.lower():
                if bot.world.modal is not None:
                    bot.chat_continue()
                    bot.pump(3)
                return True
    return False


def wait_modal_closed(bot, timeout=50):
    target = bot.world.ticks + timeout
    while bot.world.ticks < target:
        bot.pump(5)
        if bot.world.modal is None:
            return True
    return False


def obj_id(bot, name):
    bot.ensure_nav()
    for index, entry in enumerate(bot.loctypes["obj"]):
        if entry and entry["name"] == name:
            return index
    raise KeyError("unknown obj %r" % name)


def inv_slot(bot, obj):
    com, slots = bot.backpack()
    for slot, (held, _count) in slots.items():
        if held == obj:
            return com, slot
    raise KeyError("no %d in backpack" % obj)


def find_loc(bot, *names, radius=150, timeout=20):
    """Nearest loc whose debugname contains one of names, on this level.

    Only matches within radius tiles: locs are interaction targets, and
    a match on the other side of the world (a stale static entry while
    the live one is still streaming in) must fail loudly instead of
    sending the bot across the continent. Pumps briefly first, since a
    freshly streamed area takes a few ticks to arrive.
    """
    bot.ensure_nav()
    target = bot.world.ticks + timeout
    can_pump = hasattr(bot, "pump")
    while True:
        best = None
        # static_actions carries the locs that answer a click even when a
        # decoration on the same tile replaces them in static_locs (the
        # fortress inner door and the maze statue both hide that way).
        seen = dict(bot.world.static_locs)
        seen.update(getattr(bot.world, "static_actions", None) or {})
        seen.update(bot.world.locs)
        for (x, z, level), loc in seen.items():
            if loc is None or level != bot.world.level:
                continue
            dist = abs(x - bot.world.x) + abs(z - bot.world.z)
            if dist > radius:
                continue
            label = bot.loctypes["loc"][loc[0]]["name"] if loc[0] < len(bot.loctypes["loc"]) else ""
            if not any(name in label for name in names):
                continue
            if best is None or dist < best[0]:
                best = (dist, x, z, level, loc[0])
        if best is not None:
            return best[1:]
        if bot.world.ticks >= target or not can_pump:
            where = getattr(bot, "position", (bot.world.level, bot.world.x, bot.world.z))
            raise Unreachable("no loc %r near %s" % (names, where))
        bot.pump(2)


def use_held_on_held(bot, target_obj, tool_obj):
    com, slot = inv_slot(bot, target_obj)
    use_com, use_slot = inv_slot(bot, tool_obj)
    bot.op_held_u(target_obj, slot, com, tool_obj, use_slot, use_com)


def approach_loc(bot, x, z, loc_id, level=None):
    bot.ensure_nav()
    entry = bot.loctypes["loc"][loc_id] if loc_id < len(bot.loctypes["loc"]) else {}
    radius = max(entry.get("length", 1), entry.get("width", 1)) + 1
    level = bot.world.level if level is None else level
    cands = sorted(
        [(x + dx, z + dz) for dx in range(-radius, radius + 1) for dz in range(-radius, radius + 1)],
        key=lambda tile: abs(tile[0] - bot.world.x) + abs(tile[1] - bot.world.z),
    )
    reachable = []
    for cx, cz in cands:
        try:
            if bot.router.find_path(bot.opened_map, level, bot.world.x, bot.world.z, cx, cz, move_near=False):
                reachable.append((cx, cz))
                if len(reachable) >= 3:
                    break
        except Exception:
            continue
    if not reachable:
        raise Unreachable("no path near %d,%d" % (x, z))
    for cx, cz in reachable:
        try:
            bot.walk_to(cx, cz, level, timeout=1250, open_doors=True)
            return
        except (Unreachable, TimeoutError):
            continue
    raise Unreachable("nothing reachable near %d,%d" % (x, z))


def use_held_on_loc(bot, x, z, loc, tool_obj):
    use_com, use_slot = inv_slot(bot, tool_obj)
    bot.op_loc_u(x, z, loc, tool_obj, use_slot, use_com)


def op_loc_near(bot, loc_names, op, done, timeout=400):
    target = bot.world.ticks + timeout
    x, z, _level, loc = find_loc(bot, *loc_names)
    entry = bot.loctypes["loc"][loc] if loc < len(bot.loctypes["loc"]) else {}
    radius = max(entry.get("length", 1), entry.get("width", 1))
    cands = sorted(
        [(x + dx, z + dz) for dx in range(-radius, radius + 1) for dz in range(-radius, radius + 1) if dx or dz],
        key=lambda tile: abs(tile[0] - bot.world.x) + abs(tile[1] - bot.world.z),
    )
    tried = set()
    while bot.world.ticks < target:
        progressed = False
        for cx, cz in cands:
            if bot.world.ticks > target or (cx, cz) in tried:
                continue
            try:
                path = bot.router.find_path(bot.opened_map, bot.world.level, bot.world.x, bot.world.z, cx, cz, move_near=False)
            except Exception:
                continue
            if not path:
                continue
            try:
                bot.walk_to(cx, cz, bot.world.level, timeout=1250, open_doors=True)
            except (Unreachable, TimeoutError):
                continue
            tried.add((cx, cz))
            progressed = True
            seen_msgs = len(bot.world.messages)
            seen_texts = len(bot.world.texts)
            bot.op_loc(op, x, z, loc)
            inner = bot.world.ticks + 20
            while bot.world.ticks < inner:
                bot.pump(2)
                if done(bot):
                    return True
                for message in bot.world.messages[seen_msgs:]:
                    if "can't reach that" in message.lower():
                        inner = 0
                        break
                if len(bot.world.texts) != seen_texts:
                    inner = min(inner, bot.world.ticks + 7)
        if not progressed:
            bot.pump(5)
    return False


def use_held_on_loc_near(bot, loc_names, tool_obj, done, timeout=400):
    target = bot.world.ticks + timeout
    x, z, _level, loc = find_loc(bot, *loc_names)
    entry = bot.loctypes["loc"][loc] if loc < len(bot.loctypes["loc"]) else {}
    radius = max(entry.get("length", 1), entry.get("width", 1)) + 1
    cands = sorted(
        [(x + dx, z + dz) for dx in range(-radius, radius + 1) for dz in range(-radius, radius + 1) if dx or dz],
        key=lambda tile: abs(tile[0] - bot.world.x) + abs(tile[1] - bot.world.z),
    )
    tried = set()
    while bot.world.ticks < target:
        progressed = False
        for cx, cz in cands:
            if bot.world.ticks > target or (cx, cz) in tried:
                continue
            try:
                path = bot.router.find_path(bot.opened_map, bot.world.level, bot.world.x, bot.world.z, cx, cz, move_near=False)
            except Exception:
                continue
            if not path:
                continue
            try:
                bot.walk_to(cx, cz, bot.world.level, timeout=1250, open_doors=True)
            except (Unreachable, TimeoutError):
                continue
            tried.add((cx, cz))
            progressed = True
            seen_msgs = len(bot.world.messages)
            use_held_on_loc(bot, x, z, loc, tool_obj)
            inner = bot.world.ticks + 20
            while bot.world.ticks < inner:
                bot.pump(2)
                if done(bot):
                    return True
                for message in bot.world.messages[seen_msgs:]:
                    if "can't reach that" in message.lower():
                        inner = 0
                        break
        if not progressed:
            bot.pump(5)
    return False


def cast_on_npc(bot, slot, spell_com=SPELL_WIND_STRIKE):
    bot.op_npc_t(slot, spell_com)


def talk(bot, npc_name, timeout=90, near=None):
    if _scan_and_talk(bot, npc_name, near):
        return True
    print("talk relogging to resync NPCs", flush=True)
    try:
        bot.relogin()
        bot.close_modal()
        bot.pump(5)
    except Exception as error:
        print("relogin failed: %s" % error, flush=True)
        return False
    if _scan_and_talk(bot, npc_name, near):
        return True
    # Last resort: wide patrol forces the server to drop then re-send
    # the NPC as a new spawn. Short legs: measured walks are fast when
    # a path exists; unreachable legs fail fast via the router.
    base = (bot.world.x, bot.world.z)
    for dx, dz in ((20, 0), (0, 0), (-20, 0)):
        if dx or dz:
            try:
                bot.walk_to(base[0] + dx, base[1] + dz, open_doors=True, timeout=1250)
            except Exception:
                pass
        if _scan_and_talk(bot, npc_name, None):
            return True
    print("talk giving up at %s" % (bot.position,), flush=True)
    return False


def _scan_and_talk(bot, npc_name, near=None):
    if near is not None and abs(bot.world.x - near[0]) + abs(bot.world.z - near[1]) > 4:
        try:
            # Measured: near points are chosen reachable, reached in <10 s.
            bot.walk_to(near[0], near[1], bot.world.level, timeout=1250, open_doors=True)
            print("talk near %s reached" % (bot.position,), flush=True)
        except Exception as error:
            print("talk near walk failed: %s at %s" % (error, bot.position), flush=True)
    frames = 0
    # NPC streaming flickers (wanderers drift in and out of view): scan
    # patiently and keep moving a little, since moving forces the server
    # to re-send nearby NPCs. ~150 ticks cover several update rounds.
    target = bot.world.ticks + 150
    nid = None
    stepped = 0
    while bot.world.ticks < target and nid is None:
        for _op, _name, _body in bot.pump(5):
            if _name == "NPC_INFO":
                frames += 1
        nid = bot.find_npc(npc_name)
        if nid is None and stepped < 4:
            stepped += 1
            for dx, dz in ((2, 0), (0, 2), (-2, 0), (0, -2)):
                try:
                    bot.walk_to(bot.world.x + dx, bot.world.z + dz, bot.world.level, timeout=500, open_doors=True)
                    break
                except (Unreachable, TimeoutError):
                    continue
    if nid is None:
        _types = {}
        for _n in bot.world.npcs.values():
            _types[_n["type"]] = _types.get(_n["type"], 0) + 1
        print("talk scan at %s: %d npcs %s, %d NPC frames" % (bot.position, len(bot.world.npcs), sorted(_types.items()), frames), flush=True)
        try:
            with open("/tmp/opencode/talk_fail_%s.json" % npc_name, "w") as handle:
                json.dump(sorted((nid, n["type"], n["x"], n["z"]) for nid, n in bot.world.npcs.items()), handle)
        except Exception:
            pass
        return False
    return _talk_from_best_tile(bot, nid)


def _click_npc(bot, nid):
    seen_msgs = len(bot.world.messages)
    bot.talk_to(nid)
    target = bot.world.ticks + 15
    unsets = 0
    while not bot.world.modal and bot.world.ticks < target:
        for _op, _name, _body in bot.pump(5):
            if _name == "UNSET_MAP_FLAG":
                unsets += 1
        if any("can't reach that" in m.lower() for m in bot.world.messages[seen_msgs:]):
            break
    if bot.world.modal is not None:
        return True
    if unsets:
        print("talk click rejected x%d at %s" % (unsets, bot.position), flush=True)
    return False


def _talk_from_best_tile(bot, nid, rounds=2):
    for _round in range(rounds):
        npc = bot.world.npcs.get(nid)
        if npc is None:
            bot.pump(5)
            continue
        # Click from where we stand first: in a cramped room every walk to an
        # adjacent tile can time out and the click never happens.
        if _click_npc(bot, nid):
            return True
        tiles = sorted(
            [(npc["x"] + dx, npc["z"] + dz) for dx in (-1, 0, 1) for dz in (-1, 0, 1) if dx or dz],
            key=lambda tile: abs(tile[0] - bot.world.x) + abs(tile[1] - bot.world.z),
        )
        for nx, nz in tiles:
            try:
                bot.walk_to(nx, nz, bot.world.level, timeout=750, open_doors=True)
            except (Unreachable, TimeoutError):
                continue
            if _click_npc(bot, nid):
                return True
    return False


def finish_dialog(bot, answers=(), timeout=300):
    target = bot.world.ticks + timeout
    answered = set()
    quiet = 0
    modal_still_since = None
    while bot.world.ticks < target:
        had_modal = bot.world.modal is not None
        before = list(bot.chat_texts())
        bot.pump(5)
        options = [text for _com, text in bot.chat_texts() if text in answers]
        pending = [o for o in options if o not in answered]
        if pending:
            choice = next((a for a in answers if a in pending), pending[0])
            bot.chat_choose(choice)
            answered.add(choice)
            quiet = 0
            modal_still_since = None
        elif bot.world.modal:
            bot.chat_continue()
            quiet = 0
            # The modal stays open but nothing changes: we are not
            # answering it (wrong dialog). Fail fast instead of hanging
            # until the timeout: a recap answer lands within ~35 ticks.
            if list(bot.chat_texts()) == before and had_modal:
                if modal_still_since is None:
                    modal_still_since = bot.world.ticks
                    print("dialog not closing, recent texts: %s" % ([text[:60] for _com, text in list(bot.chat_texts())[-4:]],), flush=True)
                if bot.world.ticks - modal_still_since > 75:
                    raise SystemExit("tutorial stuck: dialog not closing at %s modal=%s" % (bot.position, bot.world.modal))
            else:
                modal_still_since = None
        elif list(bot.chat_texts()) != before or had_modal:
            quiet = 0
        else:
            quiet += 1
            if quiet >= 4:
                return True
    return False


def click_tut_tab(bot):
    bot.tut_clickside(TUT_TAB)
    bot.pump(5)


def wait_inv(bot, obj, count=1, timeout=100):
    target = bot.world.ticks + timeout
    while bot.world.ticks < target:
        bot.pump(5)
        if bot.inventory().get(obj, 0) >= count:
            return True
    return False


def wait_level(bot, level, timeout=100):
    target = bot.world.ticks + timeout
    while bot.world.ticks < target:
        bot.pump(5)
        if bot.world.level == level:
            return True
    return False


def wait_moved(bot, timeout=100):
    before = (bot.world.x, bot.world.z, bot.world.level)
    target = bot.world.ticks + timeout
    while bot.world.ticks < target:
        bot.pump(5)
        if (bot.world.x, bot.world.z, bot.world.level) != before:
            bot.pump(10)
            return True
    return False


def pass_door(bot, door_name, beyond, timeout=120):
    x, z, _level, loc = find_loc(bot, door_name)
    return cross_door(bot, x, z, loc, beyond, what=door_name)


def cross_door(bot, x, z, loc, beyond=None, what="door", tries=6):
    for _attempt in range(tries):
        if beyond is not None and (bot.world.x, bot.world.z) == beyond:
            bot.recent_doors[(x, z)] = bot.world.ticks
            return True
        if abs(bot.world.x - x) + abs(bot.world.z - z) > 1:
            stepped = False
            for nx, nz in sorted(
                [(x + dx, z + dz) for dx in (-1, 0, 1) for dz in (-1, 0, 1) if dx or dz],
                key=lambda tile: abs(tile[0] - bot.world.x) + abs(tile[1] - bot.world.z),
            ):
                try:
                    bot.walk_to(nx, nz, bot.world.level, timeout=1000, open_doors=False)
                    stepped = True
                    break
                except (Unreachable, TimeoutError):
                    continue
            if not stepped:
                bot.pump(5)
                continue
        before = (bot.world.x, bot.world.z)
        was = bot.world.loc_at(x, z, bot.world.level)
        bot.op_loc(1, x, z, loc)
        target = bot.world.ticks + 15
        while bot.world.ticks < target:
            bot.pump(1)
            if beyond is not None and (bot.world.x, bot.world.z) == beyond:
                print("ok %s" % what, flush=True)
                bot.recent_doors[(x, z)] = bot.world.ticks
                return True
            if bot.world.loc_at(x, z, bot.world.level) != was:
                break
            if (bot.world.x, bot.world.z) != before and abs(bot.world.x - x) + abs(bot.world.z - z) <= 1:
                break
        # Teleport doors open server-side on click; plan the dash as open
        # (the local map still shows them closed).
        try:
            bot.unseal(x, z, bot.world.level)
        except Exception:
            pass
        bot.recent_doors[(x, z)] = bot.world.ticks
        refresh_openings(bot)
        if beyond is not None:
            # The dash can land while the door is mid-cycle; retry once
            # after it settles instead of clicking again (ping-pong).
            for _dash in range(2):
                try:
                    bot.walk_raw(beyond[0], beyond[1], bot.world.level, bot.world.ticks + 400)
                    print("ok %s" % what, flush=True)
                    return True
                except Exception:
                    bot.pump(5)
                    continue
    if beyond is not None:
        return (bot.world.x, bot.world.z) == beyond
    bot.pump(10)
    return True


def refresh_openings(bot):
    bot.ensure_nav()
    for spot in bot.wall_doors:
        x, z, level = spot[0], spot[1], spot[2]
        if (x, z, level) in bot.unsealed_doors:
            continue
        if (x, z, level) not in bot.world.locs:
            continue
        dyn = bot.world.locs[(x, z, level)]
        if dyn is not None and doors_mod.open_op(bot.loctypes, dyn[0]) is not None:
            continue
        static = bot.world.static_locs.get((x, z, level))
        entry = bot.loctypes["loc"][static[0]] if static else bot.loctypes["loc"][dyn[0]]
        collision_mod.change_loc_collision(
            bot.opened_map, spot[3], spot[4], entry["blockrange"], entry["length"], entry["width"], entry["active"], x, z, level, False
        )
        bot.unsealed_doors.add((x, z, level))


def use_exit(bot, names, what, timeout=20):
    for _attempt in range(3):
        x, z, _level, loc = find_loc(bot, *names)
        bot.approach(x, z, radius=1, open_doors=True)
        bot.op_loc(1, x, z, loc)
        if wait_moved(bot, timeout=timeout):
            bot.recent_doors[(x, z)] = bot.world.ticks
            refresh_openings(bot)
            print("ok %s" % what, flush=True)
            return
    raise SystemExit("tutorial stuck at: %s pos=%s" % (what, bot.position))


def check(bot, ok, what):
    print(("ok %s" % what if ok else "FAIL %s" % what), flush=True)
    if not ok:
        raise SystemExit("tutorial stuck at: %s pos=%s modal=%s" % (what, bot.position, bot.world.modal))


def has(bot, name, count=1):
    return bot.inventory().get(obj_id(bot, name), 0) >= count


TUT_TITLE_COM = 6180
TUT_LINE_COMS = (6181, 6182, 6183, 6184)

TAB_COMBAT = 0
TAB_SKILLS = 1
TAB_QUEST = 2
TAB_INVENTORY = 3
TAB_WORN = 4
TAB_PRAYER = 5
TAB_MAGIC = 6
TAB_FRIENDS = 8
TAB_CONTROLS = 12
TAB_MUSIC = 13


def instruction(bot):
    """Current tutorial instruction from the client-visible text box.

    The server rewrites this box at every stage, so it is the solid
    stage signal (the progress varp itself is never transmitted).
    """
    texts = dict(bot.chat_texts())
    title = texts.get(TUT_TITLE_COM, "")
    body = " ".join(texts.get(line_com, "") for line_com in TUT_LINE_COMS)
    return (title + "\n" + body).strip()


def stage_cache_path(username):
    return "/tmp/opencode/stage_%s.json" % username


def save_stage(username, stage_key):
    try:
        with open(stage_cache_path(username), "w") as handle:
            json.dump({"stage": stage_key, "when": time.time()}, handle)
    except Exception:
        pass


def load_stage(username):
    try:
        with open(stage_cache_path(username)) as handle:
            return json.load(handle).get("stage")
    except Exception:
        return None


def refresh_instruction(bot):
    """Re-show the stage instruction after a login.

    Fresh logins never carry the tutorial box: it is only (re)sent
    when the tutorial proc runs. A side-tab click re-runs it; on
    click-to-advance stages that click IS the step, otherwise the
    stage is left untouched.
    """
    if instruction(bot):
        return True
    bot.tut_clickside(TAB_INVENTORY)
    bot.pump(10)
    return bool(instruction(bot))


# (step key, distinctive instruction fragments) in tutorial order.
STAGE_MATCHERS = (
    ("guide", ("getting started",)),
    ("controls_tab", ("spanner icon",)),
    ("door1", ("interacting with scenery",)),
    ("survival_start", ("moving around", "survival expert")),
    ("open_inventory", ("flashing backpack icon",)),
    ("chop", ("cut down a tree",)),
    ("light_fire", ("building a fire",)),
    ("open_stats", ("bar graph icon",)),
    ("survival_food", ("these are your stats",)),
    ("catch", ("catch some shrimp", "fishing spot")),
    ("cook_burnt", ("cooking your shrimp",)),
    ("cook_fresh", ("burning your shrimp",)),
    ("survival_gate", ("just cooked your first",)),
    ("chef_path", ("mini-map",)),
    ("chef", ("talk to the chef",)),
    ("make_dough", ("making dough",)),
    ("bake", ("cooking dough",)),
    ("music_tab", ("first loaf of bread",)),
    ("chef_door", ("the music player", "next door")),
    ("run_controls", ("running man",)),
    ("run_button", ("click on the run button",)),
    ("quest_door", ("run to the next guide",)),
    ("quest_talk", ("talk with the quest guide",)),
    ("quest_tab", ("open the quest journal",)),
    ("quest_recap", ("your quest journal",)),
    ("ladder_down", ("enter some caves",)),
    ("mining_intro", ("mining and smithing",)),
    ("dig_tin_first", ("mine one tin ore",)),
    ("dig_copper", ("need some copper",)),
    ("dig_tin", ("need some tin",)),
    ("prospect", ("prospect",)),
    ("smelt", ("smelting",)),
    ("smith_talk", ("made a bronze bar",)),
    ("smith_anvil", ("smithing a dagger",)),
    ("mine_exit", ("finished this area",)),
    ("combat_intro", ("find out about combat",)),
    ("wield_tab", ("wielding weapons",)),
    ("wield", ("select wield from the menu",)),
    ("sword_talk", ("now holding your dagger",)),
    ("sword_equip", ("wield the sword and shield",)),
    ("combat_tab", ("crossed swords",)),
    ("rat_cage", ("tools needed for battle",)),
    ("melee", ("to attack the rat",)),
    ("melee_watch", ("sit back and watch",)),
    ("ranged_prep", ("made your first kill",)),
    ("ranged_kill", ("rat ranging",)),
    ("ladder_bank", ("completed the tasks here",)),
    ("bank_booth", ("bank of runescape",)),
    ("bank_door", ("this is your bank box",)),
    ("advisor_talk", ("financial advice",)),
    ("advisor_door", ("continue through the next door",)),
    ("chapel_path", ("follow the path to the chapel",)),
    ("prayer_tab", ("prayer menu", "flashing icon")),
    ("chapel_recap", ("tell you about prayers",)),
    ("friends_tab", ("flashing smily",)),
    ("ignore_tab", ("this is your friends list",)),
    ("chapel_final", ("this is your ignore list",)),
    ("chapel_door", ("almost finished", "pass through the")),
    ("terrova_talk", ("wizards house",)),
    ("magic_tab", ("open up your final menu",)),
    ("oops_runes", ("unhandled tutorial step",)),
    ("wind", ("several tries",)),
    ("wind_retry", ("didn't have any effect",)),
    ("final", ("almost completed the tutorial",)),
    ("wait", ("please wait",)),
)


def detect_stage(bot):
    """Map the visible instruction to a step key.

    Inventory is deliberately NOT used here: it only proves which items
    the player holds, not where the quest stands. Each step function
    checks its own required items before acting.
    """
    text = instruction(bot).lower()
    if not text.strip():
        return None
    for stage_key, fragments in STAGE_MATCHERS:
        if all(fragment in text for fragment in fragments):
            return stage_key
    return None
