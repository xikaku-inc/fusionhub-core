use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use axum::extract::{Multipart, State};
use axum::http::{header, StatusCode, Uri};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use include_dir::{include_dir, Dir};
use serde_json::{json, Value};
use tokio::sync::{broadcast, Mutex, Notify};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use crypto::LicenseInfo;
use fusion_types::{ApiRequest, JsonValueExt};
use websocket_server::CommandChannel;

static DIST_DIR: Dir = include_dir!("$CARGO_MANIFEST_DIR/src/ui/dist");

const VERSION_NUMBER: &str = "1.0.0";

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

struct ServerState {
    config_path: PathBuf,
    config: Value,
    config_persistent: Value,
    is_reset: bool,
    reset_notify: Arc<Notify>,
    command_channel: CommandChannel,
    /// Broadcast sender for SSE events to all connected browsers.
    sse_tx: broadcast::Sender<String>,
    license_info: LicenseInfo,
    license_key: String,
    license_server_url: String,
    paused: Arc<AtomicBool>,
}

type SharedState = Arc<Mutex<ServerState>>;

// ---------------------------------------------------------------------------
// ConfigHandle — public accessor for MCP server and other consumers
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct ConfigHandle {
    state: SharedState,
}

impl ConfigHandle {
    pub async fn get(&self) -> Value {
        self.state.lock().await.config.clone()
    }

    pub async fn set(&self, config: Value) {
        let mut s = self.state.lock().await;
        s.config = config.clone();
        let event = json!({ "type": "config", "data": config });
        let _ = s.sse_tx.send(event.to_string());
    }

    pub async fn config_path(&self) -> PathBuf {
        self.state.lock().await.config_path.clone()
    }

    pub async fn trigger_restart(&self) {
        let mut s = self.state.lock().await;
        s.is_reset = true;
        s.reset_notify.notify_one();
    }
}

// ---------------------------------------------------------------------------
// WebUiServer
// ---------------------------------------------------------------------------

pub struct WebUiServer {
    _server_handle: tokio::task::JoinHandle<()>,
    state: SharedState,
    reset_notify: Arc<Notify>,
}

