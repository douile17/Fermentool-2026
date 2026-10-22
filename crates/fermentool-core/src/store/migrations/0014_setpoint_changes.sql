-- The operator changed the rate of a running Constant run: the original curve
-- stays in `runs.curve_json`, each change is one row here (from `elapsed_s`
-- the setpoint is `value`).
CREATE TABLE setpoint_changes (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id    INTEGER NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    elapsed_s REAL NOT NULL,
    value     REAL NOT NULL,
    wall_time TEXT NOT NULL
);
CREATE INDEX ix_setpoint_changes_run ON setpoint_changes(run_id, elapsed_s);
