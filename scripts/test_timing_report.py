"""Pins the log format and the acceptance rule of timing_report.py.

Run from the repo root: python -m unittest discover -s scripts -p "test_*.py"
"""
import os
import sys
import unittest
from datetime import datetime, timedelta, timezone

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import timing_report as tr  # noqa: E402

T0 = datetime(2026, 11, 3, 6, 0, tzinfo=timezone.utc)


def line(minute, metric, p50, p99, mx, count=360):
    ts = (T0 + timedelta(minutes=minute)).strftime("%Y-%m-%dT%H:%M:%S.878607Z")
    return f'{ts}  INFO timing: metric="{metric}" count={count} p50_ms={p50} p99_ms={p99} max_ms={mx}\n'


def rows_of(lines, tmp):
    path = os.path.join(tmp, "t.log")
    with open(path, "w", encoding="utf-8") as f:
        f.write("2026-11-03T07:53:59.878607Z  INFO fermentool_core::control: serial link reopened\n")
        f.writelines(lines)
    return tr.read([path])


class TimingReport(unittest.TestCase):
    def setUp(self):
        import tempfile
        self._d = tempfile.TemporaryDirectory()
        self.tmp = self._d.name

    def tearDown(self):
        self._d.cleanup()

    def test_parses_the_exact_tracing_line(self):
        raw = "2026-11-03T07:53:59.878607Z  INFO timing: metric=\"setpoint_late\" count=360 p50_ms=3.1 p99_ms=15.6 max_ms=16.0\n"
        rows = rows_of([raw], self.tmp)
        self.assertEqual(len(rows), 1)
        ts, metric, p50, p99, mx, count = rows[0]
        self.assertEqual((metric, p50, p99, mx, count), ("setpoint_late", 3.1, 15.6, 16.0, 360))
        self.assertEqual(ts, datetime(2026, 11, 3, 7, 53, 59, 878607, tzinfo=timezone.utc))

    def test_pass_under_the_20ms_floor(self):
        # A.p99 = 2 ms: 10 % is 0.2 ms, the 20 ms floor rules: 21.9 passes, 22.1 fails.
        self.assertTrue(tr.verdict(2.0, 21.9)[1])
        self.assertFalse(tr.verdict(2.0, 22.1)[1])

    def test_fail_over_ten_percent_on_a_large_p99(self):
        diff, ok = tr.verdict(400.0, 441.0)
        self.assertFalse(ok)
        self.assertAlmostEqual(diff, 41.0)
        self.assertTrue(tr.verdict(400.0, 440.0)[1])

    def test_worst_minute_survives_many_quiet_minutes(self):
        lines = [line(i, "tick", 2.0, 4.0, 9.0) for i in range(100)]
        lines.append(line(100, "tick", 2.0, 4.0, 900.0))
        s = tr.summarize(rows_of(lines, self.tmp), T0, T0 + timedelta(minutes=200))["tick"]
        self.assertEqual(s["max"], 9.0)  # the median hides it
        self.assertEqual(s["worst"], 900.0)
        self.assertEqual(s["max99"], 9.0)  # 1 minute in 101 is under the 99th rank; "worst" is the column for it
        self.assertEqual(s["minutes"], 101)
        self.assertEqual(s["count"], 360)


if __name__ == "__main__":
    unittest.main()