impl WebUiServer {
    pub async fn new(
        addr: &str,
        port: u16,
        config: Value,
        config_path: &str,
        command_channel: CommandChannel,
        license_info: LicenseInfo,
        paused: Arc<AtomicBool>,
    ) -> Result<Self> {
        let config_persistent = config.clone();
        let (sse_tx, _) = broadcast::channel::<String>(512);
        let reset_notify = Arc::new(Notify::new());

        let li = config.get("LicenseInfo");
        let license_key = li.and_then(|v| v.get("LicenseKey")).and_then(|v| v.as_str()).unwrap_or("").to_owned();
        let license_server_url = li.and_then(|v| v.get("ServerUrl")).and_then(|v| v.as_str()).unwrap_or("").to_owned();

        let state = Arc::new(Mutex::new(ServerState {
            config_path: PathBuf::from(config_path),
            config,
            config_persistent,
            is_reset: false,
            reset_notify: reset_notify.clone(),
            command_channel: command_channel.clone(),
            sse_tx: sse_tx.clone(),
            license_info,
            license_key,
            license_server_url,
            paused,
        }));

        // Subscribe to CommandChannel and forward node responses as SSE events
        let sse_state = state.clone();
        let mut cmd_rx = command_channel.subscribe();
        tokio::spawn(async move {
            while let Ok(req) = cmd_rx.recv().await {
                Self::handle_node_response(&sse_state, &req).await;
            }
        });

        // Forward log entries to SSE with per-target rate aggregation.
        // High-frequency targets (>10 msgs/sec) get suppressed into periodic
        // summary lines so they don't flood the UI.
        if let Some(mut log_rx) = log_utils::subscribe() {
            let log_sse_tx = sse_tx.clone();
            tokio::spawn(async move {
                use std::collections::HashMap;
                const RATE_THRESHOLD: u32 = 10;

                struct TargetStats {
                    count: u32,
                    level: String,
                    last_msg: String,
                }
                let mut target_counts: HashMap<String, TargetStats> = HashMap::new();
                let mut suppressed: std::collections::HashSet<String> =
                    std::collections::HashSet::new();

                let mut tick =
                    tokio::time::interval(Duration::from_secs(1));
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

                let mut pending: Vec<Value> = Vec::new();

                loop {
                    tokio::select! {
                        result = log_rx.recv() => {
                            match result {
                                Ok(raw) => {
                                    if let Ok(entry) = serde_json::from_str::<Value>(&raw) {
                                        let target = entry["target"]
                                            .as_str()
                                            .unwrap_or("")
                                            .to_owned();

                                        let stats = target_counts
                                            .entry(target.clone())
                                            .or_insert_with(|| TargetStats {
                                                count: 0,
                                                level: String::new(),
                                                last_msg: String::new(),
                                            });
                                        stats.count += 1;
                                        stats.level = entry["level"]
                                            .as_str()
                                            .unwrap_or("INFO")
                                            .to_owned();
                                        stats.last_msg = entry["message"]
                                            .as_str()
                                            .unwrap_or("")
                                            .to_owned();

                                        if !suppressed.contains(&target) {
                                            pending.push(entry);
                                        }
                                    }
                                }
                                Err(broadcast::error::RecvError::Lagged(_)) => {}
                                Err(_) => break,
                            }
                        }
                        _ = tick.tick() => {
                            // Emit summaries for high-frequency targets.
                            for (target, stats) in &target_counts {
                                if stats.count > RATE_THRESHOLD {
                                    suppressed.insert(target.clone());
                                    pending.push(json!({
                                        "ts": chrono::Local::now()
                                            .format("%H:%M:%S%.3f")
                                            .to_string(),
                                        "level": stats.level,
                                        "target": target,
                                        "message": format!(
                                            "{}/s (suppressed) — {}",
                                            stats.count, stats.last_msg
                                        ),
                                    }));
                                }
                            }
                            // Only unsuppress targets that had zero messages
                            // this tick (completely silent).
                            suppressed.retain(|t| target_counts.contains_key(t));
                            target_counts.clear();

                            if !pending.is_empty() {
                                let entries: Vec<Value> = pending.drain(..).collect();
                                let event = json!({ "type": "log", "data": entries });
                                let _ = log_sse_tx.send(event.to_string());
                            }
                        }
                    }

                    if pending.len() >= 30 {
                        let entries: Vec<Value> = pending.drain(..).collect();
                        let event = json!({ "type": "log", "data": entries });
                        let _ = log_sse_tx.send(event.to_string());
                    }
                }
            });
        }

        let bind_addr = format!("{}:{}", addr, port);

        let app = Router::new()
            // REST API
            .route("/api/config", get(api_get_config).post(api_set_config))
            .route("/api/config/save", post(api_save_config))
            .route("/api/config/save-as", post(api_save_config_as))
            .route("/api/config/load", post(api_load_config))
            .route("/api/config/path", post(api_set_config_path))
            .route("/api/restart", post(api_restart))
            .route("/api/pause", post(api_pause))
            .route("/api/resume", post(api_resume))
            .route("/api/paused", get(api_get_paused))
            .route("/api/version", get(api_get_version))
            .route(
                "/api/intercalibration/status",
                post(api_get_intercalibration_status),
            )
            .route(
                "/api/intercalibration/apply",
                post(api_apply_intercalibration),
            )
            .route("/api/forward", post(api_forward))
            .route("/api/logs", get(api_get_logs))
            // Node types (dynamic registry)
            .route("/api/node-types", get(api_get_node_types))
            // UI extensions (dynamic registry)
            .route("/api/ui-extensions", get(api_get_ui_extensions))
            .route("/ui-ext/{id}", get(serve_ui_extension_bundle))
            // License API
            .route("/api/license/status", get(api_get_license_status))
            .route("/api/license/check-file", post(api_check_license_file))
            .route("/api/license/check-server", post(api_check_license_server))
            .route("/api/license/check-token", post(api_check_license_token))
            .route("/api/license/upload", post(api_upload_license))
            .route("/api/license/machines", post(api_list_machines))
            .route("/api/license/deactivate-machine", post(api_deactivate_machine))
            // File dialog
            .route("/api/file-dialog", post(api_file_dialog))
            // SSE
            .route("/api/events", get(sse_handler))
            // Static files from React build (catch-all, must be last)
            .fallback(serve_static)
            .with_state(state.clone());

        let listener = tokio::net::TcpListener::bind(&bind_addr)
            .await
            .with_context(|| format!("Failed to bind Web UI server on {}", bind_addr))?;

        log::info!("Web UI server listening on http://{}", bind_addr);

        let server_handle = tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, app).await {
                log::error!("Web UI server error: {}", e);
            }
        });

        Ok(Self {
            _server_handle: server_handle,
            state,
            reset_notify,
        })
    }

    async fn handle_node_response(state: &SharedState, req: &ApiRequest) {
        let s = state.lock().await;
        let command = &req.command;

        if command == "ws" {
            let data = &req.data;
            let id = &req.id;

            // Check for wrapped status messages (DataMonitor, ImuOpticalFilter, etc.)
            // These have { "data": {...}, "description": "inputStatus"|"status", "status": "OK" }
            if let Some(desc) = data.get("description").and_then(|v| v.as_str()) {
                let inner = data.get("data").unwrap_or(data);
                let event = json!({
                    "type": desc,
                    "data": inner,
                });
                let _ = s.sse_tx.send(event.to_string());
                return;
            }

            // Direct data messages (fusedPose, opticalData, fusedVehiclePose)
            let event_type = if data.get("fusedPose").is_some() {
                "fusedPose"
            } else if data.get("opticalData").is_some() {
                "opticalData"
            } else if data.get("fusedVehiclePose").is_some()
                || data.get("fusedVehiclePoseV2").is_some()
            {
                "fusedVehiclePose"
            } else if !id.is_empty() {
                id.as_str()
            } else {
                "data"
            };

            let event = json!({
                "type": event_type,
                "data": data,
            });
            let _ = s.sse_tx.send(event.to_string());
        } else if command == "setConfigJsonPath" {
            // Config update from node
            // Note: the actual config mutation is handled by the WebSocket server
            // We just forward the notification to SSE clients
            let event = json!({
                "type": "configUpdate",
                "data": req.data,
            });
            let _ = s.sse_tx.send(event.to_string());
        } else if command == "getIntercalibrationStatus"
            || command == "intercalibrationResult"
        {
            let event = json!({
                "type": command,
                "data": req.data,
            });
            let _ = s.sse_tx.send(event.to_string());
        } else if command == "applyIntercalibrationResults" {
            // After applying results, send updated config
            let event = json!({
                "type": "config",
                "data": s.config.clone(),
            });
            let _ = s.sse_tx.send(event.to_string());
        }
    }

    pub async fn get_config(&self) -> Value {
        self.state.lock().await.config.clone()
    }

    pub async fn set_config(&self, config: Value) {
        let mut s = self.state.lock().await;
        s.config = config;
    }

    pub async fn is_reset(&self) -> bool {
        let mut s = self.state.lock().await;
        std::mem::replace(&mut s.is_reset, false)
    }

    pub async fn notified(&self) {
        self.reset_notify.notified().await;
    }

    pub async fn update_config(&self, config: Value) {
        let mut s = self.state.lock().await;
        s.config = config.clone();
        let event = json!({ "type": "config", "data": config });
        let _ = s.sse_tx.send(event.to_string());
    }

    pub async fn update_license_status(&self, info: LicenseInfo) {
        let mut s = self.state.lock().await;
        s.license_info = info.clone();
        let event = json!({
            "type": "licenseStatus",
            "data": {
                "info": info,
                "licenseKey": s.license_key,
                "serverUrl": s.license_server_url,
            }
        });
        let _ = s.sse_tx.send(event.to_string());
    }

    pub async fn sse_sender(&self) -> broadcast::Sender<String> {
        self.state.lock().await.sse_tx.clone()
    }

    pub async fn paused_flag(&self) -> Arc<AtomicBool> {
        self.state.lock().await.paused.clone()
    }

    pub fn config_handle(&self) -> ConfigHandle {
        ConfigHandle { state: self.state.clone() }
    }

    pub async fn send_startup_config(&self) {
        let s = self.state.lock().await;
        let event = json!({
            "type": "config",
            "data": s.config.clone(),
        });
        let _ = s.sse_tx.send(event.to_string());
    }
}

