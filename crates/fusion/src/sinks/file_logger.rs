use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use fusion_registry::{sf, SettingsField};
use fusion_types::StreamableData;
use serde_json::json;

use crate::encoders::json_encoder::JsonEncoder;
use crate::node::{Node, NodeBase};

pub fn settings_schema() -> Vec<SettingsField> {
    vec![
        sf("filePath", "File Path", "string", json!("log.json")),
    ]
}

/// Configuration for file rotation.
#[derive(Clone, Debug)]
pub struct FileRotationConfig {
    pub max_file_size_bytes: u64,
    pub max_files: usize,
}

impl Default for FileRotationConfig {
    fn default() -> Self {
        Self {
            max_file_size_bytes: 100 * 1024 * 1024, // 100 MB
            max_files: 10,
        }
    }
}

/// Internal writer state shared with the background writer thread.
struct WriterState {
    buffer: Vec<String>,
    file_path: PathBuf,
    rotation: FileRotationConfig,
    current_file_index: usize,
    current_file_size: u64,
    total_written: u64,
}

/// Sink that writes StreamableData as JSON lines to files on disk.
/// Uses a background thread for async file writing. Supports configurable
/// output directory and file rotation.
pub struct FileLogger {
    pub base: NodeBase,
    m_state: Arc<Mutex<WriterState>>,
    m_writer_handle: Option<std::thread::JoinHandle<()>>,
    m_running: Arc<std::sync::atomic::AtomicBool>,
}

impl FileLogger {
    pub fn new(name: impl Into<String>, file_path: &str) -> Self {
        let path = PathBuf::from(file_path);
        Self {
            base: NodeBase::new(name),
            m_state: Arc::new(Mutex::new(WriterState {
                buffer: Vec::new(),
                file_path: path,
                rotation: FileRotationConfig::default(),
                current_file_index: 0,
                current_file_size: 0,
                total_written: 0,
            })),
            m_writer_handle: None,
            m_running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    pub fn with_rotation(self, config: FileRotationConfig) -> Self {
        let mut state = self.m_state.lock().unwrap();
        state.rotation = config;
        drop(state);
        self
    }

    pub fn set_output_directory(&self, dir: &str) {
        let mut state = self.m_state.lock().unwrap();
        let file_name = state
            .file_path
            .file_name()
            .map(|f| f.to_owned())
            .unwrap_or_default();
        state.file_path = PathBuf::from(dir).join(file_name);
    }

    pub fn on_data(&self, data: StreamableData) {
        if !self.base.is_enabled() {
            return;
        }
        match JsonEncoder::encode(&data) {
            Ok(json_line) => {
                let mut state = self.m_state.lock().unwrap();
                state.buffer.push(json_line);
            }
            Err(e) => {
                log::warn!(
                    "[{}] Failed to encode data for file: {}",
                    self.base.name(),
                    e
                );
            }
        }
        self.base.notify_consumers(data);
    }

    pub fn total_written(&self) -> u64 {
        self.m_state.lock().unwrap().total_written
    }

    fn rotated_path(base_path: &Path, index: usize) -> PathBuf {
        if index == 0 {
            base_path.to_path_buf()
        } else {
            let stem = base_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("log");
            let ext = base_path
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("json");
            let parent = base_path.parent().unwrap_or(Path::new("."));
            parent.join(format!("{}.{}.{}", stem, index, ext))
        }
    }

    fn start_writer(&mut self) {
        use std::io::Write;
        use std::sync::atomic::Ordering;

        let state = self.m_state.clone();
        let running = self.m_running.clone();
        running.store(true, Ordering::Relaxed);

        self.m_writer_handle = Some(std::thread::spawn(move || {
            while running.load(Ordering::Relaxed) {
                let lines: Vec<String> = {
                    let mut s = state.lock().unwrap();
                    std::mem::take(&mut s.buffer)
                };

                if lines.is_empty() {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    continue;
                }

                let mut s = state.lock().unwrap();
                let path = Self::rotated_path(&s.file_path, s.current_file_index);

                match std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
                {
                    Ok(mut file) => {
                        for line in &lines {
                            let bytes = line.as_bytes();
                            if file.write_all(bytes).is_ok() && file.write_all(b"\n").is_ok() {
                                s.current_file_size += bytes.len() as u64 + 1;
                                s.total_written += 1;
                            }
                        }

                        if s.current_file_size >= s.rotation.max_file_size_bytes {
                            s.current_file_index =
                                (s.current_file_index + 1) % s.rotation.max_files;
                            s.current_file_size = 0;
                            let next_path =
                                Self::rotated_path(&s.file_path, s.current_file_index);
                            let _ = std::fs::remove_file(&next_path);
                        }
                    }
                    Err(e) => {
                        log::error!("FileLogger failed to open '{}': {}", path.display(), e);
                    }
                }
            }
        }));
    }
}

impl Node for FileLogger {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn status(&self) -> serde_json::Value {
        let state = self.m_state.lock().unwrap();
        serde_json::json!({
            "totalWritten": state.total_written,
            "file": state.file_path.display().to_string(),
        })
    }

    fn start(&mut self) -> anyhow::Result<()> {
        log::info!(
            "FileLogger '{}' starting (path={})",
            self.base.name(),
            self.m_state.lock().unwrap().file_path.display()
        );
        self.start_writer();
        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        log::info!(
            "FileLogger '{}' stopping (wrote {} entries)",
            self.base.name(),
            self.total_written()
        );
        self.m_running
            .store(false, std::sync::atomic::Ordering::Relaxed);
        if let Some(handle) = self.m_writer_handle.take() {
            let _ = handle.join();
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

    fn receive_data(&mut self, data: StreamableData) {
        self.on_data(data);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotated_path_index_zero() {
        let base = PathBuf::from("/tmp/output.json");
        let p = FileLogger::rotated_path(&base, 0);
        assert_eq!(p, PathBuf::from("/tmp/output.json"));
    }

    #[test]
    fn rotated_path_index_nonzero() {
        let base = PathBuf::from("/tmp/output.json");
        let p = FileLogger::rotated_path(&base, 3);
        assert_eq!(p, PathBuf::from("/tmp/output.3.json"));
    }

    #[test]
    fn file_logger_default_rotation() {
        let config = FileRotationConfig::default();
        assert_eq!(config.max_file_size_bytes, 100 * 1024 * 1024);
        assert_eq!(config.max_files, 10);
    }
}
