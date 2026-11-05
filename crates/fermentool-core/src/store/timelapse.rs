//! Timelapses and their images (camera plan C). Written only by the
//! `timelapse` thread on its own connection; read by the API.

use jiff::Timestamp;
use rusqlite::{params, OptionalExtension, Row};

use super::{parse_ts, Result, Store, StoreError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TimelapseKind {
    /// Started and stopped with its run.
    Run,
    /// Started from the Camera page, no run.
    Standalone,
    /// A day's pictures taken by hand outside any timelapse.
    Pictures,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TimelapseState {
    Recording,
    Paused,
    Ended,
}

macro_rules! text_enum {
    ($t:ty { $($v:ident = $s:literal),+ }) => {
        impl $t {
            pub fn as_str(self) -> &'static str {
                match self { $(Self::$v => $s),+ }
            }
            fn parse(s: &str) -> rusqlite::Result<Self> {
                match s {
                    $($s => Ok(Self::$v),)+
                    other => Err(rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        format!("unknown {}: {other}", stringify!($t)).into(),
                    )),
                }
            }
        }
    };
}
text_enum!(TimelapseKind { Run = "run", Standalone = "standalone", Pictures = "pictures" });
text_enum!(TimelapseState { Recording = "recording", Paused = "paused", Ended = "ended" });

