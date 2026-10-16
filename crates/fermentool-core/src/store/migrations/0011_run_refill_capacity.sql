-- The weighed bottle's capacity (mL) a run with automatic refill was set up
-- with, chosen in New run: kept for Run again. NULL otherwise.
ALTER TABLE runs ADD COLUMN refill_capacity_ml REAL;

-- A tick's delivered_g is an estimate (pump ratio times commanded mass)
-- while the balance cannot measure it: a refill, the bottle touched. The
-- charts draw those stretches apart instead of a flat line that dips.
ALTER TABLE ticks ADD COLUMN delivered_estimated INTEGER NOT NULL DEFAULT 0;
