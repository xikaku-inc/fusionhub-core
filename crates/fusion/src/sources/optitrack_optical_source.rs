use std::ffi::{c_void, CString};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use libloading::Library;
use nalgebra::UnitQuaternion;
use tokio::task::JoinHandle;

use fusion_types::{OpticalData, StreamableData, Vec3d};

use crate::node::{ConsumerCallback, Node, NodeBase};

// ---------------------------------------------------------------------------
// NatNet C API types
// ---------------------------------------------------------------------------

type NatNetClient = *mut c_void;

/// Connection parameters passed to NatNet_ConnectClient.
#[repr(C)]
struct NatNetConnectParams {
    connection_type: i32, // 0 = multicast, 1 = unicast
    server_address: *const u8,
    local_address: *const u8,
    server_command_port: u16,
    server_data_port: u16,
    multicast_address: *const u8,
}

/// Rigid body data received per frame.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct RigidBodyData {
    id: i32,
    x: f32,
    y: f32,
    z: f32,
    qx: f32,
    qy: f32,
    qz: f32,
    qw: f32,
    mean_error: f32,
    params: u16,
}

/// Simplified frame of mocap data containing only rigid body section.
/// The real NatNet struct is much larger; we use a polling API that
/// returns rigid body data directly.
#[repr(C)]
struct FrameOfMocapData {
    frame_number: i32,
    n_rigid_bodies: i32,
    rigid_bodies: *const RigidBodyData,
}

// ---------------------------------------------------------------------------
// Function pointer types
// ---------------------------------------------------------------------------

type FnNatNetCreate = unsafe extern "C" fn() -> NatNetClient;
type FnNatNetConnect =
    unsafe extern "C" fn(NatNetClient, *const NatNetConnectParams) -> i32;
type FnNatNetGetLastFrame =
    unsafe extern "C" fn(NatNetClient, *mut FrameOfMocapData) -> i32;
type FnNatNetDestroy = unsafe extern "C" fn(NatNetClient);

/// Holds the loaded NatNetLib.dll and its function pointers.
struct NatNetApi {
    _lib: Library,
    natnet_create: FnNatNetCreate,
    natnet_connect: FnNatNetConnect,
    natnet_get_last_frame: FnNatNetGetLastFrame,
    natnet_destroy: FnNatNetDestroy,
}

impl NatNetApi {
    fn load() -> Result<Self, libloading::Error> {
        let lib = unsafe { Library::new("NatNetLib.dll") }?;
        unsafe {
            let natnet_create = *lib.get::<FnNatNetCreate>(b"NatNet_CreateClient\0")?;
            let natnet_connect = *lib.get::<FnNatNetConnect>(b"NatNet_ConnectClient\0")?;
            let natnet_get_last_frame =
                *lib.get::<FnNatNetGetLastFrame>(b"NatNet_GetLastFrameOfData\0")?;
            let natnet_destroy = *lib.get::<FnNatNetDestroy>(b"NatNet_DestroyClient\0")?;

            Ok(Self {
                _lib: lib,
                natnet_create,
                natnet_connect,
                natnet_get_last_frame,
                natnet_destroy,
            })
        }
    }
}

// SAFETY: NatNetApi holds function pointers loaded from the DLL. The pointers
// are valid for the library's lifetime. The NatNet client is used exclusively
// from one worker thread.
unsafe impl Send for NatNetApi {}
unsafe impl Sync for NatNetApi {}

// ---------------------------------------------------------------------------
// OptitrackOpticalSource
// ---------------------------------------------------------------------------

/// OptiTrack/NatNet optical tracking source node.
///
/// Reads optical tracking data from an OptiTrack system via the NatNet SDK
/// (`NatNetLib.dll`). If the DLL is not present on the system, the node logs
/// a warning and returns without error.
pub struct OptitrackOpticalSource {
    pub base: NodeBase,
    m_config: serde_json::Value,
    m_server_address: String,
    m_local_address: String,
    m_running: Arc<AtomicBool>,
    m_worker_handle: Option<JoinHandle<()>>,
}

impl OptitrackOpticalSource {
    pub fn new(name: impl Into<String>, config: &serde_json::Value) -> Self {
        let server_address = config
            .get("serverAddress")
            .and_then(|v| v.as_str())
            .unwrap_or("127.0.0.1")
            .to_string();
        let local_address = config
            .get("localAddress")
            .and_then(|v| v.as_str())
            .unwrap_or("127.0.0.1")
            .to_string();

        Self {
            base: NodeBase::new(name),
            m_config: config.clone(),
            m_server_address: server_address,
            m_local_address: local_address,
            m_running: Arc::new(AtomicBool::new(false)),
            m_worker_handle: None,
        }
    }
}

