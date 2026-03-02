use crate::node::{ConsumerCallback, Node, NodeBase};

/// Builder/wrapper for GNSS source nodes.
///
/// Creates the appropriate concrete GNSS source based on the JSON config "type" field.
/// Currently the primary implementation is the NmeaSource.
pub struct GnssSource {
    pub base: NodeBase,
    m_source_type: String,
    m_config: serde_json::Value,
}

impl GnssSource {
    pub fn new(name: impl Into<String>, config: &serde_json::Value) -> Self {
        let name = name.into();
        let source_type = config
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("nmea")
            .to_string();
        let settings = config
            .get("settings")
            .cloned()
            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

        log::info!(
            "Creating GNSS source of type '{}': {}",
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

impl Node for GnssSource {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn start(&mut self) -> anyhow::Result<()> {
        log::info!(
            "Starting GNSS source '{}' (type={})",
            self.base.name(),
            self.m_source_type
        );

        match self.m_source_type.to_lowercase().as_str() {
            "nmea" | "nmeasourcev2" => {
                log::info!("NMEA GNSS source - use NmeaSource directly for full functionality");
            }
            other => {
                log::warn!("Unknown GNSS source type: {}", other);
            }
        }

        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        log::info!("Stopping GNSS source: {}", self.base.name());
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
