use std::sync::Arc;

use fusion_types::StreamableData;
use tokio::sync::broadcast;

use crate::encoders::json_encoder::JsonEncoder;
use crate::node::{Node, NodeBase};

/// WebSocket server sink that broadcasts StreamableData as JSON to all
/// connected browser clients. Uses tokio-tungstenite for async WebSocket
/// handling.
pub struct WebsocketSink {
    pub base: NodeBase,
    m_port: u16,
    m_broadcast_tx: broadcast::Sender<String>,
    m_accept_handle: Option<tokio::task::JoinHandle<()>>,
    m_client_count: Arc<std::sync::atomic::AtomicUsize>,
    m_message_count: Arc<std::sync::atomic::AtomicU64>,
}

impl WebsocketSink {
    pub fn new(name: impl Into<String>, port: u16) -> Self {
        let (tx, _) = broadcast::channel(512);
        Self {
            base: NodeBase::new(name),
            m_port: port,
            m_broadcast_tx: tx,
            m_accept_handle: None,
            m_client_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            m_message_count: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }

    pub fn on_data(&self, data: StreamableData) {
        if !self.base.is_enabled() {
            return;
        }

        match JsonEncoder::encode(&data) {
            Ok(json) => {
                // Only send if there are active receivers
                if self.m_broadcast_tx.receiver_count() > 0 {
                    let _ = self.m_broadcast_tx.send(json);
                }
            }
            Err(e) => {
                log::warn!("[{}] JSON encode failed: {}", self.base.name(), e);
            }
        }

        self.m_message_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        self.base.notify_consumers(data);
    }

    pub fn port(&self) -> u16 {
        self.m_port
    }

    pub fn client_count(&self) -> usize {
        self.m_client_count
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn message_count(&self) -> u64 {
        self.m_message_count
            .load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl Node for WebsocketSink {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn start(&mut self) -> anyhow::Result<()> {
        log::info!(
            "WebsocketSink '{}' starting on port {}",
            self.base.name(),
            self.m_port
        );

        let port = self.m_port;
        let broadcast_tx = self.m_broadcast_tx.clone();
        let client_count = self.m_client_count.clone();
        let name = self.base.name().to_owned();

        self.m_accept_handle = Some(tokio::spawn(async move {
            let bind_addr = format!("0.0.0.0:{}", port);
            let listener = match tokio::net::TcpListener::bind(&bind_addr).await {
                Ok(l) => l,
                Err(e) => {
                    log::error!("[{}] Failed to bind on {}: {}", name, bind_addr, e);
                    return;
                }
            };

            log::info!("[{}] WebSocket server listening on {}", name, bind_addr);

            loop {
                let (stream, peer) = match listener.accept().await {
                    Ok(v) => v,
                    Err(e) => {
                        log::error!("[{}] Accept failed: {}", name, e);
                        continue;
                    }
                };

                log::info!("[{}] New WebSocket client from {}", name, peer);
                client_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                let mut broadcast_rx = broadcast_tx.subscribe();
                let client_count_inner = client_count.clone();
                let name_inner = name.clone();

                tokio::spawn(async move {
                    use futures_util::{SinkExt, StreamExt};
                    use tokio_tungstenite::tungstenite::Message;

                    let ws_stream = match tokio_tungstenite::accept_async(stream).await {
                        Ok(ws) => ws,
                        Err(e) => {
                            log::error!(
                                "[{}] WebSocket handshake failed: {}",
                                name_inner,
                                e
                            );
                            client_count_inner
                                .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                            return;
                        }
                    };

                    let (mut ws_sender, mut ws_receiver) = ws_stream.split();

                    loop {
                        tokio::select! {
                            msg = broadcast_rx.recv() => {
                                match msg {
                                    Ok(text) => {
                                        if ws_sender
                                            .send(Message::Text(text.into()))
                                            .await
                                            .is_err()
                                        {
                                            break;
                                        }
                                    }
                                    Err(broadcast::error::RecvError::Lagged(n)) => {
                                        log::warn!(
                                            "[{}] Client {} lagged {} messages",
                                            name_inner,
                                            peer,
                                            n
                                        );
                                    }
                                    Err(_) => break,
                                }
                            }
                            msg = ws_receiver.next() => {
                                match msg {
                                    Some(Ok(Message::Close(_))) | None => break,
                                    Some(Err(_)) => break,
                                    _ => {}
                                }
                            }
                        }
                    }

                    client_count_inner.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                    log::info!("[{}] Client {} disconnected", name_inner, peer);
                });
            }
        }));

        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        log::info!(
            "WebsocketSink '{}' stopping (sent {} messages to {} clients)",
            self.base.name(),
            self.message_count(),
            self.client_count()
        );

        if let Some(handle) = self.m_accept_handle.take() {
            handle.abort();
        }
        self.base.stop_heartbeat();
        Ok(())
    }

    fn is_enabled(&self) -> bool {
        self.base.is_enabled()
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.base.set_enabled(enabled);
    }

    fn receive_data(&mut self, data: StreamableData) {
        self.on_data(data);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn websocket_sink_creation() {
        let sink = WebsocketSink::new("ws_test", 8080);
        assert_eq!(sink.name(), "ws_test");
        assert_eq!(sink.port(), 8080);
        assert_eq!(sink.client_count(), 0);
        assert_eq!(sink.message_count(), 0);
    }
}
