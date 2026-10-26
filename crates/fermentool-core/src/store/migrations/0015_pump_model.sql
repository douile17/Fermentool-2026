-- The pump model a tube was calibrated on: a calibration (ml per rpm)
-- is only true on that model's head. Every calibration before this was
-- measured on the LabQ.
ALTER TABLE tubing_calibrations ADD COLUMN pump_model TEXT NOT NULL DEFAULT 'labq';
-- The feed pump a run drove, for its History detail.
ALTER TABLE runs ADD COLUMN pump_model TEXT NOT NULL DEFAULT 'labq';
ALTER TABLE runs ADD COLUMN pump_head INTEGER;
