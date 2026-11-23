// A run's images: where they go (New run's "Images saved in") and how they
// are found and played afterwards (History's Images section, the player).
// Plain functions, tested in tlplayer.test.js.

/** Two Windows folders are the same one: any case, `/` or `\`, a trailing
 *  separator or not. */
export function sameFolder(a, b) {
  if (a == null || b == null) return false;
  const norm = (p) => String(p).trim().replace(/\//g, '\\').replace(/\\+$/, '').toLowerCase();
  return norm(a) === norm(b);
}

/** What New run sends as `timelapse.folder`: nothing when the folder shown
 *  is the one already saved, `''` for the default folder (the setting left
 *  empty), else the folder picked. */
export function folderToSend(shown, saved, def) {
  if (!shown || sameFolder(shown, saved)) return undefined;
  return sameFolder(shown, def) ? '' : shown;
}

const pad = (n) => String(n).padStart(2, '0');

/** The folder a run's images get, as the daemon names it but for the
 *  timelapse number it adds once started: `"2026-11-19_0930 Fed-batch 12"`
 *  (local time, Windows' refused characters as `_`, the name cut to 60). */
export function folderPreview(date, name) {
  const at = `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}_${pad(date.getHours())}${pad(date.getMinutes())}`;
  const clean = Array.from(String(name ?? ''))
    .map((c) => (c < ' ' || '<>:"/\\|?*'.includes(c) ? '_' : c))
    .slice(0, 60)
    .join('')
    .trim()
    .replace(/[. ]+$/, '');
  return clean ? `${at} ${clean}` : at;
}

/** `n` frames spread over the run, the first and the last included, in
 *  order, none twice. */
export function spreadPick(frames, n) {
  const list = frames ?? [];
  if (list.length <= n) return list.slice();
  if (n <= 1) return list.slice(0, Math.max(0, n));
  const out = [];
  for (let k = 0; k < n; k++) out.push(list[Math.round((k * (list.length - 1)) / (n - 1))]);
  return out;
}

/** The player's speeds, images a second. */
export const PLAYER_SPEEDS = [5, 10, 25, 50];

/** One image on (`+1`) or back (`-1`), kept within the run. */
export function stepFrame(i, count, d) {
  if (!(count > 0)) return 0;
  return Math.min(count - 1, Math.max(0, i + d));
}

/** Where Play starts: from the start again once at the end. */
export function playFrom(i, count) {
  return i >= count - 1 ? 0 : i;
}

/** "the whole run in 2 min", "the whole run in 24 s". */
export function wholeRunWords(count, fps) {
  const s = Math.max(1, Math.round(count / fps));
  if (s < 60) return `the whole run in ${s} s`;
  const m = Math.floor(s / 60);
  const r = s % 60;
  return `the whole run in ${m} min${r ? ` ${r} s` : ''}`;
}

/** A frame's time in the run: "42 h 15 min", "35 min", "0 min". */
export function frameClock(elapsedS) {
  const m = Math.max(0, Math.floor((elapsedS || 0) / 60));
  if (m < 60) return `${m} min`;
  return `${Math.floor(m / 60)} h ${pad(m % 60)} min`;
}

/** When an image was taken, local time: "Tue 18 Nov 2026, 14:32:05"; ''
 *  unknown. */
export function frameStamp(takenAt) {
  const d = takenAt ? new Date(takenAt) : null;
  if (!d || Number.isNaN(d.getTime())) return '';
  const wd = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'][d.getDay()];
  const mo = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'][d.getMonth()];
  return `${wd} ${d.getDate()} ${mo} ${d.getFullYear()}, ${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}

/** What the run had fed at that image: "1 410 mL fed", or '' unknown.
 *  Not the setpoint: the one kept with an image is the pump's, corrected
 *  by the balance (6.032 on a 6.330 ml/min run), which read as the rate. */
export function frameFigures(f) {
  if (f?.delivered_ml == null) return '';
  return `${Math.round(f.delivered_ml).toLocaleString('en-GB').replace(/,/g, ' ')} mL fed`;
}
