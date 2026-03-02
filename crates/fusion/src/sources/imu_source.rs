use crate::node::{ConsumerCallback, Node, NodeBase};

/// Builder/wrapper for IMU source nodes.
///
/// Creates the appropriate concrete IMU source based on the JSON config "type" field.
/// Supported types: "hid", "openzen", "xreal_air2_ultra".
pub struct ImuSource {
    pub base: NodeBase,
    m_source_type: String,
    m_config: serde_json::Value,
}

impl ImuSource {
    pub fn new(name: impl Into<String>, config: &serde_json::Value) -> Self {
        let name = name.into();
        let source_type = config
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("hid")
            .to_string();
        let settings = config
            .get("settings")
            .cloned()
            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

        log::info!("Creating IMU source of type '{}': {}", source_type, name);

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

impl Node for ImuSource {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn start(&mut self) -> anyhow::Result<()> {
        log::info!(
            "Starting IMU source '{}' (type={})",
            self.base.name(),
            self.m_source_type
        );

        match self.m_source_type.to_lowercase().as_str() {
            "hid" | "hidimu" => {
                log::info!("HID IMU source selected - requires device SDK");
            }
            "openzen" => {
                log::info!("OpenZen IMU source selected - requires OpenZen SDK");
            }
            "xreal_air2_ultra" | "xreal" => {
                log::info!("XREAL Air 2 Ultra IMU source selected - requires device SDK");
            }
            other => {
                log::warn!("Unknown IMU source type: {}", other);
            }
        }

        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        log::info!("Stopping IMU source: {}", self.base.name());
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
