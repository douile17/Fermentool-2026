//! Tests of the `timelapse` thread: a fake camera, plain `RunView`s for the
//! run, a database file and a captures folder of their own.

use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use fermentool_camera::protocol::{Control, ControlInfo};
use jiff::Timestamp;

use super::*;
use crate::camera::Still;
use crate::store::{Store, TimelapseKind, TimelapseState};

#[derive(Default)]
struct CamState {
    connected: bool,
    taken: u32,
    /// While set, a still waits for a message on it.
    gate: Option<mpsc::Receiver<()>>,
}

#[derive(Clone, Default)]
struct FakeCam(Arc<Mutex<CamState>>);

impl FakeCam {
    fn on() -> Self {
        let c = FakeCam::default();
        c.0.lock().unwrap().connected = true;
        c
    }
    fn set_connected(&self, on: bool) {
        self.0.lock().unwrap().connected = on;
    }
}

impl StillSource for FakeCam {
    fn still(&self, quality: u8) -> Result<Still, String> {
        let gate = self.0.lock().unwrap().gate.take();
        if let Some(g) = gate {
            let _ = g.recv();
        }
        let mut s = self.0.lock().unwrap();
        if !s.connected {
            return Err("the camera is not connected".into());
        }
        s.taken += 1;
        Ok(Still {
            jpeg: vec![0xFF, 0xD8, quality, s.taken as u8],
            thumb: vec![0xFF, 0xD8, 1],
            device: "Fake cam".into(),
            width: 1280,
            height: 720,
            at: Timestamp::now(),
        })
    }
    fn controls(&self) -> Result<Vec<ControlInfo>, String> {
        Ok(vec![ControlInfo {
            control: Control::Exposure,
            min: -10,
            max: -4,
            step: 1,
            default: -6,
            value: -7,
            can_auto: true,
            auto: false,
        }])
    }
    fn connected(&self) -> bool {
        self.0.lock().unwrap().connected
    }
}

struct Fixture {
    db: PathBuf,
    root: PathBuf,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("ft-tls-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Fixture { db: dir.join("db.sqlite"), root: dir.join("captures") }
    }
    fn spawn(&self, cam: FakeCam) -> Timelapses {
        let root = self.root.clone();
        Timelapses::spawn_with(Store::open(&self.db).unwrap(), cam, move || root.clone(), quick())
    }
    fn store(&self) -> Store {
        Store::open(&self.db).unwrap()
    }
}

fn quick() -> Timings {
    Timings {
        poll: Duration::from_millis(20),
        run_gone_grace: Duration::from_millis(300),
        trouble_recheck: Duration::from_millis(300),
        free_bytes: |_| Some(u64::MAX),
    }
}

