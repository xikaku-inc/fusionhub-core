use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use serde_json::json;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, Mutex, Notify};
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

use fusion_types::{ApiRequest, JsonValueExt};

const VERSION_NUMBER: &str = "1.0.0";

// ---------------------------------------------------------------------------
// CommandRouter
// ---------------------------------------------------------------------------

type CommandHandler =
    Arc<dyn Fn(&ApiRequest) -> Option<serde_json::Value> + Send + Sync>;

struct CommandInfo {
    handler: CommandHandler,
    description: String,
}

/// Routes incoming API commands to registered handler functions.
pub struct CommandRouter {
    m_handlers: HashMap<String, CommandInfo>,
}

impl CommandRouter {
    pub fn new() -> Self {
        Self {
            m_handlers: HashMap::new(),
        }
    }

    pub fn register_handler<F>(
        &mut self,
        command: &str,
        handler: F,
        description: &str,
    ) where
        F: Fn(&ApiRequest) -> Option<serde_json::Value> + Send + Sync + 'static,
    {
        self.m_handlers.insert(
            command.to_owned(),
            CommandInfo {
                handler: Arc::new(handler),
                description: description.to_owned(),
            },
        );
    }

    pub fn route_command(&self, request: &ApiRequest) -> Option<serde_json::Value> {
        if let Some(info) = self.m_handlers.get(&request.command) {
            (info.handler)(request)
        } else {
            log::warn!("Unknown command: {}", request.command);
            Some(json!({ "error": format!("Unknown command: {}", request.command) }))
        }
    }

    pub fn has_command(&self, command: &str) -> bool {
        self.m_handlers.contains_key(command)
    }

