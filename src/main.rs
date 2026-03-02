mod wiring;

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use clap::Parser;
use serde_json::Value;

use crypto::Crypto;
use fusion::clock::Clockwork;
use fusion::factory;
use fusion_types::JsonValueExt;
use networking::{CommandPublisher, CommandSubscriber};
use websocket_server::WebsocketServer;
use web_ui::WebUiServer;
use wiring::{NodeConnection, NodeRole, wire_node};

const DEFAULT_WS_PORT: u16 = 19358;
const DEFAULT_UI_PORT: u16 = 19359;

#[derive(Parser)]
#[command(name = "fusionhub-core", about = "LP FusionHub Core - open-source sensor fusion framework")]
struct Cli {
    /// Path to the JSON configuration file
    #[arg(short, long)]
    config: PathBuf,

    /// WebSocket server port (overrides config value)
    #[arg(short, long)]
    port: Option<u16>,
}

/// Holds all wired node connections.
///
/// Mirrors C++ DataBlock: maintains a running list of accumulated input endpoints
/// (`m_inputEndpoints`) that grows as sources and filters are wired. Each new node
/// receives the current accumulated list as its default input endpoints (unless it
/// specifies its own in the JSON config). After wiring, the node's output data
/// endpoint is appended to the list so subsequent nodes can receive its data.
struct DataBlock {
    connections: Vec<NodeConnection>,
    /// Accumulated data output endpoints from all sources and filters wired so far.
    /// Mirrors C++ `m_inputEndpoints`.
    input_endpoints: Vec<String>,
    /// Default command input endpoints, starts with `["inproc://websocket_command"]`.
    /// Mirrors C++ `m_defCmdEndpoints`.
    default_cmd_endpoints: Vec<String>,
    /// ZMQ CommandSubscriber bridging node command output → WebSocket server.
    /// Held here for ownership (dropping it tears down the subscriber).
    #[allow(dead_code)]
    node_cmd_subscriber: Option<CommandSubscriber>,
}

impl DataBlock {
    fn new() -> Self {
        Self {
            connections: Vec::new(),
            input_endpoints: Vec::new(),
            default_cmd_endpoints: vec!["inproc://websocket_command".to_owned()],
            node_cmd_subscriber: None,
        }
    }

    fn start_all(&self) -> Result<()> {
        // Start in order: sources, filters, sinks
        for conn in &self.connections {
            let mut n = conn.node.lock().unwrap();
            n.start()
                .with_context(|| format!("Failed to start node '{}'", n.name()))?;
        }
        Ok(())
    }

    fn stop_all(&mut self) {
        // Stop in reverse order: sinks, filters, sources
        for conn in self.connections.iter().rev() {
            let mut n = conn.node.lock().unwrap();
            if let Err(e) = n.stop() {
                log::error!("Error stopping node '{}': {}", n.name(), e);
            }
        }

        // Abort PUB socket tasks first. The zeromq PUB internals use
        // dbg!(e) when a connected peer resets, so the PUB tasks must be
        // gone before subscribers disconnect. We call shutdown() through
        // shared refs because Arc cycles prevent dropping publishers here.
        for conn in &self.connections {
            if let Some(ref p) = conn.publisher {
                p.shutdown();
            }
            if let Some(ref cp) = conn.cmd_publisher {
                cp.shutdown();
            }
        }

        // Now drop subscribers — aborts their async recv tasks and closes
        // mpsc channels so callback threads exit.
        self.node_cmd_subscriber.take();
        for conn in &mut self.connections {
            conn.subscriber.take();
            conn.cmd_subscriber.take();
        }

        self.connections.clear();
    }

    fn source_count(&self) -> usize {
        self.connections.iter().filter(|c| c.role == NodeRole::Source).count()
    }

    fn filter_count(&self) -> usize {
        self.connections.iter().filter(|c| c.role == NodeRole::Filter).count()
    }

    fn sink_count(&self) -> usize {
        self.connections.iter().filter(|c| c.role == NodeRole::Sink).count()
    }

