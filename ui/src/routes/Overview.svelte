<script>
  import { app } from '../lib/state.svelte.js';
  import { get, post } from '../lib/api.js';
  import { num, dur, clock, shortTime, unitFor } from '../lib/fmt.js';
  import Chart from '../components/Chart.svelte';
  import FigureBand from '../components/FigureBand.svelte';

  const active = $derived(app.status?.active ?? null);

  let run = $state(null);
  let planned = $state([]);
  let ticks = $state([]);
  let events = $state([]);
  let err = $state(null);
  let loadedId = $state(null);
  let now = $state(Date.now());

  $effect(() => {
    const id = setInterval(() => (now = Date.now()), 1000);
    return () => clearInterval(id);
  });

  $effect(() => {
    const id = active?.run_id ?? null;
    if (id === loadedId) return;
    loadedId = id;
    run = null;
    planned = [];
    ticks = [];
    events = [];
    err = null;
    if (id != null) load(id);
  });

  // refetch ticks + events whenever the WS advances the tick sequence
  $effect(() => {
    const seq = active?.last_seq;
    void seq;
    if (loadedId != null && run) {
      get(`/api/runs/${loadedId}/ticks`).then((t) => (ticks = t)).catch(() => {});
      get(`/api/runs/${loadedId}/events?limit=6`).then((e) => (events = e)).catch(() => {});
    }
  });

  async function load(id) {
    try {
      run = await get(`/api/runs/${id}`);
      const p = await post('/api/preview', { curve: run.curve, samples: 200 });
      planned = p.series;
      ticks = await get(`/api/runs/${id}/ticks`);
      events = await get(`/api/runs/${id}/events?limit=6`);
    } catch (e) {
      err = e.message;
    }
  }

  const unit = $derived(run ? unitFor(run.control_var) : 'rpm');
  const elapsed = $derived(
    active ? Math.max(0, (now - Date.parse(active.started_at)) / 1000) : 0
  );
  const pct = $derived(run && run.duration_s ? Math.min(100, (elapsed / run.duration_s) * 100) : 0);
  const actualSeries = $derived(ticks.map((t) => [t.elapsed_s, t.target]));
  const delta = $derived(
    ticks.length >= 2 ? ticks[ticks.length - 1].target - ticks[ticks.length - 2].target : null
  );

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
</script>

{#if !active}
  <div class="card empty">
    <div class="eyebrow">No active run</div>
    <h2>The pump is idle.</h2>
    <p>Build a time profile and start a run.</p>
    <button class="btn-primary" onclick={() => (app.route = 'new')}>New run</button>
  </div>
{:else}
  <section class="card">
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
        now={active.last_target}
        end={run.curve.end}
        {unit}
        {delta}
      />

      <div class="progress">
        <div class="bar"><span style="width:{pct}%"></span></div>
        <div class="cap mono">
          <span><b>{dur(elapsed)}</b> elapsed · {clock(elapsed)}</span>
          <span>{dur(Math.max(0, run.duration_s - elapsed))} left · <b>{dur(run.duration_s)}</b></span>
        </div>
      </div>

      <Chart {planned} actual={actualSeries} nowS={elapsed} durationS={run.duration_s} {unit} />

      <div class="meta">
        <div><div class="k">Direction</div><div class="v mono">{run.direction === 'cw' ? 'clockwise' : 'counter-cw'}</div></div>
        <div><div class="k">Tick cadence</div><div class="v mono">{run.tick_interval_s} s</div></div>
        <div><div class="k">Setpoint clamp</div><div class="v mono">{num(run.curve.clamp_min, 1)}–{num(run.curve.clamp_max, 0)}</div></div>
        <div><div class="k">Started</div><div class="v mono">{shortTime(run.started_at)}</div></div>
      </div>

      <div class="foot">
        <button class="btn-danger" disabled={stopping} onclick={stop}>Stop run</button>
      </div>
    {/if}
  </section>

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
  .empty { text-align: center; }
  .empty h2 { font-size: 18px; margin: 6px 0; }
  .empty p { color: var(--muted); margin: 0 0 var(--s-5); }

  .progress { margin-top: var(--s-5); }
  .bar { height: 6px; border-radius: 999px; background: var(--surface-sunken); overflow: hidden; }
  .bar span { display: block; height: 100%; background: var(--green-500); border-radius: 999px; transition: width 0.4s ease; }
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
