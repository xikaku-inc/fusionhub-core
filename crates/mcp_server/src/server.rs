use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use fusion_registry::NodeRole;
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    schemars,
    service::RequestContext,
    tool, tool_handler, tool_router,
};
use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::RwLock;

use web_ui::ConfigHandle;
use websocket_server::CommandChannel;

use crate::data_buffer::DataBuffer;

const VERSION: &str = "1.0.0";
const MAX_RECENT_TOOLS: usize = 10;

// ---------------------------------------------------------------------------
// MCP status tracking
// ---------------------------------------------------------------------------

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallEntry {
    pub tool: String,
    pub time: String,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpStatus {
    pub connected: bool,
    pub client_name: Option<String>,
    pub connected_since: Option<String>,
    pub tool_call_count: u64,
    pub last_tool: Option<String>,
    pub last_tool_time: Option<String>,
    pub recent_tools: Vec<ToolCallEntry>,
}

// ---------------------------------------------------------------------------
// Tool parameter types
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct UpdateNodeSettingParams {
    #[schemars(description = "JSON pointer path, e.g. /sinks/fusion/settings/gain")]
    pub path: String,
    #[schemars(description = "New value for the setting")]
    pub value: Value,
    #[schemars(description = "If true, create intermediate objects along the path if they don't exist. Default: false")]
    pub create_path: Option<bool>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GetLatestDataParams {
    #[schemars(description = "Event type, e.g. fusedPose, inputStatus, nodeStatuses, fusedVehiclePose, opticalData")]
    pub event_type: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GetPoseHistoryParams {
    #[schemars(description = "Seconds of history to return")]
    pub last_secs: f64,
    #[schemars(description = "Maximum samples to return (default 100)")]
    pub max_samples: Option<usize>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct AddNodeParams {
    #[schemars(description = "Config key for the node (e.g. 'imu', 'fusion', 'logger'). Must be unique within its section.")]
    pub key: String,
    #[schemars(description = "Node type ID from the registry (e.g. 'openZen', 'gnssImuFusion', 'echo'). Use list_node_types to see available types.")]
    pub node_type: String,
    #[schemars(description = "Output data endpoint (e.g. 'inproc://imu_data' or 'tcp://*:8799'). Auto-generated as 'inproc://<key>_data' for sources/filters if omitted. Sinks typically omit this.")]
    pub endpoint: Option<String>,
    #[schemars(description = "Input endpoints to subscribe to (e.g. ['inproc://imu_data']). For filters and sinks. If omitted, uses pipeline accumulated defaults on restart.")]
    pub input_endpoints: Option<Vec<String>>,
    #[schemars(description = "Node settings merged over registry defaults (e.g. {\"sampleRate\": 200}). Use list_node_types to see settings_schema. If omitted, uses registry default_settings.")]
    pub settings: Option<Value>,
    #[schemars(description = "Add in disabled state (key prefixed with '_'). Default: false")]
    pub disabled: Option<bool>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct RemoveNodeParams {
    #[schemars(description = "Config key of the node to remove (e.g. 'imu', 'fusion', '_logger'). Searches both sources and sinks.")]
    pub key: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SetNodeEnabledParams {
    #[schemars(description = "Config key of the node (without '_' prefix, e.g. 'logger' not '_logger')")]
    pub key: String,
    #[schemars(description = "true to enable the node, false to disable it")]
    pub enabled: bool,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ConnectNodesParams {
    #[schemars(description = "Config key of the upstream (output) node, e.g. 'imu' or 'fusion'")]
    pub from_node: String,
    #[schemars(description = "Config key of the downstream (input) node, e.g. 'fusion' or 'echo'")]
    pub to_node: String,
    #[schemars(description = "Explicit endpoint to use. If omitted, auto-detected from upstream node's outEndpoint/dataEndpoint.")]
    pub endpoint: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct DisconnectNodesParams {
    #[schemars(description = "Config key of the upstream (output) node to disconnect from")]
    pub from_node: String,
    #[schemars(description = "Config key of the downstream (input) node")]
    pub to_node: String,
    #[schemars(description = "Explicit endpoint to remove. If omitted, auto-detected from upstream node's outEndpoint/dataEndpoint.")]
    pub endpoint: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ProbeNodeParams {
    #[schemars(description = "Config key of the node to probe (e.g. 'imu', 'fusion')")]
    pub node: String,
    #[schemars(description = "Max samples to collect (default 5, max 50)")]
    pub max_samples: Option<usize>,
    #[schemars(description = "Timeout in seconds (default 3.0, max 10.0)")]
    pub timeout_secs: Option<f64>,
}

// ---------------------------------------------------------------------------
// FusionMcpServer
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct FusionMcpServer {
    tool_router: ToolRouter<Self>,
    buffer: Arc<RwLock<DataBuffer>>,
    config_handle: ConfigHandle,
    command_channel: CommandChannel,
    paused: Arc<AtomicBool>,
    pub(crate) status: Arc<RwLock<McpStatus>>,
}

#[tool_router]
impl FusionMcpServer {
    pub fn new(
        buffer: Arc<RwLock<DataBuffer>>,
        config_handle: ConfigHandle,
        command_channel: CommandChannel,
        paused: Arc<AtomicBool>,
        status: Arc<RwLock<McpStatus>>,
    ) -> Self {
        Self {
            tool_router: Self::tool_router(),
            buffer,
            config_handle,
            command_channel,
            paused,
            status,
        }
    }

    async fn record_tool_call(&self, name: &str) {
        let now = Utc::now().to_rfc3339();
        let mut s = self.status.write().await;
        s.tool_call_count += 1;
        s.last_tool = Some(name.to_string());
        s.last_tool_time = Some(now.clone());
        s.recent_tools.push(ToolCallEntry {
            tool: name.to_string(),
            time: now,
        });
        if s.recent_tools.len() > MAX_RECENT_TOOLS {
            s.recent_tools.remove(0);
        }
    }

    #[tool(description = "Get the current FusionHub pipeline configuration as JSON")]
    async fn get_config(&self) -> Result<CallToolResult, McpError> {
        self.record_tool_call("get_config").await;
        let config = self.config_handle.get().await;
        let text = serde_json::to_string_pretty(&config).unwrap_or_else(|_| "{}".into());
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    #[tool(description = "Update a specific node setting by JSON pointer path. Changes are applied in real-time if the node supports it. Set create_path=true to create intermediate objects for new settings.")]
    async fn update_node_setting(
        &self,
        Parameters(params): Parameters<UpdateNodeSettingParams>,
    ) -> Result<CallToolResult, McpError> {
        self.record_tool_call("update_node_setting").await;
        let mut config = self.config_handle.get().await;
        if config.pointer(&params.path).is_none() {
            if params.create_path.unwrap_or(false) {
                ensure_pointer_path(&mut config, &params.path);
            } else {
                return Ok(CallToolResult::error(vec![Content::text(format!(
                    "Path '{}' not found in config. Use create_path=true to create it.",
                    params.path
                ))]));
            }
        }
        if let Some(target) = config.pointer_mut(&params.path) {
            *target = params.value;
        }
        let node_path = extract_node_settings_path(&params.path);
        if let Some(settings) = config.pointer(&node_path).cloned() {
            let req = fusion_types::ApiRequest::new("setConfigJsonPath", &node_path, settings, "");
            self.command_channel.send(&req);
        }
        self.config_handle.set(config).await;
        Ok(CallToolResult::success(vec![Content::text(
            "Setting updated",
        )]))
    }

    #[tool(description = "Save the current configuration to disk and restart the pipeline")]
    async fn save_config(&self) -> Result<CallToolResult, McpError> {
        self.record_tool_call("save_config").await;
        let config = self.config_handle.get().await;
        let path = self.config_handle.config_path().await;
        let content = serde_json::to_string_pretty(&config)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        std::fs::write(&path, &content)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        self.config_handle.trigger_restart().await;
        Ok(CallToolResult::success(vec![Content::text(
            "Config saved, pipeline restarting",
        )]))
    }

    #[tool(description = "Restart the FusionHub pipeline")]
    async fn restart_pipeline(&self) -> Result<CallToolResult, McpError> {
        self.record_tool_call("restart_pipeline").await;
        self.config_handle.trigger_restart().await;
        Ok(CallToolResult::success(vec![Content::text(
            "Pipeline restart triggered",
        )]))
    }

    #[tool(description = "Pause data processing in the pipeline")]
    async fn pause_pipeline(&self) -> Result<CallToolResult, McpError> {
        self.record_tool_call("pause_pipeline").await;
        self.paused.store(true, Ordering::Relaxed);
        Ok(CallToolResult::success(vec![Content::text(
            "Pipeline paused",
        )]))
    }

    #[tool(description = "Resume data processing in the pipeline")]
    async fn resume_pipeline(&self) -> Result<CallToolResult, McpError> {
        self.record_tool_call("resume_pipeline").await;
        self.paused.store(false, Ordering::Relaxed);
        Ok(CallToolResult::success(vec![Content::text(
            "Pipeline resumed",
        )]))
    }

    #[tool(description = "Get pipeline status: running/paused state and node statuses")]
    async fn get_status(&self) -> Result<CallToolResult, McpError> {
        self.record_tool_call("get_status").await;
        let paused = self.paused.load(Ordering::Relaxed);
        let buf = self.buffer.read().await;
        let node_statuses = buf.latest("nodeStatuses").cloned().unwrap_or(Value::Null);
        let input_status = buf.latest("inputStatus").cloned().unwrap_or(Value::Null);
        let status = json!({
            "version": VERSION,
            "paused": paused,
            "nodeStatuses": node_statuses,
            "inputStatus": input_status,
        });
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&status).unwrap(),
        )]))
    }

    #[tool(description = "List all registered node types with metadata (id, role, inputs, outputs, settings schema)")]
    async fn list_node_types(&self) -> Result<CallToolResult, McpError> {
        self.record_tool_call("list_node_types").await;
        let metadata = fusion_registry::all_metadata();
        let text = serde_json::to_string_pretty(&metadata).unwrap_or_else(|_| "[]".into());
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    #[tool(description = "Get the latest value for a specific SSE event type (e.g. fusedPose, inputStatus, nodeStatuses, fusedVehiclePose, opticalData)")]
    async fn get_latest_data(
        &self,
        Parameters(params): Parameters<GetLatestDataParams>,
    ) -> Result<CallToolResult, McpError> {
        self.record_tool_call("get_latest_data").await;
        let buf = self.buffer.read().await;
        match buf.latest(&params.event_type) {
            Some(data) => Ok(CallToolResult::success(vec![Content::text(
                serde_json::to_string_pretty(data).unwrap(),
            )])),
            None => Ok(CallToolResult::success(vec![Content::text(format!(
                "No data available for '{}'",
                params.event_type
            ))])),
        }
    }

    #[tool(description = "Get fused pose history for the last N seconds, downsampled to max_samples (default 100). Returns position, orientation, velocity over time.")]
    async fn get_pose_history(
        &self,
        Parameters(params): Parameters<GetPoseHistoryParams>,
    ) -> Result<CallToolResult, McpError> {
        self.record_tool_call("get_pose_history").await;
        let buf = self.buffer.read().await;
        let max_samples = params.max_samples.unwrap_or(100);
        let entries = buf.history("fusedPose", params.last_secs, max_samples);
        let text = serde_json::to_string_pretty(&entries).unwrap();
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    #[tool(description = "Add a new node (source, filter, or sink) to the pipeline config. The node type must exist in the registry (use list_node_types to discover types). Call save_config after to apply changes.")]
    async fn add_node(
        &self,
        Parameters(params): Parameters<AddNodeParams>,
    ) -> Result<CallToolResult, McpError> {
        self.record_tool_call("add_node").await;
        let metadata = match fusion_registry::metadata_for_key(&params.node_type) {
            Some(m) => m,
            None => {
                let all: Vec<String> = fusion_registry::all_metadata()
                    .iter()
                    .map(|m| format!("{} ({})", m.id, match m.role {
                        NodeRole::Source => "source",
                        NodeRole::Filter => "filter",
                        NodeRole::Sink => "sink",
                    }))
                    .collect();
                return Ok(CallToolResult::error(vec![Content::text(format!(
                    "Unknown node type '{}'. Available types:\n{}",
                    params.node_type,
                    all.join("\n")
                ))]));
            }
        };

        let section = match metadata.role {
            NodeRole::Source => "sources",
            NodeRole::Filter | NodeRole::Sink => "sinks",
        };

        let final_key = if params.disabled.unwrap_or(false) {
            format!("_{}", params.key)
        } else {
            params.key.clone()
        };

        let mut config = self.config_handle.get().await;
        if let Some(obj) = config.get(section).and_then(|s| s.as_object()) {
            if obj.contains_key(&final_key) {
                return Ok(CallToolResult::error(vec![Content::text(format!(
                    "Key '{}' already exists in config.{}. Choose a different key or remove the existing node first.",
                    final_key, section
                ))]));
            }
        }

        // Build node config
        let mut node_config = json!({});
        node_config["type"] = Value::String(metadata.id.clone());

        // Endpoint
        let endpoint = match &params.endpoint {
            Some(ep) => Some(ep.clone()),
            None => match metadata.role {
                NodeRole::Source | NodeRole::Filter => {
                    Some(format!("inproc://{}_data", params.key))
                }
                NodeRole::Sink => None,
            },
        };
        if let Some(ref ep) = endpoint {
            let ep_field = match metadata.role {
                NodeRole::Source => "outEndpoint",
                _ => "dataEndpoint",
            };
            node_config[ep_field] = Value::String(ep.clone());
        }

        // Input endpoints
        if let Some(ref inputs) = params.input_endpoints {
            node_config["inputEndpoints"] = json!(inputs);
        }

        // Settings: merge user settings over defaults
        let default_settings = &metadata.default_settings;
        let settings = match params.settings {
            Some(user_settings) => {
                let mut merged = default_settings.clone();
                if let (Some(base), Some(overlay)) = (merged.as_object_mut(), user_settings.as_object()) {
                    for (k, v) in overlay {
                        base.insert(k.clone(), v.clone());
                    }
                }
                merged
            }
            None => default_settings.clone(),
        };
        if settings.as_object().map_or(false, |o| !o.is_empty()) {
            node_config["settings"] = settings;
        }

        // Insert into config
        if config.get(section).is_none() {
            config[section] = json!({});
        }
        config[section][&final_key] = node_config;
        self.config_handle.set(config).await;

        let role_str = match metadata.role {
            NodeRole::Source => "source",
            NodeRole::Filter => "filter",
            NodeRole::Sink => "sink",
        };
        let ep_msg = endpoint.as_deref().unwrap_or("none");
        Ok(CallToolResult::success(vec![Content::text(format!(
            "Added {} '{}' (type: {}) to config.{}. Endpoint: {}. Call save_config to apply.",
            role_str, final_key, metadata.id, section, ep_msg
        ))]))
    }

    #[tool(description = "Remove a node from the pipeline config by its key. Searches both sources and sinks sections. Call save_config after to apply changes.")]
    async fn remove_node(
        &self,
        Parameters(params): Parameters<RemoveNodeParams>,
    ) -> Result<CallToolResult, McpError> {
        self.record_tool_call("remove_node").await;
        let mut config = self.config_handle.get().await;

        let (section, actual_key) = match find_node_in_config(&config, &params.key) {
            Some((s, k, _)) => (s, k),
            None => {
                let (src_keys, sink_keys) = list_all_node_keys(&config);
                return Ok(CallToolResult::error(vec![Content::text(format!(
                    "Node '{}' not found. Sources: [{}]. Sinks: [{}]",
                    params.key,
                    src_keys.join(", "),
                    sink_keys.join(", ")
                ))]));
            }
        };

        // Check for endpoint references before removing
        let removed_config = config[&section][&actual_key].clone();
        let removed_ep = resolve_output_endpoint(&removed_config);

        config[&section]
            .as_object_mut()
            .unwrap()
            .remove(&actual_key);

        // Scan for dangling references
        let mut warnings = Vec::new();
        if let Some(ref ep) = removed_ep {
            for section_name in &["sources", "sinks"] {
                if let Some(obj) = config.get(section_name).and_then(|s| s.as_object()) {
                    for (key, node_cfg) in obj {
                        if let Some(inputs) = node_cfg.get("inputEndpoints").and_then(|v| v.as_array()) {
                            if inputs.iter().any(|v| v.as_str() == Some(ep)) {
                                warnings.push(key.clone());
                            }
                        }
                    }
                }
            }
        }

        self.config_handle.set(config).await;

        let mut msg = format!(
            "Removed '{}' from config.{}. Call save_config to apply.",
            actual_key, section
        );
        if !warnings.is_empty() {
            msg.push_str(&format!(
                "\nWarning: these nodes still reference endpoint '{}': [{}]. Consider updating their inputEndpoints.",
                removed_ep.unwrap_or_default(),
                warnings.join(", ")
            ));
        }
        Ok(CallToolResult::success(vec![Content::text(msg)]))
    }

    #[tool(description = "Enable or disable a node by toggling the '_' prefix on its config key. Disabled nodes are skipped during pipeline startup. Call save_config after to apply changes.")]
    async fn set_node_enabled(
        &self,
        Parameters(params): Parameters<SetNodeEnabledParams>,
    ) -> Result<CallToolResult, McpError> {
        self.record_tool_call("set_node_enabled").await;
        let mut config = self.config_handle.get().await;
        let base_key = params.key.strip_prefix('_').unwrap_or(&params.key);
        let disabled_key = format!("_{}", base_key);

        // Find the node
        let (section, actual_key) = match find_node_in_config(&config, base_key) {
            Some((s, k, _)) => (s, k),
            None => {
                let (src_keys, sink_keys) = list_all_node_keys(&config);
                return Ok(CallToolResult::error(vec![Content::text(format!(
                    "Node '{}' not found. Sources: [{}]. Sinks: [{}]",
                    base_key,
                    src_keys.join(", "),
                    sink_keys.join(", ")
                ))]));
            }
        };

        let is_currently_enabled = !actual_key.starts_with('_');
        if is_currently_enabled == params.enabled {
            let state = if params.enabled { "enabled" } else { "disabled" };
            return Ok(CallToolResult::success(vec![Content::text(format!(
                "Node '{}' is already {}. No changes made.",
                base_key, state
            ))]));
        }

        let new_key = if params.enabled {
            base_key.to_string()
        } else {
            disabled_key
        };

        // Move the value from old key to new key
        let value = config[&section][&actual_key].clone();
        config[&section]
            .as_object_mut()
            .unwrap()
            .remove(&actual_key);
        config[&section][&new_key] = value;

        self.config_handle.set(config).await;

        let state = if params.enabled { "enabled" } else { "disabled" };
        Ok(CallToolResult::success(vec![Content::text(format!(
            "Node '{}' is now {} (key: '{}'). Call save_config to apply.",
            base_key, state, new_key
        ))]))
    }

    #[tool(description = "Connect two nodes by adding the upstream node's output endpoint to the downstream node's inputEndpoints array. Call save_config after to apply changes.")]
    async fn connect_nodes(
        &self,
        Parameters(params): Parameters<ConnectNodesParams>,
    ) -> Result<CallToolResult, McpError> {
        self.record_tool_call("connect_nodes").await;
        let mut config = self.config_handle.get().await;

        // Find upstream node and resolve endpoint
        let endpoint = match params.endpoint {
            Some(ep) => ep,
            None => {
                let (_, _, from_cfg) = match find_node_in_config(&config, &params.from_node) {
                    Some(found) => found,
                    None => {
                        return Ok(CallToolResult::error(vec![Content::text(format!(
                            "Upstream node '{}' not found in config",
                            params.from_node
                        ))]));
                    }
                };
                match resolve_output_endpoint(&from_cfg) {
                    Some(ep) => ep,
                    None => {
                        return Ok(CallToolResult::error(vec![Content::text(format!(
                            "Node '{}' has no output endpoint (outEndpoint/dataEndpoint). Specify the endpoint parameter explicitly.",
                            params.from_node
                        ))]));
                    }
                }
            }
        };

        // Find downstream node
        let (to_section, to_key, _) = match find_node_in_config(&config, &params.to_node) {
            Some(found) => found,
            None => {
                return Ok(CallToolResult::error(vec![Content::text(format!(
                    "Downstream node '{}' not found in config",
                    params.to_node
                ))]));
            }
        };

        // Add endpoint to inputEndpoints
        let node_cfg = &mut config[&to_section][&to_key];
        let inputs = node_cfg
            .get("inputEndpoints")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        if inputs.iter().any(|v| v.as_str() == Some(&endpoint)) {
            return Ok(CallToolResult::success(vec![Content::text(format!(
                "'{}' is already connected to '{}' via '{}'. No changes made.",
                params.to_node, params.from_node, endpoint
            ))]));
        }

        let mut new_inputs: Vec<Value> = inputs;
        new_inputs.push(Value::String(endpoint.clone()));
        node_cfg["inputEndpoints"] = json!(new_inputs);

        self.config_handle.set(config).await;

        let input_strs: Vec<&str> = new_inputs
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        Ok(CallToolResult::success(vec![Content::text(format!(
            "Connected '{}' -> '{}' via endpoint '{}'. {}.inputEndpoints: [{}]. Call save_config to apply.",
            params.from_node, params.to_node, endpoint, to_key,
            input_strs.join(", ")
        ))]))
    }

    #[tool(description = "Disconnect two nodes by removing the upstream node's endpoint from the downstream node's inputEndpoints array. Call save_config after to apply changes.")]
    async fn disconnect_nodes(
        &self,
        Parameters(params): Parameters<DisconnectNodesParams>,
    ) -> Result<CallToolResult, McpError> {
        self.record_tool_call("disconnect_nodes").await;
        let mut config = self.config_handle.get().await;

        // Resolve endpoint
        let endpoint = match params.endpoint {
            Some(ep) => ep,
            None => {
                let (_, _, from_cfg) = match find_node_in_config(&config, &params.from_node) {
                    Some(found) => found,
                    None => {
                        return Ok(CallToolResult::error(vec![Content::text(format!(
                            "Upstream node '{}' not found in config",
                            params.from_node
                        ))]));
                    }
                };
                match resolve_output_endpoint(&from_cfg) {
                    Some(ep) => ep,
                    None => {
                        return Ok(CallToolResult::error(vec![Content::text(format!(
                            "Node '{}' has no output endpoint. Specify the endpoint parameter explicitly.",
                            params.from_node
                        ))]));
                    }
                }
            }
        };

        // Find downstream node
        let (to_section, to_key, _) = match find_node_in_config(&config, &params.to_node) {
            Some(found) => found,
            None => {
                return Ok(CallToolResult::error(vec![Content::text(format!(
                    "Downstream node '{}' not found in config",
                    params.to_node
                ))]));
            }
        };

        let node_cfg = &mut config[&to_section][&to_key];
        let inputs = match node_cfg.get("inputEndpoints").and_then(|v| v.as_array()) {
            Some(arr) => arr.clone(),
            None => {
                return Ok(CallToolResult::error(vec![Content::text(format!(
                    "'{}' has no inputEndpoints array. Nothing to disconnect.",
                    params.to_node
                ))]));
            }
        };

        let new_inputs: Vec<Value> = inputs
            .into_iter()
            .filter(|v| v.as_str() != Some(&endpoint))
            .collect();

        if new_inputs.len() == node_cfg["inputEndpoints"].as_array().unwrap().len() {
            return Ok(CallToolResult::error(vec![Content::text(format!(
                "Endpoint '{}' not found in '{}'.inputEndpoints. Current: [{}]",
                endpoint,
                params.to_node,
                node_cfg["inputEndpoints"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(|v| v.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))]));
        }

        node_cfg["inputEndpoints"] = json!(new_inputs);
        self.config_handle.set(config).await;

        let input_strs: Vec<&str> = new_inputs
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        Ok(CallToolResult::success(vec![Content::text(format!(
            "Disconnected '{}' from '{}' (removed endpoint '{}'). {}.inputEndpoints: [{}]. Call save_config to apply.",
            params.from_node, params.to_node, endpoint, to_key,
            input_strs.join(", ")
        ))]))
    }

    #[tool(description = "Probe a node's output endpoint and return live sensor data samples. Subscribes to the node's ZMQ/inproc endpoint, collects a few samples, and returns them as JSON. Use this to inspect actual IMU readings, GNSS positions, fused poses, or any data flowing through the graph.")]
    async fn probe_node(
        &self,
        Parameters(params): Parameters<ProbeNodeParams>,
    ) -> Result<CallToolResult, McpError> {
        self.record_tool_call("probe_node").await;
        let config = self.config_handle.get().await;

        let (_, _, node_cfg) = match find_node_in_config(&config, &params.node) {
            Some(found) => found,
            None => {
                let (src_keys, sink_keys) = list_all_node_keys(&config);
                return Ok(CallToolResult::error(vec![Content::text(format!(
                    "Node '{}' not found. Sources: [{}]. Sinks: [{}]",
                    params.node,
                    src_keys.join(", "),
                    sink_keys.join(", ")
                ))]));
            }
        };

        let endpoint = match resolve_output_endpoint(&node_cfg) {
            Some(ep) => ep,
            None => {
                return Ok(CallToolResult::error(vec![Content::text(format!(
                    "Node '{}' has no output endpoint (outEndpoint/dataEndpoint).",
                    params.node
                ))]));
            }
        };

        let max_samples = params.max_samples.unwrap_or(5).min(50);
        let timeout = Duration::from_secs_f64(params.timeout_secs.unwrap_or(3.0).min(10.0));

        match networking::probe::probe_endpoint(&endpoint, max_samples, timeout).await {
            Ok(samples) => {
                let json_samples: Vec<Value> = samples
                    .iter()
                    .map(|s| json!({ "type": s.variant_name(), "data": s }))
                    .collect();
                let result = json!({
                    "endpoint": endpoint,
                    "sampleCount": json_samples.len(),
                    "samples": json_samples,
                });
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string_pretty(&result).unwrap(),
                )]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Probe failed for '{}' ({}): {}",
                params.node, endpoint, e
            ))])),
        }
    }

    #[tool(description = "List all node endpoints in the pipeline, showing which are currently active (have data flowing).")]
    async fn list_endpoints(&self) -> Result<CallToolResult, McpError> {
        self.record_tool_call("list_endpoints").await;
        let config = self.config_handle.get().await;
        let mut entries = Vec::new();

        for section in &["sources", "sinks"] {
            if let Some(obj) = config.get(section).and_then(|s| s.as_object()) {
                for (key, node_cfg) in obj {
                    if let Some(ep) = resolve_output_endpoint(node_cfg) {
                        let active = networking::is_inproc(&ep)
                            && networking::get_data_channel(&ep).is_some();
                        entries.push(json!({
                            "node": key,
                            "section": section,
                            "endpoint": ep,
                            "active": active,
                        }));
                    }
                }
            }
        }

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&entries).unwrap(),
        )]))
    }
}

