<script>
  // The tube on the pump, as a dropdown that searches. Closed it is one line
  // (the tube, its size, when it was calibrated, its figures); open it is a
  // list of fixed height that scrolls, so 4 or 100 calibrations take the same
  // room. Tubes are filed by bore, each bore in its own pastel box, sizes in
  // mm and inches (Settings, Display, says which leads). One row per tube
  // with its newest calibration; older ones on request.
  import { app } from '../lib/state.svelte.js';
  import { num, stamp } from '../lib/fmt.js';
  import { sizeParts, diaParts, boreKey, boreClass, sizeMatches, tubeLabel } from '../lib/tubesize.js';

  /**
   * @type {{
   *   cals: any[],               // newest first, the ones this run can use
   *   value: number | null,
   *   onchange: (id: number | null) => void,
   *   lastUsed?: number | null,  // calibration of the last run on this pump
   *   allowNone?: boolean,       // the run can go without a calibration
   *   noneText?: string,
   *   oncalibrate?: () => void,
   *   id?: string,
   * }}
   */
  let { cals, value, onchange, lastUsed = null, allowNone = false, noneText = '', oncalibrate, id } = $props();

  // Same limit as the calibration page: a spread past it is worth a look.
  const CV_WARN_PCT = 5;
  const OLD_DAYS = 30;
  const DAY = 86_400_000;

  const ageDays = (iso) => Math.floor((Date.now() - Date.parse(iso)) / DAY);
  function ago(iso) {
    const d = ageDays(iso);
    if (d <= 0) return `today ${new Date(iso).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}`;
    if (d === 1) return 'yesterday';
    return d < 60 ? `${d} days ago` : stamp(iso);
  }
  const tubeName = tubeLabel;
  const size = (c) => sizeParts(c.inner_diameter_mm, c.outer_diameter_mm, app.tubeSizes);
  const tint = (c) => boreClass(c.inner_diameter_mm, c.outer_diameter_mm);
  const isOld = (c) => ageDays(c.created_at) > OLD_DAYS;
  const cvHigh = (c) => c.cv_pct > CV_WARN_PCT;

  // One group per tube (and kind: an rpm and an ml/min calibration of the
  // same tube do different things), its calibrations newest first.
  const groups = $derived.by(() => {
    const by = new Map();
    for (const c of cals) {
      const key = `${c.tubing_lot_id}|${c.brand ?? ''}|${c.product_ref ?? ''}|${c.internal_ref ?? ''}|${boreKey(c.inner_diameter_mm, c.outer_diameter_mm)}|${c.control_var}`;
      if (!by.has(key)) by.set(key, []);
      by.get(key).push(c);
    }
    return [...by.values()];
  });
  // The bores, smallest first, each with its tubes (newest first).
  const bores = $derived.by(() => {
    const by = new Map();
    for (const g of groups) {
      const c = g[0];
      const key = boreKey(c.inner_diameter_mm, c.outer_diameter_mm);
      if (!by.has(key)) by.set(key, { key, inner: c.inner_diameter_mm, outer: c.outer_diameter_mm, tubes: [] });
      by.get(key).tubes.push(g);
    }
    return [...by.values()].sort((a, b) => a.inner - b.inner || a.outer - b.outer);
  });
  const chosen = $derived(cals.find((c) => c.id === value) ?? null);

  let open = $state(false);
  let q = $state('');
  let showOlder = $state(false);
  let boreOnly = $state(null); // a bore key, from the chips
  let active = $state(0);
  let root;
  // inside {#if open}: bound once the list is drawn, so they must be state
  let search = $state();
  let list = $state();
  let pop = $state();

  const matches = (c) => {
    const s = q.trim();
    return !s || tubeName(c).toLowerCase().includes(s.toLowerCase()) || sizeMatches(s, c.inner_diameter_mm, c.outer_diameter_mm);
  };
  const itemsOf = (g) => {
    const out = [{ c: g[0] }];
    if (showOlder || g.slice(1).some((c) => c.id === value)) for (const c of g.slice(1)) out.push({ c, older: true });
    return out;
  };
  // What the list shows: the last used tube on its own first (unless a search
  // or a bore filter narrows the list), then one box per bore.
  const sections = $derived.by(() => {
    const out = [];
    const narrowed = q.trim() || boreOnly;
    const last = groups.find((g) => g.some((c) => c.id === lastUsed));
    if (last && !narrowed) out.push({ last: true, items: [{ c: last.find((c) => c.id === lastUsed) }] });
    for (const b of bores) {
      if (boreOnly && b.key !== boreOnly) continue;
      const tubes = b.tubes.filter((g) => matches(g[0]));
      if (tubes.length) out.push({ bore: b, items: tubes.flatMap(itemsOf), count: tubes.length });
    }
    return out;
  });
  const picks = $derived(sections.flatMap((s) => s.items));
  const olderCount = $derived(groups.reduce((n, g) => n + g.length - 1, 0));

  function toggle() {
    open = !open;
    q = '';
    boreOnly = null;
    active = Math.max(0, picks.findIndex((r) => r.c.id === value));
    // The whole list in view: in a short window the page scrolls to it.
    if (open)
      requestAnimationFrame(() => {
        search?.focus({ preventScroll: true });
        pop?.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
      });
  }
  function pick(id) {
    onchange(id);
    open = false;
    root?.querySelector('.trigger')?.focus();
  }
  function onKey(e) {
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      active = Math.min(picks.length - 1, active + 1);
      scrollActive();
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      active = Math.max(0, active - 1);
      scrollActive();
    } else if (e.key === 'Enter' && picks[active]) {
      e.preventDefault();
      pick(picks[active].c.id);
    } else if (e.key === 'Escape') {
      e.preventDefault();
      open = false;
      root?.querySelector('.trigger')?.focus();
    }
  }
  function scrollActive() {
    requestAnimationFrame(() => list?.querySelector('.opt.active')?.scrollIntoView({ block: 'nearest' }));
  }
  // A click outside closes it. Only a click on something still on the page:
  // a node replaced by the click itself is detached and would look outside.
  function onDocClick(e) {
    if (open && e.target.isConnected && !root.contains(e.target)) open = false;
  }

  // The searched text, marked in the tube's name.
  function parts(text) {
    const s = q.trim();
    const i = s ? text.toLowerCase().indexOf(s.toLowerCase()) : -1;
    return i < 0 ? [text, '', ''] : [text.slice(0, i), text.slice(i, i + s.length), text.slice(i + s.length)];
  }
