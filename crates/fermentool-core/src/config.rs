//! `config.toml`, machine-local settings (`docs/IMPLEMENTATION_PLAN.md` §4.9).
//!
//! Every field has a default, so a missing or partial file still loads. On first
//! run the file is written out with the defaults.

use std::path::Path;

use fermentool_modbus::{PumpKind, PumpModel, PumpTarget};
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
    pub refill: RefillConfig,
    pub notify: NotifyConfig,
    pub display: DisplayConfig,
}

/// How the UI shows things, the same for everyone on this PC (the browser
/// and the desktop window read it from here, not from their own storage).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplayConfig {
    /// Tube sizes: `"mm"` (the inches beside them, in grey) or `"in"` (the
    /// other way round). The calibration's diameter entry starts in it too.
    pub tube_sizes: String,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self { tube_sizes: "mm".into() }
    }
}

/// The values `display.tube_sizes` takes.
pub const TUBE_SIZE_UNITS: [&str; 2] = ["mm", "in"];

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
    /// MODBUS slave address, 1..247 on a LabQ, 1..32 on a LabV / PDS II.
    pub address: u8,
    /// Which pump model answers at `address`.
    pub model: PumpModelCfg,
    /// PDS II only: the head (1..=8) this config drives. Ignored otherwise.
    pub head: u8,
    // No max rpm: it is the model's (`PumpModel::max_rpm`). A `max_rpm` left
    // in an older config.toml is ignored.
}

/// The pump model as written in `config.toml` (the modbus crate has no serde).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum PumpModelCfg {
    #[default]
    Labq,
    Labv,
    Pds2,
}

impl PumpModelCfg {
    pub fn model(self) -> PumpModel {
        match self {
            PumpModelCfg::Labq => PumpModel::LabQ,
            PumpModelCfg::Labv => PumpModel::LabV,
            PumpModelCfg::Pds2 => PumpModel::PdsII,
        }
    }
}

/// The pump a config describes. An invalid PDS II head (validation refuses
/// it, but an unsanitised config can still carry one) falls back to head 1
/// so this never panics.
fn build_target(address: u8, model: PumpModelCfg, head: u8) -> PumpTarget {
    let kind = PumpKind::new(model.model(), head)
        .or_else(|_| PumpKind::new(model.model(), 1))
        .unwrap_or(PumpKind::LabQ);
    PumpTarget { address, kind }
}

/// Problem with one pump's address and head, if any. `who` is the prefix of
/// the message, e.g. "the refill pump's".
fn pump_problem(who: &str, address: u8, model: PumpModelCfg, head: u8) -> Option<String> {
    let m = model.model();
    if !(1..=m.max_address()).contains(&address) {
        return Some(format!("{who} MODBUS address must be 1..={} on a {}", m.max_address(), m.label()));
    }
    if model == PumpModelCfg::Pds2 && !(1..=8).contains(&head) {
        return Some(format!("{who} PDS II head must be 1..=8"));
    }
    None
}

/// [`Config::sanitize`] for one pump: each field out of range for `model`
/// back to its default (`default_address`, head 1), the model itself never
/// touched. Returns `section.field` of each.
fn sanitize_pump(
    section: &str,
    fields: [&str; 2],
    address: &mut u8,
    default_address: u8,
    model: PumpModelCfg,
    head: &mut u8,
) -> Vec<String> {
    let m = model.model();
    let mut fixed = Vec::new();
    if !(1..=m.max_address()).contains(address) {
        *address = default_address;
        fixed.push(format!("{section}.{}", fields[0]));
    }
    if model == PumpModelCfg::Pds2 && !(1..=8).contains(head) {
        *head = 1;
        fixed.push(format!("{section}.{}", fields[1]));
    }
    fixed
}

