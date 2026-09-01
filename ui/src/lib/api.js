// Thin wrappers over the daemon's REST + WebSocket API (same origin).

async function body(res) {
  const text = await res.text();
  try {
    return text ? JSON.parse(text) : null;
  } catch {
    return text;
  }
}

async function req(method, path, payload) {
  const res = await fetch(path, {
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

/** Subscribe to status frames. Returns an unsubscribe function. Auto-reconnects. */
export function connectWs(onStatus) {
  let live = true;
  let ws = null;

  const open = () => {
    const proto = location.protocol === 'https:' ? 'wss' : 'ws';
    ws = new WebSocket(`${proto}://${location.host}/api/ws`);
    ws.onmessage = (e) => {
      try {
        onStatus(JSON.parse(e.data));
      } catch {
        /* ignore malformed frame */
      }
    };
    ws.onclose = () => {
      if (live) setTimeout(open, 1500);
    };
    ws.onerror = () => ws && ws.close();
  };
  open();

  return () => {
    live = false;
    if (ws) ws.close();
  };
}
