"""Jambes : walk_to, legs, envoi paquets.."""

from ... import collision as collision_mod
from ... import doors as doors_mod
from ... import steps as steps_mod
from ...buffer import Writer
from ...protocol import client_op_id
from ..core import Unreachable

from .base import MovementBase, STEP_CAP, _DoorOpened, _tracks_destination


class MovementWalk(MovementBase):

    def _follow_route(self, plan, dest, run, timeout, open_doors):
        # A transition can be blocked (locked gate, quest item). Drop it and
        # plan again from where we stand instead of failing the whole route.
        # A walking leg can also turn out to be impassable (the estimate is
        # straight-line): drop that walk edge and plan again the same way.
        deadline = self.world.ticks + timeout
        skip = set()
        skip_walks = set()
        self.dbg("route plan %s" % ([(k, getattr(q, "name", q)) for k, q in plan][:12]))
        while True:
            # Replans are pure CPU; keep the server from dropping us.
            if getattr(self.connection, "sock", None) is not None:
                self.pump(1)
            leg_start = (self.world.level, self.world.x, self.world.z)
            replanned = False
            for kind, payload in plan:
                if self.world.ticks > deadline:
                    raise TimeoutError("route timed out at %s" % (self.world.position,))
                if kind == "walk":
                    try:
                        self.walk_to(payload[1], payload[2], payload[0], run=run, timeout=max(1500, deadline - self.world.ticks), open_doors=open_doors)
                    except (Unreachable, TimeoutError) as error:
                        print("walk leg to %s failed: %s, replanning" % (payload, error), flush=True)
                        skip_walks.add((leg_start, payload))
                        replanned = True
                        break
                    leg_start = payload
                    continue
                self.dbg("ride %s" % (payload.name,))
                if not payload.ride(self):
                    print("transition %s failed at %s, replanning" % (payload.name, self.world.position), flush=True)
                    skip.add(payload.name)
                    # Session-wide, with expiry: a locked door or missing key
                    # will fail the same way on every fresh plan, so remember
                    # it instead of rediscovering it on each walk_to. Short
                    # expiry: a transition failed for a transient reason
                    # (closed door, missed click) must become usable again
                    # quickly, or one hiccup detours the whole session.
                    self._bad_transitions[payload.name] = self.world.ticks + 9000
                    replanned = True
                    break
                leg_start = tuple(payload.to)
            if not replanned:
                return True
            plan = self.plan_portals((self.world.level, self.world.x, self.world.z), dest, skip, skip_walks)
            if plan is None:
                raise Unreachable("no route to %s" % (dest,))

    def approach(self, x, z, level=0, radius=2, timeout=3000, open_doors=True, paid=None):
        self.dbg("approach %d,%d,%d r=%d" % (x, z, level, radius))
        self.ensure_nav()
        # Already close enough: the exact walk below would grind a blocked
        # target tile (a closed door) for its whole budget first.
        if self.world.level == level and abs(self.world.x - x) + abs(self.world.z - z) <= radius:
            return True
        try:
            # Exact first, but briefly: a blocked target (wall, locked door)
            # can never be stood on, and grinding it burns the whole budget
            # while an adjacent tile already satisfies the radius. The budget
            # scales with distance (about 50 ticks a tile running), so a far
            # target still gets a real walk instead of a 750-tick dash.
            dist = abs(self.world.x - x) + abs(self.world.z - z)
            self.walk_to(x, z, level, timeout=min(timeout, max(750, dist * 50)), open_doors=open_doors, paid=paid)
        except (Unreachable, TimeoutError) as error:
            print("exact approach to %d,%d failed: %s%s" % (x, z, error, self._dest_suffix()), flush=True)
        if self.world.level == level and abs(self.world.x - x) + abs(self.world.z - z) <= radius:
            return True
        deadline = self.world.ticks + timeout
        here = (self.world.x, self.world.z)
        candidates = sorted(
            [(x + dx, z + dz) for dx in range(-radius, radius + 1) for dz in range(-radius, radius + 1)],
            key=lambda tile: abs(tile[0] - here[0]) + abs(tile[1] - here[1]),
        )
        for cx, cz in candidates:
            if (cx, cz) == here and self.world.level == level:
                return True
            try:
                path = self._find_path(self.opened_map if open_doors else self.collision, level, here[0], here[1], cx, cz)
                path = [step for step in path if step[0] == level]
            except (KeyError, IndexError, ValueError) as error:
                print("router failed near %s for %d,%d: %s" % (here, cx, cz, error), flush=True)
                continue
            if not path:
                continue
            try:
                self.walk_to(cx, cz, level, timeout=max(250, deadline - self.world.ticks), open_doors=open_doors, paid=paid)
            except (Unreachable, TimeoutError) as error:
                print("approach candidate %d,%d failed: %s%s" % (cx, cz, error, self._dest_suffix()), flush=True)
                continue
            if self.world.level == level and abs(self.world.x - x) + abs(self.world.z - z) <= radius:
                return True
        raise Unreachable("nothing reachable within %d of %d,%d from %s" % (
            radius, x, z, self.world.position
        ))

    def approach_npc(self, slot, timeout=3000, open_doors=True):
        """Se placer sur une tuile d'ou le clic sur l'NPC est accepte.

        approach() juge "assez pres" en distance de Manhattan, diagonale
        comprise, alors que la portee serveur ne prend que les tuiles
        adjacentes par un cote : depuis une diagonale le clic repond
        "I can't reach that!" et la cible ne reagit jamais.
        """
        self.dbg("approach_npc slot=%d" % (slot,))
        self.ensure_nav()
        deadline = self.world.ticks + timeout
        while self.world.ticks < deadline:
            # Re-read each round: wanderers move while we walk.
            npc = self.world.npcs.get(slot)
            if npc is None:
                raise Unreachable("NPC %d left view" % (slot,))
            if npc.get("level", self.world.level) != self.world.level:
                raise Unreachable("NPC %d is on level %s" % (slot, npc.get("level")))
            if self._npc_clickable(npc):
                return True
            size = self._npc_size(npc)
            # Only the orthogonal ring answers a click: the bounding box
            # corners are diagonal tiles the server refuses.
            ring = sorted(set(
                [(npc["x"] - 1, npc["z"] + dz) for dz in range(size)]
                + [(npc["x"] + size, npc["z"] + dz) for dz in range(size)]
                + [(npc["x"] + dx, npc["z"] - 1) for dx in range(size)]
                + [(npc["x"] + dx, npc["z"] + size) for dx in range(size)]
            ), key=lambda tile: abs(tile[0] - self.world.x) + abs(tile[1] - self.world.z))
            for cx, cz in ring:
                if self.world.ticks >= deadline:
                    break
                if (cx, cz) == (self.world.x, self.world.z):
                    continue
                try:
                    self.walk_to(cx, cz, self.world.level,
                                 timeout=max(250, deadline - self.world.ticks), open_doors=open_doors)
                except (Unreachable, TimeoutError) as error:
                    print("approach_npc tile %d,%d failed: %s" % (cx, cz, error), flush=True)
                    continue
                npc = self.world.npcs.get(slot)
                if npc is not None and self._npc_clickable(npc):
                    return True
            self.pump(1)
        raise Unreachable("no clickable tile for NPC %d from %s" % (
            slot, (self.world.level, self.world.x, self.world.z)))

    @_tracks_destination
    def walk_to(self, x, z, level=0, run=None, timeout=9000, open_doors=True, paid=None):
        # paid=None herite du flag ambiant (appels niches) ; True/False
        # explicite ne fuit pas hors de l'appel.
        if paid is None:
            return self._walk_to_inner(x, z, level, run, timeout, open_doors)
        previous = self.allow_paid_paths
        self.allow_paid_paths = paid
        try:
            return self._walk_to_inner(x, z, level, run, timeout, open_doors)
        finally:
            self.allow_paid_paths = previous

    def _walk_to_inner(self, x, z, level=0, run=None, timeout=9000, open_doors=True):
        # Movement, doors and known ladder/trapdoor links are the SDK's job:
        # a script only names the destination.
        if level != self.world.level:
            if self._in_portal_route:
                raise Unreachable("walk_to stays on one level inside a portal route")
            self.ensure_nav()
            plan = self.plan_portals((self.world.level, self.world.x, self.world.z), (level, x, z))
            if plan is not None:
                self._in_portal_route = True
                try:
                    return self._follow_route(plan, (level, x, z), run, timeout, open_doors)
                finally:
                    self._in_portal_route = False
            raise Unreachable("walk_to stays on one level, use a portal to change level")
        if run is None:
            run = self.want_run()
        self.ensure_nav()
        self.dbg("walk_to %d,%d,%d" % (x, z, level))
        if not self._in_portal_route and abs(x - self.world.x) + abs(z - self.world.z) > 120:
            # Same level number, disconnected region (a cellar shares
            # level 0 with the surface): no direct walk can arrive, and
            # grinding one only burns the walk timeout. Probe with the
            # doors-aware planner first; only a portal route can cross.
            try:
                direct = (
                    self._planner.plan(
                        self.world.level, self.world.x, self.world.z,
                        x, z, self._door_closed_at, self._blocked_doors | self._toll_blocked(),
                    )
                    if self._planner is not None
                    else True
                )
            except Exception:
                direct = True
            if not direct:
                plan = self.plan_portals((self.world.level, self.world.x, self.world.z), (level, x, z))
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
                plan = self.plan_portals((self.world.level, self.world.x, self.world.z), (level, x, z))
                if plan is not None and any(kind == "portal" for kind, _payload in plan):
                    self._in_portal_route = True
                    try:
                        return self._follow_route(plan, (level, x, z), run, timeout, open_doors)
                    finally:
                        self._in_portal_route = False
            # A door we believe open can be closed on the server: retry with
            # the raw map, which routes around it instead of through it.
            print("walk with doors open stalled, retrying on the raw map%s" % (self._dest_suffix(),), flush=True)
            return self._walk_direct(x, z, level, run, min(timeout, 1500), False)
        except Unreachable:
            if self._in_portal_route:
                raise
            plan = self.plan_portals((self.world.level, self.world.x, self.world.z), (level, x, z))
            if plan is None:
                raise
            self._in_portal_route = True
            try:
                return self._follow_route(plan, (level, x, z), run, timeout, open_doors)
            finally:
                self._in_portal_route = False

    def _walk_direct(self, x, z, level, run, timeout, open_doors):
        # Cible bloquee (mur, table, booth) : on rejoint la tuile
        # atteignable la plus proche et on s'arrete a portee d'interaction.
        if not collision_mod.tile_walkable(self.collision.get(x, z, level)):
            routing_map = self.opened_map if open_doors else self.collision
            path = self._find_path(routing_map, level, self.world.x, self.world.z, x, z)
            if path:
                x, z = path[-1][1], path[-1][2]
            else:
                report = self._report(routing_map, x, z, level)
                if report["nearest"] == (self.world.x, self.world.z):
                    self.dbg("nothing reachable toward %d,%d: %s" % (
                        x, z, self.explain_unreachable(x, z, open_doors)))
                    return True
        deadline = self.world.ticks + timeout
        visited = [(self.world.x, self.world.z)]
        seen = {(self.world.x, self.world.z, level): 1}
        planned_only = False
        while not self._arrived(x, z, level):
            if self.world.ticks > deadline:
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
            if here == before and not self._arrived(x, z, level):
                raise Unreachable(self.explain_unreachable(x, z, open_doors))
            # Door flapping (on/off the same tiles while a shut door opens
            # and closes) moves every leg but converges nowhere: fail fast
            # instead of burning the whole timeout. Three full cycles is
            # plenty for genuine door timing.
            key = (here[0], here[1], self.world.level)
            seen[key] = seen.get(key, 0) + 1
            if seen[key] >= 4 and not self._arrived(x, z, level):
                raise Unreachable("walk looping at %s dest=(%d,%d,%d)%s" % (
                    self.world.position, x, z, level, self._dest_suffix()))
        return True

    def _count_doors_on_path(self, path):
        """Portes fermees traversees : un detour sans porte est moins cher
        (clic + attente + risque de refus) qu'un raccourci a portes."""
        if not self.wall_doors:
            return 0
        steps = set(steps_mod.expand_waypoints(path, (self.world.x, self.world.z)))
        count = 0
        for spot in self.wall_doors:
            if id(spot) in self._blocked_doors or id(spot) in self._opening:
                continue
            if self.world.ticks - self.recent_doors.get((spot[0], spot[1]), 0) < 500:
                continue
            if not self._door_closed_at(spot):
                continue
            footprints = self.door_footprints.get(id(spot), ())
            if any((tile_x, tile_z) in steps for tile_x, tile_z, _level in footprints):
                count += 1
        return count

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
        routing_map = self.opened_map if open_doors else self.collision
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
        ranked = []
        for candidate in candidates:
            if candidate == (self.world.x, self.world.z):
                continue
            if any(abs(candidate[0] - seen[0]) + abs(candidate[1] - seen[1]) < 4 for seen in visited):
                continue
            path = self._find_path(routing_map, self.world.level, self.world.x, self.world.z, candidate[0], candidate[1])
            if not path and open_doors and self.open_nearby_door(deadline, (x, z)):
                routing_map = self.opened_map
                path = self._find_path(routing_map, self.world.level, self.world.x, self.world.z, candidate[0], candidate[1])
            if path:
                # A straight-line waypoint inside a field ends at the fence.
                # Keep the candidate whose path actually gets closest to it,
                # so the walk follows the road/gate instead of hugging walls.
                # Tiebreak sans portes : a gap egal, la route ouverte gagne
                # sur le raccourci qui force des clics.
                end_x, end_z = path[-1][1], path[-1][2]
                gap = abs(end_x - candidate[0]) + abs(end_z - candidate[1])
                ranked.append((gap, path))
        ranked.sort(key=lambda item: item[0])
        for gap, path in ranked:
            if step is not None and gap > step[0]:
                break
            doors = self._count_doors_on_path(path) if open_doors else 0
            if step is None or (gap, doors) < (step[0], step[1]):
                end_x, end_z = path[-1][1], path[-1][2]
                step = (gap, doors, (end_x, end_z))
        if step is not None and step[2] != (self.world.x, self.world.z):
            return step[2]
        raise Unreachable(self.explain_unreachable(x, z, open_doors))

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
            limit = min(deadline, self.world.ticks + len(chunk) * 100 + 500)
            try:
                self._await_leg(chunk, limit)
                continue
            except TimeoutError as error:
                print("walk leg stalled: %s%s" % (error, self._dest_suffix()), flush=True)
                # A refused step is almost always a closed door ahead: open
                # it, then replan from the new position. Replaying the same
                # chunk would stall on the very same tile forever.
                if self._open_door_toward(chunk[0]):
                    raise _DoorOpened()
                raise

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
                    self._door_closed_at, self._blocked_doors | self._opening | self._toll_blocked(), self.planning_map,
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
                    print("planned route timed out at %s: %s%s" % (self.world.position, error, self._dest_suffix()), flush=True)
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
        if self._arrived(x, z, self.world.level):
            return True
        while not self._arrived(x, z, self.world.level):
            if self.world.ticks > deadline:
                raise TimeoutError("walk to %d,%d timed out at %s" % (x, z, self.world.position))
            # The server drops clients that fall behind the tick: pump for
            # keepalive even on pure-planning iterations with no leg to walk.
            if getattr(self.connection, "sock", None) is not None:
                self.pump(1)
            if self.world.ticks > deadline:
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
            routing_map = self.opened_map if open_doors else self.collision
            if open_doors:
                stuck = self._standing_on_closed_door()
                if stuck is not None:
                    if self._go_open_door(stuck, deadline):
                        continue
                    if self._step_off_door(stuck, deadline):
                        continue
                    self._block_door(stuck)
                    continue
            path = self._find_path(routing_map, self.world.level, self.world.x, self.world.z, x, z)
            path = [step for step in path if step[0] == self.world.level]
            if not path:
                if open_doors and self.planning_map is not None:
                    path = self._find_path(self.planning_map, self.world.level, self.world.x, self.world.z, x, z)
                    path = [step for step in path if step[0] == self.world.level]
                    if path:
                        print("planning through known doors to %d,%d" % (x, z), flush=True)
                if not path and open_doors:
                    path = self._find_path(self.collision, self.world.level, self.world.x, self.world.z, x, z)
                    path = [step for step in path if step[0] == self.world.level]
                if not path:
                    if open_doors and self.open_nearby_door(deadline, (x, z)):
                        continue
                    if open_doors and self._click_doors_around():
                        continue
                    raise Unreachable(self.explain_unreachable(x, z, open_doors))
            self._route = [(step_x, step_z) for _level, step_x, step_z in path[:64]]
            self._route_target = (x, z)
            if open_doors:
                door = self._door_on_path(path, limit=24)
                if door is not None:
                    door_op = doors_mod.open_op(self.loctypes, door[9])
                    self._route_action = {
                        "type": "loc",
                        "x": door[0],
                        "z": door[1],
                        "id": door[9],
                        "name": self.loc_name_at(door[0], door[1], door[2]) or "",
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
                    self.recent_doors[(door[0], door[1])] = self.world.ticks
                    continue
            self._write_state()  # show the route at once, not at the next heartbeat
            leg = self._expand(path[:STEP_CAP])
            before = (self.world.x, self.world.z)
            self._send_walk(leg, run)
            try:
                self._await_leg(leg, min(deadline, self.world.ticks + len(leg) * 100 + 500))
            except TimeoutError as error:
                print("walk leg to %s timed out, recomputing: %s%s" % (leg[-1], error, self._dest_suffix()), flush=True)
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

    def walk_raw(self, x, z, level, deadline):
        while (self.world.x, self.world.z, self.world.level) != (x, z, level):
            if self.world.ticks > deadline:
                raise TimeoutError("walk to %d,%d timed out at %s" % (x, z, self.world.position))
            path = self._find_path(self.opened_map, level, self.world.x, self.world.z, x, z)
            path = [step for step in path if step[0] == level]
            if not path:
                raise Unreachable("no path to %d,%d" % (x, z))
            leg = self._expand(path[:STEP_CAP])
            self._send_walk(leg, False)
            try:
                self._await_leg(leg, min(deadline, self.world.ticks + len(leg) * 100 + 500))
            except TimeoutError as error:
                print("raw walk leg to %s timed out, recomputing: %s%s" % (leg[-1], error, self._dest_suffix()), flush=True)
                continue
        return True

    def _expand(self, waypoints):
        return steps_mod.expand_waypoints(waypoints, (self.world.x, self.world.z))[:STEP_CAP]

    def step_to(self, x, z, timeout_ticks=120):
        """Walk one tile past an opened door, without routing.

        A wall door is open for a moment after its click: routing again
        from the live map races that window and, in a maze of look-alike
        walls, sends the planner hunting other doors. The caller already
        knows the tile, so send the single step directly. Returns True
        when the server moved us there.
        """
        if (self.world.x, self.world.z) == (x, z):
            return True
        self.ensure_nav()
        if abs(x - self.world.x) + abs(z - self.world.z) != 1:
            raise Unreachable("step_to is one tile, not %s -> %d,%d" % (self.world.position, x, z))
        if not collision_mod.tile_walkable(self.collision.get(x, z, self.world.level)):
            raise Unreachable("step target %d,%d is blocked at %s" % (x, z, self.world.position))
        self._send_walk([(x, z)], self.want_run())
        deadline = self.world.ticks + timeout_ticks
        while self.world.ticks < deadline:
            self.pump(1)
            if (self.world.x, self.world.z) == (x, z):
                return True
        return False

    def walk_steps(self, steps, timeout_ticks=120, run=None):
        """Walk an explicit tile route, no router involved.

        The maze cannot be routed (its corridors are walllow tiles that the
        collision map blocks), but the walk packet takes the whole leg, so
        a caller that already knows the tiles sends them in one go. Returns
        True when the last tile was reached.
        """
        steps = [tuple(step) for step in steps]
        if not steps:
            return True
        self.ensure_nav()
        if run is None:
            run = self.want_run()
        self._send_walk(steps, run)
        deadline = self.world.ticks + max(1, timeout_ticks)
        target = steps[-1]
        while self.world.ticks < deadline:
            self.pump(1)
            if (self.world.x, self.world.z) == target:
                return True
        return (self.world.x, self.world.z) == target

    def _send_walk(self, steps, run):
        self.dbg("send_walk %d steps run=%s" % (len(steps), run))
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
        last_move = self.world.ticks
        while True:
            position = (self.world.x, self.world.z)
            if position == target:
                self.dbg("leg arrived %s%s" % (target, self._dest_suffix()))
                return
            distance = abs(position[0] - target[0]) + abs(position[1] - target[1])
            if distance > best:
                return  # passed it: the next leg recomputes from here
            if distance < best:
                best = distance
                last_move = self.world.ticks
            elif self.world.ticks - last_move > 100:
                # No progress for 100 ticks: the leg is refused, not slow.
                raise TimeoutError("leg to %s stalled at %s" % (target, self.world.position))
            if self.world.ticks > deadline:
                raise TimeoutError("leg to %s timed out at %s" % (target, self.world.position))
            self.pump(1)
