<script>
  import { app } from '../lib/state.svelte.js';
  import { get, post } from '../lib/api.js';
  import { linkState } from '../lib/link.js';

  // `compact` = the single-line bar pinned at the top of the app.
  // Default (false) = the stacked block shown inline where a run is started.
  // `controls` = show the port picker + Connect button. Set false for a
  // condition a reconnect can't fix (pump not tracking, journal stalled) - the
  // status line alone is then shown.
  let { compact = false, controls = true } = $props();

  let ports = $state([]);
  let portErr = $state(null);
  let cfgPath = $state('sim');
  let cfgBaud = $state(9600);
  let addr = $state(1); // MODBUS slave address; default 1
  let portSel = $state('sim');
  let customPath = $state('');
  let connecting = $state(false);
  let msg = $state(null);
  let err = $state(null);

  const effectivePath = $derived(
    portSel === '__custom' ? customPath.trim() : portSel,
  );
  const runActive = $derived(app.status?.active != null);
  const link = $derived(linkState(app.status, app.connected));

  $effect(() => {
    loadConfig();
    rescan();
  });

  function loadConfig() {
    return get('/api/config')
      .then((c) => {
        cfgPath = c.serial?.path ?? 'sim';
        cfgBaud = c.serial?.baud ?? 9600;
        if (!addrTouched) addr = c.pump?.address ?? 1;
        syncSelection();
      })
      .catch(() => {});
  }

  // `true` once the user picks something themselves - after that we stop
  // steering the field from config/rescan.
  let touched = $state(false);
  let addrTouched = $state(false);

  // Keep the <select> pointed at the configured port. If that port isn't among
  // the ones actually present, don't add a phantom "(not detected)" row - offer
  // it through the custom field instead, so the list stays uncluttered. A port
  // that shows up on a later rescan is then promoted out of the custom field.
  function syncSelection() {
    if (touched) return;
    if (cfgPath === 'sim' || cfgPath === '') {
      portSel = 'sim';
    } else if (ports.some((p) => p.name === cfgPath)) {
      portSel = cfgPath;
    } else {
      portSel = '__custom';
      customPath = cfgPath;
    }
  }

  function rescan() {
    portErr = null;
    get('/api/serial/ports')
      .then((r) => {
        ports = r.ports ?? [];
        syncSelection();
      })
      .catch((e) => (portErr = e.message));
  }

  async function connect() {
    connecting = true;
    err = null;
    msg = null;
    try {
      // Re-read config so baud (only editable in Settings) is current, not the
      // value cached when this component first mounted.
      await loadConfig();
      const r = await post('/api/serial/reconnect', {
        path: effectivePath,
        baud: cfgBaud,
        pump_addr: Number(addr) || 1,
      });
      msg = r.connected ?? 'Reconnected.';
      cfgPath = effectivePath;
    } catch (e) {
      err = e.message;
    }
    connecting = false;
  }
</script>

