"""Melee farmer: best F2P spot for the level, best banked gear, banked food.

Rotates attack, strength and defence styles, eats when hurt, loots every
drop except bones, and banks when the pack fills up. Underground spots
(Edgeville dungeon hill giants, Varrock sewers moss giants) are reached
through their trapdoors and ladders.

Usage: run.py [account] [spot ...] [--kills N] [--ticks N]
Without a spot, picks the highest spot whose minimum level is reached.
"""

import os
import sys
import time

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "gamedata"))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))

from lifescape.client import Bot, Unreachable
from lifescape.connection import LoginError
from lifescape.waits import wait_backpack, wait_ticks

import gamedata as knowledge

from scripts import randomevents
from scripts.hazards import wait_respawn
from scripts.tutorial import common as C

BACKPACK = 3214
BANK_MAIN = knowledge.comps.com_id("bank_main:inv")
BANK_SIDE = knowledge.comps.com_id("bank_side:inv")
WORN = knowledge.comps.com_id("wornitems:wear")

EAT_FRACTION = 0.55
FOOD_TARGET = 10
LOOT_RADIUS = 6
STYLE_KILLS = 3

MELEE_ORDER = ("attack", "strength", "defence")

# category -> {skill: combat-style component}, from player_attackstyles.rs2.
STYLE_BUTTONS = {
    "weapon_slash": {
        "attack": "combat_hacksword:hack0",
        "strength": "combat_hacksword:hack1",
        "defence": "combat_hacksword:hack3",
    },
    "weapon_stab": {
        "attack": "combat_stabsword:stab0",
        "strength": "combat_stabsword:stab1",
        "defence": "combat_stabsword:stab3",
    },
    "weapon_blunt": {
        "attack": "combat_blunt:blunt0",
        "strength": "combat_blunt:blunt1",
        "defence": "combat_blunt:blunt2",
    },
    "weapon_axe": {
        "attack": "combat_axe:axe0",
        "strength": "combat_axe:axe1",
        "defence": "combat_axe:axe3",
    },
    "weapon_2h_sword": {
        "attack": "combat_heavysword:heavy0",
        "strength": "combat_heavysword:heavy1",
        "defence": "combat_heavysword:heavy3",
    },
    "weapon_pickaxe": {
        "attack": "combat_pickaxe:pickaxe0",
        "strength": "combat_pickaxe:pickaxe1",
        "defence": "combat_pickaxe:pickaxe3",
    },
    "weapon_spear": {
        "attack": "combat_spear:spear0",
        "strength": "combat_spear:spear0",
        "defence": "combat_spear:spear3",
    },
    "unarmed": {
        "attack": "combat_unarmed:unarmed0",
        "strength": "combat_unarmed:unarmed1",
        "defence": "combat_unarmed:unarmed2",
    },
}

# A scimitar's speed beats a 2h sword's raw bonus, so one-handed slashers
# win ties. Values are strategy, not game data.
WEAPON_WEIGHT = {
    "weapon_slash": 1.0,
    "weapon_stab": 0.9,
    "weapon_axe": 0.85,
    "weapon_blunt": 0.85,
    "weapon_pickaxe": 0.7,
    "weapon_2h_sword": 0.6,
    "weapon_spear": 0.6,
}

ARMOUR_SLOTS = (0, 1, 2, 4, 5, 7, 9, 10, 12)


def levels_by_index(bot):
    return {index: level for index, (level, _xp) in bot.world.stats.items()}


def levels_by_name(bot):
    stats = bot.world.stats
    return {name: stats.get(index, (1, 0))[0] for name, index in knowledge.equipment.STATS.items()}


def melee_level(bot):
    levels = levels_by_index(bot)
    return (levels.get(0, 1) + levels.get(2, 1) + levels.get(1, 1)) // 3


def free_slots(bot):
    com, slots = bot.backpack()
    return 28 - len(slots)


def item_name(obj_id):
    return knowledge.obj_name_by_id(obj_id) or ""


def pack_names(bot):
    return {item_name(held) for (held, _count) in bot.backpack()[1].values()}


def worn_names(bot):
    return {item_name(held) for (held, _count) in bot.world.inv.get(WORN, {}).values()}


def bank_names(bot):
    return {item_name(held) for (held, _count) in bot.world.inv.get(BANK_MAIN, {}).values()}


