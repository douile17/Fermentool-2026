import { describe, expect, it } from 'vitest';
import { inDesktop, plainName, saveWay } from './savefile.js';

describe('saving a file', () => {
  // the desktop window's webview saved no download: the Template did nothing (2026-11-17)
  it('goes through the daemon in the desktop window', () => {
    expect(inDesktop({ __TAURI_INTERNALS__: {} })).toBe(true);
    expect(saveWay(true)).toBe('daemon');
  });
  it("makes a run's name a name the daemon saves", () => {
    expect(plainName('Run 12 (E. coli)')).toBe('Run 12 (E. coli)');
    expect(plainName('Fed-batch é/µ: 3')).toBe('Fed-batch _ 3');
    expect(plainName('../x')).toBe('_x');
    expect(plainName('')).toBe('run');
    expect(plainName('a'.repeat(200))).toHaveLength(80);
  });
  it('stays a browser download in a browser', () => {
    expect(inDesktop({})).toBe(false);
    expect(saveWay(false)).toBe('browser');
  });
});
