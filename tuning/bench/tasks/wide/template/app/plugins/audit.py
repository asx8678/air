"""The audit plugin."""
from app.util import setting

NAME = "audit"


def handle(event, config):
    log_path = config.get("log_path", "/var/log/app/audit.log")
    rotate_mb = setting(config, "rotate_mb", 50)
    return {"plugin": NAME, "event": event}
