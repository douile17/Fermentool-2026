//! `config.toml` - machine-local settings (`docs/IMPLEMENTATION_PLAN.md` §4.9).
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
    /// to a pump - or whose `config.toml` still carries the shipped
    /// `path = "sim"` - then refuses to run a cycle that would silently drive
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
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(path, cfg.to_toml()?)?;
                Ok(cfg)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Write the current config back to `path`.
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, self.to_toml()?)?;
        Ok(())
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
    fn grace_is_minutes_to_duration() {
        let mut cfg = Config::default();
        cfg.resume.grace_minutes = 5;
        assert_eq!(cfg.grace(), std::time::Duration::from_secs(300));
    }
}
