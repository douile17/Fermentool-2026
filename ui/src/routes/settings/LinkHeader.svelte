<script>
  // A cable's state, said first and big (Settings > Pumps and Balance,
  // mockup B of 2026-10-22: a small dot under the fields did not show it).
  // `line` is `{ tone, word, text }` from lib/pumps.js `lineState` or
  // lib/balance.js `balanceLine`; `action` is the Connect / Disconnect button.
  import Icon from '../../components/Icon.svelte';

  let { line, action } = $props();
  const icon = $derived(line.tone === '' || line.tone === 'warn' ? 'plug' : 'unplug');
</script>

<!-- tone classes prefixed: settings.css styles a bare `.ok` as a green band -->
<div class="head t-{line.tone || 'ok'}" role="status">
  <span class="ico" aria-hidden="true"><Icon name={icon} size={22} /></span>
  <div class="txt">
    <span class="word">{line.word}</span>
    {#if line.text}<span class="text">{line.text}</span>{/if}
  </div>
  {@render action?.()}
</div>

<style>
  /* a band tinted by the state (the user preferred it to the plain header
     of mockup B, 2026-10-22): green connected, orange a pump silent, red
     the port silent, grey not connected */
  .head {
    display: flex; align-items: center; gap: var(--s-3);
    padding: var(--s-3) var(--s-4); margin-bottom: var(--s-4); border-radius: var(--radius-ctl);
    background: var(--surface-sunken);
  }
  .ico {
    width: 44px; height: 44px; border-radius: 50%; flex: none; display: grid; place-items: center;
    background: color-mix(in srgb, var(--muted) 10%, transparent); color: var(--muted);
  }
  .txt { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
  .word { font-size: 20px; font-weight: 600; line-height: 1.2; color: var(--muted); }
  .text { font-size: 13px; color: var(--muted); }
  .t-ok { background: color-mix(in srgb, var(--green-500) 12%, var(--surface)); }
  .t-ok .ico { background: color-mix(in srgb, var(--green-500) 14%, transparent); color: var(--green-600); }
  .t-ok .word { color: var(--green-ink); }
  .t-warn { background: var(--warn-bg); }
  .t-warn .ico { background: color-mix(in srgb, var(--measured) 14%, transparent); color: var(--warn-ink); }
  .t-warn .word, .t-warn .text { color: var(--warn-ink); }
  .t-bad { background: var(--danger-bg); }
  .t-bad .ico { background: color-mix(in srgb, var(--danger) 14%, transparent); color: var(--danger-ink); }
  .t-bad .word, .t-bad .text { color: var(--danger-ink); }
</style>
