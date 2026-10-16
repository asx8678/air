import argparse
import sys

from . import store


def cmd_add(args):
    items = store.load()
    items.append({"title": args.title, "done": False})
    store.save(items)
    print(f"added {len(items)}")


def cmd_list(args):
    for n, item in enumerate(store.load(), 1):
        mark = "x" if item["done"] else " "
        print(f"{n}. [{mark}] {item['title']}")


def cmd_done(args):
    items = store.load()
    if not 1 <= args.n <= len(items):
        sys.exit(f"no item {args.n}")
    items[args.n - 1]["done"] = True
    store.save(items)
    print(f"done {args.n}")


def main(argv=None):
    parser = argparse.ArgumentParser(prog="todo")
    sub = parser.add_subparsers(dest="command", required=True)
    add = sub.add_parser("add")
    add.add_argument("title")
    add.set_defaults(func=cmd_add)
    lst = sub.add_parser("list")
    lst.set_defaults(func=cmd_list)
    done = sub.add_parser("done")
    done.add_argument("n", type=int)
    done.set_defaults(func=cmd_done)
    args = parser.parse_args(argv)
    args.func(args)


if __name__ == "__main__":
    main()
