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

_EMPTY = "#000000"
_EMPTY_VALUE = 0x000000

# World.ts MINIMAP_SHAPE / MINIMAP_ROTATE: the client paints an overlay tile
# as 4x4 pixels, one per mask cell, mixing overlay and underlay colour.
MINIMAP_SHAPE = (
    (0,) * 16,
    (1,) * 16,
    (1, 0, 0, 0, 1, 1, 0, 0, 1, 1, 1, 0, 1, 1, 1, 1),
    (1, 1, 0, 0, 1, 1, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0),
    (0, 0, 1, 1, 0, 0, 1, 1, 0, 0, 0, 1, 0, 0, 0, 1),
    (0, 1, 1, 1, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1),
    (1, 1, 1, 0, 1, 1, 1, 0, 1, 1, 1, 1, 1, 1, 1, 1),
    (1, 1, 0, 0, 1, 1, 0, 0, 1, 1, 0, 0, 1, 1, 0, 0),
    (0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 1, 1, 0, 0),
    (1, 1, 1, 1, 1, 1, 1, 1, 0, 1, 1, 1, 0, 0, 1, 1),
    (1, 1, 1, 1, 1, 1, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0),
    (0, 0, 0, 0, 0, 0, 1, 1, 0, 1, 1, 1, 0, 1, 1, 1),
    (0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 0, 1, 1, 1, 1),
)

MINIMAP_ROTATE = (
    (0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15),
    (12, 8, 4, 0, 13, 9, 5, 1, 14, 10, 6, 2, 15, 11, 7, 3),
    (15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0),
    (3, 7, 11, 15, 2, 6, 10, 14, 1, 5, 9, 13, 0, 4, 8, 12),
)

# Client minimap palette chain (Client-TS, branch 274): the client builds a
# 512x128 HSL colour table, gamma-corrects it, and indexes it from the flo
# HSL. Reproduced here so the spectator paints the same colours.
_BRIGHTNESS = 0.8


def _gamma(rgb, gamma):
    value = 0
    for shift in (16, 8, 0):
        channel = (rgb >> shift) & 0xFF
        value |= (int(pow(channel / 256.0, gamma) * 256.0) & 0xFF) << shift
    return value


def _hsl_to_rgb(hue, saturation, lightness):
    red = green = blue = lightness
    if saturation != 0.0:
        if lightness < 0.5:
            second = lightness * (saturation + 1.0)
        else:
            second = lightness + saturation - lightness * saturation
        first = lightness * 2.0 - second
        values = []
        for shift in (1.0 / 3.0, 0.0, -1.0 / 3.0):
            position = hue + shift
            if position > 1.0:
                position -= 1.0
            elif position < 0.0:
                position += 1.0
            if position * 6.0 < 1.0:
                values.append(first + (second - first) * 6.0 * position)
            elif position * 2.0 < 1.0:
                values.append(second)
            elif position * 3.0 < 2.0:
                values.append(first + (second - first) * (2.0 / 3.0 - position) * 6.0)
            else:
                values.append(first)
        red, green, blue = values
    return (int(red * 256.0) << 16) + (int(green * 256.0) << 8) + int(blue * 256.0)


