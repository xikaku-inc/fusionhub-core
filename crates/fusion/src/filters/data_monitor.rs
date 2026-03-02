use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fusion_types::{ApiRequest, GnssData, StreamableData};

use crate::node::{ConsumerCallback, Node};

/// Per-source data flow statistics.
#[derive(Clone, Debug)]
pub struct SourceStats {
    pub sender_id: String,
    pub data_type: String,
    pub sample_count: u64,
    pub last_sample_time: Instant,
    pub first_sample_time: Option<Instant>,
    pub measured_rate_hz: f64,
    pub min_interval_ms: f64,
    pub max_interval_ms: f64,
    pub last_interval_ms: f64,
}

impl SourceStats {
    fn new(sender_id: &str, data_type: &str) -> Self {
        Self {
            sender_id: sender_id.to_owned(),
            data_type: data_type.to_owned(),
            sample_count: 0,
            last_sample_time: Instant::now(),
            first_sample_time: None,
            measured_rate_hz: 0.0,
            min_interval_ms: f64::MAX,
            max_interval_ms: 0.0,
            last_interval_ms: 0.0,
        }
    }

    fn record_sample(&mut self) {
        let now = Instant::now();
        if self.sample_count > 0 {
            let interval = now.duration_since(self.last_sample_time);
            let interval_ms = interval.as_secs_f64() * 1000.0;
            self.last_interval_ms = interval_ms;
            if interval_ms < self.min_interval_ms {
                self.min_interval_ms = interval_ms;
            }
            if interval_ms > self.max_interval_ms {
                self.max_interval_ms = interval_ms;
            }
        } else {
            self.first_sample_time = Some(now);
        }
        self.last_sample_time = now;
        self.sample_count += 1;

        // Compute running rate
        if let Some(first) = self.first_sample_time {
            let elapsed = now.duration_since(first).as_secs_f64();
            if elapsed > 0.0 && self.sample_count > 1 {
                self.measured_rate_hz = (self.sample_count - 1) as f64 / elapsed;
            }
        }
    }

    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "senderId": self.sender_id,
            "dataType": self.data_type,
            "sampleCount": self.sample_count,
            "measuredRateHz": (self.measured_rate_hz * 10.0).round() / 10.0,
            "lastIntervalMs": (self.last_interval_ms * 100.0).round() / 100.0,
            "minIntervalMs": if self.min_interval_ms < f64::MAX {
                (self.min_interval_ms * 100.0).round() / 100.0
            } else {
                0.0
            },
            "maxIntervalMs": (self.max_interval_ms * 100.0).round() / 100.0,
        })
    }
}

/// GNSS quality snapshot.
#[derive(Clone, Debug)]
pub struct GnssQuality {
    pub quality: i32,
    pub n_sat: i32,
    pub hdop: f64,
    pub horizontal_accuracy: f64,
    pub vertical_accuracy: f64,
    pub diff_age: f64,
}

impl Default for GnssQuality {
    fn default() -> Self {
        Self {
            quality: 0,
            n_sat: 0,
            hdop: 99.9,
            horizontal_accuracy: 0.0,
            vertical_accuracy: 0.0,
            diff_age: 0.0,
        }
    }
}

impl GnssQuality {
    fn from_gnss_data(data: &GnssData) -> Self {
        Self {
            quality: data.quality,
            n_sat: data.n_sat,
            hdop: data.hdop,
            horizontal_accuracy: data.horizontal_accuracy,
            vertical_accuracy: data.vertical_accuracy,
            diff_age: data.diff_age,
        }
    }

    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "quality": self.quality,
            "nSat": self.n_sat,
            "hdop": self.hdop,
            "horizontalAccuracy": self.horizontal_accuracy,
            "verticalAccuracy": self.vertical_accuracy,
            "diffAge": self.diff_age,
        })
    }
}

/// Data monitor that tracks flow rates and GNSS quality for all data sources.
///
/// Receives all StreamableData, maintains per-source statistics, and publishes
/// status as JSON via the command channel when requested.
pub struct DataMonitor {
    m_name: String,
    m_enabled: bool,
    m_stats: HashMap<String, SourceStats>,
    m_gnss_quality: HashMap<String, GnssQuality>,
    m_report_interval: Duration,
    m_last_report_time: Instant,
    m_on_output: Arc<Mutex<Option<ConsumerCallback>>>,
}

