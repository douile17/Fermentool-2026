import { get, put } from './api.js';

// Each Settings section owns a few fields of config.toml and saves only
// those. It merges them onto the current server config rather than
// round-tripping a snapshot taken when the page loaded: the connection bar
// (serial.path, pump.address) and the other sections may have saved since.

export function loadConfig() {
  return get('/api/config');
}

/** Fetch the live config, let `apply` edit it, PUT it back. */
export async function patchConfig(apply) {
  const cfg = await get('/api/config');
  apply(cfg);
  const res = await put('/api/config', cfg);
  return { cfg, res };
}
