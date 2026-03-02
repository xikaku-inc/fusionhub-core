//! HID IMU source node.
//!
//! Reads IMU data from USB HID devices such as the JVC VS1W, HTC Vive
//! tracker, or XREAL headsets.  Uses the `hidapi` crate to enumerate and
//! communicate with devices.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use tokio::task::JoinHandle;

use fusion_types::{ImuData, JsonValueExt, StreamableData, Vec3d};

use crate::node::{ConsumerCallback, Node, NodeBase};

// ---------------------------------------------------------------------------
// Axis remapping configuration
// ---------------------------------------------------------------------------

/// Per-axis assignment and direction.
///
/// An axis configuration string like `"+X+Y+Z"` or `"-Z+X+Y"` encodes three
/// pairs of (sign, source axis).  The sign determines direction, the letter
/// determines which source component (0=X, 1=Y, 2=Z) feeds the output axis.
#[derive(Clone, Debug)]
struct AxisConfig {
    axis_assignment: [usize; 3],
    axis_direction: [f64; 3],
}

impl Default for AxisConfig {
    fn default() -> Self {
        Self {
            axis_assignment: [0, 1, 2],
            axis_direction: [1.0, 1.0, 1.0],
        }
    }
}

impl AxisConfig {
    fn parse(s: &str) -> Self {
        let bytes = s.as_bytes();
        let mut cfg = AxisConfig::default();
        // Expected format: "+X+Y+Z" or "-Z+X+Y" (6 chars)
        if bytes.len() >= 6 {
            for p in 0..3 {
                cfg.axis_direction[p] = if bytes[2 * p] == b'+' { 1.0 } else { -1.0 };
                cfg.axis_assignment[p] = match bytes[2 * p + 1] {
                    b'X' | b'x' => 0,
                    b'Y' | b'y' => 1,
                    b'Z' | b'z' => 2,
                    _ => p, // fallback identity
                };
            }
        }
        cfg
    }

    fn convert(&self, v: Vec3d) -> Vec3d {
        Vec3d::new(
            self.axis_direction[0] * v[self.axis_assignment[0]],
            self.axis_direction[1] * v[self.axis_assignment[1]],
            self.axis_direction[2] * v[self.axis_assignment[2]],
        )
    }
}

// ---------------------------------------------------------------------------
// Sample buffering (3 samples per USB message)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Sample {
    acc: Vec3d,
    gyro: Vec3d,
    timestamp: i64,
}

#[derive(Clone, Debug)]
struct SampleBuffer {
    samples: [Sample; 3],
    seq: [i32; 3],
    used: [bool; 3],
}

/// Find the index into `seq` that points to the entry immediately after
/// `last_seq`, treating values as 8-bit counters that wrap from 255 to 0.
/// Returns `None` if all three slots have been consumed.
fn next_index(seq: &[i32; 3], last_seq: i32) -> Option<usize> {
    let mut diffs = [0i32; 3];
    for i in 0..3 {
        diffs[i] = ((seq[i] - last_seq) as u8) as i32;
    }
    let mut min_val = i32::MAX;
    let mut min_idx: Option<usize> = None;
    for i in 0..3 {
        // Only consider forward steps within a reasonable range (1..=128).
        // Values above 128 are actually backward steps in 8-bit wrapping.
        if diffs[i] > 0 && diffs[i] <= 128 && diffs[i] < min_val {
            min_val = diffs[i];
            min_idx = Some(i);
        }
    }
    min_idx
}

/// Stateful sample buffer consumer that tracks sequence numbers and
/// reconstructs full 48-bit timestamps from 32-bit per-sample values.
struct SampleBufferState {
    buffer: Option<SampleBuffer>,
    last_time: Option<i64>,
    last_seq: i32,
}

impl SampleBufferState {
    fn new() -> Self {
        Self {
            buffer: None,
            last_time: None,
            last_seq: 0,
        }
    }

    fn set_buffer(&mut self, buf: SampleBuffer) {
        self.buffer = Some(buf);
    }

