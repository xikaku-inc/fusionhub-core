use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use tokio::task::JoinHandle;

use fusion_registry::{sf, SettingsField};
use fusion_types::{StreamableData, VehicleSpeed};
use serde_json::json;

use crate::node::{ConsumerCallback, Node, NodeBase};

pub fn settings_schema() -> Vec<SettingsField> {
    vec![
        sf("port", "Serial Port", "string", json!("COM10")),
        sf("baudrate", "Baud Rate", "number", json!(115200)),
        sf("isPorsche", "Porsche Mode", "boolean", json!(false)),
    ]
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Initializing,
    Ready,
    Error,
    Stopped,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GearState {
    Unknown,
    Park,
    Reverse,
    Neutral,
    Drive,
    Manual,
}

const RECOVERY_INTERVAL: Duration = Duration::from_secs(15);

/// ELM327 initialization commands: (name, command, expected_reply, timeout_ms)
const ELM327_INIT_COMMANDS: &[(&str, &str, &str, u64)] = &[
    ("reset", "ATZ", "ELM", 3000),
    ("echoOff", "ATE0", "OK", 1000),
    ("linefeedsOff", "ATL0", "OK", 1000),
    ("spacesOff", "ATS0", "OK", 1000),
    ("headersOff", "ATH0", "OK", 1000),
    ("setProtocol", "ATSP0", "OK", 1000),
    ("setECUHeader", "AT SH 7E0", "OK", 1000),
];

/// OBD-II commands: (command, expected_prefix)
const CMD_GET_VELOCITY: (&str, &str) = ("010D", "410D");
const CMD_GET_GEAR_STATE: (&str, &str) = ("01A5", "41A5");

/// OBD-II ELM327 serial source for vehicle speed and gear data.
///
/// Communicates with an ELM327 OBD-II adapter over serial to read vehicle speed.
/// Supports automatic error recovery and reconnection.
pub struct Elm327Source {
    pub base: NodeBase,
    m_port: String,
    m_baudrate: u32,
    m_is_porsche: bool,
    m_state: Arc<Mutex<State>>,
    m_done: Arc<AtomicBool>,
    m_velocity: Arc<Mutex<f64>>,
    m_current_gear: Arc<Mutex<GearState>>,
    m_last_error_time: Arc<Mutex<Instant>>,
    m_worker_handle: Option<JoinHandle<()>>,
}

impl Elm327Source {
    pub fn new(name: impl Into<String>, config: &serde_json::Value) -> Self {
        let name = name.into();
        let port = config
            .get("port")
            .and_then(|v| v.as_str())
            .unwrap_or("COM10")
            .to_string();
        let baudrate = config
            .get("baudrate")
            .and_then(|v| v.as_u64())
            .unwrap_or(115200) as u32;
        let is_porsche = config
            .get("isPorsche")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let mut base = NodeBase::new(&name);
        base.set_heartbeat_interval(Duration::from_millis(100));

        Self {
            base,
            m_port: port,
            m_baudrate: baudrate,
            m_is_porsche: is_porsche,
            m_state: Arc::new(Mutex::new(State::Initializing)),
            m_done: Arc::new(AtomicBool::new(false)),
            m_velocity: Arc::new(Mutex::new(0.0)),
            m_current_gear: Arc::new(Mutex::new(GearState::Unknown)),
            m_last_error_time: Arc::new(Mutex::new(Instant::now())),
            m_worker_handle: None,
        }
    }

    fn send_command(
        port: &mut Box<dyn serialport::SerialPort>,
        cmd: &str,
        timeout: Duration,
    ) -> anyhow::Result<String> {
        let full_cmd = format!("{}\r\n", cmd);
        port.write_all(full_cmd.as_bytes())?;
        port.flush()?;

        port.set_timeout(timeout)?;

        let mut response = Vec::new();
        let mut buf = [0u8; 256];
        let deadline = Instant::now() + timeout;

        while Instant::now() < deadline {
            match port.read(&mut buf) {
                Ok(n) => {
                    response.extend_from_slice(&buf[..n]);
                    if response.contains(&b'>') {
                        break;
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => {
                    if response.contains(&b'>') {
                        break;
                    }
                }
                Err(e) => return Err(e.into()),
            }
        }

        let mut result = String::from_utf8_lossy(&response).to_string();

        // Clean up response
        result.retain(|c| c != '\r' && c != '\n' && c != '\t' && c != '>');

        // Remove status messages
        for status in &["SEARCHING...", "BUS INIT: ...OK", "BUS INIT: OK"] {
            if let Some(pos) = result.find(status) {
                result.replace_range(pos..pos + status.len(), "");
            }
        }

        Ok(result)
    }

    fn initialize(
        port: &mut Box<dyn serialport::SerialPort>,
        is_porsche: bool,
    ) -> anyhow::Result<()> {
        for &(name, cmd, expected, timeout_ms) in ELM327_INIT_COMMANDS {
            if name == "setECUHeader" && !is_porsche {
                log::info!("isPorsche is false, not sending ECU header");
                continue;
            }

            let response = Self::send_command(port, cmd, Duration::from_millis(timeout_ms))?;

            if !response.contains(expected) {
                log::warn!(
                    "Warning: Unexpected response. Expected '{}', got '{}'",
                    expected,
                    response
                );
            }

            std::thread::sleep(Duration::from_millis(100));
        }

        Ok(())
    }

    fn parse_gear_state(response: &str) -> GearState {
        if response.len() >= 8 {
            let gear_byte = &response[6..8];
            if let Ok(gear_value) = i32::from_str_radix(gear_byte, 16) {
                return match gear_value {
                    0x00 => GearState::Park,
                    0x01 => GearState::Reverse,
                    0x02 => GearState::Neutral,
                    0x03 => GearState::Drive,
                    0x04 => GearState::Manual,
                    _ => GearState::Unknown,
                };
            }
        }
        GearState::Unknown
    }

    fn set_error(state: &Mutex<State>, last_error: &Mutex<Instant>, msg: &str) {
        log::warn!("ELM327 error: {}", msg);
        *state.lock().unwrap() = State::Error;
        *last_error.lock().unwrap() = Instant::now();
    }
}

impl Node for Elm327Source {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn start(&mut self) -> anyhow::Result<()> {
        log::info!(
            "Starting ELM327 source on port {} baudrate {}",
            self.m_port,
            self.m_baudrate
        );

        let port_name = self.m_port.clone();
        let baudrate = self.m_baudrate;
        let is_porsche = self.m_is_porsche;
        let done = self.m_done.clone();
        let state = self.m_state.clone();
        let velocity = self.m_velocity.clone();
        let current_gear = self.m_current_gear.clone();
        let last_error = self.m_last_error_time.clone();
        let consumers = self.base.consumers_arc();
        let enabled = self.base.enabled_arc();
        let node_name = self.base.name().to_string();

        self.m_worker_handle = Some(tokio::spawn(async move {
            let result = tokio::task::spawn_blocking(move || {
                let port_result = serialport::new(&port_name, baudrate)
                    .data_bits(serialport::DataBits::Eight)
                    .stop_bits(serialport::StopBits::One)
                    .parity(serialport::Parity::None)
                    .flow_control(serialport::FlowControl::Hardware)
                    .timeout(Duration::from_secs(5))
                    .open();

                let mut port = match port_result {
                    Ok(p) => p,
                    Err(e) => {
                        Self::set_error(
                            &state,
                            &last_error,
                            &format!("Failed to open port {}: {}", port_name, e),
                        );
                        return;
                    }
                };

                log::info!(
                    "Connecting to ELM327 on port {} with hardware flow control",
                    port_name
                );
                std::thread::sleep(Duration::from_secs(1));

                if let Err(e) = Self::initialize(&mut port, is_porsche) {
                    Self::set_error(&state, &last_error, &format!("Init failed: {}", e));
                    return;
                }

                *state.lock().unwrap() = State::Ready;
                log::info!("ELM327 initialization complete");

                // Polling loop: read velocity at heartbeat rate
                while !done.load(Ordering::Relaxed) {
                    let current_state = *state.lock().unwrap();

                    match current_state {
                        State::Ready => {
                            let response = match Self::send_command(
                                &mut port,
                                CMD_GET_VELOCITY.0,
                                Duration::from_secs(5),
                            ) {
                                Ok(r) => r,
                                Err(e) => {
                                    Self::set_error(
                                        &state,
                                        &last_error,
                                        &format!("Velocity read failed: {}", e),
                                    );
                                    continue;
                                }
                            };

                            if !response.starts_with(CMD_GET_VELOCITY.1) {
                                Self::set_error(
                                    &state,
                                    &last_error,
                                    &format!("Unexpected velocity response: {}", response),
                                );
                                continue;
                            }

                            if response.len() >= 6 {
                                let hex_speed = &response[4..6];
                                if let Ok(speed_kmh) = i32::from_str_radix(hex_speed, 16) {
                                    let abs_velocity = speed_kmh as f64 / 3.6;
                                    let gear = *current_gear.lock().unwrap();
                                    let vel = if gear == GearState::Reverse {
                                        -abs_velocity
                                    } else {
                                        abs_velocity
                                    };

                                    *velocity.lock().unwrap() = vel;

                                    if enabled.load(Ordering::Relaxed) {
                                        let vspeed = VehicleSpeed {
                                            sender_id: node_name.clone(),
                                            timestamp: SystemTime::now(),
                                            linear: vel,
                                            angular: 0.0,
                                            valid_angular: false,
                                        };
                                        let data = StreamableData::VehicleSpeed(vspeed);
                                        let cbs = consumers.lock().unwrap();
                                        for cb in cbs.iter() {
                                            cb(data.clone());
                                        }
                                    }
                                }
                            }
                        }
                        State::Error => {
                            let elapsed = last_error.lock().unwrap().elapsed();
                            if elapsed >= RECOVERY_INTERVAL {
                                log::info!("Attempting to recover ELM327 connection...");

                                // Close and reopen port
                                let port_result = serialport::new(&port_name, baudrate)
                                    .data_bits(serialport::DataBits::Eight)
                                    .stop_bits(serialport::StopBits::One)
                                    .parity(serialport::Parity::None)
                                    .flow_control(serialport::FlowControl::Hardware)
                                    .timeout(Duration::from_secs(5))
                                    .open();

                                port = match port_result {
                                    Ok(p) => p,
                                    Err(e) => {
                                        Self::set_error(
                                            &state,
                                            &last_error,
                                            &format!("Recovery failed: {}", e),
                                        );
                                        std::thread::sleep(Duration::from_secs(1));
                                        continue;
                                    }
                                };

                                std::thread::sleep(Duration::from_secs(1));

                                if let Err(e) = Self::initialize(&mut port, is_porsche) {
                                    Self::set_error(
                                        &state,
                                        &last_error,
                                        &format!("Recovery init failed: {}", e),
                                    );
                                    continue;
                                }

                                *state.lock().unwrap() = State::Ready;
                                log::info!("ELM327 recovery successful");
                            } else {
                                std::thread::sleep(Duration::from_millis(100));
                            }
                        }
                        _ => {
                            std::thread::sleep(Duration::from_millis(100));
                        }
                    }

                    std::thread::sleep(Duration::from_millis(100));
                }
            })
            .await;

            if let Err(e) = result {
                log::warn!("ELM327 worker thread panicked: {}", e);
            }
        }));

        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        log::info!("Stopping ELM327 source: {}", self.base.name());
        self.m_done.store(true, Ordering::Relaxed);

        if let Some(handle) = self.m_worker_handle.take() {
            handle.abort();
        }

        *self.m_state.lock().unwrap() = State::Stopped;
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
