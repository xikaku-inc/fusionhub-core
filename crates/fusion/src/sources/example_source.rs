use std::sync::atomic::Ordering;
use std::time::Duration;

use fusion_types::{StreamableData, Timestamp};

use crate::node::{Node, NodeBase, ConsumerCallback};

/// Example source node that emits a Timestamp at a configurable interval.
///
/// This is a minimal reference implementation showing how to build a source
/// node. Sources produce data on a periodic heartbeat and forward it to
/// downstream consumers via `notify_consumers`.
pub struct ExampleSource {
    pub base: NodeBase,
    m_interval: Duration,
}

impl ExampleSource {
    pub fn new(name: impl Into<String>, interval_ms: u64) -> Self {
        let mut base = NodeBase::new(name);
        let interval = Duration::from_millis(interval_ms.max(10));
        base.set_heartbeat_interval(interval);
        Self { base, m_interval: interval }
    }
}

impl Node for ExampleSource {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn start(&mut self) -> anyhow::Result<()> {
        log::info!("ExampleSource '{}' started (interval {:?})", self.base.name(), self.m_interval);
        let consumers = self.base.consumers_arc();
        let enabled = self.base.enabled_arc();
        self.base.start_heartbeat(move || {
            if !enabled.load(Ordering::Relaxed) {
                return;
            }
            let data = StreamableData::Timestamp(Timestamp::current());
            let cbs = consumers.lock().unwrap();
            for cb in cbs.iter() {
                cb(data.clone());
            }
        });
        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        self.base.stop_heartbeat();
        log::info!("ExampleSource '{}' stopped", self.base.name());
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn example_source_lifecycle() {
        let mut src = ExampleSource::new("test_source", 100);
        assert_eq!(src.name(), "test_source");
        assert!(src.is_enabled());
        src.set_enabled(false);
        assert!(!src.is_enabled());
    }

    #[test]
    fn example_source_consumer_wiring() {
        let src = ExampleSource::new("test_source", 100);
        let count = Arc::new(Mutex::new(0u64));
        let c = count.clone();
        src.set_on_output(Box::new(move |_| { *c.lock().unwrap() += 1; }));
        assert_eq!(src.base.consumer_count(), 1);
    }
}
