<script>
  import { num } from '../lib/fmt.js';
  import PumpHead from './PumpHead.svelte';
  let {
    start,
    now = null,
    end,
    unit = 'rpm',
    direction = 'cw',
    frac = 0,
    digits = 1,
  } = $props();
</script>

<div class="band">
  <div class="fig side">
    <span class="lbl">Start</span>
    <span class="mono val">{num(start, digits)}<span class="u">&nbsp;{unit}</span></span>
  </div>

  <div class="arrow" aria-hidden="true">→</div>

  <div class="fig now">
    <span class="lbl">Now</span>
    <PumpHead {direction} {frac} />
    <span class="mono val big">{now == null ? '–' : num(now, digits)}</span>
    <span class="u nowu">{unit}</span>
  </div>

  <div class="arrow" aria-hidden="true">→</div>

  <div class="fig side end">
    <span class="lbl">Target</span>
    <span class="mono val">{num(end, digits)}<span class="u">&nbsp;{unit}</span></span>
  </div>
</div>

<style>
  .band {
    display: grid;
    grid-template-columns: 1fr auto 1fr auto 1fr;
    align-items: end;
    gap: var(--s-4);
    padding-bottom: var(--s-5);
    border-bottom: 1px solid var(--line-soft);
  }
  .fig { display: flex; flex-direction: column; gap: var(--s-2); }
  .fig.end { text-align: right; }
  .fig.now { align-items: center; text-align: center; }
  .lbl {
    font-size: 11px; font-weight: 600; letter-spacing: 0.11em;
    text-transform: uppercase; color: var(--muted);
  }
  .val { font-weight: 600; color: var(--ink); font-size: 30px; letter-spacing: -0.02em; }
  .val.big { font-size: 52px; line-height: 1; letter-spacing: -0.03em; }
  .u { font-size: 14px; color: var(--muted); }
  .nowu { font-size: 13px; letter-spacing: 0.04em; margin-top: -2px; }
  .arrow {
    color: var(--muted);
    font-size: 18px;
    align-self: center;
    padding-bottom: 14px;
  }

  @media (max-width: 720px) {
    .band { grid-template-columns: 1fr; text-align: left; }
    .fig.end, .fig.now { text-align: left; align-items: flex-start; }
    .arrow { display: none; }
  }
</style>
