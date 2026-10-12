<script>
  // An explanation kept at hand rather than in the way: a small "i" next to
  // what it explains, opening a bubble over the page on a click (or Enter).
  // The bubble floats (position: fixed), so nothing around it moves and no
  // card can clip it. It closes on a second click, a click elsewhere,
  // Escape, or a scroll; a resize moves it along. Click rather than hover: it works on a touch screen
  // and stays open while it is read.
  import Icon from './Icon.svelte';

  let { label = 'More about this', children } = $props();
  let open = $state(false);
  let btn = $state(null);
  let bubble = $state(null);
  let pos = $state({ left: 0, top: 0, arrow: 0, above: false });
  const id = `hint-${Math.random().toString(36).slice(2, 9)}`;

  const WIDTH = 300;
  const GAP = 8;
  const MARGIN = 12;

  function place() {
    if (!btn) return;
    const r = btn.getBoundingClientRect();
    const w = Math.min(WIDTH, window.innerWidth - 2 * MARGIN);
    const cx = r.left + r.width / 2;
    const left = Math.max(MARGIN, Math.min(cx - w / 2, window.innerWidth - MARGIN - w));
    const h = bubble?.offsetHeight ?? 0;
    // Below the button, or above it when there is no room under it.
    const above = r.bottom + GAP + h > window.innerHeight - MARGIN && r.top - GAP - h > MARGIN;
    pos = { left, top: above ? r.top - GAP - h : r.bottom + GAP, arrow: cx - left, above, w };
  }

  // The bubble lives on <body>: an animated or clipping ancestor (the page
  // transition's transform, a card's overflow) would otherwise become its
  // positioning box and hide or shift it.
  function portal(node) {
    document.body.appendChild(node);
    return { destroy: () => node.remove() };
  }

  function toggle() {
    open = !open;
    if (open) requestAnimationFrame(place);
  }

  $effect(() => {
    if (!open) return;
    place();
    const close = () => (open = false);
    const onDown = (e) => {
      if (!btn?.contains(e.target) && !bubble?.contains(e.target)) close();
    };
    const onKey = (e) => {
      if (e.key === 'Escape') {
        close();
        btn?.focus();
      }
    };
    document.addEventListener('pointerdown', onDown, true);
    document.addEventListener('keydown', onKey);
    window.addEventListener('scroll', close, true);
    window.addEventListener('resize', place);
    return () => {
      document.removeEventListener('pointerdown', onDown, true);
      document.removeEventListener('keydown', onKey);
      window.removeEventListener('scroll', close, true);
      window.removeEventListener('resize', place);
    };
  });
</script>

<button
  type="button"
  class="hint-btn"
  class:open
  bind:this={btn}
  aria-expanded={open}
  aria-controls={id}
  aria-label={label}
  title={open ? undefined : label}
  onclick={toggle}
><Icon name="info" size={14} /></button>
{#if open}
  <span
    class="bubble"
    class:above={pos.above}
    role="tooltip"
    use:portal
    {id}
    bind:this={bubble}
    style="left:{pos.left}px; top:{pos.top}px; width:{pos.w ?? WIDTH}px; --arrow:{pos.arrow}px"
  >{@render children()}</span>
{/if}

<style>
  .hint-btn {
    display: inline-flex; align-items: center; justify-content: center;
    width: 22px; height: 22px; padding: 0; margin: -4px 0 -4px 4px;
    vertical-align: middle;
    border: none; border-radius: 999px;
    background: transparent; color: var(--muted);
    cursor: pointer;
    transition: color 0.15s ease, background 0.15s ease;
  }
  .hint-btn:hover, .hint-btn.open { color: var(--teal-700); background: color-mix(in srgb, var(--teal-700) 10%, transparent); }
  .hint-btn:focus-visible { outline: 2px solid var(--teal-400); outline-offset: 1px; }

  /* Floats over the page: whatever line it hangs from (often an uppercase
     eyebrow or a field label) keeps its layout and styling to itself. */
  .bubble {
    position: fixed; z-index: 80; box-sizing: border-box;
    display: block; padding: 10px 12px;
    border-radius: 10px; border: 1px solid var(--line);
    background: var(--surface); color: var(--ink);
    box-shadow: var(--shadow-pop);
    font-size: 12.5px; font-weight: 400; line-height: 1.5;
    letter-spacing: normal; text-transform: none; text-align: left; white-space: normal;
    animation: bubble-in 0.12s ease-out;
  }
  /* The little arrow, pointing at the "i". */
  .bubble::before {
    content: ''; position: absolute; left: calc(var(--arrow) - 6px); top: -6px;
    width: 10px; height: 10px; transform: rotate(45deg);
    background: var(--surface); border-left: 1px solid var(--line); border-top: 1px solid var(--line);
  }
  .bubble.above::before { top: auto; bottom: -6px; border: none; border-right: 1px solid var(--line); border-bottom: 1px solid var(--line); }
  @keyframes bubble-in {
    from { opacity: 0; transform: translateY(-3px); }
    to { opacity: 1; transform: none; }
  }
  @media (prefers-reduced-motion: reduce) {
    .bubble { animation: none; }
    .hint-btn { transition: none; }
  }
</style>
