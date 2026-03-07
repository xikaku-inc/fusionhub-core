use std::ffi::{c_void, CString};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use libloading::Library;
use nalgebra::UnitQuaternion;
use tokio::task::JoinHandle;

use fusion_registry::SettingsField;
use fusion_types::{OpticalData, StreamableData, Vec3d};

use crate::node::{ConsumerCallback, Node, NodeBase};

pub fn settings_schema() -> Vec<SettingsField> {
    vec![]
}

// ---------------------------------------------------------------------------
// Vicon bridge DLL types
// ---------------------------------------------------------------------------
// These correspond to a thin C-compatible wrapper around the Vicon DataStream
// SDK C++ API.  The wrapper DLL ("vicon_bridge.dll") exposes plain C functions.

type ViconClient = *mut c_void;

type FnViconConnect = unsafe extern "C" fn(*const u8) -> ViconClient;
type FnViconGetFrame = unsafe extern "C" fn(ViconClient) -> i32;
type FnViconGetSubjectCount = unsafe extern "C" fn(ViconClient) -> u32;
type FnViconGetSegmentPosition =
    unsafe extern "C" fn(ViconClient, u32, u32, *mut f64, *mut f64, *mut f64) -> i32;
type FnViconGetSegmentQuaternion =
    unsafe extern "C" fn(ViconClient, u32, u32, *mut f64, *mut f64, *mut f64, *mut f64) -> i32;
type FnViconDisconnect = unsafe extern "C" fn(ViconClient);

/// Holds the loaded vicon_bridge.dll and its function pointers.
struct ViconApi {
    _lib: Library,
    vicon_connect: FnViconConnect,
    vicon_get_frame: FnViconGetFrame,
    vicon_get_subject_count: FnViconGetSubjectCount,
    vicon_get_segment_position: FnViconGetSegmentPosition,
    vicon_get_segment_quaternion: FnViconGetSegmentQuaternion,
    vicon_disconnect: FnViconDisconnect,
}

impl ViconApi {
    fn load() -> Result<Self, libloading::Error> {
        let lib = unsafe { Library::new("vicon_bridge.dll") }?;
        unsafe {
            let vicon_connect = *lib.get::<FnViconConnect>(b"vicon_connect\0")?;
            let vicon_get_frame = *lib.get::<FnViconGetFrame>(b"vicon_get_frame\0")?;
            let vicon_get_subject_count =
                *lib.get::<FnViconGetSubjectCount>(b"vicon_get_subject_count\0")?;
            let vicon_get_segment_position =
                *lib.get::<FnViconGetSegmentPosition>(b"vicon_get_segment_position\0")?;
            let vicon_get_segment_quaternion =
                *lib.get::<FnViconGetSegmentQuaternion>(b"vicon_get_segment_quaternion\0")?;
            let vicon_disconnect = *lib.get::<FnViconDisconnect>(b"vicon_disconnect\0")?;

            Ok(Self {
                _lib: lib,
                vicon_connect,
                vicon_get_frame,
                vicon_get_subject_count,
                vicon_get_segment_position,
                vicon_get_segment_quaternion,
                vicon_disconnect,
            })
        }
    }
}

// SAFETY: The ViconApi struct holds function pointers loaded from the DLL and
// the library handle itself. The function pointers remain valid for the
// library's lifetime. The Vicon DataStream SDK client is used from a single
// worker thread.
unsafe impl Send for ViconApi {}
unsafe impl Sync for ViconApi {}

// ---------------------------------------------------------------------------
// ViconOpticalSource
// ---------------------------------------------------------------------------

/// Vicon optical tracking source node.
///
/// Reads optical tracking data from a Vicon motion capture system via a thin
/// C-compatible wrapper DLL (`vicon_bridge.dll`) around the Vicon DataStream SDK.
/// If the DLL is not present on the system, the node logs a warning and returns
/// without error.
pub struct ViconOpticalSource {
    pub base: NodeBase,
    m_config: serde_json::Value,
    m_host: String,
    m_subject_index: u32,
    m_segment_index: u32,
    m_running: Arc<AtomicBool>,
    m_worker_handle: Option<JoinHandle<()>>,
}

impl ViconOpticalSource {
    pub fn new(name: impl Into<String>, config: &serde_json::Value) -> Self {
        let host = config
            .get("host")
            .and_then(|v| v.as_str())
            .unwrap_or("localhost:801")
            .to_string();

        // Subject and segment indices default to 0 (first subject, first segment).
        // Advanced users can override via config.  Named subject/segment lookup
        // would require additional bridge DLL functions.
        let subject_index = config
            .get("subjectIndex")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;
        let segment_index = config
            .get("segmentIndex")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;

        Self {
            base: NodeBase::new(name),
            m_config: config.clone(),
            m_host: host,
            m_subject_index: subject_index,
            m_segment_index: segment_index,
            m_running: Arc::new(AtomicBool::new(false)),
            m_worker_handle: None,
        }
    }
}

impl Node for ViconOpticalSource {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn start(&mut self) -> anyhow::Result<()> {
        log::info!(
            "ViconOpticalSource '{}': starting (host={}, subject={}, segment={})",
            self.base.name(),
            self.m_host,
            self.m_subject_index,
            self.m_segment_index,
        );