def gear_score(entry, slot):
    if slot == 3:
        attack = max(entry.attack.values()) if entry.attack else 0
        return (attack + 2 * entry.strength) * WEAPON_WEIGHT.get(entry.category, 0.5)
    defence = sum(entry.defence.values()) if entry.defence else 0
    return defence + 2 * entry.strength + entry.prayer


def slot_candidates(slot, names, levels):
    found = []
    for name in names:
        entry = knowledge.wearable_by_name(name)
        if entry is None or entry.wearpos != slot:
            continue
        if not knowledge.requirements_met(entry, levels):
            continue
        found.append(entry)
    found.sort(key=lambda entry: gear_score(entry, slot), reverse=True)
    if slot == 3:
        # Prefer a one-handed weapon so the shield slot stays useful.
        found.sort(key=lambda entry: (knowledge.covers_slot(entry, 5), -gear_score(entry, slot)))
    return found


def gear_plan(names, levels):
    plan = {}
    for slot in (3,) + ARMOUR_SLOTS:
        candidates = slot_candidates(slot, names, levels)
        if candidates:
            plan[slot] = candidates
    return plan


def style_plan(category):
    return STYLE_BUTTONS.get(category or "unarmed", STYLE_BUTTONS["unarmed"])


def find_bank(bot):
    best = None
    for bank in knowledge.all_banks():
        if bank.members or bank.level != bot.world.level:
            continue
        dist = abs(bank.x - bot.world.x) + abs(bank.z - bot.world.z)
        if best is None or dist < best[0]:
            best = (dist, bank)
    return best[1] if best else None


def open_bank(bot, bank):
    try:
        booth_x, booth_z, level, booth_loc = C.find_loc(bot, "bankbooth")
    except Unreachable:
        return False
    tiles = sorted(
        [(booth_x + dx, booth_z + dz) for dx in (-1, 0, 1) for dz in (-1, 0, 1)],
        key=lambda tile: abs(tile[0] - bot.world.x) + abs(tile[1] - bot.world.z),
    )
    for tile_x, tile_z in tiles:
        try:
            bot.walk_to(tile_x, tile_z, level, open_doors=True, timeout=20)
        except (Unreachable, TimeoutError):
            continue
        for op in (2, 1):
            bot.op_loc(op, booth_x, booth_z, booth_loc)
            target = bot.world.ticks + 20
            while bot.world.ticks < target and bot.world.modal is None:
                bot.pump(5)
            if bot.world.modal is not None:
                return True
    return False


def withdraw(bot, obj_id, count=1):
    slots = bot.world.inv.get(BANK_MAIN, {})
    slot = next((slot for slot, (held, _count) in slots.items() if held == obj_id), None)
    if slot is None:
        return False
    amount = count
    while amount > 0:
        op = 3 if amount >= 10 else (2 if amount >= 5 else 1)
        taken = 10 if amount >= 10 else (5 if amount >= 5 else 1)
        bot.inv_button(op, obj_id, slot, BANK_MAIN)
        bot.pump(3)
        amount -= taken
    return True


def equip(bot, entry, timeout=25):
    obj_id = next((obj.obj_id for obj in knowledge.obj_by_name(entry.name)), None)
    if obj_id is None:
        return False
    worn = bot.world.inv.get(WORN, {})
    held = worn.get(entry.wearpos)
    if held is not None and held[0] == obj_id:
        return True
    try:
        com, slot = C.inv_slot(bot, obj_id)
    except KeyError:
        return False
    before = len(bot.world.messages)
    bot.op_held(2, obj_id, slot, com)
    target = bot.world.ticks + timeout
    while bot.world.ticks < target:
        bot.pump(2)
        worn = bot.world.inv.get(WORN, {})
        held = worn.get(entry.wearpos)
        if held is not None and held[0] == obj_id:
            return True
        if any("not a high enough level" in line.lower() for line in bot.world.messages[before:]):
            return False
        if any("free space" in line.lower() for line in bot.world.messages[before:]):
            return False
    return False


def equip_plan(bot, plan):
    equipped = {}
    for slot in (3,) + ARMOUR_SLOTS:
        candidates = plan.get(slot, ())
        if slot == 5 and 3 in equipped and knowledge.covers_slot(equipped[3], 5):
            continue
        for entry in candidates:
            if equip(bot, entry):
                equipped[slot] = entry
                break
    return equipped


