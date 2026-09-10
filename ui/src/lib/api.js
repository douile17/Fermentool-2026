// Thin wrappers over the daemon's REST + WebSocket API.
//
// In a Tauri window the UI is bundled into the app, so it runs on a different
// origin (http://tauri.localhost) from the daemon and must use an absolute URL.
// In the browser (Vite dev proxy, or the daemon serving ui/dist itself) it is
// same-origin, so BASE is '' and every path stays relative - unchanged.
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

async function req(method, path, payload) {
  const res = await fetch(BASE + path, {
    method,
    headers: payload !== undefined ? { 'content-type': 'application/json' } : undefined,
    body: payload !== undefined ? JSON.stringify(payload) : undefined,
  });
  const data = await body(res);
  if (!res.ok) {
    const msg = (data && data.error) || `${res.status} ${res.statusText}`;
    throw new Error(msg);
  }
  return data;
}

export const get = (path) => req('GET', path);
export const post = (path, payload) => req('POST', path, payload ?? {});
export const put = (path, payload) => req('PUT', path, payload);
export const del = (path) => req('DELETE', path);

/** Subscribe to status frames. Returns an unsubscribe function. Auto-reconnects. */
export function connectWs(onStatus) {
  let live = true;
  let ws = null;
  let retry = 1500;

  const open = () => {
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
      setTimeout(open, retry);
      retry = Math.min(retry * 2, 15000); // backoff, capped at 15s
    };
    ws.onerror = () => ws && ws.close();
  };
  open();

  return () => {
    live = false;
    if (ws) ws.close();
  };
}
