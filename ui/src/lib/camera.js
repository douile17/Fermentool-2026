// The camera, in words: its state from GET /api/camera, the PC's cameras
// from GET /api/camera/devices. Shared by the Camera page and Settings >
// Camera (and the timelapse after them).
import { shortTime } from './fmt.js';

/** The picture sizes the daemon accepts (config.rs `CAMERA_SIZES`). */
const SIZES = [
  [640, 480],
  [1280, 720],
  [1920, 1080],
];

const sizeLabel = ([w, h]) => `${w} × ${h}`;

/** The camera's state, `{ tone: 'ok'|'warn'|'idle', word, detail }`, from
 *  GET /api/camera. Nothing heard yet reads as not connected. */
export function cameraLine(cam) {
  switch (cam?.state) {
    case 'connected':
      return { tone: 'ok', word: 'Connected', detail: `${cam.device}, ${sizeLabel([cam.width, cam.height])}` };
    case 'connecting':
      return { tone: 'idle', word: 'Connecting…', detail: cam.device ?? '' };
    case 'in_use':
      return { tone: 'warn', word: 'Camera in use', detail: 'Another program holds it (Teams, Camera). Close it, then Connect.' };
    case 'lost':
      return { tone: 'warn', word: 'Camera not answering', detail: `Since ${shortTime(cam.since)}. The run is not affected.` };
    case 'no_helper':
      return { tone: 'warn', word: 'Camera helper missing', detail: 'Reinstall Fermentool.' };
    default:
      // off: saved but disconnected, or none saved yet
      return { tone: 'idle', word: 'Not connected', detail: cam?.configured ? 'Connect to see the picture.' : 'No camera chosen.' };
  }
}

/** The live view is worth opening only while the camera is connected (lost,
 *  the stream would repeat its last picture as if live) and the page is
 *  `visible` (`document.visibilityState`): a hidden tab or a minimised
 *  window would keep the camera grabbing at 15 fps for days. */
export function liveShown(cam, visible) {
  return cam?.state === 'connected' && visible === true;
}

/** A shown live view counts as live this long after its stream was mounted
 *  before the daemon must count it as a viewer and send pictures. */
export const LIVE_GRACE_MS = 5000;
/** A picture older than this, while a viewer is counted, is a frozen one
 *  (the daemon grabs at 15 fps for a viewer). */
const FRAME_STALE_MS = 4000;

/** Is a shown live view really live: `'live'`, `'starting'` or `'stale'`?
 *  `cam` is the last GET /api/camera, `pollOk` whether the last poll
 *  answered, `shownSince` when the stream was mounted (ms). A stream that
 *  ended (the daemon stopped or restarted) keeps its last picture without
 *  an error, so the page tells it from the daemon's side: live only while
 *  the daemon counts a viewer and its latest picture is fresh; starting for
 *  `LIVE_GRACE_MS` after a mount; stale after that, or once the polls fail. */
export function liveState(cam, nowMs, pollOk, shownSince) {
  if (!pollOk || !cam) return 'stale';
  const at = cam.frame_at ? Date.parse(cam.frame_at) : NaN;
  if (cam.viewers > 0 && Number.isFinite(at) && nowMs - at <= FRAME_STALE_MS) return 'live';
  return nowMs - shownSince < LIVE_GRACE_MS ? 'starting' : 'stale';
}

/** In the picture box while the camera gives none: since when, if lost. */
export function noPictureSince(cam) {
  return cam?.state === 'lost' && cam.since ? `No picture since ${shortTime(cam.since)}` : 'No picture';
}

/** Svelte action for every live `<img>` (`use:liveSrc={url}`): sets its
 *  source, and blanks it when the image goes, so the stream's request is
 *  aborted before removal. Chromium can keep a multipart/x-mixed-replace
 *  download alive after the element is gone, and the daemon would keep
 *  counting a viewer (and grabbing). */
export function liveSrc(node, src) {
  node.setAttribute('src', src);
  return {
    update(next) {
      node.setAttribute('src', next);
    },
    destroy() {
      node.removeAttribute('src');
    },
  };
}

/** One camera of the list, as a key for a `<select>`: two identical webcams
 *  share a name and differ by their index. */
export function deviceKey(name, index) {
  return `${index}:${name}`;
}

/** A camera's name, numbered "(n)" (n = index + 1) when another camera of
 *  the list has the same name. */
export function deviceLabel(dev, all) {
  const twins = (all ?? []).filter((d) => d.name === dev.name).length;
  return twins > 1 ? `${dev.name} (${dev.index + 1})` : dev.name;
}

/** The standard sizes `dev` offers, smallest first, as
 *  `{ label: '1280 × 720', size: [1280, 720] }`. All three while no camera
 *  of the list is picked (the saved one unplugged): the daemon accepts them. */
export function sizesFor(dev) {
  const offered = dev ? SIZES.filter(([w, h]) => (dev.sizes ?? []).some(([a, b]) => a === w && b === h)) : SIZES;
  return offered.map((s) => ({ label: sizeLabel(s), size: [...s] }));
}

