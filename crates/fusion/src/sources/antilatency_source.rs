use std::ffi::c_void;
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
// Antilatency COM-like interface types
// ---------------------------------------------------------------------------
//
// The Antilatency SDK uses COM-style vtable interfaces. We model the minimal
// subset needed for 6-DOF tracking via the Alt Tracker.  Each interface is
// represented as a pointer to a vtable struct.

/// Opaque handle to an Antilatency interface object.
type InterfacePtr = *mut c_void;

/// Result type returned by Antilatency SDK functions (HRESULT-like).
type AntilatencyResult = i32;

const ANTILATENCY_OK: AntilatencyResult = 0;

/// Pose returned by the tracking API.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct AltPose {
    px: f32,
    py: f32,
    pz: f32,
    qx: f32,
    qy: f32,
    qz: f32,
    qw: f32,
}

/// Tracking stability indicator.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Default)]
struct AltTrackingState(u32);

const ALT_TRACKING_STATE_TRACKING: AltTrackingState = AltTrackingState(2);

// ---------------------------------------------------------------------------
// Function pointer types for each DLL
// ---------------------------------------------------------------------------

// AntilatencyDeviceNetwork.dll
type FnAdnCreate = unsafe extern "system" fn(*mut InterfacePtr) -> AntilatencyResult;
type FnAdnGetUpdateId = unsafe extern "system" fn(InterfacePtr, *mut u32) -> AntilatencyResult;
type FnAdnGetDeviceCount = unsafe extern "system" fn(InterfacePtr, *mut u32) -> AntilatencyResult;
type FnAdnGetDeviceTag =
    unsafe extern "system" fn(InterfacePtr, u32, *mut u8, u32) -> AntilatencyResult;
type FnAdnDestroy = unsafe extern "system" fn(InterfacePtr) -> AntilatencyResult;

// AntilatencyAltTracking.dll
type FnAltCreateTracker = unsafe extern "system" fn(
    InterfacePtr, // device network
    u32,          // device index
    InterfacePtr, // environment
    *mut InterfacePtr, // out: tracker
) -> AntilatencyResult;
type FnAltGetPose =
    unsafe extern "system" fn(InterfacePtr, *mut AltPose, *mut AltTrackingState) -> AntilatencyResult;
type FnAltDestroyTracker = unsafe extern "system" fn(InterfacePtr) -> AntilatencyResult;

// AntilatencyAltEnvironmentSelector.dll
type FnAltEnvCreate =
    unsafe extern "system" fn(*const u8, u32, *mut InterfacePtr) -> AntilatencyResult;
type FnAltEnvDestroy = unsafe extern "system" fn(InterfacePtr) -> AntilatencyResult;

// AntilatencyStorageClient.dll
type FnStorageGetEnvironment =
    unsafe extern "system" fn(*const u8, u32, *mut u8, u32) -> AntilatencyResult;

/// Holds all loaded Antilatency DLLs and their function pointers.
struct AntilatencyApi {
    _lib_device_network: Library,
    _lib_alt_tracking: Library,
    _lib_environment: Library,
    _lib_storage: Library,
    // DeviceNetwork
    adn_create: FnAdnCreate,
    adn_get_update_id: FnAdnGetUpdateId,
    adn_get_device_count: FnAdnGetDeviceCount,
    adn_get_device_tag: FnAdnGetDeviceTag,
    adn_destroy: FnAdnDestroy,
    // AltTracking
    alt_create_tracker: FnAltCreateTracker,
    alt_get_pose: FnAltGetPose,
    alt_destroy_tracker: FnAltDestroyTracker,
    // Environment
    alt_env_create: FnAltEnvCreate,
    alt_env_destroy: FnAltEnvDestroy,
    // Storage
    storage_get_environment: FnStorageGetEnvironment,
}

