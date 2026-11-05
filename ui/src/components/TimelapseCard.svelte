<script>
  import { app } from '../lib/state.svelte.js';
  import { post } from '../lib/api.js';
  import { shortTime, stamp } from '../lib/fmt.js';
  import { bytesText, cardState, estimate, everyWords, freeWarning, nextInText, perDayText } from '../lib/timelapse.js';

  // The Camera page's Timelapse card (mockup of 2026-11-02, its four
  // states, and the pauses of plan C): `cur` and `warnFree` are GET
  // /api/timelapses/current, `cam` GET /api/camera, polled by the page;
  // `defaultMin` is Settings' interval; `onchanged()` asks the page to poll
  // again after a change.
  let { cur, warnFree = false, cam, defaultMin = 5, onchanged = () => {} } = $props();

  const state = $derived(cardState(cur, cam));
  const warning = $derived(freeWarning(cur, warnFree));
  const runGoing = $derived(app.status?.active != null);

  let now = $state(Date.now());
  $effect(() => {
    const t = setInterval(() => (now = Date.now()), 1000);
    return () => clearInterval(t);
  });

  let busy = $state(false);
  let err = $state(null);
  async function act(path, body) {
    busy = true;
    err = null;
    try {
      await post(path, body);
      onchanged();
    } catch (e) {
      err = e.message;
    }
    busy = false;
  }

  // the form, with nothing recording
  let name = $state('');
  let everyMin = $state(null);
  let stopAfterH = $state(null);
  const every = $derived(everyMin ?? defaultMin);
  const size = $derived([cam?.width ?? 1280, cam?.height ?? 720]);
  const perDay = $derived(estimate(86400, every, size[0], size[1]));
  const formOk = $derived(name.trim().length > 0 && every >= 1 && every <= 1440 && (stopAfterH == null || stopAfterH >= 0.1));
  function start() {
    act('/api/timelapses', {
      name: name.trim(),
      interval_min: every,
      stop_after_h: stopAfterH || null,
    });
  }

  // Change interval: an inline field, applied on Enter or leaving it
  let editing = $state(false);
  function setInterval_(input) {
    const n = Math.round(Number(input.value));
    editing = false;
    if (n >= 1 && n <= 1440 && n * 60 !== cur.interval_s) act('/api/timelapses/current/interval', { interval_min: n });
  }

  const everyText = $derived(cur ? everyWords(cur.interval_s) : '');
</script>

