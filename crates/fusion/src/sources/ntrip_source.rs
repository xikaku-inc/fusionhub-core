use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use tokio::task::JoinHandle;

use fusion_types::{GnssData, JsonValueExt, RTCMData, StreamableData};

use crate::node::{ConsumerCallback, Node, NodeBase};

const BASE64_TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(input: &[u8]) -> String {
    let mut out = Vec::new();
    let len = input.len();
    let mut i = 0;
    while i < len {
        out.push(BASE64_TABLE[((input[i] & 0xFC) >> 2) as usize]);
        if i + 1 >= len {
            out.push(BASE64_TABLE[((input[i] & 0x03) << 4) as usize]);
            break;
        }
        out.push(
            BASE64_TABLE[((input[i] & 0x03) << 4 | (input[i + 1] & 0xF0) >> 4) as usize],
        );
        if i + 2 >= len {
            out.push(BASE64_TABLE[((input[i + 1] & 0x0F) << 2) as usize]);
            break;
        }
        out.push(
            BASE64_TABLE
                [((input[i + 1] & 0x0F) << 2 | (input[i + 2] & 0xC0) >> 6) as usize],
        );
        out.push(BASE64_TABLE[(input[i + 2] & 0x3F) as usize]);
        i += 3;
    }
    while out.len() % 4 != 0 {
        out.push(b'=');
    }
    String::from_utf8(out).unwrap()
}

/// Verify BCC checksum for a GGA sentence.
///
/// Returns true if the checksum embedded after '*' matches the XOR of all bytes
/// between '$' and '*'.
pub fn bcc_checksum_compare_for_gga(line: &str) -> bool {
    let bytes = line.as_bytes();
    if bytes.is_empty() || bytes[0] != b'$' {
        return false;
    }

    let mut checksum: u8 = 0;
    let mut i = 1;
    while i < bytes.len() && bytes[i] != b'*' {
        checksum ^= bytes[i];
        i += 1;
    }

    if i >= bytes.len() || bytes[i] != b'*' {
        return false;
    }

    i += 1; // Skip '*'
    if i + 2 > bytes.len() {
        return false;
    }

    let hex_str = std::str::from_utf8(&bytes[i..i + 2]).unwrap_or("");
    let expected = u8::from_str_radix(hex_str, 16).unwrap_or(0);

    checksum == expected
}

/// Convert decimal degrees to DDDMM.MMMM format used in NMEA.
fn degree_to_ddmm(degree: f64) -> f64 {
    let deg = degree.floor() as i32;
    let minute = degree - deg as f64;
    deg as f64 + minute * 60.0 / 100.0
}

/// Generate a GGA frame from GNSS data for forwarding to NTRIP caster.
fn generate_gga_frame(gnss: &GnssData) -> String {
    let now = chrono::Utc::now();
    let hours = now.format("%H").to_string();
    let minutes = now.format("%M").to_string();
    let seconds = now.format("%S%.3f").to_string();

    let lat_ddmm = (degree_to_ddmm(gnss.latitude.abs()) * 100.0).abs();
    let lat_dir = if gnss.latitude > 0.0 { "N" } else { "S" };
    let lon_ddmm = (degree_to_ddmm(gnss.longitude.abs()) * 100.0).abs();
    let lon_dir = if gnss.longitude > 0.0 { "E" } else { "W" };

    let body = format!(
        "GPGGA,{}{}{},{:012.7},{},{:013.7},{},{},{},1.2,{:.4},M,-2.860,M,1,0000",
        hours,
        minutes,
        seconds,
        lat_ddmm,
        lat_dir,
        lon_ddmm,
        lon_dir,
        gnss.quality,
        gnss.n_sat,
        gnss.height
    );

    let mut checksum: u8 = 0;
    for b in body.as_bytes() {
        checksum ^= b;
    }

    format!("${body}*{checksum:02X}\r\n")
}

/// NTRIP source node.
///
/// Connects to an NTRIP caster via HTTP and receives RTCM correction data.
/// Supports automatic reconnection and periodic GGA forwarding.
pub struct NtripSource {
    pub base: NodeBase,
    m_host: String,
    m_port: String,
    m_mountpoint: String,
    m_user: String,
    m_password: String,
    m_user_agent: String,
    m_forward_gnss: bool,
    m_reconnect_interval_ms: u64,
    m_max_reconnect_attempts: i32,
    m_initial_latitude: f64,
    m_initial_longitude: f64,
    m_done: Arc<AtomicBool>,
    m_worker_handle: Option<JoinHandle<()>>,
    m_latest_gnss_data: Arc<Mutex<GnssData>>,
}

