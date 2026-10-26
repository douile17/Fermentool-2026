// The refill pump: a second pump on the feed pump's cable (a LabQ, a LabV or
// a PDS II head), at its own MODBUS address or head, that tops the weighed
// bottle up from the reservoir. Settings
// (Refill pump) and the sidebar both describe it through these.
import { modelOf, pumpProblem, samePump } from './pumps.js';

export const REFILL_RPM_MIN = 0.1;

/** What the daemon would refuse in `[refill]`, as a sentence; null if fine.
 *  `feed` is the feed pump, `{ address, model, head }`; `maxRpm` the refill
 *  pump's own top speed. Mirrors `RefillConfig::problem` (config.rs) and the
 *  run-time clash check. */
export function refillProblem({ address, model = 'labq', head = 1, speedRpm, maxRpm, feed }) {
  const m = modelOf(model);
  const a = Number(address);
  if (!Number.isInteger(a) || a < 1 || a > m.maxAddress) return `The refill pump's MODBUS address must be 1 to ${m.maxAddress}.`;
  if (m.id === 'pds2') {
    const h = Number(head);
    if (!Number.isInteger(h) || h < 1 || h > 8) return "The refill pump's PDS II head must be 1 to 8.";
  }
  if (feed && samePump({ address: a, model: m.id, head }, feed))
    return m.id === 'pds2' && feed.model === 'pds2'
      ? `Head ${Number(head)} of address ${a} is the feed pump's: pick another head.`
      : `Address ${a} is the feed pump's: set another address in the refill pump's own menu, then here.`;
  return refillSpeedProblem(speedRpm, maxRpm ?? m.maxRpm);
}

/** The refill speed against the refill pump's top speed `max`; null if fine. */
export function refillSpeedProblem(speedRpm, max) {
  const s = Number(speedRpm);
  if (!(s >= REFILL_RPM_MIN && s <= max)) return `The speed must be between ${REFILL_RPM_MIN} and ${max} rpm (the refill pump's max).`;
  return null;
}

const lower = (t) => t.charAt(0).toLowerCase() + t.slice(1);

/** Settings > Pumps' check before Save, one sentence at a time: the feed
 *  pump, then the refill pump's own address, head and max rpm (a max of 0
 *  says so, not "between 0.1 and 0 rpm"), then the clash and its speed.
 *  `{ message, speed }`, `speed` when the refill speed is what is wrong (its
 *  field is in the Change panel, which then opens), or null. `feed` and
 *  `refill` are `{ address, model, head, maxRpm }`. */
export function pumpsProblem({ feed, refill, speedRpm }) {
  const f = pumpProblem(feed);
  if (f) return { message: `Feed pump: ${lower(f)}`, speed: false };
  const m = pumpProblem(refill);
  if (m) return { message: `Refill pump: ${lower(m)}`, speed: false };
  const r = refillProblem({ ...refill, speedRpm, feed: { address: feed.address, model: feed.model, head: feed.head } });
  if (r) return { message: r, speed: r === refillSpeedProblem(speedRpm, refill.maxRpm ?? modelOf(refill.model).maxRpm) };
  return null;
}

/** A refusal from the daemon about the refill speed (its field is in the
 *  Change panel). */
export const aboutRefillSpeed = (message) => /refill pump's speed|^the speed/i.test(message ?? '');

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
  const c = refill.cycle;
  if (c?.phase === 'stopping')
    return { tone: 'bad', label: 'Refill pump: Stop not confirmed', title: `The refill pump (${addr}) did not acknowledge Stop: it may still be filling. Stop it by hand.` };
  if (c?.halted)
    return { tone: 'bad', label: `Refill: ${cycleLine(c)}`, title: 'Automatic refill stopped (an alarm, or Stop refill). Overview, Refill now, starts it again.' };
  if (c?.phase === 'filling' && c.not_rising)
    return { tone: 'bad', label: 'Refill: the bottle is not filling', title: 'The refill pump keeps going, in case the line is only priming: check the reservoir, the line and the pump.' };
  if (c?.phase === 'filling')
    return { tone: 'warn', label: `Refill: ${cycleLine(c)}`, title: 'The refill pump is filling the weighed bottle; the regulation holds its correction meanwhile.' };
  if (c) return { tone: 'ok', label: `Refill: ${cycleLine(c)}`, title: `The refill pump (${addr}) answers, stopped until the bottle falls under its refill level.` };
  return {
    tone: 'ok',
    label: 'Refill pump: ready',
    title: `The refill pump (${addr}) answers, stopped. Fermentool sends it a Stop every 10 s.`,
  };
}

/** Whether run time `t` falls in one of the report's estimated stretches
 *  (`[from_s, to_s]`: a refill, the bottle touched). */
export const inSpans = (t, spans) => (spans ?? []).some(([a, b]) => t >= a && t <= b);

/** A chart series cut where it enters and leaves the estimated stretches:
 *  `[{ est, pts }]` in order, each piece sharing its first point with the
 *  end of the one before, so the drawn line stays unbroken. */
export function splitEstimated(pts, spans) {
  const out = [];
  for (const p of pts) {
    const est = inSpans(p[0], spans);
    const cur = out.at(-1);
    if (cur && cur.est === est) cur.pts.push(p);
    else out.push({ est, pts: cur ? [cur.pts.at(-1), p] : [p] });
  }
  return out;
}

// --- the automatic refill cycle (mirrors engine/refill_cycle.rs) -----------

/** A run must use at least this much before a refill. */
export const MIN_SPAN_G = 100;

export const kg = (g) => `${(g / 1000).toFixed(2)} kg`;

/** The levels a run would refill between: once the bottle has given
 *  `drawG` (New run, "Refill after using"), it is topped back up to the
 *  weight on the balance at the start, the full bottle. `{ lowG, highG }`
 *  or `{ error }`. Mirrors `plan_for` (refill_cycle.rs), which has the last
 *  word. */
export function refillPlan({ drawG, weightG }) {
  if (!(drawG > 0)) return { error: 'Enter how much the bottle gives before a refill.' };
  if (drawG < MIN_SPAN_G) return { error: `Refill after using at least ${MIN_SPAN_G} g.` };
  if (weightG == null) return { error: 'No balance reading.' };
  if (weightG <= drawG) return { error: `The balance reads ${kg(weightG)}, less than the ${kg(drawG)} to use before a refill: fill the bottle, or use less.` };
  return { lowG: weightG - drawG, highG: weightG };
}

/** The cycle in a few words, for the sidebar and Overview. */
export function cycleLine(cycle) {
  if (!cycle) return null;
  const v = cycle.weight_g != null ? kg(cycle.weight_g) : '?';
  if (cycle.phase === 'filling') return `filling ${v} → ${kg(cycle.high_g)}`;
  if (cycle.phase === 'stopping') return 'Stop not confirmed';
  if (cycle.halted) return `stopped, ${v}`;
  if (cycle.phase === 'settling') return `${v}, settling`;
  return `${v}, refills under ${kg(cycle.low_g)}`;
}
