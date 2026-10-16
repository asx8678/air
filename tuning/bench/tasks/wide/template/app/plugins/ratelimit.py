"""The ratelimit plugin."""

NAME = "ratelimit"


def handle(event, config):
    limit = config["limit"]
    window = config.get("window", 60)
    redis_url = config.get("redis_url")
    return {"plugin": NAME, "event": event}


def describe(options):
    return options.get("verbose", False)
