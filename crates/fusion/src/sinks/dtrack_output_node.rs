use std::sync::{Arc, Mutex};

use fusion_registry::{sf, SettingsField};
use fusion_types::{FusedPose, OpticalData, StreamableData};
use serde_json::json;

use crate::node::{Node, NodeBase};

pub fn settings_schema() -> Vec<SettingsField> {
    vec![
        sf("host", "Host", "string", json!("127.0.0.1")),
        sf("port", "Port", "number", json!(5001)),
        sf("bodyId", "Body ID", "number", json!(0)),
        sf("qualityThreshold", "Quality Threshold", "number", json!(0.0)),
    ]
}

/// DTrack output format configuration.
#[derive(Clone, Debug)]
pub struct DTrackConfig {
    pub host: String,
    pub port: u16,
    pub body_id: i32,
    pub quality_threshold: f64,
}

impl Default for DTrackConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".into(),
            port: 5000,
            body_id: 0,
            quality_threshold: 0.0,
        }
    }
}

/// Sink that formats FusedPose or OpticalData as DTrack-format UDP packets
/// and sends them to a DTrack receiver.
///
/// DTrack body line format:
/// `6d <n_bodies> [<id> <quality> [<pos_x> <pos_y> <pos_z>] [<rot_00> ... <rot_22>]]`
pub struct DTrackOutputNode {
    pub base: NodeBase,
    m_config: DTrackConfig,
    m_socket: Arc<Mutex<Option<std::net::UdpSocket>>>,
    m_frame_counter: Arc<Mutex<u64>>,
}

impl DTrackOutputNode {
    pub fn new(name: impl Into<String>, config: &serde_json::Value) -> Self {
        let dtrack_config = DTrackConfig {
            host: config
                .get("host")
                .and_then(|v| v.as_str())
                .unwrap_or("127.0.0.1")
                .to_owned(),
            port: config
                .get("port")
                .and_then(|v| v.as_u64())
                .unwrap_or(5000) as u16,
            body_id: config
                .get("bodyId")
                .and_then(|v| v.as_i64())
                .unwrap_or(0) as i32,
            quality_threshold: config
                .get("qualityThreshold")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0),
        };

        Self {
            base: NodeBase::new(name),
            m_config: dtrack_config,
            m_socket: Arc::new(Mutex::new(None)),
            m_frame_counter: Arc::new(Mutex::new(0)),
        }
    }

    pub fn on_data(&self, data: StreamableData) {
        if !self.base.is_enabled() {
            return;
        }

        let packet = match &data {
            StreamableData::FusedPose(pose) => Some(self.format_fused_pose(pose)),
            StreamableData::Optical(optical) => Some(self.format_optical(optical)),
            _ => None,
        };

        if let Some(packet) = packet {
            self.send_packet(&packet);
        }

        self.base.notify_consumers(data);
    }

    fn format_fused_pose(&self, pose: &FusedPose) -> String {
        let rot = pose.orientation.to_rotation_matrix();
        let m = rot.matrix();

        let mut counter = self.m_frame_counter.lock().unwrap();
        *counter += 1;
        let frame = *counter;

        format!(
            "fr {}\n6d 1 [{} 1.0 [{:.6} {:.6} {:.6}] \
             [{:.6} {:.6} {:.6} {:.6} {:.6} {:.6} {:.6} {:.6} {:.6}]]\n",
            frame,
            self.m_config.body_id,
            pose.position.x * 1000.0,
            pose.position.y * 1000.0,
            pose.position.z * 1000.0,
            m[(0, 0)], m[(0, 1)], m[(0, 2)],
            m[(1, 0)], m[(1, 1)], m[(1, 2)],
            m[(2, 0)], m[(2, 1)], m[(2, 2)],
        )
    }

    fn format_optical(&self, optical: &OpticalData) -> String {
        let rot = optical.orientation.to_rotation_matrix();
        let m = rot.matrix();

        let mut counter = self.m_frame_counter.lock().unwrap();
        *counter += 1;
        let frame = *counter;

        let quality = if optical.quality >= self.m_config.quality_threshold {
            optical.quality
        } else {
            -1.0
        };

        format!(
            "fr {}\n6d 1 [{} {:.3} [{:.6} {:.6} {:.6}] \
             [{:.6} {:.6} {:.6} {:.6} {:.6} {:.6} {:.6} {:.6} {:.6}]]\n",
            frame,
            self.m_config.body_id,
            quality,
            optical.position.x * 1000.0,
            optical.position.y * 1000.0,
            optical.position.z * 1000.0,
            m[(0, 0)], m[(0, 1)], m[(0, 2)],
            m[(1, 0)], m[(1, 1)], m[(1, 2)],
            m[(2, 0)], m[(2, 1)], m[(2, 2)],
        )
    }

    fn send_packet(&self, packet: &str) {
        let socket = self.m_socket.lock().unwrap();
        if let Some(ref sock) = *socket {
            let addr = format!("{}:{}", self.m_config.host, self.m_config.port);
            if let Err(e) = sock.send_to(packet.as_bytes(), &addr) {
                log::warn!("[{}] DTrack UDP send failed: {}", self.base.name(), e);
            }
        }
    }

    pub fn frame_count(&self) -> u64 {
        *self.m_frame_counter.lock().unwrap()
    }

    pub fn config(&self) -> &DTrackConfig {
        &self.m_config
    }
}

