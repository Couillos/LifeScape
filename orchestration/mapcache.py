"""Ground colours read from the packed game cache, the way the game paints them.

The ground file of each map square stores, per 64x64 level, an underlay
and an overlay floor id. Each flo id maps to the colour the terrain is
painted with: the flo rgb, or, for textured floors (water, planks,
rock), the average colour of the texture in ``content/textures``. The
overlay wins when present, like the 3D ground the client minimap is
rendered from. ``mapview/worldmap.jag`` is deliberately not used: its
``floorcol.dat`` is the world-map palette, visibly darker and redder
than the in-game ground.

Stdlib only. Each map square is parsed once and cached in memory; the
daemon serves a small colour grid around the bot, never the whole map.
"""

import bz2
import os
import struct
import threading
import zipfile
import zlib

from .paths import PACK_DIR, ROOT

MAP_SIZE = 64
LEVELS = 4

_EMPTY = "#121212"


class MapCache:
    def __init__(self):
        self._floor_rgb = None
        self._floor_textures = None
        self._texture_averages = {}
        self._palette = None
        self._squares = {}
        self._lock = threading.RLock()

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
        except (OSError, ValueError):
            data = None
        if not data and os.path.exists(os.path.join(PACK_DIR, "client", "config")):
            return [], []
        if not data:
            path = os.path.join(PACK_DIR, "server", "flo.dat")
            with open(path, "rb") as handle:
                data = handle.read()
        if len(data) < 2:
            return [], []
        count = (data[0] << 8) | data[1]
        position = 2
        colours = []
        textures = []
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
            colours.append(rgb)
            textures.append(texture)
        return colours, textures

    def _ensure_flo(self):
        if self._floor_rgb is None or self._floor_textures is None:
            with self._lock:
                if self._floor_rgb is None or self._floor_textures is None:
                    colours, textures = self._load_flo()
                    self._floor_rgb = colours
                    self._floor_textures = textures

    def _floor(self):
        self._ensure_flo()
        return self._floor_rgb

    def _flo_texture(self):
        self._ensure_flo()
        return self._floor_textures

    # -- ground palette ---------------------------------------------------

    def _content_dir(self):
        candidate = os.path.normpath(os.path.join(PACK_DIR, "..", "..", "..", "content"))
        if os.path.isdir(candidate):
            return candidate
        return os.path.normpath(os.path.join(ROOT, "..", "lostcity", "content"))

    def tex_names(self):
        """texture id -> texture name, from ``content/pack/texture.pack``."""
        names = {}
        try:
            with open(os.path.join(self._content_dir(), "pack", "texture.pack"), encoding="ascii") as handle:
                for line in handle:
                    line = line.strip()
                    if not line or "=" not in line:
                        continue
                    texture_id, texture_name = line.split("=", 1)
                    try:
                        names[int(texture_id)] = texture_name
                    except ValueError:
                        continue
        except OSError:
            pass
        return names

    def _decode_average(self, texture_name):
        """Mean rgb of a texture: what a flat 2D view should show for it."""
        path = os.path.join(self._content_dir(), "textures", texture_name + ".png")
        png = open(path, "rb").read()
        position = 8
        width = height = color_type = None
        compressed = bytearray()
        while position + 8 <= len(png):
            length = struct.unpack(">I", png[position:position + 4])[0]
            kind = png[position + 4:position + 8]
            body = png[position + 8:position + 8 + length]
            if kind == b"IHDR":
                width, height, _depth, color_type = struct.unpack(">IIBB", body[:10])
            elif kind == b"IDAT":
                compressed += body
            elif kind == b"IEND":
                break
            position += 12 + length
        channels = 3 if color_type == 2 else 4
        raw = zlib.decompress(bytes(compressed))
        stride = width * channels
        totals = [0, 0, 0]
        previous = bytearray(stride)
        offset = 0
        for _row in range(height):
            kind = raw[offset]
            current = bytearray(raw[offset + 1:offset + 1 + stride])
            offset += 1 + stride
            for index in range(stride):
                left = current[index - channels] if index >= channels else 0
                up = previous[index]
                upper_left = previous[index - channels] if index >= channels else 0
                if kind == 1:
                    current[index] = (current[index] + left) & 0xFF
                elif kind == 2:
                    current[index] = (current[index] + up) & 0xFF
                elif kind == 3:
                    current[index] = (current[index] + ((left + up) >> 1)) & 0xFF
                elif kind == 4:
                    guess = left + up - upper_left
                    dl, du, dd = abs(guess - left), abs(guess - up), abs(guess - upper_left)
                    current[index] = (current[index] + (left if dl <= du and dl <= dd else up if du <= dd else upper_left)) & 0xFF
            previous = current
            for index in range(0, stride, channels):
                totals[0] += current[index]
                totals[1] += current[index + 1]
                totals[2] += current[index + 2]
        pixels = width * height
        return tuple(total // pixels for total in totals)

    def _texture_colour(self, texture_name):
        if texture_name not in self._texture_averages:
            with self._lock:
                if texture_name not in self._texture_averages:
                    try:
                        self._texture_averages[texture_name] = self._decode_average(texture_name)
                    except (OSError, ValueError, struct.error, zlib.error):
                        self._texture_averages[texture_name] = None
        return self._texture_averages[texture_name]

    def palette(self):
        """flo id -> "#rrggbb" the terrain is painted with, cached once."""
        if self._palette is None:
            with self._lock:
                if self._palette is None:
                    names = self.tex_names()
                    palette = []
                    for floor_id, rgb in enumerate(self._floor()):
                        colour = None
                        texture_id = self._flo_texture()[floor_id]
                        if texture_id >= 0 and names.get(texture_id):
                            colour = self._texture_colour(names[texture_id])
                        if colour is None:
                            colour = ((rgb >> 16) & 0xFF, (rgb >> 8) & 0xFF, rgb & 0xFF)
                        palette.append("#%02x%02x%02x" % colour)
                    self._palette = palette
        return self._palette

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
        # Cache the misses too: a square missing from the pack (random-event
        # areas are not in it) used to reopen the zip once per tile, which
        # made one spectator frame take seconds.
        key = (square_x, square_z)
        if key in self._squares:
            return self._squares[key]
        with self._lock:
            if key not in self._squares:
                self._squares[key] = self._load_square(square_x, square_z)
        return self._squares[key]

    def _tile_ids(self, level, tile_x, tile_z):
        square = self._square(tile_x >> 6, tile_z >> 6)
        if square is None or not 0 <= level < LEVELS:
            return -1, -1
        return square[level][(tile_x & 63) * MAP_SIZE + (tile_z & 63)]

    def colour_at(self, level, tile_x, tile_z):
        """Terrain colour: overlay first, else underlay, as the game paints it."""
        underlay, overlay = self._tile_ids(level, tile_x, tile_z)
        palette = self.palette()
        rgbs = self._floor()
        for floor_id in (overlay, underlay):
            if floor_id < 0 or floor_id >= len(palette):
                continue
            if floor_id < len(rgbs) and rgbs[floor_id] == 0xFF00FF:
                continue  # invisible floor: see through to the layer below
            return palette[floor_id]
        return _EMPTY

    # -- spectator payload ----------------------------------------------

    def grid(self, level, tile_x, tile_z, radius=16):
        """Compact colour grid around (tile_x,tile_z): palette + rows of indices."""
        size = radius * 2 + 1
        origin_x = tile_x - radius
        origin_z = tile_z - radius
        palette = []
        index_of = {}
        rows = []
        for delta_z in range(size):
            row = []
            for delta_x in range(size):
                colour = self.colour_at(level, origin_x + delta_x, origin_z + delta_z)
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
