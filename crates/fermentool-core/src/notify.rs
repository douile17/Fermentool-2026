//! Notifications for the person responsible for a run.
//!
//! A thread of its own follows the journal's `events` table (through its own
//! SQLite connection) and sends the ones that matter to the run's owner, on
//! the channels configured for them (`[notify]`): their ntfy topic (a phone
//! app, push even at night) and/or a Teams Workflow webhook. Nothing here
//! runs on the control thread: a slow or unreachable service can delay a
//! message, never a pump write. Working off the journal means nothing is
//! lost across a restart either: a cursor in `app_state` says how far it got.
//!
//! An alarm is not said once and forgotten: one notification is easily lost
//! among a phone's others, so it becomes a [`Page`] sent again on ntfy every
//! [`REPEAT`] until someone acknowledges it (the notification's own button,
//! or the UI), its condition clears (`alarm_cleared`, `scale_recovered`...)
//! or the run stops. The button publishes to the person's `<topic>-ack`
//! topic, which this thread polls: the phone never has to reach this PC, and
//! nobody has a setting to change on it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::RwLock;

use crate::config::{Config, Person};
use crate::store::{EventLevel, EventRow, NewEvent, RunKind, RunRow, RunStatus, Store};

/// `app_state` key: id of the last event handled.
const CURSOR_KEY: &str = "notify_cursor";
const POLL: Duration = Duration::from_secs(5);
const SEND_TIMEOUT: Duration = Duration::from_secs(15);
/// Waits between attempts; past the last, the message is dropped (logged).
const RETRY_WAITS: [Duration; 3] = [Duration::from_secs(5), Duration::from_secs(30), Duration::from_secs(120)];
/// A link that drops and comes back is said at most once per this per run.
const LINK_QUIET: Duration = Duration::from_secs(15 * 60);
/// An unacknowledged alarm is sent again this often.
pub const REPEAT: Duration = Duration::from_secs(3 * 60);
/// Settings' "Test alarm": quicker, and it gives up by itself.
const TEST_REPEAT: Duration = Duration::from_secs(60);
const TEST_SENDS: u32 = 5;
/// How often an ack topic is asked for new acknowledgements while one of
/// that person's alarms rings (ntfy.sh allows about one request per 5 s).
const ACK_POLL: Duration = Duration::from_secs(15);
/// Slack between ntfy's clock and this PC's when matching an ack to an alarm.
const ACK_SKEW_S: i64 = 60;
/// The `run_id` of Settings' test alarm (no run has it).
pub const TEST_RUN: i64 = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Alarm,
    Good,
    Info,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub tone: Tone,
    pub title: String,
}

/// What a journal event tells the run's owner, if anything worth a message.
pub fn message_for(e: &EventRow, run: &RunRow) -> Option<Message> {
    use Tone::*;
    let (tone, title) = match e.kind.as_str() {
        "start" => (Info, "Run started".to_string()),
        "alarm_feed_stopped" => (Alarm, "Feed stopped: bottle empty or line blocked".into()),
        "alarm_saturated" => (Alarm, "Pump out of the correction limit".into()),
        "alarm_wrong_side" => (Alarm, "Balance reads the wrong way for its position".into()),
        "alarm_cleared" => (Good, "Feed flowing again, regulation resumed".into()),
        "scale_lost" => (Alarm, "Balance not answering".into()),
        "scale_recovered" => (Good, "Balance answering again".into()),
        "serial_lost" => (Alarm, "Pump link lost".into()),
        "serial_recovered" => (Good, "Pump link back".into()),
        "readback_mismatch" => (Alarm, "Pump not on its setpoint".into()),
        "journal_stalled" => (Alarm, "Journal not writing (disk full?)".into()),
        "crash_detected" => (Alarm, "Fermentool was interrupted during the run".into()),
        "resume" => (Good, "Run resumed".into()),
        "refill" => (Info, "Bottle refilled".into()),
        "trim_limit" => (Info, "Correction limit changed".into()),
        "curve_done" => (Info, "Curve finished, holding its end value until Stop".into()),
        "stop" => match run.status {
            RunStatus::Completed => (Good, "Run completed".into()),
            _ => (Info, "Run stopped".into()),
        },
        "abort" => (Alarm, "Run aborted".into()),
        "stop_pending" => (Alarm, "The Stop did not reach the pump: it may still be running".into()),
        "pump_stopped" => (Good, "Pump stopped".into()),
        _ => return None,
    };
    Some(Message { tone, title })
}