/** How many cameras the PC has, in a sentence. */
export function camerasFound(n) {
  if (!n) return 'No camera on this PC.';
  return `${n} camera${n === 1 ? '' : 's'} on this PC.`;
}

/** The run's time over the picture: "+11 h 52 m 07 s", "+4 m 07 s". */
export function runClock(seconds) {
  let s = Math.max(0, Math.floor(seconds || 0));
  const h = Math.floor(s / 3600);
  s -= h * 3600;
  const m = Math.floor(s / 60);
  s -= m * 60;
  const ss = String(s).padStart(2, '0');
  return h ? `+${h} h ${String(m).padStart(2, '0')} m ${ss} s` : `+${m} m ${ss} s`;
}

// --- Picture controls (Settings > Camera, 2026-11-05) ----------------------
// What GET /api/camera/controls returns, one entry per control the camera
// has: `{ control, min, max, step, default, value, can_auto, auto }`, in the
// camera's own units. Every change is set on the camera and saved at once.

/** Live view pictures a second offered (the daemon takes 1 to 15). */
export const FPS_CHOICES = [5, 10, 15];

const CONTROL_LABEL = {
  exposure: 'Exposure',
  white_balance: 'White balance',
  focus: 'Focus',
  brightness: 'Brightness',
  contrast: 'Contrast',
  saturation: 'Saturation',
  sharpness: 'Sharpness',
  gain: 'Gain',
  power_line: 'Anti-flicker',
};

export function controlLabel(name) {
  return CONTROL_LABEL[name] ?? name;
}

const POWER_LINE = [
  { value: 0, label: 'Off' },
  { value: 1, label: '50 Hz' },
  { value: 2, label: '60 Hz' },
  { value: 3, label: 'Auto' },
];

/** Anti-flicker is a choice, not a slider: the camera's values among off,
 *  50 Hz, 60 Hz and auto. */
export function powerLineOptions(c) {
  return POWER_LINE.filter((o) => o.value >= c.min && o.value <= c.max);
}

/** A value in words: exposure as a shutter time (Windows gives log2 of
 *  seconds, -6 is 1/64 s), white balance in kelvin. */
export function valueText(name, value) {
  if (name === 'exposure') return value < 0 ? `1/${2 ** -value} s` : `${2 ** value} s`;
  if (name === 'white_balance') return `${value} K`;
  if (name === 'power_line') return POWER_LINE.find((o) => o.value === value)?.label ?? String(value);
  return String(value);
}

/** What the row shows on the right. In auto only "Auto": the camera
 *  reports its last fixed value there, not the one it has chosen. */
export function controlText(c) {
  return c.auto ? 'Auto' : valueText(c.control, c.value);
}

/** Away from the camera's own default (the row's reset arrow shows). */
export function controlChanged(c) {
  return c.can_auto ? !c.auto : c.value !== c.default;
}

/** One change, said in the status line: "Exposure 1/32 s", "Exposure auto". */
export function changeText(v) {
  const label = controlLabel(v.control);
  return v.auto ? `${label} auto` : `${label} ${valueText(v.control, v.value)}`;
}

/** Every control with an auto mode is fixed (and there is one). */
export function pictureLocked(controls) {
  const auto = (controls ?? []).filter((c) => c.can_auto);
  return auto.length > 0 && auto.every((c) => !c.auto);
}

/** "Lock the picture": what to send. Locking fixes each automatic control at
 *  its value; unlocking hands each fixed one back to the camera. */
export function lockPlan(controls) {
  const auto = (controls ?? []).filter((c) => c.can_auto);
  const lock = !pictureLocked(controls);
  return auto
    .filter((c) => c.auto === lock)
    .map((c) => ({ control: c.control, value: c.value, auto: !lock }));
}

/** Undo: what to send to bring the controls of `after` back to `before`. */
export function restorePlan(before, after) {
  const now = new Map((after ?? []).map((c) => [c.control, c]));
  return (before ?? [])
    .filter((b) => {
      const a = now.get(b.control);
      return a && (a.auto !== b.auto || (!b.auto && a.value !== b.value));
    })
    .map((b) => ({ control: b.control, value: b.value, auto: b.auto }));
}

/** At most one call of `send` per `ms` while a slider moves, the latest
 *  value always sent: `push(v)` keeps it, `flush()` sends it now, `cancel()`
 *  drops it. */
export function throttle(send, ms) {
  let last = 0;
  let timer = null;
  let pending;
  const fire = () => {
    timer = null;
    last = Date.now();
    const v = pending;
    pending = undefined;
    send(v);
  };
  return {
    push(v) {
      pending = v;
      if (timer) return;
      timer = setTimeout(fire, Math.max(0, last + ms - Date.now()));
    },
    flush() {
      if (timer) clearTimeout(timer);
      timer = null;
      if (pending !== undefined) fire();
    },
    cancel() {
      if (timer) clearTimeout(timer);
      timer = null;
      pending = undefined;
    },
  };
}
