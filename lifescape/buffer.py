"""Big-endian packet reader and writer, mirroring the server Packet layout."""


class Reader:
    def __init__(self, data):
        self._data = bytes(data)
        self.pos = 0

    def remaining(self):
        return len(self._data) - self.pos

    def peek_u8(self):
        return self._data[self.pos]

    def raw(self, length):
        chunk = self._data[self.pos:self.pos + length]
        self.pos += length
        return chunk

    def u8(self):
        value = self._data[self.pos]
        self.pos += 1
        return value

    def u16(self):
        value = int.from_bytes(self._data[self.pos:self.pos + 2], "big")
        self.pos += 2
        return value

    def i16(self):
        value = int.from_bytes(self._data[self.pos:self.pos + 2], "big", signed=True)
        self.pos += 2
        return value

    def u24(self):
        value = int.from_bytes(self._data[self.pos:self.pos + 3], "big")
        self.pos += 3
        return value

    def i32(self):
        value = int.from_bytes(self._data[self.pos:self.pos + 4], "big", signed=True)
        self.pos += 4
        return value

    def u32(self):
        value = int.from_bytes(self._data[self.pos:self.pos + 4], "big")
        self.pos += 4
        return value

    def boolean(self):
        return self.u8() == 1

    def jstr(self, terminator=10):
        end = self._data.index(terminator, self.pos)
        text = self._data[self.pos:end].decode("latin-1")
        self.pos = end + 1
        return text

    def varint(self):
        value = 0
        while True:
            byte = self.u8()
            value = (value << 7) | (byte & 0x7F)
            if not byte & 0x80:
                return value

    def smart(self):
        if self._data[self.pos] < 0x80:
            return self.u8() - 0x40
        return self.u16() - 0xC000

    def smarts(self):
        if self._data[self.pos] < 0x80:
            return self.u8()
        return self.u16() - 0x8000


class Writer:
    def __init__(self):
        self._out = bytearray()

    def raw(self, data):
        self._out += data

    def u8(self, value):
        self._out.append(value & 0xFF)

    def i8(self, value):
        self._out += value.to_bytes(1, "big", signed=True)

    def u16(self, value):
        self._out += (value & 0xFFFF).to_bytes(2, "big")

    def i16(self, value):
        self._out += value.to_bytes(2, "big", signed=True)

    def u24(self, value):
        self._out += (value & 0xFFFFFF).to_bytes(3, "big")

    def i32(self, value):
        self._out += value.to_bytes(4, "big", signed=True)

    def u32(self, value):
        self._out += (value & 0xFFFFFFFF).to_bytes(4, "big")

    def u64(self, value):
        self._out += (value & 0xFFFFFFFFFFFFFFFF).to_bytes(8, "big")

    def boolean(self, value):
        self.u8(1 if value else 0)

    def jstr(self, text, terminator=10):
        self._out += text.encode("latin-1")
        self._out.append(terminator)

    def smart(self, value):
        if -64 <= value < 64:
            self.u8(value + 64)
        else:
            self.u16(value + 0xC000)

    def bytes(self):
        return bytes(self._out)


class BitReader:
    def __init__(self, data):
        self._data = bytes(data)
        self._bit = 0

    def bits(self, count):
        value = 0
        for _ in range(count):
            value <<= 1
            value |= (self._data[self._bit >> 3] >> (7 - (self._bit & 7))) & 1
            self._bit += 1
        return value

    def remaining_bits(self):
        return len(self._data) * 8 - self._bit
