use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

use tokio::sync::broadcast;

static LOG_TX: OnceLock<broadcast::Sender<String>> = OnceLock::new();
static LOG_BUFFER: OnceLock<Mutex<VecDeque<String>>> = OnceLock::new();

const BUFFER_CAPACITY: usize = 500;

struct BroadcastLogger {
    inner: env_logger::Logger,
    level: log::LevelFilter,
}

impl log::Log for BroadcastLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= self.level
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        if self.inner.matches(record) {
            self.inner.log(record);
        }

        let entry = serde_json::json!({
            "ts": chrono::Local::now().format("%H:%M:%S%.3f").to_string(),
            "level": record.level().to_string(),
            "target": record.target(),
            "message": format!("{}", record.args()),
        })
        .to_string();

        // Store in ring buffer so new clients can fetch history
        if let Some(buf) = LOG_BUFFER.get() {
            if let Ok(mut buf) = buf.lock() {
                if buf.len() >= BUFFER_CAPACITY {
                    buf.pop_front();
                }
                buf.push_back(entry.clone());
            }
        }

        // Broadcast to live subscribers
        if let Some(tx) = LOG_TX.get() {
            let _ = tx.send(entry);
        }
    }

    fn flush(&self) {
        self.inner.flush();
    }
}

/// Initialize the global logger using env_logger.
pub fn init() {
    let _ = env_logger::builder().try_init();
}

/// Initialize the global logger with a specific default filter level.
/// Also sets up a broadcast channel so log entries can be streamed to the UI.
pub fn init_with_level(level: log::LevelFilter) {
    LOG_BUFFER.set(Mutex::new(VecDeque::with_capacity(BUFFER_CAPACITY))).ok();
    let (tx, _) = broadcast::channel::<String>(256);
    LOG_TX.set(tx).ok();

    let inner = env_logger::Builder::new()
        .filter_level(level)
        .build();

    let logger = BroadcastLogger { inner, level };
    log::set_boxed_logger(Box::new(logger)).ok();
    log::set_max_level(level);
}

/// Initialize the global logger targeting stderr (for MCP mode).
pub fn init_with_level_stderr(level: log::LevelFilter) {
    LOG_BUFFER.set(Mutex::new(VecDeque::with_capacity(BUFFER_CAPACITY))).ok();
    let (tx, _) = broadcast::channel::<String>(256);
    LOG_TX.set(tx).ok();

    let inner = env_logger::Builder::new()
        .target(env_logger::Target::Stderr)
        .filter_level(level)
        .build();

    let logger = BroadcastLogger { inner, level };
    log::set_boxed_logger(Box::new(logger)).ok();
    log::set_max_level(level);
}

/// Subscribe to the log broadcast channel.
pub fn subscribe() -> Option<broadcast::Receiver<String>> {
    LOG_TX.get().map(|tx| tx.subscribe())
}

/// Return all buffered log entries (up to 500). These include messages
/// logged before any SSE client connected (startup messages, etc.).
pub fn buffered_entries() -> Vec<String> {
    LOG_BUFFER
        .get()
        .and_then(|buf| buf.lock().ok())
        .map(|buf| buf.iter().cloned().collect())
        .unwrap_or_default()
}
