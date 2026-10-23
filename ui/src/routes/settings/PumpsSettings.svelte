<script>
  // Settings > Pumps: the serial cable every LabQ is wired on (port, baud,
  // Reconnect) and the pumps on it, each at its own MODBUS address, each with
  // its own state. Replaces the connection bar's port picker, the old Pump
  // section and the Refill pump section (2026-10-21, mockup v6).
  import { get, post } from '../../lib/api.js';
  import { loadConfig, patchConfig } from '../../lib/config.js';
  import { app } from '../../lib/state.svelte.js';
  import { refillProblem } from '../../lib/refill.js';
  import LinkHeader from './LinkHeader.svelte';
  import { lineUp, lineState, pumpTone, feedResponding, needsReconnect, pingWanted, pingDelayMs, feedCameBack } from '../../lib/pumps.js';
  import { radioKeys, rovingTab } from '../../lib/radio.js';
  import Icon from '../../components/Icon.svelte';
  import PumpHead from '../../components/PumpHead.svelte';
  import SettingsCard from './SettingsCard.svelte';

  // `group` (passed to every section) is not shown here: the side menu says it.

  let loaded = $state(false);
  let err = $state(null);
  let msg = $state(null);
  let busy = $state(false);

  // the form, and what the daemon has (saved)
  let path = $state('sim');
  let custom = $state('');
  let baud = $state(9600);
  let feedAddress = $state(1);
  let allowSim = $state(false);
  let refillAddress = $state(2);
  let speed = $state(350);
  let direction = $state('cw');
  let saved = $state(null);
  let cfgMaxRpm = $state(350); // the refill pump's max rpm, from config (350 for a LabQ)

  let ports = $state([]);
  let refillOpen = $state(false);
  let refillResponding = $state(null);

  const portPath = $derived(path === '__custom' ? custom.trim() : path);
  const runActive = $derived(!!app.status?.active);
  const testing = $derived(!!app.status?.refill?.testing);

  function take(c) {
    const p = c.serial?.path ?? 'sim';
    baud = c.serial?.baud ?? 9600;
    allowSim = !!c.serial?.allow_simulator;
    feedAddress = c.pump?.address ?? 1;
    refillAddress = c.refill?.pump_address ?? 2;
    speed = c.refill?.speed_rpm ?? 350;
    cfgMaxRpm = c.refill?.pump_max_rpm ?? 350;
    direction = c.refill?.direction ?? 'cw';
    path = p === 'sim' || ports.some((x) => x.name === p) ? p : '__custom';
    custom = path === '__custom' ? p : '';
    saved = {
      path: p, baud: Number(baud), feedAddress: Number(feedAddress), allowSim,
      refillAddress: Number(refillAddress), speed: Number(speed), direction,
    };
  }

  function rescan() {
    return get('/api/serial/ports')
      .then((r) => (ports = r.ports ?? []))
      .catch((e) => (err = e.message));
  }

  // Does the refill pump answer? A read, never a Start (the daemon asks it
  // nothing else outside a run). During a run the status frame says it.
  // Asked again after `pingDelayMs`: 3 s while it does not answer, 15 s
  // otherwise; at once when the feed pump comes back (see below).
  let pingTimer = null;
  let gone = false; // the page left: no ping, and none rescheduled by one in flight
  async function ping() {
    clearTimeout(pingTimer);
    if (gone) return;
    if (!pingWanted(app.status)) {
      if (!lineUp(app.status)) refillResponding = null;
    } else {
      try {
        refillResponding = (await post('/api/refill/ping')).responding;
      } catch {
        refillResponding = null;
      }
    }
    if (!gone) pingTimer = setTimeout(ping, pingDelayMs(refillResponding));
  }

  $effect(() => {
    rescan()
      .then(loadConfig)
      .then((c) => {
        take(c);
        loaded = true;
        ping();
      })
      .catch((e) => (err = e.message));
    return () => {
      gone = true;
      clearTimeout(pingTimer);
    };
  });

  const s = $derived(app.status);
  const up = $derived(lineUp(s));
  // the cable plugged back: the feed pump answers again within a second (the
  // daemon probes it), the refill pump's last answer is from the cut
  let feedBefore = null;
  $effect(() => {
    const now = feedResponding(s);
    if (loaded && feedCameBack(feedBefore, now)) ping();
    feedBefore = now;
  });
  const feedTone = $derived(pumpTone({ up, responding: feedResponding(s) }));
  const refillTone = $derived(pumpTone({ up, responding: s?.refill?.responding ?? refillResponding }));
  const line = $derived(
    lineState(s, { answering: [feedTone, refillTone].filter((t) => t === '').length, total: 2 }),
  );

  const now = $derived({ path: portPath, baud, feedAddress });
  const cableChanged = $derived(needsReconnect(saved, now));
  const dirty = $derived(
    !!saved &&
      (cableChanged ||
        allowSim !== saved.allowSim ||
        Number(refillAddress) !== saved.refillAddress ||
        Number(speed) !== saved.speed ||
        direction !== saved.direction),
  );

  async function reconnect() {
    await post('/api/serial/reconnect', { path: portPath, baud: Number(baud), pump_addr: Number(feedAddress) || 1 });
  }

  // One Save for the whole card: the cable is reconnected only if its port,
  // baud or the feed pump's address changed.
  async function save() {
    const problem = refillProblem({
      address: refillAddress,
      model: 'labq',
      head: 1,
      speedRpm: speed,
      maxRpm: cfgMaxRpm,
      feed: { address: feedAddress, model: 'labq', head: 1 },
    });
    if (problem) {
      err = problem;
      msg = null;
      return;
    }
    busy = true;
    err = null;
    msg = null;
    try {
      if (cableChanged) await reconnect();
      const { cfg } = await patchConfig((c) => {
        c.serial.allow_simulator = allowSim;
        c.refill = { ...c.refill, pump_address: Number(refillAddress), speed_rpm: Number(speed), direction };
      });
      take(cfg);
      msg = cableChanged ? `Saved, and reconnected to ${portPath}.` : 'Saved.';
      await ping();
    } catch (e) {
      err = e.message;
    }
    busy = false;
  }

  async function connectClicked() {
    busy = true;
    err = null;
    msg = null;
    try {
      await reconnect();
      take(await loadConfig());
      msg = `Connected to ${portPath}.`;
      await ping();
    } catch (e) {
      err = e.message;
    }
    busy = false;
  }

  async function disconnect() {
    busy = true;
    err = null;
    msg = null;
    try {
      const r = await post('/api/serial/disconnect');
      msg = `${r.disconnected} closed: the pumps no longer answer until you Connect.`;
      refillResponding = null;
    } catch (e) {
      err = e.message;
    }
    busy = false;
  }

  async function test() {
    busy = true;
    err = null;
    msg = null;
    try {
      const r = await post('/api/refill/test');
      msg = `The refill pump runs ${r.seconds} s at ${saved.speed} rpm: the liquid must go from the reservoir into the weighed bottle. If it goes the other way, pick the other direction, save and test again.`;
    } catch (e) {
      err = e.message;
    }
    busy = false;
  }