</script>

<svelte:document onclick={onDocClick} />

{#snippet figures(c)}
  <span>calibrated <b>{ago(c.created_at)}</b></span>
  {#if c.control_var === 'rpm'}
    <span><b>{num(c.mean_measured_ml_min / c.setpoint, 3)}</b> ml/min per rpm</span>
  {:else}
    <span>regulation starts at <b>×{num(c.c0, 2)}</b></span>
  {/if}
  <span class="badge" class:warn={cvHigh(c)}>CV {num(c.cv_pct, 1)} %{cvHigh(c) ? ', high' : ''}</span>
  {#if isOld(c)}<span class="badge warn">over {OLD_DAYS} days</span>{/if}
{/snippet}

{#snippet sizeOf(c)}
  {@const s = size(c)}
  <span class="pri">{s.pri}</span><span class="sec">{s.sec}</span>
{/snippet}

{#snippet option(r, withSize)}
  {@const i = picks.indexOf(r)}
  {@const [a, m, b] = parts(tubeName(r.c))}
  <!-- svelte-ignore a11y_click_events_have_key_events (the search field drives the keys) -->
  <div class="opt {withSize ? tint(r.c) : ''}" class:older={r.older} class:active={i === active} class:sel={r.c.id === value}
    role="option" aria-selected={r.c.id === value} id="tube-opt-{r.c.id}" tabindex="-1"
    onclick={() => pick(r.c.id)} onmousemove={() => (active = i)}>
    <span class="o1">
      {#if r.older}older calibration
      {:else}{#if withSize}<span class="sw"></span>{/if}{a}{#if m}<mark>{m}</mark>{/if}{b}{#if withSize}<span class="gap"></span>{@render sizeOf(r.c)}{/if}{/if}
    </span>
    <span class="o2">
      calibrated {ago(r.c.created_at)},
      <span class="badge" class:warn={cvHigh(r.c)}>CV {num(r.c.cv_pct, 1)} %</span>
      {#if isOld(r.c)}<span class="badge warn">over {OLD_DAYS} days</span>{/if}
    </span>
    <span class="o3 mono">
      {#if r.c.control_var === 'rpm'}{num(r.c.mean_measured_ml_min / r.c.setpoint, 3)}<small>ml/min per rpm</small>
      {:else}×{num(r.c.c0, 2)}<small>regulation start</small>{/if}
    </span>
  </div>
{/snippet}

<div class="dd" bind:this={root} {id}>
  <button type="button" class="trigger {chosen ? tint(chosen) : ''}" class:need={!chosen && !allowNone}
    aria-haspopup="listbox" aria-expanded={open} onclick={toggle}>
    {#if chosen}
      <span class="t1"><span class="sw"></span>{tubeName(chosen)}, {@render sizeOf(chosen)}</span>
      <span class="t2">{@render figures(chosen)}</span>
    {:else if allowNone}
      <span class="t1">No calibration</span>
      <span class="t2">{noneText}</span>
    {:else}
      <span class="t1">Choose the tube on the pump</span>
      <span class="t2">{cals.length ? `${groups.length} tube${groups.length === 1 ? '' : 's'} calibrated` : 'No tube calibrated yet'}</span>
    {/if}
    <span class="chg">{open ? 'Close' : chosen ? 'Change tube' : 'Choose'}<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M6 9l6 6 6-6" /></svg></span>
  </button>

  {#if open}
    <div class="pop" bind:this={pop}>
      <label class="search">
        <svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="11" cy="11" r="7" /><path d="M20 20l-3.5-3.5" /></svg>
        <input bind:this={search} bind:value={q} oninput={() => (active = 0)} onkeydown={onKey}
          placeholder="Type a lot, a reference, 0.063, 1/16 or 1.6" aria-label="Find a tube" autocomplete="off"
          role="combobox" aria-expanded="true" aria-controls="tube-list" aria-activedescendant={picks[active] ? `tube-opt-${picks[active].c.id}` : undefined} />
        <span class="count">{groups.length} tube{groups.length === 1 ? '' : 's'}</span>
      </label>
      {#if bores.length > 1}
        <div class="chips" role="group" aria-label="Bore">
          <button type="button" class="chip" class:on={!boreOnly} onclick={() => ((boreOnly = null), (active = 0))}>All bores</button>
          {#each bores as b (b.key)}
            {@const d = diaParts(b.inner, app.tubeSizes)}
            <button type="button" class="chip {boreClass(b.inner, b.outer)}" class:on={boreOnly === b.key}
              onclick={() => ((boreOnly = boreOnly === b.key ? null : b.key), (active = 0))}>
              <span class="dot"></span><b>{d.pri}</b> <small>{d.sec}</small>
            </button>
          {/each}
        </div>
      {/if}
      <div class="list" role="listbox" id="tube-list" aria-label="Tubes" bind:this={list}>
        {#each sections as s (s.last ? 'last' : s.bore.key)}
          {#if s.last}
            <div class="grp">Last used on this pump</div>
            <div class="box last {tint(s.items[0].c)}">{@render option(s.items[0], true)}</div>
          {:else}
            {@const sp = sizeParts(s.bore.inner, s.bore.outer, app.tubeSizes)}
            {@const ro = 12}
            {@const ri = Math.max(2, (ro * s.bore.inner) / s.bore.outer)}
            <div class="box {boreClass(s.bore.inner, s.bore.outer)}">
              <div class="band">
                <svg class="ring" viewBox="0 0 26 26" aria-hidden="true"><circle cx="13" cy="13" r={ro} /><circle cx="13" cy="13" r={ri} /></svg>
                <span>Ø <span class="pri">{sp.pri}</span><span class="sec">{sp.sec}</span></span>
                <span class="cnt">{s.count} tube{s.count === 1 ? '' : 's'}</span>
              </div>
              {#each s.items as r (r.c.id)}{@render option(r, false)}{/each}
            </div>
          {/if}
        {:else}
          <div class="empty">
            {q.trim() ? `No tube matches “${q.trim()}”.` : 'No tube calibrated for this run yet.'}
            {#if oncalibrate}<button type="button" class="linkish" onclick={oncalibrate}>Calibrate a tube</button>{/if}
          </div>
        {/each}
      </div>
      <div class="foot">
        {#if olderCount}
          <label><input type="checkbox" bind:checked={showOlder} /> Show older calibrations ({olderCount})</label>
        {:else}<span></span>{/if}
        <span class="foot-acts">
          {#if allowNone}<button type="button" class="linkish" onclick={() => pick(null)}>Run without calibration</button>{/if}
          {#if oncalibrate}<button type="button" class="linkish" onclick={oncalibrate}>Calibrate a new tube</button>{/if}
        </span>
      </div>
    </div>
  {/if}
</div>

<style>
  .dd { position: relative; max-width: 700px; }
  .pri { font-family: var(--mono); font-weight: 600; }
  .sec { font-family: var(--mono); font-weight: 400; font-size: 0.86em; color: var(--muted); margin-left: 6px; }
  .gap { display: inline-block; width: 8px; }
  .sw { display: inline-block; width: 10px; height: 10px; border-radius: 50%; background: var(--c-line); margin-right: 8px; vertical-align: 0; }

  .trigger {
    width: 100%; display: grid; grid-template-columns: minmax(0, 1fr) auto; gap: 2px var(--s-3); align-items: center;
    text-align: left; padding: 10px 14px; border: 1px solid var(--line); border-radius: var(--radius-ctl);
    background: var(--surface); color: var(--ink); cursor: pointer;
  }
  .trigger:hover { border-color: var(--teal-400); background: color-mix(in srgb, var(--teal-700) 4%, var(--surface)); }
  .trigger[aria-expanded='true'] { border-color: var(--teal-700); box-shadow: 0 0 0 3px color-mix(in srgb, var(--teal-700) 15%, transparent); }
  .trigger.need { border: 2px solid var(--measured); }
  .t1 { font-weight: 600; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .t2 { grid-column: 1; display: flex; flex-wrap: wrap; align-items: center; gap: 2px var(--s-3); font-size: 12.5px; color: var(--muted); font-weight: 400; }
  .t2 b { color: var(--ink); font-family: var(--mono); font-weight: 500; font-size: 12px; }
  /* says it can be changed: a real button look, filled on hover */
  .chg {
    grid-row: 1 / span 2; grid-column: 2; display: inline-flex; align-items: center; gap: 6px;
    padding: 7px 12px; border-radius: 8px; border: 1px solid var(--teal-700); color: var(--teal-700);
    font-size: 13px; font-weight: 600; white-space: nowrap; transition: background 0.15s, color 0.15s;
  }
  .chg svg { width: 16px; height: 16px; stroke: currentColor; fill: none; stroke-width: 2.2; transition: transform 0.15s; }
  .trigger:hover .chg, .trigger[aria-expanded='true'] .chg { background: var(--teal-700); color: var(--on-primary); }
  .trigger[aria-expanded='true'] .chg svg { transform: rotate(180deg); }

  .badge {
    display: inline-flex; align-items: center; padding: 0 7px; border-radius: 999px; white-space: nowrap;
    font-size: 11px; font-weight: 600;
    background: color-mix(in srgb, var(--lime-300) 50%, var(--surface)); color: var(--green-600);
  }
  .badge.warn { background: color-mix(in srgb, var(--measured) 16%, var(--surface)); color: color-mix(in srgb, var(--measured) 75%, var(--ink)); }

  .pop {
    position: absolute; left: 0; right: 0; top: calc(100% + 6px); z-index: 20;
    display: flex; flex-direction: column; overflow: hidden;
    background: var(--surface); border: 1px solid var(--line); border-radius: var(--radius-ctl);
    box-shadow: var(--shadow-pop);
  }
  .search { display: flex; align-items: center; gap: var(--s-2); padding: 0 var(--s-3); border-bottom: 1px solid var(--line); }
  .search svg { width: 16px; height: 16px; flex: none; stroke: var(--muted); fill: none; stroke-width: 2; }
  .search input { flex: 1; min-width: 0; border: 0; outline: 0; background: transparent; padding: 11px 0; box-shadow: none; }
  .count { font-size: 12px; color: var(--muted); white-space: nowrap; }

  .chips { display: flex; flex-wrap: wrap; gap: 6px; padding: 10px var(--s-3); border-bottom: 1px solid var(--line); background: var(--surface-sunken); }
  .chip { display: inline-flex; align-items: baseline; gap: 4px; padding: 3px 10px; border: 1px solid var(--line); border-radius: 999px; background: var(--surface); color: var(--ink); font-size: 12.5px; cursor: pointer; white-space: nowrap; }
  .chip b { font-family: var(--mono); font-weight: 600; }
  .chip small { font-family: var(--mono); color: var(--muted); }
  .chip .dot { align-self: center; width: 9px; height: 9px; border-radius: 50%; background: var(--c-line); margin-right: 2px; }
  .chip.on { background: var(--c-bg, color-mix(in srgb, var(--teal-700) 10%, var(--surface))); border-color: var(--c-line, var(--teal-700)); color: var(--c-ink, var(--teal-ink)); }

  .list { max-height: min(340px, calc(100vh - 300px)); overflow-y: auto; padding: 6px; }
  .grp { padding: 4px 6px; font-size: 11px; font-weight: 600; letter-spacing: 0.06em; text-transform: uppercase; color: var(--muted); }
  /* each bore a box of its own: a tinted band (a ring drawn to the tube's
     proportions, the size in both units, how many tubes), its tubes below */
  .box { margin: 4px 0 10px; border: 1px solid var(--c-line); border-radius: var(--radius-ctl); overflow: hidden; }
  .box.last { border-style: dashed; }
  .band {
    position: sticky; top: -6px; z-index: 1; display: flex; align-items: center; gap: var(--s-3); flex-wrap: wrap;
    padding: 8px 12px; background: var(--c-bg); border-bottom: 1px solid var(--c-line); color: var(--c-ink); font-family: var(--mono);
  }
  .band .pri { font-size: 14px; color: var(--c-ink); }
  .ring { flex: none; width: 24px; height: 24px; }
  .ring circle { fill: none; stroke: var(--c-ink); stroke-width: 1.5; }
  .cnt { margin-left: auto; font-family: var(--font); font-size: 12px; font-weight: 600; }

  .opt {
    display: grid; grid-template-columns: minmax(0, 1fr) auto; gap: 1px var(--s-3); padding: 8px 12px 8px 16px; cursor: pointer;
    background: color-mix(in srgb, var(--c-bg) 45%, var(--surface)); box-shadow: inset 4px 0 0 var(--c-line);
  }
  .opt + .opt { border-top: 1px solid var(--line-soft); }
  .opt.active { background: var(--c-bg); }
  .opt.sel .o1::before { content: '✓ '; color: var(--teal-700); }
  .opt.older { padding-left: 32px; }
  .opt.older .o1 { font-weight: 500; color: var(--muted); }
  .o1 { font-weight: 600; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .o2 { display: flex; flex-wrap: wrap; align-items: center; gap: 4px; font-size: 12px; color: var(--muted); }
  .o3 { grid-row: 1 / span 2; grid-column: 2; text-align: right; font-size: 12.5px; }
  .o3 small { display: block; font-family: var(--font); font-size: 11px; color: var(--muted); }
  mark { background: color-mix(in srgb, var(--measured) 28%, transparent); color: inherit; border-radius: 2px; }
  .empty { padding: var(--s-4) var(--s-3); font-size: 13px; color: var(--muted); }
  .foot {
    display: flex; justify-content: space-between; align-items: center; gap: var(--s-3); flex-wrap: wrap;
    padding: 10px var(--s-3); border-top: 1px solid var(--line); background: var(--surface-sunken); font-size: 13px;
  }
  .foot label { display: flex; align-items: center; gap: 6px; color: var(--muted); cursor: pointer; }
  .foot-acts { display: flex; gap: var(--s-4); flex-wrap: wrap; }
  .linkish { background: none; border: none; padding: 0; font: inherit; color: var(--teal-700); text-decoration: underline; cursor: pointer; }
  @media (prefers-reduced-motion: reduce) { .chg, .chg svg { transition: none; } }
</style>
