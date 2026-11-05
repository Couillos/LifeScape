"""RSA roundtrip against the real server key. Catches encrypt bugs."""

import base64
import os
import unittest

from lifescape.buffer import Writer
from lifescape.rsa import _read_tlv, load_private_numbers

HERE = os.path.dirname(os.path.abspath(__file__))
SERVER_KEY = os.path.join(HERE, "..", "..", "lostcity", "engine", "data", "config", "private.pem")


def _load_numbers():
    der = base64.b64decode("".join(line for line in open(SERVER_KEY).read().splitlines() if not line.startswith("-----")))
    _, outer, _ = _read_tlv(der, 0)
    pos = 0
    _, _, pos = _read_tlv(outer, pos)
    _, _, pos = _read_tlv(outer, pos)
    _, inner, _ = _read_tlv(outer, pos)
    _, seq, _ = _read_tlv(inner, 0)
    numbers = []
    cursor = 0
    while cursor < len(seq):
        _, value, cursor = _read_tlv(seq, cursor)
        numbers.append(int.from_bytes(value, "big"))
    return numbers


class RsaTest(unittest.TestCase):
    def test_key_loads(self):
        key = load_private_numbers(os.path.normpath(SERVER_KEY))
        self.assertEqual(key.size, 64)

    def test_encrypt_decrypt_roundtrip(self):
        key = load_private_numbers(os.path.normpath(SERVER_KEY))
        numbers = _load_numbers()
        plain = Writer()
        plain.u8(10)
        for seed in (11, 22, 33, 44):
            plain.i32(seed)
        plain.i32(99)
        plain.jstr("bite")
        plain.jstr("bite")
        cipher = key.encrypt(plain.bytes())
        recovered = pow(int.from_bytes(cipher, "big"), numbers[3], key.modulus)
        self.assertEqual(recovered, int.from_bytes(plain.bytes(), "big"))

    def test_cipher_stays_positive_for_jsbn(self):
        key = load_private_numbers(os.path.normpath(SERVER_KEY))
        for seed in range(20):
            plain = Writer()
            plain.u8(10)
            plain.i32(seed)
            plain.jstr("t%02d" % (seed + 1))
            plain.jstr("t%02d" % (seed + 1))
            cipher = key.encrypt(plain.bytes())
            self.assertLess(cipher[0], 0x80)


if __name__ == "__main__":
    unittest.main()