/// Same physical pump: same address, and on a PDS II (one address, several
/// heads) the same head.
pub fn same_pump(a: &PumpTarget, b: &PumpTarget) -> bool {
    if a.address != b.address {
        return false;
    }
    match (a.kind, b.kind) {
        (PumpKind::PdsII { head: x }, PumpKind::PdsII { head: y }) => x == y,
        _ => true,
    }
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

/// The refill pump: a second LabQ on the pump's RS-485 line, at its own
/// MODBUS address, that tops the weighed feed bottle up from the main
/// reservoir (`docs/superpowers/specs/2026-10-02-auto-refill-design.md`).
/// Only its speed, direction and start/stop are ever written, and only for a
/// run that chose automatic refill in New run (or a Test): otherwise nothing
/// is sent to `pump_address`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RefillConfig {
    /// MODBUS slave address, 1..247, not the feed pump's (checked when a run
    /// or a Test uses it: a config with the default 2 on a feed pump at 2
    /// must still save).
    pub pump_address: u8,
    /// Which pump model the refill pump is.
    pub pump_model: PumpModelCfg,
    /// PDS II only: the head (1..=8).
    pub pump_head: u8,
    /// Speed the refill pump runs at, rpm (0.1..its model's max).
    pub speed_rpm: f64,
    /// The direction that pumps from the reservoir into the weighed bottle.
    /// No expected flow: the refill pump is not calibrated, a fill is judged
    /// by the balance alone (`refill_cycle`). An `expected_ml_min` left in
    /// an older config.toml is ignored.
    pub direction: Direction,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    /// Clockwise (the LabQ's direction register at 1).
    #[default]
    Cw,
    Ccw,
}

impl Direction {
    pub fn clockwise(self) -> bool {
        self == Direction::Cw
    }
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
            refill: RefillConfig::default(),
            notify: NotifyConfig::default(),
            display: DisplayConfig::default(),
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

impl Default for RefillConfig {
    fn default() -> Self {
        Self {
            pump_address: 2,
            pump_model: PumpModelCfg::Labq,
            pump_head: 1,
            speed_rpm: 350.0,
            direction: Direction::Cw,
        }
    }
}

/// Lower bound of the refill pump's speed; the upper one is
/// [`RefillConfig::max_rpm`].
pub const REFILL_RPM_MIN: f64 = 0.1;

impl RefillConfig {
    /// The refill pump's top speed, its model's.
    pub fn max_rpm(&self) -> f64 {
        self.pump_model.model().max_rpm()
    }

    pub fn target(&self) -> PumpTarget {
        build_target(self.pump_address, self.pump_model, self.pump_head)
    }

    /// What [`Config::validate`] refuses in `[refill]`.
    fn problem(&self) -> Option<String> {
        let pump = pump_problem(
            "the refill pump's",
            self.pump_address,
            self.pump_model,
            self.pump_head,
        );
        if pump.is_some() {
            return pump;
        }
        let max = self.max_rpm();
        if !self.speed_rpm.is_finite() || self.speed_rpm < REFILL_RPM_MIN {
            return Some(format!("the refill pump's speed must be {REFILL_RPM_MIN}..{max} rpm"));
        }
        if self.speed_rpm > max {
            return Some(format!(
                "the refill pump's speed {} rpm is over its max {max} rpm",
                self.speed_rpm
            ));
        }
        None
    }
}

impl PumpConfig {
    /// The feed pump's top speed, its model's.
    pub fn max_rpm(&self) -> f64 {
        self.model.model().max_rpm()
    }

    pub fn target(&self) -> PumpTarget {
        build_target(self.address, self.model, self.head)
    }

