<script>
  import { untrack } from 'svelte';
  import { get, post } from '../../lib/api.js';
  import { loadConfig } from '../../lib/config.js';
  import {
    cameraLine,
    camerasFound,
    changeText,
    controlChanged,
    controlLabel,
    controlText,
    deviceKey,
    deviceLabel,
    FPS_CHOICES,
    liveShown,
    lockPlan,
    pictureLocked,
    powerLineOptions,
    restorePlan,
    sizesFor,
    throttle,
  } from '../../lib/camera.js';
  import { shortTime } from '../../lib/fmt.js';
  import { bytesText } from '../../lib/timelapse.js';
  import { segThumb } from '../../lib/segthumb.js';
  import Icon from '../../components/Icon.svelte';
  import LiveImg from '../../components/LiveImg.svelte';
  import LinkHeader from './LinkHeader.svelte';
  import SettingsCard from './SettingsCard.svelte';

  // `group` (passed to every section) is not shown here: the side menu says it.
  // As Pumps and Balance: the link's state first, then the camera, its size,
  // the live view's pace and its picture controls. No Save (mockup of
  // 2026-11-05, "ultra efficace"): every click is set on the camera and saved
  // at once, the line under the card says what was done, with Undo.

  let loaded = $state(false);
  let err = $state(null);
  let busy = $state(false);

  // the camera's state, every 2 s while the section is open (a failed poll
  // marks the preview not live)
  let cam = $state(null);
  let pollOk = $state(true);
  $effect(() => {
    let live = true;
    const load = () =>
      get('/api/camera')
        .then((c) => {
          if (!live) return;
          cam = c;
          pollOk = true;
        })
        .catch(() => live && (pollOk = false));
    load();
    const t = setInterval(load, 2000);
    return () => {
      live = false;
      clearInterval(t);
    };
  });
  const line = $derived(cameraLine(cam));
  // LinkHeader's shape: tone '' when connected, the detail as its text
  const head = $derived({ tone: line.tone === 'ok' ? '' : line.tone, word: line.word, text: line.detail });
  const holding = $derived(['connected', 'connecting', 'lost'].includes(cam?.state));

  // The PC's cameras: listed by a helper, a few seconds; "Look again" asks
  // for a fresh list (a camera plugged in since).
  let devices = $state([]);
  // once a first list arrived: before, the saved camera is not "not detected"
  let listed = $state(false);
  let looking = $state(false);
  function lookAgain(refresh) {
    looking = true;
    get(`/api/camera/devices${refresh ? '?refresh=1' : ''}`)
      .then((d) => {
        devices = d ?? [];
        listed = true;
      })
      .catch((e) => (err = e.message))
      .finally(() => (looking = false));
  }
  // asked again at each opening: a webcam plugged in since the last list
  // stayed out of a cached one until "Look again" (2026-11-03)
  $effect(() => lookAgain(true));

  // what config.toml has
  let devKey = $state('');
  let width = $state(1280);
  let height = $state(720);
  let fps = $state(15);
  // the timelapse's: New run's default interval, the folder new ones go in
  let intervalMin = $state(5);
  let captures = $state('');
  let savedDevice = $state(null);
  let rev = $state(null);

  function loadSaved() {
    return loadConfig().then((c) => {
      const s = c.camera;
      devKey = s?.device ? deviceKey(s.device, s.device_index ?? 0) : '';
      width = s?.width ?? 1280;
      height = s?.height ?? 720;
      fps = s?.live_fps ?? 15;
      intervalMin = s?.interval_min ?? 5;
      captures = s?.captures_dir ?? '';
      savedDevice = s?.device ? { name: s.device, index: s.device_index ?? 0 } : null;
      rev = c._rev ?? null;
      loaded = true;
    });
  }
  $effect(() => {
    loadSaved().catch((e) => (err = e.message));
  });
  // after a save elsewhere than Connect: Connect checks the revision
  const refreshRev = () => loadConfig().then((c) => (rev = c._rev ?? null)).catch(() => {});

  const picked = $derived(devices.find((d) => deviceKey(d.name, d.index) === devKey) ?? null);
  // the saved camera, while the list does not have it (unplugged)
  const missing = $derived(
    listed && savedDevice && !devices.some((d) => deviceKey(d.name, d.index) === deviceKey(savedDevice.name, savedDevice.index))
      ? savedDevice
      : null,
  );
  const sizes = $derived(sizesFor(picked));

  // --- what was done, and Undo ---------------------------------------------
  // `history` holds one undo per change, newest last; the line says the last.
  let history = $state([]);
  let last = $state(null);
  function done(text, undo) {
    history = [...history, undo];
    last = `${text}, saved ${shortTime(new Date().toISOString())}`;
  }
  async function undo() {
    const u = history.at(-1);
    if (!u || busy) return;
    history = history.slice(0, -1);
    await act(u);
    last = history.length ? `Undone, saved ${shortTime(new Date().toISOString())}` : null;
  }
  // one change at a time, its error said on the card
  async function act(f) {
    busy = true;
    err = null;
    try {
      await f();
    } catch (e) {
      err = e.message;
    }
    busy = false;
  }

  // --- camera and size: connected on the click -----------------------------
  async function connectTo(dev, w, h) {
    await post('/api/camera/connect', { device: dev.name, index: dev.index, width: w, height: h, _rev: rev });
    await loadSaved();
  }
  function connectWith(dev, w, h, text) {
    const before = savedDevice ? { dev: savedDevice, w: width, h: height } : null;
    return act(async () => {
      await connectTo(dev, w, h);
      done(text, before ? () => connectTo(before.dev, before.w, before.h) : async () => {});
    });
  }
  function pickDevice(key) {
    const dev = devices.find((d) => deviceKey(d.name, d.index) === key);
    if (!dev) return;
    // a size the new camera lacks: 1280 × 720 if it has it, else its first
    const offered = sizesFor(dev);
    let size = [width, height];
    if (!offered.some((s) => s.size[0] === width && s.size[1] === height)) {
      size = (offered.find((o) => o.size[0] === 1280) ?? offered[0])?.size ?? size;
    }
    connectWith(dev, size[0], size[1], deviceLabel(dev, devices));
  }
  function pickSize(s) {
    const dev = picked ?? missing;
    if (!dev || (s.size[0] === width && s.size[1] === height)) return;
    connectWith(dev, s.size[0], s.size[1], `Picture size ${s.label}`);
  }
  // Connect on the band: the saved camera again (after closing what held it)
  function reconnect() {
    const dev = picked ?? missing;
    if (dev) act(() => connectTo(dev, width, height));
  }
  function disconnect() {
    act(async () => {
      await post('/api/camera/disconnect');
      await loadSaved();
    });
  }

  // --- live view pace ------------------------------------------------------
  async function sendFps(n) {
    await post('/api/camera/fps', { fps: n });
    fps = n;
    await refreshRev();
  }
  function pickFps(n) {
    const before = fps;
    if (n === before) return;
    act(async () => {
      await sendFps(n);
      done(`Live view ${n} a second`, () => sendFps(before));
    });
  }

  // --- timelapse: New run's default interval, the captures folder ----------
  // Applied on leaving the field (or Enter), like every other change here.
  let capInfo = $state(null);
  let folderErr = $state(null);
  const loadCaptures = () =>
    get('/api/camera/captures')
      .then((c) => (capInfo = c))
      .catch(() => {});
  $effect(() => {
    loadCaptures();
  });

  async function sendInterval(n) {
    await post('/api/camera/interval', { interval_min: n });
    intervalMin = n;
    await refreshRev();
  }
  function pickInterval(input) {
    const n = Math.round(Number(input.value));
    const before = intervalMin;
    if (!(n >= 1 && n <= 1440)) {
      err = 'Timelapse every: 1 to 1440 minutes';
      input.value = String(before);
      return;
    }
    if (n === before) return;
    act(async () => {
      await sendInterval(n);
      done(`Timelapse every ${n} min`, () => sendInterval(before));
    });
  }

  async function sendFolder(folder) {
    await post('/api/camera/captures', { folder });
    captures = folder;
    await refreshRev();
    await loadCaptures();
  }
  // a folder refused stays in the field, orange, the reason under it
  async function pickFolder(input) {
    const folder = input.value.trim();
    const before = captures;
    if (folder === before) {
      folderErr = null;
      return;
    }
    busy = true;
    err = null;
    try {
      await sendFolder(folder);
      folderErr = null;
      done(folder ? 'Images saved in the new folder' : 'Images saved in the default folder', () => sendFolder(before));
    } catch (e) {
      folderErr = e.message;
    }
    busy = false;
  }

  // --- picture controls ----------------------------------------------------
  // asked once the camera is connected, then kept from each answer
  let controls = $state(null);
  const connectedAs = $derived(cam?.state === 'connected' ? `${cam.device}|${cam.width}|${cam.height}` : null);
  const reloadControls = () =>
    get('/api/camera/controls')
      .then((r) => (controls = r.controls))
      .catch((e) => (err = e.message));
  $effect(() => {
    if (!connectedAs) {
      controls = null;
      return;
    }
    untrack(reloadControls);
  });
  const snapshot = () => $state.snapshot(controls) ?? [];

  // one request after the other: a slider's last value always lands last
  let queue = Promise.resolve();
  function send(v, save = true) {
    const sent = queue.then(() => post('/api/camera/control', { ...v, save }));
    queue = sent.catch(() => {});
    return sent.then((r) => {
      controls = r.controls;
    });
  }
  // a refused value: the rows show the camera's own values again
  async function sendAll(plan) {
    try {
      for (const v of plan) await send(v);
    } catch (e) {
      reloadControls();
      throw e;
    }
    await refreshRev();
  }
  // `plan` set on the camera and saved; undone by sending back what it moved
  function change(plan, text) {
    if (!plan.length) return;
    const before = snapshot();
    act(async () => {
      await sendAll(plan);
      done(text, () => sendAll(restorePlan(before, snapshot())));
    });
  }

  // a slider: set on the camera as it moves (not saved, 8 a second at most),
  // saved once let go
  let dragFrom = null;
  const mover = throttle((v) => send(v, false).catch((e) => (err = e.message)), 120);
  function moving(c, value) {
    dragFrom ??= snapshot();
    controls = controls.map((x) => (x.control === c.control ? { ...x, value } : x));
    mover.push({ control: c.control, value, auto: false });
  }
  function moved(c, value) {
    mover.cancel();
    const before = dragFrom ?? snapshot();
    dragFrom = null;
    const v = { control: c.control, value, auto: false };
    act(async () => {
      await sendAll([v]);
      done(changeText(v), () => sendAll(restorePlan(before, snapshot())));
    });
  }

  const locked = $derived(pictureLocked(controls));
  const lockable = $derived((controls ?? []).some((c) => c.can_auto));
  function lockPicture() {
    change(lockPlan(controls), locked ? 'Picture back to auto' : 'Picture locked');
  }
  function cameraDefaults() {
    const before = snapshot();
    act(async () => {
      const r = await post('/api/camera/controls/reset');
      controls = r.controls;
      await refreshRev();
      done('Camera defaults', () => sendAll(restorePlan(before, snapshot())));
    });
  }

  // "What this camera sees now": the live view while the section is open
  // and visible and the camera connected (one viewer: the daemon grabs only
  // for one, a polled frame.jpg stayed the picture of the Connect).
  let visible = $state(typeof document === 'undefined' || document.visibilityState === 'visible');
  $effect(() => {
    const seen = () => (visible = document.visibilityState === 'visible');
    document.addEventListener('visibilitychange', seen);
    return () => document.removeEventListener('visibilitychange', seen);
  });
  const showPreview = $derived(liveShown(cam, visible));
  let live = $state('starting');
