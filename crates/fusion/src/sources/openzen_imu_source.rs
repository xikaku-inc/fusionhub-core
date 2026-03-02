//! OpenZen IMU source node.
//!
//! Reads IMU data via the OpenZen sensor abstraction library. The companion
//! `openzen-sys` crate builds OpenZen from source (pinned commit) and links
//! it at compile time, so the DLL is always available next to the executable.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use tokio::task::JoinHandle;

use fusion_types::{ImuData, JsonValueExt, Quatd, StreamableData, Vec3d};
use openzen_sys::*;

use crate::node::{ConsumerCallback, Node, NodeBase};

// ---------------------------------------------------------------------------
// OpenZenImuSource
// ---------------------------------------------------------------------------

/// OpenZen IMU source node.
///
/// Discovers LP-Research IMU sensors via the OpenZen library, connects to the
/// first matching device (or a named one), and streams IMU data to downstream
/// consumers.
pub struct OpenZenImuSource {
    pub base: NodeBase,

    // Configuration
    m_sensor_name: String,
    m_autodetect_type: String,
    m_stream_frequency: i32,
    m_configure_frequency: bool,
    m_on_device_autocalibration: bool,
    m_id: String,

    // Runtime
    m_done: Arc<AtomicBool>,
    m_worker_handle: Option<JoinHandle<()>>,
}

impl OpenZenImuSource {
    pub fn new(name: impl Into<String>, config: &serde_json::Value) -> Self {
        let name = name.into();

        let sensor_name = config.value_str("name", "").to_lowercase();

        let id = config.value_str("id", "imu");

        let on_device_autocalibration = config.value_bool("onDeviceAutocalibration", false);

        let mut autodetect_type = config.value_str("autodetectType", "ig1").to_lowercase();

        if autodetect_type != "lpms" && autodetect_type != "ig1" {
            log::warn!(
                "Unknown autodetectType '{}', defaulting to ig1",
                autodetect_type
            );
            autodetect_type = "ig1".to_string();
        }

        let (stream_frequency, configure_frequency) =
            if let Some(freq) = config.get("streamFrequency").and_then(|v| v.as_i64()) {
                (freq as i32, true)
            } else {
                (400, false)
            };

        log::info!(
            "Creating OpenZenImuSource '{}' id='{}' autodetect='{}' freq={}",
            name,
            id,
            autodetect_type,
            stream_frequency
        );

        Self {
            base: NodeBase::new(&name),
            m_sensor_name: sensor_name,
            m_autodetect_type: autodetect_type,
            m_stream_frequency: stream_frequency,
            m_configure_frequency: configure_frequency,
            m_on_device_autocalibration: on_device_autocalibration,
            m_id: id,
            m_done: Arc::new(AtomicBool::new(false)),
            m_worker_handle: None,
        }
    }
}

impl Node for OpenZenImuSource {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn start(&mut self) -> anyhow::Result<()> {
        log::info!("Starting OpenZen IMU source: {}", self.base.name());

        let done = self.m_done.clone();
        let consumers = self.base.consumers_arc();
        let enabled = self.base.enabled_arc();
        let sensor_name = self.m_sensor_name.clone();
        let autodetect_type = self.m_autodetect_type.clone();
        let stream_frequency = self.m_stream_frequency;
        let configure_frequency = self.m_configure_frequency;
        let id = self.m_id.clone();
        let on_device_autocal = self.m_on_device_autocalibration;

        done.store(false, Ordering::Relaxed);