impl NtripSource {
    pub fn new(name: impl Into<String>, config: &serde_json::Value) -> Self {
        let name = name.into();
        let host = config.value_str("host", "192.168.1.1");
        // Port can be specified as a string or integer in the config.
        let port = if let Some(s) = config.get("port").and_then(|v| v.as_str()) {
            s.to_string()
        } else {
            config.value_u16("port", 2101).to_string()
        };
        let mountpoint = config.value_str("mountpoint", "mountpoint");
        let user = config.value_str("user", "user");
        let password = config.value_str("password", "password");
        let user_agent = config.value_str("userAgent", "LPVR-POS");
        let forward_gnss = config.value_bool("forwardGnss", false);
        let reconnect_interval_ms = config.value_u64("reconnectIntervalMs", 5000);
        let max_reconnect_attempts = config.value_i64("maxReconnectAttempts", -1) as i32;
        let initial_lat = config.value_f64("initialLatitude", 0.0);
        let initial_lon = config.value_f64("initialLongitude", 0.0);

        log::info!("NTRIP Server: {}:{} /{}", host, port, mountpoint);
        log::info!(
            "Reconnection enabled: interval={}ms, maxAttempts={}",
            reconnect_interval_ms,
            if max_reconnect_attempts == -1 {
                "infinite".to_string()
            } else {
                max_reconnect_attempts.to_string()
            }
        );

        let mut gnss = GnssData::default();
        gnss.quality = 1;
        gnss.latitude = initial_lat;
        gnss.longitude = initial_lon;

        Self {
            base: NodeBase::new(&name),
            m_host: host,
            m_port: port,
            m_mountpoint: mountpoint,
            m_user: user,
            m_password: password,
            m_user_agent: user_agent,
            m_forward_gnss: forward_gnss,
            m_reconnect_interval_ms: reconnect_interval_ms,
            m_max_reconnect_attempts: max_reconnect_attempts,
            m_initial_latitude: initial_lat,
            m_initial_longitude: initial_lon,
            m_done: Arc::new(AtomicBool::new(false)),
            m_worker_handle: None,
            m_latest_gnss_data: Arc::new(Mutex::new(gnss)),
        }
    }

    /// Update the latest GNSS data used for GGA frame generation.
    pub fn update_gnss_data(&self, gnss: GnssData) {
        *self.m_latest_gnss_data.lock().unwrap() = gnss;
    }
}

impl Node for NtripSource {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn start(&mut self) -> anyhow::Result<()> {
        log::info!("Starting NTRIP source: {}", self.base.name());

        let host = self.m_host.clone();
        let port = self.m_port.clone();
        let mountpoint = self.m_mountpoint.clone();
        let user = self.m_user.clone();
        let password = self.m_password.clone();
        let user_agent = self.m_user_agent.clone();
        let done = self.m_done.clone();
        let consumers = self.base.consumers_arc();
        let enabled = self.base.enabled_arc();
        let latest_gnss = self.m_latest_gnss_data.clone();
        let reconnect_interval_ms = self.m_reconnect_interval_ms;
        let max_reconnect_attempts = self.m_max_reconnect_attempts;
        let node_name = self.base.name().to_string();

