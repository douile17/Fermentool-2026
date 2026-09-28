-- tubing_calibrations is created before the ALTER TABLEs below reference it:
-- SQLite does not validate FK targets at DDL time, but creating the
-- referenced table first keeps the migration readable in the order a reader
-- would expect (the plan's original draft had this backwards).
CREATE TABLE tubing_calibrations (
    id                    INTEGER PRIMARY KEY AUTOINCREMENT,
    created_at            TEXT    NOT NULL,
    tubing_lot_id         TEXT    NOT NULL,
    tubing_size           TEXT    NOT NULL,
    control_var           TEXT    NOT NULL CHECK (control_var IN ('rpm','ml_min')),
    setpoint              REAL    NOT NULL,
    density_g_per_ml      REAL    NOT NULL,
    run_1_id INTEGER NOT NULL REFERENCES runs(id),
    run_2_id INTEGER NOT NULL REFERENCES runs(id),
    run_3_id INTEGER NOT NULL REFERENCES runs(id),
    weight_1_g REAL NOT NULL,
    weight_2_g REAL NOT NULL,
    weight_3_g REAL NOT NULL,
    measured_1_ml_min REAL NOT NULL,
    measured_2_ml_min REAL NOT NULL,
    measured_3_ml_min REAL NOT NULL,
    mean_measured_ml_min  REAL    NOT NULL,
    cv_pct                REAL    NOT NULL,
    c0                    REAL    NOT NULL,
    operator              TEXT,
    note                  TEXT
);
CREATE INDEX ix_tubing_cal_lot_size ON tubing_calibrations(tubing_lot_id, tubing_size, created_at);

ALTER TABLE runs ADD COLUMN kind TEXT NOT NULL DEFAULT 'dosing' CHECK (kind IN ('dosing','calibration'));
ALTER TABLE runs ADD COLUMN tubing_calibration_id INTEGER REFERENCES tubing_calibrations(id);