fn wait_for(max: Duration, mut f: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + max;
    while Instant::now() < end {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    f()
}

fn driving(run_id: i64, elapsed_s: f64) -> RunView {
    RunView {
        active: Some(RunNow {
            run_id,
            elapsed_s,
            setpoint: Some(2.5),
            unit: Some("ml_min".into()),
            delivered_ml: Some(40.0),
            weight_g: Some(812.4),
            trim_c: Some(1.02),
        }),
        pending_recovery: false,
    }
}

fn gone() -> RunView {
    RunView { active: None, pending_recovery: false }
}

fn awaiting() -> RunView {
    RunView { active: None, pending_recovery: true }
}

fn images(tl: &Timelapses) -> i64 {
    tl.current().map_or(0, |c| c.images)
}

#[test]
fn standalone_takes_images_on_schedule_and_resumes_after_a_restart() {
    let fx = Fixture::new("standalone");
    let tl = fx.spawn(FakeCam::on());
    let id = tl.start_standalone("Night".into(), 1, None).unwrap();
    assert!(wait_for(Duration::from_secs(5), || images(&tl) >= 3), "{:?}", tl.current());
    tl.shutdown();
    // past the schedule's 2 s grace (`LATE_S`): 2 to 4 slots go by unseen
    std::thread::sleep(Duration::from_millis(4500));
    let tl = fx.spawn(FakeCam::on());
    let before = fx.store().timelapse(id).unwrap().unwrap().images;
    assert!(wait_for(Duration::from_secs(3), || images(&tl) > before));
    let row = fx.store().timelapse(id).unwrap().unwrap();
    assert!((2..=4).contains(&row.missed), "missed {} while the daemon was down", row.missed);
    assert_eq!(row.state, TimelapseState::Recording);
    let files = std::fs::read_dir(PathBuf::from(&row.folder)).unwrap().count();
    assert!(files >= 4, "images, thumbs/ and images.csv in {}", row.folder);
    tl.shutdown();
}

#[test]
fn run_linked_starts_with_the_run_and_ends_with_it() {
    let fx = Fixture::new("runlinked");
    // a real run: its events need it (foreign key)
    let run = fx.store().insert_run(&crate::store::tests::sample_run()).unwrap();
    let tl = fx.spawn(FakeCam::on());
    let feed = tl.status_sink();
    feed.send(driving(run, 0.0)).unwrap();
    let id = tl.start_with_run(run, "Run 7".into(), 1).unwrap();
    for i in 0..30 {
        feed.send(driving(run, i as f64 * 0.1)).unwrap();
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(images(&tl) >= 2);
    feed.send(gone()).unwrap();
    assert!(wait_for(Duration::from_secs(3), || tl.current().is_none()));
    let row = fx.store().timelapse(id).unwrap().unwrap();
    assert_eq!((row.kind, row.state, row.run_id), (TimelapseKind::Run, TimelapseState::Ended, Some(run)));
    assert!(row.ended_at.is_some());
    let kinds: Vec<String> = fx.store().events(Some(run), 50).unwrap().into_iter().map(|e| e.kind).collect();
    assert!(kinds.contains(&"timelapse_start".to_string()) && kinds.contains(&"timelapse_end".to_string()), "{kinds:?}");
    tl.shutdown();
}

#[test]
fn awaiting_resume_pauses_without_missing() {
    let fx = Fixture::new("awaiting");
    let tl = fx.spawn(FakeCam::on());
    let feed = tl.status_sink();
    feed.send(driving(3, 0.0)).unwrap();
    let id = tl.start_with_run(3, "Run 3".into(), 1).unwrap();
    assert!(wait_for(Duration::from_secs(3), || images(&tl) >= 1));
    for _ in 0..30 {
        feed.send(awaiting()).unwrap();
        std::thread::sleep(Duration::from_millis(100));
    }
    let cur = tl.current().unwrap();
    assert_eq!(cur.state, "awaiting_resume");
    let during = cur.images;
    assert_eq!(cur.missed, 0);
    for i in 0..25 {
        feed.send(driving(3, 10.0 + i as f64 * 0.1)).unwrap();
        std::thread::sleep(Duration::from_millis(100));
    }
    let row = fx.store().timelapse(id).unwrap().unwrap();
    assert!(row.images > during, "images again once resumed");
    assert_eq!(row.missed, 0, "a wait for Resume is not missed");
    tl.shutdown();
}

#[test]
fn run_end_during_a_still_keeps_it_and_ends() {
    let fx = Fixture::new("endduring");
    let cam = FakeCam::on();
    let (release, gate) = mpsc::channel();
    cam.0.lock().unwrap().gate = Some(gate);
    let tl = fx.spawn(cam);
    let feed = tl.status_sink();
    feed.send(driving(5, 0.0)).unwrap();
    let id = tl.start_with_run(5, "Run 5".into(), 60).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    // the first still is held; the run ends meanwhile
    feed.send(gone()).unwrap();
    std::thread::sleep(Duration::from_millis(500));
    release.send(()).unwrap();
    assert!(wait_for(Duration::from_secs(3), || tl.current().is_none()));
    let st = fx.store();
    let row = st.timelapse(id).unwrap().unwrap();
    let imgs = st.images(id, None, 10).unwrap();
    assert_eq!(imgs.len(), 1);
    assert!(row.ended_at.unwrap() >= imgs[0].taken_at);
    tl.shutdown();
}

#[test]
fn a_run_ends_a_standalone_timelapse() {
    let fx = Fixture::new("runends");
    let tl = fx.spawn(FakeCam::on());
    let alone = tl.start_standalone("Before the feed".into(), 1, None).unwrap();
    assert!(wait_for(Duration::from_secs(3), || images(&tl) >= 1));
    let feed = tl.status_sink();
    feed.send(driving(9, 0.0)).unwrap();
    let run = tl.start_with_run(9, "Run 9".into(), 1).unwrap();
    let st = fx.store();
    let a = st.timelapse(alone).unwrap().unwrap();
    assert_eq!(a.state, TimelapseState::Ended);
    assert!(a.images >= 1);
    assert_eq!(tl.current().unwrap().id, run);
    assert!(tl.start_standalone("Another".into(), 1, None).is_err(), "refused while a run's records");
    tl.shutdown();
}

#[test]
fn each_image_keeps_the_runs_figures_and_the_camera_controls() {
    let fx = Fixture::new("figures");
    let tl = fx.spawn(FakeCam::on());
    let feed = tl.status_sink();
    feed.send(driving(4, 3600.0)).unwrap();
    let id = tl.start_with_run(4, "Run 4".into(), 60).unwrap();
    assert!(wait_for(Duration::from_secs(3), || images(&tl) >= 1));
    let img = fx.store().images(id, None, 1).unwrap().remove(0);
    assert_eq!(img.slot, Some(0));
    assert!((img.elapsed_s - 3600.0).abs() < 2.0, "{}", img.elapsed_s);
    assert_eq!((img.setpoint, img.unit.as_deref()), (Some(2.5), Some("ml_min")));
    assert_eq!((img.delivered_ml, img.weight_g, img.trim_c), (Some(40.0), Some(812.4), Some(1.02)));
    assert_eq!((img.camera.as_str(), img.width, img.height), ("Fake cam", 1280, 720));
    assert!(img.controls.contains("\"exposure\"") && img.controls.contains("-7"), "{}", img.controls);
    assert!(img.file.starts_with("000000_+001h00m"), "{}", img.file);
    let row = fx.store().timelapse(id).unwrap().unwrap();
    let csv = std::fs::read_to_string(PathBuf::from(&row.folder).join("images.csv")).unwrap();
    assert!(csv.lines().nth(1).unwrap().contains(",Fake cam,1280,720,2.5,ml_min,40,812.4,1.02,"), "{csv}");
    assert_eq!(std::fs::read(PathBuf::from(&row.folder).join(&img.file)).unwrap()[2], 95, "stored at quality 95");
    tl.shutdown();
}

#[test]
fn take_picture_without_a_timelapse_goes_to_the_days_bucket() {
    let fx = Fixture::new("pictures");
    let tl = fx.spawn(FakeCam::on());
    let a = tl.take_picture().unwrap();
    let b = tl.take_picture().unwrap();
    let st = fx.store();
    let img = st.image(a).unwrap().unwrap();
    assert_eq!(img.slot, None);
    assert_eq!(st.image(b).unwrap().unwrap().timelapse_id, img.timelapse_id, "one bucket a day");
    let t = st.timelapse(img.timelapse_id).unwrap().unwrap();
    assert_eq!(t.kind, TimelapseKind::Pictures);
    assert!(t.name.starts_with("Pictures 20"), "{}", t.name);
    assert!(tl.current().is_none(), "the bucket is not a timelapse recording");
    tl.shutdown();
}

#[test]
fn take_picture_goes_into_the_running_timelapse() {
    let fx = Fixture::new("manual");
    let tl = fx.spawn(FakeCam::on());
    let id = tl.start_standalone("Night".into(), 3600, None).unwrap();
    assert!(wait_for(Duration::from_secs(3), || images(&tl) >= 1));
    let img = tl.take_picture().unwrap();
    let row = fx.store().image(img).unwrap().unwrap();
    assert_eq!((row.timelapse_id, row.slot), (id, None));
    assert!(row.file.starts_with('m'));
    tl.shutdown();
}

#[test]
fn pause_is_not_missed_and_interval_change_keeps_the_next_image() {
    let fx = Fixture::new("pause");
    let tl = fx.spawn(FakeCam::on());
    let id = tl.start_standalone("Night".into(), 1, None).unwrap();
    assert!(wait_for(Duration::from_secs(3), || images(&tl) >= 1));
    tl.pause().unwrap();
    assert_eq!(tl.current().unwrap().state, "paused");
    let paused_at = images(&tl);
    std::thread::sleep(Duration::from_millis(2500));
    assert_eq!(images(&tl), paused_at);
    tl.resume().unwrap();
    assert!(wait_for(Duration::from_secs(3), || images(&tl) > paused_at));
    assert_eq!(fx.store().timelapse(id).unwrap().unwrap().missed, 0);
    let next_before = tl.current().unwrap().next_at.unwrap();
    tl.set_interval(3600).unwrap();
    let cur = tl.current().unwrap();
    assert_eq!(cur.interval_s, 3600);
    // the image due keeps its time (an image may land between the two reads,
    // at 1 s apart): never an hour later
    let gap = cur.next_at.unwrap().duration_since(next_before).as_secs_f64();
    assert!((0.0..=1.0).contains(&gap), "next image moved by {gap} s");
    tl.shutdown();
}

fn event_kinds(fx: &Fixture, run: i64) -> Vec<String> {
    fx.store().events(Some(run), 100).unwrap().into_iter().rev().map(|e| e.kind).collect()
}

#[test]
fn camera_lost_counts_missed_and_recovers() {
    let fx = Fixture::new("camlost");
    let run = fx.store().insert_run(&crate::store::tests::sample_run()).unwrap();
    let cam = FakeCam::on();
    let tl = fx.spawn(cam.clone());
    let feed = tl.status_sink();
    let drive = |secs: u64| {
        for i in 0..secs * 10 {
            feed.send(driving(run, i as f64 * 0.1)).unwrap();
            std::thread::sleep(Duration::from_millis(100));
        }
    };
    feed.send(driving(run, 0.0)).unwrap();
    let id = tl.start_with_run(run, "Run".into(), 1).unwrap();
    drive(2);
    cam.set_connected(false);
    drive(3);
    assert_eq!(tl.current().unwrap().state, "waiting_camera");
    let lost_images = tl.current().unwrap().images;
    cam.set_connected(true);
    drive(2);
    let row = fx.store().timelapse(id).unwrap().unwrap();
    assert!(row.images > lost_images);
    assert!((2..=4).contains(&row.missed), "missed {}", row.missed);
    assert_eq!(tl.current().unwrap().state, "recording");
    let kinds = event_kinds(&fx, run);
    assert_eq!(kinds.iter().filter(|k| *k == "camera_lost").count(), 1, "{kinds:?}");
    assert_eq!(kinds.iter().filter(|k| *k == "camera_recovered").count(), 1, "{kinds:?}");
    tl.shutdown();
}

#[test]
fn folder_gone_pauses_then_resumes() {
    let fx = Fixture::new("foldergone");
    let tl = fx.spawn(FakeCam::on());
    let id = tl.start_standalone("Night".into(), 1, None).unwrap();
    assert!(wait_for(Duration::from_secs(3), || images(&tl) >= 1));
    // the captures folder gone, a file in its place: it cannot come back
    // (a USB disk unplugged)
    std::fs::remove_dir_all(&fx.root).unwrap();
    std::fs::write(&fx.root, b"not a folder").unwrap();
    assert!(wait_for(Duration::from_secs(3), || tl.current().unwrap().pause_reason.as_deref() == Some("folder")));
    assert_eq!(tl.current().unwrap().state, "paused");
    let paused_at = images(&tl);
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(images(&tl), paused_at);
    // the disk is back
    std::fs::remove_file(&fx.root).unwrap();
    assert!(wait_for(Duration::from_secs(3), || images(&tl) > paused_at), "{:?}", tl.current());
    let row = fx.store().timelapse(id).unwrap().unwrap();
    assert_eq!((row.state, row.pause_reason), (TimelapseState::Recording, None));
    assert_eq!(row.missed, 0, "a pause for the folder is not missed");
    let left: Vec<_> = std::fs::read_dir(&row.folder).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".tmp")).collect();
    assert!(left.is_empty());
    tl.shutdown();
}

