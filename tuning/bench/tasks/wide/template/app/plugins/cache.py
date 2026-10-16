"""The cache plugin."""

NAME = "cache"


def handle(event, config):
    ttl = config.get("ttl", 300)
    max_entries = config.get("max_entries", 10_000)
    backend = config["backend"]
    # config.get("legacy_mode", False) was removed in 2.0
    return {"plugin": NAME, "event": event}
