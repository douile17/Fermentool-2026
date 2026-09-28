-- A run's `gravimetric_trim` choice must be durable, not just an in-memory
-- ActiveRun flag: crash-resume rebuilds ActiveRun from this row, and without
-- it a converged trim_c (persisted separately via app_state) would silently
-- stop being applied after a restart because resume() would not know the
-- interrupted run had opted in.
ALTER TABLE runs ADD COLUMN gravimetric_trim INTEGER NOT NULL DEFAULT 0
    CHECK (gravimetric_trim IN (0, 1));
