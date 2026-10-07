<script>
  import { app } from './lib/state.svelte.js';
  import { get, post, connectWs } from './lib/api.js';
  import Overview from './routes/Overview.svelte';
  import NewRun from './routes/NewRun.svelte';
  import History from './routes/History.svelte';
  import Settings from './routes/Settings.svelte';
  import TubingCalibration from './routes/TubingCalibration.svelte';
  import PumpHead from './components/PumpHead.svelte';
  import Icon from './components/Icon.svelte';
  import ConnBar from './components/ConnBar.svelte';
  import ResumeModal from './components/ResumeModal.svelte';
  import FinishModal from './components/FinishModal.svelte';
  import { linkState } from './lib/link.js';

  const pumpLink = $derived(linkState(app.status, app.connected));

  // Flat sidebar: "Pump" up top, "Settings" pinned to the foot. Pump's
  // sub-views are tabs on the main panel, not a nested menu.
  // "New run" isn't a tab, Overview already offers it (idle empty state,
  // completed run) and History's "Run again" jumps straight to it.
  const pumpTabs = [
    ['overview', 'Overview', 'dashboard'],
    ['history', 'History', 'history'],
    ['calibration', 'Tubing calibration', 'ruler'],
  ];

  const running = $derived(!!app.status?.active);

  // Balance indicator: plain words instead of the raw state token. The
  // correction factor only shows while a run actually applies it.
  const SCALE_STATES = {
    normal: 'stable',
    perturbation: 'disturbed',
    refill_pending: 'refilling…',
    refill_settling: 'settling…',
  };
  const scaleView = $derived.by(() => {
    const s = app.status;
    if (!s?.scale_state) return null;
    if (s.scale_connected === false) {
      return {
        tone: 'bad',
        label: 'Balance: disconnected',
        title: 'The balance is not answering. Check its cable; the daemon keeps retrying.',
      };
    }
    if (s.tracking?.wrong_side) {
      return {
        tone: 'bad',
        label: 'Balance: on the other side | trim frozen',
        title:
          'The weight moves the wrong way for where Settings says the balance is (feed bottle or receiving vessel). Fix "Balance weighs" in Settings for the next run; this run keeps its pump setpoint, uncorrected.',
      };
    }
    if (s.tracking?.alarm === 'feed_stopped') {
      return {
        tone: 'bad',
        label: 'Feed stopped | bottle empty or line blocked',
        title:
          'The pump is running but the bottle weight has not moved for 3 minutes: refill the bottle, or check the line (pinched, disconnected, air). The correction is held at its last good value and resumes by itself once the feed flows again; what was missed meanwhile is not caught up.',
      };
    }
    if (!s.scale_ok) {
      return {
        tone: 'bad',
        label: 'Balance: correction out of bounds',
        title:
          'For 5 minutes the pump has delivered further from its setpoint than the correction limit (Settings, Balance) can make up: check the tube in the head, leaks, the bottle. The correction is held at its value from before; it resumes by itself once the pump is back within bounds, or if you widen the limit.',
      };
    }
    const trimOn = !!s.active?.gravimetric_trim;
    const c = trimOn && s.trim_c != null ? ` | ×${s.trim_c.toFixed(3)}` : '';
    // Live reading; "~" while the balance itself says it is still moving.
    const w =
      s.scale_weight_g != null
        ? `${s.scale_weight_g.toFixed(1)} g${s.scale_stable === false ? ' ~' : ''} | `
        : '';
    return {
      tone: 'ok',
      label: `Balance: ${w}${SCALE_STATES[s.scale_state] ?? s.scale_state}${c}`,
      title: trimOn
        ? `Gravimetric trim active: the pump setpoint is multiplied by ${s.trim_c?.toFixed(3)}.`
        : 'Balance connected. The trim applies only to runs started with it enabled.',
    };
  });

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
  // A dosing run whose curve ended stays active, still regulated and
  // journalled: the pop-up only tells so. `holding` is a hold left by a daemon
  // from before that (no regulation), still shown so it can be stopped.
  const hold = $derived.by(() => {
    const a = app.status?.active;
    if (a?.curve_done) {
      return {
        run_id: a.run_id,
        name: a.name,
        control_var: a.control_var,
        value: a.last_target,
        regulating: true,
        trimmed: a.gravimetric_trim,
      };
    }
    return app.status?.active ? null : (app.status?.holding ?? null);
  });
  const showFinish = $derived(!!hold && hold.run_id !== finishAck);
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

  // Alarms the phone is being reminded of until someone acknowledges them.
  let pages = $state([]);
  let ackErr = $state(null);
  $effect(() => {
    let live = true;
    const load = () =>
      get('/api/notify/pages')
        .then((p) => live && (pages = p ?? []))
        .catch(() => {});
    load();
    const t = setInterval(load, 5000);
    return () => {
      live = false;
      clearInterval(t);
    };
  });
  const pageRuns = $derived([...new Map(pages.map((p) => [p.run_id, p])).values()]);
  async function acknowledge(runId) {
    ackErr = null;
    try {
      await post('/api/notify/ack', { run_id: runId });
      pages = pages.filter((p) => p.run_id !== runId);
    } catch (e) {
      ackErr = e.message;
    }
  }

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
        <span class="ic pump-ic" aria-hidden="true"><PumpHead size={18} spin={running} frac={running ? 0.4 : 0} solid /></span>Pump control
      </button>
    </nav>

    <button
      class="nav-item nav-settings"
      class:active={app.route === 'settings'}
      onclick={() => (app.route = 'settings')}
    >
      <span class="ic gear-ic" aria-hidden="true">
        <!-- same drawing as the pump glyph: one flat colour, ring cut out -->
        <svg viewBox="-2 -2 104 104">
          <defs>
            <mask id="gear-hole">
              <rect x="-2" y="-2" width="104" height="104" fill="white" />
              <circle cx="50" cy="50" r="15" fill="black" />
            </mask>
          </defs>
          <path
            mask="url(#gear-hole)"
            class="gear-body"
            d="M41.6 15.0 L42.1 4.7 L57.9 4.7 L58.4 15.0 L68.8 19.3 L76.5 12.4 L87.6 23.5 L80.7 31.2 L85.0 41.6 L95.3 42.1 L95.3 57.9 L85.0 58.4 L80.7 68.8 L87.6 76.5 L76.5 87.6 L68.8 80.7 L58.4 85.0 L57.9 95.3 L42.1 95.3 L41.6 85.0 L31.2 80.7 L23.5 87.6 L12.4 76.5 L19.3 68.8 L15.0 58.4 L4.7 57.9 L4.7 42.1 L15.0 41.6 L19.3 31.2 L12.4 23.5 L23.5 12.4 L31.2 19.3Z"
          />
        </svg>
      </span>Settings
    </button>

    <div class="rail-foot">
      {#if app.status}
        <span class="pumpstat {pumpLink.tone}" title={pumpLink.label}>
          <span class="dot" aria-hidden="true"></span>{pumpLink.short}
        </span>
      {/if}
      {#if scaleView}
        <span class="pumpstat {scaleView.tone}" title={scaleView.title}>
          <span class="dot" aria-hidden="true"></span>{scaleView.label}
        </span>
      {/if}
      <div class="rail-foot-row">
        <span class="status" class:on={app.connected}>
          <span class="dot" aria-hidden="true"></span>
          {app.connected ? 'Daemon online' : 'Reconnecting…'}
        </span>
        <button class="theme" onclick={setTheme} title="Toggle theme">◐</button>
      </div>
    </div>
  </aside>

  <main class="main">
    <div class="wrap">
      <ConnBar />

      {#each pageRuns as p (p.run_id)}
        <div class="paging" role="alert">
          <span class="paging-text">
            <b>{pages.filter((q) => q.run_id === p.run_id).map((q) => q.title).join(' | ')}</b>
            <span class="paging-sub">Reminding {p.responsible}'s phone every few minutes until acknowledged ({p.sent} sent).</span>
          </span>
          <button class="btn-ghost" onclick={() => acknowledge(p.run_id)}>Acknowledge</button>
        </div>
      {/each}
      {#if ackErr}<div class="err" style="margin-bottom:16px">{ackErr}</div>{/if}

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
          {#each pumpTabs as [id, label, icon]}
            <button
              class="tab"
              class:on={app.tab === id}
              role="tab"
              aria-selected={app.tab === id}
              onclick={() => (app.tab = id)}
            ><Icon name={icon} size={15} />{label}</button>
          {/each}
        </div>

        {#if app.tab === 'new'}
          <NewRun />
        {:else if app.tab === 'history'}
          <History />
        {:else if app.tab === 'calibration'}
          <TubingCalibration />
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
    /* one flat colour, ring cut out (see PumpHead `solid`) */
    --ph-body: currentColor;
    --ph-detail: currentColor;
  }
  /* A live run: the glyph goes green (semantic) and spins, so the pump's
     state is visible from any section. Text keeps its normal colour. */
  .nav-item.running .pump-ic {
    opacity: 1;
    --ph-detail: var(--green-500);
  }
  .nav-item .gear-ic {
    width: 18px; height: 18px;
    display: inline-flex; align-items: center; justify-content: center;
  }
  .gear-ic svg { display: block; width: 100%; height: 100%; overflow: visible; }
  .gear-body { fill: currentColor; stroke: currentColor; stroke-width: 6; stroke-linejoin: round; }
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
  .status { display: inline-flex; align-items: center; gap: var(--s-2); font-size: 12px; font-weight: 600; color: var(--muted); }
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

  .paging {
    display: flex; align-items: center; justify-content: space-between; gap: var(--s-4);
    margin-bottom: var(--s-4);
    padding: var(--s-3) var(--s-4);
    border: 1px solid color-mix(in srgb, var(--danger) 45%, transparent);
    background: color-mix(in srgb, var(--danger) 9%, var(--surface));
    border-radius: var(--radius-ctl);
    color: var(--danger);
  }
  .paging-text { display: flex; flex-direction: column; gap: 2px; }
  .paging-sub { font-size: 12px; color: var(--muted); }

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
  .tab { display: inline-flex; align-items: center; gap: 6px; }
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
    /* minmax(0, …): the bar scrolls inside itself instead of widening the page */
    .app { grid-template-columns: minmax(0, 1fr); grid-template-rows: auto 1fr; }
    .rail {
      flex-direction: row; align-items: center;
      border-right: none; border-bottom: 1px solid var(--line-soft);
      overflow-x: auto;
      /* back to a normal flow bar on narrow screens */
      position: static;
      height: auto;
      overflow-y: visible;
    }
    /* Phone: one compact bar, the status lines wrapped under it as chips. */
    .rail { flex-wrap: wrap; gap: var(--s-2) var(--s-3); padding: var(--s-3) var(--s-4); }
    .brand .word { display: none; }
    nav { flex-direction: row; }
    .nav-item { width: auto; white-space: nowrap; }
    .nav-settings { margin-top: 0; }
    .rail-foot {
      margin-top: 0; padding: 0;
      flex-basis: 100%;
      flex-direction: row; flex-wrap: wrap; align-items: center;
      gap: var(--s-1) var(--s-4);
    }
    .rail-foot-row { gap: var(--s-4); }
    .rail-foot .theme { display: none; }
    .main { padding: var(--s-5) var(--s-4) var(--s-7); }
  }
</style>
