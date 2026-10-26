// The flow a calibrated tube can deliver: an ml/min run with an rpm
// calibration is driven in rpm, so its range is the pump's speed range
// (0.1 to the pump's max rpm) times the tube's measured ml per rpm. The daemon refuses a
// curve whose peak needs more than the pump's max rpm; this lets New run say so first.
import { RPM_LIMITS } from './fmt.js';

// A LabQ's top speed, when the status does not give the pump's.
const DEFAULT_MAX_RPM = 350;

/** ml per rpm of an rpm calibration, or null when it gives none. */
export function mlPerRpmOf(cal) {
  if (cal?.control_var !== 'rpm' || !(cal.setpoint > 0) || !(cal.mean_measured_ml_min > 0)) return null;
  return cal.mean_measured_ml_min / cal.setpoint;
}

/** Real ml/min at the pump's slowest and fastest speed, or null. */
export function tubeRange(mlPerRpm, maxRpm = DEFAULT_MAX_RPM) {
  if (!(mlPerRpm > 0)) return null;
  return { min: RPM_LIMITS.min * mlPerRpm, max: maxRpm * mlPerRpm };
}

/**
 * Whether a curve peaking at `peak` ml/min fits the tube.
 * `over`: past `maxRpm`, the daemon refuses the run.
 * `tight`: fits, but the regulation's factor c (up to 1 + limit) would push it
 * past `maxRpm`, where the pump caps and the regulation cannot catch up.
 */
export function curveFit({ peak, mlPerRpm, trimLimitPct = null, maxRpm = DEFAULT_MAX_RPM }) {
  const range = tubeRange(mlPerRpm, maxRpm);
  if (!range || !(peak > 0)) return null;
  const rpm = peak / mlPerRpm;
  const over = rpm > maxRpm + 1e-9;
  const headroom = trimLimitPct > 0 ? 1 + trimLimitPct / 100 : 1;
  const tight = !over && rpm * headroom > maxRpm + 1e-9;
  return { range, rpm, over, tight, maxRpm, safeMax: range.max / headroom };
}

/**
 * What New run says before Start when an rpm curve asks more than the feed
 * pump's top speed (the daemon refuses it, it does not flatten it), or null.
 * `label` is the pump's model as the operator reads it ("LabV").
 */
export function rpmCurveProblem({ peak, maxRpm = DEFAULT_MAX_RPM, label = 'pump' }) {
  if (!(peak > maxRpm + 1e-9)) return null;
  return `The curve asks up to ${Math.round(peak * 10) / 10} rpm; this ${label} goes to ${maxRpm} rpm (Settings > Pumps).`;
}
