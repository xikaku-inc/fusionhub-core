use fusion_types::StreamableData;

use crate::node::{Node, NodeBase, ConsumerCallback};

/// Example filter node that passes through all received data unchanged.
///
/// This is a minimal reference implementation showing how to build a filter
/// node. Filters receive data via `receive_data`, optionally transform it,
/// and forward the result to downstream consumers via `notify_consumers`.
pub struct ExampleFilter {
    pub base: NodeBase,
    m_count: u64,
}

impl ExampleFilter {
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

impl Node for ExampleFilter {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn start(&mut self) -> anyhow::Result<()> {
        self.m_count = 0;
        log::info!("ExampleFilter '{}' started", self.base.name());
        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        log::info!("ExampleFilter '{}' stopped ({} messages forwarded)", self.base.name(), self.m_count);
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
        self.base.notify_consumers(data);
    }

    fn set_on_output(&self, callback: ConsumerCallback) {
        self.base.add_consumer(callback);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use fusion_types::Timestamp;

    #[test]
    fn example_filter_passes_through() {
        let mut filter = ExampleFilter::new("test_filter");
        let received = Arc::new(Mutex::new(0u64));
        let r = received.clone();
        filter.set_on_output(Box::new(move |_| { *r.lock().unwrap() += 1; }));

        filter.receive_data(StreamableData::Timestamp(Timestamp::current()));
        filter.receive_data(StreamableData::Timestamp(Timestamp::current()));
        assert_eq!(filter.count(), 2);
        assert_eq!(*received.lock().unwrap(), 2);
    }

    #[test]
    fn example_filter_disabled_blocks() {
        let mut filter = ExampleFilter::new("test_filter");
        filter.set_enabled(false);
        filter.receive_data(StreamableData::Timestamp(Timestamp::current()));
        assert_eq!(filter.count(), 0);
    }
}
