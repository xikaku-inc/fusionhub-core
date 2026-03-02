use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};
use std::time::Duration;

use fusion_types::StreamableData;
use tokio::task::JoinHandle;

pub use fusion_registry::{Node, ConsumerCallback, CommandConsumerCallback};

/// Base implementation shared by all nodes.
pub struct NodeBase {
    m_name: String,
    m_enabled: Arc<AtomicBool>,
    m_heartbeat_interval: Duration,
    m_consumers: Arc<Mutex<Vec<ConsumerCallback>>>,
    m_heartbeat_handle: Option<JoinHandle<()>>,
    m_running: Arc<AtomicBool>,
}

impl NodeBase {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            m_name: name.into(),
            m_enabled: Arc::new(AtomicBool::new(true)),
            m_heartbeat_interval: Duration::from_millis(1000),
            m_consumers: Arc::new(Mutex::new(Vec::new())),
            m_heartbeat_handle: None,
            m_running: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn name(&self) -> &str {
        &self.m_name
    }

    pub fn is_enabled(&self) -> bool {
        self.m_enabled.load(Ordering::Relaxed)
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.m_enabled.store(enabled, Ordering::Relaxed);
    }

    pub fn set_heartbeat_interval(&mut self, interval: Duration) {
        self.m_heartbeat_interval = interval;
    }

    pub fn heartbeat_interval(&self) -> Duration {
        self.m_heartbeat_interval
    }

    pub fn add_consumer(&self, callback: ConsumerCallback) {
        let mut consumers = self.m_consumers.lock().unwrap();
        consumers.push(callback);
    }

    pub fn notify_consumers(&self, data: StreamableData) {
        if !self.m_enabled.load(Ordering::Relaxed) {
            return;
        }
        let consumers = self.m_consumers.lock().unwrap();
        for consumer in consumers.iter() {
            consumer(data.clone());
        }
    }

    pub fn consumer_count(&self) -> usize {
        self.m_consumers.lock().unwrap().len()
    }

    pub fn start_heartbeat<F>(&mut self, heartbeat_fn: F)
    where
        F: Fn() + Send + Sync + 'static,
    {
        let interval = self.m_heartbeat_interval;
        let running = self.m_running.clone();
        running.store(true, Ordering::Relaxed);

        self.m_heartbeat_handle = Some(tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            while running.load(Ordering::Relaxed) {
                ticker.tick().await;
                heartbeat_fn();
            }
        }));
    }

    pub fn stop_heartbeat(&mut self) {
        self.m_running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.m_heartbeat_handle.take() {
            handle.abort();
        }
    }

    pub fn is_running(&self) -> bool {
        self.m_running.load(Ordering::Relaxed)
    }

    pub fn consumers_arc(&self) -> Arc<Mutex<Vec<ConsumerCallback>>> {
        Arc::clone(&self.m_consumers)
    }

    pub fn enabled_arc(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.m_enabled)
    }

    pub fn running_arc(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.m_running)
    }
}

impl Drop for NodeBase {
    fn drop(&mut self) {
        self.stop_heartbeat();
    }
}

pub struct SimpleNode {
    pub base: NodeBase,
}

impl SimpleNode {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            base: NodeBase::new(name),
        }
    }
}

impl Node for SimpleNode {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn start(&mut self) -> anyhow::Result<()> {
        log::info!("Starting node: {}", self.base.name());
        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        log::info!("Stopping node: {}", self.base.name());
        self.base.stop_heartbeat();
        Ok(())
    }

    fn is_enabled(&self) -> bool {
        self.base.is_enabled()
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.base.set_enabled(enabled);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fusion_types::Timestamp;

    #[test]
    fn node_base_enable_disable() {
        let mut base = NodeBase::new("test_node");
        assert!(base.is_enabled());
        base.set_enabled(false);
        assert!(!base.is_enabled());
    }

    #[test]
    fn node_base_consumers() {
        let base = NodeBase::new("test_node");
        let counter = Arc::new(Mutex::new(0usize));
        let c = counter.clone();
        base.add_consumer(Box::new(move |_data| {
            *c.lock().unwrap() += 1;
        }));
        assert_eq!(base.consumer_count(), 1);

        let data = StreamableData::Timestamp(Timestamp::current());
        base.notify_consumers(data);
        assert_eq!(*counter.lock().unwrap(), 1);
    }

    #[test]
    fn simple_node_lifecycle() {
        let mut node = SimpleNode::new("my_node");
        assert_eq!(node.name(), "my_node");
        assert!(node.start().is_ok());
        assert!(node.stop().is_ok());
    }
}
