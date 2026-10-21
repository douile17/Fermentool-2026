// Text labels on a chart (Chart.svelte) that never sit on one another nor
// on the curve: each label tries its places in order and takes the first
// that is clear. "refill 13:45" once printed over the curve's end value
// "9.000 ml/min" (the Overview, a rising curve with its next refill near
// the end).

/** IBM Plex Mono's advance, as a share of the font size. */
const ADVANCE = 0.6;

/** The box of a text drawn at baseline (x, y): `{ x, y, w, h }`, its top
 *  left corner and size. `anchor` as in SVG: start, middle or end. */
export function textBox(x, y, text, anchor = 'start', size = 10.5) {
  const w = String(text).length * size * ADVANCE;
  const x0 = anchor === 'end' ? x - w : anchor === 'middle' ? x - w / 2 : x;
  return { x: x0, y: y - size * 0.8, w, h: size };
}

/** Whether two boxes come closer than `pad` px. */
export const overlaps = (a, b, pad = 2) =>
  a.x < b.x + b.w + pad && b.x < a.x + a.w + pad && a.y < b.y + b.h + pad && b.y < a.y + a.h + pad;

/** Whether the curve, `yAt(px)` (null where it is not drawn), passes
 *  through the box, `pad` px around it included. */
export function crossesCurve(box, yAt, pad = 2) {
  if (!yAt) return false;
  for (let px = box.x - pad; px <= box.x + box.w + pad; px += 2) {
    const y = yAt(px);
    if (y != null && y >= box.y - pad && y <= box.y + box.h + pad) return true;
  }
  return false;
}

/** The first of `candidates` (`{ x, y, text, anchor, size }`) that lies
 *  inside `[0, W] x [0, H]`, clear of every box in `taken` and of the
 *  curve: the candidate with its `box`, or null when none is clear (the
 *  caller then leaves the text out rather than draw it over another). */
export function placeLabel(candidates, { taken = [], yAt = null, W = Infinity, H = Infinity } = {}) {
  for (const c of candidates) {
    const box = textBox(c.x, c.y, c.text, c.anchor, c.size);
    if (box.x < 0 || box.x + box.w > W || box.y < 0 || box.y + box.h > H) continue;
    if (taken.some((t) => overlaps(box, t))) continue;
    if (crossesCurve(box, yAt)) continue;
    return { ...c, box };
  }
  return null;
}

/** Places beside a vertical line at `lineX` for a label: its preferred
 *  side first (right, or left near the right edge), then the other, on
 *  rows from the top of the plot down. */
export function besideLine(lineX, text, { top, bottom, W, size = 10.5, step = 14 }) {
  const preferLeft = lineX > W - 110;
  const sides = preferLeft ? ['end', 'start'] : ['start', 'end'];
  const out = [];
  for (let y = top; y <= bottom; y += step) {
    for (const anchor of sides) out.push({ x: lineX + (anchor === 'end' ? -5 : 5), y, text, anchor, size });
  }
  return out;
}