    pub(crate) fn problem(&self) -> Option<String> {
        pump_problem("the feed pump's", self.address, self.model, self.head)
    }
}

impl Default for PumpConfig {
    fn default() -> Self {
        Self { address: 1, model: PumpModelCfg::Labq, head: 1 }
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
        if let Some(p) = self.pump.problem() {
            return Err(p);
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
        if !TUBE_SIZE_UNITS.contains(&self.display.tube_sizes.as_str()) {
            return Err(format!("tube sizes must be one of {TUBE_SIZE_UNITS:?}"));
        }
        if let Some(p) = self.refill.problem() {
            return Err(p);
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
        // Field by field, never the model: a LabV or PDS II reset to a LabQ
        // would be driven through the wrong register map.
        for what in sanitize_pump(
            "pump",
            ["address", "head"],
            &mut self.pump.address,
            d.pump.address,
            self.pump.model,
            &mut self.pump.head,
        ) {
            note(&what);
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
        if !TUBE_SIZE_UNITS.contains(&self.display.tube_sizes.as_str()) {
            self.display.tube_sizes = d.display.tube_sizes.clone();
            note("display.tube_sizes");
        }
        for what in sanitize_pump(
            "refill",
            ["pump_address", "pump_head"],
            &mut self.refill.pump_address,
            d.refill.pump_address,
            self.refill.pump_model,
            &mut self.refill.pump_head,
        ) {
            note(&what);
        }
        let max = self.refill.max_rpm();
        let speed = self.refill.speed_rpm;
        if !(speed.is_finite() && (REFILL_RPM_MIN..=max).contains(&speed)) {
            self.refill.speed_rpm = max;
            fixed.push(format!(
                "config.toml: refill.speed_rpm out of range, the refill pump's max ({max} rpm) used"
            ));
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
    fn tube_sizes_default_to_mm_and_refuse_other_units() {
        let mut c = Config::from_toml("").unwrap();
        assert_eq!(c.display.tube_sizes, "mm", "an older config.toml has no [display]");
        c.display.tube_sizes = "in".into();
        assert!(c.validate().is_ok());
        c.display.tube_sizes = "cm".into();
        assert!(c.validate().is_err());
        assert_eq!(c.sanitize(), vec!["config.toml: display.tube_sizes out of range, default used".to_string()]);
        assert_eq!(c.display.tube_sizes, "mm");
    }

    #[test]
    fn the_refill_pump_defaults_to_address_2_and_checks_its_ranges() {
        let mut c = Config::from_toml("").unwrap();
        assert_eq!(c.refill.pump_address, 2, "an older config.toml has no [refill]");
        assert_eq!(c.refill.direction, Direction::Cw);
        // A feed pump at 2 must not make every other save fail.
        c.pump.address = 2;
        assert!(c.validate().is_ok());
        c.refill.pump_address = 0;
        assert!(c.validate().is_err());
        c.refill.pump_address = 3;
        c.refill.speed_rpm = 400.0;
        assert!(c.validate().is_err());
        c.refill.speed_rpm = 200.0;
        c.refill.direction = Direction::Ccw;
        assert!(c.validate().is_ok());
        let back = Config::from_toml(&c.to_toml().unwrap()).unwrap();
        assert_eq!(back.refill.direction, Direction::Ccw);
    }

    fn pump_cfg(model: PumpModelCfg, address: u8) -> PumpConfig {
        PumpConfig { address, model, ..PumpConfig::default() }
    }

    #[test]
    fn an_old_config_is_labq_and_labq() {
        let c = Config::from_toml("[pump]\naddress = 1\n").unwrap();
        assert_eq!(c.pump.model, PumpModelCfg::Labq);
        assert_eq!(c.pump.head, 1);
        assert_eq!(c.pump.max_rpm(), 350.0);
        assert_eq!(c.refill.pump_model, PumpModelCfg::Labq);
        assert_eq!(c.refill.pump_head, 1);
        assert_eq!(c.refill.max_rpm(), 350.0);
    }

    #[test]
    fn max_rpm_is_the_model_s() {
        let mut c = Config::default();
        c.pump.model = PumpModelCfg::Labv;
        assert_eq!(c.pump.max_rpm(), 150.0);
        c.pump.model = PumpModelCfg::Pds2;
        assert_eq!(c.pump.max_rpm(), 600.0);
        c.refill.pump_model = PumpModelCfg::Labv;
        assert_eq!(c.refill.max_rpm(), 150.0);
        // A max_rpm left in an older config.toml is ignored, never applied.
        let c = Config::from_toml(
            "[pump]\naddress = 1\nmodel = \"labv\"\nmax_rpm = 100\n\n[refill]\npump_max_rpm = 50\n",
        )
        .unwrap();
        assert_eq!((c.pump.max_rpm(), c.refill.max_rpm()), (150.0, 350.0));
        assert!(c.validate().is_ok());
    }

    #[test]
    fn addresses_follow_the_model() {
        let mut c = Config::default();
        c.pump = pump_cfg(PumpModelCfg::Labq, 200);
        assert!(c.validate().is_ok());
        c.pump = pump_cfg(PumpModelCfg::Labv, 33);
        assert!(c.validate().is_err());
        c.pump = pump_cfg(PumpModelCfg::Pds2, 3);
        c.pump.head = 9;
        assert!(c.validate().is_err());
        assert_eq!(c.sanitize(), vec!["config.toml: pump.head out of range, default used".to_string()]);
        assert_eq!((c.pump.model, c.pump.head, c.pump.address), (PumpModelCfg::Pds2, 1, 3), "never the model");
    }

    #[test]
    fn sanitize_resets_the_bad_field_and_keeps_the_model() {
        let mut c = Config::default();
        c.pump = PumpConfig { address: 40, model: PumpModelCfg::Pds2, head: 2 };
        assert_eq!(c.sanitize(), vec!["config.toml: pump.address out of range, default used".to_string()]);
        assert_eq!(c.pump.model, PumpModelCfg::Pds2);
        assert_eq!((c.pump.address, c.pump.head), (1, 2));
        assert!(c.validate().is_ok());

        // A LabV refill pump at 40 (over its 32): the address, not the model.
        c.refill = RefillConfig { pump_address: 40, pump_model: PumpModelCfg::Labv, speed_rpm: 100.0, ..RefillConfig::default() };
        assert_eq!(c.sanitize(), vec!["config.toml: refill.pump_address out of range, default used".to_string()]);
        assert_eq!((c.refill.pump_model, c.refill.pump_address, c.refill.speed_rpm), (PumpModelCfg::Labv, 2, 100.0));
    }

    #[test]
    fn same_pump_rule() {
        let t = |m: PumpModelCfg, a: u8, h: u8| PumpConfig { address: a, model: m, head: h }.target();
        assert!(same_pump(&t(PumpModelCfg::Labq, 2, 1), &t(PumpModelCfg::Labq, 2, 1)));
        assert!(same_pump(&t(PumpModelCfg::Labv, 2, 1), &t(PumpModelCfg::Labq, 2, 1)));
        assert!(!same_pump(&t(PumpModelCfg::Pds2, 3, 1), &t(PumpModelCfg::Pds2, 3, 2)));
        assert!(same_pump(&t(PumpModelCfg::Pds2, 3, 1), &t(PumpModelCfg::Pds2, 3, 1)));
    }

    #[test]
    fn refill_speed_over_its_max_is_refused() {
        let mut c = Config::default();
        c.refill.pump_model = PumpModelCfg::Labv;
        c.refill.speed_rpm = 300.0;
        let e = c.validate().unwrap_err();
        assert!(e.contains("300") && e.contains("150"), "{e}");
        assert_eq!(
            c.sanitize(),
            vec!["config.toml: refill.speed_rpm out of range, the refill pump's max (150 rpm) used".to_string()]
        );
        assert_eq!(c.refill.pump_model, PumpModelCfg::Labv, "never the model");
        assert_eq!(c.refill.speed_rpm, 150.0);
        assert!(c.validate().is_ok());
    }

    #[test]
    fn target_never_panics_on_a_bad_head() {
        let mut p = pump_cfg(PumpModelCfg::Pds2, 3);
        p.head = 0;
        assert_eq!(p.target().kind, PumpKind::PdsII { head: 1 });
    }

    #[test]
    fn a_hand_edited_refill_out_of_range_gets_the_defaults() {
        // `enabled` from the first version of [refill] is ignored.
        let mut c = Config::from_toml("[refill]\nenabled = true\npump_address = 0\n").unwrap();
        assert_eq!(c.sanitize().len(), 1);
        assert_eq!(c.refill.pump_address, 2);
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

    /// The documented example must stay a file the daemon takes as it is.
    #[test]
    fn the_example_config_parses_and_validates() {
        let cfg = Config::from_toml(include_str!("../../../config.example.toml")).unwrap();
        assert!(cfg.validate().is_ok(), "{:?}", cfg.validate());
        let mut fixed = cfg.clone();
        assert!(fixed.sanitize().is_empty());
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