</script>

<SettingsCard title="Camera" {err} loading={!loaded}>
  <LinkHeader line={head}>
    {#snippet action()}
      <!-- as Pumps and Balance: Disconnect (red ink, no box) while it is held, Connect otherwise -->
      {#if holding}
        <button class="unplug" type="button" onclick={disconnect} disabled={busy}
          title="Free the camera: no picture until Connect"><Icon name="unplug" size={15} />Disconnect</button>
      {:else}
        <button class="btn-primary" type="button" onclick={reconnect}
          disabled={busy || !(picked ?? missing) || cam?.state === 'no_helper'}>Connect</button>
      {/if}
    {/snippet}
  </LinkHeader>

  <div class="cam">
    <div class="fields">
      <label class="field"><span>Camera</span>
        <select class="device" value={devKey} onchange={(e) => pickDevice(e.currentTarget.value)} disabled={busy} aria-label="Camera">
          {#if !devKey}<option value="">Pick a camera</option>{/if}
          {#each devices as d (deviceKey(d.name, d.index))}
            <option value={deviceKey(d.name, d.index)}>{deviceLabel(d, devices)}</option>
          {/each}
          {#if missing}
            <option value={deviceKey(missing.name, missing.index)}>{missing.name} (not detected)</option>
          {/if}
        </select>
      </label>
      <p class="field-note">
        {#if looking}Looking for cameras…{:else}{camerasFound(devices.length)}{/if}
        <button class="link" type="button" onclick={() => lookAgain(true)} disabled={looking}>Look again</button> after plugging one in.
      </p>

      <div class="pair">
        <div class="field">
          <span>Picture size</span>
          <div use:segThumb class="seg" role="radiogroup" aria-label="Picture size">
            {#each sizes as s (s.label)}
              {@const on = s.size[0] === width && s.size[1] === height}
              <button type="button" role="radio" aria-checked={on} class:on disabled={busy || !(picked ?? missing)}
                onclick={() => pickSize(s)}>{s.label}</button>
            {/each}
          </div>
        </div>
        <div class="field">
          <span>Live view</span>
          <div class="inline">
            <div use:segThumb class="seg" role="radiogroup" aria-label="Live view, pictures a second">
              {#each FPS_CHOICES as n (n)}
                <button type="button" role="radio" aria-checked={n === fps} class:on={n === fps} disabled={busy}
                  onclick={() => pickFps(n)}>{n}</button>
              {/each}
            </div>
            <span class="unit">a second</span>
          </div>
        </div>
      </div>
    </div>

    <div class="tune">
      <div class="ctl-col">
        {#if controls}
          <div class="pic-head">
            <h3>Picture</h3>
            <div class="pic-actions">
              {#if lockable}
                <button type="button" class="lock" class:locked aria-pressed={locked} onclick={lockPicture} disabled={busy}
                  title={locked ? 'Hand exposure and colour back to the camera' : 'Fix exposure and colour where they are now'}>
                  <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" aria-hidden="true">
                    <path d="M2 16C2 13.1716 2 11.7574 2.87868 10.8787C3.75736 10 5.17157 10 8 10H16C18.8284 10 20.2426 10 21.1213 10.8787C22 11.7574 22 13.1716 22 16C22 18.8284 22 20.2426 21.1213 21.1213C20.2426 22 18.8284 22 16 22H8C5.17157 22 3.75736 22 2.87868 21.1213C2 20.2426 2 18.8284 2 16Z" />
                    <path d={locked ? 'M6 10V8C6 4.68629 8.68629 2 12 2C15.3137 2 18 4.68629 18 8V10' : 'M6 10V8C6 4.68629 8.68629 2 12 2C14.7958 2 17.1449 3.91216 17.811 6.5'} />
                  </svg>
                  {locked ? 'Locked' : 'Lock the picture'}
                </button>
              {/if}
              <button class="link" type="button" onclick={cameraDefaults} disabled={busy || !controls.length}>Camera defaults</button>
            </div>
          </div>

          {#if controls.length}
            <div class="ctls" role="group" aria-label="Picture">
              {#each controls as c (c.control)}
                {@const label = controlLabel(c.control)}
                <div class="ctl">
                  <div class="top">
                    <span class="name">{label}</span>
                    <span class="val" class:auto={c.auto}>{c.control === 'power_line' ? '' : controlText(c)}</span>
                    <span class="rst">
                      {#if controlChanged(c)}
                        <button type="button" class="reset" disabled={busy} aria-label="{label}: back to the camera default" title="Back to the camera default"
                          onclick={() => change([{ control: c.control, value: c.default, auto: c.can_auto }], `${label} back to default`)}>
                          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                            <path d="M4 12a8 8 0 1 0 2.34-5.66" /><path d="M4 4.5V8h3.5" />
                          </svg>
                        </button>
                      {/if}
                    </span>
                    {#if c.can_auto}
                      <label class="field check au">
                        <input type="checkbox" checked={c.auto} disabled={busy}
                          onchange={() => change([{ control: c.control, value: c.value, auto: !c.auto }], `${label} ${c.auto ? 'fixed' : 'auto'}`)} />
                        <span>Auto</span>
                      </label>
                    {/if}
                  </div>
                  {#if c.control === 'power_line'}
                    <div class="slot">
                      <div use:segThumb class="seg" role="radiogroup" aria-label={label}>
                        {#each powerLineOptions(c) as o (o.value)}
                          <button type="button" role="radio" aria-checked={o.value === c.value} class:on={o.value === c.value}
                            disabled={busy} onclick={() => o.value !== c.value && change([{ control: c.control, value: o.value, auto: false }], `${label} ${o.label}`)}>{o.label}</button>
                        {/each}
                      </div>
                    </div>
                  {:else}
                    <input class="slot" type="range" min={c.min} max={c.max} step={c.step} value={c.value}
                      disabled={c.auto} aria-label={label} aria-valuetext={controlText(c)}
                      oninput={(e) => moving(c, Number(e.currentTarget.value))}
                      onchange={(e) => moved(c, Number(e.currentTarget.value))} />
                  {/if}
                </div>
              {/each}
            </div>
          {:else}
            <p class="field-note">This camera has no picture controls.</p>
          {/if}
        {/if}
      </div>

      <figure class="preview">
        <div class="pic">
          {#if showPreview}
            <LiveImg {cam} {pollOk} alt="What this camera sees now" bind:live />
            {#if live === 'stale'}<span class="frozen">Not live</span>{/if}
          {:else}
            <span class="none"><Icon name="camera" size={32} /></span>
          {/if}
        </div>
        <figcaption>What this camera sees now</figcaption>
      </figure>
    </div>

    <div class="timelapse">
      <h3>Timelapse</h3>
      <div class="tl-fields">
        <label class="field every"><span>Every (min)</span>
          <input type="number" min="1" max="1440" step="1" value={intervalMin} disabled={busy}
            aria-label="Timelapse every (minutes)" onchange={(e) => pickInterval(e.currentTarget)} />
        </label>
        <label class="field folder"><span>Images saved in</span>
          <input type="text" value={captures} placeholder={capInfo?.folder ?? ''} disabled={busy} class:bad={folderErr}
            aria-label="Images saved in" aria-invalid={folderErr ? 'true' : undefined}
            onchange={(e) => pickFolder(e.currentTarget)} />
        </label>
      </div>
      {#if folderErr}
        <p class="field-note folder-err">{folderErr}</p>
      {:else if capInfo}
        <p class="field-note">{bytesText(capInfo.used_bytes)} used here{#if capInfo.free_bytes != null}, {bytesText(capInfo.free_bytes)} free on this disk{/if}</p>
      {/if}
    </div>
  </div>

  <p class="done" aria-live="polite">
    {#if last}
      <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M5 12.5 10 17 19 7" /></svg>
      <span>{last}</span>
      {#if history.length}<button class="link" type="button" onclick={undo} disabled={busy}>Undo</button>{/if}
    {/if}
  </p>
</SettingsCard>

<style>
  .cam { display: flex; flex-direction: column; gap: var(--s-4); }
  .fields { min-width: 0; display: flex; flex-direction: column; gap: var(--s-3); }
  /* the controls beside the live picture, so a change is seen as it is made */
  .tune { display: flex; flex-wrap: wrap; gap: var(--s-5); align-items: flex-start; }
  .ctl-col { flex: 1 1 300px; min-width: 0; display: flex; flex-direction: column; gap: var(--s-3); }
  .pair { display: flex; flex-wrap: wrap; gap: var(--s-3) var(--s-6); }
  .pair .seg { align-self: flex-start; }
  .inline { display: flex; align-items: center; gap: var(--s-2); }
  .unit { color: var(--muted); white-space: nowrap; }
  .link {
    padding: 0; border: none; background: none; font: inherit;
    color: var(--teal-ink); text-decoration: underline; cursor: pointer;
  }
  .link:disabled { opacity: 0.5; cursor: default; }

  .pic-head {
    display: flex; flex-wrap: wrap; align-items: center; justify-content: space-between; gap: var(--s-2) var(--s-3);
  }
  .pic-head h3 { margin: 0; font-size: 15px; font-weight: 600; color: var(--ink); }
  .pic-actions { display: flex; flex-wrap: wrap; align-items: center; gap: var(--s-2) var(--s-4); }
  .lock {
    display: inline-flex; align-items: center; gap: 6px;
    padding: 6px 12px; border-radius: var(--radius-ctl); border: 1px solid var(--line);
    background: var(--surface); color: var(--ink); font: inherit; font-weight: 500; cursor: pointer;
  }
  .lock:hover:not(:disabled) { border-color: var(--teal-700); }
  .lock.locked { background: var(--surface-sunken); border-color: var(--teal-700); color: var(--teal-ink); }
  .lock:disabled { opacity: 0.5; cursor: default; }

  .ctls { border-bottom: 1px solid var(--line); }
  .ctl { padding: var(--s-2) 0 var(--s-3); border-top: 1px solid var(--line); display: flex; flex-direction: column; gap: 6px; }
  .top { display: flex; align-items: center; gap: var(--s-2); min-height: 28px; }
  .name { flex: 1; font-weight: 500; color: var(--ink); }
  .rst { width: 24px; display: grid; place-items: center; }
  .slot { min-width: 0; align-self: stretch; }
  .ctl .slot .seg { align-self: flex-start; }
  input[type='range'] { width: 100%; margin: 0; accent-color: var(--teal-700); }
  input[type='range']:disabled { opacity: 0.35; }
  .val { text-align: right; white-space: nowrap; color: var(--ink); }
  .val.auto { color: var(--muted); }
  .reset {
    display: grid; place-items: center; padding: 4px; border: none; border-radius: 6px;
    background: none; color: var(--muted); cursor: pointer;
  }
  .reset:hover:not(:disabled) { color: var(--teal-ink); background: var(--surface-sunken); }
  .ctl .au { margin: 0 0 0 var(--s-2); }

  .preview { flex: 1 1 260px; max-width: 480px; min-width: 0; margin: 0; display: flex; flex-direction: column; gap: var(--s-1); position: sticky; top: var(--s-4); }
  .pic {
    position: relative;
    aspect-ratio: 16 / 9; border-radius: var(--radius-ctl); overflow: hidden;
    background: var(--video-bg); display: grid; place-items: center;
  }
  .frozen {
    position: absolute; top: var(--s-2); left: var(--s-2);
    padding: 2px 8px; border-radius: var(--radius-tag);
    background: var(--video-tag); color: var(--video-muted); font-size: 12px;
  }
  .none { color: var(--video-muted); display: grid; }
  figcaption { font-size: 12px; color: var(--muted); }

  .timelapse { display: flex; flex-direction: column; gap: var(--s-2); padding-top: var(--s-3); border-top: 1px solid var(--line); }
  .timelapse h3 { margin: 0; font-size: 15px; font-weight: 600; color: var(--ink); }
  .tl-fields { display: flex; flex-wrap: wrap; gap: var(--s-3) var(--s-5); align-items: flex-end; }
  .every input { width: 96px; }
  .folder { flex: 1 1 320px; min-width: 0; }
  .folder input { width: 100%; }
  .folder input.bad { border-color: var(--measured); }
  .folder-err { color: var(--warn-ink); }

  .done {
    margin: var(--s-4) 0 0; min-height: 22px;
    display: flex; flex-wrap: wrap; align-items: center; gap: var(--s-2); color: var(--muted);
  }
  .done svg { color: var(--green-ink); }
</style>
