//! A timelapse on disk: one folder per timelapse under the captures folder,
//! its JPEGs, `thumbs/` beside them and `images.csv` (what each image was
//! taken with, for a model trained on them later).

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Under this much free space the Camera page warns.
pub const WARN_FREE_BYTES: u64 = 1_000_000_000;
/// Under this much free space a timelapse pauses itself: the journal needs
/// the disk more than the images do.
pub const PAUSE_FREE_BYTES: u64 = 200_000_000;

pub const CSV_HEADER: &str =
    "file,taken_at,elapsed_s,manual,camera,width,height,setpoint,unit,delivered_ml,weight_g,trim_c,controls";

/// The folder new timelapses go in: `captures_dir` (`[camera]`), or the data
/// dir's `captures` when it is empty.
pub fn captures_root(captures_dir: &str, data_dir: &Path) -> PathBuf {
    match captures_dir.trim() {
        "" => data_dir.join("captures"),
        dir => PathBuf::from(dir),
    }
}

/// A timelapse's folder: `"<started> <name> (<id>)"`, the characters Windows
/// refuses in a name replaced by `_`, the name cut to 60 characters.
pub fn folder_name(started_local: &str, name: &str, id: i64) -> String {
    let clean: String = name
        .chars()
        .map(|c| if c.is_control() || "<>:\"/\\|?*".contains(c) { '_' } else { c })
        .take(60)
        .collect();
    // Windows drops a trailing dot or space from a name
    let clean = clean.trim().trim_end_matches(['.', ' ']);
    format!("{started_local} {clean} ({id})")
}

/// `"000123_+012h30m00s.jpg"`, or by hand `"m0004_+012h31m10s.jpg"`: sorted
/// by slot, with the time since the start.
pub fn image_name(slot: Option<i64>, manual_n: i64, elapsed_s: f64) -> String {
    let s = elapsed_s.max(0.0).floor() as i64;
    let at = format!("+{:03}h{:02}m{:02}s", s / 3600, s / 60 % 60, s % 60);
    match slot {
        Some(k) => format!("{k:06}_{at}.jpg"),
        None => format!("m{manual_n:04}_{at}.jpg"),
    }
}

/// Creates `dir` if needed and checks a file can be written there.
pub fn ensure_writable(dir: &Path) -> Result<(), String> {
    let refused = |e: io::Error| format!("This folder cannot be written to: {e}");
    fs::create_dir_all(dir).map_err(refused)?;
    let probe = dir.join(".fermentool-probe.tmp");
    fs::write(&probe, b"ok").map_err(refused)?;
    let _ = fs::remove_file(&probe);
    Ok(())
}

/// Free bytes on the disk that holds `dir`, read at its nearest folder that
/// exists (Settings asks before the first timelapse made it); `None`
/// elsewhere than Windows or when Windows will not say.
#[cfg(windows)]
pub fn free_bytes(dir: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let existing = dir.ancestors().find(|p| p.is_dir())?;
    let wide: Vec<u16> = existing.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut free = 0u64;
    // SAFETY: `wide` is NUL-terminated and outlives the call; the two other
    // outputs may be null.
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, std::ptr::null_mut(), std::ptr::null_mut()) };
    (ok != 0).then_some(free)
}

#[cfg(not(windows))]
pub fn free_bytes(_dir: &Path) -> Option<u64> {
    None
}

/// The image and its thumbnail (under `thumbs/`), each written to a `.tmp`
/// then renamed: a failure leaves neither half-written file nor an image
/// without its thumbnail.
pub fn write_image(folder: &Path, name: &str, jpeg: &[u8], thumb: &[u8]) -> io::Result<()> {
    let thumbs = folder.join("thumbs");
    let (big, small) = (folder.join(name), thumbs.join(name));
    let (big_tmp, small_tmp) = (folder.join(format!("{name}.tmp")), thumbs.join(format!("{name}.tmp")));
    let result = (|| {
        fs::create_dir_all(&thumbs)?;
        fs::write(&small_tmp, thumb)?;
        fs::write(&big_tmp, jpeg)?;
        fs::rename(&small_tmp, &small)?;
        fs::rename(&big_tmp, &big)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&big_tmp);
        let _ = fs::remove_file(&small_tmp);
        if !big.exists() {
            let _ = fs::remove_file(&small);
        }
    }
    result
}

/// One line of `images.csv`.
#[derive(Debug, Clone)]
pub struct CsvRow {
    pub file: String,
    pub taken_at: String,
    pub elapsed_s: f64,
    pub manual: bool,
    pub camera: String,
    pub width: u32,
    pub height: u32,
    pub setpoint: Option<f64>,
    pub unit: Option<String>,
    pub delivered_ml: Option<f64>,
    pub weight_g: Option<f64>,
    pub trim_c: Option<f64>,
    /// JSON of the camera's picture controls.
    pub controls: String,
}

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn opt<T: ToString>(v: &Option<T>) -> String {
    v.as_ref().map(ToString::to_string).unwrap_or_default()
}

/// Appends `row` to the folder's `images.csv`, its header first when the
/// file is new.
pub fn append_csv(folder: &Path, row: &CsvRow) -> io::Result<()> {
    let path = folder.join("images.csv");
    let new = !path.exists();
    let mut f = fs::OpenOptions::new().create(true).append(true).open(&path)?;
    let mut line = String::new();
    if new {
        line.push_str(CSV_HEADER);
        line.push('\n');
    }
    let fields = [
        csv_field(&row.file),
        row.taken_at.clone(),
        row.elapsed_s.to_string(),
        row.manual.to_string(),
        csv_field(&row.camera),
        row.width.to_string(),
        row.height.to_string(),
        opt(&row.setpoint),
        csv_field(&opt(&row.unit)),
        opt(&row.delivered_ml),
        opt(&row.weight_g),
        opt(&row.trim_c),
        csv_field(&row.controls),
    ];
    line.push_str(&fields.join(","));
    line.push('\n');
    f.write_all(line.as_bytes())
}

