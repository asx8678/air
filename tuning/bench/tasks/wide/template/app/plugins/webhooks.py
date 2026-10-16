"""The webhooks plugin."""
from app.util import setting

NAME = "webhooks"


def handle(event, config):
    urls = config["urls"]
    timeout = config.get("timeout", 5)
    retries = setting(config, "retries", 3)
    return {"plugin": NAME, "event": event}