        self.m_worker_handle = Some(tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            use tokio::net::TcpStream;

            let mut reconnect_attempt = 0;
            let mut needs_reconnect = false;

            while !done.load(Ordering::Relaxed) {
                if needs_reconnect {
                    log::info!(
                        "Waiting {}ms before reconnection attempt {}",
                        reconnect_interval_ms,
                        reconnect_attempt + 1
                    );
                    tokio::time::sleep(Duration::from_millis(reconnect_interval_ms)).await;

                    if done.load(Ordering::Relaxed) {
                        break;
                    }
                }

                log::info!("Attempting to connect to NTRIP server {}:{}...", host, port);

                let addr = format!("{}:{}", host, port);
                let stream = match TcpStream::connect(&addr).await {
                    Ok(s) => s,
                    Err(e) => {
                        log::warn!("Failed to connect to NTRIP server: {}", e);
                        reconnect_attempt += 1;
                        if max_reconnect_attempts > 0
                            && reconnect_attempt > max_reconnect_attempts
                        {
                            log::warn!("Max connection attempts reached. Giving up.");
                            return;
                        }
                        needs_reconnect = true;
                        continue;
                    }
                };

                log::info!("TCP connection established to {}:{}", host, port);

                let (mut reader, mut writer) = stream.into_split();

                // Build and send NTRIP request
                let credentials = format!("{}:{}", user, password);
                let credentials_b64 = base64_encode(credentials.as_bytes());

                let request = format!(
                    "GET /{} HTTP/1.1\r\n\
                     User-Agent: NTRIP {}\r\n\
                     Ntrip-Version: Ntrip/2.0\r\n\
                     Authorization: Basic {}\r\n\
                     \r\n",
                    mountpoint, user_agent, credentials_b64
                );

                if let Err(e) = writer.write_all(request.as_bytes()).await {
                    log::warn!("Error sending NTRIP request: {}", e);
                    needs_reconnect = true;
                    continue;
                }

                // Read server response
                let mut response_buf = [0u8; 1024];
                let n = match reader.read(&mut response_buf).await {
                    Ok(n) => n,
                    Err(e) => {
                        log::warn!("Error reading NTRIP response: {}", e);
                        needs_reconnect = true;
                        continue;
                    }
                };

                let response = String::from_utf8_lossy(&response_buf[..n]);
                if !response.contains("200 OK") {
                    log::warn!("NTRIP server rejected connection: {}", response);
                    needs_reconnect = true;
                    continue;
                }

                log::info!("Successfully connected to NTRIP Caster");
                reconnect_attempt = 0;
                needs_reconnect = false;

                // Send initial GGA frame
                let gga = {
                    let gnss = latest_gnss.lock().unwrap();
                    generate_gga_frame(&gnss)
                };
                let _ = writer.write_all(gga.as_bytes()).await;

                // Read RTCM data
                let mut data_buf = [0u8; 4096];
                loop {
                    if done.load(Ordering::Relaxed) {
                        break;
                    }

                    match reader.read(&mut data_buf).await {
                        Ok(0) => {
                            log::warn!("NTRIP connection closed by server");
                            needs_reconnect = true;
                            break;
                        }
                        Ok(n) => {
                            if enabled.load(Ordering::Relaxed) {
                                let rtcm = RTCMData {
                                    sender_id: node_name.clone(),
                                    timestamp: SystemTime::now(),
                                    chunk: data_buf[..n].to_vec(),
                                    length: n as i32,
                                };
                                let data = StreamableData::Rtcm(rtcm);
                                let cbs = consumers.lock().unwrap();
                                for cb in cbs.iter() {
                                    cb(data.clone());
                                }
                            }
                        }
                        Err(e) => {
                            log::warn!("Error reading RTCM data: {}", e);
                            needs_reconnect = true;
                            break;
                        }
                    }
                }
            }
        }));

        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        log::info!("Stopping NTRIP source: {}", self.base.name());
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
    fn bcc_checksum_valid() {
        let gga = "$GPGGA,083552.00,3000.0000000,N,11900.0000000,E,1,08,1.0,0.000,M,100.000,M,,*57";
        assert!(bcc_checksum_compare_for_gga(gga));
    }

    #[test]
    fn bcc_checksum_invalid_no_dollar() {
        assert!(!bcc_checksum_compare_for_gga("GPGGA,083552*AA"));
    }

    #[test]
    fn bcc_checksum_invalid_no_star() {
        assert!(!bcc_checksum_compare_for_gga("$GPGGA,083552"));
    }

    #[test]
    fn bcc_checksum_wrong() {
        let gga = "$GPGGA,083552.00,3000.0000000,N,11900.0000000,E,1,08,1.0,0.000,M,100.000,M,,*00";
        assert!(!bcc_checksum_compare_for_gga(gga));
    }

    #[test]
    fn generate_gga_round_trip() {
        let gnss = GnssData {
            latitude: 48.1234,
            longitude: 11.5678,
            quality: 4,
            n_sat: 12,
            height: 520.0,
            ..Default::default()
        };
        let gga = generate_gga_frame(&gnss);
        assert!(gga.starts_with("$GPGGA,"));
        assert!(gga.contains("*"));
        assert!(bcc_checksum_compare_for_gga(gga.trim()));
    }
}