static FREE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(u64::MAX);

#[test]
fn low_disk_pauses() {
    use std::sync::atomic::Ordering;
    let fx = Fixture::new("lowdisk");
    let root = fx.root.clone();
    let t = Timings { free_bytes: |_| Some(FREE.load(Ordering::SeqCst)), ..quick() };
    let tl = Timelapses::spawn_with(fx.store(), FakeCam::on(), move || root.clone(), t);
    tl.start_standalone("Night".into(), 1, None).unwrap();
    assert!(wait_for(Duration::from_secs(3), || images(&tl) >= 1));
    FREE.store(150_000_000, Ordering::SeqCst);
    assert!(wait_for(Duration::from_secs(3), || tl.current().unwrap().pause_reason.as_deref() == Some("disk")));
    let paused_at = images(&tl);
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(images(&tl), paused_at);
    FREE.store(5_000_000_000, Ordering::SeqCst);
    assert!(wait_for(Duration::from_secs(3), || images(&tl) > paused_at));
    assert_eq!(tl.current().unwrap().state, "recording");
    tl.shutdown();
}

#[test]
fn a_command_is_seen_in_current_as_soon_as_it_answers() {
    let fx = Fixture::new("atonce");
    let tl = fx.spawn(FakeCam::on());
    tl.start_standalone("Night".into(), 3600, None).unwrap();
    for i in 0..10 {
        tl.pause().unwrap();
        assert_eq!(tl.current().unwrap().state, "paused", "round {i}");
        tl.resume().unwrap();
        assert_eq!(tl.current().unwrap().state, "recording", "round {i}");
        tl.set_interval(100 + i).unwrap();
        assert_eq!(tl.current().unwrap().interval_s, 100 + i, "round {i}");
    }
    tl.stop().unwrap();
    assert!(tl.current().is_none());
    tl.shutdown();
}

