"""The geoip plugin."""

NAME = "geoip"


def handle(event, config):
    database = config["database"]
    cache_size = config.get("cache_size", 1024)
    return {"plugin": NAME, "event": event}