/// Which ongoing condition an alarm event belongs to, for the alarms that
/// ring until acknowledged. A run has at most one page per family; the event
/// that ends the condition clears it ([`clears`]).
pub fn family_of(kind: &str) -> Option<&'static str> {
    match kind {
        "alarm_feed_stopped" | "alarm_saturated" | "alarm_wrong_side" => Some("trim"),
        "scale_lost" => Some("scale"),
        "serial_lost" => Some("serial"),
        "readback_mismatch" => Some("readback"),
        "journal_stalled" => Some("journal"),
        "crash_detected" => Some("crash"),
        "stop_pending" => Some("stop"),
        // "abort" ends the run: said once, nothing goes on to repeat.
        _ => None,
    }
}

/// The family an event ends; `Some(None)`: every page of the run.
pub fn clears(kind: &str) -> Option<Option<&'static str>> {
    Some(match kind {
        "alarm_cleared" => Some("trim"),
        "scale_recovered" => Some("scale"),
        "serial_recovered" => Some("serial"),
        "readback_ok" => Some("readback"),
        "resume" => Some("crash"),
        "pump_stopped" => Some("stop"),
        "stop" | "abort" => None,
        _ => return None,
    })
}

/// A message ready for any channel: title, the run and time, the detail.
#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub tone: Tone,
    pub title: String,
    pub context: String,
    pub detail: Option<String>,
}

/// The note for a journal event of `run`.
pub fn note_for(msg: &Message, run: &RunRow, e: &EventRow) -> Note {
    let when = e
        .wall_time
        .to_zoned(jiff::tz::TimeZone::system())
        .strftime("%d/%m %H:%M")
        .to_string();
    Note {
        tone: msg.tone,
        title: msg.title.clone(),
        context: format!("Run \"{}\" (#{}) \u{b7} {when}", run.name, run.id),
        detail: e.detail.clone().filter(|d| !d.is_empty()),
    }
}

/// The note for Settings' "Test".
pub fn test_note(name: &str) -> Note {
    Note {
        tone: Tone::Good,
        title: "Fermentool test notification".into(),
        context: format!("{name}: the alerts of runs you are responsible for will arrive here."),
        detail: None,
    }
}

/// Where one person's notes go.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    Ntfy { server: String, topic: String },
    Teams { webhook: String },
}

impl Target {
    pub fn label(&self) -> &'static str {
        match self {
            Target::Ntfy { .. } => "ntfy",
            Target::Teams { .. } => "Teams",
        }
    }
}

/// Every channel configured for `p`.
pub fn targets(p: &Person, ntfy_server: &str) -> Vec<Target> {
    let mut v = Vec::new();
    if !p.ntfy_topic.trim().is_empty() {
        v.push(Target::Ntfy {
            server: ntfy_server.trim().to_string(),
            topic: p.ntfy_topic.trim().to_string(),
        });
    }
    if !p.webhook.trim().is_empty() {
        v.push(Target::Teams { webhook: p.webhook.trim().to_string() });
    }
    v
}

/// ntfy's JSON publish body (posted to the server root). Alarms at the
/// highest priority, so the phone rings even when silenced for the night if
/// the user allows it; routine news low, so it does not.
pub fn ntfy_body(topic: &str, n: &Note) -> Value {
    ntfy_body_with(topic, n, None)
}

/// The person's acknowledgement topic, next to their own.
pub fn ack_topic(topic: &str) -> String {
    format!("{}-ack", topic.trim())
}

/// [`ntfy_body`], plus an "Acknowledge" button when the note is a ringing
/// alarm, `ack` being (server, run id). The button posts `ack <run id>` to
/// the ack topic straight from the phone and dismisses the notification.
pub fn ntfy_body_with(topic: &str, n: &Note, ack: Option<(&str, i64)>) -> Value {
    let (priority, tag) = match n.tone {
        Tone::Alarm => (5, "warning"),
        Tone::Good => (3, "white_check_mark"),
        Tone::Info => (2, "information_source"),
    };
    let mut message = n.context.clone();
    if let Some(d) = &n.detail {
        message.push('\n');
        message.push_str(d);
    }
    let mut body = json!({ "topic": topic, "title": n.title, "message": message, "priority": priority, "tags": [tag] });
    if let Some((server, run_id)) = ack {
        body["actions"] = json!([{
            "action": "http",
            "label": "Acknowledge",
            "url": format!("{}/{}", server.trim().trim_end_matches('/'), ack_topic(topic)),
            "method": "POST",
            "body": format!("ack {run_id}"),
            "clear": true,
        }]);
    }
    body
}

