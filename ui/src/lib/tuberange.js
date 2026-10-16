// The flow a calibrated tube can deliver: an ml/min run with an rpm
// calibration is driven in rpm, so its range is the pump's speed range
// (0.1 to 350 rpm) times the tube's measured ml per rpm. The daemon refuses a
// curve whose peak needs more than 350 rpm; this lets New run say so first.
import { RPM_LIMITS } from './fmt.js';

/** ml per rpm of an rpm calibration, or null when it gives none. */
export function mlPerRpmOf(cal) {
  if (cal?.control_var !== 'rpm' || !(cal.setpoint > 0) || !(cal.mean_measured_ml_min > 0)) return null;
  return cal.mean_measured_ml_min / cal.setpoint;
}

/** Real ml/min at the pump's slowest and fastest speed, or null. */
export function tubeRange(mlPerRpm) {
  if (!(mlPerRpm > 0)) return null;
  return { min: RPM_LIMITS.min * mlPerRpm, max: RPM_LIMITS.max * mlPerRpm };
}

/**
 * Whether a curve peaking at `peak` ml/min fits the tube.
 * `over`: past 350 rpm, the daemon refuses the run.
 * `tight`: fits, but the regulation's factor c (up to 1 + limit) would push it
 * past 350 rpm, where the pump caps and the regulation cannot catch up.
 */
export function curveFit({ peak, mlPerRpm, trimLimitPct = null }) {
  const range = tubeRange(mlPerRpm);
  if (!range || !(peak > 0)) return null;
  const rpm = peak / mlPerRpm;
  const over = rpm > RPM_LIMITS.max + 1e-9;
  const headroom = trimLimitPct > 0 ? 1 + trimLimitPct / 100 : 1;
  const tight = !over && rpm * headroom > RPM_LIMITS.max + 1e-9;
  return { range, rpm, over, tight, safeMax: range.max / headroom };
}
