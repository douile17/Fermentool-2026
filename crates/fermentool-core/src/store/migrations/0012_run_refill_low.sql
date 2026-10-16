-- The automatic refill starts under a balance weight chosen in New run, and
-- fills back up to the weight at the start: no bottle size is needed after
-- all. Kept for Run again. (refill_capacity_ml, from 0011, stays unused.)
ALTER TABLE runs ADD COLUMN refill_low_g REAL;
