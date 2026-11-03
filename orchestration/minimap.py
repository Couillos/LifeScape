"""The client's own minimap image: ground, walls and mapscene sprites.

World.render2DGround paints the ground, then Client.drawDetail paints the
wall bars, the diagonal walls and the mapscene sprites of every loc, in the
same order as the game. This module replays both passes on the packed map
data so the spectator blits exactly what the client would build.
"""

import os
import struct
import threading
import zlib

from lifescape import collision
from lifescape.buffer import Reader
from lifescape.connection import default_paths

from .mapcache import LEVELS, MapCache

ACTIVE_RGB = 0xEE0000
INACTIVE_RGB = 0xEEEEEE

WALL_STRAIGHT = 0
WALL_DIAGONAL_CORNER = 1
WALL_L = 2
WALL_SQUARE_CORNER = 3
WALL_DIAGONAL = 9
GROUND_DECOR = 22

MAP_FLAG_VIS_BELOW = 0x08
MAP_FLAG_FORCE_HIGH_DETAIL = 0x10

# the client's wall bars, in tile pixel offsets (x, y) with y 0 at the north
_WEST_COLUMN = ((0, 0), (0, 1), (0, 2), (0, 3))
_NORTH_ROW = ((0, 0), (1, 0), (2, 0), (3, 0))
_EAST_COLUMN = ((3, 0), (3, 1), (3, 2), (3, 3))
_SOUTH_ROW = ((0, 3), (1, 3), (2, 3), (3, 3))
_L_COLUMN = ((0, 0), (0, 1), (0, 2), (0, 3))
_L_ROW = ((0, 0), (1, 0), (2, 0), (3, 0))


