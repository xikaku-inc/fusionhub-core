//! XREAL Air 2 Ultra IMU source node.
//!
//! Reads IMU data from an XREAL Air 2 Ultra headset via USB HID.
//! Opens the device with vendor 0x3318, product 0x0426, preferring
//! interface 2 (the IMU interface).  Sends a magic initialization
//! payload to start streaming, then reads 64-byte packets containing
//! 24-bit signed gyroscope and accelerometer values.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::task::JoinHandle;

use fusion_types::{ImuData, JsonValueExt, StreamableData, Vec3d};

use crate::node::{ConsumerCallback, Node, NodeBase};

// Xreal vendor ID and Air 2 Ultra product ID
const VENDOR_ID: u16 = 0x3318;
const PRODUCT_ID_AIR2_ULTRA: u16 = 0x0426;

// Preferred HID interface number for IMU data
const PREFERRED_INTERFACE: i32 = 2;

// Magic payload to start IMU streaming (from AirAPI_Windows)
const MAGIC_PAYLOAD: [u8; 10] = [0x00, 0xaa, 0xc5, 0xd1, 0x21, 0x42, 0x04, 0x00, 0x19, 0x01];

// Gyro scalar: value / 2^23 * 2000 -> deg/s
const GYRO_SCALAR: f64 = (1.0 / 8388608.0) * 2000.0;

// Accel scalar: value / 2^23 * 16 -> g
const ACCEL_SCALAR_G: f64 = (1.0 / 8388608.0) * 16.0;

/// Parse a 24-bit signed integer from a little-endian byte buffer at the
/// given offset.  Returns an `i32` with proper sign extension.
fn read_24bit_signed(data: &[u8], offset: usize) -> i32 {
    let b0 = data[offset] as u32;
    let b1 = (data[offset + 1] as u32) << 8;
    let b2 = (data[offset + 2] as u32) << 16;
    // Sign extension: if bit 23 is set, fill upper byte with 0xFF
    let sign = if data[offset + 2] & 0x80 != 0 {
        0xFF << 24
    } else {
        0x00
    };
    (sign | b2 | b1 | b0) as i32
}

/// XREAL Air 2 Ultra IMU source node.
///
/// Reads IMU data from an XREAL Air 2 Ultra headset via USB HID.
pub struct XrealAir2UltraImu {
    pub base: NodeBase,

    // Configuration
    m_id: String,
    m_period_seconds: f64,

    // Runtime
    m_done: Arc<AtomicBool>,
    m_worker_handle: Option<JoinHandle<()>>,
}

impl XrealAir2UltraImu {
    pub fn new(name: impl Into<String>, config: &serde_json::Value) -> Self {
        let name = name.into();

        let id = config.value_str("id", "xreal_air2ultra");

        let hz = config.value_f64("rateHz", 1000.0);
        let period_seconds = if hz > 0.0 { 1.0 / hz } else { 0.001 };

        log::info!(
            "Creating XrealAir2UltraImu '{}' id='{}' period={}s",
            name,
            id,
            period_seconds
        );

        Self {
            base: NodeBase::new(&name),
            m_id: id,
            m_period_seconds: period_seconds,
            m_done: Arc::new(AtomicBool::new(false)),
            m_worker_handle: None,
        }
    }
}

impl Node for XrealAir2UltraImu {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn start(&mut self) -> anyhow::Result<()> {
        log::info!("Starting XREAL Air 2 Ultra IMU source: {}", self.base.name());

        let done = self.m_done.clone();
        let consumers = self.base.consumers_arc();
        let enabled = self.base.enabled_arc();
        let id = self.m_id.clone();
        let node_name = self.base.name().to_string();
        let period_seconds = self.m_period_seconds;

        done.store(false, Ordering::Relaxed);

