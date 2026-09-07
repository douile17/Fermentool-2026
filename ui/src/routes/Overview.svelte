<script>
  import { app } from '../lib/state.svelte.js';
  import { get, post } from '../lib/api.js';
  import { num, dur, shortTime, stamp, unitFor, digitsFor } from '../lib/fmt.js';
  import Chart from '../components/Chart.svelte';
  import FigureBand from '../components/FigureBand.svelte';

  const active = $derived(app.status?.active ?? null);
  const holding = $derived(app.status?.holding ?? null);
  const complete = $derived(!active && holding != null);

  let run = $state(null);
  let planned = $state([]);
  let events = $state([]);
  let err = $state(null);
  let loadedId = $state(null);
  let lastEventsAt = 0;
  let now = $state(Date.now());
  // Separate, high-frequency clock that only drives the chart's "now" marker
  // so it glides instead of stepping once a second. Text stays on `now`.
  let nowAnim = $state(Date.now());

  const reducedMotion =
    typeof window !== 'undefined' &&
    window.matchMedia &&
    window.matchMedia('(prefers-reduced-motion: reduce)').matches;

  // Parse the run's t0 once per status change, not once per animation frame.
  const startedAtMs = $derived(active ? Date.parse(active.started_at) : NaN);

  $effect(() => {
    const id = setInterval(() => {
      now = Date.now();
      nowAnim = Date.now();
    }, 1000);
    return () => clearInterval(id);
  });

  // Glide the chart marker between ticks. rAF (so it pauses in a background
  // tab), but commit at ~15 fps — plenty smooth for the marker, and a fraction
  // of the work over a multi-day run.
  $effect(() => {
    if (!active || reducedMotion) return;
    let raf = 0;
    let lastCommit = 0;
    const step = (t) => {
      raf = requestAnimationFrame(step);
      if (t - lastCommit >= 66) {
        lastCommit = t;
        nowAnim = Date.now();
      }
    };
    raf = requestAnimationFrame(step);
    return () => cancelAnimationFrame(raf);
  });

  $effect(() => {
    const id = active?.run_id ?? holding?.run_id ?? null;
    if (id === loadedId) return;
    loadedId = id;
    run = null;
    planned = [];
    events = [];
    err = null;
    lastEventsAt = 0;
    if (id != null) load(id);
  });

  // Refresh the "recent activity" list as the run advances — but at most every
  // few seconds, not once per tick (a 100 h run would otherwise fire ~360k
  // fetches). The live chart trace comes from the planned curve, so no per-tick
  // data is pulled here at all.
  $effect(() => {
    void active?.last_seq;
    if (loadedId == null || !run) return;
    const t = Date.now();
    if (t - lastEventsAt < 4000) return;
    lastEventsAt = t;
    get(`/api/runs/${loadedId}/events?limit=6`).then((e) => (events = e)).catch(() => {});
  });

  async function load(id) {
    try {
      run = await get(`/api/runs/${id}`);
      const p = await post('/api/preview', { curve: run.curve, samples: 200 });
      planned = p.series;
      events = await get(`/api/runs/${id}/events?limit=6`);
    } catch (e) {
      err = e.message;
    }
  }

  const unit = $derived(run ? unitFor(run.control_var) : 'rpm');
  const digits = $derived(run ? digitsFor(run.control_var) : 1);
  const elapsed = $derived(
    complete && run
      ? run.duration_s
      : active
        ? Math.max(0, (now - startedAtMs) / 1000)
        : 0
  );
  // Same as `elapsed` but off the rAF clock — feeds the chart marker only.
  const elapsedAnim = $derived(
    active ? Math.max(0, (nowAnim - startedAtMs) / 1000) : elapsed
  );
  // Off the smooth clock so the progress bar fills continuously, not per tick.
  const pct = $derived(run && run.duration_s ? Math.min(100, (elapsedAnim / run.duration_s) * 100) : 0);

  // Current setpoint read off the planned curve at the smooth clock, so the
  // displayed value climbs continuously instead of jumping once per tick.
  function valueAt(series, t) {
    if (!series.length) return null;
    if (t <= series[0][0]) return series[0][1];
    const last = series[series.length - 1];
    if (t >= last[0]) return last[1];
    for (let i = 1; i < series.length; i++) {
      if (series[i][0] >= t) {
        const [t0, v0] = series[i - 1];
        const [t1, v1] = series[i];
        const f = t1 === t0 ? 0 : (t - t0) / (t1 - t0);
        return v0 + (v1 - v0) * f;
      }
    }
    return last[1];
  }
  // Smoothly interpolate the setpoint off the planned curve between ticks, but
  // keep it leashed to the daemon's last *real* commanded value (± ~1.5 ticks
  // of local slope) so a failsafe hold or comms loss can't make the readout
  // keep climbing a value the pump isn't at.
  const liveValue = $derived.by(() => {
    const real = active?.last_target ?? holding?.value ?? run?.curve?.start ?? null;
    if (!active || !planned.length || real == null) return real;
    const smooth = valueAt(planned, elapsedAnim);
    const ti = run.tick_interval_s || 1;
    const slack = Math.abs(valueAt(planned, elapsed + ti) - valueAt(planned, elapsed)) * 1.5 + 1e-9;
    return Math.max(real - slack, Math.min(real + slack, smooth));
  });
  // Spin speed reflects position within the run's own value range, not the raw
  // safety clamp (which for ml/min is 99999 → the pump would never appear to
  // spin).
  const pumpFrac = $derived.by(() => {
    if (!run) return 0;
    const v = (active ? liveValue : holding?.value) ?? 0;
    const top = Math.max(run.curve.start, run.curve.end, 1e-9);
    return Math.max(0, Math.min(1, v / top));
  });

  let stopping = $state(false);
  async function stop() {
    stopping = true;
    err = null;
    try {
      await post(`/api/runs/${loadedId}/stop`);
    } catch (e) {
      err = e.message;
    }
    stopping = false;
  }

  let stoppingPump = $state(false);
  async function stopPump() {
    stoppingPump = true;
    err = null;
    try {
      await post('/api/pump/stop');
    } catch (e) {
      err = e.message;
    }
    stoppingPump = false;
  }
