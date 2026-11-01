"""Read-only access to the packed cache the server built.

Mirrors FileStream.read(archive, file): index entries are 6 bytes
(size u24 + sector u24), data lives in 520-byte sectors holding a
8-byte header (file u16, part u16, next u24, archive+1 u8) and 512
payload bytes. Bytes are returned exactly as stored, no decompression,
which is what the login CRC check hashes.
"""

import os
import zlib

from .buffer import Reader

SECTOR_SIZE = 520
INDEX_COUNT = 5


def _read_at(path, pos, length):
    with open(path, "rb") as handle:
        handle.seek(pos)
        return handle.read(length)


def archive_count(pack_dir, archive):
    return os.path.getsize(_index_path(pack_dir, archive)) // 6


def read_file(pack_dir, archive, file):
    index = _read_at(_index_path(pack_dir, archive), file * 6, 6)
    size = int.from_bytes(index[0:3], "big")
    sector = int.from_bytes(index[3:6], "big")
    if size > 2000000 or sector <= 0:
        return None
    out = bytearray()
    part = 0
    while len(out) < size:
        header = Reader(_read_at(_dat_path(pack_dir), sector * SECTOR_SIZE, SECTOR_SIZE))
        entry_file = header.u16()
        entry_part = header.u16()
        entry_next = header.u24()
        entry_archive = header.u8() - 1
        if entry_file != file or entry_part != part or entry_archive != archive:
            return None
        out += header.raw(min(512, size - len(out)))
        sector = entry_next
        part += 1
        if sector == 0:
            break
    return bytes(out)


def login_crcs(pack_dir):
    crcs = []
    for file in range(9):
        blob = read_file(pack_dir, 0, file)
        crcs.append(0 if blob is None else zlib.crc32(blob) & 0xFFFFFFFF)
    return b"".join(crc.to_bytes(4, "big") for crc in crcs)


def _index_path(pack_dir, archive):
    return os.path.join(pack_dir, "main_file_cache.idx%d" % archive)


def _dat_path(pack_dir):
    return os.path.join(pack_dir, "main_file_cache.dat")
