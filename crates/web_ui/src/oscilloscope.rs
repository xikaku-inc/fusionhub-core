use std::collections::HashSet;
use std::time::{Duration, Instant, UNIX_EPOCH};

use fusion_types::StreamableData;
use networking::{get_data_channel, is_inproc, resolve_endpoint_for_connect};
use serde_json::{json, Value};
use tokio::sync::broadcast;
use zeromq::{Socket, SocketRecv, SubSocket};

/// Enumerate all numeric field paths for a given StreamableData variant.
/// Uses serde_json introspection on a default instance.
pub fn enumerate_numeric_fields(data_type: &str) -> Vec<String> {
    let sample: StreamableData = match data_type {
        "Imu" => StreamableData::Imu(Default::default()),
        "Gnss" => StreamableData::Gnss(Default::default()),
        "Optical" => StreamableData::Optical(Default::default()),
        "FusedPose" => StreamableData::FusedPose(Default::default()),
        "FusedVehiclePose" => StreamableData::FusedVehiclePose(Default::default()),
        "FusedVehiclePoseV2" => StreamableData::FusedVehiclePoseV2(Default::default()),
        "GlobalFusedPose" => StreamableData::GlobalFusedPose(Default::default()),
        "FusionStateInt" => StreamableData::FusionStateInt(Default::default()),
        "Rtcm" => StreamableData::Rtcm(Default::default()),
        "Can" => StreamableData::Can(Default::default()),
        "VehicleState" => StreamableData::VehicleState(Default::default()),
        "VehicleSpeed" => StreamableData::VehicleSpeed(Default::default()),
        "VelocityMeter" => StreamableData::VelocityMeter(Default::default()),
        _ => return vec![],
    };

    let json = match serde_json::to_value(&sample) {
        Ok(v) => v,
        Err(_) => return vec![],
    };

    // Serde tags enums as {"Imu": {...}} — get the inner object
    let inner = match json.as_object().and_then(|m| m.values().next()) {
        Some(v) => v,
        None => return vec![],
    };

    let mut paths = Vec::new();
    collect_numeric_paths(inner, "", &mut paths);

    // Filter out non-useful fields
    paths.retain(|p| {
        !matches!(
            p.as_str(),
            "timestamp" | "senderId" | "transmissionTime" | "lastDataTime" | "timecode"
        )
    });

    paths.sort();
    paths
}

fn collect_numeric_paths(value: &Value, prefix: &str, paths: &mut Vec<String>) {
    match value {
        Value::Number(_) => {
            paths.push(prefix.to_owned());
        }
        Value::Object(map) => {
            for (k, v) in map {
                let path = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{}.{}", prefix, k)
                };
                collect_numeric_paths(v, &path, paths);
            }
        }
        _ => {}
    }
}

/// Extract a numeric field from StreamableData by dot-path.
fn extract_field(data: &StreamableData, data_type: &str, field_path: &str) -> Option<f64> {
    if data.variant_name() != data_type {
        return None;
    }
    let json = serde_json::to_value(data).ok()?;
    let inner = json.get(data_type)?;
    let mut current = inner;
    for key in field_path.split('.') {
        current = current.get(key)?;
    }
    current.as_f64()
}

/// Probe an endpoint to detect which data types are flowing through it.
/// Collects enough samples to discover low-frequency types (e.g. GNSS at 1Hz).
pub async fn detect_types(endpoint: &str) -> Vec<String> {
    use networking::probe::probe_endpoint;
    match probe_endpoint(endpoint, 500, Duration::from_secs(3)).await {
        Ok(samples) => {
            let mut types: Vec<String> = samples
                .iter()
                .map(|s| s.variant_name().to_string())
                .collect();
            types.sort();
            types.dedup();
            types
        }
        Err(e) => {
            log::warn!("Oscilloscope detect: {}", e);
            vec![]
        }
    }
}

/// Run the oscilloscope probe loop. Subscribes to the given endpoint,
/// extracts the requested field, and pushes (timestamp, value) pairs to SSE.
pub async fn run_oscilloscope(
    endpoint: String,
    data_type: String,
    field_path: String,
    max_rate: u32,
    sse_tx: broadcast::Sender<String>,
) {
    let min_interval = Duration::from_millis(1000 / max_rate.max(1) as u64);

    if is_inproc(&endpoint) {
        run_inproc(&endpoint, &data_type, &field_path, min_interval, &sse_tx).await;
    } else {
        run_tcp(&endpoint, &data_type, &field_path, min_interval, &sse_tx).await;
    }
}

