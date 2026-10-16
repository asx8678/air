class OutOfStock(Exception):
    pass


def reserve(store, sku, qty):
    item = store.items[sku]
    if qty > item.stock:
        raise OutOfStock(sku)
    item.stock -= qty
    store.save()
    return item.stock


def price_order(store, lines, discount=0.0):
    total = 0.0
    for sku, qty in lines:
        price = store.items[sku].price * (1 - discount)
        total += price * qty
    return round(total * (1 - discount), 2)
