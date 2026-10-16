from dataclasses import dataclass, field


@dataclass
class Item:
    sku: str
    name: str
    price: float
    stock: int = 0
    tags: list = field(default_factory=list)
