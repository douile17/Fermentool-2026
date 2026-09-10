# Wiring

> Stub - expanded during hardware bring-up (milestone 10).

```
┌────────┐   USB    ┌───────────────────────────┐   RS485 (twisted pair)   ┌───────────┐
│  PC    │─────────▶│ isolated FTDI USB↔RS485    │─────────────────────────▶│ LabQ pump │
│        │◀─────────│ adapter (auto direction)  │◀─────────────────────────│           │
└────────┘          └───────────────────────────┘   A(+) · B(−) · GND      └───────────┘
```

## Adapter

- Isolated, **FT232-based**, automatic TX/RX direction control.
  Examples: Waveshare "USB TO RS485 (isolated)", DSD TECH SH-U12, FTDI USB-RS485-WE cable.
- Galvanic isolation is intended - a wet lab bench over a 100 h run.

## RS485 bus

- `A(+)` → pump `A+`, `B(−)` → pump `B−`, `GND` → pump `GND` (common reference).
- Use one twisted pair for A/B. Keep the stub short.
- 120 Ω termination only if the run is long / noisy; usually unnecessary at bench length.
- Do **not** cross A/B. If there is no traffic, swapping A/B is the first thing to try.

## Pump serial settings

- Protocol: MODBUS-RTU, **8E1**, baud 9600 (configurable on the pump).
- Slave address: 1 (must be unique on the bus).
- Commands are only accepted on the pump's **Main Interface** screen.

## Sanity check

`cargo run -p fermentool-core -- probe` (from milestone 3) reads the speed/status
registers and prints them.
