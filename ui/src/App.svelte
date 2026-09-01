<script>
  import { app } from './lib/state.svelte.js';
  import { get, connectWs } from './lib/api.js';
  import Overview from './routes/Overview.svelte';
  import NewRun from './routes/NewRun.svelte';
  import History from './routes/History.svelte';
  import Settings from './routes/Settings.svelte';
  import ResumeModal from './components/ResumeModal.svelte';

  const nav = [
    ['overview', 'Overview', '▦'],
    ['new', 'New run', '＋'],
    ['history', 'History', '≣'],
    ['settings', 'Settings', '⚙'],
  ];

  $effect(() => {
    get('/api/status')
      .then((s) => {
        app.status = s;
        app.connected = true;
      })
      .catch(() => {});
    return connectWs((s) => {
      app.status = s;
      app.connected = true;
    });
  });

  // fetch recovery details once when the daemon reports one
  let recovery = $state(null);
  let recoveryAsked = $state(false);
  $effect(() => {
    const pending = app.status?.has_pending_recovery;
    if (pending && !recoveryAsked) {
      recoveryAsked = true;
      get('/api/recovery').then((r) => (recovery = r)).catch(() => {});
    }
    if (!pending) {
      recovery = null;
      recoveryAsked = false;
    }
  });

  function setTheme() {
    const root = document.documentElement;
    const dark = root.dataset.theme
      ? root.dataset.theme === 'dark'
      : matchMedia('(prefers-color-scheme: dark)').matches;
    root.dataset.theme = dark ? 'light' : 'dark';
  }
</script>

<div class="app">
  <aside class="rail">
    <div class="brand">
      <span class="mark" aria-hidden="true"></span>
      <span class="word">Fermentool</span>
    </div>

    <nav aria-label="Sections">
      {#each nav as [id, label, icon]}
        <button class="nav-item" class:active={app.route === id} onclick={() => (app.route = id)}>
          <span class="ic" aria-hidden="true">{icon}</span>{label}
        </button>
      {/each}
    </nav>

    <div class="rail-foot">
      <span class="status" class:on={app.connected}>
        <span class="dot" aria-hidden="true"></span>
        {app.connected ? 'daemon online' : 'reconnecting…'}
      </span>
      <button class="theme" onclick={setTheme} title="Toggle theme">◐</button>
    </div>
  </aside>

  <main class="main">
    <div class="wrap">
      {#if app.route === 'overview'}
        <Overview />
      {:else if app.route === 'new'}
        <NewRun />
      {:else if app.route === 'history'}
        <History />
      {:else if app.route === 'settings'}
        <Settings />
      {/if}
    </div>
  </main>
</div>

{#if recovery}
  <ResumeModal info={recovery} ondone={() => (recovery = null)} />
{/if}

<style>
  .app { display: grid; grid-template-columns: 200px 1fr; min-height: 100vh; }

  .rail {
    display: flex;
    flex-direction: column;
    gap: var(--s-6);
    padding: var(--s-6) var(--s-3);
    background: var(--surface);
    border-right: 1px solid var(--line-soft);
  }
  .brand { display: flex; align-items: center; gap: var(--s-2); padding: 0 var(--s-2); }
  .mark {
    width: 26px; height: 26px; border-radius: 7px; flex: none;
    background: linear-gradient(135deg, var(--teal-700), var(--green-500));
  }
  .word { font-weight: 640; letter-spacing: -0.01em; }

  nav { display: flex; flex-direction: column; gap: 1px; }
  .nav-item {
    display: flex; align-items: center; gap: var(--s-3);
    padding: var(--s-2) var(--s-3);
    border-radius: 8px;
    color: var(--muted);
    background: transparent; border: none;
    text-align: left; width: 100%; cursor: pointer;
  }
  .nav-item .ic { width: 15px; text-align: center; opacity: 0.8; font-size: 12px; }
  .nav-item:hover { background: var(--surface-sunken); color: var(--ink); }
  .nav-item.active {
    background: color-mix(in srgb, var(--teal-700) 11%, var(--surface));
    color: var(--teal-700); font-weight: 600;
  }

  .rail-foot {
    margin-top: auto;
    padding: 0 var(--s-2);
    display: flex; align-items: center; justify-content: space-between; gap: var(--s-2);
  }
  .status { display: inline-flex; align-items: center; gap: var(--s-2); font-size: 12px; color: var(--muted); }
  .status .dot { width: 7px; height: 7px; border-radius: 50%; background: var(--muted); flex: none; }
  .status.on { color: var(--green-600); }
  .status.on .dot { background: var(--green-500); }
  .theme {
    background: transparent; border: 1px solid var(--line); color: var(--muted);
    border-radius: 999px; width: 26px; height: 26px; padding: 0; font-size: 13px;
  }
  .theme:hover { color: var(--ink); }

  .main { padding: var(--s-7) var(--s-8) var(--s-8); }
  .wrap { max-width: 1000px; margin: 0 auto; }

  @media (max-width: 820px) {
    .app { grid-template-columns: 1fr; }
    .rail {
      flex-direction: row; align-items: center;
      border-right: none; border-bottom: 1px solid var(--line-soft);
      overflow-x: auto;
    }
    .rail-foot { margin-top: 0; }
    .main { padding: var(--s-5) var(--s-4) var(--s-7); }
  }
</style>
