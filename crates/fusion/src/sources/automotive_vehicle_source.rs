use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use fusion_types::{CANData, StreamableData, VehicleSpeed, VehicleState};
use serde_json::json;

use crate::node::{ConsumerCallback, Node, NodeBase};
use crate::sources::peak_can_source::PeakCanSource;
use crate::sources::vector_can_source::VectorCanSource;
use fusion_registry::{sf, SettingsField};

pub fn settings_schema() -> Vec<SettingsField> {
    vec![
        sf("canInterface", "CAN Interface", "string", json!("PeakCAN")),
        sf("baudrate", "Baudrate", "number", json!(500000)),
        sf("channel", "Channel", "number", json!(0)),
    ]
}

pub fn minimal_settings_schema() -> Vec<SettingsField> {
    vec![
        sf("speedCanId", "Speed CAN ID", "number", json!(0)),
        sf("speedByteOffset", "Speed Byte Offset", "number", json!(0)),
        sf("speedScale", "Speed Scale", "number", json!(1.0)),
    ]
}

pub fn external_settings_schema() -> Vec<SettingsField> {
    vec![
        sf("speedCanId", "Speed CAN ID", "number", json!(0)),
        sf("speedStartByte", "Speed Start Byte", "number", json!(0)),
        sf("speedLength", "Speed Length (bytes)", "number", json!(1)),
        sf("speedScale", "Speed Scale", "number", json!(1.0)),
        sf("speedOffset", "Speed Offset", "number", json!(0.0)),
        sf("speedIsBigEndian", "Speed Big Endian", "boolean", json!(false)),
        sf("steeringCanId", "Steering CAN ID", "number", json!(0)),
        sf("steeringStartByte", "Steering Start Byte", "number", json!(0)),
        sf("steeringLength", "Steering Length (bytes)", "number", json!(1)),
        sf("steeringScale", "Steering Scale", "number", json!(1.0)),
        sf("steeringOffset", "Steering Offset", "number", json!(0.0)),
        sf("wheelBase", "Wheel Base (m)", "number", json!(2.9)),
        sf("trackWidth", "Track Width (m)", "number", json!(1.6)),
    ]
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CanInterface {
    None,
    PeakCan,
    Vector,
    Internal,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum VehicleType {
    Preconfigured,
    External,
    Minimal,
}

/// Minimal CAN parser that extracts vehicle speed from a single CAN message.
struct MinimalCanParser {
    m_speed_can_id: u32,
    m_speed_byte_offset: usize,
    m_speed_scale: f64,
}

impl MinimalCanParser {
    fn new(config: &serde_json::Value) -> Self {
        Self {
            m_speed_can_id: config
                .get("speedCanId")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            m_speed_byte_offset: config
                .get("speedByteOffset")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize,
            m_speed_scale: config
                .get("speedScale")
                .and_then(|v| v.as_f64())
                .unwrap_or(1.0),
        }
    }

    fn parse(&self, data: &CANData) -> Option<(VehicleState, VehicleSpeed)> {
        let state = VehicleState {
            sender_id: data.sender_id.clone(),
            timestamp: data.timestamp,
            ..Default::default()
        };

        let mut speed = VehicleSpeed {
            sender_id: data.sender_id.clone(),
            timestamp: data.timestamp,
            ..Default::default()
        };

        if data.id == self.m_speed_can_id && data.data.len() > self.m_speed_byte_offset {
            let raw_speed = data.data[self.m_speed_byte_offset] as f64;
            speed.linear = raw_speed * self.m_speed_scale / 3.6;
        }

        Some((state, speed))
    }
}

/// External CAN parser using configurable CAN message definitions.
struct ExternalCanParser {
    m_speed_can_id: u32,
    m_speed_start_byte: usize,
    m_speed_length: usize,
    m_speed_scale: f64,
    m_speed_offset: f64,
    m_speed_is_big_endian: bool,
    m_steering_can_id: u32,
    m_steering_start_byte: usize,
    m_steering_length: usize,
    m_steering_scale: f64,
    m_steering_offset: f64,
    m_wheel_base: f64,
    m_track_width: f64,
}

impl ExternalCanParser {
    fn new(config: &serde_json::Value) -> Self {
        Self {
            m_speed_can_id: config
                .get("speedCanId")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            m_speed_start_byte: config
                .get("speedStartByte")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize,
            m_speed_length: config
                .get("speedLength")
                .and_then(|v| v.as_u64())
                .unwrap_or(1) as usize,
            m_speed_scale: config
                .get("speedScale")
                .and_then(|v| v.as_f64())
                .unwrap_or(1.0),
            m_speed_offset: config
                .get("speedOffset")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0),
            m_speed_is_big_endian: config
                .get("speedIsBigEndian")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            m_steering_can_id: config
                .get("steeringCanId")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            m_steering_start_byte: config
                .get("steeringStartByte")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize,
            m_steering_length: config
                .get("steeringLength")
                .and_then(|v| v.as_u64())
                .unwrap_or(1) as usize,
            m_steering_scale: config
                .get("steeringScale")
                .and_then(|v| v.as_f64())
                .unwrap_or(1.0),
            m_steering_offset: config
                .get("steeringOffset")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0),
            m_wheel_base: config
                .get("wheelBase")
                .and_then(|v| v.as_f64())
                .unwrap_or(2.9),
            m_track_width: config
                .get("trackWidth")
                .and_then(|v| v.as_f64())
                .unwrap_or(1.6),
        }
    }

    fn extract_value(&self, data: &[u8], start: usize, len: usize, big_endian: bool) -> f64 {
        if start + len > data.len() {
            return 0.0;
        }
        let bytes = &data[start..start + len];
        match len {
            1 => bytes[0] as f64,
            2 => {
                if big_endian {
                    u16::from_be_bytes([bytes[0], bytes[1]]) as f64
                } else {
                    u16::from_le_bytes([bytes[0], bytes[1]]) as f64
                }
            }
            4 => {
                if big_endian {
                    u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f64
                } else {
                    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f64
                }
            }
            _ => 0.0,
        }
    }

    fn parse(&self, data: &CANData, current_state: &mut VehicleState, current_speed: &mut VehicleSpeed) {
        current_state.timestamp = data.timestamp;
        current_state.sender_id = data.sender_id.clone();
        current_state.wheel_base = self.m_wheel_base;
        current_state.track_width = self.m_track_width;
        current_speed.timestamp = data.timestamp;
        current_speed.sender_id = data.sender_id.clone();

        if data.id == self.m_speed_can_id {
            let raw = self.extract_value(
                &data.data,
                self.m_speed_start_byte,
                self.m_speed_length,
                self.m_speed_is_big_endian,
            );
            current_speed.linear = (raw * self.m_speed_scale + self.m_speed_offset) / 3.6;
        }

        if data.id == self.m_steering_can_id {
            let raw = self.extract_value(
                &data.data,
                self.m_steering_start_byte,
                self.m_steering_length,
                false,
            );
            let steering_angle = raw * self.m_steering_scale + self.m_steering_offset;
            current_state.steering_angle_l = steering_angle;
            current_state.steering_angle_r = steering_angle;
            current_speed.valid_angular = true;
        }
    }
}

