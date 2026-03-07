use std::sync::{Arc, Mutex};
use std::time::Duration;

use fusion_registry::{sf, SettingsField};
use fusion_types::StreamableData;
use rumqttc::{AsyncClient, MqttOptions, QoS};
use serde_json::json;

use crate::encoders::json_encoder::JsonEncoder;
use crate::node::{Node, NodeBase};

pub fn settings_schema() -> Vec<SettingsField> {
    vec![
        sf("host", "Host", "string", json!("localhost")),
        sf("port", "Port", "number", json!(1883)),
        sf("topic", "Topic", "string", json!("fusionhub/output")),
    ]
}

/// Encoding format for MQTT messages.
#[derive(Clone, Debug, PartialEq)]
pub enum MqttEncoding {
    Json,
    Protobuf,
}

/// MQTT publisher sink. Publishes StreamableData to a configurable MQTT topic
/// as JSON or protobuf-encoded messages.
pub struct MqttSink {
    pub base: NodeBase,
    m_host: String,
    m_port: u16,
    m_topic: String,
    m_encoding: MqttEncoding,
    m_client: Arc<Mutex<Option<AsyncClient>>>,
    m_event_loop_handle: Option<tokio::task::JoinHandle<()>>,
    m_count: Arc<Mutex<u64>>,
}

impl MqttSink {
    pub fn new(name: impl Into<String>, host: &str, port: u16, topic: &str) -> Self {
        Self {
            base: NodeBase::new(name),
            m_host: host.to_owned(),
            m_port: port,
            m_topic: topic.to_owned(),
            m_encoding: MqttEncoding::Json,
            m_client: Arc::new(Mutex::new(None)),
            m_event_loop_handle: None,
            m_count: Arc::new(Mutex::new(0)),
        }
    }

    pub fn with_encoding(mut self, encoding: MqttEncoding) -> Self {
        self.m_encoding = encoding;
        self
    }

    pub fn set_topic(&mut self, topic: &str) {
        self.m_topic = topic.to_owned();
    }

    pub fn on_data(&self, data: StreamableData) {
        if !self.base.is_enabled() {
            return;
        }

        let payload = match self.m_encoding {
            MqttEncoding::Json => match JsonEncoder::encode(&data) {
                Ok(json) => json.into_bytes(),
                Err(e) => {
                    log::warn!("[{}] JSON encode failed: {}", self.base.name(), e);
                    return;
                }
            },
            MqttEncoding::Protobuf => {
                // Protobuf encoding would use fusion_protobuf crate.
                // For now, fall back to JSON.
                match JsonEncoder::encode(&data) {
                    Ok(json) => json.into_bytes(),
                    Err(e) => {
                        log::warn!("[{}] Protobuf encode fallback failed: {}", self.base.name(), e);
                        return;
                    }
                }
            }
        };

        let client = self.m_client.lock().unwrap();
        if let Some(ref c) = *client {
            let topic = self.m_topic.clone();
            let c = c.clone();
            tokio::spawn(async move {
                if let Err(e) = c.publish(topic, QoS::AtLeastOnce, false, payload).await {
                    log::warn!("MQTT publish failed: {}", e);
                }
            });
        }

        let mut count = self.m_count.lock().unwrap();
        *count += 1;
        drop(count);

        self.base.notify_consumers(data);
    }

    pub fn publish_count(&self) -> u64 {
        *self.m_count.lock().unwrap()
    }
}

impl Node for MqttSink {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn status(&self) -> serde_json::Value {
        serde_json::json!({
            "publishCount": self.publish_count(),
            "topic": self.m_topic,
        })
    }

    fn start(&mut self) -> anyhow::Result<()> {
        log::info!(
            "MqttSink '{}' connecting to {}:{} (topic='{}')",
            self.base.name(),
            self.m_host,
            self.m_port,
            self.m_topic
        );

        let client_id = format!("fusionhub-sink-{}", self.base.name());
        let mut mqtt_options = MqttOptions::new(client_id, &self.m_host, self.m_port);
        mqtt_options.set_keep_alive(Duration::from_secs(30));

        let (client, mut event_loop) = AsyncClient::new(mqtt_options, 256);
        *self.m_client.lock().unwrap() = Some(client);

        let name = self.base.name().to_owned();
        self.m_event_loop_handle = Some(tokio::spawn(async move {
            loop {
                match event_loop.poll().await {
                    Ok(_) => {}
                    Err(e) => {
                        log::warn!("[{}] MQTT event loop error: {}", name, e);
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    }
                }
            }
        }));

        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        log::info!(
            "MqttSink '{}' stopping (published {} messages)",
            self.base.name(),
            self.publish_count()
        );

        *self.m_client.lock().unwrap() = None;

        if let Some(handle) = self.m_event_loop_handle.take() {
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
    fn mqtt_sink_creation() {
        let sink = MqttSink::new("mqtt_test", "localhost", 1883, "test/topic");
        assert_eq!(sink.name(), "mqtt_test");
        assert_eq!(sink.publish_count(), 0);
    }

    #[test]
    fn mqtt_sink_encoding_selection() {
        let sink = MqttSink::new("mqtt_test", "localhost", 1883, "test/topic")
            .with_encoding(MqttEncoding::Protobuf);
        assert_eq!(sink.m_encoding, MqttEncoding::Protobuf);
    }
}
