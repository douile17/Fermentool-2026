import { describe, expect, it, vi } from 'vitest';
import {
  cameraLine,
  pictureSaved,
  camerasFound,
  changeText,
  controlChanged,
  controlLabel,
  controlText,
  lockPlan,
  pictureLocked,
  powerLineOptions,
  restorePlan,
  throttle,
  valueText,
  deviceKey,
  deviceLabel,
  LIVE_GRACE_MS,
  liveShown,
  liveSrc,
  liveState,
  noPictureSince,
  runClock,
  sizesFor,
} from './camera.js';
import { shortTime } from './fmt.js';

describe('cameraLine', () => {
  it('connected: the camera and its size', () => {
    expect(cameraLine({ state: 'connected', device: 'Logitech C920', width: 1280, height: 720, configured: true })).toEqual({
      tone: 'ok',
      word: 'Connected',
      detail: 'Logitech C920, 1280 × 720',
    });
  });
  it('connecting', () => {
    const l = cameraLine({ state: 'connecting', device: 'Logitech C920', configured: true });
    expect(l.tone).toBe('idle');
    expect(l.word).toBe('Connecting…');
  });
  it('in use by another program', () => {
    expect(cameraLine({ state: 'in_use', device: 'Logitech C920', configured: true })).toEqual({
      tone: 'warn',
      word: 'Camera in use',
      detail: 'Another program holds it (Teams, Camera). Close it, then Connect.',
    });
  });
  it('lost: since when, and the run goes on', () => {
    const since = '2026-11-03T09:41:00Z';
    expect(cameraLine({ state: 'lost', device: 'Logitech C920', since, configured: true })).toEqual({
      tone: 'warn',
      word: 'Camera not answering',
      detail: `Since ${shortTime(since)}. The run is not affected.`,
    });
  });
  it('off, no camera saved', () => {
    expect(cameraLine({ state: 'off', configured: false })).toEqual({ tone: 'idle', word: 'Not connected', detail: 'No camera chosen.' });
  });
  it('off, a camera saved but disconnected', () => {
    expect(cameraLine({ state: 'off', configured: true })).toEqual({
      tone: 'idle',
      word: 'Not connected',
      detail: 'Connect to see the picture.',
    });
  });
  it('no helper next to the daemon', () => {
    expect(cameraLine({ state: 'no_helper', configured: false })).toEqual({
      tone: 'warn',
      word: 'Camera helper missing',
      detail: 'Reinstall Fermentool.',
    });
  });
  it('nothing heard yet reads as not connected', () => {
    expect(cameraLine(null).word).toBe('Not connected');
  });
});

describe('deviceLabel', () => {
  const twins = [
    { name: 'Logitech C920', index: 0, sizes: [] },
    { name: 'Logitech C920', index: 1, sizes: [] },
    { name: 'Integrated Webcam', index: 0, sizes: [] },
  ];
  it('numbers two cameras of the same name', () => {
    expect(deviceLabel(twins[1], twins)).toBe('Logitech C920 (2)');
    expect(deviceLabel(twins[0], twins)).toBe('Logitech C920 (1)');
  });
  it('a camera alone keeps its bare name', () => {
    expect(deviceLabel(twins[2], twins)).toBe('Integrated Webcam');
  });
});

describe('sizesFor', () => {
  it('the standard sizes the camera offers, smallest first', () => {
    const dev = { name: 'Integrated Webcam', index: 0, sizes: [[1280, 720], [800, 600], [640, 480]] };
    expect(sizesFor(dev)).toEqual([
      { label: '640 × 480', size: [640, 480] },
      { label: '1280 × 720', size: [1280, 720] },
    ]);
  });
  it('all three for a camera that offers them all', () => {
    const dev = { name: 'Logitech C920', index: 0, sizes: [[1920, 1080], [1280, 720], [640, 480]] };
    expect(sizesFor(dev).map((s) => s.label)).toEqual(['640 × 480', '1280 × 720', '1920 × 1080']);
  });
  it('all three while no camera is listed (saved one unplugged, list not back yet)', () => {
    expect(sizesFor(null).map((s) => s.label)).toEqual(['640 × 480', '1280 × 720', '1920 × 1080']);
  });
});

describe('liveShown', () => {
  it('only while the camera is connected', () => {
    expect(liveShown({ state: 'connected' }, true)).toBe(true);
    for (const state of ['off', 'connecting', 'in_use', 'lost', 'no_helper']) expect(liveShown({ state }, true)).toBe(false);
    expect(liveShown(null, true)).toBe(false);
  });
  it('never while the page is hidden (a hidden tab must not keep the camera grabbing)', () => {
    expect(liveShown({ state: 'connected' }, false)).toBe(false);
    expect(liveShown({ state: 'connected' })).toBe(false);
  });
});

