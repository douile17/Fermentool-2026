<script>
  // History's Images section: a run's timelapse, its pictures spread over
  // the run, Play (the player) and Open folder (Explorer, opened by the
  // daemon: the desktop window opens nothing itself).
  import { BASE, get, post } from '../lib/api.js';
  import { bytesText, everyWords } from '../lib/timelapse.js';
  import { frameClock, spreadPick } from '../lib/tlplayer.js';
  import Icon from './Icon.svelte';

  // tl: the run's `timelapse` from GET /api/runs/:id
  let { tl, onplay } = $props();

  let frames = $state([]);
  let err = $state(null);
  $effect(() => {
    const id = tl.id;
    frames = [];
    get(`/api/timelapses/${id}/frames`)
      .then((f) => (frames = f))
      .catch((e) => (err = e.message));
  });
  const picks = $derived(spreadPick(frames, 6));

  let opening = $state(false);
  async function openFolder() {
    opening = true;
    err = null;
    try {
      await post(`/api/timelapses/${tl.id}/open`);
    } catch (e) {
      err = e.status === 404 ? 'The folder is not there any more.' : e.message;
    }
    opening = false;
  }
  async function openImage(id) {
    err = null;
    try {
      await post(`/api/timelapses/images/${id}/open`);
    } catch (e) {
      err = e.message;
    }
  }
</script>

<section class="block images" aria-labelledby="run-images">
  <div class="head">
    <div>
      <h3 id="run-images">Images</h3>
      <span class="k">
        {tl.images} image{tl.images === 1 ? '' : 's'}, one every {everyWords(tl.interval_s)}{tl.bytes ? `, ${bytesText(tl.bytes)}` : ''}
      </span>
    </div>
  </div>
  {#if !tl.folder_exists}
    <p class="gone">The folder is not there any more: moved, renamed or deleted. <span class="path">{tl.folder}</span></p>
  {:else}
    {#if picks.length}
      <div class="thumbs">
        {#each picks as f (f.id)}
          <button class="thumb" type="button" title="Open" onclick={() => openImage(f.id)}>
            <img src={`${BASE}/api/timelapses/images/${f.id}/thumb.jpg`} alt={`Image at ${frameClock(f.elapsed_s)}`} loading="lazy" draggable="false" />
            <span class="mono">{frameClock(f.elapsed_s)}</span>
          </button>
        {/each}
      </div>
    {/if}
    <div class="acts">
      <button class="btn-play" type="button" disabled={frames.length === 0} onclick={() => onplay(frames)}>
        <Icon name="play" size={14} />Play
      </button>
      <button class="btn-folder" type="button" disabled={opening} onclick={openFolder}>
        <Icon name="folder" size={16} />Open folder
      </button>
    </div>
    <p class="path">{tl.folder}</p>
  {/if}
  {#if err}<p class="err">{err}</p>{/if}
</section>

<style>
  .images { display: flex; flex-direction: column; gap: var(--s-3); }
  .head { display: flex; justify-content: space-between; align-items: center; gap: var(--s-3); flex-wrap: wrap; }
  h3 { margin: 0; }
  .k { font-size: 13px; color: var(--muted); }
  .acts { display: flex; gap: var(--s-2); }
  .acts button { display: inline-flex; align-items: center; gap: var(--s-2); }
  .thumbs { display: grid; grid-template-columns: repeat(6, minmax(0, 1fr)); gap: var(--s-2); }
  .thumb {
    display: flex; flex-direction: column; gap: var(--s-1); padding: 0;
    border: 0; background: transparent; cursor: pointer; font: inherit; text-align: left;
  }
  .thumb img {
    width: 100%; aspect-ratio: 16 / 9; object-fit: cover; display: block;
    border-radius: 6px; background: var(--surface-sunken);
  }
  .thumb:hover img { outline: 2px solid var(--teal-400); outline-offset: 1px; }
  .thumb span { font-size: 12px; color: var(--muted); }
  .path { margin: 0; font-size: 12px; color: var(--muted); overflow-wrap: anywhere; }
  .gone { margin: 0; font-size: 13px; color: var(--warn-ink); }
  .gone .path { display: block; margin-top: var(--s-1); }
  .err { margin: 0; font-size: 13px; color: var(--danger-ink); }
</style>