def _colour_table(index):
    """Pix3D.colourTable[index] for the default brightness."""
    hue = ((index >> 7) // 8) / 64.0 + 0.0078125
    saturation = ((index >> 7) & 0x7) / 8.0 + 0.0625
    lightness = (index & 0x7F) / 128.0
    return _gamma(_hsl_to_rgb(hue, saturation, lightness), _BRIGHTNESS)


def _hsl_index(hue, saturation, lightness):
    for threshold in (179, 192, 217, 243):
        if lightness > threshold:
            saturation //= 2
    return ((hue // 4) << 10) + ((saturation // 32) << 7) + (lightness // 2)


def _floor_colour(rgb):
    """Flo rgb -> HSL, the way FloType stores it."""
    red = ((rgb >> 16) & 0xFF) / 256.0
    green = ((rgb >> 8) & 0xFF) / 256.0
    blue = (rgb & 0xFF) / 256.0
    low = min(red, green, blue)
    high = max(red, green, blue)
    lightness = (low + high) / 2.0
    hue = saturation = 0.0
    if low != high:
        if lightness < 0.5:
            saturation = (high - low) / (high + low)
        else:
            saturation = (high - low) / (2.0 - high - low)
        if red == high:
            hue = (green - blue) / (high - low)
        elif green == high:
            hue = (blue - red) / (high - low) + 2.0
        else:
            hue = (red - green) / (high - low) + 4.0
    hue /= 6.0
    hue_i = int(hue * 256.0)
    saturation_i = max(0, min(255, int(saturation * 256.0)))
    lightness_i = max(0, min(255, int(lightness * 256.0)))
    if lightness > 0.5:
        chroma = int((1.0 - lightness) * saturation * 512.0)
    else:
        chroma = int(lightness * saturation * 512.0)
    chroma = max(1, chroma)
    underlay_hue = int(hue * chroma)
    return hue_i, saturation_i, lightness_i, chroma, underlay_hue


def _blended_colour(hue, saturation, lightness):
    """ClientBuild: neighbourhood-average HSL -> colourTable (getUCol 96)."""
    index = _hsl_index(hue & 0xFF, saturation, lightness)
    scaled = max(2, min(126, int((96 * (index & 0x7F)) / 128)))
    return _colour_table((index & 0xFF80) + scaled)


def _png_rgba(width, height, pixels, palette):
    """RGBA PNG for a paletted sprite; palette index 0 stays transparent."""
    raw = bytearray()
    for y in range(height):
        raw.append(0)
        for x in range(width):
            index = pixels[x + y * width]
            if index == 0 or index >= len(palette):
                raw += b"\x00\x00\x00\x00"
            else:
                rgb = palette[index]
                raw += bytes(((rgb >> 16) & 0xFF, (rgb >> 8) & 0xFF, rgb & 0xFF, 255))

    def chunk(kind, body):
        return struct.pack(">I", len(body)) + kind + body + struct.pack(">I", zlib.crc32(kind + body) & 0xFFFFFFFF)

    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(bytes(raw)))
    png += chunk(b"IEND", b"")
    return png


class MapCache:
    def __init__(self):
        self._floor_rgb = None
        self._floor_textures = None
        self._texture_averages = {}
        self._palette = None
        self._hsl_cache = {}
        self._blend_cache = {}
        self._mapscene_png = {}
        self._mapscene_size = {}
        self._loc_type_table = None
        self._media = None
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

    def _loc_types(self):
        """loc id -> {mapscene, width, length, mapfunction} from the cache.

        The client draws the mapscene sprite on the minimap instead of the
        ground (LocType, opcode 68), which is how buildings show as one
        image rather than a grid of floor colours.
        """
        if self._loc_type_table is None:
            with self._lock:
                if self._loc_type_table is None:
                    table = {}
                    try:
                        container = self._jagfile()
                        data = self._jagfile_read(container, "loc.dat")
                        index = self._jagfile_read(container, "loc.idx")
                    except (OSError, ValueError):
                        data = index = None
                    if data and index:
                        count = (index[0] << 8) | index[1]
                        offsets = []
                        offset = 2
                        cursor = 2
                        for _id in range(count):
                            offsets.append(offset)
                            offset += (index[cursor] << 8) | index[cursor + 1]
                            cursor += 2
                        for id in range(count):
                            table[id] = self._decode_loc_type(data, offsets[id])
                    self._loc_type_table = table
        return self._loc_type_table

    @staticmethod
    def _decode_loc_type(data, position):
        entry = {"mapscene": -1, "width": 1, "length": 1, "mapfunction": -1}
        while position < len(data):
            opcode = data[position]
            position += 1
            if opcode == 0:
                break
            if opcode == 1:
                count = data[position]
                position += 1 + count * 3
            elif opcode in (2, 3):
                position += 1
                while position < len(data) and data[position] != 10:
                    position += 1
                position += 1
            elif opcode == 5:
                count = data[position]
                position += 1 + count * 2
            elif opcode == 14:
                entry["width"] = data[position]
                position += 1
            elif opcode == 15:
                entry["length"] = data[position]
                position += 1
            elif opcode in (17, 18, 19, 21, 22, 23, 28, 29, 39):
                position += 1
            elif opcode == 24:
                position += 2
            elif 30 <= opcode < 39:
                position += 1
                while position < len(data) and data[position] != 10:
                    position += 1
                position += 1
            elif opcode == 40:
                count = data[position]
                position += 1 + count * 4
            elif opcode == 60:
                entry["mapfunction"] = (data[position] << 8) | data[position + 1]
                position += 2
            elif opcode in (62, 64):
                pass
            elif opcode in (65, 66, 67, 68):
                if opcode == 68:
                    entry["mapscene"] = (data[position] << 8) | data[position + 1]
                position += 2
            elif opcode in (69, 70, 71, 72, 73, 74, 75):
                position += 1
            else:
                break
        return entry

    def mapscene_for(self, loc_id):
        entry = self._loc_types().get(loc_id)
        return entry["mapscene"] if entry else -1

    def loc_dimensions(self, loc_id):
        entry = self._loc_types().get(loc_id)
        return (entry["width"], entry["length"]) if entry else (1, 1)

    def _mapscene_index(self, sprite_index):
        """(width, height, offset_x, offset_y, encoding, data_offset, palette)."""
        try:
            container = self._jagfile_read(self._media_jag(), "mapscene.dat")
            index = self._jagfile_read(self._media_jag(), "index.dat")
        except (OSError, ValueError):
            return None
        if not container or not index:
            return None
        position = (container[0] << 8) | container[1]
        palette_size = index[position + 4]
        palette = [0] * max(1, palette_size)
        cursor = position + 5
        for slot in range(1, palette_size):
            palette[slot] = (index[cursor] << 16) | (index[cursor + 1] << 8) | index[cursor + 2]
            cursor += 3
        position = cursor
        data_position = 2
        for _skip in range(sprite_index):
            position += 2
            skip_w = (index[position] << 8) | index[position + 1]
            skip_h = (index[position + 2] << 8) | index[position + 3]
            data_position += skip_w * skip_h
            position += 5
        offset_x = index[position]
        offset_y = index[position + 1]
        width = (index[position + 2] << 8) | index[position + 3]
        height = (index[position + 4] << 8) | index[position + 5]
        encoding = index[position + 6]
        if width <= 0 or height <= 0 or width > 512 or height > 512:
            return None
        return width, height, offset_x, offset_y, encoding, data_position, palette

    def mapscene_png(self, sprite_index):
        """PNG bytes for one mapscene sprite, decoded from the media jag."""
        if sprite_index in self._mapscene_png:
            return self._mapscene_png[sprite_index]
        meta = self._mapscene_index(sprite_index)
        if meta is None:
            return None
        width, height, _offset_x, _offset_y, encoding, data_position, palette = meta
        container = self._jagfile_read(self._media_jag(), "mapscene.dat")
        if container is None or data_position + width * height > len(container):
            return None
        pixels = [0] * (width * height)
        if encoding == 0:
            for index in range(width * height):
                pixels[index] = container[data_position + index] & 0xFF
        else:
            for x in range(width):
                for y in range(height):
                    pixels[x + y * width] = container[data_position + x * height + y] & 0xFF
        png = _png_rgba(width, height, pixels, palette)
        self._mapscene_png[sprite_index] = png
        return png

    def mapscene_origin(self, sprite_index):
        """(x, y) offset the sprite itself carries (Pix8.xof/yof)."""
        meta = self._mapscene_index(sprite_index)
        return (meta[2], meta[3]) if meta else (0, 0)

    def mapscene_size(self, sprite_index):
        """(width, height) of a mapscene sprite, or None."""
        if sprite_index in self._mapscene_size:
            return self._mapscene_size[sprite_index]
        meta = self._mapscene_index(sprite_index)
        if meta is None:
            return None
        size = (meta[0], meta[1])
        self._mapscene_size[sprite_index] = size
        return size

    def _media_jag(self):
        if self._media is None:
            self._media = self._jagfile_named("media")
        return self._media

    def _jagfile_named(self, name):
        path = os.path.join(PACK_DIR, "client", name)
        with open(path, "rb") as handle:
            return handle.read()

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
                        mean = self._decode_average(texture_name)
                        rgb = (mean[0] << 16) | (mean[1] << 8) | mean[2]
                        self._texture_averages[texture_name] = _gamma(rgb, 1.4) or 1
                    except (OSError, ValueError, struct.error, zlib.error):
                        self._texture_averages[texture_name] = None
        return self._texture_averages[texture_name]

    def palette(self):
        """flo id -> "#rrggbb" the client minimap paints, cached once.

        Same chain as the client: flo rgb -> HSL -> colourTable index ->
        HSL bitmap -> gamma. Textured floors use the texture average with
        gamma 1.4 (Pix3D.getTextureAverage).
        """
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
                            colour = self._overlay_colour(floor_id)
                        palette.append("#%02x%02x%02x" % ((colour >> 16) & 0xFF, (colour >> 8) & 0xFF, colour & 0xFF))
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
        grounds = [[(-1, -1, -1, 0)] * (MAP_SIZE * MAP_SIZE) for _level in range(LEVELS)]
        position = 0
        for level in range(LEVELS):
            for x in range(MAP_SIZE):
                for z in range(MAP_SIZE):
                    underlay = -1
                    overlay = -1
                    shape = -1
                    rotation = 0
                    while position < len(data):
                        opcode = data[position]
                        position += 1
                        if opcode == 0:
                            break
                        if opcode == 1:
                            position += 1
                            break
                        if opcode <= 49:
                            # the map byte is flo id + 1 (ClientBuild: FloType.list[t2 - 1])
                            overlay = data[position] - 1
                            position += 1
                            shape = (opcode - 2) // 4
                            rotation = (opcode - 2) & 3
                        elif opcode <= 81:
                            pass
                        else:
                            underlay = opcode - 82  # (opcode - 81) - 1
                    grounds[level][x * MAP_SIZE + z] = (underlay, overlay, shape, rotation)
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
            return -1, -1, -1, 0
        return square[level][(tile_x & 63) * MAP_SIZE + (tile_z & 63)]

    def _flo_hsl(self, floor_id):
        """Flo HSL, chroma and chroma-weighted hue, cached."""
        hsl = self._hsl_cache.get(floor_id)
        if hsl is None:
            rgb = self._floor()[floor_id] if 0 <= floor_id < len(self._floor()) else 0
            hsl = _floor_colour(rgb)
            self._hsl_cache[floor_id] = hsl
        return hsl

    def _overlay_colour(self, floor_id):
        """Overlay colour: flo overlayHsl -> colourTable (getOCol 96)."""
        hue, saturation, lightness, _chroma, _underlay_hue = self._flo_hsl(floor_id)
        index = _hsl_index(hue, saturation, lightness)
        scaled = max(2, min(126, int((96 * (index & 0x7F)) / 128)))
        return _colour_table((index & 0xFF80) + scaled)

    def _blend(self, level, tile_x, tile_z):
        """ClientBuild: underlay colour blended over a 11x11 window of tiles.

        The client keeps running HSL sums over five tiles each way and
        converts the average, which is why neighbouring tiles of the same
        flo id never share one flat colour.
        """
        key = (level, tile_x, tile_z)
        colour = self._blend_cache.get(key)
        if colour is not None:
            return colour
        hue_total = sat_total = lig_total = chroma_total = 0
        count = 0
        for delta_z in range(-5, 6):
            for delta_x in range(-5, 6):
                underlay, _overlay, _shape, _rotation = self._tile_ids(level, tile_x + delta_x, tile_z + delta_z)
                if underlay < 0:
                    continue
                underlay_hue, saturation, lightness, chroma, _weighted = self._flo_hsl(underlay)
                hue_total += _weighted
                sat_total += saturation
                lig_total += lightness
                chroma_total += chroma
                count += 1
        if count == 0 or chroma_total == 0:
            colour = _EMPTY_VALUE
        else:
            hue = int((hue_total * 256) / chroma_total) & 0xFF
            saturation = sat_total // count
            lightness = lig_total // count
            colour = _blended_colour(hue, saturation, lightness)
        if len(self._blend_cache) > 20000:
            self._blend_cache.clear()
        self._blend_cache[key] = colour
        return colour

    def colour_at(self, level, tile_x, tile_z):
        """Colour the client minimap paints: overlay if present, else blended underlay."""
        underlay, overlay, _shape, _rotation = self._tile_ids(level, tile_x, tile_z)
        rgba = self._floor()
        palette = self.palette()
        if 0 <= overlay < len(rgba):
            if rgba[overlay] == 0xFF00FF:
                return _EMPTY  # magenta flo: the client paints nothing here
            if overlay < len(palette):
                return palette[overlay]
        if 0 <= underlay < len(rgba) and rgba[underlay] != 0xFF00FF:
            return "#%06x" % self._blend(level, tile_x, tile_z)
        return _EMPTY

    # -- spectator payload ----------------------------------------------

    def grid(self, level, tile_x, tile_z, radius=16):
        """Compact grid around (tile_x,tile_z): palette + one entry per tile.

        A tile is either a palette index (flat colour) or {"u","o","m"}:
        the client paints partial overlay shapes as a 4x4 mask mixing the
        overlay and underlay colours (World.render2DGround).
        """
        size = radius * 2 + 1
        origin_x = tile_x - radius
        origin_z = tile_z - radius
        palette = []
        index_of = {}

        def index_for(colour):
            index = index_of.get(colour)
            if index is None:
                index = len(palette)
                index_of[colour] = index
                palette.append(colour)
            return index

        rows = []
        for delta_z in range(size):
            row = []
            for delta_x in range(size):
                tx = origin_x + delta_x
                tz = origin_z + delta_z
                underlay, overlay, shape, rotation = self._tile_ids(level, tx, tz)
                flat = self.colour_at(level, tx, tz)
                if overlay < 0 or flat == _EMPTY:
                    row.append(index_for(flat))
                    continue
                if shape <= 0:
                    row.append(index_for(flat))  # plain overlay: whole tile
                    continue
                mask = 0
                cells = MINIMAP_SHAPE[shape + 1] if shape + 1 < len(MINIMAP_SHAPE) else MINIMAP_SHAPE[0]
                rotated = MINIMAP_ROTATE[rotation & 3]
                for cell in range(16):
                    if cells[rotated[cell]]:
                        mask |= 1 << cell
                row_entry = {"u": index_for(self._blend(level, tx, tz)), "o": index_for(flat), "m": mask}
                row.append(row_entry)
            rows.append(row)
        return {
            "x": origin_x,
            "z": origin_z,
            "size": size,
            "palette": palette,
            "tiles": rows,
        }
