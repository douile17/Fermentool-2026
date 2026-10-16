import { describe, expect, it } from 'vitest';
import { cycleLine, directionLabel, inSpans, refillOn, refillPlan, refillProblem, refillToChoose, refillView, splitEstimated } from './refill.js';

describe('refillProblem', () => {
  const ok = { address: 2, feedAddress: 1, speedRpm: 350 };
  it('accepts a separate address and a LabQ speed', () => {
    expect(refillProblem(ok)).toBeNull();
    expect(refillProblem({ ...ok, address: '247', speedRpm: '0.1' })).toBeNull();
  });
  it("refuses the feed pump's address", () => {
    expect(refillProblem({ ...ok, address: 1 })).toMatch(/feed pump's/);
    expect(refillProblem({ ...ok, address: '1' })).toMatch(/feed pump's/);
  });
  it('refuses an address or a speed out of range', () => {
    expect(refillProblem({ ...ok, address: 0 })).toMatch(/1 to 247/);
    expect(refillProblem({ ...ok, address: 2.5 })).toMatch(/1 to 247/);
    expect(refillProblem({ ...ok, speedRpm: 400 })).toMatch(/rpm/);
    expect(refillProblem({ ...ok, speedRpm: '' })).toMatch(/rpm/);
  });
});

describe('direction', () => {
  it('reads cw / ccw, as everywhere else', () => {
    expect(directionLabel('ccw')).toBe('ccw');
    expect(directionLabel('cw')).toBe('cw');
  });
});

describe('refillOn', () => {
  it('needs the wish, the balance regulation and the balance under the feed bottle', () => {
    expect(refillOn({ wanted: true, trimOn: true, scaleOnFeed: true })).toBe(true);
    expect(refillOn({ wanted: false, trimOn: true, scaleOnFeed: true })).toBe(false);
    expect(refillOn({ wanted: true, trimOn: false, scaleOnFeed: true })).toBe(false);
    expect(refillOn({ wanted: true, trimOn: true, scaleOnFeed: false })).toBe(false);
    expect(refillOn({ wanted: null, trimOn: true, scaleOnFeed: true })).toBe(false);
  });
});

describe('refillToChoose', () => {
  const asked = { trimOn: true, scaleOnFeed: true };
  it('waits for a choice, nothing picked by default', () => {
    expect(refillToChoose({ ...asked, wanted: null })).toBe(true);
    expect(refillToChoose({ ...asked, wanted: undefined })).toBe(true);
    expect(refillToChoose({ ...asked, wanted: true })).toBe(false);
    expect(refillToChoose({ ...asked, wanted: false })).toBe(false);
  });
  it('asks nothing without the balance regulation on the feed bottle', () => {
    expect(refillToChoose({ trimOn: false, scaleOnFeed: true, wanted: null })).toBe(false);
    expect(refillToChoose({ trimOn: true, scaleOnFeed: false, wanted: null })).toBe(false);
  });
});

describe('refillView', () => {
  const r = { in_run: true, address: 2, responding: true, testing: false };
  it('shows nothing while no run drives the refill pump', () => {
    expect(refillView(null)).toBeNull();
  });
  it('reads ready, asking, testing, silent', () => {
    expect(refillView(r).tone).toBe('ok');
    expect(refillView({ ...r, responding: null }).tone).toBe('idle');
    expect(refillView({ ...r, testing: true }).label).toMatch(/testing/);
    const silent = refillView({ ...r, responding: false });
    expect(silent.tone).toBe('bad');
    expect(silent.title).toMatch(/address 2/);
  });
  it('says a Stop owed after a test or a run may have left the pump running', () => {
    expect(refillView({ ...r, in_run: false, responding: false }).label).toMatch(/Stop not confirmed/);
  });
});

describe('splitEstimated', () => {
  const pts = [[0, 0], [1, 0], [2, -1], [3, -2], [4, 0], [5, 0]];
  it('cuts the line where a refill was estimated, unbroken', () => {
    const parts = splitEstimated(pts, [[2, 3]]);
    expect(parts.map((p) => p.est)).toEqual([false, true, false]);
    expect(parts[1].pts).toEqual([[1, 0], [2, -1], [3, -2]]);
    expect(parts[2].pts[0]).toEqual([3, -2]);
  });
  it('is one measured piece without spans', () => {
    expect(splitEstimated(pts, [])).toEqual([{ est: false, pts }]);
    expect(splitEstimated(pts, undefined)).toHaveLength(1);
  });
  it('knows a time inside a span', () => {
    expect(inSpans(2.5, [[2, 3]])).toBe(true);
    expect(inSpans(4, [[2, 3]])).toBe(false);
  });
});

describe('refillPlan', () => {
  it('refills once the bottle gave what was entered, back up to the full bottle', () => {
    // 300 g used out of a full 4.98 kg bottle: under 4.68 kg, not under 0.3 kg.
    expect(refillPlan({ drawG: 300, weightG: 4980 })).toEqual({ lowG: 4680, highG: 4980 });
    expect(refillPlan({ drawG: 1600, weightG: 9600 })).toEqual({ lowG: 8000, highG: 9600 });
  });
  it('says what is missing or wrong', () => {
    expect(refillPlan({ drawG: null, weightG: 9600 }).error).toMatch(/before a refill/);
    expect(refillPlan({ drawG: 50, weightG: 9600 }).error).toMatch(/at least 100 g/);
    expect(refillPlan({ drawG: 1600, weightG: null }).error).toMatch(/No balance/);
    expect(refillPlan({ drawG: 5000, weightG: 4980 }).error).toMatch(/fill the bottle/);
  });
});

describe('cycleLine', () => {
  const c = { phase: 'idle', halted: false, low_g: 8000, high_g: 9600, weight_g: 9100 };
  it('reads the cycle in a few words', () => {
    expect(cycleLine(c)).toBe('9.10 kg, refills under 8.00 kg');
    expect(cycleLine({ ...c, phase: 'filling', weight_g: 7950 })).toBe('filling 7.95 kg → 9.60 kg');
    expect(cycleLine({ ...c, halted: true })).toBe('stopped, 9.10 kg');
  });
  it('flags a halted refill in the sidebar', () => {
    const v = refillView({ in_run: true, address: 2, responding: true, testing: false, cycle: { ...c, halted: true } });
    expect(v.tone).toBe('bad');
  });
});