#[test]
fn stop_after_ends_a_standalone_timelapse() {
    let fx = Fixture::new("stopafter");
    let tl = fx.spawn(FakeCam::on());
    let id = tl.start_standalone("Short".into(), 1, Some(2)).unwrap();
    assert!(wait_for(Duration::from_secs(5), || tl.current().is_none()));
    assert_eq!(fx.store().timelapse(id).unwrap().unwrap().state, TimelapseState::Ended);
    tl.shutdown();
}

#[test]
fn deleting_a_runs_timelapse_removes_its_folder() {
    let fx = Fixture::new("delete");
    let tl = fx.spawn(FakeCam::on());
    let feed = tl.status_sink();
    feed.send(driving(8, 0.0)).unwrap();
    let id = tl.start_with_run(8, "Run 8".into(), 60).unwrap();
    assert!(wait_for(Duration::from_secs(3), || images(&tl) >= 1));
    feed.send(gone()).unwrap();
    assert!(wait_for(Duration::from_secs(3), || tl.current().is_none()));
    let folder = PathBuf::from(fx.store().timelapse(id).unwrap().unwrap().folder);
    assert!(folder.is_dir());
    tl.delete_for_run(8).unwrap();
    assert!(!folder.exists());
    assert!(fx.store().timelapse(id).unwrap().is_none());
    tl.shutdown();
}