    /// Collect command endpoints for the WebSocket server to subscribe to.
    fn command_endpoints(&self) -> Vec<String> {
        self.connections
            .iter()
            .filter_map(|conn| {
                conn.cmd_publisher.as_ref().map(|cp| cp.endpoint().to_owned())
            })
            .collect()
    }
}

/// Top-level application that owns the node graph.
///
/// The WebSocket and Web UI servers are owned by `main()` and persist
/// across node-graph restarts. FusionHub only borrows them during `run()`.
struct FusionHub {
    m_data_block: DataBlock,
    m_ws_cmd_publisher: Option<CommandPublisher>,
    m_bridge2_handle: Option<tokio::task::JoinHandle<()>>,
    m_status_poll_handle: Option<tokio::task::JoinHandle<()>>,
    m_config: Value,
    m_clockwork: Arc<Mutex<Clockwork>>,
    m_licensed_features: Vec<String>,
    m_paused: Arc<AtomicBool>,
}

impl FusionHub {
    fn new(config: Value, licensed_features: Vec<String>, paused: Arc<AtomicBool>) -> Self {
        // Create the CommandPublisher for `inproc://websocket_command` early,
        // before any nodes are wired. This registers the endpoint in the ZMQ
        // endpoint registry so that nodes' CommandSubscribers can resolve and
        // connect to the actual TCP address. Mirrors C++ WebsocketServer
        // constructor which creates m_commandPublisher("inproc://websocket_command").
        let ws_cmd_publisher = CommandPublisher::new("inproc://websocket_command");
        log::debug!(
            "WebSocket command publisher bound: inproc://websocket_command -> {}",
            ws_cmd_publisher.endpoint()
        );

        Self {
            m_data_block: DataBlock::new(),
            m_ws_cmd_publisher: Some(ws_cmd_publisher),
            m_bridge2_handle: None,
            m_status_poll_handle: None,
            m_config: config,
            m_clockwork: Arc::new(Mutex::new(Clockwork::new())),
            m_licensed_features: licensed_features,
            m_paused: paused,
        }
    }

    fn build_nodes(&mut self) -> Result<()> {
        self.build_sources();
        self.build_filters_and_sinks();

        log::info!(
            "Built {} sources, {} filters, {} sinks",
            self.m_data_block.source_count(),
            self.m_data_block.filter_count(),
            self.m_data_block.sink_count(),
        );
        Ok(())
    }

    fn set_connection_metadata(conn: &mut NodeConnection, key: &str, name: &str, role: NodeRole) {
        conn.config_key = name.to_owned();
        if let Some(meta) = fusion_registry::metadata_for_key(key) {
            conn.display_name = meta.display_name;
            conn.node_color = meta.color;
        } else {
            conn.display_name = name.to_owned();
            conn.node_color = match role {
                NodeRole::Source => "#4ade80".to_owned(),
                NodeRole::Filter => "#60a5fa".to_owned(),
                NodeRole::Sink => "#f472b6".to_owned(),
            };
        }
    }

    /// Check if a config key names a filter (dynamically from registry).
    fn is_filter_key(key: &str) -> bool {
        fusion_registry::is_filter_key(key)
    }

