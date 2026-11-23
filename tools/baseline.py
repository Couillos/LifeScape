#!/usr/bin/env python3
"""Reference measurements of the fleet (plan d'architecture, lot 0.1).

Read-only: the store is opened with the SQLite URI `?mode=ro`, the log is
only read. Everything is windowed by the UNIX clock (`created_clock`,
`clock`), never by multi-day text logs, except the report -> fold delay,
which only the daemon log carries (read on its tail).

    tools/baseline.py [--db run/lifescape.db] [--log run/orchestrator.log]
                      [--hours 1] [--end UNIX] [--json]

The window is [end - hours*3600, end) with `end` defaulting to the last
full hour. Run it for the hour before a deploy and for the hour after,
then compare the two outputs.
"""
import argparse
import json
import os
import re
import sqlite3
import sys
import time


def open_ro(path):
    return sqlite3.connect("file:%s?mode=ro" % os.path.abspath(path), uri=True, timeout=30)


def store_metrics(db, lo, hi):
    out = {}
    hours = (hi - lo) / 3600.0
    cur = db.cursor()
    rows = cur.execute(
        "select stop, count(*) from task_reports where created_clock>=? and created_clock<? group by stop", (lo, hi)
    ).fetchall()
    total = sum(count for _, count in rows)
    out["reports"] = total
    out["reports_per_hour"] = total / hours
    out["stop_shares"] = {stop: {"count": count, "share": (count / total if total else 0.0)} for stop, count in rows}
    for key in ("goal_reached", "blocked", "failed"):
        out["stop_shares"].setdefault(key, {"count": 0, "share": 0.0})
    work, pause = cur.execute(
        "select coalesce(sum(work_ticks),0), coalesce(sum(pause_ticks),0) from task_reports"
        " where created_clock>=? and created_clock<?", (lo, hi)
    ).fetchone()
    out["work_ticks_per_hour"] = work / hours
    out["pause_ticks_per_hour"] = pause / hours
    xp = cur.execute(
        "select coalesce(sum(y.xp),0) from task_yields y join task_reports r on r.report_id=y.report_id"
        " where r.created_clock>=? and r.created_clock<?", (lo, hi)
    ).fetchone()[0]
    out["xp_per_hour"] = xp / hours
    errors = cur.execute(
        "select kind, count(*) from error_events where clock>=? and clock<? group by kind order by 2 desc", (lo, hi)
    ).fetchall()
    out["error_events_per_hour"] = sum(count for _, count in errors) / hours
    out["error_events_by_kind_per_hour"] = {kind: count / hours for kind, count in errors}
    return out


CLOCK = re.compile(r"^\[(\d\d):(\d\d):(\d\d)\] (job|brain) (\S+): (.*)")


def tail_lines(path, max_bytes):
    size = os.path.getsize(path)
    with open(path, "rb") as handle:
        handle.seek(max(0, size - max_bytes))
        data = handle.read().decode("utf-8", errors="replace")
    lines = data.split("\n")
    return lines[1:] if size > max_bytes else lines


def fold_delay(path, hours, max_bytes=64 << 20):
    """Delay between a pushed task_report line and the matching `folded report`
    line of the same bot, over the last `hours` of the log tail (the day
    rollover is unwrapped; the window is counted back from the last line)."""
    stamps = []
    day = 0
    previous = None
    parsed = []
    for line in tail_lines(path, max_bytes):
        m = CLOCK.match(line)
        if not m:
            continue
        sec = int(m.group(1)) * 3600 + int(m.group(2)) * 60 + int(m.group(3))
        if previous is not None and sec < previous - 600:
            day += 1
        previous = sec
        parsed.append((day * 86400 + sec, m.group(4), m.group(5), m.group(6)))
    if not parsed:
        return None
    last = parsed[-1][0]
    push = {}
    delays = []
    for at, kind, who, rest in parsed:
        if at < last - hours * 3600:
            continue
        if kind == "job" and rest.startswith("task_report") and "(push)" in rest:
            push[who] = at
        elif kind == "brain" and rest.startswith("folded report") and who in push:
            delays.append(at - push.pop(who))
    if not delays:
        return {"n": 0}
    delays.sort()
    n = len(delays)
    return {
        "n": n,
        "mean_s": sum(delays) / n,
        "p50_s": delays[n // 2],
        "p90_s": delays[int(n * 0.9)],
        "p99_s": delays[int(n * 0.99)],
        "bot_hours_waiting": sum(delays) / 3600.0,
    }


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    root = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")
    ap.add_argument("--db", default=os.path.join(root, "run", "lifescape.db"))
    ap.add_argument("--log", default=os.path.join(root, "run", "orchestrator.log"))
    ap.add_argument("--hours", type=float, default=1.0)
    ap.add_argument("--end", type=int, default=None, help="window end, UNIX seconds (default: last full hour)")
    ap.add_argument("--json", action="store_true")
    args = ap.parse_args()
    now = int(time.time())
    end = args.end if args.end is not None else now - now % 3600
    lo = int(end - args.hours * 3600)
    db = open_ro(args.db)
    result = {"window": {"from": lo, "to": end, "hours": args.hours}}
    result.update(store_metrics(db, lo, end))
    try:
        result["report_to_fold_delay"] = fold_delay(args.log, args.hours) if os.path.exists(args.log) else None
    except OSError as error:
        result["report_to_fold_delay"] = {"error": str(error)}
    if args.json:
        json.dump(result, sys.stdout, indent=2, sort_keys=True)
        print()
        return
    stamp = lambda t: time.strftime("%Y-%m-%d %H:%M:%S", time.localtime(t))
    print("window %s .. %s (%.2f h)" % (stamp(lo), stamp(end), args.hours))
    print("reports: %d (%.0f/h)" % (result["reports"], result["reports_per_hour"]))
    for stop, row in sorted(result["stop_shares"].items(), key=lambda kv: -kv[1]["count"]):
        print("  %-18s %6d  %5.1f%%" % (stop, row["count"], 100 * row["share"]))
    print("work ticks/h: %.0f  pause ticks/h: %.0f" % (result["work_ticks_per_hour"], result["pause_ticks_per_hour"]))
    print("xp/h: %.0f" % result["xp_per_hour"])
    print("error_events/h: %.1f" % result["error_events_per_hour"])
    for kind, rate in result["error_events_by_kind_per_hour"].items():
        print("  %-18s %6.1f/h" % (kind, rate))
    delay = result["report_to_fold_delay"]
    print("report -> fold delay: %s" % (json.dumps(delay, sort_keys=True) if delay else "no data"))


if __name__ == "__main__":
    main()
