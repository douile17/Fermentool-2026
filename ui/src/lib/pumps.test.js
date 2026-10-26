import { describe, expect, it } from 'vitest';
import { feedCameBack, feedLocked, feedResponding, headOf, refillKindLocked, lineState, maxRpmSetting, lineUp, needsReconnect, notAnsweringHint, pingDelayMs, pingWanted, pumpProblem, pumpTone, MODELS, modelOf, samePump } from './pumps.js';

const up = { serial_ok: true, simulator: false, transport: 'COM8', pump_confirmed: true, write_fails: 0 };

describe('lineState', () => {
  it('says the port and how many pumps answer, quiet when all do', () => {
    expect(lineState(up, { answering: 2, total: 2 })).toEqual({ tone: '', word: 'Connected', text: 'COM8, 2 pumps of 2 answer' });
  });
  it('turns to a warning when a pump does not answer', () => {
    expect(lineState(up, { answering: 1, total: 2 })).toEqual({ tone: 'warn', word: 'Connected', text: 'COM8, 1 pump of 2 answers' });
  });
  it('says what to check when the cable is down', () => {
    const s = lineState({ ...up, serial_ok: false }, { answering: 0, total: 2 });
    expect(s.tone).toBe('bad');
    expect(s.word).toBe('Not answering');
    expect(s.text).toMatch(/^COM8: check/);
  });
  it('tells the simulator apart, allowed or not', () => {
    expect(lineState({ ...up, simulator: true, allow_simulator: true }, { answering: 0, total: 2 }).tone).toBe('sim');
    // not connected (disconnected, or no port picked yet): grey, Connect is the way out
    expect(lineState({ ...up, simulator: true }, { answering: 0, total: 2 })).toMatchObject({ tone: 'idle', word: 'Not connected' });
  });
});

describe('pump states', () => {
  it('a pump is not asked while the cable is down', () => {
    expect(pumpTone({ up: false, responding: true })).toBe('idle');
    expect(pumpTone({ up: true, responding: null })).toBe('idle');
    expect(pumpTone({ up: true, responding: false })).toBe('bad');
    expect(pumpTone({ up: true, responding: true })).toBe('');
  });
  it('the feed pump answers when its writes get through', () => {
    expect(feedResponding(up)).toBe(true);
    expect(feedResponding({ ...up, write_fails: 3 })).toBe(false);
    expect(feedResponding({ ...up, serial_ok: false })).toBeNull();
    expect(lineUp({ ...up, simulator: true })).toBe(false);
  });
});

describe('needsReconnect', () => {
  const saved = { path: 'COM8', baud: 9600, feedAddress: 1 };
  it('only for the port, the baud or the feed pump address', () => {
    expect(needsReconnect(saved, { path: 'COM8', baud: '9600', feedAddress: '1' })).toBe(false);
    expect(needsReconnect(saved, { path: 'COM3', baud: 9600, feedAddress: 1 })).toBe(true);
    expect(needsReconnect(saved, { path: 'COM8', baud: 4800, feedAddress: 1 })).toBe(true);
    expect(needsReconnect(saved, { path: 'COM8', baud: 9600, feedAddress: 3 })).toBe(true);
  });
});

describe('refill pump ping', () => {
  it('asks only outside a run, with the cable up', () => {
    expect(pingWanted(up)).toBe(true);
    expect(pingWanted({ ...up, active: { id: 3 } })).toBe(false);
    expect(pingWanted({ ...up, serial_ok: false })).toBe(false);
    expect(pingWanted(null)).toBe(false);
  });
  it('asks again soon while the pump does not answer', () => {
    expect(pingDelayMs(false)).toBe(3_000);
    expect(pingDelayMs(true)).toBe(15_000);
    expect(pingDelayMs(null)).toBe(15_000);
  });
  it('asks at once when the feed pump comes back on the cable', () => {
    expect(feedCameBack(false, true)).toBe(true);
    expect(feedCameBack(null, true)).toBe(true);
    expect(feedCameBack(true, true)).toBe(false);
    expect(feedCameBack(true, false)).toBe(false);
  });
});

describe('models', () => {
  it('lists the three pumps with their limits', () => {
    expect(MODELS.map((m) => m.id)).toEqual(['labq', 'labv', 'pds2']);
    expect(modelOf('labv')).toMatchObject({ label: 'LabV', maxRpm: 150, ceiling: 600, maxAddress: 32 });
    expect(modelOf('nope').id).toBe('labq');
  });
});

