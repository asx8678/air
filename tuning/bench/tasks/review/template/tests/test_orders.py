import os
import tempfile
import unittest

from inventory.orders import OutOfStock, price_order, reserve
from inventory.store import Store


class OrdersTest(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.store = Store(os.path.join(self.dir.name, "inv.json"))
        self.store.add_item("A1", "apple", 0.5, stock=10)

    def tearDown(self):
        self.dir.cleanup()

    def test_reserve(self):
        self.assertEqual(reserve(self.store, "A1", 3), 7)

    def test_out_of_stock(self):
        with self.assertRaises(OutOfStock):
            reserve(self.store, "A1", 11)

    def test_price_without_discount(self):
        self.assertEqual(price_order(self.store, [("A1", 4)]), 2.0)


if __name__ == "__main__":
    unittest.main()