// ---------------------------------------------------------------------------
// Static file serving from embedded React build
// ---------------------------------------------------------------------------

async fn serve_static(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    if let Some(file) = DIST_DIR.get_file(path) {
        let mime = mime_guess::from_path(path)
            .first_or_octet_stream()
            .to_string();
        (
            StatusCode::OK,
            [(header::CONTENT_TYPE, mime)],
            file.contents(),
        )
            .into_response()
    } else {
        // SPA fallback: serve index.html for any non-file route (client-side routing)
        match DIST_DIR.get_file("index.html") {
            Some(index) => (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/html".to_string())],
                index.contents(),
            )
                .into_response(),
            None => (StatusCode::NOT_FOUND, "Not found").into_response(),
        }
    }
}

// ---------------------------------------------------------------------------
// REST API handlers
// ---------------------------------------------------------------------------

async fn api_get_config(State(state): State<SharedState>) -> Json<Value> {
    let s = state.lock().await;
    Json(s.config.clone())
}

async fn api_set_config(
    State(state): State<SharedState>,
    Json(body): Json<Value>,
) -> Json<Value> {
    let mut s = state.lock().await;
    s.config = body;
    let config = s.config.clone();

    // Notify SSE clients
    let event = json!({ "type": "config", "data": config });
    let _ = s.sse_tx.send(event.to_string());

    Json(json!({ "status": "OK" }))
}