describe('liveState', () => {
  const now = Date.parse('2026-11-03T10:00:00Z');
  const fed = { state: 'connected', viewers: 1, frame_at: '2026-11-03T09:59:59.800Z' };
  const shownSince = now - 60_000;

  it('live while a viewer is counted and the pictures are fresh', () => {
    expect(liveState(fed, now, true, shownSince)).toBe('live');
    // the daemon's own format, nanoseconds
    expect(liveState({ ...fed, frame_at: '2026-11-03T09:59:59.800123456Z' }, now, true, shownSince)).toBe('live');
    expect(liveState(fed, now, true, now)).toBe('live');
  });
  it('stale once the polls fail (the daemon stopped)', () => {
    expect(liveState(fed, now, false, shownSince)).toBe('stale');
    expect(liveState(fed, now, false, now)).toBe('stale');
  });
  it('stale when the daemon counts no viewer (restarted: the stream ended on its last picture)', () => {
    expect(liveState({ ...fed, viewers: 0, frame_at: null }, now, true, shownSince)).toBe('stale');
    expect(liveState({ ...fed, viewers: 0 }, now, true, shownSince)).toBe('stale');
  });
  it('stale when the latest picture is a few seconds old', () => {
    expect(liveState({ ...fed, frame_at: '2026-11-03T09:59:54Z' }, now, true, shownSince)).toBe('stale');
    expect(liveState({ ...fed, frame_at: null }, now, true, shownSince)).toBe('stale');
  });
  it('starting during the grace after the stream is mounted (not counted, no picture yet), never live', () => {
    const fresh = { state: 'connected', viewers: 0, frame_at: null };
    expect(liveState(fresh, now, true, now - 1000)).toBe('starting');
    expect(liveState({ ...fed, frame_at: '2026-11-03T09:50:00Z' }, now, true, now - 1000)).toBe('starting');
    expect(liveState(fresh, now, true, now - LIVE_GRACE_MS - 1)).toBe('stale');
  });
  it('nothing heard yet is stale', () => {
    expect(liveState(null, now, true, shownSince)).toBe('stale');
  });
});

describe('runClock', () => {
  it('the run time over the picture', () => {
    expect(runClock(11 * 3600 + 52 * 60 + 7)).toBe('+11 h 52 m 07 s');
    expect(runClock(4 * 60 + 7.9)).toBe('+4 m 07 s');
    expect(runClock(-3)).toBe('+0 m 00 s');
    expect(runClock(NaN)).toBe('+0 m 00 s');
  });
});

describe('camerasFound', () => {
  it('counts the cameras of the PC in words', () => {
    expect(camerasFound(0)).toBe('No camera on this PC.');
    expect(camerasFound(1)).toBe('1 camera on this PC.');
    expect(camerasFound(2)).toBe('2 cameras on this PC.');
  });
});

describe('deviceKey', () => {
  it('tells two cameras of one name apart', () => {
    expect(deviceKey('Logitech C920', 0)).not.toBe(deviceKey('Logitech C920', 1));
    expect(deviceKey('Logitech C920', 1)).toBe(deviceKey('Logitech C920', 1));
  });
});

describe('noPictureSince', () => {
  it('says since when the lost camera gave no picture', () => {
    const since = '2026-11-03T09:41:00Z';
    expect(noPictureSince({ state: 'lost', since })).toBe(`No picture since ${shortTime(since)}`);
  });
  it('a bare "No picture" without a time', () => {
    expect(noPictureSince({ state: 'in_use' })).toBe('No picture');
  });
});

// A fake <img>: its attributes only.
const fakeImg = () => {
  const attrs = new Map();
  return {
    attrs,
    setAttribute: (k, v) => attrs.set(k, String(v)),
    removeAttribute: (k) => attrs.delete(k),
    getAttribute: (k) => (attrs.has(k) ? attrs.get(k) : null),
  };
};

describe('liveSrc', () => {
  it('sets the stream as the source, follows a new one', () => {
    const img = fakeImg();
    const a = liveSrc(img, '/api/camera/live');
    expect(img.getAttribute('src')).toBe('/api/camera/live');
    a.update('/api/camera/live?x=1');
    expect(img.getAttribute('src')).toBe('/api/camera/live?x=1');
  });
  it('blanks the source when the image goes, so the stream is cut before removal', () => {
    const img = fakeImg();
    liveSrc(img, '/api/camera/live').destroy();
    expect(img.getAttribute('src')).toBeNull();
  });
});

