<script>
  import { get } from '../../lib/api.js';
  import SettingsCard from './SettingsCard.svelte';

  let { group } = $props();

  const uiVersion = __UI_VERSION__;

  let about = $state(null);
  let err = $state(null);
  let copied = $state(null);

  $effect(() => {
    get('/api/about')
      .then((a) => (about = a))
      .catch((e) => (err = e.message));
  });

  // "Name <mail>" (Cargo's authors format) into its two halves.
  function person(s) {
    const m = /^(.*?)\s*<([^>]+)>$/.exec(s);
    return m ? { name: m[1], mail: m[2] } : { name: s, mail: null };
  }
  const authors = $derived((about?.authors ?? []).filter(Boolean).map(person));
  // A tab opened before an update keeps the old UI in memory.
  const stale = $derived(about && about.version !== uiVersion);

  function since(ms) {
    const s = Math.max(0, Math.floor((Date.now() - ms) / 1000));
    const d = Math.floor(s / 86400);
    const h = Math.floor((s % 86400) / 3600);
    const m = Math.floor((s % 3600) / 60);
    return d ? `${d} d ${h} h` : h ? `${h} h ${m} min` : `${m} min`;
  }

  async function copy(key, text) {
    try {
      await navigator.clipboard.writeText(text);
      copied = key;
      setTimeout(() => copied === key && (copied = null), 1500);
    } catch {
      copied = null;
    }
  }

  const paths = $derived(
    about
      ? [
          ['data', 'Data folder', about.data_dir],
          ['db', 'History (runs, journal, calibrations)', about.db],
          ['config', 'Settings file', about.config],
          ['logs', 'Logs', about.log_dir],
        ]
      : [],
  );
</script>

<SettingsCard {group} title="Fermentool" {err} loading={!about && !err}>
  {#if about}
    <p class="field-note intro">
      Drives a LabQ peristaltic pump along a feed profile, with gravimetric trim, crash-safe journalling and alerts.
    </p>
    <dl class="facts">
      <dt>Version</dt>
      <dd><span class="num">{about.version}</span></dd>

      {#if stale}
        <dt>This page</dt>
        <dd class="warn">
          <span class="num">{uiVersion}</span>: an older copy of the interface. Reload with Ctrl+F5.
        </dd>
      {/if}

      <dt>{authors.length > 1 ? 'Developers' : 'Developer'}</dt>
      <dd>
        {#each authors as a, i}
          {#if i}<br />{/if}
          {a.name}{#if a.mail}, <a href="mailto:{a.mail}">{a.mail}</a>{/if}
        {/each}
      </dd>

      {#if about.homepage}
        <dt>Website</dt>
        <dd><a href={about.homepage} target="_blank" rel="noopener">{about.homepage.replace(/^https?:\/\//, '')}</a></dd>
      {/if}

      <dt>Licence</dt>
      <dd>{about.license}</dd>

      <dt>Daemon running for</dt>
      <dd title={new Date(about.started_at_ms).toLocaleString()}>
        {since(about.started_at_ms)}, since {new Date(about.started_at_ms).toLocaleString()}
      </dd>
    </dl>
  {/if}
</SettingsCard>

{#if about}
  <SettingsCard
    group="Files"
    title="Where the data lives"
    intro="On this PC only. Back up the history file to keep the runs; an update never touches it."
  >
    <dl class="facts">
      {#each paths as [key, label, path]}
        <dt>{label}</dt>
        <dd class="path">
          <code>{path}</code>
          <button class="btn-ghost copy" onclick={() => copy(key, path)}>{copied === key ? 'Copied' : 'Copy'}</button>
        </dd>
      {/each}
    </dl>
  </SettingsCard>
{/if}

<style>
  .intro { margin-bottom: var(--s-4); }
  .facts {
    display: grid;
    grid-template-columns: minmax(120px, max-content) minmax(0, 1fr);
    gap: var(--s-3) var(--s-6);
    margin: 0;
    font-size: 13px;
  }
  dt { color: var(--muted); }
  dd { margin: 0; color: var(--ink); overflow-wrap: anywhere; }
  .num { font-variant-numeric: tabular-nums; font-weight: 600; }
  .warn { color: var(--danger); }
  .path { display: flex; align-items: center; gap: var(--s-3); flex-wrap: wrap; }
  code { font-size: 12px; overflow-wrap: anywhere; }
  .copy { padding: 2px var(--s-3); font-size: 12px; }
  @media (max-width: 760px) {
    .facts { grid-template-columns: minmax(0, 1fr); gap: var(--s-1); }
    dd { margin-bottom: var(--s-3); }
  }
</style>
