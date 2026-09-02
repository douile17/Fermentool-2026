<script>
  // Peristaltic pump-head glyph, rebuilt as clean vector from the product SVG.
  // Housing + ring are static; hub + 4 rollers spin, centred in the square.
  // `frac` (0..1, share of clamp_max) drives the rotation *speed*;
  // `direction` ('cw' | 'ccw') sets which way it turns.
  //
  // The angle is integrated frame-by-frame (rAF) rather than handed to a CSS
  // `animation-duration`: a live setpoint changes `frac` continuously, and
  // retiming a running CSS animation makes the rotor visibly jump. Accumulating
  // the angle means a speed change only bends the rate from here on.
  let { direction = 'cw', frac = 0, size = 84 } = $props();

  let angle = $state(0);

  const reduced =
    typeof window !== 'undefined' &&
    window.matchMedia &&
    window.matchMedia('(prefers-reduced-motion: reduce)').matches;

  $effect(() => {
    if (reduced) return;
    let raf = 0;
    let last = 0;

    const step = (now) => {
      raf = requestAnimationFrame(step);
      if (!last) {
        last = now;
        return;
      }
      const dt = Math.min(now - last, 100); // ignore long gaps (tab was hidden)
      last = now;

      const f = Math.max(0, Math.min(1, frac));
      // 4 s per turn at rest → 0.6 s per turn at full flow
      const secPerTurn = 4 - f * 3.4;
      const dir = direction === 'ccw' ? -1 : 1;
      angle = (angle + dir * (360 / (secPerTurn * 1000)) * dt) % 360;
    };

    raf = requestAnimationFrame(step);
    return () => cancelAnimationFrame(raf);
  });
</script>

<div class="pump" style="width:{size}px; height:{size}px" aria-hidden="true">
  <svg viewBox="0 0 100 100">
    <!-- housing -->
    <rect class="wall" x="12" y="14" width="76" height="72" rx="13" />
    <circle class="ring" cx="50" cy="50" r="29" />

    <g class="rotor" style="transform: rotate({angle}deg)">
      <!-- symmetric bbox so fill-box rotation stays centred on the ring -->
      <circle cx="50" cy="50" r="26" fill="none" stroke="none" />
      <circle class="hub" cx="50" cy="50" r="8" />
      <circle class="roller" cx="35.5" cy="36.5" r="4.6" />
      <circle class="roller" cx="64.5" cy="36.5" r="4.6" />
      <circle class="roller" cx="35.5" cy="63.5" r="4.6" />
      <circle class="roller" cx="64.5" cy="63.5" r="4.6" />
    </g>
  </svg>
</div>

<style>
  .pump { display: block; flex: none; }
  svg { width: 100%; height: 100%; overflow: visible; }

  .wall {
    fill: var(--surface);
    stroke: var(--muted);
    stroke-width: 2.4;
  }
  .ring {
    fill: var(--surface);
    stroke: var(--muted);
    stroke-width: 3;
  }

  .rotor {
    transform-box: fill-box;
    transform-origin: center;
  }

  .hub,
  .roller { fill: var(--ink); }
</style>
