# Pump models: LabQ, LabV, PDS II

Date: 2026-10-23. Status: design, awaiting the user's approval.
Mockup (approved look): https://claude.ai/artifact/JhLH9s8ue5mjEUBiJsspGf (v3).

## Why

Fermentool only speaks to the Shenchen **LabQ**. Some reactors on the bench
have a **LabV** (with a 12-channel AMC head) or a **PDS II** (Innofluid
"Programmable Dispensing System": one controller, up to 8 pump heads) instead.
Each reactor has its own PC and its own Fermentool, with one RS-485 cable. The
user wants to pick, in Settings > Pumps, which model the feed pump is and which
model the refill pump is, and then run exactly as with a LabQ.

## Goals

- In Settings > Pumps, each of the two pumps (feed, refill) gets a **model**:
  LabQ, LabV or PDS II. Any mix works: two LabQ (today), LabV feed with LabQ
  refill, two heads of one PDS II, a PDS II head with a LabQ, and so on.
- A PDS II pump also has a **head** number, 1 to 8. Feed and refill may share
  one PDS II address on two different heads.
- During a run, nothing changes for the operator: rpm or ml/min curves, the
  gravimetric trim, automatic refill, alarms, the tubing calibration, crash
  resume, notifications, all as with a LabQ.
- Each pump row shows a **photo** of the chosen model (approved in the
  mockup: LabQ, LabV1 with AMC head, PDS II controller with its heads; for a
  PDS II a small "Head n" badge on the photo).
- A config from today (no model in it) keeps working as LabQ + LabQ, with
  nothing to touch.

## Non-goals

- Several reactors on one PC, or one port per pump. One cable, one port.
- The pumps' dispensing, fixed-volume, timed or calibration features
  (LabV 1015..1023, 2001..2006; PDS II n008..n028, n100..n107).
- Any other Shenchen / Innofluid model.

## Anti-goals (a failure even if it works)

- Fermentool writing a pump's **head or tubing** registers, for any model
  (LabQ 1000/1001, LabV 1000/1001, PDS II n000/n001). The existing rule holds.
- Fermentool writing the PDS II **all start/stop register (0010)**: it starts
  or stops every head of the controller, possibly another experiment's.
- Writing the LabQ register map to a LabV: LabQ Start/Stop is 1006, which
  on a LabV is not Start/Stop (LabV Start/Stop is 1008, LabV Full Speed is
  1010). A wrong map must be impossible by construction, not by care.
- A LabQ setup that behaves differently after the update.

## The protocols

Sources (copy both into `docs/` with the LabQ one):
- LabV: "Lab V Series MODBUS Communication Protocol", Shenchen 2015,
  https://darwin-microfluidics.com/content/pdfs/Lab-V-Series-MODBUS-Protocol.pdf
- PDS II: "PDS Series MODBUS Protocol", Innofluid (the user's copy,
  `Downloads/PDS Series  MODBUS Protocol.md`).

All three: MODBUS RTU, CRC-16 0xA001, big-endian words, floats as two
big-endian registers, functions 03 / 06 / 10, exceptions 01 / 02 / 03 / 06,
commands accepted only while the pump is on its main screen.

| Function | LabQ (today) | LabV | PDS II, head n (1..8) |
|---|---|---|---|
| Speed, rpm, f32 | 1002 | 1002 (0.1..600) | n032 (0.1..600) |
| Flow, ml/min, f32 | 1004 | 1004 | n030 |
| Start/Stop, u16 | 1006 | 1008 | n003 |
| Direction, u16 (1 = cw) | 1007 | 1009 | n004 |
| Operating mode, u16 | none | 1020 (0 = transferring) | n005 (0 = transferring) |
| Addresses | 1..247 | 1..32 | 1..32 |

PDS II register = n × 1000 + offset. PDS II notes that matter: speed and flow
can only be written in transferring mode; direction only in transferring mode
or stopped; a head set to "logical operation" ignores MODBUS, only
"independent operation" answers. LabV: in "fixed time and volume" mode, 1002
and 1004 are ignored.

