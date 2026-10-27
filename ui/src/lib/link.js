// Single source of truth for "how is the pump link doing?", shared by the
// slim line at the top (ConnBar), Settings > Pumps, and the sidebar foot.
//
// tone: 'ok' | 'warn' | 'bad' | 'sim' | 'idle'
//   ok   → healthy real port, pump responding        (green, bar hidden)
//   warn → link degrading / pump not tracking        (amber, bar shown)
//   bad  → no port / port open but pump silent        (red, bar shown)
//   sim  → running the simulator                      (grey, bar shown)
//   idle → daemon not reachable yet                   (grey, bar shown)
//
// connectable: true  → the port settings can fix this: the line at the top
//                      links to Settings > Pumps.
//              false → it's a pump / disk / daemon condition a reconnect won't
//                      touch, the line only tells.
export function linkState(status, connected) {
  const s = status;
  if (!s) return { tone: 'idle', label: 'Connecting to daemon…', short: 'Daemon…', connectable: false };
  // It was there, and is not any more: what the page shows is from before.
  if (!connected)
    return {
      tone: 'bad',
      label: 'Daemon not responding: what is shown here may be out of date',
      short: 'Daemon offline',
      hint: 'Fermentool keeps trying to reach it. If it does not come back, start Fermentool again; a run that was going resumes from where its curve is.',
      connectable: false,
    };
  // Before anything else: the pump may be feeding with no run behind it.
  if (s.stop_pending)
    return {
      tone: 'bad',
      label: 'Stop not confirmed: the pump may still be running',
      short: 'Stop pending',
      hint: 'The Stop did not reach the pump. Fermentool sends it again as soon as the pump answers; check the pump, and stop it by hand if needed.',
      connectable: true,
    };
  if (s.simulator && s.allow_simulator)
    return {
      tone: 'sim',
      label: 'Simulator, test runs enabled',
      short: 'Simulator',
      connectable: true,
    };
  if (s.simulator)
    return {
      tone: 'bad',
      label: 'No pump connected',
      short: 'No pump',
      hint: "Pick the cable's port in Settings, Pumps, or allow simulator runs there.",
      connectable: true,
    };
  // Two pumps share the cable, and one switched off silences the other too
  // (seen on the bench, 2026-10-23: its unpowered transceiver holds the
  // line): a silent bus cannot say which pump went, so it blames neither.
  // The refill pump alone is told apart only while the feed pump answers.
  const refillSilent = s.refill?.responding === false;
  if (s.serial_ok === false) {
    if (s.refill)
      return {
        tone: 'bad',
        label: 'Pumps not answering',
        short: 'Pumps offline',
        hint: `Both pumps share the cable (addresses ${addr(s.pump_address)} and ${addr(s.refill.address)} on ${portOf(s)}): one switched off silences the other. Check that both are on, then Connect in Settings, Pumps.`,
        connectable: true,
      };
    return {
      tone: 'bad',
      label: 'Pump not answering',
      short: 'Pump offline',
      hint: `Address ${addr(s.pump_address)} on ${portOf(s)}. Check its power and its cable, then Connect in Settings, Pumps.`,
      connectable: true,
    };
  }
  if (s.pump_confirmed === false)
    return {
      tone: 'warn',
      label: 'Pump not tracking the setpoint',
      short: 'Pump: mismatch',
      hint: 'Writes get through but the pump reports a different value. Check the pump.',
      connectable: false,
    };
  // The run feeds on: orange, not red. A refill due that cannot start is
  // the Overview band's to say.
  if (refillSilent && s.refill.in_run)
    return {
      tone: 'warn',
      label: 'Refill pump not answering',
      short: 'Refill pump offline',
      hint: `Address ${addr(s.refill.address)} on ${portOf(s)}. Feeding goes on; the bottle cannot be refilled until it answers.`,
      connectable: true,
    };
  if (s.warnings?.length)
    return {
      tone: 'warn',
      label: s.warnings[0],
      short: 'Startup check',
      hint: s.warnings.length > 1 ? s.warnings.slice(1).join(' | ') : undefined,
      connectable: false,
    };
  if (s.journal_ok === false)
    return {
      tone: 'warn',
      label: 'Journal stalled, run history is not being saved',
      short: 'Journal stalled',
      hint: 'The pump is still running correctly, but tick logging is failing. Check free disk space.',
      connectable: false,
    };
  if ((s.write_fails ?? 0) >= 3)
    return {
      tone: 'warn',
      label: `Link degrading, ${s.write_fails} writes failed`,
      short: 'Link degrading',
      connectable: true,
    };
  return {
    tone: 'ok',
    label: `Pump connected | ${s.transport}`,
    short: `Pump: ${s.transport}`,
    connectable: true,
  };
}

const addr = (a) => a ?? '?';
const portOf = (s) => s.serial_port || s.transport || 'the cable';

// Can a run be started / resumed right now? A run drives the pump, so it needs
// a live link, a real port that's responding, or the simulator with simulator
// runs enabled. Keep every "can't start" gate keyed off this one function.
export function canStartRun(status) {
  if (!status) return false;
  if (status.serial_ok === false) return false;
  if (status.simulator === true && status.allow_simulator !== true) return false;
  return true;
}
