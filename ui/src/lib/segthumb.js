// The tint under the picked choice of a `.seg` (cw | ccw, mm | inches...)
// slides to the new one instead of jumping (2026-10-28). The tint is the
// `.seg`'s own `::before`, placed from `--thumb-x` / `--thumb-w`: nothing is
// added to the DOM Svelte owns. Buttons differ in width ("Balance
// regulation" | "Manual"), so the place is measured, not a fraction.

/** Where the tint goes: the first picked (`.on`) button's left and width,
 *  or null when none is picked (a choice still to make). Plain objects with
 *  `offsetLeft`, `offsetWidth` and `classList` will do. */
export function thumbOf(buttons) {
  for (const b of buttons) {
    if (b.classList.contains('on')) return { x: b.offsetLeft, w: b.offsetWidth };
  }
  return null;
}

/** Svelte action on a `.seg`: keeps its tint under the picked button, and
 *  slides it when the pick changes. The first placement, and the one after
 *  nothing was picked, do not slide: the tint appears there. */
export function segThumb(node) {
  let shown = false;
  const place = () => {
    const t = thumbOf(node.querySelectorAll(':scope > button'));
    if (!t) {
      node.classList.remove('thumbed');
      shown = false;
      return;
    }
    if (!shown) node.classList.add('thumb-still');
    node.style.setProperty('--thumb-x', `${t.x}px`);
    node.style.setProperty('--thumb-w', `${t.w}px`);
    node.classList.add('thumbed');
    if (!shown) {
      shown = true;
      // let the browser draw it in place, then slide from there on
      requestAnimationFrame(() => requestAnimationFrame(() => node.classList.remove('thumb-still')));
    }
  };
  // A pick changes a button's class. The `.seg`'s own class is ours (place
  // sets it): reacting to it looped forever and froze the page.
  const mo = new MutationObserver((records) => {
    if (records.some((r) => r.target !== node || r.type === 'childList')) place();
  });
  mo.observe(node, { subtree: true, childList: true, attributes: true, attributeFilter: ['class'] });
  const ro = typeof ResizeObserver === 'function' ? new ResizeObserver(place) : null;
  ro?.observe(node);
  place();
  return {
    destroy() {
      mo.disconnect();
      ro?.disconnect();
    },
  };
}
