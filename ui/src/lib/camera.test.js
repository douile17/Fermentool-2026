import { describe, expect, it } from 'vitest';
import { cameraLine, camerasFound, deviceKey, deviceLabel, liveShown, liveSrc, noPictureSince, runClock, sizesFor } from './camera.js';
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
    expect(liveShown({ state: 'connected' })).toBe(true);
    for (const state of ['off', 'connecting', 'in_use', 'lost', 'no_helper']) expect(liveShown({ state })).toBe(false);
    expect(liveShown(null)).toBe(false);
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
