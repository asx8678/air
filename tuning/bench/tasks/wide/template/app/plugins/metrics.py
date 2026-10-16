"""The metrics plugin."""
from app.util import setting

NAME = "metrics"


def handle(event, config):
    interval = setting(config, "interval", 60)
    prefix = config.get("prefix", "app.")
    return {"plugin": NAME, "event": event}
