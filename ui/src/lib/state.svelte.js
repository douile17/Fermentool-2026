// Shared reactive app state (Svelte 5 runes).
export const app = $state({
  /** Latest DaemonStatus from /api/status or the WebSocket. */
  status: null,
  /** WebSocket / API reachable. */
  connected: false,
  /** top-level section from the sidebar: 'pump' | 'settings' */
  route: 'pump',
  /** sub-view within the pump panel: 'overview' | 'new' | 'history' | 'calibration' */
  tab: 'overview',
  /** Settings section picked in its side menu, see routes/settings/sections.js */
  settingsSection: 'pumps',
  /** Form seed for NewRun, set by "Run again" in History. Consumed once. */
  prefill: null,
  /** Settings, Display: tube sizes led by 'mm' or 'in' (config.toml [display]). */
  tubeSizes: 'mm',
  /** A page holding work asks before it is left: { active(), title, lines,
   *  leave() } set by the page (TubingCalibration), read by App's navigation. */
  leaveGuard: null,
});
