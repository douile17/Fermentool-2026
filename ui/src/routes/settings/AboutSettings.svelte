<script>
  import { get, post } from '../../lib/api.js';
  import { LOCALE, insideFolder } from '../../lib/fmt.js';
  import Icon from '../../components/Icon.svelte';
  import SettingsCard from './SettingsCard.svelte';

  let { group } = $props();

  const uiVersion = __UI_VERSION__;

  let about = $state(null);
  let err = $state(null);
  // The location being opened, and a refusal to say under its row.
  let opening = $state(null);
  let openErr = $state(null);

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

  // The daemon opens it in Explorer, on this PC: a page cannot. A folder
  // opens as itself, a file's folder opens with the file picked.
  async function open(key) {
    opening = key;
    openErr = null;
    try {
      await post('/api/about/open', { which: key });
    } catch (e) {
      openErr = { key, message: e.message };
    }
    opening = null;
  }

  // What the data folder holds, each by its own name in it ("config.toml"),
  // or its whole path when a setting moved it elsewhere.
  const files = $derived(
    about
      ? [
          { key: 'db', icon: 'database', name: 'History', path: about.db, file: true,
            what: 'Runs, journal, calibrations. The file to back up.' },
          { key: 'config', icon: 'file-text', name: 'Settings', path: about.config, file: true,
            what: 'Everything set on these Settings pages.' },
          { key: 'logs', icon: 'folder', name: 'Logs', path: about.log_dir, file: false,
            what: 'One file a day, what the daemon did. Useful when something went wrong.' },
        ].map((f) => ({ ...f, shown: insideFolder(f.path, about.data_dir, !f.file) ?? f.path }))
      : [],
  );
</script>

<SettingsCard {group} {err} loading={!about && !err}>
  {#if about}
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
      <dd title={new Date(about.started_at_ms).toLocaleString(LOCALE)}>
        {since(about.started_at_ms)}, since {new Date(about.started_at_ms).toLocaleString(LOCALE)}
      </dd>
    </dl>
  {/if}
</SettingsCard>

{#if about}
  <SettingsCard
    group="Files"
  >
    <div class="root">
      <span class="ic"><Icon name="folder" size={22} /></span>
      <span class="who">
        <span class="name">Data folder</span>
        <code>{about.data_dir}</code>
      </span>
      {@render openBtn('data', false)}
    </div>
    {#if openErr?.key === 'data'}<p class="err-line">{openErr.message}</p>{/if}
    <ul class="files">
      {#each files as f (f.key)}
        <li>
          <span class="ic"><Icon name={f.icon} size={20} /></span>
          <span class="who">
            <span class="name">{f.name}</span> <code>{f.shown}</code>
            <span class="what">{f.what}</span>
            {#if openErr?.key === f.key}<span class="err-line">{openErr.message}</span>{/if}
          </span>
          {@render openBtn(f.key, f.file)}
        </li>
      {/each}
    </ul>
  </SettingsCard>
{/if}

<!-- A folder opens as itself; a file's folder opens with the file picked. -->
{#snippet openBtn(key, file)}
  <button class="btn-ghost open" type="button" disabled={opening === key} onclick={() => open(key)}
    title={file ? 'Open its folder in Explorer, with the file selected' : 'Open it in Explorer'}>
    <Icon name={file ? 'search' : 'external-link'} size={14} />{file ? 'Show in folder' : 'Open folder'}
  </button>
{/snippet}


<style>
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
  .warn { color: var(--danger-ink); }
  code { font-family: var(--mono); font-size: 12px; color: var(--muted); overflow-wrap: anywhere; }

  /* the data folder, once; what it holds below it, every button in one column */
  .root {
    display: flex; align-items: center; gap: var(--s-3);
    padding: var(--s-3) var(--s-4); border-radius: var(--radius-ctl); background: var(--surface-sunken);
  }
  .root .ic { color: var(--teal-700); }
  .who { flex: 1; min-width: 0; }
  .root .name { display: block; font-weight: 600; }
  .root code { display: block; }
  .files {
    list-style: none; margin: var(--s-2) 0 0 calc(var(--s-4) + 10px); padding: 0 0 0 var(--s-4);
    border-left: 2px solid var(--line);
  }
  /* right padding as the folder's grey box: every button on one edge */
  .files li { display: flex; align-items: center; gap: var(--s-3); padding: var(--s-3) var(--s-4) var(--s-3) 0; }
  .files li + li { border-top: 1px solid var(--line-soft); }
  .files .ic { color: var(--muted); }
  .files .name { font-weight: 500; }
  .what { display: block; font-size: 12.5px; color: var(--muted); }
  .open {
    flex: none; width: 136px; display: inline-flex; align-items: center; justify-content: center; gap: 6px;
    padding: 5px var(--s-2); font-size: 12.5px; white-space: nowrap;
  }
  .err-line { display: block; font-size: 12px; color: var(--danger-ink); margin: var(--s-1) 0 0; }
  @media (max-width: 760px) {
    .facts { grid-template-columns: minmax(0, 1fr); gap: var(--s-1); }
    dd { margin-bottom: var(--s-3); }
  }
  @media (max-width: 560px) {
    .root, .files li { flex-wrap: wrap; }
    .files { margin-left: var(--s-2); padding-left: var(--s-3); }
    .open { width: auto; }
  }
</style>
