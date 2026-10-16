-- New run asks how much the bottle gives before a refill, not a balance
-- weight: the refill level is the weight at the start minus this.
ALTER TABLE runs RENAME COLUMN refill_low_g TO refill_draw_g;
