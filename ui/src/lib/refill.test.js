import { describe, expect, it } from 'vitest';
import { directionLabel, refillOn, refillProblem, refillToChoose, refillView } from './refill.js';

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