    /// Pull the next sample from the buffer in sequence order.
    fn next_sample(&mut self) -> Option<Sample> {
        let buf = self.buffer.as_mut()?;

        let idx = if self.last_time.is_none() {
            // First time: find smallest sequence number.
            let mut i_smallest = 0usize;
            if ((buf.seq[1] - buf.seq[0]) as i8) < 0 {
                i_smallest = 1;
            }
            if ((buf.seq[2] - buf.seq[i_smallest as usize]) as i8) < 0 {
                i_smallest = 2;
            }
            self.last_seq = buf.seq[i_smallest];
            buf.used[i_smallest] = true;
            i_smallest
        } else {
            let idx = next_index(&buf.seq, self.last_seq)?;
            self.last_seq = buf.seq[idx];
            // Mark all samples that are at or before last_seq as used.
            for i in 0..3 {
                if ((self.last_seq - buf.seq[i]) as u8) <= 128 {
                    buf.used[i] = true;
                }
            }
            idx
        };

        let mut sample = buf.samples[idx].clone();

        // Reconstruct full timestamp from 32-bit value.
        if let Some(lt) = self.last_time {
            let mut current = (lt & !0xFFFF_FFFF_i64) | (sample.timestamp & 0xFFFF_FFFF);
            if current < lt {
                current += 1i64 << 32;
            }
            self.last_time = Some(current);
        } else {
            self.last_time = Some(sample.timestamp);
        }
        sample.timestamp = self.last_time.unwrap();

        // If all samples consumed, drop the buffer.
        if buf.used[0] && buf.used[1] && buf.used[2] {
            self.buffer = None;
        }

        Some(sample)
    }
}

// ---------------------------------------------------------------------------
// HidImuSource
// ---------------------------------------------------------------------------

/// HID IMU source node.
///
/// Enumerates USB HID devices by vendor/product ID, opens the matching device,
/// reads 64-byte IMU reports containing 3 packed samples each, and emits
/// `ImuData` to downstream consumers.
pub struct HidImuSource {
    pub base: NodeBase,

    // Configuration ---------------------------------------------------------
    m_vendor_id: u16,
    m_product_id: u16,
    m_id: String,
    m_axis_config_acc: AxisConfig,
    m_axis_config_gyro: AxisConfig,

    // Runtime ---------------------------------------------------------------
    m_done: Arc<AtomicBool>,
    m_worker_handle: Option<JoinHandle<()>>,
}

/// Parse a hexadecimal string like `"0x0BB4"` or `"0BB4"` into a `u16`.
fn parse_hex_u16(s: &str) -> u16 {
    let s = s.trim().trim_start_matches("0x").trim_start_matches("0X");
    u16::from_str_radix(s, 16).unwrap_or(0)
}

impl HidImuSource {
    pub fn new(name: impl Into<String>, config: &serde_json::Value) -> Self {
        let name = name.into();

        // Vendor / product IDs -  accept both numeric and hex string forms.
        let vendor_id = if let Some(v) = config.get("vendorId") {
            if let Some(n) = v.as_u64() {
                n as u16
            } else if let Some(s) = v.as_str() {
                parse_hex_u16(s)
            } else {
                0x28DE // Vive default
            }
        } else {
            0x28DE
        };

        let product_id = if let Some(v) = config.get("productId") {
            if let Some(n) = v.as_u64() {
                n as u16
            } else if let Some(s) = v.as_str() {
                parse_hex_u16(s)
            } else {
                0x2300 // Vive default
            }
        } else {
            0x2300
        };

        let id = config.value_str("id", "imu");

        let axis_config_acc =
            AxisConfig::parse(&config.value_str("axisConfigAcc", "+X+Y+Z"));
        let axis_config_gyro =
            AxisConfig::parse(&config.value_str("axisConfigGyro", "+X+Y+Z"));

        log::info!(
            "Creating HidImuSource '{}' id='{}' vendor=0x{:04X} product=0x{:04X}",
            name,
            id,
            vendor_id,
            product_id
        );

        Self {
            base: NodeBase::new(&name),
            m_vendor_id: vendor_id,
            m_product_id: product_id,
            m_id: id,
            m_axis_config_acc: axis_config_acc,
            m_axis_config_gyro: axis_config_gyro,
            m_done: Arc::new(AtomicBool::new(false)),
            m_worker_handle: None,
        }
    }
}

