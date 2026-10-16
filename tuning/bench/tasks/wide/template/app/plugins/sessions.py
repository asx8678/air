"""The sessions plugin."""
from app.util import setting

NAME = "sessions"


def handle(event, config):
    cookie = config.get("cookie", "sid")
    # config.get("legacy_mode", False) was removed in 2.0
    return {"plugin": NAME, "event": event}


def configure(config):
    ttl = setting(config, "ttl", 3600)
    secure = config.get("secure", True)
    return dict(locals())
