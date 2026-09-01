# Fermentool — design language

Goal: a **modern lab-instrument** UI that people *want* to leave running for days —
calm, white-forward, legible at a glance from across the bench, honest about state.

## Palette

| Token | Hex | Role |
|---|---|---|
| `--teal-700` | `#00707F` | Primary. Actions, active nav, key numbers, links, brand. |
| `--teal-400` | `#5FA4B0` | Secondary accent, focus ring, secondary chart series, muted emphasis. |
| `--white` | `#FFFFFF` | Card / surface background. |
| `--lime-300` | `#B9CD76` | Soft fills — area under the planned curve, subtle badges/highlights. |
| `--green-500` | `#7DB928` | **Live / running** indicator, success, positive deltas. |

Neutrals derived for light mode: page `--bg #F4F7F7`, hairline `--line #E1EAE9`,
text `--ink #0E2A2F`, secondary text `--muted #5C7378`.

One deliberate **out-of-palette** functional colour: `--danger #D64545` for
stop/abort and error states — a pump control must read unambiguously. Nothing
else leaves the palette.

Dark mode: same brand hues, teal lifted for contrast; `--bg #0E1719`, surfaces
`#16211F`. Driven by `prefers-color-scheme`, overridable with
`:root[data-theme="light"|"dark"]`.

## Usage rules

- **Teal is for the primary path**, not decoration. One primary button per view.
- **Green means "this is live"** — the running pill, the actual-value trace on the
  chart, a completed step. Never use green for a neutral accent.
- **Lime is a fill, not a foreground** — it sits behind things (curve area,
  badge backgrounds at low opacity), never as text or an icon colour.
- Charts: planned curve = `--teal-400` line over a faint `--lime-300` area;
  actual points = `--green-500`; the "now" marker = `--teal-700`.
- Status dots: grey = idle/unknown, green = running, amber (derived) = attention,
  `--danger` = fault.

## Form

- Radius: 12 px cards, 8 px controls. Soft two-layer shadow (`--shadow-sm`).
- Type: system UI stack; numeric readouts in the mono stack (`--font-mono`).
  Scale ~ 12 / 14 / 16.8 / 24 px. Tight letter-spacing on headings (-0.01em).
- Spacing scale: 4 / 8 / 12 / 16 / 24 / 32 / 48 (`--s-1` … `--s-7`).
- Motion: 150 ms ease on hover/state; no entrance animation on data.
- Focus is always visible: 2 px `--teal-400` outline, 2 px offset.

## Layout

- Single centred column (~820 px) for the scaffold; the running app grows to a
  two-pane layout (nav rail + content) at milestone 8.
- The big current setpoint is the largest thing on the Active-run view —
  readable from a distance.

Tokens live in [`ui/src/app.css`](../ui/src/app.css).