</script>

{#snippet state(tone)}
  {#if tone === 'bad'}
    <span class="badge bad">Not answering</span>
  {:else}
    <span class="quiet">{tone === 'idle' ? 'not asked' : 'answers'}</span>
  {/if}
{/snippet}

<!-- no "Hardware" eyebrow: the side menu says it already -->
<SettingsCard title="Pumps" {err} {msg} loading={!loaded}>
  <!-- the cable's state first, its button beside it; the port and baud,
       rarely touched once connected, below -->
  <LinkHeader {line}>
    {#snippet action()}
      <!-- Connect while the cable is down, the thing to do then; Disconnect
           while it is up (frees the COM port, e.g. for the LabQ's own
           software; the port stays saved for the next Connect) -->
      {#if up}
        <button class="unplug" type="button" onclick={disconnect} disabled={busy || runActive}
          title={runActive ? 'Not during a run' : 'Close the port: the pumps stop answering until Connect'}><Icon name="unplug" size={15} />Disconnect</button>
      {:else}
        <button class="btn-primary" type="button" onclick={connectClicked} disabled={busy || runActive || !portPath || portPath === 'sim'}
          title={runActive ? 'Not during a run' : undefined}>Connect</button>
      {/if}
    {/snippet}
  </LinkHeader>
  <div class="conn">
    <select class="port" bind:value={path} disabled={busy || runActive} aria-label="Serial port">
      <option value="sim">sim (simulator)</option>
      {#each ports as p}
        <option value={p.name}>{p.name}{p.product ? ` | ${p.product}` : ''}</option>
      {/each}
      <option value="__custom">Other port…</option>
    </select>
    {#if path === '__custom'}
      <input class="custom" type="text" placeholder="COM12" bind:value={custom} disabled={busy || runActive} aria-label="Port name" />
    {/if}
    <button class="iconbtn" type="button" title="Rescan the ports" aria-label="Rescan the ports" onclick={rescan}>
      <Icon name="refresh-cw" size={16} />
    </button>
    <select bind:value={baud} disabled={busy || runActive} aria-label="Baud">
      {#each [1200, 2400, 4800, 9600] as b}<option value={b}>{b} baud</option>{/each}
    </select>
  </div>

  <h3 class="eyebrow sub">On this cable</h3>
  <div class="pumps">
    <div class="item" class:bad={feedTone === 'bad'}>
      <div class="row">
        <span class="pic" aria-hidden="true"><PumpHead size={20} spin={false} solid /></span>
        <span class="nm">Feed pump</span>
        <label class="addr">Address
          <input type="number" min="1" max="247" bind:value={feedAddress} disabled={busy || runActive} />
        </label>
        <span class="sum">set by each run</span>
        {@render state(feedTone)}
      </div>
      {#if feedTone === 'bad'}<p class="what">Is it switched on, and set to address {saved?.feedAddress} in its own menu?</p>{/if}
    </div>

    <div class="item" class:bad={refillTone === 'bad'}>
      <div class="row">
        <span class="pic" aria-hidden="true"><Icon name="droplet" size={20} /></span>
        <span class="nm">Refill pump</span>
        <label class="addr">Address
          <input type="number" min="1" max="247" bind:value={refillAddress} disabled={busy} />
        </label>
        <span class="sum">
          <span><b>{saved?.speed ?? speed} rpm</b>, {saved?.direction ?? direction}</span>
          <button class="change" type="button" aria-expanded={refillOpen} onclick={() => (refillOpen = !refillOpen)}>
            <Icon name="sliders" size={14} />{refillOpen ? 'Close' : 'Change'}
          </button>
        </span>
        {@render state(refillTone)}
      </div>
      {#if refillTone === 'bad'}<p class="what">Is it switched on, and set to address {saved?.refillAddress} in its own menu?</p>{/if}
      {#if refillOpen}
        <div class="more">
          <label class="field"><span>Speed, rpm</span>
            <input type="number" min="0.1" max="350" step="1" bind:value={speed} />
          </label>
          <div class="field"><span>Direction</span>
            <div class="seg" role="radiogroup" aria-label="Refill pump direction" tabindex="-1" onkeydown={radioKeys}>
              <button role="radio" aria-checked={direction === 'cw'} tabindex={rovingTab(direction === 'cw', false)}
                class:on={direction === 'cw'} title="Clockwise" onclick={() => (direction = 'cw')}>↻ cw</button>
              <button role="radio" aria-checked={direction === 'ccw'} tabindex={rovingTab(direction === 'ccw', false)}
                class:on={direction === 'ccw'} title="Counter-clockwise" onclick={() => (direction = 'ccw')}>↺ ccw</button>
            </div>
          </div>
          <button class="btn-ghost" type="button" onclick={test} disabled={busy || dirty || testing || runActive}
            title={dirty ? 'Save first: the test runs what is saved' : runActive ? 'Not during a run' : undefined}>
            {testing ? 'Testing…' : 'Test 3 s'}
          </button>
          <p class="field-note">The speed applies at once, even during a fill. The direction waits for the next fill.</p>
        </div>
      {/if}
    </div>
  </div>

  {#snippet foot()}
    <label class="field check sim">
      <input type="checkbox" bind:checked={allowSim} />
      <span>Allow runs on the simulator</span>
    </label>
    <button class="btn-primary" type="button" disabled={busy || !dirty} onclick={save}>{busy ? 'Saving…' : 'Save'}</button>
    {#if runActive}<p class="field-note why">A run is active: the cable and the feed pump are locked until it stops.</p>{/if}
  {/snippet}
</SettingsCard>

<style>
  .sub { margin: var(--s-5) 0 var(--s-3); }
  .sub:first-child { margin-top: 0; }
  .conn { display: flex; gap: var(--s-2); align-items: center; flex-wrap: wrap; }
  .conn select, .conn input, .addr input, .more input {
    font: inherit; font-size: 14px; color: var(--ink); background: var(--surface);
    border: 1px solid var(--line); border-radius: var(--radius-ctl); padding: 7px 10px;
  }
  /* app.css sizes every field to full width: here they share one line */
  .conn select, .conn input { width: auto; }
  .conn .port { flex: 1 1 260px; min-width: 0; }
  .conn .custom { width: 140px; }
  /* as tall as the fields beside it, the glyph in its middle (no inherited padding) */
  .iconbtn {
    width: 38px; align-self: stretch; padding: 0; display: grid; place-items: center; flex: none; cursor: pointer;
    border: 1px solid var(--line); border-radius: var(--radius-ctl); background: var(--surface); color: var(--muted);
  }
  .iconbtn:hover { color: var(--teal-700); border-color: var(--teal-700); }
  .iconbtn:focus-visible, .change:focus-visible { outline: 2px solid var(--teal-600); outline-offset: 2px; }


  /* the pumps: every row in the same order, colour only for a problem */
  .pumps { border: 1px solid var(--line); border-radius: var(--radius-ctl); }
  .item + .item { border-top: 1px solid var(--line); }
  .row {
    display: grid; grid-template-columns: 36px 105px 120px minmax(0, 1fr) auto;
    gap: var(--s-3) var(--s-4); align-items: center; padding: var(--s-3) var(--s-4);
  }
  .pic {
    width: 36px; height: 36px; border-radius: 10px; display: grid; place-items: center;
    background: var(--surface-sunken); color: var(--teal-700);
  }
  .item.bad .pic { background: var(--danger-bg); color: var(--danger-ink); }
  .nm { font-weight: 600; }
  .addr { display: flex; align-items: center; gap: var(--s-2); font-size: 12.5px; color: var(--muted); }
  .addr input { width: 64px; text-align: center; font-family: var(--mono); padding: 5px 6px; }
  /* wraps rather than running under the state at the row's end */
  .sum { display: flex; align-items: center; flex-wrap: wrap; gap: var(--s-1) var(--s-3); font-size: 13px; color: var(--muted); min-width: 0; }
  .sum > span { white-space: nowrap; }
  .sum b { color: var(--ink); font-weight: 500; }
  .change {
    display: inline-flex; align-items: center; gap: 6px; font: 500 12.5px var(--font); color: var(--ink);
    background: var(--surface); border: 1px solid var(--line); border-radius: var(--radius-ctl); padding: 5px 12px; cursor: pointer;
  }
  .change:hover { border-color: var(--teal-700); color: var(--teal-700); }
  .change[aria-expanded='true'] { background: var(--surface-sunken); }
  .quiet { justify-self: end; font-size: 12.5px; color: var(--muted); }
  .badge {
    justify-self: end; display: inline-flex; align-items: center; gap: 6px; white-space: nowrap;
    font-size: 12px; font-weight: 600; padding: 3px 10px 3px 8px; border-radius: 999px;
    background: var(--danger-bg); color: var(--danger-ink);
  }
  .badge::before { content: ''; width: 8px; height: 8px; border-radius: 50%; background: var(--danger); }
  .what { margin: 0; padding: 0 var(--s-4) var(--s-3) calc(36px + var(--s-4) * 2); font-size: 12.5px; color: var(--danger-ink); }
  .more {
    display: flex; gap: var(--s-4); align-items: flex-end; flex-wrap: wrap;
    padding: 0 var(--s-4) var(--s-4) calc(36px + var(--s-4) * 2);
  }
  .more input { width: 110px; }
  .more .field-note { flex-basis: 100%; margin: 0; }
  .seg { align-self: start; }

  .sim { margin-right: auto; }
  .why { flex-basis: 100%; margin: 0; }

  @media (max-width: 640px) {
    .row { grid-template-columns: 36px minmax(0, 1fr) auto; }
    .row .addr, .row .sum { grid-column: 2 / -1; }
    .what, .more { padding-left: var(--s-4); }
  }
</style>