/// Automotive vehicle source node.
///
/// Parses CAN bus data into VehicleState and VehicleSpeed.
/// Supports multiple CAN interfaces (PeakCAN, Vector, Internal) and
/// configurable vehicle types with CAN message definitions.
pub struct AutomotiveVehicleSource {
    pub base: NodeBase,
    m_can_interface: CanInterface,
    m_vehicle_type: VehicleType,
    m_baudrate: u32,
    m_channel: u32,
    m_current_state: Arc<Mutex<VehicleState>>,
    m_current_speed: Arc<Mutex<VehicleSpeed>>,
    m_config: serde_json::Value,
    m_done: Arc<AtomicBool>,
    m_can_source: Option<Box<dyn Node>>,
}

impl AutomotiveVehicleSource {
    pub fn new(name: impl Into<String>, config: &serde_json::Value) -> Self {
        let name = name.into();

        let baudrate = config
            .get("baudrate")
            .and_then(|v| v.as_u64())
            .unwrap_or(500000) as u32;
        let channel = config
            .get("channel")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;

        let ci = config
            .get("canInterface")
            .and_then(|v| v.as_str())
            .unwrap_or("PeakCAN");
        let can_interface = match ci {
            "Internal" => {
                log::info!("Internal CAN interface");
                CanInterface::Internal
            }
            "PeakCAN" => CanInterface::PeakCan,
            "Vector" => CanInterface::Vector,
            _ => {
                log::info!("Unsupported CAN interface: {}", ci);
                CanInterface::None
            }
        };

        let vt = config
            .get("vehicleType")
            .or_else(|| config.get("type"))
            .and_then(|v| v.as_str())
            .unwrap_or("Default");
        let vehicle_type = match vt {
            "Minimal" => VehicleType::Minimal,
            "External" | "ExternalWithSteering" => VehicleType::External,
            _ => VehicleType::Preconfigured,
        };

        log::info!("Created automotive vehicle node (interface={}, type={})", ci, vt);

        Self {
            base: NodeBase::new(&name),
            m_can_interface: can_interface,
            m_vehicle_type: vehicle_type,
            m_baudrate: baudrate,
            m_channel: channel,
            m_current_state: Arc::new(Mutex::new(VehicleState::default())),
            m_current_speed: Arc::new(Mutex::new(VehicleSpeed::default())),
            m_config: config.clone(),
            m_done: Arc::new(AtomicBool::new(false)),
            m_can_source: None,
        }
    }

