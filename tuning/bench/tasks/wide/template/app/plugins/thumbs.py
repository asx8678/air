"""The thumbnails plugin."""
from app.util import setting

NAME = "thumbnails"


def handle(event, config):
    sizes = config.get("sizes", (64, 128, 256))
    quality = setting(config, "quality", 85)
    return {"plugin": NAME, "event": event}
