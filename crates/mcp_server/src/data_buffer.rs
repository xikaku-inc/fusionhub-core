use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::sync::{broadcast, RwLock};

const MAX_AGE_SECS: u64 = 300;
const MAX_ENTRIES_PER_TYPE: usize = 30_000;

struct TimestampedValue {
    received_at: Instant,
    data: Value,
}

pub struct DataBuffer {
    latest: HashMap<String, Value>,
    history: HashMap<String, VecDeque<TimestampedValue>>,
    max_age: Duration,
}

impl DataBuffer {
    pub fn new() -> Self {
        Self {
            latest: HashMap::new(),
            history: HashMap::new(),
            max_age: Duration::from_secs(MAX_AGE_SECS),
        }
    }

    pub fn ingest(&mut self, event_type: &str, data: Value) {
        self.latest.insert(event_type.to_owned(), data.clone());

        let history = self
            .history
            .entry(event_type.to_owned())
            .or_insert_with(VecDeque::new);
        history.push_back(TimestampedValue {
            received_at: Instant::now(),
            data,
        });

        let cutoff = Instant::now() - self.max_age;
        while history.front().map_or(false, |e| e.received_at < cutoff) {
            history.pop_front();
        }
        while history.len() > MAX_ENTRIES_PER_TYPE {
            history.pop_front();
        }
    }

    pub fn latest(&self, event_type: &str) -> Option<&Value> {
        self.latest.get(event_type)
    }

    /// Return history entries for `event_type` from the last `last_secs` seconds,
    /// evenly downsampled to at most `max_samples` entries.
    pub fn history(&self, event_type: &str, last_secs: f64, max_samples: usize) -> Vec<&Value> {
        let deque = match self.history.get(event_type) {
            Some(d) => d,
            None => return Vec::new(),
        };

        let cutoff = Instant::now() - Duration::from_secs_f64(last_secs);
        let window: Vec<&TimestampedValue> =
            deque.iter().filter(|e| e.received_at >= cutoff).collect();

        if window.is_empty() {
            return Vec::new();
        }

        if window.len() <= max_samples {
            return window.iter().map(|e| &e.data).collect();
        }

        // Downsample evenly
        let step = window.len() as f64 / max_samples as f64;
        (0..max_samples)
            .map(|i| {
                let idx = (i as f64 * step) as usize;
                &window[idx.min(window.len() - 1)].data
            })
            .collect()
    }
}

pub fn spawn_ingestion_task(
    sse_tx: &broadcast::Sender<String>,
    buffer: Arc<RwLock<DataBuffer>>,
) -> tokio::task::JoinHandle<()> {
    let mut rx = sse_tx.subscribe();
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(msg) => {
                    if let Ok(parsed) = serde_json::from_str::<Value>(&msg) {
                        let event_type = parsed
                            .get("type")
                            .and_then(|t| t.as_str())
                            .unwrap_or("unknown")
                            .to_owned();
                        let data = parsed.get("data").cloned().unwrap_or(Value::Null);
                        buffer.write().await.ingest(&event_type, data);
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    log::warn!("MCP data buffer lagged by {} messages", n);
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    })
}
