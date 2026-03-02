use std::sync::{Arc, Mutex};

use anyhow::Result;
use serde_json::Value;

use crate::node::Node;

/// Extract the `settings` sub-object from a node config block.
pub fn extract_settings(config: &Value) -> Value {
    config
        .get("settings")
        .cloned()
        .unwrap_or_else(|| config.clone())
}

/// Build a node by type name and JSON configuration.
///
/// Delegates to the global fusion_registry. Nodes must be registered
/// via `fusion::registration::register_core_nodes()` (and optionally
/// proprietary registration) before calling this.
pub fn build_node(type_name: &str, config: &Value) -> Result<Arc<Mutex<dyn Node>>> {
    fusion_registry::build_node(type_name, config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_example_sink_from_config() {
        crate::registration::register_core_nodes();
        let config = serde_json::json!({ "name": "sink0" });
        let node = build_node("ExampleSink", &config).unwrap();
        let node = node.lock().unwrap();
        assert_eq!(node.name(), "sink0");
    }

    #[test]
    fn build_unknown_type() {
        let config = serde_json::json!({});
        let result = build_node("NonExistentNode", &config);
        assert!(result.is_err());
    }
}
