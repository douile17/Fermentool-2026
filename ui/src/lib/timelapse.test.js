import { describe, expect, it } from 'vitest';
import {
  BYTES_PER_IMAGE,
  bytesText,
  cardState,
  estimate,
  estimateText,
  freeWarning,
  nextInText,
  perDayText,
  timelapseSeed,
} from './timelapse.js';

describe('timelapseSeed', () => {
  it('Run again keeps the run\'s timelapse choice and interval', () => {
    expect(timelapseSeed({ timelapse: { id: 3, interval_s: 600, images: 12 } })).toEqual({ timelapse: true, timelapse_min: 10 });
    expect(timelapseSeed({ timelapse: null })).toEqual({ timelapse: false, timelapse_min: null });
    expect(timelapseSeed({})).toEqual({ timelapse: false, timelapse_min: null });
  });
});

describe('estimate', () => {
  it('100 h every 5 min at 1280 x 720: 1201 images, the first at the start', () => {
    const e = estimate(100 * 3600, 5, 1280, 720);
    expect(e.images).toBe(1201);
    expect(e.bytes).toBe(1201 * BYTES_PER_IMAGE['1280x720']);
    expect(e.perDay).toEqual({ images: 288, bytes: 288 * BYTES_PER_IMAGE['1280x720'] });
  });
  it('uses the real average once images exist', () => {
    expect(estimate(3600, 1, 1280, 720, 100_000).bytes).toBe(61 * 100_000);
  });
  it('an unknown size falls back to 1280 x 720', () => {
    expect(estimate(3600, 60, 800, 600).bytes).toBe(2 * BYTES_PER_IMAGE['1280x720']);
  });
  it('no duration or interval yet: nothing to say', () => {
    expect(estimate(0, 5, 1280, 720)).toBeNull();
    expect(estimate(3600, 0, 1280, 720)).toBeNull();
  });
});

describe('words', () => {
  it('rounds to two figures, thousands spaced', () => {
    expect(estimateText(estimate(100 * 3600, 5, 1280, 720))).toBe('about 1 200 images, 310 MB');
    expect(estimateText({ images: 7, bytes: 1_800_000 })).toBe('about 7 images, 1.8 MB');
    expect(estimateText(null)).toBe('');
  });
  it('bytes in kB, MB, GB', () => {
    expect(bytesText(13_500)).toBe('14 kB');
    expect(bytesText(306_000_000)).toBe('310 MB');
    expect(bytesText(1_234_000_000)).toBe('1.2 GB');
    expect(bytesText(0)).toBe('0 kB');
  });
  it('a day of images', () => {
    expect(perDayText(estimate(3600, 5, 1280, 720))).toBe('288 images, 73 MB a day');
  });
  it('the next image in a few words', () => {
    const now = Date.parse('2026-11-05T10:00:00Z');
    expect(nextInText('2026-11-05T10:03:20Z', now)).toBe('in 3 m 20 s');
    expect(nextInText('2026-11-05T09:59:59Z', now)).toBe('now');
    expect(nextInText(null, now)).toBe('–');
  });
});

describe('cardState', () => {
  const rec = { state: 'recording', kind: 'standalone' };
  const connected = { state: 'connected' };
  it('says which of the card states to show', () => {
    expect(cardState(null, connected)).toBe('none');
    expect(cardState(rec, connected)).toBe('recording');
    expect(cardState({ ...rec, kind: 'run' }, connected)).toBe('recording_run');
    expect(cardState({ ...rec, state: 'paused' }, connected)).toBe('paused');
    expect(cardState({ ...rec, state: 'awaiting_resume' }, connected)).toBe('awaiting');
    expect(cardState({ ...rec, state: 'waiting_camera' }, connected)).toBe('waiting');
  });
  it('a camera lost or held elsewhere is waiting before a slot even fails', () => {
    expect(cardState(rec, { state: 'lost' })).toBe('waiting');
    expect(cardState(rec, { state: 'in_use' })).toBe('waiting');
    expect(cardState({ ...rec, state: 'paused' }, { state: 'lost' })).toBe('paused');
  });
});

describe('freeWarning', () => {
  it('says why the disk matters', () => {
    expect(freeWarning({ state: 'paused', pause_reason: 'disk' }, false)).toBe('Paused: under 200 MB free on this disk');
    expect(freeWarning({ state: 'paused', pause_reason: 'folder' }, false)).toBe('Paused: the folder cannot be written to');
    expect(freeWarning({ state: 'recording' }, true)).toBe('Under 1 GB free on this disk');
    expect(freeWarning({ state: 'paused', pause_reason: 'user' }, false)).toBeNull();
    expect(freeWarning(null, true)).toBeNull();
  });
});
