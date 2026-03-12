use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use fusion_types::{ApiRequest, StreamableData};

// ---------------------------------------------------------------------------
// Node trait (moved from fusion::node)
// ---------------------------------------------------------------------------

pub type ConsumerCallback = Box<dyn Fn(StreamableData) + Send + Sync>;
pub type CommandConsumerCallback = Box<dyn Fn(ApiRequest) + Send + Sync>;

pub trait Node: Send + Sync {
    fn name(&self) -> &str;
    fn start(&mut self) -> anyhow::Result<()>;
    fn stop(&mut self) -> anyhow::Result<()>;
    fn is_enabled(&self) -> bool;
    fn set_enabled(&mut self, enabled: bool);
    fn receive_data(&mut self, _data: StreamableData) {}
    fn receive_command(&mut self, _cmd: &ApiRequest) {}
    fn set_on_output(&self, _callback: ConsumerCallback) {}
    fn set_on_command_output(&self, _callback: CommandConsumerCallback) {}
    fn status(&self) -> Value { Value::Null }
}

// ---------------------------------------------------------------------------
// Node metadata types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum NodeRole {
    Source,
    Filter,
    Sink,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingsField {
    pub key: String,
    pub label: String,
    #[serde(rename = "type")]
    pub field_type: String,
    pub default: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeSubtype {
    pub value: String,
    pub display_name: String,
    pub additional_settings: Vec<SettingsField>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeMetadata {
    pub id: String,
    pub display_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub role: NodeRole,
    pub config_aliases: Vec<String>,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub default_settings: Value,
    pub settings_schema: Vec<SettingsField>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtypes: Option<Vec<NodeSubtype>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required_feature: Option<String>,
    pub supports_realtime_config: bool,
    pub color: String,
}

// ---------------------------------------------------------------------------
// Global node registry
// ---------------------------------------------------------------------------

type NodeBuilderFn = Box<dyn Fn(&str, &Value) -> Result<Arc<Mutex<dyn Node>>> + Send + Sync>;

pub struct NodeRegistration {
    pub metadata: NodeMetadata,
    builder: NodeBuilderFn,
}

struct Registry {
    entries: Vec<NodeRegistration>,
}

impl Registry {
    fn new() -> Self {
        Self { entries: Vec::new() }
    }
}

static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();

fn registry() -> &'static Mutex<Registry> {
    REGISTRY.get_or_init(|| Mutex::new(Registry::new()))
}

pub fn register_node<F>(metadata: NodeMetadata, builder: F)
where
    F: Fn(&str, &Value) -> Result<Arc<Mutex<dyn Node>>> + Send + Sync + 'static,
{
    let mut reg = registry().lock().unwrap();
    reg.entries.push(NodeRegistration {
        metadata,
        builder: Box::new(builder),
    });
}

pub fn build_node(type_name: &str, config: &Value) -> Result<Arc<Mutex<dyn Node>>> {
    let reg = registry().lock().unwrap();
    for r in &reg.entries {
        if r.metadata.id == type_name
            || r.metadata.config_aliases.iter().any(|a| a == type_name)
        {
            return (r.builder)(type_name, config);
        }
    }
    bail!("Unknown node type: '{}'", type_name)
}

pub fn all_metadata() -> Vec<NodeMetadata> {
    let reg = registry().lock().unwrap();
    reg.entries.iter().map(|r| r.metadata.clone()).collect()
}

pub fn is_filter_key(key: &str) -> bool {
    let reg = registry().lock().unwrap();
    reg.entries.iter().any(|r| {
        r.metadata.role == NodeRole::Filter
            && (r.metadata.id == key || r.metadata.config_aliases.iter().any(|a| a == key))
    })
}

pub fn required_feature(key: &str) -> Option<String> {
    let reg = registry().lock().unwrap();
    for r in &reg.entries {
        if r.metadata.id == key || r.metadata.config_aliases.iter().any(|a| a == key) {
            return r.metadata.required_feature.clone();
        }
    }
    None
}

pub fn metadata_for_key(key: &str) -> Option<NodeMetadata> {
    let reg = registry().lock().unwrap();
    reg.entries
        .iter()
        .find(|r| r.metadata.id == key || r.metadata.config_aliases.iter().any(|a| a == key))
        .map(|r| r.metadata.clone())
}

pub fn supports_realtime_config(node_key: &str) -> bool {
    let reg = registry().lock().unwrap();
    reg.entries.iter().any(|r| {
        r.metadata.supports_realtime_config
            && (r.metadata.id == node_key
                || r.metadata.config_aliases.iter().any(|a| a == node_key))
    })
}

// ---------------------------------------------------------------------------
// UI extension registry
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiExtensionMetadata {
    pub id: String,
    pub display_name: String,
    pub route: String,
    pub nav_section: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_nodes: Vec<String>,
}

struct UiExtensionRegistration {
    metadata: UiExtensionMetadata,
    bundle: &'static [u8],
}

static UI_EXTENSIONS: OnceLock<Mutex<Vec<UiExtensionRegistration>>> = OnceLock::new();

fn ui_extensions() -> &'static Mutex<Vec<UiExtensionRegistration>> {
    UI_EXTENSIONS.get_or_init(|| Mutex::new(Vec::new()))
}