/// The Teams Workflow webhook body: one Adaptive Card.
pub fn teams_card(n: &Note) -> Value {
    let (icon, color) = match n.tone {
        Tone::Alarm => ("\u{26a0}\u{fe0f}", "Attention"),
        Tone::Good => ("\u{2705}", "Good"),
        Tone::Info => ("\u{2139}\u{fe0f}", "Default"),
    };
    let mut body = vec![
        json!({ "type": "TextBlock", "text": format!("{icon} {}", n.title),
                "weight": "Bolder", "size": "Medium", "color": color, "wrap": true }),
        json!({ "type": "TextBlock", "text": format!("Fermentool \u{b7} {}", n.context),
                "isSubtle": true, "spacing": "None", "wrap": true }),
    ];
    if let Some(d) = &n.detail {
        body.push(json!({ "type": "TextBlock", "text": d, "wrap": true }));
    }
    json!({
        "type": "message",
        "attachments": [{
            "contentType": "application/vnd.microsoft.card.adaptive",
            "contentUrl": null,
            "content": {
                "$schema": "http://adaptivecards.io/schemas/adaptive-card.json",
                "type": "AdaptiveCard",
                "version": "1.4",
                "body": body,
            },
        }],
    })
}

/// Send one note to one target, bounded by `SEND_TIMEOUT`.
pub fn deliver(t: &Target, n: &Note) -> Result<(), String> {
    deliver_with(t, n, None)
}

/// [`deliver`]; on ntfy, with the "Acknowledge" button for run `ack`.
pub fn deliver_with(t: &Target, n: &Note, ack: Option<i64>) -> Result<(), String> {
    let (url, body) = match t {
        Target::Ntfy { server, topic } => (
            server.trim_end_matches('/').to_string(),
            ntfy_body_with(topic, n, ack.map(|id| (server.as_str(), id))),
        ),
        Target::Teams { webhook } => (webhook.clone(), teams_card(n)),
    };
    post_json(&url, &body).map_err(|e| format!("{}: {e}", t.label()))
}

fn post_json(url: &str, body: &Value) -> Result<(), String> {
    ureq::post(url.trim())
        .timeout(SEND_TIMEOUT)
        .send_json(body.clone())
        .map(|_| ())
        .map_err(|e| match e {
            ureq::Error::Status(code, _) => format!("the service answered HTTP {code}"),
            other => other.to_string(),
        })
}

/// An alarm that rings until acknowledged. Shared with the API, which lists
/// them and acknowledges from the UI: it only sets `acked_by`, the notifier
/// journals the ack and drops the page on its next pass.
#[derive(Debug, Clone, Serialize)]
pub struct Page {
    pub run_id: i64,
    pub family: &'static str,
    pub title: String,
    pub responsible: String,
    pub raised_at: jiff::Timestamp,
    /// How many times it has gone out so far.
    pub sent: u32,
    #[serde(skip)]
    pub acked_by: Option<String>,
    #[serde(skip)]
    note: Note,
    #[serde(skip)]
    ntfy: Target,
    #[serde(skip)]
    next_at: Instant,
    #[serde(skip)]
    every: Duration,
    /// Past this many sends it stops by itself (`None`: until acknowledged).
    #[serde(skip)]
    max_sends: Option<u32>,
}

pub type Pages = Arc<Mutex<Vec<Page>>>;

/// Settings' "Test alarm": a page for nobody's run, sent at once, then every
/// minute until acknowledged (at most [`TEST_SENDS`] times).
pub fn test_page(name: &str, ntfy: Target) -> Page {
    Page {
        run_id: TEST_RUN,
        family: "test",
        title: "Fermentool test alarm".into(),
        responsible: name.to_string(),
        raised_at: jiff::Timestamp::now(),
        sent: 0,
        acked_by: None,
        note: Note {
            tone: Tone::Alarm,
            title: "Fermentool test alarm".into(),
            context: format!("{name}: a real alarm comes back like this until you press Acknowledge."),
            detail: None,
        },
        ntfy,
        next_at: Instant::now(),
        every: TEST_REPEAT,
        max_sends: Some(TEST_SENDS),
    }
}

