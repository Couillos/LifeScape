import unittest

from lifescape.buffer import BitReader, Reader, Writer


class ReaderTest(unittest.TestCase):
    def test_mirrors_server_layout(self):
        data = bytes([0x01, 0x12, 0x34, 0xFF, 0xFE, 0x11, 0x22, 0x33, ord("a"), ord("b"), 10])
        read = Reader(data)
        self.assertEqual(read.u8(), 1)
        self.assertEqual(read.u16(), 0x1234)
        self.assertEqual(read.i16(), -2)
        self.assertEqual(read.u24(), 0x112233)
        self.assertEqual(read.jstr(), "ab")
        self.assertEqual(read.remaining(), 0)

    def test_signed_32_and_smart(self):
        read = Reader(bytes([0xFF, 0xFF, 0xFF, 0xFE, 0x40 + 5, 0xC0 + 1, 0x34]))
        self.assertEqual(read.i32(), -2)
        self.assertEqual(read.smart(), 5)
        self.assertEqual(read.remaining(), 2)

    def test_smart_two_byte(self):
        read = Reader(bytes([0xC1, 0x34]))
        self.assertEqual(read.smart(), 0x134)


class WriterTest(unittest.TestCase):
    def test_round_trip(self):
        write = Writer()
        write.u8(250)
        write.u16(60000)
        write.i16(-300)
        write.u24(70000)
        write.i32(-70000)
        write.u32(4000000000)
        write.jstr("bite")
        blob = write.bytes()
        read = Reader(blob)
        self.assertEqual(read.u8(), 250)
        self.assertEqual(read.u16(), 60000)
        self.assertEqual(read.i16(), -300)
        self.assertEqual(read.u24(), 70000)
        self.assertEqual(read.i32(), -70000)
        self.assertEqual(read.u32(), 4000000000)
        self.assertEqual(read.jstr(), "bite")

    def test_big_endian_bytes(self):
        write = Writer()
        write.u16(0x1234)
        write.i32(-2)
        self.assertEqual(write.bytes(), bytes([0x12, 0x34, 0xFF, 0xFF, 0xFF, 0xFE]))


class BitReaderTest(unittest.TestCase):
    def test_msb_first(self):
        read = BitReader(bytes([0b10110000]))
        self.assertEqual(read.bits(1), 1)
        self.assertEqual(read.bits(2), 0b01)
        self.assertEqual(read.bits(5), 0b10000)


if __name__ == "__main__":
    unittest.main()
