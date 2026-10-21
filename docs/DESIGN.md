# Fermentool, design language

The design direction, settled with the user over the 2026-10-19/20 redesign.
**Read this before any UI change**, and follow it rather than re-proposing what
is decided here. Tokens live in [`ui/src/app.css`](../ui/src/app.css).

Goal: a **modern lab instrument** people leave running for days. Calm,
white-forward, read at a glance from across the bench, honest about state.

## 1. How a UI change is made

1. **Mockup first, on a separate page** (an artifact), never straight into
   `ui/`. Iterate on it with the user.
2. Implement in the app only when the user says so ("mets en place", "on part
   là-dessus", "go").
3. Keep the navigation: rail with Feeding profile (Overview / History / Tubing
   calibration tabs) and Settings (gear), the light/dark toggle.
4. Verify on a **local copy** (`vite` dev or `vite preview` on the built
   bundle) with simulated states injected into `app.status`, never by acting
   on the real pump: a run may be live.
5. Done means `npm run check` clean, `npm test`, `npm run build`, deployed
   when no run is active (see `CLAUDE.md`), `npm run smoke`.

## 2. The governing rule: colour only when it needs an eye

Normal operation is **quiet**. A colour, a tint, a moving thing must mean
"look here". Green everywhere attracted the eye for nothing and was removed.

| State | Tone | Band / bar | Title, word ink | Picture (`StateIcon`) |
|---|---|---|---|---|
| Feeding normally | `ok` | **white** (plain panel) | `--ink`, word `--green-ink` | pump head, rotor turning the run's real way (cw/ccw) |
| Holding (curve done) | `ok` | white | as above | pump head |
| Refilling | `fill` | `--fill-bg` blue | `--fill-ink` | bottle filling (level rises, bubbles) |
| Settling (bottle full) | `fill` | blue | `--fill-ink` | full bottle, bubbles only |
| Refill stopped / not filling | `warn` | `--warn-bg` orange | `--warn-ink` | bottle with "!" |
| Alarm | `bad` | `--danger-bg` red | `--danger-ink` | equilateral triangle, centred on its corners |
| No news from the daemon | `idle` | white | muted | grey dot, frozen |

- The state logic is one pure function, `runState()` in
  `ui/src/lib/runview.js` (tone, word, icon, title, detail, check). New states
  go there, with a test.
- Pictures move **only while something happens**, and freeze when the daemon
  is silent: a frozen page must look frozen. Respect `prefers-reduced-motion`.
- One moving pump per screen. A second one beside the feed rate was removed.
- The state is shown in **one place per page**: the Overview band; on other
  pages a thin bar above the tabs. **Never a pill inside the nav menu** (it
  read as a menu entry).
- Window title and tab icon follow the state ("Refilling 42 % · Fermentool");
  the icon keeps its usual colour when feeding normally.

## 3. Palette

| Token | Light | Role |
|---|---|---|
| `--teal-700` | `#00707F` | Primary: one primary button per view, active nav, brand, links |
| `--teal-400` | `#5FA4B0` | Requested curve line, secondary accent |
| `--lime-300` | `#A4DDC7` | Fill only (area under the curve), never text |
| `--green-500` | `#10A074` | "Live": weighed trace, the turning pump, success |
| `--fill` | `#1F8FB0` | Refill: icons, progress bar, chart refill bands |
| `--measured` | `#E07800` | Weighed (measured) feed, warnings |
| `--danger` | `#D64545` | Stop / abort / alarm |
| `--bg` / `--surface` | `#E8EEEC` / `#FFFFFF` | Page / cards |
| `--ink` / `--muted` | `#12292E` / `#56686C` | Text / secondary text |

**Text on a tint uses its `*-ink` token**, never the tint's own colour:
`--danger-ink`, `--green-ink`, `--teal-ink`, `--warn-ink`, `--fill-ink`, and
`--on-primary` on a filled teal button (white in light, dark ink in dark
mode, where the teal is light). Every text pair is **WCAG AA, 4.5:1 or more,
in both themes**; check with axe after a colour change (it went from 67
failures to none). Every colour is a token defined in all three theme blocks
of `app.css`; no literal colour in a component.

## 4. Type

- **IBM Plex Sans** for the UI: no 1/l/I or 0/O confusion, made for technical
  interfaces. Compared on mockups with Manrope, Barlow, Archivo, Recursive.
- **IBM Plex Mono** for numbers, units, state words, table data.
- **Barlow 600** (`--font-title`) for **state titles only** (Overview band,
  the bar over other pages): DIN-like, read from across the lab. Narrower, so
  a size up (22 px) to read as large as Plex.
- All fonts are **bundled** (`@fontsource`, imported in `main.js`): the lab PC
  may be offline. No font from a CDN.

## 5. Charts

- The value axis **starts at 0** and ends at a round value just above the
  data, with **labelled gridlines** (`ui/src/lib/chartscale.js`). Never fitted
  to the data's min and max: a 1 % wobble filled the whole height (run 186).
- Requested curve: `--teal-400` line over a faint `--lime-300` area. Weighed /
  actual trace: `--green-500`. Refills: `--fill` bands behind the curves.
- Endpoint labels at the curve's start and end; a tick label that would sit
  on them is hidden.

## 6. Numbers and words

- **UI strings in English**; dates and times in English too, whatever the PC
  language (`LOCALE = 'en-GB'` in `fmt.js`: "Mon 19 Oct, 11:04", 24 h).
- **Say each figure once per screen.** The band did not repeat the flow
  shown by the big figure (two different values, 8.043 and 8.330, confused).
- Say what a figure means: "100.0 % delivered · +0.0 % vs asked", never a bare
  "0.0 %" (read as "nothing delivered"). "49 m of 8 h", not a planned "8 h"
  for a run stopped after 2 minutes.
- No task where there is none: "Next refill around 15:04", not "Next thing
  for you" with nothing to do.
- An estimated figure is labelled as such ("estimated while refilling").
- **No em dash, and no spaced hyphen as one**: commas, colons, periods.
- Red is for what went wrong: a "stopped" run is neutral grey, "aborted" red.
  Destructive actions that are rare and final (Reset history) are a quiet
  link out of the main action's place, not a red button at the top.
- Form fields turn orange once left empty, not before anything is typed.

## 7. Form and layout

- Radius 14 px cards, 9 px controls; hairline `--line` border, a barely
  visible shadow. Cards set off by role, not every block.
- Spacing scale 4 / 8 / 12 / 16 / 24 / 32 / 48 (`--s-1` … `--s-7`).
- Rail (200 px) + fluid content; collapses to a top strip under 720 px.
- The big feed rate is the largest thing on the Overview.
- Focus always visible: 2 px outline, 2 px offset. Screen readers get a page
  `h1` (visually hidden, `.sr-only`).
- Text that can wrap keeps its units with it (`white-space: nowrap` on each
  part): never a lone "g" or "min" on a line.
