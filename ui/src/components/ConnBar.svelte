<script>
  import { app } from '../lib/state.svelte.js';
  import { linkState } from '../lib/link.js';
  import ConnectControl from './ConnectControl.svelte';

  // The big bar only shows when there's something to act on. Once the pump is
  // connected and healthy it disappears - the persistent cue is the small pump
  // line in the sidebar foot. For a non-connection condition (pump not tracking,
  // journal stalled) it shows just the status line, not the port picker.
  const link = $derived(linkState(app.status, app.connected));
  const show = $derived(link.tone !== 'ok');
</script>

{#if show}
  <div class="connbar">
    <ConnectControl compact controls={link.connectable} />
  </div>
{/if}

<style>
  .connbar {
    position: sticky;
    top: 0;
    z-index: 30;
    margin-bottom: var(--s-5);
    background: var(--bg, transparent);
    padding-top: var(--s-2);
  }
</style>
