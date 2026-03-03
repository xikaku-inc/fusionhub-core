use std::sync::Arc;
use std::time::Duration;

use fusion_types::StreamableData;
use tokio::sync::broadcast;
use zeromq::{Socket, SocketRecv, SubSocket};

use crate::runtime::{get_data_channel, is_inproc, resolve_endpoint_for_connect, runtime_handle};

#[derive(Debug)]
pub enum ProbeError {
    NoChannel(String),
    ConnectFailed(String),
    Timeout,
}

impl std::fmt::Display for ProbeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProbeError::NoChannel(ep) => {
                write!(f, "No active channel for '{}' — is the node running?", ep)
            }
            ProbeError::ConnectFailed(e) => write!(f, "Failed to connect: {}", e),
            ProbeError::Timeout => write!(f, "Timeout: no samples received"),
        }
    }
}

/// Probe an endpoint and collect up to `max_samples` within `timeout`.
/// Returns collected samples (may be fewer than max_samples if timeout expires).
pub async fn probe_endpoint(
    endpoint: &str,
    max_samples: usize,
    timeout: Duration,
) -> Result<Vec<StreamableData>, ProbeError> {
    if is_inproc(endpoint) {
        probe_inproc(endpoint, max_samples, timeout).await
    } else {
        probe_tcp(endpoint, max_samples, timeout).await
    }
}

async fn probe_inproc(
    endpoint: &str,
    max_samples: usize,
    timeout: Duration,
) -> Result<Vec<StreamableData>, ProbeError> {
    let sender = get_data_channel(endpoint)
        .ok_or_else(|| ProbeError::NoChannel(endpoint.to_string()))?;
    let mut rx = sender.subscribe();
    let mut samples = Vec::with_capacity(max_samples);

    let deadline = tokio::time::Instant::now() + timeout;

    while samples.len() < max_samples {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(arc_data)) => {
                samples.push(Arc::unwrap_or_clone(arc_data));
            }
            Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(broadcast::error::RecvError::Closed)) => break,
            Err(_) => break, // timeout
        }
    }

    if samples.is_empty() {
        Err(ProbeError::Timeout)
    } else {
        Ok(samples)
    }
}

async fn probe_tcp(
    endpoint: &str,
    max_samples: usize,
    timeout: Duration,
) -> Result<Vec<StreamableData>, ProbeError> {
    let resolved = resolve_endpoint_for_connect(endpoint);
    let ep = endpoint.to_string();

    let handle = runtime_handle();
    handle
        .spawn(async move {
            let mut socket = SubSocket::new();
            socket
                .subscribe("")
                .await
                .map_err(|e| ProbeError::ConnectFailed(format!("subscribe: {}", e)))?;

            match tokio::time::timeout(Duration::from_secs(3), socket.connect(&resolved)).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    return Err(ProbeError::ConnectFailed(format!(
                        "{} ({}): {}",
                        ep, resolved, e
                    )));
                }
                Err(_) => {
                    return Err(ProbeError::ConnectFailed(format!(
                        "{} ({}): connect timeout",
                        ep, resolved
                    )));
                }
            }

            let mut samples = Vec::with_capacity(max_samples);
            let deadline = tokio::time::Instant::now() + timeout;

            while samples.len() < max_samples {
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                if remaining.is_zero() {
                    break;
                }
                match tokio::time::timeout(remaining, socket.recv()).await {
                    Ok(Ok(msg)) => {
                        let bytes: Vec<u8> = msg
                            .into_vec()
                            .first()
                            .cloned()
                            .map(|frame| frame.to_vec())
                            .unwrap_or_default();
                        if let Some(data) = fusion_protobuf::decode(&bytes) {
                            samples.push(data);
                        }
                    }
                    Ok(Err(e)) => {
                        log::warn!("Probe [{}] recv error: {}", ep, e);
                        break;
                    }
                    Err(_) => break, // timeout
                }
            }

            if samples.is_empty() {
                Err(ProbeError::Timeout)
            } else {
                Ok(samples)
            }
        })
        .await
        .unwrap_or(Err(ProbeError::Timeout))
}