<div class="cc {link.tone}" class:compact>
  <div class="cc-state">
    <span class="dot" aria-hidden="true"></span>
    <div class="cc-text">
      <span class="cc-label">{link.label}</span>
      {#if link.hint && !compact}<span class="cc-hint">{link.hint}</span>{/if}
    </div>
  </div>

  {#if controls}
  <div class="cc-controls">
    <select
      bind:value={portSel}
      onchange={() => (touched = true)}
      disabled={connecting || runActive}
      aria-label="Serial port"
    >
      <option value="sim">sim (simulator)</option>
      {#each ports as p}
        <option value={p.name}>{p.name}{p.product ? ` · ${p.product}` : ` (${p.kind})`}</option>
      {/each}
      <option value="__custom">custom path…</option>
    </select>
    {#if portSel === '__custom'}
      <input
        type="text"
        placeholder="e.g. COM12 or /dev/ttyUSB0"
        bind:value={customPath}
        disabled={connecting || runActive}
      />
    {/if}
    <label class="addr">
      <span>addr</span>
      <input
        type="number"
        min="1"
        max="247"
        bind:value={addr}
        onchange={() => (addrTouched = true)}
        disabled={connecting || runActive}
        title="Pump MODBUS (slave) address"
      />
    </label>
    <button type="button" class="ghost" onclick={rescan} disabled={connecting}>Rescan</button>
    <button
      type="button"
      class="go"
      onclick={connect}
      disabled={connecting || runActive || !effectivePath}
      title={runActive ? 'Stop the active run before reconnecting' : ''}
    >
      {connecting ? 'Connecting…' : 'Connect'}
    </button>
  </div>
  {/if}
</div>

{#if link.hint && compact}<p class="cc-hint compact-hint">{link.hint}</p>{/if}
{#if controls && portErr}<p class="cc-msg err">{portErr}</p>{/if}
{#if controls && err}<p class="cc-msg err">{err}</p>{/if}
{#if controls && msg}<p class="cc-msg ok">{msg}</p>{/if}

<style>
  .cc {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--s-3) var(--s-4);
    padding: var(--s-3) var(--s-4);
    border-radius: var(--radius-ctl);
    border: 1px solid var(--line);
    background: var(--surface);
  }
  .cc.compact { padding: var(--s-2) var(--s-4); }

  /* Tone accents: left border + dot colour. */
  .cc.ok { border-color: color-mix(in srgb, var(--green-500) 45%, transparent); }
  .cc.ok .dot { background: var(--green-500); }
  .cc.sim { border-color: var(--line); }
  .cc.sim .dot { background: var(--muted); }
  .cc.idle .dot { background: var(--muted); }
  .cc.warn {
    border-color: color-mix(in srgb, #d68b45 55%, transparent);
    background: color-mix(in srgb, #d68b45 10%, var(--surface));
  }
  .cc.warn .dot { background: #d68b45; }
  .cc.bad {
    border-color: color-mix(in srgb, var(--danger) 50%, transparent);
    background: var(--danger-bg);
  }
  .cc.bad .dot { background: var(--danger); }

  .cc-state { display: flex; align-items: center; gap: var(--s-3); flex: 1 1 auto; min-width: 0; }
  .dot {
    width: 9px; height: 9px; border-radius: 50%; flex: none;
    box-shadow: 0 0 0 3px color-mix(in srgb, currentColor 0%, transparent);
  }
  .cc.ok .dot { animation: pulse 2.4s ease-in-out infinite; }
  @keyframes pulse { 0%, 100% { opacity: 1; } 50% { opacity: 0.55; } }
  @media (prefers-reduced-motion: reduce) { .cc.ok .dot { animation: none; } }

  .cc-text { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
  .cc-label { font-size: 13px; font-weight: 600; color: var(--ink); }
  .cc.bad .cc-label { color: var(--danger); }
  .cc-hint { font-size: 12px; color: var(--muted); line-height: 1.35; }
  .compact-hint { margin: var(--s-2) 0 0; }

  .cc-controls { display: flex; align-items: center; gap: var(--s-2); flex-wrap: wrap; }
  .cc-controls select, .cc-controls input {
    font: inherit; font-size: 13px;
    padding: var(--s-2) var(--s-3);
    border-radius: 8px; border: 1px solid var(--line);
    background: var(--surface); color: var(--ink);
  }
  .addr { display: inline-flex; align-items: center; gap: var(--s-2); }
  .addr span { font-size: 12px; color: var(--muted); }
  .addr input { width: 4.5ch; text-align: right; -moz-appearance: textfield; appearance: textfield; }
  .addr input::-webkit-outer-spin-button,
  .addr input::-webkit-inner-spin-button { -webkit-appearance: none; margin: 0; }
  .cc-controls button {
    font: inherit; font-size: 13px; font-weight: 600;
    padding: var(--s-2) var(--s-4);
    border-radius: 8px; cursor: pointer;
    border: 1px solid var(--line); background: var(--surface); color: var(--ink);
  }
  .cc-controls button.go {
    border-color: var(--teal-700);
    background: color-mix(in srgb, var(--teal-700) 12%, var(--surface));
    color: var(--teal-700);
  }
  .cc-controls button:disabled { opacity: 0.5; cursor: not-allowed; }
  .cc-controls button.ghost { color: var(--muted); }

  .cc-msg { margin: var(--s-2) 0 0; font-size: 12px; }
  .cc-msg.err { color: var(--danger); }
  .cc-msg.ok { color: var(--green-600); }

  @media (max-width: 640px) {
    .cc-controls { width: 100%; }
    .cc-controls select { flex: 1 1 auto; }
  }
</style>
