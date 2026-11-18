import { describe, expect, it } from 'vitest';
import { desktopBehaviour, isReloadKey, keepsMenu } from './desktop.js';

const el = (inField) => ({ closest: (sel) => (inField && sel.includes('input') ? {} : null) });

describe('keepsMenu', () => {
  it('keeps the menu in a field and on selected text', () => {
    expect(keepsMenu(el(true))).toBe(true);
    expect(keepsMenu(el(false), 'run 266')).toBe(true);
  });
  it('drops it anywhere else', () => {
    expect(keepsMenu(el(false))).toBe(false);
    expect(keepsMenu(el(false), '   ')).toBe(false);
    expect(keepsMenu(null)).toBe(false);
  });
  it('reads a text node through its parent', () => {
    expect(keepsMenu({ parentElement: el(true) })).toBe(true);
  });
});

describe('isReloadKey', () => {
  it('knows F5 and Ctrl+R in both cases', () => {
    expect(isReloadKey({ key: 'F5' })).toBe(true);
    expect(isReloadKey({ key: 'r', ctrlKey: true })).toBe(true);
    expect(isReloadKey({ key: 'R', ctrlKey: true, shiftKey: true })).toBe(true);
  });
  it('leaves typing and other shortcuts alone', () => {
    expect(isReloadKey({ key: 'r' })).toBe(false);
    expect(isReloadKey({ key: 'c', ctrlKey: true })).toBe(false);
    expect(isReloadKey({ key: 'r', ctrlKey: true, altKey: true })).toBe(false);
  });
});

describe('desktopBehaviour', () => {
  it('does nothing in a browser', () => {
    expect(desktopBehaviour({})).toBe(false);
  });
  it('installs its listeners in the desktop window', () => {
    const on = {};
    const classes = new Set();
    const w = {
      __TAURI_INTERNALS__: {},
      getSelection: () => '',
      document: { documentElement: { classList: { add: (c) => classes.add(c) } }, addEventListener: (k, f) => (on[k] = f) },
    };
    expect(desktopBehaviour(w)).toBe(true);
    expect(classes.has('desktop')).toBe(true);
    let stopped = false;
    on.keydown({ key: 'F5', preventDefault: () => (stopped = true) });
    expect(stopped).toBe(true);
    stopped = false;
    on.contextmenu({ target: el(false), preventDefault: () => (stopped = true) });
    expect(stopped).toBe(true);
    stopped = false;
    on.contextmenu({ target: el(true), preventDefault: () => (stopped = true) });
    expect(stopped).toBe(false);
  });
});
