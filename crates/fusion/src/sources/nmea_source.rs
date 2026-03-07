use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use nalgebra::UnitQuaternion;
use tokio::task::JoinHandle;

use fusion_registry::{sf, SettingsField};
use fusion_types::{GnssData, StreamableData, Timestamp, Vec3d};
use serde_json::json;

use crate::node::{ConsumerCallback, Node, NodeBase};

pub fn settings_schema() -> Vec<SettingsField> {
    vec![
        sf("port", "Serial Port", "string", json!("COM1")),
        sf("baudrate", "Baud Rate", "number", json!(115200)),
        sf("dualGPS", "Dual GPS", "boolean", json!(false)),
        sf("initializeUnicore", "Initialize Unicore", "boolean", json!(true)),
        sf("gnssOutputPeriodMS", "Output Period (ms)", "number", json!(100)),
    ]
}

const PI: f64 = std::f64::consts::PI;

#[derive(Clone, Copy, PartialEq, Eq)]
enum NmeaState {
    WaitingForFirstParse,
    ParseFailed,
    WaitingForGoodParse,
    Running,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum GpsType {
    Single,
    Dual,
}

const GGA_READY: u8 = 0x01;
const GSA_READY: u8 = 0x02;
const VTG_READY: u8 = 0x04;
const HDT_READY: u8 = 0x08;
const SINGLE_GPS_ALL_READY: u8 = GGA_READY | GSA_READY | VTG_READY;
const DUAL_GPS_ALL_READY: u8 = GGA_READY | GSA_READY | VTG_READY | HDT_READY;

/// NMEA sentence source node.
///
/// Parses GGA, GSA, VTG, and HDT NMEA sentences from a serial port.
/// Produces GnssData when a complete set of sentences has been received.
pub struct NmeaSource {
    pub base: NodeBase,
    m_port: String,
    m_baudrate: u32,
    m_gps_type: GpsType,
    m_initialize_unicore: bool,
    m_gnss_output_period_ms: u32,
    m_use_gps_timestamps: bool,
    m_use_rtcm: bool,
    m_done: Arc<AtomicBool>,
    m_worker_handle: Option<JoinHandle<()>>,
    m_latest_gps_quality: Arc<Mutex<i32>>,
}

impl NmeaSource {
    pub fn new(name: impl Into<String>, config: &serde_json::Value) -> Self {
        let name = name.into();
        let port = config
            .get("port")
            .and_then(|v| v.as_str())
            .unwrap_or("COM1")
            .to_string();
        let baudrate = config
            .get("baudrate")
            .and_then(|v| v.as_u64())
            .unwrap_or(115200) as u32;
        let dual_gps = config
            .get("dualGPS")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let initialize_unicore = config
            .get("initializeUnicore")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let gnss_output_period_ms = config
            .get("gnssOutputPeriodMS")
            .and_then(|v| v.as_u64())
            .unwrap_or_else(|| {
                config
                    .get("gnssOutputPeriod")
                    .and_then(|v| v.as_f64())
                    .map(|s| (s * 1000.0) as u64)
                    .unwrap_or(500)
            }) as u32;
        let use_gps_timestamps = config
            .get("useGpsTimestamps")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let use_rtcm = config
            .get("rtcm")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let gps_type = if dual_gps {
            GpsType::Dual
        } else {
            GpsType::Single
        };

        log::info!(
            "NMEA Port: {}, Baudrate: {}, GPS type: {}",
            port,
            baudrate,
            if dual_gps { "Dual GPS" } else { "Single GPS" }
        );
        if initialize_unicore {
            log::info!(
                "Unicore initialization enabled with output period: {}ms",
                gnss_output_period_ms
            );
        }

        Self {
            base: NodeBase::new(&name),
            m_port: port,
            m_baudrate: baudrate,
            m_gps_type: gps_type,
            m_initialize_unicore: initialize_unicore,
            m_gnss_output_period_ms: gnss_output_period_ms,
            m_use_gps_timestamps: use_gps_timestamps,
            m_use_rtcm: use_rtcm,
            m_done: Arc::new(AtomicBool::new(false)),
            m_worker_handle: None,
            m_latest_gps_quality: Arc::new(Mutex::new(0)),
        }
    }

