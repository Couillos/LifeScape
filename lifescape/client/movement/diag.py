"""Diag marche : debug, causes d'echec.."""

from ... import collision as collision_mod
from ..core import Unreachable

from .base import MovementBase


class MovementDiag(MovementBase):

    def debug_walk(self, x, z, level=None):
        """Diagnose one walk without walking it. Read-only, for the CLI.

        Returns JSON-able data plus preformatted lines: static tiles
        around the target, raw and doors-open router paths, the
        unreachable frontier with blocker names, and every known door
        within 15 tiles with its live state. Runs on the pump thread
        through call, so a running script is never unloaded.
        """
        x, z = int(x), int(z)
        level = self.world.level if level is None else int(level)
        self.ensure_nav()
        try:
            loc_name = self.loctypes["loc"]
        except Exception:
            loc_name = []
        here = (self.world.level, self.world.x, self.world.z)
        static = []
        for dz in range(2, -3, -1):
            try:
                row = "".join(
                    "." if collision_mod.tile_walkable(self.collision.get(x + dx, z + dz, level)) else "#"
                    for dx in range(-2, 3)
                )
            except Exception:
                row = "?????"
            static.append((z + dz, row))
        paths = {}
        for label, routing_map in (("raw", self.collision), ("doors-open", self.opened_map)):
            try:
                found = self._find_path(routing_map, level, here[1], here[2], x, z)
            except Exception:
                found = None
            paths[label] = None if not found else len(found)
        try:
            frontier = self._report(self.opened_map, x, z, level)
        except Exception as error:
            frontier = {"error": repr(error)}
        doors = []
        for spot in self.wall_doors or ():
            if spot[2] != level or abs(spot[0] - x) + abs(spot[1] - z) > 15:
                continue
            try:
                entry = loc_name[spot[9]] if spot[9] < len(loc_name) else {}
                name = (entry.get("name") if isinstance(entry, dict) else None) or "#%s" % spot[9]
            except Exception:
                name = "#%s" % spot[9]
            try:
                closed = not self._door_closed_at(spot)
            except Exception:
                closed = "?"
            doors.append({"name": name, "x": spot[0], "z": spot[1],
                          "closed": closed, "learned": id(spot) in self._blocked_doors})
        lines = [
            "walk (%d,%d,%d) -> (%d,%d,%d)" % (here[0], here[1], here[2], level, x, z),
        ]
        lines += ["static %d %s" % row for row in static]
        lines.append("raw path: %s" % paths["raw"])
        lines.append("doors-open path: %s" % paths["doors-open"])
        lines.append("frontier nearest=%s blockers=%s dest_blocked=%s dest_name=%s" % (
            frontier.get("nearest"), frontier.get("blockers", frontier.get("error")),
            frontier.get("dest_blocked"), frontier.get("dest_name")))
        for door in doors:
            lines.append("door %-24s (%d,%d) closed=%s learned=%s" % (
                door["name"], door["x"], door["z"], door["closed"], door["learned"]))
        lines.append("doors shown=%d learned_total=%d unsealed=%d" % (
            len(doors), len(self._blocked_doors), len(self.unsealed_doors)))
        return {"from": list(here), "to": [level, x, z], "static": static,
                "paths": paths, "frontier": frontier, "doors": doors,
                "learned_total": len(self._blocked_doors),
                "unsealed": len(self.unsealed_doors), "lines": lines}

    def loc_name_at(self, x, z, level):
        loc = self.world.loc_at(x, z, level)
        if loc is None or self.loctypes is None:
            return None
        if loc[0] >= len(self.loctypes["loc"]):
            return None
        entry = self.loctypes["loc"][loc[0]]
        return entry.get("name") if entry else None

    def unreachable_hints(self, names):
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

    def explain_unreachable(self, x, z, open_doors=True):
        routing_map = self.opened_map if open_doors else self.collision
        try:
            report = self._report(routing_map, x, z, self.world.level)
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
        parts.append("possible cause: " + ", ".join(self.unreachable_hints(names)))
        return "; ".join(parts)

    def can_reach(self, x, z, level=None, near=1):
        """True when a path gets adjacent, planning through known doors."""
        self.ensure_nav()
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
        for routing_map in (self.planning_map, self.opened_map):
            if routing_map is None:
                continue
            # Full waypoint list: the default cap truncates long routes
            # mid-way, and the truncated end is never near the target.
            path = self._find_path(routing_map, level, self.world.x, self.world.z, x, z, max_waypoints=10000)
            if path and abs(path[-1][1] - x) + abs(path[-1][2] - z) <= near:
                return True
        return False

    def open_doors_near(self, x, z, radius=8, timeout=3000):
        """Open closed doors around a target before routing into a building."""
        self.ensure_nav()
        deadline = self.world.ticks + timeout
        candidates = [
            spot for spot in self.wall_doors
            if spot[2] == self.world.level and abs(spot[0] - x) + abs(spot[1] - z) <= radius
        ]
        candidates.sort(key=lambda spot: abs(spot[0] - x) + abs(spot[1] - z))
        for spot in candidates:
            if not self._door_closed_at(spot):
                continue
            if self.world.ticks > deadline:
                return False
            remaining = max(250, deadline - self.world.ticks)
            try:
                self.open_door(spot[0], spot[1], timeout=remaining)
            except (Unreachable, TimeoutError) as error:
                print("door near %d,%d failed: %s" % (x, z, error), flush=True)
                continue
        return True