    /// Build source nodes from the "sources" config section.
    ///
    /// Mirrors C++ DataBlock constructor source-building loop:
    /// - Each source's resolved data endpoint is appended to `m_inputEndpoints`
    /// - The special `"endpoints"` key adds explicit endpoints to the list
    /// - After all sources, `star_to_localhost` is applied to the accumulated list
    fn build_sources(&mut self) {
        let sources = match self.m_config.get("sources") {
            Some(v) => v.clone(),
            None => {
                log::warn!("No 'sources' section in configuration");
                return;
            }
        };

        let entries = match sources.as_object() {
            Some(map) => map.clone(),
            None => {
                log::warn!("'sources' section is not a JSON object");
                return;
            }
        };

        for (key, value) in &entries {
            if key.starts_with('_') {
                continue;
            }

            // Handle explicit endpoints list from config (C++ "endpoints" key)
            if key == "endpoints" {
                if let Some(arr) = value.as_array() {
                    for ep in arr.iter().filter_map(|v| v.as_str()) {
                        self.m_data_block.input_endpoints.push(ep.to_owned());
                        log::info!("Added explicit endpoint: {}", ep);
                    }
                } else if let Some(ep) = value.as_str() {
                    self.m_data_block.input_endpoints.push(ep.to_owned());
                    log::info!("Added explicit endpoint: {}", ep);
                }
                continue;
            }

            let configs: Vec<Value> = if let Some(arr) = value.as_array() {
                arr.clone()
            } else {
                vec![value.clone()]
            };

            for (i, node_config) in configs.iter().enumerate() {
                let name = if configs.len() > 1 {
                    format!("{}_{}", key, i)
                } else {
                    key.clone()
                };

                match factory::build_node(key, node_config) {
                    Ok(node) => {
                        log::info!("Created source node: '{}'", name);
                        // Sources don't subscribe to other sources, so pass empty defaults
                        let no_endpoints: Vec<String> = Vec::new();
                        let mut conn = wire_node(
                            node,
                            node_config,
                            NodeRole::Source,
                            self.m_clockwork.clone(),
                            &no_endpoints,
                            &self.m_data_block.default_cmd_endpoints,
                            self.m_paused.clone(),
                        );
                        Self::set_connection_metadata(&mut conn, key, &name, NodeRole::Source);
                        if !conn.resolved_data_endpoint.is_empty() {
                            self.m_data_block.input_endpoints.push(conn.resolved_data_endpoint.clone());
                            log::info!(
                                "Added to list of all source output endpoints: {}",
                                conn.resolved_data_endpoint
                            );
                        }
                        self.m_data_block.connections.push(conn);
                    }
                    Err(e) => {
                        log::error!("Failed to create source node '{}': {}", name, e);
                    }
                }
            }
        }

        // Convert wildcard/0.0.0.0 endpoints to localhost for subscriber connections
        // Mirrors C++: m_inputEndpoints = LP::EndpointStringConverter::starToLocalhost(m_inputEndpoints)
        self.m_data_block.input_endpoints =
            networking::star_to_localhost(&self.m_data_block.input_endpoints);
    }