impl Node for OptitrackOpticalSource {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn start(&mut self) -> anyhow::Result<()> {
        log::info!(
            "OptitrackOpticalSource '{}': starting (server={}, local={})",
            self.base.name(),
            self.m_server_address,
            self.m_local_address,
        );

        let api = match NatNetApi::load() {
            Ok(api) => {
                log::info!("OptitrackOpticalSource: NatNetLib.dll loaded successfully");
                Arc::new(api)
            }
            Err(e) => {
                log::warn!(
                    "OptitrackOpticalSource '{}': could not load NatNetLib.dll: {}. \
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
        let server_addr = self.m_server_address.clone();
        let local_addr = self.m_local_address.clone();

        self.m_worker_handle = Some(tokio::spawn(async move {
            let result = tokio::task::spawn_blocking(move || {
                Self::worker_loop(
                    api,
                    running,
                    consumers,
                    enabled,
                    node_name,
                    server_addr,
                    local_addr,
                );
            })
            .await;

            if let Err(e) = result {
                log::warn!("OptitrackOpticalSource worker thread panicked: {}", e);
            }
        }));

        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        log::info!("OptitrackOpticalSource '{}': stopping", self.base.name());
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

impl OptitrackOpticalSource {
    fn worker_loop(
        api: Arc<NatNetApi>,
        running: Arc<AtomicBool>,
        consumers: Arc<Mutex<Vec<ConsumerCallback>>>,
        enabled: Arc<AtomicBool>,
        node_name: String,
        server_addr: String,
        local_addr: String,
    ) {
        // Create client
        let client = unsafe { (api.natnet_create)() };
        if client.is_null() {
            log::error!("OptitrackOpticalSource: NatNet_CreateClient returned null");
            return;
        }

        // Prepare connection parameters
        let server_cstr = CString::new(server_addr.as_str()).unwrap_or_default();
        let local_cstr = CString::new(local_addr.as_str()).unwrap_or_default();
        let multicast_cstr = CString::new("239.255.42.99").unwrap_or_default();

        let params = NatNetConnectParams {
            connection_type: 0, // multicast
            server_address: server_cstr.as_ptr() as *const u8,
            local_address: local_cstr.as_ptr() as *const u8,
            server_command_port: 1510,
            server_data_port: 1511,
            multicast_address: multicast_cstr.as_ptr() as *const u8,
        };

        let status = unsafe { (api.natnet_connect)(client, &params) };
        if status != 0 {
            log::error!(
                "OptitrackOpticalSource: NatNet_ConnectClient failed with status {}",
                status
            );
            unsafe { (api.natnet_destroy)(client) };
            return;
        }
        log::info!(
            "OptitrackOpticalSource: connected to OptiTrack server at '{}'",
            server_addr
        );

        let mut last_time = SystemTime::now();

        // Polling loop
        while running.load(Ordering::Relaxed) {
            let mut frame = FrameOfMocapData {
                frame_number: 0,
                n_rigid_bodies: 0,
                rigid_bodies: std::ptr::null(),
            };

            let status = unsafe { (api.natnet_get_last_frame)(client, &mut frame) };
            if status != 0 {
                std::thread::sleep(Duration::from_millis(1));
                continue;
            }

            if frame.n_rigid_bodies <= 0 || frame.rigid_bodies.is_null() {
                std::thread::sleep(Duration::from_millis(1));
                continue;
            }

            // Read rigid bodies from the frame
            let rigid_bodies = unsafe {
                std::slice::from_raw_parts(frame.rigid_bodies, frame.n_rigid_bodies as usize)
            };

            let now = SystemTime::now();
            let interval = now
                .duration_since(last_time)
                .unwrap_or(Duration::from_millis(8));
            last_time = now;

            for rb in rigid_bodies {
                let position = Vec3d::new(rb.x as f64, rb.y as f64, rb.z as f64);
                let orientation = UnitQuaternion::from_quaternion(
                    nalgebra::Quaternion::new(
                        rb.qw as f64,
                        rb.qx as f64,
                        rb.qy as f64,
                        rb.qz as f64,
                    ),
                );

                // Tracking validity: bit 0 of params indicates tracking was valid
                let quality = if rb.params & 0x01 != 0 { 1.0 } else { 0.0 };

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
                    frame_number: frame.frame_number,
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

            // Brief sleep to avoid busy-waiting between frames
            std::thread::sleep(Duration::from_millis(1));
        }

        // Cleanup
        log::info!("OptitrackOpticalSource: destroying NatNet client");
        unsafe {
            (api.natnet_destroy)(client);
        }
    }
}