impl Node for DTrackOutputNode {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn start(&mut self) -> anyhow::Result<()> {
        log::info!(
            "DTrackOutputNode '{}' starting (target={}:{}, bodyId={})",
            self.base.name(),
            self.m_config.host,
            self.m_config.port,
            self.m_config.body_id
        );

        match std::net::UdpSocket::bind("0.0.0.0:0") {
            Ok(sock) => {
                *self.m_socket.lock().unwrap() = Some(sock);
            }
            Err(e) => {
                log::error!(
                    "[{}] Failed to create UDP socket: {}",
                    self.base.name(),
                    e
                );
            }
        }

        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        log::info!(
            "DTrackOutputNode '{}' stopping (sent {} frames)",
            self.base.name(),
            self.frame_count()
        );
        *self.m_socket.lock().unwrap() = None;
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
    use fusion_types::{Quatd, Vec3d};

    #[test]
    fn dtrack_output_node_creation() {
        let config = serde_json::json!({
            "host": "192.168.1.100",
            "port": 5001,
            "bodyId": 3,
        });
        let node = DTrackOutputNode::new("dtrack_test", &config);
        assert_eq!(node.name(), "dtrack_test");
        assert_eq!(node.config().host, "192.168.1.100");
        assert_eq!(node.config().port, 5001);
        assert_eq!(node.config().body_id, 3);
    }

    #[test]
    fn format_fused_pose_packet() {
        let config = serde_json::json!({});
        let node = DTrackOutputNode::new("dtrack_test", &config);

        let pose = FusedPose {
            position: Vec3d::new(1.0, 2.0, 3.0),
            orientation: Quatd::identity(),
            ..Default::default()
        };

        let packet = node.format_fused_pose(&pose);
        assert!(packet.contains("fr 1"));
        assert!(packet.contains("6d 1"));
        assert!(packet.contains("1000.000000"));
        assert!(packet.contains("2000.000000"));
        assert!(packet.contains("3000.000000"));
    }

    #[test]
    fn format_optical_low_quality() {
        let config = serde_json::json!({ "qualityThreshold": 0.5 });
        let node = DTrackOutputNode::new("dtrack_test", &config);

        let optical = OpticalData {
            quality: 0.1,
            position: Vec3d::new(0.5, 0.5, 0.5),
            ..Default::default()
        };

        let packet = node.format_optical(&optical);
        assert!(packet.contains("-1.000"));
    }
}