    /// Build filter and sink nodes from the "sinks" config section.
    ///
    /// Mirrors C++ DataBlock constructor:
    /// - First pass: filter nodes — each filter's data endpoint is appended to `m_inputEndpoints`
    /// - After filters, `star_to_localhost` is applied
    /// - Second pass: sink nodes — they receive the final accumulated endpoints as defaults
    /// - DataMonitor is always added as a sink
    fn build_filters_and_sinks(&mut self) {
        let sinks_section = match self.m_config.get("sinks") {
            Some(v) => v.clone(),
            None => {
                log::info!("No 'sinks' section in configuration");
                Value::Object(serde_json::Map::new())
            }
        };

        let entries = match sinks_section.as_object() {
            Some(map) => map.clone(),
            None => {
                log::warn!("'sinks' section is not a JSON object");
                return;
            }
        };

        // First pass: filters (key names like "fusion", "differentialImu", etc.)
        for (key, value) in &entries {
            if key.starts_with('_') || !Self::is_filter_key(key) {
                continue;
            }

            if let Some(required) = fusion_registry::required_feature(key) {
                if !self.m_licensed_features.iter().any(|f| f == &required) {
                    log::warn!("Skipping '{}': feature '{}' not licensed", key, required);
                    continue;
                }
            }

            let configs: Vec<Value> = if let Some(arr) = value.as_array() {
                arr.clone()
            } else {
                vec![value.clone()]
            };

            for (i, node_config) in configs.iter().enumerate() {
                let name = if configs.len() > 1 {
                    format!("{}_{}", key, i)
                } else {
                    key.clone()
                };

                match factory::build_node(key, node_config) {
                    Ok(node) => {
                        log::info!("Created filter node: '{}'", name);
                        let current_input_eps = self.m_data_block.input_endpoints.clone();
                        let current_cmd_eps = self.m_data_block.default_cmd_endpoints.clone();
                        let mut conn = wire_node(
                            node,
                            node_config,
                            NodeRole::Filter,
                            self.m_clockwork.clone(),
                            &current_input_eps,
                            &current_cmd_eps,
                            self.m_paused.clone(),
                        );
                        Self::set_connection_metadata(&mut conn, key, &name, NodeRole::Filter);
                        if !conn.resolved_data_endpoint.is_empty() {
                            self.m_data_block.input_endpoints.push(conn.resolved_data_endpoint.clone());
                            log::info!(
                                "Added to list of all filter output endpoints: {}",
                                conn.resolved_data_endpoint
                            );
                        }
                        self.m_data_block.connections.push(conn);
                    }
                    Err(e) => {
                        log::error!("Failed to create filter node '{}': {}", name, e);
                    }
                }
            }
        }

        // Convert wildcard/0.0.0.0 endpoints to localhost after filters
        // Mirrors C++: m_inputEndpoints = LP::EndpointStringConverter::starToLocalhost(m_inputEndpoints)
        self.m_data_block.input_endpoints =
            networking::star_to_localhost(&self.m_data_block.input_endpoints);

        // Add DataMonitor by default (C++ always adds one)
        let monitor_config = Value::Object(serde_json::Map::new());
        match factory::build_node("DataMonitor", &monitor_config) {
            Ok(node) => {
                log::info!("Created default DataMonitor sink");
                let current_input_eps = self.m_data_block.input_endpoints.clone();
                let current_cmd_eps = self.m_data_block.default_cmd_endpoints.clone();
                let mut conn = wire_node(
                    node,
                    &monitor_config,
                    NodeRole::Sink,
                    self.m_clockwork.clone(),
                    &current_input_eps,
                    &current_cmd_eps,
                    self.m_paused.clone(),
                );
                conn.config_key = "DataMonitor".to_owned();
                conn.display_name = "Data Monitor".to_owned();
                conn.node_color = "#f472b6".to_owned();
                self.m_data_block.connections.push(conn);
            }
            Err(e) => {
                log::error!("Failed to create DataMonitor: {}", e);
            }
        }

        // Second pass: actual sinks (key names like "echo", "logger", "dtrackOutput", etc.)
        for (key, value) in &entries {
            if key.starts_with('_') || Self::is_filter_key(key) {
                continue;
            }

            match factory::build_node(key, value) {
                Ok(node) => {
                    log::info!("Created sink node: '{}'", key);
                    let current_input_eps = self.m_data_block.input_endpoints.clone();
                    let current_cmd_eps = self.m_data_block.default_cmd_endpoints.clone();
                    let mut conn = wire_node(
                        node,
                        value,
                        NodeRole::Sink,
                        self.m_clockwork.clone(),
                        &current_input_eps,
                        &current_cmd_eps,
                        self.m_paused.clone(),
                    );
                    Self::set_connection_metadata(&mut conn, key, key, NodeRole::Sink);
                    self.m_data_block.connections.push(conn);
                }
                Err(e) => {
                    log::error!("Failed to create sink node '{}': {}", key, e);
                }
            }
        }
    }

