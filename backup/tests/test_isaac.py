"""ISAAC vectors generated from the server implementation."""

import unittest

from lifescape.isaac import Isaac

VECTORS = [
    (
        (0, 0, 0, 0),
        [405143795, 806046349, 807101986, -1333080799, 695195257, -1722677527, -1275090763, 264870948],
    ),
    (
        (1, 2, 3, 4),
        [-621246914, 1957022519, -1345000077, -2021884860, -1882702437, 1616913581, -8779862, 1337573575],
    ),
    (
        (123456789, -1, 2147483647, -2147483648),
        [-131764465, 835306136, 14534765, 1126653441, -1761642342, 728764291, 1203577890, -1438083942],
    ),
    (
        (50, 51, 52, 53),
        [-803115556, -228876953, 198545339, -2100090616, -158391970, 355898769, -120645264, -2074236694],
    ),
]


class IsaacTest(unittest.TestCase):
    def test_vectors_match_server(self):
        for seed, expected in VECTORS:
            stream = Isaac(seed)
            self.assertEqual([stream.next_int() for _ in range(8)], expected)

    def test_longer_seeds_use_first_256_words(self):
        stream = Isaac((7,) * 300)
        self.assertIsInstance(stream.next_int(), int)


if __name__ == "__main__":
    unittest.main()