    /// Process incoming CAN data. Called when this source receives CAN data
    /// from an internal subscriber.
    pub fn process_can_data(&self, data: &CANData) {
        let parser = ExternalCanParser::new(&self.m_config);
        let mut state = self.m_current_state.lock().unwrap();
        let mut speed = self.m_current_speed.lock().unwrap();
        parser.parse(data, &mut state, &mut speed);

        if self.base.is_enabled() {
            self.base
                .notify_consumers(StreamableData::VehicleState(state.clone()));
            self.base
                .notify_consumers(StreamableData::VehicleSpeed(speed.clone()));
        }
    }
}

impl Node for AutomotiveVehicleSource {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn start(&mut self) -> anyhow::Result<()> {
        log::info!("Starting automotive vehicle source: {}", self.base.name());

        match self.m_can_interface {
            CanInterface::Internal => {
                log::info!("Automotive source using internal CAN interface (awaiting data)");
            }
            CanInterface::PeakCan => {
                log::info!(
                    "AutomotiveVehicleSource '{}': creating internal PeakCAN source",
                    self.base.name()
                );
                let mut can_source = PeakCanSource::new(
                    format!("{}_peak_can", self.base.name()),
                    &self.m_config,
                );
                let config = self.m_config.clone();
                let current_state = self.m_current_state.clone();
                let current_speed = self.m_current_speed.clone();
                let consumers = self.base.consumers_arc();
                let enabled = self.base.enabled_arc();
                can_source.set_on_output(Box::new(move |data| {
                    if let StreamableData::Can(ref can_data) = data {
                        let parser = ExternalCanParser::new(&config);
                        let mut state = current_state.lock().unwrap();
                        let mut speed = current_speed.lock().unwrap();
                        parser.parse(can_data, &mut state, &mut speed);
                        if enabled.load(Ordering::Relaxed) {
                            let cbs = consumers.lock().unwrap();
                            for cb in cbs.iter() {
                                cb(StreamableData::VehicleState(state.clone()));
                                cb(StreamableData::VehicleSpeed(speed.clone()));
                            }
                        }
                    }
                }));
                can_source.start()?;
                self.m_can_source = Some(Box::new(can_source));
            }
            CanInterface::Vector => {
                log::info!(
                    "AutomotiveVehicleSource '{}': creating internal Vector CAN source",
                    self.base.name()
                );
                let mut can_source = VectorCanSource::new(
                    format!("{}_vector_can", self.base.name()),
                    &self.m_config,
                );
                let config = self.m_config.clone();
                let current_state = self.m_current_state.clone();
                let current_speed = self.m_current_speed.clone();
                let consumers = self.base.consumers_arc();
                let enabled = self.base.enabled_arc();
                can_source.set_on_output(Box::new(move |data| {
                    if let StreamableData::Can(ref can_data) = data {
                        let parser = ExternalCanParser::new(&config);
                        let mut state = current_state.lock().unwrap();
                        let mut speed = current_speed.lock().unwrap();
                        parser.parse(can_data, &mut state, &mut speed);
                        if enabled.load(Ordering::Relaxed) {
                            let cbs = consumers.lock().unwrap();
                            for cb in cbs.iter() {
                                cb(StreamableData::VehicleState(state.clone()));
                                cb(StreamableData::VehicleSpeed(speed.clone()));
                            }
                        }
                    }
                }));
                can_source.start()?;
                self.m_can_source = Some(Box::new(can_source));
            }
            CanInterface::None => {
                log::warn!("No CAN interface configured");
            }
        }

        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        log::info!("Stopping automotive vehicle source: {}", self.base.name());
        self.m_done.store(true, Ordering::Relaxed);
        if let Some(ref mut can_source) = self.m_can_source {
            can_source.stop()?;
        }
        self.m_can_source = None;
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
