//! ImuP source node — dual-antenna RTK GPS + IMU via OpenZen.
//!
//! Wraps an LPMS-IG1P sensor that provides both IMU and GNSS data.
//! Discovers up to two sensors matching config-specified names, obtains their
//! IMU and GNSS components, and emits both `StreamableData::Imu` and
//! `StreamableData::Gnss` through the single output callback.
//!
//! Optionally subscribes to RTCM corrections and forwards them to the GNSS
//! components.  Includes a `DualGnssCombiner` that computes orientation from
//! two GPS antenna positions using the haversine formula.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::SystemTime;

use nalgebra::{UnitQuaternion, Vector3};
use tokio::task::JoinHandle;

use fusion_registry::{sf, SettingsField};
use fusion_types::{GnssData, ImuData, JsonValueExt, Quatd, StreamableData, Vec3d};
use openzen_sys::*;
use serde_json::json;

use crate::node::{ConsumerCallback, Node, NodeBase};

pub fn settings_schema() -> Vec<SettingsField> {
    vec![
        sf("id", "Sensor ID", "string", json!("1")),
    ]
}

// ---------------------------------------------------------------------------
// GNSS carrier-phase solution constants (from OpenZen ZenTypes.h)
// ---------------------------------------------------------------------------
const ZEN_GNSS_FIX_CARRIER_PHASE_SOLUTION_NONE: i32 = 0;
const ZEN_GNSS_FIX_CARRIER_PHASE_SOLUTION_FLOAT: i32 = 1;
const ZEN_GNSS_FIX_CARRIER_PHASE_SOLUTION_FIXED: i32 = 2;

// ---------------------------------------------------------------------------
// DualGnssCombiner
// ---------------------------------------------------------------------------

/// Computes orientation from two GPS antenna positions.
///
/// When both antennas have valid data, the combiner calculates a quaternion
/// representing the relative heading and pitch derived from the difference
/// in latitude and longitude between the two antennas.
struct DualGnssCombiner {
    last_gnss: [(f64, f64); 2], // (latitude, longitude) per antenna
    orientation: Quatd,
}

impl DualGnssCombiner {
    fn new() -> Self {
        Self {
            last_gnss: [(0.0, 0.0); 2],
            orientation: Quatd::identity(),
        }
    }

    fn update(&mut self, sensor_idx: usize, lat: f64, lon: f64) -> Quatd {
        self.last_gnss[sensor_idx] = (lat, lon);
        if self.last_gnss[0].0 != 0.0 && self.last_gnss[1].0 != 0.0 {
            self.calc_rtk_orientation();
        }
        self.orientation
    }

    fn calc_rtk_orientation(&mut self) {
        let (lat1, lon1) = self.last_gnss[0];
        let (lat2, lon2) = self.last_gnss[1];

        let d = distance_between_coordinates(lon1, lat1, lon2, lat2);
        log::info!("Distance = {}m", d);

        self.orientation = gps_quaternion(lon1, lat1, lon2, lat2);
    }
}

/// Haversine distance in metres between two WGS-84 coordinates.
fn distance_between_coordinates(
    lon1_deg: f64,
    lat1_deg: f64,
    lon2_deg: f64,
    lat2_deg: f64,
) -> f64 {
    let lon1 = lon1_deg.to_radians();
    let lat1 = lat1_deg.to_radians();
    let lon2 = lon2_deg.to_radians();
    let lat2 = lat2_deg.to_radians();

    let sdlat = (0.5 * (lat2 - lat1)).sin();
    let sdlon = (0.5 * (lon2 - lon1)).sin();
    let angle = 2.0 * (sdlat * sdlat + lat1.cos() * lat2.cos() * sdlon * sdlon).sqrt().asin();
    let r = 6_371_000.0; // Earth radius in metres
    angle * r
}

