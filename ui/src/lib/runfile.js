// A run's settings as a file: exported from History or New run, imported
// back into New run on this PC or another one. Only the New run form fields
// travel; nothing in here is trusted until `readSettings` has checked it.

const TAG = 'fermentool-run-settings';
const VERSION = 1;

// Every New run field a file may set, with the check its value must pass.
const isNum = (v) => typeof v === 'number' && Number.isFinite(v);
const numOrNull = (v) => v == null || isNum(v);
const durPart = (v) => (isNum(v) || (typeof v === 'string' && /^\d*$/.test(v)));
const FIELDS = {
  name: (v) => typeof v === 'string',
  control_var: (v) => v === 'ml_min' || v === 'rpm',
  direction: (v) => v === 'cw' || v === 'ccw',
  duration_h: durPart,
  duration_m: durPart,
  duration_s: durPart,
  kind: (v) => ['linear', 'exponential', 'sigmoid', 'constant'].includes(v),
  mode: (v) => v === 'endpoints' || v === 'physio',
  start: isNum,
  end: isNum,
  value: isNum,
  mu_per_hour: isNum,
  steepness: isNum,
  fb_x0: numOrNull,
  fb_v0: numOrNull,
  fb_yxs: numOrNull,
  fb_sf: numOrNull,
  fb_ms: numOrNull,
  fb_vmax: numOrNull,
  gravimetric_trim: (v) => typeof v === 'boolean',
  tubing_calibration_id: (v) => v == null || Number.isInteger(v),
  responsible: (v) => typeof v === 'string',
};

function fileName(name) {
  const base = String(name ?? '').trim().replace(/[^\w.-]+/g, '_') || 'run';
  return `${base}.fermentool-run.json`;
}

/** Download `settings` (New run form fields) as a JSON file. */
export function downloadSettings(settings) {
  const picked = {};
  for (const k of Object.keys(FIELDS)) if (k in settings) picked[k] = settings[k];
  const body = { format: TAG, version: VERSION, exported_at: new Date().toISOString(), settings: picked };
  const url = URL.createObjectURL(new Blob([JSON.stringify(body, null, 2)], { type: 'application/json' }));
  const a = document.createElement('a');
  a.href = url;
  a.download = fileName(picked.name);
  a.click();
  // Not at once: some browsers drop a download whose URL is gone before it starts.
  setTimeout(() => URL.revokeObjectURL(url), 10_000);
}

/** Parse an exported file's text: the known, valid fields only, or throws. */
export function readSettings(text) {
  let body;
  try {
    body = JSON.parse(text);
  } catch {
    throw new Error('not a JSON file');
  }
  if (body?.format !== TAG || typeof body.settings !== 'object' || body.settings == null) {
    throw new Error('not a Fermentool run settings file');
  }
  if (body.version > VERSION) {
    throw new Error('made by a newer Fermentool, update this one first');
  }
  const out = {};
  const bad = [];
  for (const [k, ok] of Object.entries(FIELDS)) {
    if (!(k in body.settings)) continue;
    if (ok(body.settings[k])) out[k] = body.settings[k];
    else bad.push(k);
  }
  if (bad.length) throw new Error(`invalid value for ${bad.join(', ')}`);
  return out;
}
