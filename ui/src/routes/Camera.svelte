<script>
  import { app } from '../lib/state.svelte.js';
  import { get } from '../lib/api.js';
  import { cameraLine, liveShown, noPictureSince, runClock } from '../lib/camera.js';
  import Icon from '../components/Icon.svelte';
  import LiveImg from '../components/LiveImg.svelte';

  // The camera's live view (mockup of 2026-11-02, its Live part; Take a
  // picture and the Timelapses tab come with the timelapse). The run's state
  // bar is App's, above every page but the Overview.

  // GET /api/camera every 2 s while the page is open: the status frame does
  // not carry the camera. A failed poll keeps the last answer but marks the
  // picture not live (the daemon stopped).
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

  // The live view only while the page is visible: a hidden tab or a
  // minimised window closes the stream, and the daemon stops grabbing.
  let visible = $state(typeof document === 'undefined' || document.visibilityState === 'visible');
  $effect(() => {
    const seen = () => (visible = document.visibilityState === 'visible');
    document.addEventListener('visibilitychange', seen);
    return () => document.removeEventListener('visibilitychange', seen);
  });

  const line = $derived(cameraLine(cam));
  const showLive = $derived(liveShown(cam, visible));
  // 'live', 'starting' or 'stale' (LiveImg): Live is said only when it is
  let live = $state('starting');
  const chosen = $derived(!!cam && (cam.configured || cam.state !== 'off'));

  // The run's time over the picture: a dosing run only, as the state bar.
  // The status carries no elapsed figure: counted from `started_at` as the
  // Overview does.
  const run = $derived(app.status?.active && app.status.active.kind !== 'calibration' ? app.status.active : null);
  let now = $state(Date.now());
  $effect(() => {
    const t = setInterval(() => (now = Date.now()), 1000);
    return () => clearInterval(t);
  });
  const runTime = $derived(run ? runClock((now - Date.parse(run.started_at)) / 1000) : null);

  // The stream is LiveImg's <img>: mounted only while the camera is
  // connected and this page shown, it ends with either (the daemon counts it
  // as a viewer while it loads).

  let box = $state(null);
  function fullScreen() {
    box?.requestFullscreen?.().catch(() => {});
  }

  function openSettings(e) {
    e.preventDefault();
    app.settingsSection = 'camera';
    app.route = 'settings';
  }

  const size = $derived(cam?.width && cam?.height ? `${cam.width} × ${cam.height}` : null);
</script>

<div class="camera">
  {#if cam && !chosen}
    <section class="card empty">
      <span class="empty-ico" aria-hidden="true"><Icon name="camera" size={32} /></span>
      <h2>No camera chosen</h2>
      <p class="muted">Pick a camera in <a href="#settings/camera" onclick={openSettings}>Settings, Camera</a> to see the vessel from here.</p>
    </section>
  {:else if cam}
    {#if line.tone === 'warn'}
      <div class="band" role="status">
        <span class="band-ico" aria-hidden="true"><Icon name="camera" size={28} /></span>
        <div class="band-txt">
          <span class="band-word">{line.word}</span>
          <span class="band-detail">{line.detail}</span>
        </div>
      </div>
    {/if}

    <section class="card live">
      <div class="pic" bind:this={box} style:aspect-ratio={cam.width && cam.height ? `${cam.width} / ${cam.height}` : '16 / 9'}>
        {#if showLive}
          <LiveImg {cam} {pollOk} alt="Live view of the camera" bind:live />
          {#if live === 'live'}
            <span class="tag live-tag"><span class="dot" aria-hidden="true"></span>Live</span>
          {:else if live === 'stale'}
            <span class="tag live-tag frozen"><span class="dot" aria-hidden="true"></span>Not live</span>
          {/if}
          {#if runTime}<span class="tag time-tag">{runTime}</span>{/if}
        {:else}
          <div class="none">
            <Icon name="camera" size={40} />
            <span>{line.tone === 'warn' ? noPictureSince(cam) : line.word}</span>
            {#if cam.state === 'off'}
              <a href="#settings/camera" onclick={openSettings}>Connect it in Settings, Camera</a>
            {/if}
          </div>
        {/if}
      </div>
      <div class="under">
        {#if cam.device}
          <span class="name"><span class="nw">{cam.device}</span>{#if size} · <span class="nw">{size}</span>{/if}</span>
        {/if}
        <span class="gap"></span>
        <button class="btn-ghost full" type="button" onclick={fullScreen} disabled={!showLive} aria-label="Full screen" title="Full screen">
          <Icon name="full-screen" size={18} />
        </button>
      </div>
    </section>
  {:else}
    <p class="muted">Loading…</p>
  {/if}
</div>

<style>
  .card.live { padding: var(--s-4); display: flex; flex-direction: column; gap: var(--s-4); }
  .pic {
    position: relative; overflow: hidden; border-radius: var(--radius-ctl);
    background: var(--video-bg); width: 100%;
  }
  .pic:fullscreen { border-radius: 0; aspect-ratio: auto; }
  .tag {
    position: absolute; top: var(--s-3);
    display: inline-flex; align-items: center; gap: 6px;
    padding: 2px 8px; border-radius: var(--radius-tag);
    background: var(--video-tag); color: var(--video-ink);
    font-size: 12px; white-space: nowrap;
  }
  .live-tag { left: var(--s-3); font-weight: 600; }
  .time-tag { right: var(--s-3); font-family: var(--mono); }
  .dot { width: 7px; height: 7px; border-radius: 50%; background: var(--green-500); }
  .frozen { color: var(--video-muted); }
  .frozen .dot { background: var(--video-muted); }
  .none {
    position: absolute; inset: 0;
    display: flex; flex-direction: column; align-items: center; justify-content: center; gap: var(--s-2);
    color: var(--video-muted);
  }
  .none a { color: var(--video-ink); }
  .under { display: flex; flex-wrap: wrap; align-items: center; gap: var(--s-3); }
  .name { color: var(--muted); }
  .nw { white-space: nowrap; }
  .gap { flex: 1 1 auto; }
  .full { width: 40px; height: 40px; padding: 0; display: inline-grid; place-items: center; }

  .band {
    display: flex; align-items: center; gap: var(--s-4);
    padding: var(--s-4) var(--s-5); margin-bottom: var(--s-4);
    background: var(--warn-bg); border: 1px solid var(--warn-line); border-radius: var(--radius);
    color: var(--warn-ink);
  }
  .band-ico { flex: none; display: grid; }
  .band-txt { display: flex; flex-direction: column; min-width: 0; }
  .band-word { font-family: var(--font-title); font-weight: 600; font-size: 20px; line-height: 1.2; }
  .band-detail { color: var(--warn-ink); }

  .card.empty { display: flex; flex-direction: column; align-items: center; text-align: center; gap: var(--s-2); }
  .empty-ico { color: var(--muted); display: grid; }
  .empty h2 { font-size: 16px; }
  .muted { color: var(--muted); margin: 0; }

  @media (max-width: 640px) {
    .card.live { padding: var(--s-3); }
    .band { padding: var(--s-3) var(--s-4); }
  }
</style>