        let api = match ViconApi::load() {
            Ok(api) => {
                log::info!("ViconOpticalSource: vicon_bridge.dll loaded successfully");
                Arc::new(api)
            }
            Err(e) => {
                log::warn!(
                    "ViconOpticalSource '{}': could not load vicon_bridge.dll: {}. \
                     Node will remain inactive.",
                    self.base.name(),
                    e
                );
                return Ok(());
            }
        };

        self.m_running.store(true, Ordering::SeqCst);

        let running = self.m_running.clone();
        let consumers = self.base.consumers_arc();
        let enabled = self.base.enabled_arc();
        let node_name = self.base.name().to_string();
        let host = self.m_host.clone();
        let subject_idx = self.m_subject_index;
        let segment_idx = self.m_segment_index;

        self.m_worker_handle = Some(tokio::spawn(async move {
            let result = tokio::task::spawn_blocking(move || {
                Self::worker_loop(
                    api,
                    running,
                    consumers,
                    enabled,
                    node_name,
                    host,
                    subject_idx,
                    segment_idx,
                );
            })
            .await;

            if let Err(e) = result {
                log::warn!("ViconOpticalSource worker thread panicked: {}", e);
            }
        }));

        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        log::info!("ViconOpticalSource '{}': stopping", self.base.name());
        self.m_running.store(false, Ordering::SeqCst);

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

    fn receive_data(&mut self, _data: StreamableData) {}

    fn receive_command(&mut self, _cmd: &fusion_types::ApiRequest) {}

    fn set_on_output(&self, callback: ConsumerCallback) {
        self.base.add_consumer(callback);
    }
}

impl ViconOpticalSource {
    fn worker_loop(
        api: Arc<ViconApi>,
        running: Arc<AtomicBool>,
        consumers: Arc<Mutex<Vec<ConsumerCallback>>>,
        enabled: Arc<AtomicBool>,
        node_name: String,
        host: String,
        subject_idx: u32,
        segment_idx: u32,
    ) {
        // Connect to the Vicon server
        let host_cstr = CString::new(host.as_str()).unwrap_or_default();
        let client = unsafe { (api.vicon_connect)(host_cstr.as_ptr() as *const u8) };
        if client.is_null() {
            log::error!(
                "ViconOpticalSource: failed to connect to Vicon server at '{}'",
                host
            );
            return;
        }
        log::info!("ViconOpticalSource: connected to Vicon server at '{}'", host);

        let mut frame_number: i32 = 0;
        let mut last_time = SystemTime::now();

        // Frame acquisition loop
        while running.load(Ordering::Relaxed) {
            let status = unsafe { (api.vicon_get_frame)(client) };
            if status != 0 {
                std::thread::sleep(Duration::from_millis(1));
                continue;
            }

            let subject_count = unsafe { (api.vicon_get_subject_count)(client) };
            if subject_idx >= subject_count {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }

            // Get position (Vicon returns mm, we convert to metres)
            let mut px: f64 = 0.0;
            let mut py: f64 = 0.0;
            let mut pz: f64 = 0.0;
            let pos_ok = unsafe {
                (api.vicon_get_segment_position)(
                    client,
                    subject_idx,
                    segment_idx,
                    &mut px,
                    &mut py,
                    &mut pz,
                )
            };

            // Get orientation as quaternion (w, x, y, z)
            let mut qw: f64 = 1.0;
            let mut qx: f64 = 0.0;
            let mut qy: f64 = 0.0;
            let mut qz: f64 = 0.0;
            let quat_ok = unsafe {
                (api.vicon_get_segment_quaternion)(
                    client,
                    subject_idx,
                    segment_idx,
                    &mut qw,
                    &mut qx,
                    &mut qy,
                    &mut qz,
                )
            };

            if pos_ok != 0 || quat_ok != 0 {
                continue;
            }

            let now = SystemTime::now();
            let interval = now
                .duration_since(last_time)
                .unwrap_or(Duration::from_millis(10));
            last_time = now;
            frame_number += 1;

            // Convert mm to m
            let position = Vec3d::new(px / 1000.0, py / 1000.0, pz / 1000.0);
            let orientation =
                UnitQuaternion::from_quaternion(nalgebra::Quaternion::new(qw, qx, qy, qz));

            let optical = OpticalData {
                sender_id: node_name.clone(),
                timestamp: now,
                last_data_time: now,
                latency: 0.0,
                position,
                orientation,
                angular_velocity: Vec3d::zeros(),
                quality: 1.0,
                frame_rate: if interval.as_secs_f64() > 0.0 {
                    1.0 / interval.as_secs_f64()
                } else {
                    0.0
                },
                frame_number,
                interval,
            };

            if enabled.load(Ordering::Relaxed) {
                let cbs = consumers.lock().unwrap();
                let streamable = StreamableData::Optical(optical);
                for cb in cbs.iter() {
                    cb(streamable.clone());
                }
            }
        }

        // Disconnect
        log::info!("ViconOpticalSource: disconnecting from Vicon server");
        unsafe {
            (api.vicon_disconnect)(client);
        }
    }
}
