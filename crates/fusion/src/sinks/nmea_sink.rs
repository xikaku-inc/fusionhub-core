use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use fusion_types::{FusedVehiclePoseV2, GlobalFusedPose, StreamableData};

use crate::node::{Node, NodeBase};

/// Output transport for NMEA sentences.
#[derive(Clone, Debug)]
pub enum NmeaTransport {
    Serial { port: String, baud_rate: u32 },
    Tcp { host: String, port: u16 },
    Udp { host: String, port: u16 },
}

impl Default for NmeaTransport {
    fn default() -> Self {
        NmeaTransport::Udp {
            host: "127.0.0.1".into(),
            port: 10110,
        }
    }
}

/// NMEA sentence sink. Generates NMEA 0183 sentences from fused pose data
/// and outputs them via serial port, TCP, or UDP.
pub struct NmeaSink {
    pub base: NodeBase,
    m_transport: NmeaTransport,
    m_socket: Arc<Mutex<Option<std::net::UdpSocket>>>,
    m_serial: Arc<Mutex<Option<Box<dyn serialport::SerialPort>>>>,
    m_count: Arc<Mutex<u64>>,
}

impl NmeaSink {
    pub fn new(name: impl Into<String>, config: &serde_json::Value) -> Self {
        let transport = Self::parse_transport(config);
        Self {
            base: NodeBase::new(name),
            m_transport: transport,
            m_socket: Arc::new(Mutex::new(None)),
            m_serial: Arc::new(Mutex::new(None)),
            m_count: Arc::new(Mutex::new(0)),
        }
    }

    fn parse_transport(config: &serde_json::Value) -> NmeaTransport {
        let transport_type = config
            .get("transport")
            .and_then(|v| v.as_str())
            .unwrap_or("udp");

        match transport_type {
            "serial" => {
                let port = config
                    .get("port")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_owned();
                let baud = config
                    .get("baudRate")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(4800) as u32;
                NmeaTransport::Serial {
                    port,
                    baud_rate: baud,
                }
            }
            "tcp" => {
                let host = config
                    .get("host")
                    .and_then(|v| v.as_str())
                    .unwrap_or("127.0.0.1")
                    .to_owned();
                let port = config
                    .get("nmeaPort")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(10110) as u16;
                NmeaTransport::Tcp { host, port }
            }
            _ => {
                let host = config
                    .get("host")
                    .and_then(|v| v.as_str())
                    .unwrap_or("127.0.0.1")
                    .to_owned();
                let port = config
                    .get("nmeaPort")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(10110) as u16;
                NmeaTransport::Udp { host, port }
            }
        }
    }

    pub fn on_data(&self, data: StreamableData) {
        if !self.base.is_enabled() {
            return;
        }
        match &data {
            StreamableData::FusedVehiclePoseV2(pose) => {
                self.send_vehicle_pose_sentences(pose);
            }
            StreamableData::GlobalFusedPose(pose) => {
                self.send_global_pose_sentences(pose);
            }
            _ => {}
        }
        self.base.notify_consumers(data);
    }

    fn send_vehicle_pose_sentences(&self, pose: &FusedVehiclePoseV2) {
        let lat = pose.global_position.y;
        let lon = pose.global_position.x;
        let heading = pose.yaw.to_degrees();
        let speed = (pose.velocity.x.powi(2) + pose.velocity.y.powi(2)).sqrt();

        let gga = format_gga(lat, lon, 0.0, 1, 12, 0.9);
        self.send_sentence(&gga);

        let rmc = format_rmc(lat, lon, pose.timestamp, speed, heading);
        self.send_sentence(&rmc);

        let vtg = format_vtg(heading, speed);
        self.send_sentence(&vtg);

        let hdt = format_hdt(heading);
        self.send_sentence(&hdt);

        let mut count = self.m_count.lock().unwrap();
        *count += 4;
    }

    fn send_global_pose_sentences(&self, pose: &GlobalFusedPose) {
        let lat = pose.position.latitude;
        let lon = pose.position.longitude;
        let alt = pose.position.height;

        let gga = format_gga(lat, lon, alt, 1, 12, 0.9);
        self.send_sentence(&gga);

        let rmc = format_rmc(lat, lon, pose.timestamp, 0.0, 0.0);
        self.send_sentence(&rmc);

        let mut count = self.m_count.lock().unwrap();
        *count += 2;
    }

