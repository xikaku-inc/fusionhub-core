use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::task::JoinHandle;

use fusion_registry::{sf, SettingsField};
use serde_json::json;

use crate::encoders::json_encoder::JsonDecoder;
use crate::node::{ConsumerCallback, Node, NodeBase};

pub fn settings_schema() -> Vec<SettingsField> {
    vec![
        sf("filename", "File Path", "filepath", json!("")),
        sf("playbackInterval", "Playback Interval (s)", "number", json!(0.001)),
        sf("nSkipInitial", "Skip Initial Samples", "number", json!(0)),
        sf("nStopAfter", "Stop After N Samples", "number", json!(0)),
        sf("showProgress", "Show Progress", "boolean", json!(true)),
        sf("loop", "Loop Playback", "boolean", json!(false)),
    ]
}

/// Configuration for the file reader source.
struct FileReaderConfig {
    filename: PathBuf,
    playback_interval: Duration,
    n_skip_initial: usize,
    n_stop_after: usize,
    show_progress: bool,
    do_loop: bool,
}

/// Reads prerecorded JSON data files and replays them with timing.
///
/// Supports JSON and NDJSON files. Each line is decoded as StreamableData
/// and forwarded to consumers at the configured playback interval.
pub struct FileReaderSource {
    pub base: NodeBase,
    m_config: FileReaderConfig,
    m_done: Arc<AtomicBool>,
    m_worker_handle: Option<JoinHandle<()>>,
}

impl FileReaderSource {
    pub fn new(name: impl Into<String>, config: &serde_json::Value) -> Self {
        let name = name.into();
        let filename = config
            .get("filename")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let playback_interval_secs = config
            .get("playbackInterval")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.001);
        let n_skip_initial = config
            .get("nSkipInitial")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;
        let n_stop_after = match config.get("nStopAfter").and_then(|v| v.as_u64()) {
            Some(0) | None => usize::MAX,
            Some(n) => n as usize,
        };
        let show_progress = config
            .get("showProgress")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let do_loop = config
            .get("loop")
            .or_else(|| config.get("doLoopInFile"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        Self {
            base: NodeBase::new(&name),
            m_config: FileReaderConfig {
                filename: PathBuf::from(filename),
                playback_interval: Duration::from_secs_f64(playback_interval_secs),
                n_skip_initial,
                n_stop_after,
                show_progress,
                do_loop,
            },
            m_done: Arc::new(AtomicBool::new(false)),
            m_worker_handle: None,
        }
    }

    fn replay_file(
        filename: &PathBuf,
        playback_interval: Duration,
        n_skip_initial: usize,
        n_stop_after: usize,
        show_progress: bool,
        done: &AtomicBool,
        consumers: &Mutex<Vec<ConsumerCallback>>,
        enabled: &AtomicBool,
    ) -> anyhow::Result<()> {
        use std::io::BufRead;

        let file = std::fs::File::open(filename)?;
        let reader = std::io::BufReader::new(file);

        let mut count_total: usize = 0;
        let mut count_processed: usize = 0;

        for line_result in reader.lines() {
            if done.load(Ordering::Relaxed) {
                break;
            }

            let line = match line_result {
                Ok(l) => l,
                Err(e) => {
                    log::warn!("Error reading line from file: {}", e);
                    continue;
                }
            };

            if line.trim().is_empty() {
                continue;
            }

            count_total += 1;

            if count_total <= n_skip_initial {
                continue;
            }

            if count_processed >= n_stop_after {
                break;
            }

            match JsonDecoder::decode(&line) {
                Ok(data) => {
                    if enabled.load(Ordering::Relaxed) {
                        let cbs = consumers.lock().unwrap();
                        for cb in cbs.iter() {
                            cb(data.clone());
                        }
                    }
                    count_processed += 1;
                }
                Err(_) => {
                    // Try direct serde deserialization as fallback
                    if let Ok(data) = JsonDecoder::decode_direct(&line) {
                        if enabled.load(Ordering::Relaxed) {
                            let cbs = consumers.lock().unwrap();
                            for cb in cbs.iter() {
                                cb(data.clone());
                            }
                        }
                        count_processed += 1;
                    }
                }
            }

            if show_progress && count_total % 1000 == 0 {
                log::info!("FileReader: {} lines processed...", count_total);
            }

            if !playback_interval.is_zero() {
                std::thread::sleep(playback_interval);
            }
        }

        log::info!(
            "FileReader: finished. Total={}, Processed={}",
            count_total,
            count_processed
        );
        Ok(())
    }
}

impl Node for FileReaderSource {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn start(&mut self) -> anyhow::Result<()> {
        log::info!(
            "Starting FileReader: {}",
            self.m_config.filename.display()
        );

        let filename = self.m_config.filename.clone();
        let playback_interval = self.m_config.playback_interval;
        let n_skip_initial = self.m_config.n_skip_initial;
        let n_stop_after = self.m_config.n_stop_after;
        let show_progress = self.m_config.show_progress;
        let do_loop = self.m_config.do_loop;
        let done = self.m_done.clone();
        let consumers = self.base.consumers_arc();
        let enabled = self.base.enabled_arc();

        self.m_worker_handle = Some(tokio::spawn(async move {
            let result = tokio::task::spawn_blocking(move || {
                loop {
                    if let Err(e) = Self::replay_file(
                        &filename,
                        playback_interval,
                        n_skip_initial,
                        n_stop_after,
                        show_progress,
                        &done,
                        &consumers,
                        &enabled,
                    ) {
                        log::warn!("FileReader error: {}", e);
                    }

                    if !do_loop || done.load(Ordering::Relaxed) {
                        break;
                    }

                    log::info!("FileReader: looping, restarting playback...");
                    // Signal downstream nodes to reset their state
                    let cbs = consumers.lock().unwrap();
                    for cb in cbs.iter() {
                        cb(fusion_types::StreamableData::Reset);
                    }
                    drop(cbs);
                }
            })
            .await;

            if let Err(e) = result {
                log::warn!("FileReader worker thread panicked: {}", e);
            }
        }));

        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        log::info!("Stopping FileReader: {}", self.base.name());
        self.m_done.store(true, Ordering::Relaxed);

        if let Some(handle) = self.m_worker_handle.take() {
            handle.abort();
        }

        self.base.stop_heartbeat();
        Ok(())
    }

    fn is_enabled(&self) -> bool {
        self.base.is_enabled()
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.base.set_enabled(enabled);
    }

    fn set_on_output(&self, callback: ConsumerCallback) {
        self.base.add_consumer(callback);
    }
}
