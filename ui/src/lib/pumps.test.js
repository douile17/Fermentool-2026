import { describe, expect, it } from 'vitest';
import { feedCameBack, feedResponding, lineState, lineUp, needsReconnect, pingDelayMs, pingWanted, pumpTone } from './pumps.js';

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
