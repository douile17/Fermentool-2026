// A calibration burst whose balance does not follow the pump: the bottle is
// off the balance, the line is out of the liquid or off the pump head, the
// pump is not drawing, or the liquid runs back. Said while the burst runs,
// not at its end, but never early enough to mistake a slow line for a dead
// one: every check waits until even half the expected flow would have moved
// the balance well past its noise.

/** Never before this: the pump spins up, the line primes. */
export const STALL_GRACE_S = 30;
/** A check this close to the burst's end is left to the end-of-burst one. */
const END_MARGIN_S = 10;
/** Flow per rpm of a tube, roughly k x inner diameter squared (ml/min, mm):
 *  0.8 mm draws ~0.041, 1.6 mm ~0.164 on the LabQ. */
const ML_PER_RPM_PER_MM2 = 0.064;

/** Grams that count as "the balance moved": 3 steps, and at least 0.2 g. */
export const movedG = (resG) => Math.max(3 * (resG > 0 ? resG : 0.1), 0.2);

/** The wrong way by this many times `movedG` is the pump, not a hand on the
 *  bench: 0.6 g on a 0.1 g balance, a few seconds of a slow line. */
const BACKWARDS = 3;

/** The way the weight goes as a burst pumps, from `[scale] position`: -1
 *  under the feed bottle, +1 under the collecting beaker, null unknown. */
export const weighDir = (position) => (position === 'feed' ? -1 : position === 'receiver' ? 1 : null);

/** The grams a burst collected, from the balance: the change the right way
 *  for its side (negative when it went the wrong way), either way when the
 *  side is unknown. Null without a reading. */
export function collectedG(w, startG, dir) {
  if (w == null || startG == null) return null;
  return dir ? dir * (w - startG) : Math.abs(w - startG);
}

/**
 * The expected flow of a burst, in ml/min: the setpoint in ml/min, or in rpm
 * times an earlier calibration's ml per rpm, or times the bore's rough one.
 * `null` when nothing gives one.
 */
export function expectedMlMin({ controlVar, setpoint, mlPerRpm, innerMm }) {
  if (!(setpoint > 0)) return null;
  if (controlVar === 'ml_min') return setpoint;
  if (mlPerRpm > 0) return setpoint * mlPerRpm;
  if (innerMm > 0) return setpoint * ML_PER_RPM_PER_MM2 * innerMm * innerMm;
  return null;
}

/**
 * The time, in seconds, in which half the expected flow moves the balance by
 * 3x `movedG`: the first check comes then, and every later one looks back
 * over that long. `null` when even that falls too late in the burst (the
 * end-of-burst check covers it) or no flow is known.
 */
export function stallCheckAtS({ mlMin, densityGPerMl, resG, burstS }) {
  if (!(mlMin > 0) || !(densityGPerMl > 0) || !(burstS > 0)) return null;
  const slowGPerS = (0.5 * mlMin * densityGPerMl) / 60;
  const at = Math.max(STALL_GRACE_S, (3 * movedG(resG)) / slowGPerS);
  return at <= burstS - END_MARGIN_S ? at : null;
}

/**
 * What is wrong with a running burst, from its balance readings
 * (`samples`: [{ t: seconds since the burst started, w: grams }], oldest
 * first, the first one the reading just before it):
 * - 'still': the weight has not moved since the burst started;
 * - 'stopped': it moved, then stopped moving (nothing over the last window);
 * - 'reversed': it moved, then went back the other way (line pulled off the
 *   head, liquid running back, bottle lifted);
 * - 'slow': it moves the right way but far slower than the tube should give
 *   (line off the pump head and siphoning by gravity, a kinked or loose
 *   line): under 40 % of the expected flow when that flow is known from a
 *   real calibration or an ml/min setpoint (`trusted`), under a quarter when
 *   it is only the bore's rough figure;
 * - 'backwards': the weight went the wrong way for the balance's side (up
 *   under the feed bottle): the pump turns the wrong way and pushes the
 *   line's feed back. Said at once, not after the checks' wait.
 * `null` when all is well, or too early to say.
 * The direction is `dir` (`weighDir`): down under the feed bottle, up under
 * the collecting beaker; unknown (null), whatever the weight did first.
 */
export function burstTrouble({ samples, checkAtS, resG, mlMin = null, densityGPerMl = 1, trusted = false, dir: side = null }) {
  if (!samples?.length) return null;
  const last = samples[samples.length - 1];
  const moved = movedG(resG);
  const w0 = samples[0].w;
  // 27 Oct 2026: a pump turning the wrong way pushed the line back into the
  // bottle; the weight rose and read as flow until the line ran empty.
  if (side && side * (last.w - w0) <= -BACKWARDS * moved) return 'backwards';
  if (checkAtS == null || !(last.t >= checkAtS)) return null;
  // the way the weight went first, once it went clearly (the way it should,
  // when the balance's side is known)
  const first = samples.find((s) => (side ? side * (s.w - w0) : Math.abs(s.w - w0)) >= moved);
  if (!first) return 'still';
  const dir = side ?? Math.sign(first.w - w0);
  // the last window, from a reading at least checkAtS old (and after the
  // weight first moved, so the start-up itself does not count)
  const from = last.t - checkAtS;
  if (from < first.t) return null;
  let then = null;
  for (const s of samples) {
    if (s.t <= from) then = s;
    else break;
  }
  if (!then) return null;
  const progress = dir * (last.w - then.w);
  if (progress <= -moved) return 'reversed';
  if (progress < moved) return 'stopped';
  if (mlMin > 0 && densityGPerMl > 0) {
    const expectedG = ((mlMin * densityGPerMl) / 60) * (last.t - then.t);
    // 40 %, not 50: a line right at half its flow sat on the line and the
    // balance's noise tipped it over (stall.test.js).
    if (progress < (trusted ? 0.4 : 0.25) * expectedG) return 'slow';
  }
  return null;
}
