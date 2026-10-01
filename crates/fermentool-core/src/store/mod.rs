//! SQLite persistence: the crash-safe journal behind every run.
//!
//! `runs` is the run manifest (with the immutable `started_at` resume anchor),
//! `ticks` is the append-only per-tick log, `events` is the human-readable
//! timeline, `app_state` holds odd singletons. WAL + `synchronous = FULL` so an
//! abrupt power loss cannot corrupt the file
//! (`docs/IMPLEMENTATION_PLAN.md` §4.6, §4.7).

use std::path::Path;

use fermentool_curves::CurveSpec;
use jiff::Timestamp;
use rusqlite::types::Type as SqlType;
use rusqlite::{params, Connection, OptionalExtension};

struct Migration {
    version: i64,
    sql: &'static str,
}

const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        sql: include_str!("migrations/0001_init.sql"),
    },
    Migration {
        version: 2,
        sql: include_str!("migrations/0002_gravimetric_trim.sql"),
    },
    Migration {
        version: 3,
        sql: include_str!("migrations/0003_tubing_calibration.sql"),
    },
    Migration {
        version: 4,
        sql: include_str!("migrations/0004_tick_weight.sql"),
    },
    Migration {
        version: 5,
        sql: include_str!("migrations/0005_calibration_archive.sql"),
    },
];

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum StoreError {
    Sqlite(rusqlite::Error),
    Json(serde_json::Error),
    /// `PRAGMA integrity_check` returned something other than `ok`.
    Corrupt(String),
    /// The caller's data was refused before it reached the database (e.g. a
    /// tubing calibration naming a run that is not a finished burst).
    Invalid(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Sqlite(e) => write!(f, "sqlite: {e}"),
            StoreError::Json(e) => write!(f, "json: {e}"),
            StoreError::Corrupt(s) => write!(f, "database integrity check failed: {s}"),
            StoreError::Invalid(s) => write!(f, "{s}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        StoreError::Sqlite(e)
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(e: serde_json::Error) -> Self {
        StoreError::Json(e)
    }
}

type Result<T> = std::result::Result<T, StoreError>;

// ---------------------------------------------------------------------------
// Small enums mirrored as CHECK-constrained TEXT columns
// ---------------------------------------------------------------------------

macro_rules! str_enum {
    ($(#[$m:meta])* $name:ident { $($variant:ident => $s:literal),+ $(,)? }) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum $name { $($variant),+ }

        impl $name {
            /// The token stored in the database / used on the wire.
            pub fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $s),+ }
            }
            /// Parse that token.
            pub fn from_token(s: &str) -> Option<Self> {
                match s { $($s => Some(Self::$variant),)+ _ => None }
            }
        }

        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(
                &self,
                s: S,
            ) -> ::core::result::Result<S::Ok, S::Error> {
                s.serialize_str(self.as_str())
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(
                d: D,
            ) -> ::core::result::Result<Self, D::Error> {
                let raw = <String as serde::Deserialize>::deserialize(d)?;
                Self::from_token(&raw).ok_or_else(|| {
                    <D::Error as serde::de::Error>::custom(format!(
                        "invalid {} {raw:?}",
                        stringify!($name)
                    ))
                })
            }
        }
    };
}

str_enum!(
    /// Lifecycle state of a run.
    RunStatus {
        Running => "running",
        Completed => "completed",
        Stopped => "stopped",
        Aborted => "aborted",
    }
);
str_enum!(
    /// Which quantity the run drives.
    ControlVar { Rpm => "rpm", MlMin => "ml_min" }
);
str_enum!(
    /// Pump rotation direction.
    Direction { Cw => "cw", Ccw => "ccw" }
);
str_enum!(
    /// What a run is for. A `calibration` run is a short tubing-calibration
    /// burst, kept apart from the dosing history it would otherwise pollute.
    RunKind { Dosing => "dosing", Calibration => "calibration" }
);

impl Default for RunKind {
    fn default() -> Self {
        RunKind::Dosing
    }
}

str_enum!(
    /// Severity of a timeline event.
    EventLevel { Info => "info", Warn => "warn", Error => "error" }
);

// ---------------------------------------------------------------------------
// Row / insert models
// ---------------------------------------------------------------------------