impl AntilatencyApi {
    fn load() -> Result<Self, libloading::Error> {
        let sdk_dir = "AntilatencySdk/Bin/WindowsDesktop/x64/";

        let lib_dn = unsafe {
            Library::new(format!("{}AntilatencyDeviceNetwork.dll", sdk_dir))
        }?;
        let lib_at = unsafe {
            Library::new(format!("{}AntilatencyAltTracking.dll", sdk_dir))
        }?;
        let lib_env = unsafe {
            Library::new(format!(
                "{}AntilatencyAltEnvironmentSelector.dll",
                sdk_dir
            ))
        }?;
        let lib_sc = unsafe {
            Library::new(format!("{}AntilatencyStorageClient.dll", sdk_dir))
        }?;

        unsafe {
            // DeviceNetwork
            let adn_create =
                *lib_dn.get::<FnAdnCreate>(b"AntilatencyDeviceNetwork_create\0")?;
            let adn_get_update_id =
                *lib_dn.get::<FnAdnGetUpdateId>(b"AntilatencyDeviceNetwork_getUpdateId\0")?;
            let adn_get_device_count =
                *lib_dn.get::<FnAdnGetDeviceCount>(b"AntilatencyDeviceNetwork_getDeviceCount\0")?;
            let adn_get_device_tag =
                *lib_dn.get::<FnAdnGetDeviceTag>(b"AntilatencyDeviceNetwork_getDeviceTag\0")?;
            let adn_destroy =
                *lib_dn.get::<FnAdnDestroy>(b"AntilatencyDeviceNetwork_destroy\0")?;

            // AltTracking
            let alt_create_tracker =
                *lib_at.get::<FnAltCreateTracker>(b"AntilatencyAltTracking_createTracker\0")?;
            let alt_get_pose =
                *lib_at.get::<FnAltGetPose>(b"AntilatencyAltTracking_getPose\0")?;
            let alt_destroy_tracker =
                *lib_at.get::<FnAltDestroyTracker>(b"AntilatencyAltTracking_destroyTracker\0")?;

            // Environment
            let alt_env_create =
                *lib_env.get::<FnAltEnvCreate>(b"AntilatencyAltEnvironment_create\0")?;
            let alt_env_destroy =
                *lib_env.get::<FnAltEnvDestroy>(b"AntilatencyAltEnvironment_destroy\0")?;

            // Storage
            let storage_get_environment =
                *lib_sc.get::<FnStorageGetEnvironment>(b"AntilatencyStorageClient_getEnvironment\0")?;

            Ok(Self {
                _lib_device_network: lib_dn,
                _lib_alt_tracking: lib_at,
                _lib_environment: lib_env,
                _lib_storage: lib_sc,
                adn_create,
                adn_get_update_id,
                adn_get_device_count,
                adn_get_device_tag,
                adn_destroy,
                alt_create_tracker,
                alt_get_pose,
                alt_destroy_tracker,
                alt_env_create,
                alt_env_destroy,
                storage_get_environment,
            })
        }
    }
}

// SAFETY: AntilatencyApi holds function pointers from loaded DLLs and the
// library handles. The pointers remain valid as long as the libraries are
// alive, which they are since we keep the Library values.  All Antilatency
// API calls happen on a single worker thread.
unsafe impl Send for AntilatencyApi {}
unsafe impl Sync for AntilatencyApi {}

// ---------------------------------------------------------------------------
// AntilatencySource
// ---------------------------------------------------------------------------

/// Antilatency tracking source node.
///
/// Reads 6-DOF tracking data from Antilatency Alt tracking devices via the
/// Antilatency SDK DLLs. If any required DLL cannot be loaded, the node logs
/// a warning and returns without error.
pub struct AntilatencySource {
    pub base: NodeBase,
    m_config: serde_json::Value,
    m_tag: String,
    m_environment_code: String,
    m_running: Arc<AtomicBool>,
    m_worker_handle: Option<JoinHandle<()>>,
}

impl AntilatencySource {
    pub fn new(name: impl Into<String>, config: &serde_json::Value) -> Self {
        let tag = config
            .get("tag")
            .and_then(|v| v.as_str())
            .unwrap_or("default")
            .to_string();
        let environment_code = config
            .get("environmentCode")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        Self {
            base: NodeBase::new(name),
            m_config: config.clone(),
            m_tag: tag,
            m_environment_code: environment_code,
            m_running: Arc::new(AtomicBool::new(false)),
            m_worker_handle: None,
        }
    }
}

