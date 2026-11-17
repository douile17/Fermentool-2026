<script>
  import { untrack } from 'svelte';
  import { BASE } from '../lib/api.js';
  import { liveSrc, liveState } from '../lib/camera.js';

  // The live view's <img>, for the Camera page and Settings > Camera. Its
  // parent mounts it only while `liveShown()`; it ends its stream when it
  // goes. `cam` and `pollOk` are the parent's GET /api/camera polls; `live`
  // (bindable) says whether the picture is really live (`liveState`): a
  // stream that ended keeps its last picture and fires no error. Stale while
  // the daemon answers, or broken, the stream is asked again.
  let { cam, pollOk, alt, live = $bindable('starting') } = $props();

  let attempt = $state(0);
  let shownSince = $state(Date.now());
  let now = $state(Date.now());
  $effect(() => {
    const t = setInterval(() => (now = Date.now()), 1000);
    return () => clearInterval(t);
  });

  const seen = $derived(liveState(cam, now, pollOk, shownSince));
  $effect(() => {
    live = seen;
  });

  function remount() {
    attempt++;
    shownSince = Date.now();
  }
  // Stale while the daemon answers: the stream ended (a restart) or froze.
  $effect(() => {
    if (seen === 'stale' && pollOk) untrack(remount);
  });

  // Broken (the daemon stopped mid-load): asked again 3 s later.
  let retry = null;
  function lostStream() {
    clearTimeout(retry);
    retry = setTimeout(remount, 3000);
  }
  $effect(() => () => clearTimeout(retry));
</script>

{#key attempt}
  <img use:liveSrc={BASE + '/api/camera/live'} {alt} onerror={lostStream} />
{/key}

<style>
  img { display: block; width: 100%; height: 100%; object-fit: contain; }
</style>
