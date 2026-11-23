<script>
  // A run's images one after the other, as an animation (History > Images >
  // Play). The image shown is the full one, fetched by id; the next few are
  // loaded ahead and the player waits for one not there yet rather than skip
  // it. Space plays or pauses, the arrows step one image, Escape closes.
  import { BASE } from '../lib/api.js';
  import { PLAYER_SPEEDS, frameClock, frameFigures, frameStamp, playFrom, stepFrame, wholeRunWords } from '../lib/tlplayer.js';
  import Icon from './Icon.svelte';

  let { title, frames, onclose } = $props();

  const AHEAD = 8;
  let i = $state(0);
  let playing = $state(false);
  let fps = $state(10);
  const n = $derived(frames.length);
  const cur = $derived(frames[i] ?? null);
  const src = (f) => `${BASE}/api/timelapses/images/${f.id}/full.jpg`;

  // images loaded ahead, by frame id; kept small
  const cache = new Map();
  function load(k) {
    const f = frames[k];
    if (!f || cache.has(f.id)) return;
    const img = new Image();
    img.decoding = 'async';
    img.src = src(f);
    cache.set(f.id, img);
    if (cache.size > AHEAD * 4) cache.delete(cache.keys().next().value);
  }
  const ready = (k) => {
    const img = cache.get(frames[k]?.id);
    return !!img && img.complete;
  };
  $effect(() => {
    for (let k = i; k <= i + AHEAD; k++) load(k);
  });

  $effect(() => {
    if (!playing) return;
    const t = setInterval(() => {
      if (i >= n - 1) {
        playing = false;
        return;
      }
      // a missing image waits, never skipped
      if (ready(i + 1)) i += 1;
    }, 1000 / fps);
    return () => clearInterval(t);
  });

  function toggle() {
    if (!playing) i = playFrom(i, n);
    playing = !playing;
  }
  function step(d) {
    playing = false;
    i = stepFrame(i, n, d);
  }

  function onKey(e) {
    if (e.key === 'Escape') {
      e.stopPropagation();
      onclose();
    } else if (e.key === ' ' && !(e.target instanceof HTMLButtonElement)) {
      e.preventDefault();
      toggle();
    } else if (e.key === 'ArrowLeft' && !(e.target instanceof HTMLInputElement)) {
      step(-1);
    } else if (e.key === 'ArrowRight' && !(e.target instanceof HTMLInputElement)) {
      step(1);
    }
  }

  // the page under it stays still: no second scrollbar beside the player
  $effect(() => {
    const html = document.documentElement;
    const before = html.style.overflow;
    html.style.overflow = 'hidden';
    return () => (html.style.overflow = before);
  });

  let dialog = $state(null);
  $effect(() => {
    dialog?.querySelector('.play')?.focus();
  });
</script>

<svelte:window onkeydowncapture={onKey} />

