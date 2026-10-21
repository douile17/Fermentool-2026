import { describe, expect, it } from 'vitest';
import { besideLine, crossesCurve, overlaps, placeLabel, textBox } from './chartlabels.js';

describe('textBox', () => {
  it('measures a mono text from its anchor', () => {
    expect(textBox(100, 20, 'abcd', 'start', 10)).toEqual({ x: 100, y: 12, w: 24, h: 10 });
    expect(textBox(100, 20, 'abcd', 'end', 10).x).toBe(76);
    expect(textBox(100, 20, 'abcd', 'middle', 10).x).toBe(88);
  });
});

describe('overlaps / crossesCurve', () => {
  it('tells boxes apart', () => {
    expect(overlaps({ x: 0, y: 0, w: 10, h: 10 }, { x: 11, y: 0, w: 10, h: 10 })).toBe(true); // within 2 px
    expect(overlaps({ x: 0, y: 0, w: 10, h: 10 }, { x: 20, y: 0, w: 10, h: 10 })).toBe(false);
  });
  it('sees a curve through a box', () => {
    const box = { x: 10, y: 50, w: 40, h: 10 };
    expect(crossesCurve(box, () => 55)).toBe(true);
    expect(crossesCurve(box, () => 100)).toBe(false);
    expect(crossesCurve(box, null)).toBe(false);
  });
});

describe('the refill label on the Overview chart', () => {
  // The screenshot of 2026-10-21: 650 x 240, a curve rising 6 -> 9 ml/min,
  // its end value written at the top right, the next refill at x = 515.
  const W = 650;
  const H = 240;
  const y = (v) => 218 - (v / 10) * 202;
  const yAt = (px) => (px < 0 || px > W ? null : y(6 + (3 * px) / W));
  const endLabel = textBox(W - 6, y(9) - 9, '9.000 ml/min', 'end');
  const startLabel = textBox(6, y(6) + 15, '6.000 ml/min', 'start');
  const taken = [endLabel, startLabel];

  it('no longer sits on the end value: it goes to the left of its line', () => {
    const at = placeLabel(besideLine(515, 'refill 13:45', { top: 26, bottom: 212, W }), { taken, yAt, W, H });
    expect(at).not.toBeNull();
    expect(overlaps(at.box, endLabel)).toBe(false);
    expect(crossesCurve(at.box, yAt)).toBe(false);
    expect(at.anchor).toBe('end');
  });

  it('keeps the right side when it is free', () => {
    const at = placeLabel(besideLine(200, 'refill 13:45', { top: 26, bottom: 212, W }), { taken, yAt, W, H });
    expect(at.anchor).toBe('start');
    expect(at.y).toBe(26);
  });

  it('leaves the text out rather than overlap when nothing is clear', () => {
    const everything = [{ x: 0, y: 0, w: W, h: H }];
    expect(placeLabel(besideLine(300, 'refill 13:45', { top: 26, bottom: 212, W }), { taken: everything, W, H })).toBeNull();
  });
});