        self.m_worker_handle = Some(tokio::spawn(async move {
            let result = tokio::task::spawn_blocking(move || {
                worker_thread(done, consumers, enabled, id, node_name, period_seconds);
            })
            .await;

            if let Err(e) = result {
                log::warn!("XrealAir2Ultra worker thread panicked: {}", e);
            }
        }));

        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        log::info!("Stopping XREAL Air 2 Ultra IMU source: {}", self.base.name());
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

/// Worker thread function that handles device enumeration, opening,
/// initialization, and the packet read loop.
fn worker_thread(
    done: Arc<AtomicBool>,
    consumers: Arc<std::sync::Mutex<Vec<ConsumerCallback>>>,
    enabled: Arc<AtomicBool>,
    id: String,
    node_name: String,
    period_seconds: f64,
) {
    // 1. Initialize HID API
    let hid_api = match hidapi::HidApi::new() {
        Ok(api) => api,
        Err(e) => {
            log::warn!(
                "hidapi init failed ({}). {} node will remain inactive.",
                e,
                node_name
            );
            return;
        }
    };

    // 2. Enumerate and open device, preferring interface 2
    let device = {
        let mut found_path: Option<String> = None;
        let mut found_iface: i32 = -1;

        for info in hid_api.device_list() {
            if info.vendor_id() == VENDOR_ID && info.product_id() == PRODUCT_ID_AIR2_ULTRA {
                let iface = info.interface_number();
                let path = info.path().to_string_lossy().to_string();

                log::info!(
                    "XrealAir2Ultra: found HID interface {} path {}",
                    iface,
                    path
                );

                // Prefer interface 2 if present, otherwise take any
                if iface == PREFERRED_INTERFACE {
                    found_path = Some(path);
                    found_iface = iface;
                    break;
                } else if found_path.is_none() {
                    found_path = Some(path);
                    found_iface = iface;
                }
            }
        }

        if let Some(path) = found_path {
            log::info!(
                "XrealAir2Ultra: trying iface {} path {}",
                found_iface,
                path
            );
            match hid_api.open_path(&std::ffi::CString::new(path.clone()).unwrap_or_default()) {
                Ok(dev) => dev,
                Err(e) => {
                    log::warn!(
                        "XrealAir2Ultra: failed to open HID device at {}: {}",
                        path,
                        e
                    );
                    return;
                }
            }
        } else {
            log::warn!(
                "XrealAir2Ultra: no HID interface found for {:04x}:{:04x}. {} node will remain inactive.",
                VENDOR_ID,
                PRODUCT_ID_AIR2_ULTRA,
                node_name
            );
            return;
        }
    };

    // 3. Send magic initialization payload to start streaming
    match device.write(&MAGIC_PAYLOAD) {
        Ok(n) => {
            log::info!("XrealAir2Ultra: init payload written ({} bytes)", n);
        }
        Err(e) => {
            log::warn!("XrealAir2Ultra: init payload write failed ({})", e);
            return;
        }
    }

    // 4. Read loop
    let mut buf = [0u8; 64];
    let mut first_packet_logged = false;

    while !done.load(Ordering::Relaxed) {
        let read_len = match device.read_timeout(&mut buf, 20) {
            Ok(n) => n,
            Err(e) => {
                log::warn!("XrealAir2Ultra: hid_read_timeout error: {}", e);
                break;
            }
        };

        if read_len < 64 {
            // Short read or timeout; continue polling
            continue;
        }

        // Packet layout (from AirAPI_Windows parse_report):
        //   bytes  0.. 3: unused
        //   bytes  4..11: tick (uint64 little-endian, nanoseconds)
        //   bytes 12..17: skip 6 bytes
        //   bytes 18..26: gyro (3x 24-bit signed, little-endian)
        //   bytes 27..32: skip 6 bytes
        //   bytes 33..41: accel (3x 24-bit signed, little-endian)

        // Read tick (8 bytes little-endian starting at offset 4)
        let tick = u64::from_le_bytes([
            buf[4], buf[5], buf[6], buf[7], buf[8], buf[9], buf[10], buf[11],
        ]);

        // Parse gyroscope (3x 24-bit at offset 18)
        let gyr_raw_x = read_24bit_signed(&buf, 18);
        let gyr_raw_y = read_24bit_signed(&buf, 21);
        let gyr_raw_z = read_24bit_signed(&buf, 24);

        // Parse accelerometer (3x 24-bit at offset 33)
        let acc_raw_x = read_24bit_signed(&buf, 33);
        let acc_raw_y = read_24bit_signed(&buf, 36);
        let acc_raw_z = read_24bit_signed(&buf, 39);

        // Apply scaling
        let gyro = Vec3d::new(
            gyr_raw_x as f64 * GYRO_SCALAR,
            gyr_raw_y as f64 * GYRO_SCALAR,
            gyr_raw_z as f64 * GYRO_SCALAR,
        );
        let acc = Vec3d::new(
            acc_raw_x as f64 * ACCEL_SCALAR_G,
            acc_raw_y as f64 * ACCEL_SCALAR_G,
            acc_raw_z as f64 * ACCEL_SCALAR_G,
        );

        // Axis mapping to match FusionHub conventions: (-x, z, y)
        let acc_out = Vec3d::new(-acc.x, acc.z, acc.y);
        let gyro_out = Vec3d::new(-gyro.x, gyro.z, gyro.y);

        if !first_packet_logged {
            log::info!(
                "XrealAir2Ultra: first packet size={} tick_ns={}",
                read_len,
                tick
            );
            first_packet_logged = true;
        }

        if !enabled.load(Ordering::Relaxed) {
            continue;
        }

        let out_data = ImuData {
            sender_id: id.clone(),
            timestamp: std::time::SystemTime::now(),
            latency: 0.0,
            gyroscope: gyro_out,       // deg/s
            accelerometer: acc_out,    // g
            quaternion: fusion_types::Quatd::identity(),
            euler: Vec3d::zeros(),
            period: period_seconds,
            internal_frame_count: 0,
            linear_velocity: Vec3d::zeros(),
        };

        let cbs = consumers.lock().unwrap();
        let streamable = StreamableData::Imu(out_data);
        for cb in cbs.iter() {
            cb(streamable.clone());
        }
    }

    log::info!("XrealAir2Ultra worker thread exiting");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_24bit_signed_positive() {
        // 0x000100 = 256
        let data = [0x00, 0x01, 0x00];
        assert_eq!(read_24bit_signed(&data, 0), 256);
    }

    #[test]
    fn read_24bit_signed_negative() {
        // 0xFFFFFF = -1
        let data = [0xFF, 0xFF, 0xFF];
        assert_eq!(read_24bit_signed(&data, 0), -1);
    }

    #[test]
    fn read_24bit_signed_max_positive() {
        // 0x7FFFFF = 8388607
        let data = [0xFF, 0xFF, 0x7F];
        assert_eq!(read_24bit_signed(&data, 0), 8388607);
    }

    #[test]
    fn read_24bit_signed_min_negative() {
        // 0x800000 = -8388608
        let data = [0x00, 0x00, 0x80];
        assert_eq!(read_24bit_signed(&data, 0), -8388608);
    }

    #[test]
    fn read_24bit_signed_zero() {
        let data = [0x00, 0x00, 0x00];
        assert_eq!(read_24bit_signed(&data, 0), 0);
    }

    #[test]
    fn read_24bit_signed_with_offset() {
        // Place a value at offset 3
        let data = [0x00, 0x00, 0x00, 0x01, 0x02, 0x03];
        // 0x030201 = 197121
        assert_eq!(read_24bit_signed(&data, 3), 0x030201);
    }

    #[test]
    fn read_24bit_signed_negative_small() {
        // -256 = 0xFFFF00
        let data = [0x00, 0xFF, 0xFF];
        assert_eq!(read_24bit_signed(&data, 0), -256);
    }

    #[test]
    fn scaling_gyro() {
        // Full-scale positive: 8388607 * GYRO_SCALAR ~ 2000 deg/s
        let val = 8388607_f64 * GYRO_SCALAR;
        assert!((val - 2000.0).abs() < 0.001);
    }

    #[test]
    fn scaling_accel() {
        // Full-scale positive: 8388607 * ACCEL_SCALAR_G ~ 16 g
        let val = 8388607_f64 * ACCEL_SCALAR_G;
        assert!((val - 16.0).abs() < 0.001);
    }

    #[test]
    fn axis_remapping() {
        // Test the (-x, z, y) axis mapping
        let acc = Vec3d::new(1.0, 2.0, 3.0);
        let remapped = Vec3d::new(-acc.x, acc.z, acc.y);
        assert!((remapped.x - (-1.0)).abs() < 1e-10);
        assert!((remapped.y - 3.0).abs() < 1e-10);
        assert!((remapped.z - 2.0).abs() < 1e-10);
    }

    #[test]
    fn construct_default_config() {
        let config = serde_json::json!({});
        let source = XrealAir2UltraImu::new("test_xreal", &config);
        assert_eq!(source.name(), "test_xreal");
        assert_eq!(source.m_id, "xreal_air2ultra");
        assert!((source.m_period_seconds - 0.001).abs() < 1e-10);
    }

    #[test]
    fn construct_custom_config() {
        let config = serde_json::json!({
            "id": "my_xreal",
            "rateHz": 500.0
        });
        let source = XrealAir2UltraImu::new("test_xreal", &config);
        assert_eq!(source.m_id, "my_xreal");
        assert!((source.m_period_seconds - 0.002).abs() < 1e-10);
    }

    #[test]
    fn node_enabled_default() {
        let config = serde_json::json!({});
        let source = XrealAir2UltraImu::new("test_xreal", &config);
        assert!(source.is_enabled());
    }

    #[test]
    fn node_enable_disable() {
        let config = serde_json::json!({});
        let mut source = XrealAir2UltraImu::new("test_xreal", &config);
        source.set_enabled(false);
        assert!(!source.is_enabled());
        source.set_enabled(true);
        assert!(source.is_enabled());
    }

    #[test]
    fn parse_24bit_matches_cpp_behavior() {
        // Verify that our Rust parse24 matches the C++ lambda behavior.
        // C++ lambda:
        //   t0 = (*(pp + 2) & 0x80) ? (0xff << 24) : 0x00;
        //   t3 = *(pp++);
        //   t1 = (*(pp++) << 8);
        //   t2 = (*(pp++) << 16);
        //   return (int32_t)(t0 | t1 | t2 | t3);

        // Test case: bytes [0xAB, 0xCD, 0xEF]
        // t0 = (0xEF & 0x80) ? 0xFF000000 : 0x00 = 0xFF000000 (negative)
        // t3 = 0xAB
        // t1 = 0xCD00
        // t2 = 0xEF0000
        // result = 0xFF000000 | 0xEF0000 | 0xCD00 | 0xAB = 0xFFEFCDAB
        let data = [0xAB, 0xCD, 0xEF];
        let expected: i32 = 0xFFEFCDABu32 as i32;
        assert_eq!(read_24bit_signed(&data, 0), expected);

        // Test case: bytes [0x12, 0x34, 0x56]
        // t0 = (0x56 & 0x80) ? 0xFF000000 : 0x00 = 0x00 (positive)
        // result = 0x00 | 0x560000 | 0x3400 | 0x12 = 0x00563412
        let data = [0x12, 0x34, 0x56];
        let expected: i32 = 0x00563412;
        assert_eq!(read_24bit_signed(&data, 0), expected);
    }
}
