use std::sync::{Arc, Mutex};

use fusion_types::StreamableData;
use networking::DiskWriter;

use crate::node::{Node, NodeBase};

/// Simple file recorder that writes StreamableData as JSON lines to disk
/// using the networking::DiskWriter.
pub struct LegacyFileRecorder {
    pub base: NodeBase,
    m_writer: Arc<Mutex<Option<DiskWriter>>>,
    m_file_path: String,
    m_recording: bool,
}

impl LegacyFileRecorder {
    pub fn new(name: impl Into<String>, file_path: &str) -> Self {
        Self {
            base: NodeBase::new(name),
            m_writer: Arc::new(Mutex::new(None)),
            m_file_path: file_path.to_owned(),
            m_recording: false,
        }
    }

    pub fn on_data(&self, data: StreamableData) {
        if !self.base.is_enabled() || !self.m_recording {
            return;
        }

        let writer = self.m_writer.lock().unwrap();
        if let Some(ref w) = *writer {
            if let Err(e) = w.write(&data) {
                log::warn!("[{}] Failed to write data: {}", self.base.name(), e);
            }
        }

        self.base.notify_consumers(data);
    }

    pub fn start_recording(&mut self) {
        let writer = DiskWriter::new(&self.m_file_path);
        *self.m_writer.lock().unwrap() = Some(writer);
        self.m_recording = true;
        log::info!(
            "[{}] Recording started to '{}'",
            self.base.name(),
            self.m_file_path
        );
    }

    pub fn stop_recording(&mut self) {
        self.m_recording = false;
        *self.m_writer.lock().unwrap() = None;
        log::info!("[{}] Recording stopped", self.base.name());
    }

    pub fn is_recording(&self) -> bool {
        self.m_recording
    }

    pub fn file_path(&self) -> &str {
        &self.m_file_path
    }

    pub fn record_count(&self) -> usize {
        let writer = self.m_writer.lock().unwrap();
        match *writer {
            Some(ref w) => w.count(),
            None => 0,
        }
    }
}

impl Node for LegacyFileRecorder {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn start(&mut self) -> anyhow::Result<()> {
        log::info!(
            "LegacyFileRecorder '{}' starting (path='{}')",
            self.base.name(),
            self.m_file_path
        );
        self.start_recording();
        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        log::info!(
            "LegacyFileRecorder '{}' stopping (recorded {} entries)",
            self.base.name(),
            self.record_count()
        );
        self.stop_recording();
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
    fn legacy_file_recorder_creation() {
        let recorder = LegacyFileRecorder::new("rec_test", "/tmp/recording.json");
        assert_eq!(recorder.name(), "rec_test");
        assert_eq!(recorder.file_path(), "/tmp/recording.json");
        assert!(!recorder.is_recording());
    }

    #[test]
    fn legacy_file_recorder_start_stop_recording() {
        let mut recorder = LegacyFileRecorder::new("rec_test", "/tmp/recording.json");
        recorder.start_recording();
        assert!(recorder.is_recording());
        recorder.stop_recording();
        assert!(!recorder.is_recording());
    }
}
