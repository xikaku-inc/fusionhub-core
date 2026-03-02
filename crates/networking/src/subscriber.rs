use std::sync::{Arc, Mutex};

use anyhow::Result;
use fusion_types::StreamableData;
use tokio::sync::broadcast;
use zeromq::{Socket, SocketRecv, SubSocket};

use crate::runtime::{get_data_channel, is_inproc, resolve_endpoint_for_connect, runtime_handle};

/// A subscriber that receives StreamableData from one or more endpoints.
///
/// Endpoints prefixed with `inproc://` use tokio broadcast channels (zero-copy,
/// no serialization). TCP endpoints use ZMQ SubSocket (protobuf wire format).
///
/// Call `start_listening()` to spawn background tasks that receive data and
/// invoke the callback. Each endpoint gets its own dedicated task.
pub struct Subscriber {
    m_endpoints: Vec<String>,
    m_task_handles: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
}

impl Subscriber {
    pub fn new(endpoints: Vec<String>) -> Self {
        Self {
            m_endpoints: endpoints,
            m_task_handles: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn from_config(config: &serde_json::Value) -> Self {
        let endpoints = config
            .get("endpoints")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        Self::new(endpoints)
    }

    pub fn endpoints(&self) -> &[String] {
        &self.m_endpoints
    }

    pub fn start_listening<F>(&self, callback: F) -> Result<()>
    where
        F: Fn(StreamableData) + Send + 'static,
    {
        let endpoints = self.m_endpoints.clone();
        if endpoints.is_empty() {
            return Err(anyhow::anyhow!("No endpoints configured for subscriber"));
        }

        let (tx, rx) = std::sync::mpsc::channel::<StreamableData>();

        std::thread::Builder::new()
            .name("sub-cb".into())
            .spawn(move || {
                while let Ok(data) = rx.recv() {
                    callback(data);
                }
            })
            .expect("Failed to spawn subscriber callback thread");

        let handle = runtime_handle();
        let mut handles = Vec::new();

        for ep in endpoints {
            let tx = tx.clone();

            if is_inproc(&ep) {
                let sender = match get_data_channel(&ep) {
                    Some(s) => s,
                    None => {
                        log::warn!("Subscriber: no broadcast channel for '{}'; skipping", ep);
                        continue;
                    }
                };
                let mut rx_bc = sender.subscribe();
                let ep_name = ep.clone();

                let task = handle.spawn(async move {
                    loop {
                        match rx_bc.recv().await {
                            Ok(arc_data) => {
                                let data = Arc::unwrap_or_clone(arc_data);
                                if tx.send(data).is_err() {
                                    break;
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(n)) => {
                                log::warn!("Subscriber [{}] lagged, skipped {} messages", ep_name, n);
                            }
                            Err(broadcast::error::RecvError::Closed) => {
                                break;
                            }
                        }
                    }
                });
                handles.push(task);
            } else {
                let resolved = resolve_endpoint_for_connect(&ep);

                let task = handle.spawn(async move {
                    let mut socket = SubSocket::new();

                    if let Err(e) = socket.subscribe("").await {
                        log::error!("Subscriber [{}] subscribe error: {}", ep, e);
                        return;
                    }

                    match tokio::time::timeout(
                        std::time::Duration::from_secs(3),
                        socket.connect(&resolved),
                    )
                    .await
                    {
                        Ok(Ok(())) => {
                            log::info!("Subscriber connected to {} (from {})", resolved, ep);
                        }
                        Ok(Err(e)) => {
                            log::warn!(
                                "Subscriber failed to connect to {} (from {}): {}",
                                resolved, ep, e
                            );
                            return;
                        }
                        Err(_) => {
                            log::warn!(
                                "Subscriber connect timed out for {} (from {}); skipping",
                                resolved, ep
                            );
                            return;
                        }
                    }

                    loop {
                        match socket.recv().await {
                            Ok(msg) => {
                                let bytes: Vec<u8> = msg
                                    .into_vec()
                                    .first()
                                    .cloned()
                                    .map(|frame| frame.to_vec())
                                    .unwrap_or_default();

                                match fusion_protobuf::decode(&bytes) {
                                    Some(data) => {
                                        if tx.send(data).is_err() {
                                            break;
                                        }
                                    }
                                    None => {
                                        log::warn!(
                                            "Subscriber [{}] failed to decode protobuf ({} bytes)",
                                            ep,
                                            bytes.len()
                                        );
                                    }
                                }
                            }
                            Err(e) => {
                                log::error!("Subscriber [{}] recv error: {}", ep, e);
                                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                            }
                        }
                    }
                });

                handles.push(task);
            }
        }

        *self.m_task_handles.lock().unwrap() = handles;
        Ok(())
    }
}

impl Drop for Subscriber {
    fn drop(&mut self) {
        for handle in self.m_task_handles.lock().unwrap().drain(..) {
            handle.abort();
        }
    }
}