pub struct NewTimelapse {
    pub run_id: Option<i64>,
    pub kind: TimelapseKind,
    pub name: String,
    pub folder: String,
    pub interval_s: i64,
    pub started_at: Timestamp,
    pub stop_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct TimelapseRow {
    pub id: i64,
    pub run_id: Option<i64>,
    pub kind: TimelapseKind,
    pub name: String,
    pub folder: String,
    pub interval_s: i64,
    pub started_at: Timestamp,
    pub stop_at: Option<Timestamp>,
    pub ended_at: Option<Timestamp>,
    pub state: TimelapseState,
    /// `user`, `disk` or `folder` while paused.
    pub pause_reason: Option<String>,
    /// The schedule: slot k is due `anchor_at + (k - anchor_slot) * interval_s`.
    pub anchor_at: Timestamp,
    pub anchor_slot: i64,
    /// The last slot passed, taken or missed (`None` before the first).
    pub last_slot: Option<i64>,
    pub missed: i64,
    pub images: i64,
    pub bytes: i64,
}

/// A kept picture, with what the run was doing then.
pub struct NewImage {
    pub timelapse_id: i64,
    /// `None`: taken by hand.
    pub slot: Option<i64>,
    pub taken_at: Timestamp,
    pub elapsed_s: f64,
    pub file: String,
    pub thumb: String,
    pub bytes: i64,
    pub width: u32,
    pub height: u32,
    pub camera: String,
    /// JSON of the camera's picture controls then.
    pub controls: String,
    pub setpoint: Option<f64>,
    pub unit: Option<String>,
    pub delivered_ml: Option<f64>,
    pub weight_g: Option<f64>,
    pub trim_c: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ImageRow {
    pub id: i64,
    pub timelapse_id: i64,
    pub slot: Option<i64>,
    pub taken_at: Timestamp,
    pub elapsed_s: f64,
    pub file: String,
    pub thumb: String,
    pub bytes: i64,
    pub width: u32,
    pub height: u32,
    pub camera: String,
    pub controls: String,
    pub setpoint: Option<f64>,
    pub unit: Option<String>,
    pub delivered_ml: Option<f64>,
    pub weight_g: Option<f64>,
    pub trim_c: Option<f64>,
}

const TL_COLS: &str = "id, run_id, kind, name, folder, interval_s, started_at, stop_at, ended_at, \
     state, pause_reason, anchor_at, anchor_slot, last_slot, missed, images, bytes";
const IMG_COLS: &str = "id, timelapse_id, slot, taken_at, elapsed_s, file, thumb, bytes, width, height, \
     camera, controls, setpoint, unit, delivered_ml, weight_g, trim_c";

fn opt_ts(s: Option<String>) -> rusqlite::Result<Option<Timestamp>> {
    s.map(parse_ts).transpose()
}

fn timelapse_row(r: &Row) -> rusqlite::Result<TimelapseRow> {
    Ok(TimelapseRow {
        id: r.get(0)?,
        run_id: r.get(1)?,
        kind: TimelapseKind::parse(&r.get::<_, String>(2)?)?,
        name: r.get(3)?,
        folder: r.get(4)?,
        interval_s: r.get(5)?,
        started_at: parse_ts(r.get(6)?)?,
        stop_at: opt_ts(r.get(7)?)?,
        ended_at: opt_ts(r.get(8)?)?,
        state: TimelapseState::parse(&r.get::<_, String>(9)?)?,
        pause_reason: r.get(10)?,
        anchor_at: parse_ts(r.get(11)?)?,
        anchor_slot: r.get(12)?,
        last_slot: r.get(13)?,
        missed: r.get(14)?,
        images: r.get(15)?,
        bytes: r.get(16)?,
    })
}

fn image_row(r: &Row) -> rusqlite::Result<ImageRow> {
    Ok(ImageRow {
        id: r.get(0)?,
        timelapse_id: r.get(1)?,
        slot: r.get(2)?,
        taken_at: parse_ts(r.get(3)?)?,
        elapsed_s: r.get(4)?,
        file: r.get(5)?,
        thumb: r.get(6)?,
        bytes: r.get(7)?,
        width: r.get(8)?,
        height: r.get(9)?,
        camera: r.get(10)?,
        controls: r.get(11)?,
        setpoint: r.get(12)?,
        unit: r.get(13)?,
        delivered_ml: r.get(14)?,
        weight_g: r.get(15)?,
        trim_c: r.get(16)?,
    })
}

impl Store {
    /// A new timelapse, recording, its schedule anchored at its start.
    pub fn insert_timelapse(&self, t: &NewTimelapse) -> Result<i64> {
        let started = t.started_at.to_string();
        self.conn.execute(
            "INSERT INTO timelapses (run_id, kind, name, folder, interval_s, started_at, stop_at, state, anchor_at, anchor_slot)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'recording', ?6, 0)",
            params![t.run_id, t.kind.as_str(), t.name, t.folder, t.interval_s, started, t.stop_at.map(|s| s.to_string())],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn timelapse(&self, id: i64) -> Result<Option<TimelapseRow>> {
        Ok(self
            .conn
            .query_row(&format!("SELECT {TL_COLS} FROM timelapses WHERE id = ?1"), [id], timelapse_row)
            .optional()?)
    }

    /// The timelapse not ended, newest first (one at a time; a day's
    /// pictures bucket is not one).
    pub fn open_timelapse(&self) -> Result<Option<TimelapseRow>> {
        Ok(self
            .conn
            .query_row(
                &format!(
                    "SELECT {TL_COLS} FROM timelapses WHERE state != 'ended' AND kind != 'pictures'
                     ORDER BY id DESC LIMIT 1"
                ),
                [],
                timelapse_row,
            )
            .optional()?)
    }

    pub fn timelapse_of_run(&self, run_id: i64) -> Result<Option<TimelapseRow>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {TL_COLS} FROM timelapses WHERE run_id = ?1 ORDER BY id DESC LIMIT 1"),
                [run_id],
                timelapse_row,
            )
            .optional()?)
    }

    pub fn timelapses_of_run_ids(&self, ids: &[i64]) -> Result<Vec<TimelapseRow>> {
        let mut out = Vec::new();
        for &id in ids {
            out.extend(self.timelapse_of_run(id)?);
        }
        Ok(out)
    }

    pub fn all_timelapses(&self) -> Result<Vec<TimelapseRow>> {
        let mut st = self.conn.prepare(&format!("SELECT {TL_COLS} FROM timelapses ORDER BY id DESC"))?;
        let rows = st.query_map([], timelapse_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// The bucket of pictures taken by hand on `day` (`YYYY-MM-DD`).
    pub fn pictures_of_day(&self, day: &str) -> Result<Option<TimelapseRow>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {TL_COLS} FROM timelapses WHERE kind = 'pictures' AND name = ?1 ORDER BY id DESC LIMIT 1"),
                [format!("Pictures {day}")],
                timelapse_row,
            )
            .optional()?)
    }