impl DataMonitor {
    pub fn new(config: serde_json::Value) -> Self {
        let report_interval_ms = config
            .get("reportIntervalMs")
            .and_then(|v| v.as_u64())
            .unwrap_or(1000);

        Self {
            m_name: config
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("DataMonitor")
                .to_owned(),
            m_enabled: true,
            m_stats: HashMap::new(),
            m_gnss_quality: HashMap::new(),
            m_report_interval: Duration::from_millis(report_interval_ms),
            m_last_report_time: Instant::now(),
            m_on_output: Arc::new(Mutex::new(None)),
        }
    }

    pub fn name(&self) -> &str {
        &self.m_name
    }

    pub fn stats(&self) -> &HashMap<String, SourceStats> {
        &self.m_stats
    }

    pub fn gnss_quality(&self) -> &HashMap<String, GnssQuality> {
        &self.m_gnss_quality
    }

    pub fn set_on_output(&self, callback: ConsumerCallback) {
        let mut on_output = self.m_on_output.lock().unwrap();
        *on_output = Some(callback);
    }

    pub fn process_data(&mut self, data: StreamableData) {
        let (sender_id, data_type) = match &data {
            StreamableData::Imu(d) => (d.sender_id.clone(), "Imu"),
            StreamableData::Gnss(d) => {
                self.m_gnss_quality
                    .insert(d.sender_id.clone(), GnssQuality::from_gnss_data(d));
                (d.sender_id.clone(), "Gnss")
            }
            StreamableData::Optical(d) => (d.sender_id.clone(), "Optical"),
            StreamableData::FusedPose(d) => (d.sender_id.clone(), "FusedPose"),
            StreamableData::FusedVehiclePose(d) => (d.sender_id.clone(), "FusedVehiclePose"),
            StreamableData::FusedVehiclePoseV2(d) => (d.sender_id.clone(), "FusedVehiclePoseV2"),
            StreamableData::GlobalFusedPose(d) => (d.sender_id.clone(), "GlobalFusedPose"),
            StreamableData::FusionStateInt(d) => (d.sender_id.clone(), "FusionStateInt"),
            StreamableData::Rtcm(d) => (d.sender_id.clone(), "Rtcm"),
            StreamableData::Can(d) => (d.sender_id.clone(), "Can"),
            StreamableData::VehicleState(d) => (d.sender_id.clone(), "VehicleState"),
            StreamableData::VehicleSpeed(d) => (d.sender_id.clone(), "VehicleSpeed"),
            StreamableData::VelocityMeter(d) => (d.sender_id.clone(), "VelocityMeter"),
            StreamableData::Timestamp(_) => return,
        };

        let key = format!("{}:{}", sender_id, data_type);
        let stats = self
            .m_stats
            .entry(key)
            .or_insert_with(|| SourceStats::new(&sender_id, data_type));
        stats.record_sample();

        // Check if it's time for a periodic report
        if self.m_last_report_time.elapsed() >= self.m_report_interval {
            self.emit_status_report();
            self.m_last_report_time = Instant::now();
        }
    }

    pub fn process_command(&mut self, cmd: &ApiRequest) {
        match cmd.command.as_str() {
            "getStatus" | "getMonitorStatus" => {
                self.emit_status_report();
            }
            "getGnssQuality" => {
                self.emit_gnss_quality_report();
            }
            "reset" => {
                self.m_stats.clear();
                self.m_gnss_quality.clear();
                log::info!("{}: monitor reset", self.m_name);
            }
            "setReportInterval" => {
                if let Some(ms) = cmd.get_param("intervalMs").and_then(|v| v.as_u64()) {
                    self.m_report_interval = Duration::from_millis(ms);
                }
            }
            _ => {
                log::trace!("{}: unhandled command '{}'", self.m_name, cmd.command);
            }
        }
    }

    /// Generate a status report as JSON.
    pub fn status_json(&self) -> serde_json::Value {
        let sources: Vec<serde_json::Value> =
            self.m_stats.values().map(|s| s.to_json()).collect();

        let gnss: HashMap<&str, serde_json::Value> = self
            .m_gnss_quality
            .iter()
            .map(|(k, v)| (k.as_str(), v.to_json()))
            .collect();

        serde_json::json!({
            "sources": sources,
            "gnssQuality": gnss,
            "totalSources": self.m_stats.len(),
        })
    }

    fn emit_status_report(&self) {
        let report = self.status_json();
        log::debug!("{}: status report: {}", self.m_name, report);

        let on_output = self.m_on_output.lock().unwrap();
        if let Some(ref callback) = *on_output {
            let ts = fusion_types::Timestamp::current();
            callback(StreamableData::Timestamp(ts));
        }
    }

