import { get, put } from './api.js';

// Each Settings section owns a few fields of config.toml and saves only
// those. It merges them onto the current server config rather than
// round-tripping a snapshot taken when the page loaded: the connection bar
// (serial.path, pump.address) and the other sections may have saved since.

export function loadConfig() {
  return get('/api/config');
}

/** Fetch the live config, let `apply` edit it, PUT it back. The config
 *  carries the revision it was read at (`_rev`): if something else saved in
 *  between (another page, Connect), the daemon refuses with 409 and the edit
 *  is applied once more onto the fresh config. */
export async function patchConfig(apply) {
  for (let attempt = 0; ; attempt++) {
    const cfg = await get('/api/config');
    apply(cfg);
    try {
      const res = await put('/api/config', cfg);
      return { cfg, res };
    } catch (e) {
      if (e.status !== 409 || attempt > 0) throw e;
    }
  }
}
