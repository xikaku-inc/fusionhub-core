use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use fusion::clock::Clockwork;
use fusion::connected_node::{
    check_endpoints, passes_data_filter, ConnectedNodeConfig, ConnectedNodeImpl,
    EndpointConfig,
};
use fusion::node::Node;
use fusion_types::JsonValueExt;
use networking::{CommandPublisher, CommandSubscriber, Publisher, Subscriber};
use serde_json::Value;

/// Role of a node in the pipeline (for ordering start/stop).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeRole {
    Source,
    Filter,
    Sink,
}

/// Wraps a node with its ZMQ networking connections and ConnectedNode logic.
///
/// This is the Rust equivalent of C++ `ConnectedNode<NodeType>`:
/// - Publisher for data output (with rate limiting, clock stamping, sender ID)
/// - Subscriber for data input (with optional type filtering)
/// - CommandPublisher for forwarding command output
/// - CommandSubscriber for receiving commands
///
/// Fields are held for ownership — dropping them tears down the underlying ZMQ sockets.
#[allow(dead_code)]
pub struct NodeConnection {
    pub node: Arc<Mutex<dyn Node>>,
    pub role: NodeRole,
    pub config_key: String,
    pub display_name: String,
    pub node_color: String,
    pub connected_node: Arc<ConnectedNodeImpl>,
    pub publisher: Option<Arc<Publisher>>,
    pub subscriber: Option<Subscriber>,
    pub cmd_publisher: Option<CommandPublisher>,
    pub cmd_subscriber: Option<CommandSubscriber>,
    pub resolved_data_endpoint: String,
    pub resolved_cmd_endpoint: String,
}

/// Wire a node with ZMQ Publisher/Subscriber based on its JSON config.
///
/// Mirrors C++ ConnectedNode constructor + initializeConnections:
/// 1. Parse ConnectedNodeConfig (clocks, rate limiting, data filter)
/// 2. Create Publisher, hook node output through process_output → Publisher
/// 3. Create Subscriber, hook incoming data through data filter → node.receive_data
/// 4. Create CommandSubscriber, hook → node.receive_command
/// 5. Create CommandPublisher, hook node command output through rate limiter → CommandPublisher
///
/// `default_input_endpoints` mirrors C++ DataBlock::m_inputEndpoints — the accumulated list of
/// all source/filter output endpoints seen so far. If a node's config doesn't specify
/// `inputEndpoints`, these defaults are used.
///
/// `default_command_endpoints` mirrors C++ DataBlock::m_defCmdEndpoints — typically starts with
/// `["inproc://websocket_command"]`.
pub fn wire_node(
    node: Arc<Mutex<dyn Node>>,
    config: &Value,
    role: NodeRole,
    clockwork: Arc<Mutex<Clockwork>>,
    default_input_endpoints: &[String],
    default_command_endpoints: &[String],
    paused: Arc<AtomicBool>,
    explicit_connections: bool,
) -> NodeConnection {
    let node_name = node.lock().unwrap().name().to_owned();

    // Parse ConnectedNode config from JSON
    let cn_config = ConnectedNodeConfig::from_json(config, &node_name);

    // --- Resolve endpoints from config ---

    // Data publish endpoint
    let data_endpoint = config
        .get("dataEndpoint")
        .or_else(|| config.get("outEndpoint"))
        .and_then(|v| v.as_str())
        .or_else(|| {
            config
                .get("settings")
                .and_then(|s| s.get("endpoints"))
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.first())
                .and_then(|v| v.as_str())
        })
        .unwrap_or("")
        .to_owned();

    // Input endpoints — check config root, then settings, then fall back to accumulated defaults
    let input_endpoints = resolve_input_endpoints(config, default_input_endpoints, &node_name, explicit_connections);

    // Command input endpoints — check config, fall back to accumulated defaults
    // Mirrors C++ config.value("commandInputEndpoints", commandInputEndpoints)
    let command_input_endpoints: Vec<String> = config
        .get("commandInputEndpoints")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_else(|| default_command_endpoints.to_vec());

    // Command endpoint
    let command_endpoint = config.value_str("commandEndpoint", "");

    let endpoint_config = EndpointConfig {
        data_publish: data_endpoint.clone(),
        data_subscribe: input_endpoints.clone(),
        command_in: String::new(),
        command_out: command_endpoint,
        command_input_endpoints: command_input_endpoints.clone(),
    };

    // Create ConnectedNodeImpl with full config
    let connected_node = Arc::new(ConnectedNodeImpl::new(
        &node_name,
        endpoint_config,
        cn_config.clone(),
        clockwork,
    ));

    // --- Data Publisher (output) ---
    let (publisher, resolved_data_endpoint) = if !data_endpoint.is_empty() {
        let pub_obj = Publisher::new(&data_endpoint);
        let resolved = pub_obj.endpoint().to_owned();
        log::info!(
            "[{}] Publisher bound: {} -> {}",
            node_name, data_endpoint, resolved
        );
        (Some(Arc::new(pub_obj)), resolved)
    } else {
        (None, String::new())
    };

    // Hook node output → ConnectedNode.process_output → Publisher
    if let Some(ref pub_arc) = publisher {
        let pub_clone = Arc::clone(pub_arc);
        let cn = Arc::clone(&connected_node);
        let name = node_name.clone();
        let paused_flag = paused.clone();
        node.lock().unwrap().set_on_output(Box::new(move |data| {
            if paused_flag.load(Ordering::Relaxed) {
                return;
            }
            if let Some(processed) = cn.process_output(data) {
                cn.increment_output();
                if let Err(e) = pub_clone.publish(&processed) {
                    log::warn!("[{}] Publish error: {}", name, e);
                }
            }
        }));
    }

    // --- Data Subscriber (input) ---
    let subscriber = if !input_endpoints.is_empty() {
        log::info!("[{}] Subscriber connecting to {:?}", node_name, input_endpoints);

        let sub = Subscriber::new(input_endpoints);
        let node_arc = Arc::clone(&node);
        let cn_for_input = Arc::clone(&connected_node);
        let name = node_name.clone();
        let data_filter = cn_config.input_data_filter.clone();

        if let Err(e) = sub.start_listening(move |data| {
            // Apply input data filter (matching C++ inputDataFilter)
            if !passes_data_filter(&data, &data_filter) {
                return;
            }

            cn_for_input.increment_input();
            match node_arc.lock() {
                Ok(mut n) => n.receive_data(data),
                Err(e) => log::error!("[{}] Lock poisoned in subscriber: {}", name, e),
            }
        }) {
            log::error!("[{}] Failed to start subscriber: {}", node_name, e);
        }
        Some(sub)
    } else {
        None
    };

    // --- Command Publisher (for forwarding commands from this node) ---
    let cmd_publisher = CommandPublisher::new("tcp://*:0");
    let resolved_cmd_endpoint = cmd_publisher.endpoint().to_owned();
    log::debug!(
        "[{}] CommandPublisher on {}",
        node_name,
        resolved_cmd_endpoint
    );

    // Hook node command output → ConnectedNode rate limiter → CommandPublisher
    {
        let cmd_pub = cmd_publisher.clone();
        let cn = Arc::clone(&connected_node);
        let name = node_name.clone();
        node.lock().unwrap().set_on_command_output(Box::new(move |req| {
            if cn.should_publish_command(&req) {
                if let Err(e) = cmd_pub.publish(&req) {
                    log::warn!("[{}] Command publish error: {}", name, e);
                }
            }
        }));
    }

    // --- Command Subscriber (for receiving commands) ---
    let cmd_subscriber = if !command_input_endpoints.is_empty() {
        log::info!(
            "[{}] CommandSubscriber connecting to {:?}",
            node_name,
            command_input_endpoints
        );

        let node_arc = Arc::clone(&node);
        let name = node_name.clone();

        Some(CommandSubscriber::new(
            move |req| {
                match node_arc.lock() {
                    Ok(mut n) => n.receive_command(req),
                    Err(e) => log::error!("[{}] Lock poisoned in command subscriber: {}", name, e),
                }
            },
            command_input_endpoints,
        ))
    } else {
        None
    };

    NodeConnection {
        node,
        role,
        config_key: String::new(),
        display_name: String::new(),
        node_color: String::new(),
        connected_node,
        publisher,
        subscriber,
        cmd_publisher: Some(cmd_publisher),
        cmd_subscriber,
        resolved_data_endpoint,
        resolved_cmd_endpoint,
    }
}

