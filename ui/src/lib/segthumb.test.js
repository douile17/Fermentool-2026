import { describe, expect, it } from 'vitest';
import { thumbOf } from './segthumb.js';

const btn = (offsetLeft, offsetWidth, on = false) => ({
  offsetLeft,
  offsetWidth,
  classList: { contains: (c) => c === 'on' && on },
});

describe('thumbOf', () => {
  it('sits under the picked button, whatever the widths', () => {
    expect(thumbOf([btn(0, 160), btn(160, 72, true)])).toEqual({ x: 160, w: 72 });
    expect(thumbOf([btn(0, 60, true), btn(60, 60)])).toEqual({ x: 0, w: 60 });
  });
  it('is nowhere while nothing is picked', () => {
    expect(thumbOf([btn(0, 60), btn(60, 60)])).toBeNull();
    expect(thumbOf([])).toBeNull();
  });
});
