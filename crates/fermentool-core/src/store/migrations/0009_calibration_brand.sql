-- The tube's brand (Watson-Marlow, VWR, Masterflex...), beside its product
-- reference and lot. Required for a new calibration; NULL only for the
-- calibrations from before.
ALTER TABLE tubing_calibrations ADD COLUMN brand TEXT;
