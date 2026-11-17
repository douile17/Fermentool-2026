#!/usr/bin/env python3
"""Compare the daemon's `timing` log lines over two periods.

    python scripts/timing_report.py LOG [LOG ...] --a FROM TO --b FROM TO

FROM and TO are ISO local times (2026-11-03T09:00). The log timestamps are UTC.
Per metric and range, prints the median over the range's minutes of p50, p99
and max ("p99" is the median minute's p99), the worst minute's max, the 99th
percentile (nearest rank) of the per-minute max, and the median count per
minute. The medians hide rare stalls; the worst-minute columns show them.

Acceptance, per metric: PASS when B.p99 <= A.p99 + max(10 % of A.p99, 20 ms).
Exit 1 if any metric fails, or if range A or B matched no lines.
"""
import argparse
import math
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
                    rows.append((parse_ts(m["ts"]), m["metric"], float(m["p50"]), float(m["p99"]), float(m["max"]), int(m["count"])))
    return rows


def nearest_rank(values, p):
    v = sorted(values)
    return v[min(max(math.ceil(p * len(v)), 1), len(v)) - 1]


def summarize(rows, start, end):
    """metric -> dict: minutes, p50, p99, max (medians), worst, max99, count (median)."""
    by = {}
    for ts, metric, p50, p99, mx, count in rows:
        if start <= ts < end:
            by.setdefault(metric, []).append((p50, p99, mx, count))
    out = {}
    for m, v in by.items():
        maxes = [x[2] for x in v]
        out[m] = {
            "minutes": len(v),
            "p50": statistics.median(x[0] for x in v),
            "p99": statistics.median(x[1] for x in v),
            "max": statistics.median(maxes),
            "worst": max(maxes),
            "max99": nearest_rank(maxes, 0.99),
            "count": statistics.median(x[3] for x in v),
        }
    return out


def verdict(a99, b99):
    """(B-A in ms, passes): B.p99 <= A.p99 + max(10 % of A.p99, 20 ms)."""
    return b99 - a99, b99 <= a99 + max(0.10 * a99, 20.0)


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
    bad = False
    for name, d in (("A", a), ("B", b)):
        if not d:
            print(f"range {name} matched no timing lines", file=sys.stderr)
            bad = True
    if bad:
        return 1

    print(f"{'metric':<14}{'range':<6}{'minutes':>8}{'count':>7}{'p50':>9}{'p99':>9}{'max':>9}{'worst':>9}{'max p99':>9}"
          f"{'B-A ms':>9}  verdict")
    failed = False
    for m in ORDER + sorted((set(a) | set(b)) - set(ORDER)):
        if m not in a and m not in b:
            continue
        diff = ok = None
        if m in a and m in b:
            diff, ok = verdict(a[m]["p99"], b[m]["p99"])
            failed |= not ok
        for name, d in (("A", a), ("B", b)):
            if m not in d:
                print(f"{m:<14}{name:<6}{'none':>8}")
                continue
            r = d[m]
            tail = ""
            if name == "B" and diff is not None:
                tail = f"{diff:>9.1f}  {'PASS' if ok else 'FAIL'}"
            print(f"{m:<14}{name:<6}{r['minutes']:>8}{r['count']:>7.0f}{r['p50']:>9.1f}{r['p99']:>9.1f}"
                  f"{r['max']:>9.1f}{r['worst']:>9.1f}{r['max99']:>9.1f}{tail}")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
