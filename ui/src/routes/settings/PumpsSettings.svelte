<script>
  // Settings > Pumps: the serial cable every pump is wired on (port,
  // Reconnect; fixed at 9600 baud 8E1) and the pumps on it, each a LabQ, a
  // LabV or a PDS II head at its own MODBUS address, each with its photo and
  // its own state. Replaces the connection bar's port picker, the old Pump
  // section and the Refill pump section (2026-10-21, mockup v6; pump models
  // 2026-10-23, mockup v3).
  import { get, post } from '../../lib/api.js';
  import { loadConfig, patchConfig } from '../../lib/config.js';
  import { app } from '../../lib/state.svelte.js';
  import { aboutRefillSpeed, pumpsProblem } from '../../lib/refill.js';
  import LinkHeader from './LinkHeader.svelte';
  import {
    MODELS, modelOf, notAnsweringHint, headOf, feedLocked, refillKindLocked,
    lineUp, lineState, pumpTone, feedResponding, needsReconnect, pingWanted, pingDelayMs, feedCameBack,
  } from '../../lib/pumps.js';
  import { radioKeys, rovingTab } from '../../lib/radio.js';
  import Icon from '../../components/Icon.svelte';
  import SettingsCard from './SettingsCard.svelte';
  import labqPhoto from '../../assets/pumps/labq.jpg';
  import labvPhoto from '../../assets/pumps/labv.jpg';
  import pds2Photo from '../../assets/pumps/pds2.jpg';

  // `group` (passed to every section) is not shown here: the side menu says it.

  const PHOTO = { labq: labqPhoto, labv: labvPhoto, pds2: pds2Photo };
  const ALT = { labq: 'LabQ pump', labv: 'LabV pump with its 12-channel head', pds2: 'PDS II controller with its pump heads' };
  const HEADS = [1, 2, 3, 4, 5, 6, 7, 8];

  let loaded = $state(false);
  let err = $state(null);
  let msg = $state(null);
  let busy = $state(false);

  // the form, and what the daemon has (saved)
  let path = $state('sim');
  let custom = $state('');
  let baud = $state(9600); // config.toml's, kept as is (the UI no longer offers it)
  let feedAddress = $state(1);
  let feedModel = $state('labq');
  let feedHead = $state(1);
  let allowSim = $state(false);
  let refillAddress = $state(2);
  let refillModel = $state('labq');
  let refillHead = $state(1);
  let speed = $state(350);
  let direction = $state('cw');
  let saved = $state(null);

  let ports = $state([]);
  let refillOpen = $state(false);
  let refillResponding = $state(null);

  const portPath = $derived(path === '__custom' ? custom.trim() : path);
  const runActive = $derived(!!app.status?.active);
  // a run refilling the bottle drives the refill pump: its model and head
  // stay, as while such a run waits to be resumed (refillKindLocked)
  const refillInRun = $derived(!!app.status?.refill?.in_run);
  const refillLocked = $derived(refillKindLocked(app.status));
  // an interrupted run resumes only on the feed pump it drove
  const recoveryPending = $derived(!!app.status?.has_pending_recovery);
  const feedFixed = $derived(feedLocked(app.status));
  const testing = $derived(!!app.status?.refill?.testing);

  function take(c) {
    const p = c.serial?.path ?? 'sim';
    baud = c.serial?.baud ?? 9600;
    allowSim = !!c.serial?.allow_simulator;
    feedAddress = c.pump?.address ?? 1;
    feedModel = modelOf(c.pump?.model).id;
    feedHead = c.pump?.head ?? 1;
    refillAddress = c.refill?.pump_address ?? 2;
    refillModel = modelOf(c.refill?.pump_model).id;
    refillHead = c.refill?.pump_head ?? 1;
    speed = c.refill?.speed_rpm ?? 350;
    direction = c.refill?.direction ?? 'cw';
    path = p === 'sim' || ports.some((x) => x.name === p) ? p : '__custom';
    custom = path === '__custom' ? p : '';
    saved = {
      path: p, baud: Number(baud), allowSim,
      feedAddress: Number(feedAddress), feedModel, feedHead: headOf(feedModel, feedHead),
      refillAddress: Number(refillAddress), refillModel, refillHead: headOf(refillModel, refillHead),
      speed: Number(speed), direction,
    };
  }

  // A model picked: its max rpm and steps are the model's, nothing to set.
  function pickFeed(id) {
    feedModel = id;
  }
  function pickRefill(id) {
    refillModel = id;
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

  const feedNow = $derived({ address: Number(feedAddress), model: feedModel, head: headOf(feedModel, feedHead) });
  const refillNow = $derived({ address: Number(refillAddress), model: refillModel, head: headOf(refillModel, refillHead) });
  const now = $derived({
    path: portPath, baud, feedAddress, feedModel: feedNow.model, feedHead: feedNow.head,
  });
  const cableChanged = $derived(needsReconnect(saved, now));
  const dirty = $derived(
    !!saved &&
      (cableChanged ||
        allowSim !== saved.allowSim ||
        refillNow.address !== saved.refillAddress ||
        refillNow.model !== saved.refillModel ||
        refillNow.head !== saved.refillHead ||
        Number(speed) !== saved.speed ||
        direction !== saved.direction),
  );
  // the refill speed's top and step follow the refill pump's model
  const refillTop = $derived(modelOf(refillModel).maxRpm);
  const refillStep = $derived(modelOf(refillModel).rpmStep);

  // the baud is config.toml's own (omitted: kept), the UI does not offer it
  async function reconnect() {
    await post('/api/serial/reconnect', {
      path: portPath,
      pump_addr: feedNow.address || 1,
      pump_model: feedNow.model,
      pump_head: feedNow.head,
    });
  }

  // One Save for the whole card: the cable is reconnected only if its port
  // or the feed pump (address, model, head) changed. A refused
  // refill speed opens the Change panel, where its field is.
  async function save() {
    const p = pumpsProblem({ feed: feedNow, refill: refillNow, speedRpm: speed });
    if (p) {
      err = p.message;
      msg = null;
      if (p.speed) refillOpen = true;
      return;
    }
    busy = true;
    err = null;
    msg = null;
    try {
      if (cableChanged) await reconnect();
      const { cfg } = await patchConfig((c) => {
        c.serial.allow_simulator = allowSim;
        c.refill = {
          ...c.refill,
          pump_address: refillNow.address,
          pump_model: refillNow.model,
          pump_head: refillNow.head,
          speed_rpm: Number(speed),
          direction,
        };
      });
      take(cfg);
      msg = cableChanged ? `Saved, and reconnected to ${portPath}.` : 'Saved.';
      await ping();
    } catch (e) {
      err = e.message;
      if (aboutRefillSpeed(e.message)) refillOpen = true;
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

{#snippet photo(model, head)}
  <span class="pic">
    <img src={PHOTO[model]} alt={ALT[model]} />
    {#if model === 'pds2'}<span class="hd">Head {head}</span>{/if}
  </span>
{/snippet}

{#snippet models(who, value, pick, locked)}
  <div class="seg models" role="radiogroup" aria-label="{who} pump model" tabindex="-1" onkeydown={radioKeys}>
    {#each MODELS as m}
      <button type="button" role="radio" aria-checked={value === m.id} tabindex={rovingTab(value === m.id, false)}
        class:on={value === m.id} disabled={locked} onclick={() => pick(m.id)}>{m.label}</button>
    {/each}
  </div>
{/snippet}

<!-- no "Hardware" eyebrow: the side menu says it already -->
<SettingsCard title="Pumps" {err} {msg} loading={!loaded}>
  <!-- the cable's state first, its button beside it; the port, rarely
       touched once connected, below -->
  <LinkHeader {line}>
    {#snippet action()}
      <!-- Connect while the cable is down, the thing to do then; Disconnect
           while it is up (frees the COM port, e.g. for the pump's own
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
    <span class="bus" title="Set every pump on this cable to {saved?.baud ?? baud} baud">{saved?.baud ?? baud} baud · 8E1</span>
  </div>

  <h3 class="eyebrow sub">On this cable</h3>
  <div class="pumps">
    <div class="item" class:bad={feedTone === 'bad'}>
      <div class="row">
        {@render photo(feedModel, feedNow.head)}
        <div class="top">
          <span class="nm">Feed pump</span>
          <span class="sum">set by each run</span>
        </div>
        {@render state(feedTone)}
        <div class="ctl">
          {@render models('Feed', feedModel, pickFeed, busy || feedFixed)}
          <label class="lbl addr">Address
            <input type="number" min="1" max={modelOf(feedModel).maxAddress} bind:value={feedAddress} disabled={busy || feedFixed} />
          </label>
          {#if feedModel === 'pds2'}
            <label class="lbl head">Head
              <select bind:value={feedHead} disabled={busy || feedFixed}>
                {#each HEADS as h}<option value={h}>{h}</option>{/each}
              </select>
            </label>
          {/if}
        </div>
      </div>
      {#if feedTone === 'bad' && saved}
        <p class="what">{notAnsweringHint({ model: saved.feedModel, address: saved.feedAddress, head: saved.feedHead })}</p>
      {/if}
    </div>

    <div class="item" class:bad={refillTone === 'bad'}>
      <div class="row">
        {@render photo(refillModel, refillNow.head)}
        <div class="top">
          <span class="nm">Refill pump</span>
          <span class="sum">
            <span><b>{saved?.speed ?? speed} rpm</b>, {saved?.direction ?? direction}</span>
            <button class="change" type="button" aria-expanded={refillOpen} onclick={() => (refillOpen = !refillOpen)}>
              <Icon name="sliders" size={14} />{refillOpen ? 'Close' : 'Change'}
            </button>
          </span>
        </div>
        {@render state(refillTone)}
        <div class="ctl">
          {@render models('Refill', refillModel, pickRefill, busy || refillLocked)}
          <label class="lbl addr">Address
            <input type="number" min="1" max={modelOf(refillModel).maxAddress} bind:value={refillAddress} disabled={busy} />
          </label>
          {#if refillModel === 'pds2'}
            <label class="lbl head">Head
              <select bind:value={refillHead} disabled={busy || refillLocked}>
                {#each HEADS as h}<option value={h}>{h}</option>{/each}
              </select>
            </label>
          {/if}
        </div>
      </div>
      {#if refillTone === 'bad' && saved}
        <p class="what">{notAnsweringHint({ model: saved.refillModel, address: saved.refillAddress, head: saved.refillHead })}</p>
      {/if}
      {#if refillOpen}
        <div class="more">
          <label class="field"><span>Speed, rpm</span>
            <input type="number" min="0.1" max={refillTop} step={refillStep} bind:value={speed} />
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
    {#if refillInRun}
      <p class="field-note why">A run is active: the cable, the feed pump and the refill pump's model and head are locked until it stops.</p>
    {:else if runActive}
      <p class="field-note why">A run is active: the cable and the feed pump are locked until it stops.</p>
    {:else if recoveryPending}
      <p class="field-note why">An interrupted run waits: the feed pump{refillLocked ? " and the refill pump's model and head are" : ' is'} locked until it is resumed or ended.</p>
    {:else if refillLocked}
      <p class="field-note why">A run with automatic refill waits to be resumed: the refill pump's model and head are locked until it resumes or is aborted.</p>
    {/if}
  {/snippet}
</SettingsCard>

<style>
  .sub { margin: var(--s-5) 0 var(--s-3); }
  .sub:first-child { margin-top: 0; }
  .conn { display: flex; gap: var(--s-2); align-items: center; flex-wrap: wrap; }
  .conn select, .conn input, .lbl input, .lbl select, .more input {
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
  /* the cable's one setting, said rather than offered */
  .bus {
    font-family: var(--mono); font-size: 12.5px; color: var(--muted); white-space: nowrap;
    padding: 7px 10px; background: var(--surface-sunken); border-radius: var(--radius-ctl);
  }

  /* the pumps: every row in the same order, colour only for a problem */
  .pumps { border: 1px solid var(--line); border-radius: var(--radius-ctl); }
  .item + .item { border-top: 1px solid var(--line); }
  /* the photo; beside it the name and the state on one line, the model and
     its fields under them, with the whole width (the card is narrow) */
  .row {
    display: grid; grid-template-columns: 120px minmax(0, 1fr) auto;
    grid-template-areas: 'pic top state' 'pic ctl ctl';
    grid-template-rows: auto 1fr;
    gap: var(--s-2) var(--s-4); align-items: start; padding: var(--s-4);
  }
  /* a product photo, shot on white: a white tile in every theme */
  .pic {
    grid-area: pic; align-self: center;
    position: relative; width: 120px; height: 96px; border-radius: 10px; overflow: hidden;
    display: grid; place-items: center; background: var(--photo-bg); border: 1px solid var(--line);
  }
  .item.bad .pic { border-color: var(--danger); box-shadow: 0 0 0 1px var(--danger); }
  .pic img { max-width: 108px; max-height: 84px; object-fit: contain; display: block; }
  .hd {
    position: absolute; right: 4px; bottom: 4px; font: 600 10.5px var(--mono); white-space: nowrap;
    background: var(--teal-700); color: var(--on-primary); padding: 1px 6px; border-radius: 999px;
  }
  .top { grid-area: top; display: flex; align-items: center; gap: var(--s-1) var(--s-3); flex-wrap: wrap; min-width: 0; min-height: 28px; }
  .nm { font-weight: 600; }
  .ctl { grid-area: ctl; display: flex; align-items: center; gap: var(--s-2) var(--s-4); flex-wrap: wrap; min-width: 0; }
  .models button { padding: 5px 12px; font-size: 13px; }
  .lbl { display: inline-flex; align-items: center; gap: var(--s-2); font-size: 12.5px; color: var(--muted); white-space: nowrap; }
  .lbl input, .lbl select { width: 64px; text-align: center; font-family: var(--mono); padding: 5px 6px; }
  .lbl select { width: auto; }
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
  .quiet { grid-area: state; justify-self: end; align-self: center; font-size: 12.5px; color: var(--muted); white-space: nowrap; }
  .badge {
    grid-area: state; justify-self: end; align-self: center; display: inline-flex; align-items: center; gap: 6px; white-space: nowrap;
    font-size: 12px; font-weight: 600; padding: 3px 10px 3px 8px; border-radius: 999px;
    background: var(--danger-bg); color: var(--danger-ink);
  }
  .badge::before { content: ''; width: 8px; height: 8px; border-radius: 50%; background: var(--danger); }
  .what { margin: 0; padding: 0 var(--s-4) var(--s-4) calc(120px + var(--s-4) * 2); font-size: 12.5px; color: var(--danger-ink); }
  .more {
    display: flex; gap: var(--s-4); align-items: flex-end; flex-wrap: wrap;
    padding: 0 var(--s-4) var(--s-4) calc(120px + var(--s-4) * 2);
  }
  .more input { width: 110px; }
  .more .field-note { flex-basis: 100%; margin: 0; }
  .more .seg { align-self: start; }

  .sim { margin-right: auto; }
  .why { flex-basis: 100%; margin: 0; }

  /* a phone: the photo above, everything else under it at full width */
  @media (max-width: 560px) {
    .row { grid-template-columns: minmax(0, 1fr) auto; grid-template-areas: 'pic state' 'top top' 'ctl ctl'; grid-template-rows: auto; }
    .pic { justify-self: start; }
    .what, .more { padding-left: var(--s-4); }
  }
</style>