/// Resolve input endpoints from config, mirroring C++ ConnectedNode logic:
/// 1. Check config root "inputEndpoints"
/// 2. Check config["settings"]["inputEndpoints"]
/// 3. Fall back to default_endpoints (passed from DataBlock)
/// 4. Validate with checkEndpoints; fall back to defaults if invalid
fn resolve_input_endpoints(
    config: &Value,
    default_endpoints: &[String],
    node_name: &str,
    explicit_connections: bool,
) -> Vec<String> {
    // Try config root
    if let Some(eps) = config.get("inputEndpoints").and_then(|v| v.as_array()) {
        let endpoints: Vec<String> = eps.iter().filter_map(|v| v.as_str().map(String::from)).collect();
        if !endpoints.is_empty() {
            if check_endpoints(&endpoints) {
                return endpoints;
            }
            log::warn!(
                "[{}] Invalid input endpoints in config, using defaults: {:?}",
                node_name, default_endpoints
            );
            return default_endpoints.to_vec();
        }
    }

    // Try settings.inputEndpoints
    if let Some(eps) = config
        .get("settings")
        .and_then(|s| s.get("inputEndpoints"))
        .and_then(|v| v.as_array())
    {
        let endpoints: Vec<String> = eps.iter().filter_map(|v| v.as_str().map(String::from)).collect();
        if !endpoints.is_empty() {
            if check_endpoints(&endpoints) {
                return endpoints;
            }
            log::warn!(
                "[{}] Invalid input endpoints in settings, using defaults: {:?}",
                node_name, default_endpoints
            );
            return default_endpoints.to_vec();
        }
    }

    // Explicit mode: no auto-subscribe when inputEndpoints absent
    if explicit_connections {
        log::debug!(
            "[{}] No inputEndpoints in config, explicit mode — no auto-subscribe",
            node_name
        );
        return Vec::new();
    }

    if !default_endpoints.is_empty() {
        log::debug!(
            "[{}] No inputEndpoints in config, using default endpoints: {:?}",
            node_name, default_endpoints
        );
    }

    default_endpoints.to_vec()
}
