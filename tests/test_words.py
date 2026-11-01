import unittest

from lifescape.words import pack, sentence_case, unpack

VECTORS = {
    "hello lifescape": "61bb40b5d418d23d61",
    "Hi! How are you? 123": "65e9064d103910d34cea0dfe0e10",
    "a": "30",
    "ab": "3d70",
    "abc": "3d7d20",
    "testing 1 2 3 ...": "218257d50df0e00e10ebebeb",
    "zzzz": "dddddddd",
    "  spaces  ": "008d63d21800",
}

UNPACKED = {
    "hello lifescape": "Hello lifescape",
    "Hi! How are you? 123": "Hi! How are you? 123 ",
    "a": "A ",
    "ab": "Ab ",
    "abc": "Abc ",
    "testing 1 2 3 ...": "Testing 1 2 3 ...",
    "zzzz": "Zzzz",
    "  spaces  ": "  Spaces  ",
}


class WordsTest(unittest.TestCase):
    def test_pack_matches_server(self):
        for text, expected in VECTORS.items():
            self.assertEqual(pack(text).hex(), expected)

    def test_unpack_roundtrip(self):
        for text, expected in UNPACKED.items():
            self.assertEqual(unpack(pack(text)), expected)

    def test_sentence_case_matches_server(self):
        self.assertEqual(sentence_case("hello. how are you! fine? ok"), "Hello. How are you! Fine? ok")


if __name__ == "__main__":
    unittest.main()
