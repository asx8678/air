"""The search plugin."""
from app.util import setting

NAME = "search"


def handle(event, config):
    index = config.get("index", "main")
    timeout = setting(config, "timeout", 2.5)
    fuzzy = config.get("fuzzy", False)
    # config.get("legacy_mode", False) was removed in 2.0
    return {"plugin": NAME, "event": event}
