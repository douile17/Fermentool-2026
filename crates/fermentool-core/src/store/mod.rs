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

const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    sql: include_str!("migrations/0001_init.sql"),
}];

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum StoreError {
    Sqlite(rusqlite::Error),
    Json(serde_json::Error),
    /// `PRAGMA integrity_check` returned something other than `ok`.
    Corrupt(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Sqlite(e) => write!(f, "sqlite: {e}"),
            StoreError::Json(e) => write!(f, "json: {e}"),
            StoreError::Corrupt(s) => write!(f, "database integrity check failed: {s}"),
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
     direction, duration_s, tick_interval_s, curve_params, pump_addr, app_version";

const TICK_COLS: &str = "id, run_id, seq, wall_time, elapsed_s, target, written_ok, readback, note";

const EVENT_COLS: &str = "id, run_id, wall_time, level, kind, detail";

impl Store {
    /// Open (creating if needed) the database at `path`, set the durability
    /// pragmas, and run migrations.
    pub fn open(path: &Path) -> Result<Self> {
        Self::from_connection(Connection::open(path)?)
    }

    /// An in-memory database — for tests.
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

    /// `PRAGMA integrity_check` — `Ok(())` iff the database reports `ok`.
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
                pump_addr, app_version)
             VALUES
               (?1, ?2, ?3, 'running', ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
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

    /// Move a run to a terminal state.
    pub fn finish_run(&self, id: i64, status: RunStatus, ended_at: Timestamp) -> Result<()> {
        self.conn.execute(
            "UPDATE runs SET status = ?2, ended_at = ?3 WHERE id = ?1",
            params![id, status.as_str(), ended_at.to_string()],
        )?;
        Ok(())
    }

    // ---- ticks ----

    pub fn append_tick(&self, t: &NewTick) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO ticks (run_id, seq, wall_time, elapsed_s, target, written_ok, readback, note)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                t.run_id,
                t.seq,
                t.wall_time.to_string(),
                t.elapsed_s,
                t.target,
                t.written_ok as i64,
                t.readback,
                t.note,
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
        }
    }

    #[test]
    fn migrate_creates_schema_at_version_1() {
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
        assert_eq!(v, 1);
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
        assert_eq!(v, 1);
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
}
