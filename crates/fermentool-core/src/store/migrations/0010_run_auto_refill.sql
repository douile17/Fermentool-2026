-- The run drives the refill pump (`[refill]`): chosen in New run, kept with
-- the run so a resume after a crash drives it again. 0 for runs from before.
ALTER TABLE runs ADD COLUMN auto_refill INTEGER NOT NULL DEFAULT 0;
