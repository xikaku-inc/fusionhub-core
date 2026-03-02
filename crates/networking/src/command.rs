use std::sync::{Arc, Mutex};

use anyhow::Result;
use fusion_types::ApiRequest;
use tokio::sync::broadcast;
use zeromq::{PubSocket, Socket, SocketRecv, SocketSend, SubSocket};

use crate::runtime::{
    get_command_channel, is_inproc, register_command_channel, register_endpoint,
    resolve_endpoint_for_bind, resolve_endpoint_for_connect, runtime_handle,
};

enum CommandPublisherInner {
    Broadcast {
        sender: broadcast::Sender<ApiRequest>,
    },
    Zmq {
        tx: tokio::sync::mpsc::Sender<Vec<u8>>,
        task_handle: Option<tokio::task::JoinHandle<()>>,
    },
}

#[derive(Clone)]
pub struct CommandPublisher {
    m_endpoint: String,
    m_resolved_endpoint: String,
    m_inner: Arc<Mutex<CommandPublisherInner>>,
}

impl CommandPublisher {
    pub fn new(endpoint: impl Into<String>) -> Self {
        let endpoint = endpoint.into();

        if is_inproc(&endpoint) {
            let (sender, _) = broadcast::channel::<ApiRequest>(256);
            register_command_channel(&endpoint, sender.clone());
            log::info!("CommandPublisher registered broadcast channel for {}", endpoint);

            return Self {
                m_endpoint: endpoint.clone(),
                m_resolved_endpoint: endpoint,
                m_inner: Arc::new(Mutex::new(CommandPublisherInner::Broadcast { sender })),
            };
        }

        let bind_endpoint = resolve_endpoint_for_bind(&endpoint);
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(256);
        let (resolved_tx, resolved_rx) = std::sync::mpsc::channel::<String>();

        let handle = runtime_handle();
        let task_handle = handle.spawn(async move {
            let mut socket = PubSocket::new();
            match socket.bind(&bind_endpoint).await {
                Ok(ep) => {
                    let resolved = ep.to_string();
                    log::info!("CommandPublisher bound to {}", resolved);
                    let _ = resolved_tx.send(resolved);
                }
                Err(e) => {
                    log::error!("CommandPublisher bind error on {}: {}", bind_endpoint, e);
                    let _ = resolved_tx.send(bind_endpoint);
                    return;
                }
            }

            while let Some(data) = rx.recv().await {
                let msg = zeromq::ZmqMessage::from(data);
                if let Err(e) = socket.send(msg).await {
                    log::error!("CommandPublisher send error: {}", e);
                }
            }
        });

        let resolved = resolved_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap_or_else(|_| {
                log::error!("Timed out waiting for CommandPublisher bind");
                endpoint.clone()
            });

        register_endpoint(&endpoint, &resolved);

        Self {
            m_endpoint: endpoint,
            m_resolved_endpoint: resolved,
            m_inner: Arc::new(Mutex::new(CommandPublisherInner::Zmq {
                tx,
                task_handle: Some(task_handle),
            })),
        }
    }

    pub fn endpoint(&self) -> &str {
        &self.m_resolved_endpoint
    }

    pub fn send(&self, request: &ApiRequest) -> Result<()> {
        let inner = self.m_inner.lock().unwrap();
        match &*inner {
            CommandPublisherInner::Broadcast { sender } => {
                let _ = sender.send(request.clone());
                Ok(())
            }
            CommandPublisherInner::Zmq { tx, .. } => {
                let json = serde_json::to_string(request)?;
                tx.try_send(json.into_bytes())
                    .map_err(|e| anyhow::anyhow!("CommandPublisher channel error: {}", e))
            }
        }
    }

    pub fn publish(&self, request: &ApiRequest) -> Result<()> {
        self.send(request)
    }

