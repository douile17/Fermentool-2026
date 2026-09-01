// Shared reactive app state (Svelte 5 runes).
export const app = $state({
  /** Latest DaemonStatus from /api/status or the WebSocket. */
  status: null,
  /** WebSocket / API reachable. */
  connected: false,
  /** current route id */
  route: 'overview',
});