/// Fields needed to open a run. `status` is always `running` on insert;
/// `created_at` and `duration_s` are filled in by the store.
#[derive(Debug, Clone)]
pub struct NewRun {
    pub name: String,
    pub started_at: Timestamp,
    pub control_var: ControlVar,
    pub direction: Direction,
    pub tick_interval_s: i64,
    pub pump_addr: u8,
    pub app_version: String,
    pub curve: CurveSpec,
    pub gravimetric_trim: bool,
    pub kind: RunKind,
    /// The tubing calibration that seeded this run's trim, if any.
    pub tubing_calibration_id: Option<i64>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RunRow {
    pub id: i64,
    pub name: String,
    pub created_at: Timestamp,
    pub started_at: Timestamp,
    pub ended_at: Option<Timestamp>,
    pub status: RunStatus,
    pub control_var: ControlVar,
    pub direction: Direction,
    pub duration_s: i64,
    pub tick_interval_s: i64,
    pub pump_addr: u8,
    pub app_version: String,
    pub curve: CurveSpec,
    pub gravimetric_trim: bool,
    pub kind: RunKind,
    pub tubing_calibration_id: Option<i64>,
}

/// A tubing calibration to record: three hand-weighed bursts of one tube at
/// one setpoint. Only the raw inputs; the store derives the flows itself.
/// Also the `POST /api/calibrations` body.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct NewCalibration {
    pub tubing_lot_id: String,
    /// The lab's own reference for this tube, optional.
    #[serde(default)]
    pub internal_ref: Option<String>,
    pub inner_diameter_mm: f64,
    pub outer_diameter_mm: f64,
    pub control_var: ControlVar,
    pub setpoint: f64,
    pub density_g_per_ml: f64,
    /// The three `kind = calibration` runs, each finished.
    pub run_ids: [i64; 3],
    pub weights_g: [f64; 3],
    pub operator: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CalibrationRow {
    pub id: i64,
    pub created_at: Timestamp,
    pub tubing_lot_id: String,
    pub internal_ref: Option<String>,
    pub inner_diameter_mm: f64,
    pub outer_diameter_mm: f64,
    pub control_var: ControlVar,
    pub setpoint: f64,
    pub density_g_per_ml: f64,
    pub run_ids: [i64; 3],
    pub weights_g: [f64; 3],
    pub measured_ml_min: [f64; 3],
    pub mean_measured_ml_min: f64,
    pub cv_pct: f64,
    pub c0: f64,
    pub operator: Option<String>,
    pub note: Option<String>,
    /// Set when the tube is retired: hidden from New run, kept for history.
    pub archived_at: Option<Timestamp>,
}

#[derive(Debug, Clone)]
pub struct NewTick {
    pub run_id: i64,
    pub seq: i64,
    pub wall_time: Timestamp,
    pub elapsed_s: f64,
    pub target: f64,
    pub written_ok: bool,
    pub readback: Option<f64>,
    pub note: Option<String>,
    /// Balance reading at this tick, grams.
    pub weight_g: Option<f64>,
    /// Cumulative mass delivered since run start (trimmed runs), grams.
    pub delivered_g: Option<f64>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TickRow {
    pub id: i64,
    pub run_id: i64,
    pub seq: i64,
    pub wall_time: Timestamp,
    pub elapsed_s: f64,
    pub target: f64,
    pub written_ok: bool,
    pub readback: Option<f64>,
    pub note: Option<String>,
    pub weight_g: Option<f64>,
    pub delivered_g: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct NewEvent {
    pub run_id: Option<i64>,
    pub wall_time: Timestamp,
    pub level: EventLevel,
    pub kind: String,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct EventRow {
    pub id: i64,
    pub run_id: Option<i64>,
    pub wall_time: Timestamp,
    pub level: EventLevel,
    pub kind: String,
    pub detail: Option<String>,
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

/// Owns one SQLite connection. Not `Sync`; the engine keeps it on one thread.
pub struct Store {
    conn: Connection,
}

const RUN_COLS: &str = "id, name, created_at, started_at, ended_at, status, control_var, \
     direction, duration_s, tick_interval_s, curve_params, pump_addr, app_version, \
     gravimetric_trim, kind, tubing_calibration_id";

const CAL_COLS: &str = "id, created_at, tubing_lot_id, control_var, setpoint, \
     density_g_per_ml, run_1_id, run_2_id, run_3_id, weight_1_g, weight_2_g, weight_3_g, \
     measured_1_ml_min, measured_2_ml_min, measured_3_ml_min, mean_measured_ml_min, cv_pct, \
     c0, operator, note, inner_diameter_mm, outer_diameter_mm, internal_ref, archived_at";

const TICK_COLS: &str =
    "id, run_id, seq, wall_time, elapsed_s, target, written_ok, readback, note, weight_g, delivered_g";

const EVENT_COLS: &str = "id, run_id, wall_time, level, kind, detail";

impl Store {
    /// Open (creating if needed) the database at `path`, set the durability
    /// pragmas, and run migrations.
    pub fn open(path: &Path) -> Result<Self> {
        Self::from_connection(Connection::open(path)?)
    }

    /// An in-memory database, for tests.
    pub fn open_in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(conn: Connection) -> Result<Self> {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = FULL;
             PRAGMA foreign_keys = ON;",
        )?;
        let mut store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&mut self) -> Result<()> {
        let current: i64 = self
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))?;
        let pending: Vec<&Migration> = MIGRATIONS.iter().filter(|m| m.version > current).collect();
        if pending.is_empty() {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        for m in &pending {
            tx.execute_batch(m.sql)?;
        }
        let target = pending.iter().map(|m| m.version).max().unwrap_or(current);
        tx.execute_batch(&format!("PRAGMA user_version = {target};"))?;
        tx.commit()?;
        Ok(())
    }

    /// `PRAGMA integrity_check`, `Ok(())` iff the database reports `ok`.
    pub fn integrity_check(&self) -> Result<()> {
        let report: String = self
            .conn
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
        if report == "ok" {
            Ok(())
        } else {
            Err(StoreError::Corrupt(report))
        }
    }

    // ---- runs ----

    /// Insert a new `running` run; returns its id. Fails if another run is
    /// already `running` (enforced by a partial unique index).
    pub fn insert_run(&self, r: &NewRun) -> Result<i64> {
        let created = Timestamp::now().to_string();
        let started = r.started_at.to_string();
        let curve_json = serde_json::to_string(&r.curve)?;
        let duration_s = r.curve.duration.as_secs() as i64;
        self.conn.execute(
            "INSERT INTO runs
               (name, created_at, started_at, status, control_var, direction,
                duration_s, tick_interval_s, curve_kind, curve_mode, curve_params,
                pump_addr, app_version, gravimetric_trim, kind, tubing_calibration_id)
             VALUES
               (?1, ?2, ?3, 'running', ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                r.name,
                created,
                started,
                r.control_var.as_str(),
                r.direction.as_str(),
                duration_s,
                r.tick_interval_s,
                r.curve.kind().as_str(),
                r.curve.mode.as_str(),
                curve_json,
                r.pump_addr as i64,
                r.app_version,
                r.gravimetric_trim as i64,
                r.kind.as_str(),
                r.tubing_calibration_id,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn run(&self, id: i64) -> Result<Option<RunRow>> {
        self.conn
            .query_row(
                &format!("SELECT {RUN_COLS} FROM runs WHERE id = ?1"),
                [id],
                row_to_run,
            )
            .optional()
            .map_err(StoreError::from)
    }

    /// The single `running` run, if any (the crash-recovery entry point).
    pub fn running_run(&self) -> Result<Option<RunRow>> {
        self.conn
            .query_row(
                &format!("SELECT {RUN_COLS} FROM runs WHERE status = 'running' LIMIT 1"),
                [],
                row_to_run,
            )
            .optional()
            .map_err(StoreError::from)
    }

    /// Most recent runs first.
    pub fn list_runs(&self, limit: i64) -> Result<Vec<RunRow>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {RUN_COLS} FROM runs ORDER BY id DESC LIMIT ?1"
        ))?;
        let rows = stmt.query_map([limit], row_to_run)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Wipe every run and its journal (ticks + events), atomically. The caller
    /// is responsible for refusing this while a run is active or holding.
    ///
    /// Uses a real transaction (not a raw `BEGIN; … COMMIT;` batch) so that a
    /// mid-way failure rolls back on drop instead of leaving this long-lived
    /// connection stuck inside an open transaction.
    pub fn clear_history(&self) -> Result<()> {
        // Tubing calibrations are kept, and so are the bursts they point at
        // (with their ticks and events): the record must stay traceable to
        // its raw data, and the foreign key would refuse deleting them anyway.
        const KEEP: &str = "SELECT run_1_id FROM tubing_calibrations \
             UNION SELECT run_2_id FROM tubing_calibrations \
             UNION SELECT run_3_id FROM tubing_calibrations";
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(&format!("DELETE FROM ticks WHERE run_id NOT IN ({KEEP})"), [])?;
        tx.execute(
            &format!("DELETE FROM events WHERE run_id IS NULL OR run_id NOT IN ({KEEP})"),
            [],
        )?;
        tx.execute(&format!("DELETE FROM runs WHERE id NOT IN ({KEEP})"), [])?;
        tx.commit()?;
        Ok(())
    }

    /// Move a run to a terminal state.
    pub fn finish_run(&self, id: i64, status: RunStatus, ended_at: Timestamp) -> Result<()> {
        self.conn.execute(
            "UPDATE runs SET status = ?2, ended_at = ?3 WHERE id = ?1",
            params![id, status.as_str(), ended_at.to_string()],
        )?;
        Ok(())
    }

    // ---- tubing calibrations ----

    /// Record a tubing calibration. Every derived number (per-burst flow,
    /// mean, CV%, `c0`) is computed here from the three runs' own
    /// `started_at`/`ended_at` and the typed-in weights, never taken from the
    /// client, so the record stays traceable to the raw bursts.
    pub fn insert_calibration(&self, c: &NewCalibration) -> Result<i64> {
        let invalid = |m: String| StoreError::Invalid(m);
        if c.tubing_lot_id.trim().is_empty() {
            return Err(invalid("the tubing lot is required".into()));
        }
        if [c.inner_diameter_mm, c.outer_diameter_mm].iter().any(|v| !(v.is_finite() && *v > 0.0)) {
            return Err(invalid("inner and outer diameters must be positive (mm)".into()));
        }
        if c.outer_diameter_mm <= c.inner_diameter_mm {
            return Err(invalid("the outer diameter must be larger than the inner one".into()));
        }
        if !(c.density_g_per_ml.is_finite() && c.density_g_per_ml > 0.0) {
            return Err(invalid("density must be a positive number".into()));
        }
        if !(c.setpoint.is_finite() && c.setpoint > 0.0) {
            return Err(invalid("setpoint must be a positive number".into()));
        }
        if c.weights_g.iter().any(|w| !(w.is_finite() && *w > 0.0)) {
            return Err(invalid("every weight must be a positive number of grams".into()));
        }
        let [a, b, d] = c.run_ids;
        if a == b || a == d || b == d {
            return Err(invalid("the three bursts must be three different runs".into()));
        }

        let mut samples = [(0.0, 0.0); 3];
        for (i, &run_id) in c.run_ids.iter().enumerate() {
            let run = self
                .run(run_id)?
                .ok_or_else(|| invalid(format!("run {run_id} does not exist")))?;
            if run.kind != RunKind::Calibration {
                return Err(invalid(format!("run {run_id} is not a calibration burst")));
            }
            if run.control_var != c.control_var {
                return Err(invalid(format!(
                    "run {run_id} ran in {}, not {}",
                    run.control_var.as_str(),
                    c.control_var.as_str()
                )));
            }
            let commanded = run.curve.value_at(std::time::Duration::ZERO);
            if (commanded - c.setpoint).abs() > 1e-6 * c.setpoint.max(1.0) {
                return Err(invalid(format!(
                    "run {run_id} ran at {commanded}, not the setpoint {}",
                    c.setpoint
                )));
            }
            let ended = run
                .ended_at
                .ok_or_else(|| invalid(format!("run {run_id} has not finished")))?;
            let minutes = ended.duration_since(run.started_at).as_secs_f64() / 60.0;
            if minutes <= 0.0 {
                return Err(invalid(format!("run {run_id} has no duration")));
            }
            samples[i] = (minutes, c.weights_g[i]);
        }
        let r = crate::trim::compute_calibration(&crate::trim::CalibrationInput {
            setpoint: c.setpoint,
            density_g_per_ml: c.density_g_per_ml,
            samples,
        });

        self.conn.execute(
            "INSERT INTO tubing_calibrations
               (created_at, tubing_lot_id, control_var, setpoint,
                density_g_per_ml, run_1_id, run_2_id, run_3_id,
                weight_1_g, weight_2_g, weight_3_g,
                measured_1_ml_min, measured_2_ml_min, measured_3_ml_min,
                mean_measured_ml_min, cv_pct, c0, operator, note,
                inner_diameter_mm, outer_diameter_mm, internal_ref)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                     ?16, ?17, ?18, ?19, ?20, ?21, ?22)",
            params![
                Timestamp::now().to_string(),
                c.tubing_lot_id.trim(),
                c.control_var.as_str(),
                c.setpoint,
                c.density_g_per_ml,
                a,
                b,
                d,
                c.weights_g[0],
                c.weights_g[1],
                c.weights_g[2],
                r.measured_ml_min[0],
                r.measured_ml_min[1],
                r.measured_ml_min[2],
                r.mean_measured_ml_min,
                r.cv_pct,
                r.c0,
                c.operator,
                c.note,
                c.inner_diameter_mm,
                c.outer_diameter_mm,
                c.internal_ref.as_deref().map(str::trim).filter(|r| !r.is_empty()),
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn calibration(&self, id: i64) -> Result<Option<CalibrationRow>> {
        self.conn
            .query_row(
                &format!("SELECT {CAL_COLS} FROM tubing_calibrations WHERE id = ?1"),
                [id],
                row_to_calibration,
            )
            .optional()
            .map_err(StoreError::from)
    }

    /// Archive (`true`) or restore (`false`) a calibration. Archiving an
    /// already archived one keeps its first date. `None` if no such id.
    pub fn set_calibration_archived(
        &self,
        id: i64,
        archived: bool,
    ) -> Result<Option<CalibrationRow>> {
        let at = archived.then(|| Timestamp::now().to_string());
        let n = self.conn.execute(
            "UPDATE tubing_calibrations
             SET archived_at = CASE WHEN ?2 IS NULL THEN NULL
                                    ELSE COALESCE(archived_at, ?2) END
             WHERE id = ?1",
            params![id, at],
        )?;
        if n == 0 {
            return Ok(None);
        }
        self.calibration(id)
    }

    /// Calibrations newest first, archived ones included, optionally narrowed
    /// to one tubing lot (exact match).
    pub fn list_calibrations(
        &self,
        tubing_lot_id: Option<&str>,
    ) -> Result<Vec<CalibrationRow>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {CAL_COLS} FROM tubing_calibrations
             WHERE (?1 IS NULL OR tubing_lot_id = ?1)
             ORDER BY created_at DESC, id DESC"
        ))?;
        let rows = stmt.query_map(params![tubing_lot_id], row_to_calibration)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    // ---- ticks ----

    pub fn append_tick(&self, t: &NewTick) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO ticks (run_id, seq, wall_time, elapsed_s, target, written_ok, readback, note,
                                weight_g, delivered_g)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                t.run_id,
                t.seq,
                t.wall_time.to_string(),
                t.elapsed_s,
                t.target,
                t.written_ok as i64,
                t.readback,
                t.note,
                t.weight_g,
                t.delivered_g,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Ticks for a run with `seq` in `[from_seq, to_seq]`, ordered by `seq`.
    pub fn ticks(&self, run_id: i64, from_seq: i64, to_seq: i64) -> Result<Vec<TickRow>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {TICK_COLS} FROM ticks
             WHERE run_id = ?1 AND seq >= ?2 AND seq <= ?3 ORDER BY seq"
        ))?;
        let rows = stmt.query_map(params![run_id, from_seq, to_seq], row_to_tick)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// `(elapsed_s, delivered_g)` of a run's ticks that carry a delivered
    /// mass, sampled in SQL to about `max_points` (one tick in N, plus the
    /// latest) and reading only those two columns: a 100 h run journals
    /// ~360k ticks, and this runs on the control thread.
    pub fn delivery_samples(&self, run_id: i64, max_points: i64) -> Result<Vec<(f64, f64)>> {
        let n: i64 = self.conn.query_row(
            "SELECT count(*) FROM ticks WHERE run_id = ?1 AND delivered_g IS NOT NULL",
            [run_id],
            |r| r.get(0),
        )?;
        let stride = ((n + max_points - 1) / max_points.max(1)).max(1);
        let mut stmt = self.conn.prepare(
            "SELECT elapsed_s, delivered_g FROM ticks
             WHERE run_id = ?1 AND delivered_g IS NOT NULL
               AND (seq % ?2 = 0 OR seq = (SELECT max(seq) FROM ticks
                                           WHERE run_id = ?1 AND delivered_g IS NOT NULL))
             ORDER BY seq",
        )?;
        let rows = stmt.query_map(params![run_id, stride], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// The highest-`seq` tick for a run, if any.
    pub fn last_tick(&self, run_id: i64) -> Result<Option<TickRow>> {
        self.conn
            .query_row(
                &format!(
                    "SELECT {TICK_COLS} FROM ticks WHERE run_id = ?1 ORDER BY seq DESC LIMIT 1"
                ),
                [run_id],
                row_to_tick,
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn tick_count(&self, run_id: i64) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT count(*) FROM ticks WHERE run_id = ?1",
            [run_id],
            |r| r.get(0),
        )?)
    }

    // ---- events ----

    pub fn log_event(&self, e: &NewEvent) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO events (run_id, wall_time, level, kind, detail)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                e.run_id,
                e.wall_time.to_string(),
                e.level.as_str(),
                e.kind,
                e.detail,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Events for a run (or, with `run_id = None`, global events), newest first.
    pub fn events(&self, run_id: Option<i64>, limit: i64) -> Result<Vec<EventRow>> {
        let (sql, bind_run): (String, i64) = match run_id {
            Some(id) => (
                format!(
                    "SELECT {EVENT_COLS} FROM events WHERE run_id = ?1 ORDER BY id DESC LIMIT ?2"
                ),
                id,
            ),
            None => (
                format!(
                    "SELECT {EVENT_COLS} FROM events WHERE run_id IS NULL ORDER BY id DESC LIMIT ?2"
                ),
                0,
            ),
        };
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![bind_run, limit], row_to_event)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    // ---- app_state ----

    pub fn set_state(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO app_state (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn get_state(&self, key: &str) -> Result<Option<String>> {
        self.conn
            .query_row("SELECT value FROM app_state WHERE key = ?1", [key], |r| {
                r.get(0)
            })
            .optional()
            .map_err(StoreError::from)
    }
}

// ---------------------------------------------------------------------------
// Row mapping helpers
// ---------------------------------------------------------------------------

fn conv_err<E: std::error::Error + Send + Sync + 'static>(e: E) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, SqlType::Text, Box::new(e))
}

fn parse_ts(s: String) -> rusqlite::Result<Timestamp> {
    s.parse().map_err(conv_err::<jiff::Error>)
}

fn parse_curve(s: String) -> rusqlite::Result<CurveSpec> {
    serde_json::from_str(&s).map_err(conv_err::<serde_json::Error>)
}

fn parse_token<T>(s: String, f: impl FnOnce(&str) -> Option<T>, what: &str) -> rusqlite::Result<T> {
    f(&s).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            SqlType::Text,
            format!("invalid {what}: {s:?}").into(),
        )
    })
}

fn row_to_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<RunRow> {
    Ok(RunRow {
        id: row.get(0)?,
        name: row.get(1)?,
        created_at: parse_ts(row.get(2)?)?,
        started_at: parse_ts(row.get(3)?)?,
        ended_at: row.get::<_, Option<String>>(4)?.map(parse_ts).transpose()?,
        status: parse_token(row.get(5)?, RunStatus::from_token, "run status")?,
        control_var: parse_token(row.get(6)?, ControlVar::from_token, "control var")?,
        direction: parse_token(row.get(7)?, Direction::from_token, "direction")?,
        duration_s: row.get(8)?,
        tick_interval_s: row.get(9)?,
        curve: parse_curve(row.get(10)?)?,
        pump_addr: row.get::<_, i64>(11)? as u8,
        app_version: row.get(12)?,
        gravimetric_trim: row.get::<_, i64>(13)? != 0,
        kind: parse_token(row.get(14)?, RunKind::from_token, "run kind")?,
        tubing_calibration_id: row.get(15)?,
    })
}

fn row_to_calibration(row: &rusqlite::Row<'_>) -> rusqlite::Result<CalibrationRow> {
    Ok(CalibrationRow {
        id: row.get("id")?,
        created_at: parse_ts(row.get("created_at")?)?,
        tubing_lot_id: row.get("tubing_lot_id")?,
        internal_ref: row.get("internal_ref")?,
        inner_diameter_mm: row.get("inner_diameter_mm")?,
        outer_diameter_mm: row.get("outer_diameter_mm")?,
        control_var: parse_token(row.get("control_var")?, ControlVar::from_token, "control var")?,
        setpoint: row.get("setpoint")?,
        density_g_per_ml: row.get("density_g_per_ml")?,
        run_ids: [row.get("run_1_id")?, row.get("run_2_id")?, row.get("run_3_id")?],
        weights_g: [row.get("weight_1_g")?, row.get("weight_2_g")?, row.get("weight_3_g")?],
        measured_ml_min: [
            row.get("measured_1_ml_min")?,
            row.get("measured_2_ml_min")?,
            row.get("measured_3_ml_min")?,
        ],
        mean_measured_ml_min: row.get("mean_measured_ml_min")?,
        cv_pct: row.get("cv_pct")?,
        c0: row.get("c0")?,
        operator: row.get("operator")?,
        note: row.get("note")?,
        archived_at: row.get::<_, Option<String>>("archived_at")?.map(parse_ts).transpose()?,
    })
}

fn row_to_tick(row: &rusqlite::Row<'_>) -> rusqlite::Result<TickRow> {
    Ok(TickRow {
        id: row.get(0)?,
        run_id: row.get(1)?,
        seq: row.get(2)?,
        wall_time: parse_ts(row.get(3)?)?,
        elapsed_s: row.get(4)?,
        target: row.get(5)?,
        written_ok: row.get::<_, i64>(6)? != 0,
        readback: row.get(7)?,
        note: row.get(8)?,
        weight_g: row.get(9)?,
        delivered_g: row.get(10)?,
    })
}

fn row_to_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<EventRow> {
    Ok(EventRow {
        id: row.get(0)?,
        run_id: row.get(1)?,
        wall_time: parse_ts(row.get(2)?)?,
        level: parse_token(row.get(3)?, EventLevel::from_token, "event level")?,
        kind: row.get(4)?,
        detail: row.get(5)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn ts(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    fn sample_run() -> NewRun {
        NewRun {
            name: "soak-A".into(),
            started_at: ts("2026-09-01T09:30:00Z"),
            control_var: ControlVar::Rpm,
            direction: Direction::Cw,
            tick_interval_s: 10,
            pump_addr: 1,
            app_version: "0.1.0".into(),
            curve: CurveSpec::exponential_physio(2.0, 0.15, Duration::from_secs(100 * 3600))
                .with_clamp(0.1, 350.0),
            gravimetric_trim: false,
            kind: RunKind::Dosing,
            tubing_calibration_id: None,
        }
    }

    #[test]
    fn migrate_creates_schema_at_the_latest_version() {
        let s = Store::open_in_memory().unwrap();
        let tables: i64 = s
            .conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name IN
                 ('runs','ticks','events','app_state')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tables, 4);
        let v: i64 = s
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(v, 5);
    }

    #[test]
    fn migrate_is_idempotent() {
        let mut s = Store::open_in_memory().unwrap();
        s.migrate().unwrap();
        s.migrate().unwrap();
        let v: i64 = s
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(v, 5);
    }

    #[test]
    fn run_round_trips() {
        let s = Store::open_in_memory().unwrap();
        let new = sample_run();
        let id = s.insert_run(&new).unwrap();

        let got = s.run(id).unwrap().expect("run exists");
        assert_eq!(got.id, id);
        assert_eq!(got.name, "soak-A");
        assert_eq!(got.status, RunStatus::Running);
        assert_eq!(got.control_var, ControlVar::Rpm);
        assert_eq!(got.direction, Direction::Cw);
        assert_eq!(got.started_at, new.started_at);
        assert_eq!(got.duration_s, 100 * 3600);
        assert_eq!(got.curve, new.curve);
        assert!(got.ended_at.is_none());
    }

    #[test]
    fn only_one_running_run_allowed() {
        let s = Store::open_in_memory().unwrap();
        s.insert_run(&sample_run()).unwrap();
        let err = s.insert_run(&sample_run()).unwrap_err();
        assert!(matches!(err, StoreError::Sqlite(_)), "got {err:?}");
    }

    #[test]
    fn finish_run_clears_running_slot() {
        let s = Store::open_in_memory().unwrap();
        let id = s.insert_run(&sample_run()).unwrap();
        assert!(s.running_run().unwrap().is_some());

        s.finish_run(id, RunStatus::Completed, ts("2026-09-05T13:30:00Z"))
            .unwrap();
        assert!(s.running_run().unwrap().is_none());

        let got = s.run(id).unwrap().unwrap();
        assert_eq!(got.status, RunStatus::Completed);
        assert_eq!(got.ended_at, Some(ts("2026-09-05T13:30:00Z")));

        // the running slot is free again
        s.insert_run(&sample_run()).unwrap();
    }

    #[test]
    fn ticks_append_range_and_last() {
        let s = Store::open_in_memory().unwrap();
        let run_id = s.insert_run(&sample_run()).unwrap();
        for seq in 0..5 {
            s.append_tick(&NewTick {
                run_id,
                seq,
                wall_time: ts("2026-09-01T09:30:00Z"),
                elapsed_s: seq as f64 * 10.0,
                target: 2.0 + seq as f64,
                written_ok: seq != 3,
                readback: if seq == 0 { Some(1.9) } else { None },
                note: None,
                weight_g: None,
                delivered_g: None,
            })
            .unwrap();
        }

        let mid = s.ticks(run_id, 1, 3).unwrap();
        assert_eq!(mid.iter().map(|t| t.seq).collect::<Vec<_>>(), vec![1, 2, 3]);
        assert!(!mid[2].written_ok); // seq 3

        assert_eq!(s.tick_count(run_id).unwrap(), 5);
        assert_eq!(s.last_tick(run_id).unwrap().unwrap().seq, 4);
    }

    #[test]
    fn tick_seq_is_unique_per_run() {
        let s = Store::open_in_memory().unwrap();
        let run_id = s.insert_run(&sample_run()).unwrap();
        let t = NewTick {
            run_id,
            seq: 0,
            wall_time: ts("2026-09-01T09:30:00Z"),
            elapsed_s: 0.0,
            target: 2.0,
            written_ok: true,
            readback: None,
            note: None,
            weight_g: None,
            delivered_g: None,
        };
        s.append_tick(&t).unwrap();
        assert!(s.append_tick(&t).is_err());
    }

    #[test]
    fn events_log_and_list_newest_first() {
        let s = Store::open_in_memory().unwrap();
        let run_id = s.insert_run(&sample_run()).unwrap();
        for kind in ["start", "resume", "curve_done"] {
            s.log_event(&NewEvent {
                run_id: Some(run_id),
                wall_time: ts("2026-09-01T09:30:00Z"),
                level: EventLevel::Info,
                kind: kind.into(),
                detail: None,
            })
            .unwrap();
        }
        let list = s.events(Some(run_id), 10).unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].kind, "curve_done"); // newest first
        assert_eq!(list[0].level, EventLevel::Info);
    }

    #[test]
    fn app_state_round_trips_and_overwrites() {
        let s = Store::open_in_memory().unwrap();
        assert_eq!(s.get_state("k").unwrap(), None);
        s.set_state("k", "v1").unwrap();
        assert_eq!(s.get_state("k").unwrap().as_deref(), Some("v1"));
        s.set_state("k", "v2").unwrap();
        assert_eq!(s.get_state("k").unwrap().as_deref(), Some("v2"));
    }

    #[test]
    fn integrity_check_passes_on_fresh_db() {
        let s = Store::open_in_memory().unwrap();
        s.integrity_check().unwrap();
    }

    // ---- tubing calibrations ----

    /// A finished `kind = calibration` burst: a constant `setpoint` ml/min run
    /// stopped `minutes` after it started.
    fn calibration_run(s: &Store, setpoint: f64, start: &str, minutes: i64) -> i64 {
        let started = ts(start);
        let id = s
            .insert_run(&NewRun {
                name: "cal".into(),
                started_at: started,
                control_var: ControlVar::MlMin,
                curve: CurveSpec::linear(setpoint, setpoint, Duration::from_secs(3600)),
                kind: RunKind::Calibration,
                ..sample_run()
            })
            .unwrap();
        let ended = started + jiff::SignedDuration::from_mins(minutes);
        s.finish_run(id, RunStatus::Stopped, ended).unwrap();
        id
    }

    fn new_calibration(run_ids: [i64; 3], weights_g: [f64; 3]) -> NewCalibration {
        NewCalibration {
            tubing_lot_id: "LOT-42".into(),
            internal_ref: Some("TUB-007".into()),
            inner_diameter_mm: 1.6,
            outer_diameter_mm: 4.8,
            control_var: ControlVar::MlMin,
            setpoint: 10.0,
            density_g_per_ml: 1.0,
            run_ids,
            weights_g,
            operator: Some("AZ".into()),
            note: None,
        }
    }

    fn three_bursts(s: &Store) -> [i64; 3] {
        [
            calibration_run(s, 10.0, "2026-09-01T09:00:00Z", 5),
            calibration_run(s, 10.0, "2026-09-01T09:10:00Z", 5),
            calibration_run(s, 10.0, "2026-09-01T09:20:00Z", 5),
        ]
    }

    #[test]
    fn clear_history_keeps_calibrations_and_their_bursts() {
        // A calibration record points at its three bursts; clearing the run
        // history must neither fail on that reference nor orphan the record.
        let s = Store::open_in_memory().unwrap();
        let runs = three_bursts(&s);
        let cal = s.insert_calibration(&new_calibration(runs, [50.0; 3])).unwrap();
        let dosing = s.insert_run(&sample_run()).unwrap();
        s.finish_run(dosing, RunStatus::Stopped, ts("2026-09-02T09:00:00Z")).unwrap();

        s.clear_history().unwrap();

        assert!(s.run(dosing).unwrap().is_none());
        assert!(s.calibration(cal).unwrap().is_some());
        for id in runs {
            assert!(s.run(id).unwrap().is_some(), "burst {id} was deleted");
        }
    }

    #[test]
    fn a_tick_carries_the_weight_and_delivered_mass() {
        let s = Store::open_in_memory().unwrap();
        let run = s.insert_run(&sample_run()).unwrap();
        s.append_tick(&NewTick {
            run_id: run,
            seq: 0,
            wall_time: ts("2026-09-01T09:30:01Z"),
            elapsed_s: 1.0,
            target: 1.0,
            written_ok: true,
            readback: None,
            note: None,
            weight_g: Some(742.5),
            delivered_g: Some(0.3),
        })
        .unwrap();
        let t = s.last_tick(run).unwrap().unwrap();
        assert_eq!(t.weight_g, Some(742.5));
        assert_eq!(t.delivered_g, Some(0.3));
    }

    #[test]
    fn delivery_samples_are_bounded_and_keep_both_ends() {
        // A 100 h run journals ~360k ticks; the tracking report must not load
        // them all on the control thread every 10 s.
        let s = Store::open_in_memory().unwrap();
        let run = s.insert_run(&sample_run()).unwrap();
        for seq in 0..5_000 {
            s.append_tick(&NewTick {
                run_id: run,
                seq,
                wall_time: ts("2026-09-01T09:30:01Z"),
                elapsed_s: seq as f64,
                target: 1.0,
                written_ok: true,
                readback: None,
                note: None,
                weight_g: None,
                // the first ticks precede the first balance read
                delivered_g: (seq >= 3).then_some(seq as f64 * 0.5),
            })
            .unwrap();
        }
        let pts = s.delivery_samples(run, 500).unwrap();
        assert!(pts.len() <= 501, "got {}", pts.len());
        assert!(pts.len() >= 400, "got {}", pts.len());
        assert_eq!(pts.last(), Some(&(4_999.0, 2_499.5)), "the latest tick is kept");
        assert!(pts[0].0 <= 10.0, "starts near the first delivered tick: {:?}", pts[0]);
        assert!(pts.windows(2).all(|w| w[0].0 < w[1].0));
    }

    #[test]
    fn a_run_defaults_to_dosing_with_no_calibration() {
        let s = Store::open_in_memory().unwrap();
        let id = s.insert_run(&sample_run()).unwrap();
        let got = s.run(id).unwrap().unwrap();
        assert_eq!(got.kind, RunKind::Dosing);
        assert_eq!(got.tubing_calibration_id, None);
    }

    #[test]
    fn insert_and_fetch_a_calibration_round_trips() {
        let s = Store::open_in_memory().unwrap();
        let runs = three_bursts(&s);
        // 5 min each at a commanded 10 ml/min; 45 g at 1.0 g/mL is 9 ml/min.
        let id = s.insert_calibration(&new_calibration(runs, [45.0, 45.0, 45.0])).unwrap();
        let c = s.calibration(id).unwrap().expect("calibration exists");
        assert_eq!(c.id, id);
        assert_eq!(c.tubing_lot_id, "LOT-42");
        assert_eq!(c.internal_ref.as_deref(), Some("TUB-007"));
        assert_eq!(c.inner_diameter_mm, 1.6);
        assert_eq!(c.outer_diameter_mm, 4.8);
        assert_eq!(c.control_var, ControlVar::MlMin);
        assert_eq!(c.run_ids, runs);
        assert_eq!(c.weights_g, [45.0, 45.0, 45.0]);
        assert_eq!(c.operator.as_deref(), Some("AZ"));
        // Derived server-side from each run's own started_at/ended_at.
        for m in c.measured_ml_min {
            assert!((m - 9.0).abs() < 1e-9, "got {m}");
        }
        assert!((c.mean_measured_ml_min - 9.0).abs() < 1e-9);
        assert!(c.cv_pct.abs() < 1e-9);
        assert!((c.c0 - 10.0 / 9.0).abs() < 1e-9);
        assert!(s.calibration(id + 1).unwrap().is_none());
    }

    #[test]
    fn the_duration_comes_from_each_runs_own_clock() {
        let s = Store::open_in_memory().unwrap();
        let runs = [
            calibration_run(&s, 10.0, "2026-09-01T09:00:00Z", 5),
            calibration_run(&s, 10.0, "2026-09-01T09:10:00Z", 10),
            calibration_run(&s, 10.0, "2026-09-01T09:30:00Z", 4),
        ];
        let id = s.insert_calibration(&new_calibration(runs, [50.0, 100.0, 40.0])).unwrap();
        let c = s.calibration(id).unwrap().unwrap();
        for m in c.measured_ml_min {
            assert!((m - 10.0).abs() < 1e-9, "got {m}");
        }
    }

    #[test]
    fn the_internal_ref_is_optional_and_a_blank_one_is_stored_as_none() {
        let s = Store::open_in_memory().unwrap();
        for given in [None, Some("   ".to_string())] {
            let runs = three_bursts(&s);
            let c = NewCalibration { internal_ref: given, ..new_calibration(runs, [50.0; 3]) };
            let id = s.insert_calibration(&c).unwrap();
            assert_eq!(s.calibration(id).unwrap().unwrap().internal_ref, None);
        }
    }

    #[test]
    fn list_calibrations_filters_by_lot_newest_first() {
        let s = Store::open_in_memory().unwrap();
        let mut ids = Vec::new();
        for lot in ["A", "A", "B", "A"] {
            let runs = three_bursts(&s);
            let c = NewCalibration { tubing_lot_id: lot.into(), ..new_calibration(runs, [50.0; 3]) };
            ids.push(s.insert_calibration(&c).unwrap());
        }
        let all = s.list_calibrations(None).unwrap();
        assert_eq!(all.len(), 4);
        let lot_a = s.list_calibrations(Some("A")).unwrap();
        assert_eq!(lot_a.iter().map(|c| c.id).collect::<Vec<_>>(), vec![ids[3], ids[1], ids[0]]);
    }

    #[test]
    fn archive_and_restore_a_calibration() {
        let s = Store::open_in_memory().unwrap();
        let id = s.insert_calibration(&new_calibration(three_bursts(&s), [50.0; 3])).unwrap();
        assert!(s.calibration(id).unwrap().unwrap().archived_at.is_none());

        let first = s.set_calibration_archived(id, true).unwrap().unwrap().archived_at;
        assert!(first.is_some());
        // Archiving twice keeps the first date; the row stays listed.
        let again = s.set_calibration_archived(id, true).unwrap().unwrap().archived_at;
        assert_eq!(again, first);
        assert_eq!(s.list_calibrations(None).unwrap().len(), 1);

        let restored = s.set_calibration_archived(id, false).unwrap().unwrap();
        assert!(restored.archived_at.is_none());
        assert!(s.set_calibration_archived(id + 1, true).unwrap().is_none());
    }

    #[test]
    fn insert_calibration_refuses_runs_that_are_not_finished_calibration_bursts() {
        let s = Store::open_in_memory().unwrap();
        let good = three_bursts(&s);

        // A dosing run is not a calibration burst.
        let dosing = s.insert_run(&sample_run()).unwrap();
        s.finish_run(dosing, RunStatus::Stopped, ts("2026-09-01T10:00:00Z")).unwrap();
        let err = s
            .insert_calibration(&new_calibration([good[0], good[1], dosing], [50.0; 3]))
            .unwrap_err();
        assert!(matches!(err, StoreError::Invalid(_)), "got {err:?}");

        // A burst still running has no duration yet.
        let running = s
            .insert_run(&NewRun { kind: RunKind::Calibration, ..sample_run() })
            .unwrap();
        let err = s
            .insert_calibration(&new_calibration([good[0], good[1], running], [50.0; 3]))
            .unwrap_err();
        assert!(matches!(err, StoreError::Invalid(_)), "got {err:?}");

        // An unknown run id.
        let err = s
            .insert_calibration(&new_calibration([good[0], good[1], 9999], [50.0; 3]))
            .unwrap_err();
        assert!(matches!(err, StoreError::Invalid(_)), "got {err:?}");

        // The same burst counted twice.
        let err = s
            .insert_calibration(&new_calibration([good[0], good[0], good[1]], [50.0; 3]))
            .unwrap_err();
        assert!(matches!(err, StoreError::Invalid(_)), "got {err:?}");
    }

    #[test]
    fn insert_calibration_refuses_a_setpoint_or_unit_the_bursts_did_not_run_at() {
        let s = Store::open_in_memory().unwrap();
        let runs = three_bursts(&s);
        let wrong_setpoint = NewCalibration { setpoint: 12.0, ..new_calibration(runs, [50.0; 3]) };
        assert!(matches!(
            s.insert_calibration(&wrong_setpoint).unwrap_err(),
            StoreError::Invalid(_)
        ));
        let wrong_unit = NewCalibration {
            control_var: ControlVar::Rpm,
            ..new_calibration(runs, [50.0; 3])
        };
        assert!(matches!(
            s.insert_calibration(&wrong_unit).unwrap_err(),
            StoreError::Invalid(_)
        ));
    }

    #[test]
    fn insert_calibration_refuses_non_positive_or_non_finite_numbers() {
        let s = Store::open_in_memory().unwrap();
        let runs = three_bursts(&s);
        for bad in [
            new_calibration(runs, [50.0, 0.0, 50.0]),
            new_calibration(runs, [50.0, f64::NAN, 50.0]),
            NewCalibration { density_g_per_ml: 0.0, ..new_calibration(runs, [50.0; 3]) },
            NewCalibration { tubing_lot_id: "  ".into(), ..new_calibration(runs, [50.0; 3]) },
            NewCalibration { inner_diameter_mm: 0.0, ..new_calibration(runs, [50.0; 3]) },
            NewCalibration { outer_diameter_mm: f64::NAN, ..new_calibration(runs, [50.0; 3]) },
            // The outside of a tube is wider than its inside.
            NewCalibration { outer_diameter_mm: 1.6, ..new_calibration(runs, [50.0; 3]) },
        ] {
            assert!(matches!(s.insert_calibration(&bad).unwrap_err(), StoreError::Invalid(_)));
        }
    }
}