/// Quaternion encoding the heading/pitch offset between two antenna positions.
///
/// Matches the C++ `gpsQuaternion()` helper: yaw from delta-longitude around Z,
/// pitch from delta-latitude around Y.
fn gps_quaternion(lon1_deg: f64, lat1_deg: f64, lon2_deg: f64, lat2_deg: f64) -> Quatd {
    let d_lat_rad = (lat2_deg - lat1_deg).to_radians();
    let d_lon_rad = (lon2_deg - lon1_deg).to_radians();

    let yaw = UnitQuaternion::from_axis_angle(&Vector3::z_axis(), d_lon_rad);
    let pitch = UnitQuaternion::from_axis_angle(&Vector3::y_axis(), d_lat_rad);

    yaw * pitch
}

// ---------------------------------------------------------------------------
// ImuPSource
// ---------------------------------------------------------------------------

/// OpenZen-based IMU + GNSS dual-endpoint source node.
///
/// Discovers and connects to LP-Research IG1P sensors, streaming both IMU and
/// GNSS data through the node output callback.
pub struct ImuPSource {
    pub base: NodeBase,

    // Configuration
    m_imu_endpoint: String,
    m_gnss_endpoint: String,
    m_sensor_names: [String; 2],
    m_rtcm_enabled: bool,
    m_id: String,

    // Runtime
    m_running: Arc<AtomicBool>,
    m_worker_handle: Option<JoinHandle<()>>,
}

impl ImuPSource {
    pub fn new(name: impl Into<String>, settings: &serde_json::Value, id: &str) -> Self {
        let name = name.into();

        let sensor1_name = settings
            .get("sensor1")
            .map(|s| s.value_str("name", ""))
            .unwrap_or_default()
            .to_lowercase();

        let sensor2_name = settings
            .get("sensor2")
            .map(|s| s.value_str("name", ""))
            .unwrap_or_default()
            .to_lowercase();

        let imu_endpoint = settings.value_str("imuEndpoint", "tcp://*:8802");

        let gnss_endpoint = format!("inproc://gnss_data_source_{}", id);

        let rtcm_enabled = settings.value_bool("rtcm", false);

        log::info!(
            "Creating ImuPSource '{}' id='{}' sensor1='{}' sensor2='{}' imuEp='{}' gnssEp='{}' rtcm={}",
            name, id, sensor1_name, sensor2_name, imu_endpoint, gnss_endpoint, rtcm_enabled
        );

        Self {
            base: NodeBase::new(&name),
            m_imu_endpoint: imu_endpoint,
            m_gnss_endpoint: gnss_endpoint,
            m_sensor_names: [sensor1_name, sensor2_name],
            m_rtcm_enabled: rtcm_enabled,
            m_id: id.to_string(),
            m_running: Arc::new(AtomicBool::new(false)),
            m_worker_handle: None,
        }
    }

    pub fn get_imu_endpoint(&self) -> &str {
        &self.m_imu_endpoint
    }

    pub fn get_gnss_endpoint(&self) -> &str {
        &self.m_gnss_endpoint
    }
}

impl Node for ImuPSource {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn start(&mut self) -> anyhow::Result<()> {
        log::info!("Starting ImuP source: {}", self.base.name());

        let done = self.m_running.clone();
        let consumers = self.base.consumers_arc();
        let enabled = self.base.enabled_arc();
        let sensor_names = self.m_sensor_names.clone();
        let id = self.m_id.clone();

        done.store(true, Ordering::Relaxed);