    pub fn shutdown(&self) {
        let mut inner = self.m_inner.lock().unwrap();
        if let CommandPublisherInner::Zmq { task_handle, .. } = &mut *inner {
            if let Some(h) = task_handle.take() {
                h.abort();
            }
        }
    }
}

impl Drop for CommandPublisher {
    fn drop(&mut self) {
        if Arc::strong_count(&self.m_inner) == 1 {
            self.shutdown();
        }
    }
}

/// Command subscriber — receives ApiRequests from one or more endpoints.
///
/// Inproc endpoints use broadcast channels directly. TCP endpoints use ZMQ SubSocket
/// with JSON deserialization. Both paths feed into the same callback thread.
pub struct CommandSubscriber {
    m_endpoints: Vec<String>,
    m_task_handles: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
}

impl CommandSubscriber {
    pub fn new<F>(callback: F, endpoints: Vec<String>) -> Self
    where
        F: Fn(&ApiRequest) + Send + 'static,
    {
        let task_handles = Arc::new(Mutex::new(Vec::new()));

        let (tx, rx) = std::sync::mpsc::channel::<ApiRequest>();

        std::thread::Builder::new()
            .name("cmd-sub-cb".into())
            .spawn(move || {
                while let Ok(req) = rx.recv() {
                    callback(&req);
                }
            })
            .expect("Failed to spawn command subscriber callback thread");

        let handle = runtime_handle();
        let mut handles = Vec::new();

        for ep in endpoints.iter().cloned() {
            let tx = tx.clone();

            if is_inproc(&ep) {
                let sender = match get_command_channel(&ep) {
                    Some(s) => s,
                    None => {
                        log::warn!("CommandSubscriber: no broadcast channel for '{}'; skipping", ep);
                        continue;
                    }
                };
                let mut rx_bc = sender.subscribe();
                let ep_name = ep.clone();

                let task = handle.spawn(async move {
                    loop {
                        match rx_bc.recv().await {
                            Ok(req) => {
                                if tx.send(req).is_err() {
                                    break;
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(n)) => {
                                log::warn!(
                                    "CommandSubscriber [{}] lagged, skipped {} messages",
                                    ep_name, n
                                );
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
                        log::error!("CommandSubscriber [{}] subscribe error: {}", ep, e);
                        return;
                    }

                    match tokio::time::timeout(
                        std::time::Duration::from_secs(3),
                        socket.connect(&resolved),
                    )
                    .await
                    {
                        Ok(Ok(())) => {
                            log::info!(
                                "CommandSubscriber connected to {} (from {})",
                                resolved, ep
                            );
                        }
                        Ok(Err(e)) => {
                            log::warn!(
                                "CommandSubscriber failed to connect to {} (from {}): {}",
                                resolved, ep, e
                            );
                            return;
                        }
                        Err(_) => {
                            log::warn!(
                                "CommandSubscriber connect timed out for {} (from {}); skipping",
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

                                let json_str = match std::str::from_utf8(&bytes) {
                                    Ok(s) => s,
                                    Err(_) => continue,
                                };

                                match serde_json::from_str::<ApiRequest>(json_str) {
                                    Ok(req) => {
                                        if tx.send(req).is_err() {
                                            break;
                                        }
                                    }
                                    Err(e) => {
                                        log::warn!(
                                            "CommandSubscriber [{}] deserialize error: {}",
                                            ep, e
                                        );
                                    }
                                }
                            }
                            Err(e) => {
                                log::error!("CommandSubscriber [{}] recv error: {}", ep, e);
                                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                            }
                        }
                    }
                });

                handles.push(task);
            }
        }

        *task_handles.lock().unwrap() = handles;

        Self {
            m_endpoints: endpoints,
            m_task_handles: task_handles,
        }
    }

    pub fn endpoints(&self) -> &[String] {
        &self.m_endpoints
    }
}

impl Drop for CommandSubscriber {
    fn drop(&mut self) {
        for handle in self.m_task_handles.lock().unwrap().drain(..) {
            handle.abort();
        }
    }
}