def current_weapon_category(bot):
    worn = bot.world.inv.get(WORN, {})
    held = worn.get(3)
    if held is None:
        return "unarmed"
    entry = knowledge.wearable_by_name(item_name(held[0]))
    return entry.category if entry is not None else "unarmed"


def set_style(bot, skill):
    com_name = style_plan(current_weapon_category(bot)).get(skill)
    com = knowledge.comps.com_id(com_name) if com_name else None
    if com is not None:
        bot.if_button(com)
        bot.pump(3)


def eat(bot):
    com, slots = bot.backpack()
    for slot, (held, _count) in sorted(slots.items()):
        if knowledge.food_heal(item_name(held)) <= 0:
            continue
        bot.op_held(1, held, slot, com)
        bot.pump(3)
        return True
    return False


def food_count(bot):
    return sum(count for (_held, count) in bot.backpack()[1].values() if knowledge.food_heal(item_name(_held)) > 0)


def withdraw_food(bot):
    names = bank_names(bot)
    chosen = knowledge.best_food(names)
    if chosen is None:
        return 0
    food_id = next((obj.obj_id for obj in knowledge.obj_by_name(chosen[0])), None)
    if food_id is None:
        return 0
    have = bot.inventory().get(food_id, 0)
    need = max(0, FOOD_TARGET - have)
    if need > 0:
        withdraw(bot, food_id, need)
    return bot.inventory().get(food_id, 0)


def equip_inventory(bot):
    """Fallback when the bank is unreachable: gear already carried."""
    levels = levels_by_name(bot)
    plan = gear_plan(pack_names(bot) | worn_names(bot), levels)
    return equip_plan(bot, plan)


def prepare_at_bank(bot):
    """Deposit loot, withdraw the best gear and food for the current levels."""
    bank = find_bank(bot)
    if bank is None:
        equip_inventory(bot)
        return False
    bot.intend("banking at (%d,%d)" % (bank.x, bank.z))
    bot.walk_to(bank.x, bank.z, bank.level, open_doors=True, timeout=300)
    if not open_bank(bot, bank):
        print("bank did not open at %s" % (bot.position,), flush=True)
        equip_inventory(bot)
        return False
    target = bot.world.ticks + 50
    while bot.world.ticks < target and BANK_MAIN not in bot.world.inv:
        bot.pump(5)
    if BANK_MAIN not in bot.world.inv:
        bot.close_modal()
        equip_inventory(bot)
        return False

    carried = bot.backpack()[1].values()
    keep = {
        held
        for (held, _count) in carried
        if knowledge.wearable_by_name(item_name(held)) is not None
    }
    # Keep carried food only when the bank cannot restock it.
    if knowledge.best_food(bank_names(bot)) is None:
        keep |= {held for (held, _count) in carried if knowledge.food_heal(item_name(held)) > 0}
    bot.bank_deposit_all(BANK_SIDE, keep)

    levels = levels_by_name(bot)
    names = bank_names(bot) | pack_names(bot) | worn_names(bot)
    plan = gear_plan(names, levels)

    # Withdraw the top two candidates per slot, weapons and armour first.
    order = (3, 0, 4, 7, 5, 2, 9, 10, 12, 1)
    picked = []
    for slot in order:
        for entry in plan.get(slot, ())[:2]:
            obj_id = next((obj.obj_id for obj in knowledge.obj_by_name(entry.name)), None)
            if obj_id is None or obj_id in bot.inventory():
                continue
            if free_slots(bot) <= FOOD_TARGET:
                break
            if withdraw(bot, obj_id, 1):
                picked.append(entry)
        if free_slots(bot) <= FOOD_TARGET:
            break

    withdraw_food(bot)
    bot.close_modal()
    bot.pump(5)
    equipped = equip_plan(bot, plan)
    for slot, entry in sorted(equipped.items()):
        print("equipped %s (slot %d)" % (entry.name, slot), flush=True)
    return True


def npc_spawns(spot):
    wanted = set(spot.npc_names)
    found = []
    for entry in knowledge.all_npcs():
        if entry is None or (entry.name or "") not in wanted or entry.members:
            continue
        for spawn in entry.spawns:
            underground = spawn.z >= 9000
            if underground != (spot.entrance is not None):
                continue
            found.append(spawn)
    return found


def clusters(spawns, gap=15):
    groups = []
    for spawn in spawns:
        placed = False
        for group in groups:
            center_x = sum(item.x for item in group) // len(group)
            center_z = sum(item.z for item in group) // len(group)
            if max(abs(spawn.x - center_x), abs(spawn.z - center_z)) <= gap:
                group.append(spawn)
                placed = True
                break
        if not placed:
            groups.append([spawn])
    return groups


