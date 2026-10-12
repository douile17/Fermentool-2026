// Tube sizes (tubesize.js): mm and inches as the bags print them, bores
// grouped and coloured, sizes found by search however they are typed.
import { describe, expect, it } from 'vitest';
import { boreClass, boreKey, inches, parseSize, sizeMatches, sizeParts, tubeLabel } from './tubesize.js';

describe('inches, as decimals', () => {
  it('a usual size reads its standard value, not the raw conversion', () => {
    expect(inches(0.8)).toBe('0.031″');
    expect(inches(1.6)).toBe('0.063″');
    expect(inches(3.2)).toBe('0.125″'); // not 0.126
    expect(inches(4.8)).toBe('0.188″'); // the metric name of 3/16"
    expect(inches(6.4)).toBe('0.250″');
  });
  it('an odd size reads its own value', () => {
    expect(inches(0.5)).toBe('0.020″');
  });
});

describe('sizes shown in the leading unit', () => {
  it('mm first, inches beside', () => {
    expect(sizeParts(0.8, 1.6, 'mm')).toEqual({ pri: '0.8 / 1.6 mm', sec: '0.031″ × 0.063″' });
  });
  it('inches first, mm beside', () => {
    expect(sizeParts(0.8, 1.6, 'in')).toEqual({ pri: '0.031″ × 0.063″', sec: '0.8 / 1.6 mm' });
  });
});

describe('bores', () => {
  it('a size typed from 1/16" and one typed in mm are the same bore', () => {
    expect(boreKey(1.59, 3.18)).toBe(boreKey(1.6, 3.2));
  });
  it('the usual bores each keep their own colour', () => {
    const classes = [0.5, 0.8, 1.6, 2.4, 3.2, 4.8, 6.4, 7.9].map((i) => boreClass(i, i * 2));
    expect(new Set(classes).size).toBe(8);
    expect(boreClass(0.8, 1.6)).toBe(boreClass(0.79, 1.59));
  });
});

describe('typing a size', () => {
  it('reads a fraction, a decimal, a bare decimal point or a mark', () => {
    expect(parseSize('1/16')).toBe(0.0625);
    expect(parseSize('0.0625"')).toBe(0.0625);
    expect(parseSize('.062')).toBe(0.062);
    expect(parseSize('1.6')).toBe(1.6);
    expect(parseSize('abc')).toBeNaN();
    expect(parseSize('')).toBeNaN();
  });
  it('finds a bore by inches (under 1) or mm (from 1), as a fraction or a decimal', () => {
    for (const q of ['0.063', '.062', '1/16', '1.6']) expect(sizeMatches(q, 1.6, 3.2)).toBe(true);
    expect(sizeMatches('0.031', 0.8, 1.6)).toBe(true);
    expect(sizeMatches('0.031', 1.6, 3.2)).toBe(false);
    expect(sizeMatches('baguette', 0.8, 1.6)).toBe(false);
  });
});

describe('a tube by its name', () => {
  it('brand and reference, then the lot, then the lab reference', () => {
    expect(tubeLabel({ brand: 'Masterflex', product_ref: '06409-14', tubing_lot_id: '2409', internal_ref: 'bench 2' }))
      .toBe('Masterflex 06409-14 · lot 2409 (bench 2)');
  });
  it('a calibration from before brands and references: its lot alone', () => {
    expect(tubeLabel({ tubing_lot_id: 'baguette' })).toBe('baguette');
  });
});
