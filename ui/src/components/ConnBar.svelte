<script>
  import { app } from '../lib/state.svelte.js';
  import { linkState } from '../lib/link.js';

  // A slim line at the top of every page, only when there is something to
  // act on (the cable down, the pump not following, the journal stalled):
  // what is wrong, in a sentence, and the way to Settings > Pumps, where the
  // port and the addresses are set. The port picker that used to sit here
  // moved there (2026-10-21); this line only tells.
  let { onopen } = $props();

  const link = $derived(linkState(app.status, app.connected));
  const show = $derived(link.tone !== 'ok');
</script>

{#if show}
  <div class="connbar {link.tone}" id="connbar" role={link.tone === 'bad' ? 'alert' : 'status'}>
    <span class="dot" aria-hidden="true"></span>
    <span class="txt"><b>{link.label}</b>{#if link.hint}<span class="hint">{link.hint}</span>{/if}</span>
    {#if link.connectable && onopen}
      <button class="go" type="button" onclick={onopen}>Pump settings</button>
    {/if}
  </div>
{/if}

<style>
  .connbar {
    position: sticky; top: 0; z-index: 30;
    display: flex; align-items: center; gap: var(--s-3);
    margin-bottom: var(--s-4); padding: 6px var(--s-4);
    border-radius: var(--radius-ctl); background: var(--surface);
    border-left: 3px solid var(--muted); font-size: 13px;
  }
  .dot { width: 8px; height: 8px; border-radius: 50%; background: var(--muted); flex: none; }
  .txt { flex: 1; min-width: 0; display: flex; flex-wrap: wrap; align-items: baseline; column-gap: var(--s-3); }
  .txt b { font-weight: 600; color: var(--ink); }
  .hint { font-size: 12.5px; color: var(--muted); }
  .bad { border-left-color: var(--danger); }
  .bad .dot { background: var(--danger); }
  .bad b { color: var(--danger-ink); }
  .warn { border-left-color: var(--measured); }
  .warn .dot { background: var(--measured); }
  .go {
    flex: none; font: 500 12.5px var(--font); color: var(--teal-700); background: none; border: none;
    padding: 4px 0; cursor: pointer; text-decoration: underline; text-underline-offset: 3px;
  }
  .go:focus-visible { outline: 2px solid var(--teal-600); outline-offset: 2px; border-radius: 4px; }
</style>