    fn emit_gnss_quality_report(&self) {
        let gnss_report: HashMap<&str, serde_json::Value> = self
            .m_gnss_quality
            .iter()
            .map(|(k, v)| (k.as_str(), v.to_json()))
            .collect();

        let report = serde_json::json!({
            "gnssQuality": gnss_report,
        });

        log::debug!("{}: GNSS quality report: {}", self.m_name, report);

        let on_output = self.m_on_output.lock().unwrap();
        if let Some(ref callback) = *on_output {
            let ts = fusion_types::Timestamp::current();
            callback(StreamableData::Timestamp(ts));
        }
    }
}

impl Node for DataMonitor {
    fn name(&self) -> &str {
        &self.m_name
    }

    fn start(&mut self) -> anyhow::Result<()> {
        log::info!("Starting DataMonitor: {}", self.m_name);
        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        log::info!("Stopping DataMonitor: {}", self.m_name);
        Ok(())
    }

    fn is_enabled(&self) -> bool {
        self.m_enabled
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.m_enabled = enabled;
    }

    fn receive_data(&mut self, data: StreamableData) {
        self.process_data(data);
    }

    fn receive_command(&mut self, cmd: &ApiRequest) {
        self.process_command(cmd);
    }

    fn set_on_output(&self, callback: ConsumerCallback) {
        self.set_on_output(callback);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fusion_types::ImuData;

    #[test]
    fn tracks_sample_counts() {
        let mut monitor = DataMonitor::new(serde_json::json!({}));

        for _ in 0..10 {
            let imu = ImuData {
                sender_id: "imu0".into(),
                ..Default::default()
            };
            monitor.process_data(StreamableData::Imu(imu));
        }

        let key = "imu0:Imu";
        assert!(monitor.stats().contains_key(key));
        assert_eq!(monitor.stats()[key].sample_count, 10);
    }

    #[test]
    fn tracks_gnss_quality() {
        let mut monitor = DataMonitor::new(serde_json::json!({}));

        let gnss = GnssData {
            sender_id: "gnss0".into(),
            quality: 4,
            n_sat: 12,
            hdop: 0.8,
            horizontal_accuracy: 0.02,
            ..Default::default()
        };
        monitor.process_data(StreamableData::Gnss(gnss));

        assert!(monitor.gnss_quality().contains_key("gnss0"));
        assert_eq!(monitor.gnss_quality()["gnss0"].quality, 4);
        assert_eq!(monitor.gnss_quality()["gnss0"].n_sat, 12);
    }

    #[test]
    fn status_json_structure() {
        let mut monitor = DataMonitor::new(serde_json::json!({}));

        let imu = ImuData {
            sender_id: "imu0".into(),
            ..Default::default()
        };
        monitor.process_data(StreamableData::Imu(imu));

        let status = monitor.status_json();
        assert!(status.get("sources").is_some());
        assert!(status.get("gnssQuality").is_some());
        assert_eq!(status["totalSources"], 1);
    }

    #[test]
    fn reset_clears_all() {
        let mut monitor = DataMonitor::new(serde_json::json!({}));

        let imu = ImuData {
            sender_id: "imu0".into(),
            ..Default::default()
        };
        monitor.process_data(StreamableData::Imu(imu));

        let gnss = GnssData {
            sender_id: "gnss0".into(),
            ..Default::default()
        };
        monitor.process_data(StreamableData::Gnss(gnss));

        assert!(!monitor.stats().is_empty());
        assert!(!monitor.gnss_quality().is_empty());

        let cmd = ApiRequest::new("reset", "", serde_json::Value::Null, "1");
        monitor.process_command(&cmd);

        assert!(monitor.stats().is_empty());
        assert!(monitor.gnss_quality().is_empty());
    }

    #[test]
    fn multiple_sources_tracked_independently() {
        let mut monitor = DataMonitor::new(serde_json::json!({}));

        for _ in 0..5 {
            let imu = ImuData {
                sender_id: "imu0".into(),
                ..Default::default()
            };
            monitor.process_data(StreamableData::Imu(imu));
        }

        for _ in 0..3 {
            let imu = ImuData {
                sender_id: "imu1".into(),
                ..Default::default()
            };
            monitor.process_data(StreamableData::Imu(imu));
        }

        assert_eq!(monitor.stats()["imu0:Imu"].sample_count, 5);
        assert_eq!(monitor.stats()["imu1:Imu"].sample_count, 3);
    }
}
