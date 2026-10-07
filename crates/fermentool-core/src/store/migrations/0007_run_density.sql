-- The feed density a trimmed run counted its delivered volume with: the
-- tracking report of a past run converts its grams with it, not with
-- whatever density Settings holds today. NULL for runs from before, and for
-- runs without the balance trim.
ALTER TABLE runs ADD COLUMN density_g_per_ml REAL;
