import { describe, expect, it } from 'vitest';
import {
  folderPreview,
  folderToSend,
  frameClock,
  frameFigures,
  frameStamp,
  playFrom,
  sameFolder,
  spreadPick,
  stepFrame,
  wholeRunWords,
} from './tlplayer.js';

describe('sameFolder', () => {
  it('reads Windows paths as Windows does', () => {
    expect(sameFolder('D:\\Manips\\A', 'd:/manips/a/')).toBe(true);
    expect(sameFolder('D:\\Manips\\A', 'D:\\Manips\\AB')).toBe(false);
    expect(sameFolder(null, 'D:\\')).toBe(false);
  });
});

describe('folderToSend', () => {
  const saved = 'D:\\Manips\\A';
  const def = 'C:\\Users\\x\\AppData\\Roaming\\Fermentool\\captures';
  it('sends nothing when the folder is the one saved', () => {
    expect(folderToSend('d:\\manips\\a\\', saved, def)).toBeUndefined();
    expect(folderToSend(null, saved, def)).toBeUndefined();
  });
  it('sends an empty folder for the default one, so the setting stays empty', () => {
    expect(folderToSend(def, saved, def)).toBe('');
  });
  it('sends a folder picked', () => {
    expect(folderToSend('E:\\B', saved, def)).toBe('E:\\B');
  });
});

describe('folderPreview', () => {
  const at = new Date(2026, 10, 19, 9, 5);
  it('names the folder as the daemon does', () => {
    expect(folderPreview(at, 'Fed-batch 12')).toBe('2026-11-19_0905 Fed-batch 12');
  });
  it('replaces what Windows refuses and drops a trailing dot', () => {
    expect(folderPreview(at, 'a/b:c?.')).toBe('2026-11-19_0905 a_b_c_');
    expect(folderPreview(at, 'x'.repeat(80)).length).toBe('2026-11-19_0905 '.length + 60);
    expect(folderPreview(at, '  ')).toBe('2026-11-19_0905');
  });
});

describe('spreadPick', () => {
  const frames = Array.from({ length: 101 }, (_, i) => ({ id: i }));
  it('spreads over the run, first and last included', () => {
    expect(spreadPick(frames, 6).map((f) => f.id)).toEqual([0, 20, 40, 60, 80, 100]);
  });
  it('keeps every frame when there are few', () => {
    expect(spreadPick(frames.slice(0, 3), 6).map((f) => f.id)).toEqual([0, 1, 2]);
    expect(spreadPick([], 6)).toEqual([]);
    expect(spreadPick(undefined, 6)).toEqual([]);
  });
});

describe('the player', () => {
  it('steps within the run', () => {
    expect(stepFrame(0, 10, -1)).toBe(0);
    expect(stepFrame(9, 10, 1)).toBe(9);
    expect(stepFrame(4, 10, 1)).toBe(5);
    expect(stepFrame(3, 0, 1)).toBe(0);
  });
  it('plays again from the start once at the end', () => {
    expect(playFrom(9, 10)).toBe(0);
    expect(playFrom(4, 10)).toBe(4);
  });
  it('says how long the whole run plays', () => {
    expect(wholeRunWords(1201, 10)).toBe('the whole run in 2 min');
    expect(wholeRunWords(1201, 50)).toBe('the whole run in 24 s');
    expect(wholeRunWords(1201, 5)).toBe('the whole run in 4 min');
    expect(wholeRunWords(3, 50)).toBe('the whole run in 1 s');
  });
  it('says the time in the run and what it was doing', () => {
    expect(frameClock(0)).toBe('0 min');
    expect(frameClock(35 * 60 + 10)).toBe('35 min');
    expect(frameClock(42 * 3600 + 5 * 60)).toBe('42 h 05 min');
    // the pump's corrected setpoint is never said as the rate
    expect(frameFigures({ setpoint: 6.032, unit: 'ml_min', delivered_ml: 1410.4 })).toBe('1 410 mL fed');
    expect(frameFigures({ setpoint: 43.3, unit: 'rpm', delivered_ml: null })).toBe('');
    expect(frameFigures({})).toBe('');
  });
  it('says when the image was taken, in local time', () => {
    const at = new Date(2026, 10, 18, 14, 32, 5).toISOString();
    expect(frameStamp(at)).toBe('Wed 18 Nov 2026, 14:32:05');
    expect(frameStamp(null)).toBe('');
    expect(frameStamp('not a date')).toBe('');
  });
});