async fn api_save_config(State(state): State<SharedState>) -> Json<Value> {
    let mut s = state.lock().await;
    match save_config_to_disk(&s.config_path, &s.config) {
        Ok(()) => {
            s.config_persistent = s.config.clone();
            s.is_reset = true;
            s.reset_notify.notify_one();
            Json(json!({ "status": "OK" }))
        }
        Err(e) => {
            log::error!("Failed to save config: {}", e);
            Json(json!({ "status": "FAIL", "error": e.to_string() }))
        }
    }
}

async fn api_save_config_as(
    State(state): State<SharedState>,
    Json(body): Json<Value>,
) -> Json<Value> {
    let mut s = state.lock().await;
    let path = match body.get("path").and_then(|v| v.as_str()) {
        Some(p) => PathBuf::from(p),
        None => return Json(json!({ "status": "FAIL", "error": "Missing 'path' field" })),
    };
    match save_config_to_disk(&path, &s.config) {
        Ok(()) => {
            s.config_path = path;
            s.config_persistent = s.config.clone();
            s.is_reset = true;
            s.reset_notify.notify_one();
            Json(json!({ "status": "OK" }))
        }
        Err(e) => {
            log::error!("Failed to save config as {:?}: {}", path, e);
            Json(json!({ "status": "FAIL", "error": e.to_string() }))
        }
    }
}

async fn api_load_config(
    State(state): State<SharedState>,
    Json(body): Json<Value>,
) -> Json<Value> {
    let path = match body.get("path").and_then(|v| v.as_str()) {
        Some(p) => PathBuf::from(p),
        None => return Json(json!({ "status": "FAIL", "error": "Missing 'path' field" })),
    };
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => return Json(json!({ "status": "FAIL", "error": format!("Failed to read file: {}", e) })),
    };
    let config: Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => return Json(json!({ "status": "FAIL", "error": format!("Invalid JSON: {}", e) })),
    };
    let mut s = state.lock().await;
    s.config = config.clone();
    s.config_path = path;
    let event = json!({ "type": "config", "data": &config });
    let _ = s.sse_tx.send(event.to_string());
    Json(json!({ "status": "OK", "config": config }))
}

async fn api_restart(State(state): State<SharedState>) -> Json<Value> {
    let mut s = state.lock().await;
    s.is_reset = true;
    s.reset_notify.notify_one();
    Json(json!({ "status": "OK" }))
}