    pub fn use_rtcm(&self) -> bool {
        self.m_use_rtcm
    }
}

/// Convert NMEA DDDMM.MMMM format to decimal degrees.
pub(crate) fn ddmm_to_decimal(ddmm: f64) -> f64 {
    let d = (ddmm / 100.0).floor() as i32;
    let m = ddmm - (d as f64) * 100.0;
    d as f64 + m / 60.0
}

/// Verify NMEA checksum: XOR all chars between '$' and '*', compare with hex after '*'.
pub(crate) fn verify_nmea_checksum(sentence: &[u8]) -> bool {
    let start = match sentence.iter().position(|&b| b == b'$') {
        Some(i) => i + 1,
        None => return false,
    };
    let star = match sentence.iter().position(|&b| b == b'*') {
        Some(i) => i,
        None => return false,
    };
    if star + 3 > sentence.len() {
        return false;
    }

    let mut checksum: u8 = 0;
    for &b in &sentence[start..star] {
        checksum ^= b;
    }

    let hex_str = std::str::from_utf8(&sentence[star + 1..star + 3]).unwrap_or("");
    let expected = u8::from_str_radix(hex_str, 16).unwrap_or(0);
    checksum == expected
}

/// Parse a GGA sentence, updating the output data.
pub(crate) fn parse_gga(fields: &[&str], out: &mut GnssData, use_gps_timestamps: bool) {
    if fields.len() < 14 {
        return;
    }

    // Field 0: UTC time (hhmmss.ss)
    if !fields[0].is_empty() && fields[0].len() >= 6 {
        if use_gps_timestamps {
            out.timestamp = SystemTime::now();
        } else {
            out.timestamp = SystemTime::now();
        }
    }

    // Field 1: Latitude
    if !fields[1].is_empty() {
        if let Ok(val) = fields[1].parse::<f64>() {
            out.latitude = ddmm_to_decimal(val);
        }
    }

    // Field 2: N/S
    if fields[2] == "S" {
        out.latitude = -out.latitude;
    }

    // Field 3: Longitude
    if !fields[3].is_empty() {
        if let Ok(val) = fields[3].parse::<f64>() {
            out.longitude = ddmm_to_decimal(val);
        }
    }

    // Field 4: E/W
    if fields[4] == "W" {
        out.longitude = -out.longitude;
    }

    // Field 5: GPS Quality indicator
    if !fields[5].is_empty() {
        if let Ok(q) = fields[5].parse::<i32>() {
            out.quality = q;
        }
    }

    // Field 6: Number of satellites
    if !fields[6].is_empty() {
        if let Ok(n) = fields[6].parse::<i32>() {
            out.n_sat = n;
        }
    }

    // Field 7: HDOP
    if !fields[7].is_empty() {
        if let Ok(h) = fields[7].parse::<f64>() {
            out.hdop = h;
        }
    }

    // Field 8: Altitude (MSL)
    if !fields[8].is_empty() {
        if let Ok(a) = fields[8].parse::<f64>() {
            out.altitude = a;
        }
    }

    // Field 10: Geoid separation
    if fields.len() > 10 && !fields[10].is_empty() {
        if let Ok(u) = fields[10].parse::<f64>() {
            out.undulation = u;
            out.height = out.altitude + out.undulation;
        }
    }

    // Field 12: Age of differential GPS data
    if fields.len() > 12 && !fields[12].is_empty() {
        if let Ok(age) = fields[12].parse::<f64>() {
            out.diff_age = age;
        }
    }
}

/// Parse a GSA sentence, updating the output data.
fn parse_gsa(fields: &[&str], out: &mut GnssData) {
    // GSA has mode1, mode2, 12 PRN numbers, PDOP, HDOP, VDOP
    if fields.len() < 17 {
        return;
    }

    // Field 14 (index 14): PDOP - skipped
    // Field 15 (index 15): HDOP
    if !fields[14].is_empty() {
        // PDOP, skip
    }
    if !fields[15].is_empty() {
        if let Ok(h) = fields[15].parse::<f64>() {
            out.horizontal_accuracy = h;
        }
    }
    // Field 16: VDOP
    if !fields[16].is_empty() {
        // Remove any trailing checksum marker
        let val_str = fields[16].split('*').next().unwrap_or("");
        if !val_str.is_empty() {
            if let Ok(v) = val_str.parse::<f64>() {
                out.vertical_accuracy = v;
            }
        }
    }
}

/// Parse a VTG sentence, updating the output data.
fn parse_vtg(fields: &[&str], out: &mut GnssData, gps_type: GpsType) {
    if fields.is_empty() {
        return;
    }

    // Field 0: Track made good (degrees true)
    if gps_type == GpsType::Single && !fields[0].is_empty() {
        if let Ok(tmg) = fields[0].parse::<f64>() {
            out.tmg = tmg;
            out.orientation =
                UnitQuaternion::from_axis_angle(&Vec3d::z_axis(), tmg * PI / 180.0);
        }
    }
}

/// Parse an HDT sentence, updating the output data.
fn parse_hdt(fields: &[&str], out: &mut GnssData, gps_type: GpsType) {
    if fields.is_empty() {
        return;
    }

    // Field 0: Heading in degrees
    if gps_type == GpsType::Dual && !fields[0].is_empty() {
        if let Ok(heading) = fields[0].parse::<f64>() {
            out.heading = heading;
            out.orientation =
                UnitQuaternion::from_axis_angle(&Vec3d::z_axis(), heading * PI / 180.0);
        }
    }
}

/// Parse NMEA sentence from a complete line and return the sentence type.
pub(crate) fn parse_nmea_sentence(
    line: &str,
    out: &mut GnssData,
    gps_type: GpsType,
    use_gps_timestamps: bool,
) -> Option<&'static str> {
    let bytes = line.as_bytes();
    if !verify_nmea_checksum(bytes) {
        return None;
    }