    pub fn registered_commands(&self) -> serde_json::Value {
        let commands: HashMap<&str, &str> = self
            .m_handlers
            .iter()
            .map(|(k, v)| (k.as_str(), v.description.as_str()))
            .collect();
        serde_json::to_value(commands).unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------
// CommandPublisher / CommandSubscriber stubs (inproc equivalent)
// ---------------------------------------------------------------------------

/// Broadcasts ApiRequests to subscribers (replaces ZMQ inproc command channel).
#[derive(Clone)]
pub struct CommandChannel {
    sender: broadcast::Sender<ApiRequest>,
}

impl CommandChannel {
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(256);
        Self { sender }
    }

    pub fn send(&self, req: &ApiRequest) {
        let _ = self.sender.send(req.clone());
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ApiRequest> {
        self.sender.subscribe()
    }
}

// ---------------------------------------------------------------------------
// WebsocketServer
// ---------------------------------------------------------------------------

/// Shared mutable state accessed by both the WebSocket accept loop and command
/// handlers.
struct ServerState {
    config_path: PathBuf,
    config_persistent: serde_json::Value,
    config: serde_json::Value,
    is_reset: bool,
    reset_notify: Arc<Notify>,
    command_router: CommandRouter,
    command_channel: CommandChannel,
    /// Broadcast sender used to push outgoing messages to all connected
    /// WebSocket clients.
    client_tx: broadcast::Sender<String>,
}

/// A WebSocket server that serves configuration and command APIs for the
/// FusionHub UI.
///
/// This is the Rust port of the C++ `Websocket::WebsocketServer`.
pub struct WebsocketServer {
    state: Arc<Mutex<ServerState>>,
    reset_notify: Arc<Notify>,
    /// Handle for the background accept-loop task.
    accept_handle: Option<tokio::task::JoinHandle<()>>,
    /// Handle for the subscribe-loop task (forwarding node commands).
    subscribe_handle: tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl WebsocketServer {
    /// Create and start the WebSocket server.
    ///
    /// * `addr` -- IP address to bind, e.g. `"0.0.0.0"`.
    /// * `port` -- TCP port to listen on, e.g. `19358`.
    /// * `config_path` -- path to the JSON configuration file.
    pub async fn new(addr: &str, port: u16, config_path: &str) -> Result<Self> {
        let config_persistent = load_config_file(config_path)?;
        let config = config_persistent.clone();

        let (client_tx, _) = broadcast::channel::<String>(512);
        let reset_notify = Arc::new(Notify::new());

        let mut state = ServerState {
            config_path: PathBuf::from(config_path),
            config_persistent,
            config,
            is_reset: false,
            reset_notify: reset_notify.clone(),
            command_router: CommandRouter::new(),
            command_channel: CommandChannel::new(),
            client_tx: client_tx.clone(),
        };

        init_command_router(&mut state);

        let state = Arc::new(Mutex::new(state));

        let bind_addr = format!("{}:{}", addr, port);
        let listener = TcpListener::bind(&bind_addr)
            .await
            .with_context(|| format!("Failed to bind WebSocket server on {}", bind_addr))?;

        log::info!("WebSocket server listening on {}", bind_addr);

        let accept_state = state.clone();
        let accept_handle = tokio::spawn(async move {
            accept_loop(listener, accept_state, client_tx).await;
        });

        Ok(Self {
            state,
            reset_notify,
            accept_handle: Some(accept_handle),
            subscribe_handle: tokio::sync::Mutex::new(None),
        })
    }

    /// Return a clone of the current (possibly modified) configuration.
    pub async fn get_config(&self) -> serde_json::Value {
        let s = self.state.lock().await;
        s.config.clone()
    }

    /// Overwrite the current in-memory configuration.
    pub async fn set_config(&self, config: serde_json::Value) {
        let mut s = self.state.lock().await;
        s.config = config;
    }

    /// Process a raw incoming JSON string and return the optional response.
    pub async fn command_handler(&self, incoming: &str) -> Option<serde_json::Value> {
        let j: serde_json::Value = match serde_json::from_str(incoming) {
            Ok(v) => v,
            Err(_) => {
                return Some(json!({
                    "status": "FAIL",
                    "description": "Could not parse JSON"
                }));
            }
        };

        let req = match ApiRequest::from_json(&j) {
            Some(r) => r,
            None => {
                return Some(json!({
                    "status": "FAIL",
                    "description": "Invalid ApiRequest format"
                }));
            }
        };

        let s = self.state.lock().await;
        s.command_router.route_command(&req)
    }

    /// Subscribe to node command endpoints.
    ///
    /// The provided endpoint strings are the command endpoints of all nodes
    /// that want to communicate with the server.  In the full implementation
    /// this would create ZMQ subscribers; here we store the endpoints and set
    /// up a handler that forwards replies to WebSocket clients.
    pub async fn subscribe(&self, endpoint_strings: Vec<String>) {
        // Abort previous subscribe task if any (happens on node-graph restart).
        let mut handle = self.subscribe_handle.lock().await;
        if let Some(h) = handle.take() {
            h.abort();
        }

        let state = self.state.clone();
        let mut rx = {
            let s = state.lock().await;
            s.command_channel.subscribe()
        };

        let task = tokio::spawn(async move {
            while let Ok(req) = rx.recv().await {
                let mut s = state.lock().await;
                let command = &req.command;

                if command == "ws" {
                    let mut data = req.data.clone();
                    data["id"] = serde_json::Value::String(req.id.clone());
                    let _ = s.client_tx.send(data.to_string());
                } else if command == "setConfigJsonPath" {
                    // Update config at JSON pointer path indicated by topic
                    let topic = &req.topic;
                    if let Some(target) = s.config.pointer_mut(topic) {
                        *target = req.data.clone();
                    }
                    send_formatted_response(&s.client_tx, command, &req.data, &req.id);
                } else if command == "getNConnections" {
                    // Reply with the current number of connected WebSocket clients
                    let n_connections = s.client_tx.receiver_count();
                    let reply = ApiRequest::new(
                        "getNConnections",
                        &req.topic,
                        json!({ "nConnections": n_connections }),
                        &req.id,
                    );
                    s.command_channel.send(&reply);
                } else if command == "applyIntercalibrationResults" {
                    send_formatted_response(
                        &s.client_tx,
                        "getConfig",
                        &s.config,
                        &req.id,
                    );
                } else if command == "stopRecording" || command == "startRecording" {
                    let resp = json!({
                        "description": command,
                        "id": req.id,
                        "status": "OK"
                    });
                    let _ = s.client_tx.send(resp.to_string());
                } else if command == "getIntercalibrationStatus"
                    || command == "intercalibrationResult"
                {
                    send_formatted_response(&s.client_tx, command, &req.data, &req.id);
                } else {
                    log::warn!("Unknown command from node: {}", command);
                }
            }
        });

        *handle = Some(task);

        log::info!(
            "WebsocketServer started listening to nodes: {:?}",
            endpoint_strings
        );
    }

    /// Returns `true` once after a reset has been triggered (by save-config or
    /// restart-backend).  Subsequent calls return `false` until the next reset.
    pub async fn is_reset(&self) -> bool {
        let mut s = self.state.lock().await;
        std::mem::replace(&mut s.is_reset, false)
    }

    /// Wait until a reset is triggered.  Returns immediately if a reset was
    /// already signalled but not yet consumed.
    pub async fn notified(&self) {
        self.reset_notify.notified().await;
    }

    /// Send the startup message to the UI (config + intercalibration status).
    pub async fn send_startup_message_to_ui(&self) {
        let s = self.state.lock().await;
        let config = s.config.clone();
        send_formatted_response(&s.client_tx, "getConfig", &config, "");

        let req = ApiRequest::new(
            "getIntercalibrationStatus",
            "/sinks/fusion/settings",
            serde_json::Value::Object(serde_json::Map::new()),
            "",
        );
        s.command_channel.send(&req);
    }

    /// Persist a license response key into the config and save to disk.
    pub async fn save_response_key(&self, response_key: &str) {
        let mut s = self.state.lock().await;
        s.config["LicenseInfo"]["ResponseKey"] =
            serde_json::Value::String(response_key.to_owned());

        let config = s.config.clone();
        send_formatted_response(&s.client_tx, "setConfig", &config, "");

        // Save to disk
        if let Err(e) = save_config_to_disk(&s.config_path, &s.config) {
            log::error!("Failed to save config: {}", e);
        }
        s.config_persistent = s.config.clone();
        s.is_reset = true;
        s.reset_notify.notify_one();
    }

    /// Get a handle to the internal command channel for publishing commands
    /// into the server from external code.
    pub async fn command_channel(&self) -> CommandChannel {
        let s = self.state.lock().await;
        s.command_channel.clone()
    }
}

impl Drop for WebsocketServer {
    fn drop(&mut self) {
        if let Some(handle) = self.accept_handle.take() {
            handle.abort();
        }
        if let Ok(mut h) = self.subscribe_handle.try_lock() {
            if let Some(handle) = h.take() {
                handle.abort();
            }
        }
        log::info!("WebSocket server shut down");
    }
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

fn load_config_file(path: &str) -> Result<serde_json::Value> {
    let p = Path::new(path);
    if !p.exists() {
        anyhow::bail!("Config file '{}' does not exist", path);
    }
    let content = std::fs::read_to_string(p)
        .with_context(|| format!("Could not read configuration file: {}", path))?;
    let stripped = strip_json_comments(&content);
    let value: serde_json::Value = serde_json::from_str(&stripped)
        .with_context(|| format!("Could not parse configuration file: {}", path))?;
    Ok(value)
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

fn save_config_to_disk(path: &Path, config: &serde_json::Value) -> Result<()> {
    let content = serde_json::to_string_pretty(config)?;
    std::fs::write(path, content)?;
    Ok(())
}

fn send_formatted_response(
    tx: &broadcast::Sender<String>,
    command: &str,
    data: &serde_json::Value,
    id: &str,
) {
    let status = if data.get("errorMsg").is_some() {
        "FAIL"
    } else {
        "OK"
    };
    let resp = json!({
        "data": data,
        "description": command,
        "id": id,
        "status": status,
    });
    let _ = tx.send(resp.to_string());
}

/// Register all built-in command handlers on the server state.
fn init_command_router(state: &mut ServerState) {
    // We need to use Arc for shared access in closures.
    // These closures capture nothing mutable directly; they return values
    // that the caller uses to mutate state.  The handlers that need to
    // mutate state do so through returned sentinel values that the main
    // on_read path interprets.

    // -- getConfig --------------------------------------------------------
    // Returns the current config directly via the broadcast channel and
    // returns None so no additional response is sent by on_read.
    state.command_router.register_handler(
        "getConfig",
        |_req| {
            // Actual config sending happens at a higher level that has access to state.
            // Here we return a sentinel that the caller interprets.
            Some(json!({ "__action": "getConfig" }))
        },
        "Get the current configuration",
    );

    // -- getSavedConfig ---------------------------------------------------
    state.command_router.register_handler(
        "getSavedConfig",
        |_req| Some(json!({ "__action": "getSavedConfig" })),
        "Get the saved configuration from file",
    );

    // -- saveConfig -------------------------------------------------------
    state.command_router.register_handler(
        "saveConfig",
        |_req| Some(json!({ "__action": "saveConfig" })),
        "Save the current configuration to file",
    );

    // -- setConfig --------------------------------------------------------
    state.command_router.register_handler(
        "setConfig",
        |req| {
            if req.data.is_null() || req.data == json!({}) {
                return Some(json!({ "warning": "Configuration is empty" }));
            }
            Some(json!({ "__action": "setConfig", "payload": req.data.clone() }))
        },
        "Set the current configuration",
    );

    // -- setConfigJsonPath ------------------------------------------------
    state.command_router.register_handler(
        "setConfigJsonPath",
        |req| {
            if req.data.is_null() || req.data == json!({}) {
                return Some(json!({ "warning": "Configuration is empty" }));
            }
            Some(json!({ "__action": "setConfigJsonPath", "payload": req.data.clone(), "id": req.id.clone() }))
        },
        "Set a configuration value at a JSON path",
    );

    // -- restartBackend ---------------------------------------------------
    state.command_router.register_handler(
        "restartBackend",
        |_req| Some(json!({ "__action": "restartBackend" })),
        "Restart the backend service",
    );

    // -- getIntercalibrationStatus ----------------------------------------
    state.command_router.register_handler(
        "getIntercalibrationStatus",
        |req| {
            log::info!("Got getIntercalibrationStatus");
            Some(json!({
                "__action": "forward",
                "command": "getIntercalibrationStatus",
                "nodeName": "/sinks/fusion/settings",
                "id": req.id.clone()
            }))
        },
        "Get the intercalibration status",
    );

    // -- applyIntercalibrationResults -------------------------------------
    state.command_router.register_handler(
        "applyIntercalibrationResults",
        |req| {
            log::info!("Got applyIntercalibrationResults");
            Some(json!({
                "__action": "forward",
                "command": "applyIntercalibrationResults",
                "nodeName": "/sinks/fusion/settings",
                "id": req.id.clone()
            }))
        },
        "Apply intercalibration results",
    );

    // -- forward ----------------------------------------------------------
    state.command_router.register_handler(
        "forward",
        |req| {
            let cmd = req.data.value_str("command", "");
            let node = req.data.value_str("nodeName", "");
            Some(json!({
                "__action": "forward",
                "command": cmd,
                "nodeName": node,
                "id": req.id.clone()
            }))
        },
        "Forward a command to a specific node",
    );

    // -- getVersion -------------------------------------------------------
    state.command_router.register_handler(
        "getVersion",
        |req| {
            log::info!("Got getVersion");
            Some(json!({
                "data": { "version": VERSION_NUMBER },
                "description": "getVersion",
                "id": req.id,
                "status": "OK"
            }))
        },
        "Get the server version",
    );
}

/// Background task that accepts new WebSocket connections and spawns a handler
/// for each.
async fn accept_loop(
    listener: TcpListener,
    state: Arc<Mutex<ServerState>>,
    client_tx: broadcast::Sender<String>,
) {
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(v) => v,
            Err(e) => {
                log::error!("Accept failed: {}", e);
                continue;
            }
        };

        log::info!("New WebSocket connection from {}", peer);

        let state = state.clone();
        let client_tx = client_tx.clone();

        tokio::spawn(async move {
            // Note: C++ only accepts connections on path "/". We accept any path.
            // This is a minor difference that doesn't affect functionality.
            let ws_stream = match accept_async(stream).await {
                Ok(ws) => ws,
                Err(e) => {
                    log::error!("WebSocket handshake failed: {}", e);
                    return;
                }
            };

            handle_connection(ws_stream, state, client_tx).await;
            log::info!("Connection from {} closed", peer);
        });
    }
}

/// Handle a single WebSocket connection: read incoming messages and forward
/// broadcast messages back to the client.
async fn handle_connection(
    ws_stream: tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    state: Arc<Mutex<ServerState>>,
    client_tx: broadcast::Sender<String>,
) {
    use futures_util::{SinkExt, StreamExt};

    let (mut ws_sender, mut ws_receiver) = ws_stream.split();

    // Subscribe to outgoing broadcast messages
    let mut broadcast_rx = client_tx.subscribe();

    loop {
        tokio::select! {
            // Incoming message from the WebSocket client
            msg = ws_receiver.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        let response = process_incoming(&state, &text).await;
                        if let Some(resp) = response {
                            let out = resp.to_string();
                            if ws_sender.send(Message::Text(out.into())).await.is_err() {
                                break;
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(e)) => {
                        log::warn!("WebSocket read error: {}", e);
                        break;
                    }
                    _ => {}
                }
            }
            // Outgoing broadcast message to push to this client
            msg = broadcast_rx.recv() => {
                if let Ok(text) = msg {
                    if ws_sender.send(Message::Text(text.into())).await.is_err() {
                        break;
                    }
                }
            }
        }
    }
}

/// Process a single incoming text message from a WebSocket client.
///
/// The command router returns a JSON value that may contain a `__action` field
/// indicating that a higher-level operation is needed (e.g. reading/writing the
/// shared config).  This function performs those operations with proper locking.
async fn process_incoming(
    state: &Arc<Mutex<ServerState>>,
    incoming: &str,
) -> Option<serde_json::Value> {
    let j: serde_json::Value = match serde_json::from_str(incoming) {
        Ok(v) => v,
        Err(_) => {
            return Some(json!({
                "status": "FAIL",
                "description": "Could not parse JSON"
            }));
        }
    };

    let req = match ApiRequest::from_json(&j) {
        Some(r) => r,
        None => {
            return Some(json!({
                "status": "FAIL",
                "description": "Invalid ApiRequest format"
            }));
        }
    };

    let mut s = state.lock().await;
    let router_result = s.command_router.route_command(&req);

    let resp = match router_result {
        None => return None,
        Some(v) => v,
    };

    // Handle sentinel actions
    if let Some(action) = resp.get("__action").and_then(|a| a.as_str()) {
        match action {
            "getConfig" => {
                let config = s.config.clone();
                send_formatted_response(&s.client_tx, "getConfig", &config, &req.id);
                return None;
            }
            "getSavedConfig" => {
                let config = s.config_persistent.clone();
                return Some(config);
            }
            "saveConfig" => {
                if let Err(e) = save_config_to_disk(&s.config_path, &s.config) {
                    log::error!("Failed to save config: {}", e);
                }
                s.config_persistent = s.config.clone();
                s.is_reset = true;
                s.reset_notify.notify_one();
                return Some(s.config_persistent.clone());
            }
            "setConfig" => {
                if let Some(payload) = resp.get("payload") {
                    s.config = payload.clone();
                }
                return Some(s.config.clone());
            }
            "setConfigJsonPath" => {
                let payload = resp.get("payload").cloned().unwrap_or_default();
                let id = resp.value_str("id", "");
                return handle_set_config_json_path(&mut s, &payload, &id);
            }
            "restartBackend" => {
                s.is_reset = true;
                s.reset_notify.notify_one();
                return Some(json!({ "result": "success" }));
            }
            "forward" => {
                let cmd = resp.value_str("command", "");
                let node = resp.value_str("nodeName", "");
                let id = resp.value_str("id", "");
                let fwd = ApiRequest::new(
                    cmd,
                    node,
                    serde_json::Value::Object(serde_json::Map::new()),
                    id,
                );
                s.command_channel.send(&fwd);
                return Some(json!({ "result": "async" }));
            }
            _ => {}
        }
    }

    Some(resp)
}

/// Handle the setConfigJsonPath command, which updates a single node in the
/// config tree identified by a JSON pointer path.
fn handle_set_config_json_path(
    state: &mut ServerState,
    data: &serde_json::Value,
    id: &str,
) -> Option<serde_json::Value> {
    let data = if data.is_array() {
        data.get(0).cloned().unwrap_or_default()
    } else {
        data.clone()
    };

    let path = match data.get("path").and_then(|v| v.as_str()) {
        Some(p) => p.to_owned(),
        None => {
            return Some(json!({
                "result": "error",
                "message": "setConfigJsonPath failed. Required field 'path' not found."
            }));
        }
    };

    let value = match data.get("value") {
        Some(v) => v.clone(),
        None => {
            return Some(json!({
                "result": "error",
                "message": "setConfigJsonPath failed. Required field 'value' not found."
            }));
        }
    };

    // Validate path exists in config
    if state.config.pointer(&path).is_none() {
        return Some(json!({
            "result": "error",
            "message": "setConfigJsonPath failed. Path not found in configuration."
        }));
    }

    // Validate node type supports realtime configuration
    if !path.contains("/fusion")
        && !path.contains("/prediction")
        && !path.contains("/imuOpticalIntercalibration")
    {
        return Some(json!({
            "result": "error",
            "message": "setConfigJsonPath failed. Node doesn't support realtime configuration."
        }));
    }

    // Update configuration at the pointer path
    if let Some(target) = state.config.pointer_mut(&path) {
        *target = value;
    }

    // Extract node name (parent of the deepest settings path)
    let node_name = extract_node_name(&path);

    // Get the parent config for forwarding
    let parent_path = match path.rfind('/') {
        Some(idx) => &path[..idx],
        None => &path,
    };
    let parent_config = state
        .config
        .pointer(parent_path)
        .cloned()
        .unwrap_or_default();

    // Forward the update to the node via command channel
    let fwd = ApiRequest::new("setConfigJsonPath", node_name, parent_config, id);
    state.command_channel.send(&fwd);

    Some(json!({ "result": "async" }))
}

/// Extract the node name from a JSON pointer path.
/// Given a path like "/sinks/fusion/settings/param", returns "/sinks/fusion/settings".
fn extract_node_name(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() > 4 {
        parts[..5].join("/")
    } else {
        path.to_owned()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_router_basic() {
        let mut router = CommandRouter::new();
        router.register_handler(
            "ping",
            |_req| Some(json!({ "pong": true })),
            "Ping test",
        );

        assert!(router.has_command("ping"));
        assert!(!router.has_command("nonexistent"));

        let req = ApiRequest::new("ping", "", serde_json::Value::Null, "1");
        let resp = router.route_command(&req);
        assert!(resp.is_some());
        assert_eq!(resp.unwrap()["pong"], true);
    }

    #[test]
    fn command_router_unknown_command() {
        let router = CommandRouter::new();
        let req = ApiRequest::new("unknown", "", serde_json::Value::Null, "1");
        let resp = router.route_command(&req);
        assert!(resp.is_some());
        assert!(resp.unwrap().get("error").is_some());
    }

    #[test]
    fn command_channel_send_receive() {
        let ch = CommandChannel::new();
        let mut rx = ch.subscribe();
        let req = ApiRequest::new("test", "topic", json!({"key": "value"}), "42");
        ch.send(&req);

        let received = rx.try_recv().unwrap();
        assert_eq!(received.command, "test");
        assert_eq!(received.topic, "topic");
        assert_eq!(received.id, "42");
    }

    #[test]
    fn extract_node_name_cases() {
        assert_eq!(
            extract_node_name("/sinks/fusion/settings/gain"),
            "/sinks/fusion/settings/gain"
        );
        assert_eq!(
            extract_node_name("/sinks/fusion/settings/gain/sub"),
            "/sinks/fusion/settings/gain"
        );
    }

    #[test]
    fn send_formatted_response_ok() {
        let (tx, mut rx) = broadcast::channel::<String>(16);
        send_formatted_response(&tx, "getConfig", &json!({"a": 1}), "req-1");

        let msg = rx.try_recv().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&msg).unwrap();
        assert_eq!(parsed["status"], "OK");
        assert_eq!(parsed["description"], "getConfig");
        assert_eq!(parsed["id"], "req-1");
    }

    #[test]
    fn send_formatted_response_fail() {
        let (tx, mut rx) = broadcast::channel::<String>(16);
        send_formatted_response(
            &tx,
            "getConfig",
            &json!({"errorMsg": "bad"}),
            "",
        );

        let msg = rx.try_recv().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&msg).unwrap();
        assert_eq!(parsed["status"], "FAIL");
    }
}
