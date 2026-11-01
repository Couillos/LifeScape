"""ISAAC stream cipher, as used by the 274 game protocol.

Ports the server implementation field by field. All arithmetic follows
the original 32-bit wrapping rules so both sides stay in sync.
"""

MASK = 0xFFFFFFFF


def _signed(word):
    word &= MASK
    return word - 0x100000000 if word & 0x80000000 else word


def _unsigned(word):
    return word & MASK


class Isaac:
    def __init__(self, seed=(0, 0, 0, 0)):
        self._count = 0
        self._results = [0] * 256
        self._memory = [0] * 256
        self._a = 0
        self._b = 0
        self._c = 0
        for index, word in enumerate(seed):
            if index >= 256:
                break
            self._results[index] = _signed(word)
        self._init()

    def next_int(self):
        if self._count == 0:
            self._cycle()
            self._count = 255
        else:
            self._count -= 1
        return self._results[self._count]

    def _init(self):
        a = b = c = d = e = f = g = h = 0x9E3779B9

        for _ in range(4):
            a, b, c, d, e, f, g, h = _mix(a, b, c, d, e, f, g, h)

        for i in range(0, 256, 8):
            a += self._results[i]
            b += self._results[i + 1]
            c += self._results[i + 2]
            d += self._results[i + 3]
            e += self._results[i + 4]
            f += self._results[i + 5]
            g += self._results[i + 6]
            h += self._results[i + 7]
            a, b, c, d, e, f, g, h = _mix(a, b, c, d, e, f, g, h)
            self._memory[i] = _signed(a)
            self._memory[i + 1] = _signed(b)
            self._memory[i + 2] = _signed(c)
            self._memory[i + 3] = _signed(d)
            self._memory[i + 4] = _signed(e)
            self._memory[i + 5] = _signed(f)
            self._memory[i + 6] = _signed(g)
            self._memory[i + 7] = _signed(h)

        for i in range(0, 256, 8):
            a += self._memory[i]
            b += self._memory[i + 1]
            c += self._memory[i + 2]
            d += self._memory[i + 3]
            e += self._memory[i + 4]
            f += self._memory[i + 5]
            g += self._memory[i + 6]
            h += self._memory[i + 7]
            a, b, c, d, e, f, g, h = _mix(a, b, c, d, e, f, g, h)
            self._memory[i] = _signed(a)
            self._memory[i + 1] = _signed(b)
            self._memory[i + 2] = _signed(c)
            self._memory[i + 3] = _signed(d)
            self._memory[i + 4] = _signed(e)
            self._memory[i + 5] = _signed(f)
            self._memory[i + 6] = _signed(g)
            self._memory[i + 7] = _signed(h)

        self._cycle()
        self._count = 256

    def _cycle(self):
        self._c += 1
        self._b += self._c
        for i in range(256):
            x = self._memory[i]
            if i & 3 == 0:
                self._a ^= _signed(self._a << 13)
            elif i & 3 == 1:
                self._a ^= _unsigned(self._a) >> 6
            elif i & 3 == 2:
                self._a ^= _signed(self._a << 2)
            else:
                self._a ^= _unsigned(self._a) >> 16
            self._a = _signed(self._a)
            self._a += self._memory[(i + 128) & 0xFF]
            slot = (_unsigned(x) >> 2) & 0xFF
            y = _signed(self._memory[slot] + self._a + self._b)
            self._memory[i] = y
            slot = (_unsigned(y) >> 10) & 0xFF
            self._b = _signed(self._memory[slot] + x)
            self._results[i] = self._b
        self._a = _signed(self._a)
        self._b = _signed(self._b)


def _mix(a, b, c, d, e, f, g, h):
    a ^= _signed(b << 11)
    d += a
    b += c
    b ^= _unsigned(c) >> 2
    e += b
    c += d
    c ^= _signed(d << 8)
    f += c
    d += e
    d ^= _unsigned(e) >> 16
    g += d
    e += f
    e ^= _signed(f << 10)
    h += e
    f += g
    f ^= _unsigned(g) >> 4
    a += f
    g += h
    g ^= _signed(h << 8)
    b += g
    h += a
    h ^= _unsigned(a) >> 9
    c += h
    a += b
    return a, b, c, d, e, f, g, h
