//! The app's own warnings and errors, kept where you can see them.
//!
//! The desktop app has no console, so a `tracing::warn!` there goes nowhere.
//! [`AppLogLayer`] catches every WARN and ERROR event (whatever the console
//! filter says) into an in-memory ring served at `/api/logs`, and appends
//! it to `<data folder>/logs/kanbain.log` so the trail survives a crash.

use std::{
    collections::VecDeque,
    fmt::Write as _,
    fs::{File, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

use chrono::{DateTime, Utc};
use serde::Serialize;
use tracing::{Event, Level, Subscriber, field::Field};
use tracing_subscriber::{Layer, layer::Context};
use ts_rs::TS;

/// Entries kept in memory.
const MAX_ENTRIES: usize = 2000;
/// The log file is rotated to `kanbain.log.1` past this size.
const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, TS)]
pub struct AppLogEntry {
    /// Increases by one per entry, for "everything since" polling.
    #[ts(type = "number")]
    pub seq: u64,
    pub timestamp: DateTime<Utc>,
    /// `ERROR` or `WARN`.
    pub level: String,
    /// The module that logged it, e.g. `local_deployment::orchestrator`.
    pub target: String,
    /// The message plus any structured fields, as `key=value`.
    pub message: String,
}

impl AppLogEntry {
    /// One line, the way the console prints it.
    pub fn line(&self) -> String {
        format!(
            "{} {:>5} {}: {}",
            self.timestamp
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            self.level,
            self.target,
            self.message
        )
    }
}

struct Inner {
    entries: VecDeque<AppLogEntry>,
    next_seq: u64,
    file: Option<(PathBuf, File)>,
}

/// The ring and the file; one per process.
pub struct AppLog {
    inner: Mutex<Inner>,
}

static APP_LOG: OnceLock<AppLog> = OnceLock::new();

/// The process-wide log. Entries are only recorded once [`layer`] is
/// installed, but reading works either way.
pub fn app_log() -> &'static AppLog {
    APP_LOG.get_or_init(|| AppLog {
        inner: Mutex::new(Inner {
            entries: VecDeque::new(),
            next_seq: 1,
            file: None,
        }),
    })
}

impl AppLog {
    /// Starts appending to `<dir>/kanbain.log`. Without it, entries only
    /// live in memory.
    pub fn open_file(&self, dir: &Path) {
        let path = dir.join("kanbain.log");
        let opened = std::fs::create_dir_all(dir)
            .and_then(|_| OpenOptions::new().create(true).append(true).open(&path));
        match opened {
            Ok(file) => {
                let path = path.canonicalize().unwrap_or(path);
                self.inner.lock().unwrap().file = Some((path, file));
            }
            // Not through tracing: that would come right back here.
            Err(e) => eprintln!("Can't open the app log {}: {e}", path.display()),
        }
    }

    pub fn file_path(&self) -> Option<PathBuf> {
        self.inner
            .lock()
            .unwrap()
            .file
            .as_ref()
            .map(|(p, _)| p.clone())
    }

    fn push(&self, level: &Level, target: &str, message: String) {
        let mut inner = self.inner.lock().unwrap();
        let entry = AppLogEntry {
            seq: inner.next_seq,
            timestamp: Utc::now(),
            level: level.to_string(),
            target: target.to_string(),
            message,
        };
        inner.next_seq += 1;
        if let Some((path, file)) = inner.file.as_mut() {
            let _ = writeln!(file, "{}", entry.line());
            if file.metadata().map(|m| m.len()).unwrap_or(0) > MAX_FILE_BYTES {
                let rotated = path.with_extension("log.1");
                let _ = std::fs::rename(&*path, &rotated);
                if let Ok(fresh) = OpenOptions::new().create(true).append(true).open(&*path) {
                    *file = fresh;
                }
            }
        }
        if inner.entries.len() == MAX_ENTRIES {
            inner.entries.pop_front();
        }
        inner.entries.push_back(entry);
    }

    /// Entries after `since` (a `seq`), errors only unless `warnings`, at
    /// most the newest `limit`, oldest first.
    pub fn entries(&self, since: u64, warnings: bool, limit: usize) -> Vec<AppLogEntry> {
        let inner = self.inner.lock().unwrap();
        let mut out: Vec<AppLogEntry> = inner
            .entries
            .iter()
            .rev()
            .filter(|e| e.seq > since)
            .filter(|e| warnings || e.level == "ERROR")
            .take(limit)
            .cloned()
            .collect();
        out.reverse();
        out
    }

    /// Forgets the in-memory entries (the file keeps them).
    pub fn clear(&self) {
        self.inner.lock().unwrap().entries.clear();
    }
}

/// Collects an event's message and fields into one line.
#[derive(Default)]
struct Message(String);

impl tracing::field::Visit for Message {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let fields = std::mem::take(&mut self.0);
            let _ = write!(self.0, "{value:?}");
            if !fields.is_empty() {
                self.0.push_str(&fields);
            }
        } else {
            let _ = write!(self.0, " {}={value:?}", field.name());
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.record_debug(field, &format_args!("{value}"));
        } else {
            let _ = write!(self.0, " {}={value:?}", field.name());
        }
    }
}

/// The layer to add to the tracing registry.
pub struct AppLogLayer;

pub fn layer() -> AppLogLayer {
    AppLogLayer
}

impl<S: Subscriber> Layer<S> for AppLogLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let level = event.metadata().level();
        if *level > Level::WARN {
            return;
        }
        let mut message = Message::default();
        event.record(&mut message);
        app_log().push(level, event.metadata().target(), message.0);
    }
}

#[cfg(test)]
mod tests {
    use tracing_subscriber::prelude::*;

    use super::*;

    #[test]
    fn keeps_warnings_and_errors_with_their_fields() {
        let dir = std::env::temp_dir().join(format!("kanbain-app-log-{}", std::process::id()));
        app_log().open_file(&dir);
        let start = app_log()
            .entries(0, true, usize::MAX)
            .last()
            .map_or(0, |e| e.seq);
        let subscriber = tracing_subscriber::registry().with(layer());
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!("not kept");
            tracing::warn!(repo = "shop", "Indexing failed");
            tracing::error!("Merge failed: {}", "conflict");
        });

        let all = app_log().entries(start, true, usize::MAX);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].level, "WARN");
        assert_eq!(all[0].message, "Indexing failed repo=\"shop\"");
        assert!(all[0].target.contains("app_log"));
        assert_eq!(all[1].message, "Merge failed: conflict");
        let errors = app_log().entries(start, false, usize::MAX);
        assert_eq!(errors.len(), 1);
        assert_eq!(app_log().entries(all[0].seq, true, 10).len(), 1);
        assert_eq!(app_log().entries(start, true, 1)[0].seq, all[1].seq);

        let file = std::fs::read_to_string(dir.join("kanbain.log")).unwrap();
        assert!(file.contains(" ERROR ") && file.contains("Merge failed: conflict"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
