#!/usr/bin/env python3
"""Harness mechanics per variant, pooled over tasks: what the main report doesn't show.

  mechanics.py LABEL [LABEL ...] [--model MODEL] [--results FILE]
"""
import argparse
import json
import os
import statistics

BENCH = os.path.dirname(os.path.abspath(__file__))


def mean(xs):
    return statistics.mean(xs) if xs else 0.0


def session_facts(r):
    """From the run's session log: cache efficiency (cached / the previous request's prompt, which is all that can be
    cached) and the upstream the gateway routed the session to."""
    path = None
    try:
        for line in open(os.path.join(r["rundir"], "out.log"), errors="replace"):
            if line.startswith("[session "):
                path = line.split()[1].rstrip("]")
        es = [json.loads(l) for l in open(path) if l.startswith("{")]
    except (OSError, TypeError, ValueError):
        return None
    reqs = [e["meta"] for e in es if e.get("role") == "assistant" and (e.get("meta") or {}).get("usage")]
    cached = possible = 0
    for prev, cur in zip(reqs, reqs[1:]):
        p = prev["usage"].get("prompt_tokens") or 0
        c = (cur["usage"].get("prompt_tokens_details") or {}).get("cached_tokens") or 0
        cached, possible = cached + min(c, p), possible + p
    ups = {(m.get("response") or {}).get("headers", {}).get("x-opencode-upstream-model-id") for m in reqs}
    first = reqs[0]["usage"].get("prompt_tokens") if reqs else 0
    return cached, possible, ",".join(sorted(u for u in ups if u)) or "?", first


def main():
    p = argparse.ArgumentParser()
    p.add_argument("labels", nargs="+")
    p.add_argument("--model", default="deepseek-v4.1-flash")
    p.add_argument("--results", default=os.path.join(BENCH, "results.jsonl"))
    args = p.parse_args()
    rows = [json.loads(l) for l in open(args.results) if l.strip()]
    rows = [r for r in rows if r["model"] == args.model]
    head = ("| variant | runs | cache efficiency | upstreams | first prompt | out: reasoning | accept/run | plan/run | done/run | "
            "show/run | sh/run | edit+write/run | check()/run | failed edits | rejected runs | tool errors | answer chars | gate nudges | verdicts |")
    print(head)
    print("|" + "|".join(["---"] * (head.count("|") - 1)) + "|")
    for label in args.labels:
        rs = [r for r in rows if r["variant"] == label]
        if not rs:
            print(f"| {label} | 0 |")
            continue
        later = [c for r in rs for c in r["cache_rates"][1:]]
        facts = [f for f in map(session_facts, rs) if f]
        eff = sum(f[0] for f in facts) / max(1, sum(f[1] for f in facts))
        ups = {}
        for f in facts:
            ups[f[2]] = ups.get(f[2], 0) + 1
        tools = lambda k: mean([r["tools"].get(k, 0) for r in rs])  # noqa: E731
        sizes = []
        for r in rs:
            try:
                sizes.append(os.path.getsize(os.path.join(r["rundir"], "answer.md")))
            except OSError:
                pass
        verdicts = {}
        for r in rs:
            verdicts[r.get("verdict")] = verdicts.get(r.get("verdict"), 0) + 1
        print(f"| {label} | {len(rs)} | {eff:.1%} | " + " ".join(f"{k.split('/')[0]}:{v}" for k, v in sorted(ups.items())) + " | "
              f"{mean([f[3] for f in facts]):.0f} | "
              f"{sum(r['reasoning'] for r in rs) / max(1, sum(r['out'] for r in rs)):.0%} | {tools('accept'):.2f} | {tools('plan'):.2f} | "
              f"{tools('done'):.2f} | {tools('show'):.1f} | {tools('sh'):.1f} | {tools('edit') + tools('write'):.1f} | {tools('check'):.2f} | "
              f"{sum(r['edit_failures'] for r in rs)} | {sum(r['rejected'] for r in rs)} | {sum(r['tool_errors'] for r in rs)} | "
              f"{mean(sizes):.0f} | {sum(r['events'].get('nudge', 0) for r in rs)} | "
              + ", ".join(f"{k}:{v}" for k, v in sorted(verdicts.items(), key=str)) + " |")


if __name__ == "__main__":
    main()
