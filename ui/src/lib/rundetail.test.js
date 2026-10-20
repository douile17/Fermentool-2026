import { describe, expect, it } from 'vitest';
import {
  alarmCount, clockTicks, curveLine, gapExtremes, gapScale, gapSeries, grouped, journalLines,
  refillRows, regulationFigures, verdict, whenLine,
} from './rundetail.js';

// Local times, whatever the machine's zone: the UI shows the PC's clock.
const at = (d, h, m, s = 0) => new Date(2026, 9, d, h, m, s).toISOString();

// Run 194 as journalled: 3.5 ml/min step, three automatic refills.
const run194 = {
  status: 'completed',
  control_var: 'ml_min',
  duration_s: 56664,
  curve: { mode: 'endpoints', start: 3.5, end: 0, params: { kind: 'step', segments: [{ at_seconds: 0, value: 3.5 }, { at_seconds: 56664, value: 0 }] } },
};
let nextId = 100;
const ev = (kind, wall_time, detail = null, level = 'info') => ({ id: nextId--, kind, wall_time, detail, level });
const events194 = [
  ev('stop', at(20, 9, 0, 12)),
  ev('curve_done', at(20, 8, 59, 59), 'holding at 0.000, regulation and journal continue until Stop'),
  ev('refill', at(20, 8, 54, 48), 'bottle refilled: 3656.7 g -> 4655.3 g (+998.6 g)'),
  ev('refill_stop', at(20, 8, 54, 18), 'filled 1000 g in 1617 s at 55 rpm (+37 g/min net of the feed)'),
  ev('refill_start', at(20, 8, 27, 21), 'under the refill level: 3657 g on the balance, filling to 4657 g at 55 rpm'),
  ev('refill', at(20, 3, 41, 53), 'bottle refilled: 3656.7 g -> 4655.3 g (+998.6 g)'),
  ev('refill_stop', at(20, 3, 41, 22), 'filled 1000 g in 1607 s at 55 rpm (+37 g/min net of the feed)'),
  ev('refill_start', at(20, 3, 14, 35), 'under the refill level: 3657 g on the balance, filling to 4657 g at 55 rpm'),
  ev('refill', at(19, 22, 29, 14), 'bottle refilled: 3656.8 g -> 4655.1 g (+998.3 g)'),
  ev('refill_stop', at(19, 22, 28, 43), 'filled 999 g in 1642 s at 55 rpm (+37 g/min net of the feed)'),
  ev('refill_start', at(19, 22, 1, 21), 'under the refill level: 3657 g on the balance, filling to 4657 g at 55 rpm'),
  ev('trim_ratio', at(19, 17, 17, 42), 'pump ratio measured: 1.040, regulating on it'),
  ev('trim_start', at(19, 17, 17, 42), 'correction starts: x0.980 (+0.25 g vs requested)'),
  ev('start', at(19, 17, 15, 35), 'first setpoint 3.499'),
];

describe('journalLines', () => {
  const lines = journalLines(events194, run194);

  it('makes one line per refill, and one for the correction and its first ratio', () => {
    expect(lines.map((l) => l.title)).toEqual([
      'Started', 'Balance correction on', 'Refill 1', 'Refill 2', 'Refill 3', 'Curve done', 'Stopped',
    ]);
  });

  it('says a refill in words: how long, how fast, the weights', () => {
    expect(lines[2]).toMatchObject({ time: '22:01', tone: 'refill', detail: '28 min at 55 rpm, 3 656.8 → 4 655.1 g' });
  });

  it('names the day on the first line and when it changes, not otherwise', () => {
    expect(lines.map((l) => l.day)).toEqual(['Mon', '', '', 'Tue', '', '', '']);
  });

  it('puts units on the values', () => {
    expect(lines[0].detail).toBe('at 3.499 ml/min');
    expect(lines[1].detail).toBe('pump ratio 1.040');
    expect(lines[5].detail).toBe('holding 0.000 ml/min until Stop');
    expect(lines[6].detail).toBe('completed');
  });

  it('marks an alarm, and words a kind it does not know', () => {
    const l = journalLines([ev('alarm_feed_stopped', at(19, 18, 0), '1.2 g commanded in 180 s'), ev('some_new_kind', at(19, 18, 1), 'x')], run194);
    expect(l[0]).toMatchObject({ title: 'Alarm: feed stopped', tone: 'bad' });
    expect(l[1].title).toBe('Some new kind');
  });

  it('counts a refill by hand, with no start event', () => {
    const l = journalLines([ev('refill', at(19, 18, 0), 'bottle refilled: 100.0 g -> 600.0 g (+500.0 g)')], run194);
    expect(l[0]).toMatchObject({ title: 'Refill 1', detail: 'by hand, 100.0 → 600.0 g' });
  });
});

describe('alarmCount', () => {
  it('counts alarms raised, not their clearing or acknowledgement', () => {
    expect(alarmCount(events194)).toBe(0);
    expect(alarmCount([ev('alarm_saturated', at(19, 1, 0)), ev('alarm_cleared', at(19, 1, 5)), ev('alarm_ack', at(19, 1, 6))])).toBe(1);
  });
});

describe('regulationFigures', () => {
  it('reads the starting correction and the first ratio', () => {
    expect(regulationFigures(events194)).toEqual({ correction: '0.980', ratio: '1.040' });
    expect(regulationFigures([])).toEqual({ correction: null, ratio: null });
  });
});

