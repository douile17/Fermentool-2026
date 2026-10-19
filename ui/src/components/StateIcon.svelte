<script>
  /**
   * The run state's large picture, in its ring: what the Overview band and
   * the sidebar show beside the state's word (runState().icon).
   * @type {{ icon: string, size?: number, ring?: boolean, moving?: boolean }}
   */
  let { icon, size = 56, ring = true, moving = true } = $props();

  // Two icons can share a page (band and sidebar): each its own clip path.
  const clipId = `ftbottle-${Math.random().toString(36).slice(2, 9)}`;
  const BOTTLE = 'M22 8h12v8l6 6v26a4 4 0 0 1-4 4H20a4 4 0 0 1-4-4V22l6-6z';
  // The bottle's box is 16..40 x 8..52: centred on the ring, a little smaller.
  const BOTTLE_FIT = 'translate(28 28) scale(.72) translate(-28 -30)';
</script>

<svg class="si {icon}" class:moving class:ring width={size} height={size} viewBox="0 0 56 56" aria-hidden="true">
  {#if ring}<circle class="ring-c" cx="28" cy="28" r="26" />{/if}

  {#if icon === 'pump'}
    <circle cx="28" cy="28" r="17" fill="none" class="track" stroke-width="3" />
    <g class="rotor">
      <circle cx="28" cy="11" r="4.5" />
      <circle cx="42.7" cy="36.5" r="4.5" />
      <circle cx="13.3" cy="36.5" r="4.5" />
      <circle cx="28" cy="28" r="5" class="hub" />
    </g>
  {:else if icon === 'fill' || icon === 'full'}
    <g transform={BOTTLE_FIT}>
      <defs><clipPath id={clipId}><path d={BOTTLE} /></clipPath></defs>
      <g clip-path="url(#{clipId})">
        <!-- filling: the level goes up and down; full: to the top, still -->
        <rect class="liq" class:rise={icon === 'fill'} x="10" y={icon === 'full' ? 13 : 30} width="36" height="47" />
        <circle class="bub" cx="24" cy="46" r="1.6" />
        <circle class="bub b2" cx="31" cy="48" r="1.3" />
        <circle class="bub b3" cx="27" cy="44" r="1.1" />
      </g>
      <path d={BOTTLE} class="outline" />
    </g>
  {:else if icon === 'bottle-warn'}
    <g transform={BOTTLE_FIT}>
      <path d={BOTTLE} class="outline" />
      <rect x="26.6" y="24" width="2.8" height="13" rx="1.4" class="mark" />
      <circle cx="28" cy="42" r="1.7" class="mark" />
    </g>
  {:else if icon === 'alarm' || icon === 'warn'}
    <!-- equilateral, its three corners 16 from the ring's centre -->
    <path d="M28 12 L41.86 36 H14.14 Z" class="tri" stroke-width="2" stroke-linejoin="round" />
    <rect x="26.8" y="20.5" width="2.4" height="8.5" rx="1.2" class="bang" />
    <circle cx="28" cy="32.2" r="1.5" class="bang" />
  {:else}
    <circle cx="28" cy="28" r="7" class="dot" />
  {/if}
</svg>

<style>
  .si { flex: none; display: block; overflow: visible; }
  .ring-c { fill: var(--surface); stroke: var(--si-ring, var(--line)); stroke-width: 2; }

  .pump .track { stroke: var(--green-500); stroke-opacity: 0.35; }
  .pump .rotor { fill: var(--green-500); transform-origin: 28px 28px; }
  .pump .hub { fill: var(--green-600); }

  .liq { fill: var(--fill); fill-opacity: 0.85; }
  .bub { fill: #fff; }
  .outline { fill: none; stroke: var(--fill); stroke-width: 2.2; stroke-linejoin: round; }

  .bottle-warn .outline { stroke: var(--measured); }
  .bottle-warn .mark { fill: var(--measured); }

  .tri { fill: var(--danger); stroke: var(--danger); }
  .warn .tri { fill: var(--measured); stroke: var(--measured); }
  .bang { fill: #fff; }

  .dot { fill: var(--muted); }

  /* Moving only while something happens: a frozen page also looks frozen. */
  .moving .rotor { animation: si-spin 3s linear infinite; }
  .moving .liq.rise { animation: si-rise 4s ease-in-out infinite; }
  .moving .bub { animation: si-bub 2s ease-in infinite; }
  .moving .bub.b2 { animation-delay: 0.7s; }
  .moving .bub.b3 { animation-delay: 1.3s; }
  .bub { opacity: 0; }
  .si:not(.moving) .bub { opacity: 0.8; }
  @keyframes si-spin { to { transform: rotate(360deg); } }
  @keyframes si-rise { 0%, 100% { transform: translateY(8px); } 50% { transform: translateY(0); } }
  @keyframes si-bub {
    0% { transform: translateY(0); opacity: 0; }
    20% { opacity: 1; }
    100% { transform: translateY(-14px); opacity: 0; }
  }
  @media (prefers-reduced-motion: reduce) {
    .moving .rotor, .moving .liq.rise, .moving .bub { animation: none; }
    .moving .bub { opacity: 0.8; }
  }
</style>