    /// Run the hub. Returns `true` if a restart was requested, `false` on Ctrl+C.
    ///
    /// The WebSocket and Web UI servers are owned by `main()` and persist
    /// across restarts. This method only sets up the command bridges and
    /// starts the node graph.
    async fn run(
        &mut self,
        ws_server: &WebsocketServer,
        ui_server: &WebUiServer,
    ) -> Result<bool> {
        let command_endpoints = self.m_data_block.command_endpoints();
        ws_server.subscribe(command_endpoints.clone()).await;

        let cmd_channel = ws_server.command_channel().await;

        // Bridge 1: Node command output (ZMQ) → WebSocket server (CommandChannel).
        if !command_endpoints.is_empty() {
            log::info!(
                "Bridging {} node command endpoints to WebSocket server",
                command_endpoints.len()
            );
            let ch = cmd_channel.clone();
            let sub = CommandSubscriber::new(
                move |req: &fusion_types::ApiRequest| {
                    ch.send(req);
                },
                command_endpoints,
            );
            self.m_data_block.node_cmd_subscriber = Some(sub);
        }

        // Bridge 2: WebSocket server (CommandChannel) → nodes (ZMQ).
        if let Some(ref cmd_pub) = self.m_ws_cmd_publisher {
            let mut rx = cmd_channel.subscribe();
            let pub_clone = cmd_pub.clone();
            self.m_bridge2_handle = Some(tokio::spawn(async move {
                while let Ok(req) = rx.recv().await {
                    if req.command == "ws" {
                        continue;
                    }
                    if let Err(e) = pub_clone.publish(&req) {
                        log::warn!("Failed to forward command to nodes: {}", e);
                    }
                }
            }));
        }

        ws_server.send_startup_message_to_ui().await;
        ui_server.update_config(self.m_config.clone()).await;

        self.m_data_block.start_all()?;
        log::info!("All nodes started, entering main loop");

        // Spawn status polling task
        let node_refs: Vec<fusion::status_poller::NodeRef> = self
            .m_data_block
            .connections
            .iter()
            .map(|c| fusion::status_poller::NodeRef {
                node: c.node.clone(),
                config_key: c.config_key.clone(),
                display_name: c.display_name.clone(),
                role: match c.role {
                    NodeRole::Source => "source".to_owned(),
                    NodeRole::Filter => "filter".to_owned(),
                    NodeRole::Sink => "sink".to_owned(),
                },
                color: c.node_color.clone(),
                connected_node: c.connected_node.clone(),
            })
            .collect();
        let sse_tx = ui_server.sse_sender().await;
        let poll_paused = self.m_paused.clone();
        self.m_status_poll_handle = Some(tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
            loop {
                interval.tick().await;
                let nodes = fusion::status_poller::collect_node_statuses(&node_refs);
                let msg = serde_json::json!({
                    "type": "nodeStatuses",
                    "data": {
                        "paused": poll_paused.load(std::sync::atomic::Ordering::Relaxed),
                        "nodes": nodes,
                    },
                });
                let _ = sse_tx.send(msg.to_string());
            }
        }));

        let restart = Self::main_loop(ws_server, ui_server).await;

        self.shutdown();
        Ok(restart)
    }

    async fn main_loop(
        ws_server: &WebsocketServer,
        ui_server: &WebUiServer,
    ) -> bool {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                log::info!("Ctrl+C received, shutting down");
                false
            }
            _ = ws_server.notified() => {
                log::info!("Reset triggered (WebSocket), restarting node graph");
                true
            }
            _ = ui_server.notified() => {
                log::info!("Reset triggered (Web UI), restarting node graph");
                true
            }
        }
    }

    fn shutdown(&mut self) {
        log::info!("Shutting down node graph");
        if let Some(ref cp) = self.m_ws_cmd_publisher {
            cp.shutdown();
        }
        if let Some(h) = self.m_bridge2_handle.take() {
            h.abort();
        }
        if let Some(h) = self.m_status_poll_handle.take() {
            h.abort();
        }
        self.m_data_block.stop_all();
        self.m_ws_cmd_publisher.take();
        log::info!("All nodes stopped");
    }
}

fn load_config(path: &std::path::Path) -> Result<Value> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Could not read config file: {}", path.display()))?;
    let stripped = strip_json_comments(&content);
    let config: Value = serde_json::from_str(&stripped)
        .with_context(|| format!("Could not parse config file: {}", path.display()))?;
    Ok(config)
}