impl Node for AntilatencySource {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn start(&mut self) -> anyhow::Result<()> {
        log::info!(
            "AntilatencySource '{}': starting (tag='{}', envCode='{}')",
            self.base.name(),
            self.m_tag,
            self.m_environment_code,
        );

        let api = match AntilatencyApi::load() {
            Ok(api) => {
                log::info!("AntilatencySource: all Antilatency SDK DLLs loaded successfully");
                Arc::new(api)
            }
            Err(e) => {
                log::warn!(
                    "AntilatencySource '{}': could not load Antilatency SDK DLLs: {}. \
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
        let tag = self.m_tag.clone();
        let env_code = self.m_environment_code.clone();

        self.m_worker_handle = Some(tokio::spawn(async move {
            let result = tokio::task::spawn_blocking(move || {
                Self::worker_loop(api, running, consumers, enabled, node_name, tag, env_code);
            })
            .await;

            if let Err(e) = result {
                log::warn!("AntilatencySource worker thread panicked: {}", e);
            }
        }));

        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        log::info!("AntilatencySource '{}': stopping", self.base.name());
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

impl AntilatencySource {
    /// Find the device index whose tag matches the requested tag string.
    /// Returns `None` if no matching device is found.
    fn find_device_by_tag(
        api: &AntilatencyApi,
        network: InterfacePtr,
        tag: &str,
    ) -> Option<u32> {
        let mut count: u32 = 0;
        let status = unsafe { (api.adn_get_device_count)(network, &mut count) };
        if status != ANTILATENCY_OK || count == 0 {
            return None;
        }

        // If tag is empty or "default", use the first available device
        if tag.is_empty() || tag == "default" {
            return Some(0);
        }

        let mut tag_buf = [0u8; 256];
        for idx in 0..count {
            let status = unsafe {
                (api.adn_get_device_tag)(
                    network,
                    idx,
                    tag_buf.as_mut_ptr(),
                    tag_buf.len() as u32,
                )
            };
            if status == ANTILATENCY_OK {
                let len = tag_buf
                    .iter()
                    .position(|&b| b == 0)
                    .unwrap_or(tag_buf.len());
                let device_tag = String::from_utf8_lossy(&tag_buf[..len]);
                if device_tag == tag {
                    return Some(idx);
                }
            }
        }

        None
    }

    fn worker_loop(
        api: Arc<AntilatencyApi>,
        running: Arc<AtomicBool>,
        consumers: Arc<Mutex<Vec<ConsumerCallback>>>,
        enabled: Arc<AtomicBool>,
        node_name: String,
        tag: String,
        env_code: String,
    ) {
        // Create device network
        let mut network: InterfacePtr = std::ptr::null_mut();
        let status = unsafe { (api.adn_create)(&mut network) };
        if status != ANTILATENCY_OK || network.is_null() {
            log::error!(
                "AntilatencySource: failed to create device network (status={})",
                status
            );
            return;
        }
        log::info!("AntilatencySource: device network created");

        // Create or load environment
        let mut environment: InterfacePtr = std::ptr::null_mut();
        if env_code.is_empty() {
            // Try to get environment from storage
            let mut env_buf = [0u8; 4096];
            let tag_bytes = b"default\0";
            let status = unsafe {
                (api.storage_get_environment)(
                    tag_bytes.as_ptr(),
                    tag_bytes.len() as u32,
                    env_buf.as_mut_ptr(),
                    env_buf.len() as u32,
                )
            };
            if status == ANTILATENCY_OK {
                let len = env_buf
                    .iter()
                    .position(|&b| b == 0)
                    .unwrap_or(env_buf.len());
                let code = &env_buf[..len];
                let env_status = unsafe {
                    (api.alt_env_create)(code.as_ptr(), code.len() as u32, &mut environment)
                };
                if env_status != ANTILATENCY_OK {
                    log::warn!(
                        "AntilatencySource: failed to create environment from storage (status={})",
                        env_status
                    );
                }
            } else {
                log::warn!(
                    "AntilatencySource: storage_get_environment failed (status={}), \
                     tracking without environment",
                    status
                );
            }
        } else {
            let code_bytes = env_code.as_bytes();
            let status = unsafe {
                (api.alt_env_create)(
                    code_bytes.as_ptr(),
                    code_bytes.len() as u32,
                    &mut environment,
                )
            };
            if status != ANTILATENCY_OK {
                log::warn!(
                    "AntilatencySource: failed to create environment from code (status={})",
                    status
                );
            }
        }

        // Wait for the target device and create tracker
        let mut tracker: InterfacePtr = std::ptr::null_mut();
        let mut last_update_id: u32 = 0;
        let mut frame_number: i32 = 0;
        let mut last_time = SystemTime::now();

        while running.load(Ordering::Relaxed) {
            // Check for device network updates
            let mut update_id: u32 = 0;
            let status = unsafe { (api.adn_get_update_id)(network, &mut update_id) };
            if status != ANTILATENCY_OK {
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }

            // If device network changed and we don't have a tracker, try to create one
            if tracker.is_null() && (update_id != last_update_id || last_update_id == 0) {
                last_update_id = update_id;

                if let Some(device_idx) = Self::find_device_by_tag(&api, network, &tag) {
                    log::info!(
                        "AntilatencySource: found device at index {} with tag '{}'",
                        device_idx,
                        tag
                    );
                    let env_ptr = if environment.is_null() {
                        std::ptr::null_mut()
                    } else {
                        environment
                    };
                    let status = unsafe {
                        (api.alt_create_tracker)(
                            network,
                            device_idx,
                            env_ptr,
                            &mut tracker,
                        )
                    };
                    if status != ANTILATENCY_OK || tracker.is_null() {
                        log::warn!(
                            "AntilatencySource: failed to create tracker (status={})",
                            status
                        );
                        tracker = std::ptr::null_mut();
                    } else {
                        log::info!("AntilatencySource: tracker created for device {}", device_idx);
                    }
                } else {
                    log::debug!(
                        "AntilatencySource: no device with tag '{}' found, retrying...",
                        tag
                    );
                    std::thread::sleep(Duration::from_millis(500));
                    continue;
                }
            }

            // If we still don't have a tracker, wait
            if tracker.is_null() {
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }

            // Get pose from tracker
            let mut pose = AltPose::default();
            let mut tracking_state = AltTrackingState::default();
            let status = unsafe { (api.alt_get_pose)(tracker, &mut pose, &mut tracking_state) };

            if status != ANTILATENCY_OK {
                log::warn!(
                    "AntilatencySource: alt_get_pose failed (status={}), tracker lost",
                    status
                );
                // Tracker lost – destroy and try to recreate
                unsafe { (api.alt_destroy_tracker)(tracker) };
                tracker = std::ptr::null_mut();
                continue;
            }

            let now = SystemTime::now();
            let interval = now
                .duration_since(last_time)
                .unwrap_or(Duration::from_millis(5));
            last_time = now;
            frame_number += 1;

            let quality = if tracking_state == ALT_TRACKING_STATE_TRACKING {
                1.0
            } else {
                0.0
            };

            let position = Vec3d::new(pose.px as f64, pose.py as f64, pose.pz as f64);
            let orientation = UnitQuaternion::from_quaternion(
                nalgebra::Quaternion::new(
                    pose.qw as f64,
                    pose.qx as f64,
                    pose.qy as f64,
                    pose.qz as f64,
                ),
            );

            let optical = OpticalData {
                sender_id: node_name.clone(),
                timestamp: now,
                last_data_time: now,
                latency: 0.0,
                position,
                orientation,
                angular_velocity: Vec3d::zeros(),
                quality,
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

            // Alt tracker typically runs at ~120 Hz
            std::thread::sleep(Duration::from_millis(2));
        }

        // Cleanup
        log::info!("AntilatencySource: shutting down");
        if !tracker.is_null() {
            unsafe { (api.alt_destroy_tracker)(tracker) };
        }
        if !environment.is_null() {
            unsafe { (api.alt_env_destroy)(environment) };
        }
        unsafe { (api.adn_destroy)(network) };
    }
}
