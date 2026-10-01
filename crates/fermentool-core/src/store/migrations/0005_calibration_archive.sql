-- A tubing calibration is archived, never deleted: past runs keep pointing at
-- the one that seeded their trim. NULL = active, offered in New run.
ALTER TABLE tubing_calibrations ADD COLUMN archived_at TEXT;
