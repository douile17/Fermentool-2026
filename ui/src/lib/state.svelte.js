// Shared reactive app state (Svelte 5 runes).
export const app = $state({
  /** Latest DaemonStatus from /api/status or the WebSocket. */
  status: null,
  /** WebSocket / API reachable. */
  connected: false,
  /** top-level section from the sidebar: 'pump' | 'settings' */
  route: 'pump',
  /** sub-view within the pump panel: 'overview' | 'new' | 'history' */
  tab: 'overview',
  /** Form seed for NewRun, set by "Run again" in History. Consumed once. */
  prefill: null,
});
