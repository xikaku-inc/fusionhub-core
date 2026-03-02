use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use libloading::Library;
use tokio::task::JoinHandle;

use fusion_types::{CANData, StreamableData};

use crate::node::{ConsumerCallback, Node, NodeBase};

// ---------------------------------------------------------------------------
// PCAN-Basic types and constants
// ---------------------------------------------------------------------------

type TPCANHandle = u16;
type TPCANBaudrate = u16;
type TPCANStatus = u32;

const PCAN_ERROR_OK: TPCANStatus = 0x00000;
const PCAN_ERROR_QRCVEMPTY: TPCANStatus = 0x00020;

const PCAN_USBBUS1: TPCANHandle = 0x51;
const PCAN_USBBUS2: TPCANHandle = 0x52;
const PCAN_USBBUS3: TPCANHandle = 0x53;
const PCAN_USBBUS4: TPCANHandle = 0x54;

const PCAN_BAUD_1M: TPCANBaudrate = 0x0014;
const PCAN_BAUD_500K: TPCANBaudrate = 0x001C;
const PCAN_BAUD_250K: TPCANBaudrate = 0x011C;
const PCAN_BAUD_125K: TPCANBaudrate = 0x031C;
const PCAN_BAUD_100K: TPCANBaudrate = 0x432F;

