#!/usr/bin/env python3
"""Compare the daemon's `timing` log lines over two periods.

    python scripts/timing_report.py LOG [LOG ...] --a FROM TO --b FROM TO

FROM and TO are ISO local times (2026-11-03T09:00). The log timestamps are UTC.
Per metric, prints the median over each range's minutes of p50, p99 and max,
and the B/A ratio of p99.
"""
import argparse
import re
import statistics
import sys
from datetime import datetime, timezone

LINE = re.compile(
    r'^(?P<ts>\S+Z)\s+INFO timing: metric="?(?P<metric>\w+)"? '
    r'count=(?P<count>\d+) p50_ms=(?P<p50>[\d.]+) p99_ms=(?P<p99>[\d.]+) max_ms=(?P<max>[\d.]+)'
)
ORDER = ["setpoint_late", "tick", "scale_age", "scale_poll"]


def local_to_utc(text):
    """An ISO local time (no zone) as an aware UTC datetime, in the machine's zone."""
    return datetime.fromisoformat(text).astimezone(timezone.utc)


def parse_ts(text):
    # Python < 3.11 wants at most 6 fractional digits.
    text = re.sub(r"(\.\d{6})\d+", r"\1", text)
    return datetime.fromisoformat(text.replace("Z", "+00:00"))


def read(paths):
    rows = []
    for path in paths:
        with open(path, encoding="utf-8", errors="replace") as f:
            for line in f:
                m = LINE.match(line)
                if m:
                    rows.append((parse_ts(m["ts"]), m["metric"], float(m["p50"]), float(m["p99"]), float(m["max"])))
    return rows


def summarize(rows, start, end):
    """metric -> (minutes, median p50, median p99, median max) within [start, end)."""
    by = {}
    for ts, metric, p50, p99, mx in rows:
        if start <= ts < end:
            by.setdefault(metric, []).append((p50, p99, mx))
    return {
        m: (len(v), *(statistics.median(x[i] for x in v) for i in range(3))) for m, v in by.items()
    }


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("logs", nargs="+", help="daily log files (a run may cross midnight)")
    ap.add_argument("--a", nargs=2, required=True, metavar=("FROM", "TO"), help="range A, ISO local times")
    ap.add_argument("--b", nargs=2, required=True, metavar=("FROM", "TO"), help="range B, ISO local times")
    args = ap.parse_args()

    rows = read(args.logs)
    if not rows:
        print("no timing lines in the log files", file=sys.stderr)
        return 1
    a = summarize(rows, *map(local_to_utc, args.a))
    b = summarize(rows, *map(local_to_utc, args.b))

    print(f"{'metric':<14}{'range':<6}{'minutes':>8}{'p50':>9}{'p99':>9}{'max':>9}   p99 B/A")
    for m in ORDER + sorted((set(a) | set(b)) - set(ORDER)):
        if m not in a and m not in b:
            continue
        ratio = ""
        if m in a and m in b and a[m][2] > 0:
            ratio = f"{b[m][2] / a[m][2]:.2f}"
        for name, d in (("A", a), ("B", b)):
            if m in d:
                n, p50, p99, mx = d[m]
                print(f"{m:<14}{name:<6}{n:>8}{p50:>9.1f}{p99:>9.1f}{mx:>9.1f}   {ratio if name == 'B' else ''}")
            else:
                print(f"{m:<14}{name:<6}{'none':>8}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
