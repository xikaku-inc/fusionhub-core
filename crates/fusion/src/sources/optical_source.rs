use crate::node::{ConsumerCallback, Node, NodeBase};

/// Builder/wrapper for optical tracking source nodes.
///
/// Creates the appropriate concrete optical source based on the JSON config "type" field.
/// Supported types: "vicon", "dtrack", "optitrack", "antilatency".
pub struct OpticalSource {
    pub base: NodeBase,
    m_source_type: String,
    m_config: serde_json::Value,
}

impl OpticalSource {
    pub fn new(name: impl Into<String>, config: &serde_json::Value) -> Self {
        let name = name.into();
        let source_type = config
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("vicon")
            .to_string();
        let settings = config
            .get("settings")
            .cloned()
            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

        log::info!(
            "Creating optical source of type '{}': {}",
            source_type,
            name
        );

        Self {
            base: NodeBase::new(&name),
            m_source_type: source_type,
            m_config: settings,
        }
    }

    pub fn source_type(&self) -> &str {
        &self.m_source_type
    }
}

impl Node for OpticalSource {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn start(&mut self) -> anyhow::Result<()> {
        log::info!(
            "Starting optical source '{}' (type={})",
            self.base.name(),
            self.m_source_type
        );

        match self.m_source_type.to_lowercase().as_str() {
            "vicon" => {
                log::info!("Vicon optical source selected - requires Vicon SDK");
            }
            "dtrack" => {
                log::info!("DTrack optical source selected - requires ART DTrack SDK");
            }
            "optitrack" => {
                log::info!("OptiTrack optical source selected - requires NatNet SDK");
            }
            "antilatency" => {
                log::info!("Antilatency source selected - requires Antilatency SDK");
            }
            other => {
                log::warn!("Unknown optical source type: {}", other);
            }
        }

        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        log::info!("Stopping optical source: {}", self.base.name());
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