## Decided with the user

- **Model per pump**, feed and refill separately, any mix (Settings > Pumps).
- **Photos** of the real products in each row, LabV shown with the AMC
  12-channel head (mockup v3).
- **PDS II and LabV not in transferring mode: Fermentool switches them to
  transferring itself** before driving them (LabV 1020, PDS II n005).
- **Maximum speed is set per pump**: a "Max rpm" field for each pump in
  Settings > Pumps, prefilled from the model when it is chosen (LabQ 350,
  LabV 150, PDS II 600), corrected by hand when a pump differs (the LabV1 is
  sold as 150 rpm, the protocol says 600, PDS II heads vary).
- **A tubing calibration belongs to a pump model.** Each calibration records
  the model of the pump it was measured on; New run only offers the
  calibrations made on the feed pump's current model.
- **The cable: one setting, 9600 baud, 8 data bits, even parity, 1 stop**
  (8E1). The user asked for the most reliable and fastest engineer's answer.
  9600 is the fastest all three offer; one fixed setting means a pump at 4800
  cannot silently miss every frame. Every pump is set by hand to 9600 and a
  unique address. The LabQ's own screen shows "Communication 9600.8.e.1";
  the PDS II protocol states even parity; the LabV protocol says "1 parity
  bit" without saying which: check on the real LabV.
- **Same driving as the LabQ**: rpm, ml/min, trim, refill, alarms, calibration.

## The how

### fermentool-modbus

- A `PumpModel` enum (`LabQ`, `LabV`, `PdsII`) and a `RegisterMap` built
  only from `(model, head)`: speed, flow, start/stop, direction, and an
  optional operating-mode register with its "transferring" value. `head` only
  exists for `PdsII` (a type that cannot carry a head for the others, e.g.
  `PumpKind::PdsII { head: u8 }`).
- `Pump` addresses a **target** (`address` + `RegisterMap`) instead of a bare
  address. `with_address` becomes `with_target` (the refill pump still rides
  the feed pump's transport, on the control thread). The `PumpTransport`
  operations read their registers from the target's map. `set_head_type` /
  `set_tubing_size` stay LabQ-only test helpers or go; nothing in the daemon
  calls them.
- New `ensure_transferring()`: if the map has a mode register, **read** it
  and write the transferring value only if it differs. No write at all for a
  LabQ. If it must write, it stops that pump first (designer's call: whether
  either pump takes a mode write while running is unknown, and a pump in the
  wrong mode was not delivering the curve anyway). On a resume the pump is
  normally still in transferring mode, so nothing is written and the feed is
  not interrupted.
- Range checks: speed 0.1..the pump's configured max rpm, flow 0..99999.
- `SimPump` / `SimBus` simulate each model's map, including the PDS II
  heads behind one address, refusing a speed write outside transferring mode
  (exception 03), and ignoring a head in logical operation, so the engine
  tests can cover all three.

### Config (`config.toml`)

```toml
[pump]
address = 1
model = "labq"      # "labq" | "labv" | "pds2"; absent = "labq"
head = 1            # PDS II only, 1..8; ignored otherwise
max_rpm = 350       # absent = the model's default (LabQ 350, LabV 150, PDS II 600)

[refill]
pump_address = 2
pump_model = "labq"
pump_head = 1
pump_max_rpm = 350
```

- `max_rpm` is bounded 0.1..350 for a LabQ and 0.1..600 for LabV / PDS II.
  The refill `speed_rpm` must stay at or under `pump_max_rpm`.

- Validation: address 1..247 for a LabQ, 1..32 for LabV / PDS II. Feed and
  refill may share an address **only** when both are PDS II on different
  heads; today's "not the feed pump's address" check becomes "not the same
  pump". An invalid value is reset to its default with a note, as today.
- `serial.baud` stays in `config.toml` (an odd bench can still set it by
  hand), but the UI no longer offers it: it shows the cable as
  "9600 baud · 8E1".
- Model, head and address of the feed pump are refused mid-run (like the
  feed address today). The refill pump's model and head too while a run uses
  automatic refill.

### Engine and daemon

- `limits::RPM_MAX` becomes the pump's configured `max_rpm` wherever it bounds a
  setpoint, a curve clamp, the "driven in rpm" refusal (`drive_ml_per_rpm`:
  "Refused if the curve needs more than N rpm"), and the refill speed
  (`REFILL_RPM_MAX`). Messages say the pump's figure.
- `ensure_transferring()` runs before the pump is first driven: run start,
  resume, a refill fill's start, the refill Test. Not on the 150 ms setpoint
  path. A refusal is a structured `EngineError` with a `hint()` naming the
  probable cause per model (PDS II head in logical operation, pump not on its
  main screen).
- `refill_ping` reads the refill pump's speed register from its map, as now.
- `/api/status` reports each pump's model (and head), so the UI and the
  smoke test can show it.
- The run records its feed pump's model and head (a migration adding two
  columns to `runs`), shown in the run detail's Settings fold: a run on a
  LabV must stay recognisable in History.