pub fn register_ui_extension(metadata: UiExtensionMetadata, bundle: &'static [u8]) {
    let mut exts = ui_extensions().lock().unwrap();
    exts.push(UiExtensionRegistration { metadata, bundle });
}

pub fn all_ui_extensions() -> Vec<UiExtensionMetadata> {
    let exts = ui_extensions().lock().unwrap();
    exts.iter().map(|e| e.metadata.clone()).collect()
}

pub fn get_ui_extension_bundle(id: &str) -> Option<&'static [u8]> {
    let exts = ui_extensions().lock().unwrap();
    exts.iter().find(|e| e.metadata.id == id).map(|e| e.bundle)
}

// ---------------------------------------------------------------------------
// Helpers for node builders & schema definition
// ---------------------------------------------------------------------------

pub fn extract_settings(config: &Value) -> Value {
    config
        .get("settings")
        .cloned()
        .unwrap_or_else(|| config.clone())
}

pub fn sf(key: &str, label: &str, ft: &str, default: Value) -> SettingsField {
    SettingsField { key: key.into(), label: label.into(), field_type: ft.into(), default }
}

pub fn defaults_from_schema(schema: &[SettingsField]) -> Value {
    let mut map = serde_json::Map::new();
    for field in schema {
        if let Some((parent, child)) = field.key.split_once('.') {
            let nested = map.entry(parent).or_insert_with(|| Value::Object(serde_json::Map::new()));
            if let Value::Object(ref mut m) = nested {
                m.insert(child.into(), field.default.clone());
            }
        } else {
            map.insert(field.key.clone(), field.default.clone());
        }
    }
    Value::Object(map)
}

// ---------------------------------------------------------------------------
// Per-node log buffer
// ---------------------------------------------------------------------------

const NODE_LOG_MAX_ENTRIES: usize = 500;

#[derive(Debug, Clone, Serialize)]
pub struct NodeLogEntry {
    pub ts: String,
    pub level: String,
    pub message: String,
}

pub struct NodeLogBuffer {
    entries: VecDeque<NodeLogEntry>,
    drain_cursor: usize,
}

impl NodeLogBuffer {
    fn new() -> Self {
        Self { entries: VecDeque::new(), drain_cursor: 0 }
    }

    pub fn push(&mut self, level: &str, message: &str) {
        if self.entries.len() >= NODE_LOG_MAX_ENTRIES {
            self.entries.pop_front();
            if self.drain_cursor > 0 {
                self.drain_cursor -= 1;
            }
        }
        self.entries.push_back(NodeLogEntry {
            ts: chrono::Local::now().format("%H:%M:%S%.3f").to_string(),
            level: level.to_owned(),
            message: message.to_owned(),
        });
    }

    /// Return entries added since the last drain call.
    pub fn drain_new(&mut self) -> Vec<NodeLogEntry> {
        let new_entries: Vec<_> = self.entries.iter().skip(self.drain_cursor).cloned().collect();
        self.drain_cursor = self.entries.len();
        new_entries
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.drain_cursor = 0;
    }
}

static NODE_LOG_REGISTRY: OnceLock<Mutex<HashMap<String, Arc<Mutex<NodeLogBuffer>>>>> =
    OnceLock::new();

fn node_log_registry() -> &'static Mutex<HashMap<String, Arc<Mutex<NodeLogBuffer>>>> {
    NODE_LOG_REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

static NODE_LOG_SEQ: AtomicU64 = AtomicU64::new(0);

/// Create a log buffer for a node. Returns the shared buffer reference.
pub fn init_node_logger(config_key: &str) -> Arc<Mutex<NodeLogBuffer>> {
    let mut reg = node_log_registry().lock().unwrap();
    let buf = reg
        .entry(config_key.to_owned())
        .or_insert_with(|| Arc::new(Mutex::new(NodeLogBuffer::new())));
    buf.clone()
}

/// Push a log entry to a node's buffer.
pub fn node_log(config_key: &str, level: &str, msg: impl fmt::Display) {
    let reg = node_log_registry().lock().unwrap();
    if let Some(buf) = reg.get(config_key) {
        buf.lock().unwrap().push(level, &msg.to_string());
    }
    let _ = NODE_LOG_SEQ.fetch_add(1, Ordering::Relaxed);
}

/// Drain new entries from a node's log buffer since the last drain.
pub fn drain_node_logs(config_key: &str) -> Vec<NodeLogEntry> {
    let reg = node_log_registry().lock().unwrap();
    if let Some(buf) = reg.get(config_key) {
        buf.lock().unwrap().drain_new()
    } else {
        Vec::new()
    }
}

/// Clear all node log buffers (call on engine restart).
pub fn clear_all_node_logs() {
    let reg = node_log_registry().lock().unwrap();
    for buf in reg.values() {
        buf.lock().unwrap().clear();
    }
}

/// Return all known node log keys.
pub fn node_log_keys() -> Vec<String> {
    let reg = node_log_registry().lock().unwrap();
    reg.keys().cloned().collect()
}