impl Node for HidImuSource {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn start(&mut self) -> anyhow::Result<()> {
        log::info!("Starting HID IMU source: {}", self.base.name());

        let done = self.m_done.clone();
        let consumers = self.base.consumers_arc();
        let enabled = self.base.enabled_arc();
        let vendor_id = self.m_vendor_id;
        let product_id = self.m_product_id;
        let id = self.m_id.clone();
        let node_name = self.base.name().to_string();
        let axis_config_acc = self.m_axis_config_acc.clone();
        let axis_config_gyro = self.m_axis_config_gyro.clone();

        done.store(false, Ordering::Relaxed);

        self.m_worker_handle = Some(tokio::spawn(async move {
            let result = tokio::task::spawn_blocking(move || {
                // Outer loop handles reconnection.
                while !done.load(Ordering::Relaxed) {
                    // 1. Initialise HID API ----------------------------------
                    match hidapi::HidApi::new() {
                        Ok(_) => {}
                        Err(e) => {
                            log::warn!(
                                "hidapi init failed ({}). {} node will remain inactive.",
                                e,
                                node_name
                            );
                            return;
                        }
                    };

                    // 2. Enumerate and open device ---------------------------
                    let device = loop {
                        if done.load(Ordering::Relaxed) {
                            return;
                        }

                        // Refresh enumeration each attempt.
                        let hid_api = match hidapi::HidApi::new() {
                            Ok(api) => api,
                            Err(_) => {
                                std::thread::sleep(Duration::from_millis(500));
                                continue;
                            }
                        };

                        let mut found_path: Option<String> = None;
                        for info in hid_api.device_list() {
                            if info.vendor_id() == vendor_id
                                && info.product_id() == product_id
                            {
                                // Prefer entries whose product string is "IMU" when available,
                                // but fall back to the first match.
                                let is_imu = info
                                    .product_string()
                                    .map(|s| s == "IMU")
                                    .unwrap_or(false);
                                if is_imu || found_path.is_none() {
                                    found_path = Some(info.path().to_string_lossy().to_string());
                                    if is_imu {
                                        break;
                                    }
                                }
                            }
                        }

                        if let Some(path) = found_path {
                            match hid_api.open_path(&std::ffi::CString::new(path.clone()).unwrap_or_default()) {
                                Ok(dev) => break dev,
                                Err(e) => {
                                    log::warn!("Unable to open HID device at {}: {}", path, e);
                                    std::thread::sleep(Duration::from_millis(500));
                                    continue;
                                }
                            }
                        } else {
                            // Device not found yet; wait and retry.
                            log::info!(
                                "HID IMU device {:04X}:{:04X} not found, retrying...",
                                vendor_id,
                                product_id
                            );
                            std::thread::sleep(Duration::from_millis(500));
                        }
                    };

                    log::info!(
                        "HID device {:04X}:{:04X} opened",
                        vendor_id,
                        product_id
                    );

                    // Try to read IMU scale reports from the device.  For Vive-
                    // style devices these come from feature reports 0x01 / 0x10 /
                    // 0x11 but not all devices support them.  Fall back to
                    // sensible defaults if the reports are unavailable.
                    let mut raw_acc_scale: f64 = 4.0 / 32768.0; // 4g range, 16 bit
                    let mut raw_gyro_scale: f64 =
                        500.0 / 32768.0 / 180.0 * std::f64::consts::PI; // 500 deg/s, rad/s

                    // Attempt to read scale report (Vive protocol).
                    let mut scale_buf = [0u8; 256];
                    scale_buf[0] = 0x01; // VIVE_REPORT_IMU_SCALES
                    if let Ok(n) = device.get_feature_report(&mut scale_buf) {
                        if n >= 3 {
                            let gyro_mode = scale_buf[1] as usize;
                            let acc_mode = scale_buf[2] as usize;
                            if gyro_mode < 4 && acc_mode < 4 {
                                let gyro_scales = [250.0, 500.0, 1000.0, 2000.0];
                                let acc_scales = [2.0, 4.0, 8.0, 16.0];
                                raw_acc_scale = acc_scales[acc_mode] / 32768.0;
                                raw_gyro_scale =
                                    gyro_scales[gyro_mode] / 32768.0 / 180.0 * std::f64::consts::PI;
                                log::info!(
                                    "IMU scales from device: acc={} gyro={}",
                                    raw_acc_scale,
                                    raw_gyro_scale
                                );
                            }
                        }
                    }

                    // Bias / scale vectors.  If the device provides a Vive-style
                    // compressed JSON config these would be populated from there.
                    // For now, identity scale and zero bias.
                    let acc_bias = Vec3d::zeros();
                    let gyro_bias = Vec3d::zeros();
                    let acc_scale = Vec3d::new(1.0, 1.0, 1.0);
                    let gyro_scale = Vec3d::new(1.0, 1.0, 1.0);

                    // Optionally send magic "start streaming" feature reports
                    // (Vive protocol).  Errors are non-fatal.
                    {
                        let start1 = [0x04u8, 0x00];
                        let _ = device.send_feature_report(&start1);
                        let start2 = [0x07u8, 0x05];
                        let _ = device.send_feature_report(&start2);
                    }

                    log::info!("Done configuring HID IMU node");

                    // 3. Read loop -------------------------------------------
                    let mut buf = [0u8; 256];
                    let mut sample_state = SampleBufferState::new();
                    let mut last_data_time = std::time::Instant::now();
                    let mut first_data_received = false;

                    // Set a read timeout so we can check the done flag periodically.
                    let _ = device.set_blocking_mode(false);

                    while !done.load(Ordering::Relaxed) {
                        let read_len = match device.read_timeout(&mut buf, 100) {
                            Ok(n) => n,
                            Err(e) => {
                                log::warn!("HID read error: {}", e);
                                break; // will reconnect
                            }
                        };

                        if read_len == 0 {
                            // Timeout, check for connection loss.
                            if first_data_received
                                && last_data_time.elapsed() > Duration::from_secs(3)
                            {
                                log::warn!("HID IMU data timeout, reconnecting");
                                break;
                            }
                            continue;
                        }

                        let now_instant = std::time::Instant::now();

                        // JVC / Vive report: report ID 0x20, at least 52 bytes
                        // containing 3 packed samples of 17 bytes each.
                        if read_len >= 52 && buf[0] == 0x20 {
                            let default_sample = Sample {
                                acc: Vec3d::zeros(),
                                gyro: Vec3d::zeros(),
                                timestamp: 0,
                            };
                            let mut sb = SampleBuffer {
                                samples: [
                                    default_sample.clone(),
                                    default_sample.clone(),
                                    default_sample,
                                ],
                                seq: [0; 3],
                                used: [false; 3],
                            };

                            let mut p = 1usize; // skip report ID byte
                            for i in 0..3 {
                                if p + 17 > read_len {
                                    break;
                                }

                                let acc_x =
                                    i16::from_le_bytes([buf[p], buf[p + 1]]) as f64;
                                let acc_y =
                                    i16::from_le_bytes([buf[p + 2], buf[p + 3]]) as f64;
                                let acc_z =
                                    i16::from_le_bytes([buf[p + 4], buf[p + 5]]) as f64;
                                let mut acc = Vec3d::new(acc_x, acc_y, acc_z) * raw_acc_scale;

                                let gyr_x =
                                    i16::from_le_bytes([buf[p + 6], buf[p + 7]]) as f64;
                                let gyr_y =
                                    i16::from_le_bytes([buf[p + 8], buf[p + 9]]) as f64;
                                let gyr_z =
                                    i16::from_le_bytes([buf[p + 10], buf[p + 11]]) as f64;
                                let mut gyro = Vec3d::new(gyr_x, gyr_y, gyr_z) * raw_gyro_scale;

                                let timestamp = u32::from_le_bytes([
                                    buf[p + 12],
                                    buf[p + 13],
                                    buf[p + 14],
                                    buf[p + 15],
                                ]) as i64;

                                let seq = buf[p + 16] as i32;

                                // Apply bias and per-axis scale.
                                acc -= acc_bias;
                                gyro -= gyro_bias;
                                acc = acc.component_mul(&acc_scale);
                                gyro = gyro.component_mul(&gyro_scale);

                                sb.samples[i] = Sample {
                                    acc,
                                    gyro,
                                    timestamp,
                                };
                                sb.seq[i] = seq;
                                sb.used[i] = false;

                                p += 17;
                            }

                            sample_state.set_buffer(sb);

                            // Pull first available sample from the buffer.
                            if let Some(sample) = sample_state.next_sample() {
                                if !enabled.load(Ordering::Relaxed) {
                                    first_data_received = true;
                                    last_data_time = now_instant;
                                    continue;
                                }

                                // Apply axis configuration.
                                let acc_out = axis_config_acc.convert(sample.acc);
                                // The C++ source converts gyro from rad/s to deg/s
                                // because ImuData.gyroscope is defined in deg/s.
                                let gyro_out = axis_config_gyro.convert(sample.gyro)
                                    * (180.0 / std::f64::consts::PI);

                                let out_data = ImuData {
                                    sender_id: id.clone(),
                                    timestamp: SystemTime::now(),
                                    latency: 0.0,
                                    gyroscope: gyro_out,
                                    accelerometer: acc_out,
                                    quaternion: fusion_types::Quatd::identity(),
                                    euler: Vec3d::zeros(),
                                    period: 0.0,
                                    internal_frame_count: 0,
                                    linear_velocity: Vec3d::zeros(),
                                };

                                let cbs = consumers.lock().unwrap();
                                let streamable = StreamableData::Imu(out_data);
                                for cb in cbs.iter() {
                                    cb(streamable.clone());
                                }
                            }

                            first_data_received = true;
                            last_data_time = now_instant;
                        }
                        // Fallback for simple (non-Vive) HID IMU reports:
                        // 64-byte report with report ID != 0x20 and raw 6-axis
                        // data starting at byte 4.
                        else if read_len >= 16 && buf[0] != 0x20 {
                            if !enabled.load(Ordering::Relaxed) {
                                first_data_received = true;
                                last_data_time = now_instant;
                                continue;
                            }

                            let acc_x =
                                i16::from_le_bytes([buf[4], buf[5]]) as f64
                                    / 16384.0
                                    * 9.81;
                            let acc_y =
                                i16::from_le_bytes([buf[6], buf[7]]) as f64
                                    / 16384.0
                                    * 9.81;
                            let acc_z =
                                i16::from_le_bytes([buf[8], buf[9]]) as f64
                                    / 16384.0
                                    * 9.81;
                            let gyr_x =
                                i16::from_le_bytes([buf[10], buf[11]]) as f64
                                    / 131.0; // deg/s
                            let gyr_y =
                                i16::from_le_bytes([buf[12], buf[13]]) as f64
                                    / 131.0;
                            let gyr_z =
                                i16::from_le_bytes([buf[14], buf[15]]) as f64
                                    / 131.0;

                            let acc = Vec3d::new(acc_x, acc_y, acc_z);
                            let gyro = Vec3d::new(gyr_x, gyr_y, gyr_z);

                            let acc_out = axis_config_acc.convert(acc);
                            let gyro_out = axis_config_gyro.convert(gyro);

                            let out_data = ImuData {
                                sender_id: id.clone(),
                                timestamp: SystemTime::now(),
                                latency: 0.0,
                                gyroscope: gyro_out,
                                accelerometer: acc_out,
                                quaternion: fusion_types::Quatd::identity(),
                                euler: Vec3d::zeros(),
                                period: 0.0,
                                internal_frame_count: 0,
                                linear_velocity: Vec3d::zeros(),
                            };

                            let cbs = consumers.lock().unwrap();
                            let streamable = StreamableData::Imu(out_data);
                            for cb in cbs.iter() {
                                cb(streamable.clone());
                            }

                            first_data_received = true;
                            last_data_time = now_instant;
                        }

                        // Check for data timeout triggering reconnection.
                        if first_data_received
                            && last_data_time.elapsed() > Duration::from_secs(3)
                        {
                            log::warn!("HID IMU data timeout, reconnecting");
                            break;
                        }
                    }

                    // Brief pause before attempting reconnection.
                    if !done.load(Ordering::Relaxed) {
                        std::thread::sleep(Duration::from_millis(500));
                    }
                }

                log::info!("HID IMU worker thread exiting");
            })
            .await;

            if let Err(e) = result {
                log::warn!("HID IMU worker thread panicked: {}", e);
            }
        }));

        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        log::info!("Stopping HID IMU source: {}", self.base.name());
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

