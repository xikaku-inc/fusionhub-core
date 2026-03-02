use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use tokio::task::JoinHandle;

use fusion_types::{StreamableData, VehicleSpeed, VelocityMeterData};

use crate::node::{ConsumerCallback, Node, NodeBase};
use crate::sources::act2535_data_format::parse_act2535_line;

const STX: u8 = 0x02;
const ETX: u8 = 0x03;

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Initializing,
    Ready,
    Error,
    Stopped,
}

/// ACT2535 velocity meter source node.
///
/// Reads ACT2535 data from a serial port using STX/ETX framing for commands
/// and CR/LF-delimited CSV lines for measurement data.
pub struct Act2535Source {
    pub base: NodeBase,
    m_port: String,
    m_baudrate: u32,
    m_hw_flow_control: bool,
    m_state: Arc<Mutex<State>>,
    m_done: Arc<AtomicBool>,
    m_velocity_meter_data: Arc<Mutex<VelocityMeterData>>,
    m_worker_handle: Option<JoinHandle<()>>,
    m_last_error_time: Arc<Mutex<Instant>>,
}

impl Act2535Source {
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
        let hw_flow_control = config
            .get("hwFlowControl")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let heartbeatrate = config
            .get("heartbeatrate")
            .and_then(|v| v.as_u64())
            .unwrap_or(1000);

        let mut base = NodeBase::new(&name);
        base.set_heartbeat_interval(Duration::from_millis(heartbeatrate));

        Self {
            base,
            m_port: port,
            m_baudrate: baudrate,
            m_hw_flow_control: hw_flow_control,
            m_state: Arc::new(Mutex::new(State::Initializing)),
            m_done: Arc::new(AtomicBool::new(false)),
            m_velocity_meter_data: Arc::new(Mutex::new(VelocityMeterData::default())),
            m_worker_handle: None,
            m_last_error_time: Arc::new(Mutex::new(Instant::now())),
        }
    }

    /// Send a framed command (STX + cmd + ETX) and optionally read a response.
    fn send_command(
        port: &mut Box<dyn serialport::SerialPort>,
        cmd: &str,
        expect_response: bool,
    ) -> anyhow::Result<Option<String>> {
        let frame = format!("{}{}{}", STX as char, cmd, ETX as char);
        port.write_all(frame.as_bytes())?;
        port.flush()?;

        if !expect_response {
            return Ok(None);
        }

        let mut buf = vec![0u8; 1024];
        let timeout = Duration::from_millis(500);
        port.set_timeout(timeout)?;

        let mut response = Vec::new();
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            match port.read(&mut buf) {
                Ok(n) => {
                    response.extend_from_slice(&buf[..n]);
                    if response.contains(&ETX) {
                        break;
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => break,
                Err(e) => return Err(e.into()),
            }
        }

        Ok(Some(String::from_utf8_lossy(&response).to_string()))
    }

    fn initialize(port: &mut Box<dyn serialport::SerialPort>) -> anyhow::Result<()> {
        Self::send_command(port, "STOP", false)?;
        std::thread::sleep(Duration::from_millis(100));
        Self::send_command(port, "VER", true)?;
        Ok(())
    }

    fn set_error(state: &Arc<Mutex<State>>, last_error: &Arc<Mutex<Instant>>, msg: &str) {
        log::warn!("Velocity Meter error: {}", msg);
        *state.lock().unwrap() = State::Error;
        *last_error.lock().unwrap() = Instant::now();
    }
}

impl Node for Act2535Source {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn start(&mut self) -> anyhow::Result<()> {
        log::info!(
            "Connecting to Velocity Meter on port {} baudrate {} hwFlowControl={}",
            self.m_port,
            self.m_baudrate,
            self.m_hw_flow_control
        );

        let port_name = self.m_port.clone();
        let baudrate = self.m_baudrate;
        let hw_flow = self.m_hw_flow_control;
        let done = self.m_done.clone();
        let state = self.m_state.clone();
        let vmd = self.m_velocity_meter_data.clone();
        let last_error = self.m_last_error_time.clone();
        let node_name = self.base.name().to_string();
        let consumers = self.base.consumers_arc();
        let enabled = self.base.enabled_arc();

        self.m_worker_handle = Some(tokio::spawn(async move {
            let result = tokio::task::spawn_blocking(move || {
                let flow_control = if hw_flow {
                    serialport::FlowControl::Hardware
                } else {
                    serialport::FlowControl::None
                };

                let port_result = serialport::new(&port_name, baudrate)
                    .data_bits(serialport::DataBits::Eight)
                    .stop_bits(serialport::StopBits::One)
                    .parity(serialport::Parity::None)
                    .flow_control(flow_control)
                    .timeout(Duration::from_millis(100))
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

                std::thread::sleep(Duration::from_secs(1));
                if done.load(Ordering::Relaxed) {
                    return;
                }

                if let Err(e) = Self::initialize(&mut port) {
                    Self::set_error(&state, &last_error, &format!("Init failed: {}", e));
                    return;
                }

                if done.load(Ordering::Relaxed) {
                    return;
                }

                if let Err(e) = Self::send_command(&mut port, "START0", false) {
                    Self::set_error(&state, &last_error, &format!("Start command failed: {}", e));
                    return;
                }

                *state.lock().unwrap() = State::Ready;
                log::info!("Velocity Meter initialization complete");

                let mut line_buf = String::new();
                let mut read_buf = [0u8; 1024];

                while !done.load(Ordering::Relaxed) {
                    match port.read(&mut read_buf) {
                        Ok(n) => {
                            let chunk = String::from_utf8_lossy(&read_buf[..n]);
                            line_buf.push_str(&chunk);

                            while let Some(pos) = line_buf.find("\r\n") {
                                let line = line_buf[..pos].to_string();
                                line_buf = line_buf[pos + 2..].to_string();

                                if let Some(mut parsed) = parse_act2535_line(&line) {
                                    parsed.timestamp = SystemTime::now();
                                    if parsed.sender_id.is_empty() {
                                        parsed.sender_id = node_name.clone();
                                    }

                                    *vmd.lock().unwrap() = parsed.clone();

                                    if enabled.load(Ordering::Relaxed) {
                                        let cbs = consumers.lock().unwrap();
                                        let vm_data =
                                            StreamableData::VelocityMeter(parsed.clone());
                                        for cb in cbs.iter() {
                                            cb(vm_data.clone());
                                        }

                                        let vspeed = VehicleSpeed {
                                            sender_id: parsed.sender_id.clone(),
                                            timestamp: parsed.timestamp,
                                            linear: parsed.velocity,
                                            angular: 0.0,
                                            valid_angular: false,
                                        };
                                        let vs_data = StreamableData::VehicleSpeed(vspeed);
                                        for cb in cbs.iter() {
                                            cb(vs_data.clone());
                                        }
                                    }
                                }
                            }
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => continue,
                        Err(e) => {
                            Self::set_error(
                                &state,
                                &last_error,
                                &format!("Read error: {}", e),
                            );
                            break;
                        }
                    }
                }
            })
            .await;

            if let Err(e) = result {
                log::warn!("ACT2535 worker thread panicked: {}", e);
            }
        }));

        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        log::info!("Stopping ACT2535 source: {}", self.base.name());
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
