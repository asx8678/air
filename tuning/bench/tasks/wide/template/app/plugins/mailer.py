"""The smtp plugin."""
from app.util import setting

NAME = "smtp"


def handle(event, config):
    host = config["host"]
    port = config.get("port", 25)
    timeout = setting(config, "timeout", 10.0)
    username = config.get("username")
    return {"plugin": NAME, "event": event}


def describe(options):
    return options.get("verbose", False)
