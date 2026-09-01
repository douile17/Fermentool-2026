-- Fermentool schema v1. Applied inside one transaction by store::Store::migrate.

CREATE TABLE runs (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    name            TEXT    NOT NULL,
    created_at      TEXT    NOT NULL,                 -- RFC-3339 UTC
    started_at      TEXT    NOT NULL,                 -- t0, the immutable resume anchor
    ended_at        TEXT,
    status          TEXT    NOT NULL CHECK (status IN ('running','completed','stopped','aborted')),
    control_var     TEXT    NOT NULL CHECK (control_var IN ('rpm','ml_min')),
    direction       TEXT    NOT NULL CHECK (direction IN ('cw','ccw')),
    duration_s      INTEGER NOT NULL,
    tick_interval_s INTEGER NOT NULL,
    curve_kind      TEXT    NOT NULL,                 -- denormalised from curve_params for queries
    curve_mode      TEXT    NOT NULL,
    curve_params    TEXT    NOT NULL,                 -- full CurveSpec as JSON
    pump_addr       INTEGER NOT NULL,
    app_version     TEXT    NOT NULL
);
-- Fermentool never writes the pump's head-type / tubing-size registers: those
-- feed the pump's built-in ml/min table, which we don't use. Control is by rpm,
-- with a Fermentool-side calibration on top. So there is nothing to record here.

-- At most one run may be 'running' at a time.
CREATE UNIQUE INDEX ix_runs_one_running ON runs(status) WHERE status = 'running';

CREATE TABLE ticks (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id     INTEGER NOT NULL REFERENCES runs(id),
    seq        INTEGER NOT NULL,
    wall_time  TEXT    NOT NULL,                      -- RFC-3339 UTC
    elapsed_s  REAL    NOT NULL,
    target     REAL    NOT NULL,                      -- commanded setpoint
    written_ok INTEGER NOT NULL CHECK (written_ok IN (0,1)),
    readback   REAL,                                  -- verification read, if taken
    note       TEXT
);
CREATE UNIQUE INDEX ix_ticks_run_seq ON ticks(run_id, seq);

CREATE TABLE events (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id    INTEGER REFERENCES runs(id),
    wall_time TEXT NOT NULL,
    level     TEXT NOT NULL CHECK (level IN ('info','warn','error')),
    kind      TEXT NOT NULL,                          -- start|stop|resume|serial_lost|...
    detail    TEXT
);
CREATE INDEX ix_events_run ON events(run_id, id);

CREATE TABLE app_state (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
