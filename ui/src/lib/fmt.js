// Small formatting helpers.

/** Dates and times in English like the rest of the UI ("Mon 19 Oct,
 *  11:04", 24 h), whatever the PC's own language: they came out as
 *  "lun. 19 oct." on a French Windows. */
export const LOCALE = 'en-GB';

export function num(x, digits = 1) {
  if (x == null || Number.isNaN(x)) return '–';
  return Number(x).toFixed(digits);
}

/** "31 h 12 m", "4 m 10 s", "12 s" */
export function dur(seconds) {
  let s = Math.max(0, Math.floor(seconds || 0));
  const h = Math.floor(s / 3600);
  s -= h * 3600;
  const m = Math.floor(s / 60);
  s -= m * 60;
  if (h) return `${h} h ${m} m`;
  if (m) return `${m} m ${s} s`;
  return `${s} s`;
}

/** "031:12:04" clock */
export function clock(seconds) {
  let s = Math.max(0, Math.floor(seconds || 0));
  const h = Math.floor(s / 3600);
  s -= h * 3600;
  const m = Math.floor(s / 60);
  s -= m * 60;
  const p = (n, w = 2) => String(n).padStart(w, '0');
  return `${p(h, 3)}:${p(m)}:${p(s)}`;
}

export function shortTime(iso) {
  if (!iso) return '–';
  const d = new Date(iso);
  return Number.isNaN(d.getTime())
    ? '–'
    : d.toLocaleTimeString(LOCALE, { hour: '2-digit', minute: '2-digit' });
}

/** "4 Sep 10:04", date + time, for run stamps that can span days. */
export function stamp(iso) {
  if (!iso) return '–';
  const d = new Date(iso);
  return Number.isNaN(d.getTime())
    ? '–'
    : d.toLocaleString(LOCALE, {
        day: 'numeric',
        month: 'short',
        hour: '2-digit',
        minute: '2-digit',
      });
}

/** "4 Sep 2026, 10:04", full stamp with year, for history where runs age. */
export function stampY(iso) {
  if (!iso) return '–';
  const d = new Date(iso);
  return Number.isNaN(d.getTime())
    ? '–'
    : d.toLocaleString(LOCALE, {
        day: 'numeric',
        month: 'short',
        year: 'numeric',
        hour: '2-digit',
        minute: '2-digit',
      });
}

export function elapsedSeconds(startedAtIso) {
  const t0 = Date.parse(startedAtIso);
  return Number.isNaN(t0) ? 0 : (Date.now() - t0) / 1000;
}

export const RPM_LIMITS = { min: 0.1, max: 350 };
export const FLOW_LIMITS = { min: 0, max: 99999 };
export const unitFor = (cv) => (cv === 'ml_min' ? 'ml/min' : 'rpm');

/** Display precision the pump uses for a control variable: ml/min shows 3
 *  decimals, rpm shows 1 (its 0.1 motor step). */
export const digitsFor = (cv) => (cv === 'ml_min' ? 3 : 1);

/** A commanded-volume total, mL: "482.3 mL" below 1 L, "1.204 L" at or above
 *  it, so the readout never grows into an unwieldy number of digits. */
export function vol(ml) {
  if (ml == null || Number.isNaN(ml)) return '–';
  return Math.abs(ml) >= 1000 ? `${num(ml / 1000, 3)} L` : `${num(ml, 1)} mL`;
}

/** What a run actually ran, for History: `{ ran, of }`, "49 m 22 s" of
 *  "8 h 0 m". The planned duration alone said 8 h for a run stopped after
 *  2 minutes. A completed run ran its whole curve, `of` is null; a running
 *  one counts to `nowMs`. */
export function ranFor(run, nowMs = Date.now()) {
  const t0 = Date.parse(run.started_at);
  const t1 = run.ended_at ? Date.parse(run.ended_at) : nowMs;
  const ran = Number.isNaN(t0) || Number.isNaN(t1) ? null : Math.max(0, (t1 - t0) / 1000);
  if (ran == null) return { ran: dur(run.duration_s), of: null };
  if (run.status === 'completed') return { ran: dur(ran), of: null };
  return { ran: dur(ran), of: `of ${dur(run.duration_s)}${run.status === 'running' ? ', so far' : ''}` };
}

/** Where `path` sits inside `folder`, as it would be written there
 *  ("fermentool.sqlite", "logs\"): null when it is not inside it. Either
 *  separator, any case (Windows paths). `isFolder` adds the separator. */
export function insideFolder(path, folder, isFolder = false) {
  if (!path || !folder) return null;
  const sep = path.includes('\\') ? '\\' : '/';
  const norm = (p) => p.replace(/[\\/]+$/, '');
  const f = norm(folder);
  const p = norm(path);
  if (p.length <= f.length || p.slice(0, f.length).toLowerCase() !== f.toLowerCase()) return null;
  if (!/[\\/]/.test(p[f.length])) return null;
  const rest = p.slice(f.length + 1);
  return rest ? rest + (isFolder ? sep : '') : null;
}
