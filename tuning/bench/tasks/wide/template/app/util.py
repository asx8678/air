def setting(config, key, default):
    """A plugin setting: config[key] if present, else default."""
    return config.get(key, default)