    fn receive_data(&mut self, _data: StreamableData) {
        // Source node does not consume upstream data.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axis_config_parse_identity() {
        let cfg = AxisConfig::parse("+X+Y+Z");
        assert_eq!(cfg.axis_assignment, [0, 1, 2]);
        assert_eq!(cfg.axis_direction, [1.0, 1.0, 1.0]);
    }

    #[test]
    fn axis_config_parse_remap() {
        let cfg = AxisConfig::parse("-Z+X+Y");
        assert_eq!(cfg.axis_assignment, [2, 0, 1]);
        assert_eq!(cfg.axis_direction, [-1.0, 1.0, 1.0]);
    }

    #[test]
    fn axis_config_convert() {
        let cfg = AxisConfig::parse("-Z+X+Y");
        let v = Vec3d::new(1.0, 2.0, 3.0);
        let out = cfg.convert(v);
        // output[0] = -1 * v[2] = -3
        // output[1] = +1 * v[0] = 1
        // output[2] = +1 * v[1] = 2
        assert!((out.x - (-3.0)).abs() < 1e-10);
        assert!((out.y - 1.0).abs() < 1e-10);
        assert!((out.z - 2.0).abs() < 1e-10);
    }

    #[test]
    fn next_index_basic() {
        let seq = [10, 11, 12];
        assert_eq!(next_index(&seq, 9), Some(0));
        assert_eq!(next_index(&seq, 10), Some(1));
        assert_eq!(next_index(&seq, 11), Some(2));
        assert_eq!(next_index(&seq, 12), None);
    }

    #[test]
    fn next_index_wraparound() {
        let seq = [254, 255, 0];
        assert_eq!(next_index(&seq, 253), Some(0));
        assert_eq!(next_index(&seq, 255), Some(2)); // 0 follows 255
    }

    #[test]
    fn parse_hex_u16_values() {
        assert_eq!(parse_hex_u16("0x0BB4"), 0x0BB4);
        assert_eq!(parse_hex_u16("0BB4"), 0x0BB4);
        assert_eq!(parse_hex_u16("0x0306"), 0x0306);
        assert_eq!(parse_hex_u16("28DE"), 0x28DE);
    }

    #[test]
    fn hid_imu_source_construct() {
        let config = serde_json::json!({
            "vendorId": "0x0BB4",
            "productId": "0x0306",
            "id": "testImu"
        });
        let source = HidImuSource::new("test_hid", &config);
        assert_eq!(source.name(), "test_hid");
        assert_eq!(source.m_vendor_id, 0x0BB4);
        assert_eq!(source.m_product_id, 0x0306);
        assert_eq!(source.m_id, "testImu");
    }
}
