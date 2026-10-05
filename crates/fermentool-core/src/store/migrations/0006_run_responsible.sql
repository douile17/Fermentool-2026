-- Who a run belongs to: its notifications (Teams) go to that person only.
-- NULL for runs from before, and for tubing-calibration bursts.
ALTER TABLE runs ADD COLUMN responsible TEXT;
