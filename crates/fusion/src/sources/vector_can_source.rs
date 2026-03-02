use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use libloading::Library;
use tokio::task::JoinHandle;

use fusion_types::{CANData, StreamableData};

use crate::node::{ConsumerCallback, Node, NodeBase};

// ---------------------------------------------------------------------------
// Vector XL Driver Library types
// ---------------------------------------------------------------------------

type XLstatus = i32;
type XLportHandle = isize;
type XLaccess = u64;

const XL_SUCCESS: XLstatus = 0;
const XL_BUS_TYPE_CAN: u32 = 0x0000_0001;
const XL_INTERFACE_VERSION: u32 = 3; // XL_INTERFACE_VERSION_V3
const XL_ACTIVATE_NONE: u32 = 0;

/// Mirrors the fixed-size driver config structure from vxlapi.h.
/// We only need the channel count and access mask per channel.
const XL_CONFIG_MAX_CHANNELS: usize = 64;

#[repr(C)]
#[derive(Copy, Clone)]
struct XLchannelConfig {
    _padding: [u8; 344], // opaque – we only care about channelIndex at offset 0
}

impl Default for XLchannelConfig {
    fn default() -> Self {
        Self {
            _padding: [0u8; 344],
        }
    }
}

#[repr(C)]
struct XLdriverConfig {
    dll_version: u32,
    channel_count: u32,
    _reserved: [u32; 10],
    channel: [XLchannelConfig; XL_CONFIG_MAX_CHANNELS],
}

impl Default for XLdriverConfig {
    fn default() -> Self {
        Self {
            dll_version: 0,
            channel_count: 0,
            _reserved: [0u32; 10],
            channel: [XLchannelConfig::default(); XL_CONFIG_MAX_CHANNELS],
        }
    }
}

/// Simplified CAN receive event (XL_CAN_EV_TAG_RX_OK payload).
#[repr(C)]
struct XLcanRxEvent {
    size: u32,
    tag: u16,
    channel_index: u8,
    _padding1: u8,
    user_handle: u32,
    flags: u16,
    _padding2: u16,
    timestamp: u64,
    // For tag == XL_CAN_EV_TAG_RX_OK the payload follows:
    can_id: u32,
    can_flags: u16,
    can_crc: u16,
    can_dlc: u8,
    _reserved_data: [u8; 7],
    can_data: [u8; 64],
}

impl Default for XLcanRxEvent {
    fn default() -> Self {
        Self {
            size: 0,
            tag: 0,
            channel_index: 0,
            _padding1: 0,
            user_handle: 0,
            flags: 0,
            _padding2: 0,
            timestamp: 0,
            can_id: 0,
            can_flags: 0,
            can_crc: 0,
            can_dlc: 0,
            _reserved_data: [0u8; 7],
            can_data: [0u8; 64],
        }
    }
}

const XL_CAN_EV_TAG_RX_OK: u16 = 0x0400;

// ---------------------------------------------------------------------------
// Type aliases for loaded functions
// ---------------------------------------------------------------------------

type FnXlOpenDriver = unsafe extern "system" fn() -> XLstatus;
type FnXlCloseDriver = unsafe extern "system" fn() -> XLstatus;
type FnXlGetDriverConfig = unsafe extern "system" fn(*mut XLdriverConfig) -> XLstatus;
type FnXlGetApplConfig =
    unsafe extern "system" fn(*const u8, u32, *mut u32, *mut u32, *mut u32, u32) -> XLstatus;
type FnXlOpenPort = unsafe extern "system" fn(
    *mut XLportHandle,
    *const u8,
    XLaccess,
    *mut XLaccess,
    u32,
    u32,
    u32,
) -> XLstatus;
type FnXlCanSetChannelBitrate =
    unsafe extern "system" fn(XLportHandle, XLaccess, u32) -> XLstatus;
type FnXlActivateChannel =
    unsafe extern "system" fn(XLportHandle, XLaccess, u32, u32) -> XLstatus;
type FnXlSetNotification =
    unsafe extern "system" fn(XLportHandle, *mut isize, i32) -> XLstatus;
type FnXlCanReceive = unsafe extern "system" fn(XLportHandle, *mut XLcanRxEvent) -> XLstatus;
type FnXlDeactivateChannel =
    unsafe extern "system" fn(XLportHandle, XLaccess) -> XLstatus;
type FnXlClosePort = unsafe extern "system" fn(XLportHandle) -> XLstatus;

/// Holds the loaded vxlapi64.dll and its function pointers.
struct VxlApi {
    _lib: Library,
    xl_open_driver: FnXlOpenDriver,
    xl_close_driver: FnXlCloseDriver,
    xl_get_driver_config: FnXlGetDriverConfig,
    xl_get_appl_config: FnXlGetApplConfig,
    xl_open_port: FnXlOpenPort,
    xl_can_set_channel_bitrate: FnXlCanSetChannelBitrate,
    xl_activate_channel: FnXlActivateChannel,
    xl_set_notification: FnXlSetNotification,
    xl_can_receive: FnXlCanReceive,
    xl_deactivate_channel: FnXlDeactivateChannel,
    xl_close_port: FnXlClosePort,
}

