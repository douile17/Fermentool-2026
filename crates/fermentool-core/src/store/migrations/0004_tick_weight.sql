-- Balance reading and cumulative delivered mass at each journal tick, for
-- the tracking proof (chart, R², fitted µ). NULL when no balance was read.
ALTER TABLE ticks ADD COLUMN weight_g REAL;
ALTER TABLE ticks ADD COLUMN delivered_g REAL;