/// Deletes a timelapse's folder and everything in it; one already gone is
/// fine.
pub fn remove_folder(dir: &Path) -> io::Result<()> {
    match fs::remove_dir_all(dir) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        r => r,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh empty folder under the temp dir.
    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ft-tl-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn row(file: &str) -> CsvRow {
        CsvRow {
            file: file.into(),
            taken_at: "2026-11-05T10:05:00Z".into(),
            elapsed_s: 300.0,
            manual: false,
            camera: "Integrated Webcam".into(),
            width: 1280,
            height: 720,
            setpoint: Some(2.5),
            unit: Some("ml_min".into()),
            delivered_ml: Some(12.25),
            weight_g: None,
            trim_c: Some(1.012),
            controls: r#"[{"control":"exposure","value":-6}]"#.into(),
        }
    }

    #[test]
    fn folder_names_are_safe_on_windows() {
        assert_eq!(folder_name("2026-11-05_1430", "Run 2/3: E. coli?", 7), "2026-11-05_1430 Run 2_3_ E. coli_ (7)");
        assert_eq!(folder_name("2026-11-05_1430", " dots. ", 8), "2026-11-05_1430 dots (8)");
        assert_eq!(folder_name("2026-11-05_1430", &"x".repeat(100), 9).len(), "2026-11-05_1430 ".len() + 60 + " (9)".len());
    }

    #[test]
    fn image_names_sort_by_slot() {
        assert_eq!(image_name(Some(123), 0, 45000.0), "000123_+012h30m00s.jpg");
        assert_eq!(image_name(None, 4, 45070.4), "m0004_+012h31m10s.jpg");
    }

    #[test]
    fn write_image_writes_both_and_leaves_no_tmp() {
        let d = scratch("write");
        write_image(&d, "000001_+000h05m00s.jpg", b"big", b"small").unwrap();
        assert_eq!(std::fs::read(d.join("000001_+000h05m00s.jpg")).unwrap(), b"big");
        assert_eq!(std::fs::read(d.join("thumbs").join("000001_+000h05m00s.jpg")).unwrap(), b"small");
        assert!(no_tmp(&d));
    }

    fn no_tmp(d: &Path) -> bool {
        let tmp = |p: &Path| std::fs::read_dir(p).map(|r| r.flatten().all(|e| !e.file_name().to_string_lossy().ends_with(".tmp")));
        tmp(d).unwrap_or(true) && tmp(&d.join("thumbs")).unwrap_or(true)
    }

    #[test]
    fn write_image_leaves_no_tmp_on_failure() {
        let d = scratch("fail");
        // `thumbs` is a file: the thumbnail cannot be written
        std::fs::write(d.join("thumbs"), b"x").unwrap();
        assert!(write_image(&d, "a.jpg", b"big", b"small").is_err());
        assert!(no_tmp(&d));
        assert!(!d.join("a.jpg").exists(), "an image without its thumbnail is not kept");
    }

    #[test]
    fn csv_header_once_and_quotes_json() {
        let d = scratch("csv");
        append_csv(&d, &row("a.jpg")).unwrap();
        append_csv(&d, &CsvRow { manual: true, ..row("b.jpg") }).unwrap();
        let text = std::fs::read_to_string(d.join("images.csv")).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], CSV_HEADER);
        assert_eq!(
            lines[1],
            r#"a.jpg,2026-11-05T10:05:00Z,300,false,Integrated Webcam,1280,720,2.5,ml_min,12.25,,1.012,"[{""control"":""exposure"",""value"":-6}]""#
        );
        assert!(lines[2].starts_with("b.jpg,2026-11-05T10:05:00Z,300,true,"));
    }

    #[test]
    fn ensure_writable_creates_and_refuses_a_file_path() {
        let d = scratch("writable");
        assert!(ensure_writable(&d.join("new").join("deeper")).is_ok());
        assert!(d.join("new").join("deeper").is_dir());
        std::fs::write(d.join("file"), b"x").unwrap();
        let e = ensure_writable(&d.join("file")).unwrap_err();
        assert!(e.starts_with("This folder cannot be written to"), "{e}");
    }

    #[test]
    fn remove_folder_is_fine_when_missing() {
        let d = scratch("remove");
        write_image(&d, "a.jpg", b"1", b"2").unwrap();
        remove_folder(&d).unwrap();
        assert!(!d.exists());
        remove_folder(&d).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn free_bytes_of_the_temp_dir_is_some() {
        assert!(free_bytes(&std::env::temp_dir()).is_some_and(|b| b > 0));
        assert!(free_bytes(Path::new("Q:\\surely\\not\\here")).is_none());
    }

    #[cfg(windows)]
    #[test]
    fn free_bytes_of_a_folder_not_made_yet_reads_its_disk() {
        // Settings asks before the first timelapse made the captures folder
        let missing = std::env::temp_dir().join("ft-not-made-yet").join("captures");
        assert!(!missing.exists());
        assert!(free_bytes(&missing).is_some_and(|b| b > 0));
    }
}