    // Extract content between '$' and '*'
    let start = line.find('$')? + 1;
    let end = line.find('*')?;
    let content = &line[start..end];

    // Split into fields
    let fields: Vec<&str> = content.split(',').collect();
    if fields.is_empty() || fields[0].len() < 5 {
        return None;
    }

    // Command is chars 2..5 of the talker+sentence ID (e.g., "GPGGA" -> "GGA")
    let cmd = &fields[0][2..];
    let data_fields = &fields[1..];

    match cmd {
        "GGA" => {
            parse_gga(data_fields, out, use_gps_timestamps);
            Some("GGA")
        }
        "GSA" => {
            parse_gsa(data_fields, out);
            Some("GSA")
        }
        "VTG" => {
            parse_vtg(data_fields, out, gps_type);
            Some("VTG")
        }
        "HDT" => {
            parse_hdt(data_fields, out, gps_type);
            Some("HDT")
        }
        _ => None,
    }
}

impl Node for NmeaSource {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn status(&self) -> serde_json::Value {
        let quality = *self.m_latest_gps_quality.lock().unwrap();
        serde_json::json!({
            "gpsQuality": quality,
            "port": self.m_port,
        })
    }

    fn start(&mut self) -> anyhow::Result<()> {
        log::info!(
            "Starting NMEA source on port {} baudrate {}",
            self.m_port,
            self.m_baudrate
        );

        let port_name = self.m_port.clone();
        let baudrate = self.m_baudrate;
        let gps_type = self.m_gps_type;
        let use_gps_timestamps = self.m_use_gps_timestamps;
        let initialize_unicore = self.m_initialize_unicore;
        let gnss_output_period_ms = self.m_gnss_output_period_ms;
        let done = self.m_done.clone();
        let consumers = self.base.consumers_arc();
        let enabled = self.base.enabled_arc();
        let latest_quality = self.m_latest_gps_quality.clone();
        let node_name = self.base.name().to_string();

        self.m_worker_handle = Some(tokio::spawn(async move {
            let result = tokio::task::spawn_blocking(move || {
                let port_result = serialport::new(&port_name, baudrate)
                    .timeout(Duration::from_millis(100))
                    .open();

                let mut port = match port_result {
                    Ok(p) => p,
                    Err(e) => {
                        log::warn!("Failed to open NMEA port {}: {}", port_name, e);
                        return;
                    }
                };

                log::info!("NMEA port is open. Waiting for data...");

                if initialize_unicore {
                    log::info!("Initializing Unicore UM982 GNSS module...");
                    let period_str = format!("{}", gnss_output_period_ms as f64 / 1000.0);

                    std::thread::sleep(Duration::from_millis(500));

                    let send_cmd = |port: &mut Box<dyn serialport::SerialPort>, cmd: &str| {
                        let full = format!("{}\r\n", cmd);
                        let _ = port.write_all(full.as_bytes());
                        log::info!("Sent GNSS command: {}", cmd);
                        std::thread::sleep(Duration::from_millis(100));
                    };

                    // Disable unnecessary messages
                    send_cmd(&mut port, "GPRMC 0");
                    send_cmd(&mut port, "GPGSV 0");
                    send_cmd(&mut port, "GPGLL 0");
                    send_cmd(&mut port, "GPZDA 0");

                    // Enable required NMEA messages with configured period
                    send_cmd(&mut port, &format!("GPGGA {}", period_str));
                    send_cmd(&mut port, &format!("GPGSA {}", period_str));
                    send_cmd(&mut port, &format!("GPVTG {}", period_str));
                    send_cmd(&mut port, &format!("GPHDT {}", period_str));

                    // Save configuration
                    send_cmd(&mut port, "SAVECONFIG");

                    log::info!(
                        "UM982 initialization complete. Messages configured at {}ms period",
                        gnss_output_period_ms
                    );
                }

                let mut line_buf = String::new();
                let mut read_buf = [0u8; 1024];
                let mut out_data = GnssData::default();
                out_data.sender_id = node_name.clone();
                let mut sending_stage: u8 = 0;
                let mut nmea_state = NmeaState::WaitingForFirstParse;

                while !done.load(Ordering::Relaxed) {
                    match port.read(&mut read_buf) {
                        Ok(n) => {
                            let chunk = String::from_utf8_lossy(&read_buf[..n]);
                            line_buf.push_str(&chunk);

                            while let Some(pos) = line_buf.find("\r\n") {
                                let line = line_buf[..pos].to_string();
                                line_buf = line_buf[pos + 2..].to_string();

                                match parse_nmea_sentence(&line, &mut out_data, gps_type, use_gps_timestamps) {
                                    Some("GGA") => {
                                        sending_stage |= GGA_READY;

                                        // Forward GPS timestamp to consumers
                                        if enabled.load(Ordering::Relaxed) {
                                            let ts = Timestamp { now: out_data.timestamp };
                                            let cbs = consumers.lock().unwrap();
                                            for cb in cbs.iter() {
                                                cb(StreamableData::Timestamp(ts.clone()));
                                            }
                                        }
                                    }
                                    Some("GSA") => sending_stage |= GSA_READY,
                                    Some("VTG") => sending_stage |= VTG_READY,
                                    Some("HDT") => sending_stage |= HDT_READY,
                                    _ => {
                                        if line.starts_with('$') {
                                            nmea_state = NmeaState::ParseFailed;
                                        }
                                    }
                                }

                                let all_ready = match gps_type {
                                    GpsType::Single => sending_stage & SINGLE_GPS_ALL_READY == SINGLE_GPS_ALL_READY,
                                    GpsType::Dual => sending_stage & DUAL_GPS_ALL_READY == DUAL_GPS_ALL_READY,
                                };

                                if all_ready {
                                    match nmea_state {
                                        NmeaState::WaitingForFirstParse => {
                                            log::info!("Successfully received data from NMEA source (first connection)");
                                            nmea_state = NmeaState::Running;
                                        }
                                        NmeaState::WaitingForGoodParse => {
                                            log::info!("Successfully received data from NMEA source (recovered)");
                                            nmea_state = NmeaState::Running;
                                        }
                                        _ => {}
                                    }

                                    *latest_quality.lock().unwrap() = out_data.quality;

                                    if enabled.load(Ordering::Relaxed) {
                                        let cbs = consumers.lock().unwrap();
                                        let data = StreamableData::Gnss(out_data.clone());
                                        for cb in cbs.iter() {
                                            cb(data.clone());
                                        }
                                    }

                                    sending_stage = 0;
                                } else if nmea_state == NmeaState::ParseFailed {
                                    nmea_state = NmeaState::WaitingForGoodParse;
                                }
                            }
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => continue,
                        Err(e) => {
                            log::warn!("NMEA read error: {}", e);
                            break;
                        }
                    }
                }
            })
            .await;

            if let Err(e) = result {
                log::warn!("NMEA worker thread panicked: {}", e);
            }
        }));

        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        log::info!("Stopping NMEA source: {}", self.base.name());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ddmm_to_decimal_conversion() {
        let result = ddmm_to_decimal(4807.038);
        assert!((result - 48.1173).abs() < 0.001);
    }

    #[test]
    fn verify_checksum_valid() {
        let sentence = b"$GPGGA,092750.000,5321.6802,N,00630.3372,W,1,8,1.03,61.7,M,55.2,M,,*76";
        assert!(verify_nmea_checksum(sentence));
    }

    #[test]
    fn verify_checksum_invalid() {
        let sentence = b"$GPGGA,092750.000,5321.6802,N,00630.3372,W,1,8,1.03,61.7,M,55.2,M,,*00";
        assert!(!verify_nmea_checksum(sentence));
    }

    #[test]
    fn parse_gga_sentence() {
        let line = "$GPGGA,092750.000,5321.6802,N,00630.3372,W,1,8,1.03,61.7,M,55.2,M,,*76";
        let mut out = GnssData::default();
        let result = parse_nmea_sentence(line, &mut out, GpsType::Single, false);
        assert_eq!(result, Some("GGA"));
        assert!((out.latitude - 53.36134).abs() < 0.001);
        assert!((out.longitude - (-6.505620)).abs() < 0.001);
        assert_eq!(out.quality, 1);
        assert_eq!(out.n_sat, 8);
    }
}
