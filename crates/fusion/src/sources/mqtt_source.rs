use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::task::JoinHandle;

use fusion_registry::{sf, SettingsField};
use fusion_types::{FusedVehiclePose, StreamableData};
use serde_json::json;

use crate::encoders::json_encoder::JsonDecoder;
use crate::node::{ConsumerCallback, Node, NodeBase};

pub fn settings_schema() -> Vec<SettingsField> {
    vec![
        sf("host", "Host", "string", json!("192.168.1.200")),
        sf("port", "Port", "number", json!(1883)),
        sf("topic", "Topic", "string", json!("testTopic")),
        sf("clientId", "Client ID", "string", json!("FusionHubSubscriber")),
        sf("qos", "QoS", "number", json!(0)),
    ]
}

/// MQTT subscriber source node.
///
/// Subscribes to an MQTT topic and receives FusedPose, GlobalFusedPose,
/// FusedVehiclePose, and FusedVehiclePoseV2 data encoded as JSON.
pub struct MqttSource {
    pub base: NodeBase,
    m_host: String,
    m_port: u16,
    m_client_id: String,
    m_topic: String,
    m_qos: u8,
    m_done: Arc<AtomicBool>,
    m_worker_handle: Option<JoinHandle<()>>,
    m_packet_count: Arc<Mutex<u64>>,
}

impl MqttSource {
    pub fn new(name: impl Into<String>, config: &serde_json::Value) -> Self {
        let name = name.into();
        let host = config
            .get("host")
            .and_then(|v| v.as_str())
            .unwrap_or("192.168.1.200")
            .to_string();
        let port = config
            .get("port")
            .and_then(|v| v.as_u64())
            .unwrap_or(1883) as u16;
        let client_id = config
            .get("clientId")
            .and_then(|v| v.as_str())
            .unwrap_or("FusionHubSubscriber")
            .to_string();
        let topic = config
            .get("topic")
            .and_then(|v| v.as_str())
            .unwrap_or("testTopic")
            .to_string();
        let qos = config
            .get("qos")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u8;

        let mut base = NodeBase::new(&name);
        base.set_heartbeat_interval(Duration::from_secs(5));

        Self {
            base,
            m_host: host,
            m_port: port,
            m_client_id: client_id,
            m_topic: topic,
            m_qos: qos,
            m_done: Arc::new(AtomicBool::new(false)),
            m_worker_handle: None,
            m_packet_count: Arc::new(Mutex::new(0)),
        }
    }
}

impl Node for MqttSource {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn start(&mut self) -> anyhow::Result<()> {
        log::info!(
            "Starting MQTT source: {} ({}:{} topic={})",
            self.base.name(),
            self.m_host,
            self.m_port,
            self.m_topic
        );

        let host = self.m_host.clone();
        let port = self.m_port;
        let client_id = self.m_client_id.clone();
        let topic = self.m_topic.clone();
        let qos = self.m_qos;
        let done = self.m_done.clone();
        let consumers = self.base.consumers_arc();
        let enabled = self.base.enabled_arc();
        let packet_count = self.m_packet_count.clone();

        self.m_worker_handle = Some(tokio::spawn(async move {
            let mqtt_qos = match qos {
                0 => rumqttc::QoS::AtMostOnce,
                1 => rumqttc::QoS::AtLeastOnce,
                _ => rumqttc::QoS::ExactlyOnce,
            };

            let mut mqttoptions = rumqttc::MqttOptions::new(&client_id, &host, port);
            mqttoptions.set_keep_alive(Duration::from_secs(30));
            mqttoptions.set_clean_session(true);

            let (client, mut eventloop) = rumqttc::AsyncClient::new(mqttoptions, 256);

            match client.subscribe(&topic, mqtt_qos).await {
                Ok(_) => log::info!("Subscribed to MQTT topic: {}", topic),
                Err(e) => {
                    log::warn!("Failed to subscribe to MQTT topic: {}", e);
                    return;
                }
            }

            while !done.load(Ordering::Relaxed) {
                match eventloop.poll().await {
                    Ok(rumqttc::Event::Incoming(rumqttc::Packet::Publish(msg))) => {
                        *packet_count.lock().unwrap() += 1;

                        let payload = String::from_utf8_lossy(&msg.payload);

                        match JsonDecoder::decode(&payload) {
                            Ok(data) => {
                                // Preserve transmission time for pose data
                                let data = match data {
                                    StreamableData::FusedVehiclePoseV2(mut pose) => {
                                        pose.transmission_time = pose.timestamp;
                                        let pose_v1 = FusedVehiclePose::from(pose.clone());

                                        if enabled.load(Ordering::Relaxed) {
                                            let cbs = consumers.lock().unwrap();
                                            let v2_data = StreamableData::FusedVehiclePoseV2(pose);
                                            for cb in cbs.iter() {
                                                cb(v2_data.clone());
                                            }
                                            let v1_data =
                                                StreamableData::FusedVehiclePose(pose_v1);
                                            for cb in cbs.iter() {
                                                cb(v1_data.clone());
                                            }
                                        }
                                        continue;
                                    }
                                    StreamableData::FusedPose(mut pose) => {
                                        pose.transmission_time = pose.timestamp;
                                        StreamableData::FusedPose(pose)
                                    }
                                    StreamableData::GlobalFusedPose(mut pose) => {
                                        pose.transmission_time = pose.timestamp;
                                        StreamableData::GlobalFusedPose(pose)
                                    }
                                    other => other,
                                };

                                if enabled.load(Ordering::Relaxed) {
                                    let cbs = consumers.lock().unwrap();
                                    for cb in cbs.iter() {
                                        cb(data.clone());
                                    }
                                }
                            }
                            Err(e) => {
                                log::warn!("Failed to decode MQTT payload: {}", e);
                            }
                        }
                    }
                    Ok(_) => {}
                    Err(e) => {
                        log::warn!("MQTT connection error: {}", e);
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
            }

            let _ = client.disconnect().await;
            log::info!("MQTT subscriber disconnected");
        }));

        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        log::info!("Stopping MQTT source: {}", self.base.name());
        self.m_done.store(true, Ordering::Relaxed);

        if let Some(handle) = self.m_worker_handle.take() {
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

    fn set_on_output(&self, callback: ConsumerCallback) {
        self.base.add_consumer(callback);
    }
}
