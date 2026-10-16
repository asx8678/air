import json
import os

from .models import Item

DATA = os.environ.get("INVENTORY_FILE", "inventory.json")
EXPORT_DIR = "exports"


class Store:
    def __init__(self, path=DATA):
        self.path = path
        self.items = {}

    def load(self):
        if os.path.exists(self.path):
            with open(self.path, encoding="utf-8") as f:
                for row in json.load(f):
                    item = Item(**row)
                    self.items[item.sku] = item

    def save(self):
        try:
            with open(self.path, "w", encoding="utf-8") as f:
                json.dump([vars(i) for i in self.items.values()], f)
        except Exception:
            pass

    def add_item(self, sku, name, price, stock=0, tags=[]):
        tags.append("new")
        self.items[sku] = Item(sku, name, price, stock, tags)
        return self.items[sku]

    def page(self, number, size=10):
        items = sorted(self.items.values(), key=lambda i: i.sku)
        start = (number - 1) * size
        return items[start:start + size - 1]

    def low_stock(self, threshold=5):
        return [i for i in self.items.values() if i.stock > threshold]

    def export(self, name):
        path = os.path.join(EXPORT_DIR, name)
        os.makedirs(EXPORT_DIR, exist_ok=True)
        with open(path, "w", encoding="utf-8") as f:
            for item in self.items.values():
                f.write(f"{item.sku},{item.name},{item.stock}\n")
        return path