/// Drop the pages an event ends (see [`clears`]). The end of a run ends
/// every page of it but a Stop still pending: that one is about the pump
/// left running after the run, and rings until the Stop gets through.
pub fn apply_clears(pages: &mut Vec<Page>, run_id: i64, kind: &str) {
    if let Some(family) = clears(kind) {
        pages.retain(|p| {
            p.run_id != run_id
                || match family {
                    Some(f) => f != p.family,
                    None => p.family == "stop",
                }
        });
    }
}

/// Mark acknowledged every page of `run_id` raised before the ack (`at`, unix
/// seconds on ntfy's clock). Returns whether any was.
pub fn apply_ack(pages: &mut [Page], run_id: i64, at: i64, by: &str) -> bool {
    let mut hit = false;
    for p in pages.iter_mut().filter(|p| p.run_id == run_id && p.acked_by.is_none()) {
        if p.raised_at.as_second() - ACK_SKEW_S <= at {
            p.acked_by = Some(by.to_string());
            hit = true;
        }
    }
    hit
}

/// The acks in an ntfy poll answer (newline-delimited JSON): (run id, time).
pub fn parse_acks(body: &str) -> Vec<(i64, i64)> {
    body.lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|v| v["event"] == "message")
        .filter_map(|v| {
            let id = v["message"].as_str()?.trim().strip_prefix("ack")?.trim().parse().ok()?;
            Some((id, v["time"].as_i64()?))
        })
        .collect()
}

/// The messages posted to `topic` since `since` (unix seconds).
fn poll_topic(server: &str, topic: &str, since: i64) -> Result<String, String> {
    let url = format!("{}/{topic}/json?poll=1&since={since}", server.trim().trim_end_matches('/'));
    ureq::get(&url)
        .timeout(SEND_TIMEOUT)
        .call()
        .map_err(|e| e.to_string())?
        .into_string()
        .map_err(|e| e.to_string())
}

/// Start the notifier. It never returns an error to the caller: a database it
/// cannot open is retried, a webhook that fails is logged and skipped.
pub fn spawn(db: PathBuf, config: Arc<RwLock<Config>>, pages: Pages) -> std::io::Result<JoinHandle<()>> {
    thread::Builder::new()
        .name("notifier".into())
        .spawn(move || {
            let store = loop {
                match Store::open(&db) {
                    Ok(s) => break s,
                    Err(e) => {
                        tracing::warn!("notifier cannot open the journal ({e}), retrying");
                        thread::sleep(Duration::from_secs(30));
                    }
                }
            };
            let mut n = Notifier::new(store, config, pages);
            loop {
                thread::sleep(POLL);
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| n.pass())).is_err() {
                    tracing::error!("panic in the notifier; it carries on");
                }
            }
        })
}

struct Notifier {
    store: Store,
    config: Arc<RwLock<Config>>,
    cursor: i64,
    /// (run id, "scale"/"serial") -> last time a link message went out.
    link_said: HashMap<(i64, &'static str), Instant>,
    pages: Pages,
    /// Ack topic -> last time it was polled.
    ack_polled: HashMap<String, Instant>,
}

impl Notifier {
    fn new(store: Store, config: Arc<RwLock<Config>>, pages: Pages) -> Self {
        // First start: begin from now, never replay the whole history.
        let cursor = match store.get_state(CURSOR_KEY).ok().flatten().and_then(|v| v.parse().ok()) {
            Some(c) => c,
            None => {
                let c = store.last_event_id().unwrap_or(0);
                let _ = store.set_state(CURSOR_KEY, &c.to_string());
                c
            }
        };
        Self { store, config, cursor, link_said: HashMap::new(), pages, ack_polled: HashMap::new() }
    }

    /// Handle every event since the cursor, then move it past them.
    fn pass(&mut self) {
        let Ok(events) = self.store.events_after(self.cursor, 200) else {
            return;
        };
        for e in events {
            self.handle(&e);
            self.cursor = e.id;
            let _ = self.store.set_state(CURSOR_KEY, &e.id.to_string());
        }
        self.poll_acks();
        self.ring();
    }

