import unittest

from slots import free_slots


class SlotsTest(unittest.TestCase):
    def test_simple(self):
        self.assertEqual(free_slots([(10, 11), (13, 14)], 9, 17, 1), [(9, 10), (11, 13), (14, 17)])

    def test_unsorted(self):
        self.assertEqual(free_slots([(13, 14), (10, 11)], 9, 17, 1), [(9, 10), (11, 13), (14, 17)])

    def test_nested(self):
        self.assertEqual(free_slots([(10, 15), (11, 12)], 9, 17, 1), [(9, 10), (15, 17)])


if __name__ == "__main__":
    unittest.main()
