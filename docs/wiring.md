# Wiring

> Stub, expanded during hardware bring-up (milestone 10).

```
┌────────┐   USB    ┌───────────────────────────┐   RS485 (twisted pair)   ┌───────────┐
│  PC    │─────────▶│ isolated FTDI USB↔RS485    │─────────────────────────▶│ LabQ pump │
│        │◀─────────│ adapter (auto direction)  │◀─────────────────────────│           │
└────────┘          └───────────────────────────┘   A(+) · B(−) · GND      └───────────┘
```

## Adapter

- Isolated, **FT232-based**, automatic TX/RX direction control.
  Examples: Waveshare "USB TO RS485 (isolated)", DSD TECH SH-U12, FTDI USB-RS485-WE cable.
- Galvanic isolation is intended, a wet lab bench over a 100 h run.

## RS485 bus

- `A(+)` → pump `A+`, `B(−)` → pump `B−`.
- Use one twisted pair for A/B. Keep the stub short.
- 120 Ω termination only if the run is long / noisy; usually unnecessary at bench length.
- Do **not** cross A/B. If there is no traffic, swapping A/B is the first thing to try.

### Pump-side connector (15-hole D-sub, "External Control Interface")

Per the LabQ manual ("8. External Control Interface"), the RS485 pins on the
pump's DB15 are:

- **Pin 5 = RS485 A+**
- **Pin 4 = RS485 B−**

There is no dedicated RS485 GND on this connector (only AGND, pin 9, for the
analog speed input, and POWER_GND, pin 11, for the motor-status output;
neither is a general signal ground). With the isolated adapter this project
uses, no common ground reference is needed for a short bench run.

If the adapter cable exposes separate `TXD+/TXD-/RXD+/RXD-` (RS422-capable
chip used half-duplex) instead of a single `A/B` pair, bridge `TXD+` with
`RXD+` and wire that joined pair to pin 5, and bridge `TXD-` with `RXD-` and
wire that joined pair to pin 4. This bridging is safe: TX is a driver output,
RX is a high-impedance input, so tying them together does not create bus
contention.

All other DB15 pins are unrelated to MODBUS and must not be wired to the
adapter: pin1/2 are external start/stop and reversing inputs, pins 6-9 are
analog speed-control inputs (0-5V/0-10V/4-20mA, **do not tie these together,
it damages the pump**), pins 11-13 are the motor-running status output, and
pins 14/15 are an isolated 5V supply.

## Pump serial settings

- Protocol: MODBUS-RTU, **8E1**, baud 9600 (configurable on the pump).
- Slave address: 1 (must be unique on the bus).
- Commands are only accepted on the pump's **Main Interface** screen.

## Sanity check

`cargo run -p fermentool-core -- probe` (from milestone 3) reads the speed/status
registers and prints them.