        self.m_worker_handle = Some(tokio::spawn(async move {
            let result = tokio::task::spawn_blocking(move || {
                // 1. Initialize client -------------------------------------------
                let mut client_handle = ZenClientHandle_t { handle: 0 };
                let err = unsafe { ZenInit(&mut client_handle) };
                if err != ZenError_None {
                    log::error!("ZenInit failed with error {}", err);
                    return;
                }

                // Ensure cleanup on all exit paths.
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
                    if done.load(Ordering::Relaxed) {
                        return;
                    }
                    let ok = unsafe { ZenWaitForNextEvent(client_handle, &mut event) };
                    if !ok {
                        // ZenWaitForNextEvent returns false when ZenShutdown is called.
                        log::info!("ZenWaitForNextEvent returned false (shutdown)");
                        return;
                    }
                    #[allow(non_upper_case_globals)]
                    match event.eventType {
                        ZenEventType_SensorFound => {
                            let desc = unsafe { event.data.sensorFound };
                            log::info!(
                                "Discovered sensor: name='{}' serial='{}'",
                                desc.name_str(),
                                desc.serial_number_str()
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

                // 3. Find matching sensor ----------------------------------------
                let maybe_desc = discovered.iter().find(|d| {
                    let serial = d.serial_number_str().to_lowercase();

                    if !sensor_name.is_empty() {
                        return serial == sensor_name;
                    }

                    // Autodetect
                    match autodetect_type.as_str() {
                        "lpms" => serial.starts_with("lpms"),
                        "ig1" => serial.starts_with("ig1") || serial.starts_with("lpms-ig1"),
                        _ => false,
                    }
                });

                let desc = match maybe_desc {
                    Some(d) => *d,
                    None => {
                        log::warn!(
                            "Could not find matching sensor (name='{}', autodetect='{}')",
                            sensor_name,
                            autodetect_type
                        );
                        return;
                    }
                };

                log::info!(
                    "Connecting to sensor: name='{}' serial='{}'",
                    desc.name_str(),
                    desc.serial_number_str()
                );

                // 4. Obtain sensor -----------------------------------------------
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
                        err
                    );
                }
                if !obtained {
                    log::error!("Failed to obtain sensor after 3 attempts");
                    return;
                }

                // Ensure sensor is released on exit.
                struct SensorGuard {
                    client: ZenClientHandle_t,
                    sensor: ZenSensorHandle_t,
                }
                impl Drop for SensorGuard {
                    fn drop(&mut self) {
                        unsafe {
                            ZenReleaseSensor(self.client, self.sensor);
                        }
                    }
                }
                let _sensor_guard = SensorGuard {
                    client: client_handle,
                    sensor: sensor_handle,
                };

                // 5. Get IMU component -------------------------------------------
                let imu_type = std::ffi::CString::new("imu").unwrap();
                let mut components_ptr: *mut ZenComponentHandle_t = std::ptr::null_mut();
                let mut count: usize = 0;
                let err = unsafe {
                    ZenSensorComponents(
                        client_handle,
                        sensor_handle,
                        imu_type.as_ptr(),
                        &mut components_ptr,
                        &mut count,
                    )
                };
                if err != ZenError_None || count == 0 || components_ptr.is_null() {
                    log::error!("Sensor has no IMU component (error={})", err);
                    return;
                }
                let component_handle = unsafe { *components_ptr };

                // 6. Configure IMU -----------------------------------------------
                // Disable streaming while configuring
                let _ = unsafe {
                    ZenSensorComponentSetBoolProperty(
                        client_handle,
                        sensor_handle,
                        component_handle,
                        ZenImuProperty_StreamData,
                        false,
                    )
                };

                let serial_lower = desc.serial_number_str().to_lowercase();
                let is_ig1 = serial_lower.contains("ig1");
                let use_gyro1 = is_ig1;
                let flip_accel = is_ig1;

                // Set sampling rate
                let actual_freq = if configure_frequency {
                    stream_frequency
                } else {
                    400 // default maximum
                };

                let err = unsafe {
                    ZenSensorComponentSetInt32Property(
                        client_handle,
                        sensor_handle,
                        component_handle,
                        ZenImuProperty_SamplingRate,
                        actual_freq,
                    )
                };
                if err != ZenError_None {
                    log::warn!(
                        "Failed to set sampling rate to {} Hz (error={})",
                        actual_freq,
                        err
                    );
                }
                let interval = 1.0 / actual_freq as f64;
                log::info!("Sampling interval set to {:.6}s ({}Hz)", interval, actual_freq);

                // On-device gyroscope autocalibration
                if on_device_autocal {
                    let err = unsafe {
                        ZenSensorComponentSetBoolProperty(
                            client_handle,
                            sensor_handle,
                            component_handle,
                            ZenImuProperty_GyrUseAutoCalibration,
                            true,
                        )
                    };
                    if err != ZenError_None {
                        log::warn!("Failed to enable on-device gyro autocalibration (error={})", err);
                    } else {
                        log::info!("On-device gyroscope autocalibration enabled");
                    }
                }

                // Re-enable streaming
                let _ = unsafe {
                    ZenSensorComponentSetBoolProperty(
                        client_handle,
                        sensor_handle,
                        component_handle,
                        ZenImuProperty_StreamData,
                        true,
                    )
                };
                log::info!("OpenZen sensor configured and streaming");

                // 7. Event loop --------------------------------------------------
                let mut first_frame_time: Option<SystemTime> = None;
                let mut first_frame_count: Option<i32> = None;
                let mut received_sample_count: u64 = 0;
                let mut last_event_instant = std::time::Instant::now();

                while !done.load(Ordering::Relaxed) {
                    // Check for timeout / reconnection need
                    if last_event_instant.elapsed() > Duration::from_secs(2) {
                        log::warn!("No IMU data for 2 seconds, resetting timing");
                        first_frame_time = None;
                        first_frame_count = None;
                        received_sample_count = 0;
                        last_event_instant = std::time::Instant::now();
                    }

                    let ok = unsafe {
                        ZenWaitForNextEvent(client_handle, &mut event)
                    };
                    if !ok {
                        // Shutdown or error
                        break;
                    }

                    if event.eventType != ZenEventType_ImuData
                        || event.sensor != sensor_handle
                        || event.component != component_handle
                    {
                        continue;
                    }

                    last_event_instant = std::time::Instant::now();
                    let imu = unsafe { event.data.imuData };

                    if !enabled.load(Ordering::Relaxed) {
                        continue;
                    }

                    // Compute timestamp from sample counter
                    let timestamp = if first_frame_time.is_none() {
                        let now = SystemTime::now();
                        first_frame_time = Some(now);
                        first_frame_count = Some(imu.frameCount);
                        received_sample_count = 0;
                        log::info!(
                            "IMU timing initialized: first frame {} at rate {}Hz",
                            imu.frameCount,
                            1.0 / interval
                        );
                        now
                    } else {
                        received_sample_count += 1;
                        let base = first_frame_time.unwrap();
                        let delta = Duration::from_secs_f64(
                            received_sample_count as f64 * interval,
                        );
                        base + delta
                    };

                    // Handle frame count wraparound
                    if let Some(ref mut first_fc) = first_frame_count {
                        if imu.frameCount < *first_fc {
                            log::info!(
                                "IMU frame count wrapped (was {}, now {}), resetting",
                                *first_fc,
                                imu.frameCount
                            );
                            *first_fc = imu.frameCount;
                        }
                    }

                    // Extract gyroscope (g1 for IG1 sensors, g2 otherwise)
                    let gyro_raw = if use_gyro1 { imu.g1 } else { imu.g2 };
                    let gyroscope = Vec3d::new(
                        gyro_raw[0] as f64,
                        gyro_raw[1] as f64,
                        gyro_raw[2] as f64,
                    );

                    // Extract accelerometer (negated for IG1 sensors)
                    let accel_sign = if flip_accel { -1.0 } else { 1.0 };
                    let accelerometer = Vec3d::new(
                        accel_sign * imu.a[0] as f64,
                        accel_sign * imu.a[1] as f64,
                        accel_sign * imu.a[2] as f64,
                    );

                    // Quaternion: OpenZen delivers w,x,y,z
                    let quaternion = Quatd::from_quaternion(nalgebra::Quaternion::new(
                        imu.q[0] as f64,
                        imu.q[1] as f64,
                        imu.q[2] as f64,
                        imu.q[3] as f64,
                    ));

                    let euler = Vec3d::new(
                        imu.r[0] as f64,
                        imu.r[1] as f64,
                        imu.r[2] as f64,
                    );

                    let out_data = ImuData {
                        sender_id: id.clone(),
                        timestamp,
                        latency: 0.0,
                        gyroscope,
                        accelerometer,
                        quaternion,
                        euler,
                        period: interval,
                        internal_frame_count: imu.frameCount,
                        linear_velocity: Vec3d::zeros(),
                    };

                    let cbs = consumers.lock().unwrap();
                    let streamable = StreamableData::Imu(out_data);
                    for cb in cbs.iter() {
                        cb(streamable.clone());
                    }
                }

                log::info!("OpenZen worker thread exiting");
            })
            .await;

            if let Err(e) = result {
                log::warn!("OpenZen worker thread panicked: {}", e);
            }
        }));

        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        log::info!("Stopping OpenZen IMU source: {}", self.base.name());
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
