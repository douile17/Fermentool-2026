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
      return { tone: 'idle', word: 'Not connected', detail: 'No camera chosen.' };
  }
}

/** The live view is worth opening only while the camera is connected: lost,
 *  the stream would repeat its last picture as if live. */
export function liveShown(cam) {
  return cam?.state === 'connected';
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
