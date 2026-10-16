import unittest

from calc import evaluate


class CalcTest(unittest.TestCase):
    def test_precedence(self):
        self.assertEqual(evaluate("1 + 2 * 3"), 7)

    def test_parentheses(self):
        self.assertEqual(evaluate("(1+2)*3"), 9)

    def test_left_associative(self):
        self.assertEqual(evaluate("8/2/2"), 2)
        self.assertEqual(evaluate("10-4-3"), 3)

    def test_minus_without_spaces(self):
        self.assertEqual(evaluate("2-3"), -1)
        self.assertEqual(evaluate("(4)-1"), 3)

    def test_negative_operand(self):
        self.assertEqual(evaluate("2 - -3"), 5)


if __name__ == "__main__":
    unittest.main()
