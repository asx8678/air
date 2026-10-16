import json
import os


def path():
    return os.environ.get("TODO_FILE", "todo.json")


def load():
    try:
        with open(path(), encoding="utf-8") as f:
            return json.load(f)
    except FileNotFoundError:
        return []


def save(items):
    with open(path(), "w", encoding="utf-8") as f:
        json.dump(items, f, indent=2)
