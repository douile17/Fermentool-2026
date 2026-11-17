<script>
  import { app } from '../lib/state.svelte.js';
  import { BASE, get, post } from '../lib/api.js';
  import { cameraLine, liveShown, noPictureSince, pictureSaved, runClock } from '../lib/camera.js';
  import Icon from '../components/Icon.svelte';
  import LiveImg from '../components/LiveImg.svelte';
  import TimelapseCard from '../components/TimelapseCard.svelte';

  // The camera's live view, Take a picture, the latest images and the
  // Timelapse card (mockup of 2026-11-02; the player and the Timelapses tab
  // come with plan D). The run's state bar is App's, above every page but
  // the Overview.

  // GET /api/camera every 2 s while the page is open: the status frame does
  // not carry the camera. A failed poll keeps the last answer but marks the
  // picture not live (the daemon stopped).
  let cam = $state(null);
  let pollOk = $state(true);
  $effect(() => {
    let live = true;
    const load = () => {
      get('/api/camera')
        .then((c) => {
          if (!live) return;
          cam = c;
          pollOk = true;
        })
        .catch(() => live && (pollOk = false));
      loadTimelapse();
    };
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

  // The timelapse recording or paused, with the camera's poll.
  let tl = $state(null);
  let warnFree = $state(false);
  let defaultMin = $state(5);
  const loadTimelapse = () =>
    get('/api/timelapses/current')
      .then((r) => {
        tl = r.timelapse;
        warnFree = r.warn_free;
      })
      .catch(() => {});
  $effect(() => {
    get('/api/config')
      .then((c) => (defaultMin = c.camera?.interval_min ?? 5))
      .catch(() => {});
  });

  // The latest 8 images of the current timelapse, asked again when its
  // count changes.
  let latest = $state([]);
  const tlKey = $derived(tl ? `${tl.id}:${tl.images}` : null);
  $effect(() => {
    if (!tlKey) {
      latest = [];
      return;
    }
    const id = tl.id;
    get(`/api/timelapses/${id}/images?limit=8`)
      .then((list) => (latest = list))
      .catch(() => {});
  });

  // Take a picture: into the timelapse recording, or today's pictures.
  let taking = $state(false);
  let picMsg = $state(null);
  let picErr = $state(null);
  let flashId = $state(null);
  // Open (Windows' image viewer) and Show in folder (Explorer, the image
  // picked): the daemon opens them on this PC, so they work in the desktop
  // window too, whose webview opens no new tab.
  async function imageAction(id, what) {
    picErr = null;
    try {
      await post(`/api/timelapses/images/${id}/${what}`);
    } catch (e) {
      picErr = e.message;
    }
  }

  async function takePicture() {
    taking = true;
    picErr = null;
    picMsg = null;
    try {
      const r = await post('/api/camera/picture');
      flashId = r.image_id;
      picMsg = pictureSaved(r.folder);
      loadTimelapse();
    } catch (e) {
      picErr = e.message;
    }
    taking = false;
  }
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

    <div class="cols">
    <div class="main">
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
        <button class="btn-ghost shot" type="button" onclick={takePicture} disabled={taking || cam.state !== 'connected'}>
          <Icon name="camera" size={16} />{taking ? 'Taking…' : 'Take a picture'}
        </button>
        <button class="btn-ghost full" type="button" onclick={fullScreen} disabled={!showLive} aria-label="Full screen" title="Full screen">
          <Icon name="full-screen" size={18} />
        </button>
      </div>
      {#if picErr}<p class="pic-err">{picErr}</p>{:else if picMsg}
        <p class="saved" id="cam-saved">
          <span class="said"><Icon name="check" size={16} />{picMsg}</span>
          <button class="btn-ghost act" type="button" onclick={() => imageAction(flashId, 'open')}>Open</button>
          <button class="btn-ghost act" type="button" onclick={() => imageAction(flashId, 'show')}>Show in folder</button>
        </p>
      {/if}
    </section>

    {#if latest.length}
      <section class="card strip" aria-label="Latest images">
        <h2>Latest images</h2>
        <ul>
          {#each latest as img (img.id)}
            <li class:flash={img.id === flashId}>
              <button class="thumb" type="button" onclick={() => imageAction(img.id, 'open')} title="Open">
                <img src={`${BASE}/api/timelapses/images/${img.id}/thumb.jpg`} alt={`Image at ${runClock(img.elapsed_s)}`} loading="lazy" />
              </button>
              <span>{runClock(img.elapsed_s)}{img.slot == null ? ', by hand' : ''}</span>
            </li>
          {/each}
        </ul>
      </section>
    {/if}
    </div>
    <aside class="side">
      <TimelapseCard cur={tl} {warnFree} {cam} {defaultMin} onchanged={loadTimelapse} />
    </aside>
    </div>
  {:else}
    <p class="muted">Loading…</p>
  {/if}
</div>

<style>
  .cols { display: flex; flex-wrap: wrap; gap: var(--s-4); align-items: flex-start; }
  .main { flex: 999 1 480px; min-width: 0; display: flex; flex-direction: column; gap: var(--s-4); }
  .side { flex: 1 1 280px; min-width: 0; }
  .card.live { padding: var(--s-4); display: flex; flex-direction: column; gap: var(--s-4); }
  .pic-err { color: var(--danger-ink); margin: 0; }
  .saved { margin: 0; display: flex; flex-wrap: wrap; align-items: center; gap: var(--s-2); }
  .said { display: inline-flex; align-items: center; gap: var(--s-2); color: var(--muted); margin-right: var(--s-2); }
  /* the picture's two ways out, ghost buttons like Take a picture, smaller */
  .act { height: 32px; padding: 0 var(--s-3); display: inline-flex; align-items: center; gap: var(--s-2); font-size: 13px; color: var(--ink); text-decoration: none; }
  .strip h2 { margin: 0 0 var(--s-3); font-size: 16px; }
  .strip ul { list-style: none; margin: 0; padding: 0; display: grid; grid-template-columns: repeat(auto-fill, minmax(120px, 1fr)); gap: var(--s-3); }
  .strip li { display: flex; flex-direction: column; gap: 4px; font-size: 12px; color: var(--muted); font-family: var(--mono); }
  .strip img { width: 100%; aspect-ratio: 16 / 9; object-fit: cover; border-radius: 6px; background: var(--video-bg); display: block; }
  .thumb { padding: 0; border: 0; background: none; display: block; width: 100%; border-radius: 6px; }
  .strip li.flash img { outline: 2px solid var(--green-500); outline-offset: 2px; }
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
  /* the icon centred on the words beside it, not sat on their baseline */
  .shot { height: 40px; display: inline-flex; align-items: center; gap: var(--s-2); }
  /* 1 px lower: the words read at their lowercase middle, under the line box's */
  .shot :global(svg) { position: relative; top: 1px; }
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
