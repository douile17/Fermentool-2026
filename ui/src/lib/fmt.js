// Small formatting helpers.

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
    : d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
}

/** "4 Sep 10:04" — date + time, for run stamps that can span days. */
export function stamp(iso) {
  if (!iso) return '–';
  const d = new Date(iso);
  return Number.isNaN(d.getTime())
    ? '–'
    : d.toLocaleString(undefined, {
        day: 'numeric',
        month: 'short',
        hour: '2-digit',
        minute: '2-digit',
      });
}

/** "4 Sep 2026, 10:04" — full stamp with year, for history where runs age. */
export function stampY(iso) {
  if (!iso) return '–';
  const d = new Date(iso);
  return Number.isNaN(d.getTime())
    ? '–'
    : d.toLocaleString(undefined, {
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
