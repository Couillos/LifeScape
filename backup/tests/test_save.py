import os
import tempfile
import unittest


from lifescape.buffer import Writer
from lifescape.save import parse

COINS = 995
IRON_CHAINBODY = 1101
POT_FLOUR = 1933


def build_save():
    out = Writer()
    out.u16(0x2004)
    out.u16(7)
    out.u16(3222)
    out.u16(3222)
    out.u8(0)
    out.raw(bytes(7))
    out.raw(bytes(5))
    out.u8(0)
    out.u16(1000)
    out.i32(12345)
    for index in range(21):
        out.i32(index * 100)
        out.u8(index + 1)
    out.u16(2)
    out.u16(29)
    out.u8(2)  # varint 2
    out.u16(101)
    out.raw(b"\x89\x52")  # varint 1234
    out.u8(3)
    for inv_type, size, placed in (
        (3214, 28, {0: (COINS, 5)}),
        (1688, 14, {0: (IRON_CHAINBODY, 1)}),
        (5382, 100, {0: (POT_FLOUR, 1)}),
    ):
        out.u16(inv_type)
        out.u16(size)
        for slot in range(size):
            if slot in placed:
                obj_id, count = placed[slot]
                out.u16(obj_id + 1)
                out.u8(count)
            else:
                out.u16(0)
    return out.bytes()


class SaveTest(unittest.TestCase):
    def test_reads_stats_varps_and_the_three_containers(self):
        handle = tempfile.NamedTemporaryFile(delete=False)
        handle.write(build_save())
        handle.close()
        try:
            saved = parse(handle.name)
        finally:
            os.unlink(handle.name)
        self.assertEqual(saved["stats"][0], (1, 0))
        self.assertEqual(saved["varps"], {29: 2, 101: 1234})
        self.assertEqual(saved["inv"], {COINS: 5})
        self.assertEqual(saved["worn"], {IRON_CHAINBODY: 1})
        self.assertEqual(saved["bank"], {POT_FLOUR: 1})

    def test_missing_file_is_empty(self):
        self.assertEqual(parse("/tmp/opencode/does-not-exist.sav"), {})


if __name__ == "__main__":
    unittest.main()