describe('samePump', () => {
  it('two heads of one PDS II share an address but are two pumps', () => {
    expect(samePump({ address: 3, model: 'pds2', head: 1 }, { address: 3, model: 'pds2', head: 2 })).toBe(false);
    expect(samePump({ address: 3, model: 'pds2', head: 1 }, { address: 3, model: 'pds2', head: 1 })).toBe(true);
    expect(samePump({ address: 2, model: 'labv' }, { address: 2, model: 'labq' })).toBe(true);
    expect(samePump({ address: 2, model: 'labv' }, { address: 3, model: 'labv' })).toBe(false);
  });
});

describe('pumpProblem', () => {
  it('follows the model', () => {
    expect(pumpProblem({ model: 'labq', address: 200, maxRpm: null })).toBeNull();
    expect(pumpProblem({ model: 'labv', address: 33 })).toMatch(/1 to 32/);
    expect(pumpProblem({ model: 'pds2', address: 3, head: 9 })).toMatch(/1 to 8/);
    expect(pumpProblem({ model: 'labq', address: 1, maxRpm: 400 })).toMatch(/0\.1 and 350/);
    expect(pumpProblem({ model: 'labv', address: 1, maxRpm: 400 })).toBeNull();
    expect(pumpProblem({ model: 'labv', address: 1, maxRpm: 700 })).toMatch(/600/);
  });
});

describe('notAnsweringHint', () => {
  it('asks the right questions per model', () => {
    expect(notAnsweringHint({ model: 'labq', address: 1 })).toBe('Is the LabQ switched on, at address 1 and 9600 baud in its own menu?');
    expect(notAnsweringHint({ model: 'labv', address: 4 })).toMatch(/^Is the LabV switched on, at address 4/);
    expect(notAnsweringHint({ model: 'pds2', address: 3, head: 2 })).toMatch(/head 2 set to independent operation/);
  });
});

describe('needsReconnect, pump model', () => {
  const saved = { path: 'COM8', baud: 9600, feedAddress: 1, feedModel: 'labq', feedHead: 1, feedMaxRpm: null };
  const now = { path: 'COM8', baud: 9600, feedAddress: 1, feedModel: 'labq', feedHead: 1, feedMaxRpm: null };
  it('also when the feed model, head or max rpm changed', () => {
    expect(needsReconnect(saved, now)).toBe(false);
    expect(needsReconnect(saved, { ...now, feedModel: 'labv' })).toBe(true);
    expect(needsReconnect(saved, { ...now, feedHead: 2 })).toBe(true);
    expect(needsReconnect(saved, { ...now, feedMaxRpm: 200 })).toBe(true);
    expect(needsReconnect(saved, { ...now, feedMaxRpm: '' })).toBe(false);
  });
});

describe('maxRpmSetting', () => {
  it('keeps the model default as null, so it follows the model', () => {
    expect(maxRpmSetting('labq', 350)).toBe(null);
    expect(maxRpmSetting('labv', '150')).toBe(null);
    expect(maxRpmSetting('pds2', '')).toBe(null);
    expect(maxRpmSetting('labq', null)).toBe(null);
  });
  it('keeps a speed typed by hand', () => {
    expect(maxRpmSetting('labv', '120')).toBe(120);
    expect(maxRpmSetting('pds2', 350)).toBe(350);
  });
});

describe('headOf', () => {
  it('drives a LabQ or a LabV on head 1, a PDS II on its chosen head', () => {
    expect(headOf('labq', 4)).toBe(1);
    expect(headOf('labv', 4)).toBe(1);
    expect(headOf('pds2', '4')).toBe(4);
  });
});

describe('locks (Settings > Pumps)', () => {
  it('the feed pump is locked during a run and while an interrupted run waits', () => {
    expect(feedLocked({ active: null, has_pending_recovery: false })).toBe(false);
    expect(feedLocked({ active: { id: 1 } })).toBe(true);
    expect(feedLocked({ active: null, has_pending_recovery: true })).toBe(true);
    expect(feedLocked(null)).toBe(false);
  });
  it("the refill pump's model and head are locked in a refill run and while an interrupted one waits", () => {
    expect(refillKindLocked({ refill: null })).toBe(false);
    expect(refillKindLocked({ refill: { in_run: true, awaiting_resume: false } })).toBe(true);
    expect(refillKindLocked({ refill: { in_run: false, awaiting_resume: true } })).toBe(true);
    expect(refillKindLocked({ refill: { in_run: false, awaiting_resume: false, testing: true } })).toBe(false);
  });
  it('a Stop still owed after the run ended does not lock the refill model', () => {
    // A wrong model is what makes that Stop fail: changing it is the way out.
    expect(refillKindLocked({ refill: { in_run: false, recovering: true, awaiting_resume: false } })).toBe(false);
  });
});
