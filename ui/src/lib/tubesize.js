// Tube sizes: always stored in mm, shown in mm and in inches (the leading
// unit set in Settings, Display), grouped by bore and coloured by it.

export const MM_PER_IN = 25.4;

/** A tube's name: brand and reference, then its lot, then the lab's own
 *  reference. "Masterflex 06409-14 · lot 2409 (bench 2)"; a calibration
 *  from before brands and references reads by its lot alone. */
export function tubeLabel(c) {
  const head = [c.brand, c.product_ref].filter(Boolean).join(' ');
  const lot = head ? `${head} · lot ${c.tubing_lot_id}` : c.tubing_lot_id;
  return `${lot}${c.internal_ref ? ` (${c.internal_ref})` : ''}`;
}

/** Brands offered while typing, before the ones this PC already used. */
export const KNOWN_BRANDS = ['Boxer', 'Cole-Parmer', 'Gilson', 'Ismatec', 'Masterflex', 'PharMed', 'Tygon', 'VWR', 'Watson-Marlow'];

// Inches as tubing bags print them, a decimal: 0.063". A size within 0.06 mm
// of a 1/64" step is shown at that step, so 3.2 mm (the metric name of 1/8")
// reads 0.125", not 0.126", and 4.8 mm (3/16") reads 0.188".
export function inches(mm) {
  let inch = mm / MM_PER_IN;
  const n = Math.round(inch * 64);
  if (n > 0 && Math.abs(n / 64 - inch) * MM_PER_IN < 0.06) inch = n / 64;
  return `${inch.toFixed(3)}″`;
}

// The same size as a fraction, 1/16", for the search only.
function fraction(mm) {
  const inch = mm / MM_PER_IN;
  for (const den of [2, 4, 8, 16, 32, 64]) {
    const num = Math.round(inch * den);
    if (num > 0 && Math.abs(num / den - inch) * MM_PER_IN < 0.06) {
      let [a, b] = [num, den];
      while (a % 2 === 0 && b % 2 === 0) [a, b] = [a / 2, b / 2];
      return `${a}/${b}`;
    }
  }
  return '';
}

const r1 = (x) => Math.round(Number(x) * 10) / 10;

/** "0.8 / 1.6 mm" */
export const mmSize = (inner, outer) => `${r1(inner).toFixed(1)} / ${r1(outer).toFixed(1)} mm`;
/** "0.031″ × 0.063″" */
export const inSize = (inner, outer) => `${inches(inner)} × ${inches(outer)}`;

/** The leading and the second unit of a tube's size, for `lead` "mm" | "in". */
export function sizeParts(inner, outer, lead) {
  const mm = mmSize(inner, outer);
  const inch = inSize(inner, outer);
  return lead === 'in' ? { pri: inch, sec: mm } : { pri: mm, sec: inch };
}

/** One diameter alone, the inner one of a bore chip. */
export function diaParts(mm, lead) {
  const m = `${r1(mm).toFixed(1)} mm`;
  return lead === 'in' ? { pri: inches(mm), sec: m } : { pri: m, sec: inches(mm) };
}

// Two tubes share a bore when both diameters match to 0.1 mm: 1.59 mm (typed
// from 1/16") and 1.6 mm are the same tube.
export const boreKey = (inner, outer) => `${r1(inner)}|${r1(outer)}`;

// A bore's colour comes from the bore itself, never from its place in a list:
// the usual inner bores of peristaltic tubing each own one of eight pastels,
// (0.020", 1/32", 1/16", 3/32", 1/8", 3/16", 1/4", 5/16"), any other takes one
// from its value. Classes .bore-c0 to .bore-c7 are in app.css.
const USUAL = [0.5, 0.8, 1.6, 2.4, 3.2, 4.8, 6.4, 7.9];
export function boreClass(inner, outer) {
  const i = USUAL.indexOf(r1(inner));
  if (i >= 0) return `bore-c${i}`;
  let h = 0;
  for (const ch of boreKey(inner, outer)) h = (h * 31 + ch.charCodeAt(0)) >>> 0;
  return `bore-c${h % 8}`;
}

/** A typed size: "1/16", "0.0625", ".062", '0.063"', "1.6". NaN otherwise. */
export function parseSize(text) {
  const v = String(text ?? '').trim().replace(/[″"]|in(ch(es)?)?$/gi, '').trim();
  const m = v.match(/^(\d+)\s*\/\s*(\d+)$/);
  if (m) return Number(m[2]) > 0 ? Number(m[1]) / Number(m[2]) : NaN;
  return v === '' ? NaN : Number(v);
}

/** Does a search text name this bore? A number below 1 is read as inches
 *  (0.063, .062), from 1 up as mm (1.6), a fraction as inches (1/16). */
export function sizeMatches(text, inner, outer) {
  const v = parseSize(text);
  if (!(v > 0)) return false;
  const near = (mm) => (v < 1 ? Math.abs(mm / MM_PER_IN - v) < 0.0015 : Math.abs(mm - v) < 0.05);
  return near(inner) || near(outer) || `${fraction(inner)} ${fraction(outer)}`.includes(String(text).trim());
}

/** The usual tube sizes, inner and outer, in inches. */
export const COMMON_SIZES = [
  [1 / 32, 1 / 16],
  [1 / 16, 1 / 8],
  [1 / 8, 3 / 16],
  [3 / 16, 1 / 4],
  [1 / 4, 3 / 8],
];
