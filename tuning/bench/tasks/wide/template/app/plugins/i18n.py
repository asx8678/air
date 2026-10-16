"""The i18n plugin."""

NAME = "i18n"


def handle(event, config):
    default_locale = config.get("default_locale", "en")
    fallback = config.get("fallback")
    return {"plugin": NAME, "event": event}


def describe(options):
    return options.get("verbose", False)