    fn send_sentence(&self, sentence: &str) {
        let bytes = sentence.as_bytes();

        match &self.m_transport {
            NmeaTransport::Udp { host, port } => {
                let socket = self.m_socket.lock().unwrap();
                if let Some(ref sock) = *socket {
                    let addr = format!("{}:{}", host, port);
                    if let Err(e) = sock.send_to(bytes, &addr) {
                        log::warn!("[{}] UDP send failed: {}", self.base.name(), e);
                    }
                }
            }
            NmeaTransport::Serial { .. } => {
                let mut serial = self.m_serial.lock().unwrap();
                if let Some(ref mut port) = *serial {
                    if let Err(e) = std::io::Write::write_all(port.as_mut(), bytes) {
                        log::warn!("[{}] Serial write failed: {}", self.base.name(), e);
                    }
                }
            }
            NmeaTransport::Tcp { .. } => {
                // TCP output would maintain a persistent connection.
                // Placeholder for future implementation.
                log::trace!("[{}] TCP NMEA output not yet connected", self.base.name());
            }
        }
    }

    pub fn sentence_count(&self) -> u64 {
        *self.m_count.lock().unwrap()
    }
}

impl Node for NmeaSink {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn start(&mut self) -> anyhow::Result<()> {
        log::info!(
            "NmeaSink '{}' starting (transport={:?})",
            self.base.name(),
            self.m_transport
        );

