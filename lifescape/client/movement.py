"""Bot movement: pathfinding, doors, portals and unreachable diagnostics.

The mixin assumes the core session and state helpers are on the same Bot.
"""

import json
import os
import time

from .. import collision as collision_mod
from .. import doors as doors_mod
from .. import nav as nav_mod
from .. import objs as objs_mod
from .. import portals as portals_mod
from .. import route as route_mod
from ..buffer import Writer
from ..protocol import client_op_id
from .core import StateError, Unreachable

STEP_CAP = 24
# The server either sends the open state within a couple of ticks or never
# (stale zone); waiting seconds just freezes the bot in front of the door.
DOOR_WAIT = 0.7


class _DoorOpened(Exception):
    """A rescue click opened a door mid-leg: replan from the new position
    instead of replaying the stale chunk that just failed."""


class MovementMixin:

    def _follow_route(self, plan, dest, run, timeout, open_doors):
        # A transition can be blocked (locked gate, quest item). Drop it and
        # plan again from where we stand instead of failing the whole route.
        # A walking leg can also turn out to be impassable (the estimate is
        # straight-line): drop that walk edge and plan again the same way.
        deadline = time.time() + timeout
        skip = set()
        skip_walks = set()
        self._dbg("route plan %s" % ([(k, getattr(q, "name", q)) for k, q in plan][:12]))
        while True:
            # Replans are pure CPU; keep the server from dropping us.
            if getattr(self.connection, "sock", None) is not None:
                self.pump(1)
            leg_start = (self.world.level, self.world.x, self.world.z)
            replanned = False
            for kind, payload in plan:
                if time.time() > deadline:
                    raise TimeoutError("route timed out at %s" % (self.world.position,))
                if kind == "walk":
                    try:
                        self.walk_to(payload[1], payload[2], payload[0], run=run, timeout=max(30, deadline - time.time()), open_doors=open_doors)
                    except (Unreachable, TimeoutError) as error:
                        print("walk leg to %s failed: %s, replanning" % (payload, error), flush=True)
                        skip_walks.add((leg_start, payload))
                        replanned = True
                        break
                    leg_start = payload
                    continue
                self._dbg("ride %s" % (payload.name,))
                if not payload.ride(self):
                    print("transition %s failed at %s, replanning" % (payload.name, self.world.position), flush=True)
                    skip.add(payload.name)
                    # Session-wide, with expiry: a locked door or missing key
                    # will fail the same way on every fresh plan, so remember
                    # it instead of rediscovering it on each walk_to. Short
                    # expiry: a transition failed for a transient reason
                    # (closed door, missed click) must become usable again
                    # quickly, or one hiccup detours the whole session.
                    self._bad_transitions[payload.name] = time.time() + 180
                    replanned = True
                    break
                leg_start = tuple(payload.to)
            if not replanned:
                return True
            plan = self._plan_portals((self.world.level, self.world.x, self.world.z), dest, skip, skip_walks)
            if plan is None:
                raise Unreachable("no route to %s" % (dest,))

    def approach(self, x, z, level=0, radius=2, timeout=60, open_doors=True):
        self._dbg("approach %d,%d,%d r=%d" % (x, z, level, radius))
        self._ensure_nav()
        try:
            # Exact first, but briefly: a blocked target (wall, locked door)
            # can never be stood on, and grinding it burns the whole budget
            # while an adjacent tile already satisfies the radius.
            self.walk_to(x, z, level, timeout=min(timeout, 15), open_doors=open_doors)
        except (Unreachable, TimeoutError) as error:
            print("exact approach to %d,%d failed: %s" % (x, z, error), flush=True)
        if abs(self.world.x - x) + abs(self.world.z - z) <= radius:
            return True
        deadline = time.time() + timeout
        here = (self.world.x, self.world.z)
        candidates = sorted(
            [(x + dx, z + dz) for dx in range(-radius, radius + 1) for dz in range(-radius, radius + 1)],
            key=lambda tile: abs(tile[0] - here[0]) + abs(tile[1] - here[1]),
        )
        for cx, cz in candidates:
            if (cx, cz) == here:
                return True
            try:
                path = self._router.find_path(self._opened_map if open_doors else self._collision, level, here[0], here[1], cx, cz)
                path = [step for step in path if step[0] == level]
            except (KeyError, IndexError, ValueError) as error:
                print("router failed near %s for %d,%d: %s" % (here, cx, cz, error), flush=True)
                continue
            if not path:
                continue
            try:
                self.walk_to(cx, cz, level, timeout=max(5, deadline - time.time()), open_doors=open_doors)
            except (Unreachable, TimeoutError) as error:
                print("approach candidate %d,%d failed: %s" % (cx, cz, error), flush=True)
                continue
            if abs(self.world.x - x) + abs(self.world.z - z) <= radius:
                return True
        raise Unreachable("nothing reachable within %d of %d,%d from %s" % (
            radius, x, z, self.world.position
        ))

    def walk_to(self, x, z, level=0, run=None, timeout=180, open_doors=True):
        # Movement, doors and known ladder/trapdoor links are the SDK's job:
        # a script only names the destination.
        if level != self.world.level:
            if self._in_portal_route:
                raise Unreachable("walk_to stays on one level inside a portal route")
            self._ensure_nav()
            plan = self._plan_portals((self.world.level, self.world.x, self.world.z), (level, x, z))
            if plan is not None:
                self._in_portal_route = True
                try:
                    return self._follow_route(plan, (level, x, z), run, timeout, open_doors)
                finally:
                    self._in_portal_route = False
            raise Unreachable("walk_to stays on one level, use a portal to change level")
        if run is None:
            run = self.want_run()
        self._ensure_nav()
        self._dbg("walk_to %d,%d,%d" % (x, z, level))
        if not self._in_portal_route and abs(x - self.world.x) + abs(z - self.world.z) > 120:
            # Same level number, disconnected region (a cellar shares
            # level 0 with the surface): no direct walk can arrive, and
            # grinding one only burns the walk timeout. Probe with the
            # doors-aware planner first; only a portal route can cross.
            try:
                direct = (
                    self._planner.plan(
                        self.world.level, self.world.x, self.world.z,
                        x, z, self._door_closed_at, self._blocked_doors,
                    )
                    if self._planner is not None
                    else True
                )
            except Exception:
                direct = True
            if not direct:
                plan = self._plan_portals((self.world.level, self.world.x, self.world.z), (level, x, z))
                if plan is not None and any(kind == "portal" for kind, _payload in plan):
                    self._in_portal_route = True
                    try:
                        return self._follow_route(plan, (level, x, z), run, timeout, open_doors)
                    finally:
                        self._in_portal_route = False
        try:
            return self._walk_direct(x, z, level, run, timeout, open_doors)
        except TimeoutError:
            if not open_doors:
                raise
            # Same level number, disconnected region (tower cellar, dungeon):
            # the direct walk stalls on a wall it can never cross. A portal
            # route (ladder/trapdoor between the regions) is the only way.
            if not self._in_portal_route and abs(x - self.world.x) + abs(z - self.world.z) > 200:
                plan = self._plan_portals((self.world.level, self.world.x, self.world.z), (level, x, z))
                if plan is not None and any(kind == "portal" for kind, _payload in plan):
                    self._in_portal_route = True
                    try:
                        return self._follow_route(plan, (level, x, z), run, timeout, open_doors)
                    finally:
                        self._in_portal_route = False
            # A door we believe open can be closed on the server: retry with
            # the raw map, which routes around it instead of through it.
            print("walk with doors open stalled, retrying on the raw map", flush=True)
            return self._walk_direct(x, z, level, run, min(timeout, 30), False)
        except Unreachable:
            if self._in_portal_route:
                raise
            plan = self._plan_portals((self.world.level, self.world.x, self.world.z), (level, x, z))
            if plan is None:
                raise
            self._in_portal_route = True
            try:
                return self._follow_route(plan, (level, x, z), run, timeout, open_doors)
            finally:
                self._in_portal_route = False

    def _walk_direct(self, x, z, level, run, timeout, open_doors):
        # A purposefully blocked target (wall, table, booth) ends at the
        # nearest reachable tile, chosen by path length.
        if not collision_mod.tile_walkable(self._collision.get(x, z, level)):
            routing_map = self._opened_map if open_doors else self._collision
            path = self._router.find_path(routing_map, level, self.world.x, self.world.z, x, z)
            if path:
                x, z = path[-1][1], path[-1][2]
            else:
                report = self._router.unreachable_report(
                    routing_map, level, self.world.x, self.world.z, x, z, self._loc_name_at
                )
                if report["nearest"] == (self.world.x, self.world.z):
                    print("nothing reachable toward %d,%d from %s: %s" % (
                        x, z, (self.world.x, self.world.z),
                        self._explain_unreachable(x, z, open_doors),
                    ), flush=True)
                    return True
        deadline = time.time() + timeout
        visited = [(self.world.x, self.world.z)]
        planned_only = False
        while (self.world.x, self.world.z) != (x, z):
            if time.time() > deadline:
                raise TimeoutError("walk to %d,%d timed out at %s" % (x, z, self.world.position))
            before = (self.world.x, self.world.z)
            try:
                step = self._next_step(x, z, visited, deadline, open_doors)
            except Unreachable:
                # No raw path, but the planning map (all doors open) may have
                # one: let the door-aware planner drive the walk.
                if planned_only or not open_doors:
                    raise
                planned_only = True
                try:
                    self._walk_single(x, z, run, deadline, open_doors)
                except _DoorOpened:
                    pass
                continue
            try:
                self._walk_single(step[0], step[1], run, deadline, open_doors)
            except _DoorOpened:
                continue
            here = (self.world.x, self.world.z)
            if here != before and (not visited or abs(here[0] - visited[-1][0]) + abs(here[1] - visited[-1][1]) >= 4):
                visited.append(here)
                del visited[:-12]
            if here == before and here != (x, z):
                raise Unreachable(self._explain_unreachable(x, z, open_doors))
        return True

    def _sync_open_doors(self):
        """Mirror doors the server already opened or removed."""
        for spot in self._wall_doors:
            key = (spot[0], spot[1], spot[2])
            if key not in self.world.locs:
                continue
            loc = self.world.locs[key]
            if loc is None or doors_mod.open_op(self._loctypes, loc[0]) is None:
                if key not in self._unsealed_doors:
                    self._unseal(*key)

    def _next_step(self, x, z, visited, deadline, open_doors):
        """A reachable waypoint toward a far destination.

        Straight-line chunks cross walls; ask the router instead and take
        the end of the reachable path. Closed doors are opened first so the
        walker can use them.
        """
        if open_doors:
            self._sync_open_doors()
        dx = x - self.world.x
        dz = z - self.world.z
        if max(abs(dx), abs(dz)) <= 32:
            return (x, z)
        routing_map = self._opened_map if open_doors else self._collision
        candidates = [(self.world.x + max(-32, min(32, dx)), self.world.z + max(-32, min(32, dz)))]
        for lateral in (16, 32):
            for sign in (1, -1):
                if abs(dx) >= abs(dz):
                    candidates.append((self.world.x + max(-32, min(32, dx)), self.world.z + sign * lateral))
                else:
                    candidates.append((self.world.x + sign * lateral, self.world.z + max(-32, min(32, dz))))
        candidates.append((self.world.x + max(-8, min(8, dx)), self.world.z + max(-8, min(8, dz))))
        # Dense scenery (swamp trees, fences) needs a small hop to get around.
        candidates.append((self.world.x + max(-4, min(4, dx)), self.world.z + max(-4, min(4, dz))))
        candidates.append((self.world.x + max(-2, min(2, dx)), self.world.z + max(-2, min(2, dz))))
        step = None
        for candidate in candidates:
            if candidate == (self.world.x, self.world.z):
                continue
            if any(abs(candidate[0] - seen[0]) + abs(candidate[1] - seen[1]) < 4 for seen in visited):
                continue
            path = self._router.find_path(routing_map, self.world.level, self.world.x, self.world.z, candidate[0], candidate[1])
            if not path and open_doors and self._open_nearby_door(deadline, (x, z)):
                routing_map = self._opened_map
                path = self._router.find_path(routing_map, self.world.level, self.world.x, self.world.z, candidate[0], candidate[1])
            if path:
                # A straight-line waypoint inside a field ends at the fence.
                # Keep the candidate whose path actually gets closest to it,
                # so the walk follows the road/gate instead of hugging walls.
                end_x, end_z = path[-1][1], path[-1][2]
                gap = abs(end_x - candidate[0]) + abs(end_z - candidate[1])
                if step is None or gap < step[0]:
                    step = (gap, (end_x, end_z))
        if step is not None and step[1] != (self.world.x, self.world.z):
            return step[1]
        raise Unreachable(self._explain_unreachable(x, z, open_doors))

    def _loc_name_at(self, x, z, level):
        loc = self.world.loc_at(x, z, level)
        if loc is None or self._loctypes is None:
            return None
        if loc[0] >= len(self._loctypes["loc"]):
            return None
        entry = self._loctypes["loc"][loc[0]]
        return entry.get("name") if entry else None

    def _unreachable_hints(self, names):
        text = " ".join(name for name in names if name).lower()
        hints = []
        if "toll" in text:
            hints.append("toll gate (needs money)")
        if "member" in text:
            hints.append("members-only area")
        if "quest" in text:
            hints.append("quest requirement")
        if "key" in text:
            hints.append("key/item requirement")
        if "ladder" in text or "stair" in text:
            hints.append("level change (ladder/stair)")
        if "gate" in text or "door" in text:
            hints.append("open the gate/door (key, money, quest or level may be needed)")
        if not hints:
            hints.append("wall, water, different level or missing access")
        return hints

    def _explain_unreachable(self, x, z, open_doors=True):
        routing_map = self._opened_map if open_doors else self._collision
        try:
            report = self._router.unreachable_report(
                routing_map, self.world.level, self.world.x, self.world.z, x, z, self._loc_name_at
            )
        except (KeyError, IndexError, ValueError) as error:
            return "no path to %d,%d from %s (diagnosis failed: %r)" % (x, z, (self.world.x, self.world.z), error)
        parts = ["no path to %d,%d from %s" % (x, z, (self.world.x, self.world.z))]
        if report["dest_blocked"]:
            parts.append("target tile is blocked" + (" by %s" % report["dest_name"] if report["dest_name"] else ""))
        if report["nearest"] is not None:
            parts.append("nearest reachable by path %d,%d (path %d, %d tiles from target)" % (
                report["nearest"][0], report["nearest"][1], report["path_distance"], report["spread"]
            ))
        names = []
        for _block_x, _block_z, name in report["blockers"]:
            if name and name not in names:
                names.append(name)
        if names:
            parts.append("frontier blocked by " + ", ".join(names[:6]))
        parts.append("possible cause: " + ", ".join(self._unreachable_hints(names)))
        return "; ".join(parts)

    def _walk_plan(self, plan, run, deadline):
        steps = plan.steps
        actions = list(plan.actions)
        index = 0
        while index < len(steps):
            action = next((candidate for candidate in actions if candidate[0] >= index), None)
            if action is None:
                self._route = [(step_x, step_z) for step_x, step_z in steps[index:]]
                self._route_target = plan.target
                self._action_route = []
                self._route_action = None
                self._write_state()
                self._walk_steps(steps[index:], run, deadline)
                index = len(steps)
                break
            action_index, action_data = action
            segment = steps[index:action_index]
            if segment:
                self._route = [(step_x, step_z) for step_x, step_z in steps]
                self._route_target = plan.target
                self._write_state()
                self._walk_steps(segment, run, deadline)
            self._route_action = {
                "type": action_data.kind,
                "x": action_data.x,
                "z": action_data.z,
                "id": action_data.loc_id,
                "name": action_data.name,
                "op": action_data.op,
            }
            self._action_route = [(step_x, step_z) for step_x, step_z in steps]
            self._write_state()
            if not self._open_action(action_data, deadline):
                self._route_action = None
                self._action_route = []
                return False
            self._route_action = None
            self._action_route = []
            index = action_index + 1
        self._route = []
        self._route_target = None
        return True

    def _walk_steps(self, steps, run, deadline):
        for offset in range(0, len(steps), STEP_CAP):
            chunk = list(steps[offset:offset + STEP_CAP])
            if not chunk:
                continue
            self._send_walk(chunk, run)
            limit = min(deadline, time.time() + len(chunk) * 2 + 10)
            try:
                self._await_leg(chunk, limit)
                continue
            except TimeoutError as error:
                print("walk leg stalled: %s" % (error,), flush=True)
                # A refused step is almost always a closed door ahead: open
                # it, then replan from the new position. Replaying the same
                # chunk would stall on the very same tile forever.
                if self._open_door_toward(chunk[0]):
                    raise _DoorOpened()
                raise

    def _door_spot(self, x, z, level):
        for spot in self._wall_doors:
            if spot[0] == x and spot[1] == z and spot[2] == level:
                return spot
        return None

    def _door_standing_sides(self, spot):
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
                    flags = self._collision.get(side_x, side_z, level)
                except (KeyError, IndexError):
                    continue
                if not collision_mod.tile_walkable(flags):
                    continue
                if wall is not None and flags & wall:
                    continue
                sides.append((side_x, side_z))
            return sides
        tiles = {(tile_x, tile_z) for tile_x, tile_z, _lvl in self._door_footprints[id(spot)]} or {(x, z)}
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
            flags = self._collision.get(side[0], side[1], level)
            if not collision_mod.tile_walkable(flags):
                continue
            if flags & flag:
                continue
            sides.append(side)
        return sorted(sides, key=lambda tile: abs(tile[0] - self.world.x) + abs(tile[1] - self.world.z))

    def _open_action(self, action, deadline):
        if action.kind != "door":
            return False
        spot = self._door_spot(action.x, action.z, action.level)
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
                if not self._stand_on_door_side(spot, timeout=10):
                    print("door %d,%d has no reachable side" % (action.x, action.z), flush=True)
                    return False
            # The plan crosses this tile: entry steps succeed even onto a
            # closed door, so only a real click opens the way. The click
            # itself resolves the live loc id (a stale static id is
            # silently ignored by the server once the door changed state).
            through = self._open_door_inner(action.x, action.z, deadline)
            self._recent_doors[(action.x, action.z)] = time.time()
            if through:
                self._unseal(action.x, action.z, action.level)
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
        for spot in self._wall_doors:
            if spot[2] != self.world.level:
                continue
            if abs(spot[0] - self.world.x) + abs(spot[1] - self.world.z) > radius:
                continue
            if not self._door_closed_at(spot):
                continue
            if time.time() - self._recent_doors.get((spot[0], spot[1]), 0) < 10:
                continue
            distance = abs(spot[0] - self.world.x) + abs(spot[1] - self.world.z)
            if best is None or distance < best[0]:
                best = (distance, spot)
        if best is None:
            return False
        spot = best[1]
        self._dbg("door toward %d,%d" % (spot[0], spot[1]))
        if (self.world.x, self.world.z) in [(t[0], t[1]) for t in self._door_footprints.get(id(spot), ())]:
            # Standing on the door tile itself: stepping straight off with a
            # bare packet is the only thing that can work here (the router
            # cannot plan off this tile, and a click from the doorway tile
            # is the last resort, not the first).
            if self._raw_step_off():
                return True
            op = doors_mod.open_op(self._loctypes, spot[9])
            if op is not None and id(spot) not in self._opening:
                print("clicking %d,%d from its own tile (last resort)" % (spot[0], spot[1]), flush=True)
                self._opening.add(id(spot))
                try:
                    self._send_oploc(op, spot[0], spot[1], spot[9])
                    self._await_loc_change(spot[0], spot[1], spot[2], spot[9], time.time() + DOOR_WAIT)
                finally:
                    self._opening.discard(id(spot))
                if self._raw_step_off():
                    self._unseal(spot[0], spot[1], spot[2])
                    return True
            return False
        if id(spot) in self._opening:
            return False
        op = doors_mod.open_op(self._loctypes, spot[9])
        if op is None:
            return False
        print("step refused toward %s, opening door at %d,%d" % (step, spot[0], spot[1]), flush=True)
        self._opening.add(id(spot))
        try:
            if not self._stand_on_door_side(spot, timeout=10):
                return False
            # The click resolves the live loc id itself (a stale static id
            # is silently ignored by the server once the door changed).
            if self._open_door_inner(spot[0], spot[1], time.time() + 10):
                self._recent_doors[(spot[0], spot[1])] = time.time()
                self._door_failures.pop(id(spot), None)
                return True
            print("door %d,%d refused the click" % (spot[0], spot[1]), flush=True)
            self._recent_doors[(spot[0], spot[1])] = time.time()
            if self._door_failures.get(id(spot), 0) + 1 >= 2:
                print("door %d,%d blocked after repeated refusals" % (spot[0], spot[1]), flush=True)
                self._block_door(spot)
            else:
                self._door_failures[id(spot)] = self._door_failures.get(id(spot), 0) + 1
            return False
        finally:
            self._opening.discard(id(spot))

    def _try_step_on(self, x, z, level, deadline):
        """Walk one tile onto the door and report whether the server agreed."""
        if self.world.level != level:
            return False
        if (self.world.x, self.world.z) == (x, z):
            return True
        if abs(self.world.x - x) + abs(self.world.z - z) != 1:
            return False
        if (x, z, level) in (self._stair_tiles or ()):
            # Never step onto a transition tile by accident: the server
            # would move the player to another floor mid-route.
            return False
        before = (self.world.x, self.world.z)
        self._send_walk([(x, z)], False)
        limit = time.time() + min(1.0, max(0.2, deadline - time.time()))
        while time.time() < limit:
            self.pump(1)
            if (self.world.x, self.world.z) != before:
                return True
        return False

    def _walk_single(self, x, z, run, deadline, open_doors):

        if open_doors:
            # Standing on a closed gate blocks every direction: open it, or
            # step off, before any planning.
            stuck = self._standing_on_closed_door()
            if stuck is not None:
                if self._go_open_door(stuck, deadline):
                    return True
                if self._step_off_door(stuck, deadline):
                    return True
                self._block_door(stuck)
        if open_doors and self._planner is not None:
            for attempt in range(2):
                plan = self._planner.plan(
                    self.world.level, self.world.x, self.world.z, x, z,
                    self._door_closed_at, self._blocked_doors | self._opening, self._planning_map,
                )
                if plan is None:
                    break
                try:
                    if self._walk_plan(plan, run, deadline):
                        return True
                    print("planned route hit a closed door at %s, replanning" % (self.world.position,), flush=True)
                except _DoorOpened:
                    continue
                except TimeoutError as error:
                    print("planned route timed out at %s: %s" % (self.world.position, error), flush=True)
                if attempt == 0:
                    if self._forget_unsealed_near(radius=8):
                        continue
                    if self._escape_stuck_tile():
                        continue
                break
            self._route_action = None
            self._action_route = []
        if open_doors:
            self._sync_open_doors()
        idle_rounds = 0
        leg_timeouts = 0
        recent_spots = []
        while (self.world.x, self.world.z, self.world.level) != (x, z, self.world.level):
            if time.time() > deadline:
                raise TimeoutError("walk to %d,%d timed out at %s" % (x, z, self.world.position))
            # The server drops clients that fall behind the tick: pump for
            # keepalive even on pure-planning iterations with no leg to walk.
            if getattr(self.connection, "sock", None) is not None:
                self.pump(1)
            if time.time() > deadline:
                raise TimeoutError("walk to %d,%d timed out at %s" % (x, z, self.world.position))
            # Tight-loop guard: three timed-out legs in a row means the
            # route is refused, not slow (door ping-pong, wall the maps
            # think is open). Fail fast so the caller replans instead of
            # grinding until the walk deadline. A backstop iteration cap
            # catches loops that keep moving without approaching, and a
            # spot-cycle check catches ping-pong between adjacent tiles
            # whose legs all technically arrive.
            recent_spots.append((self.world.level, self.world.x, self.world.z))
            del recent_spots[:-12]
            if leg_timeouts >= 3 or idle_rounds >= 60 or (len(recent_spots) >= 12 and len(set(recent_spots)) <= 2):
                raise TimeoutError("walk to %d,%d stuck at %s" % (x, z, self.world.position))
            idle_rounds += 1
            # Rebuilt maps are new objects (_rebuild_opened copies): always
            # route on the current one so temp blocks and unseals apply.
            routing_map = self._opened_map if open_doors else self._collision
            if open_doors:
                stuck = self._standing_on_closed_door()
                if stuck is not None:
                    if self._go_open_door(stuck, deadline):
                        continue
                    if self._step_off_door(stuck, deadline):
                        continue
                    self._block_door(stuck)
                    continue
            path = self._router.find_path(routing_map, self.world.level, self.world.x, self.world.z, x, z)
            path = [step for step in path if step[0] == self.world.level]
            if not path:
                if open_doors and self._planning_map is not None:
                    path = self._router.find_path(self._planning_map, self.world.level, self.world.x, self.world.z, x, z)
                    path = [step for step in path if step[0] == self.world.level]
                    if path:
                        print("planning through known doors to %d,%d" % (x, z), flush=True)
                if not path and open_doors:
                    path = self._router.find_path(self._collision, self.world.level, self.world.x, self.world.z, x, z)
                    path = [step for step in path if step[0] == self.world.level]
                if not path:
                    if open_doors and self._open_nearby_door(deadline, (x, z)):
                        continue
                    if open_doors and self._click_doors_around():
                        continue
                    raise Unreachable(self._explain_unreachable(x, z, open_doors))
            self._route = [(step_x, step_z) for _level, step_x, step_z in path[:64]]
            self._route_target = (x, z)
            if open_doors:
                door = self._door_on_path(path, limit=None)
                if door is not None:
                    door_op = doors_mod.open_op(self._loctypes, door[9])
                    self._route_action = {
                        "type": "loc",
                        "x": door[0],
                        "z": door[1],
                        "id": door[9],
                        "name": self._loc_name_at(door[0], door[1], door[2]) or "",
                        "op": door_op,
                    }
                    self._action_route = self._route
                    self._write_state()
                    if self._go_open_door(door, deadline):
                        self._route_action = None
                        self._action_route = []
                        continue
                    self._route_action = None
                    self._action_route = []
                    # No permanent blacklist: a failed approach usually means a
                    # stale door state, not a locked door. Cooldown instead so
                    # the next plan tries another door; locked doors stay shut
                    # and keep failing fast through the router.
                    self._recent_doors[(door[0], door[1])] = time.time()
                    continue
            self._write_state()  # show the route at once, not at the next heartbeat
            leg = self._expand(path[:STEP_CAP])
            before = (self.world.x, self.world.z)
            self._send_walk(leg, run)
            try:
                self._await_leg(leg, min(deadline, time.time() + len(leg) * 2 + 10))
            except TimeoutError as error:
                print("walk leg to %s timed out, recomputing: %s" % (leg[-1], error), flush=True)
                if (self.world.x, self.world.z) == before:
                    leg_timeouts += 1
                    if leg_timeouts >= 3:
                        # Same start three legs running: the first step is
                        # refused, not slow. Fail fast into a replan instead
                        # of burning another full leg timeout.
                        raise TimeoutError("walk to %d,%d stuck at %s" % (x, z, self.world.position))
                else:
                    leg_timeouts = 0
            else:
                leg_timeouts = 0
                if (self.world.x, self.world.z) == before:
                    # Zero progress: the server refused the first step,
                    # usually a closed door the plan missed (diagonal
                    # corner-cut) or mapped open. Click the blocker first;
                    # stale-open and step-off come after. Applies to the
                    # final approach too: its target can sit behind a door.
                    if self._open_door_toward(leg[0]):
                        continue
                if (self.world.x, self.world.z) == before and (leg[-1][0], leg[-1][1]) != (x, z):
                    # A stale "open" can hide a door the server closed behind
                    # us (we may even stand on it). Expose it and step off
                    # before barring any tile: blocking the escape route
                    # seals us in.
                    if self._forget_unsealed_near(radius=8):
                        continue
                    if self._escape_stuck_tile():
                        continue
                    # Zero progress and not the final approach: something the
                    # maps cannot see (an open door half, a desync) sits on
                    # the first step. Bar it briefly and recompute around it
                    # instead of hammering it until the walk deadline.
                    print("DIAG no-progress dest=%s,%s leg0=%s legN=%s blockednow=%s" % (
                        x, z, leg[0], leg[-1],
                        collision_mod.tile_walkable(routing_map.get(leg[0][0], leg[0][1], self.world.level)),
                    ), flush=True)
                    self._temp_block(leg[0][0], leg[0][1], self.world.level)
                    self._rebuild_opened()
                continue
        self._route = []
        self._route_target = None
        self._action_route = []
        self._route_action = None
        return True

    def _walk_raw(self, x, z, level, deadline):
        while (self.world.x, self.world.z, self.world.level) != (x, z, level):
            if time.time() > deadline:
                raise TimeoutError("walk to %d,%d timed out at %s" % (x, z, self.world.position))
            path = self._router.find_path(self._opened_map, level, self.world.x, self.world.z, x, z)
            path = [step for step in path if step[0] == level]
            if not path:
                raise Unreachable("no path to %d,%d" % (x, z))
            leg = self._expand(path[:STEP_CAP])
            self._send_walk(leg, False)
            try:
                self._await_leg(leg, min(deadline, time.time() + len(leg) * 2 + 10))
            except TimeoutError as error:
                print("raw walk leg to %s timed out, recomputing: %s" % (leg[-1], error), flush=True)
                continue
        return True

    def _stand_on_door_side(self, spot, timeout=10):
        """Stand on a tile the server accepts clicks from, trying each side.

        Only the first side used to be tried: when it sat behind another
        wall the bot burned the whole timeout and never clicked at all.
        """
        sides = self._door_standing_sides(spot)
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
        routing = self._planning_map or self._opened_map
        ranked = []
        for side in sides:
            path = self._router.find_path(routing, spot[2], self.world.x, self.world.z, side[0], side[1])
            ranked.append((0 if path else 1, len(path), side))
        ranked.sort()
        deadline = time.time() + timeout
        for _, _, (side_x, side_z) in ranked:
            if time.time() > deadline:
                break
            try:
                self.walk_to(side_x, side_z, spot[2], timeout=min(4, max(3, deadline - time.time())))
            except (Unreachable, TimeoutError) as error:
                print("door side %d,%d failed: %s" % (side_x, side_z, error), flush=True)
                continue
            if (self.world.x, self.world.z) == (side_x, side_z):
                self._dbg("door side ok %d,%d" % (side_x, side_z))
                return True
        return (self.world.x, self.world.z) in sides

    def open_door(self, x, z, timeout=25):
        self._ensure_nav()
        spot = self._door_spot(x, z, self.world.level)
        if spot is not None:
            if id(spot) in self._opening:
                return False
            self._opening.add(id(spot))
        try:
            return self._open_door_inner(x, z, timeout)
        finally:
            if spot is not None:
                self._opening.discard(id(spot))

    def _open_door_inner(self, x, z, timeout):
        level = self.world.level
        deadline = time.time() + timeout
        spot = self._door_spot(x, z, level)
        self._unseal(x, z, level)
        loc = self.world.loc_at(x, z, level)
        if loc is None and spot is None:
            return True
        op = None
        loc_id = None
        if loc is not None:
            op = doors_mod.open_op(self._loctypes, loc[0])
            if op is None:
                return True
            loc_id = loc[0]
        if op is None and spot is not None:
            # No dynamic loc (stale zone): the static door still needs a real
            # click, it must not be waved through.
            op = doors_mod.open_op(self._loctypes, spot[9])
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
                    self.walk_to(nx, nz, level, timeout=max(5, deadline - time.time()), open_doors=True)
                except (Unreachable, TimeoutError) as error:
                    print("door step to %d,%d failed: %s" % (nx, nz, error), flush=True)
                    continue
            seen_messages = len(self.world.messages)
            self._dbg("door click %d,%d id=%s" % (x, z, loc_id))
            self._send_oploc(op, x, z, loc_id)
            if self._await_loc_change(x, z, level, loc_id, min(deadline, time.time() + DOOR_WAIT)):
                if self._try_step_on(x, z, level, time.time() + 2.0):
                    self._unseal(x, z, level)
                    self._dbg("door through %d,%d" % (x, z))
                    return True
                continue
            messages = " ".join(self.world.messages[seen_messages:]).lower()
            if any(word in messages for word in ("locked", "can't reach", "nothing interesting")):
                continue
            # The server often applies the open without the client seeing the
            # zone loc update: only unblock after proving the step, otherwise
            # rescind the optimistic double-pair unseal done above.
            if self._try_step_on(x, z, level, time.time() + 2.0):
                self._unseal(x, z, level)
                return True
            self._reseal(x, z, level)
            continue
        self._rebuild_opened()
        return False

    def _reseal(self, x, z, level):
        """Undo an optimistic unseal whose click was refused.

        Drops only our own marker (a dynamic None): a real zone update
        showing something else is server truth and stays untouched.
        """
        if self.world.locs.get((x, z, level)) is None:
            self.world.locs.pop((x, z, level), None)
        self._unsealed_doors.discard((x, z, level))
        for spot in self._wall_doors:
            if spot[2] != level or abs(spot[0] - x) + abs(spot[1] - z) > 1:
                continue
            if self._loctypes["loc"][spot[9]].get("category", -1) not in (21, 22, 23, 24):
                continue
            if self.world.locs.get((spot[0], spot[1], level)) is None:
                self.world.locs.pop((spot[0], spot[1], level), None)
            self._unsealed_doors.discard((spot[0], spot[1], level))
        self._rebuild_opened()

    def _click_doors_around(self, radius=6):
        """Click every closed door in reach before giving up on a walk.

        Gates are often two locs sharing a fence line, and a failed approach
        used to blacklist them for the rest of the session.
        """
        clicked = False
        for spot in self._wall_doors:
            if spot[2] != self.world.level:
                continue
            if abs(spot[0] - self.world.x) + abs(spot[1] - self.world.z) > radius:
                continue
            if not self._door_closed_at(spot):
                continue
            op = doors_mod.open_op(self._loctypes, spot[9])
            if op is None:
                continue
            self._send_oploc(op, spot[0], spot[1], spot[9])
            clicked = True
        if not clicked:
            return False
        self.pump(4)
        for spot in self._wall_doors:
            if spot[2] != self.world.level:
                continue
            if abs(spot[0] - self.world.x) + abs(spot[1] - self.world.z) > radius:
                continue
            self._unseal(spot[0], spot[1], spot[2])
            self._recent_doors[(spot[0], spot[1])] = time.time()
        self._blocked_doors.clear()
        self._blocked_doors_exp.clear()
        return True

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
            if not collision_mod.tile_walkable(self._collision.get(nx, nz, level)):
                continue
            if (nx, nz, level) in (self._stair_tiles or ()):
                # Never step onto a transition tile by accident: the server
                # would move the player to another floor mid-route.
                continue
            self._send_walk([(nx, nz)], False)
            deadline = time.time() + 1.0
            while time.time() < deadline:
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
        for spot in self._wall_doors:
            key = (spot[0], spot[1], spot[2])
            if abs(key[0] - self.world.x) + abs(key[1] - self.world.z) > radius:
                continue
            if key in self.world.locs:
                self.world.locs.pop(key, None)
                changed = True
            if key in self._unsealed_doors:
                self._unsealed_doors.discard(key)
                changed = True
        if changed:
            self._rebuild_opened()
        return changed

    def _unseal(self, x, z, level):
        self._unsealed_doors.add((x, z, level))
        # The server opened it; drop the stale closed loc locally even when the
        # zone update never reaches this client.
        self.world.locs[(x, z, level)] = None
        self._apply_removal(x, z, level)

    def _apply_removal_to(self, target, x, z, level):
        for loc in (self.world.static_locs.get((x, z, level)), self.world.loc_at(x, z, level)):
            if loc is None:
                continue
            loc_id, shape, angle = loc
            entry = self._loctypes["loc"][loc_id]
            collision_mod.change_loc_collision(
                target, shape, angle, entry["blockrange"], entry["length"], entry["width"], entry["active"], x, z, level, False
            )

    def _apply_removal(self, x, z, level):
        self._apply_removal_to(self._opened_map, x, z, level)

    def _open_nearby_door(self, deadline, target=None):
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
        for spot in self._wall_doors or ():
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
            if self.open_door(x, z, timeout=max(5, deadline - time.time())):
                return True
        return False

    def can_reach(self, x, z, level=None, near=1):
        """True when a path gets adjacent, planning through known doors."""
        self._ensure_nav()
        level = self.world.level if level is None else level
        if level != self.world.level:
            return False
        if abs(self.world.x - x) + abs(self.world.z - z) <= near:
            # Already there: find_path compresses the start tile away and
            # reports [], which must not read as unreachable.
            return True
        # Full-window routing is CPU-heavy and callers loop over many
        # targets (every bank): keep the connection alive between maps.
        if getattr(self.connection, "sock", None) is not None:
            self.pump(1)
        for routing_map in (self._planning_map, self._opened_map):
            if routing_map is None:
                continue
            # Full waypoint list: the default cap truncates long routes
            # mid-way, and the truncated end is never near the target.
            path = self._router.find_path(routing_map, level, self.world.x, self.world.z, x, z, max_waypoints=10000)
            if path and abs(path[-1][1] - x) + abs(path[-1][2] - z) <= near:
                return True
        return False

    def open_doors_near(self, x, z, radius=8, timeout=60):
        """Open closed doors around a target before routing into a building."""
        self._ensure_nav()
        deadline = time.time() + timeout
        candidates = [
            spot for spot in self._wall_doors
            if spot[2] == self.world.level and abs(spot[0] - x) + abs(spot[1] - z) <= radius
        ]
        candidates.sort(key=lambda spot: abs(spot[0] - x) + abs(spot[1] - z))
        for spot in candidates:
            if not self._door_closed_at(spot):
                continue
            if time.time() > deadline:
                return False
            remaining = max(5, deadline - time.time())
            try:
                self.open_door(spot[0], spot[1], timeout=remaining)
            except (Unreachable, TimeoutError) as error:
                print("door near %d,%d failed: %s" % (x, z, error), flush=True)
                continue
        return True

    def _door_on_path(self, path, limit=12):
        """First closed door crossed by the path, by step order."""
        steps = route_mod.expand_steps(path, (self.world.x, self.world.z), 256)
        if limit is not None:
            steps = steps[:limit]
        index_by_tile = {tile: index for index, tile in enumerate(steps)}
        best = None
        for spot in self._wall_doors:
            if spot[2] != self.world.level:
                # Footprints are matched as bare (x, z) tiles below: without
                # this guard a door on another floor (tower L2 over L0)
                # matches the current path, and opening it calls walk_to on
                # the wrong level, which a portal route always refuses.
                continue
            if id(spot) in self._blocked_doors:
                continue
            if id(spot) in self._opening:
                continue
            if time.time() - self._recent_doors.get((spot[0], spot[1]), 0) < 30:
                continue
            indices = [
                index_by_tile[(tile_x, tile_z)]
                for tile_x, tile_z, _level in self._door_footprints[id(spot)]
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
        return doors_mod.open_op(self._loctypes, loc[0]) is not None

    def _block_door(self, spot):
        # Temporary, not forever: a blocked door on the only path (castle
        # exit, tower door) would otherwise disconnect the map for the rest
        # of the session and every later route reports "no route".
        self._blocked_doors.add(id(spot))
        self._blocked_doors_exp[id(spot)] = time.time() + 120
        self._rebuild_opened()

    def _sweep_blocked_doors(self):
        now = time.time()
        for key in [key for key, expiry in self._blocked_doors_exp.items() if expiry <= now]:
            self._blocked_doors_exp.pop(key, None)
            self._blocked_doors.discard(key)

    def _rebuild_opened(self):
        self._sweep_blocked_doors()
        self._opened_map = self._collision.copy()
        for key in self._unsealed_doors:
            self._apply_removal(*key)
        if self._stair_tiles:
            # Bots never climb on purpose: stepping on a stair/ladder tile
            # makes the server move us upstairs, stranding the run. Route
            # around them; explicit op_loc on stairs still works adjacent.
            for stair_x, stair_z, stair_level in self._stair_tiles:
                try:
                    self._opened_map.set(stair_x, stair_z, stair_level, collision_mod.WALK_BLOCKED)
                except (KeyError, IndexError) as error:
                    print("cannot block stair tile %d,%d,%d: %s" % (
                        stair_x, stair_z, stair_level, error
                    ), flush=True)
        self._apply_temp_blocks(self._opened_map)
        self._rebuild_planning()

    def _rebuild_planning(self):
        """Routing map with every known door open, for planning through rooms."""
        if self._collision is None:
            self._planning_map = None
            return
        self._planning_map = self._collision.copy()
        for spot in self._wall_doors or ():
            if id(spot) in self._blocked_doors:
                continue
            self._apply_removal_to(self._planning_map, spot[0], spot[1], spot[2])
        for stair_x, stair_z, stair_level in self._stair_tiles or ():
            try:
                self._planning_map.set(stair_x, stair_z, stair_level, collision_mod.WALK_BLOCKED)
            except (KeyError, IndexError) as error:
                print("cannot block stair in planning map %d,%d,%d: %s" % (
                    stair_x, stair_z, stair_level, error
                ), flush=True)
        self._apply_temp_blocks(self._planning_map)

    def _temp_block(self, x, z, level, seconds=15):
        """Bar a tile briefly: the server refuses it but the maps say free.

        Capped and short-lived: an unbounded block list would seal the bot
        into a pocket of its own making (every stall adds one, and a
        deterministic router keeps returning the same first step).
        """
        self._blocked_tiles[(x, z, level)] = time.time() + seconds
        while len(self._blocked_tiles) > 8:
            oldest = min(self._blocked_tiles, key=self._blocked_tiles.get)
            del self._blocked_tiles[oldest]

    def _apply_temp_blocks(self, target):
        now = time.time()
        for key in [key for key, expiry in self._blocked_tiles.items() if expiry <= now]:
            del self._blocked_tiles[key]
        for (x, z, level) in self._blocked_tiles:
            try:
                target.set(x, z, level, collision_mod.WALK_BLOCKED)
            except (KeyError, IndexError):
                pass

    def _standing_on_closed_door(self):
        here = (self.world.x, self.world.z, self.world.level)
        for spot in self._wall_doors:
            if id(spot) in self._blocked_doors:
                continue
            if time.time() - self._recent_doors.get((spot[0], spot[1]), 0) < 30:
                continue
            if here in self._door_footprints[id(spot)] and self._door_closed_at(spot):
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
            if any((nx, nz, self.world.level) in self._door_footprints[id(other)] for other in self._wall_doors):
                continue
            if (nx, nz, self.world.level) in (self._stair_tiles or ()):
                # Never step onto a transition tile by accident: the server
                # would move the player to another floor mid-route.
                continue
            if not collision_mod.tile_walkable(self._collision.get(nx, nz, self.world.level)):
                continue
            if self._try_step_on(nx, nz, self.world.level, time.time() + 1.5):
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
                if any((nx, nz, self.world.level) in self._door_footprints[id(other)] for other in self._wall_doors):
                    continue
                try:
                    self.walk_to(nx, nz, self.world.level, timeout=max(5, deadline - time.time()), open_doors=False)
                    return True
                except (Unreachable, TimeoutError) as error:
                    print("door step-off to %d,%d failed: %s" % (nx, nz, error), flush=True)
                    continue
        return False

    def _go_open_door(self, spot, deadline):
        # Reaching a door can mean crossing other doors first (castle halls):
        # walk there with doors allowed. The target itself stays out of any
        # nested plan via _opening so the approach never recurses into it.
        if id(spot) in self._opening:
            return False
        self._opening.add(id(spot))
        try:
            x, z = spot[0], spot[1]
            if not self._stand_on_door_side(spot, timeout=10):
                self._recent_doors[(x, z)] = time.time()
                return False
            if self.open_door(x, z, timeout=max(5, deadline - time.time())):
                self._recent_doors[(x, z)] = time.time()
                self._door_failures.pop(id(spot), None)
                return True
            self._recent_doors[(x, z)] = time.time()
            if self._door_failures.get(id(spot), 0) + 1 >= 2:
                print("door %d,%d blocked after repeated refusals" % (x, z), flush=True)
                self._block_door(spot)
            else:
                self._door_failures[id(spot)] = self._door_failures.get(id(spot), 0) + 1
            return False
        finally:
            self._opening.discard(id(spot))

    def _send_oploc(self, op, x, z, loc_id):
        body = Writer()
        body.u16(x)
        body.u16(z)
        body.u16(loc_id)
        self.connection.send(client_op_id("OPLOC%d" % op), body.bytes())

    def _await_loc_change(self, x, z, level, was, deadline):

        while time.time() < deadline:
            self.pump(1)
            now = self.world.loc_at(x, z, level)
            if now is None or now[0] != was:
                return True
        return False

    def _ensure_nav(self):
        if self._collision is not None:
            return
        self._loctypes = collision_mod.load_types(self._paths["types"])
        self._collision, static_locs = collision_mod.load_maps(self._paths["pack_dir"], self._loctypes)
        self.world.static_locs = static_locs
        self.world.static_objs = {}
        for x, z, level, oid, count in objs_mod.load_static_objs(self._paths["pack_dir"]):
            self.world.static_objs[((x, z, level), oid)] = count
        self._wall_doors = [
            spot for spot in doors_mod.find_openables(self._paths["pack_dir"], self._loctypes)
            if doors_mod.is_door(self._loctypes, spot[9])
        ]
        self._door_footprints = {id(spot): doors_mod.footprint(spot) for spot in self._wall_doors}
        self._wall_door_keys = {(spot[0], spot[1], spot[2]) for spot in self._wall_doors}
        self._load_portals()
        # Never step on a transition tile by accident: the server would move
        # the player to another floor mid-route. Block the whole footprint,
        # not just the origin tile: a 2x2 staircase has three other tiles a
        # route could otherwise cut through and trigger a surprise climb.
        self._stair_tiles = set()
        for row in self._portal_rows:
            for tile_x, tile_z in row.get("footprint") or [[row["from"][1], row["from"][2]]]:
                self._stair_tiles.add((tile_x, tile_z, row["from"][0]))
        self._rebuild_opened()
        self._planner = nav_mod.Planner(self._router, self._collision, self._loctypes, self._wall_doors)
        for spot in self._wall_doors:
            key = (spot[0], spot[1], spot[2])
            if key in self.world.locs:
                loc = self.world.locs[key]
                if loc is None or doors_mod.open_op(self._loctypes, loc[0]) is None:
                    self._unseal(*key)

    def _load_portals(self):
        self._portals = []
        self._portal_rows = []
        path = os.path.join(os.path.dirname(os.path.dirname(__file__)), "data", "transitions.json")
        if not os.path.exists(path):
            raise StateError("missing transition table %r, run scripts/dump_transitions.py" % (path,))
        with open(path) as handle:
            rows = json.load(handle)
        for index, row in enumerate(rows):
            if row.get("disabled"):
                continue
            try:
                self._portals.append(
                    portals_mod.Portal(
                        row.get("name", "portal"),
                        tuple(row["from"]),
                        tuple(row["to"]),
                        lambda bot, row=row: bot._ride_portal(row),
                    )
                )
                self._portal_rows.append(row)
            except (KeyError, TypeError, ValueError) as error:
                raise StateError("invalid portal %d in %r: %s" % (index, path, error)) from error

    def ride_transition_at(self, x, z, level):
        """Ride the known stair/ladder/trapdoor that sits on this tile."""
        for row in self._portal_rows:
            if row.get("disabled"):
                continue
            if tuple(row["from"]) == (level, x, z):
                return self._ride_portal(row)
        return False

    def _ride_portal(self, row, deadline=None):
        level, frm_x, frm_z = row["from"]
        delta = row.get("delta")
        sides = self._transition_sides(level, frm_x, frm_z, delta, row.get("footprint"))
        if not sides:
            print("transition %s has no reachable side from %s" % (row.get("name"), self.world.position), flush=True)
            return False
        for side_x, side_z in sides:
            if deadline is not None and time.time() > deadline:
                print("transition %s: out of time at %s" % (row.get("name"), self.world.position), flush=True)
                break
            timeout = 45 if deadline is None else max(3.0, deadline - time.time())
            if not self._stand_beside(level, side_x, side_z, timeout):
                continue
            if not self._open_transition(row, level, frm_x, frm_z):
                continue
            expected = (
                (level + delta[0], self.world.x + delta[1], self.world.z + delta[2])
                if delta else tuple(row["to"])
            )
            if self._await_teleport(row, expected, seconds=2.0, quiet=True):
                return True
            print("transition %s: side %d,%d refused" % (row.get("name"), side_x, side_z), flush=True)
        print("transition %s failed at %s" % (row.get("name"), self.world.position), flush=True)
        return False

    def _open_transition(self, row, level, frm_x, frm_z):
        opened = row.get("open_to")
        loc = self.world.loc_at(frm_x, frm_z, level)
        if opened is not None and (loc is None or loc[0] != opened):
            self.op_loc(1, frm_x, frm_z, row.get("loc") or row["climb_loc"])
            deadline = self.world.ticks + 20
            while self.world.ticks < deadline:
                self.pump(2)
                loc = self.world.loc_at(frm_x, frm_z, level)
                if loc is not None and loc[0] == opened:
                    break
        self.op_loc(row.get("climb_op", 1), frm_x, frm_z, row["climb_loc"])
        return True

    def _transition_sides(self, level, x, z, delta=None, footprint=None):
        """The orthogonal tiles a stair/ladder can be clicked from.

        The server's reach check for rectangular scenery only accepts
        orthogonal sides, and a multi-tile staircase is entered from any tile
        around its whole footprint. A ladder whose script moves the player
        from their own tile also needs the landing tile to be walkable.
        """
        def lands_ok(tile_x, tile_z):
            if not delta:
                return True
            dlevel, dx, dz = delta
            return collision_mod.tile_walkable(self._collision.get(tile_x + dx, tile_z + dz, level + dlevel))

        tiles = [(tile_x, tile_z) for tile_x, tile_z in (footprint or [[x, z]])]
        inside = set(tiles)
        facing = {}
        for tile_x, tile_z in tiles:
            for side_x, side_z, wall in (
                (tile_x - 1, tile_z, collision_mod.WALL_EAST),
                (tile_x + 1, tile_z, collision_mod.WALL_WEST),
                (tile_x, tile_z - 1, collision_mod.WALL_NORTH),
                (tile_x, tile_z + 1, collision_mod.WALL_SOUTH),
            ):
                if (side_x, side_z) in inside or (side_x, side_z) in facing:
                    continue
                facing[(side_x, side_z)] = wall
        # Stair tiles are valid sides here: standing on one is exactly how a
        # neighbour staircase is used, and the click names its own loc.
        sides = [
            side for side in facing
            if collision_mod.tile_walkable(self._collision.get(side[0], side[1], level))
        ]
        # Rank, never filter: the server teleports onto blocked tiles (ship
        # decks, stairwells), so a side whose landing looks wrong can still
        # be the only one it accepts. Reachability first (a side nobody can
        # stand on is useless), then its reachRectangle1 (no wall facing the
        # loc), then a walkable landing, then the real walking distance.
        routing = self._planning_map or self._opened_map
        stairs = self._stair_tiles or set()
        ranked = []
        for side in sides:
            walled = bool(self._collision.get(side[0], side[1], level) & facing[side])
            if (self.world.level, self.world.x, self.world.z) == (level, side[0], side[1]):
                # find_path compresses a zero-length route to []: rank
                # standing-put as the best side instead of unwalkable.
                ranked.append((0, walled, not lands_ok(*side), False, -1, side))
                continue
            path = self._router.find_path(routing, level, self.world.x, self.world.z, side[0], side[1])
            stair = (side[0], side[1], level) in stairs
            ranked.append((0 if path else 1, walled, not lands_ok(*side), stair, len(path), side))
        return [side for _found, _walled, _landing, _stair, _length, side in sorted(ranked)]

    def _stand_beside(self, level, x, z, timeout=45):
        if self.world.level == level and (self.world.x, self.world.z) == (x, z):
            return True
        try:
            self.walk_to(x, z, level, timeout=timeout)
        except (Unreachable, TimeoutError) as error:
            print("transition step to %d,%d failed: %s" % (x, z, error), flush=True)
            return False
        return (self.world.level, self.world.x, self.world.z) == (level, x, z)

    def _await_teleport(self, row, expected, seconds=3.0, quiet=False):
        # Wall clock, not ticks: the dev server ticks every 20 ms, where a
        # tick budget is far shorter than the round trip.
        to_level, to_x, to_z = expected
        deadline = time.time() + seconds
        while time.time() < deadline:
            self.pump(2)
            if self.world.level == to_level and abs(self.world.x - to_x) + abs(self.world.z - to_z) <= 1:
                return True
        if not quiet:
            print("transition %s ended at %s, wanted %s" % (
                row.get("name"), self.world.position, tuple(expected)
            ), flush=True)
        return False

    def _plan_portals(self, start, dest, skip=(), skip_walks=()):
        routing = self._planning_map or self._opened_map
        skip = set(skip)
        skip_walks = set(skip_walks)
        now = time.time()
        for name in [name for name, expiry in self._bad_transitions.items() if expiry <= now]:
            del self._bad_transitions[name]
        skip.update(self._bad_transitions)
        # Walk edges are straight-line estimates: a staircase can look close
        # and still sit behind battlements, swamps or a missing bridge.
        # Validate every walking leg with the real router and plan again
        # without the ones that do not go through. Attempts are generous
        # because each rejection only rules out one decoy edge (dungeon
        # portals lie close as the crow flies); results are cached so
        # revalidating the survivors stays cheap.
        checked = {}
        for _attempt in range(150):
            # Planning is pure CPU and can run for a while on hard routes:
            # keep the connection alive or the server drops us mid-think.
            # (Skipped in unit tests, where the bot is never connected.)
            if getattr(self.connection, "sock", None) is not None:
                self.pump(1)
            selected = [portal for portal in self._portals if portal.name not in skip]
            if not selected:
                return None
            planner = portals_mod.RoutePlanner(self._router, routing)
            for portal in selected:
                planner.add_portal(portal)
            plan = planner.plan(start, dest, skip_walks)
            if plan is None:
                return None
            bad = self._first_bad_walk(plan, start, routing, checked)
            if bad is None:
                return plan
            print("walk leg %s -> %s looks blocked, replanning without it" % (bad[0], bad[1]), flush=True)
            skip_walks.add(bad)
        return None

    def _walk_leg_ok(self, routing, frm, to):
        """True when a planned walking leg really goes through on foot."""
        if frm[0] != to[0]:
            return True
        if abs(frm[1] - to[1]) + abs(frm[2] - to[2]) <= 2:
            # Already there or adjacent: walk_to retargets blocked tiles
            # onto the nearest reachable tile and finishes from here.
            return True
        if abs(to[1] - frm[1]) > 120 or abs(to[2] - frm[2]) > 120:
            # Beyond the router window the destination truncates, so an
            # end-near-target check is meaningless. Fail only when the
            # flood proves disconnection: a small enclosed region (wrong
            # dungeon, far shore) exhausts inside the window, while open
            # ground fills it. Without this the walker follows impossible
            # straight lines into dead ends.
            path = self._router.find_path(routing, frm[0], frm[1], frm[2], to[1], to[2], max_waypoints=10000)
            if path:
                end = path[-1]
                before = abs(frm[1] - to[1]) + abs(frm[2] - to[2])
                after = abs(end[1] - to[1]) + abs(end[2] - to[2])
                return before - after >= 15
            visited = sum(1 for distance in self._router.distances if distance < route_mod.FAR)
            return visited >= 20000
        # Full waypoint list: the default cap truncates long routes mid-way,
        # which would look like a dead end that is not one.
        path = self._router.find_path(routing, frm[0], frm[1], frm[2], to[1], to[2], max_waypoints=10000)
        if not path:
            return False
        end = path[-1]
        return abs(end[1] - to[1]) + abs(end[2] - to[2]) <= 2

    def _first_bad_walk(self, plan, start, routing, checked=None):
        """First (frm, to) walking leg with no real path, if any."""
        pos = start
        for kind, payload in plan:
            if kind == "walk":
                key = (pos, tuple(payload) if not hasattr(payload, "to") else tuple(payload.to))
                if checked is not None and key in checked:
                    ok = checked[key]
                else:
                    # Full-window router validation is CPU-heavy: keep the
                    # connection alive between legs or the server drops us.
                    if getattr(self.connection, "sock", None) is not None:
                        self.pump(1)
                    ok = self._walk_leg_ok(routing, pos, payload)
                    if checked is not None:
                        checked[key] = ok
                if not ok:
                    return (pos, payload)
                pos = payload
            else:
                pos = tuple(payload.to)
        return None

    def _expand(self, waypoints):
        return route_mod.expand_steps(waypoints, (self.world.x, self.world.z), STEP_CAP)

    def _send_walk(self, steps, run):
        self._dbg("send_walk %d steps run=%s" % (len(steps), run))
        start_x, start_z = self.world.x, self.world.z
        for x, z in steps:
            # A random-event teleport invalidates the route mid-walk: fail
            # fast with a replan instead of an OverflowError traceback.
            if abs(x - start_x) > 127 or abs(z - start_z) > 127:
                raise Unreachable("stale route after teleport at %s" % (self.world.position,))
        body = Writer()
        body.u8(1 if run else 0)
        body.u16(start_x)
        body.u16(start_z)
        for x, z in steps:
            body.i8(x - start_x)
            body.i8(z - start_z)
        self.connection.send(client_op_id("MOVE_GAMECLICK"), body.bytes())

    def _await_leg(self, leg, deadline):

        # One tick per pump: the dev server ticks every 20 ms, so reading a
        # batch of ticks can jump several tiles past the target and leave the
        # bot re-sending a stale leg (it looks like it circles).
        target = leg[-1]
        best = abs(self.world.x - target[0]) + abs(self.world.z - target[1])
        last_move = time.time()
        while True:
            position = (self.world.x, self.world.z)
            if position == target:
                self._dbg("leg arrived %s" % (target,))
                return
            distance = abs(position[0] - target[0]) + abs(position[1] - target[1])
            if distance > best:
                return  # passed it: the next leg recomputes from here
            if distance < best:
                best = distance
                last_move = time.time()
            elif time.time() - last_move > 2.0:
                # Wall clock: a 20 ms tick makes a tick budget meaningless.
                raise TimeoutError("leg to %s stalled at %s" % (target, self.world.position))
            if time.time() > deadline:
                raise TimeoutError("leg to %s timed out at %s" % (target, self.world.position))
            self.pump(1)
