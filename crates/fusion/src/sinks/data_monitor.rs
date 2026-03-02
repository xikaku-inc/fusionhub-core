use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicI64, Ordering};

use fusion_types::{ApiRequest, StreamableData};

use crate::node::{CommandConsumerCallback, Node, NodeBase};

/// Shared counters that can be safely read from the heartbeat thread.
#[derive(Clone)]
struct Counters {
    n_imu: Arc<AtomicI64>,
    n_gnss: Arc<AtomicI64>,
    n_optical: Arc<AtomicI64>,
    n_can: Arc<AtomicI64>,
    n_vehicle_speed: Arc<AtomicI64>,
    n_rtcm: Arc<AtomicI64>,
    gnss_quality: Arc<AtomicI64>,
}

impl Counters {
    fn new() -> Self {
        Self {
            n_imu: Arc::new(AtomicI64::new(0)),
            n_gnss: Arc::new(AtomicI64::new(0)),
            n_optical: Arc::new(AtomicI64::new(0)),
            n_can: Arc::new(AtomicI64::new(0)),
            n_vehicle_speed: Arc::new(AtomicI64::new(0)),
            n_rtcm: Arc::new(AtomicI64::new(0)),
            gnss_quality: Arc::new(AtomicI64::new(0)),
        }
    }

    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "nImu": self.n_imu.load(Ordering::Relaxed),
            "nGnss": self.n_gnss.load(Ordering::Relaxed),
            "nOptical": self.n_optical.load(Ordering::Relaxed),
            "nCan": self.n_can.load(Ordering::Relaxed),
            "gnssQuality": self.gnss_quality.load(Ordering::Relaxed),
            "nVehicleSpeed": self.n_vehicle_speed.load(Ordering::Relaxed),
            "nRtcmData": self.n_rtcm.load(Ordering::Relaxed),
        })
    }
}

/// Sink that monitors incoming data streams and periodically reports
/// message counts to the WebSocket server via heartbeat.
///
/// Mirrors C++ `Fusion::DataMonitor`:
/// - Counts messages by type (IMU, GNSS, Optical, CAN, VehicleSpeed, RTCM)
/// - Tracks GNSS quality
/// - Sends `ApiRequest("ws", ...)` with `inputStatus` every heartbeat (1s)
pub struct DataMonitor {
    pub base: NodeBase,
    m_on_command_output: Arc<Mutex<Option<CommandConsumerCallback>>>,
    m_counters: Counters,
}

impl DataMonitor {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            base: NodeBase::new(name),
            m_on_command_output: Arc::new(Mutex::new(None)),
            m_counters: Counters::new(),
        }
    }

    fn get_status(&self) -> serde_json::Value {
        self.m_counters.to_json()
    }
}

impl Node for DataMonitor {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn status(&self) -> serde_json::Value {
        self.m_counters.to_json()
    }

    fn start(&mut self) -> anyhow::Result<()> {
        log::info!("DataMonitor '{}' started", self.base.name());

        let cmd_cb = Arc::clone(&self.m_on_command_output);
        let counters = self.m_counters.clone();
        let name = self.base.name().to_owned();

        self.base.start_heartbeat(move || {
            let data = serde_json::json!({
                "data": counters.to_json(),
                "description": "inputStatus",
                "status": "OK",
            });

            let req = ApiRequest::new("ws", &name, data, "");
            let cb = cmd_cb.lock().unwrap();
            if let Some(ref callback) = *cb {
                callback(req);
            }
        });

        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        self.base.stop_heartbeat();
        log::info!(
            "DataMonitor '{}' stopping (imu={}, gnss={}, optical={}, can={}, vehicleSpeed={}, rtcm={})",
            self.base.name(),
            self.m_counters.n_imu.load(Ordering::Relaxed),
            self.m_counters.n_gnss.load(Ordering::Relaxed),
            self.m_counters.n_optical.load(Ordering::Relaxed),
            self.m_counters.n_can.load(Ordering::Relaxed),
            self.m_counters.n_vehicle_speed.load(Ordering::Relaxed),
            self.m_counters.n_rtcm.load(Ordering::Relaxed),
        );
        Ok(())
    }

    fn is_enabled(&self) -> bool {
        self.base.is_enabled()
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.base.set_enabled(enabled);
    }

    fn receive_data(&mut self, data: StreamableData) {
        match &data {
            StreamableData::Imu(_) => { self.m_counters.n_imu.fetch_add(1, Ordering::Relaxed); }
            StreamableData::Optical(_) => { self.m_counters.n_optical.fetch_add(1, Ordering::Relaxed); }
            StreamableData::Gnss(d) => {
                self.m_counters.gnss_quality.store(d.quality as i64, Ordering::Relaxed);
                self.m_counters.n_gnss.fetch_add(1, Ordering::Relaxed);
            }
            StreamableData::Can(_) => { self.m_counters.n_can.fetch_add(1, Ordering::Relaxed); }
            StreamableData::VehicleSpeed(_) => { self.m_counters.n_vehicle_speed.fetch_add(1, Ordering::Relaxed); }
            StreamableData::Rtcm(_) => { self.m_counters.n_rtcm.fetch_add(1, Ordering::Relaxed); }
            _ => {}
        }
    }

    fn set_on_command_output(&self, callback: CommandConsumerCallback) {
        *self.m_on_command_output.lock().unwrap() = Some(callback);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use fusion_types::{ImuData, GnssData, OpticalData};

    #[test]
    fn data_monitor_counts_messages() {
        let mut monitor = DataMonitor::new("monitor_test");

        monitor.receive_data(StreamableData::Imu(ImuData {
            sender_id: "imu0".into(),
            ..Default::default()
        }));
        monitor.receive_data(StreamableData::Imu(ImuData {
            sender_id: "imu0".into(),
            ..Default::default()
        }));
        monitor.receive_data(StreamableData::Optical(OpticalData {
            sender_id: "opt0".into(),
            ..Default::default()
        }));
        monitor.receive_data(StreamableData::Gnss(GnssData {
            sender_id: "gnss0".into(),
            quality: 4,
            ..Default::default()
        }));

        let status = monitor.get_status();
        assert_eq!(status["nImu"], 2);
        assert_eq!(status["nOptical"], 1);
        assert_eq!(status["nGnss"], 1);
        assert_eq!(status["gnssQuality"], 4);
        assert_eq!(status["nCan"], 0);
    }

    #[test]
    fn data_monitor_sends_ws_command() {
        let monitor = DataMonitor::new("monitor_test");

        let (tx, rx) = mpsc::channel();
        monitor.set_on_command_output(Box::new(move |req| {
            let _ = tx.send(req);
        }));

        // Simulate what the heartbeat does
        let status = monitor.get_status();
        let data = serde_json::json!({
            "data": status,
            "description": "inputStatus",
            "status": "OK",
        });
        let req = ApiRequest::new("ws", monitor.name(), data, "");
        let cb = monitor.m_on_command_output.lock().unwrap();
        if let Some(ref callback) = *cb {
            callback(req);
        }
        drop(cb);

        let received = rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap();
        assert_eq!(received.command, "ws");
        assert_eq!(received.data["description"], "inputStatus");
        assert_eq!(received.data["data"]["nImu"], 0);
    }
}