async fn api_pause(State(state): State<SharedState>) -> Json<Value> {
    let s = state.lock().await;
    s.paused.store(true, Ordering::Relaxed);
    Json(json!({ "status": "OK" }))
}

async fn api_resume(State(state): State<SharedState>) -> Json<Value> {
    let s = state.lock().await;
    s.paused.store(false, Ordering::Relaxed);
    Json(json!({ "status": "OK" }))
}

async fn api_get_paused(State(state): State<SharedState>) -> Json<Value> {
    let s = state.lock().await;
    Json(json!({ "paused": s.paused.load(Ordering::Relaxed) }))
}

async fn api_set_config_path(
    State(state): State<SharedState>,
    Json(body): Json<Value>,
) -> Json<Value> {
    let mut s = state.lock().await;

    let path = match body.get("path").and_then(|v| v.as_str()) {
        Some(p) => p.to_owned(),
        None => {
            return Json(json!({
                "status": "FAIL",
                "error": "Missing 'path' field"
            }));
        }
    };

    let value = match body.get("value") {
        Some(v) => v.clone(),
        None => {
            return Json(json!({
                "status": "FAIL",
                "error": "Missing 'value' field"
            }));
        }
    };

    // Validate path exists
    if s.config.pointer(&path).is_none() {
        return Json(json!({
            "status": "FAIL",
            "error": "Path not found in configuration"
        }));
    }

    // Validate node supports realtime config (dynamic from registry)
    let node_key = extract_node_key_from_config_path(&path);
    if !fusion_registry::supports_realtime_config(&node_key) {
        return Json(json!({
            "status": "FAIL",
            "error": "Node doesn't support realtime configuration"
        }));
    }

    // Update config
    if let Some(target) = s.config.pointer_mut(&path) {
        *target = value;
    }

    // Extract node name and forward
    let node_name = extract_node_name(&path);
    let parent_path = match path.rfind('/') {
        Some(idx) => &path[..idx],
        None => &path,
    };
    let parent_config = s
        .config
        .pointer(parent_path)
        .cloned()
        .unwrap_or_default();

    let fwd = ApiRequest::new("setConfigJsonPath", &node_name, parent_config, "");
    s.command_channel.send(&fwd);

    Json(json!({ "status": "OK" }))
}

async fn api_get_version() -> Json<Value> {
    Json(json!({ "version": VERSION_NUMBER }))
}

async fn api_get_logs() -> Json<Value> {
    let entries: Vec<Value> = log_utils::buffered_entries()
        .into_iter()
        .filter_map(|s| serde_json::from_str(&s).ok())
        .collect();
    Json(json!(entries))
}

async fn api_get_node_types() -> Json<Value> {
    let metadata = fusion_registry::all_metadata();
    Json(serde_json::to_value(metadata).unwrap_or_default())
}

async fn api_get_ui_extensions() -> Json<Value> {
    let extensions = fusion_registry::all_ui_extensions();
    Json(serde_json::to_value(extensions).unwrap_or_default())
}

async fn serve_ui_extension_bundle(axum::extract::Path(id): axum::extract::Path<String>) -> Response {
    let id = id.strip_suffix(".js").unwrap_or(&id);
    match fusion_registry::get_ui_extension_bundle(id) {
        Some(bundle) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/javascript")],
            bundle,
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "Extension not found").into_response(),
    }
}

async fn api_get_intercalibration_status(State(state): State<SharedState>) -> Json<Value> {
    let s = state.lock().await;
    let req = ApiRequest::new(
        "getIntercalibrationStatus",
        "/sinks/fusion/settings",
        Value::Object(serde_json::Map::new()),
        "",
    );
    s.command_channel.send(&req);
    Json(json!({ "status": "OK", "message": "Request sent" }))
}

async fn api_apply_intercalibration(State(state): State<SharedState>) -> Json<Value> {
    let s = state.lock().await;
    let req = ApiRequest::new(
        "applyIntercalibrationResults",
        "/sinks/fusion/settings",
        Value::Object(serde_json::Map::new()),
        "",
    );
    s.command_channel.send(&req);
    Json(json!({ "status": "OK", "message": "Request sent" }))
}