def bank_distance(bot, spot, spawn):
    bank = find_bank(bot)
    if bank is None:
        return 10000
    if spot.entrance is not None:
        entrance = spot.entrance.surface
        return abs(bank.x - entrance[0]) + abs(bank.z - entrance[1]) + abs(entrance[0] - spawn.x) + abs(entrance[1] - spawn.z)
    return abs(bank.x - spawn.x) + abs(bank.z - spawn.z)


def ranked_clusters(bot, spot):
    groups = [group for group in clusters(npc_spawns(spot)) if group]
    ranked = []
    for group in groups:
        center_x = sum(item.x for item in group) // len(group)
        center_z = sum(item.z for item in group) // len(group)
        score = len(group) - bank_distance(bot, spot, group[0]) / 40.0
        ranked.append((score, center_x, center_z, len(group)))
    ranked.sort(reverse=True)
    return [(center_x, center_z, count) for _score, center_x, center_z, count in ranked]


def pick_cluster(bot, spot):
    ranked = ranked_clusters(bot, spot)
    return ranked[0] if ranked else None


def go_to_spot(bot, spot, target):
    bot.walk_to(target[0], target[1], open_doors=True, timeout=300)


def attack_nearest(bot, spot):
    candidates = bot.npcs_named(spot.npc_names)
    if not candidates:
        return None
    nid, npc = min(candidates, key=lambda item: abs(item[1]["x"] - bot.world.x) + abs(item[1]["z"] - bot.world.z))
    distance = abs(npc["x"] - bot.world.x) + abs(npc["z"] - bot.world.z)
    if distance > 6:
        try:
            bot.approach(npc["x"], npc["z"], open_doors=True, timeout=30)
        except (Unreachable, TimeoutError):
            return None
    bot.op_npc(2, nid)
    return nid


def bury_bones(bot):
    """Bury every bone in the pack, freeing slots and training prayer."""
    while True:
        com, slots = bot.backpack()
        found = next(
            ((slot, held) for slot, (held, _count) in sorted(slots.items()) if "bones" in item_name(held)),
            None,
        )
        if found is None:
            return
        slot, held = found
        bot.op_held(1, held, slot, com)
        bot.pump(2)


def loot_ground(bot):
    taken = 0
    for (tile, obj_id) in sorted(bot.world.ground_items(bot.world.x, bot.world.z, bot.world.level, radius=LOOT_RADIUS)):
        if free_slots(bot) <= 0:
            break
        name = item_name(obj_id)
        if not name:
            continue
        try:
            if bot.take(obj_id=obj_id, timeout=12):
                taken += 1
        except Unreachable:
            continue
    bury_bones(bot)
    return taken


def ensure_not_mazed(bot):
    if not randomevents.check(bot):
        raise SystemExit("random event unresolved at %s" % (bot.position,))


def fight(bot, spot, target, kills_limit, ticks_limit, bank_food=True):
    kills = 0
    started = bot.world.ticks
    target_nid = None
    last_attack = 0
    style_index = 0
    last_event_check = bot.world.ticks
    max_hp = bot.world.stats.get(3, (10, 0))[0]
    set_style(bot, MELEE_ORDER[style_index])
    while True:
        if ticks_limit and bot.world.ticks - started >= ticks_limit:
            break
        if kills_limit and kills >= kills_limit:
            break
        bot.pump(2)
        if bot.world.ticks - last_event_check >= 25:
            last_event_check = bot.world.ticks
            ensure_not_mazed(bot)
        hp = bot.world.stats.get(3, (10, 0))[0]
        if hp <= 0:
            bot.intend("dead, waiting for respawn")
            wait_respawn(bot)
            max_hp = bot.world.stats.get(3, (10, 0))[0]
            target_nid = None
            continue
        max_hp = max(max_hp, hp)
        if hp <= max_hp * EAT_FRACTION:
            eat(bot)
        if target_nid is not None and target_nid in bot.world.npcs:
            # Re-issue the attack: a click from a few tiles away can fail
            # without clearing the NPC from view (path, line of sight).
            if bot.world.ticks - last_attack >= 5:
                npc = bot.world.npcs[target_nid]
                if abs(npc["x"] - bot.world.x) + abs(npc["z"] - bot.world.z) > 1:
                    try:
                        bot.approach(npc["x"], npc["z"], radius=1, timeout=10)
                    except (Unreachable, TimeoutError):
                        pass
                bot.op_npc(2, target_nid)
                last_attack = bot.world.ticks
            continue
        if target_nid is not None and target_nid not in bot.world.npcs:
            kills += 1
            print("kill %d at %s" % (kills, bot.position), flush=True)
            target_nid = None
            loot_ground(bot)
            if kills % STYLE_KILLS == 0:
                style_index = (style_index + 1) % len(MELEE_ORDER)
                set_style(bot, MELEE_ORDER[style_index])
            if free_slots(bot) <= 1 or (bank_food and food_count(bot) == 0):
                return kills, "bank"
            continue
        if target_nid is None:
            target_nid = attack_nearest(bot, spot)
            if target_nid is not None:
                last_attack = bot.world.ticks
            if target_nid is None:
                if abs(bot.world.x - target[0]) + abs(bot.world.z - target[1]) > 8:
                    try:
                        bot.approach(target[0], target[1], open_doors=True, timeout=30)
                    except (Unreachable, TimeoutError):
                        pass
                wait_ticks(bot, 5)
    return kills, "done"


