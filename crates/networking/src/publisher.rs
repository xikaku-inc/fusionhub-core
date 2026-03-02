use std::sync::{Arc, Mutex};

use anyhow::Result;
use fusion_protobuf::ProtobufEncoder;
use fusion_types::StreamableData;
use tokio::sync::broadcast;
use zeromq::{PubSocket, Socket, SocketSend};

use crate::runtime::{
    is_inproc, register_data_channel, register_endpoint, resolve_endpoint_for_bind, runtime_handle,
};

enum PublisherInner {
    Broadcast {
        sender: broadcast::Sender<Arc<StreamableData>>,
    },
    Zmq {
        tx: tokio::sync::mpsc::Sender<Vec<u8>>,
        encoder: ProtobufEncoder,
        task_handle: Option<tokio::task::JoinHandle<()>>,
    },
}

pub struct Publisher {
    m_endpoint: String,
    m_resolved_endpoint: String,
    m_inner: Mutex<PublisherInner>,
}

impl Publisher {
    pub fn new(endpoint: impl Into<String>) -> Self {
        let endpoint = endpoint.into();

        if is_inproc(&endpoint) {
            let (sender, _) = broadcast::channel::<Arc<StreamableData>>(1000);
            register_data_channel(&endpoint, sender.clone());
            log::info!("Publisher registered broadcast channel for {}", endpoint);

            return Self {
                m_endpoint: endpoint.clone(),
                m_resolved_endpoint: endpoint,
                m_inner: Mutex::new(PublisherInner::Broadcast { sender }),
            };
        }

        let bind_endpoint = resolve_endpoint_for_bind(&endpoint);
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(1000);
        let (resolved_tx, resolved_rx) = std::sync::mpsc::channel::<String>();

        let handle = runtime_handle();
        let task_handle = handle.spawn(async move {
            let mut socket = PubSocket::new();
            match socket.bind(&bind_endpoint).await {
                Ok(ep) => {
                    let resolved = ep.to_string();
                    log::info!("Publisher bound to {}", resolved);
                    let _ = resolved_tx.send(resolved);
                }
                Err(e) => {
                    log::error!("Failed to bind publisher to {}: {}", bind_endpoint, e);
                    let _ = resolved_tx.send(bind_endpoint);
                    return;
                }
            }

            while let Some(data) = rx.recv().await {
                let msg = zeromq::ZmqMessage::from(data);
                if let Err(e) = socket.send(msg).await {
                    log::error!("Publisher send error: {}", e);
                }
            }
        });

        let resolved = resolved_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap_or_else(|_| {
                log::error!("Timed out waiting for publisher bind on '{}'", endpoint);
                endpoint.clone()
            });

        register_endpoint(&endpoint, &resolved);

        Self {
            m_endpoint: endpoint,
            m_resolved_endpoint: resolved,
            m_inner: Mutex::new(PublisherInner::Zmq {
                tx,
                encoder: ProtobufEncoder::new(),
                task_handle: Some(task_handle),
            }),
        }
    }

    pub fn from_config(config: &serde_json::Value) -> Self {
        let endpoint = config
            .get("endpoint")
            .and_then(|v| v.as_str())
            .unwrap_or("tcp://*:0")
            .to_owned();
        Self::new(endpoint)
    }

    pub fn endpoint(&self) -> &str {
        &self.m_resolved_endpoint
    }

    pub fn original_endpoint(&self) -> &str {
        &self.m_endpoint
    }

    pub fn shutdown(&self) {
        let mut inner = self.m_inner.lock().unwrap();
        if let PublisherInner::Zmq { task_handle, .. } = &mut *inner {
            if let Some(h) = task_handle.take() {
                h.abort();
            }
        }
    }

    pub fn publish(&self, data: &StreamableData) -> Result<()> {
        let inner = self.m_inner.lock().unwrap();
        match &*inner {
            PublisherInner::Broadcast { sender } => {
                let _ = sender.send(Arc::new(data.clone()));
                Ok(())
            }
            PublisherInner::Zmq { tx, encoder, .. } => {
                let bytes = encoder.encode(data);
                tx.try_send(bytes)
                    .map_err(|e| anyhow::anyhow!("Publisher channel send error: {}", e))
            }
        }
    }
}

impl Drop for Publisher {
    fn drop(&mut self) {
        self.shutdown();
    }
}
