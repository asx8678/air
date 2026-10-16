"""The backup plugin."""
from app.util import setting

NAME = "backup"


def handle(event, config):
    dest = config.get("dest", "s3://backups")
    keep = setting(config, "keep", 7)
    schedule = config["schedule"]
    return {"plugin": NAME, "event": event}
