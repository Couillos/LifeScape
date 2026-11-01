"""High-level bot: login, chat, walk, read what the server sends."""

from .actions import ActionsMixin
from .core import BotCore, StateError, Unreachable
from .movement import MovementMixin


class Bot(ActionsMixin, MovementMixin, BotCore):
    pass


__all__ = ["Bot", "StateError", "Unreachable"]