<section class="card tl" aria-label="Timelapse">
  <h2>Timelapse</h2>

  {#if state === 'waiting'}
    <div class="band" role="status">
      <b>Camera not answering</b>
      <span>The run is not affected. {cur.missed} image{cur.missed === 1 ? '' : 's'} missed so far.</span>
    </div>
  {:else if warning}
    <div class="band" role="status"><b>{warning}</b></div>
  {/if}

  {#if state === 'none'}
    {#if runGoing}
      <p class="muted">A run is going: switch its timelapse on in New run.</p>
    {:else}
      <label class="field"><span>Name</span><input type="text" bind:value={name} placeholder="Before the feed" disabled={busy} /></label>
      <div class="pair">
        <label class="field"><span>Every (min)</span>
          <input type="number" min="1" max="1440" step="1" value={every} disabled={busy}
            oninput={(e) => (everyMin = Number(e.currentTarget.value) || null)} /></label>
        <label class="field"><span>Stop after (h)</span>
          <input type="number" min="0.1" step="0.5" placeholder="optional" value={stopAfterH ?? ''} disabled={busy}
            oninput={(e) => (stopAfterH = Number(e.currentTarget.value) || null)} /></label>
      </div>
      {#if perDay}<p class="muted">{perDayText(perDay)}</p>{/if}
      <button class="btn-primary" type="button" onclick={start} disabled={busy || !formOk || cam?.state !== 'connected'}>Start timelapse</button>
    {/if}
  {:else}
    <p class="rec">
      {#if state === 'paused'}<b class="word paused">Paused</b>
      {:else if state === 'awaiting'}<b class="word paused">Waiting for Resume</b>
      {:else if state === 'waiting'}<b class="word paused">Waiting for the camera</b>
      {:else}<b class="word">Recording</b>{/if}
      {#if cur.kind === 'run'}<span class="muted">Started with run {cur.run_id}, stops with it</span>
      {:else}<span class="muted nw">{cur.name}</span>{/if}
    </p>
    <dl class="figs">
      <dt>Every</dt>
      <dd>
        {#if editing}
          <!-- svelte-ignore a11y_autofocus -->
          <input class="every" type="number" min="1" max="1440" step="1" value={Math.round(cur.interval_s / 60)} autofocus
            aria-label="Every (minutes)" onchange={(e) => setInterval_(e.currentTarget)} onblur={() => (editing = false)} />
          min
        {:else}{everyText}{/if}
      </dd>
      <dt>Images</dt>
      <dd>{cur.images}{#if cur.missed}, {cur.missed} missed{/if}</dd>
      {#if cur.next_at}<dt>Next image</dt><dd>{nextInText(cur.next_at, now)}</dd>{/if}
      <dt>On disk</dt>
      <dd>{bytesText(cur.bytes)}{#if cur.free_bytes != null}, {bytesText(cur.free_bytes)} free{/if}</dd>
      {#if cur.ends_at}<dt>Ends</dt><dd>{stamp(cur.ends_at)}</dd>{/if}
      <dt>Since</dt>
      <dd>{shortTime(cur.started_at)}</dd>
    </dl>
    <div class="actions">
      {#if !editing}
        <button class="btn-ghost" type="button" disabled={busy} onclick={() => (editing = true)}>Change interval</button>
      {/if}
      {#if cur.state === 'paused'}
        <button class="btn-ghost" type="button" disabled={busy} onclick={() => act('/api/timelapses/current/resume')}>Resume</button>
      {:else if state !== 'awaiting'}
        <button class="btn-ghost" type="button" disabled={busy} onclick={() => act('/api/timelapses/current/pause')}>Pause</button>
      {/if}
      {#if cur.kind !== 'run'}
        <button class="btn-ghost" type="button" disabled={busy} onclick={() => act('/api/timelapses/current/stop')}>Stop timelapse</button>
      {/if}
    </div>
  {/if}
  {#if err}<p class="err">{err}</p>{/if}
</section>

<style>
  .tl { display: flex; flex-direction: column; gap: var(--s-3); }
  .tl h2 { margin: 0; font-size: 16px; }
  .band {
    display: flex; flex-direction: column; gap: 2px;
    padding: var(--s-3); border-radius: var(--radius-ctl);
    background: var(--warn-bg); border: 1px solid var(--warn-line); color: var(--warn-ink);
  }
  .muted { color: var(--muted); margin: 0; }
  .pair { display: flex; gap: var(--s-3); }
  .pair .field { flex: 1 1 0; min-width: 0; }
  .field { display: flex; flex-direction: column; gap: 4px; }
  .field input { width: 100%; box-sizing: border-box; }
  .rec { margin: 0; display: flex; flex-direction: column; gap: 2px; }
  /* Recording is quiet green ink, never a red dot (DESIGN.md) */
  .word { color: var(--green-ink); font-weight: 600; }
  .word.paused { color: var(--warn-ink); }
  .figs { margin: 0; display: grid; grid-template-columns: auto minmax(0, 1fr); gap: var(--s-1) var(--s-3); }
  .figs dt { color: var(--muted); }
  .figs dd { margin: 0; }
  .every { width: 72px; }
  .actions { display: flex; flex-wrap: wrap; gap: var(--s-2); }
  .nw { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .err { color: var(--danger-ink); margin: 0; }
</style>
