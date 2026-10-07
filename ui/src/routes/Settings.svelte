<script>
  import { app } from '../lib/state.svelte.js';
  import { SECTIONS, GROUPS } from './settings/sections.js';
  import './settings/settings.css';
  import Icon from '../components/Icon.svelte';
  import PumpHead from '../components/PumpHead.svelte';

  // One section at a time, picked from the side menu (sections.js).
  const current = $derived(SECTIONS.find((s) => s.id === app.settingsSection) ?? SECTIONS[0]);

  // On a phone the menu and the section take the whole width in turn, as in
  // a phone's own settings: pick a line, then "back" to the list.
  let open = $state(false);
  function pick(id) {
    app.settingsSection = id;
    open = true;
  }
</script>

{#snippet glyph(icon)}
  <span class="ic" aria-hidden="true">
    {#if icon === 'pump'}
      <PumpHead size={18} spin={false} solid />
    {:else}
      <Icon name={icon} size={17} />
    {/if}
  </span>
{/snippet}

<div class="settings" class:open>
  <nav class="side" aria-label="Settings sections">
    {#each GROUPS as g}
      <div class="group">
        <div class="eyebrow group-title">{g}</div>
        {#each SECTIONS.filter((s) => s.group === g) as s (s.id)}
          <button
            class="item"
            class:on={s.id === current.id}
            aria-current={s.id === current.id ? 'page' : undefined}
            onclick={() => pick(s.id)}
          >
            {@render glyph(s.icon)}
            <span class="label">{s.label}</span>
            <span class="chev"><Icon name="chevron-right" size={16} /></span>
          </button>
        {/each}
      </div>
    {/each}
  </nav>

  <div class="pane settings-pane">
    <button class="back" onclick={() => (open = false)}>
      <Icon name="arrow-left" size={18} />Settings
    </button>
    {#key current.id}
      <current.component group={current.group} />
    {/key}
  </div>
</div>

<style>
  .settings {
    display: grid;
    grid-template-columns: 180px minmax(0, 1fr);
    gap: var(--s-6);
    align-items: start;
  }
  .side {
    position: sticky;
    top: var(--s-6);
    display: flex;
    flex-direction: column;
    gap: var(--s-5);
  }
  .group { display: flex; flex-direction: column; gap: 1px; }
  .group-title { padding: 0 var(--s-3); margin-bottom: var(--s-1); }
  .item {
    display: flex;
    align-items: center;
    gap: var(--s-3);
    text-align: left;
    width: 100%;
    border: none;
    background: transparent;
    color: var(--muted);
    padding: 7px var(--s-3);
    border-radius: 8px;
    font-size: 13px;
    line-height: 18px;
  }
  .ic {
    width: 18px; height: 18px; flex: none;
    display: inline-flex; align-items: center; justify-content: center;
    opacity: 0.85;
    /* the pump glyph in the label colour, ring cut out */
    --ph-body: currentColor;
    --ph-detail: currentColor;
  }
  .label { flex: 1; }
  .chev, .back { display: none; }
  .item:hover { background: var(--surface-sunken); color: var(--ink); }
  .item.on {
    background: color-mix(in srgb, var(--teal-700) 11%, var(--surface));
    color: var(--teal-700);
    font-weight: 600;
  }

  /* Narrow window: the menu is a full-width list (one line per section,
     a chevron at the end); a line opens its section in its place, with a
     back arrow to the list. */
  @media (max-width: 760px) {
    .settings { grid-template-columns: minmax(0, 1fr); }
    .side { position: static; }
    .settings.open .side, .settings:not(.open) .pane { display: none; }
    .group { gap: 0; }
    .item {
      padding: var(--s-3);
      font-size: 15px;
      color: var(--ink);
      background: var(--surface);
      border-radius: 0;
      border-bottom: 1px solid var(--line-soft);
    }
    .group .item:first-of-type { border-radius: 10px 10px 0 0; }
    .group .item:last-of-type { border-radius: 0 0 10px 10px; border-bottom: none; }
    .group .item:first-of-type:last-of-type { border-radius: 10px; }
    .item.on { background: var(--surface); color: var(--ink); font-weight: 400; }
    .chev { display: inline-flex; color: var(--muted); }
    .back {
      display: inline-flex; align-items: center; gap: var(--s-2);
      margin-bottom: var(--s-4);
      border: none; background: transparent; padding: var(--s-1) 0;
      color: var(--teal-700); font-size: 15px; font-weight: 600;
    }
  }
</style>
