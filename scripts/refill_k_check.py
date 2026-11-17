"""What the feed pump really delivered during each refill of a run, against
the daemon's estimate, from a second balance under the receiving vessel.

    python scripts/refill_k_check.py <run_id> receiver.csv [--api http://127.0.0.1:8730]

For each refill, over the stretch the daemon does not weigh (refill start to
the refill's end plus REFILL_SETTLE_S), it prints the mass the receiver
gained (the truth) and the estimate with three ratios: k from before the
refill (what the daemon counts), k from after it, and their mean. Each k is
the feed balance's loss over the commanded mass, 10 min before the refill
start and 10 min after the settling. The receiver's own ratio over the same
windows is printed too: if it is not the feed balance's, the two balances
disagree (drips, evaporation, a line not yet full) and the comparison says
nothing until that is understood.
"""

import argparse
import bisect
import csv
import datetime as dt
import json
import urllib.request

REFILL_SETTLE_S = 180.0
WINDOW_S = 600.0


def ts(s):
    s = s.rstrip("Z")
    if "." in s:
        head, frac = s.split(".")
        s = f"{head}.{frac[:6]}"
    return dt.datetime.fromisoformat(s).replace(tzinfo=dt.timezone.utc).timestamp()


def get(api, path):
    return json.load(urllib.request.urlopen(api + path))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("run_id", type=int)
    ap.add_argument("receiver_csv")
    ap.add_argument("--api", default="http://127.0.0.1:8730")
    ap.add_argument("--density", type=float, default=1.0, help="g/ml of the feed")
    a = ap.parse_args()

    ticks = get(a.api, f"/api/runs/{a.run_id}/ticks")
    events = get(a.api, f"/api/runs/{a.run_id}/events?limit=1000")
    t = [ts(x["wall_time"]) for x in ticks]
    feed_w = [x["weight_g"] for x in ticks]
    # Commanded mass, cumulative, from the setpoint written each second.
    cmd = [0.0]
    for i in range(1, len(ticks)):
        cmd.append(cmd[-1] + ticks[i - 1]["target"] / 60.0 * a.density * (t[i] - t[i - 1]))

    with open(a.receiver_csv, encoding="utf-8") as f:
        rows = [(ts(r["wall_time"]), float(r["weight_g"])) for r in csv.DictReader(f)]
    rt = [r[0] for r in rows]

    def at(times, values, when):
        i = bisect.bisect_left(times, when)
        if i == 0 or i >= len(times):
            return None
        return values[i] if times[i] - when < when - times[i - 1] else values[i - 1]

    def cmd_between(t0, t1):
        c0, c1 = at(t, cmd, t0), at(t, cmd, t1)
        return c1 - c0 if None not in (c0, c1) else None

    def k_feed(t0, t1):
        w0, w1, c = at(t, feed_w, t0), at(t, feed_w, t1), cmd_between(t0, t1)
        return (w0 - w1) / c if None not in (w0, w1, c) else None

    def k_recv(t0, t1):
        w0, w1, c = at(rt, [r[1] for r in rows], t0), at(rt, [r[1] for r in rows], t1), cmd_between(t0, t1)
        return (w1 - w0) / c if None not in (w0, w1, c) else None

    starts = sorted(ts(e["wall_time"]) for e in events if e["kind"] == "refill_start")
    ends = sorted(ts(e["wall_time"]) for e in events if e["kind"] == "refill")
    if not starts:
        print("no refill in this run")
    for s in starts:
        e = next((x for x in ends if x > s), None)
        if e is None:
            print(f"refill at {s}: no end")
            continue
        blind_end = e + REFILL_SETTLE_S
        when = dt.datetime.fromtimestamp(s).strftime("%H:%M")
        kb, ka = k_feed(s - WINDOW_S, s), k_feed(blind_end, blind_end + WINDOW_S)
        rb, ra = k_recv(s - WINDOW_S, s), k_recv(blind_end, blind_end + WINDOW_S)
        truth = k_recv(s, blind_end)
        c = cmd_between(s, blind_end)
        if c is None:
            print(f"refill {when}: the run ended before its settling")
            continue
        print(f"refill {when}, {blind_end - s:.0f} s unweighed, {c:.1f} g commanded")
        if None in (kb, ka, rb, ra, truth):
            print("  a window falls outside the ticks or the receiver log")
            continue
        print(f"  k feed balance: before {kb:.4f}, after {ka:.4f}")
        print(f"  k receiver:     before {rb:.4f}, after {ra:.4f}, during {truth:.4f}")
        real = truth * c
        for name, k in (("before (daemon)", kb), ("after", ka), ("mean", (kb + ka) / 2)):
            print(f"  {name:16} estimate {k * c:7.2f} g, real {real:7.2f} g, off {k * c - real:+.2f} g")


if __name__ == "__main__":
    main()