describe('verdict', () => {
  const report = (pts, estimated = []) => ({ points: pts, estimated });

  it('is on target within 2 % of requested, with the worst gap after start-up', () => {
    const v = verdict(report([[100, 5, 6], [1000, 100, 100.3], [2000, 200, 200.2]]));
    expect(v.onTarget).toBe(true);
    expect(v.pct).toBeCloseTo(100.1, 3);
    // the 20 % at 100 s is start-up, left out
    expect(v.worstPct).toBeCloseTo(0.3, 6);
  });

  it('is off target past 2 %', () => {
    expect(verdict(report([[2000, 200, 195]])).onTarget).toBe(false);
  });

  it('leaves refills out of the worst gap', () => {
    const v = verdict(report([[1000, 100, 110], [2000, 200, 200]], [[900, 1100]]));
    expect(v.worstPct).toBe(0);
  });

  it('flags a balance weighing the wrong way', () => {
    expect(verdict(report([[2000, 200, -190]])).wrongSide).toBe(true);
  });

  it('has nothing to say without a report', () => {
    expect(verdict(null)).toBeNull();
  });
});

describe('gapSeries', () => {
  it('averages the weighed gap over 5 min, so 0.1 g steps do not zigzag', () => {
    const pts = [];
    for (let i = 0; i <= 40; i++) pts.push([30 * i, 30 * i, 30 * i + (i % 2 ? 0.1 : 0)]);
    // away from the ends, every point reads about 0.05, never 0 nor 0.1
    for (const [, g] of gapSeries(pts, []).slice(5, -5)) expect(g).toBeCloseTo(0.05, 1);
  });

  it('draws an estimated stretch straight between the weighed points around it, flagged', () => {
    // a zigzagging estimate inside the refill, 0 before it, 0.4 after
    const pts = [[0, 0, 0], [30, 1, 1.9], [60, 2, 1.8], [90, 3, 3.9], [120, 4, 4.4]];
    const s = gapSeries(pts, [[20, 100]], 1);
    expect(s.map((p) => p[2])).toEqual([false, true, true, true, false]);
    expect(s[1][1]).toBeCloseTo(0.1, 9);
    expect(s[2][1]).toBeCloseTo(0.2, 9);
    expect(s[3][1]).toBeCloseTo(0.3, 9);
  });
});

describe('gapScale', () => {
  it('is symmetric on a round step, at least ±0.5 mL', () => {
    expect(gapScale([[0, 0.1], [1, -0.05]])).toEqual({ hi: 1, ticks: [-1, -0.5, 0, 0.5, 1] });
    const s = gapScale([[0, 3.2], [1, -1]]);
    expect(s.ticks[0]).toBe(-s.hi);
    expect(s.ticks.at(-1)).toBe(s.hi);
    expect(s.hi).toBeGreaterThanOrEqual(3.2);
  });
});

describe('gapExtremes', () => {
  it('finds the largest weighed gap ahead and behind', () => {
    expect(gapExtremes([[0, 0, 0], [10, 10, 10.5], [20, 20, 19.7], [30, 30, 40]], [[25, 35]])).toEqual({ ahead: 0.5, behind: expect.closeTo(-0.3, 9) });
  });
});

describe('refillRows', () => {
  it('gives each refill its start and length from its estimated stretch', () => {
    const rows = refillRows({ refills: [{ t_s: 18819, before_g: 3656.8, after_g: 4655.1 }] }, [[17147, 18819]]);
    expect(rows[0]).toMatchObject({ from_s: 17147, took_s: 1672 });
  });
  it('keeps a refill with no stretch, its length unknown', () => {
    expect(refillRows({ refills: [{ t_s: 500 }] }, [])[0]).toMatchObject({ from_s: 500, took_s: null });
  });
});

describe('whenLine', () => {
  it('names the end day only when it is another day', () => {
    expect(whenLine(at(19, 17, 15), at(20, 9, 0))).toBe('Mon 19 Oct 17:15 → Tue 20 Oct 09:00');
    expect(whenLine(at(15, 16, 19), at(15, 17, 19))).toBe('Thu 15 Oct 16:19 → 17:19');
    expect(whenLine(at(15, 16, 19), null)).toBe('Thu 15 Oct 16:19');
  });
});

describe('clockTicks', () => {
  it('puts a 15 h 45 m run on round 3 h clock times, midnight with its day', () => {
    const t = clockTicks(new Date(2026, 9, 19, 17, 15, 35).getTime(), 56677);
    expect(t.map((x) => x.label)).toEqual(['18:00', '21:00', 'Tue 00:00', '03:00', '06:00', '09:00']);
    expect(t[0].t).toBe(2665);
  });
  it('uses minutes for a short run', () => {
    const t = clockTicks(new Date(2026, 9, 15, 16, 19).getTime(), 3600);
    expect(t.map((x) => x.label)).toEqual(['16:20', '16:30', '16:40', '16:50', '17:00', '17:10']);
  });
});

describe('curveLine', () => {
  it('says a one-level step as a level and its end', () => {
    expect(curveLine(run194)).toBe('Step, 3.500 ml/min for 15 h 44 m, then 0.000');
  });
  it('says a linear curve from start to end', () => {
    expect(curveLine({ control_var: 'ml_min', duration_s: 3600, curve: { start: 13, end: 10, params: { kind: 'linear' } } }))
      .toBe('Linear, 13.000 → 10.000 ml/min over 1 h 0 m');
  });
  it('says a constant in rpm', () => {
    expect(curveLine({ control_var: 'rpm', duration_s: 600, curve: { start: 50, end: 50, params: { kind: 'constant' } } }))
      .toBe('Constant, 50.0 rpm for 10 m 0 s');
  });
});

describe('grouped', () => {
  it('sets thousands apart with a thin space', () => {
    expect(grouped(3305.61)).toBe('3 305.6');
    expect(grouped(98)).toBe('98.0');
    expect(grouped(-1234.5, 0)).toBe('−1 235');
  });
});