    fn pages(&self) -> std::sync::MutexGuard<'_, Vec<Page>> {
        self.pages.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Ask each ringing person's ack topic for the acks posted since their
    /// oldest alarm, at most every [`ACK_POLL`]. A failed poll is retried on
    /// a later round; the alarm keeps ringing meanwhile.
    fn poll_acks(&mut self) {
        let mut topics: HashMap<String, (String, String, i64)> = HashMap::new();
        for p in self.pages().iter().filter(|p| p.acked_by.is_none()) {
            if let Target::Ntfy { server, topic } = &p.ntfy {
                let since = p.raised_at.as_second() - ACK_SKEW_S;
                let e = topics.entry(ack_topic(topic)).or_insert((server.clone(), p.responsible.clone(), since));
                e.2 = e.2.min(since);
            }
        }
        for (topic, (server, who, since)) in topics {
            if self.ack_polled.get(&topic).is_some_and(|at| at.elapsed() < ACK_POLL) {
                continue;
            }
            self.ack_polled.insert(topic.clone(), Instant::now());
            match poll_topic(&server, &topic, since) {
                Ok(body) => {
                    let by = format!("{who} (phone)");
                    let mut pages = self.pages();
                    for (run_id, at) in parse_acks(&body) {
                        apply_ack(&mut pages, run_id, at, &by);
                    }
                }
                Err(e) => tracing::warn!("polling {topic} for acknowledgements: {e}"),
            }
        }
    }

    /// Journal and drop the acknowledged pages, drop the spent ones, and send
    /// again those whose time has come (ntfy only, one attempt: the next
    /// round is the retry).
    fn ring(&mut self) {
        let mut acked = Vec::new();
        let mut due = Vec::new();
        {
            let mut pages = self.pages();
            pages.retain(|p| match &p.acked_by {
                Some(by) => {
                    acked.push((p.run_id, p.title.clone(), by.clone()));
                    false
                }
                None => p.max_sends.is_none_or(|m| p.sent < m),
            });
            let now = Instant::now();
            for p in pages.iter_mut().filter(|p| p.next_at <= now) {
                p.sent += 1;
                p.next_at = now + p.every;
                let mut note = p.note.clone();
                if p.sent > 1 {
                    note.title = format!("Reminder {}: {}", p.sent, note.title);
                }
                due.push((p.ntfy.clone(), note, p.run_id, p.responsible.clone()));
            }
        }
        for (run_id, title, by) in acked {
            tracing::info!("run {run_id}: \"{title}\" acknowledged by {by}");
            if run_id != TEST_RUN {
                let _ = self.store.log_event(&NewEvent {
                    run_id: Some(run_id),
                    wall_time: jiff::Timestamp::now(),
                    level: EventLevel::Info,
                    kind: "alarm_ack".into(),
                    detail: Some(format!("{title}, acknowledged by {by}")),
                });
            }
        }
        for (t, note, run_id, who) in due {
            if let Err(err) = deliver_with(&t, &note, Some(run_id)) {
                tracing::warn!("reminding {who} ({}): {err}", note.title);
            }
        }
    }

