// Thin wrappers over the daemon's REST + WebSocket API.
//
// In a Tauri window the UI is bundled into the app, so it runs on a different
// origin (http://tauri.localhost) from the daemon and must use an absolute URL.
// In the browser (Vite dev proxy, or the daemon serving ui/dist itself) it is
// same-origin, so BASE is '' and every path stays relative, unchanged.
const BASE =
  typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
    ? 'http://127.0.0.1:8730'
    : '';

async function body(res) {
  const text = await res.text();
  try {
    return text ? JSON.parse(text) : null;
  } catch {
    return text;
  }
}

// A request the daemon never answers (its control thread waiting on a
// device) must not leave a button on "Stopping…" for good.
const TIMEOUT_MS = 15000;

async function req(method, path, payload) {
  let res;
  try {
    res = await fetch(BASE + path, {
      method,
      headers: payload !== undefined ? { 'content-type': 'application/json' } : undefined,
      body: payload !== undefined ? JSON.stringify(payload) : undefined,
      signal: AbortSignal.timeout(TIMEOUT_MS),
    });
  } catch (e) {
    const err = new Error(
      e?.name === 'TimeoutError' ? 'The daemon did not answer in time.' : 'The daemon cannot be reached.',
    );
    err.status = 0;
    throw err;
  }
  const data = await body(res);
  if (!res.ok) {
    const msg = (data && data.error) || `${res.status} ${res.statusText}`;
    const err = new Error(msg);
    err.status = res.status;
    if (data && typeof data === 'object') {
      // Structured engine refusals carry a stable `code` and a longer `hint`.
      if (data.code) err.code = data.code;
      if (data.hint) err.hint = data.hint;
    }
    throw err;
  }
  return data;
}

export const get = (path) => req('GET', path);
export const post = (path, payload) => req('POST', path, payload ?? {});
export const put = (path, payload) => req('PUT', path, payload);
export const del = (path) => req('DELETE', path);

/** Subscribe to status frames; `onDown` is called when the socket drops.
 *  Returns an unsubscribe function. Auto-reconnects. */
export function connectWs(onStatus, onDown = () => {}) {
  let live = true;
  let ws = null;
  let retry = 1500;
  let timer = null;

  const open = () => {
    // A reconnect scheduled before an unsubscribe must not open a socket
    // nobody closes.
    if (!live) return;
    const wsBase = BASE
      ? BASE.replace(/^http/, 'ws')
      : `${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}`;
    ws = new WebSocket(`${wsBase}/api/ws`);
    ws.onopen = () => {
      retry = 1500;
      // Pull a fresh snapshot on every (re)connect so a dropped socket during
      // a long run doesn't leave the UI on stale status.
      fetch(BASE + '/api/status')
        .then((r) => (r.ok ? r.json() : null))
        .then((s) => s && onStatus(s))
        .catch(() => {});
    };
    ws.onmessage = (e) => {
      try {
        onStatus(JSON.parse(e.data));
      } catch {
        /* ignore malformed frame */
      }
    };
    ws.onclose = () => {
      if (!live) return;
      onDown();
      timer = setTimeout(open, retry);
      retry = Math.min(retry * 2, 15000); // backoff, capped at 15s
    };
    ws.onerror = () => ws && ws.close();
  };
  open();

  return () => {
    live = false;
    clearTimeout(timer);
    if (ws) ws.close();
  };
}
