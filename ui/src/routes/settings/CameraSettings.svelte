<script>
  import { BASE, get, post } from '../../lib/api.js';
  import { loadConfig } from '../../lib/config.js';
  import { cameraLine, camerasFound, deviceKey, deviceLabel, liveSrc, sizesFor } from '../../lib/camera.js';
  import { segThumb } from '../../lib/segthumb.js';
  import Icon from '../../components/Icon.svelte';
  import LinkHeader from './LinkHeader.svelte';
  import SettingsCard from './SettingsCard.svelte';

  // `group` (passed to every section) is not shown here: the side menu says it.
  // As Pumps and Balance: the link's state first, then the camera and its
  // size. Connect saves `[camera]` and connects (POST /api/camera/connect);
  // Save does the same once the camera or its size changed. "Timelapse,
  // every" and "Images kept in" come with the timelapse.

  let loaded = $state(false);
  let err = $state(null);
  let msg = $state(null);
  let saving = $state(false);

  // the camera's state, every 2 s while the section is open
  let cam = $state(null);
  $effect(() => {
    let live = true;
    const load = () =>
      get('/api/camera')
        .then((c) => live && (cam = c))
        .catch(() => {});
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
  $effect(() => lookAgain(false));

  // the form, and what config.toml has (Save waits for a change)
  let devKey = $state('');
  let width = $state(1280);
  let height = $state(720);
  let saved = $state(null);
  let savedDevice = $state(null);
  let rev = $state(null);
  const formKey = () => JSON.stringify([devKey, width, height]);
  const dirty = $derived(saved !== null && formKey() !== saved);

  function loadSaved() {
    return loadConfig().then((c) => {
      const s = c.camera;
      devKey = s?.device ? deviceKey(s.device, s.device_index ?? 0) : '';
      width = s?.width ?? 1280;
      height = s?.height ?? 720;
      savedDevice = s?.device ? { name: s.device, index: s.device_index ?? 0 } : null;
      rev = c._rev ?? null;
      saved = formKey();
      loaded = true;
    });
  }
  $effect(() => {
    loadSaved().catch((e) => (err = e.message));
  });

  const picked = $derived(devices.find((d) => deviceKey(d.name, d.index) === devKey) ?? null);
  // the saved camera, while the list does not have it (unplugged)
  const missing = $derived(
    listed && savedDevice && !devices.some((d) => deviceKey(d.name, d.index) === deviceKey(savedDevice.name, savedDevice.index))
      ? savedDevice
      : null,
  );
  const sizes = $derived(sizesFor(picked));

  function pickDevice(key) {
    // a size the new camera lacks: 1280 × 720 if it has it, else its first
    const offered = sizesFor(devices.find((d) => deviceKey(d.name, d.index) === key) ?? null);
    if (!offered.some((s) => s.size[0] === width && s.size[1] === height)) {
      const s = offered.find((o) => o.size[0] === 1280) ?? offered[0];
      if (s) [width, height] = s.size;
    }
  }

  // Connect, and Save: the camera and size of the form, saved then connected.
  async function connect() {
    const dev = picked ?? missing;
    if (!dev) return;
    saving = true;
    err = null;
    msg = null;
    try {
      await post('/api/camera/connect', { device: dev.name, index: dev.index, width, height, _rev: rev });
      await loadSaved();
      msg = `Saved. Connecting to ${dev.name}, ${width} × ${height}.`;
    } catch (e) {
      err = e.message;
    }
    saving = false;
  }

  async function disconnect() {
    saving = true;
    err = null;
    msg = null;
    try {
      await post('/api/camera/disconnect');
      await loadSaved();
      msg = 'Camera closed: no picture until you Connect.';
    } catch (e) {
      err = e.message;
    }
    saving = false;
  }

  // "What this camera sees now": the live view while the section is open
  // and the camera connected (one viewer: the daemon grabs only for one, a
  // polled frame.jpg stayed the picture of the Connect).
  const showPreview = $derived(cam?.state === 'connected');
</script>

<SettingsCard title="Camera" {err} {msg} loading={!loaded}>
  <LinkHeader line={head}>
    {#snippet action()}
      <!-- as Pumps and Balance: Disconnect (red ink, no box) while it is held, Connect otherwise -->
      {#if holding}
        <button class="unplug" type="button" onclick={disconnect} disabled={saving}
          title="Free the camera: no picture until Connect"><Icon name="unplug" size={15} />Disconnect</button>
      {:else}
        <button class="btn-primary" type="button" onclick={connect}
          disabled={saving || !(picked ?? missing) || cam?.state === 'no_helper'}>Connect</button>
      {/if}
    {/snippet}
  </LinkHeader>

  <div class="cam">
    <div class="fields">
      <label class="field"><span>Camera</span>
        <select class="device" bind:value={devKey} onchange={(e) => pickDevice(e.currentTarget.value)} aria-label="Camera">
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
      <div class="field">
        <span>Picture size</span>
        <div use:segThumb class="seg" role="radiogroup" aria-label="Picture size">
          {#each sizes as s (s.label)}
            {@const on = s.size[0] === width && s.size[1] === height}
            <button type="button" role="radio" aria-checked={on} class:on onclick={() => ([width, height] = s.size)}>{s.label}</button>
          {/each}
        </div>
      </div>
    </div>

    <figure class="preview">
      <div class="pic">
        {#if showPreview}
          <img use:liveSrc={BASE + '/api/camera/live'} alt="What this camera sees now" />
        {:else}
          <span class="none"><Icon name="camera" size={32} /></span>
        {/if}
      </div>
      <figcaption>What this camera sees now</figcaption>
    </figure>
  </div>

  {#snippet foot()}
    <button class="btn-primary save" disabled={saving || !dirty || !(picked ?? missing)} onclick={connect}>{saving ? 'Saving…' : 'Save'}</button>
  {/snippet}
</SettingsCard>

<style>
  .cam { display: flex; flex-wrap: wrap; gap: var(--s-5); align-items: flex-start; }
  .fields { flex: 1 1 300px; min-width: 0; display: flex; flex-direction: column; gap: var(--s-3); }
  .fields .seg { align-self: flex-start; }
  .link {
    padding: 0; border: none; background: none; font: inherit;
    color: var(--teal-700); text-decoration: underline; cursor: pointer;
  }
  .preview { flex: 0 1 280px; min-width: 0; margin: 0; display: flex; flex-direction: column; gap: var(--s-1); }
  .pic {
    aspect-ratio: 16 / 9; border-radius: var(--radius-ctl); overflow: hidden;
    background: var(--video-bg); display: grid; place-items: center;
  }
  .pic img { width: 100%; height: 100%; object-fit: contain; display: block; }
  .none { color: var(--video-muted); display: grid; }
  figcaption { font-size: 12px; color: var(--muted); }
</style>