        match &self.m_transport {
            NmeaTransport::Udp { .. } => {
                match std::net::UdpSocket::bind("0.0.0.0:0") {
                    Ok(sock) => {
                        *self.m_socket.lock().unwrap() = Some(sock);
                    }
                    Err(e) => {
                        log::error!("[{}] Failed to create UDP socket: {}", self.base.name(), e);
                    }
                }
            }
            NmeaTransport::Serial { port, baud_rate } => {
                match serialport::new(port, *baud_rate)
                    .timeout(std::time::Duration::from_millis(100))
                    .open()
                {
                    Ok(serial) => {
                        *self.m_serial.lock().unwrap() = Some(serial);
                    }
                    Err(e) => {
                        log::warn!(
                            "[{}] Could not open serial port '{}': {}",
                            self.base.name(),
                            port,
                            e
                        );
                    }
                }
            }
            NmeaTransport::Tcp { host, port } => {
                log::info!(
                    "[{}] TCP transport configured for {}:{}",
                    self.base.name(),
                    host,
                    port
                );
            }
        }

        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        log::info!(
            "NmeaSink '{}' stopping (sent {} sentences)",
            self.base.name(),
            self.sentence_count()
        );
        *self.m_socket.lock().unwrap() = None;
        *self.m_serial.lock().unwrap() = None;
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

// ---------------------------------------------------------------------------
// NMEA formatting functions
// ---------------------------------------------------------------------------

/// Compute the NMEA checksum: XOR all characters between '$' and '*'.
/// Returns a 2-digit uppercase hex string.
pub fn checksum(sentence: &str) -> String {
    let start = sentence.find('$').map(|i| i + 1).unwrap_or(0);
    let end = sentence.find('*').unwrap_or(sentence.len());
    let xor = sentence[start..end]
        .bytes()
        .fold(0u8, |acc, b| acc ^ b);
    format!("{:02X}", xor)
}

/// Convert decimal degrees to NMEA DDMM.MMMM format.
/// Returns (nmea_value, hemisphere_char).
/// For latitude: N/S; for longitude: E/W.
/// The `is_longitude` flag controls the formatting width.
pub fn decimal_degrees_to_nmea(deg: f64, is_longitude: bool) -> (String, char) {
    let hemisphere = if is_longitude {
        if deg >= 0.0 { 'E' } else { 'W' }
    } else if deg >= 0.0 {
        'N'
    } else {
        'S'
    };

    let abs_deg = deg.abs();
    let degrees = abs_deg.floor() as i32;
    let minutes = (abs_deg - degrees as f64) * 60.0;

    let formatted = if is_longitude {
        format!("{:03}{:09.6}", degrees, minutes)
    } else {
        format!("{:02}{:09.6}", degrees, minutes)
    };

    (formatted, hemisphere)
}

/// Format a $GPGGA sentence.
pub fn format_gga(
    latitude: f64,
    longitude: f64,
    altitude: f64,
    gnss_quality: i32,
    n_sat: i32,
    hdop: f64,
) -> String {
    let now = SystemTime::now();
    let secs = now
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let hours = (secs % 86400) / 3600;
    let mins = (secs % 3600) / 60;
    let s = secs % 60;
    let time_str = format!("{:02}{:02}{:02}.00", hours, mins, s);

    let (lat_str, lat_hem) = decimal_degrees_to_nmea(latitude, false);
    let (lon_str, lon_hem) = decimal_degrees_to_nmea(longitude, true);

    let body = format!(
        "GPGGA,{},{},{},{},{},{},{:02},{:.1},{:.1},M,0.0,M,,",
        time_str, lat_str, lat_hem, lon_str, lon_hem, gnss_quality, n_sat, hdop, altitude
    );
    let sentence = format!("${}*", body);
    let cs = checksum(&sentence);
    format!("{}{}\\r\\n", sentence, cs)
}

/// Format a $GPRMC sentence.
pub fn format_rmc(
    latitude: f64,
    longitude: f64,
    timestamp: SystemTime,
    speed_mps: f64,
    heading: f64,
) -> String {
    let secs = timestamp
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let hours = (secs % 86400) / 3600;
    let mins = (secs % 3600) / 60;
    let s = secs % 60;
    let time_str = format!("{:02}{:02}{:02}.00", hours, mins, s);

    let days = secs / 86400;
    let (y, m, d) = days_to_date(days);
    let date_str = format!("{:02}{:02}{:02}", d, m, y % 100);

    let (lat_str, lat_hem) = decimal_degrees_to_nmea(latitude, false);
    let (lon_str, lon_hem) = decimal_degrees_to_nmea(longitude, true);

    let speed_knots = speed_mps * 1.94384;
    let heading_norm = ((heading % 360.0) + 360.0) % 360.0;

    let body = format!(
        "GPRMC,{},A,{},{},{},{},{:.1},{:.1},{},,,A",
        time_str, lat_str, lat_hem, lon_str, lon_hem, speed_knots, heading_norm, date_str
    );
    let sentence = format!("${}*", body);
    let cs = checksum(&sentence);
    format!("{}{}\\r\\n", sentence, cs)
}

/// Format a $GPVTG sentence.
pub fn format_vtg(heading: f64, speed_mps: f64) -> String {
    let heading_norm = ((heading % 360.0) + 360.0) % 360.0;
    let speed_knots = speed_mps * 1.94384;
    let speed_kmh = speed_mps * 3.6;

    let body = format!(
        "GPVTG,{:.1},T,,M,{:.1},N,{:.1},K,A",
        heading_norm, speed_knots, speed_kmh
    );
    let sentence = format!("${}*", body);
    let cs = checksum(&sentence);
    format!("{}{}\\r\\n", sentence, cs)
}

/// Format a $GPHDT sentence (true heading).
pub fn format_hdt(heading: f64) -> String {
    let heading_norm = ((heading % 360.0) + 360.0) % 360.0;
    let body = format!("GPHDT,{:.1},T", heading_norm);
    let sentence = format!("${}*", body);
    let cs = checksum(&sentence);
    format!("{}{}\\r\\n", sentence, cs)
}

/// Simple days-since-epoch to (year, month, day) conversion.
fn days_to_date(days_since_epoch: u64) -> (u64, u64, u64) {
    // Simplified algorithm based on civil_from_days
    let z = days_since_epoch as i64 + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as u64, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_basic() {
        let sentence = "$GPGGA,123456.00,4807.038,N,01131.000,E,1,08,0.9,545.4,M,47.0,M,,*";
        let cs = checksum(sentence);
        assert_eq!(cs.len(), 2);
        // Checksum should be two hex characters
        assert!(u8::from_str_radix(&cs, 16).is_ok());
    }

    #[test]
    fn decimal_degrees_to_nmea_north() {
        let (val, hem) = decimal_degrees_to_nmea(48.1173, false);
        assert_eq!(hem, 'N');
        assert!(val.starts_with("48"));
    }

    #[test]
    fn decimal_degrees_to_nmea_south() {
        let (_, hem) = decimal_degrees_to_nmea(-33.856, false);
        assert_eq!(hem, 'S');
    }

    #[test]
    fn decimal_degrees_to_nmea_east() {
        let (val, hem) = decimal_degrees_to_nmea(11.516, true);
        assert_eq!(hem, 'E');
        assert!(val.starts_with("011"));
    }

    #[test]
    fn decimal_degrees_to_nmea_west() {
        let (_, hem) = decimal_degrees_to_nmea(-122.4, true);
        assert_eq!(hem, 'W');
    }

    #[test]
    fn format_gga_contains_gpgga() {
        let sentence = format_gga(48.1173, 11.516, 520.0, 1, 10, 0.9);
        assert!(sentence.contains("GPGGA"));
    }

    #[test]
    fn format_rmc_contains_gprmc() {
        let sentence = format_rmc(48.0, 11.0, SystemTime::now(), 5.0, 180.0);
        assert!(sentence.contains("GPRMC"));
    }

    #[test]
    fn format_vtg_contains_gpvtg() {
        let sentence = format_vtg(90.0, 10.0);
        assert!(sentence.contains("GPVTG"));
    }

    #[test]
    fn format_hdt_contains_gphdt() {
        let sentence = format_hdt(270.0);
        assert!(sentence.contains("GPHDT"));
    }

    #[test]
    fn heading_normalization() {
        let s1 = format_hdt(-90.0);
        assert!(s1.contains("270.0"));

        let s2 = format_hdt(450.0);
        assert!(s2.contains("90.0"));
    }

    #[test]
    fn days_to_date_epoch() {
        let (y, m, d) = days_to_date(0);
        assert_eq!((y, m, d), (1970, 1, 1));
    }
}
