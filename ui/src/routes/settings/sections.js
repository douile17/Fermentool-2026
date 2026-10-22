import PumpsSettings from './PumpsSettings.svelte';
import BalanceSettings from './BalanceSettings.svelte';
import RunSettings from './RunSettings.svelte';
import NotificationSettings from './NotificationSettings.svelte';
import DaemonSettings from './DaemonSettings.svelte';
import DisplaySettings from './DisplaySettings.svelte';
import AboutSettings from './AboutSettings.svelte';

// The Settings side menu, in display order. A new section is one component
// (built on SettingsCard, saving only its own fields through patchConfig)
// plus one line here; `group` is the menu heading it sits under, `icon` a
// name from components/Icon.svelte ('pump' draws the pump-head glyph).
export const SECTIONS = [
  // the cable and every pump on it (feed, refill), one section (2026-10-21)
  { id: 'pumps', label: 'Pumps', group: 'Hardware', icon: 'pump', component: PumpsSettings },
  { id: 'balance', label: 'Balance', group: 'Hardware', icon: 'weight', component: BalanceSettings },
  { id: 'runs', label: 'Crash resume', group: 'Runs', icon: 'power', component: RunSettings },
  { id: 'notifications', label: 'Notifications', group: 'Alerts', icon: 'bell', component: NotificationSettings },
  { id: 'display', label: 'Display', group: 'System', icon: 'ruler', component: DisplaySettings },
  { id: 'daemon', label: 'Daemon', group: 'System', icon: 'server', component: DaemonSettings },
  { id: 'about', label: 'About', group: 'System', icon: 'info', component: AboutSettings },
];

export const GROUPS = [...new Set(SECTIONS.map((s) => s.group))];
