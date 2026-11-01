"""4-bit chat packing, mirroring the server WordPack exactly."""


LOOKUP = [
    " ",
    "e", "t", "a", "o", "i", "h", "n", "s", "r", "d", "l", "u", "m",
    "w", "c", "y", "f", "g", "p", "b", "v", "k", "x", "j", "q", "z",
    "0", "1", "2", "3", "4", "5", "6", "7", "8", "9",
    " ", "!", "?", ".", ",", ":", ";", "(", ")", "-",
    "&", "*", "\\", "'", "@", "#", "+", "=", "£", "$", "%", '"', "[", "]",
]


def pack(text):
    text = text.lower()[:80]
    out = bytearray()
    carry = -1
    for char in text:
        index = 0
        for position, candidate in enumerate(LOOKUP):
            if char == candidate:
                index = position
                break
        if index > 12:
            index += 195
        if carry == -1:
            if index < 13:
                carry = index
            else:
                out.append(index)
        elif index < 13:
            out.append((carry << 4) + index)
            carry = -1
        else:
            out.append((carry << 4) + (index >> 4))
            carry = index & 0xF
    if carry != -1:
        out.append(carry << 4)
    return bytes(out)


def unpack(packed):
    chars = []
    carry = -1
    for byte in packed:
        for nibble in ((byte >> 4) & 0xF, byte & 0xF):
            if carry != -1:
                chars.append(LOOKUP[(carry << 4) + nibble - 195])
                carry = -1
            elif nibble < 13:
                chars.append(LOOKUP[nibble])
            else:
                carry = nibble
    return sentence_case("".join(chars))


def sentence_case(text):
    chars = list(text.lower())
    punctuation = True
    for index, char in enumerate(chars):
        if punctuation and "a" <= char <= "z":
            chars[index] = char.upper()
            punctuation = False
        if char == "." or char == "!":
            punctuation = True
    return "".join(chars)
