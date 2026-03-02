use fusion_types::StreamableData;

use crate::node::{Node, NodeBase};

/// ROS 2 topic publisher sink.
///
/// This is a stub -- building a full ROS 2 sink requires the ROS 2 SDK
/// (rclrs) which is not available as a standard Rust crate yet.
pub struct Ros2Sink {
    base: NodeBase,
    topic: String,
    frame_id: String,
}

impl Ros2Sink {
    pub fn new(name: &str, settings: &serde_json::Value) -> Self {
        let topic = settings
            .get("topic")
            .and_then(|v| v.as_str())
            .unwrap_or("/fusionhub/pose")
            .to_owned();
        let frame_id = settings
            .get("frameId")
            .and_then(|v| v.as_str())
            .unwrap_or("world")
            .to_owned();
        Self {
            base: NodeBase::new(name),
            topic,
            frame_id,
        }
    }
}

impl Node for Ros2Sink {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn start(&mut self) -> anyhow::Result<()> {
        log::warn!(
            "Ros2Sink '{}': ROS 2 integration not available in Rust port. \
             Topic '{}' will not be published.",
            self.name(),
            self.topic
        );
        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        self.base.stop_heartbeat();
        Ok(())
    }

    fn is_enabled(&self) -> bool {
        self.base.is_enabled()
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.base.set_enabled(enabled);
    }

    fn receive_data(&mut self, _data: StreamableData) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ros2_sink_creation() {
        let settings = serde_json::json!({ "topic": "/test/pose", "frameId": "base_link" });
        let sink = Ros2Sink::new("ros2_test", &settings);
        assert_eq!(sink.name(), "ros2_test");
        assert_eq!(sink.topic, "/test/pose");
        assert_eq!(sink.frame_id, "base_link");
    }

    #[test]
    fn ros2_sink_start_logs_warning() {
        let settings = serde_json::json!({});
        let mut sink = Ros2Sink::new("ros2_test", &settings);
        // Should NOT panic, just log a warning
        assert!(sink.start().is_ok());
    }
}