async fn api_forward(
    State(state): State<SharedState>,
    Json(body): Json<Value>,
) -> Json<Value> {
    let command = body.value_str("command", "");
    let node_name = body.value_str("nodeName", "");

    if command.is_empty() || node_name.is_empty() {
        return Json(json!({
            "status": "FAIL",
            "error": "Missing 'command' or 'nodeName'"
        }));
    }

    let s = state.lock().await;
    let data = body.get("data").cloned().unwrap_or(Value::Object(serde_json::Map::new()));
    let req = ApiRequest::new(command, node_name, data, "");
    s.command_channel.send(&req);

    Json(json!({ "status": "OK" }))
}

// ---------------------------------------------------------------------------
// License API handlers
// ---------------------------------------------------------------------------

async fn api_get_license_status(State(state): State<SharedState>) -> Json<Value> {
    let s = state.lock().await;
    Json(json!({
        "status": "OK",
        "license": s.license_info,
        "licenseKey": s.license_key,
        "serverUrl": s.license_server_url,
    }))
}

async fn api_check_license_file(
    State(state): State<SharedState>,
    Json(body): Json<Value>,
) -> Json<Value> {
    let license_file = body
        .get("licenseFile")
        .and_then(|v| v.as_str())
        .unwrap_or("license.json")
        .to_owned();

    let config_dir = {
        let s = state.lock().await;
        s.config_path
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf()
    };

    let info = tokio::task::spawn_blocking(move || {
        let resolved = if Path::new(&license_file).is_absolute() {
            PathBuf::from(&license_file)
        } else {
            config_dir.join(&license_file)
        };
        let mut crypto = crypto::Crypto::new();
        crypto.check_license_file(&resolved.to_string_lossy())
    })
    .await
    .unwrap_or_else(|e| LicenseInfo {
        status: "error".into(),
        error: format!("Task failed: {}", e),
        ..Default::default()
    });

    let mut s = state.lock().await;
    s.license_info = info.clone();
    let event = json!({
        "type": "licenseStatus",
        "data": { "info": &info, "licenseKey": &s.license_key, "serverUrl": &s.license_server_url }
    });
    let _ = s.sse_tx.send(event.to_string());
    if info.valid {
        s.is_reset = true;
        s.reset_notify.notify_one();
    }
    Json(json!({ "status": "OK", "license": info }))
}

async fn api_check_license_server(
    State(state): State<SharedState>,
    Json(body): Json<Value>,
) -> Json<Value> {
    let license_file = body
        .get("licenseFile")
        .and_then(|v| v.as_str())
        .unwrap_or("license.json")
        .to_owned();
    let license_key = body
        .get("licenseKey")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned();
    let server_url = body
        .get("serverUrl")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned();

    if license_key.is_empty() || server_url.is_empty() {
        return Json(json!({
            "status": "FAIL",
            "error": "licenseKey and serverUrl are required"
        }));
    }

    let config_dir = {
        let s = state.lock().await;
        s.config_path
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf()
    };

    let key_clone = license_key.clone();
    let url_clone = server_url.clone();
    let info = tokio::task::spawn_blocking(move || {
        let resolved = if Path::new(&license_file).is_absolute() {
            PathBuf::from(&license_file)
        } else {
            config_dir.join(&license_file)
        };
        let mut crypto = crypto::Crypto::new();
        crypto.check_license_server(&resolved.to_string_lossy(), &key_clone, &url_clone)
    })
    .await
    .unwrap_or_else(|e| LicenseInfo {
        status: "error".into(),
        error: format!("Task failed: {}", e),
        ..Default::default()
    });

    let mut s = state.lock().await;
    s.license_info = info.clone();
    if info.valid {
        s.license_key = license_key.clone();
        s.license_server_url = server_url.clone();
        // Persist into in-memory config so it survives save-to-disk
        let li = s.config.get("LicenseInfo").cloned().unwrap_or(json!({}));
        let mut li = li.as_object().cloned().unwrap_or_default();
        li.insert("LicenseKey".into(), json!(license_key));
        li.insert("ServerUrl".into(), json!(server_url));
        s.config["LicenseInfo"] = json!(li);
    }
    let event = json!({
        "type": "licenseStatus",
        "data": { "info": &info, "licenseKey": &s.license_key, "serverUrl": &s.license_server_url }
    });
    let _ = s.sse_tx.send(event.to_string());
    if info.valid {
        s.is_reset = true;
        s.reset_notify.notify_one();
    }
    Json(json!({ "status": "OK", "license": info }))
}