    fn handle(&mut self, e: &EventRow) {
        let Some(run_id) = e.run_id else {
            return; // idle-time events: nobody's run
        };
        // Before any early return: `readback_ok` is no message of its own.
        apply_clears(&mut self.pages(), run_id, &e.kind);
        let Ok(Some(run)) = self.store.run(run_id) else {
            return;
        };
        if run.kind == RunKind::Calibration {
            return; // short bursts with someone at the bench
        }
        let Some(responsible) = run.responsible.as_deref() else {
            return;
        };
        let Some(msg) = message_for(e, &run) else {
            return;
        };
        // A flapping link is not said again within LINK_QUIET, but a loss
        // still rings: its first message is then its first reminder.
        let mut quiet = false;
        if let Some(link) = link_of(&e.kind) {
            let key = (run_id, link);
            if self.link_said.get(&key).is_some_and(|t| t.elapsed() < LINK_QUIET) {
                quiet = true;
            } else {
                self.link_said.insert(key, Instant::now());
            }
        }
        let targets = {
            let cfg = self.config.blocking_read();
            cfg.notify.person(responsible).map(|p| targets(p, &cfg.notify.ntfy_server))
        };
        let Some(targets) = targets.filter(|t| !t.is_empty()) else {
            tracing::warn!("run {run_id}: \"{responsible}\" has no channel in Settings; not notified");
            return;
        };
        let note = note_for(&msg, &run, e);
        let family = if msg.tone == Tone::Alarm { family_of(&e.kind) } else { None };
        let ntfy = targets.iter().find(|t| matches!(t, Target::Ntfy { .. })).cloned();
        if let (Some(family), Some(ntfy)) = (family, ntfy) {
            let mut pages = self.pages();
            pages.retain(|p| !(p.run_id == run_id && p.family == family));
            pages.push(Page {
                run_id,
                family,
                title: note.title.clone(),
                responsible: responsible.to_string(),
                raised_at: e.wall_time,
                sent: u32::from(!quiet),
                acked_by: None,
                note: note.clone(),
                ntfy,
                next_at: Instant::now() + REPEAT,
                every: REPEAT,
                max_sends: None,
            });
        }
        if quiet {
            return;
        }
        let ack = family.map(|_| run_id);
        for t in &targets {
            let mut sent = false;
            for (attempt, wait) in std::iter::once(None).chain(RETRY_WAITS.iter().map(Some)).enumerate() {
                if let Some(w) = wait {
                    thread::sleep(*w);
                }
                match deliver_with(t, &note, ack) {
                    Ok(()) => {
                        sent = true;
                        break;
                    }
                    Err(err) => tracing::warn!(attempt, "notifying {responsible} ({}): {err}", e.kind),
                }
            }
            if !sent {
                tracing::error!("gave up notifying {responsible} of {} on {} (run {run_id})", e.kind, t.label());
            }
        }
    }
}

/// Link up/down events, rate-limited so a flapping cable does not flood.
fn link_of(kind: &str) -> Option<&'static str> {
    match kind {
        "scale_lost" | "scale_recovered" => Some("scale"),
        "serial_lost" | "serial_recovered" => Some("serial"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{EventLevel, NewEvent};
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;

    fn run_row(status: RunStatus) -> RunRow {
        let s = Store::open_in_memory().unwrap();
        let id = s
            .insert_run(&crate::store::NewRun {
                name: "fed-batch E. coli".into(),
                started_at: jiff::Timestamp::now(),
                control_var: crate::store::ControlVar::MlMin,
                direction: crate::store::Direction::Cw,
                tick_interval_s: 1,
                pump_addr: 1,
                app_version: "test".into(),
                curve: fermentool_curves::CurveSpec::linear(1.0, 1.0, Duration::from_secs(3600)),
                gravimetric_trim: true,
                kind: RunKind::Dosing,
                tubing_calibration_id: None,
                responsible: Some("Drew".into()),
            })
            .unwrap();
        s.finish_run(id, status, jiff::Timestamp::now()).ok();
        s.run(id).unwrap().unwrap()
    }

    fn event(kind: &str, detail: Option<&str>) -> EventRow {
        EventRow {
            id: 1,
            run_id: Some(1),
            wall_time: jiff::Timestamp::now(),
            level: EventLevel::Info,
            kind: kind.into(),
            detail: detail.map(Into::into),
        }
    }

    #[test]
    fn the_events_that_matter_become_messages() {
        let run = run_row(RunStatus::Running);
        let m = message_for(&event("alarm_feed_stopped", None), &run).unwrap();
        assert_eq!(m.tone, Tone::Alarm);
        assert!(m.title.contains("bottle empty"));
        assert_eq!(message_for(&event("alarm_cleared", None), &run).unwrap().tone, Tone::Good);
        // Routine journal lines are not messages.
        for quiet in ["write_fail", "readback_ok", "readback_fail", "trim_start", "trim_ratio", "pump_stop"] {
            assert!(message_for(&event(quiet, None), &run).is_none(), "{quiet}");
        }
        let done = run_row(RunStatus::Completed);
        assert_eq!(message_for(&event("stop", None), &done).unwrap().title, "Run completed");
    }

    #[test]
    fn the_note_reads_the_same_on_ntfy_and_teams() {
        let run = run_row(RunStatus::Running);
        let e = event("alarm_feed_stopped", Some("1.2 g commanded in 180 s"));
        let note = note_for(&message_for(&e, &run).unwrap(), &run, &e);
        let card = teams_card(&note).to_string();
        assert!(card.contains("AdaptiveCard") && card.contains("Attention"));
        assert!(card.contains("bottle empty") && card.contains(&run.name) && card.contains("180 s"));
        let push = ntfy_body("fermentool-drew-x7k2", &note);
        assert_eq!(push["topic"], "fermentool-drew-x7k2");
        assert_eq!(push["priority"], 5, "an alarm rings");
        assert!(push["title"].as_str().unwrap().contains("bottle empty"));
        let msg = push["message"].as_str().unwrap();
        assert!(msg.contains(&run.name) && msg.contains("180 s"));
        let info = note_for(&message_for(&event("refill", None), &run).unwrap(), &run, &e);
        assert_eq!(ntfy_body("t", &info)["priority"], 2, "routine news stays quiet");
    }

    #[test]
    fn a_person_is_reached_on_every_channel_they_set() {
        let p = Person { name: "A".into(), ntfy_topic: " fermentool-a-123456 ".into(), webhook: String::new() };
        assert_eq!(
            targets(&p, "https://ntfy.sh"),
            vec![Target::Ntfy { server: "https://ntfy.sh".into(), topic: "fermentool-a-123456".into() }]
        );
        let both = Person { webhook: "https://hook".into(), ..p };
        assert_eq!(targets(&both, "https://ntfy.sh").len(), 2);
    }

    #[test]
    fn the_cursor_starts_at_now_not_at_the_whole_history() {
        let s = Store::open_in_memory().unwrap();
        for _ in 0..3 {
            s.log_event(&NewEvent {
                run_id: None,
                wall_time: jiff::Timestamp::now(),
                level: EventLevel::Info,
                kind: "start".into(),
                detail: None,
            })
            .unwrap();
        }
        let n = Notifier::new(s, Arc::new(RwLock::new(Config::default())), Pages::default());
        assert_eq!(n.cursor, 3);
        assert_eq!(n.store.get_state(CURSOR_KEY).unwrap().as_deref(), Some("3"));
    }

    /// A one-shot local HTTP server standing in for ntfy or Teams: returns
    /// its URL and a handle yielding (request line, body).
    fn one_shot_server() -> (String, thread::JoinHandle<(String, String)>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(sock.try_clone().unwrap());
            let mut request = String::new();
            reader.read_line(&mut request).unwrap();
            let mut len = 0usize;
            let mut line = String::new();
            loop {
                line.clear();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap();
                }
            }
            let mut body = vec![0u8; len];
            reader.read_exact(&mut body).unwrap();
            sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n").unwrap();
            (request, String::from_utf8(body).unwrap())
        });
        (url, server)
    }

    #[test]
    fn ntfy_gets_a_json_publish_at_the_server_root() {
        let (url, server) = one_shot_server();
        let t = Target::Ntfy { server: format!("{url}/"), topic: "fermentool-drew-x7k2".into() };
        deliver(&t, &test_note("Drew")).unwrap();
        let (request, body) = server.join().unwrap();
        assert!(request.starts_with("POST / "), "{request}");
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["topic"], "fermentool-drew-x7k2");
        assert!(v["message"].as_str().unwrap().contains("Drew"));
    }

    #[test]
    fn teams_gets_the_card_on_its_webhook() {
        let (url, server) = one_shot_server();
        let t = Target::Teams { webhook: format!("{url}/hook") };
        deliver(&t, &test_note("Drew")).unwrap();
        let (request, body) = server.join().unwrap();
        assert!(request.starts_with("POST /hook "), "{request}");
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["attachments"][0]["content"]["type"], "AdaptiveCard");
    }

    #[test]
    fn a_dead_service_is_an_error_not_a_hang() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        drop(listener); // nothing listens: connection refused
        let t = Instant::now();
        let r = deliver(&Target::Ntfy { server: url, topic: "fermentool-x-123456".into() }, &test_note("x"));
        assert!(r.unwrap_err().starts_with("ntfy:"));
        assert!(t.elapsed() < SEND_TIMEOUT + Duration::from_secs(5));
    }

    fn page(run_id: i64, family: &'static str) -> Page {
        let ntfy = Target::Ntfy { server: "https://ntfy.sh".into(), topic: "fermentool-a-123456".into() };
        Page { run_id, family, every: REPEAT, max_sends: None, ..test_page("A", ntfy) }
    }

    #[test]
    fn ringing_alarms_and_what_ends_them() {
        assert_eq!(family_of("alarm_feed_stopped"), Some("trim"));
        assert_eq!(family_of("alarm_saturated"), Some("trim"));
        assert_eq!(family_of("scale_lost"), Some("scale"));
        assert_eq!(family_of("abort"), None, "said once");
        assert_eq!(family_of("refill"), None);
        let mut pages = vec![page(7, "trim"), page(7, "scale"), page(8, "trim")];
        apply_clears(&mut pages, 7, "alarm_cleared");
        let left: Vec<_> = pages.iter().map(|p| (p.run_id, p.family)).collect();
        assert_eq!(left, [(7, "scale"), (8, "trim")]);
        apply_clears(&mut pages, 7, "refill");
        assert_eq!(pages.len(), 2, "unrelated events leave them");
        apply_clears(&mut pages, 7, "stop");
        let left: Vec<_> = pages.iter().map(|p| p.run_id).collect();
        assert_eq!(left, [8], "stop ends every page of its run only");
        // A Stop that did not reach the pump outlives the run's end.
        let mut pages = vec![page(7, "stop"), page(7, "trim")];
        apply_clears(&mut pages, 7, "stop");
        assert_eq!(pages.iter().map(|p| p.family).collect::<Vec<_>>(), ["stop"]);
        apply_clears(&mut pages, 7, "pump_stopped");
        assert!(pages.is_empty());
    }

    #[test]
    fn an_ack_reaches_the_alarms_raised_before_it_only() {
        let mut pages = vec![page(7, "trim"), page(8, "trim")];
        let raised = pages[0].raised_at.as_second();
        assert!(!apply_ack(&mut pages, 7, raised - 3600, "A (phone)"), "an old ack is not this alarm's");
        assert!(apply_ack(&mut pages, 7, raised + 5, "A (phone)"));
        assert_eq!(pages[0].acked_by.as_deref(), Some("A (phone)"));
        assert!(pages[1].acked_by.is_none(), "another run keeps ringing");
    }

    #[test]
    fn acks_are_read_from_the_ntfy_poll_answer() {
        let body = concat!(
            r#"{"id":"a","time":1700000000,"event":"open","topic":"t-ack"}"#,
            "\n",
            r#"{"id":"b","time":1700000010,"event":"message","topic":"t-ack","message":"ack 51"}"#,
            "\n",
            r#"{"id":"c","time":1700000020,"event":"message","topic":"t-ack","message":"hello"}"#,
            "\nnot json\n",
        );
        assert_eq!(parse_acks(body), [(51, 1700000010)]);
    }

    #[test]
    fn a_ringing_alarm_carries_the_acknowledge_button() {
        let run = run_row(RunStatus::Running);
        let e = event("alarm_feed_stopped", None);
        let note = note_for(&message_for(&e, &run).unwrap(), &run, &e);
        let v = ntfy_body_with("fermentool-a-123456", &note, Some(("https://ntfy.sh/", 51)));
        let a = &v["actions"][0];
        assert_eq!(a["action"], "http");
        assert_eq!(a["url"], "https://ntfy.sh/fermentool-a-123456-ack");
        assert_eq!(a["body"], "ack 51");
        let posted = json!({ "event": "message", "time": 1, "message": a["body"] }).to_string();
        assert_eq!(parse_acks(&posted), [(51, 1)], "what the button posts is what the poll reads");
        assert!(ntfy_body("t", &note).get("actions").is_none(), "plain news has no button");
    }

    #[test]
    fn a_page_repeats_then_an_ack_journals_and_stops_it() {
        let (url, server) = one_shot_server();
        let s = Store::open_in_memory().unwrap();
        let pages = Pages::default();
        let mut n = Notifier::new(s, Arc::new(RwLock::new(Config::default())), Arc::clone(&pages));
        let ntfy = Target::Ntfy { server: url, topic: "fermentool-a-123456".into() };
        pages.lock().unwrap().push(test_page("A", ntfy));
        n.ring();
        let (_, body) = server.join().unwrap();
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["title"], "Fermentool test alarm", "the first send has no reminder number");
        assert_eq!(v["actions"][0]["body"], "ack 0");
        assert_eq!(pages.lock().unwrap()[0].sent, 1);
        n.ring();
        assert_eq!(pages.lock().unwrap()[0].sent, 1, "not again before its interval");
        let at = jiff::Timestamp::now().as_second();
        assert!(apply_ack(&mut pages.lock().unwrap(), TEST_RUN, at, "A (UI)"));
        n.ring();
        assert!(pages.lock().unwrap().is_empty());
    }
}
