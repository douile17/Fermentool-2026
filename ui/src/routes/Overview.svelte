<script>
  import { app } from '../lib/state.svelte.js';
  import { get, post } from '../lib/api.js';
  import { num, dur, shortTime, stamp, unitFor, digitsFor, vol } from '../lib/fmt.js';
  import { loadConfig } from '../lib/config.js';
  import { bottleWeights } from '../lib/balance.js';
  import { valueAt, hm, when, bottleEmptyInS, runState, DELIVERY_TOL as TOL, NOISE_ML, MIN_ASKED_ML, toleranceMl } from '../lib/runview.js';
  import Chart from '../components/Chart.svelte';
  import FigureBand from '../components/FigureBand.svelte';
  import PumpHead from '../components/PumpHead.svelte';
  import RegulationPanel from '../components/RegulationPanel.svelte';

  // Overview follows feeding cycles only: a calibration burst is watched
  // from Tubing calibration, here it only reads as "not a cycle".
  const calibrating = $derived(app.status?.active?.kind === 'calibration');
  const active = $derived(calibrating ? null : (app.status?.active ?? null));
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

  // A boolean, so the animation loop below restarts when a run starts or
  // ends, not on every status frame (`active` is a new object each time).
  const isLive = $derived(!!active);

  // Parse the run's t0 once per status change, not once per animation frame.
  const startedAtMs = $derived(active ? Date.parse(active.started_at) : NaN);

  // The clocks stop with the daemon: a run whose daemon went silent must not
  // look like it is still advancing.
  $effect(() => {
    const id = setInterval(() => {
      if (!app.connected) return;
      now = Date.now();
      nowAnim = Date.now();
    }, 1000);
    return () => clearInterval(id);
  });

  // Glide the chart marker between ticks. rAF (so it pauses in a background
  // tab), but commit at ~15 fps, plenty smooth for the marker, and a fraction
  // of the work over a multi-day run.
  $effect(() => {
    if (!isLive || reducedMotion) return;
    let raf = 0;
    let lastCommit = 0;
    const step = (t) => {
      raf = requestAnimationFrame(step);
      if (app.connected && t - lastCommit >= 66) {
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

  // Refresh the "recent activity" list as the run advances, at most once a
  // second whatever the status frame rate.
  $effect(() => {
    void active?.last_seq;
    if (loadedId == null || !run) return;
    const t = Date.now();
    if (t - lastEventsAt < 1000) return;
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
  // Same as `elapsed` but off the rAF clock, feeds the chart marker only.
  const elapsedAnim = $derived(
    active ? Math.max(0, (nowAnim - startedAtMs) / 1000) : elapsed
  );
  // Off the smooth clock so the progress bar fills continuously, not per tick.
  const pct = $derived(run && run.duration_s ? Math.min(100, (elapsedAnim / run.duration_s) * 100) : 0);
  // Past its curve a dosing run holds the end value, still regulated and
  // journalled, until Stop: show how long it has been holding.
  // The run's volume is stated in one place only. With the balance trim on,
  // that is the tracking panel (weighed vs requested): `volume_added_ml` is
  // what the pump was *told* (setpoint x time, including the correction
  // factor), not what went in, so it is not shown at all. Without the
  // balance it is the only figure there is, shown once, marked estimated.
  const estimatedMl = $derived(active?.gravimetric_trim ? null : (active?.volume_added_ml ?? null));
  const inHold = $derived(!!active?.curve_done);
  const holdS = $derived(inHold && run ? Math.max(0, elapsed - run.duration_s) : 0);

  // The pump holds at a constant rate after natural completion, so the volume
  // delivered since then is just rate * elapsed, no curve integration needed.
  // Ticks off the 1 Hz `now` clock, same as the rest of this "complete" card.
  const holdVolumeSinceMl = $derived.by(() => {
    if (!complete || !holding?.finished_at || holding.value == null) return 0;
    const sinceMs = now - Date.parse(holding.finished_at);
    return sinceMs > 0 ? (holding.value * sinceMs) / 60000 : 0;
  });

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

  // --- the live run, as the Overview states it
  const trimmed = $derived(!!active?.gravimetric_trim);
  // What the curve asks for now (the big figure), off the smooth clock; the
  // value actually written, curve x c, is `active.last_target`.
  const curveNow = $derived(active && planned.length ? valueAt(planned, elapsedAnim) : null);
  const endMs = $derived(run ? startedAtMs + run.duration_s * 1000 : NaN);
  const rs = $derived(runState(app.status, app.connected, inHold));

  // Balance settings the page needs: the correction limit, and whether the
  // balance weighs the feed bottle (then its weight says when it runs dry).
  let limitPct = $state(null);
  let feedSide = $state(true);
  $effect(() => {
    if (!trimmed) return;
    loadConfig()
      .then((cfg) => {
        limitPct = cfg.scale?.trim_limit_pct ?? null;
        feedSide = (cfg.scale?.position ?? 'feed') === 'feed';
      })
      .catch(() => {});
  });

  // When the bottle runs dry: the curve integrated forward, at the grams per
  // minute the balance measures for one unit of setpoint now.
  const weightG = $derived(app.status?.scale_weight_g ?? null);
  const emptyInS = $derived.by(() => {
    void now;
    const s = app.status;
    if (!trimmed || !feedSide || !run || s?.tracking?.alarm || s?.scale_connected === false) return null;
    const rate = Math.abs(s?.rate_g_per_min ?? 0);
    const target = active?.last_target ?? 0;
    if (!(rate > 0) || !(target > 0)) return null;
    return bottleEmptyInS({
      weightG,
      gPerUnit: rate / target,
      planned,
      elapsedS: elapsed,
      durationS: run.duration_s,
      c: s?.trim_c ?? 1,
    });
  });
  const dryMs = $derived(emptyInS != null ? now + emptyInS * 1000 : null);
  // The one thing the operator has to do next, as a clock time.
  const next = $derived.by(() => {
    if (!run) return null;
    if (inHold) return { what: 'Stop the run when you are done', sub: `curve done ${hm(holdS)} ago, pump still feeding` };
    if (dryMs != null && dryMs < endMs)
      return { what: `Refill the feed bottle before ${when(dryMs, now)}`, sub: `in about ${hm(emptyInS)}, ${num(weightG, 1)} g left` };
    if (dryMs != null) return { what: 'Nothing until the run ends', sub: `the bottle lasts past ${when(endMs, now)}` };
    return { what: `Run ends ${when(endMs, now, true)}`, sub: `in ${hm(run.duration_s - elapsed)}` };
  });

  // Weighed against requested, from the tracking report (every second).
  // Keyed on the run id alone: `active` is a new object with every status
  // frame, and an effect reading it directly reran each second, emptying the
  // report and making the page jump.
  const trackId = $derived(trimmed ? (active?.run_id ?? null) : null);
  let report = $state(null);
  $effect(() => {
    const id = trackId;
    report = null;
    if (id == null) return;
    let stop = false;
    // Every second, never two at once (a slow answer is not stacked on).
    let busy = false;
    const load = () => {
      if (busy) return;
      busy = true;
      get(`/api/runs/${id}/tracking`)
        .then((r) => !stop && (report = r))
        .catch(() => {})
        .finally(() => (busy = false));
    };
    load();
    const t = setInterval(load, 1_000);
    return () => {
      stop = true;
      clearInterval(t);
    };
  });
  // Asked and weighed totals off the status frame, every second, as the old
  // page did; the 10 s report is only for R², µ and the charts.
  const lastPt = $derived.by(() => {
    const tr = app.status?.tracking;
    if (trimmed && tr) return [elapsed, tr.required_ml, tr.delivered_ml];
    return report?.points.at(-1) ?? null;
  });
  const gapPct = $derived(lastPt && lastPt[1] > 0 ? (100 * (lastPt[2] - lastPt[1])) / lastPt[1] : null);
  const gapMl = $derived(lastPt ? lastPt[2] - lastPt[1] : null);
  // The bottle end to end, from the same report: start, refills, weighed out.
  // Weighed out runs on to the live weight: the report's figure at its last
  // point, plus what left the bottle since.
  const bottle = $derived.by(() => {
    const b = bottleWeights(report);
    if (!b || weightG == null) return b;
    return { ...b, weighedOut: b.weighedOut + (b.end - weightG) };
  });
  // The tolerance has a 1 mL floor (runview.js): at the start the % swung by
  // ±15 % between reads and the verdict flipped green/red at every refresh.
  const onTarget = $derived(gapMl != null && Math.abs(gapMl) <= toleranceMl(lastPt[1]));
  const early = $derived(lastPt != null && lastPt[1] < MIN_ASKED_ML);
  // The card's tint: light green on target, light red off target or while an
  // alarm holds the correction; none until there is a first weighing.
  const deliveryTone = $derived(
    app.status?.tracking?.alarm ? 'off' : gapMl == null ? null : onTarget ? 'on' : 'off',
  );
  const missedMl = $derived(app.status?.tracking?.missed_ml ?? 0);

  // Stopping ends the run for good (a stopped run is not resumed): asked in
  // a small dialog of its own first, like a delete.
  let confirmStop = $state(false);
  let stopping = $state(false);
  let stopErr = $state(null);
  async function stop() {
    stopping = true;
    stopErr = null;
    try {
      await post(`/api/runs/${loadedId}/stop`);
      confirmStop = false;
    } catch (e) {
      stopErr = e.message;
    }
    stopping = false;
  }
  function onKey(e) {
    if (e.key === 'Escape' && confirmStop && !stopping) confirmStop = false;
  }

  // Bottle refill on a trimmed run: announce it, then "Done" once poured.
  // The balance also sees a refill by itself (a jump, or a steady rise from
  // a transfer pump); the buttons only make it certain.
  const refilling = $derived(
    app.status?.scale_state === 'refill_pending' || app.status?.scale_state === 'refill_settling'
  );
  let refillBusy = $state(false);
  async function refill(done) {
    refillBusy = true;
    err = null;
    try {
      await post(done ? '/api/scale/refill_done' : '/api/scale/refill_mode');
    } catch (e) {
      err = e.message;
    }
    refillBusy = false;
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

<svelte:window on:keydown={onKey} />

{#if confirmStop && active && run}
  <button class="backdrop" aria-label="Cancel" onclick={() => !stopping && (confirmStop = false)}></button>
  <div class="layer">
    <div class="modal card confirm" role="alertdialog" aria-modal="true" aria-labelledby="ft-stop">
      <div class="eyebrow">Stop run</div>
      <h2 id="ft-stop">Stop “{run.name}”?</h2>
      <p class="lede">
        {inHold
          ? 'Its curve is done: the run is recorded completed and the pump stops.'
          : 'The pump stops and the run is recorded stopped: it cannot be resumed.'}
      </p>
      {#if stopErr}<div class="err">{stopErr}</div>{/if}
      <div class="modal-foot">
        <button class="btn-danger" disabled={stopping} onclick={stop}>{stopping ? 'Stopping…' : 'Stop run'}</button>
        <button class="btn-ghost" disabled={stopping} onclick={() => (confirmStop = false)}>Keep running</button>
      </div>
    </div>
  </div>
{/if}

{#if complete}
  <section class="card done">
    <div class="card-head">
      <div>
        <div class="eyebrow">Feed profile | {run ? run.curve.params.kind : ''}</div>
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
        <span><b>completed</b>{run ? ` | ran ${dur(run.duration_s)}` : ''}</span>
        <span>pump holding at <b>{num(holding.value, digits)} {unit}</b></span>
      </div>
    </div>

    {#if run}
      <Chart {planned} actual={[]} nowS={run.duration_s} durationS={run.duration_s} {unit} {digits} />

      <div class="meta">
        <div><div class="k">Direction</div><div class="v mono">{run.direction === 'cw' ? 'clockwise' : 'counter-cw'}</div></div>
        <div><div class="k">Started</div><div class="v mono">{stamp(run.started_at)}</div></div>
        <div><div class="k">Finished</div><div class="v mono">{stamp(holding.finished_at)}</div></div>
        {#if run.control_var === 'ml_min' && holding.volume_added_ml != null}
          <div>
            <div class="k" title="Commanded volume over the run's planned duration, frozen at completion. Estimated, not measured.">
              Volume at run end
            </div>
            <div class="v mono">{vol(holding.volume_added_ml)}</div>
          </div>
          <div>
            <div class="k" title="Above, plus what the pump has kept delivering at its holding rate since the run ended. Keeps climbing until you stop the pump.">
              Total volume (pump still running)
            </div>
            <div class="v mono">{vol(holding.volume_added_ml + holdVolumeSinceMl)}</div>
          </div>
        {/if}
      </div>
    {/if}

    <div class="foot done-foot">
      <button class="btn-danger" disabled={stoppingPump} onclick={stopPump}>
        {stoppingPump ? 'Stopping…' : 'Stop pump'}
      </button>
      <button class="btn-ghost" onclick={() => (app.tab = 'new')}>New run</button>
    </div>
  </section>
{:else if calibrating}
  <div class="card empty">
    <div class="eyebrow">No active run</div>
    <h2>A tubing calibration is pumping.</h2>
    <p>Follow it from Tubing calibration.</p>
    <button class="btn-primary" onclick={() => (app.tab = 'calibration')}>Tubing calibration</button>
  </div>
{:else if !active}
  <div class="card empty">
    <div class="eyebrow">No active run</div>
    <h2>The pump is idle.</h2>
    <p>Build a time profile and start a run.</p>
    <button class="btn-primary" onclick={() => (app.tab = 'new')}>New run</button>
  </div>
{:else}
  <div class="ov">
    <!-- 1. Is everything all right, and what do I do next? -->
    <section class="band {rs.tone}" role={rs.tone === 'bad' ? 'alert' : undefined} aria-label="Run state">
      <div class="band-state">
        {#if rs.tone === 'bad' || rs.tone === 'warn'}
          <svg class="tri" viewBox="0 0 28 28" aria-hidden="true">
            <path d="M14 3 L26 24 H2 Z" />
            <rect x="12.8" y="10" width="2.4" height="8" rx="1.2" />
            <circle cx="14" cy="20.6" r="1.4" />
          </svg>
        {:else}
          <span class="state-dot" aria-hidden="true"></span>
        {/if}
        <div class="band-text">
          <div class="band-title">{rs.title}</div>
          <div class="band-sub">
            {#if rs.detail}{rs.detail}{:else if run}Run {run.id}, {run.name}{/if}
          </div>
        </div>
      </div>
      {#if rs.check || next}
        <div class="band-next">
          {#if rs.check}
            <div class="k">Check</div>
            <div class="what">{rs.check}</div>
            {#if rs.note}<div class="k">{rs.note}</div>{/if}
          {:else}
            <div class="k">Next thing for you</div>
            <div class="what">{next.what}</div>
            <div class="k">{next.sub}</div>
          {/if}
        </div>
      {/if}
      <div class="band-actions">
        {#if trimmed}
          {#if refilling}
            <button class="btn-ghost" disabled={refillBusy} onclick={() => refill(true)}>Refill done</button>
          {:else}
            <button
              class="btn-ghost"
              disabled={refillBusy}
              title="Hold the correction while you refill the bottle (by hand or with a transfer pump). Press Refill done when finished, or let the balance see the weight settle."
              onclick={() => refill(false)}
            >Refill bottle</button>
          {/if}
        {/if}
        <button class="btn-danger" disabled={stopping} onclick={() => { stopErr = null; confirmStop = true; }}>Stop run</button>
      </div>
    </section>

    {#if err}<div class="err">{err}</div>{/if}

    {#if run}
      <div class="row">
        <!-- 2. Is it doing what it should? -->
        <section class="panel rate" aria-label="Feed rate">
          <div class="rate-head">
            <div>
              <div class="k">Feed rate now</div>
              <div class="big-line">
                <PumpHead direction={run.direction} frac={pumpFrac} size={60} spin={app.connected && !app.status?.stop_pending} />
                <span class="big mono-num">{num(curveNow ?? active.last_target, digits)}</span>
                <span class="big-u">{unit}</span>
              </div>
            </div>
            <dl class="facts">
              <dt>Pump driven at</dt><dd class="mono">{num(active.last_target, digits)} {unit}</dd>
              {#if trimmed}
                <dt>Balance trim</dt><dd class="mono">×{num(app.status?.trim_c, 3)}</dd>
              {/if}
              <dt>Curve</dt><dd class="mono">{num(run.curve.start, digits)} → {num(run.curve.end, digits)}</dd>
              <dt>Direction</dt><dd>{run.direction === 'cw' ? 'clockwise' : 'counter-clockwise'}</dd>
            </dl>
          </div>

          <div class="timeline">
            <div class="track">
              <span class="fill" style="width:{pct}%"></span>
              <span class="tick" style="left:{pct}%"></span>
            </div>
            <div class="tl-caps">
              <span>Started {when(startedAtMs, now, true)}</span>
              <span class="strong">
                {#if inHold}curve done, holding for {hm(holdS)}{:else}{hm(elapsed)} done, {hm(run.duration_s - elapsed)} to go{/if}
              </span>
              <span>Ends {when(endMs, now, true)}</span>
            </div>
          </div>

          <Chart
            {planned}
            actual={[]}
            nowS={Math.min(elapsedAnim, run.duration_s - 0.05)}
            durationS={run.duration_s}
            {unit}
            {digits}
          />
        </section>

        <div class="side">
          {#if trimmed}
            <section class="panel delivery" class:on={deliveryTone === 'on'} class:off={deliveryTone === 'off'} aria-label="Delivery">
              <div class="p-head">
                <h3>Delivery</h3>
                {#if app.status?.tracking?.alarm}
                  <span class="tag bad">correction held</span>
                {:else if gapPct != null}
                  <span class="tag" class:ok={onTarget} class:bad={!onTarget}>{onTarget ? 'on target' : 'off target'}</span>
                {/if}
              </div>
              {#if gapPct != null}
                <div class="mid-line">
                  {#if early}
                    <span class="mid mono-num">{Math.abs(gapMl) < 0.05 ? '' : gapMl > 0 ? '+' : '−'}{num(Math.abs(gapMl), 1)} mL</span>
                    <span class="k">tolerance ±{NOISE_ML} mL until {MIN_ASKED_ML} mL asked</span>
                  {:else}
                    <span class="mid mono-num">{Math.abs(gapPct) < 0.05 ? '' : gapPct > 0 ? '+' : '−'}{num(Math.abs(gapPct), 1)} %</span>
                    <span class="k">limit ±{TOL * 100} %</span>
                  {/if}
                </div>
                <dl class="stats">
                  <div><dt>R²</dt><dd class="mono">{report?.r_squared != null ? report.r_squared.toFixed(5) : '·'}</dd></div>
                  {#if report?.mu_requested != null}
                    <div><dt>µ asked</dt><dd class="mono">{report.mu_requested.toFixed(4)} h⁻¹</dd></div>
                    <div><dt>µ delivered</dt><dd class="mono">{report.mu_delivered != null ? report.mu_delivered.toFixed(4) : '·'} h⁻¹</dd></div>
                  {/if}
                </dl>
                <p class="k">{vol(lastPt[2])} weighed out of {vol(lastPt[1])} asked since the start.</p>
                {#if missedMl > 0.05}
                  <p class="k">Of which {vol(missedMl)} missed while the feed was stopped, not caught up.</p>
                {/if}
              {:else}
                <p class="k">Waiting for the first weighings.</p>
              {/if}
            </section>

            {#if feedSide}
              <section class="panel grow" class:alarmed={app.status?.tracking?.alarm === 'feed_stopped'} aria-label="Feed bottle">
                <div class="p-head">
                  <h3>Feed bottle</h3>
                  {#if app.status?.tracking?.alarm === 'feed_stopped'}
                    <span class="tag bad">not moving</span>
                  {:else if app.status?.scale_connected === false}
                    <span class="tag bad">balance not answering</span>
                  {:else}
                    <span class="k">{app.status?.scale_stable === false ? 'balance settling' : 'balance stable'}</span>
                  {/if}
                </div>
                <!-- What left the bottle, large; start and the live reading beside. -->
                <div class="mid-line">
                  <span class="mid mono-num">{bottle ? `${num(bottle.weighedOut, 1)} g` : '–'}</span>
                  <span class="k">weighed out</span>
                </div>
                {#if dryMs != null}
                  <p class="k">Empty around <b>{when(dryMs, now)}</b>, in about {hm(emptyInS)}.</p>
                {:else if app.status?.tracking?.alarm === 'feed_stopped'}
                  <p class="k">The pump turns but this weight does not fall: look at the line first.</p>
                {/if}
                <dl class="stats">
                  <div><dt>At start</dt><dd class="mono">{bottle ? `${num(bottle.start, 1)} g` : '–'}</dd></div>
                  <div><dt>On the balance</dt><dd class="mono">{weightG != null ? `${num(weightG, 1)} g` : '–'}</dd></div>
                  {#if bottle?.refills.length}
                    <div>
                      <dt>Refilled</dt>
                      <dd class="mono">+{num(bottle.refills.reduce((g, r) => g + r.after_g - r.before_g, 0), 1)} g</dd>
                    </div>
                  {/if}
                </dl>
              </section>
            {/if}
          {:else}
            <section class="panel grow" aria-label="Run">
              <div class="p-head"><h3>This run</h3><span class="k">no balance</span></div>
              {#if run.control_var === 'ml_min' && estimatedMl != null}
                <div class="mid-line">
                  <span class="mid mono-num">{vol(estimatedMl)}</span>
                  <span class="k">pumped (estimated)</span>
                </div>
                <p class="k">Integrated from the setpoint, not measured: no balance on this run.</p>
              {/if}
              <dl class="stats">
                <div><dt>Started</dt><dd class="mono">{stamp(run.started_at)}</dd></div>
                {#if run.responsible}<div><dt>Alerts to</dt><dd>{run.responsible}</dd></div>{/if}
              </dl>
            </section>
          {/if}
        </div>
      </div>

      {#if trimmed}
        <!-- 3. What did the regulation do, and when? -->
        <section class="panel" aria-label="Balance regulation">
          <div class="p-head">
            <h3>Balance regulation</h3>
            <span class="k">
              {#if app.status?.tracking?.alarm}Correction held at ×{num(app.status?.trim_c, 3)} while the alarm lasts.
              {:else if limitPct != null}Correction ×{num(app.status?.trim_c, 3)}, limit ±{limitPct} %. Delivery tolerance ±{TOL * 100} % of what was asked.
              {/if}
            </span>
          </div>
          <RegulationPanel
            runId={active.run_id}
            {report}
            {planned}
            durationS={run.duration_s}
            nowS={elapsed}
            {startedAtMs}
            {limitPct}
            {feedSide}
            {emptyInS}
            live={{
              c: app.status?.trim_c,
              weightG,
              askedMl: app.status?.tracking?.required_ml,
              weighedMl: app.status?.tracking?.delivered_ml,
            }}
          />
        </section>
      {/if}
    {/if}
  </div>
{/if}

{#if (active || complete) && run}
  <section class="panel journal" aria-label="Journal">
    <div class="p-head">
      <h3>Recent activity</h3>
      <button class="link" onclick={() => (app.tab = 'history')}>Full journal in History</button>
    </div>
    <ol class="acts">
      {#each events as e (e.id)}
        <li class="act {e.level}">
          <time class="mono">{shortTime(e.wall_time)}</time>
          <span><b>{e.kind.replaceAll('_', ' ')}</b>{#if e.detail}<span class="body">, {e.detail}</span>{/if}</span>
        </li>
      {:else}
        <li class="muted">No events yet.</li>
      {/each}
    </ol>
  </section>
{/if}

<style>
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

  @media (prefers-reduced-motion: reduce) {
    .done,
    .pill.complete { animation: none; }
  }

  .backdrop {
    position: fixed; inset: 0; border: none; padding: 0;
    background: rgba(18, 41, 46, 0.28); cursor: default; z-index: 60;
  }
  .layer {
    position: fixed; inset: 0; display: grid; place-items: center;
    padding: var(--s-5); z-index: 61; pointer-events: none;
  }
  .modal { box-shadow: var(--shadow-pop); pointer-events: auto; animation: pop 0.18s ease-out; }
  @keyframes pop {
    from { transform: scale(0.97); opacity: 0; }
    to { transform: scale(1); opacity: 1; }
  }
  .confirm { max-width: 420px; }
  .confirm h2 { font-size: 17px; margin: 6px 0 var(--s-3); }
  .confirm .lede { margin: 0; color: var(--muted); font-size: 13px; }
  .confirm .err { margin-top: var(--s-3); }
  .modal-foot { display: flex; gap: var(--s-3); justify-content: flex-end; flex-wrap: wrap; margin-top: var(--s-5); }
  @media (prefers-reduced-motion: reduce) { .modal { animation: none; } }

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

  .foot { margin-top: var(--s-6); display: flex; gap: var(--s-4); align-items: center; flex-wrap: wrap; }
  .hold-note { font-size: 12px; color: var(--muted); }


  /* --- live run: one question per zone, colour only when it needs you --- */
  .ov { display: flex; flex-direction: column; gap: var(--s-4); }
  .k { font-size: 12.5px; color: var(--muted); margin: 0; }
  .mono-num { font-variant-numeric: tabular-nums; }

  .band {
    display: flex; flex-wrap: wrap; align-items: center; gap: var(--s-4) var(--s-6);
    padding: var(--s-5) var(--s-5);
    background: var(--surface); border: 1px solid var(--line); border-radius: var(--radius);
  }
  .band-state { display: flex; align-items: center; gap: var(--s-3); flex: 1 1 280px; min-width: 0; }
  .state-dot {
    flex: none; width: 14px; height: 14px; border-radius: 50%;
    background: var(--green-500);
    box-shadow: 0 0 0 5px color-mix(in srgb, var(--green-500) 16%, transparent);
  }
  .band.info .state-dot { background: var(--teal-700); box-shadow: 0 0 0 5px color-mix(in srgb, var(--teal-700) 14%, transparent); }
  .band.idle .state-dot { background: var(--muted); box-shadow: 0 0 0 5px color-mix(in srgb, var(--muted) 16%, transparent); }
  .tri { flex: none; width: 28px; height: 28px; }
  .tri path { fill: var(--danger-solid); }
  .tri rect, .tri circle { fill: #fff; }
  .band.warn .tri path { fill: var(--measured); }
  .band-title { font-size: 19px; font-weight: 600; line-height: 1.25; }
  .band-sub { color: var(--muted); }
  .band-next {
    flex: 1 1 260px; min-width: 0;
    padding-left: var(--s-4); border-left: 2px solid var(--line);
  }
  .band-next .what { font-weight: 600; font-size: 15px; }
  .band-actions { display: flex; gap: var(--s-2); flex-wrap: wrap; }
  .band-actions button { min-height: 40px; }
  /* An alarm: the band, and only the band, turns red. */
  .band.bad {
    background: var(--danger-bg);
    border-color: color-mix(in srgb, var(--danger) 35%, var(--surface));
  }
  .band.bad .band-title { color: var(--danger); }
  .band.bad .band-next { border-left-color: color-mix(in srgb, var(--danger) 25%, transparent); }
  .band.bad .btn-ghost { background: var(--surface); border-color: color-mix(in srgb, var(--danger) 25%, var(--surface)); }
  .band.warn { border-color: color-mix(in srgb, var(--measured) 45%, var(--surface)); }

  .row { display: flex; flex-wrap: wrap; gap: var(--s-4); align-items: stretch; }
  .panel {
    background: var(--surface); border: 1px solid var(--line); border-radius: var(--radius);
    padding: var(--s-5) var(--s-5); min-width: 0;
    display: flex; flex-direction: column; gap: var(--s-3);
  }
  .panel h3 { margin: 0; font-size: 15px; font-weight: 600; letter-spacing: 0; }
  .p-head { display: flex; justify-content: space-between; align-items: baseline; gap: var(--s-3); flex-wrap: wrap; }
  .rate { flex: 2 1 560px; gap: var(--s-5); }
  .side { flex: 1 1 300px; min-width: 0; display: flex; flex-direction: column; gap: var(--s-4); }
  .side .grow { flex: 1 1 auto; }
  .panel.alarmed { border: 2px solid var(--danger); }
  .delivery { transition: background 0.3s ease, border-color 0.3s ease; }
  /* Strong enough to read at a glance, light enough to keep black text
     crisp; red a touch lighter than green, as it reads stronger. */
  .delivery.on {
    /* The darkest stop of the curve chart's area gradient (Chart.svelte). */
    background: color-mix(in srgb, var(--lime-300) 42%, var(--surface));
    border-color: color-mix(in srgb, var(--green-500) 45%, var(--surface));
  }
  .delivery.off {
    background: color-mix(in srgb, var(--danger) 12%, var(--surface));
    border-color: color-mix(in srgb, var(--danger) 45%, var(--surface));
  }
  .delivery .stats { border-top-color: color-mix(in srgb, var(--ink) 8%, transparent); }
  @media (prefers-reduced-motion: reduce) { .delivery { transition: none; } }

  .rate-head { display: flex; flex-wrap: wrap; align-items: flex-end; justify-content: space-between; gap: var(--s-4) var(--s-6); }
  .big-line { display: flex; align-items: center; gap: var(--s-3); }
  .big { font-size: 64px; font-weight: 500; line-height: 1; letter-spacing: -0.025em; }
  .big-u { font-size: 18px; color: var(--muted); align-self: flex-end; padding-bottom: 6px; }
  .facts { margin: 0; display: grid; grid-template-columns: auto auto; gap: 3px var(--s-4); font-size: 13px; }
  .facts dt { color: var(--muted); }
  .facts dd { margin: 0; }

  .timeline { display: flex; flex-direction: column; gap: var(--s-2); }
  .track { position: relative; height: 8px; border-radius: 999px; background: var(--surface-sunken); }
  /* No CSS transition: driven off the rAF clock, in step with the chart's
     now-marker. */
  .track .fill { position: absolute; inset: 0 auto 0 0; border-radius: 999px; background: var(--teal-700); }
  .track .tick { position: absolute; top: -5px; width: 3px; height: 18px; margin-left: -1px; border-radius: 2px; background: var(--ink); }
  .tl-caps { display: flex; justify-content: space-between; gap: var(--s-3); flex-wrap: wrap; font-size: 12.5px; color: var(--muted); }
  .tl-caps .strong { color: var(--ink); font-weight: 500; }

  .mid-line { display: flex; flex-wrap: wrap; align-items: baseline; gap: 2px var(--s-2); }
  .mid { white-space: nowrap; font-size: 30px; font-weight: 500; letter-spacing: -0.02em; line-height: 1.1; }
  .tag { font-size: 12.5px; font-weight: 500; color: var(--muted); }
  .tag.ok { color: var(--green-600); }
  .tag.bad { color: var(--danger); font-weight: 600; }
  .stats {
    margin: 0; padding-top: var(--s-3); border-top: 1px solid var(--line-soft);
    display: grid; grid-template-columns: repeat(auto-fit, minmax(90px, 1fr)); gap: var(--s-3);
  }
  .stats dt { font-size: 12px; color: var(--muted); }
  .stats dd { margin: 0; font-size: 14.5px; font-weight: 500; }
  .panel b { color: var(--ink); font-weight: 600; }

  .journal { margin-top: var(--s-4); }
  .link { background: none; border: none; padding: 0; color: var(--teal-700); font-size: 13px; }
  .link:hover { text-decoration: underline; }
  .acts {
    list-style: none; margin: 0; padding: 0;
    display: grid; grid-template-columns: repeat(auto-fit, minmax(300px, 1fr)); gap: 0 var(--s-6);
  }
  .act {
    display: grid; grid-template-columns: 5.5em 1fr; gap: var(--s-3); align-items: baseline;
    padding: var(--s-2) 0; border-top: 1px solid var(--line-soft); font-size: 13px; min-width: 0;
  }
  .act time { color: var(--muted); font-size: 12px; white-space: nowrap; }
  .act b { font-weight: 500; }
  .act .body { color: var(--muted); overflow-wrap: anywhere; }
  .act.warn b { color: #a2621c; }
  .act.error b { color: var(--danger); }
  .muted { color: var(--muted); }

  @media (max-width: 720px) {
    .meta { grid-template-columns: repeat(2, 1fr); }
  }
</style>
