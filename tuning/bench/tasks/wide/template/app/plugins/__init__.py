import importlib
import pkgutil


def load(options):
    """Every plugin module in this package, keyed by its NAME."""
    found = {}
    for info in pkgutil.iter_modules(__path__):
        module = importlib.import_module(f"{__name__}.{info.name}")
        found[module.NAME] = module
    return {name: m for name, m in found.items() if name in options.get("enabled", found)}