/// Strip C/C++ style comments from a JSON string.
/// Handles // line comments and /* block comments */.
/// Respects string literals (doesn't strip inside "...").
fn strip_json_comments(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    let mut in_string = false;

    while i < len {
        if in_string {
            result.push(bytes[i] as char);
            if bytes[i] == b'\\' && i + 1 < len {
                i += 1;
                result.push(bytes[i] as char);
            } else if bytes[i] == b'"' {
                in_string = false;
            }
            i += 1;
        } else if bytes[i] == b'"' {
            in_string = true;
            result.push('"');
            i += 1;
        } else if i + 1 < len && bytes[i] == b'/' && bytes[i + 1] == b'/' {
            // Line comment — skip to end of line
            i += 2;
            while i < len && bytes[i] != b'\n' {
                i += 1;
            }
        } else if i + 1 < len && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            // Block comment — skip to */
            i += 2;
            while i + 1 < len && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            if i + 1 < len {
                i += 2; // skip */
            }
        } else {
            result.push(bytes[i] as char);
            i += 1;
        }
    }
    result
}

fn check_license(config: &Value) -> (bool, crypto::LicenseInfo) {
    let mut crypto = Crypto::new();
    let license_info = config
        .get("LicenseInfo")
        .map(|v| v.to_string())
        .unwrap_or_else(|| "{}".to_owned());
    let valid = crypto.check_license(&license_info);
    if valid {
        log::info!("License check passed");
        if !crypto.features().is_empty() {
            log::info!("Licensed features: {}", crypto.features().join(", "));
        }
    } else {
        log::error!("License check failed — FusionHub may run with limited functionality");
    }
    (valid, crypto.last_info().clone())
}

#[tokio::main]
async fn main() -> Result<()> {
    log_utils::init_with_level(log::LevelFilter::Info);

    let cli = Cli::parse();

    // Register core node types with the global registry
    fusion::registration::register_core_nodes();

    log::info!("FusionHub Core starting");
    log::info!("Configuration: {}", cli.config.display());

    let mut config = load_config(&cli.config)?;

    if let Some(port) = cli.port {
        config["websocketPort"] = Value::from(port);
    }

    let (_, license_info) = check_license(&config);
    let mut licensed_features = license_info.features.clone();

    let config_path = cli.config.to_string_lossy().to_string();
    let paused = Arc::new(AtomicBool::new(false));

    // Create servers once — they persist across node-graph restarts so
    // UI clients stay connected.
    let ws_port = config.value_u16("websocketPort", DEFAULT_WS_PORT);
    let ws_server = WebsocketServer::new("0.0.0.0", ws_port, &config_path).await?;

    let cmd_channel = ws_server.command_channel().await;
    let ui_port = config.value_u16("webUiPort", DEFAULT_UI_PORT);
    let ui_server = WebUiServer::new(
        "0.0.0.0",
        ui_port,
        config.clone(),
        &config_path,
        cmd_channel,
        license_info,
        paused.clone(),
    )
    .await?;

    loop {
        paused.store(false, std::sync::atomic::Ordering::Relaxed);
        let mut hub = FusionHub::new(config.clone(), licensed_features.clone(), paused.clone());

        hub.build_nodes()?;
        let restart = hub.run(&ws_server, &ui_server).await?;

        if !restart {
            drop(hub);
            drop(ui_server);
            drop(ws_server);
            std::thread::sleep(std::time::Duration::from_millis(100));
            log::info!("FusionHub terminated");
            std::process::exit(0);
        }

        // Reload from disk — both the WebSocket and Web UI save paths
        // write to disk before triggering a reset, so this is always
        // the most up-to-date config regardless of which UI triggered it.
        config = load_config(&cli.config)?;
        ws_server.set_config(config.clone()).await;
        let (_, new_license_info) = check_license(&config);
        licensed_features = new_license_info.features.clone();
        ui_server.update_license_status(new_license_info).await;
        log::info!("Restarting FusionHub with updated configuration");
    }
}