    /// Writes back what changes during a timelapse: name, folder, interval,
    /// stop and end, state and reason, schedule anchor, last slot, missed.
    pub fn update_timelapse(&self, t: &TimelapseRow) -> Result<()> {
        let n = self.conn.execute(
            "UPDATE timelapses SET name = ?2, folder = ?3, interval_s = ?4, stop_at = ?5, ended_at = ?6,
                 state = ?7, pause_reason = ?8, anchor_at = ?9, anchor_slot = ?10, missed = ?11, last_slot = ?12
             WHERE id = ?1",
            params![
                t.id,
                t.name,
                t.folder,
                t.interval_s,
                t.stop_at.map(|s| s.to_string()),
                t.ended_at.map(|s| s.to_string()),
                t.state.as_str(),
                t.pause_reason,
                t.anchor_at.to_string(),
                t.anchor_slot,
                t.missed,
                t.last_slot,
            ],
        )?;
        if n == 0 {
            return Err(StoreError::Invalid(format!("no timelapse {}", t.id)));
        }
        Ok(())
    }

    /// Records one image and counts it on its timelapse, together.
    pub fn insert_image(&self, i: &NewImage) -> Result<i64> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO timelapse_images (timelapse_id, slot, taken_at, elapsed_s, file, thumb, bytes, width, height,
                 camera, controls, setpoint, unit, delivered_ml, weight_g, trim_c)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            params![
                i.timelapse_id,
                i.slot,
                i.taken_at.to_string(),
                i.elapsed_s,
                i.file,
                i.thumb,
                i.bytes,
                i.width,
                i.height,
                i.camera,
                i.controls,
                i.setpoint,
                i.unit,
                i.delivered_ml,
                i.weight_g,
                i.trim_c,
            ],
        )?;
        let id = tx.last_insert_rowid();
        tx.execute(
            "UPDATE timelapses SET images = images + 1, bytes = bytes + ?2 WHERE id = ?1",
            params![i.timelapse_id, i.bytes],
        )?;
        tx.commit()?;
        Ok(id)
    }

    /// A timelapse's images, newest first, older than `before_id` if given.
    pub fn images(&self, timelapse_id: i64, before_id: Option<i64>, limit: i64) -> Result<Vec<ImageRow>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {IMG_COLS} FROM timelapse_images WHERE timelapse_id = ?1 AND id < ?2 ORDER BY id DESC LIMIT ?3"
        ))?;
        let rows = st
            .query_map(params![timelapse_id, before_id.unwrap_or(i64::MAX), limit], image_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn image(&self, id: i64) -> Result<Option<ImageRow>> {
        Ok(self
            .conn
            .query_row(&format!("SELECT {IMG_COLS} FROM timelapse_images WHERE id = ?1"), [id], image_row)
            .optional()?)
    }

    /// The rows of a timelapse and its images (the files are the caller's).
    pub fn delete_timelapse(&self, id: i64) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM timelapse_images WHERE timelapse_id = ?1", [id])?;
        tx.execute("DELETE FROM timelapses WHERE id = ?1", [id])?;
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    fn ts(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    fn standalone(name: &str) -> NewTimelapse {
        NewTimelapse {
            run_id: None,
            kind: TimelapseKind::Standalone,
            name: name.into(),
            folder: "C:\\captures\\x".into(),
            interval_s: 300,
            started_at: ts("2026-11-05T10:00:00Z"),
            stop_at: Some(ts("2026-11-06T10:00:00Z")),
        }
    }

    fn image(timelapse_id: i64, slot: Option<i64>, bytes: i64) -> NewImage {
        NewImage {
            timelapse_id,
            slot,
            taken_at: ts("2026-11-05T10:05:00Z"),
            elapsed_s: 300.0,
            file: "000001_+000h05m00s.jpg".into(),
            thumb: "000001_+000h05m00s.jpg".into(),
            bytes,
            width: 1280,
            height: 720,
            camera: "Integrated Webcam".into(),
            controls: "[]".into(),
            setpoint: Some(2.5),
            unit: Some("ml_min".into()),
            delivered_ml: Some(12.0),
            weight_g: Some(850.2),
            trim_c: Some(1.01),
        }
    }

    #[test]
    fn timelapse_rows_round_trip() {
        let s = Store::open_in_memory().unwrap();
        let id = s.insert_timelapse(&standalone("Night")).unwrap();
        let mut t = s.timelapse(id).unwrap().unwrap();
        assert_eq!(t.name, "Night");
        assert_eq!(t.kind, TimelapseKind::Standalone);
        assert_eq!(t.state, TimelapseState::Recording);
        assert_eq!((t.anchor_at, t.anchor_slot, t.last_slot, t.missed, t.images), (t.started_at, 0, None, 0, 0));
        assert_eq!(t.stop_at, Some(ts("2026-11-06T10:00:00Z")));
        t.state = TimelapseState::Paused;
        t.pause_reason = Some("disk".into());
        t.missed = 3;
        t.last_slot = Some(7);
        t.interval_s = 60;
        t.folder = "D:\\elsewhere".into();
        s.update_timelapse(&t).unwrap();
        assert_eq!(s.timelapse(id).unwrap().unwrap(), t);
        assert!(s.timelapse(id + 1).unwrap().is_none());
    }

    #[test]
    fn insert_image_counts_images_and_bytes() {
        let s = Store::open_in_memory().unwrap();
        let id = s.insert_timelapse(&standalone("Night")).unwrap();
        let a = s.insert_image(&image(id, Some(1), 250_000)).unwrap();
        let b = s.insert_image(&image(id, None, 240_000)).unwrap();
        let t = s.timelapse(id).unwrap().unwrap();
        assert_eq!((t.images, t.bytes), (2, 490_000));
        let newest = s.images(id, None, 8).unwrap();
        assert_eq!(newest.iter().map(|i| i.id).collect::<Vec<_>>(), vec![b, a]);
        assert_eq!(s.images(id, Some(b), 8).unwrap().len(), 1);
        let one = s.image(a).unwrap().unwrap();
        assert_eq!(one.slot, Some(1));
        assert_eq!(one.weight_g, Some(850.2));
        assert_eq!(one.unit.as_deref(), Some("ml_min"));
    }

    #[test]
    fn open_timelapse_ignores_ended_ones() {
        let s = Store::open_in_memory().unwrap();
        let old = s.insert_timelapse(&standalone("Old")).unwrap();
        let mut t = s.timelapse(old).unwrap().unwrap();
        t.state = TimelapseState::Ended;
        t.ended_at = Some(ts("2026-11-05T11:00:00Z"));
        s.update_timelapse(&t).unwrap();
        assert!(s.open_timelapse().unwrap().is_none());
        let new = s.insert_timelapse(&standalone("New")).unwrap();
        assert_eq!(s.open_timelapse().unwrap().unwrap().id, new);
        assert_eq!(s.all_timelapses().unwrap().len(), 2);
    }

    #[test]
    fn a_runs_timelapse_and_the_days_pictures_are_found() {
        let s = Store::open_in_memory().unwrap();
        let run = s.insert_run(&crate::store::tests::sample_run()).unwrap();
        let mut nt = standalone("Run");
        nt.run_id = Some(run);
        nt.kind = TimelapseKind::Run;
        let id = s.insert_timelapse(&nt).unwrap();
        assert_eq!(s.timelapse_of_run(run).unwrap().unwrap().id, id);
        assert_eq!(s.timelapses_of_run_ids(&[run, 999]).unwrap().len(), 1);
        let mut p = standalone("Pictures 2026-11-05");
        p.kind = TimelapseKind::Pictures;
        p.interval_s = 0;
        let pid = s.insert_timelapse(&p).unwrap();
        assert_eq!(s.pictures_of_day("2026-11-05").unwrap().unwrap().id, pid);
        assert!(s.pictures_of_day("2026-11-06").unwrap().is_none());
    }

    #[test]
    fn deleting_a_run_is_not_blocked_by_its_timelapse() {
        let s = Store::open_in_memory().unwrap();
        let run = s.insert_run(&crate::store::tests::sample_run()).unwrap();
        s.finish_run(run, crate::store::RunStatus::Stopped, ts("2026-09-01T10:00:00Z")).unwrap();
        let mut nt = standalone("Run");
        nt.run_id = Some(run);
        nt.kind = TimelapseKind::Run;
        let id = s.insert_timelapse(&nt).unwrap();
        s.insert_image(&image(id, Some(0), 1)).unwrap();
        assert!(s.delete_run(run).unwrap());
        // the timelapse is deleted after, by the daemon
        s.delete_timelapse(id).unwrap();
        assert!(s.timelapse(id).unwrap().is_none());
        assert!(s.image(1).unwrap().is_none());
    }
}
