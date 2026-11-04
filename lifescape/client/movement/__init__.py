"""Marche : un role par module (base, walk, doors, portals, diag)."""

from .base import DOOR_WAIT, STEP_CAP, MovementBase, _DoorOpened, _tracks_destination
from .diag import MovementDiag
from .doors import MovementDoors
from .portals import MovementPortals
from .walk import MovementWalk


class MovementMixin(MovementWalk, MovementDoors, MovementPortals, MovementDiag):
    pass


__all__ = [
    "MovementMixin",
    "MovementBase",
    "MovementWalk",
    "MovementDoors",
    "MovementPortals",
    "MovementDiag",
    "STEP_CAP",
    "DOOR_WAIT",
    "_DoorOpened",
    "_tracks_destination",
]