impl VxlApi {
    fn load() -> Result<Self, libloading::Error> {
        let lib = unsafe { Library::new("vxlapi64.dll") }?;
        unsafe {
            let xl_open_driver = *lib.get::<FnXlOpenDriver>(b"xlOpenDriver\0")?;
            let xl_close_driver = *lib.get::<FnXlCloseDriver>(b"xlCloseDriver\0")?;
            let xl_get_driver_config =
                *lib.get::<FnXlGetDriverConfig>(b"xlGetDriverConfig\0")?;
            let xl_get_appl_config =
                *lib.get::<FnXlGetApplConfig>(b"xlGetApplConfig\0")?;
            let xl_open_port = *lib.get::<FnXlOpenPort>(b"xlOpenPort\0")?;
            let xl_can_set_channel_bitrate =
                *lib.get::<FnXlCanSetChannelBitrate>(b"xlCanSetChannelBitrate\0")?;
            let xl_activate_channel =
                *lib.get::<FnXlActivateChannel>(b"xlActivateChannel\0")?;
            let xl_set_notification =
                *lib.get::<FnXlSetNotification>(b"xlSetNotification\0")?;
            let xl_can_receive = *lib.get::<FnXlCanReceive>(b"xlCanReceive\0")?;
            let xl_deactivate_channel =
                *lib.get::<FnXlDeactivateChannel>(b"xlDeactivateChannel\0")?;
            let xl_close_port = *lib.get::<FnXlClosePort>(b"xlClosePort\0")?;

            Ok(Self {
                _lib: lib,
                xl_open_driver,
                xl_close_driver,
                xl_get_driver_config,
                xl_get_appl_config,
                xl_open_port,
                xl_can_set_channel_bitrate,
                xl_activate_channel,
                xl_set_notification,
                xl_can_receive,
                xl_deactivate_channel,
                xl_close_port,
            })
        }
    }
}

// SAFETY: The VxlApi struct contains only function pointers and the library handle.
// The function pointers are valid for the lifetime of the Library, and we ensure
// the library is kept alive alongside the pointers. The DLL functions themselves
// are thread-safe per the Vector XL API specification.
unsafe impl Send for VxlApi {}
unsafe impl Sync for VxlApi {}

// ---------------------------------------------------------------------------
// VectorCanSource
// ---------------------------------------------------------------------------

/// Vector CAN source node.
///
/// Reads CAN bus data from Vector VN series hardware via the XL Driver Library
/// (`vxlapi64.dll`). If the DLL is not present on the system, the node logs
/// a warning and returns without error.
pub struct VectorCanSource {
    pub base: NodeBase,
    m_config: serde_json::Value,
    m_baudrate: u32,
    m_channel: u32,
    m_running: Arc<AtomicBool>,
    m_worker_handle: Option<JoinHandle<()>>,
}

impl VectorCanSource {
    pub fn new(name: impl Into<String>, config: &serde_json::Value) -> Self {
        let baudrate = config
            .get("baudrate")
            .and_then(|v| v.as_u64())
            .unwrap_or(500_000) as u32;
        let channel = config
            .get("channel")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;

        Self {
            base: NodeBase::new(name),
            m_config: config.clone(),
            m_baudrate: baudrate,
            m_channel: channel,
            m_running: Arc::new(AtomicBool::new(false)),
            m_worker_handle: None,
        }
    }
}

impl Node for VectorCanSource {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn start(&mut self) -> anyhow::Result<()> {
        log::info!(
            "VectorCanSource '{}': starting (channel={}, baudrate={})",
            self.base.name(),
            self.m_channel,
            self.m_baudrate
        );