def run_spot(bot, spot, kills_limit, ticks_limit, no_bank=False):
    targets = ranked_clusters(bot, spot)
    if not targets:
        print("no spawn for %s" % spot.key, flush=True)
        return 0
    total = 0
    bank_food = not no_bank
    for target in targets:
        print("spot %s target (%d,%d) spawns=%d" % (spot.key, target[0], target[1], target[2]), flush=True)
        while total < kills_limit if kills_limit else True:
            try:
                ensure_ready = not no_bank and prepare_at_bank(bot)
                if no_bank:
                    equip_inventory(bot)
                else:
                    bank_food = food_count(bot) > 0
            except LoginError:
                print("protocol error at the bank, relogging", flush=True)
                bot.relogin()
                wait_backpack(bot, BACKPACK)
                continue
            if not no_bank and not ensure_ready:
                print("bank prep failed, fighting with what we have", flush=True)
            try:
                go_to_spot(bot, spot, target)
            except (Unreachable, TimeoutError) as error:
                print("target unreachable: %s" % error, flush=True)
                break
            try:
                kills, reason = fight(bot, spot, target, kills_limit - total if kills_limit else 0, ticks_limit, bank_food=bank_food)
            except (ConnectionError, LoginError):
                print("connection or protocol error, relogging", flush=True)
                bot.relogin()
                wait_backpack(bot, BACKPACK)
                continue
            total += kills
            if reason == "done" or (kills_limit and total >= kills_limit):
                return total
    return total


def parse_args(argv):
    spots = []
    kills = 0
    ticks = 0
    no_bank = False
    index = 2
    while index < len(argv):
        value = argv[index]
        if value == "--kills" and index + 1 < len(argv):
            kills = int(argv[index + 1])
            index += 2
        elif value == "--ticks" and index + 1 < len(argv):
            ticks = int(argv[index + 1])
            index += 2
        elif value == "--no-bank":
            no_bank = True
            index += 1
        else:
            spots.append(value)
            index += 1
    return spots, kills, ticks, no_bank


def main():
    username = sys.argv[1] if len(sys.argv) > 1 else "t46"
    requested, kills_limit, ticks_limit, no_bank = parse_args(sys.argv)
    bot = Bot()
    try:
        bot.login(username, username)
        wait_backpack(bot, BACKPACK)
        target = bot.world.ticks + 60
        while bot.world.ticks < target and not (0 in bot.world.stats and 3 in bot.world.stats):
            bot.pump(5)
        randomevents.greet(bot)
        spots = [knowledge.spot_by_key(key) for key in requested if knowledge.spot_by_key(key)]
        if not spots:
            spots = [knowledge.best_spot(melee_level(bot))]
        print("melee=%d spots=%s" % (melee_level(bot), ",".join(spot.key for spot in spots)), flush=True)
        for spot in spots:
            print("=== %s ===" % spot.key, flush=True)
            bot.intend("melee at %s" % spot.key)
            total = run_spot(bot, spot, kills_limit, ticks_limit, no_bank=no_bank)
            print("ok %s killed %d" % (spot.key, total), flush=True)
        return 0
    finally:
        bot.close()


if __name__ == "__main__":
    sys.exit(main())
