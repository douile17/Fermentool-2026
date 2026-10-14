// The refill pump: a second LabQ on the feed pump's cable, at its own MODBUS
// address, that tops the weighed bottle up from the reservoir. Settings
// (Refill pump) and the sidebar both describe it through these.

export const REFILL_RPM_MIN = 0.1;
export const REFILL_RPM_MAX = 350;

/** What the daemon would refuse in `[refill]`, as a sentence; null if fine.
 *  Mirrors `RefillConfig::problem` (config.rs). */
export function refillProblem({ address, feedAddress, speedRpm }) {
  const a = Number(address);
  if (!Number.isInteger(a) || a < 1 || a > 247) return "The refill pump's MODBUS address must be 1 to 247.";
  if (a === Number(feedAddress))
    return `Address ${a} is the feed pump's: set another address in the refill pump's own menu, then here.`;
  const s = Number(speedRpm);
  if (!(s >= REFILL_RPM_MIN && s <= REFILL_RPM_MAX))
    return `The speed must be between ${REFILL_RPM_MIN} and ${REFILL_RPM_MAX} rpm.`;
  return null;
}

/** New run asks how the bottle is refilled only with the balance
 *  regulation on a balance under the feed bottle (the daemon refuses the
 *  refill pump otherwise). */
export const refillAsked = ({ trimOn, scaleOnFeed }) => !!trimOn && !!scaleOnFeed;

/** Whether a run drives the refill pump: `wanted` is New run's choice,
 *  true (refill pump), false (by hand) or null (not chosen yet). */
export const refillOn = ({ wanted, trimOn, scaleOnFeed }) => wanted === true && refillAsked({ trimOn, scaleOnFeed });

/** The choice is asked and not made yet: like the pump direction, nothing
 *  is picked for the operator. */
export const refillToChoose = ({ wanted, trimOn, scaleOnFeed }) =>
  refillAsked({ trimOn, scaleOnFeed }) && wanted !== true && wanted !== false;

// cw / ccw, as New run and the tubing calibration write it.
export const directionLabel = (d) => (d === 'ccw' ? 'ccw' : 'cw');

/** The refill pump's line in the sidebar and in Settings, from the status
 *  frame's `refill` (null while no run drives it and nothing is owed). */
export function refillView(refill) {
  if (!refill) return null;
  const addr = `address ${refill.address}`;
  if (refill.testing)
    return { tone: 'warn', label: 'Refill pump: testing', title: `The refill pump (${addr}) runs for a 3 s test.` };
  if (refill.responding === false)
    return refill.in_run
      ? {
          tone: 'bad',
          label: 'Refill pump: not answering',
          title: `The refill pump (${addr}) does not answer. Check its power, its RS-485 wiring and the MODBUS address set in its own menu. Fermentool asks again every minute.`,
        }
      : {
          tone: 'bad',
          label: 'Refill pump: Stop not confirmed',
          title: `The last Stop did not reach the refill pump (${addr}): it may still be running. Fermentool sends it again; stop it by hand if needed.`,
        };
  if (refill.responding == null)
    return { tone: 'idle', label: 'Refill pump: asking…', title: `Fermentool is asking the refill pump (${addr}).` };
  return {
    tone: 'ok',
    label: 'Refill pump: ready',
    title: `The refill pump (${addr}) answers, stopped. Fermentool sends it a Stop every 10 s.`,
  };
}