        // Attempt to load the DLL
        let api = match VxlApi::load() {
            Ok(api) => {
                log::info!("VectorCanSource: vxlapi64.dll loaded successfully");
                Arc::new(api)
            }
            Err(e) => {
                log::warn!(
                    "VectorCanSource '{}': could not load vxlapi64.dll: {}. \
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
        let baudrate = self.m_baudrate;
        let channel = self.m_channel;

        self.m_worker_handle = Some(tokio::spawn(async move {
            let result = tokio::task::spawn_blocking(move || {
                Self::worker_loop(api, running, consumers, enabled, node_name, baudrate, channel);
            })
            .await;

            if let Err(e) = result {
                log::warn!("VectorCanSource worker thread panicked: {}", e);
            }
        }));

        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        log::info!("VectorCanSource '{}': stopping", self.base.name());
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

impl VectorCanSource {
    fn worker_loop(
        api: Arc<VxlApi>,
        running: Arc<AtomicBool>,
        consumers: Arc<Mutex<Vec<ConsumerCallback>>>,
        enabled: Arc<AtomicBool>,
        node_name: String,
        baudrate: u32,
        channel: u32,
    ) {
        // Open driver
        let status = unsafe { (api.xl_open_driver)() };
        if status != XL_SUCCESS {
            log::error!("VectorCanSource: xlOpenDriver failed with status {}", status);
            return;
        }

        // Get driver config
        let mut driver_config = XLdriverConfig::default();
        let status = unsafe { (api.xl_get_driver_config)(&mut driver_config) };
        if status != XL_SUCCESS {
            log::error!(
                "VectorCanSource: xlGetDriverConfig failed with status {}",
                status
            );
            unsafe { (api.xl_close_driver)() };
            return;
        }
        log::info!(
            "VectorCanSource: driver config has {} channels",
            driver_config.channel_count
        );

        // Build access mask for requested channel
        let access_mask: XLaccess = 1u64 << channel;

        // Get application config
        let app_name = b"FusionHub\0";
        let mut hw_type: u32 = 0;
        let mut hw_index: u32 = 0;
        let mut hw_channel: u32 = 0;
        let status = unsafe {
            (api.xl_get_appl_config)(
                app_name.as_ptr(),
                channel,
                &mut hw_type,
                &mut hw_index,
                &mut hw_channel,
                XL_BUS_TYPE_CAN,
            )
        };
        if status != XL_SUCCESS {
            log::warn!(
                "VectorCanSource: xlGetApplConfig returned {} (may be unconfigured, using defaults)",
                status
            );
        }

        // Open port
        let mut port_handle: XLportHandle = -1;
        let mut permission_mask: XLaccess = access_mask;
        let user_name = b"FusionHub\0";
        let status = unsafe {
            (api.xl_open_port)(
                &mut port_handle,
                user_name.as_ptr(),
                access_mask,
                &mut permission_mask,
                256, // rxQueueSize
                XL_INTERFACE_VERSION,
                XL_BUS_TYPE_CAN,
            )
        };
        if status != XL_SUCCESS {
            log::error!("VectorCanSource: xlOpenPort failed with status {}", status);
            unsafe { (api.xl_close_driver)() };
            return;
        }

        // Set bitrate (only if we have permission)
        if permission_mask & access_mask != 0 {
            let status =
                unsafe { (api.xl_can_set_channel_bitrate)(port_handle, access_mask, baudrate) };
            if status != XL_SUCCESS {
                log::warn!(
                    "VectorCanSource: xlCanSetChannelBitrate failed with status {}",
                    status
                );
            }
        }

        // Activate channel
        let status = unsafe {
            (api.xl_activate_channel)(port_handle, access_mask, XL_BUS_TYPE_CAN, XL_ACTIVATE_NONE)
        };
        if status != XL_SUCCESS {
            log::error!(
                "VectorCanSource: xlActivateChannel failed with status {}",
                status
            );
            unsafe {
                (api.xl_close_port)(port_handle);
                (api.xl_close_driver)();
            }
            return;
        }

        // Set notification
        let mut event_handle: isize = 0;
        let status =
            unsafe { (api.xl_set_notification)(port_handle, &mut event_handle, 1) };
        if status != XL_SUCCESS {
            log::warn!(
                "VectorCanSource: xlSetNotification failed with status {} (polling mode)",
                status
            );
        }

        log::info!("VectorCanSource: receiving CAN data...");

        // Receive loop
        while running.load(Ordering::Relaxed) {
            let mut rx_event = XLcanRxEvent::default();
            let status = unsafe { (api.xl_can_receive)(port_handle, &mut rx_event) };

            if status != XL_SUCCESS {
                // XL_ERR_QUEUE_IS_EMPTY = 10 – expected when no data available
                if status == 10 {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                    continue;
                }
                log::warn!("VectorCanSource: xlCanReceive returned status {}", status);
                std::thread::sleep(std::time::Duration::from_millis(10));
                continue;
            }

            if rx_event.tag != XL_CAN_EV_TAG_RX_OK {
                continue;
            }

            let is_extended = (rx_event.can_id & 0x8000_0000) != 0;
            let can_id = rx_event.can_id & 0x1FFF_FFFF;
            let dlc = rx_event.can_dlc.min(8) as usize;

            let can_data = CANData {
                sender_id: node_name.clone(),
                timestamp: SystemTime::now(),
                is_extended,
                id: can_id,
                length: dlc as i32,
                data: rx_event.can_data[..dlc].to_vec(),
            };

            if enabled.load(Ordering::Relaxed) {
                let cbs = consumers.lock().unwrap();
                let streamable = StreamableData::Can(can_data);
                for cb in cbs.iter() {
                    cb(streamable.clone());
                }
            }
        }

        // Cleanup
        log::info!("VectorCanSource: shutting down XL port");
        unsafe {
            (api.xl_deactivate_channel)(port_handle, access_mask);
            (api.xl_close_port)(port_handle);
            (api.xl_close_driver)();
        }
    }
}
