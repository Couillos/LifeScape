"""Floor colours and textures read from the packed game cache.

The ground file of each map square stores, per 64x64 level, an underlay
and an overlay floor id. ``client/config`` (``flo.dat`` inside) maps
those ids to an RGB colour and, for textured floors, to a texture id
(``content/pack/texture.pack`` maps those to ``content/textures/*.png``,
the true colours). The spectator prefers the overlay (paths, water,
floors) and falls back to the underlay (grass, sand, rock) when absent.

Stdlib only. Each map square is parsed once and cached in memory; the
daemon serves a small colour grid around the bot, never the whole map.
The detailed tile mode additionally serves per-tile floor ids plus the
flo/texture tables, so the browser can paint true texture colours.
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

# Textured floors have no colour of their own. Water (1), fountain (17) and
# gungy water (25) are the only ones overlaid on the spectator map.
_TEXTURE_RGB = {1: 0x3f76e4, 17: 0x3f76e4, 25: 0x2f5f9e}


class MapCache:
    def __init__(self):
        self._floor_colours = None
        self._floor_textures = None
        self._floor_info = None
        self._texture_names = None
        self._texture_pixels_cache = {}
        self._texture_cells_cache = {}
        self._texture_warned = set()
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
            colours.append(rgb or _TEXTURE_RGB.get(texture, 0))
            textures.append(texture)
        return colours, textures

    def _ensure_flo(self):
        if self._floor_colours is None or self._floor_textures is None:
            with self._lock:
                if self._floor_colours is None or self._floor_textures is None:
                    colours, textures = self._load_flo()
                    self._floor_colours = colours
                    self._floor_textures = textures

    def _floor(self):
        self._ensure_flo()
        return self._floor_colours

    def _flo_tex(self):
        self._ensure_flo()
        return self._floor_textures

    # -- content textures -------------------------------------------------

    def _content_dir(self):
        candidate = os.path.normpath(os.path.join(PACK_DIR, "..", "..", "..", "content"))
        if os.path.isdir(candidate):
            return candidate
        return os.path.normpath(os.path.join(ROOT, "..", "lostcity", "content"))

    def tex_names(self):
        """texture id -> texture name, from ``content/pack/texture.pack``."""
        if self._texture_names is None:
            with self._lock:
                if self._texture_names is None:
                    names = {}
                    try:
                        pack_path = os.path.join(self._content_dir(), "pack", "texture.pack")
                        with open(pack_path, encoding="ascii") as handle:
                            for pack_line in handle:
                                pack_line = pack_line.strip()
                                if not pack_line or pack_line.startswith("#") or "=" not in pack_line:
                                    continue
                                texture_id, texture_name = pack_line.split("=", 1)
                                try:
                                    names[int(texture_id.strip())] = texture_name.strip()
                                except ValueError:
                                    continue
                    except OSError:
                        pass
                    self._texture_names = names
        return self._texture_names

    def texture_png(self, texture_name):
        """Raw PNG bytes for a texture, cached in RAM; None if unknown."""
        if not texture_name or not all(char.isalnum() or char == "_" for char in texture_name):
            return None
        if texture_name not in set(self.tex_names().values()):
            return None
        if texture_name not in self._texture_files:
            with self._lock:
                if texture_name not in self._texture_files:
                    try:
                        png_path = os.path.join(self._content_dir(), "textures", texture_name + ".png")
                        with open(png_path, "rb") as handle:
                            self._texture_files[texture_name] = handle.read()
                    except OSError:
                        return None
        return self._texture_files[texture_name]

    # -- server-side texture pixels -------------------------------------

    def _decode_png(self, png_bytes):
        """Decode an 8-bit non-interlaced RGB/RGBA PNG (stdlib only).

        Returns (width, height, rgb_bytes). Raises ValueError on anything
        fancier: the cache textures are all plain 8-bit RGBA.
        """
        if png_bytes[:8] != b"\x89PNG\r\n\x1a\n":
            raise ValueError("not a PNG")
        position = 8
        width = height = 0
        bit_depth = color_type = None
        interlace = 1
        image_data = bytearray()
        while position + 8 <= len(png_bytes):
            (length,) = struct.unpack(">I", png_bytes[position:position + 4])
            chunk = png_bytes[position + 4:position + 8]
            body = png_bytes[position + 8:position + 8 + length]
            if chunk == b"IHDR":
                width, height, bit_depth, color_type, _comp, _filter, interlace = struct.unpack(">IIBBBBB", body)
            elif chunk == b"IDAT":
                image_data += body
            elif chunk == b"IEND":
                break
            position += 12 + length
        if bit_depth != 8 or color_type not in (2, 6) or interlace != 0:
            raise ValueError("unsupported PNG: depth=%r type=%r interlace=%r" % (bit_depth, color_type, interlace))
        channels = 3 if color_type == 2 else 4
        raw = zlib.decompress(bytes(image_data))
        stride = width * channels
        pixels = bytearray(width * height * 3)
        previous = bytearray(stride)
        offset = 0
        for row in range(height):
            current = bytearray(raw[offset + 1:offset + 1 + stride])
            filter_kind = raw[offset]
            offset += 1 + stride
            for index in range(stride):
                left = current[index - channels] if index >= channels else 0
                up = previous[index]
                upper_left = previous[index - channels] if index >= channels else 0
                if filter_kind == 1:
                    current[index] = (current[index] + left) & 0xFF
                elif filter_kind == 2:
                    current[index] = (current[index] + up) & 0xFF
                elif filter_kind == 3:
                    current[index] = (current[index] + ((left + up) >> 1)) & 0xFF
                elif filter_kind == 4:
                    guess = left + up - upper_left
                    distance_left = abs(guess - left)
                    distance_up = abs(guess - up)
                    distance_diag = abs(guess - upper_left)
                    if distance_left <= distance_up and distance_left <= distance_diag:
                        paeth = left
                    elif distance_up <= distance_diag:
                        paeth = up
                    else:
                        paeth = upper_left
                    current[index] = (current[index] + paeth) & 0xFF
            previous = current
            row_base = row * width * 3
            if channels == 3:
                pixels[row_base:row_base + width * 3] = bytes(current)
            else:
                pixels[row_base:row_base + width * 3] = bytes(
                    channel for index, channel in enumerate(current) if index % 4 != 3
                )
        return width, height, bytes(pixels)

    def _texture_pixels(self, texture_name):
        """(width, height, rgb_bytes) for a texture, decoded once, kept in RAM."""
        if texture_name not in self._texture_pixels_cache:
            with self._lock:
                if texture_name not in self._texture_pixels_cache:
                    png_path = os.path.join(self._content_dir(), "textures", texture_name + ".png")
                    with open(png_path, "rb") as handle:
                        self._texture_pixels_cache[texture_name] = self._decode_png(handle.read())
        return self._texture_pixels_cache[texture_name]

    def _texture_cells(self, texture_name, resolution):
        """resolution x resolution true colours for a texture, cached in RAM.

        The server downsamples: y=1 is the true average colour, higher y
        gets closer to the in-game texture. Keyed (texture, resolution) so
        identical tiles share one rendering.
        """
        key = (texture_name, resolution)
        if key not in self._texture_cells_cache:
            with self._lock:
                if key not in self._texture_cells_cache:
                    width, height, pixels = self._texture_pixels(texture_name)
                    cells = []
                    for cell_y in range(resolution):
                        top = cell_y * height // resolution
                        bottom = (cell_y + 1) * height // resolution
                        for cell_x in range(resolution):
                            left = cell_x * width // resolution
                            right = (cell_x + 1) * width // resolution
                            red = green = blue = count = 0
                            for sample_y in range(top, bottom):
                                row_base = sample_y * width * 3
                                for sample_x in range(left, right):
                                    pixel_base = row_base + sample_x * 3
                                    red += pixels[pixel_base]
                                    green += pixels[pixel_base + 1]
                                    blue += pixels[pixel_base + 2]
                                    count += 1
                            count = count or 1
                            cells.append("#%02x%02x%02x" % (red // count, green // count, blue // count))
                    self._texture_cells_cache[key] = cells
        return self._texture_cells_cache[key]

    def detail_table(self, resolution):
        """Everything the browser needs for detailed tiles, at resolution y.

        Sent over the websocket (never HTTP): per-flo colour+texture plus
        the downsampled texture pixels. The canvas paints tiles from it
        asynchronously as it arrives, reusing identical tiles from RAM.
        """
        resolution = min(32, max(1, int(resolution)))
        floors = self.flo_info()
        wanted = sorted({entry["texture"] for entry in floors.values() if entry["texture"]})
        textures = {}
        for texture_name in wanted:
            try:
                textures[texture_name] = self._texture_cells(texture_name, resolution)
            except (OSError, ValueError, struct.error, zlib.error) as decode_error:
                if texture_name not in self._texture_warned:
                    self._texture_warned.add(texture_name)
                    print("detail: cannot decode %s.png (%r), flat colour used" % (texture_name, decode_error), flush=True)
        return {"y": resolution, "flo": floors, "tex": textures}

    def flo_info(self):
        """flo id -> {color, texture}: the lookup table for detailed mode."""
        if self._floor_info is None:
            with self._lock:
                if self._floor_info is None:
                    colours = self._floor()
                    textures = self._flo_tex()
                    names = self.tex_names()
                    info = {}
                    for floor_id, rgb in enumerate(colours):
                        texture_id = textures[floor_id] if floor_id < len(textures) else -1
                        info[str(floor_id)] = {
                            "color": "#%02x%02x%02x" % ((rgb >> 16) & 0xFF, (rgb >> 8) & 0xFF, rgb & 0xFF),
                            "texture": names.get(texture_id),
                        }
                    self._floor_info = info
        return self._floor_info

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

    def tile_at(self, level, tile_x, tile_z):
        """(flat colour, effective floor id) for one ground tile."""
        underlay, overlay = self._tile_ids(level, tile_x, tile_z)
        colours = self._floor()
        for floor_id in (overlay, underlay):
            if floor_id < 0 or floor_id >= len(colours):
                continue
            rgb = colours[floor_id]
            colour = "#%02x%02x%02x" % ((rgb >> 16) & 0xFF, (rgb >> 8) & 0xFF, rgb & 0xFF)
            if colour != "#ff00ff":  # texture placeholder, not a real ground colour
                return colour, floor_id
        return _EMPTY, -1

    def colour_at(self, level, tile_x, tile_z):
        colour, _floor_id = self.tile_at(level, tile_x, tile_z)
        return colour

    def flo_at(self, level, tile_x, tile_z):
        """Effective floor id (overlay first, like colour_at); -1 if none."""
        _colour, floor_id = self.tile_at(level, tile_x, tile_z)
        return floor_id

    # -- spectator payload ----------------------------------------------

    def grid(self, level, tile_x, tile_z, radius=16):
        """Compact colour grid around (tile_x,tile_z): palette + rows of
        indices, plus the effective floor id per tile for detailed mode."""
        size = radius * 2 + 1
        origin_x = tile_x - radius
        origin_z = tile_z - radius
        palette = []
        index_of = {}
        rows = []
        floors = []
        for delta_z in range(size):
            row = []
            floor_row = []
            for delta_x in range(size):
                colour, floor_id = self.tile_at(level, origin_x + delta_x, origin_z + delta_z)
                index = index_of.get(colour)
                if index is None:
                    index = len(palette)
                    index_of[colour] = index
                    palette.append(colour)
                row.append(index)
                floor_row.append(floor_id)
            rows.append(row)
            floors.append(floor_row)
        return {
            "x": origin_x,
            "z": origin_z,
            "size": size,
            "palette": palette,
            "tiles": rows,
            "flo": floors,
        }