async fn api_check_license_token(State(state): State<SharedState>) -> Json<Value> {
    let info = tokio::task::spawn_blocking(|| {
        let mut crypto = crypto::Crypto::new();
        crypto.check_license_token()
    })
    .await
    .unwrap_or_else(|e| LicenseInfo {
        status: "error".into(),
        error: format!("Task failed: {}", e),
        ..Default::default()
    });

    let mut s = state.lock().await;
    s.license_info = info.clone();
    let event = json!({
        "type": "licenseStatus",
        "data": { "info": &info, "licenseKey": &s.license_key, "serverUrl": &s.license_server_url }
    });
    let _ = s.sse_tx.send(event.to_string());
    if info.valid {
        s.is_reset = true;
        s.reset_notify.notify_one();
    }
    Json(json!({ "status": "OK", "license": info }))
}

async fn api_upload_license(
    State(state): State<SharedState>,
    mut multipart: Multipart,
) -> Json<Value> {
    while let Ok(Some(field)) = multipart.next_field().await {
        if field.name() == Some("file") {
            let data = match field.bytes().await {
                Ok(d) => d,
                Err(e) => {
                    return Json(json!({ "status": "FAIL", "error": e.to_string() }));
                }
            };

            let save_path = {
                let s = state.lock().await;
                s.config_path
                    .parent()
                    .unwrap_or(Path::new("."))
                    .join("license.json")
            };

            if let Err(e) = std::fs::write(&save_path, &data) {
                return Json(json!({ "status": "FAIL", "error": e.to_string() }));
            }

            let path_str = save_path.to_string_lossy().to_string();
            let info = tokio::task::spawn_blocking(move || {
                let mut crypto = crypto::Crypto::new();
                crypto.check_license_file(&path_str)
            })
            .await
            .unwrap_or_else(|e| LicenseInfo {
                status: "error".into(),
                error: format!("Task failed: {}", e),
                ..Default::default()
            });

            let mut s = state.lock().await;
            s.license_info = info.clone();
            let event = json!({
                "type": "licenseStatus",
                "data": { "info": &info, "licenseKey": &s.license_key, "serverUrl": &s.license_server_url }
            });
            let _ = s.sse_tx.send(event.to_string());
            if info.valid {
                s.is_reset = true;
                s.reset_notify.notify_one();
            }
            return Json(json!({ "status": "OK", "license": info }));
        }
    }
    Json(json!({ "status": "FAIL", "error": "No file field found" }))
}

async fn api_list_machines(Json(body): Json<Value>) -> Json<Value> {
    let license_key = body.get("licenseKey").and_then(|v| v.as_str()).unwrap_or("");
    let server_url = body.get("serverUrl").and_then(|v| v.as_str()).unwrap_or("");

    if license_key.is_empty() || server_url.is_empty() {
        return Json(json!({ "status": "FAIL", "error": "licenseKey and serverUrl are required" }));
    }

    let url = format!(
        "{}/licenses/{}/status",
        server_url.trim_end_matches('/'),
        license_key
    );

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();

    match client.get(&url).send().await {
        Ok(resp) => {
            if resp.status().is_success() {
                match resp.json::<Value>().await {
                    Ok(data) => Json(json!({ "status": "OK", "data": data })),
                    Err(e) => Json(json!({ "status": "FAIL", "error": format!("Invalid response: {}", e) })),
                }
            } else {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                Json(json!({ "status": "FAIL", "error": format!("Server returned {}: {}", status, text) }))
            }
        }
        Err(e) => Json(json!({ "status": "FAIL", "error": format!("Request failed: {}", e) })),
    }
}

