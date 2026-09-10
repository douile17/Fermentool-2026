// Single source of truth for "how is the pump link doing?", shared by the
// top ConnBar, the inline connect-gate, and the sidebar foot indicator.
//
// tone: 'ok' | 'warn' | 'bad' | 'sim' | 'idle'
//   ok   → healthy real port, pump responding        (green, bar hidden)
//   warn → link degrading / pump not tracking        (amber, bar shown)
//   bad  → no port / port open but pump silent        (red, bar shown)
//   sim  → running the simulator                      (grey, bar shown)
//   idle → daemon not reachable yet                   (grey, bar shown)
//
// connectable: true  → picking a port and clicking Connect can fix this, so the
//                      bar shows the full ConnectControl.
//              false → it's a pump / disk / daemon condition a reconnect won't
//                      touch, the bar shows just the status line.
export function linkState(status, connected) {
  const s = status;
  if (!connected || !s)
    return { tone: 'idle', label: 'Connecting to daemon…', short: 'daemon…', connectable: false };
  if (s.simulator && s.allow_simulator)
    return {
      tone: 'sim',
      label: 'Simulator, test runs enabled',
      short: 'simulator',
      connectable: true,
    };
  if (s.simulator)
    return {
      tone: 'bad',
      label: 'No pump connected',
      short: 'no pump',
      hint: 'Pick a serial port and click Connect (or enable simulator runs in Settings).',
      connectable: true,
    };
  if (s.serial_ok === false)
    return {
      tone: 'bad',
      label: 'Pump not connected',
      short: 'pump offline',
      hint: 'Check power, wiring, the MODBUS address and the baud rate, then Connect.',
      connectable: true,
    };
  if (s.pump_confirmed === false)
    return {
      tone: 'warn',
      label: 'Pump not tracking the setpoint',
      short: 'pump: mismatch',
      hint: 'Writes get through but the pump reports a different value. Check the pump.',
      connectable: false,
    };
  if (s.journal_ok === false)
    return {
      tone: 'warn',
      label: 'Journal stalled, run history is not being saved',
      short: 'journal stalled',
      hint: 'The pump is still running correctly, but tick logging is failing. Check free disk space.',
      connectable: false,
    };
  if ((s.write_fails ?? 0) >= 3)
    return {
      tone: 'warn',
      label: `Link degrading, ${s.write_fails} writes failed`,
      short: 'link degrading',
      connectable: true,
    };
  return {
    tone: 'ok',
    label: `Pump connected · ${s.transport}`,
    short: `pump · ${s.transport}`,
    connectable: true,
  };
}

// Can a run be started / resumed right now? A run drives the pump, so it needs
// a live link, a real port that's responding, or the simulator with simulator
// runs enabled. Keep every "can't start" gate keyed off this one function.
export function canStartRun(status) {
  if (!status) return false;
  if (status.serial_ok === false) return false;
  if (status.simulator === true && status.allow_simulator !== true) return false;
  return true;
}
