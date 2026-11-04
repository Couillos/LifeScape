"""Portes : clics, cotes, blacklist, rebuild maps.."""

from ... import collision as collision_mod
from ... import doors as doors_mod
from ... import steps as steps_mod
from ..core import Unreachable

from .base import DOOR_WAIT, MovementBase


class MovementDoors(MovementBase):

    def _sync_open_doors(self):
        """Mirror doors the server already opened or removed."""
        for spot in self.wall_doors:
            key = (spot[0], spot[1], spot[2])
            if key not in self.world.locs:
                continue
            loc = self.world.locs[key]
            if loc is None or doors_mod.open_op(self.loctypes, loc[0]) is None:
                if key not in self.unsealed_doors:
                    self.unseal(*key)

    def door_spot(self, x, z, level):
        for spot in self.wall_doors:
            if spot[0] == x and spot[1] == z and spot[2] == level:
                return spot
        return None

    def door_standing_sides(self, spot):
        """Tiles the server accepts a click on this door from.

        Exact mirror of ReachStrategy.reachWall1 for straight walls: the
        tile the wall faces is always valid, the perpendicular tiles need
        no wall facing the door on their own tile. Anything else is a
        wasted click (and a wasted 10 seconds standing in the wrong place).
        """
        x, z, level = spot[0], spot[1], spot[2]
        shape, angle = spot[3], spot[4]
        if shape == 0:
            if angle == 0:  # WEST
                ordered = [((x - 1, z), None),
                           ((x, z + 1), collision_mod.BLOCK_NORTH),
                           ((x, z - 1), collision_mod.BLOCK_SOUTH)]
            elif angle == 1:  # NORTH
                ordered = [((x, z + 1), None),
                           ((x - 1, z), collision_mod.BLOCK_WEST),
                           ((x + 1, z), collision_mod.BLOCK_EAST)]
            elif angle == 2:  # EAST
                ordered = [((x + 1, z), None),
                           ((x, z + 1), collision_mod.BLOCK_NORTH),
                           ((x, z - 1), collision_mod.BLOCK_SOUTH)]
            else:  # SOUTH
                ordered = [((x, z - 1), None),
                           ((x - 1, z), collision_mod.BLOCK_WEST),
                           ((x + 1, z), collision_mod.BLOCK_EAST)]
            sides = []
            for (side_x, side_z), wall in ordered:
                try:
                    flags = self.collision.get(side_x, side_z, level)
                except (KeyError, IndexError):
                    continue
                if not collision_mod.tile_walkable(flags):
                    continue
                if wall is not None and flags & wall:
                    continue
                sides.append((side_x, side_z))
            return sides
        tiles = {(tile_x, tile_z) for tile_x, tile_z, _lvl in self.door_footprints[id(spot)]} or {(x, z)}
        facing = {}
        for tile_x, tile_z in tiles:
            for side_x, side_z, flag in (
                (tile_x - 1, tile_z, collision_mod.WALL_EAST),
                (tile_x + 1, tile_z, collision_mod.WALL_WEST),
                (tile_x, tile_z - 1, collision_mod.WALL_NORTH),
                (tile_x, tile_z + 1, collision_mod.WALL_SOUTH),
            ):
                if (side_x, side_z) in tiles:
                    continue
                facing.setdefault((side_x, side_z), flag)
        sides = []
        for side, flag in facing.items():
            flags = self.collision.get(side[0], side[1], level)
            if not collision_mod.tile_walkable(flags):
                continue
            if flags & flag:
                continue
            sides.append(side)
        return sorted(sides, key=lambda tile: abs(tile[0] - self.world.x) + abs(tile[1] - self.world.z))

    def _open_action(self, action, deadline):
        if action.kind != "door":
            return False
        spot = self.door_spot(action.x, action.z, action.level)
        if spot is not None and id(spot) in self._opening:
            return False
        if spot is not None:
            self._opening.add(id(spot))
        try:
            if spot is not None:
                if (self.world.x, self.world.z) == (action.x, action.z):
                    # Standing on the crossing itself: step off to a side
                    # first, the click below must go out from beside the door.
                    if not self._raw_step_off():
                        print("door %d,%d has no reachable side" % (action.x, action.z), flush=True)
                        return False
                if not self.stand_on_door_side(spot, timeout=500):
                    print("door %d,%d has no reachable side" % (action.x, action.z), flush=True)
                    return False
            # The plan crosses this tile: entry steps succeed even onto a
            # closed door, so only a real click opens the way. The click
            # itself resolves the live loc id (a stale static id is
            # silently ignored by the server once the door changed state).
            through = self.open_door_inner(action.x, action.z, deadline)
            self.recent_doors[(action.x, action.z)] = self.world.ticks
            if through:
                self.unseal(action.x, action.z, action.level)
                if spot is not None:
                    self._door_failures.pop(id(spot), None)
                return True
            # Unverified: keep the door closed in the map instead of corrupting it
            # with an optimistic open. Two verified failures blacklist it so the
            # next plan routes around instead of hammering the same door.
            print("door %d,%d not confirmed open" % (action.x, action.z), flush=True)
            if spot is not None:
                if self._door_failures.get(id(spot), 0) + 1 >= 2:
                    print("door %d,%d blocked after repeated refusals" % (action.x, action.z), flush=True)
                    self._block_door(spot)
                else:
                    self._door_failures[id(spot)] = self._door_failures.get(id(spot), 0) + 1
            return False
        finally:
            if spot is not None:
                self._opening.discard(id(spot))

    def _open_door_toward(self, step, radius=4):
        """Open the door blocking the tile we were refused on.

        The click goes out from a tile the server accepts (orthogonal, no
        wall facing the door): clicking from a diagonal never opens anything
        and used to loop forever. Recently clicked doors are skipped so a
        refusal fails fast instead of re-clicking the same door.
        """
        best = None
        for spot in self.wall_doors:
            if spot[2] != self.world.level:
                continue
            if abs(spot[0] - self.world.x) + abs(spot[1] - self.world.z) > radius:
                continue
            if not self._door_closed_at(spot):
                continue
            if self._door_avoid(spot):
                # Known-dead for now (two refused clicks): re-clicking it
                # here would also extend its block on every step refusal.
                continue
            if self.world.ticks - self.recent_doors.get((spot[0], spot[1]), 0) < 500:
                continue
            distance = abs(spot[0] - self.world.x) + abs(spot[1] - self.world.z)
            if best is None or distance < best[0]:
                best = (distance, spot)
        if best is None:
            return False
        spot = best[1]
        self.dbg("door toward %d,%d" % (spot[0], spot[1]))
        if (self.world.x, self.world.z) in [(t[0], t[1]) for t in self.door_footprints.get(id(spot), ())]:
            # Standing on the door tile itself: stepping straight off with a
            # bare packet is the only thing that can work here (the router
            # cannot plan off this tile, and a click from the doorway tile
            # is the last resort, not the first).
            if self._raw_step_off():
                return True
            op = doors_mod.open_op(self.loctypes, spot[9])
            if op is not None and id(spot) not in self._opening:
                print("clicking %d,%d from its own tile (last resort)" % (spot[0], spot[1]), flush=True)
                self._opening.add(id(spot))
                try:
                    self.send_oploc(op, spot[0], spot[1], spot[9])
                    self._await_loc_change(spot[0], spot[1], spot[2], spot[9], self.world.ticks + DOOR_WAIT)
                finally:
                    self._opening.discard(id(spot))
                if self._raw_step_off():
                    self.unseal(spot[0], spot[1], spot[2])
                    return True
            return False
        if id(spot) in self._opening:
            return False
        op = doors_mod.open_op(self.loctypes, spot[9])
        if op is None:
            return False
        print("step refused toward %s, opening door at %d,%d" % (step, spot[0], spot[1]), flush=True)
        self._opening.add(id(spot))
        try:
            if not self.stand_on_door_side(spot, timeout=500):
                return False
            # The click resolves the live loc id itself (a stale static id
            # is silently ignored by the server once the door changed).
            if self.open_door_inner(spot[0], spot[1], self.world.ticks + 500):
                self.recent_doors[(spot[0], spot[1])] = self.world.ticks
                self._door_failures.pop(id(spot), None)
                return True
            if self.world.modal is not None:
                # The door answered with a window (guard, password, refusal):
                # that is a scripted door, not a door this layer can open.
                # Leave the window to the caller and never blocklist it.
                print("door %d,%d answered with a window" % (spot[0], spot[1]), flush=True)
                return False
            print("door %d,%d refused the click" % (spot[0], spot[1]), flush=True)
            self.recent_doors[(spot[0], spot[1])] = self.world.ticks
            if self._door_failures.get(id(spot), 0) + 1 >= 2:
                print("door %d,%d blocked after repeated refusals" % (spot[0], spot[1]), flush=True)
                self._block_door(spot)
            else:
                self._door_failures[id(spot)] = self._door_failures.get(id(spot), 0) + 1
            return False
        finally:
            self._opening.discard(id(spot))

    def try_step_on(self, x, z, level, deadline):
        """Walk one tile onto the door and report whether the server agreed."""
        if self.world.level != level:
            return False
        if (self.world.x, self.world.z) == (x, z):
            return True
        if abs(self.world.x - x) + abs(self.world.z - z) != 1:
            return False
        if (x, z, level) in (self.stair_tiles or ()):
            # Never step onto a transition tile by accident: the server
            # would move the player to another floor mid-route.
            return False
        before = (self.world.x, self.world.z)
        self._send_walk([(x, z)], False)
        limit = self.world.ticks + min(50, max(10, deadline - self.world.ticks))
        while self.world.ticks < limit:
            self.pump(1)
            if (self.world.x, self.world.z) != before:
                return True
        return False

    def stand_on_door_side(self, spot, timeout=500):
        """Stand on a tile the server accepts clicks from, trying each side.

        Only the first side used to be tried: when it sat behind another
        wall the bot burned the whole timeout and never clicked at all.
        """
        sides = self.door_standing_sides(spot)
        if not sides:
            return False
        if (self.world.x, self.world.z) in sides:
            return True
        if (self.world.x, self.world.z) == (spot[0], spot[1]):
            # The router cannot plan off the doorway tile itself, and every
            # side ranks pathless from here: step off first so the ranking
            # below means something and one bad side cannot eat the budget.
            self._raw_step_off()
            if (self.world.x, self.world.z) in sides:
                return True
        routing = self.planning_map or self.opened_map
        ranked = []
        for side in sides:
            path = self._find_path(routing, spot[2], self.world.x, self.world.z, side[0], side[1])
            ranked.append((0 if path else 1, len(path), side))
        ranked.sort()
        deadline = self.world.ticks + timeout
        for _, _, (side_x, side_z) in ranked:
            if self.world.ticks > deadline:
                break
            try:
                self.walk_to(side_x, side_z, spot[2], timeout=min(200, max(150, deadline - self.world.ticks)))
            except (Unreachable, TimeoutError) as error:
                print("door side %d,%d failed: %s" % (side_x, side_z, error), flush=True)
                continue
            if (self.world.x, self.world.z) == (side_x, side_z):
                self.dbg("door side ok %d,%d" % (side_x, side_z))
                return True
        return (self.world.x, self.world.z) in sides

    def open_door(self, x, z, timeout=1250):
        self.ensure_nav()
        spot = self.door_spot(x, z, self.world.level)
        if spot is not None:
            if id(spot) in self._opening:
                return False
            self._opening.add(id(spot))
        try:
            return self.open_door_inner(x, z, self.world.ticks + timeout)
        finally:
            if spot is not None:
                self._opening.discard(id(spot))

    def open_door_inner(self, x, z, deadline):
        level = self.world.level
        spot = self.door_spot(x, z, level)
        if spot is not None and self._toll_locked(spot):
            return False
        if spot is not None and id(spot) in getattr(self, "toll_doors", set()):
            return self._open_toll(spot, deadline)
        self.unseal(x, z, level)
        loc = self.world.loc_at(x, z, level)
        if loc is None and spot is None:
            return True
        op = None
        loc_id = None
        if loc is not None:
            op = doors_mod.open_op(self.loctypes, loc[0])
            if op is None:
                return True
            loc_id = loc[0]
        if op is None and spot is not None:
            # No dynamic loc (stale zone): the static door still needs a real
            # click, it must not be waved through.
            op = doors_mod.open_op(self.loctypes, spot[9])
            loc_id = spot[9]
        if op is None:
            return True
        here = (self.world.x, self.world.z)
        if here == (x, z):
            # Clicks must go out from beside the door: step off first.
            if not self._raw_step_off():
                return False
            here = (self.world.x, self.world.z)
        # Rectangular and wall scenery are only reachable from the four
        # orthogonal tiles, so a diagonal neighbour is a wasted attempt.
        neighbors = sorted(
            [(x + dx, z + dz) for dx, dz in ((0, 1), (1, 0), (0, -1), (-1, 0), (0, 0))],
            key=lambda tile: abs(tile[0] - here[0]) + abs(tile[1] - here[1]),
        )
        self._apply_removal(x, z, level)
        for nx, nz in neighbors:
            if (nx, nz) != here:
                try:
                    self.walk_to(nx, nz, level, timeout=max(250, deadline - self.world.ticks), open_doors=True)
                except (Unreachable, TimeoutError) as error:
                    print("door step to %d,%d failed: %s" % (nx, nz, error), flush=True)
                    continue
            seen_messages = len(self.world.messages)
            self.dbg("door click %d,%d id=%s" % (x, z, loc_id))
            self.send_oploc(op, x, z, loc_id)
            if self._await_loc_change(x, z, level, loc_id, min(deadline, self.world.ticks + DOOR_WAIT)):
                if self.try_step_on(x, z, level, self.world.ticks + 100):
                    self.unseal(x, z, level)
                    self.dbg("door through %d,%d" % (x, z))
                    return True
                continue
            messages = " ".join(self.world.messages[seen_messages:]).lower()
            if "won't open" in messages or "wont open" in messages:
                # Scripted one-way door (manor entrance from inside): no other
                # side will open it, fail fast instead of grinding each side.
                self.reseal(x, z, level)
                return False
            if any(word in messages for word in ("locked", "can't reach", "nothing interesting")):
                self.reseal(x, z, level)
                continue
            # The server often applies the open without the client seeing the
            # zone loc update: only unblock after proving the step, otherwise
            # rescind the optimistic double-pair unseal done above.
            if self.try_step_on(x, z, level, self.world.ticks + 100):
                self.unseal(x, z, level)
                return True
            self.reseal(x, z, level)
            if self.world.modal is not None:
                # Scripted door: the click opened a dialogue or a refusal
                # box, not a door. Hand the window to the caller untouched.
                print("door %d,%d answered with a window" % (x, z), flush=True)
                return False
                # A refused click (wrong side, locked) lands in a mesbox:
                # dismiss it now or every later side clicks behind a modal.
                self.close_modal()
                self.pump(2)
            continue
        self._rebuild_opened()
        return False

    def _open_toll(self, spot, deadline):
        """Payer le peage : clic -> dialogue du garde -> choix "Yes" -> tele.

        Le serveur ne change pas le loc, il teleporte de l'autre cote :
        le succes se mesure au cote franchi, pas a un changement de loc.
        """
        x, z, level = spot[0], spot[1], spot[2]
        if not self.stand_on_door_side(spot, timeout=500):
            return False
        loc = self.world.loc_at(x, z, level)
        loc_id = loc[0] if loc is not None else spot[9]
        op = doors_mod.open_op(self.loctypes, loc_id) or 1
        # Axe de traversee : mur W/E = cloture N-S, on passe en x.
        along_x = spot[4] in (0, 2)
        before = (self.world.x <= x) if along_x else (self.world.z <= z)

        def crossed():
            now = (self.world.x <= x) if along_x else (self.world.z <= z)
            return now != before and self.world.level == level

        self.send_oploc(op, x, z, loc_id)
        modal_at = self.world.ticks + 40
        while self.world.ticks < min(deadline, modal_at):
            self.pump(2)
            if self.world.modal is not None or crossed():
                break
        if self.world.modal is None and not crossed():
            return False
        while self.world.ticks < deadline:
            if crossed():
                self.unseal(x, z, level)
                self.dbg("toll through %d,%d" % (x, z))
                return True
            if self.world.modal is None:
                self.pump(2)
                continue
            try:
                options = self.chat_options()
            except (AttributeError, KeyError, IndexError, ValueError):
                options = []
            if any("yes" in text.lower() for _com, text in options):
                try:
                    self.chat_choose("Yes, ok.")
                except KeyError:
                    pass
            else:
                try:
                    self.chat_continue()
                except (AttributeError, KeyError, IndexError):
                    pass
            self.pump(3)
        return crossed()

    def reseal(self, x, z, level):
        """Undo an optimistic unseal whose click was refused.

        Drops only our own marker (a dynamic None): a real zone update
        showing something else is server truth and stays untouched.
        """
        if self.world.locs.get((x, z, level)) is None:
            self.world.locs.pop((x, z, level), None)
        self.unsealed_doors.discard((x, z, level))
        for spot in self.wall_doors:
            if spot[2] != level or abs(spot[0] - x) + abs(spot[1] - z) > 1:
                continue
            if self.loctypes["loc"][spot[9]].get("category", -1) not in (21, 22, 23, 24):
                continue
            if self.world.locs.get((spot[0], spot[1], level)) is None:
                self.world.locs.pop((spot[0], spot[1], level), None)
            self.unsealed_doors.discard((spot[0], spot[1], level))
        self._rebuild_opened()

    def _click_doors_around(self, radius=6):
        """Click every closed door in reach before giving up on a walk.

        Gates are often two locs sharing a fence line, and a failed approach
        used to blacklist them for the rest of the session.
        """
        clicked = False
        for spot in self.wall_doors:
            if spot[2] != self.world.level:
                continue
            if abs(spot[0] - self.world.x) + abs(spot[1] - self.world.z) > radius:
                continue
            if not self._door_closed_at(spot):
                continue
            if self._toll_locked(spot):
                continue
            op = doors_mod.open_op(self.loctypes, spot[9])
            if op is None:
                continue
            self.send_oploc(op, spot[0], spot[1], spot[9])
            clicked = True
        if not clicked:
            return False
        self.pump(4)
        opened = False
        for spot in self.wall_doors:
            if spot[2] != self.world.level:
                continue
            if abs(spot[0] - self.world.x) + abs(spot[1] - self.world.z) > radius:
                continue
            # Only trust a door the server actually opened: blindly unsealing
            # a refused click (one-way manor doors, locked gates) corrupts the
            # map and wipes the blocklist that routes around it.
            key = (spot[0], spot[1], spot[2])
            if key not in self.world.locs:
                continue
            loc = self.world.locs[key]
            if loc is None or doors_mod.open_op(self.loctypes, loc[0]) is None:
                self.unseal(*key)
                self.recent_doors[(spot[0], spot[1])] = self.world.ticks
                opened = True
        return opened

    def _escape_stuck_tile(self):
        """Force one step off a tile the server will not let us leave.

        Interior stalls happen when the client believes a door is open and the
        server does not: every planned leg is refused from that tile, so walk
        to a cardinal neighbour with a bare move packet instead.
        """
        level = self.world.level
        start = (self.world.x, self.world.z)
        for nx, nz in ((start[0] + 1, start[1]), (start[0] - 1, start[1]),
                       (start[0], start[1] + 1), (start[0], start[1] - 1)):
            if not collision_mod.tile_walkable(self.collision.get(nx, nz, level)):
                continue
            if (nx, nz, level) in (self.stair_tiles or ()):
                # Never step onto a transition tile by accident: the server
                # would move the player to another floor mid-route.
                continue
            self._send_walk([(nx, nz)], False)
            deadline = self.world.ticks + 50
            while self.world.ticks < deadline:
                self.pump(1)
                if (self.world.x, self.world.z) != start:
                    print("stepped off stuck tile to %s" % (self.position,), flush=True)
                    return True
        return False

    def _forget_unsealed_near(self, radius=3):
        """Make every door near us closed again, then plan with clicks.

        Both our own optimistic unseal and a stale server "removed" update
        hide a closed door from the planner: either way the next walk bounces
        on it. Dropping the dynamic entry exposes the static closed door, so
        the plan contains the click that actually opens it.
        """
        changed = False
        for spot in self.wall_doors:
            key = (spot[0], spot[1], spot[2])
            if abs(key[0] - self.world.x) + abs(key[1] - self.world.z) > radius:
                continue
            if key in self.world.locs:
                self.world.locs.pop(key, None)
                changed = True
            if key in self.unsealed_doors:
                self.unsealed_doors.discard(key)
                changed = True
        if changed:
            self._rebuild_opened()
        return changed

    def unseal(self, x, z, level):
        self.unsealed_doors.add((x, z, level))
        # The server opened it; drop the stale closed loc locally even when the
        # zone update never reaches this client.
        self.world.locs[(x, z, level)] = None
        self._apply_removal(x, z, level)

    def _apply_removal_to(self, target, x, z, level):
        for loc in (self.world.static_locs.get((x, z, level)), self.world.loc_at(x, z, level)):
            if loc is None:
                continue
            loc_id, shape, angle = loc
            entry = self.loctypes["loc"][loc_id]
            collision_mod.change_loc_collision(
                target, shape, angle, entry["blockrange"], entry["length"], entry["width"], entry["active"], x, z, level, False
            )

    def _apply_removal(self, x, z, level):
        self._apply_removal_to(self.opened_map, x, z, level)

    def open_nearby_door(self, deadline, target=None):
        # Single-flight: _open_door_inner walks with doors allowed, which can
        # land right back here when its approach has no path. Without a guard
        # the rescue nests once per nearby door until the stack (or the
        # server connection) gives out.
        if getattr(self, "_in_nearby_door", False):
            return False
        self._in_nearby_door = True
        try:
            return self._open_nearby_door_inner(deadline, target)
        finally:
            self._in_nearby_door = False

    def _open_nearby_door_inner(self, deadline, target=None):

        candidates = []
        for spot in self.wall_doors or ():
            x, z, level = spot[0], spot[1], spot[2]
            if level != self.world.level:
                continue
            if not self._door_closed_at(spot):
                continue
            dist = abs(x - self.world.x) + abs(z - self.world.z)
            if dist > 12:
                continue
            # Only doors that take us closer to the goal: opening a random
            # door walks the bot into a house it then cannot leave.
            if target is not None:
                if abs(x - target[0]) + abs(z - target[1]) >= abs(self.world.x - target[0]) + abs(self.world.z - target[1]):
                    continue
            candidates.append((dist, x, z))
        for _, x, z in sorted(candidates):
            if self.open_door(x, z, timeout=max(250, deadline - self.world.ticks)):
                return True
        return False

    def _door_on_path(self, path, limit=12):
        """First closed door crossed by the path, by step order."""
        steps = steps_mod.expand_waypoints(path, (self.world.x, self.world.z))
        if limit is not None:
            steps = steps[:limit]
        index_by_tile = {tile: index for index, tile in enumerate(steps)}
        best = None
        for spot in self.wall_doors:
            if spot[2] != self.world.level:
                # Footprints are matched as bare (x, z) tiles below: without
                # this guard a door on another floor (tower L2 over L0)
                # matches the current path, and opening it calls walk_to on
                # the wrong level, which a portal route always refuses.
                continue
            if self._door_avoid(spot):
                continue
            if id(spot) in self._opening:
                continue
            if self.world.ticks - self.recent_doors.get((spot[0], spot[1]), 0) < 1500:
                continue
            indices = [
                index_by_tile[(tile_x, tile_z)]
                for tile_x, tile_z, _level in self.door_footprints[id(spot)]
                if (tile_x, tile_z) in index_by_tile
            ]
            if not indices or not self._door_closed_at(spot):
                continue
            index = min(indices)
            if best is None or index < best[0]:
                best = (index, spot)
        return best[1] if best is not None else None

    def _door_closed_at(self, spot):
        x, z, level = spot[0], spot[1], spot[2]
        loc = self.world.loc_at(x, z, level)
        if loc is None:
            # A door tile with no visible loc is unknown, not open: the zone
            # update is often missing, and trusting it walked the bot into a
            # closed door it then could not leave.
            return (x, z, level) in self._wall_door_keys
        return doors_mod.open_op(self.loctypes, loc[0]) is not None

    def _block_door(self, spot):
        # Temporary, not forever: a blocked door on the only path (castle
        # exit, tower door) would otherwise disconnect the map for the rest
        # of the session and every later route reports "no route".
        self._blocked_doors.add(id(spot))
        self._blocked_doors_exp[id(spot)] = self.world.ticks + 6000
        self._rebuild_opened()

    def _sweep_blocked_doors(self):
        now = self.world.ticks
        for key in [key for key, expiry in self._blocked_doors_exp.items() if expiry <= now]:
            self._blocked_doors_exp.pop(key, None)
            self._blocked_doors.discard(key)

    def _rebuild_opened(self):
        self._sweep_blocked_doors()
        self.opened_map = self.collision.copy()
        for key in self.unsealed_doors:
            self._apply_removal(*key)
        if self.stair_tiles:
            # Bots never climb on purpose: stepping on a stair/ladder tile
            # makes the server move us upstairs, stranding the run. Route
            # around them; explicit op_loc on stairs still works adjacent.
            for stair_x, stair_z, stair_level in self.stair_tiles:
                try:
                    self.opened_map.set(stair_x, stair_z, stair_level, collision_mod.WALK_BLOCKED)
                except (KeyError, IndexError) as error:
                    print("cannot block stair tile %d,%d,%d: %s" % (
                        stair_x, stair_z, stair_level, error
                    ), flush=True)
        self._apply_temp_blocks(self.opened_map)
        self._rebuild_planning()

    def _rebuild_planning(self):
        """Routing map with every known door open, for planning through rooms."""
        if self.collision is None:
            self.planning_map = None
            return
        self.planning_map = self.collision.copy()
        for spot in self.wall_doors or ():
            if id(spot) in self._blocked_doors:
                continue
            self._apply_removal_to(self.planning_map, spot[0], spot[1], spot[2])
        for stair_x, stair_z, stair_level in self.stair_tiles or ():
            try:
                self.planning_map.set(stair_x, stair_z, stair_level, collision_mod.WALK_BLOCKED)
            except (KeyError, IndexError) as error:
                print("cannot block stair in planning map %d,%d,%d: %s" % (
                    stair_x, stair_z, stair_level, error
                ), flush=True)
        self._apply_temp_blocks(self.planning_map)

    def _temp_block(self, x, z, level, ticks=750):
        """Bar a tile briefly: the server refuses it but the maps say free.

        Capped and short-lived: an unbounded block list would seal the bot
        into a pocket of its own making (every stall adds one, and a
        deterministic router keeps returning the same first step).
        """
        self._blocked_tiles[(x, z, level)] = self.world.ticks + ticks
        while len(self._blocked_tiles) > 8:
            oldest = min(self._blocked_tiles, key=self._blocked_tiles.get)
            del self._blocked_tiles[oldest]

    def _apply_temp_blocks(self, target):
        now = self.world.ticks
        for key in [key for key, expiry in self._blocked_tiles.items() if expiry <= now]:
            del self._blocked_tiles[key]
        for (x, z, level) in self._blocked_tiles:
            try:
                target.set(x, z, level, collision_mod.WALK_BLOCKED)
            except (KeyError, IndexError):
                pass

    def _standing_on_closed_door(self):
        here = (self.world.x, self.world.z, self.world.level)
        for spot in self.wall_doors:
            if self._door_avoid(spot):
                continue
            if self.world.ticks - self.recent_doors.get((spot[0], spot[1]), 0) < 1500:
                continue
            if here in self.door_footprints[id(spot)] and self._door_closed_at(spot):
                return spot
        return None

    def _raw_step_off(self, radius=1):
        """Step off the current tile with a bare move packet, no router.

        The router (like the server's own findPath) cannot plan a first step
        off a closed-door tile, so any router-based step-off fails by
        construction. The server itself often still accepts the raw step.
        """
        start = (self.world.x, self.world.z)
        steps = [(start[0] + dx, start[1] + dz)
                 for dx in range(-radius, radius + 1)
                 for dz in range(-radius, radius + 1)
                 if dx or dz]
        steps.sort(key=lambda tile: abs(tile[0] - start[0]) + abs(tile[1] - start[1]))
        for nx, nz in steps:
            if any((nx, nz, self.world.level) in self.door_footprints[id(other)] for other in self.wall_doors):
                continue
            if (nx, nz, self.world.level) in (self.stair_tiles or ()):
                # Never step onto a transition tile by accident: the server
                # would move the player to another floor mid-route.
                continue
            if not collision_mod.tile_walkable(self.collision.get(nx, nz, self.world.level)):
                continue
            if self.try_step_on(nx, nz, self.world.level, self.world.ticks + 75):
                print("stepped off %s to %d,%d" % (self.position, nx, nz), flush=True)
                return True
        return False

    def _step_off_door(self, spot, deadline):
        if self._raw_step_off():
            return True
        for dx in (-1, 0, 1):
            for dz in (-1, 0, 1):
                if not dx and not dz:
                    continue
                nx, nz = self.world.x + dx, self.world.z + dz
                if any((nx, nz, self.world.level) in self.door_footprints[id(other)] for other in self.wall_doors):
                    continue
                try:
                    self.walk_to(nx, nz, self.world.level, timeout=max(250, deadline - self.world.ticks), open_doors=False)
                    return True
                except (Unreachable, TimeoutError) as error:
                    print("door step-off to %d,%d failed: %s" % (nx, nz, error), flush=True)
                    continue
        return False

    def _go_open_door(self, spot, deadline):
        # Reaching a door can mean crossing other doors first (castle halls):
        # walk there with doors allowed. The target itself stays out of any
        # nested plan via _opening so the approach never recurses into it.
        if id(spot) in self._opening or self._toll_locked(spot):
            return False
        self._opening.add(id(spot))
        try:
            x, z = spot[0], spot[1]
            if not self.stand_on_door_side(spot, timeout=500):
                self.recent_doors[(x, z)] = self.world.ticks
                return False
            if self.open_door(x, z, timeout=max(250, deadline - self.world.ticks)):
                self.recent_doors[(x, z)] = self.world.ticks
                self._door_failures.pop(id(spot), None)
                return True
            self.recent_doors[(x, z)] = self.world.ticks
            if self._door_failures.get(id(spot), 0) + 1 >= 2:
                print("door %d,%d blocked after repeated refusals" % (x, z), flush=True)
                self._block_door(spot)
            else:
                self._door_failures[id(spot)] = self._door_failures.get(id(spot), 0) + 1
            return False
        finally:
            self._opening.discard(id(spot))