async fn api_deactivate_machine(Json(body): Json<Value>) -> Json<Value> {
    let license_key = body.get("licenseKey").and_then(|v| v.as_str()).unwrap_or("");
    let server_url = body.get("serverUrl").and_then(|v| v.as_str()).unwrap_or("");
    let machine_code = body.get("machineCode").and_then(|v| v.as_str()).unwrap_or("");

    if license_key.is_empty() || server_url.is_empty() || machine_code.is_empty() {
        return Json(json!({ "status": "FAIL", "error": "licenseKey, serverUrl and machineCode are required" }));
    }

    let url = format!("{}/deactivate", server_url.trim_end_matches('/'));

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();

    match client
        .post(&url)
        .json(&json!({ "license_key": license_key, "machine_code": machine_code }))
        .send()
        .await
    {
        Ok(resp) => {
            if resp.status().is_success() {
                Json(json!({ "status": "OK" }))
            } else {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                Json(json!({ "status": "FAIL", "error": format!("Server returned {}: {}", status, text) }))
            }
        }
        Err(e) => Json(json!({ "status": "FAIL", "error": format!("Request failed: {}", e) })),
    }
}

// ---------------------------------------------------------------------------
// File dialog
// ---------------------------------------------------------------------------

async fn api_file_dialog(Json(body): Json<Value>) -> Json<Value> {
    let title = body.get("title").and_then(|v| v.as_str()).unwrap_or("Select File");
    let filters: Vec<(String, Vec<String>)> = body
        .get("filters")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|f| {
                    let name = f.get("name")?.as_str()?.to_owned();
                    let exts: Vec<String> = f
                        .get("extensions")?
                        .as_array()?
                        .iter()
                        .filter_map(|e| e.as_str().map(|s| s.to_owned()))
                        .collect();
                    Some((name, exts))
                })
                .collect()
        })
        .unwrap_or_default();

    let title = title.to_owned();
    let result = tokio::task::spawn_blocking(move || {
        let mut dialog = rfd::FileDialog::new().set_title(&title);
        for (name, exts) in &filters {
            let ext_refs: Vec<&str> = exts.iter().map(|s| s.as_str()).collect();
            dialog = dialog.add_filter(name, &ext_refs);
        }
        dialog.pick_file()
    })
    .await
    .unwrap_or(None);

    match result {
        Some(path) => Json(json!({ "status": "OK", "path": path.to_string_lossy() })),
        None => Json(json!({ "status": "cancelled" })),
    }
}

// ---------------------------------------------------------------------------
// SSE handler
// ---------------------------------------------------------------------------

async fn sse_handler(
    State(state): State<SharedState>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let rx = {
        let s = state.lock().await;
        s.sse_tx.subscribe()
    };

    let stream = BroadcastStream::new(rx).filter_map(|result| {
        match result {
            Ok(msg) => {
                // Parse the JSON to extract event type
                if let Ok(parsed) = serde_json::from_str::<Value>(&msg) {
                    let event_type = parsed
                        .get("type")
                        .and_then(|t| t.as_str())
                        .unwrap_or("message")
                        .to_owned();
                    let data = parsed.get("data").unwrap_or(&parsed).to_string();
                    Some(Ok::<_, Infallible>(
                        Event::default().event(event_type).data(data),
                    ))
                } else {
                    Some(Ok(Event::default().data(msg)))
                }
            }
            Err(_) => None, // Lagged — skip
        }
    });

    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("ping"),
    )
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn save_config_to_disk(path: &Path, config: &Value) -> Result<()> {
    let content = serde_json::to_string_pretty(config)?;
    std::fs::write(path, content)?;
    Ok(())
}

fn extract_node_name(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() > 4 {
        parts[..5].join("/")
    } else {
        path.to_owned()
    }
}

/// Extract the node config key from a JSON pointer path.
/// E.g. "/sinks/fusion/settings/gain" → "fusion"
fn extract_node_key_from_config_path(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() >= 2 {
        parts[1].to_owned()
    } else {
        String::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

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
}