async fn run_inproc(
    endpoint: &str,
    data_type: &str,
    field_path: &str,
    min_interval: Duration,
    sse_tx: &broadcast::Sender<String>,
) {
    let sender = match get_data_channel(endpoint) {
        Some(s) => s,
        None => {
            log::warn!("Oscilloscope: no channel for '{}'", endpoint);
            return;
        }
    };
    let mut rx = sender.subscribe();
    let mut last_send = Instant::now() - min_interval;

    loop {
        match rx.recv().await {
            Ok(arc_data) => {
                let now = Instant::now();
                if now.duration_since(last_send) < min_interval {
                    continue;
                }
                if let Some(value) = extract_field(&arc_data, data_type, field_path) {
                    last_send = now;
                    let ts = arc_data
                        .timestamp()
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .map(|d| d.as_secs_f64())
                        .unwrap_or(0.0);
                    let event = json!({"type": "oscilloscope", "data": {"t": ts, "v": value}});
                    let _ = sse_tx.send(event.to_string());
                }
            }
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

async fn run_tcp(
    endpoint: &str,
    data_type: &str,
    field_path: &str,
    min_interval: Duration,
    sse_tx: &broadcast::Sender<String>,
) {
    let resolved = resolve_endpoint_for_connect(endpoint);
    let ep = endpoint.to_string();
    let data_type = data_type.to_string();
    let field_path = field_path.to_string();
    let sse_tx = sse_tx.clone();

    let mut socket = SubSocket::new();
    if let Err(e) = socket.subscribe("").await {
        log::warn!("Oscilloscope: subscribe failed for {}: {}", ep, e);
        return;
    }

    match tokio::time::timeout(Duration::from_secs(3), socket.connect(&resolved)).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            log::warn!("Oscilloscope: connect failed for {} ({}): {}", ep, resolved, e);
            return;
        }
        Err(_) => {
            log::warn!("Oscilloscope: connect timeout for {} ({})", ep, resolved);
            return;
        }
    }

    let mut last_send = Instant::now() - min_interval;

    loop {
        match socket.recv().await {
            Ok(msg) => {
                let now = Instant::now();
                if now.duration_since(last_send) < min_interval {
                    continue;
                }
                let bytes: Vec<u8> = msg
                    .into_vec()
                    .first()
                    .cloned()
                    .map(|frame| frame.to_vec())
                    .unwrap_or_default();
                if let Some(data) = fusion_protobuf::decode(&bytes) {
                    if let Some(value) = extract_field(&data, &data_type, &field_path) {
                        last_send = now;
                        let ts = data
                            .timestamp()
                            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                            .map(|d| d.as_secs_f64())
                            .unwrap_or(0.0);
                        let event = json!({"type": "oscilloscope", "data": {"t": ts, "v": value}});
                        let _ = sse_tx.send(event.to_string());
                    }
                }
            }
            Err(e) => {
                log::warn!("Oscilloscope: recv error on {}: {}", ep, e);
                break;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Continuous type discovery
// ---------------------------------------------------------------------------

fn emit_discovered_types(known: &HashSet<String>, sse_tx: &broadcast::Sender<String>) {
    let mut names: Vec<&String> = known.iter().collect();
    names.sort();
    let types: Vec<Value> = names
        .iter()
        .map(|t| {
            json!({
                "dataType": t,
                "fields": enumerate_numeric_fields(t),
            })
        })
        .collect();
    let event = json!({"type": "oscilloscopeTypes", "data": {"types": types}});
    let _ = sse_tx.send(event.to_string());
}

/// Continuously watch an endpoint and emit SSE events when new data types appear.
/// `initial_types` seeds the known set so discovery only emits updates for genuinely new types.
pub async fn run_discovery(
    endpoint: String,
    initial_types: Vec<String>,
    sse_tx: broadcast::Sender<String>,
) {
    let known: HashSet<String> = initial_types.into_iter().collect();
    if is_inproc(&endpoint) {
        discover_inproc(&endpoint, known, &sse_tx).await;
    } else {
        discover_tcp(&endpoint, known, &sse_tx).await;
    }
}

async fn discover_inproc(
    endpoint: &str,
    mut known: HashSet<String>,
    sse_tx: &broadcast::Sender<String>,
) {
    let sender = match get_data_channel(endpoint) {
        Some(s) => s,
        None => return,
    };
    let mut rx = sender.subscribe();

    loop {
        match rx.recv().await {
            Ok(data) => {
                if known.insert(data.variant_name().to_string()) {
                    emit_discovered_types(&known, sse_tx);
                }
            }
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

async fn discover_tcp(
    endpoint: &str,
    mut known: HashSet<String>,
    sse_tx: &broadcast::Sender<String>,
) {
    let resolved = resolve_endpoint_for_connect(endpoint);
    let mut socket = SubSocket::new();
    if socket.subscribe("").await.is_err() {
        return;
    }
    match tokio::time::timeout(Duration::from_secs(3), socket.connect(&resolved)).await {
        Ok(Ok(())) => {}
        _ => return,
    }

    loop {
        match socket.recv().await {
            Ok(msg) => {
                let bytes: Vec<u8> = msg
                    .into_vec()
                    .first()
                    .cloned()
                    .map(|f| f.to_vec())
                    .unwrap_or_default();
                if let Some(data) = fusion_protobuf::decode(&bytes) {
                    if known.insert(data.variant_name().to_string()) {
                        emit_discovered_types(&known, sse_tx);
                    }
                }
            }
            Err(_) => break,
        }
    }
}
