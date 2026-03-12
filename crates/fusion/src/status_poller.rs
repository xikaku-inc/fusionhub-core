use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::connected_node::ConnectedNodeImpl;
use fusion_registry::Node;

pub struct NodeRef {
    pub node: Arc<Mutex<dyn Node>>,
    pub config_key: String,
    pub display_name: String,
    pub role: String,
    pub color: String,
    pub connected_node: Arc<ConnectedNodeImpl>,
}

pub fn collect_node_statuses(nodes: &[NodeRef]) -> Value {
    let mut statuses = serde_json::Map::new();
    for info in nodes {
        let (enabled, node_status) = match info.node.lock() {
            Ok(n) => (n.is_enabled(), n.status()),
            Err(_) => (false, Value::Null),
        };
        let mut entry = json!({
            "displayName": info.display_name,
            "role": info.role,
            "color": info.color,
            "enabled": enabled,
            "inputCount": info.connected_node.input_count(),
            "outputCount": info.connected_node.output_count(),
            "nodeStatus": node_status,
        });
        let logs = fusion_registry::drain_node_logs(&info.config_key);
        if !logs.is_empty() {
            entry["logs"] = json!(logs);
        }
        statuses.insert(info.config_key.clone(), entry);
    }
    Value::Object(statuses)
}
