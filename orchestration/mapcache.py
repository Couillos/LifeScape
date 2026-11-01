"""Minimal 2D floor colours read from the packed game cache.

The ground file of each map square stores, per 64x64 level, an underlay
and an overlay floor id. ``server/flo.dat`` maps those ids to an RGB
colour. The spectator shows the underlay (grass, water, sand) and falls
back to the overlay (paths, floors) when present.

Stdlib only. Each map square is parsed once and cached in memory; the
daemon serves a small colour grid around the bot, never the whole map.
"""

import bz2
import os
import threading
import zipfile

from .paths import PACK_DIR

MAP_SIZE = 64
LEVELS = 4

_EMPTY = "#121212"

# Textured floors have no colour of their own. Water (1), fountain (17) and
# gungy water (25) are the only ones overlaid on the spectator map.
_TEXTURE_RGB = {1: 0x3f76e4, 17: 0x3f76e4, 25: 0x2f5f9e}


class MapCache:
    def __init__(self):
        self._flo = None
        self._squares = {}
        self._lock = threading.Lock()

    # -- config ---------------------------------------------------------

    @staticmethod
    def _name_hash(name):
        value = 0
        for character in name.upper():
            value = (value * 61 + ord(character) - 32) & 0xFFFFFFFF
        return value - 0x100000000 if value & 0x80000000 else value

    def _jagfile(self):
        """The client config container holds the real floor colours."""
        path = os.path.join(PACK_DIR, "client", "config")
        with open(path, "rb") as handle:
            return handle.read()

    def _jagfile_read(self, container, wanted):
        if len(container) < 8:
            return None
        unpacked = (container[0] << 16) | (container[1] << 8) | container[2]
        packed = (container[3] << 16) | (container[4] << 8) | container[5]
        whole = unpacked != packed
        data = bz2.decompress(b"BZh9" + container[6:]) if whole else container
        count = (data[6] << 8) | data[7]
        position = 8
        entries = []
        for _index in range(count):
            file_hash = int.from_bytes(data[position:position + 4], "big", signed=True)
            position += 4
            file_unpacked = (data[position] << 16) | (data[position + 1] << 8) | data[position + 2]
            position += 3
            file_packed = (data[position] << 16) | (data[position + 1] << 8) | data[position + 2]
            position += 3
            entries.append((file_hash, file_unpacked, file_packed))
        start = 8 + count * 10
        wanted_hash = self._name_hash(wanted)
        for file_hash, file_unpacked, file_packed in entries:
            if file_hash == wanted_hash:
                raw = data[start:start + file_packed]
                if whole or file_unpacked == file_packed:
                    return raw
                return bz2.decompress(b"BZh9" + raw)
            start += file_packed
        return None

    def _load_flo(self):
        try:
            container = self._jagfile()
            data = self._jagfile_read(container, "flo.dat")
        except (OSError, ValueError, OSError):
            data = None
        if not data and os.path.exists(os.path.join(PACK_DIR, "client", "config")):
            return []
        if not data:
            path = os.path.join(PACK_DIR, "server", "flo.dat")
            with open(path, "rb") as handle:
                data = handle.read()
        if len(data) < 2:
            return []
        count = (data[0] << 8) | data[1]
        position = 2
        colours = []
        for _index in range(count):
            rgb = 0
            texture = -1
            while position < len(data):
                opcode = data[position]
                position += 1
                if opcode == 0:
                    break
                if opcode == 1:
                    rgb = (data[position] << 16) | (data[position + 1] << 8) | data[position + 2]
                    position += 3
                elif opcode == 2:
                    texture = data[position]
                    position += 1
                elif opcode == 5:
                    pass
                elif opcode == 6:
                    while position < len(data) and data[position] != 10:
                        position += 1
                    position += 1
            colours.append(rgb or _TEXTURE_RGB.get(texture, 0))
        return colours

    def _floor(self):
        if self._flo is None:
            with self._lock:
                if self._flo is None:
                    self._flo = self._load_flo()
        return self._flo

    # -- map squares ----------------------------------------------------

    def _load_square(self, square_x, square_z):
        path = os.path.join(PACK_DIR, ".cache", "maps-server.zip")
        with zipfile.ZipFile(path) as archive:
            name = "m%d_%d" % (square_x, square_z)
            if name not in archive.namelist():
                return None
            data = archive.read(name)
        grounds = [[(-1, -1)] * (MAP_SIZE * MAP_SIZE) for _level in range(LEVELS)]
        position = 0
        for level in range(LEVELS):
            for x in range(MAP_SIZE):
                for z in range(MAP_SIZE):
                    underlay = -1
                    overlay = -1
                    while position < len(data):
                        opcode = data[position]
                        position += 1
                        if opcode == 0:
                            break
                        if opcode == 1:
                            position += 1
                            break
                        if opcode <= 49:
                            overlay = data[position]
                            position += 1
                        elif opcode <= 81:
                            pass
                        else:
                            underlay = opcode - 81
                    grounds[level][x * MAP_SIZE + z] = (underlay, overlay)
        return grounds

    def _square(self, square_x, square_z):
        key = (square_x, square_z)
        square = self._squares.get(key)
        if square is None:
            with self._lock:
                square = self._squares.get(key)
                if square is None:
                    square = self._load_square(square_x, square_z)
                    self._squares[key] = square
        return square

    def colour_at(self, level, x, z):
        square = self._square(x >> 6, z >> 6)
        if square is None or not 0 <= level < LEVELS:
            return _EMPTY
        underlay, overlay = square[level][(x & 63) * MAP_SIZE + (z & 63)]
        colours = self._floor()
        for floor_id in (overlay, underlay):
            if floor_id < 0 or floor_id >= len(colours):
                continue
            rgb = colours[floor_id]
            colour = "#%02x%02x%02x" % ((rgb >> 16) & 0xFF, (rgb >> 8) & 0xFF, rgb & 0xFF)
            if colour != "#ff00ff":  # texture placeholder, not a real ground colour
                return colour
        return _EMPTY

    # -- spectator payload ----------------------------------------------

    def grid(self, level, x, z, radius=16):
        """Compact colour grid around (x,z): palette + rows of indices."""
        size = radius * 2 + 1
        origin_x = x - radius
        origin_z = z - radius
        palette = []
        index_of = {}
        rows = []
        for dz in range(size):
            row = []
            for dx in range(size):
                colour = self.colour_at(level, origin_x + dx, origin_z + dz)
                index = index_of.get(colour)
                if index is None:
                    index = len(palette)
                    index_of[colour] = index
                    palette.append(colour)
                row.append(index)
            rows.append(row)
        return {
            "x": origin_x,
            "z": origin_z,
            "size": size,
            "palette": palette,
            "tiles": rows,
        }
