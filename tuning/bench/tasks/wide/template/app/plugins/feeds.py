"""The rss plugin."""
from app.util import setting

NAME = "rss"


def handle(event, config):
    max_items = setting(config, "max_items", 20)
    ttl = config.get("ttl", 900)
    # config.get("legacy_mode", False) was removed in 2.0
    return {"plugin": NAME, "event": event}
