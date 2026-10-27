import { describe, expect, it } from 'vitest';
import {
  alarmCount, atRun, blindLegend, curveChips, curveLine, gapExtremes, gapScale, gapSeries, grouped, journalLines, pumpLine,
  refillRows, regulationFigures, verdict, whenLine,
} from './rundetail.js';

// Local times, whatever the machine's zone: the UI shows the PC's clock.
const at = (d, h, m, s = 0) => new Date(2026, 9, d, h, m, s).toISOString();

// Run 194 as journalled: 3.5 ml/min step, three automatic refills.
const run194 = {
  status: 'completed',
  started_at: at(19, 17, 15, 35),
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
    expect(lines[2]).toMatchObject({ time: '4 h 45', tone: 'refill', detail: '28 min at 55 rpm, 3 656.8 → 4 655.1 g' });
  });

  it('counts the experiment time, not the time of day', () => {
    expect(lines[0].time).toBe('0');
    expect(lines[1].time).toBe('2 min');
    expect(lines.every((l) => !/:/.test(l.time))).toBe(true);
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

describe('atRun', () => {
  it('says run time as the experiment counts it', () => {
    expect(atRun(0)).toBe('0');
    expect(atRun(47 * 60 + 10)).toBe('47 min');
    expect(atRun(4 * 3600 + 45 * 60 + 46)).toBe('4 h 45');
    expect(atRun(26 * 3600 + 3 * 60)).toBe('26 h 03');
  });
});

describe('curveChips', () => {
  const text = (run) => curveChips(run).map((c) => c.text ?? `${c.value} ${c.unit}`);
  it('says the shape, start and end, length and direction (run 210)', () => {
    const run210 = { control_var: 'ml_min', duration_s: 10800, direction: 'ccw', curve: { mode: 'endpoints', start: 6, end: 9, params: { kind: 'exponential', mu_per_hour: 0.15 } } };
    expect(text(run210)).toEqual(['Exponential', '6.000 → 9.000 ml/min', 'over 3 h 0 m', 'counter-clockwise']);
  });
  it('gives µ only when it was asked (Physio)', () => {
    const run = { control_var: 'ml_min', duration_s: 3600, direction: 'cw', curve: { mode: 'physio', start: 2, end: 2.3, params: { kind: 'exponential', mu_per_hour: 0.14 } } };
    expect(text(run)).toContain('µ 0.140 h⁻¹');
  });
  it('a constant or a one-level step says its level', () => {
    expect(text({ control_var: 'ml_min', duration_s: 3600, direction: 'cw', curve: { start: 5, end: 5, params: { kind: 'constant' } } }).slice(1, 3)).toEqual(['5.000 ml/min', 'for 1 h 0 m']);
    expect(text(run194)[1]).toBe('3.500 ml/min, then 0.000');
  });
  // Run 214 of the mockup: its rate changed twice.
  const constant = { control_var: 'ml_min', duration_s: 86400, direction: 'cw', curve: { start: 5, end: 5, params: { kind: 'constant' } } };
  const change = (elapsed_s, value) => ({ elapsed_s, value, wall_time: at(26, 14, 10) });
  it('a constant whose rate changed says each rate', () => {
    const run214 = { ...constant, setpoint_changes: [change(22800, 3), change(65100, 4)] };
    expect(text(run214)).toEqual(['Constant', '5.000 ml/min, then 3.000, 4.000', 'for 24 h 0 m', 'clockwise']);
  });
  it('many changes are counted, the last said', () => {
    const run = { ...constant, setpoint_changes: [change(10, 3), change(20, 4), change(30, 6), change(40, 2)] };
    expect(text(run)[1]).toBe('5.000 ml/min, changed 4×, last 2.000');
  });
  it('without a change, exactly as before', () => {
    expect(curveChips({ ...constant, setpoint_changes: [] })).toEqual(curveChips(constant));
    expect(text(constant)).toEqual(['Constant', '5.000 ml/min', 'for 24 h 0 m', 'clockwise']);
  });
});

describe('journalLines, a rate change', () => {
  it('titles it Rate changed, from and to', () => {
    const run = { control_var: 'ml_min', status: 'completed', started_at: at(26, 14, 10), curve: { start: 5, end: 5, params: { kind: 'constant' } } };
    const lines = journalLines([ev('setpoint_change', at(26, 20, 30), '5.000 -> 3.000 ml/min')], run);
    expect(lines.map((l) => [l.time, l.title, l.detail])).toEqual([['6 h 20', 'Rate changed', '5.000 → 3.000 ml/min']]);
  });
});

describe('journalLines, a refill cut and resumed', () => {
  // Run 210: the balance went silent mid-fill, the refill pump stopped, the
  // still weight read as a refill over; the fill resumed 1 s later. One
  // refill, not "Refill 1" and "Refill 2".
  const run210 = { control_var: 'ml_min', status: 'completed', started_at: at(21, 11, 26, 45), curve: { start: 6, end: 9, params: { kind: 'exponential' } } };
  const evs = [
    ev('refill_start', at(21, 13, 48, 16), 'under the refill level: 3612 g on the balance, filling to 4612 g at 55 rpm'),
    ev('refill_interrupted', at(21, 14, 19, 3), 'no balance reading: refill pump stopped, the refill starts again once the balance answers', 'warn'),
    ev('refill', at(21, 14, 19, 33), 'bottle refilled: 3612.3 g -> 4570.8 g (+958.5 g)'),
    ev('refill_start', at(21, 14, 19, 34), 'resuming the interrupted refill: 4570 g on the balance, filling to 4612 g at 350 rpm'),
    ev('refill_stop', at(21, 14, 19, 44), 'filled 38 g in 10 s at 350 rpm (+225 g/min net of the feed)'),
    ev('refill', at(21, 14, 20, 16), 'bottle refilled: 4570.8 g -> 4609.5 g (+38.7 g)'),
  ];
  const lines = journalLines(evs, run210);
  it('is one refill line, from the first weight to the last', () => {
    const refills = lines.filter((l) => l.title.startsWith('Refill ') && l.tone === 'refill');
    expect(refills.map((l) => l.title)).toEqual(['Refill 1']);
    expect(refills[0].detail.replace(/\s/g, ' ')).toBe('32 min at 55 rpm, 3 612.3 → 4 609.5 g');
  });
  it('keeps the interruption as its own line', () => {
    expect(lines.some((l) => l.title === 'Refill interrupted')).toBe(true);
  });
});

describe('blindLegend', () => {
  it('calls a stretch a refill only when it is one', () => {
    expect(blindLegend(3, 3)).toBe('Refill, not weighed');
    expect(blindLegend(8, 2)).toBe('Refill or bump, not weighed');
    expect(blindLegend(4, 0)).toBe('Not weighed');
  });
});

describe('pumpLine', () => {
  it('names the model, and the head on a PDS II', () => {
    expect(pumpLine({ pump_model: 'labq' })).toBe('LabQ');
    expect(pumpLine({ pump_model: 'labv', pump_head: 1 })).toBe('LabV');
    expect(pumpLine({ pump_model: 'pds2', pump_head: 2 })).toBe('PDS II, head 2');
  });
  it('a run made before the models was a LabQ', () => {
    expect(pumpLine({})).toBe('LabQ');
    expect(pumpLine({ pump_model: null })).toBe('LabQ');
  });
});