class Minimap:
    """Builds the 4 px per tile minimap image for a window of world tiles."""

    def __init__(self, cache=None):
        self.cache = cache or MapCache()
        self._layers = None
        self._lock = threading.RLock()

    # -- loc placement ----------------------------------------------------

    def _loc_layers(self):
        if self._layers is None:
            with self._lock:
                if self._layers is None:
                    self._layers = _load_loc_layers(default_paths()["pack_dir"])
        return self._layers

    # -- render -----------------------------------------------------------

    def render(self, level, world_x0, world_z0, size, margin=10):
        """RGB pixels for a size x size window, row 0 at world_z0.

        Details are drawn from a padded window so mapscene sprites coming
        from the tiles around the window still land inside it.
        """
        left = world_x0 - margin
        top = world_z0 - margin
        span = size + margin * 2
        pixels = self.cache.render_ground(level, left, top, span)
        self._draw_details(pixels, level, left, top, span)
        if margin == 0:
            return pixels
        return _crop(pixels, span * 4, margin * 4, size * 4)

    def _draw_details(self, pixels, level, world_x0, world_z0, size):
        stride = size * 4
        layers = self._loc_layers()
        for world_z in range(world_z0, world_z0 + size):
            for world_x in range(world_x0, world_x0 + size):
                if (self.cache._tile_ids(level, world_x, world_z)[4] & 0x18) == 0:
                    self._draw_detail(pixels, stride, level, layers, world_x, world_z, world_x0, world_z0, size)
                if level < LEVELS - 1 and self.cache._tile_ids(level + 1, world_x, world_z)[4] & MAP_FLAG_VIS_BELOW:
                    self._draw_detail(pixels, stride, level + 1, layers, world_x, world_z, world_x0, world_z0, size)

    def _draw_detail(self, pixels, stride, level, layers, world_x, world_z, world_x0, world_z0, size):
        info = self.cache._loc_types()
        tx = (world_x - world_x0) * 4
        ty = (size - 1 - (world_z - world_z0)) * 4

        wall = layers.get((world_x, world_z, level, collision.LAYER_WALL))
        if wall:
            loc_id, shape, angle = wall
            loc = info.get(loc_id)
            if loc is not None and loc["mapscene"] >= 0:
                self._plot_sprite(pixels, stride, world_x, world_z, loc, world_x0, world_z0, size)
            elif loc is not None:
                rgb = ACTIVE_RGB if loc["active"] else INACTIVE_RGB
                self._draw_wall(pixels, stride, tx, ty, shape, angle, rgb)

        scene = layers.get((world_x, world_z, level, collision.LAYER_GROUND))
        if scene:
            loc_id, shape, angle = scene
            loc = info.get(loc_id)
            if loc is not None:
                if loc["mapscene"] >= 0:
                    self._plot_sprite(pixels, stride, world_x, world_z, loc, world_x0, world_z0, size)
                elif shape == WALL_DIAGONAL:
                    rgb = ACTIVE_RGB if loc["active"] else INACTIVE_RGB
                    diagonal = (((0, 3), (1, 2), (2, 1), (3, 0)) if angle in (0, 2)
                                else ((0, 0), (1, 1), (2, 2), (3, 3)))
                    for dx, dy in diagonal:
                        pixels[(ty + dy) * stride + tx + dx] = rgb

        decor = layers.get((world_x, world_z, level, collision.LAYER_GROUND_DECOR))
        if decor:
            loc_id, _shape, _angle = decor
            loc = info.get(loc_id)
            if loc is not None and loc["mapscene"] >= 0:
                self._plot_sprite(pixels, stride, world_x, world_z, loc, world_x0, world_z0, size)

    @staticmethod
    def _draw_wall(pixels, stride, tx, ty, shape, angle, rgb):
        if shape in (WALL_STRAIGHT, WALL_L):
            bars = (_WEST_COLUMN, _NORTH_ROW, _EAST_COLUMN, _SOUTH_ROW)
            for dx, dy in bars[angle & 3]:
                pixels[(ty + dy) * stride + tx + dx] = rgb
        if shape == WALL_L:
            # WALL_L adds a second bar, with the angles shifted by one
            second = (_NORTH_ROW, _EAST_COLUMN, _SOUTH_ROW, _WEST_COLUMN)
            for dx, dy in second[angle & 3]:
                pixels[(ty + dy) * stride + tx + dx] = rgb
        if shape == WALL_SQUARE_CORNER:
            corners = ((0, 0), (3, 0), (3, 3), (0, 3))
            dx, dy = corners[angle & 3]
            pixels[(ty + dy) * stride + tx + dx] = rgb

    def _plot_sprite(self, pixels, stride, world_x, world_z, loc, world_x0, world_z0, size):
        sprite = self.cache.mapscene_sprite(loc["mapscene"])
        if sprite is None:
            return
        width, height, xof, yof, data, palette = sprite
        offset_x = int((loc["width"] * 4 - width) / 2)
        offset_y = int((loc["length"] * 4 - height) / 2)
        left = (world_x - world_x0) * 4 + offset_x + xof
        top = (size - 1 - (world_z - world_z0)) * 4 + (1 - loc["length"]) * 4 + offset_y + yof
        border = size * 4
        for y in range(height):
            dst_y = top + y
            if dst_y < 0 or dst_y >= border:
                continue
            row = dst_y * stride
            source = y * width
            for x in range(width):
                index = data[source + x]
                if index == 0:
                    continue
                dst_x = left + x
                if dst_x < 0 or dst_x >= border:
                    continue
                pixels[row + dst_x] = palette[index]

    # -- png --------------------------------------------------------------

    def png(self, level, world_x0, world_z0, size, margin=10):
        pixels = self.render(level, world_x0, world_z0, size, margin)
        border = size * 4
        rows = []
        for y in range(border):
            row = pixels[y * border:(y + 1) * border]
            line = bytearray()
            for value in row:
                line += bytes(((value >> 16) & 0xFF, (value >> 8) & 0xFF, value & 0xFF))
            rows.append(bytes(line))
        return _png(border, border, rows)


def _load_loc_layers(pack_dir):
    """(x, z, level, layer) -> (loc id, shape, angle), as the client places them."""
    layers = {}
    for square_x, square_z, _ground, locs in collision._map_files(pack_dir):
        packet = Reader(locs)
        loc_id = -1
        delta = packet.smarts()
        while delta != 0:
            loc_id += delta
            coord = 0
            coord_offset = packet.smarts()
            while coord_offset != 0:
                coord += coord_offset - 1
                z = coord & 0x3F
                x = (coord >> 6) & 0x3F
                level = (coord >> 12) & 0x3
                info = packet.u8()
                coord_offset = packet.smarts()
                shape = info >> 2
                angle = info & 0x3
                layers[(x + square_x, z + square_z, level, collision.layer_of(shape))] = (loc_id, shape, angle)
            delta = packet.smarts()
    return layers


def _crop(pixels, stride, margin, size):
    out = [0] * (size * size)
    for y in range(size):
        start = (y + margin) * stride + margin
        out[y * size:(y + 1) * size] = pixels[start:start + size]
    return out


def _png(width, height, rows):
    raw = b"".join(b"\x00" + row for row in rows)

    def chunk(kind, body):
        return struct.pack(">I", len(body)) + kind + body + struct.pack(">I", zlib.crc32(kind + body) & 0xFFFFFFFF)

    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw))
        + chunk(b"IEND", b"")
    )
