"""The export plugin."""
from app.util import setting

NAME = "export"


def handle(event, config):
    format = config.get("format", "csv")
    batch_size = setting(config, "batch_size", 500)
    timeout = config.get("timeout", 30)
    return {"plugin": NAME, "event": event}
