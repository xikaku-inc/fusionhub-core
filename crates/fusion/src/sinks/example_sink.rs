use fusion_types::StreamableData;

use crate::node::{Node, NodeBase};

/// Example sink node that counts received messages.
///
/// This is a minimal reference implementation showing how to build a sink
/// node. Sinks receive data via `receive_data` and perform some final
/// action (logging, writing to file, sending over network, etc.).
pub struct ExampleSink {
    pub base: NodeBase,
    m_count: u64,
}

impl ExampleSink {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            base: NodeBase::new(name),
            m_count: 0,
        }
    }

    pub fn count(&self) -> u64 {
        self.m_count
    }
}

impl Node for ExampleSink {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn start(&mut self) -> anyhow::Result<()> {
        self.m_count = 0;
        log::info!("ExampleSink '{}' started", self.base.name());
        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        log::info!("ExampleSink '{}' stopped ({} messages received)", self.base.name(), self.m_count);
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
        if !self.base.is_enabled() {
            return;
        }
        self.m_count += 1;
        log::debug!("[{}] received {} (total: {})", self.base.name(), data.variant_name(), self.m_count);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fusion_types::Timestamp;

    #[test]
    fn example_sink_counts() {
        let mut sink = ExampleSink::new("test_sink");
        assert_eq!(sink.count(), 0);
        sink.receive_data(StreamableData::Timestamp(Timestamp::current()));
        sink.receive_data(StreamableData::Timestamp(Timestamp::current()));
        assert_eq!(sink.count(), 2);
    }

    #[test]
    fn example_sink_disabled_ignores() {
        let mut sink = ExampleSink::new("test_sink");
        sink.set_enabled(false);
        sink.receive_data(StreamableData::Timestamp(Timestamp::current()));
        assert_eq!(sink.count(), 0);
    }
}
