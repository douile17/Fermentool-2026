<script>
  import { app } from './lib/state.svelte.js';
  import { get, connectWs } from './lib/api.js';
  import Overview from './routes/Overview.svelte';
  import NewRun from './routes/NewRun.svelte';
  import History from './routes/History.svelte';
  import Settings from './routes/Settings.svelte';
  import PumpHead from './components/PumpHead.svelte';
  import ConnBar from './components/ConnBar.svelte';
  import ResumeModal from './components/ResumeModal.svelte';
  import FinishModal from './components/FinishModal.svelte';
  import { linkState } from './lib/link.js';

  const pumpLink = $derived(linkState(app.status, app.connected));

  // Flat sidebar: "Pump" up top, "Settings" pinned to the foot. Pump's
  // sub-views are tabs on the main panel, not a nested menu.
  // "New run" isn't a tab — Overview already offers it (idle empty state,
  // completed run) and History's "Run again" jumps straight to it.
  const pumpTabs = [
    ['overview', 'Overview'],
    ['history', 'History'],
  ];

  const running = $derived(!!app.status?.active);

  $effect(() => {
    if (running && app.tab === 'new') app.tab = 'overview';
  });

  // Sliding indicator for the tab bar: track the active button's box.
  let tabsEl = $state(null);
  let ind = $state({ x: 0, w: 0 });
  let indReady = $state(false);

  function measureTab() {
    if (!tabsEl) return;
    const on = tabsEl.querySelector('.tab.on');
    // No selected tab (e.g. the hidden 'new' view): collapse the indicator
    // rather than leaving it frozen under the previous tab.
    if (!on) {
      ind = { x: ind.x, w: 0 };
      return;
    }
    ind = { x: on.offsetLeft, w: on.offsetWidth };
    indReady = true;
  }

  $effect(() => {
    void app.tab;
    if (!tabsEl) return;
    requestAnimationFrame(measureTab);
  });

  $effect(() => {
    const onResize = () => measureTab();
    window.addEventListener('resize', onResize);
    return () => window.removeEventListener('resize', onResize);
  });

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

  // "run complete" pop-up: shows once per completed run per browser session.
  const readFinishAck = () => {
    try {
      return Number(sessionStorage.getItem('ft-finish-ack')) || null;
    } catch {
      return null;
    }
  };
  let finishAck = $state(readFinishAck());
  const hold = $derived(app.status?.holding ?? null);
  const showFinish = $derived(!!hold && !app.status?.active && hold.run_id !== finishAck);
  function dismissFinish() {
    if (hold) {
      try {
        sessionStorage.setItem('ft-finish-ack', String(hold.run_id));
      } catch {
        /* private mode: modal just won't re-suppress across reloads */
      }
      finishAck = hold.run_id;
    }
  }

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
      <button
        class="nav-item"
        class:active={app.route === 'pump'}
        class:running
        onclick={() => (app.route = 'pump')}
      >
        <span class="ic pump-ic" aria-hidden="true"><PumpHead size={18} spin={running} frac={running ? 0.4 : 0} /></span>Pump control
      </button>
    </nav>

    <button
      class="nav-item nav-settings"
      class:active={app.route === 'settings'}
      onclick={() => (app.route = 'settings')}
    >
      <span class="ic" aria-hidden="true">⚙</span>Settings
    </button>

    <div class="rail-foot">
      {#if app.status}
        <span class="pumpstat {pumpLink.tone}" title={pumpLink.label}>
          <span class="dot" aria-hidden="true"></span>{pumpLink.short}
        </span>
      {/if}
      <div class="rail-foot-row">
        <span class="status" class:on={app.connected}>
          <span class="dot" aria-hidden="true"></span>
          {app.connected ? 'daemon online' : 'reconnecting…'}
        </span>
        <button class="theme" onclick={setTheme} title="Toggle theme">◐</button>
      </div>
    </div>
  </aside>

  <main class="main">
    <div class="wrap">
      <ConnBar />

      {#if app.route === 'settings'}
        <Settings />
      {:else}
        <div
          class="tabs"
          class:ready={indReady}
          role="tablist"
          aria-label="Pump views"
          bind:this={tabsEl}
          style="--ind-x:{ind.x}px; --ind-w:{ind.w}px"
        >
          <span class="tab-ind" aria-hidden="true"></span>
          {#each pumpTabs as [id, label]}
            <button
              class="tab"
              class:on={app.tab === id}
              role="tab"
              aria-selected={app.tab === id}
              onclick={() => (app.tab = id)}
            >{label}</button>
          {/each}
        </div>

        {#if app.tab === 'new'}
          <NewRun />
        {:else if app.tab === 'history'}
          <History />
        {:else}
          <Overview />
        {/if}
      {/if}
    </div>
  </main>
</div>

{#if recovery}
  <ResumeModal info={recovery} ondone={() => (recovery = null)} />
{/if}

{#if showFinish}
  <FinishModal {hold} ondismiss={dismissFinish} />
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
    /* Pin the rail to the viewport so its foot (daemon status, theme
       toggle) stays at the physical bottom instead of the page bottom. */
    position: sticky;
    top: 0;
    align-self: start;
    height: 100vh;
    overflow-y: auto;
  }
  .brand { display: flex; align-items: center; gap: var(--s-2); padding: 0 var(--s-2); }
  .mark {
    width: 28px; height: 28px; flex: none;
    background: center / contain no-repeat url("/favicon.svg");
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
    /* label line-box = icon box, so align-items:center lands them dead level */
    line-height: 18px;
  }
  .nav-item .ic { width: 15px; text-align: center; opacity: 0.8; font-size: 12px; }
  .nav-item .pump-ic {
    width: 18px; height: 18px;
    display: inline-flex; align-items: center; justify-content: center;
    opacity: 0.8;
    /* filled square body in the label colour (green while running); the central
       disc stays hollow — filled with the rail's own background so it reads as
       an empty ring */
    --ph-body: currentColor;
    --ph-fill: var(--surface);
    --ph-line: currentColor;
    --ph-detail: currentColor;
  }
  /* A live run: the glyph goes green (semantic) and spins, so the pump's
     state is visible from any section. Text keeps its normal colour. */
  .nav-item.running .pump-ic {
    opacity: 1;
    --ph-line: var(--green-500);
    --ph-detail: var(--green-500);
  }
  .nav-item:hover { background: var(--surface-sunken); color: var(--ink); }
  .nav-item.active {
    background: color-mix(in srgb, var(--teal-700) 11%, var(--surface));
    color: var(--teal-700); font-weight: 600;
  }
  /* Settings sits apart from Pump, pushed down to just above the rail foot. */
  .nav-settings { margin-top: auto; }

  .rail-foot {
    padding: 0 var(--s-2);
    display: flex; flex-direction: column; align-items: stretch; gap: var(--s-2);
  }
  .rail-foot-row { display: flex; align-items: center; justify-content: space-between; gap: var(--s-2); }
  .status { display: inline-flex; align-items: center; gap: var(--s-2); font-size: 12px; color: var(--muted); }
  .status .dot { width: 7px; height: 7px; border-radius: 50%; background: var(--muted); flex: none; }
  .status.on { color: var(--green-600); }
  .status.on .dot { background: var(--green-500); }

  /* Pump-link line, sits just above "daemon online". Persistent cue once the
     big ConnBar has hidden itself. */
  .pumpstat {
    display: inline-flex; align-items: center; gap: var(--s-2);
    font-size: 12px; font-weight: 600; color: var(--muted);
  }
  .pumpstat .dot { width: 7px; height: 7px; border-radius: 50%; background: var(--muted); flex: none; }
  .pumpstat.ok { color: var(--green-600); }
  .pumpstat.ok .dot { background: var(--green-500); }
  .pumpstat.warn { color: #a2621c; }
  .pumpstat.warn .dot { background: #d68b45; }
  .pumpstat.bad { color: var(--danger); }
  .pumpstat.bad .dot { background: var(--danger); }
  .pumpstat.sim .dot, .pumpstat.idle .dot { background: var(--muted); }
  .theme {
    background: transparent; border: 1px solid var(--line); color: var(--muted);
    border-radius: 999px; width: 26px; height: 26px; padding: 0; font-size: 13px;
  }
  .theme:hover { color: var(--ink); }

  .main { padding: var(--s-7) var(--s-8) var(--s-8); }
  .wrap { max-width: 1000px; margin: 0 auto; }

  /* Segmented control with a sliding teal indicator behind the active tab. */
  .tabs {
    position: relative;
    display: inline-flex;
    gap: 2px;
    padding: 3px;
    margin-bottom: var(--s-6);
    background: var(--surface-sunken);
    border-radius: var(--radius-ctl);
  }
  .tab {
    position: relative;
    z-index: 1;
    border: none;
    background: transparent;
    color: var(--muted);
    font: inherit;
    font-size: 13px;
    font-weight: 500;
    padding: var(--s-2) var(--s-5);
    border-radius: 7px;
    cursor: pointer;
    transition: color 0.2s ease;
  }
  .tab:hover:not(.on) { color: var(--ink); }
  .tab.on { color: var(--teal-700); font-weight: 600; }
  .tab:focus-visible {
    outline: 2px solid color-mix(in srgb, var(--teal-700) 45%, transparent);
    outline-offset: -2px;
  }
  .tab-ind {
    position: absolute;
    top: 3px;
    bottom: 3px;
    left: 0;
    width: var(--ind-w, 0);
    transform: translateX(var(--ind-x, 0));
    background: color-mix(in srgb, var(--teal-700) 13%, var(--surface));
    border-radius: 7px;
  }
  .tabs.ready .tab-ind {
    transition: transform 0.26s cubic-bezier(0.4, 0, 0.2, 1),
      width 0.26s cubic-bezier(0.4, 0, 0.2, 1);
  }
  @media (prefers-reduced-motion: reduce) {
    .tab, .tabs.ready .tab-ind { transition: none; }
  }

  @media (max-width: 820px) {
    .app { grid-template-columns: 1fr; }
    .rail {
      flex-direction: row; align-items: center;
      border-right: none; border-bottom: 1px solid var(--line-soft);
      overflow-x: auto;
      /* back to a normal flow bar on narrow screens */
      position: static;
      height: auto;
      overflow-y: visible;
    }
    .rail-foot { margin-top: 0; }
    .main { padding: var(--s-5) var(--s-4) var(--s-7); }
  }
</style>
