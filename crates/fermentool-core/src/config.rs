//! `config.toml`, machine-local settings (`docs/IMPLEMENTATION_PLAN.md` §4.9).
//!
//! Every field has a default, so a missing or partial file still loads. On first
//! run the file is written out with the defaults.

use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug)]
pub enum ConfigError {
    Io(std::io::Error),
    Parse(toml::de::Error),
    Serialize(toml::ser::Error),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Io(e) => write!(f, "config I/O: {e}"),
            ConfigError::Parse(e) => write!(f, "config parse: {e}"),
            ConfigError::Serialize(e) => write!(f, "config write: {e}"),
        }
    }
}
impl std::error::Error for ConfigError {}
impl From<std::io::Error> for ConfigError {
    fn from(e: std::io::Error) -> Self {
        ConfigError::Io(e)
    }
}
impl From<toml::de::Error> for ConfigError {
    fn from(e: toml::de::Error) -> Self {
        ConfigError::Parse(e)
    }
}
impl From<toml::ser::Error> for ConfigError {
    fn from(e: toml::ser::Error) -> Self {
        ConfigError::Serialize(e)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Local web UI / API port (bound to 127.0.0.1 only).
    pub port: u16,
    pub serial: SerialConfig,
    pub pump: PumpConfig,
    pub resume: ResumeConfig,
    pub storage: StorageConfig,
    pub log: LogConfig,
    pub scale: ScaleConfig,
    pub notify: NotifyConfig,
}

/// Notifications: who can run experiments, each with their own channel, so a
/// run's alerts reach the person who started it only. The usual channel is
/// ntfy (a phone app, one secret topic per person); a Teams Workflow webhook
/// works too where the organisation allows it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NotifyConfig {
    pub people: Vec<Person>,
    /// The ntfy server: the public one, or a lab-hosted instance.
    pub ntfy_server: String,
}