describe('picture controls', () => {
  const exp = { control: 'exposure', min: -10, max: -4, step: 1, default: -6, value: -5, can_auto: true, auto: true };
  const wb = { control: 'white_balance', min: 2800, max: 6500, step: 10, default: 4600, value: 4000, can_auto: true, auto: false };
  const br = { control: 'brightness', min: -64, max: 64, step: 1, default: 0, value: 0, can_auto: false, auto: false };
  const pl = { control: 'power_line', min: 0, max: 2, step: 1, default: 1, value: 2, can_auto: false, auto: false };

  it('says values in the words people know', () => {
    expect(valueText('exposure', -6)).toBe('1/64 s');
    expect(valueText('exposure', 1)).toBe('2 s');
    expect(valueText('white_balance', 4600)).toBe('4600 K');
    expect(valueText('power_line', 1)).toBe('50 Hz');
    expect(valueText('brightness', -3)).toBe('-3');
  });
  it('in auto says only Auto (the camera does not report its choice)', () => {
    expect(controlText(exp)).toBe('Auto');
    expect(controlText(wb)).toBe('4000 K');
  });
  it('anti-flicker offers only what the camera has', () => {
    expect(powerLineOptions(pl).map((o) => o.label)).toEqual(['Off', '50 Hz', '60 Hz']);
    expect(powerLineOptions({ ...pl, max: 3 }).map((o) => o.label)).toEqual(['Off', '50 Hz', '60 Hz', 'Auto']);
  });
  it('a control is changed off its default, or fixed where auto exists', () => {
    expect(controlChanged(exp)).toBe(false);
    expect(controlChanged(wb)).toBe(true);
    expect(controlChanged(br)).toBe(false);
    expect(controlChanged({ ...br, value: 4 })).toBe(true);
  });
  it('lock fixes the automatic ones, unlock hands them back', () => {
    expect(pictureLocked([exp, wb, br])).toBe(false);
    expect(lockPlan([exp, wb, br])).toEqual([{ control: 'exposure', value: -5, auto: false }]);
    const locked = [{ ...exp, auto: false }, wb, br];
    expect(pictureLocked(locked)).toBe(true);
    expect(lockPlan(locked)).toEqual([
      { control: 'exposure', value: -5, auto: true },
      { control: 'white_balance', value: 4000, auto: true },
    ]);
    expect(pictureLocked([br])).toBe(false);
    expect(lockPlan([br])).toEqual([]);
  });
  it('undo sends back only what changed', () => {
    const after = [{ ...exp, auto: false, value: -8 }, wb, { ...br, value: 9 }];
    expect(restorePlan([exp, wb, br], after)).toEqual([
      { control: 'exposure', value: -5, auto: true },
      { control: 'brightness', value: 0, auto: false },
    ]);
    // a value moved while in auto is no change
    expect(restorePlan([exp], [{ ...exp, value: -9 }])).toEqual([]);
  });
  it('a change in words', () => {
    expect(changeText({ control: 'exposure', value: -5, auto: false })).toBe('Exposure 1/32 s');
    expect(changeText({ control: 'white_balance', value: 0, auto: true })).toBe('White balance auto');
    expect(controlLabel('power_line')).toBe('Anti-flicker');
  });
});

describe('throttle', () => {
  it('sends at once, then at most once a period, always the latest value', () => {
    vi.useFakeTimers();
    const sent = [];
    const t = throttle((v) => sent.push(v), 100);
    t.push(1);
    vi.advanceTimersByTime(0);
    t.push(2);
    t.push(3);
    vi.advanceTimersByTime(50);
    expect(sent).toEqual([1]);
    vi.advanceTimersByTime(60);
    expect(sent).toEqual([1, 3]);
    t.push(4);
    t.flush();
    expect(sent).toEqual([1, 3, 4]);
    t.flush();
    expect(sent).toEqual([1, 3, 4]);
    t.push(5);
    t.cancel();
    vi.advanceTimersByTime(500);
    expect(sent).toEqual([1, 3, 4]);
    vi.useRealTimers();
  });
});

describe('pictureSaved', () => {
  // "Saved in today's pictures." said nothing of where (2026-11-16)
  it("names the picture's folder", () => {
    expect(pictureSaved('Pictures 2026-11-16')).toBe('Saved in Pictures 2026-11-16.');
    expect(pictureSaved('Night')).toBe('Saved in Night.');
  });
  it('says saved without a folder', () => {
    expect(pictureSaved(null)).toBe('Saved.');
  });
});
