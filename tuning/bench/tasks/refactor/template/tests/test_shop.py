import unittest

from shop.cart import Cart
from shop.invoice import render
from shop.pricing import calc_total


class ShopTest(unittest.TestCase):
    def test_calc_total(self):
        self.assertEqual(calc_total([(2.5, 2), (1.0, 3)], 0.1), 8.8)

    def test_cart(self):
        cart = Cart()
        cart.add(10.0, 2)
        self.assertEqual(cart.total(), 20.0)

    def test_render(self):
        self.assertEqual(render([(1.5, 2)]), "2 x 1.50\ntotal 3.00")


if __name__ == "__main__":
    unittest.main()
