// Timelapses in words: estimates, the Camera page card's state, the disk.
// From GET /api/timelapses/current (`{ timelapse, warn_free }`) and
// GET /api/camera. Shared by New run, Settings > Camera and the Camera page.
import { dur } from './fmt.js';

/** A stored image's size by picture size, JPEG 95 (measured 2026-11-05 on
 *  the Integrated Webcam: 255 kB at 1280 × 720, 98 kB at 640 × 480; no
 *  camera here offers 1920 × 1080, estimated at 2.25 × 720p). */
export const BYTES_PER_IMAGE = { '640x480': 98_000, '1280x720': 255_000, '1920x1080': 575_000 };

/** Images and bytes of a timelapse every `intervalMin` over `durationS`
 *  (the first image at the start), and per day. `avgBytes`: the real
 *  average once images exist. `null` until both are known. */
export function estimate(durationS, intervalMin, width, height, avgBytes = null) {
  if (!(durationS > 0) || !(intervalMin > 0)) return null;
  const each = avgBytes ?? BYTES_PER_IMAGE[`${width}x${height}`] ?? BYTES_PER_IMAGE['1280x720'];
  const images = Math.floor(durationS / (intervalMin * 60)) + 1;
  const perDay = Math.floor(86400 / (intervalMin * 60));
  return { images, bytes: images * each, perDay: { images: perDay, bytes: perDay * each } };
}

const twoFigures = (x) => Number(Number(x).toPrecision(2));
const spaced = (n) => String(n).replace(/\B(?=(\d{3})+(?!\d))/g, ' ');

/** "14 kB", "310 MB", "1.2 GB": two figures. */
export function bytesText(n) {
  const b = Math.max(0, n || 0);
  if (b < 1e6) return `${twoFigures(b / 1e3)} kB`;
  if (b < 1e9) return `${twoFigures(b / 1e6)} MB`;
  return `${twoFigures(b / 1e9)} GB`;
}

/** "about 1 200 images, 310 MB" */
export function estimateText(e) {
  if (!e) return '';
  return `about ${spaced(twoFigures(e.images))} images, ${bytesText(e.bytes)}`;
}

/** "288 images, 73 MB a day" */
export function perDayText(e) {
  if (!e) return '';
  return `${spaced(e.perDay.images)} images, ${bytesText(e.perDay.bytes)} a day`;
}

/** "in 3 m 20 s", "now", "–" without a next image. */
export function nextInText(nextAt, nowMs) {
  const at = nextAt ? Date.parse(nextAt) : NaN;
  if (!Number.isFinite(at)) return '–';
  const s = (at - nowMs) / 1000;
  return s <= 0 ? 'now' : `in ${dur(Math.ceil(s))}`;
}

/** The Camera page card: `none`, `recording_run`, `recording`, `waiting`
 *  (the camera), `paused` or `awaiting` (the run waits for Resume). A camera
 *  lost or held elsewhere shows as waiting before a slot even fails. */
export function cardState(cur, cam) {
  if (!cur) return 'none';
  if (cur.state === 'paused') return 'paused';
  if (cur.state === 'awaiting_resume') return 'awaiting';
  if (cur.state === 'waiting_camera' || ['lost', 'in_use'].includes(cam?.state)) return 'waiting';
  return cur.kind === 'run' ? 'recording_run' : 'recording';
}

/** Why the disk or the folder matters now, or `null`. */
export function freeWarning(cur, warnFree) {
  if (!cur) return null;
  if (cur.state === 'paused' && cur.pause_reason === 'disk') return 'Paused: under 200 MB free on this disk';
  if (cur.state === 'paused' && cur.pause_reason === 'folder') return 'Paused: the folder cannot be written to';
  return warnFree ? 'Under 1 GB free on this disk' : null;
}