impl Default for NotifyConfig {
    fn default() -> Self {
        Self { people: Vec::new(), ntfy_server: "https://ntfy.sh".into() }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Person {
    pub name: String,
    /// Their ntfy topic. A secret: anyone who knows it can read and post.
    pub ntfy_topic: String,
    /// A Teams Workflow webhook URL, optional. A secret too.
    pub webhook: String,
}

/// An ntfy topic must be hard to guess (it is the only access control on a
/// public server) and made of the characters ntfy accepts.
pub const NTFY_TOPIC_MIN_LEN: usize = 12;

impl NotifyConfig {
    /// The person called `name` (case-insensitive, trimmed).
    pub fn person(&self, name: &str) -> Option<&Person> {
        let name = name.trim();
        self.people.iter().find(|p| p.name.trim().eq_ignore_ascii_case(name))
    }

    /// What a save checks: names set and distinct, each person reachable
    /// (an ntfy topic, a Teams webhook, or both), topics well formed and not
    /// shared, webhooks and the server https.
    pub fn validate(&self) -> Result<(), String> {
        if !self.ntfy_server.trim().starts_with("https://") {
            return Err("the ntfy server must be an https:// URL".into());
        }
        for (i, p) in self.people.iter().enumerate() {
            let name = p.name.trim();
            if name.is_empty() {
                return Err("every person needs a name".into());
            }
            if self.people[..i].iter().any(|q| q.name.trim().eq_ignore_ascii_case(name)) {
                return Err(format!("\"{name}\" is listed twice"));
            }
            let (topic, hook) = (p.ntfy_topic.trim(), p.webhook.trim());
            if topic.is_empty() && hook.is_empty() {
                return Err(format!("{name} needs an ntfy topic (or a Teams webhook)"));
            }
            if !topic.is_empty() {
                if topic.len() < NTFY_TOPIC_MIN_LEN
                    || !topic.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                {
                    return Err(format!(
                        "{name}'s ntfy topic must be at least {NTFY_TOPIC_MIN_LEN} letters, digits, - or _ \
                         (use Generate)"
                    ));
                }
                if self.people[..i].iter().any(|q| q.ntfy_topic.trim() == topic) {
                    return Err(format!("{name}'s ntfy topic is already someone else's"));
                }
            }
            if !hook.is_empty() && !hook.starts_with("https://") {
                return Err(format!("{name}'s webhook must be an https:// URL"));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SerialConfig {
    /// Serial device path, or the literal `"sim"` to use the pump simulator.
    pub path: String,
    /// 1200 | 2400 | 4800 | 9600. Frame is fixed at 8E1 by the LabQ pump.
    pub baud: u32,
    /// Permit starting / resuming a run while the engine is on the pump
    /// simulator (no real port). Off by default: a machine that was never wired
    /// to a pump, or whose `config.toml` still carries the shipped
    /// `path = "sim"`, then refuses to run a cycle that would silently drive
    /// nothing. Turn it on for bench testing against the simulator.
    #[serde(default)]
    pub allow_simulator: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PumpConfig {
    /// MODBUS slave address, 1..247.
    pub address: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ResumeConfig {
    /// Ask before resuming an interrupted run.
    pub prompt: bool,
    /// A run is still "resumable" this long past its planned end.
    pub grace_minutes: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StorageConfig {
    /// Empty = OS default data directory.
    pub dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ScaleConfig {
    /// Serial device path. Empty = no scale configured; automatic
    /// (gravimetric-trim) runs are unavailable until this is set.
    pub path: String,
    /// Whatever baud the scale's own Communications menu is set to.
    pub baud: u32,
    /// Feed density (g/mL), used to convert a measured mass rate to a volume
    /// rate. Water ~= 1.0; a 500 g/L glucose feed ~= 1.18.
    pub density_g_per_ml: f64,
    /// What the balance weighs: the feed bottle (its weight falls as the
    /// pump draws) or the receiving vessel (its weight rises).
    pub position: ScalePosition,
    /// How far the gravimetric trim may push the pump, ± percent of its
    /// setpoint, before it alarms instead. 25 = c in [0.80, 1.25]. Wider
    /// tolerates a worse-calibrated tube, but hides a slipping tube, a leak
    /// or a bad reading for longer.
    pub trim_limit_pct: f64,
}

/// The bauds the LabQ pump offers.
pub const SERIAL_BAUDS: [u32; 4] = [1200, 2400, 4800, 9600];

/// Bounds accepted for `trim_limit_pct`.
pub const TRIM_LIMIT_PCT_MIN: f64 = 5.0;
pub const TRIM_LIMIT_PCT_MAX: f64 = 100.0;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScalePosition {
    /// Under the feed bottle. Delivered mass = weight lost.
    #[default]
    Feed,
    /// Under the receiving vessel. Delivered mass = weight gained; it also
    /// counts anything else added to the vessel (base, antifoam) and misses
    /// what leaves it (samples, evaporation).
    Receiver,
}

impl ScalePosition {
    /// Sign that turns a balance reading into a feed-bottle-equivalent
    /// weight, falling as feed is delivered, which is what the trim expects.
    pub fn sign(self) -> f64 {
        match self {
            ScalePosition::Feed => 1.0,
            ScalePosition::Receiver => -1.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LogConfig {
    /// Empty = `<storage>/logs`.
    pub dir: String,
    /// error | warn | info | debug | trace (or a full `RUST_LOG`-style filter).
    pub level: String,
    pub retain_days: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            port: 8730,
            serial: SerialConfig::default(),
            pump: PumpConfig::default(),
            resume: ResumeConfig::default(),
            storage: StorageConfig::default(),
            log: LogConfig::default(),
            scale: ScaleConfig::default(),
            notify: NotifyConfig::default(),
        }
    }
}

impl Default for SerialConfig {
    fn default() -> Self {
        Self {
            path: "sim".into(),
            baud: 9600,
            allow_simulator: false,
        }
    }
}

impl SerialConfig {
    /// `true` when this points at the pump simulator rather than a real port.
    pub fn use_simulator(&self) -> bool {
        self.path.eq_ignore_ascii_case("sim") || self.path.is_empty()
    }
}

impl Default for ScaleConfig {
    fn default() -> Self {
        Self {
            path: String::new(),
            baud: 9600,
            density_g_per_ml: 1.0,
            position: ScalePosition::Feed,
            trim_limit_pct: 25.0,
        }
    }
}

impl ScaleConfig {
    pub fn configured(&self) -> bool {
        !self.path.trim().is_empty()
    }
}

impl Default for PumpConfig {
    fn default() -> Self {
        Self { address: 1 }
    }
}

impl Default for ResumeConfig {
    fn default() -> Self {
        Self {
            prompt: true,
            grace_minutes: 5,
        }
    }
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            dir: String::new(),
            level: "info".into(),
            retain_days: 30,
        }
    }
}

impl Config {
    /// Parse `toml`, filling any missing field with its default.
    pub fn from_toml(toml: &str) -> Result<Self, ConfigError> {
        Ok(toml::from_str(toml)?)
    }

    /// Render to a pretty TOML string.
    pub fn to_toml(&self) -> Result<String, ConfigError> {
        Ok(toml::to_string_pretty(self)?)
    }

    /// Load `path`, or create it with the defaults if it doesn't exist.
    pub fn load_or_create(path: &Path) -> Result<Self, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::from_toml(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let cfg = Config::default();
                cfg.save(path)?;
                Ok(cfg)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// [`load_or_create`](Self::load_or_create) for the daemon's boot, which
    /// must come up whatever the file holds: a daemon that refuses to start
    /// on a damaged `config.toml` cannot resume the run it was driving, and
    /// has no window to say why. A file that does not parse is copied aside
    /// (`config.toml.bad`) and the defaults are used; values out of range are
    /// put back to their defaults ([`sanitize`](Self::sanitize)). Returns the
    /// config and what was wrong with the file, if anything.
    pub fn load_for_boot(path: &Path) -> (Self, Vec<String>) {
        let mut problems = Vec::new();
        let mut cfg = match Self::load_or_create(path) {
            Ok(c) => c,
            Err(e) => {
                let aside = path.with_extension("toml.bad");
                let _ = std::fs::copy(path, &aside);
                problems.push(format!(
                    "{} could not be read ({e}); running on the defaults, the file is kept as {}",
                    path.display(),
                    aside.display()
                ));
                Config::default()
            }
        };
        problems.extend(cfg.sanitize());
        (cfg, problems)
    }

    /// Write the current config back to `path`, atomically: written to a
    /// temporary file, flushed to disk, then renamed over the old one. A power
    /// cut during a save leaves the old file or the new one, never half of
    /// one.
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        use std::io::Write;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("toml.tmp");
        {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(self.to_toml()?.as_bytes())?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// What a save from the API checks beyond the notifications: every value
    /// the daemon uses as a number in range.
    pub fn validate(&self) -> Result<(), String> {
        if self.port == 0 {
            return Err("the API port must be 1..65535".into());
        }
        if !SERIAL_BAUDS.contains(&self.serial.baud) {
            return Err(format!("the pump baud must be one of {SERIAL_BAUDS:?}"));
        }
        if !(1..=247).contains(&self.pump.address) {
            return Err("the MODBUS address must be 1..=247".into());
        }
        if self.scale.baud == 0 {
            return Err("the balance baud must be a positive number".into());
        }
        if !(self.scale.density_g_per_ml.is_finite() && self.scale.density_g_per_ml > 0.0) {
            return Err("liquid density must be a positive number (g/mL)".into());
        }
        let lim = self.scale.trim_limit_pct;
        if !(lim.is_finite() && (TRIM_LIMIT_PCT_MIN..=TRIM_LIMIT_PCT_MAX).contains(&lim)) {
            return Err(format!(
                "correction limit must be between {TRIM_LIMIT_PCT_MIN} and {TRIM_LIMIT_PCT_MAX} %"
            ));
        }
        if tracing_subscriber::EnvFilter::try_new(&self.log.level).is_err() {
            return Err(format!("\"{}\" is not a log level", self.log.level));
        }
        self.notify.validate()
    }

    /// Put every out-of-range value back to its default, saying which. For a
    /// file edited by hand: [`validate`](Self::validate) refuses the same
    /// values when they come through the API.
    pub fn sanitize(&mut self) -> Vec<String> {
        let d = Config::default();
        let mut fixed = Vec::new();
        let mut note = |what: &str| fixed.push(format!("config.toml: {what} out of range, default used"));
        if self.port == 0 {
            self.port = d.port;
            note("port");
        }
        if !SERIAL_BAUDS.contains(&self.serial.baud) {
            self.serial.baud = d.serial.baud;
            note("serial.baud");
        }
        if !(1..=247).contains(&self.pump.address) {
            self.pump.address = d.pump.address;
            note("pump.address");
        }
        if self.scale.baud == 0 {
            self.scale.baud = d.scale.baud;
            note("scale.baud");
        }
        if !(self.scale.density_g_per_ml.is_finite() && self.scale.density_g_per_ml > 0.0) {
            self.scale.density_g_per_ml = d.scale.density_g_per_ml;
            note("scale.density_g_per_ml");
        }
        let lim = self.scale.trim_limit_pct;
        if !(lim.is_finite() && (TRIM_LIMIT_PCT_MIN..=TRIM_LIMIT_PCT_MAX).contains(&lim)) {
            self.scale.trim_limit_pct = d.scale.trim_limit_pct;
            note("scale.trim_limit_pct");
        }
        if tracing_subscriber::EnvFilter::try_new(&self.log.level).is_err() {
            self.log.level = d.log.level.clone();
            note("log.level");
        }
        fixed
    }

    pub fn grace(&self) -> std::time::Duration {
        std::time::Duration::from_secs(u64::from(self.resume.grace_minutes) * 60)
    }

    /// `true` when the pump simulator should be used instead of a real port.
    pub fn use_simulator(&self) -> bool {
        self.serial.use_simulator()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_round_trips_through_toml() {
        let a = Config::default();
        let b = Config::from_toml(&a.to_toml().unwrap()).unwrap();
        assert_eq!(a.port, b.port);
        assert_eq!(a.serial.path, b.serial.path);
        assert_eq!(a.resume.grace_minutes, b.resume.grace_minutes);
    }

    #[test]
    fn partial_toml_fills_defaults() {
        let cfg = Config::from_toml("port = 9001\n[serial]\npath = \"COM7\"\n").unwrap();
        assert_eq!(cfg.port, 9001);
        assert_eq!(cfg.serial.path, "COM7");
        assert_eq!(cfg.serial.baud, 9600); // default
        assert_eq!(cfg.pump.address, 1); // default
        assert!(cfg.resume.prompt); // default
    }

    #[test]
    fn allow_simulator_defaults_off_for_new_and_pre_existing_configs() {
        // Fresh defaults.
        assert!(!Config::default().serial.allow_simulator);
        // A config.toml written before this field existed (the shipped
        // `path = "sim"` default) must not silently permit simulator runs.
        let cfg = Config::from_toml("[serial]\npath = \"sim\"\nbaud = 9600\n").unwrap();
        assert!(!cfg.serial.allow_simulator);
    }

    #[test]
    fn empty_toml_is_all_defaults() {
        let cfg = Config::from_toml("").unwrap();
        assert_eq!(cfg.port, Config::default().port);
        assert!(cfg.use_simulator());
    }

    #[test]
    fn load_or_create_writes_a_missing_file() {
        let dir = std::env::temp_dir().join(format!("fermentool-cfg-{}", std::process::id()));
        let path = dir.join("config.toml");
        let _ = std::fs::remove_dir_all(&dir);

        let cfg = Config::load_or_create(&path).unwrap();
        assert_eq!(cfg.port, 8730);
        assert!(path.exists());

        // second load reads the file back
        let again = Config::load_or_create(&path).unwrap();
        assert_eq!(again.port, 8730);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_save_replaces_the_file_whole() {
        let dir = std::env::temp_dir().join(format!("fermentool-cfg-save-{}", std::process::id()));
        let path = dir.join("config.toml");
        let _ = std::fs::remove_dir_all(&dir);
        let mut cfg = Config::default();
        cfg.save(&path).unwrap();
        cfg.port = 9100;
        cfg.save(&path).unwrap();
        assert_eq!(Config::load_or_create(&path).unwrap().port, 9100);
        assert!(!path.with_extension("toml.tmp").exists(), "no temporary file left");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_damaged_file_still_boots_on_the_defaults() {
        let dir = std::env::temp_dir().join(format!("fermentool-cfg-bad-{}", std::process::id()));
        let path = dir.join("config.toml");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, "port = 87\n[serial]\npath = \"CO").unwrap(); // cut mid-write
        let (cfg, problems) = Config::load_for_boot(&path);
        assert_eq!(cfg.port, 8730);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(path.with_extension("toml.bad").exists(), "the damaged file is kept");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn values_out_of_range_go_back_to_their_defaults() {
        let mut cfg = Config::from_toml(
            "port = 0\n[pump]\naddress = 0\n[serial]\nbaud = 1234\n\
             [scale]\ndensity_g_per_ml = 0.0\ntrim_limit_pct = 500.0\n[log]\nlevel = \"loud=,=\"\n",
        )
        .unwrap();
        assert!(cfg.validate().is_err());
        assert_eq!(cfg.sanitize().len(), 6);
        assert_eq!(cfg.pump.address, 1);
        assert_eq!(cfg.scale.density_g_per_ml, 1.0);
        assert_eq!(cfg.scale.trim_limit_pct, 25.0);
        assert!(cfg.validate().is_ok());
        assert!(Config::default().validate().is_ok());
    }

    #[test]
    fn grace_is_minutes_to_duration() {
        let mut cfg = Config::default();
        cfg.resume.grace_minutes = 5;
        assert_eq!(cfg.grace(), std::time::Duration::from_secs(300));
    }

    #[test]
    fn scale_defaults_to_unconfigured() {
        let cfg = Config::default();
        assert_eq!(cfg.scale.path, "");
        assert!(!cfg.scale.configured());
        assert_eq!(cfg.scale.baud, 9600);
        assert_eq!(cfg.scale.density_g_per_ml, 1.0);
    }

    #[test]
    fn scale_config_round_trips_through_toml() {
        let toml = "[scale]\npath = \"COM5\"\nbaud = 9600\ndensity_g_per_ml = 1.18\n";
        let cfg = Config::from_toml(toml).unwrap();
        assert!(cfg.scale.configured());
        assert_eq!(cfg.scale.path, "COM5");
        assert_eq!(cfg.scale.density_g_per_ml, 1.18);
        // A file written before `position` existed means the feed bottle,
        // and one before `trim_limit_pct` the historical ±25 %.
        assert_eq!(cfg.scale.position, ScalePosition::Feed);
        assert_eq!(cfg.scale.trim_limit_pct, 25.0);
    }

    #[test]
    fn notify_people_round_trip_and_validate() {
        let toml = "[[notify.people]]\nname = \"Drew\"\nwebhook = \"https://example.invalid/hook\"\n";
        let cfg = Config::from_toml(toml).unwrap();
        assert_eq!(cfg.notify.person(" drew ").map(|p| p.name.as_str()), Some("Drew"));
        assert!(cfg.notify.validate().is_ok());
        let back = Config::from_toml(&cfg.to_toml().unwrap()).unwrap();
        assert_eq!(back.notify.people, cfg.notify.people);
        // A file from before has nobody.
        assert!(Config::from_toml("port = 8730\n").unwrap().notify.people.is_empty());

        let person = |name: &str, topic: &str, hook: &str| Person {
            name: name.into(),
            ntfy_topic: topic.into(),
            webhook: hook.into(),
        };
        let with = |people: Vec<Person>| NotifyConfig { people, ..NotifyConfig::default() };
        let mut bad = cfg.notify.clone();
        bad.people.push(person("DREW", "", "https://x"));
        assert!(bad.validate().is_err(), "duplicate name");
        assert!(with(vec![person("A", "", "http://x")]).validate().is_err(), "not https");
        assert!(with(vec![person("A", "", "")]).validate().is_err(), "unreachable");
        // ntfy alone is enough; a short or odd topic is not.
        assert!(with(vec![person("A", "fermentool-a-7x92kq", "")]).validate().is_ok());
        assert!(with(vec![person("A", "andrew", "")]).validate().is_err(), "guessable");
        assert!(with(vec![person("A", "fermentool a 7x92kq", "")]).validate().is_err(), "space");
        let shared = with(vec![person("A", "fermentool-shared-1", ""), person("B", "fermentool-shared-1", "")]);
        assert!(shared.validate().is_err(), "shared topic");
        assert_eq!(NotifyConfig::default().ntfy_server, "https://ntfy.sh");
    }

    #[test]
    fn scale_position_round_trips_through_toml() {
        let cfg = Config::from_toml("[scale]\npath = \"COM7\"\nposition = \"receiver\"\n").unwrap();
        assert_eq!(cfg.scale.position, ScalePosition::Receiver);
        let back = Config::from_toml(&cfg.to_toml().unwrap()).unwrap();
        assert_eq!(back.scale.position, ScalePosition::Receiver);
    }
}