// ---------------------------------------------------------------------------
// ServerHandler — MCP protocol integration
// ---------------------------------------------------------------------------

#[tool_handler]
impl ServerHandler for FusionMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2024_11_05,
            capabilities: ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
            server_info: Implementation::from_build_env(),
            instructions: Some(
                "FusionHub is a real-time sensor fusion engine. It processes data from IMU, \
                 GNSS, optical tracking, CAN bus, and other sensors through a configurable \
                 pipeline of Source → Filter → Sink nodes connected via ZMQ pub/sub.\n\n\
                 DATA ANALYSIS:\n\
                 - Use probe_node to read live sensor values from any node's output (IMU accel/gyro, GNSS lat/lon, fused poses, etc.)\n\
                 - Use list_endpoints to see all node endpoints and which are currently active\n\
                 - Use get_status for pipeline health, node statuses, and data flow rates\n\
                 - Use get_latest_data with event types: fusedPose, fusedVehiclePose, inputStatus, opticalData, nodeStatuses\n\
                 - Use get_pose_history to analyze motion over time (returns downsampled time-series)\n\n\
                 PIPELINE BUILDING:\n\
                 - Use list_node_types to discover available node types with settings schemas, inputs, and outputs\n\
                 - Use add_node to create sources, filters, and sinks\n\
                 - Use connect_nodes to wire node outputs to node inputs via endpoints\n\
                 - Use update_node_setting to fine-tune individual parameters (supports create_path for new fields)\n\
                 - Use save_config to persist changes and restart the pipeline\n\n\
                 PIPELINE MODIFICATION:\n\
                 - Use set_node_enabled to enable/disable nodes without removing them (toggles '_' prefix)\n\
                 - Use disconnect_nodes to remove connections between nodes\n\
                 - Use remove_node to permanently remove a node from the config\n\
                 - All structural changes are staged in memory — call save_config to apply\n\n\
                 CONFIGURATION STRUCTURE:\n\
                 - config.sources: input nodes (IMU, GNSS, optical, CAN, etc.)\n\
                 - config.sinks: filters AND output sinks (fusion filters, file loggers, MQTT, etc.)\n\
                 - Keys prefixed with '_' are disabled nodes\n\
                 - Nodes connect via ZMQ endpoints (inproc:// for in-process, tcp:// for network)\n\
                 - JSON pointer paths: /sources/imu/settings/sampleRate or /sinks/fusion/settings/gain"
                    .into(),
            ),
        }
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        let resources = vec![
            ("fusionhub://config", "Current Configuration", "The current FusionHub pipeline configuration"),
            ("fusionhub://node-types", "Node Type Catalog", "All registered node types and their metadata"),
            ("fusionhub://status", "System Status", "Current pipeline status and data flow info"),
        ];
        Ok(ListResourcesResult {
            resources: resources
                .into_iter()
                .map(|(uri, name, desc)| {
                    let mut r = RawResource::new(uri, name.to_string());
                    r.description = Some(desc.to_string());
                    r.mime_type = Some("application/json".to_string());
                    r.no_annotation()
                })
                .collect(),
            next_cursor: None,
            meta: None,
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, McpError> {
        let content = match request.uri.as_str() {
            "fusionhub://config" => {
                let config = self.config_handle.get().await;
                serde_json::to_string_pretty(&config).unwrap()
            }
            "fusionhub://node-types" => {
                let metadata = fusion_registry::all_metadata();
                serde_json::to_string_pretty(&metadata).unwrap()
            }
            "fusionhub://status" => {
                let paused = self.paused.load(Ordering::Relaxed);
                let buf = self.buffer.read().await;
                let node_statuses =
                    buf.latest("nodeStatuses").cloned().unwrap_or(Value::Null);
                serde_json::to_string_pretty(&json!({
                    "version": VERSION,
                    "paused": paused,
                    "nodeStatuses": node_statuses,
                }))
                .unwrap()
            }
            _ => {
                return Err(McpError::resource_not_found(
                    "resource_not_found",
                    Some(json!({ "uri": request.uri })),
                ))
            }
        };
        Ok(ReadResourceResult {
            contents: vec![ResourceContents::text(content, request.uri)],
        })
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Extract the path to the node's settings object from a full JSON pointer.
/// e.g. "/sinks/fusion/settings/gain" -> "/sinks/fusion/settings"
fn extract_node_settings_path(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() >= 4 {
        parts[..4].join("/")
    } else {
        path.to_owned()
    }
}

/// Search sources and sinks for a node key (tries key and _{key}).
/// Returns (section_name, actual_key, node_config_clone).
fn find_node_in_config(config: &Value, key: &str) -> Option<(String, String, Value)> {
    let base = key.strip_prefix('_').unwrap_or(key);
    let variants = [base.to_string(), format!("_{}", base)];
    for section in &["sources", "sinks"] {
        if let Some(obj) = config.get(section).and_then(|s| s.as_object()) {
            for variant in &variants {
                if let Some(val) = obj.get(variant.as_str()) {
                    return Some((section.to_string(), variant.clone(), val.clone()));
                }
            }
        }
    }
    None
}

/// Collect all node keys from both sections.
fn list_all_node_keys(config: &Value) -> (Vec<String>, Vec<String>) {
    let src = config
        .get("sources")
        .and_then(|s| s.as_object())
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default();
    let sink = config
        .get("sinks")
        .and_then(|s| s.as_object())
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default();
    (src, sink)
}

/// Read output endpoint from a node config object.
fn resolve_output_endpoint(node_config: &Value) -> Option<String> {
    node_config
        .get("outEndpoint")
        .or_else(|| node_config.get("dataEndpoint"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            node_config
                .get("settings")
                .and_then(|s| s.get("endpoints"))
                .and_then(|a| a.as_array())
                .and_then(|a| a.first())
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
}

/// Create intermediate JSON objects along a pointer path.
fn ensure_pointer_path(root: &mut Value, path: &str) {
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let mut current = root;
    for seg in &segments {
        if current.get(seg).is_none() {
            current[seg] = json!({});
        }
        current = current.get_mut(seg).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_config() -> Value {
        json!({
            "sources": {
                "imu": {
                    "type": "OpenZen",
                    "outEndpoint": "inproc://imu_data",
                    "settings": { "streamFrequency": 500 }
                },
                "gnss": {
                    "type": "NMEA",
                    "dataEndpoint": "inproc://gnss_data",
                    "settings": { "port": "COM9", "baudrate": 921600 }
                },
                "_optical": {
                    "type": "DTrack",
                    "settings": { "endpoints": ["inproc://optical_data"] }
                }
            },
            "sinks": {
                "fusion": {
                    "type": "ImuOpticalFusion",
                    "dataEndpoint": "tcp://*:8799",
                    "inputEndpoints": ["inproc://imu_data", "inproc://optical_data"],
                    "settings": { "gain": 1.0 }
                },
                "echo": {},
                "_logger": {
                    "type": "FileLogger",
                    "inputEndpoints": ["tcp://localhost:8799"]
                }
            }
        })
    }

    // -----------------------------------------------------------------------
    // extract_node_settings_path
    // -----------------------------------------------------------------------

    #[test]
    fn extract_settings_path_deep() {
        assert_eq!(
            extract_node_settings_path("/sinks/fusion/settings/gain"),
            "/sinks/fusion/settings"
        );
    }

    #[test]
    fn extract_settings_path_exact() {
        assert_eq!(
            extract_node_settings_path("/sinks/fusion/settings"),
            "/sinks/fusion/settings"
        );
    }

    #[test]
    fn extract_settings_path_short() {
        assert_eq!(
            extract_node_settings_path("/sinks/fusion"),
            "/sinks/fusion"
        );
    }

    // -----------------------------------------------------------------------
    // find_node_in_config
    // -----------------------------------------------------------------------

    #[test]
    fn find_source_by_key() {
        let config = sample_config();
        let (section, key, val) = find_node_in_config(&config, "imu").unwrap();
        assert_eq!(section, "sources");
        assert_eq!(key, "imu");
        assert_eq!(val["type"], "OpenZen");
    }

    #[test]
    fn find_sink_by_key() {
        let config = sample_config();
        let (section, key, _) = find_node_in_config(&config, "fusion").unwrap();
        assert_eq!(section, "sinks");
        assert_eq!(key, "fusion");
    }

    #[test]
    fn find_disabled_node_by_base_key() {
        let config = sample_config();
        let (section, key, _) = find_node_in_config(&config, "logger").unwrap();
        assert_eq!(section, "sinks");
        assert_eq!(key, "_logger");
    }

    #[test]
    fn find_disabled_node_by_prefixed_key() {
        let config = sample_config();
        let (section, key, _) = find_node_in_config(&config, "_optical").unwrap();
        assert_eq!(section, "sources");
        assert_eq!(key, "_optical");
    }

    #[test]
    fn find_nonexistent_returns_none() {
        let config = sample_config();
        assert!(find_node_in_config(&config, "nonexistent").is_none());
    }

    // -----------------------------------------------------------------------
    // list_all_node_keys
    // -----------------------------------------------------------------------

    #[test]
    fn list_keys_from_both_sections() {
        let config = sample_config();
        let (src, sink) = list_all_node_keys(&config);
        assert!(src.contains(&"imu".to_string()));
        assert!(src.contains(&"gnss".to_string()));
        assert!(src.contains(&"_optical".to_string()));
        assert!(sink.contains(&"fusion".to_string()));
        assert!(sink.contains(&"echo".to_string()));
        assert!(sink.contains(&"_logger".to_string()));
    }

    #[test]
    fn list_keys_empty_config() {
        let config = json!({});
        let (src, sink) = list_all_node_keys(&config);
        assert!(src.is_empty());
        assert!(sink.is_empty());
    }

    // -----------------------------------------------------------------------
    // resolve_output_endpoint
    // -----------------------------------------------------------------------

    #[test]
    fn resolve_out_endpoint() {
        let node = json!({ "outEndpoint": "inproc://imu_data" });
        assert_eq!(resolve_output_endpoint(&node).unwrap(), "inproc://imu_data");
    }

    #[test]
    fn resolve_data_endpoint() {
        let node = json!({ "dataEndpoint": "tcp://*:8799" });
        assert_eq!(resolve_output_endpoint(&node).unwrap(), "tcp://*:8799");
    }

    #[test]
    fn resolve_settings_endpoints_fallback() {
        let node = json!({ "settings": { "endpoints": ["inproc://optical_data"] } });
        assert_eq!(resolve_output_endpoint(&node).unwrap(), "inproc://optical_data");
    }

    #[test]
    fn resolve_out_endpoint_preferred_over_data() {
        let node = json!({ "outEndpoint": "inproc://a", "dataEndpoint": "inproc://b" });
        assert_eq!(resolve_output_endpoint(&node).unwrap(), "inproc://a");
    }

    #[test]
    fn resolve_no_endpoint_returns_none() {
        let node = json!({ "type": "Echo" });
        assert!(resolve_output_endpoint(&node).is_none());
    }

    // -----------------------------------------------------------------------
    // ensure_pointer_path
    // -----------------------------------------------------------------------

    #[test]
    fn ensure_path_creates_intermediates() {
        let mut root = json!({});
        ensure_pointer_path(&mut root, "/sinks/fusion/settings/gain");
        assert!(root.pointer("/sinks/fusion/settings/gain").is_some());
    }

    #[test]
    fn ensure_path_preserves_existing() {
        let mut root = json!({ "sinks": { "fusion": { "type": "Foo" } } });
        ensure_pointer_path(&mut root, "/sinks/fusion/settings/gain");
        assert_eq!(root["sinks"]["fusion"]["type"], "Foo");
        assert!(root.pointer("/sinks/fusion/settings/gain").is_some());
    }

    // -----------------------------------------------------------------------
    // Integration-style tests using sample_config
    // -----------------------------------------------------------------------

    #[test]
    fn find_and_resolve_endpoint_roundtrip() {
        let config = sample_config();
        // Find imu, get its endpoint
        let (_, _, imu_cfg) = find_node_in_config(&config, "imu").unwrap();
        let ep = resolve_output_endpoint(&imu_cfg).unwrap();
        assert_eq!(ep, "inproc://imu_data");

        // Verify fusion's inputEndpoints contains this endpoint
        let (_, _, fusion_cfg) = find_node_in_config(&config, "fusion").unwrap();
        let inputs = fusion_cfg["inputEndpoints"].as_array().unwrap();
        assert!(inputs.iter().any(|v| v.as_str() == Some(&ep)));
    }

    #[test]
    fn find_disabled_source_and_resolve_settings_endpoint() {
        let config = sample_config();
        let (_, key, cfg) = find_node_in_config(&config, "optical").unwrap();
        assert_eq!(key, "_optical");
        let ep = resolve_output_endpoint(&cfg).unwrap();
        assert_eq!(ep, "inproc://optical_data");
    }

    #[test]
    fn simulate_add_node() {
        let mut config = sample_config();
        let key = "newSink";
        let section = "sinks";

        // Verify key doesn't exist
        assert!(find_node_in_config(&config, key).is_none());

        // Insert
        let node_config = json!({
            "type": "Echo",
            "inputEndpoints": ["inproc://imu_data"]
        });
        config[section][key] = node_config;

        // Verify it's findable
        let (s, k, cfg) = find_node_in_config(&config, key).unwrap();
        assert_eq!(s, "sinks");
        assert_eq!(k, "newSink");
        assert_eq!(cfg["type"], "Echo");
    }

    #[test]
    fn simulate_remove_node_with_dangling_refs() {
        let mut config = sample_config();

        // Remove imu source
        let (section, key, removed_cfg) = find_node_in_config(&config, "imu").unwrap();
        let removed_ep = resolve_output_endpoint(&removed_cfg).unwrap();
        config[&section].as_object_mut().unwrap().remove(&key);

        // Scan for dangling references
        let mut dangling = Vec::new();
        for s in &["sources", "sinks"] {
            if let Some(obj) = config.get(s).and_then(|v| v.as_object()) {
                for (k, v) in obj {
                    if let Some(inputs) = v.get("inputEndpoints").and_then(|v| v.as_array()) {
                        if inputs.iter().any(|v| v.as_str() == Some(&removed_ep)) {
                            dangling.push(k.clone());
                        }
                    }
                }
            }
        }

        assert_eq!(dangling, vec!["fusion"]);
        assert!(find_node_in_config(&config, "imu").is_none());
    }

    #[test]
    fn simulate_enable_disable_toggle() {
        let mut config = sample_config();

        // Disable "echo" (currently enabled)
        let (section, key, val) = find_node_in_config(&config, "echo").unwrap();
        assert_eq!(key, "echo");
        config[&section].as_object_mut().unwrap().remove(&key);
        config[&section]["_echo"] = val;

        // Verify it's now found as disabled
        let (_, new_key, _) = find_node_in_config(&config, "echo").unwrap();
        assert_eq!(new_key, "_echo");

        // Re-enable
        let val = config["sinks"]["_echo"].clone();
        config["sinks"].as_object_mut().unwrap().remove("_echo");
        config["sinks"]["echo"] = val;

        let (_, restored_key, _) = find_node_in_config(&config, "echo").unwrap();
        assert_eq!(restored_key, "echo");
    }

    #[test]
    fn simulate_connect_nodes() {
        let mut config = sample_config();

        // Connect gnss -> fusion
        let (_, _, gnss_cfg) = find_node_in_config(&config, "gnss").unwrap();
        let ep = resolve_output_endpoint(&gnss_cfg).unwrap();
        assert_eq!(ep, "inproc://gnss_data");

        let (to_section, to_key, _) = find_node_in_config(&config, "fusion").unwrap();
        let node_cfg = &mut config[&to_section][&to_key];
        let mut inputs: Vec<Value> = node_cfg["inputEndpoints"]
            .as_array()
            .cloned()
            .unwrap_or_default();

        assert!(!inputs.iter().any(|v| v.as_str() == Some(&ep)));
        inputs.push(Value::String(ep.clone()));
        node_cfg["inputEndpoints"] = json!(inputs);

        // Verify
        let updated = config["sinks"]["fusion"]["inputEndpoints"].as_array().unwrap();
        assert_eq!(updated.len(), 3);
        assert!(updated.iter().any(|v| v.as_str() == Some("inproc://gnss_data")));
    }

    #[test]
    fn simulate_disconnect_nodes() {
        let mut config = sample_config();

        // Disconnect imu from fusion
        let (_, _, imu_cfg) = find_node_in_config(&config, "imu").unwrap();
        let ep = resolve_output_endpoint(&imu_cfg).unwrap();

        let node_cfg = &mut config["sinks"]["fusion"];
        let inputs: Vec<Value> = node_cfg["inputEndpoints"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|v| v.as_str() != Some(&ep))
            .cloned()
            .collect();
        node_cfg["inputEndpoints"] = json!(inputs);

        let updated = config["sinks"]["fusion"]["inputEndpoints"].as_array().unwrap();
        assert_eq!(updated.len(), 1);
        assert_eq!(updated[0].as_str().unwrap(), "inproc://optical_data");
    }
}
