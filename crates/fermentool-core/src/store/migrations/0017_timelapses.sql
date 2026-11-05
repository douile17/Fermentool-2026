-- Timelapses (camera plan C): a run's, one on its own, or a day's pictures
-- taken by hand. `run_id` is a plain integer, no REFERENCES: foreign keys
-- are on, and a reference would block deleting the run (the daemon deletes
-- the timelapse after it).
CREATE TABLE timelapses (
    id INTEGER PRIMARY KEY,
    run_id INTEGER,
    kind TEXT NOT NULL,            -- run | standalone | pictures
    name TEXT NOT NULL,
    folder TEXT NOT NULL,          -- absolute, chosen at its start
    interval_s INTEGER NOT NULL,   -- 0 for pictures
    started_at TEXT NOT NULL,
    stop_at TEXT,                  -- standalone "stop after"
    ended_at TEXT,
    state TEXT NOT NULL,           -- recording | paused | ended
    pause_reason TEXT,             -- user | disk | folder
    anchor_at TEXT NOT NULL,       -- the schedule: slot k due anchor_at + (k - anchor_slot) * interval
    anchor_slot INTEGER NOT NULL,
    missed INTEGER NOT NULL DEFAULT 0,
    images INTEGER NOT NULL DEFAULT 0,
    bytes INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX timelapses_by_run ON timelapses(run_id);

-- One kept picture, with what the run was doing then (for a model trained
-- on the images later).
CREATE TABLE timelapse_images (
    id INTEGER PRIMARY KEY,
    timelapse_id INTEGER NOT NULL REFERENCES timelapses(id),
    slot INTEGER,                  -- NULL: taken by hand
    taken_at TEXT NOT NULL,
    elapsed_s REAL NOT NULL,
    file TEXT NOT NULL,            -- name in the timelapse's folder
    thumb TEXT NOT NULL,           -- name under its thumbs/
    bytes INTEGER NOT NULL,
    width INTEGER NOT NULL,
    height INTEGER NOT NULL,
    camera TEXT NOT NULL,
    controls TEXT NOT NULL,        -- JSON: the camera's picture controls then
    setpoint REAL,
    unit TEXT,
    delivered_ml REAL,
    weight_g REAL,
    trim_c REAL
);
CREATE INDEX timelapse_images_by_timelapse ON timelapse_images(timelapse_id, id);
