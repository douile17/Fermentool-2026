// Tubing calibrations are measured on one pump model: New run offers only
// those of the feed pump's model.

/** The calibrations measured on `pumpModel`; a row without a `pump_model`
 *  (made before the models) counts as a LabQ. */
export const forFeedPump = (cals, pumpModel) => (cals ?? []).filter((c) => (c.pump_model ?? 'labq') === (pumpModel ?? 'labq'));
