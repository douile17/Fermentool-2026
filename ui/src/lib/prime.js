// Priming the feed line from the tubing calibration (2026-10-28): the pump
// at its top speed, the way the calibration turns, until Stop priming or the
// daemon's 5 min limit (`POST /api/pump/prime`, `status.priming`). Asked
// after a measurement pumped the wrong way and emptied the line back into
// the bottle. Plain functions of the status, for the band in Measurements.

/** "1:04", minutes and two-digit seconds, for the priming count. */
export function mmss(s) {
  const t = Math.max(0, Math.floor(s ?? 0));
  return `${Math.floor(t / 60)}:${String(t % 60).padStart(2, '0')}`;
}

/** Seconds the line has been primed the current way, from the status's
 *  `priming` (any window's), or null when it is not priming. */
export function primeElapsedS(priming, nowMs) {
  if (!priming) return null;
  const t = (nowMs - Date.parse(priming.since)) / 1000;
  return Number.isFinite(t) ? Math.max(0, t) : 0;
}

/** The band's state: 'run' while priming; 'done' once this page stopped it
 *  (`primedS`, until the next measurement); 'ask' when a measurement went
 *  the wrong way since the last priming (`lineEmptied`); 'plain' else. */
export function primeBand({ priming, primedS, lineEmptied }) {
  if (priming) return 'run';
  if (primedS != null) return 'done';
  if (lineEmptied) return 'ask';
  return 'plain';
}
