import { afterEach, describe, expect, it, vi } from 'vitest';
import { readFileSync } from 'node:fs';

// The desktop window loads the bundled UI from its own origin and the live
// view's <img> from the daemon: the window's CSP must let images come from
// the origin api.js uses there (BASE), or the Camera page stays black.
const conf = JSON.parse(readFileSync(new URL('../../../src-tauri/tauri.conf.json', import.meta.url), 'utf-8'));

function directive(csp, name) {
  const d = csp
    .split(';')
    .map((s) => s.trim().split(/\s+/))
    .find(([n]) => n === name);
  return d ? d.slice(1) : null;
}

describe('Tauri window CSP', () => {
  afterEach(() => {
    delete globalThis.window;
    vi.resetModules();
  });

  it("lets the live view's images come from the daemon", async () => {
    globalThis.window = { __TAURI_INTERNALS__: {} };
    vi.resetModules();
    const { BASE } = await import('./api.js');
    expect(BASE).toMatch(/^http:\/\/127\.0\.0\.1:\d+$/);
    const csp = conf.app.security.csp;
    expect(directive(csp, 'img-src')).toContain(BASE);
    expect(directive(csp, 'connect-src')).toContain(BASE);
  });
});
