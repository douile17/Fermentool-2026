// Saving a file the page made (the Steps template, a run's CSV, settings).
//
// A browser downloads it from a blob link. The desktop window's webview saves
// no download (the Template did nothing there, 2026-11-17), so there the
// daemon, on the same PC, writes it into Downloads and opens Explorer on it.
import { BASE } from './api.js';

export const inDesktop = (w = typeof window !== 'undefined' ? window : {}) => '__TAURI_INTERNALS__' in w;

/** A run's name made safe for a file name: what the daemon accepts (ASCII
 *  letters, digits, space, `_ . - ( )`), anything else an underscore. */
export const plainName = (name) => String(name ?? '').trim().replace(/[^\w .()-]+/g, '_').replace(/^[. ]+/, '').slice(0, 80) || 'run';

/** How a file is saved: by the daemon in the desktop window, else the browser. */
export const saveWay = (desktop) => (desktop ? 'daemon' : 'browser');

/** Save `blob` as `name`. Resolves once saved; throws with a message to show. */
export async function saveFile(name, blob, desktop = inDesktop()) {
  if (saveWay(desktop) === 'daemon') {
    let res;
    try {
      res = await fetch(`${BASE}/api/files/save?name=${encodeURIComponent(name)}`, { method: 'POST', body: blob });
    } catch {
      throw new Error('The daemon cannot be reached: the file was not saved.');
    }
    if (!res.ok) {
      const body = await res.json().catch(() => null);
      throw new Error(body?.error ?? `The file was not saved (${res.status}).`);
    }
    return;
  }
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = name;
  a.click();
  // Not at once: some browsers drop a download whose URL is gone before it starts.
  setTimeout(() => URL.revokeObjectURL(url), 10_000);
}

/** saveFile for a button with nowhere to show an error: a dialog says it. */
export function saveOrSay(name, blob) {
  return saveFile(name, blob).catch((e) => window.alert(e.message));
}
