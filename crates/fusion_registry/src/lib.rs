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
pub struct SelectOption {
    pub value: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingsField {
    pub key: String,
    pub label: String,
    #[serde(rename = "type")]
    pub field_type: String,
    pub default: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<SelectOption>>,
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
// Helper for node builders
// ---------------------------------------------------------------------------

pub fn extract_settings(config: &Value) -> Value {
    config
        .get("settings")
        .cloned()
        .unwrap_or_else(|| config.clone())
}
