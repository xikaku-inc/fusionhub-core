use std::sync::{Arc, Mutex};

use fusion_types::StreamableData;

use crate::encoders::json_encoder::JsonEncoder;
use crate::node::{Node, NodeBase};

/// Sink that logs all received StreamableData as JSON to the console.
/// Useful for debugging: acts as a simple passthrough logger.
pub struct EchoSink {
    pub base: NodeBase,
    m_count: Arc<Mutex<u64>>,
}

impl EchoSink {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            base: NodeBase::new(name),
            m_count: Arc::new(Mutex::new(0)),
        }
    }

    pub fn on_data(&self, data: StreamableData) {
        if !self.base.is_enabled() {
            return;
        }
        match JsonEncoder::encode(&data) {
            Ok(json) => {
                log::info!("[{}] {}", self.base.name(), json);
            }
            Err(e) => {
                log::warn!("[{}] Failed to encode data: {}", self.base.name(), e);
            }
        }
        let mut count = self.m_count.lock().unwrap();
        *count += 1;
        self.base.notify_consumers(data);
    }

    pub fn count(&self) -> u64 {
        *self.m_count.lock().unwrap()
    }
}

impl Node for EchoSink {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn start(&mut self) -> anyhow::Result<()> {
        log::info!("EchoSink '{}' started", self.base.name());
        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        log::info!(
            "EchoSink '{}' stopped (logged {} messages)",
            self.base.name(),
            self.count()
        );
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
    use fusion_types::Timestamp;

    #[test]
    fn echo_sink_counts_messages() {
        let sink = EchoSink::new("echo_test");
        assert_eq!(sink.count(), 0);
        sink.on_data(StreamableData::Timestamp(Timestamp::current()));
        assert_eq!(sink.count(), 1);
        sink.on_data(StreamableData::Timestamp(Timestamp::current()));
        assert_eq!(sink.count(), 2);
    }

    #[test]
    fn echo_sink_disabled_does_not_log() {
        let mut sink = EchoSink::new("echo_test");
        sink.set_enabled(false);
        sink.on_data(StreamableData::Timestamp(Timestamp::current()));
        assert_eq!(sink.count(), 0);
    }
}
