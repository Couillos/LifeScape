"""Raw RSA with the game public key, no padding, stdlib only.

The login block is encrypted as one big integer: ciphertext = plain^e mod n,
sent as a length byte followed by the big-endian bytes.
"""

import base64


class RsaKey:
    def __init__(self, modulus, exponent):
        self.modulus = modulus
        self.exponent = exponent
        self.size = (modulus.bit_length() + 7) // 8

    def encrypt(self, plain):
        raw = int.from_bytes(plain, "big")
        if raw >= self.modulus:
            raise ValueError("login block longer than key")
        cipher = pow(raw, self.exponent, self.modulus).to_bytes(self.size, "big")
        if cipher[0] & 0x80:
            cipher = b"\x00" + cipher
        return cipher


def load_public_key(pem_path):
    lines = open(pem_path).read().splitlines()
    body = "".join(line for line in lines if not line.startswith("-----"))
    der = base64.b64decode(body)
    modulus, exponent = _parse_spki(der)
    return RsaKey(modulus, exponent)


def load_private_numbers(pem_path):
    lines = open(pem_path).read().splitlines()
    body = "".join(line for line in lines if not line.startswith("-----"))
    der = base64.b64decode(body)
    _, outer, _ = _read_tlv(der, 0)
    pos = 0
    _, _, pos = _read_tlv(outer, pos)
    _, _, pos = _read_tlv(outer, pos)
    _, inner, _ = _read_tlv(outer, pos)
    _, inner_seq, _ = _read_tlv(inner, 0)
    int_pos = 0
    numbers = []
    while int_pos < len(inner_seq):
        _, value, int_pos = _read_tlv(inner_seq, int_pos)
        numbers.append(int.from_bytes(value, "big"))
    return RsaKey(numbers[1], numbers[2])


def _read_tlv(der, pos):
    tag = der[pos]
    length = der[pos + 1]
    pos += 2
    if length & 0x80:
        count = length & 0x7F
        length = int.from_bytes(der[pos:pos + count], "big")
        pos += count
    return tag, der[pos:pos + length], pos + length


def _parse_spki(der):
    if der[0] != 0x30:
        raise ValueError("not a DER sequence")
    _, outer, _ = _read_tlv(der, 0)
    pos = 0
    _, _, pos = _read_tlv(outer, pos)
    bit_tag, bit_string, _ = _read_tlv(outer, pos)
    if bit_tag != 0x03 or bit_string[0] != 0:
        raise ValueError("not a bit string key")
    _, inner, _ = _read_tlv(bit_string, 1)
    int_pos = 0
    _, modulus_bytes, int_pos = _read_tlv(inner, int_pos)
    _, exponent_bytes, _ = _read_tlv(inner, int_pos)
    return int.from_bytes(modulus_bytes, "big"), int.from_bytes(exponent_bytes, "big")