        self.m_worker_handle = Some(tokio::spawn(async move {
            let result = tokio::task::spawn_blocking(move || {
                // 1. Initialize client -------------------------------------------
                let mut client_handle = ZenClientHandle_t { handle: 0 };
                let err = unsafe { ZenInit(&mut client_handle) };
                if err != ZenError_None {
                    log::error!("ZenInit failed with error {}", err);
                    return;
                }

                struct ClientGuard {
                    handle: ZenClientHandle_t,
                }
                impl Drop for ClientGuard {
                    fn drop(&mut self) {
                        unsafe { ZenShutdown(self.handle) };
                    }
                }
                let _guard = ClientGuard {
                    handle: client_handle,
                };

                // 2. Discover sensors --------------------------------------------
                let err = unsafe { ZenListSensorsAsync(client_handle) };
                if err != ZenError_None {
                    log::error!("ZenListSensorsAsync failed with error {}", err);
                    return;
                }

                let mut discovered: Vec<ZenSensorDesc> = Vec::new();
                let mut event: ZenEvent = unsafe { std::mem::zeroed() };

                loop {
                    if !done.load(Ordering::Relaxed) {
                        return;
                    }
                    let ok = unsafe { ZenWaitForNextEvent(client_handle, &mut event) };
                    if !ok {
                        log::info!("ZenWaitForNextEvent returned false (shutdown)");
                        return;
                    }
                    #[allow(non_upper_case_globals)]
                    match event.eventType {
                        ZenEventType_SensorFound => {
                            let desc = unsafe { event.data.sensorFound };
                            log::info!(
                                "Discovered sensor: name='{}' serial='{}' io='{}'",
                                desc.name_str(),
                                desc.serial_number_str(),
                                desc.io_type_str(),
                            );
                            discovered.push(desc);
                        }
                        ZenEventType_SensorListingProgress => {
                            let progress = unsafe { event.data.sensorListingProgress };
                            if progress.complete != 0 || progress.progress >= 1.0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                }

                log::info!(
                    "Found {} devices that might be LP IMUs",
                    discovered.len()
                );

                for (idx, d) in discovered.iter().enumerate() {
                    log::info!("{}: {} ({})", idx, d.name_str(), d.io_type_str());
                }

                // 3. Register up to 2 sensors ------------------------------------
                struct SensorInfo {
                    _name: String,
                    sensor_handle: ZenSensorHandle_t,
                    imu_component: ZenComponentHandle_t,
                    gnss_component: ZenComponentHandle_t,
                }

                struct SensorGuard {
                    client: ZenClientHandle_t,
                    sensor: ZenSensorHandle_t,
                }
                impl Drop for SensorGuard {
                    fn drop(&mut self) {
                        unsafe { ZenReleaseSensor(self.client, self.sensor) };
                    }
                }

                let mut registered_sensors: Vec<SensorInfo> = Vec::new();
                let mut _sensor_guards: Vec<SensorGuard> = Vec::new();
                let mut registered_names: Vec<String> = Vec::new();

                let keys = ["sensor1", "sensor2"];
                for (i, _key) in keys.iter().enumerate() {
                    let requested_name = &sensor_names[i];
                    if requested_name.is_empty() && i > 0 {
                        log::info!("Skipping initialization of IG1P sensor {}", i);
                        continue;
                    }

                    // Find matching sensor in discovery list
                    let maybe_desc = discovered.iter().find(|d| {
                        let serial = d.serial_number_str().to_lowercase();

                        // Skip already-registered sensors
                        if registered_names.contains(&d.name_str().to_string()) {
                            log::info!(
                                "Sensor {} already registered, skipping",
                                d.name_str()
                            );
                            return false;
                        }

                        if !requested_name.is_empty() {
                            // Match by name prefix (serial may be padded)
                            return serial.starts_with(requested_name.as_str());
                        }

                        // Autodetect IG1P sensors
                        serial.starts_with("ig1p") || serial.starts_with("lpms-ig1p")
                    });

                    let desc = match maybe_desc {
                        Some(d) => *d,
                        None => {
                            log::warn!(
                                "Could not find matching sensor for slot {} (name='{}')",
                                i,
                                requested_name,
                            );
                            continue;
                        }
                    };

                    log::info!(
                        "Connecting to sensor {}: name='{}' serial='{}'",
                        i,
                        desc.name_str(),
                        desc.serial_number_str(),
                    );

                    // Obtain sensor
                    let mut sensor_handle = ZenSensorHandle_t { handle: 0 };
                    let mut obtained = false;
                    for attempt in 0..3 {
                        let err = unsafe {
                            ZenObtainSensor(client_handle, &desc, &mut sensor_handle)
                        };
                        if err == ZenSensorInitError_None {
                            obtained = true;
                            break;
                        }
                        log::warn!(
                            "ZenObtainSensor attempt {} failed with error {}",
                            attempt + 1,
                            err,
                        );
                    }
                    if !obtained {
                        log::error!(
                            "Failed to obtain sensor '{}' after 3 attempts",
                            desc.name_str(),
                        );
                        continue;
                    }
                    log::info!("Obtained sensor '{}'", desc.name_str());

                    _sensor_guards.push(SensorGuard {
                        client: client_handle,
                        sensor: sensor_handle,
                    });

                    // Get IMU component
                    let imu_type = std::ffi::CString::new("imu").unwrap();
                    let mut imu_ptr: *mut ZenComponentHandle_t = std::ptr::null_mut();
                    let mut imu_count: usize = 0;
                    let err = unsafe {
                        ZenSensorComponents(
                            client_handle,
                            sensor_handle,
                            imu_type.as_ptr(),
                            &mut imu_ptr,
                            &mut imu_count,
                        )
                    };
                    if err != ZenError_None || imu_count == 0 || imu_ptr.is_null() {
                        log::error!(
                            "Sensor '{}' has no IMU component (error={})",
                            desc.serial_number_str(),
                            err,
                        );
                        continue;
                    }
                    let imu_component = unsafe { *imu_ptr };
                    log::info!("Found IMU component");

                    // Get GNSS component
                    let gnss_type = std::ffi::CString::new("gnss").unwrap();
                    let mut gnss_ptr: *mut ZenComponentHandle_t = std::ptr::null_mut();
                    let mut gnss_count: usize = 0;
                    let err = unsafe {
                        ZenSensorComponents(
                            client_handle,
                            sensor_handle,
                            gnss_type.as_ptr(),
                            &mut gnss_ptr,
                            &mut gnss_count,
                        )
                    };
                    if err != ZenError_None || gnss_count == 0 || gnss_ptr.is_null() {
                        log::error!(
                            "Sensor '{}' has no GNSS component (error={})",
                            desc.serial_number_str(),
                            err,
                        );
                        continue;
                    }
                    let gnss_component = unsafe { *gnss_ptr };
                    log::info!("Found GNSS component");

                    let sensor_name = desc.name_str().to_string();
                    registered_names.push(sensor_name.clone());
                    registered_sensors.push(SensorInfo {
                        _name: sensor_name,
                        sensor_handle,
                        imu_component,
                        gnss_component,
                    });

                    log::info!(
                        "Added sensor {} '{}' to ImuPSource",
                        i,
                        desc.serial_number_str(),
                    );
                }

                let n_sensors = registered_sensors.len();
                if n_sensors < 1 {
                    log::error!("No IG1P sensor found. Aborting ImuPSource initialization.");
                    return;
                }
                log::info!("Initialized {} IG1P sensor(s)", n_sensors);
                if n_sensors > 1 {
                    log::info!(
                        "Currently only a single IG1P sensor is supported. Only the first sensor will be used."
                    );
                }

                // 4. Configure sensors -------------------------------------------
                let mut interval = 1.0 / 100.0; // 100 Hz default for IG1P

                for info in &registered_sensors {
                    // Disable streaming while configuring
                    let _ = unsafe {
                        ZenSensorComponentSetBoolProperty(
                            client_handle,
                            info.sensor_handle,
                            info.imu_component,
                            ZenImuProperty_StreamData,
                            false,
                        )
                    };

                    // Set sampling rate to 100 Hz
                    let target_freq: i32 = 100;
                    let err = unsafe {
                        ZenSensorComponentSetInt32Property(
                            client_handle,
                            info.sensor_handle,
                            info.imu_component,
                            ZenImuProperty_SamplingRate,
                            target_freq,
                        )
                    };
                    if err != ZenError_None {
                        log::warn!(
                            "Failed to set sampling rate to {} Hz (error={})",
                            target_freq,
                            err,
                        );
                    } else {
                        log::info!("Sampling frequency set to {}", target_freq);
                    }
                    interval = 1.0 / target_freq as f64;

                    // Re-enable streaming
                    let _ = unsafe {
                        ZenSensorComponentSetBoolProperty(
                            client_handle,
                            info.sensor_handle,
                            info.imu_component,
                            ZenImuProperty_StreamData,
                            true,
                        )
                    };
                }

                log::info!("ImuP sensors configured and streaming");

                // 5. Event loop --------------------------------------------------
                let mut dual_gnss_combiner = DualGnssCombiner::new();
                let mut most_recent_orientation = Quatd::identity();

                while done.load(Ordering::Relaxed) {
                    let ok = unsafe { ZenWaitForNextEvent(client_handle, &mut event) };
                    if !ok {
                        break;
                    }

                    if !enabled.load(Ordering::Relaxed) {
                        continue;
                    }

                    // Identify which sensor this event belongs to
                    let sensor_idx = registered_sensors
                        .iter()
                        .position(|s| s.sensor_handle == event.sensor);
                    let sensor_idx = match sensor_idx {
                        Some(idx) => idx,
                        None => continue,
                    };

                    #[allow(non_upper_case_globals)]
                    match event.eventType {
                        ZenEventType_ImuData => {
                            let imu = unsafe { event.data.imuData };

                            let gyroscope = Vec3d::new(
                                imu.g1[0] as f64,
                                imu.g1[1] as f64,
                                imu.g1[2] as f64,
                            );

                            // IG1P: sign-flip accelerometer
                            let accelerometer = Vec3d::new(
                                -(imu.a[0] as f64),
                                -(imu.a[1] as f64),
                                -(imu.a[2] as f64),
                            );

                            // Quaternion: OpenZen delivers w,x,y,z
                            let quaternion = Quatd::from_quaternion(
                                nalgebra::Quaternion::new(
                                    imu.q[0] as f64,
                                    imu.q[1] as f64,
                                    imu.q[2] as f64,
                                    imu.q[3] as f64,
                                ),
                            );

                            let euler = Vec3d::new(
                                imu.r[0] as f64,
                                imu.r[1] as f64,
                                imu.r[2] as f64,
                            );

                            most_recent_orientation = Quatd::from_quaternion(
                                nalgebra::Quaternion::new(
                                    imu.q[0] as f64,
                                    imu.q[1] as f64,
                                    imu.q[2] as f64,
                                    imu.q[3] as f64,
                                ),
                            );

                            let out_data = ImuData {
                                sender_id: registered_names
                                    .get(sensor_idx)
                                    .cloned()
                                    .unwrap_or_else(|| id.clone()),
                                timestamp: SystemTime::now(),
                                latency: 0.0,
                                gyroscope,
                                accelerometer,
                                quaternion,
                                euler,
                                period: interval,
                                internal_frame_count: imu.frameCount,
                                linear_velocity: Vec3d::zeros(),
                            };

                            // Only emit data from sensor 0 (matches C++ behaviour)
                            if sensor_idx == 0 {
                                let cbs = consumers.lock().unwrap();
                                let streamable = StreamableData::Imu(out_data);
                                for cb in cbs.iter() {
                                    cb(streamable.clone());
                                }
                            }
                        }

                        ZenEventType_GnssData => {
                            let gnss = unsafe { event.data.gnssData };

                            // Update dual-GNSS combiner (for future dual-antenna support)
                            let _combiner_quat = dual_gnss_combiner.update(
                                sensor_idx,
                                gnss.latitude,
                                gnss.longitude,
                            );

                            // Log RTK status
                            let rtk_status = match gnss.carrierPhaseSolution {
                                ZEN_GNSS_FIX_CARRIER_PHASE_SOLUTION_NONE => "NONE",
                                ZEN_GNSS_FIX_CARRIER_PHASE_SOLUTION_FLOAT => "FLOAT",
                                ZEN_GNSS_FIX_CARRIER_PHASE_SOLUTION_FIXED => "FIX",
                                _ => "UNKNOWN",
                            };
                            log::info!(
                                "Sensor {}: RTK status = {} with {} satellites",
                                sensor_idx,
                                rtk_status,
                                gnss.numberSatellitesUsed,
                            );

                            let out_data = GnssData {
                                sender_id: registered_names
                                    .get(sensor_idx)
                                    .cloned()
                                    .unwrap_or_else(|| id.clone()),
                                timestamp: SystemTime::now(),
                                latency: 0.0,
                                latitude: gnss.latitude,
                                longitude: gnss.longitude,
                                height: gnss.height,
                                horizontal_accuracy: gnss.horizontalAccuracy,
                                vertical_accuracy: gnss.verticalAccuracy,
                                // Use IMU orientation directly (matches C++ FIXME behaviour)
                                orientation: most_recent_orientation,
                                altitude: 0.0,
                                undulation: 0.0,
                                quality: gnss.fixType,
                                n_sat: gnss.numberSatellitesUsed as i32,
                                hdop: 0.0,
                                tmg: gnss.headingOfMotion,
                                heading: gnss.headingOfVehicle,
                                period: 0.0,
                                internal_frame_count: gnss.frameCount,
                                diff_age: 0.0,
                            };

                            // Only emit data from sensor 0 (matches C++ behaviour)
                            if sensor_idx == 0 {
                                let cbs = consumers.lock().unwrap();
                                let streamable = StreamableData::Gnss(out_data);
                                for cb in cbs.iter() {
                                    cb(streamable.clone());
                                }
                            }
                        }

                        _ => {}
                    }
                }

                log::info!("ImuP worker thread exiting");
            })
            .await;

            if let Err(e) = result {
                log::warn!("ImuP worker thread panicked: {}", e);
            }
        }));

        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        log::info!("Stopping ImuP source: {}", self.base.name());
        self.m_running.store(false, Ordering::Relaxed);

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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;

    #[test]
    fn distance_between_same_point_is_zero() {
        let d = distance_between_coordinates(11.0, 48.0, 11.0, 48.0);
        assert_relative_eq!(d, 0.0, epsilon = 1e-10);
    }

    #[test]
    fn distance_between_known_points() {
        // Munich (48.1351, 11.5820) to Berlin (52.5200, 13.4050) ~ 504 km
        let d = distance_between_coordinates(11.5820, 48.1351, 13.4050, 52.5200);
        assert!(d > 500_000.0 && d < 510_000.0, "distance was {}m", d);
    }

    #[test]
    fn gps_quaternion_identity_for_same_point() {
        let q = gps_quaternion(11.0, 48.0, 11.0, 48.0);
        let identity = Quatd::identity();
        assert_relative_eq!(q.w, identity.w, epsilon = 1e-10);
        assert_relative_eq!(q.i, identity.i, epsilon = 1e-10);
        assert_relative_eq!(q.j, identity.j, epsilon = 1e-10);
        assert_relative_eq!(q.k, identity.k, epsilon = 1e-10);
    }

    #[test]
    fn gps_quaternion_nonzero_for_different_points() {
        let q = gps_quaternion(11.0, 48.0, 11.001, 48.001);
        // Should differ from identity
        let angle = q.angle();
        assert!(angle > 0.0, "angle should be nonzero");
    }

    #[test]
    fn dual_gnss_combiner_no_update_until_both_valid() {
        let mut combiner = DualGnssCombiner::new();
        // Only update sensor 0 — should return identity since sensor 1 is still (0,0)
        let q = combiner.update(0, 48.0, 11.0);
        assert_relative_eq!(q.w, 1.0, epsilon = 1e-10);
    }

    #[test]
    fn dual_gnss_combiner_computes_orientation_when_both_valid() {
        let mut combiner = DualGnssCombiner::new();
        combiner.update(0, 48.0, 11.0);
        let q = combiner.update(1, 48.001, 11.001);
        // With both antennas having data, orientation should differ from identity
        let angle = q.angle();
        assert!(angle > 0.0, "angle should be nonzero");
    }

    #[test]
    fn imu_p_source_new_defaults() {
        let config = serde_json::json!({
            "sensor1": { "name": "ig1p232000001" },
            "sensor2": { "name": "ig1p232000002" }
        });
        let source = ImuPSource::new("test_imup", &config, "1");
        assert_eq!(source.name(), "test_imup");
        assert_eq!(source.get_imu_endpoint(), "tcp://*:8802");
        assert_eq!(source.get_gnss_endpoint(), "inproc://gnss_data_source_1");
    }

    #[test]
    fn imu_p_source_custom_endpoint() {
        let config = serde_json::json!({
            "imuEndpoint": "tcp://*:9999",
            "sensor1": { "name": "test1" }
        });
        let source = ImuPSource::new("custom", &config, "42");
        assert_eq!(source.get_imu_endpoint(), "tcp://*:9999");
        assert_eq!(source.get_gnss_endpoint(), "inproc://gnss_data_source_42");
    }
}
