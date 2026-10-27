-- A database whose user_version 14 came from another branch (the bench) has
-- no setpoint_changes table: make sure it exists before adding the column.
CREATE TABLE IF NOT EXISTS setpoint_changes (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id    INTEGER NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    elapsed_s REAL NOT NULL,
    value     REAL NOT NULL,
    wall_time TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS ix_setpoint_changes_run ON setpoint_changes(run_id, elapsed_s);
-- The grams of owed feed the balance correction wrote off at this change.
ALTER TABLE setpoint_changes ADD COLUMN reset_g REAL;