const PCAN_MESSAGE_EXTENDED: u8 = 0x02;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct TPCANMsg {
    id: u32,
    msg_type: u8,
    len: u8,
    data: [u8; 8],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct TPCANTimestamp {
    millis: u32,
    millis_overflow: u16,
    micros: u16,
}

// ---------------------------------------------------------------------------
// Function pointer types
// ---------------------------------------------------------------------------

type FnCanInitialize = unsafe extern "system" fn(TPCANHandle, TPCANBaudrate) -> TPCANStatus;
type FnCanRead =
    unsafe extern "system" fn(TPCANHandle, *mut TPCANMsg, *mut TPCANTimestamp) -> TPCANStatus;
type FnCanUninitialize = unsafe extern "system" fn(TPCANHandle) -> TPCANStatus;

/// Holds the loaded PCANBasic.dll and its function pointers.
struct PcanApi {
    _lib: Library,
    can_initialize: FnCanInitialize,
    can_read: FnCanRead,
    can_uninitialize: FnCanUninitialize,
}

impl PcanApi {
    fn load() -> Result<Self, libloading::Error> {
        let lib = unsafe { Library::new("PCANBasic.dll") }?;
        unsafe {
            let can_initialize = *lib.get::<FnCanInitialize>(b"CAN_Initialize\0")?;
            let can_read = *lib.get::<FnCanRead>(b"CAN_Read\0")?;
            let can_uninitialize = *lib.get::<FnCanUninitialize>(b"CAN_Uninitialize\0")?;

            Ok(Self {
                _lib: lib,
                can_initialize,
                can_read,
                can_uninitialize,
            })
        }
    }
}

// SAFETY: The PcanApi struct holds function pointers loaded from the DLL and the
// library handle itself. The function pointers remain valid for the library's
// lifetime. The PCAN-Basic API is documented as thread-safe for different channels.
unsafe impl Send for PcanApi {}
unsafe impl Sync for PcanApi {}

// ---------------------------------------------------------------------------
// Config helpers
// ---------------------------------------------------------------------------

/// Convert a channel name string to the PCAN handle constant.
fn parse_channel(s: &str) -> TPCANHandle {
    match s {
        "PCAN_USBBUS1" => PCAN_USBBUS1,
        "PCAN_USBBUS2" => PCAN_USBBUS2,
        "PCAN_USBBUS3" => PCAN_USBBUS3,
        "PCAN_USBBUS4" => PCAN_USBBUS4,
        _ => {
            // Try parsing as a hex or decimal number
            if let Some(hex) = s.strip_prefix("0x") {
                u16::from_str_radix(hex, 16).unwrap_or(PCAN_USBBUS1)
            } else {
                s.parse::<u16>().unwrap_or(PCAN_USBBUS1)
            }
        }
    }
}

/// Convert a baudrate in bits/s to the PCAN BTR0BTR1 constant.
fn baudrate_to_pcan(baud: u64) -> TPCANBaudrate {
    match baud {
        1_000_000 => PCAN_BAUD_1M,
        500_000 => PCAN_BAUD_500K,
        250_000 => PCAN_BAUD_250K,
        125_000 => PCAN_BAUD_125K,
        100_000 => PCAN_BAUD_100K,
        _ => {
            log::warn!(
                "PeakCanSource: unsupported baudrate {}, defaulting to 500K",
                baud
            );
            PCAN_BAUD_500K
        }
    }
}

// ---------------------------------------------------------------------------
// PeakCanSource
// ---------------------------------------------------------------------------

/// PEAK CAN source node.
///
/// Reads CAN bus data from PEAK-System PCAN hardware via `PCANBasic.dll`.
/// If the DLL is not present on the system, the node logs a warning and
/// returns without error.
pub struct PeakCanSource {
    pub base: NodeBase,
    m_config: serde_json::Value,
    m_baudrate: TPCANBaudrate,
    m_channel: TPCANHandle,
    m_running: Arc<AtomicBool>,
    m_worker_handle: Option<JoinHandle<()>>,
}

impl PeakCanSource {
    pub fn new(name: impl Into<String>, config: &serde_json::Value) -> Self {
        let baud_raw = config
            .get("baudrate")
            .and_then(|v| v.as_u64())
            .unwrap_or(500_000);
        let baudrate = baudrate_to_pcan(baud_raw);

        let channel = config
            .get("channel")
            .and_then(|v| v.as_str())
            .map(parse_channel)
            .unwrap_or(PCAN_USBBUS1);

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

impl Node for PeakCanSource {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn start(&mut self) -> anyhow::Result<()> {
        log::info!(
            "PeakCanSource '{}': starting (channel=0x{:04X}, baudrate=0x{:04X})",
            self.base.name(),
            self.m_channel,
            self.m_baudrate
        );

        let api = match PcanApi::load() {
            Ok(api) => {
                log::info!("PeakCanSource: PCANBasic.dll loaded successfully");
                Arc::new(api)
            }
            Err(e) => {
                log::warn!(
                    "PeakCanSource '{}': could not load PCANBasic.dll: {}. \
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
                log::warn!("PeakCanSource worker thread panicked: {}", e);
            }
        }));

        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        log::info!("PeakCanSource '{}': stopping", self.base.name());
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

impl PeakCanSource {
    fn worker_loop(
        api: Arc<PcanApi>,
        running: Arc<AtomicBool>,
        consumers: Arc<Mutex<Vec<ConsumerCallback>>>,
        enabled: Arc<AtomicBool>,
        node_name: String,
        baudrate: TPCANBaudrate,
        channel: TPCANHandle,
    ) {
        // Initialize the channel
        let status = unsafe { (api.can_initialize)(channel, baudrate) };
        if status != PCAN_ERROR_OK {
            log::error!(
                "PeakCanSource: CAN_Initialize failed with status 0x{:08X}",
                status
            );
            return;
        }
        log::info!(
            "PeakCanSource: CAN channel 0x{:04X} initialized successfully",
            channel
        );

        // Receive loop
        while running.load(Ordering::Relaxed) {
            let mut msg = TPCANMsg::default();
            let mut ts = TPCANTimestamp::default();

            let status = unsafe { (api.can_read)(channel, &mut msg, &mut ts) };

            if status == PCAN_ERROR_QRCVEMPTY {
                // No message available – brief sleep to avoid busy-waiting
                std::thread::sleep(std::time::Duration::from_millis(1));
                continue;
            }

            if status != PCAN_ERROR_OK {
                log::warn!(
                    "PeakCanSource: CAN_Read returned status 0x{:08X}",
                    status
                );
                std::thread::sleep(std::time::Duration::from_millis(10));
                continue;
            }

            let is_extended = (msg.msg_type & PCAN_MESSAGE_EXTENDED) != 0;
            let dlc = msg.len.min(8) as usize;

            let can_data = CANData {
                sender_id: node_name.clone(),
                timestamp: SystemTime::now(),
                is_extended,
                id: msg.id,
                length: dlc as i32,
                data: msg.data[..dlc].to_vec(),
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
        log::info!("PeakCanSource: uninitializing CAN channel 0x{:04X}", channel);
        let status = unsafe { (api.can_uninitialize)(channel) };
        if status != PCAN_ERROR_OK {
            log::warn!(
                "PeakCanSource: CAN_Uninitialize returned status 0x{:08X}",
                status
            );
        }
    }
}
