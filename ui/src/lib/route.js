// Deep links: `#settings/<section>` opens Settings on that section,
// `#<tab>` a tab of the pump panel (overview, history, calibration). Read
// once at start; an unknown link opens the app as usual. Lets a page be
// bookmarked, and the UX audit capture a Settings section directly.

const TABS = ['overview', 'history', 'calibration'];

/** `{ route, tab?, section? }` for a location hash, or null. `sections`:
 *  the Settings section ids (routes/settings/sections.js). */
export function parseHash(hash, sections) {
  const h = String(hash ?? '').replace(/^#\/?/, '').toLowerCase();
  if (!h) return null;
  const [head, sub] = h.split('/');
  if (head === 'settings') return sub && sections.includes(sub) ? { route: 'settings', section: sub } : { route: 'settings' };
  if (TABS.includes(head)) return { route: 'pump', tab: head };
  return null;
}
