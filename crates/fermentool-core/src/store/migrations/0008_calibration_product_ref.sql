-- The tube's product reference (the maker's catalogue number on the bag,
-- e.g. 06409-14), beside its lot and the lab's own reference. NULL for
-- calibrations from before, and when left blank.
ALTER TABLE tubing_calibrations ADD COLUMN product_ref TEXT;
