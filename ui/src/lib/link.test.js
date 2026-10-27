import { describe, expect, it } from 'vitest';
import { linkState } from './link.js';

const up = {
  serial_ok: true, simulator: false, transport: 'COM5', serial_port: 'COM5', pump_address: 1,
  pump_confirmed: true, write_fails: 0, journal_ok: true, warnings: [], refill: null,
};
const refill = (responding, in_run = true) => ({ in_run, address: 2, responding, testing: false, cycle: null });

describe('linkState on a cable shared by two pumps', () => {
  it('quiet while both answer', () => {
    expect(linkState({ ...up, refill: refill(true) }, true).tone).toBe('ok');
  });
  // On the bench (2026-10-23), switching off either pump silenced the other:
  // the daemon cannot tell which one went, so the line names neither.
  it('a silent cable blames neither pump, whatever the refill pump said last', () => {
    for (const r of [refill(true), refill(false), refill(null, false)]) {
      const l = linkState({ ...up, serial_ok: false, refill: r }, true);
      expect(l).toMatchObject({ tone: 'bad', label: 'Pumps not answering', connectable: true });
      expect(l.hint).toMatch(/^Both pumps share the cable \(addresses 1 and 2 on COM5\): one switched off silences the other\./);
    }
  });
  it('a single pump on its cable, with its address and port', () => {
    const l = linkState({ ...up, serial_ok: false }, true);
    expect(l).toMatchObject({ tone: 'bad', label: 'Pump not answering', connectable: true });
    expect(l.hint).toMatch(/^Address 1 on COM5\./);
  });
  it('the refill pump alone during a run, the feed pump answering: orange, feeding goes on', () => {
    const l = linkState({ ...up, refill: refill(false) }, true);
    expect(l).toMatchObject({ tone: 'warn', label: 'Refill pump not answering' });
    expect(l.hint).toMatch(/^Address 2 on COM5\. Feeding goes on/);
  });
  it('nothing for a refill pump never asked, or outside a run', () => {
    expect(linkState({ ...up, refill: refill(null) }, true).tone).toBe('ok');
    expect(linkState({ ...up, refill: refill(false, false) }, true).tone).toBe('ok');
  });
  it('falls back to the transport when the port is not given', () => {
    const l = linkState({ ...up, serial_ok: false, serial_port: undefined }, true);
    expect(l.hint).toMatch(/on COM5\./);
  });
});