</script>

{#if complete}
  <section class="card done">
    <div class="card-head">
      <div>
        <div class="eyebrow">Feed profile · {run ? run.curve.params.kind : ''}</div>
        <h2>{run?.name ?? 'Run complete'}</h2>
      </div>
      <span class="pill complete">✓&nbsp;complete</span>
    </div>

    {#if err}<div class="err" style="margin-bottom:16px">{err}</div>{/if}

    <FigureBand
      start={run ? run.curve.start : null}
      now={holding.value}
      end={run ? run.curve.end : null}
      {unit}
      {digits}
      direction={run?.direction ?? 'cw'}
      frac={pumpFrac}
    />

    <div class="progress">
      <div class="bar done"><span style="width:100%"></span></div>
      <div class="cap mono">
        <span><b>completed</b>{run ? ` · ran ${dur(run.duration_s)}` : ''}</span>
        <span>pump holding at <b>{num(holding.value, digits)} {unit}</b></span>
      </div>
    </div>

    {#if run}
      <Chart {planned} actual={[]} nowS={run.duration_s} durationS={run.duration_s} {unit} {digits} />

      <div class="meta">
        <div><div class="k">Direction</div><div class="v mono">{run.direction === 'cw' ? 'clockwise' : 'counter-cw'}</div></div>
        <div><div class="k">Started</div><div class="v mono">{stamp(run.started_at)}</div></div>
        <div><div class="k">Finished</div><div class="v mono">{stamp(holding.finished_at)}</div></div>
        <div><div class="k">Setpoint clamp</div><div class="v mono">{num(run.curve.clamp_min, digits)}–{num(run.curve.clamp_max, 0)}</div></div>
      </div>
    {/if}

    <div class="foot done-foot">
      <button class="btn-danger" disabled={stoppingPump} onclick={stopPump}>
        {stoppingPump ? 'Stopping…' : 'Stop pump'}
      </button>
      <button class="btn-ghost" onclick={() => (app.tab = 'new')}>New run</button>
    </div>
  </section>
{:else if !active}
  <div class="card empty">
    <div class="eyebrow">No active run</div>
    <h2>The pump is idle.</h2>
    <p>Build a time profile and start a run.</p>
    <button class="btn-primary" onclick={() => (app.tab = 'new')}>New run</button>
  </div>
{:else}
  <section class="card breathe">
    {#if run}
      <div class="live">
        <span class="live-dot" aria-hidden="true"></span>
        <span class="live-cell mono">{dur(Math.max(0, run.duration_s - elapsed))} left</span>
        <span class="live-cell mono">{pct.toFixed(0)}%</span>
        <span class="live-cell mono">of {dur(run.duration_s)}</span>
      </div>
    {/if}

    <div class="card-head">
      <div>
        <div class="eyebrow">Feed profile · {run ? run.curve.params.kind : ''}</div>
        <h2>{run?.name ?? 'Loading…'}</h2>
      </div>
      <span class="pill running">running</span>
    </div>

    {#if err}<div class="err" style="margin-bottom:16px">{err}</div>{/if}

    {#if run}
      <FigureBand
        start={run.curve.start}
        now={liveValue}
        end={run.curve.end}
        {unit}
        {digits}
        direction={run.direction}
        frac={pumpFrac}
      />

      <div class="progress">
        <div class="bar"><span style="width:{pct}%"></span></div>
        <div class="cap mono"><span>{dur(elapsed)} elapsed · {pct.toFixed(0)}%</span></div>
      </div>

      <Chart
        {planned}
        actual={[]}
        nowS={Math.min(elapsedAnim, run.duration_s - 0.05)}
        durationS={run.duration_s}
        {unit}
        {digits}
      />

      <div class="meta">
        <div><div class="k">Direction</div><div class="v mono">{run.direction === 'cw' ? 'clockwise' : 'counter-cw'}</div></div>
        <div><div class="k">Tick cadence</div><div class="v mono">{run.tick_interval_s} s</div></div>
        <div><div class="k">Setpoint clamp</div><div class="v mono">{num(run.curve.clamp_min, digits)}–{num(run.curve.clamp_max, 0)}</div></div>
        <div><div class="k">Started</div><div class="v mono">{stamp(run.started_at)}</div></div>
      </div>

      <div class="foot">
        <button class="btn-danger" disabled={stopping} onclick={stop}>Stop run</button>
      </div>
    {/if}
  </section>
{/if}

{#if (active || complete) && run}
  <section class="card">
    <div class="card-head"><div><div class="eyebrow">Journal</div><h2>Recent activity</h2></div></div>
    <div class="acts">
      {#each events as e (e.id)}
        <div class="act">
          <time class="mono">{shortTime(e.wall_time)}</time>
          <span class="pill {e.level}">{e.kind}</span>
          <span class="body">{e.detail ?? ''}</span>
        </div>
      {:else}
        <p class="muted">No events yet.</p>
      {/each}
    </div>
  </section>
{/if}

<style>
  /* Slow "breathing" halo on the live run card: signals the pump is working. */
  .breathe { animation: breathe 2.8s ease-in-out infinite alternate; }
  @keyframes breathe {
    from {
      box-shadow:
        0 1px 2px rgba(0, 0, 0, 0.05),
        0 4px 18px color-mix(in srgb, var(--green-500) 12%, transparent);
    }
    to {
      box-shadow:
        0 1px 2px rgba(0, 0, 0, 0.05),
        0 8px 40px color-mix(in srgb, var(--green-500) 40%, transparent);
    }
  }
  /* Calm persistent glow on the completed-run card: the pump is still
     holding its final speed, so the page keeps a gentle "alive" pulse. */
  .done { animation: done-glow 3.6s ease-in-out infinite alternate; }
  @keyframes done-glow {
    from {
      box-shadow:
        0 1px 2px rgba(0, 0, 0, 0.05),
        0 4px 16px color-mix(in srgb, var(--teal-400) 10%, transparent);
    }
    to {
      box-shadow:
        0 1px 2px rgba(0, 0, 0, 0.05),
        0 8px 34px color-mix(in srgb, var(--teal-400) 30%, transparent);
    }
  }
  .pill.complete {
    background: color-mix(in srgb, var(--green-500) 16%, var(--surface));
    color: var(--green-600);
    animation: pill-pulse 2.6s ease-in-out infinite;
  }
  @keyframes pill-pulse {
    0%, 100% { opacity: 1; }
    50% { opacity: 0.68; }
  }
  .bar.done span { background: var(--teal-400); }
  .done-foot { display: flex; gap: var(--s-3); align-items: center; }

  /* Live setpoint readout — sticks to the top of the viewport while scrolling
     so the current rate is always in view during a run. */
  .live {
    position: sticky;
    top: 0;
    z-index: 5;
    display: flex;
    align-items: stretch;
    margin: calc(var(--s-7) * -1) calc(var(--s-7) * -1) var(--s-6);
    padding: 0 var(--s-7);
    background: color-mix(in srgb, var(--surface) 80%, transparent);
    backdrop-filter: blur(10px);
    -webkit-backdrop-filter: blur(10px);
    border-bottom: 1px solid var(--line);
    border-radius: var(--radius) var(--radius) 0 0;
  }
  .live-dot {
    align-self: center;
    flex: none;
    width: 8px; height: 8px; border-radius: 999px;
    margin-right: var(--s-4);
    background: var(--green-500);
    animation: live-pulse 2s ease-in-out infinite;
  }
  @keyframes live-pulse {
    0%, 100% { opacity: 1; transform: scale(1); }
    50% { opacity: 0.5; transform: scale(0.8); }
  }
  /* Equal-size, equal-height compartments, divided by hairlines. */
  .live-cell {
    display: flex;
    align-items: center;
    padding: var(--s-3) var(--s-4);
    font-size: 13px;
    font-weight: 600;
    color: var(--ink);
    white-space: nowrap;
  }
  .live-cell:first-of-type { padding-left: 0; }
  .live-cell + .live-cell { border-left: 1px solid var(--line); }

  @media (prefers-reduced-motion: reduce) {
    .breathe,
    .done,
    .pill.complete,
    .live-dot { animation: none; }
  }

  .empty { text-align: center; }
  .empty h2 { font-size: 18px; margin: 6px 0; }
  .empty p { color: var(--muted); margin: 0 0 var(--s-5); }

  .progress { margin-top: var(--s-5); }
  .bar { height: 6px; border-radius: 999px; background: var(--surface-sunken); overflow: hidden; }
  /* No CSS transition: width is driven straight off the rAF clock, in lockstep
     with the chart's now-marker. A transition here makes the bar lag and then
     snap to 100% at the end. */
  .bar span { display: block; height: 100%; background: var(--green-500); border-radius: 999px; }
  .cap { display: flex; justify-content: space-between; margin-top: var(--s-2); font-size: 12px; color: var(--muted); }
  .cap b { color: var(--ink); font-weight: 600; }

  .meta {
    margin-top: var(--s-6);
    padding-top: var(--s-5);
    border-top: 1px solid var(--line-soft);
    display: grid;
    grid-template-columns: repeat(4, 1fr);
    gap: var(--s-4);
  }
  .meta .k { font-size: 11px; letter-spacing: 0.08em; text-transform: uppercase; color: var(--muted); }
  .meta .v { font-size: 13px; margin-top: 3px; }

  .foot { margin-top: var(--s-6); }

  .acts { display: flex; flex-direction: column; }
  .act {
    display: grid;
    grid-template-columns: 60px 96px 1fr;
    gap: var(--s-3);
    align-items: baseline;
    padding: var(--s-3) 0;
    font-size: 13px;
  }
  .act + .act { border-top: 1px solid var(--line-soft); }
  .act time { color: var(--muted); font-size: 12px; }
  .act .body { color: var(--muted); }
  .muted { color: var(--muted); }

  @media (max-width: 720px) {
    .meta { grid-template-columns: repeat(2, 1fr); }
  }
</style>
