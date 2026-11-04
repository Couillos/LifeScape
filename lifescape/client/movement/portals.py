"""Etages : transitions, plan portails, validation.."""

import json
import os
import time

from ... import collision as collision_mod
from ... import doors as doors_mod
from ... import f2p as f2p_mod
from ... import nav as nav_mod
from ... import objs as objs_mod
from ... import portals as portals_mod
from ...bfs import FAR as BFS_FAR
from ...bfs import HALF as BFS_HALF
from ...bfs import SEARCH as BFS_SEARCH
from ..core import StateError, Unreachable

from .base import MovementBase


class MovementPortals(MovementBase):

    def ensure_nav(self):
        if self.collision is not None:
            return
        self.loctypes = collision_mod.load_types(self.paths["types"])
        self.collision, static_locs, static_actions, static_layers = collision_mod.load_maps(self.paths["pack_dir"], self.loctypes)
        self.world.static_locs = static_locs
        self.world.static_locs_layered = static_layers
        self.world.static_actions = static_actions
        self.world.static_objs = {}
        for x, z, level, oid, count in objs_mod.load_static_objs(self.paths["pack_dir"]):
            self.world.static_objs[((x, z, level), oid)] = count
        self.wall_doors = [
            spot for spot in doors_mod.find_openables(self.paths["pack_dir"], self.loctypes)
            if doors_mod.is_door(self.loctypes, spot[9])
        ]
        self.door_footprints = {id(spot): doors_mod.footprint(spot) for spot in self.wall_doors}
        self._wall_door_keys = {(spot[0], spot[1], spot[2]) for spot in self.wall_doors}
        # Peages (Al Kharid...) : barres sauf option paid. Nom debugname
        # comme les hints "toll gate (needs money)".
        self.toll_doors = {
            id(spot) for spot in self.wall_doors
            if spot[9] < len(self.loctypes["loc"])
            and "toll" in (self.loctypes["loc"][spot[9]].get("name") or "").lower()
        }
        self._load_portals()
        # Never step on a transition tile by accident: the server would move
        # the player to another floor mid-route. Block the whole footprint,
        # not just the origin tile: a 2x2 staircase has three other tiles a
        # route could otherwise cut through and trigger a surprise climb.
        self.stair_tiles = set()
        for row in self._portal_rows:
            for tile_x, tile_z in row.get("footprint") or [[row["from"][1], row["from"][2]]]:
                self.stair_tiles.add((tile_x, tile_z, row["from"][0]))
        self._rebuild_opened()
        try:
            self.freemap = f2p_mod.load_freemap(f2p_mod.csv_path(self.paths["pack_dir"]))
        except OSError:
            print("no free2play.csv, routing without F2P bounds", flush=True)
            self.freemap = None
        self._planner = nav_mod.Planner(self.router, self.collision, self.loctypes, self.wall_doors, self.freemap)
        for spot in self.wall_doors:
            key = (spot[0], spot[1], spot[2])
            if key in self.world.locs:
                loc = self.world.locs[key]
                if loc is None or doors_mod.open_op(self.loctypes, loc[0]) is None:
                    self.unseal(*key)

    def _load_portals(self):
        self._portals = []
        self._portal_rows = []
        path = os.path.join(os.path.dirname(os.path.dirname(os.path.dirname(__file__))), "data", "transitions.json")
        if not os.path.exists(path):
            raise StateError("missing transition table %r, run scripts/dev/dump_transitions.py" % (path,))
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
        sides = self.transition_sides(level, frm_x, frm_z, delta, row.get("footprint"))
        if not sides:
            print("transition %s has no reachable side from %s" % (row.get("name"), self.world.position), flush=True)
            return False
        for side_x, side_z in sides:
            if deadline is not None and self.world.ticks > deadline:
                print("transition %s: out of time at %s" % (row.get("name"), self.world.position), flush=True)
                break
            timeout = 2250 if deadline is None else max(150, deadline - self.world.ticks)
            if not self._stand_beside(level, side_x, side_z, timeout):
                continue
            if not self.open_transition(row, level, frm_x, frm_z):
                continue
            expected = (
                (level + delta[0], self.world.x + delta[1], self.world.z + delta[2])
                if delta else tuple(row["to"])
            )
            if self.await_teleport(row, expected, seconds=2.0, quiet=True):
                return True
            print("transition %s: side %d,%d refused" % (row.get("name"), side_x, side_z), flush=True)
        print("transition %s failed at %s" % (row.get("name"), self.world.position), flush=True)
        return False

    def open_transition(self, row, level, frm_x, frm_z):
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

    def transition_sides(self, level, x, z, delta=None, footprint=None):
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
            return collision_mod.tile_walkable(self.collision.get(tile_x + dx, tile_z + dz, level + dlevel))

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
            if collision_mod.tile_walkable(self.collision.get(side[0], side[1], level))
        ]
        # Rank, never filter: the server teleports onto blocked tiles (ship
        # decks, stairwells), so a side whose landing looks wrong can still
        # be the only one it accepts. Reachability first (a side nobody can
        # stand on is useless), then its reachRectangle1 (no wall facing the
        # loc), then a walkable landing, then the real walking distance.
        routing = self.planning_map or self.opened_map
        stairs = self.stair_tiles or set()
        ranked = []
        for side in sides:
            walled = bool(self.collision.get(side[0], side[1], level) & facing[side])
            if (self.world.level, self.world.x, self.world.z) == (level, side[0], side[1]):
                # find_path compresses a zero-length route to []: rank
                # standing-put as the best side instead of unwalkable.
                ranked.append((0, walled, not lands_ok(*side), False, -1, side))
                continue
            path = self._find_path(routing, level, self.world.x, self.world.z, side[0], side[1])
            stair = (side[0], side[1], level) in stairs
            ranked.append((0 if path else 1, walled, not lands_ok(*side), stair, len(path), side))
        return [side for _found, _walled, _landing, _stair, _length, side in sorted(ranked)]

    def _stand_beside(self, level, x, z, timeout=2250):
        if self.world.level == level and (self.world.x, self.world.z) == (x, z):
            return True
        try:
            self.walk_to(x, z, level, timeout=timeout)
        except (Unreachable, TimeoutError) as error:
            print("transition step to %d,%d failed: %s" % (x, z, error), flush=True)
            return False
        return (self.world.level, self.world.x, self.world.z) == (level, x, z)

    def await_teleport(self, row, expected, seconds=3.0, quiet=False):
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

    def plan_portals(self, start, dest, skip=(), skip_walks=()):
        routing = self.planning_map or self.opened_map
        skip = set(skip)
        skip_walks = set(skip_walks)
        now = self.world.ticks
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
            planner = portals_mod.RoutePlanner(self.router, routing)
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

    def _toll_on_path(self, path, level):
        """Le chemin traverse-t-il un peage (interdit sauf option paid)."""
        if getattr(self, "allow_paid_paths", False):
            return False
        toll_tiles = set()
        for spot_id in getattr(self, "toll_doors", set()):
            for x, z, lvl in (getattr(self, "door_footprints", None) or {}).get(spot_id, ()):
                toll_tiles.add((x, z, lvl))
        return any((x, z, level) in toll_tiles for _lvl, x, z in path)

    def _walk_leg_ok(self, routing, frm, to, flood_min=None):
        """True when a planned walking leg really goes through on foot."""
        if frm[0] != to[0]:
            return True
        if abs(frm[1] - to[1]) + abs(frm[2] - to[2]) <= 2:
            # Already there or adjacent: walk_to retargets blocked tiles
            # onto the nearest reachable tile and finishes from here.
            return True
        if abs(to[1] - frm[1]) > BFS_HALF - 4 or abs(to[2] - frm[2]) > BFS_HALF - 4:
            # Beyond the router window the destination truncates, so an
            # end-near-target check is meaningless. Fail only when the
            # flood proves disconnection: a small enclosed region (wrong
            # dungeon, far shore) exhausts inside the window, while open
            # ground fills it. Without this the walker follows impossible
            # straight lines into dead ends.
            path = self._find_path(routing, frm[0], frm[1], frm[2], to[1], to[2], max_waypoints=10000)
            if path:
                if self._toll_on_path(path, frm[0]):
                    return False
                end = path[-1]
                before = abs(frm[1] - to[1]) + abs(frm[2] - to[2])
                after = abs(end[1] - to[1]) + abs(end[2] - to[2])
                return before - after >= 15
            visited = sum(1 for distance in self.router.distances if distance < BFS_FAR)
            if flood_min is None:
                flood_min = BFS_SEARCH * BFS_SEARCH // 2
            return visited >= flood_min
        # Full waypoint list: the default cap truncates long routes mid-way,
        # which would look like a dead end that is not one.
        path = self._find_path(routing, frm[0], frm[1], frm[2], to[1], to[2], max_waypoints=10000)
        if not path:
            return False
        if self._toll_on_path(path, frm[0]):
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