<div class="player" role="dialog" aria-modal="true" aria-label={`${title}, images`} bind:this={dialog}>
  <div class="top">
    <div class="name">
      <b>{title}, images</b>
      <span>{n} image{n === 1 ? '' : 's'}</span>
    </div>
    <button class="x" type="button" aria-label="Close" title="Close (Esc)" onclick={onclose}><Icon name="x" size={20} /></button>
  </div>

  <div class="stage">
    {#if cur}
      <img src={src(cur)} alt={`Image at ${frameClock(cur.elapsed_s)}`} draggable="false" />
      <div class="caption">
        <b class="mono">{frameClock(cur.elapsed_s)}</b>
        {#if frameFigures(cur)}<span class="mono">{frameFigures(cur)}</span>{/if}
      </div>
    {:else}
      <p class="none">No image in this run.</p>
    {/if}
  </div>

  {#if cur && frameStamp(cur.taken_at)}
    <p class="when mono" title="When this image was taken">{frameStamp(cur.taken_at)}</p>
  {/if}

  {#if n > 1}
    <div class="scrub">
      <input type="range" min="0" max={n - 1} step="1" value={i} aria-label="Position in the run"
        oninput={(e) => { playing = false; i = Number(e.currentTarget.value); }} />
      <div class="ends">
        <span class="mono">{frameClock(frames[0].elapsed_s)}</span>
        <span class="mono">Image {i + 1} of {n}</span>
        <span class="mono">{frameClock(frames[n - 1].elapsed_s)}</span>
      </div>
    </div>

    <div class="controls">
      <button class="step" type="button" aria-label="Previous image" title="Previous image (←)" disabled={i === 0} onclick={() => step(-1)}>
        <Icon name="chevron-left" size={20} />
      </button>
      <button class="play" type="button" onclick={toggle} title={playing ? 'Pause (Space)' : 'Play (Space)'}>
        <Icon name={playing ? 'pause' : 'play'} size={16} />{playing ? 'Pause' : 'Play'}
      </button>
      <button class="step" type="button" aria-label="Next image" title="Next image (→)" disabled={i === n - 1} onclick={() => step(1)}>
        <Icon name="chevron-right" size={20} />
      </button>
      <span class="grow"></span>
      <span class="lbl" id="tl-speed">Speed</span>
      <div class="speeds" role="radiogroup" aria-labelledby="tl-speed">
        {#each PLAYER_SPEEDS as s (s)}
          <button type="button" role="radio" aria-checked={fps === s} class:on={fps === s} onclick={() => (fps = s)}>{s}/s</button>
        {/each}
      </div>
      <span class="lbl whole">{wholeRunWords(n, fps)}</span>
    </div>
  {/if}
</div>

<style>
  .player {
    position: fixed; inset: 0; z-index: 70;
    display: flex; flex-direction: column; gap: var(--s-3);
    padding: var(--s-4) var(--s-5) var(--s-5);
    background: var(--player-bg); color: var(--player-ink);
  }
  .top { display: flex; align-items: center; justify-content: space-between; gap: var(--s-4); }
  .name { display: flex; flex-direction: column; min-width: 0; }
  .name b { font-size: 16px; font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .name span { font-size: 12px; color: var(--player-muted); }
  .x {
    width: 40px; height: 40px; display: grid; place-items: center;
    border: 0; border-radius: 9px; background: transparent; color: var(--player-muted); cursor: pointer;
  }
  .x:hover { background: var(--player-panel); color: var(--player-ink); }

  .stage {
    flex: 1 1 auto; min-height: 0; position: relative;
    display: grid; place-items: center; border-radius: 9px; overflow: hidden;
  }
  /* the whole picture, never cropped: absolute, so its height is the
     stage's (a percentage max-height inside the grid never applied and
     the overflow cut the top and bottom off) */
  .stage img { position: absolute; inset: 0; width: 100%; height: 100%; object-fit: contain; display: block; }
  .caption {
    position: absolute; left: var(--s-3); bottom: var(--s-3);
    display: flex; flex-direction: column; gap: 2px;
    padding: var(--s-2) var(--s-3); border-radius: 6px;
    background: rgba(20, 28, 30, 0.8); font-size: 13px;
  }
  .caption b { font-size: 18px; font-weight: 600; }
  .when { margin: 0; text-align: center; font-size: 15px; }
  .none { color: var(--player-muted); }

  .scrub { display: flex; flex-direction: column; gap: var(--s-1); }
  .scrub input { width: 100%; accent-color: var(--teal-400); }
  .ends { display: flex; justify-content: space-between; font-size: 12px; color: var(--player-muted); }

  .controls { display: flex; align-items: center; gap: var(--s-2); flex-wrap: wrap; }
  .step {
    width: 44px; height: 44px; display: grid; place-items: center; cursor: pointer;
    border: 1px solid var(--player-line); border-radius: 9px; background: transparent; color: var(--player-ink);
  }
  .step:disabled { opacity: 0.4; cursor: default; }
  .play {
    min-width: 112px; height: 44px; display: inline-flex; align-items: center; justify-content: center; gap: var(--s-2);
    border: 0; border-radius: 9px; background: var(--green-500); color: var(--player-bg);
    font: inherit; font-size: 15px; font-weight: 600; cursor: pointer;
  }
  .grow { flex: 1; }
  .lbl { font-size: 12px; color: var(--player-muted); }
  .whole { min-width: 150px; }
  .speeds { display: flex; gap: 2px; padding: 3px; border-radius: 9px; background: var(--player-panel); }
  .speeds button {
    border: 0; border-radius: 7px; padding: 7px 12px; background: transparent;
    color: var(--player-ink); font: inherit; font-size: 13px; cursor: pointer;
  }
  .speeds button.on { background: var(--player-line); font-weight: 600; }
  .player :focus-visible { outline: 2px solid var(--teal-400); outline-offset: 2px; }
</style>
