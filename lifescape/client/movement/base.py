"""Socle marche : cible, arrivee, paquets loc.."""

import functools

from ... import reach as reach_mod
from ... import f2p as f2p_mod
from ...buffer import Writer
from ...protocol import client_op_id

STEP_CAP = 24
# The server either sends the open state within a couple of ticks or never
# (stale zone); waiting longer just freezes the bot in front of the door.
DOOR_WAIT = 35


class _DoorOpened(Exception):
    """A rescue click opened a door mid-leg: replan from the new position
    instead of replaying the stale chunk that just failed."""


def _tracks_destination(func):
    """Remember walk_to's destination for the leg logs, nesting-safe."""
    def wrapper(self, x, z, level=0, *args, **kwargs):
        previous = getattr(self, "_walk_dest", None)
        self._walk_dest = (x, z, level)
        try:
            return func(self, x, z, level, *args, **kwargs)
        finally:
            self._walk_dest = previous
    return functools.wraps(func)(wrapper)


class MovementBase:

    def _dest_suffix(self):
        dest = getattr(self, "_walk_dest", None)
        return "" if dest is None else " dest=(%d,%d,%d)" % dest

    def _describe_target(self, x, z, level):
        """Forme loc de la cible, ou tuile exacte si rien dessus."""
        try:
            loc = self.world.loc_at(x, z, level)
        except (AttributeError, TypeError):
            return (1, 1, 0, -1, 0)
        return reach_mod.describe_target(loc, getattr(self, "loctypes", None), x, z, level)

    def _routing(self, routing_map):
        """Vue F2P du plan : les tuiles members ne se traversent pas."""
        if getattr(self, "freemap", None) is not None:
            return f2p_mod.F2PMap(routing_map, self.freemap)
        return routing_map

    def _toll_locked(self, spot):
        """Peage sans option paid : on ne touche pas."""
        return (id(spot) in getattr(self, "toll_doors", set())
                and not getattr(self, "allow_paid_paths", False))

    def _door_avoid(self, spot):
        """Porte a ne pas toucher : blacklistee, ou peage non paye."""
        return id(spot) in self._blocked_doors or self._toll_locked(spot)

    def _toll_blocked(self):
        """Ids a barrer au planner quand on ne paie pas."""
        if getattr(self, "allow_paid_paths", False):
            return set()
        return set(getattr(self, "toll_doors", set()))

    def _find_path(self, routing_map, level, src_x, src_z, dest_x, dest_z,
                   max_waypoints=25, move_near=True):
        """find_path avec ReachStrategy : s'arrete a cote des murs/objets."""
        width, height, angle, shape, access = self._describe_target(dest_x, dest_z, level)
        return self.router.find_path(self._routing(routing_map), level, src_x, src_z, dest_x, dest_z,
                                     max_waypoints=max_waypoints, move_near=move_near,
                                     dest_width=width, dest_height=height,
                                     angle=angle, shape=shape, block_access=access)

    def _arrived(self, x, z, level):
        """Assez pres pour interagir (mur = a cote, objet = autour)."""
        try:
            width, height, angle, shape, access = self._describe_target(x, z, level)
            return reach_mod.reached(self.collision.get, level,
                                     self.world.x, self.world.z, x, z,
                                     width, height, angle, shape, access)
        except (AttributeError, TypeError, KeyError, IndexError):
            return (self.world.x, self.world.z) == (x, z)

    def _npc_size(self, npc):
        """Cote de l'NPC (rsmod NpcType.size), 1 par defaut."""
        try:
            return max(1, int(self.loctypes["npc"][npc["type"]]["size"] or 1))
        except (AttributeError, KeyError, IndexError, TypeError, ValueError):
            return 1

    def _npc_clickable(self, npc):
        """True quand le serveur accepte un clic sur l'NPC depuis ici.

        Meme regle que reachedEntity cote serveur : rsmod.reached en shape
        NPC (-2) ne prend que l'adjacence par un cote, jamais la diagonale.
        """
        if npc.get("level", self.world.level) != self.world.level:
            return False
        size = self._npc_size(npc)
        try:
            return reach_mod.reached(
                self.collision.get, self.world.level,
                self.world.x, self.world.z, npc["x"], npc["z"],
                size, size, 0, reach_mod.SHAPE_NPC, 0)
        except (AttributeError, KeyError, TypeError, IndexError):
            return False

    def _report(self, routing_map, x, z, level):
        width, height, angle, shape, access = self._describe_target(x, z, level)
        return self.router.unreachable_report(
            self._routing(routing_map), level, self.world.x, self.world.z, x, z, self.loc_name_at,
            dest_width=width, dest_height=height, angle=angle, shape=shape, block_access=access)

    def send_oploc(self, op, x, z, loc_id):
        body = Writer()
        body.u16(x)
        body.u16(z)
        body.u16(loc_id)
        self.connection.send(client_op_id("OPLOC%d" % op), body.bytes())

    def _await_loc_change(self, x, z, level, was, deadline):

        while self.world.ticks < deadline:
            self.pump(1)
            now = self.world.loc_at(x, z, level)
            if now is None or now[0] != was:
                return True
        return False