### Tubing calibrations

- Migration: `tubing_calibrations.pump_model` (`labq` / `labv` / `pds2`),
  the feed pump's model when the calibration was measured. Existing rows get
  `labq` (all of them were measured on the LabQ).
- The calibration API returns it; the Tubing calibration page shows it on
  each calibration.
- New run lists only the calibrations whose model is the feed pump's
  current model. `POST /api/runs` refuses a calibration of another model
  (a structured `EngineError` with a `hint()`: "this tube was calibrated on a
  LabQ, the feed pump is a LabV"), so a stale draft or the API cannot get
  around the filter.
- Per model, not per head: two PDS II heads of the same type share
  calibrations (designer's call, matches "assignées à un type de pompe").

### UI (Settings > Pumps, as mockup v3)

- Per pump row: the photo (bundled under `ui/src/assets/pumps/`, JPEG, a
  few kB each, white tile in both themes), the name, a LabQ / LabV / PDS II
  segmented choice, Address, Head (1..8) only for a PDS II, and Max rpm
  (refilled with the model's default when the model changes; not in the
  mockup, placed beside Address).
- The baud select goes; the cable line shows "9600 baud · 8E1".
- "Not answering" hint per model: LabQ / LabV "switched on, at address N
  and 9600 baud in its own menu?"; PDS II adds "head n set to independent
  operation".
- The refill speed field's max follows the refill pump's model.
- Logic (address clash rule, hint text, max rpm per model) lives in
  `ui/src/lib/pumps.js` with vitest tests; the photos are display only.
- UI words in English; no "burst"; no em dash.

### Tests

- modbus: each model's frames for speed, flow, start, stop, direction, mode,
  built from the tables above; PDS II head 3 hits 3032 / 3003; nothing ever
  builds a frame for 0010 or a head/tubing register.
- engine: a run, a resume and a refill on a simulated LabV and on two heads
  of a simulated PDS II; a PDS II or LabV out of transferring mode is
  switched once, a resume in transferring mode writes nothing; a curve over
  the pump's max rpm is refused; a calibration of another model is refused.
- config: an old `config.toml` loads as LabQ + LabQ; the address clash rule.
- UI: vitest for `pumps.js`; smoke: the model choice and the Head field appear
  and switch (changes nothing saved).
- Hardware, before calling it done (the user, on a real LabV and PDS II):
  ping, speed, start / stop, direction, the mode switch, a refused setpoint
  over the max, and the LabV's parity.

### Docs

- Copy the LabV and PDS II protocols into `docs/`; `docs/wiring.md`: the
  LabV DB9 (pin 7 GND1 = RS-485 signal ground, 8 = B, 9 = A), PDS II wiring
  once known.
- `CLAUDE.md`: the rule "never write head/tubing registers" and the 0010 ban
  for all models; the per-model maximum.
- `CHANGELOG.md`.

## Left to the builder

Anything above not marked "decided with the user" is the designer's call,
open to correction while reading: notably the stop before a mode write, the
calibration being per model rather than per head, the extra `runs` columns,
and keeping `serial.baud` in `config.toml`.
